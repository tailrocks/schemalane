# Plan 028: Deduplicate the engine's apply loop, statement executors, and transaction idioms

> **Executor instructions**: Follow this plan step by step, verifying each step.
> On any STOP condition, stop and report. When done, update `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat dd0d79d..HEAD -- schemalane-core/src/lib.rs`
> This plan REQUIRES the post-018/020/021 shape — read those plans' diffs first.

## Status

- **Priority**: P3
- **Effort**: M
- **Risk**: MED (core apply path)
- **Depends on**: plans/019-core-state-model-tests.md, plans/020-history-row-atomicity.md, plans/021-engine-connection-model.md (land first — deduplicating before they land would force re-dedup)
- **Category**: tech-debt
- **Planned at**: commit `dd0d79d`, 2026-07-03

## Why this matters

Three structural duplications in `schemalane-core/src/lib.rs` mean every apply-path fix must be written twice (and plan 011's fix WAS applied twice):

1. `up_with_observer` and `fresh_with_observer` share ~90 lines of per-migration orchestration (observer events, history writes, `MixedStatements` special-case, `Db → MigrationExecution` remap), differing only in skip-logic and index bookkeeping (`applied_index` vs `index + 1` — a drift that already happened).
2. `execute_statement_txn` (1420–1470) and `execute_statement_client` (1474–1524) are byte-identical except `txn.batch_execute` vs `client.batch_execute`.
3. Two transaction idioms coexist: typed `client.transaction()` for SQL (1388) vs raw `BEGIN`/`COMMIT`/`ROLLBACK` strings for Rust migrations (1533–1539) — different failure semantics for the same concept.

## Current state

(Line refs are `dd0d79d`; the shapes survive plans 018/020/021 with signature changes — locate by symbol.)

- `up_with_observer` 395–508 vs `fresh_with_observer` 533–633: compare side-by-side; the success arm (history write + `AppliedMigration` push + `on_migration_finish`) and error arm are near-verbatim copies.
- The two statement executors, and their shared observer-event bodies.
- Rust txn wrapper `execute_rust_migration` 1526–1543:

  ```rust
  RustTransactionMode::Transaction => {
      client.batch_execute("BEGIN").await?;
      match migration.up(client).await {
          Ok(()) => client.batch_execute("COMMIT").await,
          Err(err) => { let _ = client.batch_execute("ROLLBACK").await; Err(err) }
      }
  }
  ```

  Constraint that forces raw SQL here: `Transaction<'_>` borrows the client mutably, but Rust executors receive `&Client` (public API `RustMigrationFuture`), so the typed API can't wrap a user future that holds `&Client`. This idiom is **load-bearing** — document it, don't "fix" it (see Step 3).

- Characterization safety net: plan 019 suite + original integration tests.

## Commands you will need

| Purpose | Command | Expected |
|---|---|---|
| Gate | fmt + clippy gate + `cargo test --workspace --locked` | exit 0 |
| Integration (required) | `cargo test -p schemalane-core --locked --test postgres_integration -- --include-ignored` | all pass, zero assertion edits |

## Scope

**In scope**: `schemalane-core/src/lib.rs` only.
**Out of scope**: observer event struct redesign (the 6-events-80%-identical clump — deliberately deferred: public API churn for aesthetics; revisit post-1.0); history SQL centralization (plan 030); module split (plan 031).

## Git workflow

- Branch: `advisor/028-engine-dedup`
- Suggested commit: `Extract shared apply loop and statement executor`
- No push/PR without operator instruction.

## Steps

### Step 1: One `apply_all` loop

Extract the per-migration orchestration into a private method used by both entry points:

```rust
struct ApplyOptions {
    skip_applied: bool,   // up: true (uses applied_ok set); fresh: false
}

async fn apply_all<O: MigrationObserver + ?Sized>(
    &self,
    client: &mut tokio_postgres::Client,   // the plan-021 session connection
    migrations: &[DiscoveredMigration],
    applied_ok: &HashSet<String>,          // empty for fresh
    installed_by: &str,
    next_rank: &mut i32,
    observer: &O,
    options: ApplyOptions,
) -> Result<RunReport, SchemalaneError>
```

Body = the current up-loop verbatim (post-011/018/020 shape), with `skipped` counting only under `skip_applied`. Index bookkeeping: use one convention — `applied_index` (1-based over *applied* migrations) for both; **note**: `fresh` today numbers over ALL migrations (`index + 1`), and with `skip_applied=false` every migration is applied, so the two conventions coincide for fresh — no user-visible change. `up_with_observer` and `fresh_with_observer` keep their distinct preambles (gating vs schema reset) and both end in `self.apply_all(…)`.

### Step 2: One statement executor

Collapse `execute_statement_txn`/`execute_statement_client` via a minimal async-fn generic:

```rust
trait BatchExec {
    async fn batch(&self, sql: &str) -> Result<(), tokio_postgres::Error>;
}
impl BatchExec for tokio_postgres::Client { async fn batch(&self, s: &str) -> _ { self.batch_execute(s).await } }
impl BatchExec for tokio_postgres::Transaction<'_> { … }

async fn execute_statement<E: BatchExec, O: MigrationObserver + ?Sized>(
    executor: &E, stmt: &ParsedSqlStatement, index: usize, total: usize,
    migration: &MigrationInfo, observer: &O,
) -> Result<(), tokio_postgres::Error>
```

(Rust 2024 supports async fns in traits for private, non-dyn use — this trait is private and statically dispatched; if a lint/limitation bites, take a closure `impl Fn(&str) -> impl Future` instead — either is fine, pick what compiles cleanly.)

Delete the two old functions; both call sites in `execute_sql_migration` use the generic.

### Step 3: Document the Rust-migration transaction idiom

Add above `execute_rust_migration`:

```rust
/// Rust migrations opt into transactions via raw BEGIN/COMMIT because the
/// executor future borrows `&Client` (public API) and the typed
/// `client.transaction()` API would require `&mut Client` for its lifetime —
/// the two can't coexist. The best-effort ROLLBACK on failure is deliberate:
/// if ROLLBACK itself fails the connection is torn down by the session owner
/// (plan 021), which aborts the transaction server-side.
```

No code change.

### Step 4: Full gate + parity

fmt + clippy + workspace + integration → green with **zero assertion edits** (characterization requirement).

## Test plan

No new tests: plans 019/020's suites + originals prove behavior parity. `git diff --stat` should show `schemalane-core/src/lib.rs` shrinking by roughly 100+ lines.

## Done criteria

- [ ] `grep -c "on_migration_finish" schemalane-core/src/lib.rs` → 1 call site (in `apply_all`)
- [ ] `grep -n "fn execute_statement_txn\|fn execute_statement_client" schemalane-core/src/lib.rs` → gone
- [ ] All suites green unchanged; only core lib touched
- [ ] `plans/README.md` updated

## STOP conditions

- Any test assertion needs editing — parity broken; revert the step and report.
- The dependency plans (020/021) are not landed — do not attempt against `dd0d79d` shapes.
- The `BatchExec` trait hits an async-fn-in-trait limitation that forces `Box<dyn>`/lifetime contortions — switch to the closure form; if THAT also contorts, report rather than force it.

## Maintenance notes

- After this, apply-path changes have one home; reviewers should reject PRs that reintroduce per-command copies.
- The observer-event data-clump consolidation (six structs → shared inner) remains deliberately deferred — public API; bundle with a future breaking release.
- Plan 031's module split moves `apply_all` into `runner.rs` — this plan makes that move one function instead of two loops.
