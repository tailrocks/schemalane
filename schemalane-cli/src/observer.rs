use std::sync::Mutex;

use owo_colors::{OwoColorize, Stream, Style};
use pg_query_fmt::highlight::highlight_sql_line;
use schemalane_core::{
    MigrationFailed, MigrationFinished, MigrationObserver, MigrationStarted, SqlStatementFailed,
    SqlStatementFinished, SqlStatementStarted, StatusReport,
};

use crate::render::{
    INDENT, MAX_PREVIEW_WIDTH, STATUS_WIDTH, Verbosity, format_elapsed, pad_index,
    sanitize_terminal, truncate_preview,
};
use crate::runner::{print_pending_migrations, print_status_overview};

pub(crate) struct CliProgressObserver {
    verbosity: Verbosity,
    max_script_len: Mutex<usize>,
    last_error: Mutex<Option<String>>,
    planned_report: Mutex<Option<StatusReport>>,
}

impl CliProgressObserver {
    pub(crate) fn new(verbosity: Verbosity) -> Self {
        Self {
            verbosity,
            max_script_len: Mutex::new(0),
            last_error: Mutex::new(None),
            planned_report: Mutex::new(None),
        }
    }

    pub(crate) fn last_error(&self) -> Option<String> {
        self.last_error.lock().ok().and_then(|e| e.clone())
    }

    pub(crate) fn planned_report(&self) -> Option<StatusReport> {
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
