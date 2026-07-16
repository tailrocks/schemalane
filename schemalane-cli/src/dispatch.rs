use std::path::Path;

use schemalane_core::{
    SchemalaneConfig, SchemalaneError, SchemalaneMigrator, init_migration_project,
};

use crate::args::{
    Cli, CommonDbArgs, DEFAULT_MIGRATION_DIR, DEFAULT_SQL_DIR, MigrateArgs, MigrateCommand,
    RootCommand,
};
use crate::commands::{connect_with_feedback, run_db_command};
use crate::delegate::{DelegationOptions, run_via_migration_crate};

pub(crate) async fn run_root_cli(cli: Cli) -> Result<(), SchemalaneError> {
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
