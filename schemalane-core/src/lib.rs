//! Schemalane's forward-only `PostgreSQL` migration engine.
//!
//! The same engine powers standalone migration crates, embedded applications,
//! the `schemalane` CLI, and direct programmatic use. It discovers versioned
//! SQL and Rust migrations, validates history and checksums, serializes runs
//! with a `PostgreSQL` advisory lock, and reports lifecycle events.
//!
//! # Quick start
//!
//! ```no_run
//! # async fn demo(pool: deadpool_postgres::Pool) -> Result<(), schemalane_core::SchemalaneError> {
//! let config = schemalane_core::SchemalaneConfig::default();
//! let migrator = schemalane_core::SchemalaneMigrator::new(config);
//! let _report = migrator.up(&pool).await?;
//! # Ok(())
//! # }
//! ```
//!
//! Transactional SQL migrations commit their history row atomically with the
//! migration. Non-transactional SQL and Rust migrations record history after
//! execution and therefore have at-least-once semantics; make them idempotent.

#![deny(missing_docs)]

mod checksum;
mod config;
mod discovery;
mod error;
mod execute;
mod filename;
mod history;
mod ident;
mod init;
mod migrator;
mod observer;
mod report;
mod rust_migration;
mod sql_analysis;

pub use config::SchemalaneConfig;
pub use error::SchemalaneError;
pub use init::init_migration_project;
pub use migrator::{SchemalaneMigrator, derive_advisory_lock_id, should_fail_on_pending};
pub use observer::{
    MigrationFailed, MigrationFinished, MigrationInfo, MigrationObserver, MigrationStarted,
    NoopMigrationObserver, SqlStatementFailed, SqlStatementFinished, SqlStatementStarted,
};
pub use report::{
    AppliedMigration, InitReport, MigrationState, PlannedMigration, PlannedTransactionMode,
    RunReport, StatusEntry, StatusReport, StatusSummary, UpPlan,
};
pub use rust_migration::{RustMigrationExecutor, RustMigrationFuture, RustTransactionMode};
pub use schemalane_macros::embed_migrations;
