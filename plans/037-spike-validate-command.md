# Plan 037 (spike): Design a `validate` command — read-only CI drift gate

> **Executor instructions**: This is a DESIGN SPIKE, not a build plan. Deliverable
> is a written design (`plans/designs/validate.md`) + a thin prototype behind no
> flag commitments. On any STOP condition, stop and report. Update `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat dd0d79d..HEAD -- schemalane-core/src/lib.rs schemalane-cli/src/lib.rs`
> Design against live code.

## Status

- **Priority**: P3 (direction)
- **Effort**: S-M
- **Risk**: LOW (additive)
- **Depends on**: plans/029-cli-structure-dedup.md (one enum to extend); plans/009 (documented exit codes)
- **Category**: direction
- **Planned at**: commit `dd0d79d`, 2026-07-03

## Why this matters (grounding)

Everything needed to FAIL on drift exists; nothing exposes it read-only. `status` computes `Missing`/`ChecksumMismatch`/`Failed` counts (`StatusSummary`, core lib.rs:121-128, populated at 1182-1191) and the CLI renders full drift diagnostics (cli:1179-1265) — but `status` always exits 0 unless `--fail-on-pending` (the only gate, `should_fail_on_pending`, core:1904-1910, exit 5). The defined-but-unenforced states are spec §7.1's formal "Drift"; exits 3/4 fire only from side-effecting `up`/`fresh`. CI users today must attempt a real migration to detect drift. Flyway ships `validate`; spec §1.2's out-of-scope list does NOT exclude it.

## Design questions the spike must answer (with recommendations)

1. **Command vs flags**: new `migrate validate` subcommand vs `status --fail-on-drift --fail-on-failed` flags. Recommend: **subcommand** (Flyway-familiar; composes with delegation forwarding; `status` stays purely informational) — but check: delegation forwarding cost is one more arm (plan 029 made it cheap).
2. **Exit mapping**: reuse 3 (drift) / 4 (failed) / 5 (pending, opt-in via `--fail-on-pending`)? Recommend yes — same meanings as `up`'s preflight, already spec'd.
3. **Does pending fail validate?** Flyway default: pending does NOT fail. Recommend: match Flyway (pending → exit 0 unless `--fail-on-pending`).
4. **Output**: human = existing drift diagnostics renderer; `--format json` = the `StatusReport` (already serializable) + a `validation` verdict block. Freeze keys per plan 023's convention.
5. **Engine seam**: `Migrator::validate(&Pool) -> Result<StatusReport, SchemalaneError>` that internally runs `status` + applies `ensure_no_blocking_history`-equivalent classification WITHOUT taking the advisory lock (read-only) — decide whether lock-free reads are acceptable (they are for a gate; document the race).

## Prototype scope (throwaway-quality allowed, tests required if kept)

`validate` wired through: core method + CLI arm + delegation forwarding + 2 integration tests (clean → 0; checksum-mismatch → 3). ≤ ~150 LOC.

## Deliverables

- [ ] `plans/designs/validate.md`: decisions for Q1–Q5 with rationale, CLI help text, JSON shape, spec §-addition draft
- [ ] Prototype branch compiling + 2 integration tests green (or explicitly abandoned with reasons)
- [ ] Follow-up build plan sketch (steps only) if the maintainer green-lights

## STOP conditions

- Spec owner rejects the command name/shape — record and stop.
- JSON verdict shape conflicts with existing consumers' assumptions (plan 023 key freeze) — design an additive envelope instead.

## Maintenance notes

Overlaps: spike 039 (`check`) is the OFFLINE sibling (no DB); keep their verdicts/vocabulary aligned. `--fail-on-pending` remains on `status` for back-compat regardless.
