// SPDX-License-Identifier: Apache-2.0

//! The line rules, the size format and the snippets of the memory commands.

/// The longest file view in characters. A longer view is cut.
pub(super) const VIEW_LIMIT_CHARS: usize = 16_000;

/// The line that ends a cut view.
pub(super) const CUT_LINE: &str = "[The view is cut at 16,000 characters. Use view_range to read the rest.]";

/// The number of lines around an edit in a snippet.
const SNIPPET_LINES: usize = 4;

/// The lines of a file as `view` shows them: one final `\n` is removed, then the content is split on
/// `\n`. An empty file has one empty line. `\r` is content, not a line end.
pub(super) fn view_lines(content: &str) -> Vec<&str> {
    content.strip_suffix('\n').unwrap_or(content).split('\n').collect()
}

/// Number lines like `view`: a 6-wide right-aligned number, a tab and the text. `first` is the
/// number of the first line.
pub(super) fn number_lines(lines: &[&str], first: usize) -> String {
    lines.iter().enumerate().map(|(i, l)| format!("{:>6}\t{l}", first + i)).collect::<Vec<_>>().join("\n")
}

/// Cut a view body at the character limit. The cut line is added when the body is longer.
pub(super) fn cut_view(body: String) -> String {
    if body.chars().count() <= VIEW_LIMIT_CHARS {
        return body;
    }
    let mut cut: String = body.chars().take(VIEW_LIMIT_CHARS).collect();
    cut.push('\n');
    cut.push_str(CUT_LINE);
    cut
}

/// A size as `view` shows it: `0.5K` below 1 MiB, else `1.5M`.
pub(super) fn format_size(bytes: u64) -> String {
    #[allow(clippy::cast_precision_loss)]
    let b = bytes as f64;
    if bytes < 1_048_576 { format!("{:.1}K", b / 1024.0) } else { format!("{:.1}M", b / 1_048_576.0) }
}

/// The snippet after a `str_replace`: the edited lines with four lines around them, numbered like
/// `view`. `first_line` is the zero-based line of the start of the edit. `new_breaks` is the number
/// of line breaks in the new text.
pub(super) fn snippet(content: &str, first_line: usize, new_breaks: usize) -> String {
    let lines = view_lines(content);
    let end = (first_line + SNIPPET_LINES + new_breaks + 1).min(lines.len());
    let start = first_line.saturating_sub(SNIPPET_LINES).min(end);
    number_lines(&lines[start..end], start + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_rules() {
        assert_eq!(view_lines("a\nb\n"), ["a", "b"]);
        assert_eq!(view_lines(""), [""]);
        assert_eq!(view_lines("a\r\nb"), ["a\r", "b"]);
    }

    #[test]
    fn sizes() {
        assert_eq!(format_size(512), "0.5K");
        assert_eq!(format_size(1536), "1.5K");
        assert_eq!(format_size(1_572_864), "1.5M");
    }

    #[test]
    fn numbering() {
        assert_eq!(number_lines(&["x"], 7), "     7\tx");
    }
}
