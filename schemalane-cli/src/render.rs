use clap::ValueEnum;
use comfy_table::{Attribute, Cell, CellAlignment, Color, ContentArrangement, Table, presets};
use owo_colors::{OwoColorize, Stream, Style};
use schemalane_core::{MigrationState, SchemalaneError, StatusEntry, StatusReport};
use std::collections::BTreeSet;

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

pub(crate) const INDENT: &str = " ";
pub(crate) const MAX_PREVIEW_WIDTH: usize = 60;
pub(crate) const STATUS_WIDTH: usize = 7;

pub(crate) fn sanitize_terminal(text: &str) -> String {
    text.chars()
        .filter(|character| !character.is_control() || *character == '\n' || *character == '\t')
        .collect()
}

pub(crate) fn pad_index(index: usize, total: usize) -> String {
    let width = total.to_string().len().max(2);
    format!("{index:0>width$}")
}

pub(crate) fn format_elapsed(milliseconds: i32) -> String {
    if milliseconds >= 1000 {
        let seconds = f64::from(milliseconds) / 1000.0;
        format!("{seconds:.1} s")
    } else {
        format!("{milliseconds} ms")
    }
}

pub(crate) fn truncate_preview(text: &str, max_width: usize) -> String {
    debug_assert!(max_width >= 3, "truncate_preview needs room for ellipsis");
    if text.chars().count() <= max_width {
        return text.to_owned();
    }
    let truncated: String = text.chars().take(max_width.saturating_sub(3)).collect();
    format!("{truncated}...")
}

pub(crate) fn print_branding(command: &str) {
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

pub(crate) fn print_status_table(report: &StatusReport) {
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

pub(crate) fn print_error_diagnostics(report: &StatusReport, err: &SchemalaneError) {
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

pub(crate) fn latest_database_version(report: &StatusReport) -> Option<String> {
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
