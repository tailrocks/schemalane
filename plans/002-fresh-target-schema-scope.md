# Plan 002: Restrict `fresh` to the target schema (it currently wipes every user schema in the database)

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat dd0d79d..HEAD -- schemalane-core/src/lib.rs schemalane-cli/src/lib.rs schemalane-core/tests/postgres_integration.rs`
> If any in-scope file changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding; on a
> mismatch, treat it as a STOP condition.

## Status

- **Priority**: P1
- **Effort**: M
- **Risk**: MED
- **Depends on**: plans/001-ci-verification-baseline.md (integration tests must be runnable to verify this safely)
- **Category**: bug
- **Planned at**: commit `dd0d79d`, 2026-07-03

## Why this matters

`SCHEMALANE_SPEC.md` §9 defines `fresh` as: "Drop all user tables in **target schema** (including history table) … `fresh` never drops the PostgreSQL database itself." The implementation instead enumerates **every non-system schema in the entire database** and runs `DROP SCHEMA … CASCADE` on each. Running `schemalane migrate fresh --schema app_a` destroys `app_b`, `public`, and any other application's schema sharing the database. This is unbounded data loss far beyond the documented contract — the single most dangerous defect in the codebase.

## Current state

- `schemalane-core/src/lib.rs` — the whole engine in one file. Relevant parts:

  `fresh_with_observer` (lines 549–554) drops all user schemas before migrating:

  ```rust
  self.with_advisory_lock(pool, async {
      let client = pool.get().await?;
      let schemas = Self::list_user_schemas(&client).await?;
      Self::drop_schemas(&client, &schemas).await?;
      self.ensure_target_schema(&client).await?;
      self.ensure_history_table(&client).await?;
  ```

  `list_user_schemas` (lines 1055–1072) — selects ALL user namespaces, no filter on `self.config.schema`:

  ```rust
  pub async fn list_user_schemas(client: &Client) -> Result<Vec<String>, SchemalaneError> {
      let rows = client
          .query(
              "SELECT nspname FROM pg_catalog.pg_namespace \
               WHERE nspname NOT LIKE 'pg_%' \
                 AND nspname != 'information_schema' \
               ORDER BY CASE WHEN nspname = 'public' THEN 1 ELSE 0 END, nspname",
              &[],
          )
          .await?;
  ```

  `drop_schemas` (lines 1075–1084):

  ```rust
  pub async fn drop_schemas(client: &Client, schemas: &[String]) -> Result<(), SchemalaneError> {
      for schema in schemas {
          let sql = format!("DROP SCHEMA IF EXISTS {} CASCADE", quote_ident(schema));
          client.batch_execute(&sql).await?;
      }
      client
          .batch_execute("CREATE SCHEMA IF NOT EXISTS public")
          .await?;
      Ok(())
  }
  ```

  Note it also unconditionally recreates `public` even when the target schema is something else.

- `schemalane-cli/src/lib.rs` — `run_fresh_command` (lines 929–947) prints a warning and previews the doomed schemas by calling the same helper:

  ```rust
  println!(
      "{}",
      "DANGEROUS: This will delete ALL schemas in the database using CASCADE and re-apply migrations."
          .bright_red()
          .bold()
  );
  ...
  let client = pool.get().await.map_err(SchemalaneError::Pool)?;
  let schemas = SchemalaneMigrator::list_user_schemas(&client).await?;
  ```

- `schemalane-core/tests/postgres_integration.rs` — `fresh_recreates_schema` (line 88) exercises `fresh` only against the default `public` schema; nothing asserts other schemas survive.

- Identifier quoting convention: `quote_ident` at `schemalane-core/src/lib.rs:1559-1561` doubles embedded quotes; all DDL in this file uses it. Match it.

- `SchemalaneConfig` (lines 23–30) carries `schema: String` — the target schema.

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Format check | `cargo fmt --all -- --check` | exit 0 |
| Lint | `cargo clippy --workspace --locked --all-targets --all-features -- -D warnings` | exit 0 |
| Unit tests | `cargo test --workspace --locked` | pass |
| Integration tests (Docker) | `cargo test -p schemalane-core --locked --test postgres_integration -- --include-ignored` | all pass (8 after this plan) |

## Scope

**In scope** (the only files you should modify):
- `schemalane-core/src/lib.rs`
- `schemalane-cli/src/lib.rs` (warning text + schema preview only)
- `schemalane-core/tests/postgres_integration.rs` (new test)

**Out of scope** (do NOT touch, even though they look related):
- `SCHEMALANE_SPEC.md` — §9 already states the correct behavior; the code moves to it, not vice versa. (Doc drift elsewhere is `plans/009-docs-command-surface-truth.md`.)
- The `--confirm` flag semantics (`Fresh { confirm }`) — documented/behavioral drift handled in plan 009.
- The advisory-lock structure `with_advisory_lock` — `plans/021-engine-connection-model.md`.
- `history_table_exists` quoting — `plans/006-history-table-identifier-quoting.md`.

## Git workflow

- Branch: `advisor/002-fresh-target-schema-scope`
- Commit style: short imperative (repo examples: `Add migration-dir support and crate delegation`). Suggested: `Restrict fresh to the configured target schema`
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: Replace whole-database drop with target-schema drop in core

In `schemalane-core/src/lib.rs`:

1. Add a private method on `SchemalaneMigrator`:

   ```rust
   /// Drop the configured target schema (CASCADE) and recreate it empty.
   /// `fresh` is scoped to this single schema per SCHEMALANE_SPEC.md §9 —
   /// it must never touch other schemas in the database.
   async fn reset_target_schema(&self, client: &Client) -> Result<(), SchemalaneError> {
       let sql = format!(
           "DROP SCHEMA IF EXISTS {} CASCADE",
           quote_ident(&self.config.schema)
       );
       client.batch_execute(&sql).await?;
       self.ensure_target_schema(client).await
   }
   ```

2. In `fresh_with_observer`, replace the two lines

   ```rust
   let schemas = Self::list_user_schemas(&client).await?;
   Self::drop_schemas(&client, &schemas).await?;
   self.ensure_target_schema(&client).await?;
   ```

   with

   ```rust
   self.reset_target_schema(&client).await?;
   ```

   (`ensure_history_table` on the next line stays.)

3. Keep `list_user_schemas` — the CLI still uses it? No: after Step 2 it has no callers. Delete **both** `list_user_schemas` and `drop_schemas` (they are `pub`, but the workspace has no other callers — verified — and shipping a public "drop every schema" helper is itself a hazard). If you find another caller, STOP condition.

**Verify**: `cargo clippy -p schemalane-core --locked --all-targets -- -D warnings` → exit 0 (dead-code warnings would surface any missed caller).

### Step 2: Fix the CLI warning and preview

In `schemalane-cli/src/lib.rs`, `run_fresh_command`:

1. Change the warning text to name the actual blast radius:

   ```rust
   "DANGEROUS: This will drop the target schema (CASCADE), destroying every object in it, then re-apply migrations."
   ```

2. Replace the `list_user_schemas` preview block (the `let client = pool.get()…` through the schema-listing loop) with a single line naming the one schema that will be dropped:

   ```rust
   println!("{}", "Schema to drop:".bright_white().bold());
   println!(" - {}", migrator.config().schema.bright_yellow());
   println!();
   ```

   Note: `SchemalaneMigrator::config()` exists at `schemalane-core/src/lib.rs:358-360`. The now-unused `client`/`pool.get()` preview acquisition should be removed with it.

**Verify**: `cargo clippy -p schemalane-cli --locked --all-targets -- -D warnings` → exit 0.

### Step 3: Add the co-tenant-survival integration test

In `schemalane-core/tests/postgres_integration.rs`, model on `fresh_recreates_schema` (line 88):

```rust
#[test]
#[ignore = "requires Docker daemon"]
fn fresh_drops_only_target_schema() -> Result<(), Box<dyn Error + 'static>> {
    // setup identical to fresh_recreates_schema, then:
    // 1. create an unrelated schema + table:
    //    client.batch_execute("CREATE SCHEMA other_app; CREATE TABLE other_app.keep_me (id int);")
    // 2. run migrator.fresh(&pool, true) with default config (schema = public)
    // 3. assert other_app.keep_me still exists:
    //    to_regclass('other_app.keep_me') IS NOT NULL  → true
    // 4. assert public was reset: history has exactly the applied count.
}
```

Use the existing helpers `connection_string`, `create_pool`, `write_migration`, `scalar_i64`, `table_exists` in that file.

**Verify**: `cargo test -p schemalane-core --locked --test postgres_integration -- --include-ignored` → all pass, including `fresh_drops_only_target_schema` (8 tests total).

### Step 4: Full gate

**Verify**: fmt + clippy + `cargo test --workspace --locked` all exit 0.

## Test plan

- New: `fresh_drops_only_target_schema` (Step 3) — the regression test for this exact bug.
- Existing `fresh_recreates_schema` must still pass unchanged — it covers the default `public` flow.
- Pattern: model after `fresh_recreates_schema` in the same file.

## Done criteria

Machine-checkable. ALL must hold:

- [ ] `grep -n "list_user_schemas\|drop_schemas" schemalane-core/src/lib.rs schemalane-cli/src/lib.rs` → no matches
- [ ] `grep -n "reset_target_schema" schemalane-core/src/lib.rs` → definition + one call in `fresh_with_observer`
- [ ] Integration suite passes with the new test (8 passed)
- [ ] `cargo clippy --workspace --locked --all-targets --all-features -- -D warnings` → exit 0
- [ ] Only the three in-scope files modified (`git status`)
- [ ] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- Any caller of `list_user_schemas`/`drop_schemas` exists beyond the two sites shown in "Current state" — that's an API consumer this plan didn't account for.
- The integration test reveals `DROP SCHEMA public CASCADE` + recreate breaks a subsequent `up` (e.g. extension objects the sample migrations require lived in `public`) — report; do not start dropping objects individually, that is a different design.
- Excerpts don't match live code (drift).

## Maintenance notes

- **Grant caveat (document in review):** `DROP SCHEMA public CASCADE; CREATE SCHEMA public` recreates `public` owned by the migration role, without the default `GRANT … ON SCHEMA public TO PUBLIC` that `initdb` provides (and PostgreSQL ≥15 changed those defaults anyway). Other roles may lose CREATE/USAGE on `public` after a `fresh`. This matches the "dev/test reset" purpose of `fresh`; if object-level dropping (Flyway `clean`-style: drop tables/views/sequences/types *within* the schema, keep the schema object and its ACLs) is ever wanted, that is a deliberate follow-up — deferred here because it is a much larger surface (object-type enumeration, dependency ordering).
- Removing the two `pub` helpers is a breaking change for external embedders of `schemalane-core` 0.1.x — acceptable pre-1.0 and *desirable* (removes a database-wide destruction primitive from the public API); release notes should call it out. `plans/026-published-api-hygiene.md` continues this API tightening.
- Reviewer should scrutinize: warning text matches new behavior; no residual `CREATE SCHEMA IF NOT EXISTS public` special-casing left behind.
