use crc32fast::Hasher;
use deadpool_postgres::Pool;
use pg_query::protobuf;
use serde::Serialize;
use std::collections::{BTreeSet, HashMap};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;
use thiserror::Error;
use tokio_postgres::types::ToSql;
use tokio_postgres::{Client, Transaction};

mod filename;

use filename::{ParsedVersion, parse_rust_filename, parse_sql_filename};

pub use schemalane_macros::embed_migrations;

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

#[derive(Debug, Clone)]
pub struct SchemalaneConfig {
    pub schema: String,
    pub history_table: String,
    pub migrations_dir: PathBuf,
    pub installed_by: Option<String>,
    pub advisory_lock_id: Option<i64>,
}

impl Default for SchemalaneConfig {
    fn default() -> Self {
        Self {
            schema: "public".to_owned(),
            history_table: "flyway_schema_history".to_owned(),
            migrations_dir: PathBuf::from("./migrations"),
            installed_by: None,
            advisory_lock_id: None,
        }
    }
}

#[derive(Debug, Error)]
pub enum SchemalaneError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Database error: {0}")]
    Db(#[from] tokio_postgres::Error),

    #[error("Pool error: {0}")]
    Pool(#[from] deadpool_postgres::PoolError),

    #[error("Validation error: {0}")]
    Validation(String),

    #[error("Drift detected: {0}")]
    Drift(String),

    #[error("Failed migration found in history: {0}")]
    FailedHistory(String),

    #[error("Migration execution failed for {script}: {source}")]
    MigrationExecution {
        script: String,
        #[source]
        source: tokio_postgres::Error,
    },

    #[error(
        "Detected both transactional and non-transactional statements within the same migration {script} (line {line})"
    )]
    MixedStatements { script: String, line: u64 },

    #[error("`fresh` requires --confirm yes")]
    FreshRequiresConfirm,

    #[error("Pending migrations found ({0})")]
    PendingMigrations(usize),

    #[error("migration crate command exited with code {code}")]
    Delegated { code: i32 },
}

impl SchemalaneError {
    pub const fn exit_code(&self) -> i32 {
        match self {
            Self::Validation(_) => 2,
            Self::Drift(_) => 3,
            Self::FailedHistory(_) => 4,
            Self::PendingMigrations(_) => 5,
            Self::FreshRequiresConfirm => 6,
            Self::MixedStatements { .. } => 7,
            Self::Delegated { code } => *code,
            _ => 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub enum MigrationState {
    Success,
    Pending,
    Failed,
    Missing,
    ChecksumMismatch,
}

#[derive(Debug, Clone, Serialize)]
pub struct StatusEntry {
    pub version: Option<String>,
    pub description: String,
    #[serde(rename = "type")]
    pub migration_type: String,
    pub script: String,
    pub checksum: Option<i32>,
    pub installed_rank: Option<i32>,
    pub installed_on: Option<String>,
    pub execution_time_ms: Option<i32>,
    pub state: MigrationState,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct StatusSummary {
    pub success: usize,
    pub pending: usize,
    pub failed: usize,
    pub missing: usize,
    pub checksum_mismatch: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct StatusReport {
    pub schema: String,
    pub history_table: String,
    pub migrations: Vec<StatusEntry>,
    pub summary: StatusSummary,
}

#[derive(Debug, Clone, Serialize)]
pub struct AppliedMigration {
    pub version: String,
    pub description: String,
    #[serde(rename = "type")]
    pub migration_type: String,
    pub script: String,
    pub execution_time_ms: i32,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct RunReport {
    pub applied: Vec<AppliedMigration>,
    pub skipped: usize,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct InitReport {
    pub root: PathBuf,
    pub created: Vec<PathBuf>,
    pub overwritten: Vec<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct MigrationInfo {
    pub version: String,
    pub description: String,
    pub migration_type: String,
    pub script: String,
}

#[derive(Debug, Clone)]
pub struct MigrationStarted {
    pub migration: MigrationInfo,
    pub index: usize,
    pub total: usize,
}

#[derive(Debug, Clone)]
pub struct MigrationFinished {
    pub migration: MigrationInfo,
    pub index: usize,
    pub total: usize,
    pub execution_time_ms: i32,
}

#[derive(Debug, Clone)]
pub struct MigrationFailed {
    pub migration: MigrationInfo,
    pub index: usize,
    pub total: usize,
    pub execution_time_ms: i32,
    pub error: String,
}

#[derive(Debug, Clone)]
pub struct SqlStatementStarted {
    pub migration: MigrationInfo,
    pub statement_index: usize,
    pub total_statements: usize,
    pub statement_preview: String,
    pub statement: String,
    /// Source line number in the migration file (1-based), or `None` if unavailable.
    pub source_line: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct SqlStatementFinished {
    pub migration: MigrationInfo,
    pub statement_index: usize,
    pub total_statements: usize,
    pub statement_preview: String,
    pub statement: String,
    pub execution_time_ms: i32,
    /// Source line number in the migration file (1-based), or `None` if unavailable.
    pub source_line: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct SqlStatementFailed {
    pub migration: MigrationInfo,
    pub statement_index: usize,
    pub total_statements: usize,
    pub statement_preview: String,
    pub statement: String,
    pub execution_time_ms: i32,
    pub error: String,
    /// Source line number in the migration file (1-based), or `None` if unavailable.
    pub source_line: Option<u64>,
}

pub trait MigrationObserver: Send + Sync {
    fn on_migration_start(&self, _event: &MigrationStarted) {}

    fn on_migration_finish(&self, _event: &MigrationFinished) {}

    fn on_migration_failed(&self, _event: &MigrationFailed) {}

    fn on_sql_statement_start(&self, _event: &SqlStatementStarted) {}

    fn on_sql_statement_finish(&self, _event: &SqlStatementFinished) {}

    fn on_sql_statement_failed(&self, _event: &SqlStatementFailed) {}
}

#[derive(Debug, Clone, Copy, Default)]
pub struct NoopMigrationObserver;

impl MigrationObserver for NoopMigrationObserver {}

pub fn init_migration_project(path: &Path, force: bool) -> Result<InitReport, SchemalaneError> {
    if path.exists() {
        if !path.is_dir() {
            return Err(SchemalaneError::Validation(format!(
                "init target exists and is not a directory: {}",
                path.display()
            )));
        }

        let has_entries = std::fs::read_dir(path)?.next().is_some();
        if has_entries && !force {
            return Err(SchemalaneError::Validation(format!(
                "init target directory is not empty: {} (use --force to overwrite)",
                path.display()
            )));
        }
    } else {
        std::fs::create_dir_all(path)?;
    }

    let package_name = sanitize_package_name(
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("migration"),
    );
    let lib_ident = package_to_lib_ident(&package_name);

    let files = init_template_files(&package_name, &lib_ident);
    let mut created = Vec::new();
    let mut overwritten = Vec::new();

    for (relative, content) in files {
        let target = path.join(relative);
        write_init_file(&target, &content, force, &mut created, &mut overwritten)?;
    }

    created.sort();
    overwritten.sort();

    Ok(InitReport {
        root: path.to_path_buf(),
        created,
        overwritten,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RustTransactionMode {
    NoTransaction,
    Transaction,
}

pub type RustMigrationFuture<'a> =
    Pin<Box<dyn Future<Output = Result<(), tokio_postgres::Error>> + Send + 'a>>;

type DynRustMigrationFn = dyn for<'a> Fn(&'a Client) -> RustMigrationFuture<'a> + Send + Sync;

#[derive(Clone)]
pub struct RustMigrationExecutor {
    transaction_mode: RustTransactionMode,
    run: Arc<DynRustMigrationFn>,
}

impl RustMigrationExecutor {
    pub fn new<F>(run: F) -> Self
    where
        F: for<'a> Fn(&'a Client) -> RustMigrationFuture<'a> + Send + Sync + 'static,
    {
        Self::with_mode(RustTransactionMode::NoTransaction, run)
    }

    pub fn transactional<F>(run: F) -> Self
    where
        F: for<'a> Fn(&'a Client) -> RustMigrationFuture<'a> + Send + Sync + 'static,
    {
        Self::with_mode(RustTransactionMode::Transaction, run)
    }

    pub fn with_mode<F>(transaction_mode: RustTransactionMode, run: F) -> Self
    where
        F: for<'a> Fn(&'a Client) -> RustMigrationFuture<'a> + Send + Sync + 'static,
    {
        Self {
            transaction_mode,
            run: Arc::new(run),
        }
    }

    const fn transaction_mode(&self) -> RustTransactionMode {
        self.transaction_mode
    }

    async fn up(&self, client: &Client) -> Result<(), tokio_postgres::Error> {
        (self.run)(client).await
    }
}

pub struct SchemalaneMigrator {
    config: SchemalaneConfig,
    rust_migrations: HashMap<String, RustMigrationExecutor>,
}

impl SchemalaneMigrator {
    pub fn new(config: SchemalaneConfig) -> Self {
        Self {
            config,
            rust_migrations: HashMap::new(),
        }
    }

    pub const fn config(&self) -> &SchemalaneConfig {
        &self.config
    }

    pub fn register_rust_migration<S>(&mut self, script: S, migration: RustMigrationExecutor)
    where
        S: Into<String>,
    {
        self.rust_migrations
            .insert(normalize_script_key(script.into()), migration);
    }

    #[must_use]
    pub fn with_rust_migration<S>(mut self, script: S, migration: RustMigrationExecutor) -> Self
    where
        S: Into<String>,
    {
        self.register_rust_migration(script, migration);
        self
    }

    #[must_use]
    pub fn with_rust_migrations<I, S>(mut self, migrations: I) -> Self
    where
        I: IntoIterator<Item = (S, RustMigrationExecutor)>,
        S: Into<String>,
    {
        for (script, migration) in migrations {
            self.register_rust_migration(script, migration);
        }
        self
    }

    pub async fn up(&self, pool: &Pool) -> Result<RunReport, SchemalaneError> {
        self.up_with_observer(pool, &NoopMigrationObserver).await
    }

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
        self.with_advisory_lock(pool, async {
            let client = pool.get().await?;
            self.ensure_target_schema(&client).await?;
            self.ensure_history_table(&client).await?;
            let installed_by = self.resolve_installed_by(&client).await?;
            let mut history = self.load_history(&client).await?;
            Self::ensure_no_blocking_history(&migrations, &history)?;

            let mut report = RunReport::default();
            let total_to_apply = migrations
                .iter()
                .filter(|migration| !is_applied_success(migration, &history))
                .count();
            let mut applied_index = 0usize;

            for migration in &migrations {
                if is_applied_success(migration, &history) {
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
                let run_result = self.apply_migration(pool, migration, observer).await;
                let execution_time_ms = millis_i32(started.elapsed().as_millis());

                match run_result {
                    Ok(()) => {
                        let installed_rank = self
                            .insert_history_row(
                                &client,
                                migration,
                                &installed_by,
                                execution_time_ms,
                                true,
                            )
                            .await?;
                        history.push(HistoryRow::from_migration(
                            migration,
                            execution_time_ms,
                            true,
                            installed_rank,
                        ));
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

                        // MixedStatements is a validation error — do not record a
                        // failed history row because the migration never executed.
                        if !matches!(err, SchemalaneError::MixedStatements { .. })
                            && let Err(insert_err) = self
                                .insert_history_row(
                                &client,
                                migration,
                                &installed_by,
                                execution_time_ms,
                                false,
                            )
                            .await
                        {
                            error_message = format!(
                                "{error_message} (additionally: failed to record failed history row: {insert_err})"
                            );
                        }

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
        })
        .await
    }

    pub async fn status(&self, pool: &Pool) -> Result<StatusReport, SchemalaneError> {
        let client = pool.get().await?;
        let migrations = self.discover_migrations()?;

        let history = if self.history_table_exists(&client).await? {
            self.load_history(&client).await?
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

    pub async fn fresh(&self, pool: &Pool, confirmed: bool) -> Result<RunReport, SchemalaneError> {
        self.fresh_with_observer(pool, confirmed, &NoopMigrationObserver)
            .await
    }

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

        self.with_advisory_lock(pool, async {
            let client = pool.get().await?;
            self.reset_target_schema(&client).await?;
            self.ensure_history_table(&client).await?;

            let installed_by = self.resolve_installed_by(&client).await?;
            let mut report = RunReport::default();
            let total_to_apply = migrations.len();

            for (index, migration) in migrations.iter().enumerate() {
                let migration_info = migration.info();
                observer.on_migration_start(&MigrationStarted {
                    migration: migration_info.clone(),
                    index: index + 1,
                    total: total_to_apply,
                });

                let started = Instant::now();
                let run_result = self.apply_migration(pool, migration, observer).await;
                let execution_time_ms = millis_i32(started.elapsed().as_millis());

                match run_result {
                    Ok(()) => {
                        self.insert_history_row(
                            &client,
                            migration,
                            &installed_by,
                            execution_time_ms,
                            true,
                        )
                        .await?;
                        report.applied.push(AppliedMigration {
                            version: migration.version_text.clone(),
                            description: migration.description_display.clone(),
                            migration_type: migration.migration_type.as_history_type().to_owned(),
                            script: migration.script.clone(),
                            execution_time_ms,
                        });

                        observer.on_migration_finish(&MigrationFinished {
                            migration: migration_info,
                            index: index + 1,
                            total: total_to_apply,
                            execution_time_ms,
                        });
                    }
                    Err(err) => {
                        let mut error_message = err.to_string();

                        if !matches!(err, SchemalaneError::MixedStatements { .. })
                            && let Err(insert_err) = self
                                .insert_history_row(
                                &client,
                                migration,
                                &installed_by,
                                execution_time_ms,
                                false,
                            )
                            .await
                        {
                            error_message = format!(
                                "{error_message} (additionally: failed to record failed history row: {insert_err})"
                            );
                        }

                        observer.on_migration_failed(&MigrationFailed {
                            migration: migration_info,
                            index: index + 1,
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
        })
        .await
    }

    async fn with_advisory_lock<T, F>(&self, pool: &Pool, fut: F) -> Result<T, SchemalaneError>
    where
        F: Future<Output = Result<T, SchemalaneError>>,
    {
        let lock_client = pool.get().await?;
        let lock_id = self.config.advisory_lock_id.unwrap_or_else(|| {
            derive_advisory_lock_id(&self.config.schema, &self.config.history_table)
        });

        lock_client
            .execute("SELECT pg_advisory_lock($1)", &[&lock_id])
            .await?;

        let operation_result = fut.await;

        let unlock_result = lock_client
            .execute("SELECT pg_advisory_unlock($1)", &[&lock_id])
            .await;

        match (operation_result, unlock_result) {
            (Ok(value), Ok(_)) => Ok(value),
            (Err(err), Ok(_)) => Err(err),
            (Ok(_), Err(err)) => Err(SchemalaneError::Db(err)),
            (Err(err), Err(_unlock_err)) => Err(err),
        }
    }

    fn discover_migrations(&self) -> Result<Vec<DiscoveredMigration>, SchemalaneError> {
        let mut migrations = self.discover_sql_migrations()?;
        migrations.extend(self.discover_rust_migrations()?);

        let mut versions: std::collections::BTreeMap<&ParsedVersion, &str> =
            std::collections::BTreeMap::new();
        let mut scripts = BTreeSet::new();

        for migration in &migrations {
            if let Some(existing) = versions.insert(&migration.version, migration.script.as_str()) {
                return Err(SchemalaneError::Validation(format!(
                    "duplicate migration version '{}': '{}' and '{}' resolve to the same version",
                    migration.version_text, existing, migration.script
                )));
            }
            if !scripts.insert(migration.script.clone()) {
                return Err(SchemalaneError::Validation(format!(
                    "duplicate migration script '{}'",
                    migration.script
                )));
            }
        }

        migrations.sort_by(|a, b| {
            a.version
                .cmp(&b.version)
                .then_with(|| a.script.cmp(&b.script))
        });
        Ok(migrations)
    }

    fn discover_sql_migrations(&self) -> Result<Vec<DiscoveredMigration>, SchemalaneError> {
        if !self.config.migrations_dir.exists() {
            return Err(SchemalaneError::Validation(format!(
                "migrations directory not found: {}",
                self.config.migrations_dir.display()
            )));
        }

        let mut migrations = Vec::new();
        for entry in std::fs::read_dir(&self.config.migrations_dir)? {
            let entry = entry?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }

            if !path
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("sql"))
            {
                continue;
            }

            let file_name = path.file_name().and_then(|n| n.to_str()).ok_or_else(|| {
                SchemalaneError::Validation("non-utf8 migration filename".to_owned())
            })?;

            let (version_text, parsed_version, description) = parse_sql_filename(file_name)?;
            let content = std::fs::read(&path)?;
            let checksum = Some(calculate_checksum(file_name, &content)?);
            let description_display = description.replace('_', " ");

            migrations.push(DiscoveredMigration {
                version: parsed_version,
                version_text,
                description_display,
                script: file_name.to_owned(),
                checksum,
                migration_type: MigrationType::Sql,
                source: MigrationSource::SqlFile(path),
            });
        }

        Ok(migrations)
    }

    fn discover_rust_migrations(&self) -> Result<Vec<DiscoveredMigration>, SchemalaneError> {
        let mut migrations = Vec::new();

        for entry in std::fs::read_dir(&self.config.migrations_dir)? {
            let entry = entry?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }

            if !path
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("rs"))
            {
                continue;
            }

            let file_name = path.file_name().and_then(|n| n.to_str()).ok_or_else(|| {
                SchemalaneError::Validation("non-utf8 migration filename".to_owned())
            })?;

            let (version_text, parsed_version, description) = parse_rust_filename(file_name)?;
            let content = std::fs::read(&path)?;
            let checksum = Some(calculate_checksum(file_name, &content)?);
            let description_display = description.replace('_', " ");

            migrations.push(DiscoveredMigration {
                version: parsed_version,
                version_text,
                description_display,
                script: file_name.to_owned(),
                checksum,
                migration_type: MigrationType::Rust,
                source: MigrationSource::RustFile(path),
            });
        }

        Ok(migrations)
    }

    fn ensure_rust_executors_registered(
        &self,
        migrations: &[DiscoveredMigration],
    ) -> Result<(), SchemalaneError> {
        let mut missing_scripts = Vec::new();

        for migration in migrations {
            if migration.migration_type == MigrationType::Rust
                && !self.rust_migrations.contains_key(migration.script.as_str())
            {
                missing_scripts.push(migration.script.clone());
            }
        }

        if missing_scripts.is_empty() {
            Ok(())
        } else {
            missing_scripts.sort();
            Err(SchemalaneError::Validation(format!(
                "missing Rust migration executor(s) for script(s): {}",
                missing_scripts.join(", ")
            )))
        }
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
        pool: &Pool,
        migration: &DiscoveredMigration,
        observer: &O,
    ) -> Result<(), SchemalaneError>
    where
        O: MigrationObserver + ?Sized,
    {
        let migration_info = migration.info();

        match &migration.source {
            MigrationSource::SqlFile(path) => {
                let sql = std::fs::read_to_string(path).map_err(|err| {
                    SchemalaneError::Validation(format!(
                        "failed to read SQL migration {}: {err}",
                        path.display()
                    ))
                })?;
                let mut client = pool.get().await?;
                self.set_search_path(&client).await?;
                execute_sql_migration(&mut client, &sql, &migration_info, observer).await
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
                let mut client = pool.get().await?;
                self.set_search_path(&client).await?;
                execute_rust_migration(&mut client, executor)
                    .await
                    .map_err(SchemalaneError::Db)
            }
        }
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

    async fn ensure_history_table(&self, client: &Client) -> Result<(), SchemalaneError> {
        let table = qualified_table(&self.config.schema, &self.config.history_table);
        let success_idx = quote_ident(&format!("{}_s_idx", self.config.history_table));

        let ddl = format!(
            "\
CREATE TABLE IF NOT EXISTS {table} (\
\"installed_rank\" INTEGER NOT NULL,\
\"version\" VARCHAR(50),\
\"description\" VARCHAR(200) NOT NULL,\
\"type\" VARCHAR(20) NOT NULL,\
\"script\" VARCHAR(1000) NOT NULL,\
\"checksum\" INTEGER,\
\"installed_by\" VARCHAR(100) NOT NULL,\
\"installed_on\" TIMESTAMP NOT NULL DEFAULT now(),\
\"execution_time\" INTEGER NOT NULL,\
\"success\" BOOLEAN NOT NULL,\
CONSTRAINT {pk} PRIMARY KEY (\"installed_rank\")\
);\
CREATE INDEX IF NOT EXISTS {success_idx} ON {table} (\"success\");",
            pk = quote_ident(&format!("{}_pk", self.config.history_table)),
        );

        client.batch_execute(&ddl).await?;
        Ok(())
    }

    async fn history_table_exists(&self, client: &Client) -> Result<bool, SchemalaneError> {
        let regclass = qualified_table(&self.config.schema, &self.config.history_table);
        let row = client
            .query_one("SELECT to_regclass($1) IS NOT NULL AS exists", &[&regclass])
            .await?;

        let exists: bool = row.get("exists");
        Ok(exists)
    }

    async fn load_history(&self, client: &Client) -> Result<Vec<HistoryRow>, SchemalaneError> {
        let table = qualified_table(&self.config.schema, &self.config.history_table);
        let query = format!(
            "SELECT \"installed_rank\", \"version\", \"description\", \"type\", \"script\", \"checksum\", \"installed_by\", \"installed_on\"::text AS \"installed_on\", \"execution_time\", \"success\" FROM {table} ORDER BY \"installed_rank\" ASC"
        );

        let rows = client.query(&query, &[]).await?;

        let mut history = Vec::with_capacity(rows.len());
        for row in rows {
            history.push(HistoryRow {
                installed_rank: row.get("installed_rank"),
                version: row.get("version"),
                description: row.get("description"),
                migration_type: row.get("type"),
                script: row.get("script"),
                checksum: row.get("checksum"),
                installed_on: row.get("installed_on"),
                execution_time: row.get("execution_time"),
                success: row.get("success"),
            });
        }

        Ok(history)
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

    async fn next_installed_rank(&self, client: &Client) -> Result<i32, SchemalaneError> {
        let table = qualified_table(&self.config.schema, &self.config.history_table);
        let query =
            format!("SELECT COALESCE(MAX(\"installed_rank\"), 0) + 1 AS next_rank FROM {table}");

        let row = client.query_one(&query, &[]).await?;
        let next_rank: i32 = row.get("next_rank");
        Ok(next_rank)
    }

    async fn insert_history_row(
        &self,
        client: &Client,
        migration: &DiscoveredMigration,
        installed_by: &str,
        execution_time: i32,
        success: bool,
    ) -> Result<i32, SchemalaneError> {
        let installed_rank = self.next_installed_rank(client).await?;
        let table = qualified_table(&self.config.schema, &self.config.history_table);

        let sql = format!(
            "INSERT INTO {table} (\"installed_rank\", \"version\", \"description\", \"type\", \"script\", \"checksum\", \"installed_by\", \"execution_time\", \"success\") VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)"
        );

        let version: Option<&str> = Some(migration.version_text.as_str());
        let migration_type = migration.migration_type.as_history_type();
        let params: Vec<&(dyn ToSql + Sync)> = vec![
            &installed_rank,
            &version,
            &migration.description_display,
            &migration_type,
            &migration.script,
            &migration.checksum,
            &installed_by,
            &execution_time,
            &success,
        ];

        client.execute(&sql, &params).await?;
        Ok(installed_rank)
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

fn build_status_report(
    schema: &str,
    history_table: &str,
    migrations: &[DiscoveredMigration],
    history: &[HistoryRow],
) -> StatusReport {
    let latest = latest_history_by_script(history);
    let local_by_script: HashMap<&str, &DiscoveredMigration> =
        migrations.iter().map(|m| (m.script.as_str(), m)).collect();

    let mut entries = Vec::new();

    for migration in migrations {
        let entry = match latest.get(migration.script.as_str()) {
            Some(row) if !row.success => StatusEntry {
                version: row.version.clone(),
                description: row.description.clone(),
                migration_type: row.migration_type.clone(),
                script: row.script.clone(),
                checksum: row.checksum,
                installed_rank: Some(row.installed_rank),
                installed_on: Some(row.installed_on.clone()),
                execution_time_ms: Some(row.execution_time),
                state: MigrationState::Failed,
            },
            Some(row) if row.checksum != migration.checksum => StatusEntry {
                version: Some(migration.version_text.clone()),
                description: migration.description_display.clone(),
                migration_type: migration.migration_type.as_history_type().to_owned(),
                script: migration.script.clone(),
                checksum: migration.checksum,
                installed_rank: Some(row.installed_rank),
                installed_on: Some(row.installed_on.clone()),
                execution_time_ms: Some(row.execution_time),
                state: MigrationState::ChecksumMismatch,
            },
            Some(row) => StatusEntry {
                version: Some(migration.version_text.clone()),
                description: migration.description_display.clone(),
                migration_type: migration.migration_type.as_history_type().to_owned(),
                script: migration.script.clone(),
                checksum: migration.checksum,
                installed_rank: Some(row.installed_rank),
                installed_on: Some(row.installed_on.clone()),
                execution_time_ms: Some(row.execution_time),
                state: MigrationState::Success,
            },
            None => StatusEntry {
                version: Some(migration.version_text.clone()),
                description: migration.description_display.clone(),
                migration_type: migration.migration_type.as_history_type().to_owned(),
                script: migration.script.clone(),
                checksum: migration.checksum,
                installed_rank: None,
                installed_on: None,
                execution_time_ms: None,
                state: MigrationState::Pending,
            },
        };

        entries.push(entry);
    }

    for row in latest.values() {
        if row.success && !local_by_script.contains_key(row.script.as_str()) {
            entries.push(StatusEntry {
                version: row.version.clone(),
                description: row.description.clone(),
                migration_type: row.migration_type.clone(),
                script: row.script.clone(),
                checksum: row.checksum,
                installed_rank: Some(row.installed_rank),
                installed_on: Some(row.installed_on.clone()),
                execution_time_ms: Some(row.execution_time),
                state: MigrationState::Missing,
            });
        }
    }

    entries.sort_by(|a, b| {
        let a_version = a
            .version
            .as_ref()
            .and_then(|v| ParsedVersion::parse(v).ok());
        let b_version = b
            .version
            .as_ref()
            .and_then(|v| ParsedVersion::parse(v).ok());

        a_version
            .cmp(&b_version)
            .then_with(|| a.script.cmp(&b.script))
            .then_with(|| a.installed_rank.cmp(&b.installed_rank))
    });

    let mut summary = StatusSummary::default();
    for entry in &entries {
        match entry.state {
            MigrationState::Success => summary.success += 1,
            MigrationState::Pending => summary.pending += 1,
            MigrationState::Failed => summary.failed += 1,
            MigrationState::Missing => summary.missing += 1,
            MigrationState::ChecksumMismatch => summary.checksum_mismatch += 1,
        }
    }

    StatusReport {
        schema: schema.to_owned(),
        history_table: history_table.to_owned(),
        migrations: entries,
        summary,
    }
}

/// Transaction mode resolved for a SQL migration file by analysing its statements.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SqlTransactionMode {
    /// All statements are safe to run inside a transaction.
    Transactional,
    /// All statements must run **outside** a transaction (e.g. `CREATE INDEX CONCURRENTLY`).
    NonTransactional,
}

/// A parsed SQL statement from a migration file, produced by the `PostgreSQL` parser via
/// `pg_query`.
struct ParsedSqlStatement {
    /// The raw SQL text to execute.
    sql: String,
    /// Source line number in the migration file (1-based).
    source_line: u64,
    /// A compact human-readable preview for observer events.
    preview: String,
    /// The protobuf parse node (used for AST-based non-transactional detection).
    node: Option<protobuf::Node>,
}

/// Parses a SQL migration file into a list of [`ParsedSqlStatement`]s using the real
/// `PostgreSQL` parser via `pg_query`.
fn parse_sql_migration(sql: &str) -> Result<Vec<ParsedSqlStatement>, SchemalaneError> {
    let stmts = pg_query::split_with_parser(sql).map_err(|err| {
        SchemalaneError::Validation(format!("failed to split SQL migration: {err}"))
    })?;

    let mut result = Vec::with_capacity(stmts.len());

    for stmt_sql in stmts {
        let trimmed = stmt_sql.trim();
        if trimmed.is_empty() {
            continue;
        }

        let parsed = pg_query::parse(trimmed).map_err(|err| {
            SchemalaneError::Validation(format!("failed to parse SQL statement: {err}"))
        })?;

        let source_line = offset_to_line(sql, trimmed);
        let preview = pg_query_fmt::preview::statement_preview(&parsed);
        let node = parsed
            .protobuf
            .stmts
            .into_iter()
            .next()
            .and_then(|raw_stmt| raw_stmt.stmt.map(|n| *n));

        result.push(ParsedSqlStatement {
            sql: trimmed.to_owned(),
            source_line,
            preview,
            node,
        });
    }

    Ok(result)
}

/// Converts a character offset within the full SQL text to a 1-based line number.
fn offset_to_line(full_sql: &str, stmt_slice: &str) -> u64 {
    let offset = stmt_slice.as_ptr() as usize - full_sql.as_ptr() as usize;
    let prefix = &full_sql[..offset.min(full_sql.len())];
    (prefix.chars().filter(|&c| c == '\n').count() + 1) as u64
}

/// Determines whether a parsed SQL statement cannot execute inside a `PostgreSQL` transaction
/// block.
///
/// Uses the protobuf AST node from `pg_query` (the real `PostgreSQL` parser) for detection.
/// This mirrors Flyway's `PostgreSQLParser.detectCanExecuteInTransaction()`.
fn is_non_transactional(stmt: &ParsedSqlStatement) -> bool {
    use protobuf::node::Node;

    let Some(ref node) = stmt.node else {
        return false;
    };
    let Some(ref node_inner) = node.node else {
        return false;
    };

    match node_inner {
        // CREATE [UNIQUE] INDEX CONCURRENTLY
        Node::IndexStmt(idx) => idx.concurrent,

        // DROP INDEX CONCURRENTLY (generic DropStmt with concurrent flag)
        Node::DropStmt(drop) => drop.concurrent,

        // VACUUM is non-transactional, but standalone ANALYZE (also a VacuumStmt
        // with is_vacuumcmd=false) CAN run inside a transaction.
        Node::VacuumStmt(v) => v.is_vacuumcmd,

        // REINDEX SCHEMA / DATABASE / SYSTEM are non-transactional.
        // REINDEX TABLE / INDEX can run inside a transaction (Flyway: ^REINDEX (SCHEMA|DATABASE|SYSTEM)).
        Node::ReindexStmt(r) => {
            let kind = r.kind;
            kind == protobuf::ReindexObjectType::ReindexObjectSchema as i32
                || kind == protobuf::ReindexObjectType::ReindexObjectDatabase as i32
                || kind == protobuf::ReindexObjectType::ReindexObjectSystem as i32
        }

        // Only DISCARD ALL is non-transactional (Flyway: ^DISCARD ALL).
        // DISCARD PLANS / SEQUENCES / TEMP can run inside a transaction.
        Node::DiscardStmt(d) => d.target == protobuf::DiscardMode::DiscardAll as i32,

        // Always non-transactional statements
        Node::AlterSystemStmt(_)
        | Node::CreatedbStmt(_)
        | Node::CreateTableSpaceStmt(_)
        | Node::CreateSubscriptionStmt(_)
        | Node::DropdbStmt(_)
        | Node::DropTableSpaceStmt(_)
        | Node::DropSubscriptionStmt(_) => true,

        _ => false,
    }
}

/// Determines the transaction mode for a SQL migration by examining its parsed statements.
///
/// If **all** statements are transactional -> `Transactional`.
/// If **all** statements are non-transactional -> `NonTransactional`.
/// If the file **mixes** both kinds -> returns an `Err` with the first offending line.
///
/// This replicates Flyway's default behaviour (`mixed = false`).
fn resolve_sql_transaction_mode(
    statements: &[ParsedSqlStatement],
    script: &str,
) -> Result<SqlTransactionMode, SchemalaneError> {
    let mut has_transactional = false;
    let mut has_non_transactional = false;
    let mut first_non_transactional_line: Option<u64> = None;
    let mut first_transactional_line: Option<u64> = None;

    for stmt in statements {
        let line = stmt.source_line;

        if is_non_transactional(stmt) {
            has_non_transactional = true;
            if first_non_transactional_line.is_none() {
                first_non_transactional_line = Some(line);
            }
        } else {
            has_transactional = true;
            if first_transactional_line.is_none() {
                first_transactional_line = Some(line);
            }
        }
    }

    if has_transactional && has_non_transactional {
        let offending_line = if first_transactional_line < first_non_transactional_line {
            first_non_transactional_line.unwrap_or(1)
        } else {
            first_transactional_line.unwrap_or(1)
        };
        return Err(SchemalaneError::MixedStatements {
            script: script.to_owned(),
            line: offending_line,
        });
    }

    if has_non_transactional {
        Ok(SqlTransactionMode::NonTransactional)
    } else {
        Ok(SqlTransactionMode::Transactional)
    }
}

async fn execute_sql_migration<O>(
    client: &mut Client,
    sql: &str,
    migration: &MigrationInfo,
    observer: &O,
) -> Result<(), SchemalaneError>
where
    O: MigrationObserver + ?Sized,
{
    let statements = parse_sql_migration(sql)?;
    let total_statements = statements.len();

    let tx_mode = resolve_sql_transaction_mode(&statements, &migration.script)?;

    match tx_mode {
        SqlTransactionMode::Transactional => {
            let txn = client.transaction().await?;
            for (index, stmt) in statements.iter().enumerate() {
                if let Err(err) =
                    execute_statement_txn(&txn, stmt, index, total_statements, migration, observer)
                        .await
                {
                    let _ = txn.rollback().await;
                    return Err(SchemalaneError::Db(err));
                }
            }
            txn.commit().await?;
        }
        SqlTransactionMode::NonTransactional => {
            for (index, stmt) in statements.iter().enumerate() {
                execute_statement_client(
                    client,
                    stmt,
                    index,
                    total_statements,
                    migration,
                    observer,
                )
                .await
                .map_err(SchemalaneError::Db)?;
            }
        }
    }

    Ok(())
}

/// Executes a single [`ParsedSqlStatement`] inside a transaction, emitting observer events.
async fn execute_statement_txn<O>(
    txn: &Transaction<'_>,
    stmt: &ParsedSqlStatement,
    index: usize,
    total_statements: usize,
    migration: &MigrationInfo,
    observer: &O,
) -> Result<(), tokio_postgres::Error>
where
    O: MigrationObserver + ?Sized,
{
    let source_line = Some(stmt.source_line);

    observer.on_sql_statement_start(&SqlStatementStarted {
        migration: migration.clone(),
        statement_index: index + 1,
        total_statements,
        statement_preview: stmt.preview.clone(),
        statement: stmt.sql.clone(),
        source_line,
    });

    let started = Instant::now();
    match txn.batch_execute(&stmt.sql).await {
        Ok(()) => {
            observer.on_sql_statement_finish(&SqlStatementFinished {
                migration: migration.clone(),
                statement_index: index + 1,
                total_statements,
                statement_preview: stmt.preview.clone(),
                statement: stmt.sql.clone(),
                execution_time_ms: millis_i32(started.elapsed().as_millis()),
                source_line,
            });
            Ok(())
        }
        Err(err) => {
            observer.on_sql_statement_failed(&SqlStatementFailed {
                migration: migration.clone(),
                statement_index: index + 1,
                total_statements,
                statement_preview: stmt.preview.clone(),
                statement: stmt.sql.clone(),
                execution_time_ms: millis_i32(started.elapsed().as_millis()),
                error: err.to_string(),
                source_line,
            });
            Err(err)
        }
    }
}

/// Executes a single [`ParsedSqlStatement`] on a client (outside transaction), emitting
/// observer events.
async fn execute_statement_client<O>(
    client: &Client,
    stmt: &ParsedSqlStatement,
    index: usize,
    total_statements: usize,
    migration: &MigrationInfo,
    observer: &O,
) -> Result<(), tokio_postgres::Error>
where
    O: MigrationObserver + ?Sized,
{
    let source_line = Some(stmt.source_line);

    observer.on_sql_statement_start(&SqlStatementStarted {
        migration: migration.clone(),
        statement_index: index + 1,
        total_statements,
        statement_preview: stmt.preview.clone(),
        statement: stmt.sql.clone(),
        source_line,
    });

    let started = Instant::now();
    match client.batch_execute(&stmt.sql).await {
        Ok(()) => {
            observer.on_sql_statement_finish(&SqlStatementFinished {
                migration: migration.clone(),
                statement_index: index + 1,
                total_statements,
                statement_preview: stmt.preview.clone(),
                statement: stmt.sql.clone(),
                execution_time_ms: millis_i32(started.elapsed().as_millis()),
                source_line,
            });
            Ok(())
        }
        Err(err) => {
            observer.on_sql_statement_failed(&SqlStatementFailed {
                migration: migration.clone(),
                statement_index: index + 1,
                total_statements,
                statement_preview: stmt.preview.clone(),
                statement: stmt.sql.clone(),
                execution_time_ms: millis_i32(started.elapsed().as_millis()),
                error: err.to_string(),
                source_line,
            });
            Err(err)
        }
    }
}

async fn execute_rust_migration(
    client: &mut Client,
    migration: &RustMigrationExecutor,
) -> Result<(), tokio_postgres::Error> {
    match migration.transaction_mode() {
        RustTransactionMode::NoTransaction => migration.up(client).await,
        RustTransactionMode::Transaction => {
            client.batch_execute("BEGIN").await?;
            match migration.up(client).await {
                Ok(()) => client.batch_execute("COMMIT").await,
                Err(err) => {
                    let _ = client.batch_execute("ROLLBACK").await;
                    Err(err)
                }
            }
        }
    }
}

fn is_applied_success(migration: &DiscoveredMigration, history: &[HistoryRow]) -> bool {
    latest_history_by_script(history)
        .get(migration.script.as_str())
        .is_some_and(|row| row.success && row.checksum == migration.checksum)
}

fn latest_history_by_script(history: &[HistoryRow]) -> HashMap<&str, &HistoryRow> {
    let mut latest = HashMap::new();
    for row in history {
        latest.insert(row.script.as_str(), row);
    }
    latest
}

fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn qualified_table(schema: &str, table: &str) -> String {
    format!("{}.{}", quote_ident(schema), quote_ident(table))
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "guarded by the preceding bounds check"
)]
const fn millis_i32(millis: u128) -> i32 {
    if millis > i32::MAX as u128 {
        i32::MAX
    } else {
        millis as i32
    }
}

/// Flyway-compatible checksum: CRC-32 over each line's UTF-8 bytes, with
/// line terminators excluded. Matches `org.flywaydb.core.internal.util.ChecksumCalculator`,
/// which reads via `BufferedReader.readLine()` (splits on `\n`, `\r\n`, or `\r`).
///
/// Errors if the content is not valid UTF-8 — Flyway uses a UTF-8 `BufferedReader`
/// by default and surfaces `MalformedInputException` rather than silently
/// hashing a substituted value. Hashing zero bytes (i.e. checksum `0`) for an
/// invalid file would silently misclassify drift state for every subsequent
/// `status` / `up` run.
fn calculate_checksum(script: &str, bytes: &[u8]) -> Result<i32, SchemalaneError> {
    let text = std::str::from_utf8(bytes).map_err(|err| {
        SchemalaneError::Validation(format!(
            "migration {script}: content is not valid UTF-8 (invalid byte at offset {}): {err}",
            err.valid_up_to()
        ))
    })?;
    let mut hasher = Hasher::new();
    // `str::lines()` matches BufferedReader.readLine() for files that don't
    // contain lone `\r` characters: splits on `\n` or `\r\n`, excludes the
    // terminator, no trailing empty line for files that end with a newline.
    for line in text.lines() {
        hasher.update(line.as_bytes());
    }
    Ok(i32::from_be_bytes(hasher.finalize().to_be_bytes()))
}

fn write_init_file(
    path: &Path,
    content: &str,
    force: bool,
    created: &mut Vec<PathBuf>,
    overwritten: &mut Vec<PathBuf>,
) -> Result<(), SchemalaneError> {
    if path.exists() {
        if !force {
            return Err(SchemalaneError::Validation(format!(
                "refusing to overwrite existing file: {} (use --force to overwrite)",
                path.display()
            )));
        }
        overwritten.push(path.to_path_buf());
    } else {
        created.push(path.to_path_buf());
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, content)?;
    Ok(())
}

fn init_template_files(package_name: &str, lib_ident: &str) -> Vec<(PathBuf, String)> {
    vec![
        (
            PathBuf::from("Cargo.toml"),
            INIT_CARGO_TOML_TEMPLATE.replace("__PACKAGE_NAME__", package_name),
        ),
        (
            PathBuf::from("README.md"),
            INIT_PROJECT_README_TEMPLATE.to_owned(),
        ),
        (
            PathBuf::from(".gitignore"),
            INIT_GITIGNORE_TEMPLATE.to_owned(),
        ),
        (PathBuf::from("build.rs"), INIT_BUILD_RS_TEMPLATE.to_owned()),
        (
            PathBuf::from("src/main.rs"),
            INIT_MAIN_RS_TEMPLATE
                .replace("__PACKAGE_NAME__", package_name)
                .replace("__LIB_IDENT__", lib_ident),
        ),
        (PathBuf::from("src/lib.rs"), INIT_LIB_RS_TEMPLATE.to_owned()),
        (
            PathBuf::from("migrations/V1__create_cake_table.sql"),
            INIT_SQL_MIGRATION_TEMPLATE.to_owned(),
        ),
        (
            PathBuf::from("migrations/V2__seed_cake_table.rs"),
            INIT_RUST_MIGRATION_TEMPLATE.to_owned(),
        ),
    ]
}

fn sanitize_package_name(raw: &str) -> String {
    let mut package = String::new();
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            package.push(ch.to_ascii_lowercase());
        } else {
            package.push('_');
        }
    }

    package = package.trim_matches(['-', '_']).to_owned();
    if package.is_empty() {
        package.push_str("migration");
    }

    if package
        .chars()
        .next()
        .is_some_and(|first| first.is_ascii_digit())
    {
        package = format!("m_{package}");
    }

    package
}

fn package_to_lib_ident(package_name: &str) -> String {
    package_name.replace('-', "_")
}

fn normalize_script_key(script: String) -> String {
    Path::new(&script)
        .file_name()
        .and_then(|name| name.to_str())
        .map(ToOwned::to_owned)
        .unwrap_or(script)
}

const INIT_CARGO_TOML_TEMPLATE: &str = r#"[package]
name = "__PACKAGE_NAME__"
version = "0.1.0"
edition = "2024"
publish = false

[dependencies]
schemalane-core = "0.1"
schemalane-cli = "0.1"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
tokio-postgres = "0.7"

# Developing against a local schemalane checkout? Use path dependencies:
# schemalane-core = { path = "../schemalane-core" }
# schemalane-cli = { path = "../schemalane-cli" }
"#;

const INIT_PROJECT_README_TEMPLATE: &str = r#"# Migration Crate

Generated by `schemalane migrate init`.

This crate runs Schemalane migrations programmatically and exposes a small CLI:

- `up`
- `status`
- `fresh`

Rust migration registration is automatic via `embed_migrations!("./migrations")`.

Run from this crate:

```sh
DATABASE_URL="postgres://…" cargo run -- up
```

Run from parent project:

```sh
DATABASE_URL="postgres://…" cargo run --manifest-path ./migration/Cargo.toml -- up
```
"#;

const INIT_GITIGNORE_TEMPLATE: &str = "/target\n.env\n";

const INIT_BUILD_RS_TEMPLATE: &str = r#"fn main() {
    // Re-run when migrations are added or removed so embed_migrations! re-scans.
    println!("cargo::rerun-if-changed=migrations");
}
"#;

const INIT_MAIN_RS_TEMPLATE: &str = r"use __LIB_IDENT__::embedded;

#[tokio::main]
async fn main() {
    embedded::migrations::runner().run().await;
}
";

const INIT_LIB_RS_TEMPLATE: &str = r#"pub mod embedded {
    use schemalane_core::embed_migrations;

    embed_migrations!("./migrations");
}
"#;

const INIT_SQL_MIGRATION_TEMPLATE: &str = r"CREATE TABLE cake (
    id SERIAL PRIMARY KEY,
    name TEXT NOT NULL
);
";

const INIT_RUST_MIGRATION_TEMPLATE: &str = r#"use tokio_postgres::Client;

pub async fn migration(client: &Client) -> Result<(), tokio_postgres::Error> {
    client
        .batch_execute(
            r"
INSERT INTO cake(name) VALUES ('vanilla');
INSERT INTO cake(name) VALUES ('chocolate');
",
        )
        .await?;
    Ok(())
}
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MigrationType {
    Sql,
    Rust,
}

impl MigrationType {
    const fn as_history_type(self) -> &'static str {
        match self {
            Self::Sql => "SQL",
            Self::Rust => "RUST",
        }
    }
}

#[derive(Clone)]
struct DiscoveredMigration {
    version: ParsedVersion,
    version_text: String,
    description_display: String,
    script: String,
    checksum: Option<i32>,
    migration_type: MigrationType,
    source: MigrationSource,
}

impl DiscoveredMigration {
    fn info(&self) -> MigrationInfo {
        MigrationInfo {
            version: self.version_text.clone(),
            description: self.description_display.clone(),
            migration_type: self.migration_type.as_history_type().to_owned(),
            script: self.script.clone(),
        }
    }
}

#[derive(Clone)]
enum MigrationSource {
    SqlFile(PathBuf),
    RustFile(PathBuf),
}

#[derive(Debug, Clone)]
struct HistoryRow {
    installed_rank: i32,
    version: Option<String>,
    description: String,
    migration_type: String,
    script: String,
    checksum: Option<i32>,
    installed_on: String,
    execution_time: i32,
    success: bool,
}

impl HistoryRow {
    fn from_migration(
        migration: &DiscoveredMigration,
        execution_time: i32,
        success: bool,
        installed_rank: i32,
    ) -> Self {
        Self {
            installed_rank,
            version: Some(migration.version_text.clone()),
            description: migration.description_display.clone(),
            migration_type: migration.migration_type.as_history_type().to_owned(),
            script: migration.script.clone(),
            checksum: migration.checksum,
            installed_on: String::new(),
            execution_time,
            success,
        }
    }
}

pub fn format_status_table(report: &StatusReport) -> String {
    let mut lines = Vec::new();
    lines.push(format!(
        "schema={}, history_table={}",
        report.schema, report.history_table
    ));
    lines.push(
        "version | description | type | script | state | rank | execution_time_ms".to_owned(),
    );
    lines.push(
        "--------|-------------|------|--------|-------|------|------------------".to_owned(),
    );

    for migration in &report.migrations {
        lines.push(format!(
            "{} | {} | {} | {} | {:?} | {} | {}",
            migration.version.as_deref().unwrap_or("-"),
            migration.description,
            migration.migration_type,
            migration.script,
            migration.state,
            migration
                .installed_rank
                .map_or_else(|| "-".to_owned(), |v| v.to_string()),
            migration
                .execution_time_ms
                .map_or_else(|| "-".to_owned(), |v| v.to_string()),
        ));
    }

    lines.push(String::new());
    lines.push(format!(
        "summary: success={}, pending={}, failed={}, missing={}, checksum_mismatch={}",
        report.summary.success,
        report.summary.pending,
        report.summary.failed,
        report.summary.missing,
        report.summary.checksum_mismatch
    ));

    lines.join("\n")
}

pub const fn should_fail_on_pending(report: &StatusReport) -> Result<(), SchemalaneError> {
    if report.summary.pending > 0 {
        Err(SchemalaneError::PendingMigrations(report.summary.pending))
    } else {
        Ok(())
    }
}

pub fn migrations_dir_exists(path: &Path) -> bool {
    path.exists()
}

#[cfg(test)]
mod tests {
    use super::{
        SchemalaneConfig, SchemalaneError, SchemalaneMigrator, SqlTransactionMode,
        calculate_checksum, derive_advisory_lock_id, init_migration_project, is_non_transactional,
        parse_sql_migration, resolve_sql_transaction_mode,
    };
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn delegated_error_exit_code_is_forwarded_verbatim() {
        for code in [1, 2, 3, 4, 5, 6, 7, 42] {
            assert_eq!(SchemalaneError::Delegated { code }.exit_code(), code);
        }
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
            statements[0]
                .preview
                .to_uppercase()
                .contains("CREATE TABLE")
                || statements[0].preview.contains("CreateStmt"),
            "first statement should be CREATE TABLE, got: {}",
            statements[0].preview,
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
