# Plan 016: Workspace manifest hygiene — shared `[workspace.dependencies]`, coherent pins, declared MSRV

> **Executor instructions**: Follow this plan step by step, verifying each step.
> On any STOP condition, stop and report. When done, update `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat dd0d79d..HEAD -- Cargo.toml schemalane-core/Cargo.toml schemalane-cli/Cargo.toml schemalane-macros/Cargo.toml pg_query_fmt/Cargo.toml`
> On mismatch with "Current state", STOP.

## Status

- **Priority**: P2
- **Effort**: S-M
- **Risk**: LOW
- **Depends on**: none
- **Category**: dependencies
- **Planned at**: commit `dd0d79d`, 2026-07-03

## Why this matters

Shared dependencies are declared independently in four manifests with no `[workspace.dependencies]` table, and the declared versions already disagree with each other and with the lockfile: `tokio` is `1.50.0` in the CLI but `=1.52.3` in core's dev-deps (both resolve to 1.52.3); `tokio-postgres` is `0.7.13` as a dependency but `=0.7.17` as a dev-dependency (both resolve 0.7.17). Dev-deps use exact `=` pins while regular deps float — with `Cargo.lock` committed, the `=` pins add no reproducibility, only rigidity, and the decorative version strings mislead readers. Separately, no crate declares `rust-version`, so consumers on old toolchains get an opaque edition-2024 parse error instead of a clear MSRV message, and crates.io/docs.rs show no MSRV.

## Current state

- Root `Cargo.toml` (lines 1–13): workspace members + `resolver = "3"`, `[workspace.lints.*]` — **no** `[workspace.package]`, **no** `[workspace.dependencies]`.
- `schemalane-core/Cargo.toml`:
  - deps (21–28): `crc32fast 1.5.0`, `deadpool-postgres 0.14.1`, `schemalane-macros {0.1.0, path}`, `serde 1.0.228`, `pg_query 6.1.1`, `pg_query_fmt {0.1.3, path}`, `thiserror 2.0.18`, `tokio-postgres 0.7.13`
  - dev-deps (30–34): `tempfile "=3.27.0"`, `testcontainers-modules "=0.15.0"`, `tokio "=1.52.3"`, `tokio-postgres "=0.7.17"`
- `schemalane-cli/Cargo.toml` (21–29): `clap 4.5.60`, `comfy-table 7.2.2`, `deadpool-postgres 0.14.1`, `owo-colors 4.3.0`, `schemalane-core {0.1.20, path}`, `serde_json 1.0.149`, `pg_query_fmt {0.1.3, path}`, `tokio 1.50.0`, `tokio-postgres 0.7.13`
- `schemalane-macros/Cargo.toml`: `proc-macro2 1.0.106`, `quote 1.0.45`, `syn 2.0.117`; dev `trybuild "=1.0.116"`
- `pg_query_fmt/Cargo.toml`: `owo-colors 4.3.0`, `pg_query 6.1.1`
- All four share identical `edition = "2024"`, `license`, `repository`, `homepage`, `readme = "../README.md"`, `keywords` — candidates for `[workspace.package]`.
- Edition 2024 requires Rust ≥ 1.85 (resolver 3 requires 1.84); local toolchain is 1.96.1.
- `Cargo.lock` committed (git-tracked).

## Commands you will need

| Purpose | Command | Expected |
|---|---|---|
| Resolution unchanged proof | `cargo metadata --format-version 1 > /tmp/before.json` (before) / diff of `cargo tree` output (after) | see Step 4 |
| MSRV sanity (optional if installed) | `cargo +1.85.0 check --workspace` or `cargo msrv verify` | pass / skip |
| Gate | fmt + `cargo clippy --workspace --locked --all-targets --all-features -- -D warnings` + `cargo test --workspace --locked` | exit 0 |

## Scope

**In scope**: all five `Cargo.toml` files; `Cargo.lock` only if resolution legitimately changes (goal: it does not).
**Out of scope**: dependency **upgrades** (no version bumps beyond unifying declared strings to what the lock already resolves); CI toolchain pinning (plan 015 territory); the duplicate-major noise in the dev tree (testcontainers stack) — accepted, documented below.

## Git workflow

- Branch: `advisor/016-workspace-manifest-hygiene`
- Suggested commit: `Hoist shared deps to workspace, declare MSRV, drop decorative pins`
- No push/PR without operator instruction.

## Steps

### Step 1: Snapshot current resolution

```sh
cargo tree --workspace -e normal,dev --prefix none | sort -u > /tmp/tree-before.txt
```

**Verify**: file non-empty.

### Step 2: Add `[workspace.package]` and `[workspace.dependencies]` to the root

In root `Cargo.toml`:

```toml
[workspace.package]
edition = "2024"
license = "MIT OR Apache-2.0"
repository = "https://github.com/donbeave/schemalane"
homepage = "https://github.com/donbeave/schemalane"
rust-version = "1.85"

[workspace.dependencies]
# runtime, shared across crates
deadpool-postgres = "0.14.1"
owo-colors = "4.3.0"
pg_query = "6.1.1"
tokio = "1.52"
tokio-postgres = "0.7.17"
# intra-workspace
schemalane-core = { version = "0.1.20", path = "schemalane-core" }
schemalane-cli = { version = "0.1.26", path = "schemalane-cli" }
schemalane-macros = { version = "0.1.0", path = "schemalane-macros" }
pg_query_fmt = { version = "0.1.3", path = "pg_query_fmt" }
```

(Version strings deliberately match what the lock already resolves — this must NOT bump anything.)

### Step 3: Point the crates at the workspace tables

In each crate manifest:

- Replace `edition`, `license`, `repository`, `homepage` with `.workspace = true` forms (`edition.workspace = true`, etc.) and add `rust-version.workspace = true`. Keep per-crate `name`, `version`, `description`, `documentation`, `readme`, `keywords` as-is (they genuinely differ or are per-crate).
- Replace every dep that exists in `[workspace.dependencies]` with `dep = { workspace = true }` (+ `features = […]` where the crate declares features — e.g. cli's `tokio = { workspace = true, features = ["macros", "rt-multi-thread"] }`, core dev-dep `tokio = { workspace = true, features = ["rt-multi-thread"] }`, `serde` stays per-crate (only core uses it) or hoist too — hoist only multi-crate deps; single-crate deps stay local for clarity).
- Drop the `=` from dev-deps: `tempfile = "3.27"`, `testcontainers-modules = { version = "0.15", … }`, `trybuild = "1.0"`, and use workspace `tokio`/`tokio-postgres`. The committed `Cargo.lock` preserves exact reproducibility; the manifests now express real constraints.

**Verify**: `cargo check --workspace --locked` → exit 0 **with `--locked`** (proves no resolution change; if it errors "lock file needs update", a declared string didn't match the lock — fix the string, not the lock).

### Step 4: Prove resolution is unchanged

```sh
cargo tree --workspace -e normal,dev --prefix none | sort -u > /tmp/tree-after.txt
diff /tmp/tree-before.txt /tmp/tree-after.txt
```

**Verify**: empty diff. (If non-empty, a version string drifted — reconcile to the lock's version.)

### Step 5: Full gate

fmt + clippy + `cargo test --workspace --locked` → exit 0. Also `cargo publish --dry-run --locked --allow-dirty -p schemalane-macros -p pg_query_fmt` (one at a time) → exit 0, confirming `workspace = true` inheritance survives packaging.

## Test plan

No behavioral tests — the invariant is "same resolution, cleaner declarations", proven by Steps 3–5 (`--locked` + tree diff + publish dry-run).

## Done criteria

- [ ] Root has `[workspace.package]` (with `rust-version = "1.85"`) and `[workspace.dependencies]`
- [ ] `grep -rn '"=' */Cargo.toml` → no exact pins remain
- [ ] `grep -c "workspace = true" schemalane-cli/Cargo.toml` ≥ 5
- [ ] Step 4 diff empty; Step 5 gates green
- [ ] `plans/README.md` updated

## STOP conditions

- `--locked` check fails after Step 3 and the mismatch isn't a typo — some declared version cannot express the locked resolution (e.g. lock has two semver-incompatible copies); report the exact dep.
- `cargo publish --dry-run` rejects `workspace = true` inheritance for a publishable field — report cargo's error (would indicate an old cargo on the release runner; coordinate with plan 015).
- MSRV verification (if run) fails below 1.96 for a reason other than edition/resolver — the true MSRV is higher; set `rust-version` to what actually passes and note it.

## Maintenance notes

- Renovate/Dependabot now needs to touch one table for shared bumps — that's the point.
- Known/accepted lockfile noise (do not chase): dev-tree duplicate majors from the testcontainers stack (`schemars`, `prost`, `indexmap`, …) and the runtime `thiserror 1.x` forced by `pg_query` alongside workspace `thiserror 2.x` — upstream-bound; revisit on pg_query upgrades.
- `rust-version = "1.85"` is the edition-2024 floor; verify against a real old toolchain when one is handy (`cargo msrv` in CI is a nice follow-up).
