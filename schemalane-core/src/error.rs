use thiserror::Error;

#[derive(Debug, Error)]
#[non_exhaustive]
/// Errors produced by configuration, validation, execution, and CLI delegation.
pub enum SchemalaneError {
    /// A migration source or scaffold file could not be read or written.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    /// `PostgreSQL` rejected a connection, query, transaction, or protocol operation.
    #[error("Database error: {0}")]
    Db(#[from] tokio_postgres::Error),
    /// A connection could not be acquired from the configured pool.
    #[error("Pool error: {0}")]
    Pool(#[from] deadpool_postgres::PoolError),
    /// Migration input failed validation before execution.
    #[error("Validation error: {0}")]
    Validation(String),
    /// Migrator or database connection configuration is invalid.
    #[error("Configuration error: {0}")]
    Config(String),
    /// An internal invariant or serialization operation failed.
    #[error("Internal error: {0}")]
    Internal(String),
    /// Local migrations disagree with applied schema history.
    #[error("Drift detected: {0}")]
    Drift(String),
    /// The latest history row for at least one migration records failure.
    #[error("Failed migration found in history: {0}")]
    FailedHistory(String),
    /// `PostgreSQL` failed while executing a named migration.
    #[error("Migration execution failed for {script}: {source}")]
    MigrationExecution {
        /// Migration filename being executed.
        script: String,
        /// `PostgreSQL` error that stopped execution.
        #[source]
        source: tokio_postgres::Error,
    },
    /// One SQL file mixes statements that require incompatible transaction modes.
    #[error(
        "Detected both transactional and non-transactional statements within the same migration {script} (line {line})"
    )]
    MixedStatements {
        /// Migration filename containing the incompatible statement.
        script: String,
        /// One-based source line of the incompatible statement.
        line: u64,
    },
    /// Destructive `fresh` execution was requested without explicit confirmation.
    #[error("`fresh` requires --confirm yes")]
    FreshRequiresConfirm,
    /// A pending-migration gate was enabled and pending migrations exist.
    #[error("Pending migrations found ({0})")]
    PendingMigrations(usize),
    /// An embedded migration-crate subprocess returned a nonzero exit code.
    #[error("migration crate command exited with code {code}")]
    Delegated {
        /// Exit code returned by the delegated process.
        code: i32,
    },
}

impl SchemalaneError {
    /// Returns the stable process exit code assigned by the specification.
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
