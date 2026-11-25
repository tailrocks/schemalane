//! `PostgreSQL` integration tests for deferred indexes and foreign keys: apply the schema
//! without them, bulk-load with COPY, then apply the deferred set.

use deadpool_postgres::{ManagerConfig, Pool, RecyclingMethod};
use futures_util::{SinkExt, pin_mut};
use schemalane_core::{
    ApplyDeferredOptions, DeferPolicy, DeferredKind, DeferredStatus, FkMode, SchemalaneConfig,
    SchemalaneError, SchemalaneMigrator,
};
use std::collections::BTreeSet;
use std::error::Error;
use std::fs;
use std::io::Cursor;
use std::path::Path;
use tempfile::TempDir;
use testcontainers_modules::{postgres::Postgres, testcontainers::runners::SyncRunner};
use tokio_postgres::NoTls;

type TestResult = Result<(), Box<dyn Error + 'static>>;

const V1: &str = "
CREATE TABLE parent (
    id BIGINT PRIMARY KEY,
    name TEXT NOT NULL
);
CREATE TABLE child (
    id BIGINT PRIMARY KEY,
    parent_id BIGINT NOT NULL REFERENCES parent (id),
    note TEXT
);
CREATE INDEX child_parent_id_idx ON child (parent_id);
";

const V2: &str = "
CREATE TABLE grandchild (
    id BIGINT PRIMARY KEY,
    child_id BIGINT NOT NULL,
    label TEXT
);
ALTER TABLE grandchild ADD CONSTRAINT grandchild_child_fk FOREIGN KEY (child_id) REFERENCES child (id) ON DELETE CASCADE;
CREATE INDEX ON grandchild (child_id) WHERE label IS NOT NULL;
CREATE UNIQUE INDEX grandchild_label_uq ON grandchild (label);
";

fn connection_string(
    node: &testcontainers_modules::testcontainers::core::Container<Postgres>,
) -> Result<String, Box<dyn Error + 'static>> {
    let host = node.get_host()?;
    let port = node.get_host_port_ipv4(5432)?;
    Ok(format!(
        "postgres://postgres:postgres@{host}:{port}/postgres"
    ))
}

fn create_pool(db_url: &str, max_size: usize) -> Result<Pool, Box<dyn Error + 'static>> {
    let pg_config: tokio_postgres::Config = db_url.parse()?;
    let manager = deadpool_postgres::Manager::from_config(
        pg_config,
        NoTls,
        ManagerConfig {
            recycling_method: RecyclingMethod::Fast,
        },
    );
    Ok(Pool::builder(manager).max_size(max_size).build()?)
}

fn migrations(files: &[(&str, &str)]) -> Result<(TempDir, std::path::PathBuf), std::io::Error> {
    let temp = TempDir::new()?;
    let dir = temp.path().join("migrations");
    fs::create_dir_all(&dir)?;
    for (name, sql) in files {
        fs::write(dir.join(name), sql)?;
    }
    Ok((temp, dir))
}

fn migrator(dir: &Path, schema: &str) -> SchemalaneMigrator {
    SchemalaneMigrator::new(
        SchemalaneConfig::new()
            .with_schema(schema)
            .with_migrations_dir(dir),
    )
}

async fn strings(
    pool: &Pool,
    sql: &str,
    params: &[&(dyn tokio_postgres::types::ToSql + Sync)],
) -> Result<BTreeSet<String>, Box<dyn Error + 'static>> {
    let client = pool.get().await?;
    Ok(client
        .query(sql, params)
        .await?
        .iter()
        .map(|row| row.get::<_, String>(0))
        .collect())
}

/// Index definitions of `schema`, with the schema name removed so schemas compare equal.
async fn index_definitions(
    pool: &Pool,
    schema: &str,
) -> Result<BTreeSet<String>, Box<dyn Error + 'static>> {
    strings(
        pool,
        "SELECT replace(indexdef, $1 || '.', '') FROM pg_indexes WHERE schemaname = $1 \
         AND tablename NOT LIKE 'flyway_schema_history%'",
        &[&schema],
    )
    .await
}

/// Constraint definitions of `schema` (without the history tables), schema-independent.
async fn constraint_definitions(
    pool: &Pool,
    schema: &str,
) -> Result<BTreeSet<String>, Box<dyn Error + 'static>> {
    strings(
        pool,
        "SELECT c.relname || ': ' || k.conname || ' ' || replace(pg_get_constraintdef(k.oid), $1 || '.', '') \
         FROM pg_constraint k JOIN pg_class c ON c.oid = k.conrelid \
         JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = $1 AND c.relname NOT LIKE 'flyway_schema_history%'",
        &[&schema],
    )
    .await
}

async fn history_rows(pool: &Pool, schema: &str) -> Result<Vec<String>, Box<dyn Error + 'static>> {
    let client = pool.get().await?;
    let sql = format!(
        "SELECT installed_rank::text || '|' || version || '|' || description || '|' || type || '|' || script || '|' || checksum::text || '|' || success::text FROM \"{schema}\".flyway_schema_history ORDER BY installed_rank"
    );
    Ok(client
        .query(&sql, &[])
        .await?
        .iter()
        .map(|row| row.get::<_, String>(0))
        .collect())
}

async fn copy_rows(
    pool: &Pool,
    schema: &str,
    table: &str,
    columns: &str,
    data: String,
) -> TestResult {
    let client = pool.get().await?;
    client
        .batch_execute(&format!("SET search_path TO \"{schema}\""))
        .await?;
    let sink = client
        .copy_in(&format!("COPY {table} ({columns}) FROM STDIN"))
        .await?;
    pin_mut!(sink);
    sink.send(Cursor::new(data.into_bytes())).await?;
    sink.finish().await?;
    Ok(())
}

#[test]
#[ignore = "requires Docker daemon"]
fn bulk_load_then_apply_deferred_matches_plain_up() -> TestResult {
    let node = Postgres::default().start()?;
    let db_url = connection_string(&node)?;
    let (_temp, dir) = migrations(&[
        ("V1__base.sql", V1),
        ("V2__more.sql", V2),
        ("V3__drop_note.sql", "ALTER TABLE child DROP COLUMN note;"),
    ])?;
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async move {
        let pool = create_pool(&db_url, 6)?;
        let policy = DeferPolicy::secondary_indexes_and_foreign_keys();

        // Reference: the same migrations applied plainly.
        migrator(&dir, "plain").up(&pool).await?;

        // Phase 1: schema without deferred objects.
        let deferred = migrator(&dir, "bulk");
        let report = deferred.up_deferred(&pool, &policy).await?;
        assert_eq!(report.applied.len(), 3);
        let fks = strings(
            &pool,
            "SELECT conname::text FROM pg_constraint k JOIN pg_namespace n ON n.oid = k.connamespace WHERE n.nspname = 'bulk' AND k.contype = 'f'",
            &[],
        )
        .await?;
        assert!(fks.is_empty(), "foreign keys exist before apply: {fks:?}");
        let indexes = strings(
            &pool,
            "SELECT indexname::text FROM pg_indexes WHERE schemaname = 'bulk' AND tablename NOT LIKE 'flyway_schema_history%'",
            &[],
        )
        .await?;
        assert!(!indexes.contains("child_parent_id_idx"), "{indexes:?}");
        assert!(
            indexes.contains("grandchild_label_uq"),
            "unique indexes stay by default: {indexes:?}"
        );

        // The history is exactly the plain run's history.
        assert_eq!(
            history_rows(&pool, "bulk").await?,
            history_rows(&pool, "plain").await?
        );

        let status = deferred.deferred_status(&pool).await?;
        assert_eq!(status.applied, 0);
        assert_eq!(status.pending, status.entries.len());
        assert!(status.entries.iter().any(|e| e.kind == DeferredKind::ForeignKey));
        assert!(status.entries.iter().any(|e| e.kind == DeferredKind::Index));
        assert!(!deferred.verify_deferred(&pool).await?.ok);

        // A plain run refuses to continue while objects are deferred.
        let refused = migrator(&dir, "bulk").up(&pool).await;
        assert!(matches!(refused, Err(SchemalaneError::DeferredPending(_))));

        // Bulk load with COPY, with no indexes and no foreign keys in the way.
        copy_rows(&pool, "bulk", "parent", "id, name", (1..=1000).map(|i| format!("{i}\tp{i}\n")).collect()).await?;
        copy_rows(
            &pool,
            "bulk",
            "child",
            "id, parent_id",
            (1..=3000).map(|i| format!("{i}\t{}\n", (i % 1000) + 1)).collect(),
        )
        .await?;
        copy_rows(
            &pool,
            "bulk",
            "grandchild",
            "id, child_id, label",
            (1..=500).map(|i| format!("{i}\t{i}\tl{i}\n")).collect(),
        )
        .await?;

        // Phase 2: indexes then foreign keys, validated.
        let options = ApplyDeferredOptions::new()
            .with_parallelism(3)
            .with_session_setting("maintenance_work_mem", "64MB");
        let applied = deferred.apply_deferred(&pool, &options).await?;
        assert_eq!(applied.applied.len(), status.entries.len());
        assert_eq!(applied.already_applied, 0);
        assert!(applied.applied.iter().all(|e| e.status == DeferredStatus::Applied));

        let verification = deferred.verify_deferred(&pool).await?;
        assert!(verification.ok, "{verification:#?}");
        assert!(verification.entries.iter().all(|e| e.exists && e.valid));

        // Same schema as a plain `up`: indexes and constraints.
        assert_eq!(
            index_definitions(&pool, "bulk").await?,
            index_definitions(&pool, "plain").await?
        );
        assert_eq!(
            constraint_definitions(&pool, "bulk").await?,
            constraint_definitions(&pool, "plain").await?
        );
        let unvalidated = strings(
            &pool,
            "SELECT conname::text FROM pg_constraint k JOIN pg_namespace n ON n.oid = k.connamespace WHERE n.nspname = 'bulk' AND NOT k.convalidated",
            &[],
        )
        .await?;
        assert!(unvalidated.is_empty(), "{unvalidated:?}");

        // Applying again is a no-op, and plain `up` works again.
        let again = deferred.apply_deferred(&pool, &options).await?;
        assert!(again.applied.is_empty());
        let plain_up = migrator(&dir, "bulk").up(&pool).await?;
        assert!(plain_up.applied.is_empty());
        Ok(())
    })
}

#[test]
#[ignore = "requires Docker daemon"]
fn later_migrations_see_the_same_schema_as_a_plain_run() -> TestResult {
    let node = Postgres::default().start()?;
    let db_url = connection_string(&node)?;
    let (_temp, dir) = migrations(&[
        ("V1__base.sql", V1),
        ("V2__more.sql", V2),
        ("V3__drop_index.sql", "DROP INDEX child_parent_id_idx;"),
        (
            "V4__data.sql",
            "INSERT INTO parent VALUES (1, 'a'); INSERT INTO child VALUES (1, 1); \
             INSERT INTO grandchild VALUES (1, 1, 'x'); DELETE FROM child WHERE id = 1;",
        ),
        (
            "V5__rename.sql",
            "CREATE INDEX child_note_idx ON child (note); \
             ALTER INDEX child_note_idx RENAME TO child_note_renamed;",
        ),
    ])?;
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async move {
        let pool = create_pool(&db_url, 4)?;
        migrator(&dir, "plain").up(&pool).await?;
        let deferred = migrator(&dir, "bulk");
        deferred
            .up_deferred(&pool, &DeferPolicy::secondary_indexes_and_foreign_keys())
            .await?;

        // The `ON DELETE CASCADE` foreign key was in place when V4 deleted the child row.
        for schema in ["plain", "bulk"] {
            let client = pool.get().await?;
            let remaining: i64 = client
                .query_one(
                    &format!("SELECT count(*) FROM \"{schema}\".grandchild"),
                    &[],
                )
                .await?
                .get(0);
            assert_eq!(remaining, 0, "{schema}");
        }
        let status = deferred.deferred_status(&pool).await?;
        let flushed: BTreeSet<String> = status
            .entries
            .iter()
            .filter(|entry| entry.flushed)
            .filter_map(|entry| entry.object_name.clone())
            .collect();
        assert!(flushed.contains("child_parent_id_idx"), "{flushed:?}");
        assert!(flushed.contains("grandchild_child_fk"), "{flushed:?}");
        assert!(flushed.contains("child_note_idx"), "{flushed:?}");

        deferred
            .apply_deferred(&pool, &ApplyDeferredOptions::new())
            .await?;
        assert!(deferred.verify_deferred(&pool).await?.ok);
        assert_eq!(
            index_definitions(&pool, "bulk").await?,
            index_definitions(&pool, "plain").await?
        );
        assert_eq!(
            constraint_definitions(&pool, "bulk").await?,
            constraint_definitions(&pool, "plain").await?
        );
        Ok(())
    })
}

#[test]
#[ignore = "requires Docker daemon"]
fn invalid_data_fails_validation_and_apply_resumes() -> TestResult {
    let node = Postgres::default().start()?;
    let db_url = connection_string(&node)?;
    let (_temp, dir) = migrations(&[("V1__base.sql", V1)])?;
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async move {
        let pool = create_pool(&db_url, 3)?;
        let deferred = migrator(&dir, "bulk");
        deferred
            .up_deferred(&pool, &DeferPolicy::secondary_indexes_and_foreign_keys())
            .await?;
        // An orphan row: parent 999 does not exist.
        copy_rows(
            &pool,
            "bulk",
            "child",
            "id, parent_id",
            "1\t999\n".to_owned(),
        )
        .await?;

        let failed = deferred
            .apply_deferred(&pool, &ApplyDeferredOptions::new())
            .await;
        assert!(failed.is_err(), "validation must reject the orphan row");
        // The index (applied before) stays applied; the foreign key stays pending.
        let status = deferred.deferred_status(&pool).await?;
        assert_eq!(status.applied, 1);
        assert_eq!(status.pending, 1);

        // Fix the data and resume: only the foreign key is left.
        copy_rows(
            &pool,
            "bulk",
            "parent",
            "id, name",
            "999\tfixed\n".to_owned(),
        )
        .await?;
        let resumed = deferred
            .apply_deferred(
                &pool,
                &ApplyDeferredOptions::new().with_fk_mode(FkMode::NotValidThenValidate),
            )
            .await?;
        assert_eq!(resumed.applied.len(), 1);
        assert_eq!(resumed.already_applied, 1);
        assert!(deferred.verify_deferred(&pool).await?.ok);
        Ok(())
    })
}

#[test]
#[ignore = "requires Docker daemon"]
fn concurrent_index_build_and_fresh_deferred() -> TestResult {
    let node = Postgres::default().start()?;
    let db_url = connection_string(&node)?;
    let (_temp, dir) = migrations(&[("V1__base.sql", V1), ("V2__more.sql", V2)])?;
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async move {
        let pool = create_pool(&db_url, 3)?;
        let deferred = migrator(&dir, "bulk");
        let policy = DeferPolicy::secondary_indexes_and_foreign_keys().with_unique_indexes(true);
        deferred.fresh_deferred(&pool, true, &policy).await?;
        let status = deferred.deferred_status(&pool).await?;
        assert!(
            status
                .entries
                .iter()
                .any(|e| e.kind == DeferredKind::UniqueIndex)
        );

        let applied = deferred
            .apply_deferred(&pool, &ApplyDeferredOptions::new().with_concurrently(true))
            .await?;
        assert_eq!(applied.applied.len(), status.entries.len());
        assert!(deferred.verify_deferred(&pool).await?.ok);

        // Fresh again resets the schema and the deferred set with it.
        deferred.fresh_deferred(&pool, true, &policy).await?;
        assert_eq!(deferred.deferred_status(&pool).await?.applied, 0);
        Ok(())
    })
}
