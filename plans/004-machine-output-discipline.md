# Plan 004: Make stdout machine-clean — JSON unpolluted, human chrome on stderr, ANSI gated on TTY, control chars sanitized

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat dd0d79d..HEAD -- schemalane-cli/src/lib.rs schemalane-cli/Cargo.toml pg_query_fmt/src/highlight.rs`
> If any in-scope file changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding; on a
> mismatch, treat it as a STOP condition.

## Status

- **Priority**: P1
- **Effort**: M
- **Risk**: LOW
- **Depends on**: none
- **Category**: bug
- **Planned at**: commit `dd0d79d`, 2026-07-03

## Why this matters

Three related output defects break machine consumption and terminal hygiene:

1. `schemalane migrate status --format json | jq` fails: before the JSON, the CLI prints a version banner, "PostgreSQL Migration Lane", "Command: …", and "Connecting to PostgreSQL … SUCCESS (N ms)" — all to **stdout**.
2. ANSI color codes are emitted unconditionally (no TTY detection, no `NO_COLOR` support), so piped/redirected/CI output — including the text surrounding that JSON — contains raw `ESC[…m` sequences. (comfy-table's own cell colors auto-disable off-TTY; the owo-colors call sites do not.)
3. File-derived text (migration script names, descriptions, raw SQL statement lines) is echoed to the terminal verbatim; a migration set containing control bytes (e.g. an ESC inside a string literal, or a crafted filename — descriptions are "everything after `__`" with no character whitelist) can inject terminal escape sequences at whoever runs `status`/`up`.

The structural fix for 1+2 is the Unix convention: **stdout carries only the command's payload** (status table, JSON document, init report); progress, branding, prompts, and diagnostics go to stderr; color is applied only when the destination stream is a terminal and `NO_COLOR` is unset.

## Current state

All in `schemalane-cli/src/lib.rs` unless noted.

- Crate-level allow (line 1): `#![allow(clippy::print_stdout, clippy::print_stderr, clippy::future_not_send)]` — `println!`/`print!` are used freely.
- Chrome printed to stdout:
  - `print_branding` (lines 1004–1014): banner via `println!`.
  - `connect_with_feedback` (lines 740–776): `print!("Connecting to PostgreSQL {}… ", …)` + `SUCCESS/FAILED` via `println!`.
  - `print_status_overview` (1016–1054), `print_pending_migrations` (1133–1152), "Migration Progress" headers (896, 979), fresh warning + schema list (929–948), `print_error_diagnostics`/`print_drift_details` (1154–1265) — all `println!`.
  - The whole `CliProgressObserver` (lines 151–316) prints progress via `println!`.
- The JSON path (lines 868–873):

  ```rust
  StatusFormat::Json => println!(
      "{}",
      serde_json::to_string_pretty(&report).map_err(|err| {
          SchemalaneError::Validation(format!("failed to encode JSON: {err}"))
      })?
  ),
  ```

- Colors: `use owo_colors::OwoColorize;` (line 7) with direct `.bright_green()`, `.bold()`, etc. at ~60 call sites. `schemalane-cli/Cargo.toml:24`: `owo-colors = "4.3.0"` (no features). The only `IsTerminal` use is on **stdin** for the fresh prompt (line 963).
- Highlighter: `pg_query_fmt/src/highlight.rs`, `highlight_sql_line` (lines 186–253) — colors keywords/strings/etc. and copies everything else through verbatim, including C0 control bytes (comment branch line 195, string branch 203, fallback char branch 247–249). Called from the observer at lines 246, 263, 298.
- Statement previews can be raw SQL: `pg_query_fmt/src/preview.rs:27-32` falls back to `parsed.truncate(96)` — arbitrary file content.
- `schemalane-core` types: the observer events carry `script`, `description`, `statement`, `statement_preview` strings straight from files (`schemalane-core/src/lib.rs:194-227`).

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Format check | `cargo fmt --all -- --check` | exit 0 |
| Lint | `cargo clippy --workspace --locked --all-targets --all-features -- -D warnings` | exit 0 |
| Unit tests | `cargo test --workspace --locked` | pass |

## Scope

**In scope** (the only files you should modify):
- `schemalane-cli/src/lib.rs`
- `schemalane-cli/Cargo.toml` (owo-colors feature)
- `pg_query_fmt/src/highlight.rs` (control-char handling only)

**Out of scope** (do NOT touch, even though they look related):
- `schemalane-core` observer event definitions — sanitization happens at print time in the CLI, not by mutating engine data.
- comfy-table configuration — it already handles TTY detection for its own cell styling.
- The status **table** rendering logic and JSON schema — payload content must not change, only its cleanliness.
- Exit codes / error types (plan 003).

## Git workflow

- Branch: `advisor/004-machine-output-discipline`
- Suggested commit: `Route CLI chrome to stderr, gate ANSI on TTY, sanitize control chars`
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: Enable owo-colors auto stream detection

In `schemalane-cli/Cargo.toml` change:

```toml
owo-colors = { version = "4.3.0", features = ["supports-colors"] }
```

**Verify**: `cargo check -p schemalane-cli --locked` → exit 0.

### Step 2: Introduce two tiny print helpers and a sanitizer

Near the formatting helpers (around line 52), add:

```rust
use owo_colors::{OwoColorize, Stream};

/// Remove C0/C1 control characters (except '\n' and '\t') from text that
/// originated in migration files or filenames before echoing it to a
/// terminal, so a hostile migration set cannot inject escape sequences.
fn sanitize_terminal(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect()
}
```

Convention for color from here on: every colored fragment goes through
`value.if_supports_color(Stream::Stderr, |t| t.bright_green())` (or
`Stream::Stdout` for payload-side tables) instead of bare `.bright_green()`.
owo-colors' `supports-colors` feature makes this honor both TTY detection and
`NO_COLOR`.

**Verify**: `cargo check -p schemalane-cli --locked` → exit 0.

### Step 3: Move chrome to stderr

Mechanically convert `println!`/`print!` → `eprintln!`/`eprint!` in exactly these functions (payload printers stay on stdout):

- `print_branding` (1004)
- `connect_with_feedback` (both the `print!` and the SUCCESS/FAILED lines, incl. `stdout().flush()` → `stderr().flush()`)
- `print_status_overview`, `print_pending_migrations`
- the two "Migration Progress" headers in `run_up_command` / `run_fresh_command`
- the fresh warning + schema list + "Aborted."/"Invalid --confirm" messages in `run_fresh_command`
- `prompt_yes_no` prompt output (keep reading from stdin)
- `print_error_diagnostics` / `print_drift_details`
- every `println!` in `impl MigrationObserver for CliProgressObserver`
- the "Execution Error" blocks in `run_up_command`/`run_fresh_command`

Keep on **stdout**: `print_status_table` (the table payload), the JSON `println!`, and the `init` success output in `run_root_cli` (lines 560–570).

While editing each site, wrap its color calls per the Step 2 convention with the matching stream (`Stream::Stderr` for all of the above; `Stream::Stdout` inside `print_status_table`'s surrounding `println!("{table}")` needs no wrapping — comfy-table handles itself).

**Verify**:
- `cargo clippy -p schemalane-cli --locked --all-targets -- -D warnings` → exit 0
- Smoke: `cargo run -p schemalane-cli -- migrate --database-url postgres://nouser@localhost:1/none status --format json 2>/dev/null; echo "exit=$?"` → stdout is **empty** (connection fails before JSON; the point is no banner on stdout) and exit is nonzero. And `… 2>&1 >/dev/null | head -3` shows the banner went to stderr.

### Step 4: Sanitize file-derived text at print sites

In `CliProgressObserver` methods and `print_pending_migrations`/`print_drift_details`/`print_status_table`, route these values through `sanitize_terminal(...)` before printing:

- `event.migration.script`
- `event.statement_preview`
- each `line` printed from `pretty.lines()` in `on_sql_statement_start` (sanitize the line **before** passing to `highlight_sql_line`)
- `entry.script` / `entry.description` in table + drift listings

**Verify**: `cargo test --workspace --locked` → pass.

### Step 5: Defense-in-depth in the highlighter

In `pg_query_fmt/src/highlight.rs`, `highlight_sql_line`: in the final fallback branch (lines 246–249), skip control characters:

```rust
// Consume the next char (drop control chars — terminal safety)
let c = remaining.chars().next().unwrap();
if !c.is_control() || c == '\t' {
    result.push(c);
}
pos += c.len_utf8();
```

Add a unit test in the existing `mod tests` (line 283). Assertion subtlety: when the highlighter colors a keyword its OWN output legitimately contains ESC, so the test input must avoid keywords/strings/comments entirely — feed a line that hits only the fallback char branch:

```rust
#[test]
fn strips_control_characters() {
    // OSC title-set sequence; no SQL tokens, so only the fallback
    // char-by-char branch runs and no legitimate ANSI is produced.
    let out = highlight_sql_line("\u{1b}]0;evil\u{7}");
    assert!(!out.contains('\u{1b}'));
    assert!(!out.contains('\u{7}'));
}
```

(Note: the string-literal and comment branches copy spans verbatim — the CLI-side `sanitize_terminal` in Step 4 covers those; this step only hardens the char-by-char path. Do not attempt span-level filtering here.)

**Verify**: `cargo test -p pg_query_fmt --locked strips_control` → 1 passed.

### Step 6: Full gate + JSON purity proof

**Verify**:
- fmt/clippy/tests all exit 0.
- If a local Postgres is available (or after `docker run -d -p 5433:5432 -e POSTGRES_PASSWORD=pg postgres:17`): create a scratch migrations dir `mkdir -p /tmp/ms && printf 'CREATE TABLE t1 (id int);\n' > /tmp/ms/V1__t.sql`, then from a directory whose `./migrations` is that dir (or rely on default-dir semantics: run from a parent containing `migrations/`):
  `cargo run -p schemalane-cli -- migrate --database-url postgres://postgres:pg@localhost:5433/postgres status --format json 2>/dev/null | python3 -m json.tool > /dev/null && echo JSON_OK` → prints `JSON_OK`.
  (No Docker/DB → skip; note it in the plan status update.)

## Test plan

- `strips_control_characters` in `pg_query_fmt/src/highlight.rs` (Step 5).
- Manual JSON purity proof (Step 6).
- Existing CLI unit tests (arg parsing, URL formatting) must remain green — they don't touch printing.
- Full JSON-shape contract tests are `plans/023-cli-contract-tests.md`, not here.

## Done criteria

Machine-checkable. ALL must hold:

- [ ] `grep -n "println!" schemalane-cli/src/lib.rs` shows remaining stdout prints ONLY in: `print_status_table`, the JSON arm, and `run_root_cli`'s init output
- [ ] `grep -n "if_supports_color" schemalane-cli/src/lib.rs | wc -l` ≥ 20 (chrome color sites gated)
- [ ] `grep -n "sanitize_terminal" schemalane-cli/src/lib.rs` → definition + ≥5 call sites
- [ ] fmt/clippy/`cargo test --workspace --locked` exit 0
- [ ] Only the three in-scope files modified
- [ ] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- The observer/print functions have been restructured since `dd0d79d` (e.g. plan 029/032 landed first) — the mechanical site list above is then wrong; re-derive it or stop.
- `owo-colors` 4.3 `supports-colors` feature does not expose `if_supports_color`/`Stream` as described (API drift) — report the actual API; do not hand-roll a global mutable flag without approval.
- You find yourself editing `schemalane-core` beyond reading — sanitization belongs CLI-side per Scope.

## Maintenance notes

- Convention going forward: **stdout = payload, stderr = chrome**. New commands must follow it; reviewer should reject any new `println!` that isn't payload.
- `--verbosity` progress now lands on stderr; anyone previously scraping progress from stdout must switch (call out in release notes).
- Deferred: a `--color=auto|always|never` flag (owo-colors honors `NO_COLOR`/TTY already; an explicit flag is polish).
- Deferred: span-level control-char filtering inside highlighter string/comment branches — CLI-side sanitize covers the real path; doing both would double-process.
