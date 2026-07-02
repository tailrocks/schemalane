# Plan 030: Centralize all history-table SQL behind one repository seam

> **Executor instructions**: Follow this plan step by step, verifying each step.
> On any STOP condition, stop and report. When done, update `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat dd0d79d..HEAD -- schemalane-core/src/lib.rs`
> REQUIRES post-018/020/021 shapes; read their diffs first.

## Status

- **Priority**: P3
- **Effort**: M
- **Risk**: MED
- **Depends on**: plans/020-history-row-atomicity.md, plans/021-engine-connection-model.md, plans/006-history-table-identifier-quoting.md
- **Category**: tech-debt
- **Planned at**: commit `dd0d79d`, 2026-07-03

## Why this matters

The Flyway history table's SQL is hand-built in five separate places; the column list is written independently in the SELECT and the INSERT (drift between them would corrupt reconciliation silently), and the quoting bug plan 006 fixed existed precisely because one site skipped the shared helper. A single `HistoryRepository` owning table name, DDL, column set, and every query makes the next schema evolution (e.g. a future `repair` command needing UPDATE/DELETE — spike 038) a one-file change and makes quoting correctness structural instead of disciplinary.

## Current state

(`schemalane-core/src/lib.rs`; post-020 there is additionally a txn-INSERT variant — fold it in.)

- `ensure_history_table` (935–960): DDL with inline column definitions.
- `history_table_exists` (962–970): `to_regclass` probe (quoted per plan 006).
- `load_history` (972–996): SELECT with explicit column list (975) + row → `HistoryRow` mapping.
- `next_installed_rank` (1010–1018): deleted by plan 018 (rank counter) — skip if gone.
- `insert_history_row` (1020–1051): INSERT with its own column list; plus plan 020's `insert_history_row_txn` sibling.
- `qualified_table`/`quote_ident` (1559–1565).
- `HistoryRow` struct (1827–1838).

## Commands you will need

| Purpose | Command | Expected |
|---|---|---|
| Gate | fmt + clippy gate + `cargo test --workspace --locked` | exit 0 |
| Integration (required) | `cargo test -p schemalane-core --locked --test postgres_integration -- --include-ignored` | all pass, zero assertion edits |

## Scope

**In scope**: `schemalane-core/src/lib.rs` (or `src/history.rs` if plan 031 landed first — put the repository where the module map says).
**Out of scope**: schema changes to the history table (Flyway DDL is a compatibility contract — byte-preserve it); adding UPDATE/DELETE (spike 038's job — this plan just makes room); public API.

## Git workflow

- Branch: `advisor/030-history-repository-seam`
- Suggested commit: `Centralize history-table SQL in HistoryRepository`
- No push/PR without operator instruction.

## Steps

### Step 1: Introduce the repository

```rust
/// All SQL touching the Flyway-compatible history table lives here.
/// The DDL and column set are a compatibility contract (spec §6) — any
/// change must be weighed against existing Flyway/schemalane deployments.
pub(crate) struct HistoryRepository {
    qualified: String,       // qualified_table(schema, history_table)
}

impl HistoryRepository {
    pub(crate) fn new(schema: &str, history_table: &str) -> Self { … }

    const COLUMNS: &'static str =
        "\"installed_rank\", \"version\", \"description\", \"type\", \"script\", \
         \"checksum\", \"installed_by\", \"installed_on\", \"execution_time\", \"success\"";

    pub(crate) async fn ensure_table(&self, client: &Client, history_table: &str) -> Result<…>;
    pub(crate) async fn exists(&self, client: &Client) -> Result<bool>;
    pub(crate) async fn load(&self, client: &Client) -> Result<Vec<HistoryRow>>;
    pub(crate) async fn insert(&self, exec: &impl BatchOrExec, row: &NewHistoryRow) -> Result<()>;
}
```

Move the five (post-020: six) function bodies in, byte-preserving every SQL string except that INSERT and SELECT now both derive from ONE column-list constant (`COLUMNS` minus `installed_on` for the INSERT — the INSERT omits it (DB default `now()`); express as two consts derived visibly next to each other with a comment tying them). The index/PK-name construction from `ensure_history_table` moves too.

Executor-genericity for `insert` (Client vs Transaction): reuse plan 028's `BatchExec`-style seam if landed; else two thin methods (`insert_client`, `insert_txn`) sharing a private SQL+params builder.

### Step 2: Swap call sites

`SchemalaneMigrator` constructs one `HistoryRepository` per operation (schema + table from config) and calls it everywhere the old functions were used. Delete the old free methods.

### Step 3: Full gate + parity

fmt + clippy + workspace + integration → green, zero assertion edits (SQL strings unchanged ⇒ behavior identical).

## Test plan

Existing suites only — this is a pure move with one derived-constant consolidation. If any SQL string changed beyond whitespace, that's a STOP, not a test update.

## Done criteria

- [ ] `grep -c "flyway-ish column names\|installed_rank" schemalane-core/src/lib.rs` → column list appears in the repository consts only (grep `\"installed_rank\"` occurrences: DDL + the shared consts; no scattered lists)
- [ ] Old free functions gone; all suites green unchanged
- [ ] `plans/README.md` updated

## STOP conditions

- Byte-diffing the executed SQL (log or code inspection) shows ANY change — stop, this plan must be behavior-invisible.
- Dependencies not landed (018/020/021) — do not build the seam around shapes that are about to change.

## Maintenance notes

- Spike 038 (`repair`) adds `delete_failed_rows`/`update_checksums` HERE — that's the payoff.
- The DDL string remains the Flyway contract; the repository's doc comment says so — reviewers hold that line.
