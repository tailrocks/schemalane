# pg_query_fmt

`pg_query_fmt` is a display-oriented PostgreSQL SQL formatter built on the
real PostgreSQL parser exposed by [`pg_query`](https://crates.io/crates/pg_query).
It is designed for migration logs, diagnostics, previews, and terminal output.

```rust
let formatted = pg_query_fmt::format_statement(
    "SELECT id, name FROM users WHERE active = true ORDER BY id",
)?;

assert_eq!(
    formatted,
    "SELECT id, name\nFROM users\nWHERE active = TRUE\nORDER BY id"
);
# Ok::<(), pg_query_fmt::FormatError>(())
```

Use `format_statement` for one statement or `format_sql` for a SQL string that
may contain multiple statements. Both parse input through PostgreSQL before
formatting.

The public supporting modules provide:

- `highlight` — ANSI terminal highlighting, including control-character
  filtering for untrusted migration text.
- `preview` — compact labels such as `CREATE TABLE public.wallets`, plus
  `preview_sql` for callers holding raw SQL.

## Display-only contract

Formatted SQL is intended for people, not execution or code generation.
Schemalane always executes the original SQL text. The test suite checks a broad
semantic round-trip corpus, but that invariant is a display-honesty guard—not a
promise that every PostgreSQL construct is reproduced byte-for-byte or is safe
to substitute for the source.

## License

Licensed under the MIT License. See the repository's `LICENSE` file.
