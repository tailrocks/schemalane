# Schemalane

PostgreSQL-first, forward-only migrations with SQL as default and optional Rust migrations.

Repository layout:

- Cargo workspace root
- library crate: `schemalane-core`
- CLI crate: `schemalane-cli`
- proc-macro crate: `schemalane-macros`

## Commands

Schemalane CLI supports:

- `schemalane migrate init`
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
cargo install --path backend-rust/schemalane/schemalane-cli --force

# confirm binary is available
schemalane --help
```

Validate the full flow:

```sh
# start local registry via compose
./docker-up-kellnr.sh

# scaffold migration crate
schemalane migrate init --path ./migration

# generated Cargo.toml uses registry = "kellnr" for schemalane-core/cli.
# ensure ~/.cargo/config.toml contains:
# [registries.kellnr]
# index = "sparse+http://localhost:8000/api/v1/crates/"
#
# if you do not want to publish schemalane crates yet, replace generated
# schemalane-core/schemalane-cli dependencies with local path dependencies.

# run migration binary directly
cargo run --manifest-path ./migration/Cargo.toml -- --database-url "$DATABASE_URL" up

# inspect status
cargo run --manifest-path ./migration/Cargo.toml -- --database-url "$DATABASE_URL" status

# run through installed schemalane CLI (defaults to ./migration + implicit `up`)
DATABASE_URL="$DATABASE_URL" schemalane migrate
```

## Bootstrap A Migration Crate

Generate a migration crate:

```sh
cargo run -p schemalane-cli -- migrate init --path ./migration
```

This creates:

- `migration/Cargo.toml`
- `migration/src/main.rs`
- `migration/src/lib.rs`
- `migration/migrations/V1__create_cake_table.sql`
- `migration/migrations/V2__seed_cake_table.rs`

Run it from your parent project:

```sh
cargo run --manifest-path ./migration/Cargo.toml -- --database-url "$DATABASE_URL" up
```

## Direct CLI Usage

```sh
cargo run -p schemalane-cli -- migrate --database-url "$DATABASE_URL" up
```

Use a migration crate path:

```sh
cargo run -p schemalane-cli -- migrate -d ./migration up
```

```sh
cargo run -p schemalane-cli -- migrate --database-url "$DATABASE_URL" status
```

```sh
cargo run -p schemalane-cli -- migrate --database-url "$DATABASE_URL" fresh --yes
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
