//! Execution of deferred statements and the per-migration deferral state.

use pg_query::NodeEnum;
use tokio_postgres::GenericClient;

use super::classify::classify;
use super::store::{DeferredRepository, Origin};
use super::{DeferPolicy, DeferredEntry, DeferredKind, DeferredStatus, FkMode, entries_to_flush};
use crate::SchemalaneError;
use crate::ident::quote_ident;
use crate::observer::MigrationInfo;
use crate::sql_analysis::ParsedSqlStatement;

/// How one entry is executed.
#[derive(Debug, Clone, Copy)]
pub(crate) struct EntryRun {
    pub(crate) concurrently: bool,
    pub(crate) fk_mode: FkMode,
}

impl EntryRun {
    /// Inside a migration: one statement, no extra lock choreography.
    pub(crate) const IN_PLACE: Self = Self {
        concurrently: false,
        fk_mode: FkMode::Direct,
    };
}

/// What to do with one statement of a migration.
pub(crate) enum Prepared {
    /// Execute this SQL now.
    Run(String),
    /// The whole statement was moved into the deferred set.
    Deferred,
}

/// Deferral state of one `up_deferred` run.
pub(crate) struct Deferral<'a> {
    pub(crate) policy: DeferPolicy,
    pub(crate) repository: &'a DeferredRepository,
    pub(crate) installed_by: &'a str,
    pub(crate) pending: Vec<DeferredEntry>,
}

impl Deferral<'_> {
    pub(crate) async fn prepare(
        &mut self,
        client: &impl GenericClient,
        migration: &MigrationInfo,
        statement_index: usize,
        statement: &ParsedSqlStatement,
    ) -> Result<Prepared, SchemalaneError> {
        if let Some(node) = statement.node.as_ref().and_then(|node| node.node.as_ref()) {
            let due: Vec<DeferredEntry> = entries_to_flush(node, &self.pending)
                .into_iter()
                .cloned()
                .collect();
            for entry in &due {
                let started = std::time::Instant::now();
                let resolved = execute_entry(client, entry, EntryRun::IN_PLACE).await?;
                let elapsed = crate::execute::millis_i32(started.elapsed().as_millis());
                self.repository
                    .mark_applied(client, entry.seq, resolved.as_deref(), elapsed, true)
                    .await?;
                self.pending.retain(|pending| pending.seq != entry.seq);
            }
        }
        let classified = classify(statement, self.policy)?;
        if classified.deferred.is_empty() {
            return Ok(Prepared::Run(statement.sql.clone()));
        }
        let statement_ordinal = i32::try_from(statement_index + 1).unwrap_or(i32::MAX);
        for (item, entry) in classified.deferred.iter().enumerate() {
            let origin = Origin {
                version: &migration.version,
                script: &migration.script,
                statement_ordinal,
                item_ordinal: i32::try_from(item).unwrap_or(i32::MAX),
                installed_by: self.installed_by,
            };
            self.repository.insert(client, &origin, entry).await?;
        }
        self.pending = self
            .repository
            .load(client)
            .await?
            .into_iter()
            .filter(|entry| entry.status == DeferredStatus::Pending)
            .collect();
        Ok(classified.run.map_or(Prepared::Deferred, Prepared::Run))
    }
}

fn relation_expression(entry: &DeferredEntry) -> String {
    entry.relation_schema.as_ref().map_or_else(
        || quote_ident(&entry.relation_name),
        |schema| {
            format!(
                "{}.{}",
                quote_ident(schema),
                quote_ident(&entry.relation_name)
            )
        },
    )
}

fn parse_statement(sql: &str) -> Result<NodeEnum, SchemalaneError> {
    pg_query::parse(sql, 0)
        .map_err(|error| {
            SchemalaneError::Internal(format!("deferred statement does not parse: {error}"))
        })?
        .protobuf
        .stmts
        .into_iter()
        .next()
        .and_then(|raw| raw.stmt)
        .and_then(|node| node.node)
        .ok_or_else(|| SchemalaneError::Internal("deferred statement is empty".to_owned()))
}

fn deparse(node: &NodeEnum) -> Result<String, SchemalaneError> {
    node.deparse(pg_query::DeparseOptions::default())
        .map_err(|error| {
            SchemalaneError::Internal(format!("failed to deparse a deferred statement: {error}"))
        })
}

/// Returns `sql` with `CONCURRENTLY` set as requested; the original text when it already is.
pub(crate) fn index_sql(sql: &str, concurrently: bool) -> Result<String, SchemalaneError> {
    let NodeEnum::IndexStmt(mut index) = parse_statement(sql)? else {
        return Err(SchemalaneError::Internal(
            "deferred index entry is not CREATE INDEX".to_owned(),
        ));
    };
    if index.concurrent == concurrently {
        return Ok(sql.to_owned());
    }
    index.concurrent = concurrently;
    deparse(&NodeEnum::IndexStmt(index))
}

/// Returns the `NOT VALID` form of a foreign-key statement and whether it was already so.
pub(crate) fn fk_not_valid_sql(sql: &str) -> Result<(String, bool), SchemalaneError> {
    let NodeEnum::AlterTableStmt(mut alter) = parse_statement(sql)? else {
        return Err(SchemalaneError::Internal(
            "deferred foreign-key entry is not ALTER TABLE".to_owned(),
        ));
    };
    let mut was_not_valid = false;
    for cmd in &mut alter.cmds {
        let Some(NodeEnum::AlterTableCmd(cmd)) = cmd.node.as_mut() else {
            continue;
        };
        let Some(NodeEnum::Constraint(constraint)) =
            cmd.def.as_mut().and_then(|def| def.node.as_mut())
        else {
            continue;
        };
        was_not_valid |= constraint.skip_validation;
        constraint.skip_validation = true;
    }
    Ok((deparse(&NodeEnum::AlterTableStmt(alter))?, was_not_valid))
}

async fn newest_index_name(
    client: &impl GenericClient,
    relation: &str,
) -> Result<Option<String>, SchemalaneError> {
    let row = client
        .query_opt(
            "SELECT c.relname AS name FROM pg_index i JOIN pg_class c ON c.oid = i.indexrelid \
             WHERE i.indrelid = $1::text::regclass ORDER BY i.indexrelid DESC LIMIT 1",
            &[&relation],
        )
        .await?;
    Ok(row.map(|row| row.get("name")))
}

async fn newest_foreign_key_name(
    client: &impl GenericClient,
    relation: &str,
) -> Result<Option<String>, SchemalaneError> {
    let row = client
        .query_opt(
            "SELECT conname::text AS name FROM pg_constraint \
             WHERE conrelid = $1::text::regclass AND contype = 'f' ORDER BY oid DESC LIMIT 1",
            &[&relation],
        )
        .await?;
    Ok(row.map(|row| row.get("name")))
}

/// Executes one entry and returns the name of the object it created.
pub(crate) async fn execute_entry(
    client: &impl GenericClient,
    entry: &DeferredEntry,
    run: EntryRun,
) -> Result<Option<String>, SchemalaneError> {
    let relation = relation_expression(entry);
    if entry.kind.is_index() {
        client
            .batch_execute(&index_sql(&entry.sql, run.concurrently)?)
            .await?;
        return match &entry.object_name {
            Some(name) => Ok(Some(name.clone())),
            None => newest_index_name(client, &relation).await,
        };
    }
    debug_assert_eq!(entry.kind, DeferredKind::ForeignKey);
    match run.fk_mode {
        FkMode::Direct => {
            client.batch_execute(&entry.sql).await?;
            match &entry.object_name {
                Some(name) => Ok(Some(name.clone())),
                None => newest_foreign_key_name(client, &relation).await,
            }
        }
        FkMode::NotValidThenValidate => {
            let (sql, was_not_valid) = fk_not_valid_sql(&entry.sql)?;
            client.batch_execute(&sql).await?;
            let name = match &entry.object_name {
                Some(name) => Some(name.clone()),
                None => newest_foreign_key_name(client, &relation).await?,
            };
            if !was_not_valid && let Some(name) = &name {
                client
                    .batch_execute(&format!(
                        "ALTER TABLE {relation} VALIDATE CONSTRAINT {}",
                        quote_ident(name)
                    ))
                    .await?;
            }
            Ok(name)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_sql_toggles_concurrently_and_keeps_matching_text() {
        let plain = "CREATE INDEX idx ON t (a)";
        assert_eq!(index_sql(plain, false).expect("sql"), plain);
        let concurrent = index_sql(plain, true).expect("sql");
        assert!(concurrent.contains("CONCURRENTLY"), "{concurrent}");
        let back = index_sql(&concurrent, false).expect("sql");
        assert!(!back.contains("CONCURRENTLY"), "{back}");
    }

    #[test]
    fn fk_not_valid_sql_marks_the_constraint() {
        let sql = "ALTER TABLE c ADD CONSTRAINT fk FOREIGN KEY (p) REFERENCES t (id)";
        let (not_valid, was) = fk_not_valid_sql(sql).expect("sql");
        assert!(!was);
        assert!(not_valid.contains("NOT VALID"), "{not_valid}");
        let (_, was) = fk_not_valid_sql(&not_valid).expect("sql");
        assert!(was);
    }
}
