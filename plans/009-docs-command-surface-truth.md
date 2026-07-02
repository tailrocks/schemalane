# Plan 009: Make README and SCHEMALANE_SPEC.md match the shipped command surface

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat dd0d79d..HEAD -- README.md SCHEMALANE_SPEC.md schemalane-cli/src/lib.rs`
> If any in-scope file changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding; on a
> mismatch, treat it as a STOP condition. (Changes to `schemalane-cli/src/lib.rs`
> from plans 003/004/007 are expected and fine — what matters is the CLI
> grammar: re-verify flags/subcommands with `cargo run -p schemalane-cli -- migrate --help`.)

## Status

- **Priority**: P1
- **Effort**: S
- **Risk**: LOW
- **Depends on**: none (coordinate wording with plans 002, 007, 008 if they landed — see Steps)
- **Category**: docs
- **Planned at**: commit `dd0d79d`, 2026-07-03

## Why this matters

Every entry-point command the docs teach is currently wrong. A new user following the README or the spec hits four distinct failures: an install path that doesn't exist, a bootstrap script that doesn't exist, a subcommand that doesn't parse, and a destructive-command flag that doesn't parse. Actively-wrong setup docs are worse than none — they burn trust in everything else. The spec additionally omits behavior the code has (exit code 7, `--verbosity`, transaction-mode auto-detection), so automation authors can't rely on it.

This plan fixes **documented-command truth** only. (Deep spec §10/§4.2 rewrite and rustdoc are `plans/036-docs-depth.md`; behavior changes are other plans.)

## Current state

Verified discrepancies, doc line → reality:

1. `README.md:27` — `cargo install --path backend-rust/schemalane/schemalane-cli --force`. No `backend-rust/` exists (monorepo remnant; standalone path is `schemalane-cli`).
2. `README.md:37` — `./docker-up-kellnr.sh`: no `.sh` file exists anywhere in the repo. `README.md:42-48` — kellnr registry setup for generated crates; after plan 008 the scaffold defaults to crates.io, making this section obsolete.
3. `README.md:16,39,65` and `SCHEMALANE_SPEC.md` §2 (line ~35), §2.2, §2.3 — `schemalane migrate init`. Reality: `init` is a **root** subcommand (`schemalane-cli/src/lib.rs:400-408`, `RootCommand::Init`); `MigrateCommand` (lines 445–463) has only `Up`/`Status`/`Fresh`. `schemalane migrate init` fails clap parsing.
4. `README.md:99` — `fresh --yes`; `SCHEMALANE_SPEC.md` §2.2 (line ~56) and §9 (line ~259) — `--yes (required)`. Reality: the flag is `--confirm <value>` accepting `yes` (`schemalane-cli/src/lib.rs:458-462`), plus an interactive prompt on a TTY, and the core error text is `` `fresh` requires --confirm yes `` (`schemalane-core/src/lib.rs:76`).
5. `SCHEMALANE_SPEC.md` §8 (lines 247–255) — exit codes 0–6 only. Reality: `MixedStatements => 7` (`schemalane-core/src/lib.rs:91`) for migrations mixing transactional and non-transactional statements.
6. `SCHEMALANE_SPEC.md` §2.1 — no `--verbosity`. Reality: `--verbosity minimal|compact|detailed` exists on `migrate` (`schemalane-cli/src/lib.rs:437-439`) and the embedded CLI.
7. `SCHEMALANE_SPEC.md` §4.2 — says only "SQL migrations are transactional by default". Reality: the engine auto-detects non-transactional statements (`CREATE INDEX CONCURRENTLY`, `VACUUM`, `REINDEX SCHEMA|DATABASE|SYSTEM`, `DISCARD ALL`, `ALTER SYSTEM`, `CREATE/DROP DATABASE|TABLESPACE|SUBSCRIPTION`) and runs an all-non-transactional file outside a transaction; a **mix** in one file is rejected with exit 7 (`schemalane-core/src/lib.rs:1274-1370`).
8. `README.md:107-109` also documents the Flyway filename rules — correct today; leave as-is.

Decision baked into this plan (doc-side alignment, not code-side): keep the CLI as shipped — root-level `init`, `--confirm yes` — and fix the docs. Rationale: `init` at root is arguably better UX than nesting, `--confirm yes` is a stronger guard than a bare `--yes`, and code changes would break existing users' scripts. If the maintainer prefers code-side alignment instead, that is a STOP condition, not an improvisation.

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| CLI grammar ground truth | `cargo run -p schemalane-cli -- --help` and `cargo run -p schemalane-cli -- migrate --help` | shows `init` at root; `migrate` has `up/status/fresh`, `--verbosity` |
| Fresh flag ground truth | `cargo run -p schemalane-cli -- migrate fresh --help` | shows `--confirm <CONFIRM>` |
| Doc link/path sanity | `grep -n "backend-rust\|kellnr\|docker-up" README.md` | no matches after Step 1 |

## Scope

**In scope** (the only files you should modify):
- `README.md`
- `SCHEMALANE_SPEC.md`

**Out of scope** (do NOT touch, even though they look related):
- Any Rust source — this is a docs-truth plan.
- Spec §10 (programmatic API) and §4.2's SeaORM code sample — `plans/036-docs-depth.md` rewrites those against the real API.
- Spec §9 fresh **semantics** text ("target schema") — correct spec, wrong code; `plans/002-fresh-target-schema-scope.md` fixes the code. If 002 has NOT landed yet, add one warning line (Step 2.4) and remove it when 002 lands.

## Git workflow

- Branch: `advisor/009-docs-command-surface-truth`
- Suggested commit: `docs: align README and spec with shipped CLI surface`
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: README fixes

1. Line 16 area (Commands list): `schemalane migrate init` → `schemalane init`. Present the four commands as: `schemalane init`, `schemalane migrate up`, `schemalane migrate status`, `schemalane migrate fresh`.
2. Line 27: `cargo install --path backend-rust/schemalane/schemalane-cli --force` → `cargo install --path schemalane-cli --force`.
3. Delete the kellnr block (lines ~36–48: the `./docker-up-kellnr.sh` step, the `[registries.kellnr]` instructions). If plan 008 landed, note instead: "The generated crate depends on the published schemalane crates from crates.io; for local development replace them with path dependencies (see the comments in the generated Cargo.toml)."
4. Line 39/65: `schemalane migrate init --path ./migration` → `schemalane init --path ./migration` (both occurrences).
5. Line 99: `fresh --yes` → `fresh --confirm yes`.
6. If plan 007 landed: convert `--database-url "$DATABASE_URL"` examples to `DATABASE_URL=… schemalane migrate …` / `DATABASE_URL=… cargo run … -- up` forms; if 007 has not landed, leave the examples but they still must use the correct subcommand spelling.

**Verify**: `grep -n "backend-rust\|kellnr\|docker-up\|migrate init\|--yes" README.md` → no matches.

### Step 2: Spec fixes

1. §2 command list + §2.2 + §2.3 headings: `schemalane migrate init` → `schemalane init` (namespace note: "The `init` command lives at the CLI root; `up`/`status`/`fresh` live under `migrate`.").
2. §2.2 `fresh` flags: replace `--yes (required)` with `--confirm yes` (required in non-interactive contexts; an interactive TTY prompts for confirmation). §9 line ~259: same replacement.
3. §2.1: add `--verbosity <minimal|compact|detailed>` (default: `minimal`) to the common flags of `up` and `fresh` (status output is unaffected by verbosity today — verify with `--help` before writing; document what the help shows).
4. §8 exit codes: add `- 7: migration mixes transactional and non-transactional statements`. If plan 002 has not landed, also add to §9: "> **Known deviation (tracked)**: the current implementation drops all user schemas, not only the target schema — see plans/002-fresh-target-schema-scope.md." Remove that line if 002 is DONE in `plans/README.md`.
5. §4.2: append a short "Transaction handling" paragraph: SQL files are parsed with the real PostgreSQL parser; statements that cannot run in a transaction block (`CREATE INDEX CONCURRENTLY`, `DROP INDEX CONCURRENTLY`, `VACUUM`, `REINDEX SCHEMA|DATABASE|SYSTEM`, `DISCARD ALL`, `ALTER SYSTEM`, `CREATE|DROP DATABASE`, `CREATE|DROP TABLESPACE`, `CREATE|DROP SUBSCRIPTION`) make the whole file run non-transactionally; mixing both kinds in one file is a validation error (exit 7); this mirrors Flyway's `mixed=false` default.

**Verify**: `grep -n "migrate init\|--yes" SCHEMALANE_SPEC.md` → no matches (except any deliberate "Known deviation" sentence); `grep -n "verbosity\|: 7" SCHEMALANE_SPEC.md` → both present.

### Step 3: Ground-truth pass

Run the three help commands from "Commands you will need" and read every command example remaining in both docs; each must parse against the shown grammar.

**Verify**: for each README shell example, the subcommand/flags appear in the corresponding `--help` output.

## Test plan

Docs-only; the greps and `--help` cross-checks above are the tests. No Rust tests.

## Done criteria

- [ ] All Step 1/2 greps clean
- [ ] Every remaining documented command parses against `--help` output
- [ ] `git status` shows only README.md and SCHEMALANE_SPEC.md modified
- [ ] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- The maintainer is known to prefer **code-side** alignment (rename `--confirm`→`--yes`, nest `init` under `migrate`) — that decision reverses this plan's direction; ask, don't guess.
- `--help` output contradicts the "Current state" grammar (CLI changed since `dd0d79d`) — re-derive the truth from `--help`, and if a plans/00X change caused it, document the new truth.

## Maintenance notes

- Add to review checklist: any PR changing clap definitions must update README + spec in the same PR — this drift happened because nothing enforces it.
- `plans/036-docs-depth.md` owns the remaining spec debt (§10 SeaORM-era API, §4.2 code sample) and rustdoc.
- If plan 002 lands after this one, delete the §9 "Known deviation" line added in Step 2.4.
