//! Public methods of [`SchemalaneMigrator`] for deferred objects.

use deadpool_postgres::Pool;
use futures_util::StreamExt;
use std::time::Instant;
use tokio_postgres::{Client, GenericClient};

use super::run::{EntryRun, execute_entry};
use super::{
    ApplyDeferredOptions, ApplyDeferredReport, DeferPolicy, DeferredEntry, DeferredReport,
    DeferredRepository, DeferredStatus, DeferredVerification, VerifiedEntry,
};
use crate::execute::millis_i32;
use crate::{
    MigrationObserver, NoopMigrationObserver, RunReport, SchemalaneError, SchemalaneMigrator,
};

impl SchemalaneMigrator {
    pub(crate) fn deferred_repository(&self) -> DeferredRepository {
        DeferredRepository::new(&self.config.schema, &self.config.history_table)
    }

    /// Applies pending migrations like [`up`](Self::up), recording statements that match
    /// `policy` (secondary indexes, foreign keys) instead of executing them.
    ///
    /// The history rows and checksums are exactly those of a plain `up`. Run
    /// [`apply_deferred`](Self::apply_deferred) after the bulk load to create the objects.
    /// A later statement that drops, renames or otherwise depends on a deferred object makes
    /// the migrator execute that object first, so the final schema equals a plain `up`.
    pub async fn up_deferred(
        &self,
        pool: &Pool,
        policy: &DeferPolicy,
    ) -> Result<RunReport, SchemalaneError> {
        self.up_deferred_with_observer(pool, policy, &NoopMigrationObserver)
            .await
    }

    /// [`up_deferred`](Self::up_deferred) with lifecycle events, including
    /// [`MigrationObserver::on_sql_statement_deferred`].
    pub async fn up_deferred_with_observer<O>(
        &self,
        pool: &Pool,
        policy: &DeferPolicy,
        observer: &O,
    ) -> Result<RunReport, SchemalaneError>
    where
        O: MigrationObserver + ?Sized,
    {
        self.run_up(pool, observer, Some(policy)).await
    }

    /// Recreates the schema like [`fresh`](Self::fresh), deferring statements that match
    /// `policy`. Destructive; requires `confirmed == true`.
    pub async fn fresh_deferred(
        &self,
        pool: &Pool,
        confirmed: bool,
        policy: &DeferPolicy,
    ) -> Result<RunReport, SchemalaneError> {
        self.fresh_deferred_with_observer(pool, confirmed, policy, &NoopMigrationObserver)
            .await
    }

    /// [`fresh_deferred`](Self::fresh_deferred) with lifecycle events.
    pub async fn fresh_deferred_with_observer<O>(
        &self,
        pool: &Pool,
        confirmed: bool,
        policy: &DeferPolicy,
        observer: &O,
    ) -> Result<RunReport, SchemalaneError>
    where
        O: MigrationObserver + ?Sized,
    {
        self.run_fresh(pool, confirmed, observer, Some(policy))
            .await
    }

    /// Lists the deferred set without changing the database.
    pub async fn deferred_status(&self, pool: &Pool) -> Result<DeferredReport, SchemalaneError> {
        let client = pool.get().await?;
        let entries = self.deferred_repository().load(&**client).await?;
        let pending = entries
            .iter()
            .filter(|entry| entry.status == DeferredStatus::Pending)
            .count();
        Ok(DeferredReport {
            schema: self.config.schema.clone(),
            table: format!("{}_deferred", self.config.history_table),
            pending,
            applied: entries.len() - pending,
            entries,
        })
    }

    /// Executes every pending deferred statement: indexes first, then foreign keys.
    ///
    /// Holds the migration advisory lock. Each entry is marked applied as soon as it
    /// succeeds, so a failed call can be repeated and resumes with the remaining entries.
    /// Indexes are built `options.parallelism` at a time; foreign keys run one by one.
    pub async fn apply_deferred(
        &self,
        pool: &Pool,
        options: &ApplyDeferredOptions,
    ) -> Result<ApplyDeferredReport, SchemalaneError> {
        let repository = self.deferred_repository();
        let (session, lock_id) = self.acquire_locked_session(pool).await?;
        let result = async {
            let client: &Client = &session;
            self.set_search_path(client).await?;
            apply_session_settings(client, &options.session_settings).await?;
            let entries = repository.load(client).await?;
            let already_applied = entries
                .iter()
                .filter(|entry| entry.status == DeferredStatus::Applied)
                .count();
            let (indexes, foreign_keys): (Vec<_>, Vec<_>) = entries
                .into_iter()
                .filter(|entry| entry.status == DeferredStatus::Pending)
                .partition(|entry| entry.kind.is_index());
            let run = EntryRun {
                concurrently: options.concurrently,
                fk_mode: options.fk_mode,
            };
            let mut applied = self
                .apply_indexes(pool, client, &repository, indexes, options, run)
                .await?;
            for entry in foreign_keys {
                applied.push(apply_entry(client, &repository, entry, run).await?);
            }
            Ok(ApplyDeferredReport {
                applied,
                already_applied,
            })
        }
        .await;
        Self::finish_locked_session(&session, lock_id, result).await
    }

    async fn apply_indexes(
        &self,
        pool: &Pool,
        session: &Client,
        repository: &DeferredRepository,
        indexes: Vec<DeferredEntry>,
        options: &ApplyDeferredOptions,
        run: EntryRun,
    ) -> Result<Vec<DeferredEntry>, SchemalaneError> {
        let workers = options.parallelism.min(pool.status().max_size).max(1);
        if workers == 1 || indexes.len() < 2 {
            let mut applied = Vec::with_capacity(indexes.len());
            for entry in indexes {
                applied.push(apply_entry(session, repository, entry, run).await?);
            }
            return Ok(applied);
        }
        let results: Vec<Result<DeferredEntry, SchemalaneError>> =
            futures_util::stream::iter(indexes)
                .map(|entry| async move {
                    // The connection is detached: it carries session settings and must not
                    // return to the caller's pool.
                    let connection = deadpool_postgres::Object::take(pool.get().await?);
                    self.set_search_path(&connection).await?;
                    apply_session_settings(&connection, &options.session_settings).await?;
                    apply_entry(&connection, repository, entry, run).await
                })
                .buffer_unordered(workers)
                .collect()
                .await;
        let mut applied = Vec::with_capacity(results.len());
        let mut first_error = None;
        for result in results {
            match result {
                Ok(entry) => applied.push(entry),
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        if let Some(error) = first_error {
            return Err(error);
        }
        applied.sort_by_key(|entry| entry.seq);
        Ok(applied)
    }

    /// Checks the deferred set against the catalogs: every applied entry must exist and be
    /// valid (`indisvalid`/`indisready` for indexes, `convalidated` for foreign keys).
    /// Pending entries make the result not `ok`. Entries a later migration forced into
    /// place, which that migration may have dropped again, are reported but not required.
    pub async fn verify_deferred(
        &self,
        pool: &Pool,
    ) -> Result<DeferredVerification, SchemalaneError> {
        let client = pool.get().await?;
        let entries = self.deferred_repository().load(&**client).await?;
        let mut verified = Vec::with_capacity(entries.len());
        let mut ok = true;
        for entry in entries {
            let (exists, valid) = if entry.status == DeferredStatus::Applied {
                self.check_catalog(&**client, &entry).await?
            } else {
                (false, false)
            };
            if !(entry.flushed || exists && valid) {
                ok = false;
            }
            verified.push(VerifiedEntry {
                entry,
                exists,
                valid,
            });
        }
        Ok(DeferredVerification {
            entries: verified,
            ok,
        })
    }

    async fn check_catalog(
        &self,
        client: &impl GenericClient,
        entry: &DeferredEntry,
    ) -> Result<(bool, bool), SchemalaneError> {
        let Some(name) = entry.created_name() else {
            return Ok((false, false));
        };
        let schema = entry
            .relation_schema
            .as_deref()
            .unwrap_or(&self.config.schema);
        let row = if entry.kind.is_index() {
            client
                .query_opt(
                    "SELECT (i.indisvalid AND i.indisready) AS valid FROM pg_index i \
                     JOIN pg_class c ON c.oid = i.indexrelid \
                     JOIN pg_namespace n ON n.oid = c.relnamespace \
                     WHERE n.nspname = $1 AND c.relname = $2",
                    &[&schema, &name],
                )
                .await?
        } else {
            client
                .query_opt(
                    "SELECT k.convalidated AS valid FROM pg_constraint k \
                     JOIN pg_class r ON r.oid = k.conrelid \
                     JOIN pg_namespace n ON n.oid = r.relnamespace \
                     WHERE n.nspname = $1 AND r.relname = $2 AND k.conname = $3 \
                     AND k.contype = 'f'",
                    &[&schema, &entry.relation_name, &name],
                )
                .await?
        };
        Ok(row.map_or((false, false), |row| (true, row.get("valid"))))
    }
}

async fn apply_entry(
    client: &Client,
    repository: &DeferredRepository,
    mut entry: DeferredEntry,
    run: EntryRun,
) -> Result<DeferredEntry, SchemalaneError> {
    let started = Instant::now();
    let resolved = execute_entry(client, &entry, run)
        .await
        .map_err(|error| match error {
            SchemalaneError::Db(source) => SchemalaneError::MigrationExecution {
                script: format!("{} (deferred statement {})", entry.script, entry.seq),
                source,
            },
            other => other,
        })?;
    let elapsed = millis_i32(started.elapsed().as_millis());
    repository
        .mark_applied(client, entry.seq, resolved.as_deref(), elapsed, false)
        .await?;
    entry.status = DeferredStatus::Applied;
    entry.resolved_name = resolved;
    entry.execution_time_ms = Some(elapsed);
    Ok(entry)
}

async fn apply_session_settings(
    client: &Client,
    settings: &[(String, String)],
) -> Result<(), SchemalaneError> {
    for (name, value) in settings {
        client
            .execute("SELECT set_config($1, $2, false)", &[name, value])
            .await?;
    }
    Ok(())
}
