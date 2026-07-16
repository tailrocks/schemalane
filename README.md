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
- `schemalane migrate fresh`

## Testing

Fast unit tests (no Docker):

```sh
cargo test --workspace
```

Full suite including PostgreSQL integration tests (requires a running Docker daemon;
testcontainers starts a disposable Postgres per test):

```sh
cargo test -p schemalane-core --test postgres_integration -- --include-ignored
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

```sh
DATABASE_URL="$DATABASE_URL" cargo run -p schemalane-cli -- migrate fresh --confirm yes
```

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
