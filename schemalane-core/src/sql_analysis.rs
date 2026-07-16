use pg_query::protobuf;

use crate::SchemalaneError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SqlTransactionMode {
    Transactional,
    NonTransactional,
}

pub(crate) struct ParsedSqlStatement {
    pub(crate) sql: String,
    pub(crate) source_line: u64,
    pub(crate) node: Option<protobuf::Node>,
}

pub(crate) fn parse_sql_migration(sql: &str) -> Result<Vec<ParsedSqlStatement>, SchemalaneError> {
    let stmts = pg_query::split_with_parser(sql).map_err(|err| {
        SchemalaneError::Validation(format!("failed to split SQL migration: {err}"))
    })?;
    let mut result = Vec::with_capacity(stmts.len());
    for stmt_sql in stmts {
        let trimmed = stmt_sql.trim();
        if trimmed.is_empty() {
            continue;
        }
        let parsed = pg_query::parse(trimmed).map_err(|err| {
            SchemalaneError::Validation(format!("failed to parse SQL statement: {err}"))
        })?;
        let source_line = offset_to_line(sql, trimmed);
        let node = parsed
            .protobuf
            .stmts
            .into_iter()
            .next()
            .and_then(|raw_stmt| raw_stmt.stmt.map(|node| *node));
        result.push(ParsedSqlStatement {
            sql: trimmed.to_owned(),
            source_line,
            node,
        });
    }
    Ok(result)
}

fn offset_to_line(full_sql: &str, stmt_slice: &str) -> u64 {
    let offset = stmt_slice.as_ptr() as usize - full_sql.as_ptr() as usize;
    let prefix = &full_sql[..offset.min(full_sql.len())];
    (prefix
        .chars()
        .filter(|&character| character == '\n')
        .count()
        + 1) as u64
}

pub(crate) fn is_non_transactional(stmt: &ParsedSqlStatement) -> bool {
    use protobuf::node::Node;

    let Some(node) = &stmt.node else { return false };
    let Some(node_inner) = &node.node else {
        return false;
    };
    match node_inner {
        Node::IndexStmt(index) => index.concurrent,
        Node::DropStmt(drop) => drop.concurrent,
        Node::VacuumStmt(vacuum) => vacuum.is_vacuumcmd,
        Node::ReindexStmt(reindex) => {
            let kind = reindex.kind;
            kind == protobuf::ReindexObjectType::ReindexObjectSchema as i32
                || kind == protobuf::ReindexObjectType::ReindexObjectDatabase as i32
                || kind == protobuf::ReindexObjectType::ReindexObjectSystem as i32
        }
        Node::DiscardStmt(discard) => discard.target == protobuf::DiscardMode::DiscardAll as i32,
        Node::AlterSystemStmt(_)
        | Node::CreatedbStmt(_)
        | Node::CreateTableSpaceStmt(_)
        | Node::CreateSubscriptionStmt(_)
        | Node::DropdbStmt(_)
        | Node::DropTableSpaceStmt(_)
        | Node::DropSubscriptionStmt(_) => true,
        _ => false,
    }
}

pub(crate) fn resolve_sql_transaction_mode(
    statements: &[ParsedSqlStatement],
    script: &str,
) -> Result<SqlTransactionMode, SchemalaneError> {
    let mut first_transactional_line = None;
    let mut first_non_transactional_line = None;
    for statement in statements {
        let slot = if is_non_transactional(statement) {
            &mut first_non_transactional_line
        } else {
            &mut first_transactional_line
        };
        slot.get_or_insert(statement.source_line);
    }
    if let (Some(transactional), Some(non_transactional)) =
        (first_transactional_line, first_non_transactional_line)
    {
        return Err(SchemalaneError::MixedStatements {
            script: script.to_owned(),
            line: if transactional < non_transactional {
                non_transactional
            } else {
                transactional
            },
        });
    }
    Ok(if first_non_transactional_line.is_some() {
        SqlTransactionMode::NonTransactional
    } else {
        SqlTransactionMode::Transactional
    })
}
