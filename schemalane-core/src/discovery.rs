use crate::filename::ParsedVersion;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use crate::checksum::calculate_checksum;
use crate::filename::{parse_rust_filename, parse_sql_filename};
use crate::migrator::SchemalaneMigrator;
use crate::{MigrationInfo, SchemalaneError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MigrationType {
    Sql,
    Rust,
}
impl MigrationType {
    pub(crate) const fn as_history_type(self) -> &'static str {
        match self {
            Self::Sql => "SQL",
            Self::Rust => "RUST",
        }
    }
}

#[derive(Clone)]
pub(crate) struct DiscoveredMigration {
    pub(crate) version: ParsedVersion,
    pub(crate) version_text: String,
    pub(crate) description_display: String,
    pub(crate) script: String,
    pub(crate) checksum: Option<i32>,
    pub(crate) migration_type: MigrationType,
    pub(crate) source: MigrationSource,
}

impl DiscoveredMigration {
    pub(crate) fn info(&self) -> MigrationInfo {
        MigrationInfo {
            version: self.version_text.clone(),
            description: self.description_display.clone(),
            migration_type: self.migration_type.as_history_type().to_owned(),
            script: self.script.clone(),
        }
    }
}

#[derive(Clone)]
pub(crate) enum MigrationSource {
    SqlFile { path: PathBuf, content: String },
    RustFile(PathBuf),
}

impl SchemalaneMigrator {
    pub(crate) fn discover_migrations(&self) -> Result<Vec<DiscoveredMigration>, SchemalaneError> {
        if !self.config.migrations_dir.exists() {
            return Err(SchemalaneError::Validation(format!(
                "migrations directory not found: {}",
                self.config.migrations_dir.display()
            )));
        }
        let mut migrations = Vec::new();
        for entry in std::fs::read_dir(&self.config.migrations_dir)? {
            let path = entry?.path();
            if !path.is_file() {
                continue;
            }
            let Some(extension) = path.extension().and_then(|extension| extension.to_str()) else {
                continue;
            };
            let file_name = path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| {
                    SchemalaneError::Validation("non-utf8 migration filename".to_owned())
                })?;
            let migration = if extension.eq_ignore_ascii_case("sql") {
                let (version_text, version, description) = parse_sql_filename(file_name)?;
                let bytes = std::fs::read(&path)?;
                let checksum = Some(calculate_checksum(file_name, &bytes)?);
                let content = String::from_utf8(bytes).map_err(|error| {
                    SchemalaneError::Validation(format!(
                        "SQL migration {} is not valid UTF-8: {error}",
                        path.display()
                    ))
                })?;
                DiscoveredMigration {
                    version,
                    version_text,
                    description_display: description.replace('_', " "),
                    script: file_name.to_owned(),
                    checksum,
                    migration_type: MigrationType::Sql,
                    source: MigrationSource::SqlFile { path, content },
                }
            } else if extension.eq_ignore_ascii_case("rs") {
                let (version_text, version, description) = parse_rust_filename(file_name)?;
                let content = std::fs::read(&path)?;
                DiscoveredMigration {
                    version,
                    version_text,
                    description_display: description.replace('_', " "),
                    script: file_name.to_owned(),
                    checksum: Some(calculate_checksum(file_name, &content)?),
                    migration_type: MigrationType::Rust,
                    source: MigrationSource::RustFile(path),
                }
            } else {
                continue;
            };
            migrations.push(migration);
        }
        let mut versions: BTreeMap<&ParsedVersion, &str> = BTreeMap::new();
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
        migrations.sort_by(|left, right| {
            left.version
                .cmp(&right.version)
                .then_with(|| left.script.cmp(&right.script))
        });
        Ok(migrations)
    }

    pub(crate) fn ensure_rust_executors_registered(
        &self,
        migrations: &[DiscoveredMigration],
    ) -> Result<(), SchemalaneError> {
        let mut missing_scripts = migrations
            .iter()
            .filter(|migration| {
                migration.migration_type == MigrationType::Rust
                    && !self.rust_migrations.contains_key(migration.script.as_str())
            })
            .map(|migration| migration.script.clone())
            .collect::<Vec<_>>();
        if missing_scripts.is_empty() {
            return Ok(());
        }
        missing_scripts.sort();
        Err(SchemalaneError::Validation(format!(
            "missing Rust migration executor(s) for script(s): {}",
            missing_scripts.join(", ")
        )))
    }
}
