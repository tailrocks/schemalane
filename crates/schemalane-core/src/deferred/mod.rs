//! Deferred indexes and foreign keys for bulk-load workflows.
//!
//! [`SchemalaneMigrator::up_deferred`](crate::SchemalaneMigrator::up_deferred) applies the
//! migrations like `up`, except that statements matching a [`DeferPolicy`] (secondary
//! indexes, foreign keys) are recorded in a sidecar table instead of executed. After the
//! bulk load, [`SchemalaneMigrator::apply_deferred`](crate::SchemalaneMigrator::apply_deferred)
//! executes them. Migration files and their Flyway checksums are unchanged, and the
//! `flyway_schema_history` rows are identical to those of a plain `up`.

mod api;
mod classify;
mod run;
mod store;

use serde::Serialize;

pub(crate) use classify::entries_to_flush;
pub(crate) use run::{Deferral, Prepared};
pub(crate) use store::DeferredRepository;

use crate::SchemalaneError;

/// Which statements of a migration are deferred.
///
/// Primary keys, `UNIQUE` and `CHECK` constraints, exclusion constraints and everything
/// else always run in place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct DeferPolicy {
    /// Defer `CREATE INDEX` (non-unique).
    pub secondary_indexes: bool,
    /// Defer `CREATE UNIQUE INDEX`. Off by default: a unique index can be load-bearing
    /// (`ON CONFLICT` arbiter) and rejects bad rows during the load.
    pub unique_indexes: bool,
    /// Defer `ALTER TABLE ... ADD FOREIGN KEY` and the inline foreign keys of
    /// `CREATE TABLE` and `ALTER TABLE ... ADD COLUMN`.
    pub foreign_keys: bool,
}

impl DeferPolicy {
    /// Defers non-unique indexes and foreign keys.
    #[must_use]
    pub const fn secondary_indexes_and_foreign_keys() -> Self {
        Self {
            secondary_indexes: true,
            unique_indexes: false,
            foreign_keys: true,
        }
    }

    /// Defers nothing; a deferred run with this policy equals a plain `up`.
    #[must_use]
    pub const fn none() -> Self {
        Self {
            secondary_indexes: false,
            unique_indexes: false,
            foreign_keys: false,
        }
    }

    /// Also defers (or stops deferring) `CREATE UNIQUE INDEX`.
    #[must_use]
    pub const fn with_unique_indexes(mut self, value: bool) -> Self {
        self.unique_indexes = value;
        self
    }

    /// Sets whether non-unique indexes are deferred.
    #[must_use]
    pub const fn with_secondary_indexes(mut self, value: bool) -> Self {
        self.secondary_indexes = value;
        self
    }

    /// Sets whether foreign keys are deferred.
    #[must_use]
    pub const fn with_foreign_keys(mut self, value: bool) -> Self {
        self.foreign_keys = value;
        self
    }
}

impl Default for DeferPolicy {
    fn default() -> Self {
        Self::secondary_indexes_and_foreign_keys()
    }
}

/// What a deferred statement creates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum DeferredKind {
    /// A non-unique index.
    Index,
    /// A unique index.
    UniqueIndex,
    /// A foreign-key constraint.
    ForeignKey,
}

impl DeferredKind {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Index => "INDEX",
            Self::UniqueIndex => "UNIQUE_INDEX",
            Self::ForeignKey => "FOREIGN_KEY",
        }
    }

    pub(crate) fn parse(value: &str) -> Result<Self, SchemalaneError> {
        match value {
            "INDEX" => Ok(Self::Index),
            "UNIQUE_INDEX" => Ok(Self::UniqueIndex),
            "FOREIGN_KEY" => Ok(Self::ForeignKey),
            other => Err(SchemalaneError::Internal(format!(
                "unknown deferred kind '{other}'"
            ))),
        }
    }

    pub(crate) const fn is_index(self) -> bool {
        matches!(self, Self::Index | Self::UniqueIndex)
    }
}

/// Whether a deferred statement has been executed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum DeferredStatus {
    /// Recorded but not executed yet.
    Pending,
    /// Executed.
    Applied,
}

impl DeferredStatus {
    pub(crate) fn parse(value: &str) -> Result<Self, SchemalaneError> {
        match value {
            "pending" => Ok(Self::Pending),
            "applied" => Ok(Self::Applied),
            other => Err(SchemalaneError::Internal(format!(
                "unknown deferred status '{other}'"
            ))),
        }
    }
}

/// One statement in the deferred set.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct DeferredEntry {
    /// Execution order across all migrations.
    pub seq: i64,
    /// Version of the migration the statement came from.
    pub version: String,
    /// Filename of the migration the statement came from.
    pub script: String,
    /// One-based position of the statement within the migration file.
    pub statement_ordinal: i32,
    /// Position among the entries produced by that statement.
    pub item_ordinal: i32,
    /// What the statement creates.
    pub kind: DeferredKind,
    /// Schema written on the table in the migration, if any.
    pub relation_schema: Option<String>,
    /// Table the index or foreign key belongs to.
    pub relation_name: String,
    /// Table a foreign key references.
    pub referenced_relation: Option<String>,
    /// Index or constraint name written in the migration, if any.
    pub object_name: Option<String>,
    /// Name of the created object, recorded when the entry is applied.
    pub resolved_name: Option<String>,
    /// SQL that creates the object.
    pub sql: String,
    /// Whether the statement has been executed.
    pub status: DeferredStatus,
    /// Time the statement was executed, rendered as text.
    pub applied_on: Option<String>,
    /// Execution duration in milliseconds.
    pub execution_time_ms: Option<i32>,
    /// A later migration forced the statement to run during `up_deferred`.
    pub flushed: bool,
    /// Every identifier the statement mentions (tables, columns, index and constraint
    /// names); used to decide whether a later statement touches the object.
    pub idents: Vec<String>,
}

impl DeferredEntry {
    /// Name under which the object exists once applied, when known.
    #[must_use]
    pub fn created_name(&self) -> Option<&str> {
        self.resolved_name
            .as_deref()
            .or(self.object_name.as_deref())
    }
}

/// How foreign keys are added when the deferred set is applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum FkMode {
    /// `ADD CONSTRAINT ... NOT VALID`, then `VALIDATE CONSTRAINT`: the second step only
    /// takes a `SHARE UPDATE EXCLUSIVE` lock. A constraint written `NOT VALID` in the
    /// migration is left unvalidated.
    #[default]
    NotValidThenValidate,
    /// Execute the statement as written.
    Direct,
}

/// Options for [`SchemalaneMigrator::apply_deferred`](crate::SchemalaneMigrator::apply_deferred).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ApplyDeferredOptions {
    /// Build indexes with `CREATE INDEX CONCURRENTLY`. Slower, but does not block writes.
    pub concurrently: bool,
    /// Number of indexes built at the same time (foreign keys always run one by one).
    /// Capped at the pool's maximum size.
    pub parallelism: usize,
    /// How foreign keys are added.
    pub fk_mode: FkMode,
    /// Session settings applied to every connection that builds objects, for example
    /// `("maintenance_work_mem", "2GB")` or `("max_parallel_maintenance_workers", "4")`.
    pub session_settings: Vec<(String, String)>,
}

impl Default for ApplyDeferredOptions {
    fn default() -> Self {
        Self {
            concurrently: false,
            parallelism: 1,
            fk_mode: FkMode::default(),
            session_settings: Vec::new(),
        }
    }
}

impl ApplyDeferredOptions {
    /// Returns the default options.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Builds indexes with `CREATE INDEX CONCURRENTLY`.
    #[must_use]
    pub const fn with_concurrently(mut self, value: bool) -> Self {
        self.concurrently = value;
        self
    }

    /// Sets how many indexes are built at the same time.
    #[must_use]
    pub const fn with_parallelism(mut self, value: usize) -> Self {
        self.parallelism = value;
        self
    }

    /// Sets how foreign keys are added.
    #[must_use]
    pub const fn with_fk_mode(mut self, value: FkMode) -> Self {
        self.fk_mode = value;
        self
    }

    /// Adds a session setting applied to every connection that builds objects.
    #[must_use]
    pub fn with_session_setting(
        mut self,
        name: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        self.session_settings.push((name.into(), value.into()));
        self
    }
}

/// The deferred set of one migration target.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct DeferredReport {
    /// Managed schema.
    pub schema: String,
    /// Name of the sidecar table.
    pub table: String,
    /// Number of entries not executed yet.
    pub pending: usize,
    /// Number of entries already executed.
    pub applied: usize,
    /// Every entry in execution order.
    pub entries: Vec<DeferredEntry>,
}

/// Result of applying the deferred set.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct ApplyDeferredReport {
    /// Entries executed by this call, in execution order.
    pub applied: Vec<DeferredEntry>,
    /// Entries that were already applied.
    pub already_applied: usize,
}

/// Catalog state of one deferred entry.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct VerifiedEntry {
    /// The entry that was checked.
    pub entry: DeferredEntry,
    /// The object exists in the catalog.
    pub exists: bool,
    /// The object is usable: `indisvalid` and `indisready` for an index, `convalidated`
    /// for a foreign key.
    pub valid: bool,
}

/// Result of checking the deferred set against the catalogs.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct DeferredVerification {
    /// Every entry with its catalog state.
    pub entries: Vec<VerifiedEntry>,
    /// Every entry is applied, exists and is valid.
    pub ok: bool,
}
