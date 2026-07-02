# Plan 011: Fix four small diagnostic-path bugs (UTF-8 truncation panic, EOF prompt loop, masked failure errors, off-by-whitespace line numbers)

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat dd0d79d..HEAD -- schemalane-cli/src/lib.rs schemalane-core/src/lib.rs`
> If any in-scope file changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding; on a
> mismatch, treat it as a STOP condition. (Plans 003/004/007 legitimately touch
> the same files; locate the excerpts by symbol name, not line number.)

## Status

- **Priority**: P2
- **Effort**: S
- **Risk**: LOW
- **Depends on**: none
- **Category**: bug
- **Planned at**: commit `dd0d79d`, 2026-07-03

## Why this matters

Four independently small defects, all on diagnostic paths, all cheap to fix:

- **(A)** `truncate_preview` slices a UTF-8 string at a byte offset. A statement preview over 60 bytes with a multi-byte character straddling byte 57 panics ("byte index … is not a char boundary") and **aborts an in-progress `up`/`fresh`** under `--verbosity compact`. Previews can carry arbitrary UTF-8 (identifiers, string literals, raw-SQL fallback).
- **(B)** The interactive `fresh` confirmation loops forever on stdin EOF (Ctrl-D): `read_line` returns `Ok(0)`, the empty answer matches neither yes nor no, and the loop re-prints the prompt in a tight spin.
- **(C)** When a migration fails AND the follow-up failed-history-row insert also fails, the insert error replaces the real migration error (`?` before the observer callback), and `on_migration_failed` is never emitted — the tool mis-reports the very failure it exists to report. Same pattern duplicated in `fresh`.
- **(D)** Reported SQL statement line numbers (`source_line` in observer events and the `MixedStatements { line }` error) are computed from the **untrimmed** split slice, whose leading whitespace/newlines belong to the previous statement's tail — so 2nd..nth statements report a line one-or-more too low. The trimmed slice pointing at the real first token is already available.

## Current state

**(A)** `schemalane-cli/src/lib.rs:72-78`:

```rust
fn truncate_preview(s: &str, max_width: usize) -> String {
    if s.len() <= max_width {
        s.to_owned()
    } else {
        format!("{}...", &s[..max_width - 3])
    }
}
```

Callers: `on_sql_statement_finish` (line 257) and `on_sql_statement_failed` (line 294), both with `MAX_PREVIEW_WIDTH` = 60.

**(B)** `schemalane-cli/src/lib.rs:101-118`:

```rust
fn prompt_yes_no(prompt: &str) -> Result<bool, SchemalaneError> {
    let stdin = std::io::stdin();
    let mut reader = stdin.lock();
    loop {
        print!("{prompt}");
        std::io::stdout().flush().map_err(SchemalaneError::Io)?;
        let mut answer = String::new();
        reader.read_line(&mut answer).map_err(SchemalaneError::Io)?;
        let trimmed = answer.trim();
        if trimmed.eq_ignore_ascii_case("yes") { return Ok(true); }
        if trimmed.eq_ignore_ascii_case("no") { return Ok(false); }
        println!("{}", "Please answer 'yes' or 'no'.".bright_yellow());
    }
}
```

**(C)** `schemalane-core/src/lib.rs:470-501` (in `up_with_observer`; mirrored at 597–626 in `fresh_with_observer`):

```rust
Err(err) => {
    let error_message = err.to_string();

    // MixedStatements is a validation error — do not record a
    // failed history row because the migration never executed.
    if !matches!(err, SchemalaneError::MixedStatements { .. }) {
        self.insert_history_row(&client, migration, &installed_by, execution_time_ms, false)
            .await?;                       // <-- masks `err` if this fails
    }

    observer.on_migration_failed(&MigrationFailed { … error: error_message … });

    return Err(match err { … });
}
```

**(D)** `schemalane-core/src/lib.rs:1242` inside `parse_sql_migration`:

```rust
let source_line = offset_to_line(sql, stmt_sql);
```

where `trimmed` (`= stmt_sql.trim()`, line 1233) is the slice that starts at the statement's first token; and `offset_to_line` (1263–1267) counts `\n` before the slice's start pointer. `pg_query::split_with_parser` returns borrowed sub-slices of `sql` (verified against pg_query 6.1.1 source: `statements.push(&query[start..end])`), so pointer arithmetic is sound for both slices — passing `trimmed` is strictly more accurate.

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Format check | `cargo fmt --all -- --check` | exit 0 |
| Lint | `cargo clippy --workspace --locked --all-targets --all-features -- -D warnings` | exit 0 |
| Unit tests | `cargo test --workspace --locked` | pass |

## Scope

**In scope** (the only files you should modify):
- `schemalane-cli/src/lib.rs` (A, B + their tests)
- `schemalane-core/src/lib.rs` (C, D + their tests)

**Out of scope** (do NOT touch, even though they look related):
- Display-width vs byte-width refinement for East-Asian characters — char-boundary safety is the bug; width aesthetics are polish.
- The up/fresh loop **duplication** itself (`plans/028-engine-dedup.md`) — fix C in both copies, don't unify here.
- stdout/stderr routing (plan 004).

## Git workflow

- Branch: `advisor/011-small-cli-core-bug-fixes`
- Suggested commit: `Fix preview truncation panic, EOF prompt loop, masked failure errors, statement line numbers`
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1 (A): Char-boundary-safe truncation

Replace `truncate_preview`:

```rust
fn truncate_preview(s: &str, max_width: usize) -> String {
    debug_assert!(max_width >= 3, "truncate_preview needs room for the ellipsis");
    if s.chars().count() <= max_width {
        return s.to_owned();
    }
    let truncated: String = s.chars().take(max_width.saturating_sub(3)).collect();
    format!("{truncated}...")
}
```

Add tests to the CLI test module:

```rust
#[test]
fn truncate_preview_is_char_boundary_safe() {
    // 30 × 'é' (2 bytes each) = 60 bytes, 30 chars — was a panic candidate.
    let s = "é".repeat(30);
    assert_eq!(super::truncate_preview(&s, 60), s); // 30 chars <= 60
    let long = "é".repeat(100);
    let out = super::truncate_preview(&long, 60);
    assert!(out.ends_with("..."));
    assert_eq!(out.chars().count(), 60); // 57 chars + "..."
}
```

**Verify**: `cargo test -p schemalane-cli --locked truncate_preview` → pass.

### Step 2 (B): Treat EOF as "no"

In `prompt_yes_no`, capture the read size and bail on 0:

```rust
let n = reader.read_line(&mut answer).map_err(SchemalaneError::Io)?;
if n == 0 {
    // EOF (Ctrl-D / closed stdin): treat as declining.
    eprintln!();
    return Ok(false);
}
```

(`eprintln!()` if plan 004 landed; `println!()` otherwise — match the function's current stream.)

**Verify**: `printf '' | cargo run -p schemalane-cli -- migrate fresh 2>&1 | head -5; echo exit=$?` — must terminate (any exit code; the point is no infinite loop). Note: with stdin piped, `is_terminal()` at the call site short-circuits before the prompt, so this manual check exercises the guard only indirectly; the code change is trivially total (every branch returns). Rely on review + the guard's totality.

### Step 3 (C): Never let the history-insert error mask the migration error

Design constraints that force the shape below: core is a library (workspace lints deny/warn `print_stderr`, so no `eprintln!` from core); the returned error's **variant** determines the exit code (so the migration error must be returned unchanged, not re-wrapped as `Validation`); the observer event must always fire.

In BOTH `up_with_observer` (lines ~470–501) and `fresh_with_observer` (lines ~597–626), restructure the `Err(err)` arm to exactly this order — attempt the insert without `?`, fold any insert failure into the event's error string, always emit the event, always return the mapped migration error:

```rust
Err(err) => {
    let mut error_message = err.to_string();

    // MixedStatements is a validation error — do not record a
    // failed history row because the migration never executed.
    if !matches!(err, SchemalaneError::MixedStatements { .. })
        && let Err(insert_err) = self
            .insert_history_row(&client, migration, &installed_by, execution_time_ms, false)
            .await
    {
        // Secondary failure: surface it in the event, never mask `err`.
        error_message =
            format!("{error_message} (additionally: failed to record failed history row: {insert_err})");
    }

    observer.on_migration_failed(&MigrationFailed {
        migration: migration_info,
        index: applied_index,          // `index + 1` in fresh_with_observer
        total: total_to_apply,
        execution_time_ms,
        error: error_message,
    });

    return Err(match err {
        SchemalaneError::Db(source) => SchemalaneError::MigrationExecution {
            script: migration.script.clone(),
            source,
        },
        other => other,
    });
}
```

Apply the identical treatment to both copies (they are deliberate duplicates until `plans/028-engine-dedup.md`).

**Verify**: `cargo clippy -p schemalane-core --locked --all-targets -- -D warnings` → exit 0; `cargo test --workspace --locked` → pass; if Docker available: `cargo test -p schemalane-core --locked --test postgres_integration -- --include-ignored` → all pass (failure-path tests `rust_migration_transaction_mode_rolls_back_on_failure` etc. cover the reordering).

### Step 4 (D): Use the trimmed slice for line numbers

In `parse_sql_migration` change line 1242:

```rust
let source_line = offset_to_line(sql, trimmed);
```

Add a unit test next to the existing `parse_sql_migration_*` tests:

```rust
#[test]
fn parse_sql_migration_reports_statement_line_numbers() {
    let sql = "SELECT 1;\n\n\nSELECT 2;\n";
    let stmts = parse_sql_migration(sql).expect("parse");
    assert_eq!(stmts.len(), 2);
    assert_eq!(stmts[0].source_line, 1);
    assert_eq!(stmts[1].source_line, 4, "second statement starts on line 4");
}
```

**Verify**: `cargo test -p schemalane-core --locked reports_statement_line` → pass.

### Step 5: Full gate

**Verify**: fmt + clippy + `cargo test --workspace --locked` exit 0.

## Test plan

- (A) `truncate_preview_is_char_boundary_safe` — the panic regression.
- (D) `parse_sql_migration_reports_statement_line_numbers` — locks correct lines.
- (B) totality by construction + manual pipe check.
- (C) covered by existing failure-path integration tests; the new event-error enrichment is asserted implicitly (events still fire).

## Done criteria

- [ ] `grep -n "max_width - 3\]" schemalane-cli/src/lib.rs` → no byte-slice truncation left
- [ ] `grep -n "offset_to_line(sql, trimmed)" schemalane-core/src/lib.rs` → present
- [ ] Both `Err(err)` arms (up + fresh) contain no bare `?` on `insert_history_row`
- [ ] New tests pass; fmt/clippy/workspace green
- [ ] Only the two in-scope files modified
- [ ] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- `pg_query::split_with_parser` in the locked version no longer returns borrowed slices (check `Cargo.lock` for a pg_query major bump) — (D)'s pointer arithmetic assumption dies with that; report instead of patching.
- Step 3's reordering makes any existing integration test fail — the failure-row semantics regressed; report the diff between expected/actual history rows.
- The up/fresh error arms were already unified by plan 028 — apply the fix once at the unified site and note it.

## Maintenance notes

- (C) leaves the two copies of the error arm in place (unification is plan 028); until then, changes to one MUST be mirrored — reviewers should diff the two arms.
- (A) counts chars, not display columns; if column-accurate truncation is ever wanted, use a width crate — deliberately out of scope.
- (D) makes `MixedStatements { line }` accurate too — user-facing error messages change slightly (better); no compat concern.
