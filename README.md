# Schemalane

PostgreSQL-first, forward-only migrations with SQL as default and optional Rust migrations.

Repository layout:

- Cargo workspace root
- library crate: `schemalane-core`
- CLI crate: `schemalane-cli`
- proc-macro crate: `schemalane-macros`

## Commands

Schemalane CLI supports:

- `schemalane init`
- `schemalane migrate up`
- `schemalane migrate status`
- `schemalane migrate validate`
- `schemalane migrate fresh`

## Testing

Fast unit tests (no Docker):

```sh
cargo nextest run --workspace --locked
```

Full suite including PostgreSQL integration tests (requires a running Docker daemon;
testcontainers starts a disposable Postgres per test):

```sh
cargo nextest run -p schemalane-core --locked --test postgres_integration --run-ignored all
```

## Local CLI Testing

Install `schemalane` locally and test it as a standalone command:

```sh
# from workspace root
cargo install --path schemalane-cli --force

# confirm binary is available
schemalane --help
```

Validate the full flow:

```sh
# scaffold migration crate
schemalane init --path ./migration

# The generated Cargo.toml fetches Schemalane directly from GitHub.
# For local development, use its commented path-dependency alternatives.

# run migration binary directly
DATABASE_URL="$DATABASE_URL" cargo run --manifest-path ./migration/Cargo.toml -- up

# inspect status
DATABASE_URL="$DATABASE_URL" cargo run --manifest-path ./migration/Cargo.toml -- status

# run through installed schemalane CLI (defaults to ./migration + implicit `up`)
DATABASE_URL="$DATABASE_URL" schemalane migrate
```

## Bootstrap A Migration Crate

Generate a migration crate:

```sh
cargo run -p schemalane-cli -- init --path ./migration
```

This creates:

- `migration/Cargo.toml`
- `migration/src/main.rs`
- `migration/src/lib.rs`
- `migration/migrations/V1__create_cake_table.sql`
- `migration/migrations/V2__seed_cake_table.rs`

Run it from your parent project:

```sh
DATABASE_URL="$DATABASE_URL" cargo run --manifest-path ./migration/Cargo.toml -- up
```

## Direct CLI Usage

```sh
DATABASE_URL="$DATABASE_URL" cargo run -p schemalane-cli -- migrate up
```

Use a migration crate path:

```sh
cargo run -p schemalane-cli -- migrate -d ./migration up
```

```sh
DATABASE_URL="$DATABASE_URL" cargo run -p schemalane-cli -- migrate status
```

Validate local migrations against database history without applying anything:

```sh
DATABASE_URL="$DATABASE_URL" cargo run -p schemalane-cli -- migrate validate
```

`validate` rejects failed history with exit code 4 and missing or checksum-mismatched
history with exit code 3. Pending migrations are valid unless
`--fail-on-pending` is set. `--format table|json` controls output; JSON wraps the
status report with `"validation": { "valid": ... }`.

Preview the exact pending `up` plan without applying it:

```sh
DATABASE_URL="$DATABASE_URL" cargo run -p schemalane-cli -- migrate up --dry-run
```

Dry-run performs the same discovery, history, drift, SQL parsing, and transaction-mode
gates as `up`, then prints pending SQL and transaction modes. Rust source is not
previewable. `--format json` emits the structured plan. Dry-run does not acquire the
advisory lock, so its result can become stale if another runner migrates concurrently.

```sh
DATABASE_URL="$DATABASE_URL" cargo run -p schemalane-cli -- migrate fresh --confirm yes
```

## PostgreSQL TLS

Schemalane reads `sslmode` from the PostgreSQL URL:

- `sslmode=disable` uses plaintext.
- `sslmode=prefer` uses TLS when the server offers it, otherwise falls back to plaintext.
- `sslmode=require` requires TLS.

TLS server certificates are verified against the operating system trust store for both
`prefer` and `require`; a server offering TLS with an untrusted certificate is rejected.
Current URL parsing does not support `verify-ca` or `verify-full`. Custom CA files and
client certificates are also not supported yet. Channel binding accepts
`channel_binding=disable|prefer|require`; the rustls connector supplies the
`tls-server-end-point` binding when TLS is active and the certificate supports it.

## Notes

- SQL files: `V<version>__<description>.sql`
- Rust files: `V<version>__<description>.rs`
- Version and description parsing follows Flyway's default versioned migration
  filename approach: underscores in versions are normalized to dots, and the
  raw description is everything after the first `__` separator. As in Flyway,
  the separator and description may be omitted when the description is empty.
- SQL runs in a transaction by default.
- Rust migration transaction mode is controlled by executor registration.
- `src/lib.rs` uses `embed_migrations!("./migrations")` to auto-register Rust migration files by script name.
- generated `src/main.rs` is minimal and uses shared CLI via `embedded::migrations::runner().run().await` (backed by `schemalane-cli`).
