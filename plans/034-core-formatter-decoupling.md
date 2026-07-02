# Plan 034: Decide and execute the core↛formatter dependency direction (engine pulls a pretty-printer for one preview string)

> **Executor instructions**: Follow this plan step by step, verifying each step.
> On any STOP condition, stop and report. When done, update `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat dd0d79d..HEAD -- schemalane-core/Cargo.toml schemalane-core/src/lib.rs pg_query_fmt/src/preview.rs schemalane-cli/src/lib.rs`
> On mismatch, STOP.

## Status

- **Priority**: P3
- **Effort**: M
- **Risk**: MED (public observer-event field changes)
- **Depends on**: plans/026-published-api-hygiene.md (non_exhaustive makes the event change less breaking); best after 022 (observer already extended there)
- **Category**: tech-debt
- **Planned at**: commit `dd0d79d`, 2026-07-03

## Why this matters

`schemalane-core` — the engine every embedder links — depends on `pg_query_fmt` (a ~4K-line SQL pretty-printer with its own `owo-colors` dep) for exactly **one call**: `statement_preview` producing a human-readable label stuffed into observer events. Presentation lives in the engine; every core consumer compiles a formatter they may never render. The clean cut: core emits raw facts (SQL text + AST node it already has), presentation-side consumers derive previews. This plan chooses the smallest faithful version of that cut.

## Current state

- `schemalane-core/Cargo.toml:26`: `pg_query_fmt = { version = "0.1.3", path = "../pg_query_fmt" }`.
- Sole use: `schemalane-core/src/lib.rs:1243` — `let preview = pg_query_fmt::preview::statement_preview(&parsed);` inside `parse_sql_migration`; stored in `ParsedSqlStatement.preview` and copied into the six `SqlStatement*` observer events' `statement_preview: String` field (194–227).
- Consumers of `statement_preview`: ONLY the CLI observer (`truncate_preview(&event.statement_preview, …)` at cli:257/294). The `statement: String` (full SQL) field already rides the same events.
- `pg_query_fmt::preview::statement_preview(&pg_query::ParseResult) -> String` (`preview.rs:13-33`): AST-node label or truncated raw SQL fallback. `pg_query` itself IS legitimately used by core (splitting/parsing/tx-detection) and stays.
- `pg_query_fmt` is a core dependency ONLY for this; the CLI depends on `pg_query_fmt` directly already (Cargo.toml:25).

Two candidate designs (decision embedded — take (A)):

- **(A) Move preview generation to the consumer**: core's events drop `statement_preview`; the CLI computes the preview itself. Cost: CLI must re-parse or receive the AST — re-parsing per *displayed* statement (Compact verbosity only) is one `pg_query::parse` per statement, acceptable for a display path; the CLI already re-parses in Detailed mode (`format_statement`). Removes the dep cleanly, shrinks the public event structs (breaking — soften via 026's non_exhaustive + release note).
- (B) Move the tiny `preview` module INTO core: kills the dep but moves presentation INTO the engine permanently — rejected: wrong direction of gravity, and `preview.rs` uses `object_type_label` tables that pg_query_fmt also needs.

## Commands you will need

| Purpose | Command | Expected |
|---|---|---|
| Dep proof | `cargo tree -p schemalane-core -e normal \| grep -c pg_query_fmt` | 0 after |
| Gate | fmt + clippy gate + `cargo test --workspace --locked` | exit 0 |
| Integration | standard integration command | pass |

## Scope

**In scope**: `schemalane-core/Cargo.toml`, `schemalane-core/src/lib.rs` (drop preview plumbing), `schemalane-cli/src/lib.rs` (compute preview in the observer).
**Out of scope**: `pg_query_fmt` itself; the `statement: String` field (stays — it's the raw fact); observer event consolidation (post-1.0 note in plan 028).

## Git workflow

- Branch: `advisor/034-core-formatter-decoupling`
- Suggested commit: `Drop pg_query_fmt from core; CLI derives statement previews`
- No push/PR without operator instruction.

## Steps

### Step 1: Core sheds the preview

- Remove `statement_preview` from `ParsedSqlStatement` (1212–1221) and from the six `SqlStatement*` events; delete the `statement_preview:` copies at every event construction site; remove `pg_query_fmt` from core's Cargo.toml. Core's `parse_sql_migration` no longer calls `statement_preview`.
- Release-note text in the commit: "BREAKING: SqlStatement\* events no longer carry `statement_preview`; derive it from `statement` (e.g. via pg_query_fmt::preview)."

**Verify**: `cargo tree -p schemalane-core -e normal | grep pg_query_fmt` → empty; core tests green.

### Step 2: CLI derives previews

In `CliProgressObserver` (Compact arms, cli:257/294): compute once per event:

```rust
let preview = pg_query_fmt::preview::preview_sql(&event.statement);
```

`preview_sql` is a NEW thin helper in `pg_query_fmt/src/preview.rs`:

```rust
/// Parse-and-preview convenience for callers holding raw SQL.
/// Falls back to a truncated raw string if parsing fails.
pub fn preview_sql(sql: &str) -> String {
    match pg_query::parse(sql) {
        Ok(parsed) => statement_preview(&parsed),
        Err(_) => sql.chars().take(96).collect(),
    }
}
```

(Char-based fallback truncation — consistent with plan 011's boundary-safety rule.)

**Verify**: CLI tests green; manual Compact-verbosity run shows unchanged preview text.

### Step 3: Full gate

fmt + clippy + workspace + integration → green. `cargo package --locked --allow-dirty -p schemalane-core` → exit 0.

## Test plan

- Existing suites (core events' other fields untouched).
- One new unit test for `preview_sql` in pg_query_fmt: valid SQL → label; garbage → truncated raw.
- Manual Compact transcript comparison.

## Done criteria

- [ ] Core's normal dep tree has no `pg_query_fmt`
- [ ] `grep -n "statement_preview" schemalane-core/src/lib.rs` → gone; CLI computes previews
- [ ] Gates + integration green; `plans/README.md` updated

## STOP conditions

- Another consumer of `statement_preview` exists outside the CLI (grep the workspace) — inventory it first.
- The per-displayed-statement re-parse measurably slows Compact mode on a large migration (unlikely; parse is µs–ms) — report numbers rather than caching prematurely.

## Maintenance notes

- Event structs now carry raw facts only — keep it that way; presentation belongs to observers.
- Embedders wanting previews get one function to call (`preview_sql`) — document in the release notes.
