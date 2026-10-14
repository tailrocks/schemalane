use pg_query::protobuf::node::Node;
use pg_query::protobuf::{
    AlterTableStmt, AlterTableType, ColumnDef, ConstrType, Constraint, CreateEnumStmt,
    CreateFunctionStmt, CreateStmt, CteMaterialize, DeleteStmt, DropBehavior,
    FunctionParameterMode, IndexStmt, InsertStmt, JoinType, OnConflictAction, SelectStmt,
    SetOperation, UpdateStmt, ViewStmt,
};

use std::fmt::Write;

use crate::FormatError;
use crate::INDENT;
use crate::expr::{
    fmt_index_elem, fmt_node, fmt_range_var, fmt_res_target_select, fmt_res_target_update,
    fmt_sort_by, fmt_type_name, fmt_window_def,
};

// ── CREATE TYPE ... AS ENUM ─────────────────────────────────────────────────

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
    let relation = stmt
        .relation
        .as_ref()
        .map(fmt_range_var)
        .unwrap_or_default();

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
                let text = fmt_node(elt)?;
                all_items.push(TableItem::Constraint(text));
            }
        }
    }

    if all_items.len() <= 1 {
        let single = match &all_items[0] {
            TableItem::Column(idx) => {
                let col = &columns[*idx];
                let mut s = format!("{} {}", col.name, col.type_str);
                if let Some(ref def) = col.default_expr {
                    s.push(' ');
                    s.push_str(def);
                }
                if !col.constraints.is_empty() {
                    s.push(' ');
                    s.push_str(&col.constraints);
                }
                s
            }
            TableItem::Constraint(text) => text.clone(),
        };
        return Ok(format!("{header} ({single})"));
    }

    let max_name = columns.iter().map(|c| c.name.len()).max().unwrap_or(0);
    let max_type = columns.iter().map(|c| c.type_str.len()).max().unwrap_or(0);
    let max_default = columns
        .iter()
        .map(|c| c.default_expr.as_ref().map_or(0, String::len))
        .max()
        .unwrap_or(0);

    let total_items = all_items.len();
    let mut out = String::with_capacity(total_items * 80);
    out.push_str(&header);
    out.push_str(" (\n");

    for (i, table_item) in all_items.iter().enumerate() {
        out.push_str(INDENT);
        match table_item {
            TableItem::Column(col_idx) => {
                out.push_str(&fmt_column_line(
                    &columns[*col_idx],
                    max_name,
                    max_type,
                    max_default,
                ));
            }
            TableItem::Constraint(text) => {
                out.push_str(text);
            }
        }

        if i + 1 < total_items {
            out.push(',');
        }
        out.push('\n');
    }

    out.push(')');
    Ok(out)
}

enum TableItem {
    Column(usize),
    Constraint(String),
}

struct ColumnParts {
    name: String,
    type_str: String,
    default_expr: Option<String>,
    constraints: String,
}

/// Format a single column definition with padded alignment.
fn fmt_column_line(
    col: &ColumnParts,
    max_name: usize,
    max_type: usize,
    max_default: usize,
) -> String {
    let mut line = String::new();

    line.push_str(&col.name);
    line.push_str(&" ".repeat(max_name - col.name.len()));
    line.push(' ');

    line.push_str(&col.type_str);

    if max_default > 0 {
        if let Some(ref def) = col.default_expr {
            line.push_str(&" ".repeat(max_type - col.type_str.len()));
            line.push(' ');
            line.push_str(def);
            if !col.constraints.is_empty() {
                line.push_str(&" ".repeat(max_default - def.len()));
            }
        } else if !col.constraints.is_empty() {
            line.push_str(&" ".repeat(max_type - col.type_str.len() + 1 + max_default));
        }
    } else if !col.constraints.is_empty() {
        line.push_str(&" ".repeat(max_type - col.type_str.len()));
    }

    if !col.constraints.is_empty() {
        line.push(' ');
        line.push_str(&col.constraints);
    }

    line.trim_end().to_string()
}

fn fmt_column_def_parts(cd: &ColumnDef) -> Result<ColumnParts, FormatError> {
    let name = cd.colname.clone();
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
            let contype = ConstrType::try_from(con.contype).unwrap_or(ConstrType::Undefined);
            match contype {
                ConstrType::ConstrDefault => {
                    if let Some(ref raw) = con.raw_expr {
                        let expr = fmt_node(raw)?;
                        default_expr = Some(format!("DEFAULT {expr}"));
                    }
                }
                ConstrType::ConstrNotnull => constraint_parts.push("NOT NULL".into()),
                ConstrType::ConstrNull => constraint_parts.push("NULL".into()),
                ConstrType::ConstrPrimary => constraint_parts.push("PRIMARY KEY".into()),
                ConstrType::ConstrUnique => constraint_parts.push("UNIQUE".into()),
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
                    constraint_parts.push(s);
                }
                ConstrType::ConstrForeign => {
                    let mut s = String::new();
                    if !con.conname.is_empty() {
                        s.push_str("CONSTRAINT ");
                        s.push_str(&con.conname);
                        s.push(' ');
                    }
                    s.push_str("REFERENCES ");
                    if let Some(ref pktable) = con.pktable {
                        s.push_str(&fmt_range_var(pktable));
                    }
                    if !con.pk_attrs.is_empty() {
                        let attrs = node_string_list(&con.pk_attrs);
                        let _ = write!(s, " ({})", attrs.join(", "));
                    }
                    append_fk_actions(&mut s, &con.fk_upd_action, &con.fk_del_action);
                    constraint_parts.push(s);
                }
                _ => {}
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

fn fmt_table_constraint(con: &Constraint) -> Result<String, FormatError> {
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
            if con.indexname.is_empty() {
                s.push_str("UNIQUE (");
                s.push_str(&node_string_list(&con.keys).join(", "));
                s.push(')');
            } else {
                s.push_str("UNIQUE USING INDEX ");
                s.push_str(&con.indexname);
            }
        }
        ConstrType::ConstrCheck => {
            s.push_str("CHECK (");
            if let Some(ref raw) = con.raw_expr {
                s.push_str(&fmt_node(raw)?);
            }
            s.push(')');
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
        }
        ConstrType::ConstrExclusion => {
            s.push_str("EXCLUDE (");
            let items: Vec<String> = con
                .exclusions
                .iter()
                .map(fmt_node)
                .collect::<Result<_, _>>()?;
            s.push_str(&items.join(", "));
            s.push(')');
        }
        _ => {
            let node = Node::Constraint(Box::new(con.clone()));
            s = node
                .deparse()
                .map_err(|e| FormatError::Deparse(e.to_string()))?;
        }
    }

    Ok(s)
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

    let relation = base
        .relation
        .as_ref()
        .map(fmt_range_var)
        .unwrap_or_default();

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
                    let text = fmt_node(elt)?;
                    all_items.push(TableItem::Constraint(text));
                }
            }
        }

        if all_items.len() <= 1 {
            let single = match &all_items[0] {
                TableItem::Column(idx) => {
                    let col = &columns[*idx];
                    let mut s = format!("{} {}", col.name, col.type_str);
                    if let Some(ref def) = col.default_expr {
                        s.push(' ');
                        s.push_str(def);
                    }
                    if !col.constraints.is_empty() {
                        s.push(' ');
                        s.push_str(&col.constraints);
                    }
                    s
                }
                TableItem::Constraint(text) => text.clone(),
            };
            header = format!("{header} ({single})");
        } else {
            let max_name = columns.iter().map(|c| c.name.len()).max().unwrap_or(0);
            let max_type = columns.iter().map(|c| c.type_str.len()).max().unwrap_or(0);
            let max_default = columns
                .iter()
                .map(|c| c.default_expr.as_ref().map_or(0, String::len))
                .max()
                .unwrap_or(0);

            let total_items = all_items.len();
            header.push_str(" (\n");
            for (i, table_item) in all_items.iter().enumerate() {
                header.push_str(INDENT);
                match table_item {
                    TableItem::Column(col_idx) => {
                        header.push_str(&fmt_column_line(
                            &columns[*col_idx],
                            max_name,
                            max_type,
                            max_default,
                        ));
                    }
                    TableItem::Constraint(text) => header.push_str(text),
                }
                if i + 1 < total_items {
                    header.push(',');
                }
                header.push('\n');
            }
            header.push(')');
        }
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
    let relation = stmt
        .relation
        .as_ref()
        .map(fmt_range_var)
        .unwrap_or_default();

    let header = format!("ALTER TABLE {relation}");

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
    let (max_name, max_type, max_default) = if align {
        (
            add_columns.iter().map(|c| c.name.len()).max().unwrap_or(0),
            add_columns
                .iter()
                .map(|c| c.type_str.len())
                .max()
                .unwrap_or(0),
            add_columns
                .iter()
                .map(|c| c.default_expr.as_ref().map_or(0, String::len))
                .max()
                .unwrap_or(0),
        )
    } else {
        (0, 0, 0)
    };

    let total = items.len();
    let mut out = header;
    out.push('\n');

    for (i, item) in items.iter().enumerate() {
        out.push_str(INDENT);
        match item {
            AlterItem::AddColumn(col) if align => {
                out.push_str("ADD COLUMN ");
                out.push_str(&fmt_column_line(col, max_name, max_type, max_default));
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
            node.deparse()
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
            node.deparse()
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
            node.deparse()
                .map_err(|e| FormatError::Deparse(e.to_string()))
        }
    }
}

fn fmt_column_def_inline(cd: &ColumnDef) -> Result<String, FormatError> {
    let mut parts = vec![cd.colname.clone()];

    if let Some(ref tn) = cd.type_name {
        parts.push(fmt_type_name(tn)?);
    }

    for c in &cd.constraints {
        if let Some(Node::Constraint(con)) = c.node.as_ref() {
            match ConstrType::try_from(con.contype).unwrap_or(ConstrType::Undefined) {
                ConstrType::ConstrNotnull => parts.push("NOT NULL".into()),
                ConstrType::ConstrNull => parts.push("NULL".into()),
                ConstrType::ConstrDefault => {
                    if let Some(ref raw) = con.raw_expr {
                        parts.push(format!("DEFAULT {}", fmt_node(raw)?));
                    }
                }
                ConstrType::ConstrPrimary => parts.push("PRIMARY KEY".into()),
                ConstrType::ConstrUnique => parts.push("UNIQUE".into()),
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
                    parts.push(s);
                }
                _ => {}
            }
        }
    }

    Ok(parts.join(" "))
}

// ── SELECT ──────────────────────────────────────────────────────────────────

pub(crate) fn fmt_select_stmt(stmt: &SelectStmt) -> Result<String, FormatError> {
    // Handle set operations (UNION / INTERSECT / EXCEPT)
    let set_op = SetOperation::try_from(stmt.op).unwrap_or(SetOperation::Undefined);
    if matches!(
        set_op,
        SetOperation::SetopUnion | SetOperation::SetopIntersect | SetOperation::SetopExcept
    ) {
        return fmt_set_operation(stmt, set_op);
    }

    // Handle VALUES lists
    if !stmt.values_lists.is_empty() {
        return fmt_values_clause(stmt);
    }

    let targets: Vec<String> = stmt
        .target_list
        .iter()
        .map(|n| match n.node.as_ref() {
            Some(Node::ResTarget(rt)) => fmt_res_target_select(rt),
            _ => fmt_node(n),
        })
        .collect::<Result<Vec<_>, _>>()?;

    let mut clauses: Vec<String> = Vec::new();

    // WITH clause
    if let Some(ref with) = stmt.with_clause {
        clauses.push(fmt_with_clause(with)?);
    }

    // SELECT [DISTINCT]
    let select_keyword = if stmt.distinct_clause.is_empty() {
        "SELECT"
    } else {
        "SELECT DISTINCT"
    };
    clauses.push(format!("{select_keyword} {}", targets.join(", ")));

    if !stmt.from_clause.is_empty() {
        let from_items: Vec<String> = stmt
            .from_clause
            .iter()
            .map(fmt_from_item)
            .collect::<Result<_, _>>()?;
        clauses.push(format!("FROM {}", from_items.join(", ")));
    }

    if let Some(ref wc) = stmt.where_clause {
        clauses.push(format!("WHERE {}", fmt_node(wc)?));
    }

    if !stmt.group_clause.is_empty() {
        let groups: Vec<String> = stmt
            .group_clause
            .iter()
            .map(fmt_node)
            .collect::<Result<_, _>>()?;
        clauses.push(format!("GROUP BY {}", groups.join(", ")));
    }

    if let Some(ref hc) = stmt.having_clause {
        clauses.push(format!("HAVING {}", fmt_node(hc)?));
    }

    // WINDOW clause
    if !stmt.window_clause.is_empty() {
        let wins: Vec<String> = stmt
            .window_clause
            .iter()
            .filter_map(|n| match n.node.as_ref() {
                Some(Node::WindowDef(wd)) => {
                    let body = fmt_window_def(wd).ok()?;
                    Some(format!("{} AS ({body})", wd.name))
                }
                _ => None,
            })
            .collect();
        if !wins.is_empty() {
            clauses.push(format!("WINDOW {}", wins.join(", ")));
        }
    }

    if !stmt.sort_clause.is_empty() {
        let sorts: Vec<String> = stmt
            .sort_clause
            .iter()
            .map(|n| match n.node.as_ref() {
                Some(Node::SortBy(sb)) => fmt_sort_by(sb),
                _ => fmt_node(n),
            })
            .collect::<Result<Vec<_>, _>>()?;
        clauses.push(format!("ORDER BY {}", sorts.join(", ")));
    }

    if let Some(ref lc) = stmt.limit_count {
        clauses.push(format!("LIMIT {}", fmt_node(lc)?));
    }

    if let Some(ref lo) = stmt.limit_offset {
        clauses.push(format!("OFFSET {}", fmt_node(lo)?));
    }

    Ok(clauses.join("\n"))
}

fn fmt_set_operation(stmt: &SelectStmt, set_op: SetOperation) -> Result<String, FormatError> {
    let left = stmt
        .larg
        .as_ref()
        .map(|s| fmt_select_stmt(s))
        .transpose()?
        .unwrap_or_default();

    let op_keyword = match set_op {
        SetOperation::SetopIntersect => "INTERSECT",
        SetOperation::SetopExcept => "EXCEPT",
        _ => "UNION",
    };

    let modifier = if stmt.all { " ALL" } else { "" };

    let right = stmt
        .rarg
        .as_ref()
        .map(|s| fmt_select_stmt(s))
        .transpose()?
        .unwrap_or_default();

    Ok(format!("{left}\n{op_keyword}{modifier}\n{right}"))
}

fn fmt_values_clause(stmt: &SelectStmt) -> Result<String, FormatError> {
    let rows: Vec<String> = stmt
        .values_lists
        .iter()
        .map(|row| {
            if let Some(Node::List(l)) = row.node.as_ref() {
                let items: Vec<String> = l
                    .items
                    .iter()
                    .map(fmt_node)
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(format!("({})", items.join(", ")))
            } else {
                fmt_node(row)
            }
        })
        .collect::<Result<_, FormatError>>()?;

    Ok(format!("VALUES {}", rows.join(", ")))
}

fn fmt_from_item(node: &pg_query::protobuf::Node) -> Result<String, FormatError> {
    match node.node.as_ref() {
        Some(Node::RangeVar(rv)) => Ok(fmt_range_var(rv)),
        Some(Node::JoinExpr(je)) => fmt_join_expr(je),
        Some(Node::RangeSubselect(rs)) => {
            let mut s = String::new();
            if rs.lateral {
                s.push_str("LATERAL ");
            }
            if let Some(ref subquery) = rs.subquery {
                s.push('(');
                s.push_str(&fmt_node(subquery)?);
                s.push(')');
            }
            if let Some(ref alias) = rs.alias {
                s.push_str(" AS ");
                s.push_str(&alias.aliasname);
            }
            Ok(s)
        }
        Some(Node::RangeFunction(rf)) => fmt_range_function(rf),
        _ => fmt_node(node),
    }
}

fn fmt_range_function(rf: &pg_query::protobuf::RangeFunction) -> Result<String, FormatError> {
    let mut s = String::new();
    if rf.lateral {
        s.push_str("LATERAL ");
    }

    let func_items: Vec<String> = rf
        .functions
        .iter()
        .map(|n| {
            if let Some(Node::List(l)) = n.node.as_ref()
                && let Some(first) = l.items.first()
            {
                return fmt_node(first);
            }
            fmt_node(n)
        })
        .collect::<Result<_, _>>()?;
    s.push_str(&func_items.join(", "));

    if rf.ordinality {
        s.push_str(" WITH ORDINALITY");
    }

    if let Some(ref alias) = rf.alias {
        s.push_str(" AS ");
        s.push_str(&alias.aliasname);
        if !alias.colnames.is_empty() {
            let cols = node_string_list(&alias.colnames);
            let _ = write!(s, " ({})", cols.join(", "));
        }
    }

    Ok(s)
}

fn fmt_join_expr(je: &pg_query::protobuf::JoinExpr) -> Result<String, FormatError> {
    let left = je
        .larg
        .as_ref()
        .map(|n| fmt_from_item(n))
        .transpose()?
        .unwrap_or_default();

    let join_keyword = if je.is_natural { "NATURAL " } else { "" };

    let join_type = match JoinType::try_from(je.jointype).unwrap_or(JoinType::Undefined) {
        JoinType::JoinLeft => "LEFT JOIN",
        JoinType::JoinFull => "FULL JOIN",
        JoinType::JoinRight => "RIGHT JOIN",
        _ => "JOIN",
    };

    let right = je
        .rarg
        .as_ref()
        .map(|n| fmt_from_item(n))
        .transpose()?
        .unwrap_or_default();

    let mut result = format!("{left} {join_keyword}{join_type} {right}");

    if let Some(ref quals) = je.quals {
        let _ = write!(result, " ON {}", fmt_node(quals)?);
    }

    if !je.using_clause.is_empty() {
        let cols = node_string_list(&je.using_clause);
        let _ = write!(result, " USING ({})", cols.join(", "));
    }

    if let Some(ref alias) = je.alias
        && !alias.aliasname.is_empty()
    {
        let _ = write!(result, " AS {}", alias.aliasname);
    }

    Ok(result)
}

// ── WITH (CTE) ─────────────────────────────────────────────────────────────

fn fmt_with_clause(with: &pg_query::protobuf::WithClause) -> Result<String, FormatError> {
    let keyword = if with.recursive {
        "WITH RECURSIVE"
    } else {
        "WITH"
    };

    let ctes: Vec<String> = with
        .ctes
        .iter()
        .filter_map(|n| match n.node.as_ref() {
            Some(Node::CommonTableExpr(cte)) => Some(fmt_common_table_expr(cte)),
            _ => None,
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok(format!("{keyword} {}", ctes.join(", ")))
}

fn fmt_common_table_expr(cte: &pg_query::protobuf::CommonTableExpr) -> Result<String, FormatError> {
    let mut s = cte.ctename.clone();

    if !cte.aliascolnames.is_empty() {
        let cols = node_string_list(&cte.aliascolnames);
        let _ = write!(s, " ({})", cols.join(", "));
    }

    s.push_str(" AS ");

    // Materialization hint
    match CteMaterialize::try_from(cte.ctematerialized)
        .unwrap_or(CteMaterialize::CtematerializeUndefined)
    {
        CteMaterialize::Always => s.push_str("MATERIALIZED "),
        CteMaterialize::Never => s.push_str("NOT MATERIALIZED "),
        _ => {}
    }

    s.push('(');
    if let Some(ref query) = cte.ctequery {
        s.push_str(&fmt_node(query)?);
    }
    s.push(')');

    Ok(s)
}

// ── INSERT ──────────────────────────────────────────────────────────────────

pub(crate) fn fmt_insert_stmt(stmt: &InsertStmt) -> Result<String, FormatError> {
    let relation = stmt
        .relation
        .as_ref()
        .map(fmt_range_var)
        .unwrap_or_default();

    let mut out = format!("INSERT INTO {relation}");

    // Column list
    if !stmt.cols.is_empty() {
        let cols: Vec<String> = stmt
            .cols
            .iter()
            .map(|n| match n.node.as_ref() {
                Some(Node::ResTarget(rt)) => Ok(rt.name.clone()),
                _ => fmt_node(n),
            })
            .collect::<Result<Vec<_>, _>>()?;

        if cols.len() <= 1 {
            let _ = write!(out, " ({})", cols.join(", "));
        } else {
            out.push_str(" (\n");
            for (i, col) in cols.iter().enumerate() {
                out.push_str(INDENT);
                out.push_str(col);
                if i + 1 < cols.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            out.push(')');
        }
    }

    // Source (VALUES or SELECT)
    if let Some(ref select_node) = stmt.select_stmt
        && let Some(Node::SelectStmt(select)) = select_node.node.as_ref()
    {
        let source = fmt_select_stmt(select)?;
        out.push('\n');
        out.push_str(&source);
    }

    // ON CONFLICT
    if let Some(ref oc) = stmt.on_conflict_clause {
        out.push('\n');
        out.push_str(&fmt_on_conflict(oc)?);
    }

    // RETURNING
    if !stmt.returning_list.is_empty() {
        let ret: Vec<String> = stmt
            .returning_list
            .iter()
            .map(|n| match n.node.as_ref() {
                Some(Node::ResTarget(rt)) => fmt_res_target_select(rt),
                _ => fmt_node(n),
            })
            .collect::<Result<Vec<_>, _>>()?;
        out.push('\n');
        let _ = write!(out, "RETURNING {}", ret.join(", "));
    }

    Ok(out)
}

fn fmt_on_conflict(oc: &pg_query::protobuf::OnConflictClause) -> Result<String, FormatError> {
    let mut s = "ON CONFLICT".to_string();

    if let Some(ref infer) = oc.infer {
        if !infer.index_elems.is_empty() {
            let elems: Vec<String> = infer
                .index_elems
                .iter()
                .map(|n| match n.node.as_ref() {
                    Some(Node::IndexElem(ie)) => fmt_index_elem(ie),
                    _ => fmt_node(n),
                })
                .collect::<Result<Vec<_>, _>>()?;
            let _ = write!(s, " ({})", elems.join(", "));
        } else if !infer.conname.is_empty() {
            let _ = write!(s, " ON CONSTRAINT {}", infer.conname);
        }
    }

    match OnConflictAction::try_from(oc.action).unwrap_or(OnConflictAction::Undefined) {
        OnConflictAction::OnconflictNothing => s.push_str(" DO NOTHING"),
        OnConflictAction::OnconflictUpdate => {
            s.push_str(" DO UPDATE SET ");
            let sets: Vec<String> = oc
                .target_list
                .iter()
                .map(|n| match n.node.as_ref() {
                    Some(Node::ResTarget(rt)) => fmt_res_target_update(rt),
                    _ => fmt_node(n),
                })
                .collect::<Result<Vec<_>, _>>()?;
            s.push_str(&sets.join(", "));

            if let Some(ref wc) = oc.where_clause {
                let _ = write!(s, " WHERE {}", fmt_node(wc)?);
            }
        }
        _ => {}
    }

    Ok(s)
}

// ── UPDATE ──────────────────────────────────────────────────────────────────

pub(crate) fn fmt_update_stmt(stmt: &UpdateStmt) -> Result<String, FormatError> {
    let relation = stmt
        .relation
        .as_ref()
        .map(fmt_range_var)
        .unwrap_or_default();

    let mut clauses: Vec<String> = Vec::new();
    clauses.push(format!("UPDATE {relation}"));

    let sets: Vec<String> = stmt
        .target_list
        .iter()
        .map(|n| match n.node.as_ref() {
            Some(Node::ResTarget(rt)) => fmt_res_target_update(rt),
            _ => fmt_node(n),
        })
        .collect::<Result<Vec<_>, _>>()?;
    clauses.push(format!("SET {}", sets.join(", ")));

    if !stmt.from_clause.is_empty() {
        let from_items: Vec<String> = stmt
            .from_clause
            .iter()
            .map(fmt_from_item)
            .collect::<Result<_, _>>()?;
        clauses.push(format!("FROM {}", from_items.join(", ")));
    }

    if let Some(ref wc) = stmt.where_clause {
        clauses.push(format!("WHERE {}", fmt_node(wc)?));
    }

    if !stmt.returning_list.is_empty() {
        let ret: Vec<String> = stmt
            .returning_list
            .iter()
            .map(|n| match n.node.as_ref() {
                Some(Node::ResTarget(rt)) => fmt_res_target_select(rt),
                _ => fmt_node(n),
            })
            .collect::<Result<Vec<_>, _>>()?;
        clauses.push(format!("RETURNING {}", ret.join(", ")));
    }

    Ok(clauses.join("\n"))
}

// ── DELETE ──────────────────────────────────────────────────────────────────

pub(crate) fn fmt_delete_stmt(stmt: &DeleteStmt) -> Result<String, FormatError> {
    let relation = stmt
        .relation
        .as_ref()
        .map(fmt_range_var)
        .unwrap_or_default();

    let mut clauses: Vec<String> = Vec::new();
    clauses.push(format!("DELETE FROM {relation}"));

    if !stmt.using_clause.is_empty() {
        let using_items: Vec<String> = stmt
            .using_clause
            .iter()
            .map(fmt_from_item)
            .collect::<Result<_, _>>()?;
        clauses.push(format!("USING {}", using_items.join(", ")));
    }

    if let Some(ref wc) = stmt.where_clause {
        clauses.push(format!("WHERE {}", fmt_node(wc)?));
    }

    if !stmt.returning_list.is_empty() {
        let ret: Vec<String> = stmt
            .returning_list
            .iter()
            .map(|n| match n.node.as_ref() {
                Some(Node::ResTarget(rt)) => fmt_res_target_select(rt),
                _ => fmt_node(n),
            })
            .collect::<Result<Vec<_>, _>>()?;
        clauses.push(format!("RETURNING {}", ret.join(", ")));
    }

    Ok(clauses.join("\n"))
}

// ── CREATE VIEW ─────────────────────────────────────────────────────────────

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
        header.push_str(" AS $$\n");
        header.push_str(trimmed_body);
        if !trimmed_body.ends_with('\n') {
            header.push('\n');
        }
        header.push_str("$$ LANGUAGE ");
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

// ── Helpers ─────────────────────────────────────────────────────────────────

fn name_list_to_string(nodes: &[pg_query::protobuf::Node]) -> String {
    nodes
        .iter()
        .filter_map(|n| match n.node.as_ref() {
            Some(Node::String(s)) => Some(s.sval.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(".")
}

fn node_string_list(nodes: &[pg_query::protobuf::Node]) -> Vec<String> {
    nodes
        .iter()
        .filter_map(|n| match n.node.as_ref() {
            Some(Node::String(s)) => Some(s.sval.clone()),
            _ => None,
        })
        .collect()
}
