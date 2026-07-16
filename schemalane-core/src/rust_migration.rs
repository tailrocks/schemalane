use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio_postgres::Client;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
/// Controls whether a Rust migration runs inside an explicit transaction.
pub enum RustTransactionMode {
    /// Run directly on the migration session.
    NoTransaction,
    /// Wrap the executor in `BEGIN`/`COMMIT`, rolling back on error.
    Transaction,
}
/// Boxed future returned by a Rust migration executor.
pub type RustMigrationFuture<'a> =
    Pin<Box<dyn Future<Output = Result<(), tokio_postgres::Error>> + Send + 'a>>;
type DynRustMigrationFn = dyn for<'a> Fn(&'a Client) -> RustMigrationFuture<'a> + Send + Sync;

#[derive(Clone)]
/// Type-erased executable body and transaction policy for a Rust migration.
pub struct RustMigrationExecutor {
    transaction_mode: RustTransactionMode,
    run: Arc<DynRustMigrationFn>,
}
impl RustMigrationExecutor {
    /// Creates a non-transactional executor.
    pub fn new<F>(run: F) -> Self
    where
        F: for<'a> Fn(&'a Client) -> RustMigrationFuture<'a> + Send + Sync + 'static,
    {
        Self::with_mode(RustTransactionMode::NoTransaction, run)
    }
    /// Creates a transactional executor.
    pub fn transactional<F>(run: F) -> Self
    where
        F: for<'a> Fn(&'a Client) -> RustMigrationFuture<'a> + Send + Sync + 'static,
    {
        Self::with_mode(RustTransactionMode::Transaction, run)
    }
    /// Creates an executor with an explicit transaction mode.
    pub fn with_mode<F>(transaction_mode: RustTransactionMode, run: F) -> Self
    where
        F: for<'a> Fn(&'a Client) -> RustMigrationFuture<'a> + Send + Sync + 'static,
    {
        Self {
            transaction_mode,
            run: Arc::new(run),
        }
    }
    pub(crate) const fn transaction_mode(&self) -> RustTransactionMode {
        self.transaction_mode
    }
    pub(crate) async fn up(&self, client: &Client) -> Result<(), tokio_postgres::Error> {
        (self.run)(client).await
    }
}
