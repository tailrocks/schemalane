use std::fmt::Write;

use pg_query::protobuf::node::Node;
use pg_query::protobuf::{
    AlterTableStmt, AlterTableType, ColumnDef, ConstrType, Constraint, CreateEnumStmt,
    CreateFunctionStmt, CreateStmt, DropBehavior, FunctionParameterMode, IndexStmt, ViewStmt,
};

use crate::expr::{fmt_index_elem, fmt_node, fmt_range_var, fmt_type_name, quote_identifier};
use crate::{FormatError, INDENT};

use super::table_body::{ColumnParts, TableItem, column_widths, fmt_column_line, fmt_table_body};
use super::{name_list_to_string, node_string_list};

pub(crate) fn fmt_create_enum(stmt: &CreateEnumStmt) -> Result<String, FormatError> {
    let type_name = name_list_to_string(&stmt.type_name);

    let vals: Vec<String> = stmt
        .vals
        .iter()
        .map(|n| match n.node.as_ref() {
            Some(Node::String(s)) => Ok(format!("'{}'", s.sval.replace('\'', "''"))),
            _ => fmt_node(n),
        })
        .collect::<Result<_, _>>()?;

    if vals.len() <= 1 {
        return Ok(format!(
            "CREATE TYPE {type_name} AS ENUM ({})",
            vals.join(", ")
        ));
    }

    let mut out = format!("CREATE TYPE {type_name} AS ENUM (\n");
    for (i, val) in vals.iter().enumerate() {
        out.push_str(INDENT);
        out.push_str(val);
        if i + 1 < vals.len() {
            out.push(',');
        }
        out.push('\n');
    }
    out.push(')');
    Ok(out)
}

// ── CREATE TABLE ────────────────────────────────────────────────────────────

pub(crate) fn fmt_create_table(stmt: &CreateStmt) -> Result<String, FormatError> {
    let relation = stmt.relation.as_ref().map_or_default(fmt_range_var);

    let mut header = "CREATE TABLE".to_string();
    if stmt.if_not_exists {
        header.push_str(" IF NOT EXISTS");
    }
    header.push(' ');
    header.push_str(&relation);

    if stmt.table_elts.is_empty() {
        return Ok(format!("{header} ()"));
    }

    let mut columns: Vec<ColumnParts> = Vec::new();
    let mut all_items: Vec<TableItem> = Vec::new();

    for elt in &stmt.table_elts {
        match elt.node.as_ref() {
            Some(Node::ColumnDef(cd)) => {
                let parts = fmt_column_def_parts(cd)?;
                all_items.push(TableItem::Column(columns.len()));
                columns.push(parts);
            }
            Some(Node::Constraint(c)) => {
                let text = fmt_table_constraint(c)?;
                all_items.push(TableItem::Constraint(text));
            }
            _ => {
                let text = deparse_table_element(elt)?;
                all_items.push(TableItem::Constraint(text));
            }
        }
    }

    Ok(fmt_table_body(&header, &columns, &all_items))
}

fn fmt_column_def_parts(cd: &ColumnDef) -> Result<ColumnParts, FormatError> {
    let name = quote_identifier(&cd.colname);
    let type_str = cd
        .type_name
        .as_ref()
        .map(fmt_type_name)
        .transpose()?
        .unwrap_or_default();

    // DEFAULT is always extracted as a separate field for column alignment.
    // All other constraints go into the constraints string in their original
    // order (with DEFAULT removed).
    let mut default_expr: Option<String> = None;
    let mut constraint_parts: Vec<String> = Vec::new();

    for c in &cd.constraints {
        if let Some(Node::Constraint(con)) = c.node.as_ref() {
            match fmt_column_constraint(con)? {
                ColumnConstraint::Default(text) => default_expr = Some(text),
                ColumnConstraint::Other(text) => constraint_parts.push(text),
            }
        }
    }

    Ok(ColumnParts {
        name,
        type_str,
        default_expr,
        constraints: constraint_parts.join(" "),
    })
}

/// A column constraint, with DEFAULT apart so a table can align it in a column of its own.
enum ColumnConstraint {
    Default(String),
    Other(String),
}

/// Format one constraint of a column definition.
///
/// DEFAULT and CHECK keep the expression formatting of this crate. Every other kind goes through
/// the `pg_query` deparser, which covers each clause of the constraint (names, `NO INHERIT`,
/// `STORED` and `VIRTUAL`, identity options, deferral and `NULLS NOT DISTINCT`). A constraint is
/// never skipped: an unlisted kind used to vanish from the output and change the statement.
fn fmt_column_constraint(con: &Constraint) -> Result<ColumnConstraint, FormatError> {
    match ConstrType::try_from(con.contype).unwrap_or(ConstrType::Undefined) {
        ConstrType::ConstrDefault => {
            let mut s = String::from("DEFAULT");
            if let Some(ref raw) = con.raw_expr {
                s.push(' ');
                s.push_str(&fmt_node(raw)?);
            }
            Ok(ColumnConstraint::Default(s))
        }
        ConstrType::ConstrCheck => {
            let mut s = String::new();
            if !con.conname.is_empty() {
                s.push_str("CONSTRAINT ");
                s.push_str(&con.conname);
                s.push(' ');
            }
            s.push_str("CHECK (");
            if let Some(ref raw) = con.raw_expr {
                s.push_str(&fmt_node(raw)?);
            }
            s.push(')');
            if !con.is_enforced {
                s.push_str(" NOT ENFORCED");
            }
            Ok(ColumnConstraint::Other(s))
        }
        _ => deparse_column_constraint(con).map(ColumnConstraint::Other),
    }
}

/// Deparse one column constraint. The deparser rejects a bare `Constraint` node, so the
/// constraint goes into a one-column table that is deparsed whole, and the text around it is
/// cut off again.
fn deparse_column_constraint(con: &Constraint) -> Result<String, FormatError> {
    deparse_in_wrapper("CREATE TABLE t (c int ", |create| {
        match create
            .table_elts
            .first_mut()
            .and_then(|elt| elt.node.as_mut())
        {
            Some(Node::ColumnDef(column)) => {
                column.constraints.push(pg_query::protobuf::Node {
                    node: Some(Node::Constraint(Box::new(con.clone()))),
                });
                true
            }
            _ => false,
        }
    })
}

/// Deparse one table element that this crate has no formatting for, such as a `LIKE` clause or
/// a constraint with temporal keys. The deparser only handles these inside a whole table.
fn deparse_table_element(element: &pg_query::protobuf::Node) -> Result<String, FormatError> {
    deparse_in_wrapper("CREATE TABLE t (c int, ", |create| {
        create.table_elts.push(element.clone());
        true
    })
}

/// Parse `<prefix>)`, let `insert` add the element to the table, deparse the table and return
/// the text between the prefix and the closing parenthesis.
fn deparse_in_wrapper(
    prefix: &str,
    insert: impl FnOnce(&mut CreateStmt) -> bool,
) -> Result<String, FormatError> {
    let wrapper = |what: &str| FormatError::Deparse(format!("the deparse wrapper {what}"));
    let mut parsed = pg_query::parse("CREATE TABLE t (c int)", 0)
        .map_err(|e| FormatError::Deparse(e.to_string()))?;
    let create = parsed
        .protobuf
        .stmts
        .first_mut()
        .and_then(|raw| raw.stmt.as_mut())
        .and_then(|stmt| match stmt.node.as_mut() {
            Some(Node::CreateStmt(create)) => Some(create),
            _ => None,
        })
        .ok_or_else(|| wrapper("has no table"))?;
    if !insert(create) {
        return Err(wrapper("has no column"));
    }
    let text = pg_query::deparse(&parsed.protobuf, pg_query::DeparseOptions::default())
        .map_err(|e| FormatError::Deparse(e.to_string()))?;
    text.strip_prefix(prefix)
        .and_then(|rest| rest.strip_suffix(')'))
        .map(str::to_owned)
        .ok_or_else(|| FormatError::Deparse(format!("unexpected deparse of a table: {text}")))
}

/// Format one table-level constraint. CHECK keeps the expression formatting of this crate, and
/// so does a plain key or reference. Any constraint with a clause that formatting does not
/// render (temporal keys, `INCLUDE`, `MATCH`, deferral, `NULLS NOT DISTINCT`, index options)
/// goes through the `pg_query` deparser instead, so no clause can vanish.
fn fmt_table_constraint(con: &Constraint) -> Result<String, FormatError> {
    if !is_plain_table_constraint(con) {
        return deparse_table_element(&pg_query::protobuf::Node {
            node: Some(Node::Constraint(Box::new(con.clone()))),
        });
    }
    let mut s = String::new();
    if !con.conname.is_empty() {
        s.push_str("CONSTRAINT ");
        s.push_str(&con.conname);
        s.push(' ');
    }

    match ConstrType::try_from(con.contype).unwrap_or(ConstrType::Undefined) {
        ConstrType::ConstrPrimary => {
            s.push_str("PRIMARY KEY (");
            s.push_str(&node_string_list(&con.keys).join(", "));
            s.push(')');
        }
        ConstrType::ConstrUnique => {
            s.push_str("UNIQUE (");
            s.push_str(&node_string_list(&con.keys).join(", "));
            s.push(')');
        }
        ConstrType::ConstrCheck => {
            s.push_str("CHECK (");
            if let Some(ref raw) = con.raw_expr {
                s.push_str(&fmt_node(raw)?);
            }
            s.push(')');
            if !con.is_enforced {
                s.push_str(" NOT ENFORCED");
            }
        }
        ConstrType::ConstrForeign => {
            s.push_str("FOREIGN KEY (");
            s.push_str(&node_string_list(&con.fk_attrs).join(", "));
            s.push_str(") REFERENCES ");
            if let Some(ref pktable) = con.pktable {
                s.push_str(&fmt_range_var(pktable));
            }
            if !con.pk_attrs.is_empty() {
                s.push_str(" (");
                s.push_str(&node_string_list(&con.pk_attrs).join(", "));
                s.push(')');
            }
            append_fk_actions(&mut s, &con.fk_upd_action, &con.fk_del_action);
            if !con.is_enforced {
                s.push_str(" NOT ENFORCED");
            }
        }
        _ => unreachable!("is_plain_table_constraint admits only keys, references and CHECK"),
    }

    Ok(s)
}

/// Whether the plain formatting above renders every clause of the constraint. It does when the
/// constraint equals a bare one of its kind once the fields that formatting reads are cleared,
/// so a field that `pg_query` adds later makes the constraint non-plain instead of lost.
fn is_plain_table_constraint(con: &Constraint) -> bool {
    let (bare_sql, kind) = match ConstrType::try_from(con.contype) {
        Ok(ConstrType::ConstrPrimary) => ("PRIMARY KEY (a)", ConstrType::ConstrPrimary),
        Ok(ConstrType::ConstrUnique) if con.indexname.is_empty() => {
            ("UNIQUE (a)", ConstrType::ConstrUnique)
        }
        Ok(ConstrType::ConstrCheck) => ("CHECK (a > 0)", ConstrType::ConstrCheck),
        Ok(ConstrType::ConstrForeign) => (
            "FOREIGN KEY (a) REFERENCES p (a)",
            ConstrType::ConstrForeign,
        ),
        _ => return false,
    };
    let Some(bare) = bare_table_constraint(bare_sql, kind) else {
        return false;
    };
    read_fields_cleared(con) == read_fields_cleared(&bare)
}

fn bare_table_constraint(sql: &str, kind: ConstrType) -> Option<Constraint> {
    let parsed = pg_query::parse(&format!("CREATE TABLE t (a int, {sql})"), 0).ok()?;
    let stmt = parsed.protobuf.stmts.into_iter().next()?.stmt?;
    let Some(Node::CreateStmt(create)) = stmt.node else {
        return None;
    };
    create
        .table_elts
        .into_iter()
        .find_map(|elt| match elt.node {
            Some(Node::Constraint(con)) if con.contype == kind as i32 => Some(*con),
            _ => None,
        })
}

/// The constraint without the fields that `fmt_table_constraint` reads.
fn read_fields_cleared(con: &Constraint) -> Constraint {
    Constraint {
        conname: String::new(),
        raw_expr: None,
        keys: Vec::new(),
        pktable: None,
        fk_attrs: Vec::new(),
        pk_attrs: Vec::new(),
        fk_upd_action: String::new(),
        fk_del_action: String::new(),
        is_enforced: true,
        location: 0,
        ..con.clone()
    }
}

fn append_fk_actions(s: &mut String, upd_action: &str, del_action: &str) {
    if let Some(action) = fk_action_str(upd_action)
        && action != "NO ACTION"
    {
        s.push_str(" ON UPDATE ");
        s.push_str(action);
    }
    if let Some(action) = fk_action_str(del_action)
        && action != "NO ACTION"
    {
        s.push_str(" ON DELETE ");
        s.push_str(action);
    }
}

fn fk_action_str(action: &str) -> Option<&'static str> {
    match action {
        "a" => Some("NO ACTION"),
        "r" => Some("RESTRICT"),
        "c" => Some("CASCADE"),
        "n" => Some("SET NULL"),
        "d" => Some("SET DEFAULT"),
        _ => None,
    }
}

// ── CREATE FOREIGN TABLE ────────────────────────────────────────────────────

pub(crate) fn fmt_create_foreign_table(
    stmt: &pg_query::protobuf::CreateForeignTableStmt,
) -> Result<String, FormatError> {
    let base = stmt.base_stmt.as_ref().ok_or_else(|| {
        FormatError::Deparse("missing base_stmt in CreateForeignTableStmt".into())
    })?;

    let relation = base.relation.as_ref().map_or_default(fmt_range_var);

    let mut header = "CREATE FOREIGN TABLE".to_string();
    if base.if_not_exists {
        header.push_str(" IF NOT EXISTS");
    }
    header.push(' ');
    header.push_str(&relation);

    // Columns (reuse CREATE TABLE column formatting)
    if base.table_elts.is_empty() {
        header.push_str(" ()");
    } else {
        let mut columns: Vec<ColumnParts> = Vec::new();
        let mut all_items: Vec<TableItem> = Vec::new();

        for elt in &base.table_elts {
            match elt.node.as_ref() {
                Some(Node::ColumnDef(cd)) => {
                    let parts = fmt_column_def_parts(cd)?;
                    all_items.push(TableItem::Column(columns.len()));
                    columns.push(parts);
                }
                Some(Node::Constraint(c)) => {
                    let text = fmt_table_constraint(c)?;
                    all_items.push(TableItem::Constraint(text));
                }
                _ => {
                    let text = deparse_table_element(elt)?;
                    all_items.push(TableItem::Constraint(text));
                }
            }
        }
        header = fmt_table_body(&header, &columns, &all_items);
    }

    // SERVER
    if !stmt.servername.is_empty() {
        header.push_str(" SERVER ");
        header.push_str(&stmt.servername);
    }

    // OPTIONS
    if !stmt.options.is_empty() {
        let opts: Vec<String> = stmt
            .options
            .iter()
            .filter_map(|n| match n.node.as_ref() {
                Some(Node::DefElem(de)) => {
                    let val = de.arg.as_ref().and_then(|a| match a.node.as_ref() {
                        Some(Node::String(s)) => Some(format!("'{}'", s.sval.replace('\'', "''"))),
                        _ => fmt_node(a).ok(),
                    });
                    Some(if let Some(v) = val {
                        format!("{} {v}", de.defname)
                    } else {
                        de.defname.clone()
                    })
                }
                _ => None,
            })
            .collect();
        let _ = write!(header, " OPTIONS ({})", opts.join(", "));
    }

    Ok(header)
}

// ── CREATE INDEX ────────────────────────────────────────────────────────────

pub(crate) fn fmt_index_stmt(stmt: &IndexStmt) -> Result<String, FormatError> {
    // Line 1: CREATE [UNIQUE] INDEX [CONCURRENTLY] [IF NOT EXISTS] name
    let mut header = "CREATE".to_string();
    if stmt.unique {
        header.push_str(" UNIQUE");
    }
    header.push_str(" INDEX");
    if stmt.concurrent {
        header.push_str(" CONCURRENTLY");
    }
    if stmt.if_not_exists {
        header.push_str(" IF NOT EXISTS");
    }
    header.push(' ');
    header.push_str(&stmt.idxname);

    // Line 2: ON table [USING method](params...)
    let mut on_clause = format!("{INDENT}ON ");
    if let Some(ref rel) = stmt.relation {
        on_clause.push_str(&fmt_range_var(rel));
    }
    if !stmt.access_method.is_empty() {
        on_clause.push_str(" USING ");
        on_clause.push_str(&stmt.access_method);
    }

    let params: Vec<String> = stmt
        .index_params
        .iter()
        .map(|n| match n.node.as_ref() {
            Some(Node::IndexElem(ie)) => fmt_index_elem(ie),
            _ => fmt_node(n),
        })
        .collect::<Result<Vec<_>, _>>()?;

    let mut out = if params.len() <= 1 {
        format!("{header}\n{on_clause}({})", params.join(", "))
    } else {
        let mut s = format!("{header}\n{on_clause} (\n");
        for (i, param) in params.iter().enumerate() {
            s.push_str(INDENT);
            s.push_str(INDENT);
            s.push_str(param);
            if i + 1 < params.len() {
                s.push(',');
            }
            s.push('\n');
        }
        s.push_str(INDENT);
        s.push(')');
        s
    };

    // Line 3 (optional): WHERE condition
    if let Some(ref where_clause) = stmt.where_clause {
        out.push('\n');
        out.push_str(INDENT);
        out.push_str("WHERE ");
        out.push_str(&fmt_node(where_clause)?);
    }

    Ok(out)
}

// ── ALTER TABLE ─────────────────────────────────────────────────────────────

enum AlterItem {
    AddColumn(ColumnParts),
    Other(String),
}

pub(crate) fn fmt_alter_table(stmt: &AlterTableStmt) -> Result<String, FormatError> {
    let relation = stmt.relation.as_ref().map_or_default(fmt_range_var);

    let object_type = crate::preview::object_type_label(stmt.objtype);
    let header = format!("ALTER {object_type} {relation}");

    // First pass: classify commands — extract ColumnParts for ADD COLUMN,
    // format everything else as strings.
    let mut items: Vec<AlterItem> = Vec::new();
    for n in &stmt.cmds {
        match n.node.as_ref() {
            Some(Node::AlterTableCmd(cmd)) => {
                if matches!(
                    AlterTableType::try_from(cmd.subtype),
                    Ok(AlterTableType::AtAddColumn)
                ) && let Some(ref def) = cmd.def
                    && let Some(Node::ColumnDef(cd)) = def.node.as_ref()
                {
                    items.push(AlterItem::AddColumn(fmt_column_def_parts(cd)?));
                    continue;
                }
                items.push(AlterItem::Other(fmt_alter_table_cmd(cmd)?));
            }
            _ => items.push(AlterItem::Other(fmt_node(n)?)),
        }
    }

    // Compute alignment widths across all ADD COLUMN items.
    let add_columns: Vec<&ColumnParts> = items
        .iter()
        .filter_map(|i| match i {
            AlterItem::AddColumn(cp) => Some(cp),
            AlterItem::Other(_) => None,
        })
        .collect();

    let align = add_columns.len() > 1;
    let widths = column_widths(add_columns.iter().copied());

    let total = items.len();
    let mut out = header;
    out.push('\n');

    for (i, item) in items.iter().enumerate() {
        out.push_str(INDENT);
        match item {
            AlterItem::AddColumn(col) if align => {
                out.push_str("ADD COLUMN ");
                out.push_str(&fmt_column_line(col, widths));
            }
            AlterItem::AddColumn(col) => {
                let mut s = format!("ADD COLUMN {} {}", col.name, col.type_str);
                if let Some(ref def) = col.default_expr {
                    s.push(' ');
                    s.push_str(def);
                }
                if !col.constraints.is_empty() {
                    s.push(' ');
                    s.push_str(&col.constraints);
                }
                out.push_str(s.trim_end());
            }
            AlterItem::Other(text) => out.push_str(text),
        }
        if i + 1 < total {
            out.push(',');
        }
        out.push('\n');
    }

    Ok(out.trim_end().to_string())
}

fn fmt_alter_table_cmd(cmd: &pg_query::protobuf::AlterTableCmd) -> Result<String, FormatError> {
    let drop_cascade = DropBehavior::try_from(cmd.behavior).unwrap_or(DropBehavior::Undefined)
        == DropBehavior::DropCascade;

    match AlterTableType::try_from(cmd.subtype).unwrap_or(AlterTableType::Undefined) {
        AlterTableType::AtAddColumn => {
            if let Some(ref def) = cmd.def
                && let Some(Node::ColumnDef(cd)) = def.node.as_ref()
            {
                let inline = fmt_column_def_inline(cd)?;
                return Ok(format!("ADD COLUMN {inline}"));
            }
            Ok(format!("ADD COLUMN {}", cmd.name))
        }
        AlterTableType::AtColumnDefault => {
            if let Some(ref def) = cmd.def {
                let expr = fmt_node(def)?;
                Ok(format!("ALTER COLUMN {} SET DEFAULT {expr}", cmd.name))
            } else {
                Ok(format!("ALTER COLUMN {} DROP DEFAULT", cmd.name))
            }
        }
        AlterTableType::AtDropNotNull => Ok(format!("ALTER COLUMN {} DROP NOT NULL", cmd.name)),
        AlterTableType::AtSetNotNull => Ok(format!("ALTER COLUMN {} SET NOT NULL", cmd.name)),
        AlterTableType::AtDropColumn => {
            let mut s = if cmd.missing_ok {
                format!("DROP COLUMN IF EXISTS {}", cmd.name)
            } else {
                format!("DROP COLUMN {}", cmd.name)
            };
            if drop_cascade {
                s.push_str(" CASCADE");
            }
            Ok(s)
        }
        AlterTableType::AtSetExpression => {
            if let Some(ref def) = cmd.def {
                let expr = fmt_node(def)?;
                Ok(format!(
                    "ALTER COLUMN {} SET EXPRESSION AS ({expr})",
                    cmd.name
                ))
            } else {
                Ok(format!("ALTER COLUMN {} SET EXPRESSION", cmd.name))
            }
        }
        AlterTableType::AtDropExpression => {
            Ok(format!("ALTER COLUMN {} DROP EXPRESSION", cmd.name))
        }
        AlterTableType::AtSetStatistics => {
            if let Some(ref def) = cmd.def {
                let val = fmt_node(def)?;
                Ok(format!("ALTER COLUMN {} SET STATISTICS {val}", cmd.name))
            } else {
                Ok(format!("ALTER COLUMN {} SET STATISTICS", cmd.name))
            }
        }
        AlterTableType::AtSetStorage => {
            if let Some(ref def) = cmd.def {
                let val = fmt_node(def)?;
                Ok(format!("ALTER COLUMN {} SET STORAGE {val}", cmd.name))
            } else {
                Ok(format!("ALTER COLUMN {} SET STORAGE", cmd.name))
            }
        }
        AlterTableType::AtAddConstraint => {
            if let Some(ref def) = cmd.def
                && let Some(Node::Constraint(c)) = def.node.as_ref()
            {
                let ctext = fmt_table_constraint(c)?;
                return Ok(format!("ADD {ctext}"));
            }
            let node = Node::AlterTableCmd(Box::new(cmd.clone()));
            node.deparse(pg_query::DeparseOptions::default())
                .map_err(|e| FormatError::Deparse(e.to_string()))
        }
        AlterTableType::AtValidateConstraint => Ok(format!("VALIDATE CONSTRAINT {}", cmd.name)),
        AlterTableType::AtDropConstraint => {
            let mut s = format!("DROP CONSTRAINT {}", cmd.name);
            if cmd.missing_ok {
                s = format!("DROP CONSTRAINT IF EXISTS {}", cmd.name);
            }
            if drop_cascade {
                s.push_str(" CASCADE");
            }
            Ok(s)
        }
        AlterTableType::AtAlterColumnType => {
            if let Some(ref def) = cmd.def
                && let Some(Node::ColumnDef(cd)) = def.node.as_ref()
            {
                let type_str = cd
                    .type_name
                    .as_ref()
                    .map(fmt_type_name)
                    .transpose()?
                    .unwrap_or_default();
                let mut s = format!("ALTER COLUMN {} TYPE {type_str}", cmd.name);
                if let Some(ref raw) = cd.raw_default {
                    let using_expr = fmt_node(raw)?;
                    let _ = write!(s, " USING {using_expr}");
                }
                return Ok(s);
            }
            let node = Node::AlterTableCmd(Box::new(cmd.clone()));
            node.deparse(pg_query::DeparseOptions::default())
                .map_err(|e| FormatError::Deparse(e.to_string()))
        }
        AlterTableType::AtChangeOwner => {
            if let Some(ref owner) = cmd.newowner {
                Ok(format!("OWNER TO {}", owner.rolename))
            } else {
                Ok("OWNER TO".into())
            }
        }
        AlterTableType::AtSetLogged => Ok("SET LOGGED".into()),
        AlterTableType::AtSetUnLogged => Ok("SET UNLOGGED".into()),
        AlterTableType::AtSetTableSpace => Ok(format!("SET TABLESPACE {}", cmd.name)),
        AlterTableType::AtEnableRowSecurity => Ok("ENABLE ROW LEVEL SECURITY".into()),
        AlterTableType::AtDisableRowSecurity => Ok("DISABLE ROW LEVEL SECURITY".into()),
        AlterTableType::AtForceRowSecurity => Ok("FORCE ROW LEVEL SECURITY".into()),
        AlterTableType::AtNoForceRowSecurity => Ok("NO FORCE ROW LEVEL SECURITY".into()),
        _ => {
            let node = Node::AlterTableCmd(Box::new(cmd.clone()));
            node.deparse(pg_query::DeparseOptions::default())
                .map_err(|e| FormatError::Deparse(e.to_string()))
        }
    }
}

fn fmt_column_def_inline(cd: &ColumnDef) -> Result<String, FormatError> {
    let mut parts = vec![quote_identifier(&cd.colname)];

    if let Some(ref tn) = cd.type_name {
        parts.push(fmt_type_name(tn)?);
    }

    for c in &cd.constraints {
        if let Some(Node::Constraint(con)) = c.node.as_ref() {
            match fmt_column_constraint(con)? {
                ColumnConstraint::Default(text) | ColumnConstraint::Other(text) => {
                    parts.push(text);
                }
            }
        }
    }

    Ok(parts.join(" "))
}

pub(crate) fn fmt_view_stmt(stmt: &ViewStmt) -> Result<String, FormatError> {
    let mut header = if stmt.replace {
        "CREATE OR REPLACE VIEW".to_string()
    } else {
        "CREATE VIEW".to_string()
    };

    if let Some(ref view) = stmt.view {
        header.push(' ');
        header.push_str(&fmt_range_var(view));
    }

    if !stmt.aliases.is_empty() {
        let aliases = node_string_list(&stmt.aliases);
        let _ = write!(header, " ({})", aliases.join(", "));
    }

    header.push_str(" AS ");

    if let Some(ref query) = stmt.query {
        header.push_str(&fmt_node(query)?);
    }

    Ok(header)
}

// ── CREATE FUNCTION ─────────────────────────────────────────────────────────

pub(crate) fn fmt_create_function(stmt: &CreateFunctionStmt) -> Result<String, FormatError> {
    let mut header = "CREATE".to_string();
    if stmt.replace {
        header.push_str(" OR REPLACE");
    }
    header.push_str(if stmt.is_procedure {
        " PROCEDURE"
    } else {
        " FUNCTION"
    });

    let func_name = name_list_to_string(&stmt.funcname);
    header.push(' ');
    header.push_str(&func_name);

    // Parameters
    header.push('(');
    let params: Vec<String> = stmt
        .parameters
        .iter()
        .filter_map(|n| match n.node.as_ref() {
            Some(Node::FunctionParameter(fp)) => Some(fmt_function_param(fp)),
            _ => None,
        })
        .collect::<Result<Vec<_>, _>>()?;
    header.push_str(&params.join(", "));
    header.push(')');

    // RETURNS
    if let Some(ref rt) = stmt.return_type {
        header.push_str(" RETURNS ");
        header.push_str(&fmt_type_name(rt)?);
    }

    // Extract body and language from options
    let mut body: Option<String> = None;
    let mut language: Option<String> = None;
    let mut other_opts: Vec<String> = Vec::new();

    for opt in &stmt.options {
        if let Some(Node::DefElem(de)) = opt.node.as_ref() {
            match de.defname.as_str() {
                "as" => {
                    if let Some(ref arg) = de.arg {
                        if let Some(Node::List(l)) = arg.node.as_ref() {
                            if let Some(first) = l.items.first()
                                && let Some(Node::String(s)) = first.node.as_ref()
                            {
                                body = Some(s.sval.clone());
                            }
                        } else if let Some(Node::String(s)) = arg.node.as_ref() {
                            body = Some(s.sval.clone());
                        }
                    }
                }
                "language" => {
                    if let Some(ref arg) = de.arg
                        && let Some(Node::String(s)) = arg.node.as_ref()
                    {
                        language = Some(s.sval.clone());
                    }
                }
                "volatility" => {
                    if let Some(ref arg) = de.arg
                        && let Some(Node::String(s)) = arg.node.as_ref()
                    {
                        other_opts.push(s.sval.to_uppercase());
                    }
                }
                "strict" => {
                    if let Some(ref arg) = de.arg {
                        if let Some(Node::Boolean(b)) = arg.node.as_ref() {
                            if b.boolval {
                                other_opts.push("STRICT".into());
                            }
                        } else if let Some(Node::Integer(i)) = arg.node.as_ref()
                            && i.ival != 0
                        {
                            other_opts.push("STRICT".into());
                        }
                    }
                }
                "security" => {
                    if let Some(ref arg) = de.arg {
                        if let Some(Node::Boolean(b)) = arg.node.as_ref() {
                            if b.boolval {
                                other_opts.push("SECURITY DEFINER".into());
                            }
                        } else if let Some(Node::Integer(i)) = arg.node.as_ref()
                            && i.ival != 0
                        {
                            other_opts.push("SECURITY DEFINER".into());
                        }
                    }
                }
                _ => {}
            }
        }
    }

    // Assemble
    if let Some(body_text) = body {
        let trimmed_body = body_text.trim_start_matches('\n');
        let delimiter = available_dollar_quote(trimmed_body);
        header.push_str(" AS ");
        header.push_str(&delimiter);
        header.push('\n');
        header.push_str(trimmed_body);
        if !trimmed_body.ends_with('\n') {
            header.push('\n');
        }
        header.push_str(&delimiter);
        header.push_str(" LANGUAGE ");
        header.push_str(&language.unwrap_or_else(|| "sql".into()));
    } else if let Some(lang) = language {
        header.push_str(" LANGUAGE ");
        header.push_str(&lang);
    }

    for opt in &other_opts {
        header.push(' ');
        header.push_str(opt);
    }

    Ok(header)
}

fn available_dollar_quote(body: &str) -> String {
    if !body.contains("$$") {
        return "$$".to_owned();
    }
    for suffix in 0_u32.. {
        let tag = if suffix == 0 {
            "$fn$".to_owned()
        } else {
            format!("$fn{suffix}$")
        };
        if !body.contains(&tag) {
            return tag;
        }
    }
    unreachable!("u32 tag space exhausted")
}

fn fmt_function_param(fp: &pg_query::protobuf::FunctionParameter) -> Result<String, FormatError> {
    let mut parts: Vec<String> = Vec::new();

    // Mode
    match FunctionParameterMode::try_from(fp.mode).unwrap_or(FunctionParameterMode::Undefined) {
        FunctionParameterMode::FuncParamOut => parts.push("OUT".into()),
        FunctionParameterMode::FuncParamInout => parts.push("INOUT".into()),
        FunctionParameterMode::FuncParamVariadic => parts.push("VARIADIC".into()),
        _ => {} // IN or DEFAULT — omit
    }

    if !fp.name.is_empty() {
        parts.push(fp.name.clone());
    }

    if let Some(ref at) = fp.arg_type {
        parts.push(fmt_type_name(at)?);
    }

    if let Some(ref defexpr) = fp.defexpr {
        parts.push("DEFAULT".into());
        parts.push(fmt_node(defexpr)?);
    }

    Ok(parts.join(" "))
}
