# Repair command design

## Evidence and scope decision

Flyway 12.11.0 documents three repair actions: remove failed migrations,
realign applied checksums/descriptions/types, and mark missing migrations as
deleted. A PostgreSQL 17 container characterization confirmed the physical
history changes. Schemalane v1 should implement only failed-row removal and
checksum alignment. Missing-as-deleted requires a new history type/state and is
therefore deferred under the spike's schema-change stop condition.

Official references checked on 2026-07-16:

- Redgate Flyway `repair` command documentation.
- Redgate Flyway schema-history states documentation.
- Flyway 12.11.0 Community container against PostgreSQL 17.

## Flyway parity table

| Action | Observed history mutation |
|---|---|
| Remove failed | Deletes every row where `success = false`; user objects are untouched. Rank gaps remain. |
| Align resolved applied migration | Updates `description`, `type`, and `checksum` from the resolved migration. The original `script`, `installed_by`, `installed_on`, `execution_time`, `success`, and `installed_rank` remain unchanged. |
| Mark missing as deleted | Appends a successful row with the next `installed_rank`, the missing migration's `version`, `description`, `script`, and `checksum`, `type = 'DELETE'`, `execution_time = 0`, and the repair operator as `installed_by`. The original successful row remains. |

Container transcript summary: V1's description and checksum were updated while
its original script and timing remained; a synthetic failed V3 row disappeared;
missing V2 gained a second row at rank 3 with type `DELETE`. A migration above
the highest locally resolved version was initially classified `Future`, not
missing; deletion marking occurred once validation classified it missing.

## Command and confirmation

```text
schemalane migrate repair [OPTIONS]

Options:
  --remove-failed       Delete failed history rows
  --align-checksums     Replace applied checksums with local checksums
  --confirm <CONFIRM>   Pass "yes" to execute the displayed plan
  --format <table|json> Output format [default: table]
```

Bare `repair` selects both supported actions for Flyway familiarity. Every run
first prints the exact row plan, then requires interactive `yes` or
`--confirm yes`. Non-interactive input without confirmation returns the existing
destructive-guard exit 6. The operation takes the same advisory lock as `up`
and performs all history mutations in one database transaction. It never
renumbers ranks and never changes application objects.

## `RepairReport`

```json
{
  "failed_rows_removed": [
    { "installed_rank": 3, "script": "V3__failed.sql" }
  ],
  "checksums_aligned": [
    { "script": "V1__first.sql", "old": 123, "new": 456 }
  ]
}
```

Nothing-to-do is success with empty arrays. No observer events are added. Drift
or failed-state discovery uses existing exit meanings; repair itself introduces
no new exit code.

## Edge-case decisions and test matrix

1. Clean history: no changes, empty report, exit 0.
2. One failed local script: delete its failed rows; leave rank gaps.
3. Failed script absent locally: still delete failed rows when requested.
4. Failed row followed by success: delete all stale failed rows, retain success.
5. Applied checksum mismatch: update only checksum; report old and new values.
6. Failed script whose file changed: failed removal wins; do not align a row
   being deleted.
7. Concurrent `up`: repair waits on the shared advisory lock.
8. Any mutation failure: transaction rolls back the entire repair.

## Build-plan sketch

1. Add serializable repair report entries to core.
2. Add repository methods for selecting repair candidates, deleting failed
   rows, and updating checksums; keep all SQL in `HistoryRepository`.
3. Add locked `SchemalaneMigrator::repair` with a preview/execute split.
4. Wire CLI, delegation, confirmation, table, and JSON output.
5. Add the eight integration cases above plus cancellation/lock cleanup.
6. Treat missing-as-deleted as a separate compatibility plan that first adds a
   `DELETE` history type and deleted status semantics.

## Why no prototype was retained

The parity investigation hit the explicit stop condition: full Flyway repair
requires a history-schema/state decision (`DELETE` rows). A remove-failed-only
prototype would prematurely commit mutation and confirmation APIs before that
compatibility decision. The repository seam is ready; this document is the
maintainer review boundary.
