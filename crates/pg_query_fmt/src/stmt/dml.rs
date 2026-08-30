use pg_query::protobuf::node::Node;
use pg_query::protobuf::{
    CteMaterialize, DeleteStmt, InsertStmt, JoinType, LockClauseStrength, LockWaitPolicy,
    OnConflictAction, SelectStmt, SetOperation, UpdateStmt,
};
use std::fmt::Write;

use crate::expr::{
    fmt_index_elem, fmt_node, fmt_range_var, fmt_res_target_select, fmt_res_target_update,
    fmt_sort_by, fmt_window_def,
};
use crate::{FormatError, INDENT};

use super::node_string_list;

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
        "SELECT".to_owned()
    } else if stmt.distinct_clause.iter().all(|node| node.node.is_none()) {
        "SELECT DISTINCT".to_owned()
    } else {
        let expressions = stmt
            .distinct_clause
            .iter()
            .filter(|node| node.node.is_some())
            .map(fmt_node)
            .collect::<Result<Vec<_>, _>>()?;
        format!("SELECT DISTINCT ON ({})", expressions.join(", "))
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

    for node in &stmt.locking_clause {
        let Some(Node::LockingClause(lock)) = node.node.as_ref() else {
            continue;
        };
        let strength = match LockClauseStrength::try_from(lock.strength)
            .unwrap_or(LockClauseStrength::Undefined)
        {
            LockClauseStrength::LcsForupdate => "FOR UPDATE",
            LockClauseStrength::LcsFornokeyupdate => "FOR NO KEY UPDATE",
            LockClauseStrength::LcsForshare => "FOR SHARE",
            LockClauseStrength::LcsForkeyshare => "FOR KEY SHARE",
            _ => continue,
        };
        let mut clause = strength.to_owned();
        if !lock.locked_rels.is_empty() {
            let relations = lock
                .locked_rels
                .iter()
                .map(fmt_node)
                .collect::<Result<Vec<_>, _>>()?;
            clause.push_str(" OF ");
            clause.push_str(&relations.join(", "));
        }
        match LockWaitPolicy::try_from(lock.wait_policy).unwrap_or(LockWaitPolicy::Undefined) {
            LockWaitPolicy::LockWaitSkip => clause.push_str(" SKIP LOCKED"),
            LockWaitPolicy::LockWaitError => clause.push_str(" NOWAIT"),
            _ => {}
        }
        clauses.push(clause);
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
