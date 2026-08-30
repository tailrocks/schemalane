//! Nextest-discoverable mirrors of public documentation examples.

#[test]
fn quick_start_surface_compiles() {
    async fn demo(pool: deadpool_postgres::Pool) -> Result<(), schemalane_core::SchemalaneError> {
        let config = schemalane_core::SchemalaneConfig::default();
        let migrator = schemalane_core::SchemalaneMigrator::new(config);
        let _report = migrator.up(&pool).await?;
        Ok(())
    }

    let _ = demo;
}
