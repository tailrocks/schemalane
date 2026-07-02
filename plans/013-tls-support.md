# Plan 013: Support TLS connections to PostgreSQL (currently hardcoded `NoTls`)

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat dd0d79d..HEAD -- schemalane-cli/src/lib.rs schemalane-cli/Cargo.toml`
> On mismatch with "Current state" excerpts, STOP.

## Status

- **Priority**: P1
- **Effort**: M
- **Risk**: MED
- **Depends on**: none
- **Category**: security
- **Planned at**: commit `dd0d79d`, 2026-07-03

## Why this matters

The CLI builds its connection pool with `tokio_postgres::NoTls` and no TLS connector exists anywhere in the runtime crates. Consequences: (a) with the default `sslmode=prefer`, connections to remote databases silently proceed in **cleartext** — the password (in cleartext-auth setups) and all migration SQL/data are exposed to network observers; (b) with `sslmode=require` — which managed PostgreSQL (RDS, Supabase, Neon, Azure) commonly enforces — the tool **cannot connect at all**. For a database migration tool in 2026 this is both a security defect and the largest single adoption blocker.

## Current state

- `schemalane-cli/src/lib.rs:21`: `use tokio_postgres::NoTls;`
- `create_pool` (lines 722–738):

  ```rust
  fn create_pool(database_url: &str) -> Result<Pool, SchemalaneError> {
      let pg_config: tokio_postgres::Config = database_url.parse().map_err(|err| {
          SchemalaneError::Validation(format!("failed to parse database URL: {err}"))
      })?;

      let mgr = deadpool_postgres::Manager::from_config(
          pg_config,
          NoTls,
          ManagerConfig { recycling_method: RecyclingMethod::Fast },
      );

      Pool::builder(mgr).max_size(5).build().map_err(|err| {
          SchemalaneError::Validation(format!("failed to build connection pool: {err}"))
      })
  }
  ```

- `schemalane-cli/Cargo.toml` deps (lines 21–29): `deadpool-postgres 0.14.1`, `tokio-postgres 0.7.13` — no TLS crates.
- `tokio_postgres::Config::get_ssl_mode()` exposes the parsed `sslmode` (`Disable`/`Prefer`/`Require`; tokio-postgres 0.7 does not implement `verify-ca`/`verify-full`).
- `schemalane-core` takes a caller-provided `&Pool` and is TLS-agnostic — no core changes needed.
- The engine never needs client certs; server-cert trust via the OS store is the target behavior.

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Add deps | `cargo add -p schemalane-cli tokio-postgres-rustls rustls rustls-platform-verifier` | Cargo.toml updated, resolves |
| Gate | fmt + `cargo clippy --workspace --locked --all-targets --all-features -- -D warnings` + `cargo test --workspace --locked` | exit 0 |
| Manual TLS check | Step 4 | connects |

## Scope

**In scope**:
- `schemalane-cli/Cargo.toml`
- `schemalane-cli/src/lib.rs` (`create_pool` and a small connector-selection helper + tests)

**Out of scope**:
- `schemalane-core` (pool-agnostic already), integration-test harness TLS (testcontainers Postgres is plaintext localhost — fine).
- Client-certificate auth, `verify-ca`/`verify-full` modes (tokio-postgres 0.7 doesn't parse them; document).
- `channel_binding` handling.

## Git workflow

- Branch: `advisor/013-tls-support`
- Suggested commit: `Support TLS to PostgreSQL via rustls (sslmode-driven)`
- No push/PR without operator instruction.

## Steps

### Step 1: Add dependencies

```sh
cargo add -p schemalane-cli tokio-postgres-rustls rustls rustls-platform-verifier
```

`rustls-platform-verifier` uses the OS trust store (macOS Keychain, Windows store, Linux ca-certificates) — the right default for a CLI talking to managed DBs. If `cargo add` picks incompatible majors (tokio-postgres-rustls must match tokio-postgres 0.7 / rustls 0.23-line), pin per its README.

**Verify**: `cargo check -p schemalane-cli --locked` (regenerate lock with plain `cargo check -p schemalane-cli` first since deps changed) → exit 0.

### Step 2: Build a mode-driven connector

In `schemalane-cli/src/lib.rs`, replace `create_pool` internals. Because `Manager::from_config` is generic over the TLS connector type, the clean pattern is two construction paths sharing the builder tail:

```rust
fn create_pool(database_url: &str) -> Result<Pool, SchemalaneError> {
    let pg_config: tokio_postgres::Config = database_url.parse().map_err(|err| {
        SchemalaneError::Validation(format!("failed to parse database URL: {err}"))
    })?;

    let manager_config = ManagerConfig { recycling_method: RecyclingMethod::Fast };

    // sslmode=disable → NoTls; prefer/require → rustls with OS trust roots.
    // tokio-postgres itself enforces `require` vs downgrades for `prefer`.
    let mgr = match pg_config.get_ssl_mode() {
        tokio_postgres::config::SslMode::Disable => {
            deadpool_postgres::Manager::from_config(pg_config, NoTls, manager_config)
        }
        _ => {
            let tls_config = rustls::ClientConfig::builder()
                .dangerous() // platform-verifier API shape; see crate docs
                .with_custom_certificate_verifier(std::sync::Arc::new(
                    rustls_platform_verifier::Verifier::new(),
                ))
                .with_no_client_auth();
            let tls = tokio_postgres_rustls::MakeRustlsConnect::new(tls_config);
            deadpool_postgres::Manager::from_config(pg_config, tls, manager_config)
        }
    };

    Pool::builder(mgr).max_size(5).build().map_err(|err| {
        SchemalaneError::Validation(format!("failed to build connection pool: {err}"))
    })
}
```

Type note: both arms produce `deadpool_postgres::Manager` — deadpool erases the connector type internally (`Manager` is not generic in 0.14; `from_config` takes `impl MakeTlsConnect<Socket> + …`). Verify against the installed deadpool version; if `Manager` IS generic in the resolved version, unify by boxing or by constructing the `Pool` inside each arm and returning it (duplicate 4 lines rather than fighting generics).

API-shape note: `rustls-platform-verifier`'s exact builder incantation varies by version (`Verifier::new()` may take a provider arg; there is also a convenience `tls_config()` helper — prefer `rustls_platform_verifier::tls_config()` if present). Follow the crate's README for the resolved version; the goal is: OS-trust-store verification, no client auth.

**Verify**: `cargo clippy -p schemalane-cli --all-targets -- -D warnings` → exit 0.

### Step 3: Unit-test the mode selection

Extract the decision into a testable helper `fn wants_tls(config: &tokio_postgres::Config) -> bool` used by `create_pool`; test:

```rust
#[test]
fn tls_mode_selection() {
    let disable: tokio_postgres::Config = "postgres://u@h/db?sslmode=disable".parse().unwrap();
    let prefer: tokio_postgres::Config = "postgres://u@h/db".parse().unwrap();
    let require: tokio_postgres::Config = "postgres://u@h/db?sslmode=require".parse().unwrap();
    assert!(!super::wants_tls(&disable));
    assert!(super::wants_tls(&prefer));
    assert!(super::wants_tls(&require));
}
```

**Verify**: `cargo test -p schemalane-cli tls_mode_selection` → pass.

### Step 4: Manual end-to-end verification

Against any TLS-enforcing Postgres you can access (managed instance, or local: `docker run -d -p 5544:5432 -e POSTGRES_PASSWORD=pg postgres:17 -c ssl=on -c ssl_cert_file=/etc/ssl/certs/ssl-cert-snakeoil.pem -c ssl_key_file=/etc/ssl/private/ssl-cert-snakeoil.key` — note: snakeoil certs will FAIL verification, which is itself informative):

- `sslmode=require` against a managed DB with a real cert → connects (`Connecting to PostgreSQL … SUCCESS`).
- `sslmode=require` against self-signed → clean error mentioning certificate verification (NOT a hang or panic).
- `sslmode=disable` against local plaintext Postgres → still works (regression check).

**Verify**: the three behaviors above. If no TLS endpoint is reachable, run only the disable-regression and record the limitation.

### Step 5: Full gate

fmt + clippy + `cargo test --workspace --locked` → exit 0. Commit `Cargo.lock` changes.

## Test plan

- `tls_mode_selection` unit test.
- Manual matrix (Step 4).
- Existing integration tests (plaintext localhost, default `prefer`): tokio-postgres downgrades to plaintext when the server lacks SSL — they must stay green. Run them if Docker available.

## Done criteria

- [ ] `grep -n "NoTls" schemalane-cli/src/lib.rs` → only inside the `Disable` arm
- [ ] `tls_mode_selection` passes; workspace gate green; lockfile committed
- [ ] Step 4 results recorded in the status row note
- [ ] Only in-scope files modified
- [ ] `plans/README.md` updated

## STOP conditions

- `tokio-postgres-rustls` / `rustls-platform-verifier` versions don't interoperate with the locked tokio-postgres 0.7/deadpool 0.14 (trait version mismatch) — report the exact conflict; do NOT switch to `native-tls` on your own (that's a dependency-policy decision).
- Plaintext-localhost integration tests break under `prefer` (server without SSL should downgrade; if it errors, the connector wiring is wrong — report).
- deadpool `Manager` generics prevent the two-arm construction and boxing — report the type error verbatim.

## Maintenance notes

- `verify-ca`/`verify-full` are unsupported because tokio-postgres 0.7 doesn't parse them from URLs; when it does, extend `wants_tls` accordingly. Document current support (`disable`/`prefer`/`require`) in README's flags section.
- Corporate-proxy/self-signed users will ask for a `--ssl-root-cert` flag (custom CA file) — deliberate follow-up, not in this plan.
- The integration-test harness keeps `NoTls` (its own `create_pool` in `postgres_integration.rs:427`) — fine; do not unify.
