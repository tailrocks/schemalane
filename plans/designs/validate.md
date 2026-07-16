# Validate command design

## Decision

Ship `schemalane migrate validate` as the online, read-only CI gate. It is a
subcommand rather than more `status` flags: `status` remains informational and
the command matches Flyway vocabulary. Delegation forwards the same subcommand
to embedded migration crates.

Validation reuses existing exit meanings:

- `0`: local files and applied history agree; pending files are allowed.
- `3`: a history row is missing locally or an applied checksum changed.
- `4`: the latest history row for any script failed.
- `5`: pending files exist and `--fail-on-pending` was requested.

Failed history wins over drift, matching `up` preflight. Validation performs
only history reads and takes no advisory lock. A concurrent migration can make
the result stale immediately after the read; this is acceptable for a CI gate
and is explicitly not a transaction reservation.

## CLI surface

```text
schemalane migrate validate [OPTIONS]

Options:
  --format <table|json>   Output format [default: table]
  --fail-on-pending      Return exit 5 when pending migrations exist
```

Human output reuses the status overview, table, and drift diagnostics. JSON is
an additive envelope, avoiding changes to the frozen `StatusReport` shape:

```json
{
  "report": {
    "schema": "public",
    "history_table": "flyway_schema_history",
    "migrations": [],
    "summary": {
      "success": 0,
      "pending": 0,
      "failed": 0,
      "missing": 0,
      "checksum_mismatch": 0
    }
  },
  "validation": { "valid": true }
}
```

## Engine seam

`SchemalaneMigrator::validate(&Pool) -> Result<StatusReport,
SchemalaneError>` calls the same status reconciliation used by the CLI, then
maps failed, missing, and checksum-mismatch states to existing errors. It does
not connect through the migration apply path and cannot mutate schema history.

## Specification addition

Add `validate` beside `status`: it compares resolved migrations with history,
does not apply migrations, permits pending migrations by default, and uses exit
codes 3, 4, and optionally 5 with their existing meanings.

## Prototype evidence

The retained prototype includes the core method, CLI parsing/rendering,
delegation forwarding, contract tests, and PostgreSQL integration assertions
for a clean database and a checksum mismatch.

## Follow-up build steps

1. Freeze table and JSON transcripts as CLI contract fixtures.
2. Add dedicated failed-history and missing-local integration cases.
3. Add the command to README/spec command tables.
4. Benchmark validation over a large history table and add one index only if
   measurements show a regression.
