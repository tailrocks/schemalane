# pg_query_fmt guidance

## Formatter contracts

- Formatting must preserve PostgreSQL meaning, not merely produce attractive
  text. Drive output from the `pg_query` AST; do not silently drop clauses.
- Preserve identifier case, reserved words, expression precedence, explicit
  parentheses, array slices, operator classes, collations, locks, and dollar
  quote delimiters.
- New formatter coverage needs a fingerprint/round-trip case when `pg_query`
  supports the statement, plus an exact-output regression test where layout is
  part of the contract.
- Keep shared identifier-keyword logic and table-body width calculation
  centralized. Do not fork them across statement modules.

## Module placement

- DDL emitters live in `src/stmt/ddl.rs`; DML emitters in `src/stmt/dml.rs`;
  shared column/table-body layout in `src/stmt/table_body.rs`.
- `highlight` and `preview` are public consumers. Changes must preserve terminal
  control-character filtering and honest fallback behavior.

## Checks

```sh
cargo test -p pg_query_fmt --locked
cargo doc -p pg_query_fmt --no-deps --locked
```
