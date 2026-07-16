use std::path::PathBuf;

use clap::builder::styling::{AnsiColor, Effects, Styles};
use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::Verbosity;

pub(crate) const DEFAULT_MIGRATION_DIR: &str = "./migration";
pub(crate) const DEFAULT_SQL_DIR: &str = "./migrations";

const HELP_STYLES: Styles = Styles::styled()
    .header(AnsiColor::BrightGreen.on_default().effects(Effects::BOLD))
    .usage(AnsiColor::BrightGreen.on_default().effects(Effects::BOLD))
    .literal(AnsiColor::BrightCyan.on_default().effects(Effects::BOLD))
    .placeholder(AnsiColor::Cyan.on_default())
    .error(AnsiColor::Red.on_default().effects(Effects::BOLD))
    .valid(AnsiColor::BrightCyan.on_default().effects(Effects::BOLD))
    .invalid(AnsiColor::Yellow.on_default());

#[derive(Debug, Parser)]
#[command(name = "schemalane", version, about = "Schemalane migration toolkit", styles = HELP_STYLES)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: RootCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum RootCommand {
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
pub(crate) struct MigrateArgs {
    /// Migration script directory.
    #[arg(short = 'd', long = "migration-dir", env = "MIGRATION_DIR", default_value = DEFAULT_MIGRATION_DIR)]
    pub(crate) migration_dir: PathBuf,
    #[arg(long, env = "DATABASE_URL")]
    pub(crate) database_url: Option<String>,
    #[command(flatten)]
    pub(crate) common: CommonDbArgs,
    #[command(subcommand)]
    pub(crate) command: Option<MigrateCommand>,
}

#[derive(Debug, Args)]
pub(crate) struct CommonDbArgs {
    #[arg(long, default_value = "public")]
    pub(crate) schema: String,
    #[arg(long, default_value = "flyway_schema_history")]
    pub(crate) history_table: String,
    #[arg(long)]
    pub(crate) installed_by: Option<String>,
    /// Override the advisory lock key (default: derived from schema and history table).
    #[arg(long)]
    pub(crate) advisory_lock_id: Option<i64>,
    /// Output verbosity level.
    #[arg(long, value_enum)]
    pub(crate) verbosity: Option<Verbosity>,
}

#[derive(Debug, Subcommand)]
pub(crate) enum MigrateCommand {
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
pub(crate) enum StatusFormat {
    Table,
    Json,
}

#[derive(Debug, Parser)]
#[command(styles = HELP_STYLES)]
pub(crate) struct EmbeddedCli {
    #[arg(long, env = "DATABASE_URL")]
    pub(crate) database_url: String,
    #[command(flatten)]
    pub(crate) common: CommonDbArgs,
    #[arg(long)]
    pub(crate) dir: Option<PathBuf>,
    #[command(subcommand)]
    pub(crate) command: MigrateCommand,
}

impl MigrateCommand {
    pub(crate) fn label(&self) -> &'static str {
        match self {
            Self::Up => "migrate up",
            Self::Status { .. } => "migrate status",
            Self::Fresh { .. } => "migrate fresh",
        }
    }
}
