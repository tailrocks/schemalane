#![allow(clippy::print_stdout, clippy::print_stderr, clippy::future_not_send)]

use clap::Parser;
use comfy_table::{Attribute, Cell, CellAlignment, Color, ContentArrangement, Table, presets};
use deadpool_postgres::Pool;
use owo_colors::{OwoColorize, Stream, Style};
use schemalane_core::{
    MigrationState, SchemalaneConfig, SchemalaneError, SchemalaneMigrator, StatusEntry,
    StatusReport, init_migration_project, should_fail_on_pending,
};
use std::collections::BTreeSet;
use std::ffi::OsString;
use std::io::{IsTerminal, Write as IoWrite};
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::args::{
    Cli, CommonDbArgs, DEFAULT_MIGRATION_DIR, DEFAULT_SQL_DIR, EmbeddedCli, MigrateArgs,
    MigrateCommand, RootCommand, StatusFormat,
};
use crate::connect::{create_pool, format_postgres_target};

#[cfg(test)]
use crate::connect::{parse_postgres_target, wants_tls};

use crate::render::{Verbosity, sanitize_terminal};

#[cfg(test)]
use crate::render::truncate_preview;

use crate::prompt::prompt_yes_no;

// ── Progress observer ───────────────────────────────────────────────────────

use crate::observer::CliProgressObserver;
pub struct EmbeddedRunner {
    migrations_dir: &'static str,
    build_migrator: fn(SchemalaneConfig) -> SchemalaneMigrator,
}

impl EmbeddedRunner {
    pub fn new(
        migrations_dir: &'static str,
        build_migrator: fn(SchemalaneConfig) -> SchemalaneMigrator,
    ) -> Self {
        Self {
            migrations_dir,
            build_migrator,
        }
    }

    pub async fn run(self) {
        if let Err(err) = self.run_with(std::env::args_os()).await {
            eprintln!("{err}");
            std::process::exit(err.exit_code());
        }
    }

    pub async fn run_with<I, T>(self, args: I) -> Result<(), SchemalaneError>
    where
        I: IntoIterator<Item = T>,
        T: Into<OsString> + Clone,
    {
        let cli = EmbeddedCli::parse_from(args);

        let pool = connect_with_feedback(&cli.database_url, cli.command.label()).await?;
        let migrations_dir = cli
            .dir
            .clone()
            .unwrap_or_else(|| PathBuf::from(self.migrations_dir));

        let config = SchemalaneConfig::new()
            .with_schema(cli.common.schema)
            .with_history_table(cli.common.history_table)
            .with_migrations_dir(migrations_dir)
            .with_installed_by(cli.common.installed_by)
            .with_advisory_lock_id(cli.common.advisory_lock_id);

        let migrator = (self.build_migrator)(config);
        let verbosity = cli.common.verbosity.unwrap_or_default();
        run_db_command(&migrator, &pool, cli.command, verbosity).await
    }
}

pub async fn run_cli() {
    if let Err(err) = run_cli_with(std::env::args_os()).await {
        eprintln!("{err}");
        std::process::exit(err.exit_code());
    }
}

pub async fn run_cli_with<I, T>(args: I) -> Result<(), SchemalaneError>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let cli = Cli::parse_from(args);
    run_root_cli(cli).await
}

// ── CLI argument definitions ────────────────────────────────────────────────

// ── CLI command dispatch ────────────────────────────────────────────────────

async fn run_root_cli(cli: Cli) -> Result<(), SchemalaneError> {
    match cli.command {
        RootCommand::Init { path, force } => {
            let report = init_migration_project(&path, force)?;
            println!("Initialized migration crate at {}", report.root.display());
            println!(
                "Created {} file(s), overwritten {} file(s).",
                report.created.len(),
                report.overwritten.len()
            );
            println!("Run migrations via:");
            println!(
                "DATABASE_URL=\"postgres://…\" cargo run --manifest-path {}/Cargo.toml -- up",
                report.root.display()
            );
            Ok(())
        }
        RootCommand::Migrate(args) => run_migrate_cli(args).await,
    }
}

async fn run_migrate_cli(args: MigrateArgs) -> Result<(), SchemalaneError> {
    let MigrateArgs {
        migration_dir,
        database_url,
        common:
            CommonDbArgs {
                schema,
                history_table,
                installed_by,
                advisory_lock_id,
                verbosity,
            },
        command,
    } = args;
    let command = command.unwrap_or(MigrateCommand::Up);
    let verbosity = verbosity.unwrap_or_default();

    let manifest_path = migration_dir.join("Cargo.toml");
    if manifest_path.is_file() {
        return run_via_migration_crate(
            &manifest_path,
            &DelegationOptions {
                database_url: database_url.as_deref(),
                schema: &schema,
                history_table: &history_table,
                installed_by: installed_by.as_deref(),
                advisory_lock_id,
                command: &command,
                verbosity,
            },
        );
    }
    if migration_dir != Path::new(DEFAULT_MIGRATION_DIR) {
        return Err(SchemalaneError::Validation(format!(
            "migration crate manifest not found: {}",
            manifest_path.display()
        )));
    }

    let database_url = database_url.ok_or_else(|| {
        SchemalaneError::Validation(
            "--database-url (or DATABASE_URL env var) is required for this command".to_owned(),
        )
    })?;

    let pool = connect_with_feedback(&database_url, command.label()).await?;

    let config = SchemalaneConfig::new()
        .with_schema(schema)
        .with_history_table(history_table)
        .with_migrations_dir(DEFAULT_SQL_DIR)
        .with_installed_by(installed_by)
        .with_advisory_lock_id(advisory_lock_id);

    let migrator = SchemalaneMigrator::new(config);

    run_db_command(&migrator, &pool, command, verbosity).await
}

use crate::delegate::{DelegationOptions, run_via_migration_crate};

#[cfg(test)]
use crate::delegate::delegation_command_parts;

async fn connect_with_feedback(
    database_url: &str,
    command_label: &str,
) -> Result<Pool, SchemalaneError> {
    let target = format_postgres_target(database_url);

    print_branding(command_label);

    eprint!(
        "Connecting to PostgreSQL {}... ",
        target.if_supports_color(Stream::Stderr, |text| text.bright_white())
    );
    std::io::stderr().flush().ok();

    let started = Instant::now();
    let pool = create_pool(database_url)?;

    // Verify the connection by acquiring one
    match pool.get().await {
        Ok(_client) => {
            let ms = started.elapsed().as_millis();
            eprintln!(
                "{} {}",
                "SUCCESS".if_supports_color(Stream::Stderr, |text| {
                    text.style(Style::new().green().bold())
                }),
                format!("({ms} ms)").if_supports_color(Stream::Stderr, |text| text.bright_black())
            );
            eprintln!();
            Ok(pool)
        }
        Err(err) => {
            let ms = started.elapsed().as_millis();
            eprintln!(
                "{} {}",
                "FAILED".if_supports_color(Stream::Stderr, |text| {
                    text.style(Style::new().red().bold())
                }),
                format!("({ms} ms)").if_supports_color(Stream::Stderr, |text| text.bright_black())
            );
            Err(SchemalaneError::Pool(err))
        }
    }
}

// ── DB commands ─────────────────────────────────────────────────────────────

async fn run_db_command(
    migrator: &SchemalaneMigrator,
    pool: &Pool,
    command: MigrateCommand,
    verbosity: Verbosity,
) -> Result<(), SchemalaneError> {
    match command {
        MigrateCommand::Up => run_up_command(migrator, pool, verbosity).await?,
        MigrateCommand::Status {
            format,
            fail_on_pending,
        } => {
            let report = migrator.status(pool).await?;
            match format {
                StatusFormat::Table => {
                    print_status_overview(&report);
                    print_status_table(&report);
                }
                StatusFormat::Json => println!(
                    "{}",
                    serde_json::to_string_pretty(&report).map_err(|err| {
                        SchemalaneError::Internal(format!("failed to encode JSON: {err}"))
                    })?
                ),
            }
            if fail_on_pending {
                should_fail_on_pending(&report)?;
            }
        }
        MigrateCommand::Fresh { confirm } => {
            run_fresh_command(migrator, pool, confirm.as_deref(), verbosity).await?;
        }
    }

    Ok(())
}

async fn run_up_command(
    migrator: &SchemalaneMigrator,
    pool: &Pool,
    verbosity: Verbosity,
) -> Result<(), SchemalaneError> {
    let observer = CliProgressObserver::new(verbosity);
    let report = match migrator.up_with_observer(pool, &observer).await {
        Ok(report) => report,
        Err(err) => {
            report_execution_error(&observer, &err);
            return Err(err);
        }
    };

    let _ = report;
    Ok(())
}

fn report_execution_error(observer: &CliProgressObserver, err: &SchemalaneError) {
    eprintln!();
    eprintln!(
        "{}",
        "Execution Error".if_supports_color(Stream::Stderr, |text| {
            text.style(Style::new().bright_red().bold())
        })
    );
    if let Some(last_error) = observer.last_error() {
        eprintln!(
            "{}",
            last_error.if_supports_color(Stream::Stderr, |text| text.bright_black())
        );
    } else {
        eprintln!(
            "{}",
            format!("{err}").if_supports_color(Stream::Stderr, |text| text.bright_black())
        );
    }
    if let Some(report) = observer.planned_report() {
        print_error_diagnostics(&report, err);
    }
}

async fn run_fresh_command(
    migrator: &SchemalaneMigrator,
    pool: &Pool,
    confirm: Option<&str>,
    verbosity: Verbosity,
) -> Result<(), SchemalaneError> {
    // Show DANGEROUS warning
    eprintln!(
        "{}",
        "DANGEROUS: This will drop the target schema (CASCADE), destroying every object in it, then re-apply migrations."
            .if_supports_color(Stream::Stderr, |text| {
                text.style(Style::new().bright_red().bold())
            })
    );
    eprintln!();

    eprintln!(
        "{}",
        "Schema to drop:".if_supports_color(Stream::Stderr, |text| {
            text.style(Style::new().bright_white().bold())
        })
    );
    eprintln!(
        " - {}",
        sanitize_terminal(&migrator.config().schema)
            .if_supports_color(Stream::Stderr, |text| text.bright_yellow())
    );
    eprintln!();

    // Determine confirmation
    let confirmed = match confirm {
        Some(value) if value.eq_ignore_ascii_case("yes") => true,
        Some(_) => {
            eprintln!(
                "{}",
                "Invalid --confirm value. Pass --confirm yes to proceed."
                    .if_supports_color(Stream::Stderr, |text| text.bright_red())
            );
            return Err(SchemalaneError::FreshRequiresConfirm);
        }
        None => {
            // No --confirm flag: try interactive prompt
            let stdin = std::io::stdin();
            if !stdin.is_terminal() {
                eprintln!(
                    "{}",
                    "Non-interactive terminal detected. Use --confirm yes to confirm."
                        .if_supports_color(Stream::Stderr, |text| text.bright_red())
                );
                return Err(SchemalaneError::FreshRequiresConfirm);
            }
            prompt_yes_no("Are you sure you want to continue? (yes/no): ")?
        }
    };

    if !confirmed {
        eprintln!(
            "{}",
            "Aborted.".if_supports_color(Stream::Stderr, |text| text.bright_yellow())
        );
        return Ok(());
    }

    let observer = CliProgressObserver::new(verbosity);
    let report = match migrator.fresh_with_observer(pool, true, &observer).await {
        Ok(report) => report,
        Err(err) => {
            report_execution_error(&observer, &err);
            return Err(err);
        }
    };

    let _ = report;
    Ok(())
}

// ── Display helpers ─────────────────────────────────────────────────────────

fn print_branding(command: &str) {
    eprintln!();
    eprintln!(
        "{} {}",
        "SCHEMALANE".if_supports_color(Stream::Stderr, |text| {
            text.style(Style::new().bright_cyan().bold())
        }),
        env!("CARGO_PKG_VERSION").if_supports_color(Stream::Stderr, |text| text.bright_black())
    );
    eprintln!(
        "{}",
        "PostgreSQL Migration Lane".if_supports_color(Stream::Stderr, |text| text.bright_blue())
    );
    eprintln!(
        "{} {}",
        "Command:".if_supports_color(Stream::Stderr, |text| text.bright_black()),
        command.if_supports_color(Stream::Stderr, |text| text.bright_white())
    );
    eprintln!();
}

pub(crate) fn print_status_overview(report: &StatusReport) {
    eprintln!(
        "{} {}",
        "Schema:".if_supports_color(Stream::Stderr, |text| text.bright_black()),
        sanitize_terminal(&report.schema)
            .if_supports_color(Stream::Stderr, |text| text.bright_white())
    );
    eprintln!(
        "{} {}",
        "History table:".if_supports_color(Stream::Stderr, |text| text.bright_black()),
        sanitize_terminal(&report.history_table)
            .if_supports_color(Stream::Stderr, |text| text.bright_white())
    );
    eprintln!(
        "{} {}",
        "Database version:".if_supports_color(Stream::Stderr, |text| text.bright_black()),
        database_version_label(latest_database_version(report).as_deref())
            .if_supports_color(Stream::Stderr, |text| text.bright_green())
    );

    let s = &report.summary;
    let mut parts = Vec::new();
    if s.success > 0 {
        parts.push(format!("success={}", s.success));
    }
    if s.pending > 0 {
        parts.push(format!("pending={}", s.pending));
    }
    if s.failed > 0 {
        parts.push(format!("failed={}", s.failed));
    }
    if s.missing > 0 {
        parts.push(format!("missing={}", s.missing));
    }
    if s.checksum_mismatch > 0 {
        parts.push(format!("checksum_mismatch={}", s.checksum_mismatch));
    }
    if !parts.is_empty() {
        eprintln!(
            "{} {}",
            "Status:".if_supports_color(Stream::Stderr, |text| text.bright_black()),
            parts.join(" ")
        );
    }
    eprintln!();
}

fn state_cell(state: MigrationState) -> Cell {
    let label = format!("{state:?}").to_ascii_uppercase();
    let color = match state {
        MigrationState::Success => Color::Green,
        MigrationState::Pending => Color::Yellow,
        MigrationState::Failed | MigrationState::Missing | MigrationState::ChecksumMismatch => {
            Color::Red
        }
        _ => Color::Red,
    };
    Cell::new(label).fg(color).add_attribute(Attribute::Bold)
}

fn type_cell(migration_type: &str) -> Cell {
    match migration_type {
        "SQL" => Cell::new(migration_type).fg(Color::Cyan),
        "RUST" => Cell::new(migration_type).fg(Color::Magenta),
        _ => Cell::new(migration_type),
    }
}

fn print_status_table(report: &StatusReport) {
    let mut table = Table::new();
    table
        .load_preset(presets::UTF8_FULL)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(vec![
            Cell::new("Version")
                .set_alignment(CellAlignment::Right)
                .fg(Color::DarkGrey)
                .add_attribute(Attribute::Bold),
            Cell::new("Description")
                .fg(Color::DarkGrey)
                .add_attribute(Attribute::Bold),
            Cell::new("Type")
                .fg(Color::DarkGrey)
                .add_attribute(Attribute::Bold),
            Cell::new("Script")
                .fg(Color::DarkGrey)
                .add_attribute(Attribute::Bold),
            Cell::new("State")
                .fg(Color::DarkGrey)
                .add_attribute(Attribute::Bold),
            Cell::new("Rank")
                .set_alignment(CellAlignment::Right)
                .fg(Color::DarkGrey)
                .add_attribute(Attribute::Bold),
            Cell::new("Time (ms)")
                .set_alignment(CellAlignment::Right)
                .fg(Color::DarkGrey)
                .add_attribute(Attribute::Bold),
        ]);

    for m in &report.migrations {
        let version = m.version.as_deref().unwrap_or("-");
        let rank = m
            .installed_rank
            .map_or_else(|| "-".to_owned(), |v| v.to_string());
        let time = m
            .execution_time_ms
            .map_or_else(|| "-".to_owned(), |v| v.to_string());

        table.add_row(vec![
            Cell::new(version).set_alignment(CellAlignment::Right),
            Cell::new(sanitize_terminal(&m.description)),
            type_cell(&m.migration_type),
            Cell::new(sanitize_terminal(&m.script))
                .fg(Color::White)
                .add_attribute(Attribute::Bold),
            state_cell(m.state),
            Cell::new(&rank).set_alignment(CellAlignment::Right),
            Cell::new(&time).set_alignment(CellAlignment::Right),
        ]);
    }

    println!("{table}");
}

pub(crate) fn print_pending_migrations(report: &StatusReport) {
    let pending: Vec<&StatusEntry> = report
        .migrations
        .iter()
        .filter(|entry| entry.state == MigrationState::Pending)
        .collect();

    eprintln!(
        "{} {}",
        "Pending migrations:".if_supports_color(Stream::Stderr, |text| text.bright_black()),
        pending.len()
    );
    if pending.is_empty() {
        eprintln!(
            "{}",
            "Database is already at the latest version for this crate."
                .if_supports_color(Stream::Stderr, |text| text.bright_green())
        );
    } else {
        for migration in pending {
            eprintln!("  - {}", sanitize_terminal(&migration.script));
        }
    }
    eprintln!();
}

fn print_error_diagnostics(report: &StatusReport, err: &SchemalaneError) {
    if matches!(
        err,
        SchemalaneError::Drift(_) | SchemalaneError::FailedHistory(_)
    ) {
        print_drift_details(report);
    }
}

fn sort_scripts_by_version(scripts: &mut [String]) {
    scripts.sort_by(|a, b| {
        let va = script_version_key(a);
        let vb = script_version_key(b);
        match (&va, &vb) {
            (Some(left), Some(right)) => left.cmp(right),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        }
        .then_with(|| a.cmp(b))
    });
}

fn script_version_key(script: &str) -> Option<schemalane_version::ParsedVersion> {
    let version_part = script.strip_prefix('V')?.split("__").next()?;
    schemalane_version::ParsedVersion::parse(version_part).ok()
}

fn print_drift_details(report: &StatusReport) {
    eprintln!();
    eprintln!(
        "{}",
        "Drift Diagnostics".if_supports_color(Stream::Stderr, |text| {
            text.style(Style::new().bright_red().bold())
        })
    );
    eprintln!(
        "{} {}",
        "Database version:".if_supports_color(Stream::Stderr, |text| text.bright_black()),
        database_version_label(latest_database_version(report).as_deref())
            .if_supports_color(Stream::Stderr, |text| text.bright_green())
    );

    let local_scripts: BTreeSet<String> = report
        .migrations
        .iter()
        .filter(|entry| entry.state != MigrationState::Missing)
        .map(|entry| entry.script.clone())
        .collect();
    let applied_scripts: BTreeSet<String> = report
        .migrations
        .iter()
        .filter(|entry| entry.installed_rank.is_some())
        .map(|entry| entry.script.clone())
        .collect();

    let mut only_in_database: Vec<String> = applied_scripts
        .difference(&local_scripts)
        .cloned()
        .collect();
    sort_scripts_by_version(&mut only_in_database);

    let mut only_in_crate: Vec<String> = local_scripts
        .difference(&applied_scripts)
        .cloned()
        .collect();
    sort_scripts_by_version(&mut only_in_crate);

    let mut checksum_mismatch: Vec<String> = report
        .migrations
        .iter()
        .filter(|entry| entry.state == MigrationState::ChecksumMismatch)
        .map(|entry| entry.script.clone())
        .collect();
    sort_scripts_by_version(&mut checksum_mismatch);

    let mut failed_scripts: Vec<String> = report
        .migrations
        .iter()
        .filter(|entry| entry.state == MigrationState::Failed)
        .map(|entry| entry.script.clone())
        .collect();
    sort_scripts_by_version(&mut failed_scripts);

    eprintln!(
        "{}",
        "Files only in database history:"
            .if_supports_color(Stream::Stderr, |text| text.bright_black())
    );
    if only_in_database.is_empty() {
        eprintln!("  - <none>");
    } else {
        for script in only_in_database {
            eprintln!(
                "  - {}",
                sanitize_terminal(&script)
                    .if_supports_color(Stream::Stderr, |text| text.bright_red())
            );
        }
    }

    eprintln!(
        "{}",
        "Files only in local migration crate:"
            .if_supports_color(Stream::Stderr, |text| text.bright_black())
    );
    if only_in_crate.is_empty() {
        eprintln!("  - <none>");
    } else {
        for script in only_in_crate {
            eprintln!(
                "  - {}",
                sanitize_terminal(&script)
                    .if_supports_color(Stream::Stderr, |text| text.bright_yellow())
            );
        }
    }

    eprintln!(
        "{}",
        "Checksum mismatches:".if_supports_color(Stream::Stderr, |text| text.bright_black())
    );
    if checksum_mismatch.is_empty() {
        eprintln!("  - <none>");
    } else {
        for script in checksum_mismatch {
            eprintln!(
                "  - {}",
                sanitize_terminal(&script)
                    .if_supports_color(Stream::Stderr, |text| text.bright_red())
            );
        }
    }

    eprintln!(
        "{}",
        "Failed history entries:".if_supports_color(Stream::Stderr, |text| text.bright_black())
    );
    if failed_scripts.is_empty() {
        eprintln!("  - <none>");
    } else {
        for script in failed_scripts {
            eprintln!(
                "  - {}",
                sanitize_terminal(&script)
                    .if_supports_color(Stream::Stderr, |text| text.bright_red())
            );
        }
    }
    eprintln!();
}

fn latest_database_version(report: &StatusReport) -> Option<String> {
    let mut numeric_versions: Vec<(schemalane_version::ParsedVersion, i32, String)> = Vec::new();
    let mut fallback_versions: Vec<(i32, String)> = Vec::new();

    for entry in &report.migrations {
        if entry.installed_rank.is_none() {
            continue;
        }

        let Some(version) = entry.version.as_ref() else {
            continue;
        };
        let rank = entry.installed_rank.unwrap_or_default();
        if let Ok(segments) = schemalane_version::ParsedVersion::parse(version) {
            numeric_versions.push((segments, rank, version.clone()));
        }
        fallback_versions.push((rank, version.clone()));
    }

    if !numeric_versions.is_empty() {
        numeric_versions.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        return numeric_versions
            .last()
            .map(|(_, _, version)| version.clone());
    }

    if !fallback_versions.is_empty() {
        fallback_versions.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        return fallback_versions.last().map(|(_, version)| version.clone());
    }

    None
}

fn database_version_label(version: Option<&str>) -> String {
    match version {
        Some(version) => format!("V{version}"),
        None => "empty".to_owned(),
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::{
        Cli, DEFAULT_MIGRATION_DIR, DelegationOptions, MigrateArgs, MigrateCommand, RootCommand,
        StatusFormat, Verbosity, delegation_command_parts, format_postgres_target,
        latest_database_version, parse_postgres_target,
    };
    use clap::Parser;
    use schemalane_core::{MigrationState, StatusEntry, StatusReport, StatusSummary};
    use std::path::{Path, PathBuf};

    fn unwrap_migrate(cli: Cli) -> MigrateArgs {
        match cli.command {
            RootCommand::Migrate(args) => args,
            other @ RootCommand::Init { .. } => panic!("expected Migrate, got {other:?}"),
        }
    }

    #[test]
    fn parse_init_command() {
        let cli = Cli::try_parse_from(["schemalane", "init"]).expect("CLI args should parse");
        assert!(matches!(cli.command, RootCommand::Init { .. }));
    }

    #[test]
    fn delegation_never_puts_database_url_in_argv() {
        let options = super::DelegationOptions {
            database_url: Some("postgres://u:pw-classified@h/db"),
            schema: "public",
            history_table: "flyway_schema_history",
            installed_by: None,
            advisory_lock_id: None,
            command: &MigrateCommand::Up,
            verbosity: Verbosity::Minimal,
        };
        let (args, envs) = delegation_command_parts(Path::new("./m/Cargo.toml"), &options);
        assert!(
            args.iter()
                .all(|arg| !arg.to_string_lossy().contains("pw-classified"))
        );
        assert!(
            envs.iter()
                .any(|(key, value)| *key == "DATABASE_URL" && value.contains("pw-classified"))
        );
    }

    fn delegated_args(command: &MigrateCommand) -> (Vec<String>, Vec<(&'static str, String)>) {
        let options = DelegationOptions {
            database_url: Some("postgres://u:secret@h/db"),
            schema: "tenant",
            history_table: "history",
            installed_by: Some("tester"),
            advisory_lock_id: Some(42),
            command,
            verbosity: Verbosity::Minimal,
        };
        let (args, envs) = delegation_command_parts(Path::new("./m/Cargo.toml"), &options);
        (
            args.into_iter()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect(),
            envs,
        )
    }

    #[test]
    fn delegation_up_forwards_all_options_in_order() {
        let (args, envs) = delegated_args(&MigrateCommand::Up);
        assert_eq!(
            args,
            [
                "run",
                "--manifest-path",
                "./m/Cargo.toml",
                "--",
                "--schema",
                "tenant",
                "--history-table",
                "history",
                "--installed-by",
                "tester",
                "--advisory-lock-id",
                "42",
                "--verbosity",
                "minimal",
                "up",
            ]
        );
        assert_eq!(
            envs,
            [("DATABASE_URL", "postgres://u:secret@h/db".to_owned())]
        );
        assert!(!args.iter().any(|arg| arg.contains("postgres://")));
    }

    #[test]
    fn delegation_status_forwards_json_and_pending_gate() {
        let command = MigrateCommand::Status {
            format: StatusFormat::Json,
            fail_on_pending: true,
        };
        let (args, _) = delegated_args(&command);
        assert!(args.ends_with(&[
            "status".to_owned(),
            "--format".to_owned(),
            "json".to_owned(),
            "--fail-on-pending".to_owned(),
        ]));
    }

    #[test]
    fn delegation_fresh_forwards_confirmation() {
        let command = MigrateCommand::Fresh {
            confirm: Some("yes".to_owned()),
        };
        let (args, _) = delegated_args(&command);
        assert!(args.ends_with(&["fresh".to_owned(), "--confirm".to_owned(), "yes".to_owned()]));
    }

    #[test]
    fn status_json_shape_is_stable() {
        let report = StatusReport::new(
            "public".into(),
            "flyway_schema_history".into(),
            vec![StatusEntry::new(
                Some("1".into()),
                "init".into(),
                "SQL".into(),
                "V1__init.sql".into(),
                Some(-559_038_737),
                Some(1),
                Some("2026-01-01 00:00:00".into()),
                Some(12),
                MigrationState::Success,
            )],
            StatusSummary::new(1, 0, 0, 0, 0),
        );
        let value = serde_json::to_value(&report).expect("serialize");
        let entry = &value["migrations"][0];
        assert_eq!(entry["type"], "SQL");
        assert_eq!(entry["state"], "Success");
        assert_eq!(entry["checksum"], -559_038_737);
        assert_eq!(value["summary"]["checksum_mismatch"], 0);
        let keys: Vec<&str> = entry
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            [
                "checksum",
                "description",
                "execution_time_ms",
                "installed_on",
                "installed_rank",
                "script",
                "state",
                "type",
                "version"
            ]
        );
    }

    #[test]
    fn migration_state_json_spellings_are_stable() {
        for (state, expected) in [
            (MigrationState::Success, "\"Success\""),
            (MigrationState::Pending, "\"Pending\""),
            (MigrationState::Failed, "\"Failed\""),
            (MigrationState::Missing, "\"Missing\""),
            (MigrationState::ChecksumMismatch, "\"ChecksumMismatch\""),
        ] {
            assert_eq!(serde_json::to_string(&state).expect("serialize"), expected);
        }
    }

    #[test]
    fn database_url_flag_beats_env() {
        temp_env::with_var("DATABASE_URL", Some("postgres://env@h/db"), || {
            let args = unwrap_migrate(
                Cli::try_parse_from([
                    "schemalane",
                    "migrate",
                    "--database-url",
                    "postgres://flag@h/db",
                    "up",
                ])
                .expect("parse"),
            );
            assert_eq!(args.database_url.as_deref(), Some("postgres://flag@h/db"));
        });
    }

    #[test]
    fn database_url_env_used_when_flag_absent() {
        temp_env::with_var("DATABASE_URL", Some("postgres://env@h/db"), || {
            let args = unwrap_migrate(
                Cli::try_parse_from(["schemalane", "migrate", "up"]).expect("parse"),
            );
            assert_eq!(args.database_url.as_deref(), Some("postgres://env@h/db"));
        });
    }

    #[test]
    fn migration_dir_env_beats_default() {
        temp_env::with_var("MIGRATION_DIR", Some("/tmp/envdir"), || {
            let args = unwrap_migrate(
                Cli::try_parse_from(["schemalane", "migrate", "up"]).expect("parse"),
            );
            assert_eq!(args.migration_dir, PathBuf::from("/tmp/envdir"));
        });
    }

    #[test]
    fn truncate_preview_is_char_boundary_safe() {
        let short = "é".repeat(30);
        assert_eq!(super::truncate_preview(&short, 60), short);

        let long = "é".repeat(100);
        let out = super::truncate_preview(&long, 60);
        assert!(out.ends_with("..."));
        assert_eq!(out.chars().count(), 60);
    }

    #[test]
    fn tls_mode_selection() {
        let disable: tokio_postgres::Config = "postgres://u@h/db?sslmode=disable"
            .parse()
            .expect("disable config");
        let prefer: tokio_postgres::Config = "postgres://u@h/db".parse().expect("prefer config");
        let require: tokio_postgres::Config = "postgres://u@h/db?sslmode=require"
            .parse()
            .expect("require config");
        assert!(!super::wants_tls(&disable));
        assert!(super::wants_tls(&prefer));
        assert!(super::wants_tls(&require));
    }

    #[test]
    fn parse_short_migration_dir_flag() {
        let cli = Cli::try_parse_from(["schemalane", "migrate", "-d", "test2/migration", "up"])
            .expect("CLI args should parse");
        let args = unwrap_migrate(cli);
        assert_eq!(args.migration_dir, PathBuf::from("test2/migration"));
        assert!(matches!(args.command, Some(MigrateCommand::Up)));
    }

    #[test]
    fn parse_default_migration_dir() {
        temp_env::with_var("MIGRATION_DIR", None::<&str>, || {
            let cli = Cli::try_parse_from(["schemalane", "migrate", "status"])
                .expect("CLI args should parse");
            let args = unwrap_migrate(cli);
            assert_eq!(args.migration_dir, PathBuf::from(DEFAULT_MIGRATION_DIR));
            assert!(matches!(args.command, Some(MigrateCommand::Status { .. })));
        });
    }

    #[test]
    fn parse_migrate_without_subcommand() {
        temp_env::with_var("MIGRATION_DIR", None::<&str>, || {
            let cli =
                Cli::try_parse_from(["schemalane", "migrate"]).expect("CLI args should parse");
            let args = unwrap_migrate(cli);
            assert_eq!(args.migration_dir, PathBuf::from(DEFAULT_MIGRATION_DIR));
            assert!(args.command.is_none(), "no subcommand means implicit up");
        });
    }

    #[test]
    fn parse_verbosity_flag() {
        let cli = Cli::try_parse_from(["schemalane", "migrate", "--verbosity", "detailed", "up"])
            .expect("CLI args should parse");
        let args = unwrap_migrate(cli);
        assert_eq!(args.common.verbosity, Some(Verbosity::Detailed));
    }

    #[test]
    fn latest_database_version_ignores_pending_entries() {
        let report = StatusReport::new(
            "public".to_owned(),
            "flyway_schema_history".to_owned(),
            vec![
                StatusEntry::new(
                    Some("18".to_owned()),
                    "old".to_owned(),
                    "SQL".to_owned(),
                    "V18__old.sql".to_owned(),
                    Some(1),
                    Some(18),
                    None,
                    Some(1),
                    MigrationState::Success,
                ),
                StatusEntry::new(
                    Some("19".to_owned()),
                    "new".to_owned(),
                    "SQL".to_owned(),
                    "V19__new.sql".to_owned(),
                    Some(2),
                    None,
                    None,
                    None,
                    MigrationState::Pending,
                ),
            ],
            StatusSummary::default(),
        );

        assert_eq!(latest_database_version(&report), Some("18".to_owned()));
    }

    #[test]
    fn latest_database_version_supports_arbitrarily_large_parts() {
        let huge = "99999999999999999999999999999999999999";
        let report = StatusReport::new(
            "public".to_owned(),
            "history".to_owned(),
            vec![
                StatusEntry::new(
                    Some("10".to_owned()),
                    "small".to_owned(),
                    "SQL".to_owned(),
                    "V10__small.sql".to_owned(),
                    None,
                    Some(1),
                    None,
                    Some(1),
                    MigrationState::Success,
                ),
                StatusEntry::new(
                    Some(huge.to_owned()),
                    "large".to_owned(),
                    "SQL".to_owned(),
                    format!("V{huge}__large.sql"),
                    None,
                    Some(2),
                    None,
                    Some(1),
                    MigrationState::Success,
                ),
            ],
            StatusSummary::default(),
        );
        assert_eq!(latest_database_version(&report).as_deref(), Some(huge));
    }

    #[test]
    fn parse_postgres_target_from_standard_url() {
        let parsed = parse_postgres_target("postgres://chainargos:secret@localhost:40000/test3")
            .expect("should parse postgres url");

        assert_eq!(parsed.user.as_deref(), Some("chainargos"));
        assert_eq!(parsed.host, "localhost");
        assert_eq!(parsed.port, Some(40000));
        assert_eq!(parsed.database, "test3");
    }

    #[test]
    fn format_postgres_target_hides_password() {
        assert_eq!(
            format_postgres_target("postgres://chainargos:secret@localhost:40000/test3"),
            "chainargos@localhost:40000/test3"
        );
    }
}
