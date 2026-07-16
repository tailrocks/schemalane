use serde::Serialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "PascalCase")]
#[non_exhaustive]
pub enum MigrationState {
    Success,
    Pending,
    Failed,
    Missing,
    ChecksumMismatch,
}

#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
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
impl StatusEntry {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        version: Option<String>,
        description: String,
        migration_type: String,
        script: String,
        checksum: Option<i32>,
        installed_rank: Option<i32>,
        installed_on: Option<String>,
        execution_time_ms: Option<i32>,
        state: MigrationState,
    ) -> Self {
        Self {
            version,
            description,
            migration_type,
            script,
            checksum,
            installed_rank,
            installed_on,
            execution_time_ms,
            state,
        }
    }
}

#[derive(Debug, Clone, Serialize, Default)]
#[non_exhaustive]
pub struct StatusSummary {
    pub success: usize,
    pub pending: usize,
    pub failed: usize,
    pub missing: usize,
    pub checksum_mismatch: usize,
}
impl StatusSummary {
    pub const fn new(
        success: usize,
        pending: usize,
        failed: usize,
        missing: usize,
        checksum_mismatch: usize,
    ) -> Self {
        Self {
            success,
            pending,
            failed,
            missing,
            checksum_mismatch,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct StatusReport {
    pub schema: String,
    pub history_table: String,
    pub migrations: Vec<StatusEntry>,
    pub summary: StatusSummary,
}
impl StatusReport {
    pub fn new(
        schema: String,
        history_table: String,
        migrations: Vec<StatusEntry>,
        summary: StatusSummary,
    ) -> Self {
        Self {
            schema,
            history_table,
            migrations,
            summary,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct AppliedMigration {
    pub version: String,
    pub description: String,
    #[serde(rename = "type")]
    pub migration_type: String,
    pub script: String,
    pub execution_time_ms: i32,
}
#[derive(Debug, Clone, Serialize, Default)]
#[non_exhaustive]
pub struct RunReport {
    pub applied: Vec<AppliedMigration>,
    pub skipped: usize,
}
#[derive(Debug, Clone, Serialize, Default)]
#[non_exhaustive]
pub struct InitReport {
    pub root: PathBuf,
    pub created: Vec<PathBuf>,
    pub overwritten: Vec<PathBuf>,
}
