# Plan 022: Stop discovering/checksumming/reading everything twice per CLI `up`/`fresh`

> **Executor instructions**: Follow this plan step by step, verifying each step.
> On any STOP condition, stop and report. When done, update `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat dd0d79d..HEAD -- schemalane-cli/src/lib.rs schemalane-core/src/lib.rs`
> Locate excerpts by symbol; earlier plans touched both files.

## Status

- **Priority**: P3
- **Effort**: M
- **Risk**: MED
- **Depends on**: plans/018-up-path-performance.md, plans/021-engine-connection-model.md (land both first — they reshape the functions this plan touches)
- **Category**: perf
- **Planned at**: commit `dd0d79d`, 2026-07-03

## Why this matters

`run_up_command` calls `migrator.status(pool)` to print the pre-flight overview, then `migrator.up_with_observer(pool, …)` — which re-runs discovery (every migration file read + CRC32'd again) and re-queries the full history table. Same pattern in `run_fresh_command`. Net: all file IO and the history query happen **twice** per invocation. For large migration sets this doubles startup latency; it is also a correctness sharpening — today's preflight and the run can observe *different* directory states (TOCTOU between the two discoveries).

The lock-ordering constraint that shapes the fix: the authoritative history read must stay **inside** the advisory lock (`up_with_observer` reads it under the lock; the CLI's preflight `status()` reads outside). So the CLI cannot simply pass its preflight data in — instead, core exposes the run's own view via the observer/report, and the CLI prints the overview from the run itself.

## Current state

- `schemalane-cli/src/lib.rs`, `run_up_command` (887–917):

  ```rust
  let status_before = migrator.status(pool).await?;      // discovery #1 + history #1
  print_status_overview(&status_before);
  print_pending_migrations(&status_before);
  ...
  let report = match migrator.up_with_observer(pool, &observer).await {   // discovery #2 + history #2
  ```

  `status_before` is also used for: `max_pending_script_len` (observer column width, 899), and `print_error_diagnostics(&status_before, &err)` on failure (910).

- `run_fresh_command` (919–999): same double pattern (`status(pool)` at 925, then `fresh_with_observer`).

- Core: `status()` (510–526) = discovery + optional history load + `build_status_report`. `up_with_observer` re-discovers (403) and re-loads history under the lock (410).

- After plan 021, `up_with_observer`'s body runs on one detached session; after plan 018, discovery is single-pass.

## Commands you will need

| Purpose | Command | Expected |
|---|---|---|
| Gate | fmt + clippy gate + `cargo test --workspace --locked` | exit 0 |
| Integration | `cargo test -p schemalane-core --locked --test postgres_integration -- --include-ignored` | pass |

## Scope

**In scope**: `schemalane-core/src/lib.rs` (one new pre-run hook on the observer OR a returned pre-report — see Step 1), `schemalane-cli/src/lib.rs`.
**Out of scope**: `status` command itself (single-pass already); changing what the overview *shows*; observer event struct redesign (plan 028 territory — add ONE method with a default impl, nothing more).

## Git workflow

- Branch: `advisor/022-cli-double-work`
- Suggested commit: `Derive CLI preflight output from the run itself`
- No push/PR without operator instruction.

## Steps

### Step 1: Emit the pre-run status through the observer

Add to `MigrationObserver` (default no-op, non-breaking for implementors):

```rust
/// Called once per run, after discovery and the authoritative history read
/// (inside the advisory lock), before the first migration is applied.
fn on_run_planned(&self, _report: &StatusReport) {}
```

In `up_with_observer`, after `load_history` + `ensure_no_blocking_history`, build the report from data already in hand and emit it:

```rust
observer.on_run_planned(&build_status_report(
    &self.config.schema,
    &self.config.history_table,
    &migrations,
    &history,
));
```

In `fresh_with_observer`, emit after discovery + schema reset + history-table creation with the (empty-history) report — everything is Pending, which is truthful for fresh.

### Step 2: CLI consumes `on_run_planned`

In `schemalane-cli/src/lib.rs`:

- `run_up_command`: delete the `migrator.status(pool)` preflight. `CliProgressObserver` implements `on_run_planned`: prints the overview + pending list (`print_status_overview`, `print_pending_migrations`) and captures what the error path needs — store the report in a `Mutex<Option<StatusReport>>` field (same pattern as `last_error`, lines 134/146–148) and derive `max_script_len` there (the observer's `max_script_len: usize` field becomes a `Mutex<usize>` or `AtomicUsize` set in `on_run_planned` — pick `Mutex<usize>`; construction-time width no longer available since there's no preflight).
- Error path: `print_error_diagnostics(&status_before, &err)` → use the captured report (`observer.planned_report()`); when `None` (failure before planning, e.g. validation/connection), print the error without drift diagnostics — match on that.
- `run_fresh_command`: same treatment. The schema preview (plan 002's single-schema line) needs no status call. The `--confirm` prompt happens BEFORE the run starts — but `on_run_planned` fires inside the run… ordering problem: the destructive confirmation must precede `fresh_with_observer`. Resolution: keep the prompt where it is (it needs no status data after plan 002), and let the overview print after confirmation via the observer. Verify the printed sequence reads sensibly: warning → schema line → prompt → overview → progress.

### Step 3: Delete now-dead helpers

`max_pending_script_len`/`max_script_len` free functions (80–97) fold into the observer; remove if unreferenced. Keep `print_status_overview`/`print_pending_migrations` (now called from the observer).

**Verify**: `cargo clippy -p schemalane-cli --locked --all-targets -- -D warnings` → exit 0 (dead-code catches leftovers).

### Step 4: Behavior check

Manual, with Docker Postgres + scratch migrations dir: `up` output shows overview exactly once, then progress; failure case (bad SQL) still shows drift/error diagnostics. Integration + unit suites green.

**Verify**: integration suite green; manual transcript sane.

## Test plan

- Existing integration tests exercise `up`/`fresh` through the core API (not the CLI printing) — they must stay green.
- CLI-level: extend plan 023's delegation/argv tests if landed; otherwise a unit test that `CliProgressObserver::on_run_planned` stores the report and updates the width (construct observer, feed a fixture `StatusReport`, assert `planned_report().is_some()`).
- Manual transcript check (Step 4).

## Done criteria

- [ ] `grep -n "status(pool)" schemalane-cli/src/lib.rs` → no calls in `run_up_command`/`run_fresh_command` (the `status` COMMAND arm keeps its call)
- [ ] `on_run_planned` exists with default impl; CLI implements it
- [ ] Gates + integration green; only in-scope files modified
- [ ] `plans/README.md` updated

## STOP conditions

- The observer trait is object-safe-consumed somewhere that a new defaulted method breaks (it shouldn't — default methods are non-breaking for `?Sized` trait objects) — if a compile error says otherwise, report it.
- Output ordering around the fresh confirmation cannot be made coherent (prompt after overview or double-print) — report the sequence you get; don't reshuffle the confirmation semantics.
- Plans 018/021 not landed (dependency) — do not attempt against the `dd0d79d` shapes; the line references here assume the post-018/021 structure.

## Maintenance notes

- `on_run_planned` is now the ONE place run-scoped preflight data flows to UIs — future TUI/JSON-progress consumers hook the same event (also the natural seam for spike 040's dry-run).
- Adding a defaulted trait method is non-breaking, but implementors overriding it exist only in this repo today; note it in release notes anyway (public trait).
