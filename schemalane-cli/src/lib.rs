#![allow(clippy::print_stdout, clippy::print_stderr, clippy::future_not_send)]

use clap::builder::styling::{AnsiColor, Effects, Styles};
use clap::{Args, Parser, Subcommand, ValueEnum};
use comfy_table::{Attribute, Cell, CellAlignment, Color, ContentArrangement, Table, presets};
use deadpool_postgres::{ManagerConfig, Pool, RecyclingMethod};
use owo_colors::{OwoColorize, Stream, Style};
use rustls_platform_verifier::ConfigVerifierExt;
use schemalane_core::{
    MigrationFailed, MigrationFinished, MigrationObserver, MigrationStarted, MigrationState,
    SchemalaneConfig, SchemalaneError, SchemalaneMigrator, SqlStatementFailed,
    SqlStatementFinished, SqlStatementStarted, StatusEntry, StatusReport, init_migration_project,
    should_fail_on_pending,
};
use std::collections::BTreeSet;
use std::ffi::OsString;
use std::io::{BufRead, IsTerminal, Write as IoWrite};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::Instant;
use tokio_postgres::NoTls;

const DEFAULT_MIGRATION_DIR: &str = "./migration";
const DEFAULT_SQL_DIR: &str = "./migrations";

// ── Help styles ─────────────────────────────────────────────────────────────

const HELP_STYLES: Styles = Styles::styled()
    .header(AnsiColor::BrightGreen.on_default().effects(Effects::BOLD))
    .usage(AnsiColor::BrightGreen.on_default().effects(Effects::BOLD))
    .literal(AnsiColor::BrightCyan.on_default().effects(Effects::BOLD))
    .placeholder(AnsiColor::Cyan.on_default())
    .error(AnsiColor::Red.on_default().effects(Effects::BOLD))
    .valid(AnsiColor::BrightCyan.on_default().effects(Effects::BOLD))
    .invalid(AnsiColor::Yellow.on_default());

// ── Verbosity ───────────────────────────────────────────────────────────────

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
pub enum Verbosity {
    /// Migration file names only.
    #[default]
    Minimal,
    /// Summarized operations (e.g. CREATE TABLE name).
    Compact,
    /// Full SQL queries.
    Detailed,
}

use pg_query_fmt::highlight::highlight_sql_line;

// ── Formatting helpers ──────────────────────────────────────────────────────

const INDENT: &str = " ";
const MAX_PREVIEW_WIDTH: usize = 60;
const STATUS_WIDTH: usize = 7; // "SUCCESS".len() == "FAILED ".len()

/// Remove terminal control characters from file-derived text.
fn sanitize_terminal(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect()
}

fn pad_index(index: usize, total: usize) -> String {
    let width = total.to_string().len().max(2);
    format!("{index:0>width$}")
}

fn format_elapsed(ms: i32) -> String {
    if ms >= 1000 {
        let secs = f64::from(ms) / 1000.0;
        format!("{secs:.1} s")
    } else {
        format!("{ms} ms")
    }
}

fn truncate_preview(s: &str, max_width: usize) -> String {
    debug_assert!(max_width >= 3, "truncate_preview needs room for ellipsis");
    if s.chars().count() <= max_width {
        return s.to_owned();
    }
    let truncated: String = s.chars().take(max_width.saturating_sub(3)).collect();
    format!("{truncated}...")
}

// ── Interactive prompt ──────────────────────────────────────────────────────

fn prompt_yes_no(prompt: &str) -> Result<bool, SchemalaneError> {
    let stdin = std::io::stdin();
    let mut reader = stdin.lock();
    loop {
        eprint!("{prompt}");
        std::io::stderr().flush().map_err(SchemalaneError::Io)?;
        let mut answer = String::new();
        let read = reader.read_line(&mut answer).map_err(SchemalaneError::Io)?;
        if read == 0 {
            eprintln!();
            return Ok(false);
        }
        let trimmed = answer.trim();
        if trimmed.eq_ignore_ascii_case("yes") {
            return Ok(true);
        }
        if trimmed.eq_ignore_ascii_case("no") {
            return Ok(false);
        }
        eprintln!(
            "{}",
            "Please answer 'yes' or 'no'."
                .if_supports_color(Stream::Stderr, |text| text.bright_yellow())
        );
    }
}

// ── Postgres URL parsing ────────────────────────────────────────────────────

struct PostgresTarget {
    user: Option<String>,
    host: String,
    port: Option<u16>,
    database: String,
}

// ── Progress observer ───────────────────────────────────────────────────────

struct CliProgressObserver {
    verbosity: Verbosity,
    max_script_len: Mutex<usize>,
    last_error: Mutex<Option<String>>,
    planned_report: Mutex<Option<StatusReport>>,
}

impl CliProgressObserver {
    fn new(verbosity: Verbosity) -> Self {
        Self {
            verbosity,
            max_script_len: Mutex::new(0),
            last_error: Mutex::new(None),
            planned_report: Mutex::new(None),
        }
    }

    fn last_error(&self) -> Option<String> {
        self.last_error.lock().ok().and_then(|e| e.clone())
    }

    fn planned_report(&self) -> Option<StatusReport> {
        self.planned_report.lock().ok().and_then(|r| r.clone())
    }
}

impl MigrationObserver for CliProgressObserver {
    fn on_run_planned(&self, report: &StatusReport) {
        print_status_overview(report);
        print_pending_migrations(report);
        if let Ok(mut width) = self.max_script_len.lock() {
            *width = report
                .migrations
                .iter()
                .map(|entry| entry.script.len())
                .max()
                .unwrap_or(0);
        }
        if let Ok(mut planned) = self.planned_report.lock() {
            *planned = Some(report.clone());
        }
        eprintln!(
            "{}\n",
            "Migration Progress".if_supports_color(Stream::Stderr, |text| {
                text.style(Style::new().bold().bright_white())
            })
        );
    }

    fn on_migration_start(&self, event: &MigrationStarted) {
        if self.verbosity == Verbosity::Minimal {
            return;
        }

        let idx = pad_index(event.index, event.total);
        let total = pad_index(event.total, event.total);

        if event.index > 1 {
            eprintln!();
        }
        eprintln!(
            "[{idx}/{total}] {}",
            sanitize_terminal(&event.migration.script).if_supports_color(Stream::Stderr, |text| {
                text.style(Style::new().bold().bright_white())
            })
        );
    }

    fn on_migration_finish(&self, event: &MigrationFinished) {
        let idx = pad_index(event.index, event.total);
        let total = pad_index(event.total, event.total);
        let elapsed = format_elapsed(event.execution_time_ms);

        match self.verbosity {
            Verbosity::Minimal => {
                let padded = format!(
                    "{:<width$}",
                    sanitize_terminal(&event.migration.script),
                    width = self.max_script_len.lock().map_or(0, |width| *width)
                );
                eprintln!(
                    "[{idx}/{total}] {}     {} {}",
                    padded.if_supports_color(Stream::Stderr, |text| {
                        text.style(Style::new().bold().bright_white())
                    }),
                    format!("{:<STATUS_WIDTH$}", "SUCCESS")
                        .if_supports_color(Stream::Stderr, |text| {
                            text.style(Style::new().bright_green().bold())
                        }),
                    format!("({elapsed})")
                        .if_supports_color(Stream::Stderr, |text| text.bright_black())
                );
            }
            Verbosity::Compact => {
                eprintln!(
                    "{}{}",
                    INDENT,
                    format!("Total execution time: {elapsed}")
                        .if_supports_color(Stream::Stderr, |text| text.bright_black())
                );
            }
            Verbosity::Detailed => {
                eprintln!(
                    "{INDENT}{}",
                    format!("-- Total execution time: {elapsed}")
                        .if_supports_color(Stream::Stderr, |text| text.bright_black())
                );
            }
        }
    }

    fn on_migration_failed(&self, event: &MigrationFailed) {
        let idx = pad_index(event.index, event.total);
        let total = pad_index(event.total, event.total);
        let elapsed = format_elapsed(event.execution_time_ms);

        if let Ok(mut e) = self.last_error.lock() {
            *e = Some(event.error.clone());
        }

        if self.verbosity == Verbosity::Minimal {
            let padded = format!(
                "{:<width$}",
                sanitize_terminal(&event.migration.script),
                width = self.max_script_len.lock().map_or(0, |width| *width)
            );
            eprintln!(
                "[{idx}/{total}] {}     {} {}",
                padded.if_supports_color(Stream::Stderr, |text| {
                    text.style(Style::new().bold().bright_white())
                }),
                format!("{:<STATUS_WIDTH$}", "FAILED").if_supports_color(Stream::Stderr, |text| {
                    text.style(Style::new().bright_red().bold())
                }),
                format!("({elapsed})")
                    .if_supports_color(Stream::Stderr, |text| text.bright_black())
            );
        }
    }

    fn on_sql_statement_start(&self, event: &SqlStatementStarted) {
        if self.verbosity != Verbosity::Detailed {
            return;
        }

        let line_info = event
            .source_line
            .map_or_else(String::new, |l| format!(" (line: {l})"));
        let header = format!(
            "-- Query {} of {}{}",
            event.statement_index, event.total_statements, line_info
        );
        eprintln!(
            "{INDENT}{}",
            header.if_supports_color(Stream::Stderr, |text| text.bright_black())
        );
        let pretty = pg_query_fmt::format_statement(&event.statement)
            .unwrap_or_else(|_| event.statement.clone());
        for line in pretty.lines() {
            let sanitized = sanitize_terminal(line);
            eprintln!(
                "{INDENT}{}",
                sanitized.if_supports_color(Stream::Stderr, |value| highlight_sql_line(value))
            );
        }
    }

    fn on_sql_statement_finish(&self, event: &SqlStatementFinished) {
        let idx = pad_index(event.statement_index, event.total_statements);
        let total = pad_index(event.total_statements, event.total_statements);
        let elapsed = format_elapsed(event.execution_time_ms);

        match self.verbosity {
            Verbosity::Compact => {
                let preview = truncate_preview(
                    &sanitize_terminal(&event.statement_preview),
                    MAX_PREVIEW_WIDTH,
                );
                let padded_preview = format!("{preview:<MAX_PREVIEW_WIDTH$}");
                let index_str = format!("{idx}/{total}");
                eprintln!(
                    "{INDENT}{}    {}     {} {}",
                    index_str.if_supports_color(Stream::Stderr, |text| text.bright_black()),
                    padded_preview
                        .if_supports_color(Stream::Stderr, |value| highlight_sql_line(value)),
                    format!("{:<STATUS_WIDTH$}", "SUCCESS")
                        .if_supports_color(Stream::Stderr, |text| {
                            text.style(Style::new().bright_green().bold())
                        }),
                    format!("({elapsed})")
                        .if_supports_color(Stream::Stderr, |text| text.bright_black())
                );
            }
            Verbosity::Detailed => {
                eprintln!(
                    "{INDENT}{} {} {}",
                    "--".if_supports_color(Stream::Stderr, |text| text.bright_black()),
                    "SUCCESS".if_supports_color(Stream::Stderr, |text| {
                        text.style(Style::new().bright_green().bold())
                    }),
                    format!("({elapsed})")
                        .if_supports_color(Stream::Stderr, |text| text.bright_black())
                );
                eprintln!();
            }
            Verbosity::Minimal => {}
        }
    }

    fn on_sql_statement_failed(&self, event: &SqlStatementFailed) {
        let idx = pad_index(event.statement_index, event.total_statements);
        let total = pad_index(event.total_statements, event.total_statements);
        let elapsed = format_elapsed(event.execution_time_ms);

        if let Ok(mut e) = self.last_error.lock() {
            *e = Some(event.error.clone());
        }

        match self.verbosity {
            Verbosity::Compact => {
                let preview = truncate_preview(
                    &sanitize_terminal(&event.statement_preview),
                    MAX_PREVIEW_WIDTH,
                );
                let padded_preview = format!("{preview:<MAX_PREVIEW_WIDTH$}");
                let index_str = format!("{idx}/{total}");
                eprintln!(
                    "{INDENT}{}    {}     {} {}",
                    index_str.if_supports_color(Stream::Stderr, |text| text.bright_black()),
                    padded_preview
                        .if_supports_color(Stream::Stderr, |value| highlight_sql_line(value)),
                    format!("{:<STATUS_WIDTH$}", "FAILED")
                        .if_supports_color(Stream::Stderr, |text| {
                            text.style(Style::new().bright_red().bold())
                        }),
                    format!("({elapsed})")
                        .if_supports_color(Stream::Stderr, |text| text.bright_black())
                );
            }
            Verbosity::Detailed => {
                eprintln!(
                    "{INDENT}{} {} {}",
                    "--".if_supports_color(Stream::Stderr, |text| text.bright_black()),
                    format!("{:<STATUS_WIDTH$}", "FAILED")
                        .if_supports_color(Stream::Stderr, |text| {
                            text.style(Style::new().bright_red().bold())
                        }),
                    format!("({elapsed})")
                        .if_supports_color(Stream::Stderr, |text| text.bright_black())
                );
            }
            Verbosity::Minimal => {}
        }
    }
}

// ── Embedded runner ─────────────────────────────────────────────────────────

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

        let db_command: DbCommand = cli.command.into();
        let pool = connect_with_feedback(&cli.database_url, db_command.label()).await?;
        let migrations_dir = cli
            .dir
            .clone()
            .unwrap_or_else(|| PathBuf::from(self.migrations_dir));

        let config = SchemalaneConfig {
            schema: cli.schema,
            history_table: cli.history_table,
            migrations_dir,
            installed_by: cli.installed_by,
            advisory_lock_id: cli.advisory_lock_id,
        };

        let migrator = (self.build_migrator)(config);
        let verbosity = cli.verbosity.unwrap_or_default();
        run_db_command(&migrator, &pool, db_command, verbosity).await
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

#[derive(Debug, Parser)]
#[command(name = "schemalane")]
#[command(version)]
#[command(about = "Schemalane migration toolkit")]
#[command(styles = HELP_STYLES)]
struct Cli {
    #[command(subcommand)]
    command: RootCommand,
}

#[derive(Debug, Subcommand)]
enum RootCommand {
    /// Initialize a new migration crate.
    Init {
        #[arg(long, default_value = "./migration")]
        path: PathBuf,

        #[arg(long)]
        force: bool,
    },
    /// Run database migrations.
    Migrate(MigrateArgs),
}

#[derive(Debug, Args)]
#[command(disable_help_subcommand = true, long_about = None)]
struct MigrateArgs {
    /// Migration script directory.
    #[arg(
        short = 'd',
        long = "migration-dir",
        env = "MIGRATION_DIR",
        default_value = DEFAULT_MIGRATION_DIR
    )]
    migration_dir: PathBuf,

    #[arg(long, env = "DATABASE_URL")]
    database_url: Option<String>,

    #[arg(long, default_value = "public")]
    schema: String,

    #[arg(long, default_value = "flyway_schema_history")]
    history_table: String,

    #[arg(long)]
    installed_by: Option<String>,

    /// Override the advisory lock key (default: derived from schema and history table).
    #[arg(long)]
    advisory_lock_id: Option<i64>,

    /// Output verbosity level.
    #[arg(long, value_enum)]
    verbosity: Option<Verbosity>,

    #[command(subcommand)]
    command: Option<MigrateCommand>,
}

#[derive(Debug, Subcommand)]
enum MigrateCommand {
    /// Apply pending migrations (default).
    Up,
    /// Show migration status.
    Status {
        #[arg(long, value_enum, default_value_t = StatusFormat::Table)]
        format: StatusFormat,

        #[arg(long)]
        fail_on_pending: bool,
    },
    /// Drop all schemas and re-apply migrations.
    Fresh {
        /// Pass "yes" to confirm destructive schema drop.
        #[arg(long)]
        confirm: Option<String>,
    },
}

#[derive(Debug, Parser)]
#[command(styles = HELP_STYLES)]
struct EmbeddedCli {
    #[arg(long, env = "DATABASE_URL")]
    database_url: String,

    #[arg(long, default_value = "public")]
    schema: String,

    #[arg(long, default_value = "flyway_schema_history")]
    history_table: String,

    #[arg(long)]
    installed_by: Option<String>,

    /// Override the advisory lock key (default: derived from schema and history table).
    #[arg(long)]
    advisory_lock_id: Option<i64>,

    #[arg(long)]
    dir: Option<PathBuf>,

    /// Output verbosity level.
    #[arg(long, value_enum)]
    verbosity: Option<Verbosity>,

    #[command(subcommand)]
    command: EmbeddedCommand,
}

#[derive(Debug, Subcommand)]
enum EmbeddedCommand {
    /// Apply pending migrations (default).
    Up,
    /// Show migration status.
    Status {
        #[arg(long, value_enum, default_value_t = StatusFormat::Table)]
        format: StatusFormat,

        #[arg(long)]
        fail_on_pending: bool,
    },
    /// Drop all schemas and re-apply migrations.
    Fresh {
        /// Pass "yes" to confirm destructive schema drop.
        #[arg(long)]
        confirm: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum StatusFormat {
    Table,
    Json,
}

enum DbCommand {
    Up,
    Status {
        format: StatusFormat,
        fail_on_pending: bool,
    },
    Fresh {
        confirm: Option<String>,
    },
}

impl DbCommand {
    fn label(&self) -> &'static str {
        match self {
            Self::Up => "migrate up",
            Self::Status { .. } => "migrate status",
            Self::Fresh { .. } => "migrate fresh",
        }
    }
}

impl From<EmbeddedCommand> for DbCommand {
    fn from(command: EmbeddedCommand) -> Self {
        match command {
            EmbeddedCommand::Up => Self::Up,
            EmbeddedCommand::Status {
                format,
                fail_on_pending,
            } => Self::Status {
                format,
                fail_on_pending,
            },
            EmbeddedCommand::Fresh { confirm } => Self::Fresh { confirm },
        }
    }
}

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
        schema,
        history_table,
        installed_by,
        advisory_lock_id,
        verbosity,
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

    let db_command = match command {
        MigrateCommand::Up => DbCommand::Up,
        MigrateCommand::Status {
            format,
            fail_on_pending,
        } => DbCommand::Status {
            format,
            fail_on_pending,
        },
        MigrateCommand::Fresh { confirm } => DbCommand::Fresh { confirm },
    };

    let pool = connect_with_feedback(&database_url, db_command.label()).await?;

    let config = SchemalaneConfig {
        schema,
        history_table,
        migrations_dir: PathBuf::from(DEFAULT_SQL_DIR),
        installed_by,
        advisory_lock_id,
    };

    let migrator = SchemalaneMigrator::new(config);

    run_db_command(&migrator, &pool, db_command, verbosity).await
}

struct DelegationOptions<'a> {
    database_url: Option<&'a str>,
    schema: &'a str,
    history_table: &'a str,
    installed_by: Option<&'a str>,
    advisory_lock_id: Option<i64>,
    command: &'a MigrateCommand,
    verbosity: Verbosity,
}

fn run_via_migration_crate(
    manifest_path: &Path,
    options: &DelegationOptions<'_>,
) -> Result<(), SchemalaneError> {
    let (args, envs) = delegation_command_parts(manifest_path, options);
    let mut cargo = Command::new("cargo");
    cargo.args(args).envs(envs);

    let status = cargo.status().map_err(|err| {
        SchemalaneError::Io(std::io::Error::new(
            err.kind(),
            format!(
                "failed to run cargo for migration crate {}: {err}",
                manifest_path.display()
            ),
        ))
    })?;

    if status.success() {
        Ok(())
    } else {
        // The child emitted its error with a contract-compliant exit code.
        // Signal termination has no code and is a runtime failure.
        Err(SchemalaneError::Delegated {
            code: status.code().unwrap_or(1),
        })
    }
}

/// Build arguments and environment for delegated `cargo run`.
fn delegation_command_parts(
    manifest_path: &Path,
    options: &DelegationOptions<'_>,
) -> (Vec<OsString>, Vec<(&'static str, String)>) {
    let mut args = vec![
        OsString::from("run"),
        OsString::from("--manifest-path"),
        manifest_path.as_os_str().to_owned(),
        OsString::from("--"),
    ];
    let mut envs = Vec::new();

    // Deliver secrets via environment. Process arguments are world-readable.
    if let Some(database_url) = options.database_url {
        envs.push(("DATABASE_URL", database_url.to_owned()));
    }

    args.extend([
        OsString::from("--schema"),
        OsString::from(options.schema),
        OsString::from("--history-table"),
        OsString::from(options.history_table),
    ]);

    if let Some(installed_by) = options.installed_by {
        args.extend([
            OsString::from("--installed-by"),
            OsString::from(installed_by),
        ]);
    }

    if let Some(advisory_lock_id) = options.advisory_lock_id {
        args.extend([
            OsString::from("--advisory-lock-id"),
            OsString::from(advisory_lock_id.to_string()),
        ]);
    }

    args.extend([
        OsString::from("--verbosity"),
        OsString::from(match options.verbosity {
            Verbosity::Minimal => "minimal",
            Verbosity::Compact => "compact",
            Verbosity::Detailed => "detailed",
        }),
    ]);

    match options.command {
        MigrateCommand::Up => {
            args.push(OsString::from("up"));
        }
        MigrateCommand::Status {
            format,
            fail_on_pending,
        } => {
            args.extend([
                OsString::from("status"),
                OsString::from("--format"),
                OsString::from(match format {
                    StatusFormat::Table => "table",
                    StatusFormat::Json => "json",
                }),
            ]);
            if *fail_on_pending {
                args.push(OsString::from("--fail-on-pending"));
            }
        }
        MigrateCommand::Fresh { confirm } => {
            args.push(OsString::from("fresh"));
            if let Some(value) = confirm {
                args.extend([OsString::from("--confirm"), OsString::from(value)]);
            }
        }
    }
    (args, envs)
}

// ── Database connection ─────────────────────────────────────────────────────

fn create_pool(database_url: &str) -> Result<Pool, SchemalaneError> {
    let pg_config: tokio_postgres::Config = database_url.parse().map_err(|err| {
        SchemalaneError::Validation(format!("failed to parse database URL: {err}"))
    })?;

    let manager_config = ManagerConfig {
        recycling_method: RecyclingMethod::Fast,
    };
    let mgr = if wants_tls(&pg_config) {
        let tls_config = rustls::ClientConfig::with_platform_verifier().map_err(|err| {
            SchemalaneError::Validation(format!("failed to configure TLS verifier: {err}"))
        })?;
        let tls = tokio_postgres_rustls::MakeRustlsConnect::new(tls_config);
        deadpool_postgres::Manager::from_config(pg_config, tls, manager_config)
    } else {
        deadpool_postgres::Manager::from_config(pg_config, NoTls, manager_config)
    };

    Pool::builder(mgr).max_size(5).build().map_err(|err| {
        SchemalaneError::Validation(format!("failed to build connection pool: {err}"))
    })
}

fn wants_tls(config: &tokio_postgres::Config) -> bool {
    config.get_ssl_mode() != tokio_postgres::config::SslMode::Disable
}

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

fn format_postgres_target(database_url: &str) -> String {
    match parse_postgres_target(database_url) {
        Some(target) => {
            let user = target
                .user
                .as_deref()
                .map_or_else(String::new, |value| format!("{value}@"));
            let port = target
                .port
                .map_or_else(String::new, |value| format!(":{value}"));
            format!("{user}{}{port}/{}", target.host, target.database)
        }
        None => "<unparsed-url>".to_owned(),
    }
}

fn parse_postgres_target(database_url: &str) -> Option<PostgresTarget> {
    let without_scheme = database_url
        .strip_prefix("postgres://")
        .or_else(|| database_url.strip_prefix("postgresql://"))?;

    let (authority, path) = without_scheme.split_once('/')?;
    let database = path.split(['?', '#']).next()?.to_owned();
    if database.is_empty() {
        return None;
    }

    let (userinfo, hostport) = if let Some((user, host)) = authority.rsplit_once('@') {
        (Some(user), host)
    } else {
        (None, authority)
    };

    let user = userinfo
        .and_then(|raw| raw.split(':').next())
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);

    let (host, port) = parse_host_port(hostport)?;

    Some(PostgresTarget {
        user,
        host,
        port,
        database,
    })
}

fn parse_host_port(value: &str) -> Option<(String, Option<u16>)> {
    if let Some(stripped) = value.strip_prefix('[') {
        let (host, rest) = stripped.split_once(']')?;
        if rest.is_empty() {
            return Some((host.to_owned(), None));
        }
        let port = rest
            .strip_prefix(':')
            .and_then(|candidate| candidate.parse::<u16>().ok());
        return Some((host.to_owned(), port));
    }

    if let Some((host, port_text)) = value.rsplit_once(':')
        && !host.is_empty()
        && port_text.chars().all(|ch| ch.is_ascii_digit())
    {
        return Some((host.to_owned(), port_text.parse::<u16>().ok()));
    }

    Some((value.to_owned(), None))
}

// ── DB commands ─────────────────────────────────────────────────────────────

async fn run_db_command(
    migrator: &SchemalaneMigrator,
    pool: &Pool,
    command: DbCommand,
    verbosity: Verbosity,
) -> Result<(), SchemalaneError> {
    match command {
        DbCommand::Up => run_up_command(migrator, pool, verbosity).await?,
        DbCommand::Status {
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
                        SchemalaneError::Validation(format!("failed to encode JSON: {err}"))
                    })?
                ),
            }
            if fail_on_pending {
                should_fail_on_pending(&report)?;
            }
        }
        DbCommand::Fresh { confirm } => {
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
                print_error_diagnostics(&report, &err);
            }
            return Err(err);
        }
    };

    let _ = report;
    Ok(())
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
                print_error_diagnostics(&report, &err);
            }
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

fn print_status_overview(report: &StatusReport) {
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

fn print_pending_migrations(report: &StatusReport) {
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
        va.cmp(&vb).then_with(|| a.cmp(b))
    });
}

fn script_version_key(script: &str) -> Vec<u64> {
    let version_part = script
        .strip_prefix('V')
        .and_then(|rest| rest.split("__").next())
        .unwrap_or("");
    parse_version(version_part).unwrap_or_default()
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
    let mut numeric_versions: Vec<(Vec<u64>, i32, String)> = Vec::new();
    let mut fallback_versions: Vec<(i32, String)> = Vec::new();

    for entry in &report.migrations {
        if entry.installed_rank.is_none() {
            continue;
        }

        let Some(version) = entry.version.as_ref() else {
            continue;
        };
        let rank = entry.installed_rank.unwrap_or_default();
        if let Some(segments) = parse_version(version) {
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

fn parse_version(version: &str) -> Option<Vec<u64>> {
    let mut segments = Vec::new();
    for part in version.split(['.', '_']) {
        let Ok(value) = part.parse::<u64>() else {
            return None;
        };
        segments.push(value);
    }
    Some(segments)
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
        Cli, DEFAULT_MIGRATION_DIR, MigrateArgs, MigrateCommand, RootCommand, Verbosity,
        delegation_command_parts, format_postgres_target, latest_database_version,
        parse_postgres_target,
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
        let cli = Cli::try_parse_from(["schemalane", "migrate", "status"])
            .expect("CLI args should parse");
        let args = unwrap_migrate(cli);
        assert_eq!(args.migration_dir, PathBuf::from(DEFAULT_MIGRATION_DIR));
        assert!(matches!(args.command, Some(MigrateCommand::Status { .. })));
    }

    #[test]
    fn parse_migrate_without_subcommand() {
        let cli = Cli::try_parse_from(["schemalane", "migrate"]).expect("CLI args should parse");
        let args = unwrap_migrate(cli);
        assert_eq!(args.migration_dir, PathBuf::from(DEFAULT_MIGRATION_DIR));
        assert!(args.command.is_none(), "no subcommand means implicit up");
    }

    #[test]
    fn parse_verbosity_flag() {
        let cli = Cli::try_parse_from(["schemalane", "migrate", "--verbosity", "detailed", "up"])
            .expect("CLI args should parse");
        let args = unwrap_migrate(cli);
        assert_eq!(args.verbosity, Some(Verbosity::Detailed));
    }

    #[test]
    fn latest_database_version_ignores_pending_entries() {
        let report = StatusReport {
            schema: "public".to_owned(),
            history_table: "flyway_schema_history".to_owned(),
            migrations: vec![
                StatusEntry {
                    version: Some("18".to_owned()),
                    description: "old".to_owned(),
                    migration_type: "SQL".to_owned(),
                    script: "V18__old.sql".to_owned(),
                    checksum: Some(1),
                    installed_rank: Some(18),
                    installed_on: None,
                    execution_time_ms: Some(1),
                    state: MigrationState::Success,
                },
                StatusEntry {
                    version: Some("19".to_owned()),
                    description: "new".to_owned(),
                    migration_type: "SQL".to_owned(),
                    script: "V19__new.sql".to_owned(),
                    checksum: Some(2),
                    installed_rank: None,
                    installed_on: None,
                    execution_time_ms: None,
                    state: MigrationState::Pending,
                },
            ],
            summary: StatusSummary::default(),
        };

        assert_eq!(latest_database_version(&report), Some("18".to_owned()));
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
