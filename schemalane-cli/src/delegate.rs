use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

use schemalane_core::SchemalaneError;

use crate::Verbosity;
use crate::args::{MigrateCommand, StatusFormat};

pub(crate) struct DelegationOptions<'a> {
    pub(crate) database_url: Option<&'a str>,
    pub(crate) schema: &'a str,
    pub(crate) history_table: &'a str,
    pub(crate) installed_by: Option<&'a str>,
    pub(crate) advisory_lock_id: Option<i64>,
    pub(crate) command: &'a MigrateCommand,
    pub(crate) verbosity: Verbosity,
}

pub(crate) fn run_via_migration_crate(
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
pub(crate) fn delegation_command_parts(
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
        MigrateCommand::Up { dry_run, format } => {
            args.push(OsString::from("up"));
            if *dry_run {
                args.push(OsString::from("--dry-run"));
                args.extend([
                    OsString::from("--format"),
                    OsString::from(match format {
                        StatusFormat::Table => "table",
                        StatusFormat::Json => "json",
                    }),
                ]);
            }
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
        MigrateCommand::Validate {
            format,
            fail_on_pending,
        } => {
            args.extend([
                OsString::from("validate"),
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
