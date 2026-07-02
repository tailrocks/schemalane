# Plan 023: Test the CLI's external contracts — exit-code table, JSON shape, delegation argv, env precedence, filename edge cases

> **Executor instructions**: Follow this plan step by step, verifying each step.
> On any STOP condition, stop and report. When done, update `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat dd0d79d..HEAD -- schemalane-cli/src/lib.rs schemalane-core/src/lib.rs schemalane-core/src/filename.rs`
> Earlier plans touched these; locate by symbol.

## Status

- **Priority**: P2
- **Effort**: M
- **Risk**: LOW
- **Depends on**: benefits from plans 003 (Delegated variant), 007 (`delegation_command_parts` helper); works without them with minor adaptation
- **Category**: tests
- **Planned at**: commit `dd0d79d`, 2026-07-03

## Why this matters

The CLI's machine-facing contracts have zero tests: the spec-§8 exit-code table (scripting contract), the `status --format json` document shape (field names, `"type"` rename, PascalCase states — consumed by pipelines), the delegation argv (every flag forwarded to `cargo run` children), env-var precedence (`DATABASE_URL`, `MIGRATION_DIR`), and the filename parser's **rejection** edges (Flyway compat promise §3 — what must fail). `schemalane-cli/src/lib.rs` is the repo's highest-churn file; its newest features (delegation) are its least-tested. These are all cheap, DB-free unit tests.

## Current state

- Exit codes: `SchemalaneError::exit_code` (`schemalane-core/src/lib.rs:84-94`) — Validation 2, Drift 3, FailedHistory 4, PendingMigrations 5, FreshRequiresConfirm 6, MixedStatements 7, `Delegated{code}` verbatim (if plan 003 landed), else 1. Spec §8 (+ plan 009 documents 7).
- JSON: `StatusReport`/`StatusEntry`/`StatusSummary`/`MigrationState` derive `Serialize` with `#[serde(rename_all = "PascalCase")]` on the state enum (97–136) and `#[serde(rename = "type")]` on `migration_type` (111–112). CLI prints `serde_json::to_string_pretty(&report)` (`schemalane-cli/src/lib.rs:868-873`).
- Delegation argv: `run_via_migration_crate` (642–718) — forwards `--schema`, `--history-table`, `--installed-by`, `--verbosity`, subcommand + its flags (`--format`, `--fail-on-pending`, `--confirm`); post-plan-007 the URL rides `env("DATABASE_URL")` and a pure `delegation_command_parts(…) -> (args, envs)` helper exists; post-plan-014 also `--advisory-lock-id`.
- Env precedence: `MigrateArgs` — `#[arg(long, env = "DATABASE_URL")] database_url` (425–426), `#[arg(short='d', long="migration-dir", env="MIGRATION_DIR", default_value=DEFAULT_MIGRATION_DIR)]` (417–423). Clap rule: CLI flag beats env beats default.
- Existing CLI tests (1321–1431): 8 tests — arg parsing + URL target formatting. Clap testing idiom in-repo: `Cli::try_parse_from([...])`.
- Filename rejections untested (`schemalane-core/src/filename.rs`): non-digit part / empty part (25–29), `V__desc.sql` missing version (80–84), plus display-description transform (`replace('_', " ")` at lib.rs:726/767) unasserted.
- Env-var tests caveat: `std::env::set_var` is process-global — clap's `env` feature reads real env; tests must serialize (Rust test threads share env) — use a mutex or the `temp-env` crate; prefer `temp-env` (tiny, purpose-built).

## Commands you will need

| Purpose | Command | Expected |
|---|---|---|
| Add dev-dep | `cargo add -p schemalane-cli --dev temp-env` | resolves |
| Unit | `cargo test -p schemalane-cli --locked` and `cargo test -p schemalane-core --locked` | pass |
| Gate | fmt + clippy gate + `cargo test --workspace --locked` | exit 0 |

## Scope

**In scope**: test modules of `schemalane-cli/src/lib.rs`, `schemalane-core/src/lib.rs`, `schemalane-core/src/filename.rs`; `schemalane-cli/Cargo.toml` (dev-dep).
**Out of scope**: production code (tiny testability refactors allowed ONLY if plan 007's helper is absent — then extract `delegation_command_parts` exactly as specified there); spawning real `cargo` children (argv construction is the unit; E2E delegation was proven manually in plan 003).

## Git workflow

- Branch: `advisor/023-cli-contract-tests`
- Suggested commit: `Test exit codes, JSON shape, delegation argv, env precedence, filename edges`
- No push/PR without operator instruction.

## Steps

### Step 1: Exit-code table test (core)

In core's test module. Constraint: `tokio_postgres::Error` cannot be constructed in unit tests, so the `Db`/`Pool`/`MigrationExecution` variants are covered by the `_ => 1` fallthrough argument in a comment; test every constructible variant:

```rust
#[test]
fn exit_codes_match_spec_section_8() {
    use SchemalaneError as E;
    assert_eq!(E::Validation("x".into()).exit_code(), 2);
    assert_eq!(E::Drift("x".into()).exit_code(), 3);
    assert_eq!(E::FailedHistory("x".into()).exit_code(), 4);
    assert_eq!(E::PendingMigrations(3).exit_code(), 5);
    assert_eq!(E::FreshRequiresConfirm.exit_code(), 6);
    assert_eq!(E::MixedStatements { script: "s".into(), line: 1 }.exit_code(), 7);
    assert_eq!(E::Io(std::io::Error::other("x")).exit_code(), 1);
    // Db/Pool/MigrationExecution wrap tokio_postgres::Error (not constructible
    // in unit tests) — they fall through the `_ => 1` arm by construction.
}
```

(Plus `Delegated` forwarding if plan 003 landed — it has its own test already.)

### Step 2: JSON shape contract test (CLI)

```rust
#[test]
fn status_json_shape_is_stable() {
    let report = StatusReport {
        schema: "public".into(),
        history_table: "flyway_schema_history".into(),
        migrations: vec![StatusEntry {
            version: Some("1".into()),
            description: "init".into(),
            migration_type: "SQL".into(),
            script: "V1__init.sql".into(),
            checksum: Some(-559038737),
            installed_rank: Some(1),
            installed_on: Some("2026-01-01 00:00:00".into()),
            execution_time_ms: Some(12),
            state: MigrationState::Success,
        }],
        summary: StatusSummary { success: 1, ..Default::default() },
    };
    let value: serde_json::Value =
        serde_json::from_str(&serde_json::to_string(&report).unwrap()).unwrap();
    let entry = &value["migrations"][0];
    assert_eq!(entry["type"], "SQL");            // serde rename
    assert_eq!(entry["state"], "Success");       // PascalCase
    assert_eq!(entry["checksum"], -559038737);   // signed i32 survives
    assert_eq!(value["summary"]["checksum_mismatch"], 0);
    // Field-name freeze: consumers depend on these exact keys.
    let keys: Vec<&str> = entry.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        ["version","description","type","script","checksum","installed_rank",
         "installed_on","execution_time_ms","state"]
    );
}
```

Also one per state: serialize each `MigrationState` and assert `"Pending"`, `"Failed"`, `"Missing"`, `"ChecksumMismatch"` spellings.

### Step 3: Delegation argv matrix (CLI)

Using `delegation_command_parts` (from plan 007; extract it per that plan's Step 2 if absent):

- `Up` + all options set → args contain, in order: `run --manifest-path <p> -- --schema s --history-table h --installed-by me --verbosity minimal up`; env contains `DATABASE_URL`.
- `Status { format: Json, fail_on_pending: true }` → `status --format json --fail-on-pending` present.
- `Fresh { confirm: Some("yes") }` → `fresh --confirm yes`.
- No `--database-url` string anywhere in args (secret property, already tested in plan 007 — keep both).

### Step 4: Env precedence (CLI)

With `temp-env`:

```rust
#[test]
fn database_url_flag_beats_env() {
    temp_env::with_var("DATABASE_URL", Some("postgres://env@h/db"), || {
        let cli = Cli::try_parse_from([
            "schemalane", "migrate", "--database-url", "postgres://flag@h/db", "up",
        ]).unwrap();
        let args = unwrap_migrate(cli);
        assert_eq!(args.database_url.as_deref(), Some("postgres://flag@h/db"));
    });
}

#[test]
fn database_url_env_used_when_flag_absent() { /* env only → Some(env value) */ }

#[test]
fn migration_dir_env_beats_default() {
    temp_env::with_var("MIGRATION_DIR", Some("/tmp/envdir"), || {
        let cli = Cli::try_parse_from(["schemalane", "migrate", "up"]).unwrap();
        assert_eq!(unwrap_migrate(cli).migration_dir, PathBuf::from("/tmp/envdir"));
    });
}
```

(`unwrap_migrate` helper already exists, line 1331.)

### Step 5: Filename rejection + display-description tests (core)

In `filename.rs` tests: `V__x.sql` → "missing version"; `Vabc__x.sql` and `V1..2__x.sql` (empty part) → "invalid version"; `V1.2.3.0__x.sql` parses equal to `V1.2.3__y.sql`'s version (trailing-zero pop — assert via `ParsedVersion` equality). In core lib tests: discovery of `V1__add_user_table.sql` yields `description_display == "add user table"` (via a TempDir + `discover_migrations`, pattern from plan 005).

### Step 6: Full gate

fmt + clippy + `cargo test --workspace --locked` → green. Count new tests: ≥14.

## Test plan

Steps 1–5 enumerate the cases; model on the existing CLI test idioms (`try_parse_from`, `unwrap_migrate`).

## Done criteria

- [ ] ≥14 new tests across the three test modules, all passing
- [ ] JSON key-freeze test present (the array-of-keys assertion)
- [ ] `temp-env` added as dev-dependency only
- [ ] Gates green; only test code (+ dev-dep line) modified
- [ ] `plans/README.md` updated

## STOP conditions

- The JSON key-freeze test fails — the serialized shape differs from "Current state" (a serde attribute changed since audit); this is a REAL finding about an already-shipped contract; report before "fixing" the test.
- `delegation_command_parts` doesn't exist and extracting it per plan 007 hits unexpected coupling — report.
- Env tests interfere with each other despite `temp-env` (parallel test races) — annotate with `#[serial]` via `serial_test` or run single-threaded; report which was needed.

## Maintenance notes

- The key-freeze test makes JSON evolution a deliberate act: adding a field breaks it → author must update the frozen list AND mention the consumer impact in the PR.
- When plan 029 unifies the duplicated arg structs, these parse tests are the safety net proving flag surface unchanged.
