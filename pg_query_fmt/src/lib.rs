#![doc = "`PostgreSQL` SQL formatter powered by `pg_query`."]
#![doc = ""]
#![doc = "Parses SQL using `PostgreSQL`'s actual parser (via `pg_query`) and produces"]
#![doc = "human-readable, indented output for common DDL and DML statements."]
#![doc = ""]
#![doc = "All formatting is driven by the parsed AST from `pg_query`. The AST is"]
#![doc = "walked recursively to produce formatted SQL text with proper indentation"]
#![doc = "and alignment."]

use pg_query::protobuf::node::Node;

pub(crate) mod expr;
pub mod highlight;
pub mod preview;
pub(crate) mod stmt;

pub(crate) const INDENT: &str = "    ";

// ── Error type ──────────────────────────────────────────────────────────────

/// Errors that can occur during SQL formatting.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FormatError {
    /// The SQL could not be parsed by `pg_query`.
    #[error("parse error: {0}")]
    Parse(String),
    /// A parsed AST node could not be deparsed back to SQL text.
    #[error("deparse error: {0}")]
    Deparse(String),
}

// ── Public API ──────────────────────────────────────────────────────────────

/// Format a single SQL statement for human-readable display.
///
/// Parses the statement with `pg_query`, determines its type from the AST,
/// and produces indented multi-line output. Returns an error if parsing fails.
pub fn format_statement(sql: &str) -> Result<String, FormatError> {
    let trimmed = sql.trim();
    if trimmed.is_empty() {
        return Ok(String::new());
    }

    let parsed = pg_query::parse(trimmed).map_err(|e| FormatError::Parse(e.to_string()))?;

    let node = parsed
        .protobuf
        .stmts
        .first()
        .and_then(|s| s.stmt.as_ref())
        .and_then(|n| n.node.as_ref())
        .ok_or_else(|| FormatError::Parse("empty parse result".into()))?;

    match node {
        Node::CreateStmt(s) => stmt::fmt_create_table(s),
        Node::CreateEnumStmt(s) => stmt::fmt_create_enum(s),
        Node::IndexStmt(s) => stmt::fmt_index_stmt(s),
        Node::AlterTableStmt(s) => stmt::fmt_alter_table(s),
        Node::SelectStmt(s) => stmt::fmt_select_stmt(s),
        Node::InsertStmt(s) => stmt::fmt_insert_stmt(s),
        Node::UpdateStmt(s) => stmt::fmt_update_stmt(s),
        Node::DeleteStmt(s) => stmt::fmt_delete_stmt(s),
        Node::ViewStmt(s) => stmt::fmt_view_stmt(s),
        Node::CreateFunctionStmt(s) => stmt::fmt_create_function(s),
        Node::CreateForeignTableStmt(s) => stmt::fmt_create_foreign_table(s),
        _ => node
            .deparse()
            .map_err(|e| FormatError::Deparse(e.to_string())),
    }
}

/// Format a SQL script containing multiple statements.
///
/// Splits the input using `pg_query::split_with_parser`, formats each statement
/// individually, and joins them with semicolons and blank lines.
pub fn format_sql(sql: &str) -> Result<String, FormatError> {
    let stmts = pg_query::split_with_parser(sql).map_err(|e| FormatError::Parse(e.to_string()))?;

    let formatted: Vec<String> = stmts
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(format_statement)
        .collect::<Result<Vec<_>, _>>()?;

    Ok(formatted.join(";\n\n") + if formatted.is_empty() { "" } else { ";" })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_round_trips(sql: &str) {
        let formatted = format_statement(sql).expect("format");
        let input = pg_query::fingerprint(sql).expect("fingerprint input").hex;
        let output = pg_query::fingerprint(&formatted)
            .unwrap_or_else(|error| {
                panic!("formatted output failed to parse: {error}\n---\n{formatted}")
            })
            .hex;
        assert_eq!(
            input, output,
            "AST changed:\ninput:  {sql}\noutput: {formatted}"
        );
    }

    #[test]
    fn round_trip_corpus() {
        for sql in [
            "SELECT 1",
            "SELECT * FROM t WHERE NOT (a AND b)",
            "SELECT (a + b) * c FROM t",
            "CREATE TABLE \"MyTable\" (\"MyCol\" int, \"select\" text)",
            "ALTER INDEX idx_x RENAME TO idx_y",
            "SELECT DISTINCT ON (id) id, v FROM t ORDER BY id",
            "SELECT arr[1:3] FROM t",
            "SELECT id FROM t FOR UPDATE",
            "CREATE INDEX i ON t (lower(name) text_pattern_ops)",
            "SELECT id FROM t WHERE a = 1",
            "INSERT INTO t (id) VALUES (1)",
            "UPDATE t SET id = 2 WHERE id = 1",
            "DELETE FROM t WHERE id = 1",
            "CREATE VIEW v AS SELECT 1 AS id",
            "CREATE TABLE t (id bigint PRIMARY KEY, value text NOT NULL)",
            "CREATE UNIQUE INDEX idx_t_id ON t (id)",
            "SELECT CASE WHEN a THEN b ELSE c END FROM t",
            "CREATE FUNCTION f() RETURNS text AS $body$ SELECT '$$'::text $body$ LANGUAGE sql",
        ] {
            assert_round_trips(sql);
        }
    }

    #[test]
    fn quotes_identifiers_when_plain_text_would_change_meaning() {
        use crate::expr::quote_identifier;

        assert_eq!(quote_identifier("wallet_id"), "wallet_id");
        assert_eq!(quote_identifier("MyColumn"), "\"MyColumn\"");
        assert_eq!(quote_identifier("select"), "\"select\"");
        assert_eq!(quote_identifier("has space"), "\"has space\"");
        assert_eq!(quote_identifier("a\"b"), "\"a\"\"b\"");
    }

    #[test]
    fn preserves_array_slice_bounds() {
        assert_eq!(
            format_statement("SELECT arr[1:3] FROM t").unwrap(),
            "SELECT arr[1:3]\nFROM t"
        );
    }

    #[test]
    fn alter_index_uses_the_index_object_label() {
        assert_eq!(
            format_statement("ALTER INDEX idx_x RENAME TO idx_y").unwrap(),
            "ALTER INDEX idx_x RENAME TO idx_y"
        );
    }

    // ── Edge cases ──────────────────────────────────────────────────────────

    #[test]
    fn empty_input_returns_empty() {
        assert_eq!(format_statement("").unwrap(), "");
        assert_eq!(format_statement("   ").unwrap(), "");
    }

    #[test]
    fn unparseable_sql_returns_error() {
        let result = format_statement("NOT VALID SQL !!!");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, FormatError::Parse(_)));
    }

    // ── CREATE TABLE (eth_block — columns with defaults and constraints) ────

    #[test]
    fn formats_create_table_eth_block() {
        let sql = "CREATE TABLE eth_block (row_id bigint NOT NULL PRIMARY KEY, row_created_date timestamp without time zone DEFAULT now() NOT NULL, row_updated_date timestamp without time zone DEFAULT now() NOT NULL, row_version bigint DEFAULT 1 NOT NULL, import_status import_status DEFAULT 'PENDING' NOT NULL, date timestamp without time zone NOT NULL, hash text NOT NULL UNIQUE, index bigint NOT NULL UNIQUE, transaction_count integer NOT NULL, withdrawal_count integer, miner_eth_account_row_id bigint NOT NULL, extra_data text)";
        let formatted = format_statement(sql).unwrap();
        let lines: Vec<&str> = formatted.lines().collect();

        assert_eq!(lines.len(), 14);
        assert_eq!(lines[0], "CREATE TABLE eth_block (");
        assert_eq!(
            lines[1],
            "    row_id                   bigint                          NOT NULL PRIMARY KEY,"
        );
        assert_eq!(
            lines[2],
            "    row_created_date         timestamp     DEFAULT now()     NOT NULL,"
        );
        assert_eq!(
            lines[3],
            "    row_updated_date         timestamp     DEFAULT now()     NOT NULL,"
        );
        assert_eq!(
            lines[4],
            "    row_version              bigint        DEFAULT 1         NOT NULL,"
        );
        assert_eq!(
            lines[5],
            "    import_status            import_status DEFAULT 'PENDING' NOT NULL,"
        );
        assert_eq!(
            lines[6],
            "    date                     timestamp                       NOT NULL,"
        );
        assert_eq!(
            lines[7],
            "    hash                     text                            NOT NULL UNIQUE,"
        );
        assert_eq!(
            lines[8],
            "    \"index\"                  bigint                          NOT NULL UNIQUE,"
        );
        assert_eq!(
            lines[9],
            "    transaction_count        int                             NOT NULL,"
        );
        assert_eq!(lines[10], "    withdrawal_count         int,");
        assert_eq!(
            lines[11],
            "    miner_eth_account_row_id bigint                          NOT NULL,"
        );
        assert_eq!(lines[12], "    extra_data               text");
        assert_eq!(lines[13], ")");
    }

    // ── CREATE TABLE (eth_event_log — with table-level UNIQUE constraint) ───

    #[test]
    fn formats_create_table_with_table_level_unique() {
        let sql = "CREATE TABLE eth_event_log (row_id UUID DEFAULT gen_random_uuid() NOT NULL PRIMARY KEY, row_created_date timestamp without time zone DEFAULT now() NOT NULL, eth_transaction_row_id UUID NOT NULL, contract_eth_account_row_id bigint, eth_signature_row_id bigint, index integer NOT NULL, UNIQUE (eth_transaction_row_id, index))";
        let formatted = format_statement(sql).unwrap();
        let lines: Vec<&str> = formatted.lines().collect();

        assert_eq!(lines.len(), 9);
        assert_eq!(lines[0], "CREATE TABLE eth_event_log (");
        assert_eq!(
            lines[1],
            "    row_id                      uuid      DEFAULT gen_random_uuid() NOT NULL PRIMARY KEY,"
        );
        assert_eq!(
            lines[2],
            "    row_created_date            timestamp DEFAULT now()             NOT NULL,"
        );
        assert_eq!(
            lines[3],
            "    eth_transaction_row_id      uuid                                NOT NULL,"
        );
        assert_eq!(lines[4], "    contract_eth_account_row_id bigint,");
        assert_eq!(lines[5], "    eth_signature_row_id        bigint,");
        assert_eq!(
            lines[6],
            "    \"index\"                     int                                 NOT NULL,"
        );
        assert_eq!(lines[7], "    UNIQUE (eth_transaction_row_id, \"index\")");
        assert_eq!(lines[8], ")");
    }

    // ── CREATE TABLE (whitelabel — FOREIGN KEY + ON DELETE CASCADE) ──────────

    #[test]
    fn formats_create_table_with_foreign_keys() {
        let sql = "CREATE TABLE address_result (id UUID PRIMARY KEY DEFAULT gen_random_uuid(), screening_id UUID NOT NULL, blockchain blockchain_type NOT NULL, address TEXT NOT NULL, severity severity_level, FOREIGN KEY (screening_id) REFERENCES wallet_screening_result(id) ON DELETE CASCADE, UNIQUE (screening_id, blockchain, address))";
        let formatted = format_statement(sql).unwrap();
        let lines: Vec<&str> = formatted.lines().collect();

        assert_eq!(lines.len(), 9);
        assert_eq!(lines[0], "CREATE TABLE address_result (");
        assert_eq!(
            lines[1],
            "    id           uuid            DEFAULT gen_random_uuid() PRIMARY KEY,"
        );
        assert_eq!(
            lines[2],
            "    screening_id uuid                                      NOT NULL,"
        );
        assert_eq!(
            lines[3],
            "    blockchain   blockchain_type                           NOT NULL,"
        );
        assert_eq!(
            lines[4],
            "    address      text                                      NOT NULL,"
        );
        assert_eq!(lines[5], "    severity     severity_level,");
        assert_eq!(
            lines[6],
            "    FOREIGN KEY (screening_id) REFERENCES wallet_screening_result (id) ON DELETE CASCADE,"
        );
        assert_eq!(lines[7], "    UNIQUE (screening_id, blockchain, address)");
        assert_eq!(lines[8], ")");
    }

    // ── CREATE TABLE (legacy public — character varying columns) ─────────────

    #[test]
    fn formats_create_table_character_varying_columns() {
        let sql = "CREATE TABLE public.ethereum_txns (token_address character varying(42), from_address character varying(42), to_address character varying(42), amount double precision, transaction_hash character varying(66), log_index integer, block_number bigint)";
        let formatted = format_statement(sql).unwrap();
        let lines: Vec<&str> = formatted.lines().collect();

        assert_eq!(lines.len(), 9);
        assert_eq!(lines[0], "CREATE TABLE public.ethereum_txns (");
        assert_eq!(lines[1], "    token_address    varchar(42),");
        assert_eq!(lines[2], "    from_address     varchar(42),");
        assert_eq!(lines[3], "    to_address       varchar(42),");
        assert_eq!(lines[4], "    amount           double precision,");
        assert_eq!(lines[5], "    transaction_hash varchar(66),");
        assert_eq!(lines[6], "    log_index        int,");
        assert_eq!(lines[7], "    block_number     bigint");
        assert_eq!(lines[8], ")");
    }

    // ── CREATE TABLE — single column stays compact ──────────────────────────

    #[test]
    fn single_column_table_stays_compact() {
        let formatted = format_statement("CREATE TABLE t (id int)").unwrap();
        let lines: Vec<&str> = formatted.lines().collect();

        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0], "CREATE TABLE t (id int)");
    }

    // ── CREATE TYPE ... AS ENUM ─────────────────────────────────────────────

    #[test]
    fn formats_create_enum_type() {
        let sql = "CREATE TYPE import_status AS ENUM ('PENDING', 'PROCESSED')";
        let formatted = format_statement(sql).unwrap();
        let lines: Vec<&str> = formatted.lines().collect();

        assert_eq!(lines.len(), 4);
        assert_eq!(lines[0], "CREATE TYPE import_status AS ENUM (");
        assert_eq!(lines[1], "    'PENDING',");
        assert_eq!(lines[2], "    'PROCESSED'");
        assert_eq!(lines[3], ")");
    }

    #[test]
    fn formats_create_enum_type_many_values() {
        let sql = "CREATE TYPE eth_transfer_source AS ENUM ('NATIVE_TRANSACTION', 'SMART_CONTRACT_METHOD_CALL', 'EVENT_LOG', 'SMART_CONTRACT_METHOD_CALL_AND_EVENT_LOG')";
        let formatted = format_statement(sql).unwrap();
        let lines: Vec<&str> = formatted.lines().collect();

        assert_eq!(lines.len(), 6);
        assert_eq!(lines[0], "CREATE TYPE eth_transfer_source AS ENUM (");
        assert_eq!(lines[1], "    'NATIVE_TRANSACTION',");
        assert_eq!(lines[2], "    'SMART_CONTRACT_METHOD_CALL',");
        assert_eq!(lines[3], "    'EVENT_LOG',");
        assert_eq!(lines[4], "    'SMART_CONTRACT_METHOD_CALL_AND_EVENT_LOG'");
        assert_eq!(lines[5], ")");
    }

    // ── CREATE INDEX (single column) ────────────────────────────────────────

    #[test]
    fn formats_single_column_index() {
        let formatted = format_statement(
            "CREATE INDEX idx_wallets_address ON public.wallets USING btree(address)",
        )
        .unwrap();
        let lines: Vec<&str> = formatted.lines().collect();

        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "CREATE INDEX idx_wallets_address");
        assert_eq!(lines[1], "    ON public.wallets USING btree(address)");
    }

    // ── CREATE INDEX (multi-column) ─────────────────────────────────────────

    #[test]
    fn formats_multi_column_index() {
        let sql = "CREATE INDEX idx_ethereum_txns_token_to_block ON public.ethereum_txns USING btree(token_address, to_address, block_number)";
        let formatted = format_statement(sql).unwrap();
        let lines: Vec<&str> = formatted.lines().collect();

        assert_eq!(lines.len(), 6);
        assert_eq!(lines[0], "CREATE INDEX idx_ethereum_txns_token_to_block");
        assert_eq!(lines[1], "    ON public.ethereum_txns USING btree (");
        assert_eq!(lines[2], "        token_address,");
        assert_eq!(lines[3], "        to_address,");
        assert_eq!(lines[4], "        block_number");
        assert_eq!(lines[5], "    )");
    }

    // ── CREATE INDEX CONCURRENTLY ───────────────────────────────────────────

    #[test]
    fn formats_create_index_concurrently() {
        let formatted = format_statement("CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_wallets_address_lower ON public.wallets(LOWER(address))").unwrap();
        let lines: Vec<&str> = formatted.lines().collect();

        assert_eq!(lines.len(), 2);
        assert_eq!(
            lines[0],
            "CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_wallets_address_lower"
        );
        assert_eq!(
            lines[1],
            "    ON public.wallets USING btree(lower(address))"
        );
    }

    // ── CREATE INDEX (GIN method) ───────────────────────────────────────────

    #[test]
    fn formats_create_index_gin() {
        let formatted = format_statement("CREATE INDEX idx_wallets_organization_or_deposit_org_ids ON wallets USING gin(organization_or_deposit_org_ids)").unwrap();
        let lines: Vec<&str> = formatted.lines().collect();

        assert_eq!(lines.len(), 2);
        assert_eq!(
            lines[0],
            "CREATE INDEX idx_wallets_organization_or_deposit_org_ids"
        );
        assert_eq!(
            lines[1],
            "    ON wallets USING gin(organization_or_deposit_org_ids)"
        );
    }

    // ── CREATE INDEX with WHERE expression ──────────────────────────────────

    #[test]
    fn formats_create_index_where_not() {
        let formatted = format_statement(
            "CREATE INDEX idx_wallets_normalized ON wallets(normalized) WHERE NOT normalized",
        )
        .unwrap();
        let lines: Vec<&str> = formatted.lines().collect();

        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "CREATE INDEX idx_wallets_normalized");
        assert_eq!(lines[1], "    ON wallets USING btree(normalized)");
        assert_eq!(lines[2], "    WHERE NOT normalized");
    }

    // ── CREATE UNIQUE INDEX with WHERE ───────────────────────────────────────

    #[test]
    fn formats_create_unique_index_with_where() {
        let formatted = format_statement("CREATE UNIQUE INDEX bitcoin_address_address_compressed ON bitcoin_address USING btree(address_compressed) WHERE (address_compressed IS NOT NULL)").unwrap();
        let lines: Vec<&str> = formatted.lines().collect();

        assert_eq!(lines.len(), 3);
        assert_eq!(
            lines[0],
            "CREATE UNIQUE INDEX bitcoin_address_address_compressed"
        );
        assert_eq!(
            lines[1],
            "    ON bitcoin_address USING btree(address_compressed)"
        );
        assert_eq!(lines[2], "    WHERE address_compressed IS NOT NULL");
    }

    // ── CREATE UNIQUE INDEX with WHERE IS NOT NULL ───────────────────────────

    #[test]
    fn formats_create_unique_index_where_not_null() {
        let formatted = format_statement("CREATE UNIQUE INDEX bitcoin_address_public_key_uncompressed ON bitcoin_address USING btree(public_key_uncompressed) WHERE (public_key_uncompressed IS NOT NULL)").unwrap();
        let lines: Vec<&str> = formatted.lines().collect();

        assert_eq!(lines.len(), 3);
        assert_eq!(
            lines[0],
            "CREATE UNIQUE INDEX bitcoin_address_public_key_uncompressed"
        );
        assert_eq!(
            lines[1],
            "    ON bitcoin_address USING btree(public_key_uncompressed)"
        );
        assert_eq!(lines[2], "    WHERE public_key_uncompressed IS NOT NULL");
    }

    // ── CREATE INDEX (single column, explicit btree) ────────────────────────

    #[test]
    fn formats_create_index_btree_single() {
        let formatted = format_statement("CREATE INDEX bitcoin_transaction_block_id ON bitcoin_transaction USING btree(block_id)").unwrap();
        let lines: Vec<&str> = formatted.lines().collect();

        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "CREATE INDEX bitcoin_transaction_block_id");
        assert_eq!(lines[1], "    ON bitcoin_transaction USING btree(block_id)");
    }

    // ── DROP INDEX ──────────────────────────────────────────────────────────

    #[test]
    fn formats_drop_index_if_exists() {
        let formatted =
            format_statement("DROP INDEX IF EXISTS idx_wallets_blockchain_address").unwrap();
        let lines: Vec<&str> = formatted.lines().collect();

        assert_eq!(lines.len(), 1);
        assert_eq!(
            lines[0],
            "DROP INDEX IF EXISTS idx_wallets_blockchain_address"
        );
    }

    // ── ALTER TABLE (single action) ─────────────────────────────────────────

    #[test]
    fn formats_alter_table_add_constraint() {
        let formatted = format_statement(
            "ALTER TABLE eth_block ADD CONSTRAINT eth_block_row_id_positive CHECK (row_id > 0)",
        )
        .unwrap();
        let lines: Vec<&str> = formatted.lines().collect();

        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "ALTER TABLE eth_block");
        assert_eq!(
            lines[1],
            "    ADD CONSTRAINT eth_block_row_id_positive CHECK (row_id > 0)"
        );
    }

    // ── ALTER TABLE ADD CONSTRAINT UNIQUE USING INDEX ────────────────────────

    #[test]
    fn formats_alter_table_add_unique_using_index() {
        let formatted = format_statement("ALTER TABLE eth_account ADD CONSTRAINT eth_account_address_key UNIQUE USING INDEX eth_account_address_key").unwrap();
        let lines: Vec<&str> = formatted.lines().collect();

        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "ALTER TABLE eth_account");
        assert_eq!(
            lines[1],
            "    ADD CONSTRAINT eth_account_address_key UNIQUE USING INDEX eth_account_address_key"
        );
    }

    // ── ALTER TABLE DROP CONSTRAINT IF EXISTS ───────────────────────────────

    #[test]
    fn formats_alter_table_drop_constraint_if_exists() {
        let formatted = format_statement(
            "ALTER TABLE eth_account DROP CONSTRAINT IF EXISTS eth_account_address_key",
        )
        .unwrap();
        let lines: Vec<&str> = formatted.lines().collect();

        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "ALTER TABLE eth_account");
        assert_eq!(
            lines[1],
            "    DROP CONSTRAINT IF EXISTS eth_account_address_key"
        );
    }

    // ── ALTER TABLE ALTER COLUMN SET DEFAULT ────────────────────────────────

    #[test]
    fn formats_alter_column_set_default() {
        let formatted = format_statement(
            "ALTER TABLE bitcoin_transaction ALTER COLUMN import_status SET DEFAULT 'PENDING'",
        )
        .unwrap();
        let lines: Vec<&str> = formatted.lines().collect();

        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "ALTER TABLE bitcoin_transaction");
        assert_eq!(
            lines[1],
            "    ALTER COLUMN import_status SET DEFAULT 'PENDING'"
        );
    }

    // ── ALTER TABLE ALTER COLUMN SET NOT NULL ───────────────────────────────

    #[test]
    fn formats_alter_column_set_not_null() {
        let formatted =
            format_statement("ALTER TABLE wallets ALTER COLUMN address SET NOT NULL").unwrap();
        let lines: Vec<&str> = formatted.lines().collect();

        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "ALTER TABLE wallets");
        assert_eq!(lines[1], "    ALTER COLUMN address SET NOT NULL");
    }

    // ── ALTER TABLE ADD PRIMARY KEY ─────────────────────────────────────────

    #[test]
    fn formats_alter_table_add_primary_key() {
        let formatted =
            format_statement("ALTER TABLE wallets ADD PRIMARY KEY (blockchain, address)").unwrap();
        let lines: Vec<&str> = formatted.lines().collect();

        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "ALTER TABLE wallets");
        assert_eq!(lines[1], "    ADD PRIMARY KEY (blockchain, address)");
    }

    // ── ALTER TABLE (multiple ADD COLUMN actions) ────────────────────────────

    #[test]
    fn formats_alter_table_multiple_add_columns() {
        let sql = "ALTER TABLE wallets ADD COLUMN label_ids int8[] NOT NULL DEFAULT '{}', ADD COLUMN category_ids int8[] NOT NULL DEFAULT '{}', ADD COLUMN organization_ids int8[] NOT NULL DEFAULT '{}', ADD COLUMN organization_or_deposit_org_ids int8[] NOT NULL DEFAULT '{}'";
        let formatted = format_statement(sql).unwrap();
        let lines: Vec<&str> = formatted.lines().collect();

        assert_eq!(lines.len(), 5);
        assert_eq!(lines[0], "ALTER TABLE wallets");
        assert_eq!(
            lines[1],
            "    ADD COLUMN label_ids                       int8[] DEFAULT '{}' NOT NULL,"
        );
        assert_eq!(
            lines[2],
            "    ADD COLUMN category_ids                    int8[] DEFAULT '{}' NOT NULL,"
        );
        assert_eq!(
            lines[3],
            "    ADD COLUMN organization_ids                int8[] DEFAULT '{}' NOT NULL,"
        );
        assert_eq!(
            lines[4],
            "    ADD COLUMN organization_or_deposit_org_ids int8[] DEFAULT '{}' NOT NULL"
        );
    }

    // ── ALTER TABLE ... ALTER COLUMN TYPE ───────────────────────────────────

    #[test]
    fn formats_alter_column_type() {
        let formatted = format_statement("ALTER TABLE eth_account ALTER COLUMN address TYPE public.citext USING address::public.citext").unwrap();
        let lines: Vec<&str> = formatted.lines().collect();

        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "ALTER TABLE eth_account");
        assert_eq!(
            lines[1],
            "    ALTER COLUMN address TYPE public.citext USING address::public.citext"
        );
    }

    // ── ALTER TYPE RENAME TO ────────────────────────────────────────────────

    #[test]
    fn formats_alter_type_rename() {
        let formatted = format_statement(
            "ALTER TYPE bitcoin_transaction_import_status RENAME TO import_status",
        )
        .unwrap();
        let lines: Vec<&str> = formatted.lines().collect();

        assert_eq!(lines.len(), 1);
        assert_eq!(
            lines[0],
            "ALTER TYPE bitcoin_transaction_import_status RENAME TO import_status"
        );
    }

    // ── SELECT with clauses ─────────────────────────────────────────────────

    #[test]
    fn formats_select_with_clauses() {
        let formatted = format_statement("SELECT a, b FROM t WHERE a > 1 ORDER BY b").unwrap();
        let lines: Vec<&str> = formatted.lines().collect();

        assert_eq!(lines.len(), 4);
        assert_eq!(lines[0], "SELECT a, b");
        assert_eq!(lines[1], "FROM t");
        assert_eq!(lines[2], "WHERE a > 1");
        assert_eq!(lines[3], "ORDER BY b");
    }

    #[test]
    fn formats_select_setval() {
        let formatted =
            format_statement("SELECT pg_catalog.setval('eth_block_row_id_seq', 1, false)").unwrap();
        let lines: Vec<&str> = formatted.lines().collect();

        assert_eq!(lines.len(), 1);
        assert_eq!(
            lines[0],
            "SELECT pg_catalog.setval('eth_block_row_id_seq', 1, false)"
        );
    }

    // ── INSERT ──────────────────────────────────────────────────────────────

    #[test]
    fn formats_insert_with_columns_and_values() {
        let formatted = format_statement("INSERT INTO t (a, b, c) VALUES (1, 2, 3)").unwrap();
        let lines: Vec<&str> = formatted.lines().collect();

        assert_eq!(lines.len(), 6);
        assert_eq!(lines[0], "INSERT INTO t (");
        assert_eq!(lines[1], "    a,");
        assert_eq!(lines[2], "    b,");
        assert_eq!(lines[3], "    c");
        assert_eq!(lines[4], ")");
        assert_eq!(lines[5], "VALUES (1, 2, 3)");
    }

    // ── UPDATE ──────────────────────────────────────────────────────────────

    #[test]
    fn formats_update_with_set_and_where() {
        let formatted = format_statement("UPDATE t SET a = 1, b = 2 WHERE id = 3").unwrap();
        let lines: Vec<&str> = formatted.lines().collect();

        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "UPDATE t");
        assert_eq!(lines[1], "SET a = 1, b = 2");
        assert_eq!(lines[2], "WHERE id = 3");
    }

    // ── DELETE ──────────────────────────────────────────────────────────────

    #[test]
    fn formats_delete_with_where() {
        let formatted = format_statement("DELETE FROM t WHERE id = 1").unwrap();
        let lines: Vec<&str> = formatted.lines().collect();

        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "DELETE FROM t");
        assert_eq!(lines[1], "WHERE id = 1");
    }

    // ── format_sql (multi-statement script) ─────────────────────────────────

    #[test]
    fn formats_multi_statement_migration() {
        let sql = "CREATE TYPE screening_status AS ENUM ('PROCESSING', 'PROCESSED', 'ERROR'); CREATE TABLE wallet_screening_result (id UUID PRIMARY KEY DEFAULT gen_random_uuid(), status screening_status NOT NULL, total_addresses INTEGER NOT NULL DEFAULT 0); CREATE INDEX idx_address_result_screening_id ON address_result(screening_id);";
        let formatted = format_sql(sql).unwrap();
        let lines: Vec<&str> = formatted.lines().collect();

        assert_eq!(lines.len(), 14);
        assert_eq!(lines[0], "CREATE TYPE screening_status AS ENUM (");
        assert_eq!(lines[1], "    'PROCESSING',");
        assert_eq!(lines[2], "    'PROCESSED',");
        assert_eq!(lines[3], "    'ERROR'");
        assert_eq!(lines[4], ");");
        assert_eq!(lines[5], "");
        assert_eq!(lines[6], "CREATE TABLE wallet_screening_result (");
        assert_eq!(
            lines[7],
            "    id              uuid             DEFAULT gen_random_uuid() PRIMARY KEY,"
        );
        assert_eq!(
            lines[8],
            "    status          screening_status                           NOT NULL,"
        );
        assert_eq!(
            lines[9],
            "    total_addresses int              DEFAULT 0                 NOT NULL"
        );
        assert_eq!(lines[10], ");");
        assert_eq!(lines[11], "");
        assert_eq!(lines[12], "CREATE INDEX idx_address_result_screening_id");
        assert_eq!(
            lines[13],
            "    ON address_result USING btree(screening_id);"
        );
    }

    // ── Realistic full migration (16-column table from user's bug report) ───

    #[test]
    fn formats_create_table_with_many_columns() {
        let sql = "CREATE TABLE wallet_tags.tron_amirsender_money_mules_to_binance_non_trc20_txns (txn_type varchar(256), transaction_id varchar(256), method_id varchar(256), internal_transactions varchar(256), function_name varchar(256), contract_owner varchar(256), contract_address varchar(256), caller_contract_address double precision, block_written_at varchar(256), block_number int, token varchar(256), target varchar(256), id double precision, amount double precision, _value double precision, _spender varchar(256))";
        let formatted = format_statement(sql).unwrap();
        let lines: Vec<&str> = formatted.lines().collect();

        assert_eq!(lines.len(), 18);
        assert_eq!(
            lines[0],
            "CREATE TABLE wallet_tags.tron_amirsender_money_mules_to_binance_non_trc20_txns ("
        );
        assert_eq!(lines[1], "    txn_type                varchar(256),");
        assert_eq!(lines[2], "    transaction_id          varchar(256),");
        assert_eq!(lines[3], "    method_id               varchar(256),");
        assert_eq!(lines[4], "    internal_transactions   varchar(256),");
        assert_eq!(lines[5], "    function_name           varchar(256),");
        assert_eq!(lines[6], "    contract_owner          varchar(256),");
        assert_eq!(lines[7], "    contract_address        varchar(256),");
        assert_eq!(lines[8], "    caller_contract_address double precision,");
        assert_eq!(lines[9], "    block_written_at        varchar(256),");
        assert_eq!(lines[10], "    block_number            int,");
        assert_eq!(lines[11], "    token                   varchar(256),");
        assert_eq!(lines[12], "    target                  varchar(256),");
        assert_eq!(lines[13], "    id                      double precision,");
        assert_eq!(lines[14], "    amount                  double precision,");
        assert_eq!(lines[15], "    _value                  double precision,");
        assert_eq!(lines[16], "    _spender                varchar(256)");
        assert_eq!(lines[17], ")");
    }

    // ── CREATE FUNCTION (update_row_updated_date — simple trigger) ──────────

    #[test]
    fn formats_create_function_update_row_updated_date() {
        let sql = "CREATE FUNCTION update_row_updated_date() RETURNS TRIGGER AS\n$$\nBEGIN\n    NEW.row_updated_date := NOW();\n    RETURN NEW;\nEND;\n$$ LANGUAGE plpgsql;";
        let formatted = format_statement(sql).unwrap();
        let lines: Vec<&str> = formatted.lines().collect();

        assert_eq!(lines.len(), 6);
        assert_eq!(
            lines[0],
            "CREATE FUNCTION update_row_updated_date() RETURNS trigger AS $$"
        );
        assert_eq!(lines[1], "BEGIN");
        assert_eq!(lines[2], "    NEW.row_updated_date := NOW();");
        assert_eq!(lines[3], "    RETURN NEW;");
        assert_eq!(lines[4], "END;");
        assert_eq!(lines[5], "$$ LANGUAGE plpgsql");
    }

    // ── CREATE FUNCTION (increase_row_version — trigger with IF block) ──────

    #[test]
    fn formats_create_function_increase_row_version() {
        let sql = "CREATE FUNCTION increase_row_version() RETURNS TRIGGER AS\n$$\nBEGIN\n    IF OLD.row_version = NEW.row_version\n    THEN\n        NEW.row_version = OLD.row_version + 1;\n    END IF;\n    RETURN NEW;\nEND;\n$$ LANGUAGE plpgsql;";
        let formatted = format_statement(sql).unwrap();
        let lines: Vec<&str> = formatted.lines().collect();

        assert_eq!(lines.len(), 9);
        assert_eq!(
            lines[0],
            "CREATE FUNCTION increase_row_version() RETURNS trigger AS $$"
        );
        assert_eq!(lines[1], "BEGIN");
        assert_eq!(lines[2], "    IF OLD.row_version = NEW.row_version");
        assert_eq!(lines[3], "    THEN");
        assert_eq!(lines[4], "        NEW.row_version = OLD.row_version + 1;");
        assert_eq!(lines[5], "    END IF;");
        assert_eq!(lines[6], "    RETURN NEW;");
        assert_eq!(lines[7], "END;");
        assert_eq!(lines[8], "$$ LANGUAGE plpgsql");
    }

    // ── CREATE TABLE (no defaults, type alignment with constraints) ────────

    #[test]
    fn formats_create_table_no_defaults_aligns_types() {
        let sql = "CREATE TABLE wallet_labels (id bigserial NOT NULL, name text NOT NULL, PRIMARY KEY (id), UNIQUE (name))";
        let formatted = format_statement(sql).unwrap();
        let lines: Vec<&str> = formatted.lines().collect();

        assert_eq!(lines.len(), 6);
        assert_eq!(lines[0], "CREATE TABLE wallet_labels (");
        assert_eq!(lines[1], "    id   bigserial NOT NULL,");
        assert_eq!(lines[2], "    name text      NOT NULL,");
        assert_eq!(lines[3], "    PRIMARY KEY (id),");
        assert_eq!(lines[4], "    UNIQUE (name)");
        assert_eq!(lines[5], ")");
    }

    // ── FormatError ─────────────────────────────────────────────────────────

    #[test]
    fn format_error_display() {
        let err = FormatError::Parse("syntax error".into());
        assert_eq!(err.to_string(), "parse error: syntax error");

        let err = FormatError::Deparse("failed".into());
        assert_eq!(err.to_string(), "deparse error: failed");
    }
}
