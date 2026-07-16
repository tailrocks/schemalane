use thiserror::Error;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SchemalaneError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Database error: {0}")]
    Db(#[from] tokio_postgres::Error),
    #[error("Pool error: {0}")]
    Pool(#[from] deadpool_postgres::PoolError),
    #[error("Validation error: {0}")]
    Validation(String),
    #[error("Configuration error: {0}")]
    Config(String),
    #[error("Internal error: {0}")]
    Internal(String),
    #[error("Drift detected: {0}")]
    Drift(String),
    #[error("Failed migration found in history: {0}")]
    FailedHistory(String),
    #[error("Migration execution failed for {script}: {source}")]
    MigrationExecution {
        script: String,
        #[source]
        source: tokio_postgres::Error,
    },
    #[error(
        "Detected both transactional and non-transactional statements within the same migration {script} (line {line})"
    )]
    MixedStatements { script: String, line: u64 },
    #[error("`fresh` requires --confirm yes")]
    FreshRequiresConfirm,
    #[error("Pending migrations found ({0})")]
    PendingMigrations(usize),
    #[error("migration crate command exited with code {code}")]
    Delegated { code: i32 },
}

impl SchemalaneError {
    #[allow(clippy::match_same_arms)]
    pub const fn exit_code(&self) -> i32 {
        match self {
            Self::Validation(_) => 2,
            Self::Config(_) | Self::Internal(_) => 1,
            Self::Drift(_) => 3,
            Self::FailedHistory(_) => 4,
            Self::PendingMigrations(_) => 5,
            Self::FreshRequiresConfirm => 6,
            Self::MixedStatements { .. } => 7,
            Self::Delegated { code } => *code,
            _ => 1,
        }
    }
}
