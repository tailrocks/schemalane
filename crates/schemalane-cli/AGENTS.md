# schemalane-cli guidance

## External contracts

- Stdout is payload only: status tables, requested JSON, dry-run SQL, and init
  output. Branding, progress, prompts, and diagnostics belong on stderr.
- JSON mode must remain parseable and free of banners, ANSI escapes, and other
  human chrome. Gate color by destination stream.
- Sanitize all file-, SQL-, database-, and error-derived terminal text before
  rendering it. Preserve useful error source chains.
- Never place `DATABASE_URL` or credentials in delegated argv. Pass the URL in
  the child environment and preserve the child's exit code verbatim.

## Database and command behavior

- `sslmode=disable` uses plaintext; `prefer` and `require` use rustls with the
  platform verifier. Keep mode selection and certificate failures tested.
- Keep standalone and embedded CLI grammar aligned. Shared database flags belong
  in `CommonDbArgs`; delegation must forward every applicable flag.
- `up` and `fresh` render the authoritative observer plan instead of issuing a
  second discovery/status pass.
- Help text, README examples, and `SCHEMALANE_SPEC.md` must describe the live
  command surface exactly.

## Checks

```sh
cargo nextest run -p schemalane-cli --locked
cargo run -p schemalane-cli -- migrate --help
cargo run -p schemalane-cli -- migrate up --help
cargo run -p schemalane-cli -- migrate validate --help
```

When changing output, add contract tests for JSON keys, exit codes, delegation,
environment precedence, sanitization, and help grammar as appropriate.
