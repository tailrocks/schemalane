# Plan 018: Cut redundant work on the `up`/`fresh` hot path (history map rebuilds, per-insert MAX query, double directory walk, double SQL read)

> **Executor instructions**: Follow this plan step by step, verifying each step.
> On any STOP condition, stop and report. When done, update `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat dd0d79d..HEAD -- schemalane-core/src/lib.rs`
> On mismatch with "Current state" excerpts, STOP (locate by symbol, not line).

## Status

- **Priority**: P2
- **Effort**: S
- **Risk**: LOW
- **Depends on**: plans/001-ci-verification-baseline.md (integration tests verify behavior parity)
- **Category**: perf
- **Planned at**: commit `dd0d79d`, 2026-07-03

## Why this matters

Four measured redundancies, all in `schemalane-core/src/lib.rs`, all mechanical to fix:

1. **O(N·H) map rebuilds**: `is_applied_success` rebuilds the full latest-history `HashMap` (H rows) on every call, and it's called ~2N times per `up` (once in the pre-count filter, once per loop iteration). 2,000 migrations × 2,000 history rows ≈ 8M wasted hash insertions per run.
2. **N+1 rank queries**: `insert_history_row` runs `SELECT COALESCE(MAX("installed_rank"),0)+1` per insert — one serialized round-trip per applied migration (worst on `fresh`, which applies everything). The value is derivable in memory: writes happen under the advisory lock.
3. **Double directory walk**: `discover_sql_migrations` and `discover_rust_migrations` each `read_dir` the same directory.
4. **Double file read**: a pending SQL file is read for its checksum at discovery and read **again** in `apply_migration`.

## Current state

- (1) `is_applied_success` (lines 1545–1549) + `latest_history_by_script` (1551–1557):

  ```rust
  fn is_applied_success(migration: &DiscoveredMigration, history: &[HistoryRow]) -> bool {
      latest_history_by_script(history)
          .get(migration.script.as_str())
          .is_some_and(|row| row.success && row.checksum == migration.checksum)
  }
  ```

  Call sites in `up_with_observer`: pre-count filter (414–417) and loop guard (421). After each success, `history.push(HistoryRow::from_migration(…))` (449–454) keeps the Vec current.

- (2) `next_installed_rank` (1010–1018), called from `insert_history_row` (1028), which is called at up:441/476 and fresh:574/601.

- (3) `discover_sql_migrations` (695–740) and `discover_rust_migrations` (742–781): same `std::fs::read_dir(&self.config.migrations_dir)` loop, differing only in extension + parser + `MigrationType`/`MigrationSource`.

- (4) Discovery reads content for checksums (`std::fs::read(&path)` at 724/765); `apply_migration` re-reads (`std::fs::read_to_string(path)` at 874).

- `DiscoveredMigration` (1799–1808) and `MigrationSource::{SqlFile,RustFile}(PathBuf)` (1821–1825).

- Advisory-lock guarantee that makes (2) safe: all history writes happen inside `with_advisory_lock` (up:405, fresh:549); rank races require an out-of-band writer, which is outside the documented model (Flyway assumes the same).

## Commands you will need

| Purpose | Command | Expected |
|---|---|---|
| Gate | fmt + `cargo clippy --workspace --locked --all-targets --all-features -- -D warnings` + `cargo test --workspace --locked` | exit 0 |
| Behavior parity | `cargo test -p schemalane-core --locked --test postgres_integration -- --include-ignored` | all pass, unchanged assertions |

## Scope

**In scope**: `schemalane-core/src/lib.rs`.
**Out of scope**: the CLI's duplicate `status()` call before `up` and per-migration connection churn — `plans/022-cli-double-work.md` / `plans/021-engine-connection-model.md`; caching file *contents* in `DiscoveredMigration` beyond what step 4 does; any public-API change.

## Git workflow

- Branch: `advisor/018-up-path-performance`
- Suggested commit: `Cut redundant history scans, rank queries, and file reads on up path`
- No push/PR without operator instruction.

## Steps

### Step 1: Build the latest-history map once per operation

In `up_with_observer`: before the pre-count, build `let mut latest: HashMap<String, HistoryRow-ish>` — simplest faithful shape:

```rust
let mut applied_ok: std::collections::HashSet<String> = {
    let latest = latest_history_by_script(&history);
    migrations
        .iter()
        .filter(|m| {
            latest
                .get(m.script.as_str())
                .is_some_and(|row| row.success && row.checksum == m.checksum)
        })
        .map(|m| m.script.clone())
        .collect()
};
```

Then: `total_to_apply = migrations.iter().filter(|m| !applied_ok.contains(&m.script)).count()`, loop guard `if applied_ok.contains(&migration.script)`, and after each successful apply `applied_ok.insert(migration.script.clone());` **replaces** the `history.push(…)` bookkeeping — delete the now-unneeded `history` mutation and `HistoryRow::from_migration` call *if* nothing else reads `history` after the loop (verify: in `up_with_observer` nothing does; `ensure_no_blocking_history` ran before). Keep `is_applied_success`/`latest_history_by_script` for other callers (`build_status_report` uses the latter).

Note: `is_applied_success` loses its only caller → delete it (private fn; clippy dead-code will confirm).

### Step 2: Thread an in-memory rank counter

Change `insert_history_row` signature to take the rank: `async fn insert_history_row(&self, client, migration, installed_by, execution_time, success, installed_rank: i32)`. In `up_with_observer`, seed once before the loop:

```rust
let mut next_rank: i32 = history.iter().map(|r| r.installed_rank).max().unwrap_or(0) + 1;
```

In `fresh_with_observer` (history table just recreated): `let mut next_rank: i32 = 1;`.

At each call site pass `next_rank` and increment after a **successful** insert (both success and failed rows consume a rank — increment after any insert that returned Ok). Delete `next_installed_rank` (loses its only caller).

Return value: `insert_history_row` currently returns the rank (used at up:440-448 for the deleted `HistoryRow::from_migration`) — after Step 1 nothing needs it; change to `Result<(), SchemalaneError>`.

### Step 3: Single-pass directory discovery

Merge the two loops into `discover_migrations`: one `read_dir`, classify per entry by `eq_ignore_ascii_case` extension into SQL/Rust arms (reuse `parse_sql_filename`/`parse_rust_filename` exactly as today), push `DiscoveredMigration` with the right `MigrationType`/`MigrationSource`. Delete `discover_sql_migrations`/`discover_rust_migrations`. Keep the missing-directory validation error text identical (`"migrations directory not found: …"` — currently only the SQL path checks it; the merged loop checks once up front, which also fixes the quirk that a missing dir surfaced from the SQL half only).

### Step 4: Reuse discovery's bytes for SQL execution

`MigrationSource::SqlFile(PathBuf)` → `MigrationSource::SqlFile { path: PathBuf, content: String }`.

In the discovery SQL arm, validate UTF-8 exactly once and share the result:

```rust
let bytes = std::fs::read(&path)?;
let content = String::from_utf8(bytes).map_err(|err| {
    SchemalaneError::Validation(format!(
        "migration {file_name}: content is not valid UTF-8 (invalid byte at offset {}): {}",
        err.utf8_error().valid_up_to(),
        err.utf8_error()
    ))
})?;
let checksum = Some(calculate_checksum(file_name, content.as_bytes())?);
```

Keep `calculate_checksum(script, bytes)`'s signature untouched (plan 010's golden tests pin it); its internal UTF-8 check becomes redundant-but-harmless on this path. Do NOT use `from_utf8_lossy` — a lossy conversion would execute different bytes than were checksummed.

`apply_migration`'s SQL arm then uses the stored `content` instead of `std::fs::read_to_string(path)`, keeping `path` for error messages. RustFile arm unchanged (content never executed from disk).

Memory note: this holds all SQL file contents for the run's duration — acceptable for migration sets (MBs); do NOT extend to mmap/streaming.

**Verify (after each of Steps 1–4 and at the end)**: `cargo clippy -p schemalane-core --locked --all-targets -- -D warnings` → exit 0; then full parity run:
`cargo test -p schemalane-core --locked --test postgres_integration -- --include-ignored` → all pass **without any assertion edits** (ranks, skipped counts, and states must come out identical).

## Test plan

No new tests: the invariant is behavior-identical speedup, enforced by the untouched integration suite (rank sequences asserted via `history_count` checks and status summaries) plus unit suite. If plan 019's state-model unit tests exist by execution time, run them too.

## Done criteria

- [ ] `grep -n "fn is_applied_success\|fn next_installed_rank\|fn discover_sql_migrations\|fn discover_rust_migrations" schemalane-core/src/lib.rs` → no matches (all four folded away)
- [ ] `grep -c "read_dir" schemalane-core/src/lib.rs` → exactly 2 (one in discovery, one in `init_migration_project`)
- [ ] `grep -n "read_to_string" schemalane-core/src/lib.rs` → not present in `apply_migration`
- [ ] Integration suite green with zero assertion changes; fmt/clippy/unit green
- [ ] Only `schemalane-core/src/lib.rs` modified; `plans/README.md` updated

## STOP conditions

- Any integration test needs its ASSERTIONS changed to pass — that is a behavior change, not an optimization; revert the offending step and report.
- Rank collisions surface (unique-violation on `installed_rank`) in tests — the in-memory counter missed a write path; report the path.
- Conflict with plan 020 (history-row atomicity) if it landed first — it moves the success insert into the migration's transaction; the rank counter still applies, but call-site shapes differ. Reconcile by reading 020's diff; if unclear, STOP.

## Maintenance notes

- The rank counter's safety rests on the advisory lock being the only write gate — if out-of-band history writers ever become supported, restore a SELECT-MAX (or use a sequence).
- Step 4's stored `content` becomes the natural input for `plans/020-history-row-atomicity.md` and any future `check`/`--dry-run` command (spikes 039/040) — no re-read needed.
- Deleting pub-invisible helpers is semver-safe (all four are private).
