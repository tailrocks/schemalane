# Plan 038 (spike): Design a `repair` command — non-destructive history remediation

> **Executor instructions**: DESIGN SPIKE. Deliverable is `plans/designs/repair.md`
> + optionally a prototype. This command mutates the history table — the design
> must be reviewed by the maintainer BEFORE any build plan. Update `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat dd0d79d..HEAD -- schemalane-core/src/lib.rs`

## Status

- **Priority**: P3 (direction — highest-value spike)
- **Effort**: M
- **Risk**: MED (history mutation semantics)
- **Depends on**: plans/030-history-repository-seam.md (the UPDATE/DELETE lands there); plans/019 (failure-state tests)
- **Category**: direction
- **Planned at**: commit `dd0d79d`, 2026-07-03

## Why this matters (grounding)

A single failed migration **permanently strands a database**: any `success=false` latest row blocks every future `up` with `FailedHistory`/exit 4 (`ensure_no_blocking_history`, core lib.rs:808-859), and a checksum mismatch blocks with exit 3. The ONLY in-tool escape is `fresh` — which destroys the schema (and pre-plan-002, the whole DB's user schemas). The remediation targets are already fully enumerated by the drift diagnostics (`only_in_database`, `checksum_mismatch`, `failed_scripts` — cli:1201-1263); history has INSERT-only plumbing (core:1020-1051), no UPDATE/DELETE. Flyway's `repair` (remove failed rows; realign checksums; mark missing) is the standard recovery path and is NOT in spec §1.2's rejected list. Operators of a "Flyway-compatible" tool will expect it the first time a migration fails in a shared environment.

## Design questions (with recommendations)

1. **Scope of v1**: Flyway repair does three things: (a) delete failed rows, (b) update checksums of applied migrations to match local files, (c) mark missing migrations as deleted. Recommend v1 = (a) + (b) only; (c) requires a `deleted` concept the history schema lacks (Flyway uses `success` + special types) — investigate Flyway's exact representation before promising it.
2. **Checksum rewrite is a footgun** (masks real drift): require explicit sub-flags — `repair --remove-failed --align-checksums` with NO default-everything mode? Recommend: bare `repair` = (a)+(b) like Flyway (familiarity), but print a per-row plan and require `--confirm yes` (reuse `fresh`'s guard pattern + interactive prompt).
3. **Locking**: must run under the advisory lock (same key as `up` — plan 014's derivation).
4. **`installed_rank` handling**: deleting failed rows leaves rank gaps — Flyway leaves gaps (ranks are append-only); recommend the same (NO renumbering — renumbering breaks concurrent readers' assumptions and audit trails).
5. **Reporting**: emit what changed (rows deleted, checksums updated old→new) via a `RepairReport` (serializable, plan-023 JSON conventions) and observer events or plain report — recommend report-only, no new observer events.
6. **Exit codes**: 0 on success (including nothing-to-do); dirty-state detection failures map to existing codes. New code needed? Recommend no.

## Investigation tasks

- Read Flyway's documented `repair` semantics (docs + a container run against a fixture history) and record EXACTLY what it does to each column — compatibility target.
- Enumerate edge cases: failed row for a script that no longer exists locally; failed row followed by successful retry (latest-wins already tolerates — repair should still purge the stale failed row? Flyway deletes ALL failed rows); checksum-align for a file that is ITSELF the failed one.
- Decide interaction with plan 020's transactional history writes (none expected — repair is its own transaction).

## Deliverables

- [ ] `plans/designs/repair.md`: Q1–Q6 decisions, Flyway-parity table (column-by-column), CLI surface (`migrate repair` flags + confirmation UX), `RepairReport` shape, test matrix (≥6 integration cases from the edge list)
- [ ] Optional prototype: `--remove-failed` path only, behind the confirm guard, with 2 integration tests
- [ ] Build-plan sketch for maintainer sign-off

## STOP conditions

- Flyway-parity investigation reveals repair semantics that require history-schema changes — that's a compatibility decision for the maintainer, not the spike.
- Maintainer rejects history mutation entirely — record; the alternative (documented manual SQL recipes) becomes a docs task.

## Maintenance notes

This spike is why plan 030's repository seam exists — implement UPDATE/DELETE there. Pairs with spike 037: `validate` detects, `repair` fixes; keep their vocabularies identical (same state names, same script lists).
