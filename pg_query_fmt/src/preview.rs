//! Compact, human-readable previews of SQL statements.
//!
//! Produces short summaries like `CREATE TABLE public.wallets`, `CREATE INDEX idx_foo`,
//! `DROP EXTENSION pg_trgm`, etc. by inspecting the parsed AST node.

use pg_query::protobuf;
use pg_query::protobuf::node::Node;

/// Creates a compact preview from a parsed SQL statement result.
///
/// Extracts the statement type and primary object name from the AST.
/// Falls back to truncated raw SQL if the node type is not recognized.
pub fn statement_preview(parsed: &pg_query::ParseResult) -> String {
    let node = parsed
        .protobuf
        .stmts
        .first()
        .and_then(|raw| raw.stmt.as_ref())
        .and_then(|n| n.node.as_ref());

    if let Some(node) = node
        && let Some(preview) = node_preview(node)
    {
        return preview;
    }

    // Fallback: truncated SQL.
    if let Ok(truncated) = parsed.truncate(96) {
        truncated
    } else {
        parsed.statement_types().join(", ")
    }
}

/// Parses raw SQL and creates its compact statement preview.
///
/// Invalid SQL falls back to the first 96 Unicode scalar values of the input.
pub fn preview_sql(sql: &str) -> String {
    match pg_query::parse(sql) {
        Ok(parsed) => statement_preview(&parsed),
        Err(_) => sql.chars().take(96).collect(),
    }
}

/// Formats a qualified relation name (`schema.table` or just `table`).
fn format_rangevar(rv: &protobuf::RangeVar) -> String {
    if rv.schemaname.is_empty() {
        rv.relname.clone()
    } else {
        format!("{}.{}", rv.schemaname, rv.relname)
    }
}

/// Extracts a dotted name from a list of `String` nodes (used by `funcname`, `type_name`, etc.).
fn format_name_list(nodes: &[protobuf::Node]) -> String {
    nodes
        .iter()
        .filter_map(|n| {
            if let Some(Node::String(s)) = n.node.as_ref() {
                Some(s.sval.as_str())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join(".")
}

/// Returns the SQL keyword for an `ObjectType` enum value.
fn object_type_label(objtype: i32) -> &'static str {
    match objtype {
        x if x == protobuf::ObjectType::ObjectTable as i32 => "TABLE",
        x if x == protobuf::ObjectType::ObjectIndex as i32 => "INDEX",
        x if x == protobuf::ObjectType::ObjectSequence as i32 => "SEQUENCE",
        x if x == protobuf::ObjectType::ObjectView as i32 => "VIEW",
        x if x == protobuf::ObjectType::ObjectMatview as i32 => "MATERIALIZED VIEW",
        x if x == protobuf::ObjectType::ObjectForeignTable as i32 => "FOREIGN TABLE",
        x if x == protobuf::ObjectType::ObjectType as i32 => "TYPE",
        x if x == protobuf::ObjectType::ObjectSchema as i32 => "SCHEMA",
        x if x == protobuf::ObjectType::ObjectExtension as i32 => "EXTENSION",
        x if x == protobuf::ObjectType::ObjectFunction as i32 => "FUNCTION",
        x if x == protobuf::ObjectType::ObjectProcedure as i32 => "PROCEDURE",
        x if x == protobuf::ObjectType::ObjectTrigger as i32 => "TRIGGER",
        x if x == protobuf::ObjectType::ObjectPolicy as i32 => "POLICY",
        x if x == protobuf::ObjectType::ObjectRule as i32 => "RULE",
        x if x == protobuf::ObjectType::ObjectDomain as i32 => "DOMAIN",
        x if x == protobuf::ObjectType::ObjectCollation as i32 => "COLLATION",
        x if x == protobuf::ObjectType::ObjectConversion as i32 => "CONVERSION",
        x if x == protobuf::ObjectType::ObjectDatabase as i32 => "DATABASE",
        x if x == protobuf::ObjectType::ObjectTablespace as i32 => "TABLESPACE",
        x if x == protobuf::ObjectType::ObjectRole as i32 => "ROLE",
        x if x == protobuf::ObjectType::ObjectPublication as i32 => "PUBLICATION",
        x if x == protobuf::ObjectType::ObjectSubscription as i32 => "SUBSCRIPTION",
        x if x == protobuf::ObjectType::ObjectEventTrigger as i32 => "EVENT TRIGGER",
        x if x == protobuf::ObjectType::ObjectAccessMethod as i32 => "ACCESS METHOD",
        x if x == protobuf::ObjectType::ObjectFdw as i32 => "FOREIGN DATA WRAPPER",
        x if x == protobuf::ObjectType::ObjectForeignServer as i32 => "SERVER",
        x if x == protobuf::ObjectType::ObjectLanguage as i32 => "LANGUAGE",
        x if x == protobuf::ObjectType::ObjectAggregate as i32 => "AGGREGATE",
        x if x == protobuf::ObjectType::ObjectOperator as i32 => "OPERATOR",
        x if x == protobuf::ObjectType::ObjectOpclass as i32 => "OPERATOR CLASS",
        x if x == protobuf::ObjectType::ObjectOpfamily as i32 => "OPERATOR FAMILY",
        x if x == protobuf::ObjectType::ObjectStatisticExt as i32 => "STATISTICS",
        _ => "OBJECT",
    }
}

/// Builds a compact preview string from an AST node, returning `None` for unhandled node types.
fn node_preview(node: &Node) -> Option<String> {
    match node {
        Node::CreateStmt(stmt) => {
            let name = stmt.relation.as_ref().map(format_rangevar)?;
            Some(format!("CREATE TABLE {name}"))
        }
        Node::IndexStmt(stmt) => {
            let mut parts = Vec::with_capacity(5);
            parts.push("CREATE");
            if stmt.unique {
                parts.push("UNIQUE");
            }
            parts.push("INDEX");
            if !stmt.idxname.is_empty() {
                return Some(format!("{} {}", parts.join(" "), stmt.idxname));
            }
            let table = stmt.relation.as_ref().map(format_rangevar)?;
            Some(format!("{} ON {table}", parts.join(" ")))
        }
        Node::AlterTableStmt(stmt) => {
            let name = stmt.relation.as_ref().map(format_rangevar)?;
            let kind = object_type_label(stmt.objtype);
            Some(format!("ALTER {kind} {name}"))
        }
        Node::DropStmt(stmt) => {
            let kind = object_type_label(stmt.remove_type);
            // Extract first object name from the objects list.
            let obj_name = stmt.objects.first().and_then(|n| {
                n.node.as_ref().and_then(|inner| match inner {
                    Node::String(s) => Some(s.sval.clone()),
                    Node::List(list) => Some(format_name_list(&list.items)),
                    _ => None,
                })
            });
            if let Some(name) = obj_name {
                Some(format!("DROP {kind} {name}"))
            } else {
                Some(format!("DROP {kind}"))
            }
        }
        Node::CreateSchemaStmt(stmt) => Some(format!("CREATE SCHEMA {}", stmt.schemaname)),
        Node::ViewStmt(stmt) => {
            let name = stmt.view.as_ref().map(format_rangevar)?;
            let kw = if stmt.replace {
                "CREATE OR REPLACE VIEW"
            } else {
                "CREATE VIEW"
            };
            Some(format!("{kw} {name}"))
        }
        Node::CreateSeqStmt(stmt) => {
            let name = stmt.sequence.as_ref().map(format_rangevar)?;
            Some(format!("CREATE SEQUENCE {name}"))
        }
        Node::AlterSeqStmt(stmt) => {
            let name = stmt.sequence.as_ref().map(format_rangevar)?;
            Some(format!("ALTER SEQUENCE {name}"))
        }
        Node::CreateFunctionStmt(stmt) => {
            let name = format_name_list(&stmt.funcname);
            let kind = if stmt.is_procedure {
                "PROCEDURE"
            } else {
                "FUNCTION"
            };
            let or_replace = if stmt.replace { " OR REPLACE" } else { "" };
            Some(format!("CREATE{or_replace} {kind} {name}"))
        }
        Node::CreateEnumStmt(stmt) => {
            let name = format_name_list(&stmt.type_name);
            Some(format!("CREATE TYPE {name}"))
        }
        Node::AlterEnumStmt(stmt) => {
            let name = format_name_list(&stmt.type_name);
            if stmt.new_val.is_empty() {
                Some(format!("ALTER TYPE {name}"))
            } else {
                Some(format!("ALTER TYPE {name} ADD VALUE '{}'", stmt.new_val))
            }
        }
        Node::CreateTrigStmt(stmt) => {
            let table = stmt.relation.as_ref().map(format_rangevar)?;
            Some(format!("CREATE TRIGGER {} ON {table}", stmt.trigname))
        }
        Node::CreateExtensionStmt(stmt) => Some(format!("CREATE EXTENSION {}", stmt.extname)),
        Node::RenameStmt(stmt) => {
            let kind = object_type_label(stmt.rename_type);
            if let Some(rel) = stmt.relation.as_ref() {
                Some(format!(
                    "ALTER {kind} {} RENAME TO {}",
                    format_rangevar(rel),
                    stmt.newname
                ))
            } else {
                Some(format!("ALTER {kind} RENAME TO {}", stmt.newname))
            }
        }
        Node::CommentStmt(stmt) => {
            let kind = object_type_label(stmt.objtype);
            Some(format!("COMMENT ON {kind}"))
        }
        Node::GrantStmt(stmt) => {
            let kw = if stmt.is_grant { "GRANT" } else { "REVOKE" };
            let kind = object_type_label(stmt.objtype);
            Some(format!("{kw} ON {kind}"))
        }
        Node::VacuumStmt(stmt) => Some(vacuum_preview(stmt)),
        Node::InsertStmt(stmt) => {
            let name = stmt.relation.as_ref().map(format_rangevar)?;
            Some(format!("INSERT INTO {name}"))
        }
        Node::UpdateStmt(stmt) => {
            let name = stmt.relation.as_ref().map(format_rangevar)?;
            Some(format!("UPDATE {name}"))
        }
        Node::DeleteStmt(stmt) => {
            let name = stmt.relation.as_ref().map(format_rangevar)?;
            Some(format!("DELETE FROM {name}"))
        }
        Node::SelectStmt(_) => Some("SELECT".to_owned()),
        Node::CreatedbStmt(stmt) => Some(format!("CREATE DATABASE {}", stmt.dbname)),
        Node::DropdbStmt(stmt) => Some(format!("DROP DATABASE {}", stmt.dbname)),
        Node::CreateTableSpaceStmt(stmt) => {
            Some(format!("CREATE TABLESPACE {}", stmt.tablespacename))
        }
        Node::AlterSystemStmt(_) => Some("ALTER SYSTEM".to_owned()),
        Node::ReindexStmt(stmt) => {
            let name = if stmt.name.is_empty() {
                stmt.relation.as_ref().map_or("", |r| r.relname.as_str())
            } else {
                &stmt.name
            };
            Some(format!("REINDEX {name}"))
        }
        Node::DiscardStmt(_) => Some("DISCARD ALL".to_owned()),
        _ => None,
    }
}

fn vacuum_preview(stmt: &pg_query::protobuf::VacuumStmt) -> String {
    let kw = if stmt.is_vacuumcmd {
        "VACUUM"
    } else {
        "ANALYZE"
    };
    let table = stmt.rels.first().and_then(|n| {
        if let Some(Node::VacuumRelation(vr)) = n.node.as_ref() {
            vr.relation.as_ref().map(format_rangevar)
        } else {
            None
        }
    });
    if let Some(name) = table {
        format!("{kw} {name}")
    } else {
        kw.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_sql_parses_valid_sql() {
        assert_eq!(
            preview_sql("CREATE TABLE public.wallets (id bigint)"),
            "CREATE TABLE public.wallets"
        );
    }

    #[test]
    fn preview_sql_truncates_invalid_sql_on_character_boundaries() {
        let invalid = format!("{}!", "🦀".repeat(100));
        assert_eq!(preview_sql(&invalid), "🦀".repeat(96));
    }

    fn preview_of(sql: &str) -> String {
        let parsed = pg_query::parse(sql).expect("parse");
        statement_preview(&parsed)
    }

    #[test]
    fn preview_create_table() {
        assert_eq!(
            preview_of("CREATE TABLE public.wallets (id BIGSERIAL PRIMARY KEY, address TEXT);"),
            "CREATE TABLE public.wallets"
        );
    }

    #[test]
    fn preview_create_index() {
        assert_eq!(
            preview_of("CREATE INDEX idx_foo ON public.t USING btree (col);"),
            "CREATE INDEX idx_foo"
        );
    }

    #[test]
    fn preview_create_unique_index() {
        assert_eq!(
            preview_of(
                "CREATE UNIQUE INDEX CONCURRENTLY idx_wallets_addr ON public.wallets (address);"
            ),
            "CREATE UNIQUE INDEX idx_wallets_addr"
        );
    }

    #[test]
    fn preview_drop_index() {
        assert_eq!(
            preview_of("DROP INDEX IF EXISTS public.idx_test;"),
            "DROP INDEX public.idx_test"
        );
    }

    #[test]
    fn preview_drop_extension() {
        assert_eq!(
            preview_of("DROP EXTENSION IF EXISTS pg_trgm;"),
            "DROP EXTENSION pg_trgm"
        );
    }

    #[test]
    fn preview_analyze() {
        assert_eq!(
            preview_of("ANALYZE public.ethereum_txns;"),
            "ANALYZE public.ethereum_txns"
        );
    }

    #[test]
    fn preview_vacuum() {
        assert_eq!(preview_of("VACUUM;"), "VACUUM");
    }

    #[test]
    fn preview_alter_table() {
        assert_eq!(
            preview_of("ALTER TABLE public.wallets ADD COLUMN name TEXT;"),
            "ALTER TABLE public.wallets"
        );
    }

    #[test]
    fn preview_create_extension() {
        assert_eq!(
            preview_of("CREATE EXTENSION IF NOT EXISTS pg_trgm;"),
            "CREATE EXTENSION pg_trgm"
        );
    }

    #[test]
    fn preview_create_schema() {
        assert_eq!(
            preview_of("CREATE SCHEMA IF NOT EXISTS price_histories;"),
            "CREATE SCHEMA price_histories"
        );
    }

    #[test]
    fn preview_create_view() {
        assert_eq!(
            preview_of("CREATE VIEW public.v_wallets AS SELECT * FROM wallets;"),
            "CREATE VIEW public.v_wallets"
        );
    }

    #[test]
    fn preview_create_or_replace_function() {
        assert_eq!(
            preview_of(
                "CREATE OR REPLACE FUNCTION public.my_func() RETURNS void AS $$ BEGIN END; $$ LANGUAGE plpgsql;"
            ),
            "CREATE OR REPLACE FUNCTION public.my_func"
        );
    }

    #[test]
    fn preview_insert() {
        assert_eq!(
            preview_of("INSERT INTO public.wallets (address) VALUES ('0x123');"),
            "INSERT INTO public.wallets"
        );
    }

    #[test]
    fn preview_grant() {
        assert_eq!(
            preview_of("GRANT SELECT ON ALL TABLES IN SCHEMA public TO readonly;"),
            "GRANT ON TABLE"
        );
    }

    #[test]
    fn preview_standalone_analyze() {
        assert_eq!(preview_of("ANALYZE;"), "ANALYZE");
    }
}
