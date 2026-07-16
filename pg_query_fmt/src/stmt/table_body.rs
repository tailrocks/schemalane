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

#[derive(Clone, Copy)]
pub(crate) struct ColumnWidths {
    name: usize,
    type_name: usize,
    default: usize,
}

pub(crate) fn column_widths<'a>(
    columns: impl IntoIterator<Item = &'a ColumnParts>,
) -> ColumnWidths {
    columns.into_iter().fold(
        ColumnWidths {
            name: 0,
            type_name: 0,
            default: 0,
        },
        |widths, column| ColumnWidths {
            name: widths.name.max(column.name.len()),
            type_name: widths.type_name.max(column.type_str.len()),
            default: widths
                .default
                .max(column.default_expr.as_ref().map_or(0, String::len)),
        },
    )
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

    let widths = column_widths(columns);
    let mut output = String::with_capacity(all_items.len() * 80);
    output.push_str(header);
    output.push_str(" (\n");
    for (index, item) in all_items.iter().enumerate() {
        output.push_str(INDENT);
        match item {
            TableItem::Column(column_index) => {
                output.push_str(&fmt_column_line(&columns[*column_index], widths));
            }
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

pub(crate) fn fmt_column_line(column: &ColumnParts, widths: ColumnWidths) -> String {
    let mut line = String::new();
    line.push_str(&column.name);
    line.push_str(&" ".repeat(widths.name - column.name.len()));
    line.push(' ');
    line.push_str(&column.type_str);
    if widths.default > 0 {
        if let Some(default) = &column.default_expr {
            line.push_str(&" ".repeat(widths.type_name - column.type_str.len()));
            line.push(' ');
            line.push_str(default);
            if !column.constraints.is_empty() {
                line.push_str(&" ".repeat(widths.default - default.len()));
            }
        } else if !column.constraints.is_empty() {
            line.push_str(
                &" ".repeat(widths.type_name - column.type_str.len() + 1 + widths.default),
            );
        }
    } else if !column.constraints.is_empty() {
        line.push_str(&" ".repeat(widths.type_name - column.type_str.len()));
    }
    if !column.constraints.is_empty() {
        line.push(' ');
        line.push_str(&column.constraints);
    }
    line.trim_end().to_string()
}
