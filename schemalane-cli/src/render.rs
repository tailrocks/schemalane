use clap::ValueEnum;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
pub enum Verbosity {
    /// Migration file names only.
    #[default]
    Minimal,
    /// Summarized operations (e.g. CREATE TABLE name).
    Compact,
    /// Full SQL queries.
    Detailed,
}

pub(crate) const INDENT: &str = " ";
pub(crate) const MAX_PREVIEW_WIDTH: usize = 60;
pub(crate) const STATUS_WIDTH: usize = 7;

pub(crate) fn sanitize_terminal(text: &str) -> String {
    text.chars()
        .filter(|character| !character.is_control() || *character == '\n' || *character == '\t')
        .collect()
}

pub(crate) fn pad_index(index: usize, total: usize) -> String {
    let width = total.to_string().len().max(2);
    format!("{index:0>width$}")
}

pub(crate) fn format_elapsed(milliseconds: i32) -> String {
    if milliseconds >= 1000 {
        let seconds = f64::from(milliseconds) / 1000.0;
        format!("{seconds:.1} s")
    } else {
        format!("{milliseconds} ms")
    }
}

pub(crate) fn truncate_preview(text: &str, max_width: usize) -> String {
    debug_assert!(max_width >= 3, "truncate_preview needs room for ellipsis");
    if text.chars().count() <= max_width {
        return text.to_owned();
    }
    let truncated: String = text.chars().take(max_width.saturating_sub(3)).collect();
    format!("{truncated}...")
}
