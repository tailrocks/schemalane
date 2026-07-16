use std::io::{IsTerminal, Write};
use std::time::Instant;

use deadpool_postgres::Pool;
use owo_colors::{OwoColorize, Stream, Style};
use schemalane_core::{SchemalaneError, SchemalaneMigrator, should_fail_on_pending};

use crate::args::{MigrateCommand, StatusFormat};
use crate::connect::{create_pool, format_postgres_target};
use crate::observer::CliProgressObserver;
use crate::prompt::prompt_yes_no;
use crate::render::{Verbosity, sanitize_terminal};
use crate::render::{
    print_branding, print_error_diagnostics, print_status_overview, print_status_table,
};

pub(crate) async fn connect_with_feedback(
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

pub(crate) async fn run_db_command(
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
        MigrateCommand::Validate {
            format,
            fail_on_pending,
        } => match migrator.validate(pool).await {
            Ok(report) => {
                render_validation(&report, format, true)?;
                if fail_on_pending {
                    should_fail_on_pending(&report)?;
                }
            }
            Err(error) => {
                if let Ok(report) = migrator.status(pool).await {
                    render_validation(&report, format, false)?;
                    if matches!(format, StatusFormat::Table) {
                        print_error_diagnostics(&report, &error);
                    }
                }
                return Err(error);
            }
        },
        MigrateCommand::Fresh { confirm } => {
            run_fresh_command(migrator, pool, confirm.as_deref(), verbosity).await?;
        }
    }

    Ok(())
}

fn render_validation(
    report: &schemalane_core::StatusReport,
    format: StatusFormat,
    valid: bool,
) -> Result<(), SchemalaneError> {
    match format {
        StatusFormat::Table => {
            print_status_overview(report);
            print_status_table(report);
        }
        StatusFormat::Json => println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "report": report,
                "validation": { "valid": valid }
            }))
            .map_err(|error| {
                SchemalaneError::Internal(format!("failed to encode JSON: {error}"))
            })?
        ),
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
