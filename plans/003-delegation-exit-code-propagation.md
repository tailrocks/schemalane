# Plan 003: Propagate the delegated migration crate's exit code (today every failure becomes exit 2)

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat dd0d79d..HEAD -- schemalane-cli/src/lib.rs schemalane-core/src/lib.rs`
> If any in-scope file changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding; on a
> mismatch, treat it as a STOP condition.

## Status

- **Priority**: P1
- **Effort**: S-M
- **Risk**: MED
- **Depends on**: none
- **Category**: bug
- **Planned at**: commit `dd0d79d`, 2026-07-03

## Why this matters

`SCHEMALANE_SPEC.md` §8 defines an exit-code contract: 0 success, 1 runtime/DB error, 2 validation, 3 drift, 4 failed migration in history, 5 pending with `--fail-on-pending`, 6 fresh guard. When `--migration-dir` points at a migration crate (the primary documented mode, spec §2.2), the CLI delegates to `cargo run --manifest-path …`. The child process exits with the correct code, but the parent discards it and wraps every non-zero outcome in `SchemalaneError::Validation`, which maps to exit **2**. CI pipelines keying on "3 = drift", "4 = failed migration", "5 = pending" silently misclassify every failure. Additionally, "cargo not installed" also surfaces as 2, though the spec assigns runtime errors 1.

## Current state

- `schemalane-cli/src/lib.rs`, `run_via_migration_crate` (lines 703–717) — the discard site:

  ```rust
  let status = cargo.status().map_err(|err| {
      SchemalaneError::Validation(format!(
          "failed to run cargo for migration crate {}: {err}",
          manifest_path.display()
      ))
  })?;

  if status.success() {
      Ok(())
  } else {
      Err(SchemalaneError::Validation(format!(
          "migration crate command failed for {} with status {status} (see output above)",
          manifest_path.display()
      )))
  }
  ```

- `schemalane-core/src/lib.rs`, `SchemalaneError::exit_code` (lines 84–94):

  ```rust
  pub const fn exit_code(&self) -> i32 {
      match self {
          Self::Validation(_) => 2,
          Self::Drift(_) => 3,
          Self::FailedHistory(_) => 4,
          Self::PendingMigrations(_) => 5,
          Self::FreshRequiresConfirm => 6,
          Self::MixedStatements { .. } => 7,
          _ => 1,
      }
  }
  ```

- Top-level exit sites, `schemalane-cli/src/lib.rs`:
  - `run_cli` (lines 371–376): `eprintln!("{err}"); std::process::exit(err.exit_code());`
  - `EmbeddedRunner::run` (lines 336–341): same pattern.

- The child is the generated migration binary whose `main` calls `embedded::migrations::runner().run().await` — i.e. `EmbeddedRunner::run` above — so the child already exits with the spec-correct code, and `cargo run` forwards the child's exit code as its own.

- Error enum convention: `thiserror` with `#[error("…")]` attributes, `schemalane-core/src/lib.rs:44-81`.

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Format check | `cargo fmt --all -- --check` | exit 0 |
| Lint | `cargo clippy --workspace --locked --all-targets --all-features -- -D warnings` | exit 0 |
| Unit tests | `cargo test --workspace --locked` | pass |

## Scope

**In scope** (the only files you should modify):
- `schemalane-core/src/lib.rs` (new error variant + `exit_code` arm + unit tests)
- `schemalane-cli/src/lib.rs` (`run_via_migration_crate` mapping)

**Out of scope** (do NOT touch, even though they look related):
- The undocumented exit code 7 for `MixedStatements` — spec reconciliation is `plans/009-docs-command-surface-truth.md`.
- Forwarding of `--database-url` to the child (that changes in `plans/007-database-url-env-delegation.md`; this plan must not conflict — it only touches the status-handling tail of the function).
- Testing the full delegation argv (that is `plans/023-cli-contract-tests.md`).

## Git workflow

- Branch: `advisor/003-delegation-exit-code-propagation`
- Suggested commit: `Propagate delegated migration exit codes verbatim`
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: Add a `Delegated` error variant carrying the child's exit code

In `schemalane-core/src/lib.rs`, extend `SchemalaneError` (keep alphabetic/insertion placement consistent — append after `PendingMigrations`):

```rust
#[error("migration crate command exited with code {code}")]
Delegated { code: i32 },
```

And in `exit_code()` add, before the `_ => 1` arm:

```rust
Self::Delegated { code } => *code,
```

Note `exit_code` is `pub const fn` — dereferencing a field in a const fn is fine on the workspace toolchain (edition 2024, Rust ≥1.85).

**Verify**: `cargo clippy -p schemalane-core --locked --all-targets -- -D warnings` → exit 0.

### Step 2: Map child status → `Delegated`, spawn failure → runtime error

In `schemalane-cli/src/lib.rs`, `run_via_migration_crate`, replace the block quoted in "Current state" with:

```rust
let status = cargo.status().map_err(|err| {
    SchemalaneError::Io(std::io::Error::new(
        err.kind(),
        format!(
            "failed to run cargo for migration crate {}: {err}",
            manifest_path.display()
        ),
    ))
})?;

if status.success() {
    Ok(())
} else {
    // The child already printed its own error and exited with a
    // spec-conformant code; forward that code verbatim. A signal
    // death has no code — treat it as a runtime error (1).
    Err(SchemalaneError::Delegated {
        code: status.code().unwrap_or(1),
    })
}
```

Rationale: `SchemalaneError::Io` exists (`#[error("IO error: {0}")] Io(#[from] std::io::Error)`, line 46-47) and falls into the `_ => 1` exit arm — spawn failure ("cargo not found") now exits 1 per spec.

**Verify**: `cargo clippy -p schemalane-cli --locked --all-targets -- -D warnings` → exit 0.

### Step 3: Unit tests for the new mapping

In the existing `#[cfg(test)] mod tests` of `schemalane-core/src/lib.rs` (starts line 1916), add:

```rust
#[test]
fn delegated_error_exit_code_is_forwarded_verbatim() {
    for code in [1, 2, 3, 4, 5, 6, 7, 42] {
        assert_eq!(SchemalaneError::Delegated { code }.exit_code(), code);
    }
}
```

(Import `SchemalaneError` is already in the test module's `use super::{…}` list.)

**Verify**: `cargo test -p schemalane-core --locked delegated_error` → 1 passed.

### Step 4: End-to-end smoke check (manual, no DB needed)

Create a throwaway delegation target that exits non-zero fast:

```sh
cd "$(mktemp -d)" && cargo init --name fakemig . >/dev/null 2>&1
printf 'fn main() { std::process::exit(4); }\n' > src/main.rs
cd - >/dev/null
```

Run the CLI against it (substitute the temp dir):

```sh
cargo run -p schemalane-cli -- migrate -d <TMPDIR> up; echo "exit=$?"
```

**Verify**: prints `exit=4` (previously would print `exit=2`).

## Test plan

- Unit: `delegated_error_exit_code_is_forwarded_verbatim` (Step 3).
- Manual E2E: Step 4 (a fake crate exiting 4 → CLI exits 4).
- Full exit-code table tests across all variants land in `plans/023-cli-contract-tests.md`; do not duplicate them here.

## Done criteria

Machine-checkable. ALL must hold:

- [ ] `grep -n "Delegated" schemalane-core/src/lib.rs` → variant + exit_code arm + test
- [ ] Step 4 smoke prints `exit=4`
- [ ] fmt/clippy/`cargo test --workspace --locked` all exit 0
- [ ] Only the two in-scope files modified
- [ ] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- `run_via_migration_crate` no longer matches the excerpt (e.g. plan 007 landed first and restructured it) — re-read the function and apply only the status-tail change; if the tail is unrecognizable, stop.
- Adding the variant breaks external pattern matches in-workspace beyond `exit_code` (clippy/compile will show) — list them, don't silence them.
- Step 4 prints `exit=101` or similar cargo-build-failure codes for the fake crate — the fake crate didn't build; fix the fixture, not the CLI.

## Maintenance notes

- The parent prints `Delegated`'s Display ("migration crate command exited with code N") via `eprintln!` before exiting — after the child already printed its real error. That is one extra stderr line, kept deliberately so the parent's failure is attributable. If it's judged noisy, silencing must special-case `Delegated` at the two exit sites — a display decision, deferred.
- `SchemalaneError` gains a variant — external exhaustive matches break (pre-1.0, acceptable). `plans/026-published-api-hygiene.md` adds `#[non_exhaustive]`, after which this stops being a breaking event.
- If a future plan changes delegation to `exec`-style replacement, this mapping becomes moot — delete the variant then.
