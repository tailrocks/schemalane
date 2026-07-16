use crate::StatusReport;

#[derive(Debug, Clone)]
#[non_exhaustive]
/// Stable identity and display metadata for one migration.
pub struct MigrationInfo {
    pub version: String,
    pub description: String,
    pub migration_type: String,
    pub script: String,
}
#[derive(Debug, Clone)]
#[non_exhaustive]
/// Emitted immediately before a migration begins.
pub struct MigrationStarted {
    pub migration: MigrationInfo,
    pub index: usize,
    pub total: usize,
}
#[derive(Debug, Clone)]
#[non_exhaustive]
/// Emitted after a migration and required history write succeed.
pub struct MigrationFinished {
    pub migration: MigrationInfo,
    pub index: usize,
    pub total: usize,
    pub execution_time_ms: i32,
}
#[derive(Debug, Clone)]
#[non_exhaustive]
/// Emitted after migration execution fails.
pub struct MigrationFailed {
    pub migration: MigrationInfo,
    pub index: usize,
    pub total: usize,
    pub execution_time_ms: i32,
    pub error: String,
}
#[derive(Debug, Clone)]
#[non_exhaustive]
/// Emitted before one parsed SQL statement executes.
pub struct SqlStatementStarted {
    pub migration: MigrationInfo,
    pub statement_index: usize,
    pub total_statements: usize,
    pub statement: String,
    pub source_line: Option<u64>,
}
#[derive(Debug, Clone)]
#[non_exhaustive]
/// Emitted after one SQL statement succeeds.
pub struct SqlStatementFinished {
    pub migration: MigrationInfo,
    pub statement_index: usize,
    pub total_statements: usize,
    pub statement: String,
    pub execution_time_ms: i32,
    pub source_line: Option<u64>,
}
#[derive(Debug, Clone)]
#[non_exhaustive]
/// Emitted after one SQL statement fails.
pub struct SqlStatementFailed {
    pub migration: MigrationInfo,
    pub statement_index: usize,
    pub total_statements: usize,
    pub statement: String,
    pub execution_time_ms: i32,
    pub error: String,
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
