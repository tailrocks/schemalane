# schemalane-version guidance

## Compatibility surface

- This dependency-free crate is the only implementation of Flyway-compatible
  version and migration-filename parsing in the workspace.
- Numeric version parts are arbitrary precision. Compare normalized digit
  strings; never parse parts into fixed-width integers.
- Preserve semantic equality (`V1`, `V01`, and `V1.0`) and ordering across SQL,
  Rust, core, macro, and CLI consumers.
- Keep parser error wording useful because downstream crates surface it to users
  and compile diagnostics.

## Checks

```sh
cargo nextest run -p schemalane-version --locked
cargo package -p schemalane-version --locked --allow-dirty
```
