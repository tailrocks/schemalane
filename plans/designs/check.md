# Offline check command design

## Decision

`schemalane migrate check` is the database-free sibling of online `validate`.
Version 1 runs only analyses already enforced by `up`: filename syntax,
semantic duplicate versions, duplicate scripts, PostgreSQL parsing, and mixed
transaction mode rejection. Rust files receive filename/duplicate checks;
embedded mode may additionally verify that every Rust file has a registered
executor. No lint rule ships in v1.

Exit codes are identical to execution preflight: ordinary discovery/parser
validation is 2 and mixed transaction modes are 7. That equivalence is the
command's contract.

## CLI and output

```text
schemalane migrate check [OPTIONS]

Options:
  --format <table|json>  Output format [default: table]
```

No database URL is read or required. Human output is one row per file with
version, type, statement count, transaction mode, and `OK`; errors retain their
normal diagnostic and exit code. Stable JSON shape:

```json
{
  "valid": true,
  "migrations": [
    {
      "version": "1",
      "script": "V1__create_users.sql",
      "type": "SQL",
      "statement_count": 2,
      "transaction_mode": "transactional"
    }
  ],
  "warnings": []
}
```

## Executor registration

Standalone mode skips Rust executor registration because it has no generated
factory. Embedded/delegated mode verifies registration because the macro-built
migrator contains the real executor map. The report records Rust transaction
mode when known and `unknown` otherwise.

## Exact core API required

The existing discovery, parsed statements, and transaction-mode types are all
`pub(crate)` by design. A prototype cannot reuse them from the CLI without one
new intentional façade and result model:

```rust
SchemalaneMigrator::check(
    verify_rust_executors: bool,
) -> Result<CheckReport, SchemalaneError>

pub struct CheckReport { pub migrations: Vec<CheckedMigration> }
pub struct CheckedMigration {
    pub version: String,
    pub script: String,
    pub migration_type: String,
    pub statement_count: Option<usize>,
    pub transaction_mode: Option<CheckedTransactionMode>,
}
pub enum CheckedTransactionMode { Transactional, NonTransactional }
```

No discovery or AST type should become public. The spike's explicit stop
condition requires sign-off before adding this published surface, so no
prototype is retained. Duplicating the parser in the CLI would violate the
single-source validation promise and was rejected.

## Test plan

1. Clean SQL directory returns a deterministic ordered report and exit 0.
2. Invalid filename returns 2.
3. Semantically duplicate version (`V1`/`V1.0`) returns 2.
4. Duplicate script returns 2 (fixture/injected filesystem case).
5. Invalid SQL returns 2.
6. Mixed transactional/non-transactional SQL returns 7.
7. Standalone Rust file skips registration.
8. Embedded Rust file missing its executor returns 2.
9. Neither mode reads `DATABASE_URL` or opens a pool.

## Lint seam appendix

Reserve `warnings: []` in JSON and IDs shaped `SLnnn`, but do not reserve a CLI
flag yet. Candidate future rules, each backed by existing AST facts:

- `SL001`: non-concurrent `CREATE INDEX` (`IndexStmt.concurrent == false`).
- `SL002`: maintenance commands in migrations (`VacuumStmt`/`ReindexStmt`).
- `SL003`: potentially scan-heavy `ALTER TABLE ... SET NOT NULL`
  (`AlterTableCmd` subtype).

Warnings never change exit status until a later plan defines policy.

## Build-plan sketch

1. Approve the three result types and `check` method above.
2. Implement the façade inside core using private discovery/SQL analysis.
3. Add offline dispatch before database URL resolution in standalone and
   embedded runners; forward delegation unchanged.
4. Freeze table/JSON output and implement the nine tests.
5. Document `check` and the distinction from online `validate`.
