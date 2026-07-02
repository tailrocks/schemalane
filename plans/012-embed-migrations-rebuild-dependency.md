# Plan 012: Make new Rust migrations trigger a rebuild of `embed_migrations!` crates (and align `.RS` extension handling)

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat dd0d79d..HEAD -- schemalane-macros/src/lib.rs schemalane-core/src/lib.rs`
> If any in-scope file changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding; on a
> mismatch, treat it as a STOP condition.

## Status

- **Priority**: P2
- **Effort**: S-M
- **Risk**: MED
- **Depends on**: none (touches the same template block as plans 007/008 — land after them or resolve trivial conflicts)
- **Category**: bug / dx
- **Planned at**: commit `dd0d79d`, 2026-07-03

## Why this matters

`embed_migrations!("./migrations")` scans the directory **at macro expansion time**. rustc records the *generated* `#[path] mod` files in dep-info (so edits to already-embedded migrations rebuild fine), but a **newly added** `V5__foo.rs` is referenced by nothing — cargo sees no changed input and does not re-expand the macro. The new migration silently isn't embedded; at runtime, discovery sees the file on disk and fails with "missing Rust migration executor(s)" until an unrelated edit or `cargo clean` forces re-expansion. That is a "my migration didn't run and the error makes no sense" trap for every embedded-mode user. The standard fix for compile-time directory scanning is a `build.rs` emitting `cargo::rerun-if-changed=<dir>` — directory mtimes change on file add/remove, giving cargo the missing dependency edge.

Bonus inconsistency fixed here: the macro's discovery filter is **case-sensitive** (`extension != Some("rs")` skips `V1__x.RS`), while core's runtime discovery is case-insensitive (`eq_ignore_ascii_case("rs")`) and core's filename parser explicitly supports `.RS` (test `parses_rust_filename_like_flyway`, `schemalane-core/src/filename.rs:209-215`). A `.RS` file therefore compiles to nothing but is discovered at runtime → guaranteed "missing executor" failure.

## Current state

- `schemalane-macros/src/lib.rs`:
  - Directory scan at expansion (lines 107–148), extension filter at line 124:

    ```rust
    if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
        continue;
    }
    ```

  - Generated output (lines 79–98): `pub mod migrations { … #[path = …] mod …; pub const MIGRATIONS_DIR: &str = …; pub fn build_migrator(…) …; pub fn runner() … }`. Nothing establishes a dependency on the **directory**.

- Core runtime counterpart, `schemalane-core/src/lib.rs`, `discover_rust_migrations` (lines 752–756):

  ```rust
  .is_some_and(|ext| ext.eq_ignore_ascii_case("rs"))
  ```

- The `init` scaffold (`schemalane-core/src/lib.rs`, `init_template_files` lines 1631–1661) generates `Cargo.toml`, `README.md`, `.gitignore`, `src/main.rs`, `src/lib.rs`, two sample migrations — **no `build.rs`**.

- Proc-macro crates cannot themselves emit `rerun-if-changed` for downstream crates; the build-script approach must live in the **embedding** crate (the scaffold), plus documentation for hand-rolled embedders.

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Format check | `cargo fmt --all -- --check` | exit 0 |
| Lint | `cargo clippy --workspace --locked --all-targets --all-features -- -D warnings` | exit 0 |
| Unit tests | `cargo test --workspace --locked` | pass |
| Scaffold E2E (manual) | Step 4 script | see step |

## Scope

**In scope** (the only files you should modify):
- `schemalane-macros/src/lib.rs` (extension filter + rustdoc)
- `schemalane-core/src/lib.rs` (scaffold: add `build.rs` template + test assertion)

**Out of scope** (do NOT touch, even though they look related):
- The duplicated version-parsing logic in the macro (`plans/025-version-parser-dedup.md`).
- trybuild success-path coverage (`plans/024-macro-codegen-tests.md`).
- Embedding SQL file *contents* into the binary (a design change — see Maintenance notes).

## Git workflow

- Branch: `advisor/012-embed-migrations-rebuild-dependency`
- Suggested commit: `Track migrations dir in scaffold build.rs; align .RS handling`
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: Align the macro's extension filter with runtime discovery

In `schemalane-macros/src/lib.rs` line 124, change to case-insensitive:

```rust
if !path
    .extension()
    .and_then(|ext| ext.to_str())
    .is_some_and(|ext| ext.eq_ignore_ascii_case("rs"))
{
    continue;
}
```

(`parse_rust_migration_filename` at lines 174–213 already accepts `.RS` via `eq_ignore_ascii_case` — only the directory filter was inconsistent.)

Add a unit test beside the existing filename tests (lines 285–319):

```rust
#[test]
fn parses_uppercase_extension() {
    let version = parse_rust_migration_filename("V3__seed.RS")
        .expect("uppercase .RS must parse");
    assert_eq!(version, parsed_version(&["3"]));
}
```

**Verify**: `cargo test -p schemalane-macros --locked` → pass.

### Step 2: Document the rebuild requirement on the macro

Add rustdoc to `embed_migrations` (`#[proc_macro]` fn, line 9):

```rust
/// Embeds all Rust migrations from the given directory (relative to
/// `CARGO_MANIFEST_DIR`) and generates `migrations::{build_migrator, runner,
/// MIGRATIONS_DIR}`.
///
/// # Rebuild tracking
///
/// The directory is scanned at macro expansion time. Cargo does not know
/// about that scan, so **adding or removing** a migration file does not by
/// itself trigger recompilation (editing an already-embedded file does).
/// Add a `build.rs` to the embedding crate with:
///
/// ```text
/// fn main() {
///     println!("cargo::rerun-if-changed=migrations");
/// }
/// ```
///
/// Crates generated by `schemalane init` include this automatically.
pub fn embed_migrations(input: TokenStream) -> TokenStream {
```

**Verify**: `cargo doc -p schemalane-macros --no-deps` → exit 0.

### Step 3: Ship `build.rs` in the init scaffold

In `schemalane-core/src/lib.rs`:

1. Add a template const near the other `INIT_*` consts:

   ```rust
   const INIT_BUILD_RS_TEMPLATE: &str = r#"fn main() {
       // Re-run the build when migrations are added or removed so that
       // schemalane_core::embed_migrations! re-scans the directory.
       println!("cargo::rerun-if-changed=migrations");
   }
   "#;
   ```

2. Register it in `init_template_files` (lines 1631–1661):

   ```rust
   (PathBuf::from("build.rs"), INIT_BUILD_RS_TEMPLATE.to_owned()),
   ```

3. Extend `init_scaffold_creates_expected_files` (line 1968) with:

   ```rust
   assert!(target.join("build.rs").exists(), "build.rs should be created");
   let build_rs = fs::read_to_string(target.join("build.rs")).expect("read build.rs");
   assert!(build_rs.contains("cargo::rerun-if-changed=migrations"));
   ```

Note: `cargo::rerun-if-changed` (double-colon syntax) requires Rust ≥1.77 — fine for an edition-2024 scaffold.

**Verify**: `cargo test -p schemalane-core --locked init_scaffold` → pass.

### Step 4: End-to-end rebuild proof (manual)

This is the actual bug being fixed — prove it with a real embedded crate wired to the **local** workspace:

```sh
D=$(mktemp -d)
cargo run -p schemalane-cli -- init --path "$D/mig"
# point the scaffold at the local checkout (path deps), per the template's comment block:
#   edit $D/mig/Cargo.toml: schemalane-core = { path = "<REPO>/schemalane-core" }
#                           schemalane-cli  = { path = "<REPO>/schemalane-cli" }
cargo build --manifest-path "$D/mig/Cargo.toml"          # build 1: embeds V2__seed_cake_table.rs
cp "$D/mig/migrations/V2__seed_cake_table.rs" "$D/mig/migrations/V3__more.rs"
cargo build --manifest-path "$D/mig/Cargo.toml" 2>&1 | tee /tmp/rebuild.log
```

**Verify**: the second build is NOT a no-op (`/tmp/rebuild.log` shows the migration crate recompiling — a `Compiling <crate>` line, not just `Finished`). Without `build.rs` it prints only `Finished` (the bug); with it, the crate recompiles and `V3__more.rs` is embedded.

### Step 5: Full gate

**Verify**: fmt + clippy + `cargo test --workspace --locked` exit 0.

## Test plan

- `parses_uppercase_extension` (Step 1).
- Scaffold assertions for `build.rs` (Step 3).
- Manual rebuild proof (Step 4) — the regression demonstration.
- Full macro success-path compilation tests come in `plans/024-macro-codegen-tests.md`.

## Done criteria

- [ ] `grep -n "eq_ignore_ascii_case" schemalane-macros/src/lib.rs` → present in the directory filter
- [ ] `grep -n "rerun-if-changed" schemalane-core/src/lib.rs schemalane-macros/src/lib.rs` → template + rustdoc
- [ ] Step 4 second build recompiles the scaffold crate
- [ ] fmt/clippy/workspace tests exit 0; only in-scope files modified
- [ ] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- Step 4's second build is a no-op even WITH `build.rs` — the rerun-if-changed-on-directory assumption failed on this cargo version; report cargo version + log, do not escalate to per-file `include_bytes!` tricks without approval.
- The scaffold build in Step 4 fails for reasons unrelated to this plan (e.g. published-crate API drift) — note it; plan 008's STOP condition covers that territory.

## Maintenance notes

- Existing (pre-plan) generated crates lack `build.rs`; the macro rustdoc is their upgrade path. Release notes should tell embedded-mode users to add it.
- Deliberately NOT done: embedding SQL contents in the binary (refinery-style). Today SQL files are read from disk at runtime and `MIGRATIONS_DIR` is a compile-time **absolute** path baked into the binary — deploying a compiled migration binary to another machine requires the same path or an explicit `--dir`. That is a real deployment footgun but a design change (embed contents vs. read-at-runtime) — record as a follow-up direction decision, not a patch.
- Reviewer: confirm the doc-comment build.rs snippet uses the modern `cargo::` prefix.
