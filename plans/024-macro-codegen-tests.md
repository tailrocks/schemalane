# Plan 024: Test `embed_migrations!` success-path code generation (today: one compile-fail case only)

> **Executor instructions**: Follow this plan step by step, verifying each step.
> On any STOP condition, stop and report. When done, update `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat dd0d79d..HEAD -- schemalane-macros/`
> On mismatch with "Current state", STOP.

## Status

- **Priority**: P2
- **Effort**: M
- **Risk**: LOW
- **Depends on**: none (plan 012's `.RS` fix interacts — see Step 3)
- **Category**: tests
- **Planned at**: commit `dd0d79d`, 2026-07-03

## Why this matters

`embed_migrations!` is the primary embedded-registration mechanism (spec §2.4): it generates modules, `build_migrator`, `MIGRATIONS_DIR`, and `runner()`. Its **success path is never compiled by any test** — the single trybuild case is a compile-fail (`missing_migrations_dir`), and no workspace crate invokes the macro (integration tests register executors manually). A codegen regression — bad identifier sanitization, module-name collision, wrong `#[path]`, broken `runner()` signature against `schemalane-cli` — ships cleanly and explodes only in downstream users' builds. The helper logic (`unique_module_ident` collision suffixing, `sanitize_ident` edge cases, discovery sort) is also untested.

## Current state

- `schemalane-macros/tests/trybuild.rs` (whole file):

  ```rust
  #[test]
  fn ui() {
      let t = trybuild::TestCases::new();
      t.compile_fail("tests/ui/missing_migrations_dir.rs");
  }
  ```

- `schemalane-macros/src/lib.rs`:
  - `embed_migrations` (9–99): resolves dir relative to `CARGO_MANIFEST_DIR`, canonicalizes, discovers `.rs` files, generates per-file `#[path] mod`, `build_migrator` registering `RustMigrationExecutor::new(|client| Box::pin(module::migration(client)))`, `MIGRATIONS_DIR` const, `runner() -> ::schemalane_cli::EmbeddedRunner`.
  - `unique_module_ident` (228–240): sanitized stem; collisions get `_2`, `_3`… suffixes.
  - `sanitize_ident` (242–265): non-alphanumeric → `_`, trim `_`, empty → `"migration"`, leading digit → `m_` prefix.
  - Tests (277–320): 4 filename-parse tests only.
- Generated code references BOTH `::schemalane_core` and `::schemalane_cli` — a test crate exercising the macro needs both as dependencies. `schemalane-macros` itself cannot dev-depend on `schemalane-core` (cyclic: core depends on macros). **A separate test-only crate breaks the cycle.**
- trybuild `pass` cases run with the test crate's dependencies; a `tests/ui/*.rs` pass file inside schemalane-macros cannot see schemalane-core/cli (not deps). Hence the dedicated fixture-crate approach below.
- Workspace members list: root `Cargo.toml:2-7`.

## Commands you will need

| Purpose | Command | Expected |
|---|---|---|
| New-crate check | `cargo test -p schemalane-embed-tests --locked` | pass |
| Gate | fmt + clippy gate + `cargo test --workspace --locked` | exit 0 |

## Scope

**In scope**:
- NEW crate `schemalane-embed-tests/` (workspace member, `publish = false`)
- root `Cargo.toml` (add member; exclude from `default-members`? — include it in `members` but NOT in `default-members`, matching how `schemalane-macros` is excluded)
- `schemalane-macros/src/lib.rs` — unit tests for `sanitize_ident`/`unique_module_ident` only (no production changes)

**Out of scope**: publishing config for the new crate (it must have `publish = false`); macro production code (bugs found → STOP); CI file (the workspace test command already picks up new members).

## Git workflow

- Branch: `advisor/024-macro-codegen-tests`
- Suggested commit: `Add embed_migrations success-path test crate and helper unit tests`
- No push/PR without operator instruction.

## Steps

### Step 1: Create the fixture crate

```
schemalane-embed-tests/
  Cargo.toml
  migrations/
    V1__first.rs
    V2_5__second-thing.rs      # sanitizes: dash → _, tests sanitize_ident
    V3__2fast.rs               # module stem starts with digit after V3__? No —
                               # stem is "v3__2fast"; keep it for coverage anyway
  src/lib.rs
```

`Cargo.toml`:

```toml
[package]
name = "schemalane-embed-tests"
version = "0.0.0"
edition = "2024"
publish = false

[lints]
workspace = true

[dependencies]
schemalane-core = { path = "../schemalane-core" }
schemalane-cli = { path = "../schemalane-cli" }
tokio-postgres = "0.7"
```

(Use `workspace = true` dep forms instead if plan 016 landed.)

Each migration file follows the scaffold shape (`schemalane-core/src/lib.rs`, `INIT_RUST_MIGRATION_TEMPLATE`):

```rust
use tokio_postgres::Client;

pub async fn migration(client: &Client) -> Result<(), tokio_postgres::Error> {
    let _ = client;
    Ok(())
}
```

`src/lib.rs`:

```rust
//! Compile-and-run tests for `schemalane_core::embed_migrations!` codegen.
pub mod embedded {
    use schemalane_core::embed_migrations;

    embed_migrations!("./migrations");
}

#[cfg(test)]
mod tests {
    use super::embedded::migrations;

    #[test]
    fn migrations_dir_is_absolute_and_exists() {
        let dir = std::path::Path::new(migrations::MIGRATIONS_DIR);
        assert!(dir.is_absolute());
        assert!(dir.join("V1__first.rs").exists());
    }

    #[test]
    fn build_migrator_registers_all_rust_migrations() {
        // build_migrator + a migrations dir containing the same files must
        // pass executor validation — smoke it through discovery by calling
        // the same entry the runner uses:
        let migrator = migrations::build_migrator(schemalane_core::SchemalaneConfig {
            migrations_dir: std::path::PathBuf::from(migrations::MIGRATIONS_DIR),
            ..Default::default()
        });
        // No pool here; assert registration indirectly: the migrator was
        // built and the generated fn signature matched. Executor-presence
        // assertions need a pub accessor — see STOP conditions if absent.
        let _ = migrator;
    }

    #[test]
    fn runner_constructs() {
        let _runner = migrations::runner();
    }
}
```

Add `"schemalane-embed-tests"` to `[workspace] members` (NOT `default-members`).

**Verify**: `cargo test -p schemalane-embed-tests --locked` → compiles (the codegen contract!) and tests pass.

### Step 2: Scope decision on registration assertions (recorded)

`SchemalaneMigrator` exposes no getter for registered executors, and adding one would be a production change — out of scope here. Coverage level for this plan is therefore: **compile-level** (the crate building proves codegen validity) + **construction-level** (`build_migrator`/`runner` callable). Full behavioral E2E through the generated `runner()` against a live DB cannot live in core's integration tests (dependency cycle: the fixture crate depends on core) — recorded as a deliberate gap in Maintenance notes. No action in this step; it exists so the executor doesn't invent a getter.

### Step 3: Collision + case fixtures

Exercise `unique_module_ident`'s `_2` suffix path with two files whose sanitized stems collide. Collision mechanics: `sanitize_ident` trims trailing `_`, so stems `v9__m` (from `V9__m.rs`) and `v9__m_` (from `V9__m_.rs`) both sanitize to `v9__m` → the second gets `v9__m_2`.

Add: `migrations/V9__m.rs` and `migrations/V9__m_.rs` (same trivial body as Step 1's files).

Caveat to document in a fixture `README.md` inside the crate: both files carry version 9, so this directory is a **codegen corpus, not a runnable migration set** — runtime discovery would reject the duplicate version, but nothing in these tests runs discovery (construction doesn't discover), so the tests are unaffected.

If plan 012 landed (case-insensitive extension filter): also add `migrations/V10__upper.RS` — successful compilation proves the macro embedded it (the generated `#[path]` module must resolve).

**Verify**: `cargo test -p schemalane-embed-tests --locked` still compiles+passes; read the expanded code if in doubt: `cargo expand -p schemalane-embed-tests --lib embedded 2>/dev/null | grep -c "mod v9__m"` → 1 each for `v9__m` and `v9__m_2` (skip if cargo-expand unavailable).

### Step 4: Helper unit tests in the macro crate

In `schemalane-macros/src/lib.rs` tests:

```rust
#[test]
fn sanitize_ident_edges() {
    assert_eq!(sanitize_ident("My-File.Name"), "my_file_name");
    assert_eq!(sanitize_ident("___"), "migration");        // trims to empty → fallback
    assert_eq!(sanitize_ident("9lives"), "m_9lives");      // leading digit
    assert_eq!(sanitize_ident(""), "migration");
}

#[test]
fn unique_module_ident_suffixes_collisions() {
    let mut used = std::collections::HashSet::new();
    let a = unique_module_ident("V9__m.rs", &mut used);
    let b = unique_module_ident("V9__m_.rs", &mut used);
    assert_eq!(a.to_string(), "v9__m");
    assert_eq!(b.to_string(), "v9__m_2");
}
```

**Verify**: `cargo test -p schemalane-macros --locked` → pass.

### Step 5: Full gate

fmt + clippy + `cargo test --workspace --locked` → green (the new member compiles under the workspace lint set — `[lints] workspace = true` in its manifest).

## Test plan

Steps 1–4: compile-level codegen contract (the crate building IS the test), construction smoke tests, collision corpus, helper unit tests.

## Done criteria

- [ ] `schemalane-embed-tests` exists, is a workspace member (non-default), `publish = false`, and `cargo test -p schemalane-embed-tests --locked` passes
- [ ] Collision fixtures present; macro helper unit tests pass
- [ ] Workspace gate green; `plans/README.md` updated

## STOP conditions

- The fixture crate FAILS to compile — that is the bug class this plan exists to catch, found on day one. Capture the exact rustc error; do not patch the macro here.
- Workspace lints reject the generated code (e.g. pedantic lint fires inside macro output) — real finding about generated-code lint hygiene; report (options: `#[allow]` in generated output — a macro change, out of scope).
- `cargo expand` unavailable and Step 3's module-name verification impossible — accept compile-success as the signal, note it.

## Maintenance notes

- This crate is the canary for macro changes: any `embed_migrations!` PR must keep it green — cheap insurance against downstream-only breakage.
- Deliberate gap: no live-DB E2E through the generated `runner()` (dependency cycle prevents placing it in core's integration tests). If a `schemalane-e2e` crate ever exists, move that there.
- The fixture migrations dir is a codegen corpus, NOT a valid runnable set (duplicate version 9 by design) — README in the crate says so.
