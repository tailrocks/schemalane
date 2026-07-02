# Plan 033: Split pg_query_fmt's stmt.rs and extract the duplicated table-body emitter

> **Executor instructions**: Follow this plan step by step, verifying each step.
> On any STOP condition, stop and report. When done, update `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat dd0d79d..HEAD -- pg_query_fmt/src/`
> On mismatch with excerpts, STOP.

## Status

- **Priority**: P3
- **Effort**: M
- **Risk**: LOW-MED (byte-exact output tests catch regressions)
- **Depends on**: none (independent of the engine plans)
- **Category**: tech-debt
- **Planned at**: commit `dd0d79d`, 2026-07-03

## Why this matters

`pg_query_fmt/src/stmt.rs` (1547 lines) bundles every DDL and DML formatter, and `fmt_create_foreign_table` re-implements ~90 lines of `fmt_create_table`'s column/constraint collection, single-item shortcut, and alignment-emit loop — verified near-identical (single-item branch `stmt.rs:93-111` vs `415-431`; alignment `113-149` vs `434-463`). Column-alignment bugs must be fixed twice today. The formatter's own exact-string tests in `pg_query_fmt/src/lib.rs` make this a safe, well-gated refactor.

## Current state

- `fmt_create_table` (`stmt.rs:54-150`): header build (`CREATE TABLE [IF NOT EXISTS] name`), collect `TableItem::{Column,Constraint}` list, single-item inline form `({single})`, else aligned multi-line body (max name/type/default widths).
- `fmt_create_foreign_table` (`stmt.rs:370-496`): unwraps `base_stmt`, then repeats the same collection/single/alignment logic, plus foreign-specific `SERVER`/`OPTIONS` tail.
- File inventory (dispatch at `pg_query_fmt/src/lib.rs:65-80`): DDL — create_table, create_enum, index, alter_table, view, create_function, create_foreign_table; DML — select, insert, update, delete. Shared helpers at bottom (`name_list_to_string` etc., ~1528–1547).
- Tests: exact-string assertions in `lib.rs` (38 tests) — the behavioral freeze.
- Module convention: `pub(crate) mod` files, `pub(crate) fn` formatters (see `lib.rs:14-17`).

## Commands you will need

| Purpose | Command | Expected |
|---|---|---|
| Gate | fmt + clippy gate + `cargo test --workspace --locked` | exit 0 |
| Crate tests | `cargo test -p pg_query_fmt --locked` | all pass, zero assertion edits |

## Scope

**In scope**: `pg_query_fmt/src/stmt.rs` → `pg_query_fmt/src/stmt/{mod.rs,ddl.rs,dml.rs,table_body.rs}` (or flat `ddl.rs`/`dml.rs` siblings — match `lib.rs`'s existing flat style: prefer `stmt/` directory module), `pg_query_fmt/src/lib.rs` (module decl only).
**Out of scope**: output changes of ANY kind (fidelity fixes are plan 035); `expr.rs` (fine-grained enough); public API (everything stays `pub(crate)`).

## Git workflow

- Branch: `advisor/033-formatter-split-dedup`
- Commits: `Extract shared table body emitter`, then `Split stmt.rs into ddl/dml modules`
- No push/PR without operator instruction.

## Steps

### Step 1: Extract `fmt_table_body`

New `stmt/table_body.rs` (or a section pre-split): one function producing the parenthesized body given the collected items:

```rust
pub(crate) struct ColumnParts { pub name: String, pub type_str: String,
    pub default_expr: Option<String>, pub constraints: String }
pub(crate) enum TableItem { Column(usize), Constraint(String) }

/// Single-item inline `(x)` or aligned multi-line body — the shared shape of
/// CREATE TABLE and CREATE FOREIGN TABLE. Byte-compatible with both callers.
pub(crate) fn fmt_table_body(header: &str, columns: &[ColumnParts], all_items: &[TableItem]) -> String
```

Move the single-item branch and the alignment loop in verbatim from `fmt_create_table`; diff mentally against the foreign-table copy — they must be identical modulo variable names (if a real divergence surfaces, STOP: it's an undocumented behavior difference to report, not merge silently). Both callers keep their own item-collection (they read different AST nodes) and their own tails (foreign: `SERVER`/`OPTIONS`).

**Verify**: `cargo test -p pg_query_fmt --locked` → all exact-string tests pass unchanged.

### Step 2: Split the file

`stmt/` module: `ddl.rs` (create_table, create_enum, index_stmt, alter_table, view_stmt, create_function, create_foreign_table + their private helpers), `dml.rs` (select/insert/update/delete + theirs), `table_body.rs`, `mod.rs` re-exporting the `pub(crate) fn fmt_*` set so `lib.rs`'s dispatch lines don't change. Shared bottom helpers (`name_list_to_string`, `node_string_list`) go to `mod.rs` or wherever both halves reach them.

**Verify**: clippy + crate tests green; `wc -l pg_query_fmt/src/stmt/*.rs` — no file > ~900.

### Step 3: Full gate

fmt + clippy + workspace tests → green.

## Test plan

Existing exact-string suite is the entire net (that's what it's for). No new tests here; plan 035 adds semantic round-trip coverage.

## Done criteria

- [ ] `grep -rn "max_name" pg_query_fmt/src/` → alignment computation exists ONCE (table_body)
- [ ] stmt.rs replaced by the module dir; dispatch in lib.rs unchanged
- [ ] All tests green with zero assertion edits; `plans/README.md` updated

## STOP conditions

- The two copies turn out NOT byte-equivalent (Step 1 diff) — report the divergence with both fragments; merging would silently change one statement type's output.
- Any lib.rs test needs an assertion edit — output changed; revert the offending move.

## Maintenance notes

- Plan 035's fidelity fixes should land AFTER this split (they touch `ddl.rs`/`expr.rs` cleanly).
- Future statement-type formatters: add to the right module + one dispatch arm — reviewers should reject additions to a monolith.
