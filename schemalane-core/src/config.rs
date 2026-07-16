use std::path::PathBuf;

#[derive(Debug, Clone)]
#[non_exhaustive]
/// Configuration for one migration target and migration source directory.
pub struct SchemalaneConfig {
    /// `PostgreSQL` schema managed by this migrator.
    pub schema: String,
    /// Unqualified history-table name created inside `schema`.
    pub history_table: String,
    /// Directory containing versioned `.sql` and `.rs` migration files.
    pub migrations_dir: PathBuf,
    /// Optional value stored in history rows; defaults to the database user.
    pub installed_by: Option<String>,
    /// Optional advisory-lock key; derived from schema and table when absent.
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
    /// Returns the default configuration.
    pub fn new() -> Self {
        Self::default()
    }
    /// Sets the managed `PostgreSQL` schema.
    #[must_use]
    pub fn with_schema(mut self, value: impl Into<String>) -> Self {
        self.schema = value.into();
        self
    }
    /// Sets the schema-history table name.
    #[must_use]
    pub fn with_history_table(mut self, value: impl Into<String>) -> Self {
        self.history_table = value.into();
        self
    }
    /// Sets the migration source directory.
    #[must_use]
    pub fn with_migrations_dir(mut self, value: impl Into<PathBuf>) -> Self {
        self.migrations_dir = value.into();
        self
    }
    /// Sets the identity recorded for newly applied migrations.
    #[must_use]
    pub fn with_installed_by(mut self, value: Option<String>) -> Self {
        self.installed_by = value;
        self
    }
    /// Overrides the derived `PostgreSQL` advisory-lock key.
    #[must_use]
    pub const fn with_advisory_lock_id(mut self, value: Option<i64>) -> Self {
        self.advisory_lock_id = value;
        self
    }
}
