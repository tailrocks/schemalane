# GitHub workflow guidance

## CI

- Keep third-party actions pinned to full commit SHAs with readable tag comments.
- CI must run format, clippy with `-D warnings`, workspace tests, the ignored
  PostgreSQL integration suite, workspace packaging, dependency audit, and the
  agent-instruction consistency check.
- Verify publishability with `cargo package --workspace --locked --allow-dirty`.
  Per-crate dry runs cannot resolve unpublished workspace dependencies.
- Use least-privilege workflow permissions. Do not expose secrets to pull-request
  jobs or untrusted scripts.

## Release

- Publishing is operator-controlled. Do not dispatch the release workflow or
  publish crates without explicit authorization.
- The crates.io token stays scoped to the publish step; the credentialed release
  job must not restore a shared build cache.
- Publish in dependency order: `schemalane-version`, `schemalane-macros`,
  `pg_query_fmt`, `schemalane-core`, then `schemalane-cli`.
- Publishing is idempotent: skip an already-published version and wait until the
  sparse index serves each dependency before publishing its consumers.
- Version changes must update package manifests, workspace dependency
  requirements, and `Cargo.lock` together.
