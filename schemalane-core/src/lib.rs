//! Schemalane's PostgreSQL migration engine.

mod config;
mod error;
mod migrator;
mod observer;
mod report;

pub use config::SchemalaneConfig;
pub use error::SchemalaneError;
pub use migrator::{
    RustMigrationExecutor, RustMigrationFuture, RustTransactionMode, SchemalaneMigrator,
    derive_advisory_lock_id, init_migration_project, should_fail_on_pending,
};
pub use observer::{
    MigrationFailed, MigrationFinished, MigrationInfo, MigrationObserver, MigrationStarted,
    NoopMigrationObserver, SqlStatementFailed, SqlStatementFinished, SqlStatementStarted,
};
pub use report::{
    AppliedMigration, InitReport, MigrationState, RunReport, StatusEntry, StatusReport,
    StatusSummary,
};
pub use schemalane_macros::embed_migrations;
