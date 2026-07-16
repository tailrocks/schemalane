use crate::discovery::DiscoveredMigration;
use crate::filename::ParsedVersion;
use crate::history::{HistoryRow, latest_history_by_script};
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "PascalCase")]
#[non_exhaustive]
/// Reconciliation state of one migration script.
pub enum MigrationState {
    /// Local migration matches a successful history row.
    Success,
    /// Local migration has no successful history row yet.
    Pending,
    /// The latest history row records an unsuccessful execution.
    Failed,
    /// A history row has no corresponding local migration.
    Missing,
    /// Local content differs from the checksum stored in history.
    ChecksumMismatch,
}

#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
/// One migration row in a status report.
pub struct StatusEntry {
    /// Migration version, or `None` for an unversioned history row.
    pub version: Option<String>,
    /// Human-readable migration description.
    pub description: String,
    #[serde(rename = "type")]
    /// Flyway-compatible migration type.
    pub migration_type: String,
    /// Migration filename recorded locally or in history.
    pub script: String,
    /// Local or recorded Flyway-compatible checksum, when available.
    pub checksum: Option<i32>,
    /// History insertion rank, absent for a pending local migration.
    pub installed_rank: Option<i32>,
    /// Database installation timestamp rendered as text, when applied.
    pub installed_on: Option<String>,
    /// Recorded migration execution duration in milliseconds.
    pub execution_time_ms: Option<i32>,
    /// Reconciled relationship between local migration and history.
    pub state: MigrationState,
}
impl StatusEntry {
    /// Creates a fully populated status entry.
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
/// Counts status entries by reconciliation state.
pub struct StatusSummary {
    /// Number of matching successful migrations.
    pub success: usize,
    /// Number of unapplied local migrations.
    pub pending: usize,
    /// Number of migrations whose latest history row failed.
    pub failed: usize,
    /// Number of history migrations missing locally.
    pub missing: usize,
    /// Number of applied migrations whose local checksum changed.
    pub checksum_mismatch: usize,
}
impl StatusSummary {
    /// Creates summary counts in state order.
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
/// Status of a configured schema and its local migration set.
pub struct StatusReport {
    /// `PostgreSQL` schema reconciled by the report.
    pub schema: String,
    /// Unqualified schema-history table name.
    pub history_table: String,
    /// Deterministically ordered migration reconciliation entries.
    pub migrations: Vec<StatusEntry>,
    /// Counts derived from `migrations`.
    pub summary: StatusSummary,
}
impl StatusReport {
    /// Creates a status report from entries and precomputed counts.
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
/// Metadata for one migration applied during a run.
pub struct AppliedMigration {
    /// Original migration version text.
    pub version: String,
    /// Human-readable migration description.
    pub description: String,
    #[serde(rename = "type")]
    /// Flyway-compatible migration type.
    pub migration_type: String,
    /// Applied migration filename.
    pub script: String,
    /// Wall-clock application duration in milliseconds.
    pub execution_time_ms: i32,
}
#[derive(Debug, Clone, Serialize, Default)]
#[non_exhaustive]
/// Result of applying or freshly reapplying migrations.
pub struct RunReport {
    /// Migrations successfully applied during this run.
    pub applied: Vec<AppliedMigration>,
    /// Already-applied migrations skipped by this run.
    pub skipped: usize,
}
#[derive(Debug, Clone, Serialize, Default)]
#[non_exhaustive]
/// Files created and overwritten by project initialization.
pub struct InitReport {
    /// Root directory of the initialized migration project.
    pub root: PathBuf,
    /// Files newly created by initialization.
    pub created: Vec<PathBuf>,
    /// Existing files replaced because force mode was enabled.
    pub overwritten: Vec<PathBuf>,
}

/// Transaction behavior of one planned migration.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PlannedTransactionMode {
    /// SQL executes inside one transaction.
    Transactional,
    /// SQL executes directly on the migration session.
    NonTransactional,
    /// Rust executor owns the behavior described by `transaction_mode`.
    Rust,
}

/// One pending migration in an [`UpPlan`].
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct PlannedMigration {
    /// Original migration version text.
    pub version: String,
    /// Pending migration filename.
    pub script: String,
    #[serde(rename = "type")]
    /// Flyway-compatible migration type.
    pub migration_type: String,
    /// Transaction policy that real `up` would use.
    pub transaction_mode: PlannedTransactionMode,
    /// Raw SQL statements in execution order; empty for Rust migrations.
    pub statements: Vec<String>,
}

/// Read-only, ordered preview of what `up` would execute.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct UpPlan {
    /// `PostgreSQL` schema against which history was reconciled.
    pub schema: String,
    /// Unqualified schema-history table name.
    pub history_table: String,
    /// Pending migrations in execution order.
    pub migrations: Vec<PlannedMigration>,
}
pub(crate) fn build_status_report(
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
