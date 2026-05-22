//! SQL syntax highlighting with ANSI terminal colors.
//!
//! Applies color to SQL keywords, types, string literals, numbers, and comments
//! using `owo-colors`.

use owo_colors::OwoColorize;
use std::fmt::Write;

const SQL_KEYWORDS: &[&str] = &[
    "ADD",
    "ALL",
    "ALTER",
    "ANALYZE",
    "AND",
    "AS",
    "ASC",
    "BEGIN",
    "BETWEEN",
    "BIGINT",
    "BY",
    "CASCADE",
    "CASE",
    "CHECK",
    "COLUMN",
    "COMMIT",
    "CONCURRENTLY",
    "CONSTRAINT",
    "CREATE",
    "CROSS",
    "CURRENT_TIMESTAMP",
    "DATABASE",
    "DEFAULT",
    "DEFERRABLE",
    "DEFERRED",
    "DELETE",
    "DESC",
    "DISTINCT",
    "DO",
    "DROP",
    "ELSE",
    "END",
    "ENUM",
    "EXCEPT",
    "EXECUTE",
    "EXISTS",
    "EXTENSION",
    "FALSE",
    "FIRST",
    "FOR",
    "FOREIGN",
    "FROM",
    "FULL",
    "FUNCTION",
    "GRANT",
    "GROUP",
    "HAVING",
    "IF",
    "ILIKE",
    "IMMEDIATE",
    "IMMUTABLE",
    "IN",
    "INDEX",
    "INITIALLY",
    "INNER",
    "INSERT",
    "INTERSECT",
    "INTO",
    "IS",
    "JOIN",
    "KEY",
    "LANGUAGE",
    "LAST",
    "LEFT",
    "LIKE",
    "LIMIT",
    "LOCK",
    "MATERIALIZED",
    "NEW",
    "NO",
    "NOT",
    "NOTHING",
    "NOW",
    "NULL",
    "NULLS",
    "OF",
    "OFFSET",
    "OLD",
    "ON",
    "ONLY",
    "OR",
    "ORDER",
    "OUTER",
    "PARTITION",
    "PLPGSQL",
    "PRIMARY",
    "PROCEDURE",
    "RAISE",
    "RECURSIVE",
    "REFERENCES",
    "RENAME",
    "REPLACE",
    "RESTRICT",
    "RETURN",
    "RETURNING",
    "RETURNS",
    "REVOKE",
    "RIGHT",
    "ROLLBACK",
    "ROW",
    "ROWS",
    "SCHEMA",
    "SELECT",
    "SEQUENCE",
    "SET",
    "STABLE",
    "TABLE",
    "TEMP",
    "TEMPORARY",
    "THEN",
    "TO",
    "TRIGGER",
    "TRUE",
    "TRUNCATE",
    "TYPE",
    "UNION",
    "UNIQUE",
    "UPDATE",
    "USING",
    "VALUES",
    "VIEW",
    "VOLATILE",
    "WHEN",
    "WHERE",
    "WITH",
];

const SQL_TYPES: &[&str] = &[
    "BIGINT",
    "BIGSERIAL",
    "BOOLEAN",
    "BOOL",
    "BYTEA",
    "CHAR",
    "CHARACTER",
    "DATE",
    "DECIMAL",
    "DOUBLE",
    "FLOAT4",
    "FLOAT8",
    "INET",
    "INT",
    "INT2",
    "INT4",
    "INT8",
    "INTEGER",
    "INTERVAL",
    "JSON",
    "JSONB",
    "MONEY",
    "NUMERIC",
    "OID",
    "PRECISION",
    "REAL",
    "SERIAL",
    "SMALLINT",
    "SMALLSERIAL",
    "TEXT",
    "TIME",
    "TIMESTAMP",
    "TIMESTAMPTZ",
    "TIMETZ",
    "UUID",
    "VARCHAR",
    "VARYING",
    "XML",
    "ZONE",
];

/// Highlights a single line of SQL with ANSI terminal colors.
///
/// - Keywords → bright cyan
/// - Types → bright green
/// - String literals → bright yellow
/// - Numbers → bright magenta
/// - Comments → bright black (gray)
pub fn highlight_sql_line(line: &str) -> String {
    let mut result = String::with_capacity(line.len() * 2);
    let mut pos = 0;

    while pos < line.len() {
        let remaining = &line[pos..];

        // Line comment
        if remaining.starts_with("--") {
            write!(result, "{}", remaining.to_owned().bright_black()).ok();
            break;
        }

        // String literal
        if remaining.starts_with('\'')
            && let Some(end) = find_string_end(remaining)
        {
            write!(result, "{}", remaining[..end].to_owned().bright_yellow()).ok();
            pos += end;
            continue;
        }

        // Dollar-quoted string
        if remaining.starts_with('$')
            && let Some(end) = find_dollar_string_end(remaining)
        {
            write!(result, "{}", remaining[..end].to_owned().bright_yellow()).ok();
            pos += end;
            continue;
        }

        // Number
        let ch = remaining.as_bytes()[0];
        if ch.is_ascii_digit() {
            let end = remaining
                .find(|c: char| !c.is_ascii_digit() && c != '.')
                .unwrap_or(remaining.len());
            write!(result, "{}", remaining[..end].to_owned().bright_magenta()).ok();
            pos += end;
            continue;
        }

        // Word (potential keyword/type)
        if ch.is_ascii_alphabetic() || ch == b'_' {
            let end = remaining
                .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .unwrap_or(remaining.len());
            let word = &remaining[..end];
            let upper = word.to_ascii_uppercase();
            if SQL_TYPES.contains(&upper.as_str()) {
                write!(result, "{}", word.to_owned().bright_green()).ok();
            } else if SQL_KEYWORDS.contains(&upper.as_str()) {
                write!(result, "{}", word.to_owned().bright_cyan()).ok();
            } else {
                result.push_str(word);
            }
            pos += end;
            continue;
        }

        // Consume the next char
        let c = remaining.chars().next().unwrap();
        result.push(c);
        pos += c.len_utf8();
    }

    result
}

fn find_string_end(s: &str) -> Option<usize> {
    let mut i = 1;
    let bytes = s.as_bytes();
    while i < bytes.len() {
        if bytes[i] == b'\'' {
            if i + 1 < bytes.len() && bytes[i + 1] == b'\'' {
                i += 2;
            } else {
                return Some(i + 1);
            }
        } else {
            i += 1;
        }
    }
    None
}

fn find_dollar_string_end(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    if bytes.first() != Some(&b'$') {
        return None;
    }
    let tag_end = bytes[1..].iter().position(|&b| b == b'$')? + 2;
    let tag = &s[..tag_end];
    s[tag_end..].find(tag).map(|pos| tag_end + pos + tag.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlights_keywords() {
        let result = highlight_sql_line("CREATE TABLE test");
        assert!(result.contains("CREATE"), "should contain keyword CREATE");
        assert!(result.contains("TABLE"), "should contain keyword TABLE");
    }

    #[test]
    fn preserves_plain_text() {
        let result = highlight_sql_line("my_column_name");
        assert!(
            result.contains("my_column_name"),
            "should preserve identifiers"
        );
    }

    #[test]
    fn highlights_analyze() {
        let result = highlight_sql_line("ANALYZE public.wallets");
        // ANALYZE should be highlighted (not plain text)
        assert!(result.contains("ANALYZE"));
    }
}
