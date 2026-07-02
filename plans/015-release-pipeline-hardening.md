# Plan 015: Harden the release pipeline (idempotent publish, SHA-pinned actions, advisory scanning)

> **Executor instructions**: Follow this plan step by step, verifying each step.
> On any STOP condition, stop and report. When done, update `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat dd0d79d..HEAD -- .github/workflows/release.yml .github/workflows/ci.yml`
> On mismatch with "Current state" excerpts, STOP.

## Status

- **Priority**: P2
- **Effort**: M
- **Risk**: MED
- **Depends on**: none
- **Category**: dx / security
- **Planned at**: commit `dd0d79d`, 2026-07-03

## Why this matters

Three defects in the publish path:

1. **Republish fails hard.** `release.yml` unconditionally runs `cargo publish` for all four crates in sequence. `cargo publish` errors when the version already exists on crates.io, and each `run:` step aborts on first failure — so unless EVERY crate was version-bumped, the job dies mid-sequence (e.g. `schemalane-macros` has sat at 0.1.0; any re-dispatch fails at the first publish and core/cli never publish). There is no version-bump/changelog automation at all (versions: macros 0.1.0, pg_query_fmt 0.1.3, core 0.1.20, cli 0.1.26 — hand-managed).
2. **Supply-chain surface in the token-holding job.** The release job resolves `dtolnay/rust-toolchain@stable` (mutable branch) and tag-pinned `actions/checkout@v4` / `Swatinem/rust-cache@v2`, and restores a shared build cache — all inside the job that holds `CARGO_REGISTRY_TOKEN`. A moved tag/branch or poisoned cache runs attacker-controlled code next to the publish credential.
3. **No dependency-advisory scanning** anywhere in CI; a release can ship with a known-vulnerable transitive dep.

Also: the fixed `sleep 60` between dependent publishes is a race with crates.io index propagation — no retry if 60s is too short.

## Current state

- `.github/workflows/release.yml` (full relevant body):

  ```yaml
  on:
    workflow_dispatch:
  permissions:
    contents: read
  jobs:
    crates-io:
      name: Publish crates.io packages
      runs-on: ubuntu-latest
      environment: crates.io
      steps:
        - uses: actions/checkout@v4
        - uses: dtolnay/rust-toolchain@stable
          with:
            components: rustfmt, clippy
        - uses: Swatinem/rust-cache@v2
        - name: Verify
          run: |
            cargo fmt --all -- --check
            cargo clippy --workspace --locked --all-targets --all-features -- -D warnings
            cargo test --workspace --locked --all-targets --all-features
        - name: Publish
          env:
            CARGO_REGISTRY_TOKEN: ${{ secrets.CARGO_REGISTRY_TOKEN }}
          run: |
            cargo publish --locked -p schemalane-macros
            cargo publish --locked -p pg_query_fmt
            sleep 60
            cargo publish --locked -p schemalane-core
            sleep 60
            cargo publish --locked -p schemalane-cli
  ```

  Good parts to keep: `environment: crates.io` gate, step-scoped token, `permissions: contents: read`.

- `.github/workflows/ci.yml`: same three actions by tag; publish **dry-run** only for macros + pg_query_fmt (lines 39–41); core/cli only get `cargo package --list` (43–46) — so core/cli publishability is never validated before release day. No `cargo audit`/`cargo deny` anywhere.

- Local tooling: `cargo-audit` NOT installed on the dev machine; CI can install it.

## Commands you will need

| Purpose | Command | Expected |
|---|---|---|
| YAML sanity | `python3 -c "import yaml;yaml.safe_load(open('.github/workflows/release.yml'));yaml.safe_load(open('.github/workflows/ci.yml'));print('ok')"` | `ok` |
| Pin lookup | `gh api repos/dtolnay/rust-toolchain/commits/stable --jq .sha` (and analogous for checkout v4 / rust-cache v2 tags) | 40-char SHA |
| Audit dry run (local, optional) | `cargo install cargo-audit && cargo audit` | report (see STOP) |

## Scope

**In scope**: `.github/workflows/release.yml`, `.github/workflows/ci.yml`.
**Out of scope**: adopting `release-plz`/`cargo-release` end-to-end (recommended follow-up — needs a maintainer decision on versioning/changelog policy; see Maintenance notes); `rust-toolchain.toml` MSRV pinning (`plans/016-workspace-manifest-hygiene.md` owns `rust-version`; toolchain-file pinning is optional there).

## Git workflow

- Branch: `advisor/015-release-pipeline-hardening`
- Suggested commit: `ci: idempotent publish, SHA-pinned actions, cargo-audit job`
- No push/PR without operator instruction.

## Steps

### Step 1: Make publish idempotent (skip already-published versions)

Replace the Publish step's `run:` with a loop that checks crates.io first and polls the index instead of fixed sleeps:

```yaml
      - name: Publish
        env:
          CARGO_REGISTRY_TOKEN: ${{ secrets.CARGO_REGISTRY_TOKEN }}
        run: |
          set -euo pipefail
          publish_if_needed() {
            local crate="$1"
            local version
            version=$(cargo metadata --format-version 1 --no-deps \
              | jq -r ".packages[] | select(.name == \"$crate\") | .version")
            if curl -fsSL "https://crates.io/api/v1/crates/$crate/$version" >/dev/null 2>&1; then
              echo "$crate $version already published — skipping"
              return 0
            fi
            cargo publish --locked -p "$crate"
            # Wait until the sparse index serves the new version (max ~5 min)
            for _ in $(seq 1 30); do
              if curl -fsSL "https://index.crates.io/${crate:0:2}/${crate:2:2}/$crate" 2>/dev/null \
                 | grep -q "\"vers\":\"$version\""; then
                return 0
              fi
              sleep 10
            done
            echo "index never served $crate $version" >&2
            return 1
          }
          publish_if_needed schemalane-macros
          publish_if_needed pg_query_fmt
          publish_if_needed schemalane-core
          publish_if_needed schemalane-cli
```

Index-path note: the sparse-index path scheme is `/2-char/2-char/name` for names ≥4 chars (all four crates qualify); the `${crate:0:2}/${crate:2:2}` slicing implements it. `jq` is preinstalled on ubuntu runners.

**Verify**: YAML sanity command → `ok`; shellcheck the embedded script if available (`shellcheck` is preinstalled on runners; locally optional).

### Step 2: Pin actions by commit SHA and drop the cache from the publish job

1. Resolve SHAs (Commands table) for: `actions/checkout@v4`, `dtolnay/rust-toolchain` (pin to a **release tag's** SHA, not the `stable` branch), `Swatinem/rust-cache@v2`.
2. In **release.yml**: replace each `uses: owner/repo@tag` with `uses: owner/repo@<full-sha> # tag`, and **delete** the `Swatinem/rust-cache` step entirely (clean build in the credentialed job; it runs rarely — the extra minutes are the price of not publishing from a cache).
3. In **ci.yml**: pin the same three actions by SHA (cache stays — CI has no publish credential).

**Verify**: `grep -n "uses:" .github/workflows/*.yml` → every line has a 40-hex-char ref with a trailing `# <tag>` comment; release.yml has no rust-cache.

### Step 3: Validate ALL four crates' publishability in CI

In ci.yml, replace the split dry-run/package steps (lines 38–46) with:

```yaml
      - name: Publish dry run
        run: |
          cargo publish --dry-run --locked --allow-dirty -p schemalane-macros
          cargo publish --dry-run --locked --allow-dirty -p pg_query_fmt
          cargo package --locked --allow-dirty -p schemalane-core
          cargo package --locked --allow-dirty -p schemalane-cli
```

Rationale: `--dry-run` for core/cli fails when their path-deps' versions aren't on crates.io yet, so `cargo package` (full build of the packaged artifact, minus upload) is the strongest pre-release check available for dependent crates; the current `--list >/dev/null` variant skips the build entirely. (If the resolved cargo supports `--dry-run` with unpublished path-deps in future, upgrade then.)

**Verify**: run the four commands locally → all exit 0.

### Step 4: Add an advisory-audit job to CI

Append to ci.yml:

```yaml
  audit:
    name: Security audit
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@<same-pinned-sha> # v4
      - uses: dtolnay/rust-toolchain@<same-pinned-sha> # stable tag
      - name: Install cargo-audit
        run: cargo install cargo-audit --locked
      - name: Audit
        run: cargo audit
```

**Verify**: YAML sanity → `ok`. Optionally run `cargo audit` locally first (see STOP conditions for handling existing advisories).

### Step 5: Full gate

fmt/clippy/test unchanged (no Rust code touched): run the standard gate anyway → exit 0.

## Test plan

Workflow changes can't be executed locally; the tests are: YAML parse checks, local execution of the Step 3 commands, and the reviewer's first `workflow_dispatch` after merge (expected: all four `publish_if_needed` calls print "already published — skipping" when nothing was bumped — the previously-failing case now no-ops).

## Done criteria

- [ ] `grep -c "sleep 60" .github/workflows/release.yml` → 0
- [ ] All `uses:` pinned to SHAs with tag comments; no rust-cache in release.yml
- [ ] ci.yml packages/dry-runs all four crates and has an `audit` job
- [ ] YAML sanity `ok`; Step 3 commands pass locally
- [ ] Only the two workflow files modified; `plans/README.md` updated

## STOP conditions

- `cargo audit` (local or first CI run) reports advisories in the current tree — do NOT silence or `--ignore` them in this plan; list them in your report (fixing deps is its own decision; the job may need a temporary allowlist the maintainer approves).
- `gh api` cannot resolve a tag SHA (offline) — leave that action tag-pinned, mark the step partial.
- crates.io API shape changed (Step 1 curl checks fail against known-published crates) — report; don't guess new endpoints.

## Maintenance notes

- **Recommended follow-up (maintainer decision, deliberately not in this plan)**: adopt `release-plz` for automated version bumps + changelog + tag-driven publishing; it obsoletes Step 1's hand-rolled idempotency. The Step 1 script is the low-risk bridge until then.
- Dependabot/Renovate for action-SHA bumps pairs naturally with Step 2 — otherwise pinned SHAs slowly stale.
- The `environment: crates.io` protection and step-scoped token were already correct — keep them through any future refactor.
