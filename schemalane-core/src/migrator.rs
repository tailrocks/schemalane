use crc32fast::Hasher;
use deadpool_postgres::Pool;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Instant;
use tokio_postgres::Client;

use crate::discovery::{DiscoveredMigration, MigrationSource};

const ADVISORY_LOCK_NAMESPACE: i64 = 7_333_654_209_921_337;

/// Derives the stable, database-local advisory lock key for a migration target.
pub fn derive_advisory_lock_id(schema: &str, history_table: &str) -> i64 {
    let mut hasher = Hasher::new();
    hasher.update(schema.as_bytes());
    hasher.update(&[0]);
    hasher.update(history_table.as_bytes());
    let low = i64::from(hasher.finalize());
    (ADVISORY_LOCK_NAMESPACE & !0xFFFF_FFFFi64) | low
}

use crate::execute::{Applied, execute_rust_migration, execute_sql_migration, millis_i32};
use crate::history::latest_history_by_script;
use crate::history::{HistoryRepository, HistoryRow, HistoryWrite};
use crate::ident::quote_ident;
use crate::report::build_status_report;
use crate::sql_analysis::{SqlTransactionMode, parse_sql_migration, resolve_sql_transaction_mode};
use crate::{SchemalaneConfig, SchemalaneError};

use crate::{
    AppliedMigration, MigrationFailed, MigrationFinished, MigrationObserver, MigrationStarted,
    MigrationState, NoopMigrationObserver, PlannedMigration, PlannedTransactionMode, RunReport,
    StatusReport, UpPlan,
};

#[cfg(test)]
use crate::checksum::calculate_checksum;

#[cfg(test)]
use crate::discovery::MigrationType;

#[cfg(test)]
use crate::filename::parse_sql_filename;

#[cfg(test)]
use crate::init_migration_project;

fn normalize_script_key(script: String) -> String {
    Path::new(&script)
        .file_name()
        .and_then(|name| name.to_str())
        .map(ToOwned::to_owned)
        .unwrap_or(script)
}

use crate::RustMigrationExecutor;

/// Discovers, validates, and executes migrations for one configured target.
pub struct SchemalaneMigrator {
    pub(crate) config: SchemalaneConfig,
    pub(crate) rust_migrations: HashMap<String, RustMigrationExecutor>,
}

impl SchemalaneMigrator {
    /// Creates a migrator with no registered Rust migration executors.
    pub fn new(config: SchemalaneConfig) -> Self {
        Self {
            config,
            rust_migrations: HashMap::new(),
        }
    }

    /// Returns this migrator's immutable configuration.
    pub const fn config(&self) -> &SchemalaneConfig {
        &self.config
    }

    /// Registers the executable body for a discovered Rust migration script.
    pub fn register_rust_migration<S>(&mut self, script: S, migration: RustMigrationExecutor)
    where
        S: Into<String>,
    {
        self.rust_migrations
            .insert(normalize_script_key(script.into()), migration);
    }

    /// Transactional SQL migrations commit their history row atomically with the migration.
    /// Non-transactional SQL and Rust migrations record history after execution and therefore
    /// have at-least-once semantics; make those migrations idempotent. A pool size of one is
    /// sufficient because one detached connection owns the complete migration session.
    pub async fn up(&self, pool: &Pool) -> Result<RunReport, SchemalaneError> {
        self.up_with_observer(pool, &NoopMigrationObserver).await
    }

    /// Applies pending migrations while reporting lifecycle events.
    ///
    /// Transactional SQL and its history row commit atomically. Non-transactional
    /// SQL and Rust migrations have at-least-once semantics and should be idempotent.
    pub async fn up_with_observer<O>(
        &self,
        pool: &Pool,
        observer: &O,
    ) -> Result<RunReport, SchemalaneError>
    where
        O: MigrationObserver + ?Sized,
    {
        let migrations = self.discover_migrations()?;
        self.ensure_rust_executors_registered(&migrations)?;
        let (mut session, lock_id) = self.acquire_locked_session(pool).await?;
        let result = async {
            let client: &mut Client = &mut session;
            let history_repository = self.history_repository();
            self.ensure_target_schema(client).await?;
            self.set_search_path(client).await?;
            history_repository.ensure_table(client).await?;
            let installed_by = self.resolve_installed_by(client).await?;
            let history = history_repository.load(client).await?;
            Self::ensure_no_blocking_history(&migrations, &history)?;
            observer.on_run_planned(&build_status_report(
                &self.config.schema,
                &self.config.history_table,
                &migrations,
                &history,
            ));
            let latest = latest_history_by_script(&history);
            let applied_success: HashSet<String> = latest
                .values()
                .filter(|row| row.success)
                .map(|row| row.script.clone())
                .collect();
            let mut next_rank = history
                .iter()
                .map(|row| row.installed_rank)
                .max()
                .unwrap_or(0)
                + 1;

            self.apply_all(
                client,
                &migrations,
                &applied_success,
                &installed_by,
                &mut next_rank,
                &history_repository,
                observer,
                ApplyOptions { skip_applied: true },
            )
            .await
        }
        .await;
        Self::finish_locked_session(&session, lock_id, result).await
    }

    /// Compares discovered migrations with schema history without changing either.
    pub async fn status(&self, pool: &Pool) -> Result<StatusReport, SchemalaneError> {
        let client = pool.get().await?;
        let migrations = self.discover_migrations()?;
        let history_repository = self.history_repository();

        let history = if history_repository.exists(&client).await? {
            history_repository.load(&client).await?
        } else {
            Vec::new()
        };

        Ok(build_status_report(
            &self.config.schema,
            &self.config.history_table,
            &migrations,
            &history,
        ))
    }

    /// Validates local migrations against schema history without changing the database.
    ///
    /// Failed history returns exit condition 4; missing or checksum-mismatched
    /// migrations return exit condition 3. Pending migrations are valid.
    pub async fn validate(&self, pool: &Pool) -> Result<StatusReport, SchemalaneError> {
        let report = self.status(pool).await?;
        if report.summary.failed > 0 {
            let scripts = report
                .migrations
                .iter()
                .filter(|entry| entry.state == MigrationState::Failed)
                .map(|entry| entry.script.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(SchemalaneError::FailedHistory(scripts));
        }
        let mut drift = Vec::new();
        if report.summary.missing > 0 {
            drift.push(format!(
                "{} missing migration(s) in local crate",
                report.summary.missing
            ));
        }
        if report.summary.checksum_mismatch > 0 {
            drift.push(format!(
                "{} checksum mismatch(es)",
                report.summary.checksum_mismatch
            ));
        }
        if !drift.is_empty() {
            return Err(SchemalaneError::Drift(drift.join(", ")));
        }
        Ok(report)
    }

    /// Builds a read-only ordered plan for pending migrations.
    ///
    /// The method performs the same discovery, executor-registration, history,
    /// drift, failed-history, SQL parsing, and transaction-mode gates as `up`.
    /// It does not acquire the advisory lock and can become stale if another
    /// process migrates concurrently.
    pub async fn plan_up(&self, pool: &Pool) -> Result<UpPlan, SchemalaneError> {
        let migrations = self.discover_migrations()?;
        self.ensure_rust_executors_registered(&migrations)?;
        let client = pool.get().await?;
        let repository = self.history_repository();
        let history = if repository.exists(&client).await? {
            repository.load(&client).await?
        } else {
            Vec::new()
        };
        Self::ensure_no_blocking_history(&migrations, &history)?;
        let latest = latest_history_by_script(&history);
        let mut planned = Vec::new();
        for migration in migrations {
            if latest
                .get(migration.script.as_str())
                .is_some_and(|row| row.success)
            {
                continue;
            }
            let (transaction_mode, statements) = match &migration.source {
                MigrationSource::SqlFile { content, .. } => {
                    let statements = parse_sql_migration(content)?;
                    let mode = resolve_sql_transaction_mode(&statements, &migration.script)?;
                    let mode = match mode {
                        SqlTransactionMode::Transactional => PlannedTransactionMode::Transactional,
                        SqlTransactionMode::NonTransactional => {
                            PlannedTransactionMode::NonTransactional
                        }
                    };
                    (
                        mode,
                        statements
                            .into_iter()
                            .map(|statement| statement.sql)
                            .collect(),
                    )
                }
                MigrationSource::RustFile(_) => (PlannedTransactionMode::Rust, Vec::new()),
            };
            planned.push(PlannedMigration {
                version: migration.version_text,
                script: migration.script,
                migration_type: migration.migration_type.as_history_type().to_owned(),
                transaction_mode,
                statements,
            });
        }
        Ok(UpPlan {
            schema: self.config.schema.clone(),
            history_table: self.config.history_table.clone(),
            migrations: planned,
        })
    }

    /// Transactional SQL migrations commit their history row atomically with the migration.
    /// Non-transactional SQL and Rust migrations record history after execution and therefore
    /// have at-least-once semantics; make those migrations idempotent. A pool size of one is
    /// sufficient because one detached connection owns the complete migration session.
    pub async fn fresh(&self, pool: &Pool, confirmed: bool) -> Result<RunReport, SchemalaneError> {
        self.fresh_with_observer(pool, confirmed, &NoopMigrationObserver)
            .await
    }

    /// Recreates the configured schema and reapplies all migrations with events.
    ///
    /// This destructive operation requires `confirmed == true`. Execution and
    /// history guarantees match [`Self::up_with_observer`].
    pub async fn fresh_with_observer<O>(
        &self,
        pool: &Pool,
        confirmed: bool,
        observer: &O,
    ) -> Result<RunReport, SchemalaneError>
    where
        O: MigrationObserver + ?Sized,
    {
        if !confirmed {
            return Err(SchemalaneError::FreshRequiresConfirm);
        }

        let migrations = self.discover_migrations()?;
        self.ensure_rust_executors_registered(&migrations)?;

        let (mut session, lock_id) = self.acquire_locked_session(pool).await?;
        let result = async {
            let client: &mut Client = &mut session;
            let history_repository = self.history_repository();
            self.reset_target_schema(client).await?;
            self.set_search_path(client).await?;
            history_repository.ensure_table(client).await?;
            observer.on_run_planned(&build_status_report(
                &self.config.schema,
                &self.config.history_table,
                &migrations,
                &[],
            ));

            let installed_by = self.resolve_installed_by(client).await?;
            let mut next_rank = 1;
            self.apply_all(
                client,
                &migrations,
                &HashSet::new(),
                &installed_by,
                &mut next_rank,
                &history_repository,
                observer,
                ApplyOptions {
                    skip_applied: false,
                },
            )
            .await
        }
        .await;
        Self::finish_locked_session(&session, lock_id, result).await
    }

    #[allow(clippy::too_many_arguments)] // Explicit run context keeps orchestration state visible.
    async fn apply_all<O>(
        &self,
        client: &mut Client,
        migrations: &[DiscoveredMigration],
        applied_ok: &HashSet<String>,
        installed_by: &str,
        next_rank: &mut i32,
        history_repository: &HistoryRepository,
        observer: &O,
        options: ApplyOptions,
    ) -> Result<RunReport, SchemalaneError>
    where
        O: MigrationObserver + ?Sized,
    {
        let mut report = RunReport::default();
        let total_to_apply = migrations
            .iter()
            .filter(|migration| !options.skip_applied || !applied_ok.contains(&migration.script))
            .count();
        let mut applied_index = 0usize;

        for migration in migrations {
            if options.skip_applied && applied_ok.contains(&migration.script) {
                report.skipped += 1;
                continue;
            }

            applied_index += 1;
            let migration_info = migration.info();
            observer.on_migration_start(&MigrationStarted {
                migration: migration_info.clone(),
                index: applied_index,
                total: total_to_apply,
            });

            let started = Instant::now();
            let history_write =
                Self::history_write(history_repository, migration, installed_by, *next_rank);
            let run_result = self
                .apply_migration(client, migration, observer, &history_write)
                .await;
            let execution_time_ms = millis_i32(started.elapsed().as_millis());

            match run_result {
                Ok(applied) => {
                    if applied == Applied::NeedsHistoryRow {
                        history_repository
                            .insert_client(client, &history_write, execution_time_ms, true)
                            .await?;
                    }
                    *next_rank += 1;
                    report.applied.push(AppliedMigration {
                        version: migration.version_text.clone(),
                        description: migration.description_display.clone(),
                        migration_type: migration.migration_type.as_history_type().to_owned(),
                        script: migration.script.clone(),
                        execution_time_ms,
                    });

                    observer.on_migration_finish(&MigrationFinished {
                        migration: migration_info,
                        index: applied_index,
                        total: total_to_apply,
                        execution_time_ms,
                    });
                }
                Err(err) => {
                    let mut error_message = err.to_string();
                    if !matches!(err, SchemalaneError::MixedStatements { .. }) {
                        match history_repository
                            .insert_client(client, &history_write, execution_time_ms, false)
                            .await
                        {
                            Ok(()) => *next_rank += 1,
                            Err(insert_err) => {
                                error_message = format!(
                                    "{error_message} (additionally: failed to record failed history row: {insert_err})"
                                );
                            }
                        }
                    }
                    let _ = next_rank;
                    observer.on_migration_failed(&MigrationFailed {
                        migration: migration_info,
                        index: applied_index,
                        total: total_to_apply,
                        execution_time_ms,
                        error: error_message,
                    });
                    return Err(match err {
                        SchemalaneError::Db(source) => SchemalaneError::MigrationExecution {
                            script: migration.script.clone(),
                            source,
                        },
                        other => other,
                    });
                }
            }
        }
        Ok(report)
    }

    async fn acquire_locked_session(
        &self,
        pool: &Pool,
    ) -> Result<(deadpool_postgres::ClientWrapper, i64), SchemalaneError> {
        let pooled = pool.get().await?;
        // Never return this stateful session to the caller's pool. Dropping the detached socket
        // releases its advisory lock and search_path on errors, panics, and cancellation too.
        let session = deadpool_postgres::Object::take(pooled);
        let lock_id = self.config.advisory_lock_id.unwrap_or_else(|| {
            derive_advisory_lock_id(&self.config.schema, &self.config.history_table)
        });
        session
            .execute("SELECT pg_advisory_lock($1)", &[&lock_id])
            .await?;
        Ok((session, lock_id))
    }

    async fn finish_locked_session<T>(
        session: &Client,
        lock_id: i64,
        result: Result<T, SchemalaneError>,
    ) -> Result<T, SchemalaneError> {
        let _ = session
            .execute("SELECT pg_advisory_unlock($1)", &[&lock_id])
            .await;
        result
    }

    fn ensure_no_blocking_history(
        migrations: &[DiscoveredMigration],
        history: &[HistoryRow],
    ) -> Result<(), SchemalaneError> {
        let latest = latest_history_by_script(history);
        let local_by_script: HashMap<&str, &DiscoveredMigration> =
            migrations.iter().map(|m| (m.script.as_str(), m)).collect();

        let mut failed = Vec::new();
        let mut missing = Vec::new();
        let mut checksum_mismatch = Vec::new();

        for row in latest.values() {
            if !row.success {
                failed.push(row.script.clone());
            }
            if row.success && !local_by_script.contains_key(row.script.as_str()) {
                missing.push(row.script.clone());
            }
        }

        for migration in migrations {
            if let Some(row) = latest.get(migration.script.as_str())
                && row.success
                && row.checksum != migration.checksum
            {
                checksum_mismatch.push(migration.script.clone());
            }
        }

        if !failed.is_empty() {
            failed.sort();
            return Err(SchemalaneError::FailedHistory(failed.join(", ")));
        }

        let mut drift_items = Vec::new();
        if !missing.is_empty() {
            drift_items.push(format!(
                "{} missing migration(s) in local crate",
                missing.len()
            ));
        }
        if !checksum_mismatch.is_empty() {
            drift_items.push(format!("{} checksum mismatch(es)", checksum_mismatch.len()));
        }

        if !drift_items.is_empty() {
            return Err(SchemalaneError::Drift(drift_items.join(", ")));
        }

        Ok(())
    }

    async fn apply_migration<O>(
        &self,
        client: &mut Client,
        migration: &DiscoveredMigration,
        observer: &O,
        history_write: &HistoryWrite<'_>,
    ) -> Result<Applied, SchemalaneError>
    where
        O: MigrationObserver + ?Sized,
    {
        let migration_info = migration.info();

        match &migration.source {
            MigrationSource::SqlFile {
                path: _path,
                content,
            } => {
                execute_sql_migration(client, content, &migration_info, observer, history_write)
                    .await
            }
            MigrationSource::RustFile(path) => {
                let executor = self
                    .rust_migrations
                    .get(migration.script.as_str())
                    .ok_or_else(|| {
                        SchemalaneError::Validation(format!(
                            "missing Rust migration executor for script {} ({})",
                            migration.script,
                            path.display()
                        ))
                    })?;
                execute_rust_migration(client, executor)
                    .await
                    .map_err(SchemalaneError::Db)?;
                Ok(Applied::NeedsHistoryRow)
            }
        }
    }

    fn history_write<'a>(
        repository: &'a HistoryRepository,
        migration: &'a DiscoveredMigration,
        installed_by: &'a str,
        installed_rank: i32,
    ) -> HistoryWrite<'a> {
        HistoryWrite {
            repository,
            installed_rank,
            version: &migration.version_text,
            description: &migration.description_display,
            migration_type: migration.migration_type.as_history_type(),
            script: &migration.script,
            checksum: migration.checksum,
            installed_by,
        }
    }

    fn history_repository(&self) -> HistoryRepository {
        HistoryRepository::new(&self.config.schema, &self.config.history_table)
    }

    /// Create the configured schema if it does not already exist. Mirrors
    /// Flyway's behavior with `-schemas=<name>`, which auto-creates the schema.
    async fn ensure_target_schema(&self, client: &Client) -> Result<(), SchemalaneError> {
        let sql = format!(
            "CREATE SCHEMA IF NOT EXISTS {}",
            quote_ident(&self.config.schema)
        );
        client.batch_execute(&sql).await?;
        Ok(())
    }

    /// Prepend the configured schema to the connection's existing `search_path`
    /// so unqualified DDL in user migrations lands there. Matches Flyway's
    /// `PostgreSQLConnection.doChangeCurrentSchemaOrSearchPathTo`, which uses
    /// `set_config('search_path', '<schema>,<original>', false)` rather than
    /// replacing the path outright. Replacing it would strip `public` (and any
    /// caller-configured paths), which silently hides extensions installed
    /// there — e.g. `citext` referenced as an unqualified type by tokio-postgres
    /// when binding parameters of `public.citext` columns.
    async fn set_search_path(&self, client: &Client) -> Result<(), SchemalaneError> {
        let original: String = client
            .query_one("SELECT current_setting('search_path') AS path", &[])
            .await?
            .get("path");
        let new_path = format!("{}, {}", quote_ident(&self.config.schema), original);
        client
            .execute("SELECT set_config('search_path', $1, false)", &[&new_path])
            .await?;
        Ok(())
    }

    async fn resolve_installed_by(&self, client: &Client) -> Result<String, SchemalaneError> {
        if let Some(installed_by) = &self.config.installed_by {
            return Ok(installed_by.clone());
        }

        let row = client
            .query_one("SELECT current_user AS current_user", &[])
            .await?;
        let current_user: String = row.get("current_user");
        Ok(current_user)
    }

    /// Drop the configured target schema (CASCADE) and recreate it empty.
    /// `fresh` is scoped to this single schema per `SCHEMALANE_SPEC.md` §9 —
    /// it must never touch other schemas in the database.
    async fn reset_target_schema(&self, client: &Client) -> Result<(), SchemalaneError> {
        let sql = format!(
            "DROP SCHEMA IF EXISTS {} CASCADE",
            quote_ident(&self.config.schema)
        );
        client.batch_execute(&sql).await?;
        self.ensure_target_schema(client).await
    }
}

#[derive(Debug, Clone, Copy)]
struct ApplyOptions {
    skip_applied: bool,
}

/// Returns the pending-migration exit condition used by CI status checks.
pub const fn should_fail_on_pending(report: &StatusReport) -> Result<(), SchemalaneError> {
    if report.summary.pending > 0 {
        Err(SchemalaneError::PendingMigrations(report.summary.pending))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::sql_analysis::{
        SqlTransactionMode, is_non_transactional, parse_sql_migration, resolve_sql_transaction_mode,
    };

    use super::{
        DiscoveredMigration, HistoryRow, MigrationSource, MigrationState, MigrationType,
        SchemalaneConfig, SchemalaneError, SchemalaneMigrator, build_status_report,
        calculate_checksum, derive_advisory_lock_id, init_migration_project, parse_sql_filename,
    };
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;

    #[test]
    fn delegated_error_exit_code_is_forwarded_verbatim() {
        for code in [1, 2, 3, 4, 5, 6, 7, 42] {
            assert_eq!(SchemalaneError::Delegated { code }.exit_code(), code);
        }
    }

    #[test]
    fn exit_codes_match_spec_section_8() {
        use SchemalaneError as E;
        assert_eq!(E::Validation("x".into()).exit_code(), 2);
        assert_eq!(E::Config("x".into()).exit_code(), 1);
        assert_eq!(E::Internal("x".into()).exit_code(), 1);
        assert_eq!(E::Drift("x".into()).exit_code(), 3);
        assert_eq!(E::FailedHistory("x".into()).exit_code(), 4);
        assert_eq!(E::PendingMigrations(3).exit_code(), 5);
        assert_eq!(E::FreshRequiresConfirm.exit_code(), 6);
        assert_eq!(
            E::MixedStatements {
                script: "s".into(),
                line: 1,
            }
            .exit_code(),
            7
        );
        assert_eq!(E::Io(std::io::Error::other("x")).exit_code(), 1);
        // Database-backed variants wrap non-constructible driver errors and share fallback 1.
    }

    #[test]
    fn advisory_lock_key_is_stable_and_target_scoped() {
        let a = derive_advisory_lock_id("public", "flyway_schema_history");
        assert_eq!(
            a,
            derive_advisory_lock_id("public", "flyway_schema_history")
        );
        assert_ne!(
            a,
            derive_advisory_lock_id("tenant_b", "flyway_schema_history")
        );
        assert_ne!(a, derive_advisory_lock_id("public", "other_history"));
        assert_ne!(
            derive_advisory_lock_id("ab", "c"),
            derive_advisory_lock_id("a", "bc")
        );
    }

    fn discovered(script: &str, checksum: Option<i32>) -> DiscoveredMigration {
        let (version_text, version, description) = parse_sql_filename(script).expect("filename");
        DiscoveredMigration {
            version,
            version_text,
            description_display: description.replace('_', " "),
            script: script.to_owned(),
            checksum,
            migration_type: MigrationType::Sql,
            source: MigrationSource::SqlFile {
                path: PathBuf::from(script),
                content: String::new(),
            },
        }
    }

    fn history_row(script: &str, rank: i32, success: bool, checksum: Option<i32>) -> HistoryRow {
        let (version, _, description) = parse_sql_filename(script).expect("filename");
        HistoryRow {
            installed_rank: rank,
            version: Some(version),
            description: description.replace('_', " "),
            migration_type: "SQL".to_owned(),
            script: script.to_owned(),
            checksum,
            installed_on: "now".to_owned(),
            execution_time: 1,
            success,
        }
    }

    fn state_report(
        migrations: &[DiscoveredMigration],
        history: &[HistoryRow],
    ) -> super::StatusReport {
        build_status_report("public", "flyway_schema_history", migrations, history)
    }

    #[test]
    fn status_classifies_success() {
        let report = state_report(
            &[discovered("V1__a.sql", Some(1))],
            &[history_row("V1__a.sql", 1, true, Some(1))],
        );
        assert_eq!(report.migrations[0].state, MigrationState::Success);
        assert_eq!(report.summary.success, 1);
    }

    #[test]
    fn status_classifies_pending() {
        let report = state_report(&[discovered("V2__b.sql", Some(2))], &[]);
        assert_eq!(report.migrations[0].state, MigrationState::Pending);
        assert_eq!(report.migrations[0].installed_rank, None);
        assert_eq!(report.summary.pending, 1);
    }

    #[test]
    fn status_failed_precedes_checksum_comparison() {
        let report = state_report(
            &[discovered("V1__a.sql", Some(1))],
            &[history_row("V1__a.sql", 1, false, Some(1))],
        );
        assert_eq!(report.migrations[0].state, MigrationState::Failed);
        assert_eq!(report.summary.failed, 1);
        assert_eq!(report.summary.checksum_mismatch, 0);
    }

    #[test]
    fn status_classifies_checksum_mismatch() {
        let report = state_report(
            &[discovered("V1__a.sql", Some(2))],
            &[history_row("V1__a.sql", 1, true, Some(1))],
        );
        assert_eq!(report.migrations[0].state, MigrationState::ChecksumMismatch);
        assert_eq!(report.summary.checksum_mismatch, 1);
    }

    #[test]
    fn status_classifies_missing() {
        let report = state_report(&[], &[history_row("V1__gone.sql", 1, true, Some(1))]);
        assert_eq!(report.migrations[0].state, MigrationState::Missing);
        assert_eq!(report.migrations[0].script, "V1__gone.sql");
        assert_eq!(report.summary.missing, 1);
    }

    #[test]
    fn status_latest_retry_wins() {
        let rows = [
            history_row("V1__a.sql", 1, false, Some(1)),
            history_row("V1__a.sql", 2, true, Some(1)),
        ];
        let report = state_report(&[discovered("V1__a.sql", Some(1))], &rows);
        assert_eq!(report.migrations[0].state, MigrationState::Success);
        assert_eq!(report.migrations[0].installed_rank, Some(2));
        assert_eq!(report.summary.success, 1);
    }

    #[test]
    fn status_orders_parsed_versions_with_missing_interleaved() {
        let migrations = [
            discovered("V10__ten.sql", Some(10)),
            discovered("V2__two.sql", Some(2)),
        ];
        let history = [history_row("V5__missing.sql", 1, true, Some(5))];
        let report = state_report(&migrations, &history);
        assert_eq!(
            report
                .migrations
                .iter()
                .map(|entry| entry.script.as_str())
                .collect::<Vec<_>>(),
            ["V2__two.sql", "V5__missing.sql", "V10__ten.sql"]
        );
        assert_eq!(report.summary.pending, 2);
        assert_eq!(report.summary.missing, 1);
    }

    #[test]
    fn gating_allows_clean_history() {
        assert!(
            SchemalaneMigrator::ensure_no_blocking_history(
                &[discovered("V1__a.sql", Some(1))],
                &[history_row("V1__a.sql", 1, true, Some(1))]
            )
            .is_ok()
        );
    }

    #[test]
    fn gating_rejects_failed_history_with_exit_four() {
        let err = SchemalaneMigrator::ensure_no_blocking_history(
            &[discovered("V1__a.sql", Some(1))],
            &[history_row("V1__a.sql", 1, false, Some(1))],
        )
        .expect_err("failed");
        assert!(matches!(err, SchemalaneError::FailedHistory(_)));
        assert_eq!(err.exit_code(), 4);
    }

    #[test]
    fn gating_rejects_missing_local_with_exit_three() {
        let err = SchemalaneMigrator::ensure_no_blocking_history(
            &[],
            &[history_row("V1__a.sql", 1, true, Some(1))],
        )
        .expect_err("drift");
        assert!(matches!(err, SchemalaneError::Drift(_)));
        assert_eq!(err.exit_code(), 3);
    }

    #[test]
    fn gating_rejects_checksum_mismatch_with_exit_three() {
        let err = SchemalaneMigrator::ensure_no_blocking_history(
            &[discovered("V1__a.sql", Some(2))],
            &[history_row("V1__a.sql", 1, true, Some(1))],
        )
        .expect_err("drift");
        assert!(matches!(err, SchemalaneError::Drift(_)));
        assert_eq!(err.exit_code(), 3);
    }

    #[test]
    fn gating_failed_history_precedes_drift() {
        let migrations = [discovered("V1__a.sql", Some(2))];
        let history = [
            history_row("V1__a.sql", 1, false, Some(1)),
            history_row("V2__gone.sql", 2, true, Some(2)),
        ];
        let err = SchemalaneMigrator::ensure_no_blocking_history(&migrations, &history)
            .expect_err("blocked");
        assert!(matches!(err, SchemalaneError::FailedHistory(_)));
    }

    #[test]
    fn gating_latest_success_clears_prior_failure() {
        let migrations = [discovered("V1__a.sql", Some(1))];
        let history = [
            history_row("V1__a.sql", 1, false, Some(1)),
            history_row("V1__a.sql", 2, true, Some(1)),
        ];
        assert!(SchemalaneMigrator::ensure_no_blocking_history(&migrations, &history).is_ok());
    }

    // Golden values independently computed with Python's zlib.crc32 from spec §6.3.
    #[test]
    fn checksum_golden_values() {
        let cases: &[(&str, &[u8], i32)] = &[
            ("empty.sql", b"", 0),
            ("single.sql", b"CREATE TABLE cake (id INT);", -1_600_817_622),
            (
                "single_nl.sql",
                b"CREATE TABLE cake (id INT);\n",
                -1_600_817_622,
            ),
            (
                "two_lf.sql",
                b"CREATE TABLE cake (\n    id INT\n);",
                1_160_935_991,
            ),
            (
                "utf8.sql",
                "-- caké 🍰\nSELECT 'schöne Grüße';\n".as_bytes(),
                -714_250_905,
            ),
        ];
        for (script, bytes, expected) in cases {
            assert_eq!(
                calculate_checksum(script, bytes).expect("checksum"),
                *expected,
                "golden mismatch for {script}"
            );
        }
    }

    #[test]
    fn checksum_line_endings_are_equivalent() {
        let lf = calculate_checksum("a.sql", b"line one\nline two\n").expect("checksum");
        let crlf = calculate_checksum("a.sql", b"line one\r\nline two\r\n").expect("checksum");
        assert_eq!(lf, crlf, "LF and CRLF must hash identically");
    }

    #[test]
    fn checksum_trailing_newline_is_irrelevant() {
        let with_nl = calculate_checksum("a.sql", b"SELECT 1;\n").expect("checksum");
        let without = calculate_checksum("a.sql", b"SELECT 1;").expect("checksum");
        assert_eq!(with_nl, without);
    }

    #[test]
    fn checksum_line_terminator_bytes_are_excluded() {
        let joined = calculate_checksum("a.sql", b"ab").expect("checksum");
        let split = calculate_checksum("a.sql", b"a\nb").expect("checksum");
        assert_eq!(joined, split, "line terminators must not be hashed");
        assert_eq!(
            split,
            calculate_checksum("a.sql", b"a\r\nb").expect("checksum")
        );
    }

    #[test]
    fn checksum_can_be_negative_i32() {
        let value = calculate_checksum("neg.sql", b"negative fixture 2").expect("checksum");
        assert!(value < 0, "expected negative checksum, got {value}");
        assert_eq!(value, -1_301_979_683);
    }

    #[test]
    fn checksum_rejects_non_utf8() {
        let err =
            calculate_checksum("bad.sql", &[0xff, 0xfe, b'a']).expect_err("non-UTF-8 must fail");
        assert!(err.to_string().contains("not valid UTF-8"), "got: {err}");
    }

    fn migrator_with_files(files: &[(&str, &str)]) -> (TempDir, SchemalaneMigrator) {
        let temp = TempDir::new().expect("temp dir");
        let dir = temp.path().join("migrations");
        fs::create_dir_all(&dir).expect("mkdir");
        for (name, contents) in files {
            fs::write(dir.join(name), contents).expect("write migration");
        }
        let migrator = SchemalaneMigrator::new(SchemalaneConfig {
            migrations_dir: dir,
            ..Default::default()
        });
        (temp, migrator)
    }

    #[test]
    fn rejects_semantically_duplicate_versions() {
        let (_temp, migrator) =
            migrator_with_files(&[("V1__a.sql", "SELECT 1;"), ("V1.0__b.sql", "SELECT 2;")]);

        let err = migrator
            .discover_migrations()
            .err()
            .expect("V1 and V1.0 must collide");
        let msg = err.to_string();
        assert!(msg.contains("duplicate migration version"), "got: {msg}");
        assert!(
            msg.contains("V1__a.sql") && msg.contains("V1.0__b.sql"),
            "got: {msg}"
        );
    }

    #[test]
    fn rejects_leading_zero_duplicate_versions() {
        let (_temp, migrator) =
            migrator_with_files(&[("V1__a.sql", "SELECT 1;"), ("V01__b.sql", "SELECT 2;")]);
        let err = migrator
            .discover_migrations()
            .err()
            .expect("V1 and V01 must collide");
        let msg = err.to_string();
        assert!(msg.contains("V1__a.sql") && msg.contains("V01__b.sql"));
    }

    #[test]
    fn rejects_cross_type_duplicate_versions() {
        let (_temp, migrator) = migrator_with_files(&[
            ("V1__a.sql", "SELECT 1;"),
            ("V1_0__b.rs", "pub async fn migration() {}"),
        ]);
        let err = migrator
            .discover_migrations()
            .err()
            .expect("SQL and Rust semantic versions must collide");
        let msg = err.to_string();
        assert!(msg.contains("V1__a.sql") && msg.contains("V1_0__b.rs"));
    }

    #[test]
    fn accepts_distinct_versions_with_shared_prefix() {
        let (_temp, migrator) =
            migrator_with_files(&[("V1__a.sql", "SELECT 1;"), ("V1.1__b.sql", "SELECT 2;")]);
        assert_eq!(migrator.discover_migrations().expect("distinct").len(), 2);
    }

    #[test]
    fn discovery_formats_description_for_display() {
        let (_temp, migrator) = migrator_with_files(&[("V1__add_user_table.sql", "SELECT 1;")]);
        let migrations = migrator.discover_migrations().expect("discover");
        assert_eq!(migrations[0].description_display, "add user table");
    }

    #[test]
    fn parse_sql_migration_handles_quotes_comments_and_dollar_blocks() {
        let sql = r"
CREATE TABLE ledger (
    id SERIAL PRIMARY KEY,
    note TEXT NOT NULL DEFAULT 'fee;rebate'
);

CREATE FUNCTION add_event() RETURNS trigger AS $$
BEGIN
    INSERT INTO ledger(note) VALUES ('body;semicolon');
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

INSERT INTO ledger(note) VALUES ('ok');
";

        let statements = parse_sql_migration(sql).expect("should parse");
        assert_eq!(statements.len(), 3, "expected three executable statements");
        assert!(
            statements[0].sql.to_uppercase().starts_with("CREATE TABLE"),
            "first statement should be CREATE TABLE, got: {}",
            statements[0].sql,
        );
        assert!(
            statements[1].sql.contains("body;semicolon"),
            "function body should stay intact"
        );
    }

    #[test]
    fn parse_sql_migration_ignores_empty_segments() {
        let statements = parse_sql_migration(";\n ;\nSELECT 1;;\n").expect("should parse");
        assert_eq!(statements.len(), 1);
        assert_eq!(statements[0].sql, "SELECT 1");
    }

    #[test]
    fn parse_sql_migration_reports_statement_line_numbers() {
        let sql = "SELECT 1;\n\n\nSELECT 2;\n";
        let statements = parse_sql_migration(sql).expect("parse");
        assert_eq!(statements.len(), 2);
        assert_eq!(statements[0].source_line, 1);
        assert_eq!(
            statements[1].source_line, 4,
            "second statement starts on line 4"
        );
    }

    #[test]
    fn init_scaffold_creates_expected_files() {
        let temp = TempDir::new().expect("temp dir");
        let target = temp.path().join("migration");

        let report = init_migration_project(&target, false).expect("init should succeed");
        assert!(!report.created.is_empty(), "should create scaffold files");
        assert!(
            target.join("Cargo.toml").exists(),
            "Cargo.toml should be created"
        );
        assert!(
            target.join("src/main.rs").exists(),
            "main runner should be created"
        );
        assert!(
            target.join("build.rs").exists(),
            "build.rs should be created"
        );
        let build_rs = fs::read_to_string(target.join("build.rs")).expect("read build.rs");
        assert!(build_rs.contains("cargo::rerun-if-changed=migrations"));
        assert!(
            target.join("migrations/V1__create_cake_table.sql").exists(),
            "sample SQL migration should be created"
        );
        assert!(
            target.join("migrations/V2__seed_cake_table.rs").exists(),
            "sample Rust migration should be created"
        );
        assert!(
            !target.join("src/rust_migrations/mod.rs").exists(),
            "scaffold should not require manual rust migration module lists"
        );

        let cargo_toml =
            fs::read_to_string(target.join("Cargo.toml")).expect("read generated Cargo.toml");
        assert!(
            cargo_toml.contains("schemalane-core = \"0.1\""),
            "scaffold should depend on schemalane-core from crates.io"
        );
        assert!(
            cargo_toml.contains("schemalane-cli = \"0.1\""),
            "scaffold should depend on schemalane-cli from crates.io"
        );
        assert!(
            !cargo_toml.contains("kellnr"),
            "scaffold must not reference a private registry"
        );
        assert!(
            cargo_toml.contains("tokio-postgres = \"0.7\""),
            "sample Rust migration needs its direct driver dependency"
        );

        let lib_source = fs::read_to_string(target.join("src/lib.rs")).expect("read src/lib.rs");
        assert!(
            lib_source.contains("embed_migrations!"),
            "scaffold should use embed_migrations macro"
        );
    }

    #[test]
    fn init_scaffold_requires_force_for_non_empty_directory() {
        let temp = TempDir::new().expect("temp dir");
        let target = temp.path().join("migration");
        fs::create_dir_all(&target).expect("create target");
        fs::write(target.join("existing.txt"), "existing").expect("write marker");

        let err = init_migration_project(&target, false).expect_err("expected validation failure");
        assert!(
            matches!(err, SchemalaneError::Validation(ref message) if message.contains("not empty")),
            "expected non-empty directory validation error, got: {err}"
        );

        let report = init_migration_project(&target, true).expect("force init should succeed");
        assert!(
            !report.overwritten.is_empty() || !report.created.is_empty(),
            "force init should write scaffold files"
        );
    }

    // ── Non-transactional statement detection ───────────────────────────

    /// Helper: parse a single SQL statement and check if it's non-transactional.
    fn check_non_transactional(sql: &str) -> bool {
        let stmts = parse_sql_migration(sql).expect("should parse");
        assert_eq!(stmts.len(), 1, "expected 1 statement for: {sql}");
        is_non_transactional(&stmts[0])
    }

    #[test]
    fn detects_create_index_concurrently() {
        assert!(check_non_transactional(
            "CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_test ON public.t (LOWER(col));"
        ));
    }

    #[test]
    fn detects_create_unique_index_concurrently() {
        assert!(check_non_transactional(
            "CREATE UNIQUE INDEX CONCURRENTLY idx_uniq ON t (col);"
        ));
    }

    #[test]
    fn detects_drop_index_concurrently() {
        assert!(check_non_transactional(
            "DROP INDEX CONCURRENTLY IF EXISTS idx_test;"
        ));
    }

    #[test]
    fn detects_vacuum() {
        assert!(check_non_transactional("VACUUM;"));
    }

    #[test]
    fn detects_vacuum_analyze() {
        assert!(check_non_transactional("VACUUM ANALYZE public.wallets;"));
    }

    #[test]
    fn detects_create_database() {
        assert!(check_non_transactional("CREATE DATABASE test_db;"));
    }

    #[test]
    fn detects_drop_database() {
        assert!(check_non_transactional("DROP DATABASE IF EXISTS test_db;"));
    }

    #[test]
    fn detects_alter_system() {
        assert!(check_non_transactional(
            "ALTER SYSTEM SET work_mem = '256MB';"
        ));
    }

    #[test]
    fn detects_discard_all() {
        assert!(check_non_transactional("DISCARD ALL;"));
    }

    #[test]
    fn detects_reindex_database() {
        assert!(check_non_transactional("REINDEX DATABASE chainargos;"));
    }

    #[test]
    fn detects_reindex_verbose_schema() {
        assert!(check_non_transactional("REINDEX (VERBOSE) SCHEMA public;"));
    }

    #[test]
    fn detects_reindex_system() {
        assert!(check_non_transactional("REINDEX SYSTEM chainargos;"));
    }

    #[test]
    fn reindex_table_is_transactional() {
        assert!(!check_non_transactional("REINDEX TABLE public.wallets;"));
    }

    #[test]
    fn reindex_index_is_transactional() {
        assert!(!check_non_transactional(
            "REINDEX INDEX public.idx_wallets_address;"
        ));
    }

    #[test]
    fn discard_plans_is_transactional() {
        assert!(!check_non_transactional("DISCARD PLANS;"));
    }

    #[test]
    fn discard_sequences_is_transactional() {
        assert!(!check_non_transactional("DISCARD SEQUENCES;"));
    }

    #[test]
    fn discard_temp_is_transactional() {
        assert!(!check_non_transactional("DISCARD TEMP;"));
    }

    #[test]
    fn detects_create_tablespace() {
        assert!(check_non_transactional(
            "CREATE TABLESPACE fast_space LOCATION '/ssd';"
        ));
    }

    #[test]
    fn detects_create_subscription() {
        assert!(check_non_transactional(
            "CREATE SUBSCRIPTION sub CONNECTION 'host=h dbname=d' PUBLICATION pub;"
        ));
    }

    #[test]
    fn regular_create_index_is_transactional() {
        assert!(!check_non_transactional(
            "CREATE INDEX IF NOT EXISTS idx_test ON public.t (col);"
        ));
    }

    #[test]
    fn regular_drop_index_is_transactional() {
        assert!(!check_non_transactional("DROP INDEX IF EXISTS idx_test;"));
    }

    #[test]
    fn create_table_is_transactional() {
        assert!(!check_non_transactional(
            "CREATE TABLE t (id SERIAL PRIMARY KEY);"
        ));
    }

    #[test]
    fn insert_is_transactional() {
        assert!(!check_non_transactional("INSERT INTO t (id) VALUES (1);"));
    }

    #[test]
    fn standalone_analyze_is_transactional() {
        assert!(!check_non_transactional("ANALYZE public.ethereum_txns;"));
    }

    #[test]
    fn standalone_analyze_without_table_is_transactional() {
        assert!(!check_non_transactional("ANALYZE;"));
    }

    // ── Transaction mode resolution ─────────────────────────────────────

    #[test]
    fn resolves_all_transactional_statements() {
        let sql = "CREATE TABLE t (id SERIAL PRIMARY KEY);\nINSERT INTO t (id) VALUES (1);";
        let stmts = parse_sql_migration(sql).expect("parse");
        let mode = resolve_sql_transaction_mode(&stmts, "V1__test.sql").expect("should resolve");
        assert_eq!(mode, SqlTransactionMode::Transactional);
    }

    #[test]
    fn resolves_all_non_transactional_statements() {
        let sql = concat!(
            "CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_a ON t (LOWER(a));\n",
            "CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_b ON t (b, LOWER(c));\n",
        );
        let stmts = parse_sql_migration(sql).expect("parse");
        let mode = resolve_sql_transaction_mode(&stmts, "V2__test.sql").expect("should resolve");
        assert_eq!(mode, SqlTransactionMode::NonTransactional);
    }

    #[test]
    fn rejects_mixed_transactional_and_non_transactional() {
        let sql = concat!(
            "CREATE TABLE t (id SERIAL PRIMARY KEY);\n",
            "CREATE INDEX CONCURRENTLY idx_a ON t (id);\n",
        );
        let stmts = parse_sql_migration(sql).expect("parse");
        let err =
            resolve_sql_transaction_mode(&stmts, "V3__mixed.sql").expect_err("should reject mixed");
        assert!(
            matches!(err, SchemalaneError::MixedStatements { ref script, .. } if script == "V3__mixed.sql"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn drop_index_with_analyze_resolves_to_transactional() {
        let sql = concat!(
            "DROP INDEX IF EXISTS public.idx_test;\n",
            "DROP EXTENSION IF EXISTS pg_trgm;\n",
            "ANALYZE public.ethereum_txns;\n",
        );
        let stmts = parse_sql_migration(sql).expect("parse");
        let mode = resolve_sql_transaction_mode(&stmts, "V12__revert.sql")
            .expect("should resolve as transactional");
        assert_eq!(mode, SqlTransactionMode::Transactional);
    }

    #[test]
    fn empty_migration_resolves_to_transactional() {
        let sql = "-- empty migration\n";
        let stmts = parse_sql_migration(sql).expect("parse");
        let mode = resolve_sql_transaction_mode(&stmts, "V4__empty.sql").expect("should resolve");
        assert_eq!(mode, SqlTransactionMode::Transactional);
    }

    // ── pg_query splitting ──────────────────────────────────────────────

    #[test]
    fn parse_sql_migration_splits_multiple_statements() {
        let stmts = parse_sql_migration("SELECT 1;\nSELECT 2;").expect("parse");
        assert_eq!(stmts.len(), 2);
    }

    #[test]
    fn parse_sql_migration_handles_dollar_quoting() {
        let stmts = parse_sql_migration(
            "CREATE FUNCTION f() RETURNS void AS $$BEGIN PERFORM 1; END;$$ LANGUAGE plpgsql;\nSELECT 2;",
        ).expect("parse");
        assert_eq!(stmts.len(), 2);
    }

    #[test]
    fn parse_sql_migration_handles_semicolons_in_strings() {
        let stmts = parse_sql_migration("INSERT INTO t VALUES ('a;b');\nSELECT 1;").expect("parse");
        assert_eq!(stmts.len(), 2);
        assert!(stmts[0].sql.contains("'a;b'"));
    }
}
