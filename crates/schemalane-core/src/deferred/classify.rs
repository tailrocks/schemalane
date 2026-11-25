//! Statement classification for deferral: which statements of a migration are moved into the
//! deferred set, and which later statements force deferred entries to be applied early.

use pg_query::NodeEnum;
use pg_query::protobuf::{
    AlterTableCmd, AlterTableStmt, AlterTableType, ColumnDef, ConstrType, Constraint, CreateStmt,
    IndexStmt, Node, ObjectType, RangeVar,
};

use super::{DeferPolicy, DeferredEntry, DeferredKind};
use crate::SchemalaneError;
use crate::sql_analysis::ParsedSqlStatement;

/// One statement to store in the deferred set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NewEntry {
    pub(crate) kind: DeferredKind,
    pub(crate) relation_schema: Option<String>,
    pub(crate) relation_name: String,
    pub(crate) referenced_relation: Option<String>,
    pub(crate) object_name: Option<String>,
    pub(crate) sql: String,
}

/// Result of classifying one migration statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Classified {
    /// SQL still to execute now, or `None` when the whole statement is deferred.
    pub(crate) run: Option<String>,
    /// Statements moved into the deferred set, in execution order.
    pub(crate) deferred: Vec<NewEntry>,
}

impl Classified {
    fn passthrough(statement: &ParsedSqlStatement) -> Self {
        Self {
            run: Some(statement.sql.clone()),
            deferred: Vec::new(),
        }
    }
}

fn internal(error: &pg_query::Error) -> SchemalaneError {
    SchemalaneError::Internal(format!("failed to deparse a deferred statement: {error}"))
}

fn deparse(node: NodeEnum) -> Result<String, SchemalaneError> {
    node.deparse(pg_query::DeparseOptions::default())
        .map_err(|error| internal(&error))
}

fn wrap(node: NodeEnum) -> Node {
    Node { node: Some(node) }
}

fn relation_parts(relation: &RangeVar) -> (Option<String>, String) {
    let schema = (!relation.schemaname.is_empty()).then(|| relation.schemaname.clone());
    (schema, relation.relname.clone())
}

fn non_empty(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_owned())
}

/// Classifies one parsed migration statement under `policy`.
pub(crate) fn classify(
    statement: &ParsedSqlStatement,
    policy: &DeferPolicy,
) -> Result<Classified, SchemalaneError> {
    match statement.node.as_ref().and_then(|node| node.node.as_ref()) {
        Some(NodeEnum::IndexStmt(index)) => Ok(classify_index(statement, index, policy)),
        Some(NodeEnum::AlterTableStmt(alter)) => classify_alter_table(statement, alter, policy),
        Some(NodeEnum::CreateStmt(create)) => classify_create_table(statement, create, policy),
        _ => Ok(Classified::passthrough(statement)),
    }
}

fn classify_index(
    statement: &ParsedSqlStatement,
    index: &IndexStmt,
    policy: &DeferPolicy,
) -> Classified {
    let wanted = if index.unique {
        policy.unique_indexes
    } else {
        policy.secondary_indexes
    };
    let Some(relation) = index.relation.as_ref() else {
        return Classified::passthrough(statement);
    };
    // Constraint-backed indexes (primary key, unique constraint) are never deferred.
    if !wanted || index.primary || index.isconstraint || relation.relpersistence == "t" {
        return Classified::passthrough(statement);
    }
    let (relation_schema, relation_name) = relation_parts(relation);
    Classified {
        run: None,
        deferred: vec![NewEntry {
            kind: if index.unique {
                DeferredKind::UniqueIndex
            } else {
                DeferredKind::Index
            },
            relation_schema,
            relation_name,
            referenced_relation: None,
            object_name: non_empty(&index.idxname),
            sql: statement.sql.clone(),
        }],
    }
}

fn foreign_key(node: &Node) -> Option<&Constraint> {
    match node.node.as_ref()? {
        NodeEnum::Constraint(constraint)
            if constraint.contype == ConstrType::ConstrForeign as i32 =>
        {
            Some(constraint)
        }
        _ => None,
    }
}

fn add_constraint_cmd(constraint: Constraint) -> Node {
    wrap(NodeEnum::AlterTableCmd(Box::new(AlterTableCmd {
        subtype: AlterTableType::AtAddConstraint as i32,
        def: Some(Box::new(wrap(NodeEnum::Constraint(Box::new(constraint))))),
        ..AlterTableCmd::default()
    })))
}

fn alter_table_sql(
    relation: &RangeVar,
    cmds: Vec<Node>,
    missing_ok: bool,
) -> Result<String, SchemalaneError> {
    deparse(NodeEnum::AlterTableStmt(AlterTableStmt {
        relation: Some(relation.clone()),
        cmds,
        objtype: ObjectType::ObjectTable as i32,
        missing_ok,
    }))
}

fn fk_entry(
    relation: &RangeVar,
    constraint: &Constraint,
    cmd: Node,
    missing_ok: bool,
) -> Result<NewEntry, SchemalaneError> {
    let (relation_schema, relation_name) = relation_parts(relation);
    Ok(NewEntry {
        kind: DeferredKind::ForeignKey,
        relation_schema,
        relation_name,
        referenced_relation: constraint
            .pktable
            .as_ref()
            .map(|pktable| pktable.relname.clone()),
        object_name: non_empty(&constraint.conname),
        sql: alter_table_sql(relation, vec![cmd], missing_ok)?,
    })
}

/// Removes foreign-key column constraints from `column` and returns them as table-level
/// constraints that name the column.
fn take_column_foreign_keys(column: &mut ColumnDef) -> Vec<Constraint> {
    let mut taken = Vec::new();
    let mut kept = Vec::new();
    for node in std::mem::take(&mut column.constraints) {
        if let Some(constraint) = foreign_key(&node) {
            let mut constraint = constraint.clone();
            constraint.fk_attrs = vec![wrap(NodeEnum::String(pg_query::protobuf::String {
                sval: column.colname.clone(),
            }))];
            taken.push(constraint);
        } else {
            kept.push(node);
        }
    }
    column.constraints = kept;
    taken
}

fn classify_alter_table(
    statement: &ParsedSqlStatement,
    alter: &AlterTableStmt,
    policy: &DeferPolicy,
) -> Result<Classified, SchemalaneError> {
    let Some(relation) = alter.relation.as_ref() else {
        return Ok(Classified::passthrough(statement));
    };
    if !policy.foreign_keys
        || alter.objtype != ObjectType::ObjectTable as i32
        || relation.relpersistence == "t"
    {
        return Ok(Classified::passthrough(statement));
    }
    let mut kept = Vec::new();
    let mut deferred = Vec::new();
    for cmd_node in &alter.cmds {
        let Some(NodeEnum::AlterTableCmd(cmd)) = cmd_node.node.as_ref() else {
            kept.push(cmd_node.clone());
            continue;
        };
        let def = cmd.def.as_deref().and_then(|def| def.node.as_ref());
        if cmd.subtype == AlterTableType::AtAddConstraint as i32
            && let Some(constraint) = cmd.def.as_deref().and_then(foreign_key)
        {
            deferred.push(fk_entry(
                relation,
                constraint,
                cmd_node.clone(),
                alter.missing_ok,
            )?);
        } else if cmd.subtype == AlterTableType::AtAddColumn as i32
            && let Some(NodeEnum::ColumnDef(column)) = def
        {
            let mut column = (**column).clone();
            let foreign_keys = take_column_foreign_keys(&mut column);
            if foreign_keys.is_empty() {
                kept.push(cmd_node.clone());
                continue;
            }
            let mut stripped = (**cmd).clone();
            stripped.def = Some(Box::new(wrap(NodeEnum::ColumnDef(Box::new(column)))));
            kept.push(wrap(NodeEnum::AlterTableCmd(Box::new(stripped))));
            for constraint in foreign_keys {
                let cmd = add_constraint_cmd(constraint.clone());
                deferred.push(fk_entry(relation, &constraint, cmd, alter.missing_ok)?);
            }
        } else {
            kept.push(cmd_node.clone());
        }
    }
    if deferred.is_empty() {
        return Ok(Classified::passthrough(statement));
    }
    let run = if kept.is_empty() {
        None
    } else {
        Some(alter_table_sql(relation, kept, alter.missing_ok)?)
    };
    Ok(Classified { run, deferred })
}

fn classify_create_table(
    statement: &ParsedSqlStatement,
    create: &CreateStmt,
    policy: &DeferPolicy,
) -> Result<Classified, SchemalaneError> {
    let Some(relation) = create.relation.as_ref() else {
        return Ok(Classified::passthrough(statement));
    };
    // `IF NOT EXISTS` may skip the whole definition, temporary tables vanish with the session,
    // and partitions inherit their foreign keys from the parent.
    if !policy.foreign_keys
        || create.if_not_exists
        || create.partbound.is_some()
        || relation.relpersistence == "t"
    {
        return Ok(Classified::passthrough(statement));
    }
    let mut stripped = create.clone();
    let mut constraints = Vec::new();
    let mut elements = Vec::with_capacity(stripped.table_elts.len());
    for element in std::mem::take(&mut stripped.table_elts) {
        match element.node.as_ref() {
            Some(NodeEnum::ColumnDef(column)) => {
                let mut column = (**column).clone();
                constraints.extend(take_column_foreign_keys(&mut column));
                elements.push(wrap(NodeEnum::ColumnDef(Box::new(column))));
            }
            _ => {
                if let Some(constraint) = foreign_key(&element) {
                    constraints.push(constraint.clone());
                } else {
                    elements.push(element);
                }
            }
        }
    }
    if constraints.is_empty() {
        return Ok(Classified::passthrough(statement));
    }
    stripped.table_elts = elements;
    let mut deferred = Vec::with_capacity(constraints.len());
    for constraint in constraints {
        let cmd = add_constraint_cmd(constraint.clone());
        deferred.push(fk_entry(relation, &constraint, cmd, false)?);
    }
    Ok(Classified {
        run: Some(deparse(NodeEnum::CreateStmt(stripped))?),
        deferred,
    })
}

/// Returns true for statements that cannot change or depend on an existing deferred object.
fn is_additive(node: &NodeEnum) -> bool {
    match node {
        NodeEnum::CreateStmt(_)
        | NodeEnum::IndexStmt(_)
        | NodeEnum::CommentStmt(_)
        | NodeEnum::GrantStmt(_)
        | NodeEnum::ViewStmt(_)
        | NodeEnum::CreateFunctionStmt(_)
        | NodeEnum::CreateTrigStmt(_)
        | NodeEnum::CreateSeqStmt(_)
        | NodeEnum::AlterSeqStmt(_)
        | NodeEnum::CreateSchemaStmt(_)
        | NodeEnum::CreateExtensionStmt(_)
        | NodeEnum::CreateEnumStmt(_)
        | NodeEnum::CompositeTypeStmt(_)
        | NodeEnum::CreateDomainStmt(_)
        | NodeEnum::DefineStmt(_)
        | NodeEnum::VariableSetStmt(_)
        | NodeEnum::CreateRoleStmt(_)
        | NodeEnum::CreateTableAsStmt(_)
        | NodeEnum::SelectStmt(_)
        | NodeEnum::CopyStmt(_)
        | NodeEnum::TruncateStmt(_) => true,
        NodeEnum::InsertStmt(insert) => insert.on_conflict_clause.is_none(),
        NodeEnum::AlterTableStmt(alter) => alter.cmds.iter().all(|cmd| {
            matches!(
                cmd.node.as_ref(),
                Some(NodeEnum::AlterTableCmd(cmd)) if is_additive_alter_cmd(cmd.subtype)
            )
        }),
        _ => false,
    }
}

fn is_additive_alter_cmd(subtype: i32) -> bool {
    [
        AlterTableType::AtAddColumn,
        AlterTableType::AtColumnDefault,
        AlterTableType::AtAddConstraint,
        AlterTableType::AtSetNotNull,
        AlterTableType::AtDropNotNull,
        AlterTableType::AtSetStatistics,
    ]
    .iter()
    .any(|kind| *kind as i32 == subtype)
}

/// Statements whose effect is not visible to the parser (procedural code, dynamic SQL).
const fn is_opaque(node: &NodeEnum) -> bool {
    matches!(
        node,
        NodeEnum::DoStmt(_) | NodeEnum::CallStmt(_) | NodeEnum::ExecuteStmt(_)
    )
}

fn names_in(node: &NodeEnum) -> Vec<String> {
    use pg_query::NodeRef;
    node.nodes()
        .into_iter()
        .filter_map(|(node, ..)| match node {
            NodeRef::RangeVar(range_var) => Some(range_var.relname.clone()),
            NodeRef::String(string) => Some(string.sval.clone()),
            _ => None,
        })
        .collect()
}

fn dml_target(node: &NodeEnum) -> Option<&str> {
    let relation = match node {
        NodeEnum::DeleteStmt(stmt) => stmt.relation.as_ref(),
        NodeEnum::UpdateStmt(stmt) => stmt.relation.as_ref(),
        NodeEnum::InsertStmt(stmt) => stmt.relation.as_ref(),
        NodeEnum::MergeStmt(stmt) => stmt.relation.as_ref(),
        _ => None,
    };
    relation.map(|relation| relation.relname.as_str())
}

/// Returns the pending entries that must be applied before `node` executes so that it sees
/// the same schema it would see after a plain migration.
pub(crate) fn entries_to_flush<'a>(
    node: &NodeEnum,
    pending: &'a [DeferredEntry],
) -> Vec<&'a DeferredEntry> {
    if pending.is_empty() {
        return Vec::new();
    }
    if is_opaque(node) {
        return pending.iter().collect();
    }
    if let Some(target) = dml_target(node) {
        return flush_for_dml(node, target, pending);
    }
    if is_additive(node) {
        return Vec::new();
    }
    let names = names_in(node);
    pending
        .iter()
        .filter(|entry| entry_names(entry).any(|name| names.iter().any(|n| n == name)))
        .collect()
}

fn entry_names(entry: &DeferredEntry) -> impl Iterator<Item = &str> {
    [
        Some(entry.relation_name.as_str()),
        entry.referenced_relation.as_deref(),
        entry.object_name.as_deref(),
        entry.resolved_name.as_deref(),
    ]
    .into_iter()
    .flatten()
}

/// Only referential actions (`ON DELETE`/`ON UPDATE`) and `ON CONFLICT` arbiters change what
/// data statements do; plain indexes never do.
fn flush_for_dml<'a>(
    node: &NodeEnum,
    target: &str,
    pending: &'a [DeferredEntry],
) -> Vec<&'a DeferredEntry> {
    let mutates_referenced_rows = matches!(node, NodeEnum::DeleteStmt(_) | NodeEnum::UpdateStmt(_));
    let needs_arbiter = matches!(node, NodeEnum::MergeStmt(_))
        || matches!(node, NodeEnum::InsertStmt(insert) if insert.on_conflict_clause.is_some());
    pending
        .iter()
        .filter(|entry| match entry.kind {
            DeferredKind::ForeignKey => {
                mutates_referenced_rows && entry.referenced_relation.as_deref() == Some(target)
            }
            DeferredKind::UniqueIndex => needs_arbiter && entry.relation_name == target,
            DeferredKind::Index => false,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deferred::DeferredStatus;
    use crate::sql_analysis::parse_sql_migration;

    fn classify_sql(sql: &str, policy: &DeferPolicy) -> Classified {
        let statements = parse_sql_migration(sql).expect("parse");
        assert_eq!(statements.len(), 1);
        classify(&statements[0], policy).expect("classify")
    }

    fn default_policy() -> DeferPolicy {
        DeferPolicy::secondary_indexes_and_foreign_keys()
    }

    fn pending_entry(kind: DeferredKind, relation: &str, object: Option<&str>) -> DeferredEntry {
        DeferredEntry {
            seq: 1,
            version: "1".into(),
            script: "V1__x.sql".into(),
            statement_ordinal: 1,
            item_ordinal: 0,
            kind,
            relation_schema: None,
            relation_name: relation.into(),
            referenced_relation: None,
            object_name: object.map(ToOwned::to_owned),
            resolved_name: None,
            sql: String::new(),
            status: DeferredStatus::Pending,
            applied_on: None,
            execution_time_ms: None,
            flushed: false,
        }
    }

    fn node_of(sql: &str) -> NodeEnum {
        let statements = parse_sql_migration(sql).expect("parse");
        statements[0]
            .node
            .as_ref()
            .and_then(|node| node.node.clone())
            .expect("node")
    }

    #[test]
    fn defers_secondary_index_verbatim() {
        let sql = "CREATE INDEX idx_a ON t (a) WHERE a > 1";
        let classified = classify_sql(sql, &default_policy());
        assert_eq!(classified.run, None);
        assert_eq!(classified.deferred.len(), 1);
        let entry = &classified.deferred[0];
        assert_eq!(entry.kind, DeferredKind::Index);
        assert_eq!(entry.relation_name, "t");
        assert_eq!(entry.object_name.as_deref(), Some("idx_a"));
        assert_eq!(entry.sql, sql);
    }

    #[test]
    fn unique_index_is_kept_unless_policy_asks() {
        let sql = "CREATE UNIQUE INDEX u ON t (a)";
        let kept = classify_sql(sql, &default_policy());
        assert_eq!(kept.run.as_deref(), Some(sql));
        assert!(kept.deferred.is_empty());
        let policy = default_policy().with_unique_indexes(true);
        let deferred = classify_sql(sql, &policy);
        assert_eq!(deferred.deferred[0].kind, DeferredKind::UniqueIndex);
    }

    #[test]
    fn plain_statements_pass_through() {
        for sql in [
            "CREATE TABLE t (id int PRIMARY KEY, a int UNIQUE)",
            "INSERT INTO t VALUES (1)",
            "ALTER TABLE t ADD COLUMN b int",
            "ALTER TABLE t ADD CONSTRAINT c CHECK (a > 0)",
        ] {
            let classified = classify_sql(sql, &default_policy());
            assert_eq!(classified.run.as_deref(), Some(sql), "{sql}");
            assert!(classified.deferred.is_empty(), "{sql}");
        }
    }

    #[test]
    fn defers_alter_table_foreign_key() {
        let classified = classify_sql(
            "ALTER TABLE child ADD CONSTRAINT child_parent_fk FOREIGN KEY (parent_id) REFERENCES parent (id) ON DELETE CASCADE",
            &default_policy(),
        );
        assert_eq!(classified.run, None);
        let entry = &classified.deferred[0];
        assert_eq!(entry.kind, DeferredKind::ForeignKey);
        assert_eq!(entry.relation_name, "child");
        assert_eq!(entry.referenced_relation.as_deref(), Some("parent"));
        assert_eq!(entry.object_name.as_deref(), Some("child_parent_fk"));
        assert!(entry.sql.contains("FOREIGN KEY"), "{}", entry.sql);
        assert!(entry.sql.contains("ON DELETE CASCADE"), "{}", entry.sql);
    }

    #[test]
    fn alter_table_keeps_non_foreign_key_commands() {
        let classified = classify_sql(
            "ALTER TABLE child ADD COLUMN note text, ADD FOREIGN KEY (parent_id) REFERENCES parent (id)",
            &default_policy(),
        );
        let run = classified.run.expect("remaining command runs");
        assert!(run.contains("ADD COLUMN note"), "{run}");
        assert!(!run.contains("FOREIGN KEY"), "{run}");
        assert_eq!(classified.deferred.len(), 1);
    }

    #[test]
    fn strips_inline_foreign_keys_from_create_table() {
        let classified = classify_sql(
            "CREATE TABLE child (id int PRIMARY KEY, parent_id int REFERENCES parent (id), other_id int, FOREIGN KEY (other_id) REFERENCES other (id))",
            &default_policy(),
        );
        let run = classified.run.expect("table is still created");
        assert!(run.contains("PRIMARY KEY"), "{run}");
        assert!(!run.contains("REFERENCES"), "{run}");
        assert_eq!(classified.deferred.len(), 2);
        let first = &classified.deferred[0];
        assert!(first.sql.contains("FOREIGN KEY (parent_id)"), "{}", first.sql);
        assert_eq!(first.referenced_relation.as_deref(), Some("parent"));
        assert_eq!(classified.deferred[1].referenced_relation.as_deref(), Some("other"));
    }

    #[test]
    fn strips_add_column_foreign_key() {
        let classified = classify_sql(
            "ALTER TABLE child ADD COLUMN parent_id int REFERENCES parent (id)",
            &default_policy(),
        );
        let run = classified.run.expect("column is still added");
        assert!(run.contains("ADD COLUMN parent_id"), "{run}");
        assert!(!run.contains("REFERENCES"), "{run}");
        assert!(classified.deferred[0].sql.contains("FOREIGN KEY (parent_id)"));
    }

    #[test]
    fn create_table_if_not_exists_and_temp_tables_are_kept() {
        for sql in [
            "CREATE TABLE IF NOT EXISTS c (p int REFERENCES parent (id))",
            "CREATE TEMP TABLE c (p int REFERENCES parent (id))",
        ] {
            let classified = classify_sql(sql, &default_policy());
            assert_eq!(classified.run.as_deref(), Some(sql));
            assert!(classified.deferred.is_empty());
        }
    }

    #[test]
    fn disabled_kinds_are_not_deferred() {
        let policy = DeferPolicy::none();
        for sql in [
            "CREATE INDEX i ON t (a)",
            "ALTER TABLE c ADD FOREIGN KEY (p) REFERENCES parent (id)",
        ] {
            let classified = classify_sql(sql, &policy);
            assert_eq!(classified.run.as_deref(), Some(sql));
            assert!(classified.deferred.is_empty());
        }
    }

    #[test]
    fn destructive_statements_flush_matching_entries() {
        let pending = vec![
            pending_entry(DeferredKind::Index, "t", Some("idx_t_a")),
            pending_entry(DeferredKind::Index, "other", Some("idx_other")),
        ];
        let flush = |sql: &str| {
            entries_to_flush(&node_of(sql), &pending)
                .into_iter()
                .map(|entry| entry.relation_name.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(flush("DROP INDEX idx_t_a"), ["t"]);
        assert_eq!(flush("ALTER INDEX idx_other RENAME TO idx_o2"), ["other"]);
        assert_eq!(flush("ALTER TABLE t DROP COLUMN a"), ["t"]);
        assert_eq!(flush("ALTER TABLE t ALTER COLUMN a TYPE bigint"), ["t"]);
        assert_eq!(flush("DROP TABLE t"), ["t"]);
        assert_eq!(flush("ALTER TABLE t RENAME TO t2"), ["t"]);
        assert!(flush("DROP TABLE unrelated").is_empty());
        assert_eq!(flush("DO $$ BEGIN PERFORM 1; END $$").len(), 2);
    }

    #[test]
    fn additive_and_plain_dml_statements_do_not_flush() {
        let pending = vec![pending_entry(DeferredKind::Index, "t", Some("idx_t_a"))];
        for sql in [
            "ALTER TABLE t ADD COLUMN b int",
            "CREATE INDEX idx_b ON t (b)",
            "COMMENT ON TABLE t IS 'x'",
            "INSERT INTO t VALUES (1)",
            "UPDATE t SET a = 1",
            "DELETE FROM t",
            "SELECT 1 FROM t",
        ] {
            assert!(
                entries_to_flush(&node_of(sql), &pending).is_empty(),
                "{sql}"
            );
        }
    }

    #[test]
    fn dml_flushes_only_semantic_entries() {
        let mut foreign_key = pending_entry(DeferredKind::ForeignKey, "child", None);
        foreign_key.referenced_relation = Some("parent".into());
        let unique = pending_entry(DeferredKind::UniqueIndex, "t", Some("u"));
        let pending = vec![foreign_key, unique];
        let count = |sql: &str| entries_to_flush(&node_of(sql), &pending).len();
        assert_eq!(count("DELETE FROM parent WHERE id = 1"), 1);
        assert_eq!(count("DELETE FROM child WHERE id = 1"), 0);
        assert_eq!(count("INSERT INTO t VALUES (1) ON CONFLICT DO NOTHING"), 1);
        assert_eq!(count("INSERT INTO t VALUES (1)"), 0);
    }
}
