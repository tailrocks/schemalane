use crate::StatusReport;

#[derive(Debug, Clone)]
#[non_exhaustive]
/// Stable identity and display metadata for one migration.
pub struct MigrationInfo {
    /// Original version text parsed from the migration filename.
    pub version: String,
    /// Human-readable description parsed from the migration filename.
    pub description: String,
    /// Flyway-compatible history type, such as `SQL` or `RUST`.
    pub migration_type: String,
    /// Migration filename relative to the configured source directory.
    pub script: String,
}
#[derive(Debug, Clone)]
#[non_exhaustive]
/// Emitted immediately before a migration begins.
pub struct MigrationStarted {
    /// Identity of the migration being started.
    pub migration: MigrationInfo,
    /// One-based position among migrations applied by this run.
    pub index: usize,
    /// Total migrations scheduled for this run.
    pub total: usize,
}
#[derive(Debug, Clone)]
#[non_exhaustive]
/// Emitted after a migration and required history write succeed.
pub struct MigrationFinished {
    /// Identity of the migration that completed.
    pub migration: MigrationInfo,
    /// One-based position among migrations applied by this run.
    pub index: usize,
    /// Total migrations scheduled for this run.
    pub total: usize,
    /// Wall-clock execution duration in milliseconds.
    pub execution_time_ms: i32,
}
#[derive(Debug, Clone)]
#[non_exhaustive]
/// Emitted after migration execution fails.
pub struct MigrationFailed {
    /// Identity of the migration that failed.
    pub migration: MigrationInfo,
    /// One-based position among migrations applied by this run.
    pub index: usize,
    /// Total migrations scheduled for this run.
    pub total: usize,
    /// Wall-clock duration before the failure in milliseconds.
    pub execution_time_ms: i32,
    /// Displayable failure message.
    pub error: String,
}
#[derive(Debug, Clone)]
#[non_exhaustive]
/// Emitted before one parsed SQL statement executes.
pub struct SqlStatementStarted {
    /// Identity of the migration containing the statement.
    pub migration: MigrationInfo,
    /// One-based position within the migration's parsed statements.
    pub statement_index: usize,
    /// Total parsed statements in the migration.
    pub total_statements: usize,
    /// Raw SQL text that will execute.
    pub statement: String,
    /// One-based source line, when the parser supplied a location.
    pub source_line: Option<u64>,
}
#[derive(Debug, Clone)]
#[non_exhaustive]
/// Emitted after one SQL statement succeeds.
pub struct SqlStatementFinished {
    /// Identity of the migration containing the statement.
    pub migration: MigrationInfo,
    /// One-based position within the migration's parsed statements.
    pub statement_index: usize,
    /// Total parsed statements in the migration.
    pub total_statements: usize,
    /// Raw SQL text that executed.
    pub statement: String,
    /// Wall-clock statement duration in milliseconds.
    pub execution_time_ms: i32,
    /// One-based source line, when the parser supplied a location.
    pub source_line: Option<u64>,
}
#[derive(Debug, Clone)]
#[non_exhaustive]
/// Emitted after one SQL statement fails.
pub struct SqlStatementFailed {
    /// Identity of the migration containing the statement.
    pub migration: MigrationInfo,
    /// One-based position within the migration's parsed statements.
    pub statement_index: usize,
    /// Total parsed statements in the migration.
    pub total_statements: usize,
    /// Raw SQL text whose execution failed.
    pub statement: String,
    /// Wall-clock duration before the failure in milliseconds.
    pub execution_time_ms: i32,
    /// Displayable failure message.
    pub error: String,
    /// One-based source line, when the parser supplied a location.
    pub source_line: Option<u64>,
}

/// Receives synchronous lifecycle notifications during migration execution.
pub trait MigrationObserver: Send + Sync {
    /// Receives the reconciled plan once, before any pending migration starts.
    fn on_run_planned(&self, _report: &StatusReport) {}
    /// Receives migration-start events.
    fn on_migration_start(&self, _event: &MigrationStarted) {}
    /// Receives successful migration completion events.
    fn on_migration_finish(&self, _event: &MigrationFinished) {}
    /// Receives failed migration completion events.
    fn on_migration_failed(&self, _event: &MigrationFailed) {}
    /// Receives SQL statement-start events.
    fn on_sql_statement_start(&self, _event: &SqlStatementStarted) {}
    /// Receives successful SQL statement completion events.
    fn on_sql_statement_finish(&self, _event: &SqlStatementFinished) {}
    /// Receives failed SQL statement completion events.
    fn on_sql_statement_failed(&self, _event: &SqlStatementFailed) {}
}
#[derive(Debug, Clone, Copy, Default)]
/// Observer implementation that intentionally discards every event.
pub struct NoopMigrationObserver;
impl MigrationObserver for NoopMigrationObserver {}
