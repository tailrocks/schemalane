# Plan 017: Add CLAUDE.md (agent/contributor guide) with build prerequisites, test tiers, and conventions

> **Executor instructions**: Follow this plan step by step, verifying each step.
> On any STOP condition, stop and report. When done, update `plans/README.md`.
>
> **Drift check (run first)**: `ls CLAUDE.md AGENTS.md CONTRIBUTING.md 2>&1`
> All three must be absent ("No such file"); if one exists, STOP — reconcile
> instead of overwriting.

## Status

- **Priority**: P2
- **Effort**: S
- **Risk**: LOW
- **Depends on**: none (content references plans 001/016 outputs — write for the CURRENT repo state and note the deltas)
- **Category**: dx
- **Planned at**: commit `dd0d79d`, 2026-07-03

## Why this matters

The repo has no CLAUDE.md/AGENTS.md/CONTRIBUTING.md. Every contributor — and every coding agent executing the other plans in this directory — must rediscover from CI config: the exact lint gate (`clippy … -D warnings` with pedantic on), the two-tier test split (`#[ignore]`d Docker integration tests), and the non-obvious build prerequisite that `pg_query` compiles the bundled libpg_query **C** library (needs a C toolchain + libclang for bindgen — a fresh clone on a minimal machine fails to build with a confusing error). Capturing this once is the highest-leverage DX change per line written.

## Current state

Facts to encode (all verified at `dd0d79d`):

- Workspace: `schemalane-core` (engine; `src/lib.rs` + `src/filename.rs`), `schemalane-cli` (binary `schemalane`), `schemalane-macros` (proc-macro `embed_migrations!`), `pg_query_fmt` (SQL formatter). `default-members` excludes `schemalane-macros` (root `Cargo.toml:8-12`).
- Toolchain: edition 2024 (Rust ≥ 1.85), resolver 3. `pg_query 6.1.1` builds C code (needs `cc` + libclang).
- Gates (from `.github/workflows/ci.yml`):
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --locked --all-targets --all-features -- -D warnings`
  - `cargo test --workspace --locked --all-targets --all-features`
- Lints: workspace `Cargo.toml:15-36` — `unsafe_code = "forbid"`; clippy correctness/suspicious deny; complexity/style/perf/pedantic warn (CI promotes to deny); `unimplemented`/`dbg_macro` deny; `print_stdout`/`print_stderr`/`todo` warn. `clippy.toml`: `too-many-lines-threshold = 150` (per **function**).
- Tests: unit tests inline `#[cfg(test)]`; 7 integration tests in `schemalane-core/tests/postgres_integration.rs`, all `#[ignore = "requires Docker daemon"]` (testcontainers, one disposable Postgres per test); macros use trybuild (`schemalane-macros/tests/trybuild.rs`).
- Commit style from `git log`: short imperative subject, optional area prefix (`ci:`, `chore:`).
- Release: manual `workflow_dispatch` publishing 4 crates (see `.github/workflows/release.yml`); versions independently managed.
- Spec: `SCHEMALANE_SPEC.md` is the behavioral contract (exit codes, filename rules, Flyway compatibility promises).

## Commands you will need

| Purpose | Command | Expected |
|---|---|---|
| Verify documented commands work | run each command you write into CLAUDE.md | matches documented outcome |

## Scope

**In scope**: create `CLAUDE.md` (single file, repo root).
**Out of scope**: README changes (plan 009), pre-commit hooks / `.editorconfig` (see Maintenance notes — optional additions if trivial), rustdoc (plan 036).

## Git workflow

- Branch: `advisor/017-claude-md-contributor-docs`
- Suggested commit: `docs: add CLAUDE.md contributor and agent guide`
- No push/PR without operator instruction.

## Steps

### Step 1: Write `CLAUDE.md`

Structure (fill from "Current state"; keep it under ~120 lines; every command must be one you actually ran):

```markdown
# CLAUDE.md — working on schemalane

## What this is
PostgreSQL-only, forward-only migration toolkit, Flyway-compatible history
table. Behavior contract: SCHEMALANE_SPEC.md (exit codes §8, filename rules §3,
checksum §6.3).

## Crate map
- schemalane-core — engine: discovery, validation, checksum, history table,
  advisory lock, up/status/fresh. One big lib.rs + filename.rs.
- schemalane-cli — `schemalane` binary: clap CLI, output rendering, delegation
  to migration crates via `cargo run`.
- schemalane-macros — `embed_migrations!` proc-macro (compile-time dir scan).
- pg_query_fmt — SQL formatter/highlighter over the pg_query AST (used by the
  CLI for display; published standalone).

## Build prerequisites
- Rust ≥ 1.85 (edition 2024).
- C toolchain + libclang: the `pg_query` dependency compiles the bundled
  libpg_query C parser via bindgen. macOS: `xcode-select --install`;
  Debian/Ubuntu: `apt install build-essential libclang-dev`.
- First build is slow (C compile); later builds hit the cache.

## Commands
| task | command |
|---|---|
| fast tests (no Docker) | `cargo test --workspace` |
| full tests (Docker required) | `cargo test -p schemalane-core --test postgres_integration -- --include-ignored` |
| lint (CI gate — must be clean) | `cargo clippy --workspace --locked --all-targets --all-features -- -D warnings` |
| format | `cargo fmt --all` (check: `-- --check`) |
| run the CLI | `cargo run -p schemalane-cli -- migrate --help` |

## Conventions
- Lints are strict: pedantic is on and CI runs `-D warnings`. `unsafe` is
  forbidden. No `dbg!`, no `unimplemented!`; `println!` only in the CLI crate
  (payload to stdout, chrome to stderr).
- Identifiers into SQL text go through `quote_ident`/`qualified_table`
  (schemalane-core); values are always bound parameters.
- Integration tests: one disposable testcontainers Postgres per test, own
  TempDir, `#[ignore = "requires Docker daemon"]` — keep new DB tests in that
  shape.
- Commits: short imperative subject, optional area prefix (`ci:`, `docs:`).

## Releasing
Manual: GitHub Actions → Release workflow (publishes macros, pg_query_fmt,
core, cli in dependency order). Bump versions in each crate's Cargo.toml
first; publish fails on already-published versions.

## Plans directory
`plans/` contains audit-generated implementation plans (see plans/README.md
for order/status). Executors: honor each plan's STOP conditions.
```

Adjust the "full tests" row if plan 001 landed (README already documents it — keep both consistent) and the lint row if plan 016 changed flags.

**Verify**: every command in the table runs with the documented result (`--include-ignored` row requires Docker — if unavailable, note next to it in CLAUDE.md: "requires Docker").

### Step 2: Gate

`cargo fmt --all -- --check` (untouched code, should pass trivially) and `git status` → only `CLAUDE.md` added.

## Test plan

Docs-only: Step 1's command-execution verification is the test.

## Done criteria

- [ ] `CLAUDE.md` exists, ≤ ~150 lines, every command verified
- [ ] Mentions: libclang/C prereq, Docker test tier, exact clippy gate, crate map, spec pointer
- [ ] Only `CLAUDE.md` added; `plans/README.md` updated

## STOP conditions

- A documented command fails when you run it (e.g. clippy gate broken at current HEAD) — the repo state contradicts the doc; report instead of documenting a broken gate as working.
- `CLAUDE.md` already exists (drift check) — merge content, don't clobber.

## Maintenance notes

- Keep CLAUDE.md in sync when: CI commands change, a new crate is added, or release tooling changes (plan 015 follow-ups).
- Optional cheap additions if the maintainer wants them later: `.editorconfig` (UTF-8/LF/4-space) and a pre-commit hook running fmt+clippy — deliberately left out to keep this plan single-file.
