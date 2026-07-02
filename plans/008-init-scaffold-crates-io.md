# Plan 008: Make `init`-generated crates build out of the box (drop the hardcoded `kellnr` private registry)

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat dd0d79d..HEAD -- schemalane-core/src/lib.rs`
> If any in-scope file changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding; on a
> mismatch, treat it as a STOP condition.

## Status

- **Priority**: P1
- **Effort**: S
- **Risk**: LOW
- **Depends on**: none
- **Category**: bug (product)
- **Planned at**: commit `dd0d79d`, 2026-07-03

## Why this matters

`schemalane init` exists to produce a **runnable** migration crate. The generated `Cargo.toml` pins its two schemalane dependencies to `registry = "kellnr"` — a private local registry (`sparse+http://localhost:8000/...`) that no external user has. The scaffold therefore fails `cargo build` for everyone except the original monorepo environment, and a unit test locks this broken default in. The crates are published on crates.io (schemalane-core 0.1.20, schemalane-cli 0.1.26), so the correct default is simply no `registry` key. This is the single highest-friction first-touch bug for new users.

## Current state

- `schemalane-core/src/lib.rs`, `INIT_CARGO_TOML_TEMPLATE` (lines 1701–1719):

  ```rust
  const INIT_CARGO_TOML_TEMPLATE: &str = r#"[package]
  name = "__PACKAGE_NAME__"
  version = "0.1.0"
  edition = "2024"
  publish = false

  [dependencies]
  schemalane-core = { version = "0.1", registry = "kellnr" }
  schemalane-cli = { version = "0.1", registry = "kellnr" }
  tokio = { version = "1.48.0", features = ["macros", "rt-multi-thread"] }
  "#;
  ```

  followed by comment lines instructing users to configure `[registries.kellnr]` in `~/.cargo/config.toml` (lines 1712–1718).

- The locking test, `init_scaffold_creates_expected_files` (lines 1995–2004):

  ```rust
  assert!(
      cargo_toml.contains("schemalane-core = { version = \"0.1\", registry = \"kellnr\" }"),
      "scaffold should default schemalane-core dependency to kellnr registry"
  );
  assert!(
      cargo_toml.contains("schemalane-cli = { version = \"0.1\", registry = \"kellnr\" }"),
      ...
  ```

- Template placement/convention: templates are `const &str` blocks near the bottom of core's lib.rs; `init_template_files` (lines 1631–1661) assembles them.

- tokio in the template is pinned `1.48.0` while the workspace itself uses newer tokio — a stale exact-ish pin in generated code.

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Format check | `cargo fmt --all -- --check` | exit 0 |
| Lint | `cargo clippy --workspace --locked --all-targets --all-features -- -D warnings` | exit 0 |
| Unit tests | `cargo test -p schemalane-core --locked` | pass |
| Scaffold resolution check (network) | `cargo metadata --manifest-path <generated>/Cargo.toml --format-version 1 > /dev/null` | exit 0 |

## Scope

**In scope** (the only files you should modify):
- `schemalane-core/src/lib.rs` (template consts + the locking test)

**Out of scope** (do NOT touch, even though they look related):
- `README.md` kellnr instructions — `plans/009-docs-command-surface-truth.md`.
- Version-pinning automation between CLI version and template (`env!("CARGO_PKG_VERSION")` threading) — see Maintenance notes; deliberately deferred.
- The `--database-url` examples in the README template — `plans/007-database-url-env-delegation.md` owns those lines; if it landed first, leave its text intact.

## Git workflow

- Branch: `advisor/008-init-scaffold-crates-io`
- Suggested commit: `Default init scaffold to crates.io dependencies`
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: Rewrite the Cargo.toml template

Replace `INIT_CARGO_TOML_TEMPLATE` with:

```rust
const INIT_CARGO_TOML_TEMPLATE: &str = r#"[package]
name = "__PACKAGE_NAME__"
version = "0.1.0"
edition = "2024"
publish = false

[dependencies]
schemalane-core = "0.1"
schemalane-cli = "0.1"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }

# Developing against a local schemalane checkout? Use path dependencies:
# schemalane-core = { path = "../schemalane-core" }
# schemalane-cli = { path = "../schemalane-cli" }
"#;
```

(Deletes the kellnr registry keys and the `[registries.kellnr]` instruction block; keeps the path-dependency escape hatch as comments; un-pins tokio to the major.)

**Verify**: `cargo clippy -p schemalane-core --locked --all-targets -- -D warnings` → exit 0.

### Step 2: Update the locking test to assert the new default

In `init_scaffold_creates_expected_files`, replace the two kellnr assertions with:

```rust
assert!(
    cargo_toml.contains("schemalane-core = \"0.1\""),
    "scaffold should depend on schemalane-core from crates.io"
);
assert!(
    cargo_toml.contains("schemalane-cli = \"0.1\""),
    "scaffold should depend on schemalane-cli from crates.io"
);
assert!(
    !cargo_toml.contains("kellnr"),
    "scaffold must not reference a private registry"
);
```

**Verify**: `cargo test -p schemalane-core --locked init_scaffold` → pass.

### Step 3: Prove the generated crate resolves against crates.io

```sh
cargo run -p schemalane-cli -- init --path /tmp/sl-scaffold-check
cargo metadata --manifest-path /tmp/sl-scaffold-check/Cargo.toml --format-version 1 > /dev/null && echo RESOLVES
```

**Verify**: prints `RESOLVES`. This confirms the published crates satisfy `"0.1"` from a clean environment. (Requires network; see STOP conditions.)

Note: full `cargo build` of the scaffold would also verify the published crates' API matches what the generated `src/main.rs`/`lib.rs` use (`embed_migrations!`, `runner()`); run it if time permits:
`cargo build --manifest-path /tmp/sl-scaffold-check/Cargo.toml` → exit 0.

### Step 4: Full gate

**Verify**: fmt + clippy + `cargo test --workspace --locked` exit 0.

## Test plan

- Updated `init_scaffold_creates_expected_files` (Step 2) — locks the crates.io default and forbids `kellnr` regressions.
- Manual resolution proof (Step 3).

## Done criteria

- [ ] `grep -rn "kellnr" schemalane-core/src/` → no matches
- [ ] `cargo test -p schemalane-core --locked` → pass
- [ ] Step 3 printed `RESOLVES` (or STOP-noted if offline)
- [ ] fmt/clippy exit 0; only `schemalane-core/src/lib.rs` modified
- [ ] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- Step 3's `cargo build` (the optional deep check) fails because the **published** schemalane-cli 0.1.x lacks an API the scaffold uses (e.g. `EmbeddedRunner`/`runner()` signature drift between the local workspace and crates.io) — that means a release must be published before this scaffold works for external users; report it, the template change is still correct.
- No network access — complete Steps 1–2 and 4, note Step 3 as unverified.

## Maintenance notes

- **Deferred (deliberate)**: pinning the generated deps to the exact versions the running CLI was built with (thread `env!("CARGO_PKG_VERSION")` from schemalane-cli into `init_template_files`). Trade-off: exact pins go stale as crates release; the loose `"0.1"` tracks patch/minor updates. Revisit at 1.0.
- The kellnr flow was a monorepo-era development convenience; if local-registry testing is still wanted, it belongs in CONTRIBUTING docs (`plans/017-claude-md-contributor-docs.md`), not in the user-facing default.
- Reviewers: watch that the path-dependency comment block survives — it is the documented dev-mode escape hatch.
