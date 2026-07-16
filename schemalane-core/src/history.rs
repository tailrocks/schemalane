pub(crate) struct HistoryWrite<'a> {
    pub(crate) repository: &'a HistoryRepository,
    pub(crate) installed_rank: i32,
    pub(crate) version: &'a str,
    pub(crate) description: &'a str,
    pub(crate) migration_type: &'a str,
    pub(crate) script: &'a str,
    pub(crate) checksum: Option<i32>,
    pub(crate) installed_by: &'a str,
}
#[derive(Debug, Clone)]
pub(crate) struct HistoryRow {
    pub(crate) installed_rank: i32,
    pub(crate) version: Option<String>,
    pub(crate) description: String,
    pub(crate) migration_type: String,
    pub(crate) script: String,
    pub(crate) checksum: Option<i32>,
    pub(crate) installed_on: String,
    pub(crate) execution_time: i32,
    pub(crate) success: bool,
}

/// Owns every SQL statement touching the Flyway-compatible history table.
/// Its DDL and column set are the compatibility contract from spec section 6.
pub(crate) struct HistoryRepository {
    qualified: String,
    history_table: String,
}

impl HistoryRepository {
    const SELECT_COLUMNS: &'static str = "\"installed_rank\", \"version\", \"description\", \"type\", \"script\", \"checksum\", \"installed_by\", \"installed_on\"::text AS \"installed_on\", \"execution_time\", \"success\"";
    // installed_on is intentionally omitted: PostgreSQL supplies its now() default.
    const INSERT_COLUMNS: &'static str = "\"installed_rank\", \"version\", \"description\", \"type\", \"script\", \"checksum\", \"installed_by\", \"execution_time\", \"success\"";

    pub(crate) fn new(schema: &str, history_table: &str) -> Self {
        Self {
            qualified: qualified_table(schema, history_table),
            history_table: history_table.to_owned(),
        }
    }

    pub(crate) async fn ensure_table(&self, client: &Client) -> Result<(), SchemalaneError> {
        let success_idx = quote_ident(&format!("{}_s_idx", self.history_table));
        let ddl = format!(
            "\
CREATE TABLE IF NOT EXISTS {table} (\
\"installed_rank\" INTEGER NOT NULL,\
\"version\" VARCHAR(50),\
\"description\" VARCHAR(200) NOT NULL,\
\"type\" VARCHAR(20) NOT NULL,\
\"script\" VARCHAR(1000) NOT NULL,\
\"checksum\" INTEGER,\
\"installed_by\" VARCHAR(100) NOT NULL,\
\"installed_on\" TIMESTAMP NOT NULL DEFAULT now(),\
\"execution_time\" INTEGER NOT NULL,\
\"success\" BOOLEAN NOT NULL,\
CONSTRAINT {pk} PRIMARY KEY (\"installed_rank\")\
);\
CREATE INDEX IF NOT EXISTS {success_idx} ON {table} (\"success\");",
            table = self.qualified,
            pk = quote_ident(&format!("{}_pk", self.history_table)),
        );
        client.batch_execute(&ddl).await?;
        Ok(())
    }

    pub(crate) async fn exists(&self, client: &Client) -> Result<bool, SchemalaneError> {
        let row = client
            .query_one(
                "SELECT to_regclass($1) IS NOT NULL AS exists",
                &[&self.qualified],
            )
            .await?;
        Ok(row.get("exists"))
    }

    pub(crate) async fn load(&self, client: &Client) -> Result<Vec<HistoryRow>, SchemalaneError> {
        let query = format!(
            "SELECT {} FROM {} ORDER BY \"installed_rank\" ASC",
            Self::SELECT_COLUMNS,
            self.qualified
        );
        let rows = client.query(&query, &[]).await?;
        Ok(rows
            .into_iter()
            .map(|row| HistoryRow {
                installed_rank: row.get("installed_rank"),
                version: row.get("version"),
                description: row.get("description"),
                migration_type: row.get("type"),
                script: row.get("script"),
                checksum: row.get("checksum"),
                installed_on: row.get("installed_on"),
                execution_time: row.get("execution_time"),
                success: row.get("success"),
            })
            .collect())
    }

    pub(crate) async fn insert_client(
        &self,
        client: &Client,
        history: &HistoryWrite<'_>,
        execution_time: i32,
        success: bool,
    ) -> Result<(), SchemalaneError> {
        let sql = format!(
            "INSERT INTO {} ({}) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)",
            self.qualified,
            Self::INSERT_COLUMNS
        );
        let version = Some(history.version);
        let params: Vec<&(dyn ToSql + Sync)> = vec![
            &history.installed_rank,
            &version,
            &history.description,
            &history.migration_type,
            &history.script,
            &history.checksum,
            &history.installed_by,
            &execution_time,
            &success,
        ];
        client.execute(&sql, &params).await?;
        Ok(())
    }

    pub(crate) async fn insert_transaction(
        &self,
        transaction: &Transaction<'_>,
        history: &HistoryWrite<'_>,
        execution_time: i32,
    ) -> Result<(), SchemalaneError> {
        let sql = format!(
            "INSERT INTO {} ({}) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)",
            self.qualified,
            Self::INSERT_COLUMNS
        );
        let version = Some(history.version);
        let success = true;
        let params: Vec<&(dyn ToSql + Sync)> = vec![
            &history.installed_rank,
            &version,
            &history.description,
            &history.migration_type,
            &history.script,
            &history.checksum,
            &history.installed_by,
            &execution_time,
            &success,
        ];
        transaction.execute(&sql, &params).await?;
        Ok(())
    }
}
use crate::SchemalaneError;
use crate::ident::{qualified_table, quote_ident};
use std::collections::HashMap;
use tokio_postgres::types::ToSql;
use tokio_postgres::{Client, Transaction};

pub(crate) fn latest_history_by_script(history: &[HistoryRow]) -> HashMap<&str, &HistoryRow> {
    let mut latest = HashMap::new();
    for row in history {
        latest.insert(row.script.as_str(), row);
    }
    latest
}
