# schemalane-core guidance

## Ownership

- `src/lib.rs` is the documented public facade; implementation belongs in the
  focused modules it re-exports.
- Core owns migration discovery, checksums, state reconciliation, history SQL,
  session locking, and execution. It must remain independent of CLI rendering
  and `pg_query_fmt`.

## Behavioral invariants

- `fresh` drops and recreates only `SchemalaneConfig::schema`, never unrelated
  schemas in the same database.
- One detached connection owns a migration session, search path, advisory lock,
  execution, and unlock lifecycle.
- Transactional SQL and its success history row commit atomically. Failed rows
  remain durable; non-transactional and Rust migrations retain their documented
  at-least-once semantics.
- History decisions use the latest row per script. Advisory-lock derivation is
  stable for `(schema, history_table)` unless explicitly overridden.
- Parse and checksum migration bytes once during discovery; execution reuses the
  discovered content.

## Safety and API

- Route all history-table SQL through `HistoryRepository`.
- Quote schema/table identifiers with `quote_ident` or `qualified_table`; bind
  values as PostgreSQL parameters.
- Preserve statement source-line reporting and the original database error when
  observer/reporting work also fails.
- `#![deny(missing_docs)]` is deliberate. Public items, fields, and variants need
  accurate rustdoc; public compatibility changes need semver review.

## Checks

```sh
cargo nextest run -p schemalane-core --locked
RUSTDOCFLAGS="-D missing-docs" cargo doc -p schemalane-core --no-deps --locked
cargo nextest run -p schemalane-core --locked --test postgres_integration --run-ignored all
```

The last command requires Docker and is the authoritative database-behavior gate.
