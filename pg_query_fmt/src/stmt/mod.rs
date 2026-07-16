use crate::expr::quote_identifier;
use pg_query::protobuf::node::Node;

mod ddl;
mod dml;
mod table_body;

pub(crate) use ddl::{
    fmt_alter_table, fmt_create_enum, fmt_create_foreign_table, fmt_create_function,
    fmt_create_table, fmt_index_stmt, fmt_view_stmt,
};
pub(crate) use dml::{fmt_delete_stmt, fmt_insert_stmt, fmt_select_stmt, fmt_update_stmt};

// ── Helpers ─────────────────────────────────────────────────────────────────

pub(super) fn name_list_to_string(nodes: &[pg_query::protobuf::Node]) -> String {
    nodes
        .iter()
        .filter_map(|n| match n.node.as_ref() {
            Some(Node::String(s)) => Some(quote_identifier(&s.sval)),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(".")
}

pub(super) fn node_string_list(nodes: &[pg_query::protobuf::Node]) -> Vec<String> {
    nodes
        .iter()
        .filter_map(|n| match n.node.as_ref() {
            Some(Node::String(s)) => Some(quote_identifier(&s.sval)),
            _ => None,
        })
        .collect()
}
