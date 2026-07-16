# Schemalane v1 Specification (Draft)

## 1. Scope

Schemalane v1 is a PostgreSQL-only, forward-only migration toolkit with a Flyway-compatible history table and operational model.

### 1.1 In Scope

- Migration formats:
  - SQL migrations (primary)
  - Rust migrations (for complex logic)
- Usage modes:
  - As a Rust crate (via `schemalane-core`)
  - As an embedded tool in application binaries
  - As a CLI (via `schemalane-cli` crate)
  - As a programmatic migrator API
- Commands:
  - `init`
  - `up`
  - `status`
  - `fresh`
- Driver stack:
  - tokio-postgres driver

### 1.2 Out of Scope (v1)

- MySQL or SQLite support
- `down`, `undo`, `reset`, or `refresh`
- Repeatable (`R`), baseline (`B`), or undo (`U`) migration types

## 2. Command Surface

Schemalane CLI namespace:

- `schemalane init`
- `schemalane migrate up`
- `schemalane migrate status`
- `schemalane migrate fresh`

`init` lives at the CLI root. Database commands live under `migrate`.

### 2.1 Common Flags (`up`, `status`, `fresh`)

- `-d, --migration-dir <path>` (env: `MIGRATION_DIR`, default: `./migration`)
- `--database-url <postgres://...>`
- `--schema <schema_name>` (default: `public`)
- `--history-table <name>` (default: `flyway_schema_history`)
- `--installed-by <name>` (default: current DB user)
- `--verbosity <minimal|compact|detailed>` (default: `minimal`; affects `up`
  and `fresh` progress output)

### 2.2 Command-Specific Flags

- `schemalane init`
  - `--path <path>` (default: `./migration`)
  - `--force` (overwrite existing scaffold files)
- `schemalane migrate status`
  - `--format table|json` (default: `table`)
  - `--fail-on-pending`
- `schemalane migrate fresh`
  - `--confirm yes` (required when non-interactive; interactive terminals prompt)

When `--migration-dir` points to a migration crate with `Cargo.toml`, CLI execution delegates to:
`cargo run --manifest-path <migration_dir>/Cargo.toml -- ...`.

### 2.3 `init` Scaffold Output

`schemalane init` creates a standalone migration crate with:

- a runnable CLI (`src/main.rs`)
- a reusable migrator builder (`src/lib.rs`)
- SQL and Rust sample migrations in one folder (`./migrations`)
- `embed_migrations!("./migrations")` in `src/lib.rs` for auto Rust migration detection

### 2.4 Embedded Registration

Embedded mode uses macro-based registration:

- `embed_migrations!("<dir>")` scans Rust migration files at compile time
- generates `migrations::build_migrator(config)` and `migrations::MIGRATIONS_DIR`
- generates `migrations::runner()` for shared embedded CLI execution via `schemalane-cli`
- avoids manual migration module lists in `src/lib.rs`

## 3. Migration Discovery and Parsing

Schemalane builds one ordered migration stream from SQL and Rust files in the same directory.

### 3.1 SQL Naming Rules

- Required pattern: `V<version>__<description>.sql`; the separator and
  description may be omitted when the description is empty, matching Flyway
- `<version>` uses Flyway numeric dotted notation; underscores are normalized
  to dots, numeric parts may be arbitrarily large, and trailing zero parts do
  not affect ordering
- `<description>` is the raw text after the first separator; Schemalane does
  not apply an additional character whitelist
- Display description: underscores converted to spaces, matching Flyway

Examples:

- `V1__init.sql`
- `V1.sql`
- `V2_1__add_indexes.sql`
- `V2026.02.24.1__price_histories.sql`
- `V10__bitcoin_transaction.import_status.default.sql`
- `V11__My-description.data load.sql`

### 3.2 Rust Migration Identity Rules

Rust migration files follow: `V<version>__<description>.rs`; the separator and
description may be omitted when the description is empty, matching Flyway

- `<version>` follows the same Flyway numeric dotted notation as SQL
  migrations
- `<description>` follows the same raw-description parsing as SQL migrations
- `script` is the filename
- `checksum` is calculated from Rust file content
- `type = RUST`

Rust migrations participate in the same global version ordering as SQL migrations.

### 3.3 Validation Rules

Startup validation errors (hard fail):

- Invalid filename/metadata format
- Duplicate versions across SQL and Rust migrations
- Duplicate script names
- Non-PostgreSQL URL

## 4. Execution Model

### 4.1 Forward-Only

- All migrations are forward-only.
- To undo a change, create a new higher-version migration.
- No `down`/`undo` operations exist.

### 4.2 SQL Migration Execution

SQL migrations are transactional by default and executed via tokio-postgres connection APIs:

```rust
let mut client = pool.get().await?;
let transaction = client.transaction().await?;
for statement in parsed_statements {
    transaction.batch_execute(&statement.sql).await?;
}
transaction.commit().await?;
```

Requirements:

- One SQL file may contain multiple SQL statements.
- On failure, rollback when possible.

Transaction handling: SQL files are parsed with PostgreSQL's parser. Statements
that cannot run in a transaction block (`CREATE INDEX CONCURRENTLY`, `DROP INDEX
CONCURRENTLY`, `VACUUM`, `REINDEX SCHEMA|DATABASE|SYSTEM`, `DISCARD ALL`, `ALTER
SYSTEM`, `CREATE|DROP DATABASE`, `CREATE|DROP TABLESPACE`, and `CREATE|DROP
SUBSCRIPTION`) make the whole file run non-transactionally. Mixing transactional
and non-transactional statements in one file is rejected with exit code 7,
matching Flyway's `mixed=false` default.

### 4.3 Rust Migration Execution

- Rust migrations are non-transactional by default.
- Each migration may opt into its own transaction strategy explicitly.

### 4.4 Target Schema Setup

Before running migrations, schemalane:

- Creates the configured schema if missing (`CREATE SCHEMA IF NOT EXISTS "<schema>"`).
- Sets the session `search_path` to the configured schema (`SET search_path TO "<schema>"`) on every connection used to execute a migration.

Both behaviors mirror Flyway's handling of `-schemas=<name>`. They ensure unqualified DDL in user migrations (e.g. `CREATE TABLE foo (...)`) lands in the target schema rather than `public`.

## 5. PostgreSQL Locking

Schemalane acquires a single PostgreSQL advisory lock for the full migration session (`up` and `fresh`) to prevent concurrent runners.

- Acquire lock before reading history and applying migrations.
- Release lock after completion (or on error via cleanup path).

## 6. History Table (Flyway-Compatible)

Default fully-qualified table name:

- `"public"."flyway_schema_history"` (schema configurable)

### 6.1 DDL

```sql
CREATE TABLE IF NOT EXISTS "public"."flyway_schema_history" (
    "installed_rank" INTEGER NOT NULL,
    "version" VARCHAR(50),
    "description" VARCHAR(200) NOT NULL,
    "type" VARCHAR(20) NOT NULL,
    "script" VARCHAR(1000) NOT NULL,
    "checksum" INTEGER,
    "installed_by" VARCHAR(100) NOT NULL,
    "installed_on" TIMESTAMP NOT NULL DEFAULT now(),
    "execution_time" INTEGER NOT NULL,
    "success" BOOLEAN NOT NULL,
    CONSTRAINT "flyway_schema_history_pk" PRIMARY KEY ("installed_rank")
);

CREATE INDEX IF NOT EXISTS "flyway_schema_history_s_idx"
    ON "public"."flyway_schema_history" ("success");
```

### 6.2 Write Semantics

For every migration attempt:

- Insert one row with:
  - next `installed_rank`
  - migration metadata (`version`, `description`, `type`, `script`)
  - `checksum`
  - `installed_by`
  - `execution_time` in milliseconds
  - `success = true|false`

Failed attempts are recorded (`success = false`) and surfaced in `status`.

### 6.3 Checksum Algorithm

`checksum` is a Flyway-compatible CRC-32 (IEEE polynomial) over the migration content:

- Read content as UTF-8.
- Iterate by line, splitting on `\n` or `\r\n` (matches `BufferedReader.readLine()`).
- Update the CRC-32 with each line's UTF-8 bytes; **do not** include the line terminator.
- Take the resulting `u32` and reinterpret its bit pattern as a signed `i32` (matches Java's `(int) crc32.getValue()`).

This produces byte-identical `checksum` values to Flyway for SQL files that don't contain lone `\r` characters or BOMs.

## 7. Status State Model

`status` evaluates local migrations and history rows into these states:

- `Success`:
  - Applied row exists with `success = true`
  - Checksum matches current local migration
- `Pending`:
  - Local migration not present in successful history rows
- `Failed`:
  - History row exists with `success = false`
- `Missing`:
  - Successful history row has no corresponding local migration
- `ChecksumMismatch`:
  - Successful history row exists for same migration identity, checksum differs

### 7.1 Drift Definition

Drift is any migration in:

- `Missing`
- `ChecksumMismatch`

## 8. Exit Codes

- `0`: success
- `1`: runtime/config/database error
- `2`: migration validation error
- `3`: drift detected (`Missing` or `ChecksumMismatch`)
- `4`: failed migration present (`success = false`)
- `5`: pending migrations found with `--fail-on-pending`
- `6`: destructive guard violation (`fresh` without `--confirm yes`)
- `7`: migration mixes transactional and non-transactional statements

## 9. `fresh` Semantics

`fresh` is destructive and requires `--confirm yes` in non-interactive contexts;
interactive terminals prompt for confirmation.

Execution sequence:

1. Acquire advisory lock.
2. Validate migration set.
3. Drop all user tables in target schema (including history table).
4. Recreate `flyway_schema_history`.
5. Execute `up`.
6. Release lock.

`fresh` never drops the PostgreSQL database itself.

## 10. Programmatic API

The programmatic engine is `schemalane_core::SchemalaneMigrator`. Construct a
`SchemalaneConfig`, create the migrator, then pass a
`&deadpool_postgres::Pool` to `up`, `status`, or `fresh`:

```rust
let config = schemalane_core::SchemalaneConfig::new()
    .with_schema("public")
    .with_migrations_dir("./migrations");
let migrator = schemalane_core::SchemalaneMigrator::new(config);
let report = migrator.up(&pool).await?;
```

`up_with_observer` and `fresh_with_observer` accept a `MigrationObserver` for
run, migration, and SQL-statement lifecycle events. Rust migrations are
registered with `register_rust_migration` and a `RustMigrationExecutor`.
`init_migration_project(&Path, force)` scaffolds embedded-crate mode, whose
generated entry point uses `schemalane_macros::embed_migrations!` and
`schemalane_cli::EmbeddedRunner`.

Transactional SQL migrations commit their successful history row atomically
with their SQL. Non-transactional SQL and Rust migrations record history only
after execution and therefore have at-least-once semantics; those migrations
must be idempotent.

All four usage modes (crate, embedded, CLI, and programmatic) share this core
engine. Rustdoc owns exact signatures; this specification owns behavior.
