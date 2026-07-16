use std::path::PathBuf;

#[derive(Debug, Clone)]
#[non_exhaustive]
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

impl SchemalaneConfig {
    pub fn new() -> Self {
        Self::default()
    }
    #[must_use]
    pub fn with_schema(mut self, value: impl Into<String>) -> Self {
        self.schema = value.into();
        self
    }
    #[must_use]
    pub fn with_history_table(mut self, value: impl Into<String>) -> Self {
        self.history_table = value.into();
        self
    }
    #[must_use]
    pub fn with_migrations_dir(mut self, value: impl Into<PathBuf>) -> Self {
        self.migrations_dir = value.into();
        self
    }
    #[must_use]
    pub fn with_installed_by(mut self, value: Option<String>) -> Self {
        self.installed_by = value;
        self
    }
    #[must_use]
    pub const fn with_advisory_lock_id(mut self, value: Option<i64>) -> Self {
        self.advisory_lock_id = value;
        self
    }
}
