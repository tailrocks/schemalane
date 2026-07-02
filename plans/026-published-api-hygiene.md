# Plan 026: Published-API hygiene — `#[non_exhaustive]` on public types, remove dead public surface

> **Executor instructions**: Follow this plan step by step, verifying each step.
> On any STOP condition, stop and report. When done, update `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat dd0d79d..HEAD -- schemalane-core/src/lib.rs schemalane-cli/src/lib.rs`
> Locate by symbol; earlier plans add variants/fields — that's expected.

## Status

- **Priority**: P2
- **Effort**: S
- **Risk**: LOW-MED (deliberate one-time API break, pre-1.0)
- **Depends on**: land AFTER plans 002/003/014/020/021/022 (they add/remove API; sequencing avoids repeated breakage)
- **Category**: tech-debt
- **Planned at**: commit `dd0d79d`, 2026-07-03

## Why this matters

`schemalane-core` is a published crate (0.1.20) whose public types are all exhaustively matchable and literal-constructible: adding an error variant (plan 003 did), a config field, or a report field is a **breaking change** for any downstream that `match`es or struct-literals them. `#[non_exhaustive]` now — while the user base is tiny — makes routine additions non-breaking forever. Separately, the crate exports dead surface with zero callers anywhere in the workspace (verified by grep): an unused parallel status renderer, a trivial path wrapper, builder variants nothing uses, and a getter nothing calls — each is semver liability + untested code rot.

## Current state

(`schemalane-core/src/lib.rs`)

- Public types lacking `#[non_exhaustive]`: `SchemalaneError` (44–81, grows variants), `SchemalaneConfig` (23–30, all-pub fields), `MigrationState` (97–105), `StatusEntry` (107–119), `StatusSummary` (121–128), `StatusReport` (130–136), `AppliedMigration` (138–146), `RunReport` (148–152), `InitReport` (154–159), observer event structs `MigrationStarted`/`Finished`/`Failed`, `SqlStatementStarted`/`Finished`/`Failed` (169–227).
- Dead public API (zero references outside their own definitions — verified `grep -rn` across the workspace):
  - `format_status_table(&StatusReport) -> String` (1861–1902) — the CLI has its own comfy-table renderer; this plain-text sibling is unused and asserts nothing anywhere.
  - `migrations_dir_exists(&Path) -> bool` (1912–1914) — `path.exists()` wrapper.
  - `SchemalaneMigrator::config()` (358–360) — note: plan 002 Step 2 USES it in the CLI; re-grep at execution time — if the CLI now calls it, it is no longer dead; keep it.
  - `with_rust_migration` / `with_rust_migrations` builder methods (371–389) — macro and tests use `register_rust_migration` directly.
  - `list_user_schemas`/`drop_schemas` — plan 002 already deletes them; skip here if gone.
- In-repo construction sites that `#[non_exhaustive]` on `SchemalaneConfig` would break: the CLI builds it with struct literal + `..Default::default()` (`schemalane-cli/src/lib.rs:357-363, 629-635`) — literal-with-`..Default::default()` **still works within the defining crate but NOT across crates** once non_exhaustive; the CLI is a separate crate → it breaks. Mitigation in Step 2.
- Tests construct `StatusEntry`/`StatusReport`/`StatusSummary` literals in the CLI test module (1380–1408) — same cross-crate constraint applies.

## Commands you will need

| Purpose | Command | Expected |
|---|---|---|
| Dead-API confirmation | `grep -rn "format_status_table\|migrations_dir_exists\|with_rust_migrations\|with_rust_migration\b" --include="*.rs" . \| grep -v "core/src/lib.rs"` | empty (else the symbol is live — keep it) |
| Gate | fmt + clippy gate + `cargo test --workspace --locked` | exit 0 |

## Scope

**In scope**: `schemalane-core/src/lib.rs`; `schemalane-cli/src/lib.rs` (adapting construction sites); CHANGELOG/release-note text in the commit message.
**Out of scope**: renaming/restructuring types (plan 028/031); `pg_query_fmt`'s API (fine as-is: small, documented); adding builders beyond the minimal `SchemalaneConfig` one below.

## Git workflow

- Branch: `advisor/026-published-api-hygiene`
- Suggested commit: `Mark public types non_exhaustive; drop dead public API (breaking)`
- No push/PR without operator instruction.

## Steps

### Step 1: Delete dead surface

Re-run the dead-API grep per symbol (Commands table). For each still-dead symbol: delete it. Expected deletions at plan-authoring time: `format_status_table`, `migrations_dir_exists`, `with_rust_migration`, `with_rust_migrations`. Keep `config()` if plan 002 made it live.

**Verify**: `cargo clippy --workspace --locked --all-targets -- -D warnings` → exit 0 (no dangling references).

### Step 2: `#[non_exhaustive]` + construction paths

1. Add `#[non_exhaustive]` to: `SchemalaneError`, `SchemalaneConfig`, `MigrationState`, `StatusEntry`, `StatusSummary`, `StatusReport`, `AppliedMigration`, `RunReport`, `InitReport`, and the six observer event structs.
2. `SchemalaneConfig` cross-crate construction: add a builder-lite API on core:

   ```rust
   impl SchemalaneConfig {
       pub fn new() -> Self { Self::default() }              // then mutate fields
       #[must_use] pub fn with_schema(mut self, v: impl Into<String>) -> Self { self.schema = v.into(); self }
       #[must_use] pub fn with_history_table(mut self, v: impl Into<String>) -> Self { … }
       #[must_use] pub fn with_migrations_dir(mut self, v: impl Into<PathBuf>) -> Self { … }
       #[must_use] pub fn with_installed_by(mut self, v: Option<String>) -> Self { … }
       #[must_use] pub fn with_advisory_lock_id(mut self, v: Option<i64>) -> Self { … }  // type per plan 014
   }
   ```

   Fields stay `pub` (readable + mutable after construction — `non_exhaustive` only blocks cross-crate literals). Migrate the CLI's two construction sites to the builder chain.
3. Observer event structs: the CLI only reads their fields (field access keeps working under `non_exhaustive`); core's own literal constructions are same-crate → fine. The CLI **test module's** `StatusEntry`/`StatusReport`/`StatusSummary` literals (lines ~1380–1408) are cross-crate literals → they break. Fix: give `StatusEntry`, `StatusReport`, and `StatusSummary` a plain `pub fn new(…)` constructor taking every field in declaration order (they are data records; a total constructor is legitimate, stable API), and rewrite the CLI tests to use them.

**Verify**: `cargo test --workspace --locked` → green.

### Step 3: Release-note text

Commit body must carry the downstream-visible summary:

```
BREAKING (0.x): public types are now #[non_exhaustive]; construct
SchemalaneConfig via builder methods and reports via new(). Removed unused
public API: format_status_table, migrations_dir_exists,
with_rust_migration(s). After this release, adding variants/fields is
non-breaking.
```

**Verify**: text present in the commit.

### Step 4: Full gate

fmt + clippy + workspace tests (+ integration if Docker) → green. Packaging check: `cargo package --locked --allow-dirty -p schemalane-core` (the CI convention for path-dep crates; a full `publish --dry-run` would require the dep versions to exist on crates.io) → exit 0.

## Test plan

No new behavior → no new tests beyond compile-level enforcement; the whole existing suite passing under the new construction paths IS the test. CLI parse/JSON tests (plan 023) must pass unchanged — JSON serialization is unaffected by `non_exhaustive`.

## Done criteria

- [ ] `grep -c "non_exhaustive" schemalane-core/src/lib.rs` ≥ 15
- [ ] Dead symbols absent (grep per Step 1)
- [ ] Workspace + integration tests green; package check green
- [ ] Only in-scope files modified; `plans/README.md` updated

## STOP conditions

- A "dead" symbol turns out live at execution time (grep non-empty) — keep it, note which plan made it live, adjust the deletion list.
- `non_exhaustive` on the observer events breaks the CLI in a way field-access can't fix (e.g. exhaustive destructuring somewhere) — show the site; destructuring with `..` is the fix, apply it.
- JSON output changes in any way (plan 023's key-freeze test fails) — serde must be unaffected; investigate before proceeding.

## Maintenance notes

- From now on, adding fields/variants to these types is non-breaking — but REMOVING/renaming still breaks; the builder methods are the compatibility surface to keep stable.
- `pg_query_fmt::FormatError` left exhaustive deliberately (two stable variants, display-only crate) — revisit only if it grows.
- This is the right moment to start a CHANGELOG.md if the maintainer wants one (deferred — not in scope).
