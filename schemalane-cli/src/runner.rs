#![allow(clippy::print_stdout, clippy::print_stderr, clippy::future_not_send)]

use clap::Parser;
use schemalane_core::{SchemalaneConfig, SchemalaneError, SchemalaneMigrator};
use std::ffi::OsString;
use std::path::PathBuf;

use crate::args::{Cli, EmbeddedCli};

#[cfg(test)]
use crate::args::{DEFAULT_MIGRATION_DIR, MigrateArgs, MigrateCommand, RootCommand};

#[cfg(test)]
use crate::connect::{format_postgres_target, parse_postgres_target, wants_tls};

#[cfg(test)]
use crate::args::StatusFormat;

#[cfg(test)]
use crate::render::Verbosity;

#[cfg(test)]
use crate::render::truncate_preview;

/// Runs the embedded migration CLI with a generated migrator factory.
pub struct EmbeddedRunner {
    migrations_dir: &'static str,
    build_migrator: fn(SchemalaneConfig) -> SchemalaneMigrator,
}

impl EmbeddedRunner {
    /// Creates a runner for an embedded migration directory and factory.
    pub fn new(
        migrations_dir: &'static str,
        build_migrator: fn(SchemalaneConfig) -> SchemalaneMigrator,
    ) -> Self {
        Self {
            migrations_dir,
            build_migrator,
        }
    }

    /// Runs with process arguments and exits using the specification's error code.
    pub async fn run(self) {
        if let Err(err) = self.run_with(std::env::args_os()).await {
            eprintln!("{err}");
            std::process::exit(err.exit_code());
        }
    }

    /// Runs with caller-provided arguments and returns errors to the caller.
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

/// Runs the standalone CLI and exits using the specification's error code.
pub async fn run_cli() {
    if let Err(err) = run_cli_with(std::env::args_os()).await {
        eprintln!("{err}");
        std::process::exit(err.exit_code());
    }
}

/// Runs the standalone CLI with caller-provided arguments.
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

#[cfg(test)]
use crate::delegate::DelegationOptions;
use crate::dispatch::run_root_cli;

#[cfg(test)]
use crate::delegate::delegation_command_parts;

use crate::commands::{connect_with_feedback, run_db_command};
#[cfg(test)]
use crate::render::latest_database_version;

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
