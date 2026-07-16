use crate::INDENT;

pub(crate) enum TableItem {
    Column(usize),
    Constraint(String),
}

pub(crate) struct ColumnParts {
    pub(crate) name: String,
    pub(crate) type_str: String,
    pub(crate) default_expr: Option<String>,
    pub(crate) constraints: String,
}

pub(crate) fn fmt_table_body(
    header: &str,
    columns: &[ColumnParts],
    all_items: &[TableItem],
) -> String {
    if all_items.len() <= 1 {
        let single = match &all_items[0] {
            TableItem::Column(index) => {
                let column = &columns[*index];
                let mut text = format!("{} {}", column.name, column.type_str);
                if let Some(default) = &column.default_expr {
                    text.push(' ');
                    text.push_str(default);
                }
                if !column.constraints.is_empty() {
                    text.push(' ');
                    text.push_str(&column.constraints);
                }
                text
            }
            TableItem::Constraint(text) => text.clone(),
        };
        return format!("{header} ({single})");
    }

    let max_name = columns
        .iter()
        .map(|column| column.name.len())
        .max()
        .unwrap_or(0);
    let max_type = columns
        .iter()
        .map(|column| column.type_str.len())
        .max()
        .unwrap_or(0);
    let max_default = columns
        .iter()
        .map(|column| column.default_expr.as_ref().map_or(0, String::len))
        .max()
        .unwrap_or(0);
    let mut output = String::with_capacity(all_items.len() * 80);
    output.push_str(header);
    output.push_str(" (\n");
    for (index, item) in all_items.iter().enumerate() {
        output.push_str(INDENT);
        match item {
            TableItem::Column(column_index) => output.push_str(&fmt_column_line(
                &columns[*column_index],
                max_name,
                max_type,
                max_default,
            )),
            TableItem::Constraint(text) => output.push_str(text),
        }
        if index + 1 < all_items.len() {
            output.push(',');
        }
        output.push('\n');
    }
    output.push(')');
    output
}

pub(crate) fn fmt_column_line(
    column: &ColumnParts,
    max_name: usize,
    max_type: usize,
    max_default: usize,
) -> String {
    let mut line = String::new();
    line.push_str(&column.name);
    line.push_str(&" ".repeat(max_name - column.name.len()));
    line.push(' ');
    line.push_str(&column.type_str);
    if max_default > 0 {
        if let Some(default) = &column.default_expr {
            line.push_str(&" ".repeat(max_type - column.type_str.len()));
            line.push(' ');
            line.push_str(default);
            if !column.constraints.is_empty() {
                line.push_str(&" ".repeat(max_default - default.len()));
            }
        } else if !column.constraints.is_empty() {
            line.push_str(&" ".repeat(max_type - column.type_str.len() + 1 + max_default));
        }
    } else if !column.constraints.is_empty() {
        line.push_str(&" ".repeat(max_type - column.type_str.len()));
    }
    if !column.constraints.is_empty() {
        line.push(' ');
        line.push_str(&column.constraints);
    }
    line.trim_end().to_string()
}
