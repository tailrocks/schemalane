# Plan 001: Make CI actually execute the database integration tests

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat dd0d79d..HEAD -- .github/workflows/ci.yml schemalane-core/tests/postgres_integration.rs README.md`
> If any in-scope file changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding; on a
> mismatch, treat it as a STOP condition.

## Status

- **Priority**: P1
- **Effort**: M
- **Risk**: LOW
- **Depends on**: none
- **Category**: tests
- **Planned at**: commit `dd0d79d`, 2026-07-03

## Why this matters

Schemalane is a PostgreSQL migration engine, and every test that touches PostgreSQL is marked `#[ignore]`. CI (`.github/workflows/ci.yml`) runs `cargo test` **without** `--include-ignored` and provisions no Docker/Postgres, so the product's core behavior — applying migrations, writing history rows, rollback, `fresh`, checksum-mismatch detection — has **zero** automated verification. CI stays green if any of it breaks. This plan is the verification baseline: several later plans (`plans/020-*`, `plans/021-*`, `plans/002-*`) refactor exactly these DB paths and must not land before this exists.

## Current state

- `.github/workflows/ci.yml` — single `rust` job: fmt → clippy → test → publish dry-run. The test step (line 36):

  ```yaml
  - name: Test
    run: cargo test --workspace --locked --all-targets --all-features
  ```

  No `services:` block, no `--include-ignored`. Note: no crate in the workspace declares a `[features]` table, so `--all-features` is a no-op (harmless; keep or drop).

- `schemalane-core/tests/postgres_integration.rs` — 7 tests, each annotated (lines 14, 88, 148, 209, 270, 324, 386):

  ```rust
  #[test]
  #[ignore = "requires Docker daemon"]
  fn up_and_status_with_sql_migrations() -> Result<(), Box<dyn Error + 'static>> {
      let node = Postgres::default().start()?;
  ```

  Each test starts its **own** disposable Postgres container via `testcontainers-modules` (`SyncRunner`), uses its own `TempDir`, and asserts row counts/states. They are self-isolated — no shared state, no ordering dependence. They only need a reachable Docker daemon.

- `README.md` — documents no test commands at all.

- GitHub `ubuntu-latest` runners ship with a running Docker daemon; `testcontainers` works there without extra services.

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Format check | `cargo fmt --all -- --check` | exit 0 |
| Lint | `cargo clippy --workspace --locked --all-targets --all-features -- -D warnings` | exit 0 |
| Unit tests (fast, no Docker) | `cargo test --workspace --locked` | pass; integration tests reported as `ignored` |
| Integration tests (needs Docker) | `cargo test -p schemalane-core --locked --test postgres_integration -- --include-ignored` | `test result: ok. 7 passed` |
| Docker probe | `docker info` | exit 0 |

## Scope

**In scope** (the only files you should modify):
- `.github/workflows/ci.yml`
- `README.md` (add a short "Testing" section only)

**Out of scope** (do NOT touch, even though they look related):
- `schemalane-core/tests/postgres_integration.rs` — do not remove `#[ignore]`; the attribute is what keeps the default `cargo test` fast for machines without Docker. CI opts in explicitly.
- `.github/workflows/release.yml` — release hardening is `plans/015-release-pipeline-hardening.md`.
- Any Rust source file.

## Git workflow

- Branch: `advisor/001-ci-verification-baseline`
- Commit style: short imperative with area prefix, matching repo history (e.g. `ci: add standalone validation`). Suggested: `ci: run postgres integration tests in CI`
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: Confirm the integration suite passes locally

Run `docker info`. If it fails, this machine has no Docker — see STOP conditions.

Run:

```sh
cargo test -p schemalane-core --locked --test postgres_integration -- --include-ignored
```

**Verify**: output ends with `test result: ok. 7 passed; 0 failed` (first run may take minutes pulling the `postgres` image).

### Step 2: Add an integration job to `.github/workflows/ci.yml`

Append a second job (keep the existing `rust` job untouched):

```yaml
  integration:
    name: Integration (Postgres)
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - uses: dtolnay/rust-toolchain@stable

      - uses: Swatinem/rust-cache@v2

      - name: Integration tests
        run: cargo test -p schemalane-core --locked --test postgres_integration -- --include-ignored
```

Keep indentation consistent with the existing job (2 spaces). Do not add `services: postgres` — testcontainers manages its own container and a service container would be unused.

**Verify**: `python3 -c "import yaml,sys; yaml.safe_load(open('.github/workflows/ci.yml')); print('ok')"` → prints `ok`. (If PyYAML is unavailable, use any YAML validator available; a plain `git diff` review of indentation is the fallback.)

### Step 3: Document the two-tier test loop in README

Add a `## Testing` section to `README.md` (after "Commands"):

```markdown
## Testing

Fast unit tests (no Docker):

```sh
cargo test --workspace
```

Full suite including PostgreSQL integration tests (requires a running Docker daemon; testcontainers starts a disposable Postgres per test):

```sh
cargo test -p schemalane-core --test postgres_integration -- --include-ignored
```
```

**Verify**: `grep -n "include-ignored" README.md` → one match.

### Step 4: Full local gate

**Verify**:
- `cargo fmt --all -- --check` → exit 0
- `cargo clippy --workspace --locked --all-targets --all-features -- -D warnings` → exit 0
- `cargo test --workspace --locked` → pass

## Test plan

No new tests — this plan makes existing tests run. The 7 integration tests in `schemalane-core/tests/postgres_integration.rs` are the payload; Step 1 proves them green locally, the new CI job proves them green remotely (verifiable by the reviewer on first push — the executor cannot push).

## Done criteria

Machine-checkable. ALL must hold:

- [ ] `.github/workflows/ci.yml` contains a job running `--include-ignored` against `postgres_integration`
- [ ] `cargo test -p schemalane-core --locked --test postgres_integration -- --include-ignored` → 7 passed locally
- [ ] `README.md` has the Testing section with both commands
- [ ] `cargo fmt --all -- --check` and the clippy gate exit 0
- [ ] `git status` shows only the two in-scope files modified
- [ ] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- No Docker daemon is available locally (`docker info` fails). Then: still write the workflow + README changes, state clearly that local verification was impossible, and mark the plan BLOCKED-partial in the index.
- Any of the 7 integration tests fails locally at unmodified `HEAD` — that is a product bug the audit didn't catch; report the failing test and its output instead of "fixing" it here.
- The workflow file has drifted (drift check) — reconcile before editing.

## Maintenance notes

- Later plans (`020`, `021`, `002`) rely on this job to catch regressions in the apply/lock/history paths; if the job is ever disabled, those plans' risk ratings are invalid.
- Reviewer should scrutinize: that the new job does NOT remove `#[ignore]` (keeps local default fast), and CI wall time (image pull ~1 min; acceptable).
- Deferred: `--all-features` in the existing test/clippy steps is a no-op (no features exist anywhere in the workspace). Left as-is here; `plans/015-release-pipeline-hardening.md` may tidy it.
