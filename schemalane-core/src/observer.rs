use crate::StatusReport;

#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct MigrationInfo {
    pub version: String,
    pub description: String,
    pub migration_type: String,
    pub script: String,
}
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct MigrationStarted {
    pub migration: MigrationInfo,
    pub index: usize,
    pub total: usize,
}
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct MigrationFinished {
    pub migration: MigrationInfo,
    pub index: usize,
    pub total: usize,
    pub execution_time_ms: i32,
}
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct MigrationFailed {
    pub migration: MigrationInfo,
    pub index: usize,
    pub total: usize,
    pub execution_time_ms: i32,
    pub error: String,
}
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct SqlStatementStarted {
    pub migration: MigrationInfo,
    pub statement_index: usize,
    pub total_statements: usize,
    pub statement: String,
    pub source_line: Option<u64>,
}
#[derive(Debug, Clone)]
#[non_exhaustive]
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
pub struct SqlStatementFailed {
    pub migration: MigrationInfo,
    pub statement_index: usize,
    pub total_statements: usize,
    pub statement: String,
    pub execution_time_ms: i32,
    pub error: String,
    pub source_line: Option<u64>,
}

pub trait MigrationObserver: Send + Sync {
    fn on_run_planned(&self, _report: &StatusReport) {}
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
