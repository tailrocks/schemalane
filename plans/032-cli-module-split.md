# Plan 032: Split schemalane-cli's 1431-line lib.rs into modules

> **Executor instructions**: Follow this plan step by step, verifying each step.
> On any STOP condition, stop and report. When done, update `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat dd0d79d..HEAD -- schemalane-cli/src/`
> Run LAST among CLI plans (003/004/007/011/013/014/022/023/029 done or rejected).

## Status

- **Priority**: P3
- **Effort**: L
- **Risk**: LOW
- **Depends on**: plans 022/029 (they reshape what gets moved); plan 023 (tests guard the motion)
- **Category**: tech-debt
- **Planned at**: commit `dd0d79d`, 2026-07-03

## Why this matters

One file mixes clap grammar, dispatch, subprocess delegation, connection/URL parsing, a progress observer, and all rendering. Testing rendering requires importing the whole CLI; grammar changes and display changes collide in review. Same recipe as plan 031, smaller stakes: public surface here is just `run_cli`, `run_cli_with`, `EmbeddedRunner`, `Verbosity` (check `grep "^pub" schemalane-cli/src/lib.rs` — the macro's generated `runner()` returns `EmbeddedRunner`, so that path must survive).

## Current state

Responsibility map (`schemalane-cli/src/lib.rs`, dd0d79d; shift by symbol after earlier plans):

| lines (≈) | responsibility | target module |
|---|---|---|
| 26–35 | help styles | `args.rs` |
| 37–48, 52–97 | `Verbosity`, format helpers | `render.rs` (Verbosity stays re-exported at root) |
| 99–118 | `prompt_yes_no` | `prompt.rs` |
| 120–127, 720–846 | pool + URL target parsing | `connect.rs` |
| 129–316 | `CliProgressObserver` | `observer.rs` |
| 318–385 | `EmbeddedRunner` + `run_cli*` | `runner.rs` (re-export at root — macro codegen calls `::schemalane_cli::EmbeddedRunner`) |
| 387–552 | clap structs/enums | `args.rs` |
| 554–718 | dispatch + delegation | `dispatch.rs` + `delegate.rs` |
| 848–1000 | db command runners | `commands.rs` |
| 1002–1317 | branding/status/drift rendering + version helpers | `render.rs` |
| 1321–1431 | tests | move with subjects |

Public-path constraint: `schemalane-macros` emits `::schemalane_cli::EmbeddedRunner` (macros/src/lib.rs:93-95) — the root re-export is load-bearing for every embedded crate ever compiled.

## Commands you will need

| Purpose | Command | Expected |
|---|---|---|
| Public-surface freeze | `grep -n "^pub" schemalane-cli/src/lib.rs > /tmp/cli-pub-before.txt`; re-grep across new files after | same set, all root-reachable |
| Grammar freeze | help snapshots as in plan 029 | identical |
| Gate | fmt + clippy gate + `cargo test --workspace --locked` | exit 0 |
| Macro-path proof | `cargo test -p schemalane-embed-tests --locked` (plan 024 crate) | pass |

## Scope

**In scope**: `schemalane-cli/src/**`.
**Out of scope**: behavior, grammar, output text; crate-external anything.

## Git workflow

- Branch: `advisor/032-cli-module-split`
- One commit per extraction.
- No push/PR without operator instruction.

## Steps

1. Freeze public surface + help snapshots.
2. Extract in order: `render.rs` → `prompt.rs` → `connect.rs` → `observer.rs` → `args.rs` → `delegate.rs` → `commands.rs` → `dispatch.rs` → `runner.rs`; after each, `mod` + `pub use` (only for the frozen public items) in lib.rs, clippy + `cargo test -p schemalane-cli --locked` green. Note lib.rs's crate-level `#![allow(clippy::print_stdout, clippy::print_stderr, clippy::future_not_send)]` applies crate-wide already — no per-module action needed.
3. Final: help diff identical; public-surface set identical and root-reachable; workspace gate + `schemalane-embed-tests` green.

**Verify**: as listed per step.

## Test plan

Existing CLI tests move with their subjects; plan 023's suite + help/public freezes are the net.

## Done criteria

- [ ] `wc -l schemalane-cli/src/lib.rs` ≤ ~80
- [ ] `EmbeddedRunner`, `run_cli`, `run_cli_with`, `Verbosity` importable at crate root (compile of embed-tests proves the macro path)
- [ ] Help byte-identical; all suites green
- [ ] `plans/README.md` updated

## STOP conditions

- Any frozen public item loses its root path — fix the re-export; if impossible, report.
- Help diff non-empty — clap attribute placement drifted during the move.

## Maintenance notes

- Rendering is now importable without the binary graph — future TUI/json-progress work and render unit tests get cheap.
- Keep `delegate.rs` free of any secret-in-argv regressions (plan 007's property test pins it).
