# Plan 025: One Flyway version parser (today: three copies, one already divergent)

> **Executor instructions**: Follow this plan step by step, verifying each step.
> On any STOP condition, stop and report. When done, update `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat dd0d79d..HEAD -- schemalane-core/src/filename.rs schemalane-macros/src/lib.rs schemalane-cli/src/lib.rs`
> Locate excerpts by symbol.

## Status

- **Priority**: P2
- **Effort**: M
- **Risk**: MED
- **Depends on**: plans/023-cli-contract-tests.md (its parse tests guard this refactor); coordinates with 005 (dedup key uses `ParsedVersion`)
- **Category**: tech-debt
- **Planned at**: commit `dd0d79d`, 2026-07-03

## Why this matters

Flyway version semantics — the compatibility promise of the whole tool — are implemented **three times**:

1. `schemalane-core/src/filename.rs` — `ParsedVersion` with arbitrary-precision compare (zero-strip + length-then-lex on digit strings).
2. `schemalane-macros/src/lib.rs:150-226` — a **verbatim copy** of `ParsedVersion`/`Ord`/`normalize_version_part`/`compare_normalized_number`, plus `parse_rust_migration_filename` re-implementing the filename split.
3. `schemalane-cli/src/lib.rs:1301-1310` — `parse_version(&str) -> Option<Vec<u64>>` used for drift-diagnostic sorting and "Database version" display — **already divergent**: `u64` parsing fails on versions the core parser accepts (spec §3.1: "numeric parts may be arbitrarily large"; core test uses a 38-digit version), silently dropping such entries to fallback ordering.

Any future rule change (Flyway edge, new separator) must be applied in three places or ordering silently disagrees between SQL discovery, macro-embedded Rust discovery, and CLI display.

Constraint that shaped the duplication: `schemalane-macros` is a proc-macro crate and `schemalane-core` depends on it — the macro cannot depend on core. A tiny shared **source module** included via `#[path]` (no new published crate, zero new dependencies for the proc-macro) breaks the triplication without inverting the dependency.

## Current state

- Core parser: `schemalane-core/src/filename.rs:17-116` (`ParsedVersion::parse`, `Ord` via `compare_normalized_number` = length-then-lex on zero-stripped parts, `parse_versioned_filename` handling `V` prefix / `__` split / case-insensitive suffix). 12 tests in-file; equality semantics pinned by `compares_versions_like_flyway` (235–243).
- Macro copy: `schemalane-macros/src/lib.rs:150-226` (struct + Ord + helpers) and `parse_rust_migration_filename` (174–213) — byte-slice suffix strip (`file_name.len() - 3`), same normalize/pop logic. 4 tests (285–319).
- CLI third impl: `script_version_key` (1163–1177) strips `V…__` by hand, `parse_version` (1301–1310) `u64`-parses; consumers: `sort_scripts_by_version` (1163), `latest_database_version` (1267–1299).
- The macro crate has NO path back to core (dependency direction core → macros). Proc-macro crates can include shared source: `#[path = "../../shared/version.rs"] mod version;` — but paths outside the crate break `cargo publish` (files outside the package root aren't packaged). **Publishable solution**: a new tiny crate `schemalane-version` that BOTH core and macros depend on (proc-macros may depend on normal crates — build-time only for the macro).

## Commands you will need

| Purpose | Command | Expected |
|---|---|---|
| Gate | fmt + clippy gate + `cargo test --workspace --locked` | exit 0 |
| Publish sanity | `cargo publish --dry-run --locked --allow-dirty -p schemalane-version` | exit 0 |

## Scope

**In scope**:
- NEW crate `schemalane-version/` (workspace member + default-member; published — core/macros must be publishable and can't depend on an unpublished crate)
- `schemalane-core/src/filename.rs` (re-export shim), `schemalane-macros/src/lib.rs`, `schemalane-cli/src/lib.rs`
- root `Cargo.toml` (member + `[workspace.dependencies]` entry if plan 016 landed), release workflow publish order (`.github/workflows/release.yml` — `schemalane-version` publishes FIRST)

**Out of scope**: behavior changes to parsing (byte-for-byte semantics move); `pg_query_fmt`; CLI display-format changes beyond using the correct comparator.

## Git workflow

- Branch: `advisor/025-version-parser-dedup`
- Suggested commit: `Extract shared schemalane-version crate; retire duplicate parsers`
- No push/PR without operator instruction.

## Steps

### Step 1: Create `schemalane-version`

`Cargo.toml`: name `schemalane-version`, version `0.1.0`, edition/license/repository from workspace (or literal copies pre-plan-016), `description = "Flyway-compatible version and migration filename parsing for Schemalane"`, ZERO dependencies, `[lints] workspace = true`.

`src/lib.rs`: move — verbatim — from `schemalane-core/src/filename.rs`: `ParsedVersion` (make it `pub` with `pub fn parse`, keep inner `Vec<String>` private; add `pub fn as_parts(&self) -> &[String]` only if a consumer needs it — check first), `normalize_version`, `normalize_version_part`, `compare_normalized_number`, `parse_versioned_filename` (public as `pub fn parse_versioned_filename(file_name, kind, suffix) -> Result<(String, ParsedVersion, String), VersionError>`), thin wrappers `parse_sql_filename`/`parse_rust_filename`, and `strip_suffix_ignore_ascii_case`.

Error type: core's functions return `SchemalaneError` — the shared crate must not know it. Define:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionError(pub String);
impl std::fmt::Display for VersionError { /* passthrough */ }
impl std::error::Error for VersionError {}
```

Move ALL of filename.rs's 12 tests + the macro's 4 filename tests into this crate (dedup overlapping cases).

**Verify**: `cargo test -p schemalane-version --locked` → all moved tests pass.

### Step 2: Point core at it

- `schemalane-core/Cargo.toml`: add `schemalane-version = { version = "0.1.0", path = "../schemalane-version" }`.
- Replace `schemalane-core/src/filename.rs` contents with a shim converting errors:

  ```rust
  pub(crate) use schemalane_version::ParsedVersion;

  pub(crate) fn parse_sql_filename(file_name: &str)
      -> Result<(String, ParsedVersion, String), crate::SchemalaneError> {
      schemalane_version::parse_sql_filename(file_name)
          .map_err(|e| crate::SchemalaneError::Validation(e.to_string()))
  }
  // parse_rust_filename analogous
  ```

  (Error TEXT must stay identical — the shared crate's messages carry the same wording as today; plan 023/005 tests assert substrings like "invalid SQL migration filename".)

**Verify**: `cargo test -p schemalane-core --locked` → green (incl. plan-005 duplicate-version tests, which now exercise the shared `ParsedVersion` through the shim).

### Step 3: Point the macro at it

- `schemalane-macros/Cargo.toml`: add the same dependency (normal `[dependencies]` — proc-macro crates can depend on plain crates).
- Delete the copied block (lines 150–226) and `parse_rust_migration_filename`'s body → call `schemalane_version::parse_rust_filename(file_name).map(|(_, v, _)| v).map_err(|e| e.to_string())`. Keep the macro's public error-message behavior (compile_error text) — adjust expectations in `tests/ui/*.stderr` if trybuild output changes wording (it shouldn't; messages match).

**Verify**: `cargo test -p schemalane-macros --locked` → green (incl. trybuild; regenerate `.stderr` ONLY if the diff is the crate-name prefix, and say so in the commit).

### Step 4: Retire the CLI's third parser

In `schemalane-cli/src/lib.rs`: add the dependency; replace `parse_version`/`script_version_key` internals with `schemalane_version::ParsedVersion`:

```rust
fn script_version_key(script: &str) -> Option<ParsedVersion> {
    let version_part = script.strip_prefix('V')?.split("__").next()?;
    ParsedVersion::parse(version_part).ok()
}
```

`sort_scripts_by_version` sorts by `Option<ParsedVersion>` (None last) then name; `latest_database_version` keys on `ParsedVersion` instead of `Vec<u64>` — this FIXES the 38-digit-version display divergence as a side effect (note it in the commit body). Delete `parse_version`.

**Verify**: `cargo test -p schemalane-cli --locked` → green (`latest_database_version_ignores_pending_entries` at 1379 still passes).

### Step 5: Wire workspace + release order

- Root `Cargo.toml`: add member `"schemalane-version"` (+ default-members, + `[workspace.dependencies]` per plan 016 if landed).
- `.github/workflows/release.yml`: `schemalane-version` publishes BEFORE `schemalane-macros` (it is now everyone's dependency). Also add it to CI's publish dry-run step.

**Verify**: `cargo publish --dry-run --locked --allow-dirty -p schemalane-version` → exit 0; full workspace gate green.

## Test plan

Moved tests (Step 1) + the downstream suites (Steps 2–4) are the net; no semantics change means zero assertion edits anywhere (except mechanical trybuild `.stderr` regeneration if wording shifts — call it out).

## Done criteria

- [ ] `grep -rn "struct ParsedVersion" --include="*.rs" .` → exactly one definition (in schemalane-version)
- [ ] `grep -n "fn parse_version" schemalane-cli/src/lib.rs` → gone
- [ ] All four downstream crates' tests green; publish dry-run green; release order updated
- [ ] `plans/README.md` updated

## STOP conditions

- Any existing test needs a **semantic** assertion change — the move altered parsing; diff the two implementations and report.
- trybuild `.stderr` diffs show more than message-prefix changes — macro error surface changed; report.
- Publishing constraints reject the path+version dep pattern — check `version` fields match; report cargo's exact error.

## Maintenance notes

- New crate = new publish step forever; release notes must mention it. Keep it dependency-free — it sits below a proc-macro.
- Future Flyway-rule changes now have exactly one home; the moved test suite is the compatibility contract.
- CLI display ordering for huge versions silently improves (divergence fix) — mention in changelog.
