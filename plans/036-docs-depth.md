# Plan 036: Docs depth — rewrite spec §10/§4.2 against the real API, rustdoc the published surface, give pg_query_fmt its own README

> **Executor instructions**: Follow this plan step by step, verifying each step.
> On any STOP condition, stop and report. When done, update `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat dd0d79d..HEAD -- SCHEMALANE_SPEC.md schemalane-core/src/lib.rs pg_query_fmt/Cargo.toml`
> API references below must match the LIVE code (plans 014/020/021/026 changed
> signatures) — write against what compiles, not against dd0d79d.

## Status

- **Priority**: P3
- **Effort**: M
- **Risk**: LOW
- **Depends on**: best after plans 026/027 (API stabilized) and 009 (command-surface truth)
- **Category**: docs
- **Planned at**: commit `dd0d79d`, 2026-07-03

## Why this matters

Three documentation debts beyond plan 009's command fixes:

1. **Spec §10 + §4.2 describe an abandoned SeaORM design**: §10 promises `Migrator::up(&DatabaseConnection, &Config)`; §4.2's code sample uses `manager.get_connection()` / `txn.execute_unprepared()` (SeaORM APIs). The real surface is `SchemalaneMigrator::new(config)` + `up/status/fresh(&deadpool_postgres::Pool)`. The strongest intent doc actively misleads integrators.
2. **docs.rs is bare**: `schemalane-core` has no crate-level `//!` docs and its public types/functions carry no `///` (the doc comments that exist sit on private helpers); `schemalane-macros`' `embed_migrations!` was undocumented pre-plan-012. Published 0.1.x crates render as bare signatures.
3. **pg_query_fmt's crates.io page is the wrong README**: `readme = "../README.md"` — a SQL-formatter crate fronted by the migration tool's unrelated README.

## Current state

- `SCHEMALANE_SPEC.md` §4.2 (lines ~139–144): SeaORM snippet; §10 (lines ~272–281): `init_migration_project` matches reality, the three `Migrator::*` signatures do not.
- Real API (verify live): `SchemalaneConfig` (builder per plan 026), `SchemalaneMigrator::{new, register_rust_migration, up, up_with_observer, status, fresh, fresh_with_observer}`, `MigrationObserver` + events, `RustMigrationExecutor::{new, transactional, with_mode}`, `init_migration_project`, `should_fail_on_pending`, error enum + `exit_code`, `embed_migrations!`.
- `schemalane-core/src/lib.rs:1` starts with `use` — no `//!`. Workspace allows `missing_errors_doc`/`missing_panics_doc` (root Cargo.toml:27-28) — fine, keep; this plan documents the WHAT, not every Err.
- `pg_query_fmt/Cargo.toml:10`: `readme = "../README.md"`; the crate HAS good `#![doc]` headers (`lib.rs:1-8`).
- docs.rs links already declared in all manifests.

## Commands you will need

| Purpose | Command | Expected |
|---|---|---|
| Doc build | `cargo doc --workspace --no-deps` | exit 0, no warnings |
| Doctest | `cargo test --workspace --locked --doc` | pass |
| Gate | fmt + clippy gate + `cargo test --workspace --locked` | exit 0 |

## Scope

**In scope**: `SCHEMALANE_SPEC.md` (§4.2, §10 only), `schemalane-core/src/lib.rs` (docs only — zero code), `schemalane-cli/src/lib.rs` (crate doc + `EmbeddedRunner` docs), `pg_query_fmt/README.md` (new) + `pg_query_fmt/Cargo.toml` (readme key), `schemalane-version` docs if plan 025 landed.
**Out of scope**: README.md (plan 009 owns it); doc-comment lint policy changes; API changes of any kind.

## Git workflow

- Branch: `advisor/036-docs-depth`
- Suggested commit: `docs: real programmatic API in spec, rustdoc public surface, pg_query_fmt README`
- No push/PR without operator instruction.

## Steps

### Step 1: Rewrite spec §4.2 sample and §10 against the live API

§4.2 replacement sketch (adjust to compiled truth):

```rust
let statements = /* parsed via pg_query */;
let txn = client.transaction().await?;   // tokio-postgres
for stmt in &statements { txn.batch_execute(&stmt.sql).await?; }
txn.commit().await?;
```

§10 rewrite: name `SchemalaneMigrator`, config construction, `&Pool` handle, observer variants, and the embedded macro path; state the transactional-history guarantee (plan 020) and at-least-once caveat verbatim from the rustdoc written there. Delete "(Draft)" from the spec title ONLY if the maintainer confirmed; otherwise leave and add "§4.2/§10 updated to match implementation as of <date>".

### Step 2: Rustdoc the core public surface

- Crate-level `//!`: what schemalane is, the four usage modes, a compiling quick-start doctest:

  ```rust
  //! ```no_run
  //! # async fn demo(pool: deadpool_postgres::Pool) -> Result<(), schemalane_core::SchemalaneError> {
  //! let config = schemalane_core::SchemalaneConfig::default();
  //! let migrator = schemalane_core::SchemalaneMigrator::new(config);
  //! let report = migrator.up(&pool).await?;
  //! # Ok(()) }
  //! ```
  ```

  (`no_run` — compiles in doctest, never connects.)
- `///` on every public item listed in "Current state" — one honest paragraph each; `up`/`fresh` carry the plan-020 semantics note; `SchemalaneConfig` fields get one line each; `MigrationObserver` documents when each event fires (incl. `on_run_planned` if plan 022 landed).
- CLI crate: `//!` describing binary + embedded runner; `///` on `EmbeddedRunner`/`run_cli*`/`Verbosity`.

**Verify**: `cargo doc --workspace --no-deps` → zero warnings; `cargo test --doc` → doctest passes.

### Step 3: pg_query_fmt README

New `pg_query_fmt/README.md` (~40 lines): what it is (display-oriented PostgreSQL SQL formatter on the real parser), `format_statement`/`format_sql` example with output, the highlight/preview modules, the display-only caveat (from plan 035's contract), license line. Point `readme = "README.md"` in its Cargo.toml.

**Verify**: `cargo package --locked --allow-dirty --list -p pg_query_fmt | grep README.md` → packaged.

### Step 4: Full gate

fmt/clippy/tests + doc build + doctests → green.

## Test plan

Doc build warnings-free + passing doctest are the machine checks; the §10 rewrite is checked by compiling its code sample as a doctest where feasible (put the §10 example in core's crate docs and reference it from the spec — single source).

## Done criteria

- [ ] `grep -n "execute_unprepared\|DatabaseConnection" SCHEMALANE_SPEC.md` → no matches
- [ ] `schemalane-core/src/lib.rs` begins with `//!`; public items documented (spot-check: `cargo doc` output has no bare items on the index page)
- [ ] `pg_query_fmt/README.md` exists and is the crate's readme
- [ ] Doc build + doctests green; `plans/README.md` updated

## STOP conditions

- The live API contradicts what plans 020/021/026 said it would be — document what EXISTS; if the spec then can't tell a coherent §10 story, report the mismatch.
- Doctest can't compile without network/DB — keep `no_run`; if even compilation needs unavailable types, mark ```ignore``` and note why.

## Maintenance notes

- Rule: public-API PRs must update rustdoc in the same PR (docs.rs is the storefront).
- The spec now defers API detail to rustdoc — keep it that way; spec owns BEHAVIOR (exit codes, states, checksums), rustdoc owns SIGNATURES.
