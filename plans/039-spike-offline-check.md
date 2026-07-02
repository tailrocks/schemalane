# Plan 039 (spike): Design an offline `check` command — DB-free validation + lint seam

> **Executor instructions**: DESIGN SPIKE. Deliverable is `plans/designs/check.md`
> + thin prototype. Update `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat dd0d79d..HEAD -- schemalane-core/src/lib.rs schemalane-cli/src/lib.rs`

## Status

- **Priority**: P3 (direction)
- **Effort**: M
- **Risk**: LOW (additive, read-only, no DB)
- **Depends on**: plans/029-cli-structure-dedup.md; plans/005 (semantic dup detection — check should reuse it)
- **Category**: direction
- **Planned at**: commit `dd0d79d`, 2026-07-03

## Why this matters (grounding)

Every static analysis the engine performs — real-parser SQL parsing (`parse_sql_migration`, core:1225), transaction-mode classification incl. the exit-7 mixed-statement rejection (`is_non_transactional`/`resolve_sql_transaction_mode`, core:1274-1370, 20+ unit tests), duplicate version/script detection (core:665-693), filename validation — runs today ONLY at execution time against a live database. A PR gate must either connect to Postgres or skip validation entirely. A `schemalane migrate check` (no `--database-url`) would catch unparseable SQL, duplicate versions, invalid filenames, and mixed-transaction files in CI before any deploy — pure reassembly of existing, tested pieces. The parsed AST also makes migration LINTS nearly free later (lock-heavy DDL warnings etc.), but lints are scope-creep bait — the spike must fence them.

## Design questions (with recommendations)

1. **Scope of v1 check**: exactly what discovery+parse already errors on: filename validity, duplicate version (semantic, post-plan-005), duplicate script, SQL parse failure, mixed transactional/non-transactional (exit 7 today). Recommend: v1 = these five, zero new analyses. Rust migrations: filename + duplicate checks only (no static analysis possible) — say so in output.
2. **Exit codes**: reuse 2 (validation) and 7 (mixed)? Recommend yes — `check` fails with the SAME code `up` would have failed with (that's its promise).
3. **Executor-registration check**: `ensure_rust_executors_registered` needs a built migrator — CLI-standalone mode has no executors. Recommend: skip in CLI mode; in delegated/embedded mode (migration crate) `check` CAN verify registration — a genuinely valuable extra; design the delegation forwarding.
4. **Lint framework**: OUT of v1. Design only the seam: `check --warn` reserved; lints get IDs (`SL001 create-index-non-concurrently-on-large-table` style — but no implementations now). Record 3 candidate lints with their AST evidence source (non-CONCURRENTLY index creation = `IndexStmt.concurrent == false`; `VACUUM`/`REINDEX` in migrations; `ALTER TABLE … SET NOT NULL` full-table-scan patterns) purely as appendix.
5. **Output**: per-file OK/error table (human), `--format json` list. Reuse plan 023 conventions.

## Prototype scope

`check` subcommand: discovery + parse + tx-mode + dup detection over `--migration-dir`/default, no pool construction. Unit tests: clean dir → exit 0; each of the five failure classes → correct exit. ≤ ~120 LOC given everything exists.

## Deliverables

- [ ] `plans/designs/check.md`: Q1–Q5 decisions, help text, JSON shape, lint-seam appendix
- [ ] Prototype + tests (or documented abandonment)
- [ ] Build-plan sketch

## STOP conditions

- Reusing the internal analyses requires exposing new public API from core (e.g. `discover_migrations` visibility) — list exactly what must become `pub` and get sign-off first (API surface is being deliberately tightened by plan 026).

## Maintenance notes

Keep `check` (offline, no DB) and `validate` (spike 037 — online, against history) crisply distinct in naming and docs; together they are the CI story. The lint appendix is a parking lot — nothing ships from it without its own plan.
