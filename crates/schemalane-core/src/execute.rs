use std::time::Instant;

use tokio_postgres::{Client, GenericClient, Transaction};

use crate::deferred::{Deferral, Prepared};

use crate::history::HistoryWrite;
use crate::sql_analysis::{
    ParsedSqlStatement, SqlTransactionMode, parse_sql_migration, resolve_sql_transaction_mode,
};
use crate::{
    MigrationInfo, MigrationObserver, RustMigrationExecutor, RustTransactionMode, SchemalaneError,
    SqlStatementDeferred, SqlStatementFailed, SqlStatementFinished, SqlStatementStarted,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Applied {
    HistoryRecorded,
    NeedsHistoryRow,
}

pub(crate) async fn execute_sql_migration<O>(
    client: &mut Client,
    sql: &str,
    migration: &MigrationInfo,
    observer: &O,
    history_write: &HistoryWrite<'_>,
    mut deferral: Option<&mut Deferral<'_>>,
) -> Result<Applied, SchemalaneError>
where
    O: MigrationObserver + ?Sized,
{
    let started = Instant::now();
    let statements = parse_sql_migration(sql)?;
    let total_statements = statements.len();
    match resolve_sql_transaction_mode(&statements, &migration.script)? {
        SqlTransactionMode::Transactional => {
            let transaction = client.transaction().await?;
            for (index, statement) in statements.iter().enumerate() {
                if let Err(error) = run_statement(
                    &transaction,
                    statement,
                    index,
                    total_statements,
                    migration,
                    observer,
                    deferral.as_deref_mut(),
                )
                .await
                {
                    let _ = transaction.rollback().await;
                    return Err(error);
                }
            }
            history_write
                .repository
                .insert_transaction(
                    &transaction,
                    history_write,
                    millis_i32(started.elapsed().as_millis()),
                )
                .await?;
            transaction.commit().await?;
            Ok(Applied::HistoryRecorded)
        }
        SqlTransactionMode::NonTransactional => {
            for (index, statement) in statements.iter().enumerate() {
                run_statement(
                    &*client,
                    statement,
                    index,
                    total_statements,
                    migration,
                    observer,
                    deferral.as_deref_mut(),
                )
                .await?;
            }
            Ok(Applied::NeedsHistoryRow)
        }
    }
}

/// Executes one statement, or records it in the deferred set when a deferral is active.
async fn run_statement<E, O>(
    executor: &E,
    statement: &ParsedSqlStatement,
    index: usize,
    total_statements: usize,
    migration: &MigrationInfo,
    observer: &O,
    deferral: Option<&mut Deferral<'_>>,
) -> Result<(), SchemalaneError>
where
    E: BatchExec + GenericClient,
    O: MigrationObserver + ?Sized,
{
    let sql = match deferral {
        Some(deferral) => match deferral
            .prepare(executor, migration, index, statement)
            .await?
        {
            Prepared::Run(sql) => sql,
            Prepared::Deferred => {
                observer.on_sql_statement_deferred(&SqlStatementDeferred {
                    migration: migration.clone(),
                    statement_index: index + 1,
                    total_statements,
                    statement: statement.sql.clone(),
                    source_line: Some(statement.source_line),
                });
                return Ok(());
            }
        },
        None => statement.sql.clone(),
    };
    execute_statement(
        executor,
        &sql,
        statement.source_line,
        index,
        total_statements,
        migration,
        observer,
    )
    .await
    .map_err(SchemalaneError::Db)
}

trait BatchExec {
    async fn batch(&self, sql: &str) -> Result<(), tokio_postgres::Error>;
}

impl BatchExec for Client {
    async fn batch(&self, sql: &str) -> Result<(), tokio_postgres::Error> {
        self.batch_execute(sql).await
    }
}

impl BatchExec for Transaction<'_> {
    async fn batch(&self, sql: &str) -> Result<(), tokio_postgres::Error> {
        self.batch_execute(sql).await
    }
}

async fn execute_statement<E, O>(
    executor: &E,
    sql: &str,
    source_line: u64,
    index: usize,
    total_statements: usize,
    migration: &MigrationInfo,
    observer: &O,
) -> Result<(), tokio_postgres::Error>
where
    E: BatchExec,
    O: MigrationObserver + ?Sized,
{
    let source_line = Some(source_line);
    observer.on_sql_statement_start(&SqlStatementStarted {
        migration: migration.clone(),
        statement_index: index + 1,
        total_statements,
        statement: sql.to_owned(),
        source_line,
    });
    let started = Instant::now();
    match executor.batch(sql).await {
        Ok(()) => {
            observer.on_sql_statement_finish(&SqlStatementFinished {
                migration: migration.clone(),
                statement_index: index + 1,
                total_statements,
                statement: sql.to_owned(),
                execution_time_ms: millis_i32(started.elapsed().as_millis()),
                source_line,
            });
            Ok(())
        }
        Err(error) => {
            observer.on_sql_statement_failed(&SqlStatementFailed {
                migration: migration.clone(),
                statement_index: index + 1,
                total_statements,
                statement: sql.to_owned(),
                execution_time_ms: millis_i32(started.elapsed().as_millis()),
                error: error.to_string(),
                source_line,
            });
            Err(error)
        }
    }
}

/// Rust migrations use raw BEGIN/COMMIT because their public executor future borrows `&Client`;
/// the typed transaction API requires `&mut Client` for the same lifetime. Best-effort rollback
/// is deliberate: if it fails, dropping the detached session aborts the transaction server-side.
pub(crate) async fn execute_rust_migration(
    client: &mut Client,
    migration: &RustMigrationExecutor,
) -> Result<(), tokio_postgres::Error> {
    match migration.transaction_mode() {
        RustTransactionMode::NoTransaction => migration.up(client).await,
        RustTransactionMode::Transaction => {
            client.batch_execute("BEGIN").await?;
            match migration.up(client).await {
                Ok(()) => client.batch_execute("COMMIT").await,
                Err(error) => {
                    let _ = client.batch_execute("ROLLBACK").await;
                    Err(error)
                }
            }
        }
    }
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "guarded by the preceding bounds check"
)]
pub(crate) const fn millis_i32(millis: u128) -> i32 {
    if millis > i32::MAX as u128 {
        i32::MAX
    } else {
        millis as i32
    }
}
