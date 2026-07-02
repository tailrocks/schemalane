# Plan 014: Scope the advisory lock key to (schema, history table) and expose it as a flag

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result. On any STOP condition,
> stop and report. When done, update `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat dd0d79d..HEAD -- schemalane-core/src/lib.rs schemalane-cli/src/lib.rs`
> On mismatch with "Current state" excerpts, STOP.

## Status

- **Priority**: P2
- **Effort**: S
- **Risk**: MED
- **Depends on**: none
- **Category**: security / correctness
- **Planned at**: commit `dd0d79d`, 2026-07-03

## Why this matters

PostgreSQL advisory locks are scoped per **database**. Schemalane locks a single hardcoded constant, independent of the target schema/history table and not settable from the CLI. Two independent deployments sharing one database via different `--schema` values (a supported multi-tenant pattern) therefore contend on the identical key: a long or hung migration in tenant A blocks tenant B's migrations entirely — cross-tenant availability coupling operators cannot configure away. Over-locking is safe but surprising; the fix is a deterministic per-(schema, history-table) key plus an explicit override flag. Trade-off to preserve: two runners on the SAME schema must still collide — the derivation must be stable, not random.

## Current state

- `schemalane-core/src/lib.rs:21`: `const DEFAULT_ADVISORY_LOCK_ID: i64 = 7_333_654_209_921_337;`
- `SchemalaneConfig` (lines 23–42): field `advisory_lock_id: i64`, defaulted to the constant.
- Lock usage, `with_advisory_lock` (lines 641–655): `SELECT pg_advisory_lock($1)` / `pg_advisory_unlock($1)` with `self.config.advisory_lock_id`.
- CLI never sets it: both config constructions use `..Default::default()` (`schemalane-cli/src/lib.rs:357-363` embedded, `629-635` direct). No flag exists.
- Spec §5: "a single PostgreSQL advisory lock for the full migration session" — silent on key derivation; this change stays within it.
- Checksum/CRC convention: `crc32fast::Hasher` already a core dependency (used in `calculate_checksum`).

## Commands you will need

| Purpose | Command | Expected |
|---|---|---|
| Gate | fmt + `cargo clippy --workspace --locked --all-targets --all-features -- -D warnings` + `cargo test --workspace --locked` | exit 0 |
| Integration (Docker) | `cargo test -p schemalane-core --locked --test postgres_integration -- --include-ignored` | pass |

## Scope

**In scope**: `schemalane-core/src/lib.rs`, `schemalane-cli/src/lib.rs`.
**Out of scope**: lock lifecycle/leak semantics (`plans/021-engine-connection-model.md`); spec §5 wording (note in plan 009/036 if needed); the `EmbeddedCli`/`MigrateArgs` duplication (plan 029) — add the flag to BOTH structs by hand for now.

## Git workflow

- Branch: `advisor/014-advisory-lock-key-scoping`
- Suggested commit: `Derive advisory lock key from schema and history table`
- No push/PR without operator instruction.

## Steps

### Step 1: Derive the default key

In `schemalane-core/src/lib.rs`:

```rust
/// Fixed application discriminator for schemalane advisory locks.
const ADVISORY_LOCK_NAMESPACE: i64 = 7_333_654_209_921_337;

/// Stable per-target lock key: same (schema, history_table) → same key on
/// every runner version; different targets in one database don't contend.
/// Advisory locks are database-scoped, so the database name is irrelevant.
pub fn derive_advisory_lock_id(schema: &str, history_table: &str) -> i64 {
    let mut hasher = Hasher::new();
    hasher.update(schema.as_bytes());
    hasher.update(&[0]); // separator: ("ab","c") must differ from ("a","bc")
    hasher.update(history_table.as_bytes());
    let low = i64::from(hasher.finalize());
    // High 32 bits: schemalane's fixed namespace (avoids colliding with other
    // tools' advisory keys). Low 32 bits: CRC-32 of the target identifiers.
    (ADVISORY_LOCK_NAMESPACE & !0xFFFF_FFFFi64) | low
}
```

2. Change `SchemalaneConfig`: `advisory_lock_id: i64` → `advisory_lock_id: Option<i64>` (None = derive). `Default` sets `None`. In `with_advisory_lock`, resolve once:

```rust
let lock_id = self.config.advisory_lock_id.unwrap_or_else(|| {
    derive_advisory_lock_id(&self.config.schema, &self.config.history_table)
});
```

and bind `$1` to `lock_id` for both lock and unlock.

**Breaking-change note**: `SchemalaneConfig`'s field type changes (published crate, 0.x — acceptable; `..Default::default()` users, including this repo's CLI, keep compiling only where they didn't set the field explicitly — grep for `advisory_lock_id` uses).

**Verify**: `cargo clippy -p schemalane-core --all-targets -- -D warnings` → exit 0.

### Step 2: Expose `--advisory-lock-id` in both CLIs

Add to `MigrateArgs` (after `installed_by`, `schemalane-cli/src/lib.rs:434-435`) and to `EmbeddedCli` (after its `installed_by`):

```rust
/// Override the advisory lock key (default: derived from schema + history table).
#[arg(long)]
advisory_lock_id: Option<i64>,
```

Thread it into both `SchemalaneConfig` constructions (replacing part of `..Default::default()`), and forward it in `run_via_migration_crate` delegation args (`--advisory-lock-id <v>` when set). Update the `delegation_command_parts` helper if plan 007 landed.

**Verify**: `cargo run -p schemalane-cli -- migrate --help` shows the flag; clippy green.

### Step 3: Tests

Unit (core tests module):

```rust
#[test]
fn advisory_lock_key_is_stable_and_target_scoped() {
    let a = derive_advisory_lock_id("public", "flyway_schema_history");
    assert_eq!(a, derive_advisory_lock_id("public", "flyway_schema_history")); // stable
    assert_ne!(a, derive_advisory_lock_id("tenant_b", "flyway_schema_history")); // schema-scoped
    assert_ne!(a, derive_advisory_lock_id("public", "other_history")); // table-scoped
    assert_ne!(
        derive_advisory_lock_id("ab", "c"),
        derive_advisory_lock_id("a", "bc"),
        "separator must prevent concatenation collisions"
    );
}
```

Integration (Docker, model on existing tests): hold `SELECT pg_advisory_lock(derive_advisory_lock_id("public","flyway_schema_history"))` on a side connection, then assert a second-schema migrator (`schema: "other"`) completes `up` without blocking (use a `tokio::time::timeout` around it). Release and assert same-schema DOES block (timeout elapses). If the blocking-case test proves flaky, keep only the non-blocking case and note it.

**Verify**: `cargo test -p schemalane-core advisory_lock_key` → pass; integration suite green if Docker present.

### Step 4: Full gate

fmt + clippy + workspace tests → exit 0.

## Test plan

As Step 3: stability, schema-scoping, table-scoping, separator collision; integration non-contention across schemas.

## Done criteria

- [ ] `grep -n "derive_advisory_lock_id" schemalane-core/src/lib.rs schemalane-cli/src/lib.rs` → definition + resolution + CLI threading
- [ ] `--advisory-lock-id` in `migrate --help`
- [ ] New tests pass; gate green; only in-scope files modified
- [ ] `plans/README.md` updated

## STOP conditions

- Rollout hazard check fails: during a mixed-version window (old runner locks the constant, new runner locks the derived key) two runners on the SAME schema would NOT contend. If the operator runs blue/green migrators concurrently, this needs a release-note callout — if you cannot confirm the single-runner deployment assumption, report rather than land silently.
- `with_advisory_lock` moved/replaced (plan 021 landed) — apply the same derivation at its new site.

## Maintenance notes

- Release notes MUST state the default lock key changed (mixed-version concurrent runners briefly don't exclude each other; `--advisory-lock-id <old constant>` is the escape hatch during transition).
- Key derivation is a compatibility surface from now on — changing it repeats the mixed-version hazard; the unit test pins it.
- Flyway derives its lock similarly (table-name hash); we deliberately include schema for tenant isolation.
