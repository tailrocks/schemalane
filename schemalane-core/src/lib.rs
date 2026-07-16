//! Schemalane's PostgreSQL migration engine.

mod checksum;
mod config;
mod error;
mod ident;
mod init;
mod migrator;
mod observer;
mod report;
mod rust_migration;

pub use config::SchemalaneConfig;
pub use error::SchemalaneError;
pub use init::init_migration_project;
pub use migrator::{SchemalaneMigrator, derive_advisory_lock_id, should_fail_on_pending};
pub use observer::{
    MigrationFailed, MigrationFinished, MigrationInfo, MigrationObserver, MigrationStarted,
    NoopMigrationObserver, SqlStatementFailed, SqlStatementFinished, SqlStatementStarted,
};
pub use report::{
    AppliedMigration, InitReport, MigrationState, RunReport, StatusEntry, StatusReport,
    StatusSummary,
};
pub use rust_migration::{RustMigrationExecutor, RustMigrationFuture, RustTransactionMode};
pub use schemalane_macros::embed_migrations;
