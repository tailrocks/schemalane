use crate::discovery::DiscoveredMigration;
use crate::filename::ParsedVersion;
use crate::history::{HistoryRow, latest_history_by_script};
use serde::Serialize;
use std::collections::HashMap;
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
