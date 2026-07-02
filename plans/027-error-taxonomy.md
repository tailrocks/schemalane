# Plan 027: Split the `Validation(String)` junk drawer into a real error taxonomy; align error styles across crates

> **Executor instructions**: Follow this plan step by step, verifying each step.
> On any STOP condition, stop and report. When done, update `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat dd0d79d..HEAD -- schemalane-core/src/lib.rs schemalane-cli/src/lib.rs pg_query_fmt/src/lib.rs`
> Locate by symbol; many earlier plans touch these files.

## Status

- **Priority**: P3
- **Effort**: M
- **Risk**: LOW-MED (breaking enum change — land with/after plan 026 so `non_exhaustive` absorbs future additions)
- **Depends on**: plans/026-published-api-hygiene.md, plans/023-cli-contract-tests.md (exit-code tests guard the mapping)
- **Category**: tech-debt
- **Planned at**: commit `dd0d79d`, 2026-07-03

## Why this matters

`SchemalaneError::Validation(String)` is used for ~15 unrelated failures: filename parse errors, missing directories, duplicate versions, missing Rust executors, SQL split/parse failures — and in the CLI even **infrastructure** errors (URL parse, pool build, JSON encode). Callers cannot programmatically distinguish "your migration set is malformed" (spec exit 2) from "the connection string is bad" (a runtime problem the spec assigns exit 1) — today both exit 2. Meanwhile the workspace has three error styles: thiserror enum (core), hand-rolled `Display` impl (`pg_query_fmt::FormatError`), and `Result<_, String>` (macros). One taxonomy + one style lowers the cost of every future error-handling change.

## Current state

- Core enum: `schemalane-core/src/lib.rs:44-95` (thiserror; `exit_code` maps `Validation → 2`).
- `Validation(String)` producers in core (grep `SchemalaneError::Validation`): non-UTF8 filename (720/761), missing dir (697), duplicate version/script (674/680), missing executor (801/889), SQL split/parse (1227/1239), non-UTF-8 content (1590), init-target problems (251/259/1614).
- CLI misuses (infrastructure as Validation): URL parse (723–725), pool build (735–737), JSON encode (870–872), cargo spawn (704 — plan 003 already re-maps to `Io`).
- `pg_query_fmt/src/lib.rs:24-41`: manual `FormatError` Display; `schemalane-macros`: `Result<_, String>` internally (fine for compile_error rendering — leave).
- Exit-code contract: spec §8 — 2 must mean *migration validation*; 1 runtime.

## Commands you will need

| Purpose | Command | Expected |
|---|---|---|
| Gate | fmt + clippy gate + `cargo test --workspace --locked` | exit 0 |
| Exit-table guard | `cargo test -p schemalane-core --locked exit_codes` | pass (updated) |

## Scope

**In scope**: `schemalane-core/src/lib.rs`, `schemalane-cli/src/lib.rs`, `pg_query_fmt/src/lib.rs` (thiserror adoption only).
**Out of scope**: macros' `Result<String>` (renders to `compile_error!`; adequate); message TEXT changes beyond the minimum (tests assert substrings — keep wording); further variant granularity than listed (YAGNI).

## Git workflow

- Branch: `advisor/027-error-taxonomy`
- Suggested commit: `Split Validation into config/discovery variants; unify error styles`
- No push/PR without operator instruction.

## Steps

### Step 1: New core variants

Add (keep `Validation` for genuine migration-set validation):

```rust
#[error("Configuration error: {0}")]
Config(String),          // bad URL, pool build, unusable init target
#[error("Internal error: {0}")]
Internal(String),        // JSON encode and similar "should not happen"
```

`exit_code`: `Config(_) => 1`, `Internal(_) => 1` (explicit arms — the `_ => 1` fallthrough still catches Db/Pool/Io). Update plan-023's `exit_codes_match_spec_section_8` test with the two new arms.

### Step 2: Re-home the misfiled producers

- CLI: URL parse + pool build → `Config`; JSON encode → `Internal`.
- Core `init_migration_project` target errors (251/259) and `write_init_file` refusal (1614): these ARE user-input problems but not *migration-set* validation; spec §8's exit 2 covers "validation error" broadly — **keep them `Validation`** (no behavioral change for init) and note the choice in the commit body.
- Everything discovery/parse-related stays `Validation`.

**Behavioral change to announce**: bad `--database-url` now exits **1** (was 2). Spec §8 calls runtime/config errors 1 — this is a spec-conformance fix; release notes must state it.

### Step 3: thiserror for `pg_query_fmt`

Replace the manual `Display`/`Error` impls:

```rust
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FormatError {
    #[error("parse error: {0}")]
    Parse(String),
    #[error("deparse error: {0}")]
    Deparse(String),
}
```

Add `thiserror` to `pg_query_fmt/Cargo.toml` (workspace dep if plan 016 landed). Message strings identical → no test churn.

### Step 4: Full gate

fmt + clippy + workspace tests; run CLI manually with a garbage URL:
`cargo run -p schemalane-cli -- migrate --database-url not-a-url status; echo exit=$?` → `exit=1`.

## Test plan

- Updated exit-code table test (Step 1).
- Manual exit check (Step 4).
- Everything else: existing suites unchanged (message wording preserved).

## Done criteria

- [ ] `grep -n "Config(\|Internal(" schemalane-core/src/lib.rs` → variants + exit arms
- [ ] CLI URL/pool/JSON sites use the new variants; garbage-URL exit is 1
- [ ] `pg_query_fmt` uses thiserror; gates green
- [ ] `plans/README.md` updated

## STOP conditions

- Any test asserts `Validation` for a site this plan re-homes (search for the message substrings first) — update the test only if the plan explicitly re-homed that site; otherwise report.
- Downstream (non-workspace) breakage concerns: this is 0.x + `non_exhaustive` (plan 026) — if 026 has NOT landed, adding variants is still breaking; prefer landing 026 first (dependency).

## Maintenance notes

- Rule going forward: `Validation` = the migration set/spec, `Config` = operator input/environment, `Internal` = bugs. Reviewers should police new `Validation(String)` uses.
- Deferred: structured variants with fields (script names, paths) instead of pre-formatted strings — do it when a consumer actually needs the fields.
- `#![allow(clippy::future_not_send)]` (CLI line 1) was flagged in the audit as over-broad; narrowing it is a 10-minute follow-up if clippy stays quiet — optional, not in scope.
