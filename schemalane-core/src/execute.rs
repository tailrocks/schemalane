use std::time::Instant;

use tokio_postgres::{Client, Transaction};

use crate::history::HistoryWrite;
use crate::sql_analysis::{
    ParsedSqlStatement, SqlTransactionMode, parse_sql_migration, resolve_sql_transaction_mode,
};
use crate::{
    MigrationInfo, MigrationObserver, RustMigrationExecutor, RustTransactionMode, SchemalaneError,
    SqlStatementFailed, SqlStatementFinished, SqlStatementStarted,
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
                if let Err(error) = execute_statement(
                    &transaction,
                    statement,
                    index,
                    total_statements,
                    migration,
                    observer,
                )
                .await
                {
                    let _ = transaction.rollback().await;
                    return Err(SchemalaneError::Db(error));
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
                execute_statement(
                    client,
                    statement,
                    index,
                    total_statements,
                    migration,
                    observer,
                )
                .await
                .map_err(SchemalaneError::Db)?;
            }
            Ok(Applied::NeedsHistoryRow)
        }
    }
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
    statement: &ParsedSqlStatement,
    index: usize,
    total_statements: usize,
    migration: &MigrationInfo,
    observer: &O,
) -> Result<(), tokio_postgres::Error>
where
    E: BatchExec,
    O: MigrationObserver + ?Sized,
{
    let source_line = Some(statement.source_line);
    observer.on_sql_statement_start(&SqlStatementStarted {
        migration: migration.clone(),
        statement_index: index + 1,
        total_statements,
        statement_preview: statement.preview.clone(),
        statement: statement.sql.clone(),
        source_line,
    });
    let started = Instant::now();
    match executor.batch(&statement.sql).await {
        Ok(()) => {
            observer.on_sql_statement_finish(&SqlStatementFinished {
                migration: migration.clone(),
                statement_index: index + 1,
                total_statements,
                statement_preview: statement.preview.clone(),
                statement: statement.sql.clone(),
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
                statement_preview: statement.preview.clone(),
                statement: statement.sql.clone(),
                execution_time_ms: millis_i32(started.elapsed().as_millis()),
                error: error.to_string(),
                source_line,
            });
            Err(error)
        }
    }
}

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
