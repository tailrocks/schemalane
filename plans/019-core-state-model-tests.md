# Plan 019: Unit-test the state model and gating logic; integration-test SQL failure, resume, and the advisory lock

> **Executor instructions**: Follow this plan step by step, verifying each step.
> On any STOP condition, stop and report. When done, update `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat dd0d79d..HEAD -- schemalane-core/src/lib.rs schemalane-core/tests/postgres_integration.rs`
> On mismatch with "Current state" excerpts, STOP (locate by symbol).

## Status

- **Priority**: P1 (characterization prerequisite for plans 020/021/028/031)
- **Effort**: M
- **Risk**: LOW
- **Depends on**: plans/001-ci-verification-baseline.md
- **Category**: tests
- **Planned at**: commit `dd0d79d`, 2026-07-03

## Why this matters

The engine's **decision layer** — which states migrations are in (`Success/Pending/Failed/Missing/ChecksumMismatch`, spec §7) and whether `up` is allowed to run at all (drift/failed-history gating, §7.1, exit codes 3/4) — consists of pure functions over plain structs, and has **zero unit tests**. The most safety-critical runtime behavior — a mid-file **SQL** failure rolling back and recording a `success=false` row, and the subsequent run being blocked until resolved — is untested (only *Rust* migration failures are). The advisory lock (spec §5, "prevent concurrent runners") has no test at all. Several upcoming plans refactor exactly these paths; they need characterization tests first.

## Current state

- Pure, untested decision functions in `schemalane-core/src/lib.rs`:
  - `build_status_report(schema, history_table, &[DiscoveredMigration], &[HistoryRow]) -> StatusReport` (lines 1087–1199) — five-state classification incl. the Failed-beats-mismatch precedence (`Some(row) if !row.success` arm first, line 1101) and Missing synthesis from history (1150–1164).
  - `ensure_no_blocking_history(&[DiscoveredMigration], &[HistoryRow])` (808–859) — collects failed / missing / checksum-mismatch; failed → `FailedHistory` (exit 4) takes precedence; else drift → `Drift` (exit 3).
  - `latest_history_by_script` (1551–1557) — last row per script wins (history ordered by `installed_rank` ASC from `load_history`, line 975).
- Obstacle: `DiscoveredMigration` (1799–1808), `HistoryRow` (1827–1838), and `MigrationSource` are **private** with private fields — tests in the same file's `#[cfg(test)] mod tests` (line 1916) can construct them directly; keep the new tests in that module.
- Runtime failure path (SQL): transactional execution + rollback at 1387–1398; failed-row write decision at 470–501 (post-plan-011 shape may differ slightly — locate by the `MixedStatements` comment).
- Advisory lock: `with_advisory_lock` (635–663), `pg_advisory_lock($1)` on `DEFAULT_ADVISORY_LOCK_ID` (or derived key if plan 014 landed — adapt the test to call whatever the code uses).
- Integration harness conventions: `schemalane-core/tests/postgres_integration.rs` — per-test container, helpers `connection_string`/`create_pool`/`write_migration`/`scalar_i64`/`table_exists`. All tests `#[ignore = "requires Docker daemon"]`.
- Exit-code mapping: `SchemalaneError::exit_code` (84–94).

## Commands you will need

| Purpose | Command | Expected |
|---|---|---|
| Unit | `cargo test -p schemalane-core --locked` | pass |
| Integration | `cargo test -p schemalane-core --locked --test postgres_integration -- --include-ignored` | pass |
| Gate | fmt + `cargo clippy --workspace --locked --all-targets --all-features -- -D warnings` | exit 0 |

## Scope

**In scope**: `schemalane-core/src/lib.rs` (test module only + a tiny private test-constructor helper if needed), `schemalane-core/tests/postgres_integration.rs`.
**Out of scope**: production-code changes (if a test exposes a bug, STOP and report — several known ones have their own plans); CLI tests (plan 023); macro tests (plan 024).

## Git workflow

- Branch: `advisor/019-core-state-model-tests`
- Suggested commit: `Add state-model unit tests and SQL-failure/lock integration tests`
- No push/PR without operator instruction.

## Steps

### Step 1: Fixture builders in the core test module

Add small builders to `mod tests` so cases read declaratively:

```rust
fn history_row(script: &str, rank: i32, success: bool, checksum: Option<i32>) -> HistoryRow {
    HistoryRow {
        installed_rank: rank,
        version: Some(script_version(script)),
        description: "d".to_owned(),
        migration_type: "SQL".to_owned(),
        script: script.to_owned(),
        checksum,
        installed_on: String::new(),
        execution_time: 1,
        success,
    }
}
```

and an analogous `discovered(script, checksum)` for `DiscoveredMigration` (use `MigrationSource::SqlFile(PathBuf::from(script))`; parse version via the same `parse_sql_filename` used in production; adjust to the `SqlFile { path, content }` shape if plan 018 landed). Extend the module's `use super::{…}` list as needed.

### Step 2: State-model unit tests (`build_status_report`)

One test per classification, asserting BOTH the entry state and the summary counts:

1. success: local `V1` + history(success, same checksum) → `Success`.
2. pending: local `V2`, no history → `Pending`, `installed_rank: None`.
3. failed precedence: history(success=false) + local with MATCHING checksum → `Failed` (not ChecksumMismatch — the `!row.success` arm wins).
4. checksum mismatch: history(success, checksum A) + local checksum B → `ChecksumMismatch`.
5. missing: history(success) with NO local file → synthesized `Missing` entry carrying the history row's fields.
6. retry-latest-wins: rows `V1 rank1 success=false`, `V1 rank2 success=true` (same checksum as local) → `Success` (latest-by-rank wins).
7. ordering: entries sorted by parsed version (e.g. `V2` before `V10`), Missing entries interleaved by their version.

### Step 3: Gating unit tests (`ensure_no_blocking_history`)

1. clean history → `Ok(())`.
2. failed row → `Err(FailedHistory)` and `.exit_code() == 4`.
3. missing local → `Err(Drift)`, exit 3.
4. checksum mismatch → `Err(Drift)`, exit 3.
5. failed AND drift together → `FailedHistory` wins (assert the error variant).
6. failed-then-succeeded same script (ranks 1,2) → `Ok(())` (latest wins).

### Step 4: SQL-failure integration tests

Model on `rust_migration_transaction_mode_rolls_back_on_failure` (line 271). New tests:

1. `sql_migration_failure_rolls_back_and_records_failed_row`: `V1` = two statements — `CREATE TABLE roll_a (id int);` then `SELECT * FROM missing_table_xyz;`. Expect: `up` errors with `MigrationExecution`; `table_exists("roll_a")` → false (rollback); history has exactly one row, `success=false` (query it directly like `rust_migration_success_and_history_type` does, line 244).
2. `failed_history_blocks_next_up_until_fixed`: after test 1's state, a second `up` → `FailedHistory` error (exit_code 4). (Resume-after-repair is impossible in-tool today — that asymmetry is direction spike `plans/038-spike-repair-command.md`; assert only the block.)
3. `mixed_statements_records_no_history_row`: `V1` = `CREATE TABLE t (id int); CREATE INDEX CONCURRENTLY i ON t (id);` → `MixedStatements` error; history table exists but has **zero** rows.
4. `non_transactional_sql_executes_outside_txn`: `V1` = single `CREATE INDEX CONCURRENTLY …` on a table created by `V0`… CONCURRENTLY needs the table pre-existing and can't run in the same file with its CREATE TABLE (mixed) — use two migrations: `V1__t.sql` (CREATE TABLE), `V2__idx.sql` (only the CONCURRENTLY index). Expect: both applied, index exists (`to_regclass`), 2 success rows.

### Step 5: Advisory-lock integration tests

1. `up_blocks_while_lock_held`: acquire the lock key on a raw side connection (`SELECT pg_advisory_lock($1)` with the same key the engine uses — `DEFAULT_ADVISORY_LOCK_ID` const, or `derive_advisory_lock_id("public","flyway_schema_history")` if plan 014 landed). Spawn `migrator.up(&pool)` under `tokio::time::timeout(Duration::from_secs(2), …)` → must time out (Err). Release the lock (`pg_advisory_unlock`), re-run `up` → succeeds.
2. `lock_released_after_successful_up`: after a normal `up`, `SELECT pg_try_advisory_lock($1)` from a side connection → returns true (lock free); unlock after.

Timeout-based tests can flake on slow machines — use generous bounds (≥2s) and document them.

### Step 6: Full gate

Unit + integration + fmt/clippy → all green. Count: ≥13 new unit tests, ≥6 new integration tests.

## Test plan

This plan IS the test plan (Steps 2–5). Patterns: unit tests follow the existing declarative style in `mod tests`; integration tests copy the container/TempDir scaffold of the existing 7.

## Done criteria

- [ ] `cargo test -p schemalane-core --locked` shows the new unit tests passing
- [ ] Integration run shows the ≥6 new tests passing
- [ ] No production code changed (`git diff --stat` touches only the two test locations; a private test-builder helper inside `#[cfg(test)]` counts as test code)
- [ ] fmt/clippy green; `plans/README.md` updated

## STOP conditions

- Any new test FAILS against current behavior — you likely rediscovered a known bug (cross-check plans 002/005/006/011/020) or found a new one. Report which test and the actual behavior; do not adjust the test to pass and do not fix production code here.
- Private-type construction from the test module hits visibility errors (types moved by plan 031) — relocate tests to the new module layout, or STOP if the shape changed semantically.
- Lock tests flake twice in a row — mark them `#[ignore = "timing-sensitive"]` with a comment, report, continue.

## Maintenance notes

- These are characterization tests: plans 020/021/028/031 must keep them green unchanged (except mechanical import moves). A refactor PR that edits their ASSERTIONS is changing behavior and must say so.
- The `Failed`-state entry currently renders history-row fields (version from history, not local) — Step 2.3 pins that; if that choice is ever revisited, it's a deliberate spec †decision, not a test bug.
- Follow-up (not here): property tests over version-ordering; differential Flyway container tests.
