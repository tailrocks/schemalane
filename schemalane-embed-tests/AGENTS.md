# schemalane-embed-tests guidance

## Fixture role

- This crate is an unpublished, non-default workspace fixture. Do not add
  production behavior or publish it.
- Its migration corpus exercises proc-macro generated code, uppercase suffixes,
  non-identifier filename characters, large/dotted versions, and module-name
  collisions.
- When macro output changes, extend this corpus instead of duplicating generated
  token assertions in the proc-macro crate.

## Check

```sh
cargo nextest run -p schemalane-embed-tests --locked
```
