# Plan 021: One detached connection per migration session (fixes lock leak on panic/cancel, search_path pollution, small-pool deadlock, connection churn)

> **Executor instructions**: Follow this plan step by step, verifying each step.
> On any STOP condition, stop and report. When done, update `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat dd0d79d..HEAD -- schemalane-core/src/lib.rs`
> Locate excerpts by symbol; plans 011/018/020 legitimately touched this file.

## Status

- **Priority**: P2
- **Effort**: M
- **Risk**: MED (core execution structure — requires plans 001 + 019 landed; coordinate with 020)
- **Depends on**: plans/001-ci-verification-baseline.md, plans/019-core-state-model-tests.md
- **Category**: bug
- **Planned at**: commit `dd0d79d`, 2026-07-03

## Why this matters

The engine currently uses up to **three** pooled connections per run with session-level state and no cleanup guarantees. Four concrete defects, one root cause (pooled, stateful, non-owned connections):

1. **Advisory-lock leak**: `with_advisory_lock` unlocks only on the straight-line path. If the operation future panics or is cancelled (tokio timeout/drop), the lock connection returns to the pool **still holding the session lock** (deadpool `RecyclingMethod::Fast` — the CLI's setting — performs no reset). In an embedded process that survives (catches the panic / continues after cancel), every later `up`/`fresh` blocks forever on `pg_advisory_lock`.
2. **`search_path` pollution**: `set_search_path` uses `set_config(…, false)` — session-level — and prepends the schema on *every* migration's freshly-acquired connection. Recycled connections accumulate duplicates; worse, a **caller-shared** pool (public API takes `&Pool`) leaks the modified search_path into the application's own queries.
3. **Small-pool deadlock**: lock connection + history connection + per-migration connection = 3 concurrent checkouts with no acquire timeout; a caller passing `max_size` 1–2 deadlocks forever.
4. **Churn**: 2 extra round-trips per migration re-establishing search_path (PERF-04).

Structural fix: acquire **one** connection for the whole session, **detach it from the pool** (`deadpool::managed::Object::take`), run lock + schema setup + history + migrations on it, and explicitly unlock at the end. Detachment is the safety net: on panic/cancel the owned client drops → TCP closes → PostgreSQL releases session advisory locks and all session state dies with the session. Pool demand becomes 1; search_path is set once on a session nobody else will ever see.

## Current state

(`schemalane-core/src/lib.rs`)

- `with_advisory_lock` (635–663): `lock_client = pool.get()` → `pg_advisory_lock($1)` → `fut.await` → `pg_advisory_unlock($1)`; the `(Err, Err)` arm swallows the unlock error.
- `up_with_observer` (395–508): `client = pool.get()` (406) for schema/history; `apply_migration(pool, …)` (435) does `pool.get()` per migration (880, 895) + `set_search_path(&client)` (881, 896).
- `set_search_path` (923–933): reads `current_setting('search_path')`, then `set_config('search_path', $1, false)` prepending `quote_ident(schema)`.
- `execute_sql_migration(client: &mut Client, …)` (1372) needs `&mut` for `client.transaction()`.
- Rust executors receive `&Client` (`RustMigrationFuture`, 300–343).
- deadpool-postgres 0.14: `Pool::get()` yields `Object<Manager>`; **`deadpool_postgres::Object` derefs to `ClientWrapper`/`Client`**, and `deadpool::managed::Object::take(obj)` detaches the inner client from the pool (pool slot freed; drop closes the connection). Verify exact path in the locked version: `deadpool_postgres::Client` is an alias for the managed Object; `Object::take` is an associated fn — call as `deadpool::managed::Object::take(obj)`. It returns `ClientWrapper` whose deref target is `tokio_postgres::Client`.
- Users of `with_advisory_lock`: `up_with_observer` (405), `fresh_with_observer` (549). `status` uses no lock (fine).

## Commands you will need

| Purpose | Command | Expected |
|---|---|---|
| deadpool API confirmation | `grep -rn "pub fn take" ~/.cargo/registry/src/*/deadpool-0.12*/src/managed/mod.rs` (version per Cargo.lock) | associated `take` exists |
| Gate | fmt + clippy gate + `cargo test --workspace --locked` | exit 0 |
| Integration (required) | `cargo test -p schemalane-core --locked --test postgres_integration -- --include-ignored` | all pass incl. plan-019 lock tests |

## Scope

**In scope**: `schemalane-core/src/lib.rs`; `schemalane-core/tests/postgres_integration.rs` (new tests).
**Out of scope**: CLI pool construction (`max_size(5)` stays); public signatures `up/fresh/status(&Pool)` stay (the consolidation is internal); plan 020's txn-history logic (compose, don't rewrite); RecyclingMethod choice (irrelevant once the session connection is detached).

## Git workflow

- Branch: `advisor/021-engine-connection-model`
- Suggested commit: `Run each migration session on one detached connection`
- No push/PR without operator instruction.

## Steps

### Step 1: Restructure `with_advisory_lock` into a session owner

Replace it with:

```rust
async fn with_locked_session<T, F, Fut>(
    &self,
    pool: &Pool,
    body: F,
) -> Result<T, SchemalaneError>
where
    F: FnOnce(&mut tokio_postgres::Client) -> Fut, // adapt: see borrow note
    Fut: Future<Output = Result<T, SchemalaneError>>,
{
    let pooled = pool.get().await?;
    // Detach: the pool must never see this session again. Session state
    // (advisory lock, search_path) dies with the connection on ANY exit
    // path — including panic and future-cancellation — because dropping
    // the detached client closes the socket.
    let mut client = deadpool::managed::Object::take(pooled);

    let lock_id = /* config/derived key, as today */;
    client.execute("SELECT pg_advisory_lock($1)", &[&lock_id]).await?;

    let result = body(&mut client).await;

    // Best-effort fast release; the socket close is the guarantee.
    let unlock = client
        .execute("SELECT pg_advisory_unlock($1)", &[&lock_id])
        .await;
    match (result, unlock) {
        (Ok(v), _) => Ok(v),          // unlock error irrelevant: socket closes next
        (Err(e), _) => Err(e),
    }
}
```

Borrow-shape note: an `FnOnce(&mut Client) -> Fut` closure with a borrowed argument needs HRTB gymnastics; the pragmatic Rust shape is to inline the body instead — make `up_with_observer`/`fresh_with_observer` call `let mut client = Self::acquire_session(pool, lock_id).await?;` (a helper returning the detached, locked client) and put lock-release in an explicit tail + rely on drop-on-error. Choose whichever compiles cleanly; the invariants that matter: (a) detached before locking, (b) all work on that one client, (c) explicit unlock only on the success path is acceptable because every other path closes the socket.

`ClientWrapper` derefs to `tokio_postgres::Client`; `execute_sql_migration(&mut client, …)` and Rust executors (`&client`) work on it directly.

### Step 2: Run everything on the session connection

In `up_with_observer`/`fresh_with_observer`:

- Delete the separate `client = pool.get()` — schema setup, history load, rank counter, history inserts, and `apply_migration` all use the session client.
- `apply_migration` loses its `pool` parameter and its per-migration `pool.get()`; it takes `&mut Client`.
- Call `set_search_path` **once**, right after acquiring the session (before `ensure_target_schema`? — order: `ensure_target_schema` first (needs no path), then `set_search_path`, then history table). Delete the per-migration calls. The duplicate-prepend problem disappears (fresh session, set once); keep the prepend-not-replace semantics (the function's doc comment explains why — extensions in `public`).
- `fresh`'s schema-drop runs on the same client.

### Step 3: Failed-row durability check

Failed-row inserts now share the session client. They previously used a *different* connection than the migration — but the migration's transaction is already rolled back/committed before the insert happens (sequencing, not connection identity, is what protects it), and plan 020 keeps failure writes outside migration transactions. Add an explicit integration assertion (Step 4.2) since this is the subtlest invariant of the change.

One real edge: a **non-transactional** statement failure (e.g. mid-file CONCURRENTLY failure) can leave the session in a usable state — `batch_execute` errors don't poison a non-txn session. A **transactional** failure after `rollback` is likewise clean. If the connection itself died (server crash), the failed-row insert fails too and the run surfaces the DB error — same as today.

### Step 4: Tests

1. Plan 019's lock tests must pass unchanged (blocking + release semantics).
2. `failed_sql_migration_records_failed_row_on_session_connection`: failing transactional migration → exactly one `success=false` row (proves post-rollback insert works on the same session).
3. `up_works_with_pool_max_size_one`: build a pool with `max_size(1)`, run a normal `up` → succeeds (the old model deadlocked; this is the small-pool regression test). Note: with detachment, the pool slot is freed on take — even size-1 pools can't starve.
4. Cancellation/leak test (the panic-window): run `up` against a migration that sleeps (Rust executor with `tokio::time::sleep`), wrap in `tokio::time::timeout` that fires mid-migration, then assert a fresh `pg_try_advisory_lock(key)` on a side connection **eventually** succeeds (poll up to ~5s — socket close is async). Mark `#[ignore = "timing-sensitive"]` if flaky, but attempt it.

**Verify**: full integration suite green.

### Step 5: Full gate

fmt + clippy + workspace + integration → green.

## Test plan

As Step 4; plus the entire pre-existing suite (7 original + plan 019's) unchanged.

## Done criteria

- [ ] `grep -c "pool.get()" schemalane-core/src/lib.rs` → exactly 2 (one in `up`/session acquire, one in `fresh` — or 1 if shared; `status` keeps its own non-locked `pool.get()`, adjust count accordingly and note it)
- [ ] `grep -n "Object::take" schemalane-core/src/lib.rs` → present
- [ ] `set_search_path` called once per session (grep call sites)
- [ ] All integration tests green incl. new pool-size-1 and failed-row tests
- [ ] Only in-scope files modified; `plans/README.md` updated

## STOP conditions

- `deadpool::managed::Object::take` doesn't exist in the locked deadpool version (API check in Commands fails) — report; do NOT fall back to leaving the connection pooled (that reintroduces the leak) without approval.
- Plan 020 landed and its `HistoryWrite`-in-txn shape conflicts with single-session structure — read its diff; the two compose (session client hosts the txn), but if the code disagrees with that expectation, STOP.
- The `status` path accidentally starts taking the advisory lock — it must not; verify and keep it lock-free.
- Plan 019 characterization tests need assertion edits — behavior changed; revert and report.

## Maintenance notes

- The detached connection is the load-bearing safety mechanism — a future "optimization" returning it to the pool reintroduces the lock-leak and search_path-pollution bugs; comment this at the acquire site (done in Step 1's comment).
- Callers' pools no longer need ≥3 slots; document "pool of 1 is sufficient" in rustdoc (`up`/`fresh`).
- PERF-04's per-migration round-trips are gone as a side effect; `plans/022-cli-double-work.md` handles the CLI-level duplication that remains.
