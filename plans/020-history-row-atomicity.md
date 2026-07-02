# Plan 020: Write the success history row inside the migration's own transaction (close the committed-but-unrecorded window)

> **Executor instructions**: Follow this plan step by step, verifying each step.
> On any STOP condition, stop and report. When done, update `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat dd0d79d..HEAD -- schemalane-core/src/lib.rs`
> Plans 011/018/019 legitimately touched this file — locate excerpts by symbol;
> STOP only if the apply/history structure is unrecognizable.

## Status

- **Priority**: P2
- **Effort**: M
- **Risk**: MED-HIGH (core apply path — do not start without plans 001 + 019 landed)
- **Depends on**: plans/001-ci-verification-baseline.md, plans/019-core-state-model-tests.md, plans/018-up-path-performance.md (rank counter + stored SQL content simplify this change; land 018 first)
- **Category**: bug
- **Planned at**: commit `dd0d79d`, 2026-07-03

## Why this matters

For transactional SQL migrations, the migration commits on one connection and the success history row is inserted afterwards on a **different** connection — two separate transactions. If the process dies between them, or the history INSERT itself fails, the database has the migration applied but no record of it: the next `up` re-applies it. Non-idempotent DML double-applies; DDL without `IF NOT EXISTS` errors and gets recorded as `success=false`, wedging all future runs behind exit 4. Flyway avoids this by writing the history row **in the migration's transaction** — commit makes both facts true atomically. Rust and non-transactional SQL migrations cannot get this guarantee (no wrapping transaction); for them, at-least-once semantics remain and get documented.

## Current state

(`schemalane-core/src/lib.rs`, shapes as of `dd0d79d`; plan 018 changes call-site details — the structure below still holds.)

- `up_with_observer`: history client acquired once (line 406); per-migration `apply_migration(pool, …)` (435) acquires its **own** connection (880/895) and, for transactional SQL, commits internally (`execute_sql_migration`, 1387–1398: `client.transaction()` → per-statement `batch_execute` → `txn.commit()`); the success row is then inserted on the outer `client` (440–448). `fresh_with_observer` mirrors this (569–581).
- `insert_history_row` (1020–1051): builds `INSERT INTO {table} (…) VALUES ($1..$9)` with fully parameterized values; executes on `&Client`. (Post-018: takes an explicit `installed_rank: i32`.)
- `execute_sql_migration(client, sql, migration_info, observer)` (1372–1417): decides tx-mode (`resolve_sql_transaction_mode`) and runs either the transactional loop (with rollback on statement failure) or the non-transactional loop.
- Failure semantics that MUST be preserved: the `success=false` row is written on the **outer** client precisely so it survives the migration transaction's rollback (up: `Err` arm, ~470–501). Do not move failed-row writes.
- `tokio_postgres::Transaction` implements the same `execute`/`batch_execute` query surface as `Client`, so the INSERT can run on the transaction.

## Commands you will need

| Purpose | Command | Expected |
|---|---|---|
| Gate | fmt + `cargo clippy --workspace --locked --all-targets --all-features -- -D warnings` + `cargo test --workspace --locked` | exit 0 |
| Integration (required) | `cargo test -p schemalane-core --locked --test postgres_integration -- --include-ignored` | all pass, incl. plan-019 suite |

## Scope

**In scope**: `schemalane-core/src/lib.rs`, `schemalane-core/tests/postgres_integration.rs` (new tests).
**Out of scope**: connection consolidation (plan 021 — coordinate: if 021 landed first, the "outer client" may be the same session; the transactional-insert principle is unchanged); Rust-migration atomicity (impossible without wrapping user futures in a txn they already control — documented instead); observer event shape.

## Git workflow

- Branch: `advisor/020-history-row-atomicity`
- Suggested commit: `Record success history row inside the migration transaction`
- No push/PR without operator instruction.

## Steps

### Step 1: Give the SQL executor the history write

Refactor so the transactional path can insert before commit. Concrete shape — add a parameter carrying everything the INSERT needs:

```rust
struct HistoryWrite<'a> {
    table_sql: String,          // qualified_table(schema, history_table)
    installed_rank: i32,
    version: &'a str,
    description: &'a str,
    migration_type: &'a str,
    script: &'a str,
    checksum: Option<i32>,
    installed_by: &'a str,
}
```

- Extract the INSERT text + parameter binding from `insert_history_row` into a helper usable with both `&Client` and `&Transaction` — simplest: make it take the SQL + params and a generic executor via two thin wrappers (mirror how `execute_statement_txn`/`execute_statement_client` already coexist), or duplicate the 10-line INSERT for the txn case with a comment tying the column lists together (`plans/030-history-repository-seam.md` will unify).
- `execute_sql_migration` gains a `history: &HistoryWrite<'_>` parameter (both `up` and `fresh` record success rows, so both pass it). In the **Transactional** arm, after the statement loop and **before** `txn.commit()`:

  ```rust
  // Atomic with the migration: both commit or neither does (Flyway parity).
  insert_history_row_txn(&txn, history_write, execution_time_ms_so_far).await?;
  txn.commit().await?;
  ```

  Execution-time caveat: the recorded `execution_time` now excludes the commit itself and is measured *inside* apply rather than around it. Measure from the start of `apply_migration`'s work to just before the INSERT — a small semantic shift (dozens of ms); keep the field meaning "statement execution time". Thread `started: Instant` in.
- In the **NonTransactional** arm and the **Rust** arm: leave the post-hoc outer-client insert exactly as today (at-least-once).

### Step 2: Skip the outer insert when the executor already wrote it

`apply_migration` returns an enum instead of `()`:

```rust
enum Applied {
    HistoryRecorded,       // transactional SQL: row committed with migration
    NeedsHistoryRow,       // non-transactional SQL, Rust migrations
}
```

In `up_with_observer`/`fresh_with_observer` success arms: call `insert_history_row` only for `NeedsHistoryRow`. Rank bookkeeping (`next_rank`, `applied_ok` from plan 018) is identical in both arms — increment regardless.

Failure arms: unchanged (outer failed-row write; a rolled-back transactional migration also rolled back its would-be success row — correct).

### Step 3: Document the semantics on the public API

Rustdoc on `SchemalaneMigrator::up` (and `fresh`):

```rust
/// Transactional SQL migrations commit their history row atomically with the
/// migration itself. Non-transactional SQL (e.g. CREATE INDEX CONCURRENTLY)
/// and Rust migrations record history after execution — if the process dies
/// in that window, the migration re-runs on the next `up` (at-least-once);
/// make such migrations idempotent.
```

### Step 4: Tests

Integration (model on plan-019 additions):

1. `transactional_migration_and_history_commit_atomically`: normal 2-statement migration → 1 success row (regression of the happy path through the new INSERT-in-txn).
2. `failed_transactional_migration_leaves_only_failed_row`: failing 2nd statement → table absent, exactly ONE row with `success=false` (proves the in-txn success INSERT rolled back with the migration and didn't double-write).
3. Re-run plan 019's whole suite unchanged.

The crash-window itself (kill between commit and insert) is not deterministically testable in-process; the atomicity argument is structural — reviewer verifies the INSERT is inside the txn by reading the diff.

**Verify**: full integration run green.

### Step 5: Full gate

fmt + clippy + workspace + integration → all green.

## Test plan

As Step 4; plan 019's characterization suite is the safety net and must pass without assertion edits.

## Done criteria

- [ ] In the transactional arm, the history INSERT executes on the `Transaction` before `commit()` (visible in diff; grep `insert_history_row_txn`)
- [ ] `Applied::NeedsHistoryRow` outer-insert path used only for non-transactional/Rust
- [ ] New + plan-019 integration tests green; fmt/clippy/unit green
- [ ] Only in-scope files modified; `plans/README.md` updated

## STOP conditions

- Plan 019's tests fail with assertion-level differences (row counts, states) — the refactor changed semantics; revert and report.
- The borrow checker forces restructuring beyond `execute_sql_migration`'s signature (e.g. observer + txn lifetimes conflict) into public API changes — report the design corner rather than expanding scope.
- Plan 021 landed and consolidated connections in a way that already made this trivial or conflicting — read its diff first; if the insert already rides the migration txn, mark this plan DONE-by-021 in the index.

## Maintenance notes

- `installed_rank` is now consumed inside the txn for transactional migrations; the counter in the caller must stay the single source (plan 018). If `plans/030-history-repository-seam.md` lands later, both INSERT sites collapse into the repository.
- Reviewer focus: (a) failed-row path untouched, (b) no path inserts twice, (c) execution_time semantics note in the PR description.
- At-least-once for Rust/non-transactional is now DOCUMENTED behavior — future `repair` (spike 038) is the operator remedy.
