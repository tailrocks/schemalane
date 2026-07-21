# schemalane-macros guidance

## Proc-macro contracts

- This crate may depend on `schemalane-version`; it must not depend on
  `schemalane-core`, which depends on this proc macro.
- Use the shared Flyway parser. Do not reintroduce local version or filename
  parsing.
- `embed_migrations!` accepts case-insensitive `.rs` suffixes, generates unique
  module identifiers for filename collisions, and emits rebuild tracking for
  the migrations directory through the generated scaffold.
- Compile errors must identify the offending migration/path without panicking.

## Checks

```sh
cargo nextest run -p schemalane-macros --locked
cargo nextest run -p schemalane-embed-tests --locked
```

Use unit tests for helpers, trybuild for diagnostic failures, and the embed-test
fixture crate for generated-code success paths.
