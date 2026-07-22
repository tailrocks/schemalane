# Schemalane repository guidance

## Scope and sources of truth

- This file applies to the whole repository. A nested `AGENTS.md` adds rules for
  its subtree; the nearest file owns local details.
- `SCHEMALANE_SPEC.md` is the behavioral contract. Keep it synchronized with
  command, migration, checksum, history, and exit-code behavior.
- `Cargo.toml` owns workspace membership and shared dependency requirements.
  `.github/workflows/` owns the executable CI and release gates.
- `CLAUDE.md` files are relative symlinks to sibling `AGENTS.md` files. Keep
  shared rules in `AGENTS.md`; do not maintain a second copy in `CLAUDE.md`.
- A session launched at the repository root may not preload descendant guides.
  Read the mapped local guide before changing that subtree.

## Repository map

| Path | Responsibility | Local guidance |
|---|---|---|
| `schemalane-core/` | Migration discovery, state, history, locking, execution | `schemalane-core/AGENTS.md` |
| `schemalane-cli/` | CLI grammar, rendering, TLS, migration-crate delegation | `schemalane-cli/AGENTS.md` |
| `pg_query_fmt/` | PostgreSQL AST formatter, preview, highlighting | `pg_query_fmt/AGENTS.md` |
| `schemalane-macros/` | `embed_migrations!` proc macro | `schemalane-macros/AGENTS.md` |
| `schemalane-version/` | Shared Flyway-compatible filename/version parser | `schemalane-version/AGENTS.md` |
| `schemalane-embed-tests/` | Unpublished compile-time fixture crate | `schemalane-embed-tests/AGENTS.md` |
| `.github/workflows/` | CI and manual crates.io release | `.github/AGENTS.md` |
| `plans/designs/` | Completed direction-spike decision records | — |

## Environment

- Rust 1.85 or newer; edition 2024 and resolver 3.
- A C toolchain and libclang are required because `pg_query` builds bundled C
  code and runs bindgen.
- Docker is required only for the ignored PostgreSQL integration suite.
- Commands that package crates or audit dependencies may access registries.

## Verification commands

Run the narrowest relevant check while iterating, then the applicable repo gate.

```sh
bash scripts/check-agent-instructions.sh
cargo fmt --all -- --check
cargo clippy --workspace --locked --all-targets --all-features -- -D warnings
cargo nextest run --workspace --locked --all-targets --all-features
cargo nextest run -p schemalane-core --locked --test postgres_integration --run-ignored all
RUSTDOCFLAGS="-D missing-docs" cargo doc -p schemalane-core --no-deps --locked
cargo package --workspace --locked --allow-dirty
cargo audit
```

- The ordinary workspace nextest command reports the Docker tests as ignored. Do
  not claim database behavior is verified unless the `--include-ignored` suite
  passed.
- The instruction check validates cross-tool symlinks and the 200-line
  maintenance budget before other gates run.
- `cargo package --workspace` is intentional: Cargo stages workspace archives
  in dependency order, allowing unpublished sibling crates to verify together.
- `cargo audit` is the CI security gate. Local runs require `cargo-audit`;
  install it with `cargo install cargo-audit --locked` when needed.

## Cross-cutting contracts

- Schemalane is PostgreSQL-only and forward-only. Do not add undo, repeatable,
  baseline, MySQL, or SQLite behavior without an approved design and spec update.
- Preserve Flyway-compatible filename ordering, semantic duplicate detection,
  history shape, and checksum rules. Compatibility changes require tests and
  documentation.
- Quote SQL identifiers through the shared identifier helpers. Bind values as
  parameters; never interpolate values into SQL.
- Never programmatically forward database URLs or credentials in child-process
  argv, or print them in logs and diagnostics. User-supplied `--database-url`
  remains supported; delegation passes it through `DATABASE_URL`.
- Never commit real passwords, tokens, database URLs, or environment contents
  in fixtures, instruction files, or documentation.
- Public Rust API changes require rustdoc, tests, and a semver decision. Keep
  extensible public enums and structs `#[non_exhaustive]` where applicable.
- Update README help/examples and `SCHEMALANE_SPEC.md` when the shipped CLI or
  programmatic behavior changes.

## Change workflow

1. Read the nearest `AGENTS.md`, relevant source, tests, and contract sections.
2. Find the enabling cause of a bug class; prefer a structural fix over a
   call-site symptom patch.
3. Add or update regression tests for observable behavior.
4. Run focused checks, then every applicable gate above.
5. Review `git diff --check`, the final diff, and generated lockfile changes.

Do not overwrite unrelated user changes or regenerate `Cargo.lock` without
reviewing the resulting dependency/version changes.

## Git and review

- Keep commits focused, with short imperative subjects.
- Every commit must use DCO signoff (`git commit -s`) and include
  `Co-authored-by: Codex <codex@openai.com>` for Codex-authored work.
- Do not rewrite or force-push history unless the user explicitly authorizes it.
- A change is done only when behavior, tests, docs, package metadata, and the
  relevant CI-equivalent commands agree.

## Maintaining these instructions

- Keep root guidance universal and concise. Move subtree facts to the nearest
  `AGENTS.md`; move repeatable task procedures to skills or scripts.
- Add a rule after repeated, evidenced friction—not for temporary task state.
- Remove stale or conflicting rules when commands or architecture change.
- Instruction prose guides agents; CI, permissions, hooks, and branch protection
  enforce guarantees.
