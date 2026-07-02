# Plan 006: Quote identifiers in the `to_regclass` existence probe (mixed-case schema/table makes `status` lie)

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat dd0d79d..HEAD -- schemalane-core/src/lib.rs schemalane-core/tests/postgres_integration.rs`
> If any in-scope file changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding; on a
> mismatch, treat it as a STOP condition.

## Status

- **Priority**: P1
- **Effort**: S
- **Risk**: LOW
- **Depends on**: plans/001-ci-verification-baseline.md (for the integration test to run in CI)
- **Category**: bug
- **Planned at**: commit `dd0d79d`, 2026-07-03

## Why this matters

Every DDL/DML path in the engine quotes identifiers via `quote_ident` — except the history-table existence probe, which feeds a **raw** `schema.table` string to `to_regclass`. `to_regclass` parses its argument with SQL identifier rules, so unquoted mixed-case names are case-folded: with `--schema MyApp` (or any mixed-case/special history-table name), `up` correctly creates and populates `"MyApp"."flyway_schema_history"`, but `status` probes `to_regclass('MyApp.flyway_schema_history')` → looks up `myapp.…` → NULL. `status` then treats history as empty and reports **every applied migration as Pending**, hides Failed/Missing/ChecksumMismatch, and makes `--fail-on-pending` fail spuriously. A history-table name containing a dot mis-parses outright.

## Current state

- `schemalane-core/src/lib.rs`, `history_table_exists` (lines 962–970) — the bug:

  ```rust
  async fn history_table_exists(&self, client: &Client) -> Result<bool, SchemalaneError> {
      let regclass = format!("{}.{}", self.config.schema, self.config.history_table);
      let row = client
          .query_one("SELECT to_regclass($1) IS NOT NULL AS exists", &[&regclass])
          .await?;

      let exists: bool = row.get("exists");
      Ok(exists)
  }
  ```

- The correct convention, same file:

  ```rust
  fn quote_ident(name: &str) -> String {              // lines 1559-1561
      format!("\"{}\"", name.replace('"', "\"\""))
  }

  fn qualified_table(schema: &str, table: &str) -> String {  // lines 1563-1565
      format!("{}.{}", quote_ident(schema), quote_ident(table))
  }
  ```

  `ensure_history_table` (935), `load_history` (972), `next_installed_rank` (1010), `insert_history_row` (1020) all use `qualified_table`.

- Only caller of `history_table_exists`: `status` (lines 514–518) — decides whether to `load_history` or use an empty Vec.

- Integration-test conventions: `schemalane-core/tests/postgres_integration.rs` — per-test testcontainer + `TempDir`; helper `table_exists` (line 469) uses the same `to_regclass($1)` pattern with a hardcoded lowercase `public.{table}` (fine for its fixtures; do not change it).

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Format check | `cargo fmt --all -- --check` | exit 0 |
| Lint | `cargo clippy --workspace --locked --all-targets --all-features -- -D warnings` | exit 0 |
| Unit tests | `cargo test --workspace --locked` | pass |
| Integration (Docker) | `cargo test -p schemalane-core --locked --test postgres_integration -- --include-ignored` | all pass |

## Scope

**In scope** (the only files you should modify):
- `schemalane-core/src/lib.rs` (one function)
- `schemalane-core/tests/postgres_integration.rs` (one new test)

**Out of scope** (do NOT touch, even though they look related):
- The broader "history repository seam" consolidation — `plans/030-history-repository-seam.md`.
- CLI flags/validation of schema names — none needed; quoting makes all names safe.

## Git workflow

- Branch: `advisor/006-history-table-identifier-quoting`
- Suggested commit: `Quote identifiers in history-table existence probe`
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: Use `qualified_table` in the probe

Replace the first line of `history_table_exists`'s body:

```rust
let regclass = qualified_table(&self.config.schema, &self.config.history_table);
```

`to_regclass` accepts quoted qualified names (`"MyApp"."flyway_schema_history"`) — quoting preserves case and neutralizes dots/specials inside either identifier.

**Verify**: `cargo clippy -p schemalane-core --locked --all-targets -- -D warnings` → exit 0.

### Step 2: Integration regression test

Add to `schemalane-core/tests/postgres_integration.rs`, modeled on `up_and_status_with_sql_migrations` (line 15):

```rust
#[test]
#[ignore = "requires Docker daemon"]
fn status_sees_history_in_mixed_case_schema() -> Result<(), Box<dyn Error + 'static>> {
    // setup: one SQL migration V1__create_cake.sql (reuse write_migration)
    // config: SchemalaneConfig { schema: "MyApp".to_owned(), migrations_dir, ..Default::default() }
    // 1. migrator.up(&pool) → applied.len() == 1
    // 2. migrator.status(&pool) → summary.success == 1 && summary.pending == 0
    //    (before this fix: success == 0, pending == 1)
}
```

**Verify**: `cargo test -p schemalane-core --locked --test postgres_integration -- --include-ignored` → all pass including the new test.

### Step 3: Full gate

**Verify**: fmt + clippy + `cargo test --workspace --locked` exit 0.

## Test plan

- New: `status_sees_history_in_mixed_case_schema` — asserts the exact regression (success=1/pending=0 under `--schema MyApp`).
- Existing integration tests unchanged and green (default lowercase path).

## Done criteria

- [ ] `grep -n "to_regclass" schemalane-core/src/lib.rs` → the probe builds its argument via `qualified_table`
- [ ] Integration suite green incl. new test
- [ ] fmt/clippy/workspace tests exit 0
- [ ] Only the two in-scope files modified
- [ ] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- The new test fails at Step 2 **after** the Step 1 change — that means another unquoted path exists (check `set_search_path`/`ensure_target_schema` behavior with the quoted schema); report what you find rather than patching further sites ad hoc.
- `history_table_exists` has been moved/refactored (plans 030/031 landed) — apply the same one-line principle at its new home if trivially recognizable; otherwise stop.

## Maintenance notes

- Rule for reviewers going forward: any identifier reaching SQL text goes through `quote_ident`/`qualified_table`; anything reaching a **parameter** that PostgreSQL parses as an identifier (`to_regclass`, `::regclass` casts) needs the same treatment.
- `plans/030-history-repository-seam.md` centralizes all history SQL — this fix should fold into that seam untouched.
