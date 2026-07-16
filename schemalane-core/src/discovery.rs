use crate::MigrationInfo;
use crate::filename::ParsedVersion;
use std::path::PathBuf;

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
