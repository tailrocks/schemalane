# CLAUDE.md — working on schemalane

## What this is

Schemalane is a PostgreSQL-only, forward-only migration toolkit with a
Flyway-compatible history table. `SCHEMALANE_SPEC.md` is the behavioral
contract: filename rules are in §3, checksums in §6.3, and exit codes in §8.

## Crate map

- `schemalane-core` — discovery, validation, checksums, history, locking, and
  the `up`, `status`, and `fresh` engine.
- `schemalane-cli` — `schemalane` binary, output rendering, TLS setup, and
  delegation to migration crates through `cargo run`.
- `schemalane-macros` — compile-time `embed_migrations!` proc macro.
- `schemalane-version` — shared Flyway-compatible version and migration
  filename parser used by core, CLI, and macros.
- `schemalane-embed-tests` — unpublished compile-time codegen corpus for
  `embed_migrations!`.
- `pg_query_fmt` — standalone SQL formatter/highlighter over the `pg_query`
  PostgreSQL AST.

## Build prerequisites

- Rust 1.85 or newer (edition 2024, resolver 3).
- A C toolchain and libclang. `pg_query` compiles bundled libpg_query C code
  and runs bindgen. On macOS: `xcode-select --install`. On Debian/Ubuntu:
  `apt install build-essential libclang-dev`.
- Docker for PostgreSQL integration tests. Unit tests do not need Docker.
- The first build is slow because it compiles the C parser.

## Commands

| Task | Command |
|---|---|
| Fast tests, no Docker | `cargo test --workspace --locked` |
| PostgreSQL integration tests | `cargo test -p schemalane-core --locked --test postgres_integration -- --include-ignored` |
| CI lint gate | `cargo clippy --workspace --locked --all-targets --all-features -- -D warnings` |
| Format | `cargo fmt --all` |
| Format check | `cargo fmt --all -- --check` |
| Run CLI help | `cargo run -p schemalane-cli -- migrate --help` |

## Conventions

- Pedantic Clippy runs with `-D warnings`. Unsafe code is forbidden. Do not
  use `dbg!` or `unimplemented!`.
- CLI payload goes to stdout. Branding, progress, prompts, and diagnostics go
  to stderr. Gate ANSI styling by destination stream and sanitize file-derived
  terminal text.
- SQL identifiers go through `quote_ident` or `qualified_table`; values use
  bound parameters.
- Database tests use one disposable testcontainers PostgreSQL instance and one
  `TempDir` per test. Mark them `#[ignore = "requires Docker daemon"]`.
- Rust migration directory scans require `build.rs` with
  `cargo::rerun-if-changed=migrations`.
- Commit subjects are short and imperative, optionally prefixed (`ci:`,
  `docs:`, `fix:`). Use DCO signoff and the Codex co-author trailer.

## Releasing

The manual GitHub Actions Release workflow publishes `schemalane-version`,
macros, `pg_query_fmt`, core, then CLI. Versions are managed independently in
crate manifests. The workflow skips versions already on crates.io and waits for
sparse-index propagation before publishing dependents. CI verifies the complete
interdependent source-only workspace with
`cargo package --workspace --locked --allow-dirty`.

## Plans directory

`plans/designs/` contains completed direction-spike decision records. No active
numbered implementation plans remain. Future work requires a new authorized
plan or specification.
