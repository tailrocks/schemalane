# Plan 008: Make `init`-generated crates build from GitHub

## Status

- **Priority**: P1
- **Effort**: S
- **Risk**: LOW
- **Depends on**: none
- **Category**: bug (product)
- **Status**: DONE

## Why this matters

`schemalane init` must generate a runnable migration crate for users outside
this workspace. Schemalane is source-only while it remains work in progress,
so generated projects fetch `schemalane-core` and `schemalane-cli` from the
public GitHub repository. A future release may replace these Git dependencies
with crates.io versions.

## Requirements

The generated `Cargo.toml` must:

- set `publish = false`;
- use `https://github.com/tailrocks/schemalane.git` for both Schemalane crates;
- retain commented path-dependency alternatives for local development;
- use supported major-version constraints for Tokio dependencies;
- use GitHub as the only remote source for Schemalane dependencies.

The scaffold test must lock both Git dependency declarations. The generated
crate must resolve and build when its selected Git revision contains the
required Schemalane API.

## Verification

```sh
cargo test -p schemalane-core --locked init_scaffold
cargo fmt --all -- --check
cargo clippy --workspace --locked --all-targets --all-features -- -D warnings
cargo test --workspace --locked --all-targets --all-features
```

For an external-resolution check, generate a scaffold after the implementing
branch is available at its referenced Git revision, then run:

```sh
cargo metadata --manifest-path /tmp/sl-scaffold-check/Cargo.toml --format-version 1
cargo build --manifest-path /tmp/sl-scaffold-check/Cargo.toml
```

## Done criteria

- [x] Generated Schemalane dependencies reference GitHub.
- [x] Local path alternatives remain documented in the generated manifest.
- [x] Scaffold unit test locks the GitHub URL.
- [x] Workspace format, lint, and test gates pass.

## Maintenance notes

Pin generated Git dependencies to a release tag or commit when Schemalane has
a stable release policy. Replace them with crates.io versions only after public
publication is explicitly authorized.
