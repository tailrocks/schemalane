# Out-of-order migration investigation

## Current Schemalane behavior

Schemalane always applies late-arriving versioned migrations. The retained
Docker integration characterization performs this sequence:

1. Apply V1 and V3.
2. Add V2 locally.
3. `status` reports V2 as `Pending`.
4. `up` applies only V2.
5. `status` reports V2 as `Success`.
6. History order is `(rank 1, V1)`, `(rank 2, V3)`, `(rank 3, V2)`.

Evidence command:

```text
cargo test -p schemalane-core --locked --test postgres_integration \
  characterize_late_migration_is_applied_out_of_order \
  -- --include-ignored

test result: ok. 1 passed
```

This is intentional behavior only in the sense that the current apply loop
selects every locally resolved script whose latest history row is not
successful; there is no maximum-applied-version gate or warning.

## Flyway 12.11.0 comparison

The same V1/V3-then-V2 scenario was run with Flyway Community 12.11.0 against
PostgreSQL 17.

| Operation | Flyway default | Schemalane current |
|---|---|---|
| `info` / `status` before second run | V2 is `Ignored` | V2 is `Pending` |
| validation | Fails: resolved migration 2 not applied; suggests `outOfOrder=true` | Valid unless another drift state exists |
| migrate / up | Refuses V2 because validation fails | Applies V2 |
| history if explicitly allowed | Flyway labels it out of order | Appends normal successful SQL row |

Flyway's exact diagnostic was: resolved migration 2 was not applied; use
`ignoreMigrationPatterns='*:ignored'` to ignore it or `outOfOrder=true` to
execute it. Both `validate` and `migrate` exited nonzero under defaults.

## Blast radius

- Core status has only `Pending`; no ignored/out-of-order state exists.
- Core application depends on membership in successful script names, not on
  the greatest applied version.
- `installed_rank` is append-only and already supports non-version order.
- CLI `latest_database_version` deliberately ignores pending entries and uses
  parsed maximum successful versions. A late V2 after V3 does not lower the
  displayed database version.
- Drift, checksum, locking, and history latest-row logic do not require strict
  ordering.
- Online `validate` from spike 037 currently permits all pending migrations,
  so matching Flyway strict ordering would need coordinated validation rules.

## Options memo

### A. Keep always out of order

Document this as a deliberate Flyway divergence. Lowest compatibility risk for
existing Schemalane users, but late DDL can execute after migrations that
assumed it already existed. Add an explicit `OutOfOrder` report state or warning
so operators cannot miss the condition.

### B. Match Flyway default

Reject resolved versions below the highest successful applied version, with an
`--out-of-order` opt-in on `up` and matching `validate` behavior. This is safest
for Flyway migrants but breaks any Schemalane workflow relying on current
always-on behavior. Maximum-version comparison must move into core using
`schemalane-version`; history rows need an explicit out-of-order display state
or derivation.

### C. Warn and apply

Preserve behavior while emitting a prominent warning and report state. This is
less disruptive but does not provide strict ordering as a safety property and
can normalize dangerous deployments through warning fatigue.

No behavior choice is made by this investigation.

## Maintainer questions

1. Is Flyway default compatibility or backward compatibility with current
   Schemalane behavior the governing requirement?
2. If strict-by-default wins, is the breaking change acceptable before 1.0,
   and must `validate` reject late pending migrations by default too?
3. Should out-of-order application be persisted as a distinct history type, or
   derived from version versus prior successful ranks for reporting?

## Test retention

`characterize_late_migration_is_applied_out_of_order` remains ignored with the
other Docker integration tests. Its exact status and rank assertions protect
the documented current behavior until a signed-off build plan deliberately
changes them.
