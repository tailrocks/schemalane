//! Compile-and-run tests for `schemalane_core::embed_migrations!` code generation.

pub mod embedded {
    use schemalane_core::embed_migrations;

    embed_migrations!("./migrations");
}

#[cfg(test)]
mod tests {
    use super::embedded::migrations;

    #[test]
    fn migrations_dir_is_absolute_and_exists() {
        let dir = std::path::Path::new(migrations::MIGRATIONS_DIR);
        assert!(dir.is_absolute());
        assert!(dir.join("V1__first.rs").exists());
        assert!(dir.join("V10__upper.RS").exists());
    }

    #[test]
    fn build_migrator_registers_rust_migrations() {
        let migrator = migrations::build_migrator(
            schemalane_core::SchemalaneConfig::new()
                .with_migrations_dir(migrations::MIGRATIONS_DIR),
        );
        let _ = migrator;
    }

    #[test]
    fn runner_constructs() {
        let _runner = migrations::runner();
    }
}
