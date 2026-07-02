# Plan 041 (spike): Investigate out-of-order application semantics (currently implicit and undocumented)

> **Executor instructions**: INVESTIGATION ONLY — no code changes, no design
> commitment. Deliverable is `plans/designs/out-of-order.md` recording current
> behavior with test evidence. Update `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat dd0d79d..HEAD -- schemalane-core/src/lib.rs`

## Status

- **Priority**: P3 (direction — LOW confidence, investigate before designing)
- **Effort**: S
- **Risk**: none (read-only investigation)
- **Depends on**: plans/001 (Docker tests runnable)
- **Category**: direction
- **Planned at**: commit `dd0d79d`, 2026-07-03

## Why this matters (grounding)

Flyway applies migrations strictly above the current max applied version unless `-outOfOrder=true`. Schemalane's `up` appears to apply **any** not-yet-successful local migration in version order — a lower-versioned migration arriving late (branch merge) is plain `Pending` and gets applied (`up_with_observer` loop guard is just "not applied-successfully", core lib.rs:420-424; `build_status_report` classifies it `Pending`, 1134-1144). That silently implements always-on out-of-order — more permissive than Flyway's default, undocumented, and possibly unintended. Teams migrating from Flyway may rely on strict ordering as a safety property (late-arriving DDL applied after later migrations can be wrong). No code hints at a deliberate choice; the spec is silent.

## Investigation tasks

1. **Characterize actual behavior with tests** (throwaway or keep-as-documentation):
   - Apply V1, V3. Add V2 locally. Run `up` → does V2 apply? (Expected from code reading: YES.) What `installed_rank` does it get (append — rank 3)?
   - `status` before/after: V2 shows `Pending` (before) / `Success` (after)?
   - Flyway comparison: same scenario in a Flyway container with default config → REFUSES V2 ("resolved migration not applied to database" validate error / ignored with warning?) — record exact Flyway behavior for the parity table.
2. **Blast-radius check**: does anything in schemalane depend on strict ordering (e.g. `latest_database_version` display, drift classification)? Grep + read.
3. **Options memo** (no decision): (a) keep always-out-of-order, document it loudly as a Flyway divergence; (b) match Flyway: refuse late arrivals by default + `--out-of-order` opt-in flag (breaking for anyone relying on current behavior); (c) warn-but-apply default. List migration/compat implications of each; note that (b) needs the max-applied-version computation (exists in CLI display logic only — would move to core).
4. **Maintainer question set**: 3 crisp questions the maintainer must answer before any build plan.

## Deliverables

- [ ] `plans/designs/out-of-order.md`: behavior evidence (test transcripts), Flyway parity table, options memo, question set
- [ ] Tests either deleted or committed as documentation (`#[ignore]`d + named `characterize_*`) — executor's call, stated in the doc

## STOP conditions

- Investigation reveals late-arriving migrations are actually REJECTED somewhere unexpected — the grounding premise is wrong; write that up (it changes the memo entirely) and stop there.

## Maintenance notes

Do NOT ship behavior changes from this spike. If (b) is ever chosen, it interacts with spike 037's `validate` (Flyway's validate is where "resolved but not applied" surfaces) — design them together.
