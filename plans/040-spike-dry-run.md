# Plan 040 (spike): Design `up --dry-run` — show the exact ordered SQL a run would execute

> **Executor instructions**: DESIGN SPIKE. Deliverable is `plans/designs/dry-run.md`
> + thin prototype. Update `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat dd0d79d..HEAD -- schemalane-core/src/lib.rs schemalane-cli/src/lib.rs`

## Status

- **Priority**: P3 (direction)
- **Effort**: S-M
- **Risk**: LOW (read-only mode)
- **Depends on**: plans/022-cli-double-work.md (`on_run_planned` seam), plans/035 (formatter honesty — dry-run output should not lie)
- **Category**: direction
- **Planned at**: commit `dd0d79d`, 2026-07-03

## Why this matters (grounding)

Change-approval workflows want the exact ordered, formatted SQL before it runs. Every piece exists and is only reachable MID-APPLY: pending detection (`MigrationState::Pending`, rendered at cli:1133-1152), per-statement formatting + highlighting (Detailed observer, cli:243-247, via `pg_query_fmt::format_statement`), transaction-mode classification (core:1274-1370). There is no `--dry-run`; `run_up_command` always applies. Reassembly, not new machinery.

## Design questions (with recommendations)

1. **Surface**: `up --dry-run` vs new `plan` command. Recommend **`up --dry-run`** (semantics = "the up you would get", including gating: drift/failed-history must fail the dry-run exactly like a real `up`'s preflight — that behavioral fidelity is the feature).
2. **DB or no DB**: pending-ness requires history ⇒ needs a connection (read-only queries only). Degradation without `--database-url`: refuse (recommend) — a "pretend everything is pending" mode belongs to `check` (spike 039), not here.
3. **Lock**: take the advisory lock during the read? Recommend NO lock (read-only; racing a concurrent real run only yields a stale plan — document) — but DO run `ensure_no_blocking_history` for gate fidelity.
4. **Output**: per pending migration — header (script, version, tx-mode `[transaction]`/`[no transaction]`), then formatted statements (Detailed-style); Rust migrations listed as `RUST (source not previewable)` with script name. `--format json`: ordered array {script, version, type, transaction_mode, statements[]}. Respect plan 004's stdout/stderr + color rules (payload on stdout).
5. **Engine seam**: reuse `on_run_planned` + a `DryRun` mode on the runner? Recommend instead a dedicated core method `plan_up(&Pool) -> Result<UpPlan, SchemalaneError>` returning structured data (statements already parsed) — observers stay execution-only; the CLI renders `UpPlan`. Check overlap with `StatusReport` — `UpPlan` adds parsed statements + tx-mode, which status lacks.

## Prototype scope

`plan_up` in core (discovery + history read + gating + per-pending `parse_sql_migration` + tx-mode; NO execution) + CLI `--dry-run` flag rendering it + 2 integration tests (pending set renders; drift blocks with exit 3). ≤ ~200 LOC.

## Deliverables

- [ ] `plans/designs/dry-run.md`: Q1–Q5 decisions, output samples (human + JSON), help text
- [ ] Prototype + tests (or documented abandonment)
- [ ] Build-plan sketch

## STOP conditions

- `up --dry-run` semantics can't be made gate-faithful without the lock (evidence of a real race in tests) — reconsider Q3 with data, don't guess.
- JSON statement payloads leak formatter fidelity issues (pre-plan-035) — note dependency; ship raw SQL strings in JSON (never formatted) regardless.

## Maintenance notes

JSON carries RAW statements (`stmt.sql`) — formatted text is for humans only; that separation keeps plan-035's display-only contract intact. `UpPlan` is also the natural input for a future `--dry-run` on `fresh` (out of scope).
