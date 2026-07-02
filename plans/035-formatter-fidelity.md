# Plan 035: Formatter display fidelity — real identifier quoting, precedence parens, honest ALTER labels, dropped-clause fixes, and a round-trip test harness

> **Executor instructions**: Follow this plan step by step, verifying each step.
> On any STOP condition, stop and report. When done, update `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat dd0d79d..HEAD -- pg_query_fmt/src/`
> Land AFTER plan 033 (module split); locate by symbol in the new layout.

## Status

- **Priority**: P3
- **Effort**: M
- **Risk**: LOW (display-only surface; executed SQL is never formatter output)
- **Depends on**: plans/033-formatter-split-dedup.md
- **Category**: bug (display) / tests
- **Planned at**: commit `dd0d79d`, 2026-07-03

## Why this matters

Under `--verbosity detailed`, operators read formatter output to approve what a migration is doing. Several constructs render as SQL that **parses differently from what actually executes** — a debugging trap. All verified against the source:

- **Precedence parens dropped**: `AexprOp` renders `{l} {op} {r}` bare (`expr.rs:158-163`); `fmt_bool_expr` joins `AND` bare and renders `NOT {operand}` bare while only `OR` self-parenthesizes (`expr.rs:246-257`). So `NOT (a AND b)` displays as `NOT a AND b` (≡ `(NOT a) AND b`), `(a+b)*c` as `a + b * c`.
- **No-op `quote_identifier`**: returns its input verbatim (`expr.rs:477-479`), so `"MyColumn"`, reserved words, and spaced identifiers display unquoted — invalid or different SQL.
- **Wrong keyword**: `fmt_alter_table` hardcodes `ALTER TABLE` ignoring `objtype` (`stmt.rs:578`) — `ALTER INDEX/VIEW/SEQUENCE/MATERIALIZED VIEW` all display as `ALTER TABLE` (the preview path gets it right via `object_type_label`, `preview.rs:118-122` — the two disagree).
- **Dropped clauses**: `SELECT DISTINCT ON (…)` loses the ON exprs (`stmt.rs:867-873` renders bare `SELECT DISTINCT`); array slices lose the lower bound (`fmt_a_indirection` reads only `uidx`, `expr.rs:319-327` — `arr[1:3]` → `arr[3]`); index opclass/COLLATE dropped (`fmt_index_elem`); `FOR UPDATE`/locking clauses dropped (`fmt_select_stmt`); function bodies always `$$`-wrapped (`stmt.rs:1477-1484`) — a body containing `$$` produces broken display.
- **Quadratic accumulation**: `fmt_case_expr` rebuilds `result = format!("{result} WHEN …")` per branch (`expr.rs:631,638`) — cosmetic perf, fix while touching.
- **No semantic tests**: the suite is exact-string snapshots; nothing asserts formatted output re-parses to the same AST.

The round-trip harness is the load-bearing piece: it converts "display fidelity" from whack-a-mole into an invariant.

## Current state

Excerpts verified at `dd0d79d` (post-033: files under `pg_query_fmt/src/stmt/`):

```rust
// expr.rs:158-163
AExprKind::AexprOp => match (&left, &right) {
    (Some(l), Some(r)) => Ok(format!("{l} {op_name} {r}")),
    ...
// expr.rs:250-256
BoolExprType::AndExpr => Ok(parts.join(" AND ")),
BoolExprType::OrExpr => Ok(format!("({})", parts.join(" OR "))),
BoolExprType::NotExpr => Ok(format!("NOT {}", parts.first().cloned().unwrap_or_default())),
// expr.rs:477-479
pub(crate) fn quote_identifier(ident: &str) -> String { ident.to_string() }
// stmt.rs:578
let header = format!("ALTER TABLE {relation}");
// stmt.rs:868-873 (distinct_clause non-empty → just "SELECT DISTINCT")
// stmt.rs:1477-1484 (" AS $$\n" … "$$ LANGUAGE ")
```

Fallback safety valve: unknown statement types already deparse via pg_query (`lib.rs:77-79`) — semantically exact. That is the correct escape for anything too hairy to format by hand.

## Commands you will need

| Purpose | Command | Expected |
|---|---|---|
| Crate tests | `cargo test -p pg_query_fmt --locked` | pass |
| Gate | fmt + clippy gate + `cargo test --workspace --locked` | exit 0 |

## Scope

**In scope**: `pg_query_fmt/src/{expr.rs, stmt/*, lib.rs tests}`.
**Out of scope**: `highlight.rs`/`preview.rs`; engine/CLI crates; deep recursion guard (LOW-confidence item — see Maintenance notes); making the formatter suitable for CODE GENERATION (it stays display-only; the round-trip test is a display-honesty check, not a codegen license).

## Git workflow

- Branch: `advisor/035-formatter-fidelity`
- Commit per fix area.
- No push/PR without operator instruction.

## Steps

### Step 1: Round-trip harness FIRST (it grades every later step)

New test in `pg_query_fmt/src/lib.rs` tests:

```rust
/// Formatted output must parse to the same AST fingerprint as the input.
/// (pg_query's parse-tree fingerprint is stable across whitespace/case.)
fn assert_round_trips(sql: &str) {
    let formatted = format_statement(sql).expect("format");
    let a = pg_query::fingerprint(sql).expect("fingerprint input").hex;
    let b = pg_query::fingerprint(&formatted)
        .unwrap_or_else(|e| panic!("formatted output failed to parse: {e}\n---\n{formatted}"))
        .hex;
    assert_eq!(a, b, "AST changed:\ninput:  {sql}\noutput: {formatted}");
}

#[test]
fn round_trip_corpus() {
    for sql in [
        "SELECT 1",
        "SELECT * FROM t WHERE NOT (a AND b)",
        "SELECT (a + b) * c FROM t",
        "CREATE TABLE \"MyTable\" (\"MyCol\" int, \"select\" text)",
        "ALTER INDEX idx_x RENAME TO idx_y",
        "SELECT DISTINCT ON (id) id, v FROM t ORDER BY id",
        "SELECT arr[1:3] FROM t",
        "SELECT id FROM t FOR UPDATE",
        "CREATE INDEX i ON t (lower(name) text_pattern_ops)",
        // + every statement already in the exact-string suite
    ] {
        assert_round_trips(sql);
    }
}
```

`pg_query::fingerprint` exists in pg_query 6.x (verify: `grep -rn "pub fn fingerprint" ~/.cargo/registry/src/*/pg_query-6.1.1/src/`). Run it — cases covering the bugs above will FAIL; that's the red baseline. Mark the failing inputs `// FIXME(step N)` and keep only currently-passing ones active, enabling each as its fix lands (or use `#[should_panic]` scaffolding — simpler: build the corpus incrementally per step).

**Verify**: harness compiles; passing subset green.

### Step 2: Precedence parens

Smallest honest fix (avoid a full precedence table): parenthesize **compound operands** — in `fmt_bool_expr`, wrap any `AND`/`NOT` argument that is itself a `BoolExpr` of a different type; in `AexprOp`, wrap left/right when the child is another `AexprOp` or `BoolExpr` (over-parenthesizing `a + b + c` → `(a + b) + c` is acceptable display noise; UNDER-parenthesizing is the bug). Implement via a helper `fmt_node_parenthesized_if_compound`.

**Verify**: enable the `NOT (a AND b)` and `(a+b)*c` corpus lines → round-trip green; existing exact-string tests updated ONLY where output legitimately gained parens (each such edit named in the commit).

### Step 3: Real `quote_identifier`

```rust
pub(crate) fn quote_identifier(ident: &str) -> String {
    let plain = !ident.is_empty()
        && ident.chars().next().is_some_and(|c| c.is_ascii_lowercase() || c == '_')
        && ident.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if plain && !is_reserved_keyword(ident) {
        ident.to_string()
    } else {
        format!("\"{}\"", ident.replace('"', "\"\""))
    }
}
```

`is_reserved_keyword`: reuse the keyword table already in `highlight.rs` (`SQL_KEYWORDS`) — move/share it (`pub(crate)`) rather than duplicating; it over-approximates (includes non-reserved keywords) which only causes harmless extra quoting. Apply `quote_identifier` also in `fmt_range_var` (`expr.rs:483-488`) and the name-list helpers (`stmt` module ~1528–1547).

**Verify**: `"MyTable"`/`"select"` corpus line green; snapshot updates named.

### Step 4: ALTER label + dropped clauses

- `fmt_alter_table`: derive the object keyword from `stmt.objtype` reusing `preview.rs`'s `object_type_label` (make it `pub(crate)`).
- `SELECT DISTINCT ON`: render `DISTINCT ON (expr, …)` from `distinct_clause` (non-empty nodes = ON list; a single NULL-ish empty node means plain DISTINCT — check pg_query's convention: plain DISTINCT is a list with one empty `Node {node: None}`; DISTINCT ON has real exprs. Handle both).
- Array slices: render `lidx:uidx` when `is_slice`/`lidx` present (`AIndices` has `is_slice: bool`, `lidx`, `uidx`).
- Index elem: append opclass (`name` list) and `COLLATE` when present.
- `FOR UPDATE`: render locking clause kinds from `locking_clause` (map the enum: UPDATE / NO KEY UPDATE / SHARE / KEY SHARE, plus NOWAIT/SKIP LOCKED).
- Function body `$$`: choose a dollar tag not contained in the body (`$$`, `$fn$`, `$fn1$`, …).
- While in `fmt_case_expr`: swap `result = format!("{result}…")` accumulation for `push_str` (also `fmt_a_indirection`).

Any sub-item that turns into a rabbit hole (AST shape doesn't match expectation): fall back to `node_deparse_fallback` for THAT construct and note it — the deparse valve is always semantically correct.

**Verify**: remaining corpus lines green; full crate suite green.

### Step 5: Full gate

fmt + clippy + workspace tests → green. Every exact-string snapshot change enumerated in commit messages.

## Test plan

- Round-trip corpus (grows per step; final state: all listed inputs + every legacy snapshot input round-trips).
- Existing exact-string tests: updated only where output deliberately improved; each update named.
- New unit tests: `quote_identifier` table (plain, mixed-case, reserved word, embedded quote), slice rendering, ALTER INDEX label.

## Done criteria

- [ ] `round_trip_corpus` covers ≥ 15 inputs incl. all bug cases, green
- [ ] `quote_identifier` is not a no-op; keyword table shared with highlight
- [ ] `grep -n "ALTER TABLE {relation}" pg_query_fmt/src/` → gone (objtype-driven)
- [ ] Gates green; `plans/README.md` updated

## STOP conditions

- `pg_query::fingerprint` absent/renamed in the locked version — find the equivalent (parse + compare normalized parse trees) or report.
- A fidelity fix requires >~40 lines for one construct — use the deparse fallback for it instead and record that choice.
- Snapshot churn exceeds what the named-updates rule can honestly describe — the change is too broad; split it.

## Maintenance notes

- The round-trip corpus is now the formatter's spec — every new statement formatter adds its inputs.
- Deep-recursion stack overflow on pathological nesting (audit item, LOW confidence) deliberately unaddressed: inputs are the operator's own migrations; the observer's `unwrap_or_else` fallback covers `Err` but not stack overflow — if it ever bites, add a depth counter returning `FormatError::Deparse` past ~200 levels.
- Display-only remains the contract: executed SQL is always the raw statement (`execute_*` use `stmt.sql`), never formatter output.
