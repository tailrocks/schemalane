//! Schemalane's PostgreSQL migration engine.

mod config;
mod error;
mod migrator;

pub use config::SchemalaneConfig;
pub use error::SchemalaneError;
pub use migrator::{
    AppliedMigration, InitReport, MigrationFailed, MigrationFinished, MigrationInfo,
    MigrationObserver, MigrationStarted, MigrationState, NoopMigrationObserver, RunReport,
    RustMigrationExecutor, RustMigrationFuture, RustTransactionMode, SchemalaneMigrator,
    SqlStatementFailed, SqlStatementFinished, SqlStatementStarted, StatusEntry, StatusReport,
    StatusSummary, derive_advisory_lock_id, init_migration_project, should_fail_on_pending,
};
pub use schemalane_macros::embed_migrations;
