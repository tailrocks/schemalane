use pg_query::protobuf::node::Node;
use pg_query::protobuf::{
    AConst, AExpr, AExprKind, AIndirection, BoolExpr, BoolExprType, BoolTestType, BooleanTest,
    CaseExpr, CoalesceExpr, CollateClause, ColumnRef, FuncCall, List, MinMaxExpr, MinMaxOp,
    NamedArgExpr, NullTest, NullTestType, ParamRef, RowExpr, SortByDir, SortByNulls,
    SqlValueFunction, SqlValueFunctionOp, SubLinkType, TypeCast, TypeName, a_const,
};

use crate::FormatError;

// ── Main entry points ───────────────────────────────────────────────────────

pub(crate) fn fmt_node(node: &pg_query::protobuf::Node) -> Result<String, FormatError> {
    match node.node.as_ref() {
        Some(inner) => fmt_node_inner(inner),
        None => Err(FormatError::Deparse("empty node".into())),
    }
}

pub(crate) fn fmt_node_inner(node: &Node) -> Result<String, FormatError> {
    match node {
        Node::AConst(c) => Ok(fmt_a_const(c)),
        Node::ColumnRef(cr) => fmt_column_ref(cr),
        Node::FuncCall(fc) => fmt_func_call(fc),
        Node::AExpr(expr) => fmt_a_expr(expr),
        Node::BoolExpr(expr) => fmt_bool_expr(expr),
        Node::NullTest(nt) => fmt_null_test(nt),
        Node::BooleanTest(bt) => fmt_boolean_test(bt),
        Node::TypeCast(tc) => fmt_type_cast(tc),
        Node::ParamRef(pr) => Ok(fmt_param_ref(*pr)),
        Node::AIndirection(ai) => fmt_a_indirection(ai),
        Node::SubLink(sl) => fmt_sub_link(sl),
        Node::List(l) => fmt_list(l),
        Node::CaseExpr(ce) => fmt_case_expr(ce),
        Node::CoalesceExpr(ce) => fmt_coalesce_expr(ce),
        Node::MinMaxExpr(mm) => fmt_min_max_expr(mm),
        Node::RowExpr(re) => fmt_row_expr(re),
        Node::SetToDefault(_) => Ok("DEFAULT".into()),
        Node::AArrayExpr(aa) => fmt_a_array_expr(aa),
        Node::CollateClause(cc) => fmt_collate_clause(cc),
        Node::SqlvalueFunction(svf) => Ok(fmt_sql_value_function(svf)),
        Node::NamedArgExpr(na) => fmt_named_arg_expr(na),
        Node::String(s) => Ok(s.sval.clone()),
        Node::Integer(i) => Ok(i.ival.to_string()),
        Node::Float(f) => Ok(f.fval.clone()),
        Node::Boolean(b) => Ok(if b.boolval { "true" } else { "false" }.into()),
        Node::SelectStmt(s) => crate::stmt::fmt_select_stmt(s),
        _ => node
            .deparse()
            .map_err(|e| FormatError::Deparse(e.to_string())),
    }
}

// ── AConst ──────────────────────────────────────────────────────────────────

fn fmt_a_const(c: &AConst) -> String {
    if c.isnull {
        return "NULL".into();
    }
    match &c.val {
        Some(a_const::Val::Ival(i)) => i.ival.to_string(),
        Some(a_const::Val::Fval(f)) => f.fval.clone(),
        Some(a_const::Val::Boolval(b)) => if b.boolval { "true" } else { "false" }.into(),
        Some(a_const::Val::Sval(s)) => {
            format!("'{}'", s.sval.replace('\'', "''"))
        }
        Some(a_const::Val::Bsval(bs)) => bs.bsval.clone(),
        None => "NULL".into(),
    }
}

// ── ColumnRef ───────────────────────────────────────────────────────────────

fn fmt_column_ref(cr: &ColumnRef) -> Result<String, FormatError> {
    let parts: Vec<String> = cr
        .fields
        .iter()
        .map(|f| match f.node.as_ref() {
            Some(Node::AStar(_)) => Ok("*".into()),
            Some(Node::String(s)) => Ok(quote_identifier(&s.sval)),
            _ => fmt_node(f),
        })
        .collect::<Result<_, _>>()?;
    Ok(parts.join("."))
}

// ── FuncCall ────────────────────────────────────────────────────────────────

pub(crate) fn fmt_func_call(fc: &FuncCall) -> Result<String, FormatError> {
    let name_parts: Vec<String> = fc
        .funcname
        .iter()
        .filter_map(|n| match n.node.as_ref() {
            Some(Node::String(s)) => Some(s.sval.clone()),
            _ => None,
        })
        .collect();
    let funcname = name_parts.join(".");

    if fc.agg_star {
        return Ok(format!("{funcname}(*)"));
    }

    let mut args_str = fc
        .args
        .iter()
        .map(fmt_node)
        .collect::<Result<Vec<_>, _>>()?
        .join(", ");

    if fc.agg_distinct {
        args_str = format!("DISTINCT {args_str}");
    }

    if !fc.agg_order.is_empty() {
        let order_parts: Vec<String> = fc
            .agg_order
            .iter()
            .map(|n| match n.node.as_ref() {
                Some(Node::SortBy(sb)) => fmt_sort_by(sb),
                _ => fmt_node(n),
            })
            .collect::<Result<_, _>>()?;
        args_str = format!("{args_str} ORDER BY {}", order_parts.join(", "));
    }

    let mut result = format!("{funcname}({args_str})");

    if let Some(filter) = &fc.agg_filter {
        let filter_expr = fmt_node(filter)?;
        result = format!("{result} FILTER (WHERE {filter_expr})");
    }

    if let Some(ref over) = fc.over {
        result = format!("{result} OVER ({})", fmt_window_def(over)?);
    }

    Ok(result)
}

// ── AExpr ───────────────────────────────────────────────────────────────────

fn fmt_a_expr(expr: &AExpr) -> Result<String, FormatError> {
    let op_name = expr
        .name
        .iter()
        .find_map(|n| match n.node.as_ref() {
            Some(Node::String(s)) => Some(s.sval.clone()),
            _ => None,
        })
        .unwrap_or_default();

    let left = expr
        .lexpr
        .as_ref()
        .map(|node| fmt_node_parenthesized_if_compound(node))
        .transpose()?;
    let right = expr
        .rexpr
        .as_ref()
        .map(|node| fmt_node_parenthesized_if_compound(node))
        .transpose()?;

    let kind = AExprKind::try_from(expr.kind).unwrap_or(AExprKind::Undefined);
    match kind {
        AExprKind::AexprOp => match (&left, &right) {
            (Some(l), Some(r)) => Ok(format!("{l} {op_name} {r}")),
            (None, Some(r)) => Ok(format!("{op_name} {r}")),
            (Some(l), None) => Ok(format!("{l} {op_name}")),
            (None, None) => Ok(op_name),
        },
        AExprKind::AexprOpAny => Ok(format!(
            "{} {op_name} ANY({})",
            left.unwrap_or_default(),
            right.unwrap_or_default()
        )),
        AExprKind::AexprOpAll => Ok(format!(
            "{} {op_name} ALL({})",
            left.unwrap_or_default(),
            right.unwrap_or_default()
        )),
        AExprKind::AexprDistinct => Ok(format!(
            "{} IS DISTINCT FROM {}",
            left.unwrap_or_default(),
            right.unwrap_or_default()
        )),
        AExprKind::AexprNotDistinct => Ok(format!(
            "{} IS NOT DISTINCT FROM {}",
            left.unwrap_or_default(),
            right.unwrap_or_default()
        )),
        AExprKind::AexprNullif => Ok(format!(
            "NULLIF({}, {})",
            left.unwrap_or_default(),
            right.unwrap_or_default()
        )),
        AExprKind::AexprIn => {
            let rhs = right.unwrap_or_default();
            Ok(format!("{} IN ({})", left.unwrap_or_default(), rhs))
        }
        AExprKind::AexprLike => Ok(format!(
            "{} LIKE {}",
            left.unwrap_or_default(),
            right.unwrap_or_default()
        )),
        AExprKind::AexprIlike => Ok(format!(
            "{} ILIKE {}",
            left.unwrap_or_default(),
            right.unwrap_or_default()
        )),
        AExprKind::AexprSimilar => Ok(format!(
            "{} SIMILAR TO {}",
            left.unwrap_or_default(),
            right.unwrap_or_default()
        )),
        AExprKind::AexprBetween
        | AExprKind::AexprNotBetween
        | AExprKind::AexprBetweenSym
        | AExprKind::AexprNotBetweenSym => {
            let keyword = match kind {
                AExprKind::AexprBetween => "BETWEEN",
                AExprKind::AexprNotBetween => "NOT BETWEEN",
                AExprKind::AexprBetweenSym => "BETWEEN SYMMETRIC",
                _ => "NOT BETWEEN SYMMETRIC",
            };
            let bounds = extract_between_bounds(expr)?;
            Ok(format!(
                "{} {keyword} {} AND {}",
                left.unwrap_or_default(),
                bounds.0,
                bounds.1
            ))
        }
        AExprKind::Undefined => node_deparse_fallback(&Node::AExpr(Box::new(expr.clone()))),
    }
}

fn extract_between_bounds(expr: &AExpr) -> Result<(String, String), FormatError> {
    if let Some(rexpr) = &expr.rexpr
        && let Some(Node::List(l)) = rexpr.node.as_ref()
        && l.items.len() == 2
    {
        let lo = fmt_node(&l.items[0])?;
        let hi = fmt_node(&l.items[1])?;
        return Ok((lo, hi));
    }
    Err(FormatError::Deparse(
        "BETWEEN expression missing bounds list".into(),
    ))
}

// ── BoolExpr ────────────────────────────────────────────────────────────────

fn fmt_bool_expr(expr: &BoolExpr) -> Result<String, FormatError> {
    let kind = BoolExprType::try_from(expr.boolop).unwrap_or(BoolExprType::Undefined);
    let parts: Vec<String> = expr
        .args
        .iter()
        .map(|node| {
            let formatted = fmt_node(node)?;
            let different_bool_kind = matches!(
                node.node.as_ref(),
                Some(Node::BoolExpr(child)) if child.boolop != expr.boolop
            );
            Ok(if different_bool_kind {
                format!("({formatted})")
            } else {
                formatted
            })
        })
        .collect::<Result<_, FormatError>>()?;

    match kind {
        BoolExprType::AndExpr => Ok(parts.join(" AND ")),
        BoolExprType::OrExpr => Ok(format!("({})", parts.join(" OR "))),
        BoolExprType::NotExpr => Ok(format!(
            "NOT {}",
            parts.first().cloned().unwrap_or_default()
        )),
        BoolExprType::Undefined => node_deparse_fallback(&Node::BoolExpr(Box::new(expr.clone()))),
    }
}

fn fmt_node_parenthesized_if_compound(
    node: &pg_query::protobuf::Node,
) -> Result<String, FormatError> {
    let formatted = fmt_node(node)?;
    Ok(
        if matches!(node.node.as_ref(), Some(Node::AExpr(_) | Node::BoolExpr(_))) {
            format!("({formatted})")
        } else {
            formatted
        },
    )
}

// ── NullTest ────────────────────────────────────────────────────────────────

fn fmt_null_test(nt: &NullTest) -> Result<String, FormatError> {
    let arg = nt
        .arg
        .as_ref()
        .map(|n| fmt_node(n))
        .transpose()?
        .unwrap_or_default();

    match NullTestType::try_from(nt.nulltesttype).unwrap_or(NullTestType::Undefined) {
        NullTestType::IsNull => Ok(format!("{arg} IS NULL")),
        NullTestType::IsNotNull => Ok(format!("{arg} IS NOT NULL")),
        NullTestType::Undefined => node_deparse_fallback(&Node::NullTest(Box::new(nt.clone()))),
    }
}

// ── TypeCast ────────────────────────────────────────────────────────────────

fn fmt_type_cast(tc: &TypeCast) -> Result<String, FormatError> {
    let arg = tc
        .arg
        .as_ref()
        .map(|n| fmt_node(n))
        .transpose()?
        .unwrap_or_default();

    let type_str = tc
        .type_name
        .as_ref()
        .map(fmt_type_name)
        .transpose()?
        .unwrap_or_default();

    Ok(format!("{arg}::{type_str}"))
}

// ── ParamRef ────────────────────────────────────────────────────────────────

fn fmt_param_ref(pr: ParamRef) -> String {
    format!("${}", pr.number)
}

// ── AIndirection ────────────────────────────────────────────────────────────

fn fmt_a_indirection(ai: &AIndirection) -> Result<String, FormatError> {
    let arg = ai
        .arg
        .as_ref()
        .map(|n| fmt_node(n))
        .transpose()?
        .unwrap_or_default();

    let mut result = arg;
    for elem in &ai.indirection {
        match elem.node.as_ref() {
            Some(Node::String(s)) => {
                result.push('.');
                result.push_str(&quote_identifier(&s.sval));
            }
            Some(Node::AIndices(idx)) => {
                let upper = idx
                    .uidx
                    .as_ref()
                    .map(|n| fmt_node(n))
                    .transpose()?
                    .unwrap_or_default();
                result.push('[');
                if idx.is_slice {
                    let lower = idx
                        .lidx
                        .as_ref()
                        .map(|node| fmt_node(node))
                        .transpose()?
                        .unwrap_or_default();
                    result.push_str(&lower);
                    result.push(':');
                }
                result.push_str(&upper);
                result.push(']');
            }
            Some(Node::AStar(_)) => {
                result.push_str(".*");
            }
            _ => {
                let part = fmt_node(elem)?;
                result = format!("{result}.{part}");
            }
        }
    }

    Ok(result)
}

// ── SubLink ─────────────────────────────────────────────────────────────────

fn fmt_sub_link(sl: &pg_query::protobuf::SubLink) -> Result<String, FormatError> {
    let subselect = sl
        .subselect
        .as_ref()
        .map(|n| fmt_node(n))
        .transpose()?
        .unwrap_or_default();

    match SubLinkType::try_from(sl.sub_link_type).unwrap_or(SubLinkType::ExprSublink) {
        SubLinkType::ExistsSublink => Ok(format!("EXISTS ({subselect})")),
        SubLinkType::AllSublink => {
            let testexpr = sl
                .testexpr
                .as_ref()
                .map(|n| fmt_node(n))
                .transpose()?
                .unwrap_or_default();
            let op = sl
                .oper_name
                .iter()
                .find_map(|n| match n.node.as_ref() {
                    Some(Node::String(s)) => Some(s.sval.clone()),
                    _ => None,
                })
                .unwrap_or_else(|| "=".into());
            Ok(format!("{testexpr} {op} ALL ({subselect})"))
        }
        SubLinkType::AnySublink => {
            let testexpr = sl
                .testexpr
                .as_ref()
                .map(|n| fmt_node(n))
                .transpose()?
                .unwrap_or_default();
            let op = sl
                .oper_name
                .iter()
                .find_map(|n| match n.node.as_ref() {
                    Some(Node::String(s)) => Some(s.sval.clone()),
                    _ => None,
                })
                .unwrap_or_else(|| "=".into());
            Ok(format!("{testexpr} {op} ANY ({subselect})"))
        }
        // EXPR_SUBLINK and others — scalar subquery
        _ => Ok(format!("({subselect})")),
    }
}

// ── List ────────────────────────────────────────────────────────────────────

fn fmt_list(l: &List) -> Result<String, FormatError> {
    let items: Vec<String> = l.items.iter().map(fmt_node).collect::<Result<_, _>>()?;
    Ok(items.join(", "))
}

// ── TypeName ────────────────────────────────────────────────────────────────

pub(crate) fn fmt_type_name(tn: &TypeName) -> Result<String, FormatError> {
    let name_parts: Vec<String> = tn
        .names
        .iter()
        .filter_map(|n| match n.node.as_ref() {
            Some(Node::String(s)) => Some(s.sval.clone()),
            _ => None,
        })
        .collect();

    let is_pg_catalog = name_parts.first().map(std::string::String::as_str) == Some("pg_catalog");
    let raw_name = name_parts.last().cloned().unwrap_or_default();

    let base_name = if is_pg_catalog {
        map_pg_catalog_type(&raw_name)
            .unwrap_or(&raw_name)
            .to_string()
    } else {
        name_parts.join(".")
    };

    let mut result = if tn.setof {
        format!("SETOF {base_name}")
    } else {
        base_name
    };

    if !tn.typmods.is_empty() {
        let mods: Vec<String> = tn.typmods.iter().map(fmt_node).collect::<Result<_, _>>()?;
        result = format!("{result}({})", mods.join(", "));
    }

    for bound in &tn.array_bounds {
        match bound.node.as_ref() {
            Some(Node::Integer(i)) if i.ival == -1 => {
                result.push_str("[]");
            }
            Some(Node::Integer(i)) => {
                result = format!("{result}[{}]", i.ival);
            }
            _ => {
                result.push_str("[]");
            }
        }
    }

    Ok(result)
}

fn map_pg_catalog_type(name: &str) -> Option<&'static str> {
    match name {
        "int4" => Some("int"),
        "int8" => Some("bigint"),
        "int2" => Some("smallint"),
        "float4" => Some("real"),
        "float8" => Some("double precision"),
        "bool" => Some("boolean"),
        "varchar" => Some("varchar"),
        "bpchar" => Some("char"),
        "numeric" => Some("numeric"),
        "text" => Some("text"),
        "timestamp" => Some("timestamp"),
        "timestamptz" => Some("timestamptz"),
        "date" => Some("date"),
        "time" => Some("time"),
        "timetz" => Some("timetz"),
        "uuid" => Some("uuid"),
        "json" => Some("json"),
        "jsonb" => Some("jsonb"),
        "interval" => Some("interval"),
        _ => None,
    }
}

// ── Identifier quoting ──────────────────────────────────────────────────────

pub(crate) fn quote_identifier(ident: &str) -> String {
    let plain = !ident.is_empty()
        && ident
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_lowercase() || character == '_')
        && ident.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
        });
    if plain && !crate::highlight::SQL_KEYWORDS.contains(&ident.to_ascii_uppercase().as_str()) {
        ident.to_owned()
    } else {
        format!("\"{}\"", ident.replace('"', "\"\""))
    }
}

// ── RangeVar ────────────────────────────────────────────────────────────────

pub(crate) fn fmt_range_var(rv: &pg_query::protobuf::RangeVar) -> String {
    let mut result = if rv.schemaname.is_empty() {
        quote_identifier(&rv.relname)
    } else {
        format!(
            "{}.{}",
            quote_identifier(&rv.schemaname),
            quote_identifier(&rv.relname)
        )
    };

    if let Some(alias) = &rv.alias
        && !alias.aliasname.is_empty()
    {
        result = format!("{result} AS {}", quote_identifier(&alias.aliasname));
    }

    result
}

// ── SortBy ──────────────────────────────────────────────────────────────────

pub(crate) fn fmt_sort_by(sb: &pg_query::protobuf::SortBy) -> Result<String, FormatError> {
    let node_str = sb
        .node
        .as_ref()
        .map(|n| fmt_node(n))
        .transpose()?
        .unwrap_or_default();

    let mut result = node_str;

    match SortByDir::try_from(sb.sortby_dir).unwrap_or(SortByDir::Undefined) {
        SortByDir::SortbyAsc => result.push_str(" ASC"),
        SortByDir::SortbyDesc => result.push_str(" DESC"),
        _ => {}
    }

    match SortByNulls::try_from(sb.sortby_nulls).unwrap_or(SortByNulls::Undefined) {
        SortByNulls::SortbyNullsFirst => result.push_str(" NULLS FIRST"),
        SortByNulls::SortbyNullsLast => result.push_str(" NULLS LAST"),
        _ => {}
    }

    Ok(result)
}

// ── ResTarget ───────────────────────────────────────────────────────────────

pub(crate) fn fmt_res_target_select(
    rt: &pg_query::protobuf::ResTarget,
) -> Result<String, FormatError> {
    let val_str = rt
        .val
        .as_ref()
        .map(|n| fmt_node(n))
        .transpose()?
        .unwrap_or_default();

    if rt.name.is_empty() {
        Ok(val_str)
    } else {
        Ok(format!("{val_str} AS {}", rt.name))
    }
}

pub(crate) fn fmt_res_target_update(
    rt: &pg_query::protobuf::ResTarget,
) -> Result<String, FormatError> {
    let val_str = rt
        .val
        .as_ref()
        .map(|n| fmt_node(n))
        .transpose()?
        .unwrap_or_default();

    Ok(format!("{} = {val_str}", rt.name))
}

// ── IndexElem ───────────────────────────────────────────────────────────────

pub(crate) fn fmt_index_elem(ie: &pg_query::protobuf::IndexElem) -> Result<String, FormatError> {
    let mut result = if !ie.name.is_empty() {
        quote_identifier(&ie.name)
    } else if let Some(expr) = &ie.expr {
        fmt_node(expr)?
    } else {
        String::new()
    };

    if !ie.collation.is_empty() {
        result.push_str(" COLLATE ");
        result.push_str(&fmt_identifier_name_list(&ie.collation));
    }
    if !ie.opclass.is_empty() {
        result.push(' ');
        result.push_str(&fmt_identifier_name_list(&ie.opclass));
    }

    match SortByDir::try_from(ie.ordering).unwrap_or(SortByDir::Undefined) {
        SortByDir::SortbyAsc => result.push_str(" ASC"),
        SortByDir::SortbyDesc => result.push_str(" DESC"),
        _ => {}
    }

    match SortByNulls::try_from(ie.nulls_ordering).unwrap_or(SortByNulls::Undefined) {
        SortByNulls::SortbyNullsFirst => result.push_str(" NULLS FIRST"),
        SortByNulls::SortbyNullsLast => result.push_str(" NULLS LAST"),
        _ => {}
    }

    Ok(result)
}

fn fmt_identifier_name_list(nodes: &[pg_query::protobuf::Node]) -> String {
    nodes
        .iter()
        .filter_map(|node| match node.node.as_ref() {
            Some(Node::String(value)) => Some(quote_identifier(&value.sval)),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(".")
}

// ── BooleanTest ─────────────────────────────────────────────────────────────

fn fmt_boolean_test(bt: &BooleanTest) -> Result<String, FormatError> {
    let arg = bt
        .arg
        .as_ref()
        .map(|n| fmt_node(n))
        .transpose()?
        .unwrap_or_default();

    let keyword = match BoolTestType::try_from(bt.booltesttype).unwrap_or(BoolTestType::Undefined) {
        BoolTestType::IsTrue => "IS TRUE",
        BoolTestType::IsNotTrue => "IS NOT TRUE",
        BoolTestType::IsFalse => "IS FALSE",
        BoolTestType::IsNotFalse => "IS NOT FALSE",
        BoolTestType::IsUnknown => "IS UNKNOWN",
        BoolTestType::IsNotUnknown => "IS NOT UNKNOWN",
        BoolTestType::Undefined => return Ok(arg),
    };

    Ok(format!("{arg} {keyword}"))
}

// ── CaseExpr ────────────────────────────────────────────────────────────────

fn fmt_case_expr(ce: &CaseExpr) -> Result<String, FormatError> {
    let mut result = "CASE".to_string();

    if let Some(ref arg) = ce.arg {
        let a = fmt_node(arg)?;
        result.push(' ');
        result.push_str(&a);
    }

    for when_node in &ce.args {
        if let Some(Node::CaseWhen(cw)) = when_node.node.as_ref() {
            let cond = cw
                .expr
                .as_ref()
                .map(|n| fmt_node(n))
                .transpose()?
                .unwrap_or_default();
            let then = cw
                .result
                .as_ref()
                .map(|n| fmt_node(n))
                .transpose()?
                .unwrap_or_default();
            result.push_str(" WHEN ");
            result.push_str(&cond);
            result.push_str(" THEN ");
            result.push_str(&then);
        }
    }

    if let Some(ref def) = ce.defresult {
        let d = fmt_node(def)?;
        result.push_str(" ELSE ");
        result.push_str(&d);
    }

    result.push_str(" END");
    Ok(result)
}

// ── CoalesceExpr ────────────────────────────────────────────────────────────

fn fmt_coalesce_expr(ce: &CoalesceExpr) -> Result<String, FormatError> {
    let args: Vec<String> = ce.args.iter().map(fmt_node).collect::<Result<_, _>>()?;
    Ok(format!("COALESCE({})", args.join(", ")))
}

// ── MinMaxExpr ──────────────────────────────────────────────────────────────

fn fmt_min_max_expr(mm: &MinMaxExpr) -> Result<String, FormatError> {
    let args: Vec<String> = mm.args.iter().map(fmt_node).collect::<Result<_, _>>()?;
    let func = match MinMaxOp::try_from(mm.op).unwrap_or(MinMaxOp::Undefined) {
        MinMaxOp::IsGreatest => "GREATEST",
        _ => "LEAST",
    };
    Ok(format!("{func}({})", args.join(", ")))
}

// ── RowExpr ─────────────────────────────────────────────────────────────────

fn fmt_row_expr(re: &RowExpr) -> Result<String, FormatError> {
    let args: Vec<String> = re.args.iter().map(fmt_node).collect::<Result<_, _>>()?;
    Ok(format!("ROW({})", args.join(", ")))
}

// ── AArrayExpr ──────────────────────────────────────────────────────────────

fn fmt_a_array_expr(aa: &pg_query::protobuf::AArrayExpr) -> Result<String, FormatError> {
    let elems: Vec<String> = aa.elements.iter().map(fmt_node).collect::<Result<_, _>>()?;
    Ok(format!("ARRAY[{}]", elems.join(", ")))
}

// ── CollateClause ───────────────────────────────────────────────────────────

fn fmt_collate_clause(cc: &CollateClause) -> Result<String, FormatError> {
    let arg = cc
        .arg
        .as_ref()
        .map(|n| fmt_node(n))
        .transpose()?
        .unwrap_or_default();

    let collation: Vec<String> = cc
        .collname
        .iter()
        .filter_map(|n| match n.node.as_ref() {
            Some(Node::String(s)) => Some(s.sval.clone()),
            _ => None,
        })
        .collect();

    Ok(format!("{arg} COLLATE {}", collation.join(".")))
}

// ── SqlValueFunction ────────────────────────────────────────────────────────

fn fmt_sql_value_function(svf: &SqlValueFunction) -> String {
    match SqlValueFunctionOp::try_from(svf.op)
        .unwrap_or(SqlValueFunctionOp::SqlvalueFunctionOpUndefined)
    {
        SqlValueFunctionOp::SvfopCurrentDate => "CURRENT_DATE",
        SqlValueFunctionOp::SvfopCurrentTime | SqlValueFunctionOp::SvfopCurrentTimeN => {
            "CURRENT_TIME"
        }
        SqlValueFunctionOp::SvfopLocaltime | SqlValueFunctionOp::SvfopLocaltimeN => "LOCALTIME",
        SqlValueFunctionOp::SvfopLocaltimestamp | SqlValueFunctionOp::SvfopLocaltimestampN => {
            "LOCALTIMESTAMP"
        }
        SqlValueFunctionOp::SvfopCurrentRole => "CURRENT_ROLE",
        SqlValueFunctionOp::SvfopCurrentUser => "CURRENT_USER",
        SqlValueFunctionOp::SvfopUser => "USER",
        SqlValueFunctionOp::SvfopSessionUser => "SESSION_USER",
        SqlValueFunctionOp::SvfopCurrentCatalog => "CURRENT_CATALOG",
        SqlValueFunctionOp::SvfopCurrentSchema => "CURRENT_SCHEMA",
        _ => "CURRENT_TIMESTAMP",
    }
    .into()
}

// ── NamedArgExpr ────────────────────────────────────────────────────────────

fn fmt_named_arg_expr(na: &NamedArgExpr) -> Result<String, FormatError> {
    let arg = na
        .arg
        .as_ref()
        .map(|n| fmt_node(n))
        .transpose()?
        .unwrap_or_default();
    Ok(format!("{} => {arg}", na.name))
}

// ── WindowDef ───────────────────────────────────────────────────────────────

pub(crate) fn fmt_window_def(wd: &pg_query::protobuf::WindowDef) -> Result<String, FormatError> {
    let mut parts: Vec<String> = Vec::new();

    if !wd.refname.is_empty() {
        parts.push(wd.refname.clone());
    }

    if !wd.partition_clause.is_empty() {
        let pcols: Vec<String> = wd
            .partition_clause
            .iter()
            .map(fmt_node)
            .collect::<Result<_, _>>()?;
        parts.push(format!("PARTITION BY {}", pcols.join(", ")));
    }

    if !wd.order_clause.is_empty() {
        let ocols: Vec<String> = wd
            .order_clause
            .iter()
            .map(|n| match n.node.as_ref() {
                Some(Node::SortBy(sb)) => fmt_sort_by(sb),
                _ => fmt_node(n),
            })
            .collect::<Result<_, _>>()?;
        parts.push(format!("ORDER BY {}", ocols.join(", ")));
    }

    if let Some(frame) = fmt_frame_clause(wd)? {
        parts.push(frame);
    }

    Ok(parts.join(" "))
}

// Frame option bits from windowfuncs.h
const FRAMEOPTION_RANGE: i32 = 0x0002;
const FRAMEOPTION_ROWS: i32 = 0x0004;
const FRAMEOPTION_GROUPS: i32 = 0x0008;
const FRAMEOPTION_BETWEEN: i32 = 0x0010;
const FRAMEOPTION_START_UNBOUNDED_PRECEDING: i32 = 0x0020;
const FRAMEOPTION_END_UNBOUNDED_PRECEDING: i32 = 0x0040;
const FRAMEOPTION_START_UNBOUNDED_FOLLOWING: i32 = 0x0080;
const FRAMEOPTION_END_UNBOUNDED_FOLLOWING: i32 = 0x0100;
const FRAMEOPTION_START_CURRENT_ROW: i32 = 0x0200;
const FRAMEOPTION_END_CURRENT_ROW: i32 = 0x0400;
const FRAMEOPTION_START_OFFSET_PRECEDING: i32 = 0x0800;
const FRAMEOPTION_END_OFFSET_PRECEDING: i32 = 0x1000;
const FRAMEOPTION_START_OFFSET_FOLLOWING: i32 = 0x2000;
const FRAMEOPTION_END_OFFSET_FOLLOWING: i32 = 0x4000;
const FRAMEOPTION_EXCLUDE_CURRENT_ROW: i32 = 0x0_8000;
const FRAMEOPTION_EXCLUDE_GROUP: i32 = 0x1_0000;
const FRAMEOPTION_EXCLUDE_TIES: i32 = 0x2_0000;

fn fmt_frame_clause(wd: &pg_query::protobuf::WindowDef) -> Result<Option<String>, FormatError> {
    let opts = wd.frame_options;
    if opts == 0 {
        return Ok(None);
    }

    // Default frame is RANGE BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW — skip
    let default_frame = FRAMEOPTION_RANGE
        | FRAMEOPTION_START_UNBOUNDED_PRECEDING
        | FRAMEOPTION_END_CURRENT_ROW
        | FRAMEOPTION_BETWEEN;
    if opts == default_frame {
        return Ok(None);
    }

    let mode = if opts & FRAMEOPTION_ROWS != 0 {
        "ROWS"
    } else if opts & FRAMEOPTION_GROUPS != 0 {
        "GROUPS"
    } else {
        "RANGE"
    };

    let fmt_bound = |unbounded_prec: i32,
                     unbounded_foll: i32,
                     current: i32,
                     off_prec: i32,
                     off_foll: i32,
                     offset: &Option<Box<pg_query::protobuf::Node>>|
     -> Result<String, FormatError> {
        if opts & unbounded_prec != 0 {
            Ok("UNBOUNDED PRECEDING".into())
        } else if opts & unbounded_foll != 0 {
            Ok("UNBOUNDED FOLLOWING".into())
        } else if opts & current != 0 {
            Ok("CURRENT ROW".into())
        } else if opts & off_prec != 0 {
            let v = offset
                .as_ref()
                .map(|n| fmt_node(n))
                .transpose()?
                .unwrap_or_default();
            Ok(format!("{v} PRECEDING"))
        } else if opts & off_foll != 0 {
            let v = offset
                .as_ref()
                .map(|n| fmt_node(n))
                .transpose()?
                .unwrap_or_default();
            Ok(format!("{v} FOLLOWING"))
        } else {
            Ok("CURRENT ROW".into())
        }
    };

    let frame_str = if opts & FRAMEOPTION_BETWEEN != 0 {
        let start = fmt_bound(
            FRAMEOPTION_START_UNBOUNDED_PRECEDING,
            FRAMEOPTION_START_UNBOUNDED_FOLLOWING,
            FRAMEOPTION_START_CURRENT_ROW,
            FRAMEOPTION_START_OFFSET_PRECEDING,
            FRAMEOPTION_START_OFFSET_FOLLOWING,
            &wd.start_offset,
        )?;
        let end = fmt_bound(
            FRAMEOPTION_END_UNBOUNDED_PRECEDING,
            FRAMEOPTION_END_UNBOUNDED_FOLLOWING,
            FRAMEOPTION_END_CURRENT_ROW,
            FRAMEOPTION_END_OFFSET_PRECEDING,
            FRAMEOPTION_END_OFFSET_FOLLOWING,
            &wd.end_offset,
        )?;
        format!("{mode} BETWEEN {start} AND {end}")
    } else {
        let start = fmt_bound(
            FRAMEOPTION_START_UNBOUNDED_PRECEDING,
            FRAMEOPTION_START_UNBOUNDED_FOLLOWING,
            FRAMEOPTION_START_CURRENT_ROW,
            FRAMEOPTION_START_OFFSET_PRECEDING,
            FRAMEOPTION_START_OFFSET_FOLLOWING,
            &wd.start_offset,
        )?;
        format!("{mode} {start}")
    };

    let mut result = frame_str;
    if opts & FRAMEOPTION_EXCLUDE_CURRENT_ROW != 0 {
        result.push_str(" EXCLUDE CURRENT ROW");
    } else if opts & FRAMEOPTION_EXCLUDE_GROUP != 0 {
        result.push_str(" EXCLUDE GROUP");
    } else if opts & FRAMEOPTION_EXCLUDE_TIES != 0 {
        result.push_str(" EXCLUDE TIES");
    }

    Ok(Some(result))
}

// ── Fallback ────────────────────────────────────────────────────────────────

fn node_deparse_fallback(node: &Node) -> Result<String, FormatError> {
    node.deparse()
        .map_err(|e| FormatError::Deparse(e.to_string()))
}
