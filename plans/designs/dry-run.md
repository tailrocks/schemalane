# `up --dry-run` design

## Decision

Dry-run is an option on `up`, not a separate command, because its contract is
"the same pending set and preflight failures as the next `up`, without writes."
It requires a database connection: pending state and drift cannot be inferred
offline. Offline source validation remains the proposed `check` command.

The planner does not take the advisory lock. It reads history once and can be
stale if another process migrates concurrently; output is a preview, not a
reservation. It does run the same discovery, Rust registration, failed-history,
drift, parser, and mixed-transaction gates as `up`.

## CLI

```text
schemalane migrate up [OPTIONS]

Options:
  --dry-run              Print the ordered execution plan without applying it
  --format <table|json>  Dry-run output format [default: table]
```

Human output is payload on stdout and terminal diagnostics remain on stderr:

```sql
-- V1__users.sql (V1, SQL, transaction)
CREATE TABLE users (
    id bigint
);

-- V2__backfill.rs (V2, RUST, source not previewable)
-- RUST (source not previewable)
```

Human SQL goes through `pg_query_fmt` and is display-only. JSON always carries
raw SQL exactly as core split it:

```json
{
  "schema": "public",
  "history_table": "flyway_schema_history",
  "migrations": [
    {
      "version": "1",
      "script": "V1__users.sql",
      "type": "SQL",
      "transaction_mode": "transactional",
      "statements": ["CREATE TABLE users (id bigint)"]
    }
  ]
}
```

## Engine seam

`SchemalaneMigrator::plan_up(&Pool) -> Result<UpPlan, SchemalaneError>` returns
an ordered `UpPlan` containing pending `PlannedMigration` values and a
`PlannedTransactionMode`. SQL statement strings are raw execution facts; Rust
migrations have no statement payload. Observer callbacks remain execution-only.

## Prototype evidence

The retained prototype wires standalone and delegated CLI parsing, table/JSON
rendering, structured core planning, and integration assertions. The pending
fixture proves ordered raw statements; a checksum-drift fixture proves the plan
fails with exit condition 3 before producing executable content.

## Race evidence

No correctness claim depends on a lock: `plan_up` performs no writes and returns
a snapshot. A concurrent `up` may make entries already applied by the time a
human acts, which is explicitly documented. Callers requiring reservation must
run real `up`, which takes the lock and repeats every gate.

## Follow-up build steps

1. Freeze human and JSON transcripts as CLI contract fixtures.
2. Add failed-history, mixed-mode, Rust, and empty-plan integration cases.
3. Add command documentation and shell completion snapshots.
4. Consider `fresh --dry-run` only in a separate destructive-semantics plan.
