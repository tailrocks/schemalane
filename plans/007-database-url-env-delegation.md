# Plan 007: Pass the database URL to delegated processes via environment, not argv (`ps`-visible secret)

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat dd0d79d..HEAD -- schemalane-cli/src/lib.rs schemalane-core/src/lib.rs .gitignore`
> If any in-scope file changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding; on a
> mismatch, treat it as a STOP condition.

## Status

- **Priority**: P1
- **Effort**: S
- **Risk**: LOW
- **Depends on**: none (coordinates with plan 003 — both edit `run_via_migration_crate`; land in numeric order to avoid conflicts)
- **Category**: security
- **Planned at**: commit `dd0d79d`, 2026-07-03

## Why this matters

When `--migration-dir` points at a migration crate, the CLI spawns `cargo run --manifest-path … -- --database-url <URL>` with the **full connection URL, including the password, in the child's argv**. Process arguments are world-readable (`ps auxww`, `/proc/<pid>/cmdline`) for the entire delegated run — which includes a cargo build, i.e. potentially minutes. Worse, a caller who carefully supplied the secret via the `DATABASE_URL` env var gets it *re-exposed*: the parent reads the env var and re-emits it as argv. The child already reads `DATABASE_URL` from the environment, so the fix is a two-line change. This is defensive hygiene, not an exploit write-up: local users on shared hosts/bastions should not be able to read DB credentials from the process table.

If any real database has been migrated via delegation on a multi-user host, treat that credential as exposed and rotate it.

## Current state

- `schemalane-cli/src/lib.rs`, `run_via_migration_crate` (lines 651–660):

  ```rust
  let mut cargo = Command::new("cargo");
  cargo
      .arg("run")
      .arg("--manifest-path")
      .arg(manifest_path)
      .arg("--");

  if let Some(database_url) = database_url {
      cargo.arg("--database-url").arg(database_url);
  }
  ```

- The child parses args with clap and falls back to the environment — `EmbeddedCli` (line 468):

  ```rust
  #[arg(long, env = "DATABASE_URL")]
  database_url: String,
  ```

  `std::process::Command` inherits the parent environment by default, so an explicit `.env(...)` guarantees delivery even when the parent got the URL from `--database-url` rather than the env var.

- Scaffolded docs teach the argv pattern — `schemalane-core/src/lib.rs`, `INIT_PROJECT_README_TEMPLATE` (lines 1735–1743):

  ```
  cargo run -- --database-url "$DATABASE_URL" up
  ...
  cargo run --manifest-path ./migration/Cargo.toml -- --database-url "$DATABASE_URL" up
  ```

  And the CLI's own `init` output (lines 566–570) prints the same pattern.

- `.gitignore` (repo root) does not list `.env`; the scaffold `.gitignore` template is `"/target\n"` only (`schemalane-core/src/lib.rs:1746`).

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Format check | `cargo fmt --all -- --check` | exit 0 |
| Lint | `cargo clippy --workspace --locked --all-targets --all-features -- -D warnings` | exit 0 |
| Unit tests | `cargo test --workspace --locked` | pass |

## Scope

**In scope** (the only files you should modify):
- `schemalane-cli/src/lib.rs` (delegation argv + init output text)
- `schemalane-core/src/lib.rs` (README + .gitignore templates, and their locking test)
- `.gitignore` (add `.env`)

**Out of scope** (do NOT touch, even though they look related):
- The repo `README.md` — its example rewrite is part of `plans/009-docs-command-surface-truth.md`.
- The `--database-url` CLI flag itself — it stays supported (arguments to *schemalane* are the operator's own shell; the leak is re-emitting them into long-lived child argv).
- Exit-status handling in the same function (plan 003).

## Git workflow

- Branch: `advisor/007-database-url-env-delegation`
- Suggested commit: `Pass DATABASE_URL to delegated crate via environment`
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: Switch delegation to env delivery

In `run_via_migration_crate`, replace:

```rust
if let Some(database_url) = database_url {
    cargo.arg("--database-url").arg(database_url);
}
```

with:

```rust
// Deliver the URL via environment, never argv — argv is world-readable
// in the process table for the whole (cargo build + migrate) run.
if let Some(database_url) = database_url {
    cargo.env("DATABASE_URL", database_url);
}
```

**Verify**: `cargo clippy -p schemalane-cli --locked --all-targets -- -D warnings` → exit 0.

### Step 2: Make the argv construction unit-testable and assert the property

Refactor the argument assembly into a pure helper so the no-secret property is testable (this also serves `plans/023-cli-contract-tests.md`):

```rust
/// Build (args, envs) for the delegated `cargo run` invocation.
fn delegation_command_parts(
    manifest_path: &Path,
    database_url: Option<&str>,
    schema: &str,
    history_table: &str,
    installed_by: Option<&str>,
    command: &MigrateCommand,
    verbosity: Verbosity,
) -> (Vec<std::ffi::OsString>, Vec<(&'static str, String)>) { … }
```

`run_via_migration_crate` becomes: build parts → `Command::new("cargo")`, apply `.args(...)`/`.env(...)` → status handling (unchanged / per plan 003).

Add to the CLI test module (starts `schemalane-cli/src/lib.rs:1321`):

```rust
#[test]
fn delegation_never_puts_database_url_in_argv() {
    let (args, envs) = super::delegation_command_parts(
        Path::new("./m/Cargo.toml"),
        Some("postgres://u:pw-classified@h/db"),
        "public",
        "flyway_schema_history",
        None,
        &MigrateCommand::Up,
        Verbosity::Minimal,
    );
    assert!(args.iter().all(|a| !a.to_string_lossy().contains("pw-classified")));
    assert!(envs.iter().any(|(k, v)| *k == "DATABASE_URL" && v.contains("pw-classified")));
}
```

(Adjust `use super::…` imports in the test module as needed: `delegation_command_parts`, `Path`.)

**Verify**: `cargo test -p schemalane-cli --locked delegation_never` → 1 passed.

### Step 3: Fix the taught patterns

1. `schemalane-cli/src/lib.rs`, `run_root_cli` init output (lines 566–570) — change the printed run hint to:

   ```
   DATABASE_URL="postgres://…" cargo run --manifest-path {}/Cargo.toml -- up
   ```

2. `schemalane-core/src/lib.rs`, `INIT_PROJECT_README_TEMPLATE` — change both `cargo run … -- --database-url "$DATABASE_URL" up` examples to the `DATABASE_URL=… cargo run … -- up` form.

3. `INIT_GITIGNORE_TEMPLATE` (line 1746): `"/target\n"` → `"/target\n.env\n"`.

4. Repo root `.gitignore`: append a `# Secrets` section with `.env`.

Check the template-locking test `init_scaffold_creates_expected_files` (`schemalane-core/src/lib.rs:1968-2011`) — it asserts on Cargo.toml contents only, so it should pass unchanged; if it asserts README/gitignore text, update those assertions to the new text.

**Verify**: `cargo test -p schemalane-core --locked init_scaffold` → pass.

### Step 4: Full gate

**Verify**: fmt + clippy + `cargo test --workspace --locked` exit 0.

## Test plan

- `delegation_never_puts_database_url_in_argv` (Step 2) — the regression test.
- Existing scaffold tests re-run (Step 3).
- Manual (optional, Docker): delegated `up` against a scratch DB still connects — proves env delivery end-to-end.

## Done criteria

- [ ] `grep -n '"--database-url"' schemalane-cli/src/lib.rs` → no match inside delegation code (the clap arg definition remains)
- [ ] `grep -n 'env("DATABASE_URL"' schemalane-cli/src/lib.rs` → present
- [ ] `grep -n '\-\-database-url' schemalane-core/src/lib.rs` → no matches left in templates
- [ ] `.gitignore` contains `.env`
- [ ] fmt/clippy/workspace tests exit 0; only in-scope files modified
- [ ] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- Plan 003 landed first and `run_via_migration_crate` looks different — apply Steps 1–2 around the new status handling; if the function was split beyond recognition, stop.
- The child fails to receive the URL in the manual check (clap env fallback broken) — do not revert to argv; report.

## Maintenance notes

- Rotation: if delegated migrations ran on shared multi-user hosts before this fix, rotate those database passwords — the plan removes future exposure, not past exposure.
- Reviewers: any future flag carrying a secret must follow the same env-delivery rule for child processes.
- Deferred: masking the URL in the child's own error output paths is `plans/011-small-cli-core-bug-fixes.md` (SEC-05 investigate item).
