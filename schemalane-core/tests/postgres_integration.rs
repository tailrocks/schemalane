use deadpool_postgres::{ManagerConfig, Pool, RecyclingMethod};
use schemalane_core::{
    MigrationState, RustMigrationExecutor, RustTransactionMode, SchemalaneConfig, SchemalaneError,
    SchemalaneMigrator, derive_advisory_lock_id,
};
use std::error::Error;
use std::fs;
use std::path::Path;
use tempfile::TempDir;
use testcontainers_modules::{postgres::Postgres, testcontainers::runners::SyncRunner};
use tokio_postgres::NoTls;

#[test]
#[ignore = "requires Docker daemon"]
fn up_and_status_with_sql_migrations() -> Result<(), Box<dyn Error + 'static>> {
    let node = Postgres::default().start()?;
    let db_url = connection_string(&node)?;

    let temp = TempDir::new()?;
    let migrations_dir = temp.path().join("migrations");
    fs::create_dir_all(&migrations_dir)?;

    write_migration(
        &migrations_dir,
        "V1__create_cake.sql",
        r"
CREATE TABLE cake (
    id SERIAL PRIMARY KEY,
    name TEXT NOT NULL
);
INSERT INTO cake(name) VALUES ('chocolate');
",
    )?;

    write_migration(
        &migrations_dir,
        "V2__create_price_histories.sql",
        r"
CREATE TABLE price_histories (
    id SERIAL PRIMARY KEY,
    asset TEXT NOT NULL,
    price NUMERIC NOT NULL
);
",
    )?;

    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async move {
        let pool = create_pool(&db_url)?;

        let migrator =
            SchemalaneMigrator::new(SchemalaneConfig::new().with_migrations_dir(migrations_dir));

        let up_report = migrator.up(&pool).await?;
        assert_eq!(up_report.applied.len(), 2);
        assert_eq!(up_report.skipped, 0);

        let second_up_report = migrator.up(&pool).await?;
        assert_eq!(second_up_report.applied.len(), 0);
        assert_eq!(second_up_report.skipped, 2);

        let status = migrator.status(&pool).await?;
        assert_eq!(status.summary.success, 2);
        assert_eq!(status.summary.pending, 0);
        assert_eq!(status.summary.failed, 0);
        assert_eq!(status.summary.missing, 0);
        assert_eq!(status.summary.checksum_mismatch, 0);
        let validation = migrator.validate(&pool).await?;
        assert_eq!(validation.summary.success, 2);

        let history_count = scalar_i64(
            &pool,
            "SELECT COUNT(*) AS count FROM public.flyway_schema_history",
        )
        .await?;
        assert_eq!(history_count, 2);

        let cake_count = scalar_i64(&pool, "SELECT COUNT(*) AS count FROM public.cake").await?;
        assert_eq!(cake_count, 1);

        Ok::<(), Box<dyn Error + 'static>>(())
    })?;

    Ok(())
}

#[test]
#[ignore = "requires Docker daemon"]
fn status_sees_history_in_mixed_case_schema() -> Result<(), Box<dyn Error + 'static>> {
    let node = Postgres::default().start()?;
    let db_url = connection_string(&node)?;
    let temp = TempDir::new()?;
    let migrations_dir = temp.path().join("migrations");
    fs::create_dir_all(&migrations_dir)?;
    write_migration(
        &migrations_dir,
        "V1__create_cake.sql",
        "CREATE TABLE cake (id SERIAL PRIMARY KEY);",
    )?;

    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async move {
        let pool = create_pool(&db_url)?;
        let migrator = SchemalaneMigrator::new(
            SchemalaneConfig::new()
                .with_schema("MyApp")
                .with_migrations_dir(migrations_dir),
        );

        assert_eq!(migrator.up(&pool).await?.applied.len(), 1);
        let status = migrator.status(&pool).await?;
        assert_eq!(status.summary.success, 1);
        assert_eq!(status.summary.pending, 0);

        Ok::<(), Box<dyn Error + 'static>>(())
    })?;

    Ok(())
}

#[test]
#[ignore = "requires Docker daemon"]
fn advisory_locks_do_not_contend_across_schemas() -> Result<(), Box<dyn Error + 'static>> {
    let node = Postgres::default().start()?;
    let db_url = connection_string(&node)?;
    let temp = TempDir::new()?;
    let migrations_dir = temp.path().join("migrations");
    fs::create_dir_all(&migrations_dir)?;
    write_migration(
        &migrations_dir,
        "V1__create_t.sql",
        "CREATE TABLE t (id INT);",
    )?;

    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async move {
        let pool = create_pool(&db_url)?;
        let holder = pool.get().await?;
        let public_lock = derive_advisory_lock_id("public", "flyway_schema_history");
        holder
            .execute("SELECT pg_advisory_lock($1)", &[&public_lock])
            .await?;

        let migrator = SchemalaneMigrator::new(
            SchemalaneConfig::new()
                .with_schema("other")
                .with_migrations_dir(migrations_dir),
        );
        let report = tokio::time::timeout(std::time::Duration::from_secs(5), migrator.up(&pool))
            .await
            .expect("different-schema lock must not block")?;
        assert_eq!(report.applied.len(), 1);

        holder
            .execute("SELECT pg_advisory_unlock($1)", &[&public_lock])
            .await?;
        Ok::<(), Box<dyn Error + 'static>>(())
    })?;
    Ok(())
}

#[test]
#[ignore = "requires Docker daemon"]
fn fresh_recreates_schema() -> Result<(), Box<dyn Error + 'static>> {
    let node = Postgres::default().start()?;
    let db_url = connection_string(&node)?;

    let temp = TempDir::new()?;
    let migrations_dir = temp.path().join("migrations");
    fs::create_dir_all(&migrations_dir)?;

    write_migration(
        &migrations_dir,
        "V1__create_cake.sql",
        r"
CREATE TABLE cake (
    id SERIAL PRIMARY KEY,
    name TEXT NOT NULL
);
",
    )?;

    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async move {
        let pool = create_pool(&db_url)?;

        let migrator =
            SchemalaneMigrator::new(SchemalaneConfig::new().with_migrations_dir(migrations_dir));

        migrator.up(&pool).await?;
        {
            let client = pool.get().await?;
            client
                .batch_execute("INSERT INTO public.cake(name) VALUES ('temp-row')")
                .await?;
        }

        let before = scalar_i64(&pool, "SELECT COUNT(*) AS count FROM public.cake").await?;
        assert_eq!(before, 1);

        let fresh_report = migrator.fresh(&pool, true).await?;
        assert_eq!(fresh_report.applied.len(), 1);

        let after = scalar_i64(&pool, "SELECT COUNT(*) AS count FROM public.cake").await?;
        assert_eq!(after, 0);

        let history_count = scalar_i64(
            &pool,
            "SELECT COUNT(*) AS count FROM public.flyway_schema_history",
        )
        .await?;
        assert_eq!(history_count, 1);

        Ok::<(), Box<dyn Error + 'static>>(())
    })?;

    Ok(())
}

#[test]
#[ignore = "requires Docker daemon"]
fn fresh_drops_only_target_schema() -> Result<(), Box<dyn Error + 'static>> {
    let node = Postgres::default().start()?;
    let db_url = connection_string(&node)?;

    let temp = TempDir::new()?;
    let migrations_dir = temp.path().join("migrations");
    fs::create_dir_all(&migrations_dir)?;
    write_migration(
        &migrations_dir,
        "V1__create_cake.sql",
        "CREATE TABLE cake (id SERIAL PRIMARY KEY);",
    )?;

    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async move {
        let pool = create_pool(&db_url)?;
        let migrator =
            SchemalaneMigrator::new(SchemalaneConfig::new().with_migrations_dir(migrations_dir));

        migrator.up(&pool).await?;
        {
            let client = pool.get().await?;
            client
                .batch_execute("CREATE SCHEMA other_app; CREATE TABLE other_app.keep_me (id INT);")
                .await?;
        }

        let report = migrator.fresh(&pool, true).await?;
        assert_eq!(report.applied.len(), 1);

        let client = pool.get().await?;
        let row = client
            .query_one(
                "SELECT to_regclass('other_app.keep_me') IS NOT NULL AS exists",
                &[],
            )
            .await?;
        assert!(row.get::<_, bool>("exists"));
        drop(client);

        let history_count = scalar_i64(
            &pool,
            "SELECT COUNT(*) AS count FROM public.flyway_schema_history",
        )
        .await?;
        assert_eq!(history_count, 1);

        Ok::<(), Box<dyn Error + 'static>>(())
    })?;

    Ok(())
}

#[test]
#[ignore = "requires Docker daemon"]
fn status_detects_checksum_mismatch() -> Result<(), Box<dyn Error + 'static>> {
    let node = Postgres::default().start()?;
    let db_url = connection_string(&node)?;

    let temp = TempDir::new()?;
    let migrations_dir = temp.path().join("migrations");
    fs::create_dir_all(&migrations_dir)?;

    let migration_path = migrations_dir.join("V1__create_cake.sql");
    fs::write(
        &migration_path,
        r"
CREATE TABLE cake (
    id SERIAL PRIMARY KEY,
    name TEXT NOT NULL
);
",
    )?;

    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async move {
        let pool = create_pool(&db_url)?;

        let migrator =
            SchemalaneMigrator::new(SchemalaneConfig::new().with_migrations_dir(migrations_dir));

        migrator.up(&pool).await?;

        fs::write(
            &migration_path,
            r"
CREATE TABLE cake (
    id SERIAL PRIMARY KEY,
    name TEXT NOT NULL,
    note TEXT
);
",
        )?;

        let status = migrator.status(&pool).await?;
        assert_eq!(status.summary.checksum_mismatch, 1);
        assert_eq!(status.summary.success, 0);

        let mismatch_entry = status
            .migrations
            .iter()
            .find(|entry| entry.script == "V1__create_cake.sql")
            .ok_or_else(|| "expected migration entry".to_string())?;

        assert_eq!(mismatch_entry.state, MigrationState::ChecksumMismatch);
        assert!(matches!(
            migrator.validate(&pool).await,
            Err(SchemalaneError::Drift(_))
        ));

        Ok::<(), Box<dyn Error + 'static>>(())
    })?;

    Ok(())
}

#[test]
#[ignore = "requires Docker daemon"]
fn rust_migration_success_and_history_type() -> Result<(), Box<dyn Error + 'static>> {
    let node = Postgres::default().start()?;
    let db_url = connection_string(&node)?;

    let temp = TempDir::new()?;
    let migrations_dir = temp.path().join("migrations");
    fs::create_dir_all(&migrations_dir)?;
    write_rust_migration(&migrations_dir, "V1__create_rust_records.rs")?;

    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async move {
        let pool = create_pool(&db_url)?;

        let mut migrator = SchemalaneMigrator::new(SchemalaneConfig::new().with_migrations_dir(migrations_dir));
        migrator.register_rust_migration(
            "V1__create_rust_records.rs",
            RustMigrationExecutor::new(|client| Box::pin(create_rust_records(client))),
        );

        let report = migrator.up(&pool).await?;
        assert_eq!(report.applied.len(), 1);
        assert_eq!(report.applied[0].migration_type, "RUST");
        assert_eq!(report.applied[0].script, "V1__create_rust_records.rs");

        let row_count = scalar_i64(
            &pool,
            "SELECT COUNT(*) AS count FROM public.rust_records WHERE name = 'from-rust'",
        )
        .await?;
        assert_eq!(row_count, 1);

        let client = pool.get().await?;
        let history_row = client
            .query_one(
                "SELECT \"type\", \"script\", \"success\" FROM public.flyway_schema_history ORDER BY \"installed_rank\" LIMIT 1",
                &[],
            )
            .await?;

        let migration_type: String = history_row.get("type");
        let script: String = history_row.get("script");
        let success: bool = history_row.get("success");
        assert_eq!(migration_type, "RUST");
        assert_eq!(script, "V1__create_rust_records.rs");
        assert!(success);

        let status = migrator.status(&pool).await?;
        assert_eq!(status.summary.success, 1);
        assert_eq!(status.summary.failed, 0);

        Ok::<(), Box<dyn Error + 'static>>(())
    })?;

    Ok(())
}

#[test]
#[ignore = "requires Docker daemon"]
fn rust_migration_transaction_mode_rolls_back_on_failure() -> Result<(), Box<dyn Error + 'static>> {
    let node = Postgres::default().start()?;
    let db_url = connection_string(&node)?;

    let temp = TempDir::new()?;
    let migrations_dir = temp.path().join("migrations");
    fs::create_dir_all(&migrations_dir)?;
    write_rust_migration(&migrations_dir, "V2__rust_tx_failure.rs")?;

    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async move {
        let pool = create_pool(&db_url)?;

        let mut migrator =
            SchemalaneMigrator::new(SchemalaneConfig::new().with_migrations_dir(migrations_dir));
        migrator.register_rust_migration(
            "V2__rust_tx_failure.rs",
            RustMigrationExecutor::transactional(|client| {
                Box::pin(fail_after_insert(client, "rust_tx_failure_items"))
            }),
        );

        let err = migrator.up(&pool).await.expect_err("migration should fail");
        assert!(
            matches!(err, SchemalaneError::MigrationExecution { .. }),
            "expected MigrationExecution, got: {err}"
        );

        let exists = table_exists(&pool, "rust_tx_failure_items").await?;
        assert!(
            !exists,
            "transactional failure should roll back table creation"
        );

        let status = migrator.status(&pool).await?;
        assert_eq!(status.summary.failed, 1);

        let failed_entry = status
            .migrations
            .iter()
            .find(|entry| entry.script == "V2__rust_tx_failure.rs")
            .ok_or_else(|| "expected failed rust migration entry".to_string())?;
        assert_eq!(failed_entry.state, MigrationState::Failed);

        Ok::<(), Box<dyn Error + 'static>>(())
    })?;

    Ok(())
}

#[test]
#[ignore = "requires Docker daemon"]
fn rust_migration_no_transaction_mode_persists_partial_work_on_failure()
-> Result<(), Box<dyn Error + 'static>> {
    let node = Postgres::default().start()?;
    let db_url = connection_string(&node)?;

    let temp = TempDir::new()?;
    let migrations_dir = temp.path().join("migrations");
    fs::create_dir_all(&migrations_dir)?;
    write_rust_migration(&migrations_dir, "V3__rust_no_tx_failure.rs")?;

    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async move {
        let pool = create_pool(&db_url)?;

        let mut migrator =
            SchemalaneMigrator::new(SchemalaneConfig::new().with_migrations_dir(migrations_dir));
        migrator.register_rust_migration(
            "V3__rust_no_tx_failure.rs",
            RustMigrationExecutor::with_mode(RustTransactionMode::NoTransaction, |client| {
                Box::pin(fail_after_insert(client, "rust_no_tx_failure_items"))
            }),
        );

        let err = migrator.up(&pool).await.expect_err("migration should fail");
        assert!(
            matches!(err, SchemalaneError::MigrationExecution { .. }),
            "expected MigrationExecution, got: {err}"
        );

        let exists = table_exists(&pool, "rust_no_tx_failure_items").await?;
        assert!(
            exists,
            "non-transactional failure should keep created table"
        );

        let row_count = scalar_i64(
            &pool,
            "SELECT COUNT(*) AS count FROM public.rust_no_tx_failure_items",
        )
        .await?;
        assert_eq!(row_count, 1);

        let status = migrator.status(&pool).await?;
        assert_eq!(status.summary.failed, 1);

        let failed_entry = status
            .migrations
            .iter()
            .find(|entry| entry.script == "V3__rust_no_tx_failure.rs")
            .ok_or_else(|| "expected failed rust migration entry".to_string())?;
        assert_eq!(failed_entry.state, MigrationState::Failed);

        Ok::<(), Box<dyn Error + 'static>>(())
    })?;

    Ok(())
}

#[test]
#[ignore = "requires Docker daemon"]
fn rust_migration_requires_registered_executor() -> Result<(), Box<dyn Error + 'static>> {
    let node = Postgres::default().start()?;
    let db_url = connection_string(&node)?;

    let temp = TempDir::new()?;
    let migrations_dir = temp.path().join("migrations");
    fs::create_dir_all(&migrations_dir)?;
    write_rust_migration(&migrations_dir, "V9__missing_executor.rs")?;

    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async move {
        let pool = create_pool(&db_url)?;

        let migrator = SchemalaneMigrator::new(SchemalaneConfig::new().with_migrations_dir(migrations_dir));

        let err = migrator.up(&pool).await.expect_err("expected validation error");
        assert!(
            matches!(err, SchemalaneError::Validation(ref message) if message.contains("missing Rust migration executor")),
            "expected Validation for missing Rust migration executor, got: {err}"
        );

        Ok::<(), Box<dyn Error + 'static>>(())
    })?;

    Ok(())
}

fn connection_string(
    node: &testcontainers_modules::testcontainers::core::Container<Postgres>,
) -> Result<String, Box<dyn Error + 'static>> {
    let host = node.get_host()?;
    let port = node.get_host_port_ipv4(5432)?;
    Ok(format!(
        "postgres://postgres:postgres@{host}:{port}/postgres"
    ))
}

#[test]
#[ignore = "requires Docker daemon"]
fn sql_migration_failure_rolls_back_and_records_failed_row() -> Result<(), Box<dyn Error + 'static>>
{
    let node = Postgres::default().start()?;
    let db_url = connection_string(&node)?;
    let temp = TempDir::new()?;
    let migrations_dir = temp.path().join("migrations");
    fs::create_dir_all(&migrations_dir)?;
    write_migration(
        &migrations_dir,
        "V1__fail.sql",
        "CREATE TABLE roll_a (id int); SELECT * FROM missing_table_xyz;",
    )?;
    tokio::runtime::Runtime::new()?.block_on(async move {
        let pool = create_pool(&db_url)?;
        let migrator = SchemalaneMigrator::new(SchemalaneConfig::new().with_migrations_dir(migrations_dir));
        assert!(matches!(migrator.up(&pool).await, Err(SchemalaneError::MigrationExecution { .. })));
        assert!(!table_exists(&pool, "roll_a").await?);
        let client = pool.get().await?;
        let row = client.query_one("SELECT COUNT(*) AS count, bool_and(NOT \"success\") AS failed FROM public.flyway_schema_history", &[]).await?;
        assert_eq!(row.get::<_, i64>("count"), 1);
        assert!(row.get::<_, bool>("failed"));
        Ok::<(), Box<dyn Error + 'static>>(())
    })?;
    Ok(())
}

#[test]
#[ignore = "requires Docker daemon"]
fn transactional_migration_and_history_commit_atomically() -> Result<(), Box<dyn Error + 'static>> {
    let node = Postgres::default().start()?;
    let db_url = connection_string(&node)?;
    let temp = TempDir::new()?;
    let migrations_dir = temp.path().join("migrations");
    fs::create_dir_all(&migrations_dir)?;
    write_migration(
        &migrations_dir,
        "V1__atomic.sql",
        "CREATE TABLE atomic_t (id int); INSERT INTO atomic_t VALUES (1);",
    )?;
    tokio::runtime::Runtime::new()?.block_on(async move {
        let pool = create_pool(&db_url)?;
        let migrator =
            SchemalaneMigrator::new(SchemalaneConfig::new().with_migrations_dir(migrations_dir));
        assert_eq!(migrator.up(&pool).await?.applied.len(), 1);
        assert_eq!(
            scalar_i64(&pool, "SELECT COUNT(*) AS count FROM public.atomic_t").await?,
            1
        );
        assert_eq!(
            scalar_i64(
                &pool,
                "SELECT COUNT(*) AS count FROM public.flyway_schema_history WHERE \"success\""
            )
            .await?,
            1
        );
        Ok::<(), Box<dyn Error + 'static>>(())
    })?;
    Ok(())
}

#[test]
#[ignore = "requires Docker daemon"]
fn failed_transactional_migration_leaves_only_failed_row() -> Result<(), Box<dyn Error + 'static>> {
    let node = Postgres::default().start()?;
    let db_url = connection_string(&node)?;
    let temp = TempDir::new()?;
    let migrations_dir = temp.path().join("migrations");
    fs::create_dir_all(&migrations_dir)?;
    write_migration(
        &migrations_dir,
        "V1__atomic_fail.sql",
        "CREATE TABLE atomic_fail (id int); SELECT * FROM missing_atomic_table;",
    )?;
    tokio::runtime::Runtime::new()?.block_on(async move {
        let pool = create_pool(&db_url)?;
        let migrator =
            SchemalaneMigrator::new(SchemalaneConfig::new().with_migrations_dir(migrations_dir));
        assert!(matches!(
            migrator.up(&pool).await,
            Err(SchemalaneError::MigrationExecution { .. })
        ));
        assert!(!table_exists(&pool, "atomic_fail").await?);
        assert_eq!(
            scalar_i64(
                &pool,
                "SELECT COUNT(*) AS count FROM public.flyway_schema_history WHERE NOT \"success\""
            )
            .await?,
            1
        );
        assert_eq!(
            scalar_i64(
                &pool,
                "SELECT COUNT(*) AS count FROM public.flyway_schema_history"
            )
            .await?,
            1
        );
        Ok::<(), Box<dyn Error + 'static>>(())
    })?;
    Ok(())
}

#[test]
#[ignore = "requires Docker daemon"]
fn failed_history_blocks_next_up_until_fixed() -> Result<(), Box<dyn Error + 'static>> {
    let node = Postgres::default().start()?;
    let db_url = connection_string(&node)?;
    let temp = TempDir::new()?;
    let migrations_dir = temp.path().join("migrations");
    fs::create_dir_all(&migrations_dir)?;
    write_migration(
        &migrations_dir,
        "V1__fail.sql",
        "SELECT * FROM missing_table_xyz;",
    )?;
    tokio::runtime::Runtime::new()?.block_on(async move {
        let pool = create_pool(&db_url)?;
        let migrator =
            SchemalaneMigrator::new(SchemalaneConfig::new().with_migrations_dir(migrations_dir));
        assert!(matches!(
            migrator.up(&pool).await,
            Err(SchemalaneError::MigrationExecution { .. })
        ));
        let err = migrator
            .up(&pool)
            .await
            .expect_err("failed history must block");
        assert!(matches!(err, SchemalaneError::FailedHistory(_)));
        assert_eq!(err.exit_code(), 4);
        Ok::<(), Box<dyn Error + 'static>>(())
    })?;
    Ok(())
}

#[test]
#[ignore = "requires Docker daemon"]
fn mixed_statements_records_no_history_row() -> Result<(), Box<dyn Error + 'static>> {
    let node = Postgres::default().start()?;
    let db_url = connection_string(&node)?;
    let temp = TempDir::new()?;
    let migrations_dir = temp.path().join("migrations");
    fs::create_dir_all(&migrations_dir)?;
    write_migration(
        &migrations_dir,
        "V1__mixed.sql",
        "CREATE TABLE t (id int); CREATE INDEX CONCURRENTLY i ON t (id);",
    )?;
    tokio::runtime::Runtime::new()?.block_on(async move {
        let pool = create_pool(&db_url)?;
        let migrator =
            SchemalaneMigrator::new(SchemalaneConfig::new().with_migrations_dir(migrations_dir));
        assert!(matches!(
            migrator.up(&pool).await,
            Err(SchemalaneError::MixedStatements { .. })
        ));
        assert_eq!(
            scalar_i64(
                &pool,
                "SELECT COUNT(*) AS count FROM public.flyway_schema_history"
            )
            .await?,
            0
        );
        Ok::<(), Box<dyn Error + 'static>>(())
    })?;
    Ok(())
}

#[test]
#[ignore = "requires Docker daemon"]
fn non_transactional_sql_executes_outside_txn() -> Result<(), Box<dyn Error + 'static>> {
    let node = Postgres::default().start()?;
    let db_url = connection_string(&node)?;
    let temp = TempDir::new()?;
    let migrations_dir = temp.path().join("migrations");
    fs::create_dir_all(&migrations_dir)?;
    write_migration(&migrations_dir, "V1__t.sql", "CREATE TABLE t (id int);")?;
    write_migration(
        &migrations_dir,
        "V2__idx.sql",
        "CREATE INDEX CONCURRENTLY idx_t ON t (id);",
    )?;
    tokio::runtime::Runtime::new()?.block_on(async move {
        let pool = create_pool(&db_url)?;
        let migrator =
            SchemalaneMigrator::new(SchemalaneConfig::new().with_migrations_dir(migrations_dir));
        assert_eq!(migrator.up(&pool).await?.applied.len(), 2);
        assert!(table_exists(&pool, "idx_t").await?);
        assert_eq!(
            scalar_i64(
                &pool,
                "SELECT COUNT(*) AS count FROM public.flyway_schema_history WHERE \"success\""
            )
            .await?,
            2
        );
        Ok::<(), Box<dyn Error + 'static>>(())
    })?;
    Ok(())
}

#[test]
#[ignore = "requires Docker daemon"]
fn up_blocks_while_lock_held() -> Result<(), Box<dyn Error + 'static>> {
    let node = Postgres::default().start()?;
    let db_url = connection_string(&node)?;
    let temp = TempDir::new()?;
    let migrations_dir = temp.path().join("migrations");
    fs::create_dir_all(&migrations_dir)?;
    write_migration(&migrations_dir, "V1__t.sql", "CREATE TABLE t (id int);")?;
    tokio::runtime::Runtime::new()?.block_on(async move {
        let pool = create_pool(&db_url)?;
        let holder = pool.get().await?;
        let key = derive_advisory_lock_id("public", "flyway_schema_history");
        holder
            .execute("SELECT pg_advisory_lock($1)", &[&key])
            .await?;
        let migrator =
            SchemalaneMigrator::new(SchemalaneConfig::new().with_migrations_dir(migrations_dir));
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(2), migrator.up(&pool))
                .await
                .is_err()
        );
        holder
            .execute("SELECT pg_advisory_unlock($1)", &[&key])
            .await?;
        assert_eq!(migrator.up(&pool).await?.applied.len(), 1);
        Ok::<(), Box<dyn Error + 'static>>(())
    })?;
    Ok(())
}

#[test]
#[ignore = "requires Docker daemon"]
fn lock_released_after_successful_up() -> Result<(), Box<dyn Error + 'static>> {
    let node = Postgres::default().start()?;
    let db_url = connection_string(&node)?;
    let temp = TempDir::new()?;
    let migrations_dir = temp.path().join("migrations");
    fs::create_dir_all(&migrations_dir)?;
    write_migration(&migrations_dir, "V1__t.sql", "CREATE TABLE t (id int);")?;
    tokio::runtime::Runtime::new()?.block_on(async move {
        let pool = create_pool(&db_url)?;
        let migrator =
            SchemalaneMigrator::new(SchemalaneConfig::new().with_migrations_dir(migrations_dir));
        migrator.up(&pool).await?;
        let client = pool.get().await?;
        let key = derive_advisory_lock_id("public", "flyway_schema_history");
        let row = client
            .query_one("SELECT pg_try_advisory_lock($1) AS acquired", &[&key])
            .await?;
        assert!(row.get::<_, bool>("acquired"));
        client
            .execute("SELECT pg_advisory_unlock($1)", &[&key])
            .await?;
        Ok::<(), Box<dyn Error + 'static>>(())
    })?;
    Ok(())
}

#[test]
#[ignore = "requires Docker daemon"]
fn failed_sql_migration_records_failed_row_on_session_connection()
-> Result<(), Box<dyn Error + 'static>> {
    let node = Postgres::default().start()?;
    let db_url = connection_string(&node)?;
    let temp = TempDir::new()?;
    let migrations_dir = temp.path().join("migrations");
    fs::create_dir_all(&migrations_dir)?;
    write_migration(
        &migrations_dir,
        "V1__session_fail.sql",
        "CREATE TABLE session_fail(id int); SELECT * FROM absent_session_table;",
    )?;
    tokio::runtime::Runtime::new()?.block_on(async move {
        let pool = create_pool(&db_url)?;
        let migrator =
            SchemalaneMigrator::new(SchemalaneConfig::new().with_migrations_dir(migrations_dir));
        assert!(matches!(
            migrator.up(&pool).await,
            Err(SchemalaneError::MigrationExecution { .. })
        ));
        assert_eq!(
            scalar_i64(
                &pool,
                "SELECT COUNT(*) AS count FROM public.flyway_schema_history WHERE NOT \"success\""
            )
            .await?,
            1
        );
        Ok::<(), Box<dyn Error + 'static>>(())
    })?;
    Ok(())
}

#[test]
#[ignore = "requires Docker daemon"]
fn up_works_with_pool_max_size_one() -> Result<(), Box<dyn Error + 'static>> {
    let node = Postgres::default().start()?;
    let db_url = connection_string(&node)?;
    let temp = TempDir::new()?;
    let migrations_dir = temp.path().join("migrations");
    fs::create_dir_all(&migrations_dir)?;
    write_migration(
        &migrations_dir,
        "V1__single.sql",
        "CREATE TABLE single_pool(id int);",
    )?;
    tokio::runtime::Runtime::new()?.block_on(async move {
        let pool = create_pool_with_size(&db_url, 1)?;
        let migrator =
            SchemalaneMigrator::new(SchemalaneConfig::new().with_migrations_dir(migrations_dir));
        let report = tokio::time::timeout(std::time::Duration::from_secs(5), migrator.up(&pool))
            .await
            .expect("size-one pool must not deadlock")?;
        assert_eq!(report.applied.len(), 1);
        Ok::<(), Box<dyn Error + 'static>>(())
    })?;
    Ok(())
}

#[test]
#[ignore = "requires Docker daemon"]
fn cancelled_up_releases_detached_session_lock() -> Result<(), Box<dyn Error + 'static>> {
    let node = Postgres::default().start()?;
    let db_url = connection_string(&node)?;
    let temp = TempDir::new()?;
    let migrations_dir = temp.path().join("migrations");
    fs::create_dir_all(&migrations_dir)?;
    write_rust_migration(&migrations_dir, "V1__sleep.rs")?;
    tokio::runtime::Runtime::new()?.block_on(async move {
        let pool = create_pool_with_size(&db_url, 1)?;
        let mut migrator =
            SchemalaneMigrator::new(SchemalaneConfig::new().with_migrations_dir(migrations_dir));
        migrator.register_rust_migration(
            "V1__sleep.rs",
            RustMigrationExecutor::new(|_| {
                Box::pin(async {
                    tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                    Ok(())
                })
            }),
        );
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(250), migrator.up(&pool))
                .await
                .is_err()
        );
        let key = derive_advisory_lock_id("public", "flyway_schema_history");
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let client = pool.get().await?;
            let acquired: bool = client
                .query_one("SELECT pg_try_advisory_lock($1) AS acquired", &[&key])
                .await?
                .get("acquired");
            if acquired {
                client
                    .execute("SELECT pg_advisory_unlock($1)", &[&key])
                    .await?;
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "detached session lock was not released"
            );
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        Ok::<(), Box<dyn Error + 'static>>(())
    })?;
    Ok(())
}

fn create_pool(db_url: &str) -> Result<Pool, Box<dyn Error + 'static>> {
    create_pool_with_size(db_url, 5)
}

fn create_pool_with_size(db_url: &str, max_size: usize) -> Result<Pool, Box<dyn Error + 'static>> {
    let pg_config: tokio_postgres::Config = db_url.parse()?;
    let mgr = deadpool_postgres::Manager::from_config(
        pg_config,
        NoTls,
        ManagerConfig {
            recycling_method: RecyclingMethod::Fast,
        },
    );
    Ok(Pool::builder(mgr).max_size(max_size).build()?)
}

fn write_migration(
    migrations_dir: &Path,
    file_name: &str,
    sql: &str,
) -> Result<(), std::io::Error> {
    let path = migrations_dir.join(file_name);
    fs::write(path, sql)
}

fn write_rust_migration(migrations_dir: &Path, file_name: &str) -> Result<(), std::io::Error> {
    let path = migrations_dir.join(file_name);
    fs::write(
        path,
        r"
use tokio_postgres::Client;

pub async fn migration(client: &Client) -> Result<(), tokio_postgres::Error> {
    let _ = client;
    Ok(())
}
",
    )
}

async fn scalar_i64(pool: &Pool, sql: &str) -> Result<i64, Box<dyn Error + 'static>> {
    let client = pool.get().await?;
    let row = client.query_one(sql, &[]).await?;
    Ok(row.get("count"))
}

async fn table_exists(pool: &Pool, table: &str) -> Result<bool, Box<dyn Error + 'static>> {
    let client = pool.get().await?;
    let regclass = format!("public.{table}");
    let row = client
        .query_one("SELECT to_regclass($1) IS NOT NULL AS exists", &[&regclass])
        .await?;
    Ok(row.get("exists"))
}

async fn create_rust_records(client: &tokio_postgres::Client) -> Result<(), tokio_postgres::Error> {
    client
        .batch_execute(
            r"
CREATE TABLE rust_records (
    id SERIAL PRIMARY KEY,
    name TEXT NOT NULL
);
INSERT INTO rust_records(name) VALUES ('from-rust');
",
        )
        .await
}

async fn fail_after_insert(
    client: &tokio_postgres::Client,
    table_name: &str,
) -> Result<(), tokio_postgres::Error> {
    client
        .batch_execute(&format!(
            "CREATE TABLE {table_name} (id SERIAL PRIMARY KEY, note TEXT NOT NULL);"
        ))
        .await?;
    client
        .batch_execute(&format!(
            "INSERT INTO {table_name}(note) VALUES ('partial-write');"
        ))
        .await?;

    // Intentional failure via invalid SQL
    client
        .batch_execute("SELECT * FROM this_table_does_not_exist_intentional_failure")
        .await
}
