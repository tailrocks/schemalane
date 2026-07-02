# Plan 005: Detect duplicate migration versions semantically (`V1` vs `V1.0` vs `V01` must collide)

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat dd0d79d..HEAD -- schemalane-core/src/lib.rs schemalane-core/src/filename.rs`
> If any in-scope file changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding; on a
> mismatch, treat it as a STOP condition.

## Status

- **Priority**: P1
- **Effort**: S
- **Risk**: LOW
- **Depends on**: none
- **Category**: bug
- **Planned at**: commit `dd0d79d`, 2026-07-03

## Why this matters

`SCHEMALANE_SPEC.md` §3.3 mandates a hard startup failure on "Duplicate versions across SQL and Rust migrations" — the same rule Flyway enforces ("Found more than one migration with version X"). The current check dedups on the **raw normalized text** of the version, but version *equality* is defined semantically: the parser strips leading zeros and pops trailing `.0` parts, so `ParsedVersion::parse("1.0") == ParsedVersion::parse("1")` (there is a unit test asserting exactly that). Consequently `V1__a.sql` and `V1.0__b.sql` (or `V01__b.sql`) sail through validation, sort as equal versions tie-broken by **filename**, and both execute — the exact ordering ambiguity the spec forbids.

## Current state

- `schemalane-core/src/lib.rs`, `discover_migrations` (lines 665–693) — the flawed dedup key is `version_text`:

  ```rust
  let mut versions = BTreeSet::new();
  let mut scripts = BTreeSet::new();

  for migration in &migrations {
      if !versions.insert(migration.version_text.clone()) {
          return Err(SchemalaneError::Validation(format!(
              "duplicate migration version '{}'",
              migration.version_text
          )));
      }
      ...
  ```

- `schemalane-core/src/filename.rs`:
  - `version_text` is only `_`→`.` normalization (`normalize_version`, lines 101–103; used at line 88), so `"1"`, `"1.0"`, `"01"` remain distinct strings.
  - `ParsedVersion::parse` (lines 21–38) strips leading zeros per part and pops trailing zero parts:

    ```rust
    while parts.len() > 1 && parts.last().is_some_and(|part| part == "0") {
        parts.pop();
    }
    ```

  - Semantic equality is already proven by the existing test `compares_versions_like_flyway` (lines 235–243): `parse("1.0") == parse("1")`, `parse("001_002") == parse("1.2")`.
  - `ParsedVersion` derives `PartialEq, Eq` and implements `Ord` (lines 17–60) — it is usable as a `BTreeSet`/`BTreeMap` key. It is `pub(crate)`.

- `DiscoveredMigration` (lib.rs lines 1799–1808) carries both `version: ParsedVersion` and `version_text: String` plus `script: String` — everything needed for a good error message.

- Error convention: `SchemalaneError::Validation(String)` for discovery failures (see the duplicate-script arm right below the version check).

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Format check | `cargo fmt --all -- --check` | exit 0 |
| Lint | `cargo clippy --workspace --locked --all-targets --all-features -- -D warnings` | exit 0 |
| Unit tests | `cargo test -p schemalane-core --locked` | pass |

## Scope

**In scope** (the only files you should modify):
- `schemalane-core/src/lib.rs` (`discover_migrations` + new unit tests)

**Out of scope** (do NOT touch, even though they look related):
- `schemalane-core/src/filename.rs` — the parser is correct; only the dedup key is wrong.
- `schemalane-macros/src/lib.rs` — the macro has its own (duplicated) parser and does no version dedup; runtime discovery is the enforcement point and catches macro-embedded Rust files too. Parser unification is `plans/025-version-parser-dedup.md`.
- `build_status_report` ordering (uses `ParsedVersion` already).

## Git workflow

- Branch: `advisor/005-duplicate-version-detection`
- Suggested commit: `Detect duplicate migration versions semantically`
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: Key the dedup on `ParsedVersion` and name both offenders

In `discover_migrations`, replace the `versions` set logic with a map from semantic version to the first script that claimed it:

```rust
let mut versions: std::collections::BTreeMap<&ParsedVersion, &str> = std::collections::BTreeMap::new();
let mut scripts = BTreeSet::new();

for migration in &migrations {
    if let Some(existing) = versions.insert(&migration.version, migration.script.as_str()) {
        return Err(SchemalaneError::Validation(format!(
            "duplicate migration version '{}': '{}' and '{}' resolve to the same version",
            migration.version_text, existing, migration.script
        )));
    }
    if !scripts.insert(migration.script.clone()) {
        return Err(SchemalaneError::Validation(format!(
            "duplicate migration script '{}'",
            migration.script
        )));
    }
}
```

Borrowing note: `migrations` is only iterated immutably here and the map borrows end before the later `migrations.sort_by(…)`. If the borrow checker objects to sorting after borrowing, scope the check in a block `{ … }` before the sort — the sort does not need the map.

`ParsedVersion` is `pub(crate)` and already imported in lib.rs via `use filename::{ParsedVersion, …}` (line 17).

**Verify**: `cargo clippy -p schemalane-core --locked --all-targets -- -D warnings` → exit 0.

### Step 2: Unit tests through the public discovery path

`discover_migrations` is private but reachable via `SchemalaneMigrator` + a temp dir. Add to the existing `#[cfg(test)] mod tests` in `schemalane-core/src/lib.rs` (uses `tempfile::TempDir` already — see `init_scaffold_creates_expected_files`, line 1968):

```rust
#[test]
fn rejects_semantically_duplicate_versions() {
    use super::{SchemalaneConfig, SchemalaneMigrator};
    let temp = tempfile::TempDir::new().expect("temp dir");
    let dir = temp.path().join("migrations");
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(dir.join("V1__a.sql"), "SELECT 1;").expect("write");
    std::fs::write(dir.join("V1.0__b.sql"), "SELECT 2;").expect("write");

    let migrator = SchemalaneMigrator::new(SchemalaneConfig {
        migrations_dir: dir,
        ..Default::default()
    });
    let err = migrator
        .discover_migrations()
        .expect_err("V1 and V1.0 must collide");
    let msg = err.to_string();
    assert!(msg.contains("duplicate migration version"), "got: {msg}");
    assert!(msg.contains("V1__a.sql") && msg.contains("V1.0__b.sql"), "got: {msg}");
}

#[test]
fn rejects_leading_zero_duplicate_versions() {
    // same shape with V01__b.sql instead of V1.0__b.sql
}

#[test]
fn accepts_distinct_versions_with_shared_prefix() {
    // V1__a.sql + V1.1__b.sql must be OK (discover returns 2 entries)
}
```

Fill in the two sketched tests following the first. `discover_migrations` is a private method — tests in the same file's `mod tests` can call it via `migrator.discover_migrations()` since the module is a child of the crate root; confirm the `use super::…` list includes `SchemalaneConfig, SchemalaneMigrator` (add if missing).

**Verify**: `cargo test -p schemalane-core --locked duplicate_versions` → new tests pass; and `cargo test -p schemalane-core --locked` → whole crate green.

### Step 3: Full gate

**Verify**: fmt + clippy + `cargo test --workspace --locked` all exit 0.

## Test plan

- `rejects_semantically_duplicate_versions` (V1 vs V1.0) — the reported bug.
- `rejects_leading_zero_duplicate_versions` (V1 vs V01).
- `accepts_distinct_versions_with_shared_prefix` (V1 vs V1.1) — no false positive.
- Cross-type case is inherently covered: discovery merges SQL+Rust before the check; optionally add `V1__a.sql` + `V1_0__b.rs` (a Rust file colliding with a SQL version) as a fourth test — recommended, spec §3.3 says "across SQL and Rust". A `.rs` duplicate needs no registered executor because discovery fails before executor validation.
- Pattern: model on `init_scaffold_creates_expected_files` for TempDir usage.

## Done criteria

Machine-checkable. ALL must hold:

- [ ] `grep -n "versions.insert" schemalane-core/src/lib.rs` shows the `ParsedVersion`-keyed map, not `version_text`
- [ ] New tests pass; full workspace tests pass
- [ ] fmt/clippy exit 0
- [ ] Only `schemalane-core/src/lib.rs` modified
- [ ] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- `ParsedVersion` no longer derives `Eq`/`Ord` or moved crates (drift from plan 025 landing first) — adapt the key type only if the semantic-equality tests in `filename.rs:235-243` still hold; otherwise stop.
- Existing integration tests start failing because real fixture files collide — that means the repo's own samples were relying on the bug; report, don't rename fixtures silently.

## Maintenance notes

- Error text now names both scripts — keep that property; it is what makes the failure actionable.
- `plans/025-version-parser-dedup.md` moves `ParsedVersion` into a shared module; this check should survive that move unchanged (it keys on the type, not on strings).
- History-side note: versions stored in `flyway_schema_history` keep their original `version_text` (e.g. `001.002`); status matching is script-keyed, so this change does not affect history reconciliation.
