//! The sidecar table that records deferred statements.

use tokio_postgres::{GenericClient, Row};

use super::classify::NewEntry;
use super::{DeferredEntry, DeferredKind, DeferredStatus};
use crate::SchemalaneError;
use crate::ident::{qualified_table, quote_ident};

/// Owns every SQL statement touching the deferred-statement table.
pub(crate) struct DeferredRepository {
    qualified: String,
    table: String,
}

impl DeferredRepository {
    const COLUMNS: &'static str = "\"seq\", \"version\", \"script\", \"statement_ordinal\", \"item_ordinal\", \"kind\", \"relation_schema\", \"relation_name\", \"referenced_relation\", \"object_name\", \"resolved_name\", \"sql\", \"status\", \"applied_on\"::text AS \"applied_on\", \"execution_time\", \"flushed\", \"idents\"";

    pub(crate) fn new(schema: &str, history_table: &str) -> Self {
        let table = format!("{history_table}_deferred");
        Self {
            qualified: qualified_table(schema, &table),
            table,
        }
    }

    pub(crate) async fn ensure_table(
        &self,
        client: &impl GenericClient,
    ) -> Result<(), SchemalaneError> {
        let ddl = format!(
            "\
CREATE TABLE IF NOT EXISTS {table} (\
\"seq\" BIGINT GENERATED ALWAYS AS IDENTITY,\
\"version\" VARCHAR(50) NOT NULL,\
\"script\" VARCHAR(1000) NOT NULL,\
\"statement_ordinal\" INTEGER NOT NULL,\
\"item_ordinal\" INTEGER NOT NULL,\
\"kind\" VARCHAR(20) NOT NULL,\
\"relation_schema\" VARCHAR(200),\
\"relation_name\" VARCHAR(200) NOT NULL,\
\"referenced_relation\" VARCHAR(200),\
\"object_name\" VARCHAR(200),\
\"resolved_name\" VARCHAR(200),\
\"sql\" TEXT NOT NULL,\
\"status\" VARCHAR(10) NOT NULL DEFAULT 'pending',\
\"installed_by\" VARCHAR(100) NOT NULL,\
\"created_on\" TIMESTAMP NOT NULL DEFAULT now(),\
\"applied_on\" TIMESTAMP,\
\"execution_time\" INTEGER,\
\"flushed\" BOOLEAN NOT NULL DEFAULT FALSE,\
\"idents\" TEXT[] NOT NULL DEFAULT '{}',\
CONSTRAINT {pk} PRIMARY KEY (\"seq\"),\
CONSTRAINT {origin} UNIQUE (\"script\", \"statement_ordinal\", \"item_ordinal\")\
)",
            table = self.qualified,
            pk = quote_ident(&format!("{}_pk", self.table)),
            origin = quote_ident(&format!("{}_origin_uq", self.table)),
        );
        client.batch_execute(&ddl).await?;
        Ok(())
    }

    pub(crate) async fn exists(
        &self,
        client: &impl GenericClient,
    ) -> Result<bool, SchemalaneError> {
        let row = client
            .query_one(
                "SELECT to_regclass($1::text) IS NOT NULL AS exists",
                &[&self.qualified],
            )
            .await?;
        Ok(row.get("exists"))
    }

    /// Loads every entry in execution order; an absent table means no entries.
    pub(crate) async fn load(
        &self,
        client: &impl GenericClient,
    ) -> Result<Vec<DeferredEntry>, SchemalaneError> {
        if !self.exists(client).await? {
            return Ok(Vec::new());
        }
        let query = format!(
            "SELECT {} FROM {} ORDER BY \"seq\" ASC",
            Self::COLUMNS,
            self.qualified
        );
        let rows = client.query(&query, &[]).await?;
        rows.iter().map(entry_from_row).collect()
    }

    pub(crate) async fn count_pending(
        &self,
        client: &impl GenericClient,
    ) -> Result<usize, SchemalaneError> {
        if !self.exists(client).await? {
            return Ok(0);
        }
        let query = format!(
            "SELECT count(*) AS pending FROM {} WHERE \"status\" = 'pending'",
            self.qualified
        );
        let count: i64 = client.query_one(&query, &[]).await?.get("pending");
        Ok(usize::try_from(count).unwrap_or(usize::MAX))
    }

    /// Records an entry once per origin; a retried migration does not duplicate it.
    pub(crate) async fn insert(
        &self,
        client: &impl GenericClient,
        origin: &Origin<'_>,
        entry: &NewEntry,
    ) -> Result<(), SchemalaneError> {
        let sql = format!(
            "INSERT INTO {} (\"version\", \"script\", \"statement_ordinal\", \"item_ordinal\", \"kind\", \"relation_schema\", \"relation_name\", \"referenced_relation\", \"object_name\", \"sql\", \"installed_by\", \"idents\") VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12) ON CONFLICT (\"script\", \"statement_ordinal\", \"item_ordinal\") DO NOTHING",
            self.qualified
        );
        let kind = entry.kind.as_str();
        client
            .execute(
                &sql,
                &[
                    &origin.version,
                    &origin.script,
                    &origin.statement_ordinal,
                    &origin.item_ordinal,
                    &kind,
                    &entry.relation_schema,
                    &entry.relation_name,
                    &entry.referenced_relation,
                    &entry.object_name,
                    &entry.sql,
                    &origin.installed_by,
                    &entry.idents,
                ],
            )
            .await?;
        Ok(())
    }

    pub(crate) async fn mark_applied(
        &self,
        client: &impl GenericClient,
        seq: i64,
        resolved_name: Option<&str>,
        execution_time_ms: i32,
        flushed: bool,
    ) -> Result<(), SchemalaneError> {
        let sql = format!(
            "UPDATE {} SET \"status\" = 'applied', \"applied_on\" = now(), \"resolved_name\" = $2, \"execution_time\" = $3, \"flushed\" = $4 WHERE \"seq\" = $1",
            self.qualified
        );
        client
            .execute(&sql, &[&seq, &resolved_name, &execution_time_ms, &flushed])
            .await?;
        Ok(())
    }
}

/// Where a deferred statement came from.
pub(crate) struct Origin<'a> {
    pub(crate) version: &'a str,
    pub(crate) script: &'a str,
    pub(crate) statement_ordinal: i32,
    pub(crate) item_ordinal: i32,
    pub(crate) installed_by: &'a str,
}

fn entry_from_row(row: &Row) -> Result<DeferredEntry, SchemalaneError> {
    let kind: String = row.get("kind");
    let status: String = row.get("status");
    Ok(DeferredEntry {
        seq: row.get("seq"),
        version: row.get("version"),
        script: row.get("script"),
        statement_ordinal: row.get("statement_ordinal"),
        item_ordinal: row.get("item_ordinal"),
        kind: DeferredKind::parse(&kind)?,
        relation_schema: row.get("relation_schema"),
        relation_name: row.get("relation_name"),
        referenced_relation: row.get("referenced_relation"),
        object_name: row.get("object_name"),
        resolved_name: row.get("resolved_name"),
        sql: row.get("sql"),
        status: DeferredStatus::parse(&status)?,
        applied_on: row.get("applied_on"),
        execution_time_ms: row.get("execution_time"),
        flushed: row.get("flushed"),
        idents: row.get("idents"),
    })
}
