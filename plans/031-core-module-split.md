# Plan 031: Split schemalane-core's 2264-line lib.rs into modules (public API preserved via re-exports)

> **Executor instructions**: Follow this plan step by step, verifying each step.
> On any STOP condition, stop and report. When done, update `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat dd0d79d..HEAD -- schemalane-core/src/`
> This plan runs LAST among core refactors — every earlier core plan should be
> DONE or explicitly REJECTED in plans/README.md before starting.

## Status

- **Priority**: P3
- **Effort**: L
- **Risk**: LOW (mechanical moves + `pub use` façade)
- **Depends on**: plans 018/019/020/021/026/027/028/030 (all core-shape plans) — sequencing, not correctness
- **Category**: tech-debt
- **Planned at**: commit `dd0d79d`, 2026-07-03

## Why this matters

The entire engine lives in one 2264-line file spanning ≥10 responsibilities. Every change lands in the same file (merge-conflict magnet), reviewers can't hold a subsystem in view, and `pub` items are reachable from everywhere with no layering. `pg_query_fmt` in the same workspace already demonstrates the convention (split into `expr/stmt/preview/highlight`). This is the janitorial capstone after the behavioral plans stabilize the shapes.

## Current state

Responsibility map of `schemalane-core/src/lib.rs` at `dd0d79d` (post-earlier-plans, shift by symbol):

| lines (≈) | responsibility | target module |
|---|---|---|
| 21–42 | `SchemalaneConfig` + default | `config.rs` |
| 44–95 | `SchemalaneError` + exit codes | `error.rs` |
| 97–167 | report/domain types (`StatusEntry`, `RunReport`, …) | `report.rs` |
| 169–246 | observer events + `MigrationObserver` + noop | `observer.rs` |
| 248–292, 1605–1782 | init scaffold + templates | `init.rs` |
| 294–343 | `RustMigrationExecutor` + modes | `rust_migration.rs` |
| 345–1085 | `SchemalaneMigrator` (lock/session, discovery, gating, apply, fresh) | `migrator.rs` (+ `discovery.rs` for 665–781) |
| 935–1084 / post-030 repository | history SQL | `history.rs` |
| 1087–1199 | `build_status_report` | `report.rs` (builder beside types) |
| 1201–1370 | SQL parse + tx-mode detection | `sql_analysis.rs` |
| 1372–1543 | statement/rust execution | `execute.rs` |
| 1559–1577 | `quote_ident`/`qualified_table`/`millis_i32` | `ident.rs` (quote) — `millis_i32` goes to `execute.rs` |
| 1579–1603 | checksum | `checksum.rs` |
| 1784–1859 | `DiscoveredMigration`/`HistoryRow`/`MigrationType` | `discovery.rs` / `history.rs` |
| 1916–2264 | tests | move WITH their subjects (each module gets its `#[cfg(test)]`) |
| existing | `filename.rs` (or the plan-025 shim) | unchanged |

Public API façade: `lib.rs` keeps `pub use` for every currently-public item (enumerate via `cargo doc` before/after — Step 3). `pub use schemalane_macros::embed_migrations;` (line 19) stays in lib.rs.

## Commands you will need

| Purpose | Command | Expected |
|---|---|---|
| API freeze | `cargo doc -p schemalane-core --no-deps` before/after + `diff` of `target/doc/schemalane_core/index.html` item list (or `cargo public-api` if installed) | identical public items |
| Gate | fmt + clippy gate + `cargo test --workspace --locked` | exit 0 |
| Integration | `cargo test -p schemalane-core --locked --test postgres_integration -- --include-ignored` | pass |

## Scope

**In scope**: `schemalane-core/src/**` (new module files + shrunken lib.rs).
**Out of scope**: ANY behavior/signature change (pure motion); visibility tightening beyond `pub(crate)` where an item was never re-exported (verify against the doc-item freeze); other crates (imports of `schemalane_core::X` must keep working via re-exports).

## Git workflow

- Branch: `advisor/031-core-module-split`
- One commit per module extraction (reviewable motion), suggested subjects: `Extract core error module`, etc.
- No push/PR without operator instruction.

## Steps

### Step 1: Freeze the public API list

`cargo doc -p schemalane-core --no-deps` → save the item list (or `grep -n "^pub" schemalane-core/src/lib.rs > /tmp/pub-before.txt`).

### Step 2: Extract modules bottom-up

Order (least-coupled first): `error.rs` → `config.rs` → `checksum.rs` → `ident.rs` → `report.rs` → `observer.rs` → `rust_migration.rs` → `init.rs` → `sql_analysis.rs` → `execute.rs` → `discovery.rs` → `history.rs` → `migrator.rs`. After each: `mod x;` + `pub use x::{…};` in lib.rs, move the module's tests along, run `cargo clippy -p schemalane-core --all-targets -- -D warnings` + `cargo test -p schemalane-core --locked`.

**Verify per extraction**: clippy + unit tests green.

### Step 3: API-freeze check + full gate

Re-run Step 1's capture; diff → **no public item added/removed/moved-out-of-path** (all still reachable at `schemalane_core::Name`). Full workspace + integration suites green. `cargo package --locked --allow-dirty -p schemalane-core` → exit 0.

## Test plan

Existing suites, moved with their subjects — zero assertion edits. The API-freeze diff is the extra net.

## Done criteria

- [ ] `wc -l schemalane-core/src/lib.rs` ≤ ~150 (module decls + re-exports + crate docs)
- [ ] API-freeze diff empty; all suites green; package check green
- [ ] `plans/README.md` updated

## STOP conditions

- Any `pub use` can't reproduce an item's old path (e.g. a type was `pub` at root and something imports `schemalane_core::lib::…`-style oddities) — report the exact import.
- A move forces a signature/visibility change to compile — that's coupling the map missed; report rather than redesign inline.
- Earlier core plans still TODO in the index — sequencing violation; stop.

## Maintenance notes

- New-code rule after this: new engine features get their own module or extend the matching one — lib.rs stays a façade.
- Follow-the-same-recipe candidate: `schemalane-cli` (plan 032).
- If plan 036 adds crate-level `//!` docs, they live in the new slim lib.rs.
