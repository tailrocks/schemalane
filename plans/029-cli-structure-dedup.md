# Plan 029: Deduplicate the CLI's command enums, arg structs, and error rendering

> **Executor instructions**: Follow this plan step by step, verifying each step.
> On any STOP condition, stop and report. When done, update `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat dd0d79d..HEAD -- schemalane-cli/src/lib.rs`
> Locate by symbol; several earlier plans touched this file.

## Status

- **Priority**: P3
- **Effort**: M
- **Risk**: LOW-MED (clap surface must stay byte-compatible)
- **Depends on**: plans/023-cli-contract-tests.md (parse + delegation tests are the safety net)
- **Category**: tech-debt
- **Planned at**: commit `dd0d79d`, 2026-07-03

## Why this matters

Adding one subcommand or global flag to the CLI today means editing **three structurally identical command enums** (`MigrateCommand`, `EmbeddedCommand`, `DbCommand`), **two duplicated arg structs** (`MigrateArgs`, `EmbeddedCli` — same `schema`/`history_table`/`installed_by`/`verbosity`/`database_url` flags), two conversion sites, and two copies of the execution-error rendering block. Plan 014 (advisory-lock flag) had to touch all of them — the tax is already being paid. Direction spikes (validate/check/dry-run commands) will pay it again unless this lands first.

## Current state

(`schemalane-cli/src/lib.rs`)

- `MigrateCommand` (445–463), `EmbeddedCommand` (491–509), `DbCommand` (517–526): all `Up | Status { format, fail_on_pending } | Fresh { confirm }`; wired by `From<EmbeddedCommand> for DbCommand` (538–552) and an inline `MigrateCommand → DbCommand` match in `run_migrate_cli` (615–625).
- `MigrateArgs` (413–443) vs `EmbeddedCli` (465–489): overlapping flags; differences — `MigrateArgs` has `migration_dir` (+`MIGRATION_DIR` env) and `database_url: Option<String>`; `EmbeddedCli` has `dir: Option<PathBuf>` and `database_url: String` (required, env-backed).
- `run_cli`/`run_cli_with` (371–385) vs `EmbeddedRunner::run/run_with` (336–368): both do parse → connect → config → `run_db_command`.
- Duplicated error-render block: `run_up_command` (~900–913) vs `run_fresh_command` (~983–996) — "Execution Error" + last_error + `print_error_diagnostics` + return (plan 022 reshapes these; adapt).
- clap idioms in-repo: derive API, `#[command(flatten)]` unused so far; tests parse with `try_parse_from`.

## Commands you will need

| Purpose | Command | Expected |
|---|---|---|
| Grammar freeze | `cargo run -p schemalane-cli -- migrate --help > /tmp/help-before.txt` (before) and diff after | identical output |
| Gate | fmt + clippy gate + `cargo test --workspace --locked` | exit 0 |

## Scope

**In scope**: `schemalane-cli/src/lib.rs`.
**Out of scope**: flag renames/removals (grammar is frozen — help text byte-identical is the bar); core crate; module file-split (plan 032 — this plan reduces what 032 must move).

## Git workflow

- Branch: `advisor/029-cli-structure-dedup`
- Suggested commit: `Unify CLI command enums, shared args, and error rendering`
- No push/PR without operator instruction.

## Steps

### Step 1: Snapshot the grammar

`/tmp/help-before.txt` for: root `--help`, `migrate --help`, `migrate status --help`, `migrate fresh --help`; and the embedded CLI's help via a scratch invocation if practical (or skip — its surface is covered by parse tests).

### Step 2: One command enum

Keep `MigrateCommand` as THE enum (it already derives `Subcommand`). Delete `EmbeddedCommand` (point `EmbeddedCli.command` at `MigrateCommand`) and `DbCommand` (functions take `&MigrateCommand`; `label()` moves onto it). Delete both conversion sites. `DbCommand::label` semantics preserved (`"migrate up"` etc.).

### Step 3: Shared args via `#[command(flatten)]`

```rust
#[derive(Debug, Args)]
struct CommonDbArgs {
    #[arg(long, default_value = "public")]
    schema: String,
    #[arg(long, default_value = "flyway_schema_history")]
    history_table: String,
    #[arg(long)]
    installed_by: Option<String>,
    #[arg(long, value_enum)]
    verbosity: Option<Verbosity>,
    // + advisory_lock_id if plan 014 landed
}
```

`MigrateArgs` = `migration_dir` + `database_url: Option<String>` + `#[command(flatten)] common` + subcommand; `EmbeddedCli` = `database_url: String` + `dir` + flatten + subcommand. The `database_url` fields stay per-struct (required-ness differs — a real semantic difference, keep it).

### Step 4: One error-render helper

`fn report_execution_error(planned: Option<&StatusReport>, observer: &CliProgressObserver, err: &SchemalaneError)` replacing both blocks (shape depends on whether plan 022 landed — adapt to its captured-report form; pre-022, take `&StatusReport`).

### Step 5: Verify grammar frozen + gates

Regenerate the help outputs → `diff` against Step 1 snapshots → **identical**. Full test suite (incl. plan-023 parse/env/delegation tests) green. Manual smoke: `cargo run -p schemalane-cli -- migrate -d /nonexistent up` → same error text as before.

## Test plan

Plan 023's parse/env/delegation tests + the help-diff freeze are the net; no new tests needed (structure-only change).

## Done criteria

- [ ] `grep -c "enum .*Command" schemalane-cli/src/lib.rs` → 2 (`RootCommand`, `MigrateCommand`)
- [ ] `grep -n "flatten" schemalane-cli/src/lib.rs` → both CLIs flatten `CommonDbArgs`
- [ ] Help outputs byte-identical to snapshots; all tests green
- [ ] Only the CLI lib touched; `plans/README.md` updated

## STOP conditions

- Help diff shows ANY change (flag order, metavar, about text) — clap derive placement matters; fix placement, and if unfixable without grammar change, report.
- Plan 023 tests not present (dependency skipped) — write the parse tests first or STOP; refactoring the arg surface untested is how flags silently vanish.

## Maintenance notes

- New subcommands now touch: `MigrateCommand` + `run_db_command` + delegation arg builder — one enum, one dispatch, one forwarding site. Spikes 037/039/040 build on this.
- The `dir` vs `migration_dir` naming asymmetry between root and embedded CLIs is frozen grammar — unifying it is a breaking UX change; deliberately untouched.
