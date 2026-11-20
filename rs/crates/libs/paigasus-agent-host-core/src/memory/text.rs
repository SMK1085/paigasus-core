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

/// Number lines like `number_lines`, and cut the body at the character limit. When the body is
/// longer than the limit, the first `VIEW_LIMIT_CHARS` characters stay and the cut line follows.
/// It stops at the limit, so it does not format or join the lines after the cut.
pub(super) fn number_lines_cut(lines: &[&str], first: usize) -> String {
    let mut out = String::new();
    let mut left = VIEW_LIMIT_CHARS;
    for (i, line) in lines.iter().enumerate() {
        let prefix = if i == 0 { format!("{:>6}\t", first + i) } else { format!("\n{:>6}\t", first + i) };
        if !push_limited(&mut out, &mut left, &prefix) || !push_limited(&mut out, &mut left, line) {
            out.push('\n');
            out.push_str(CUT_LINE);
            return out;
        }
    }
    out
}

/// Push `text` while `left` characters remain. It returns false when `text` did not fit whole.
fn push_limited(out: &mut String, left: &mut usize, text: &str) -> bool {
    // The byte length is an upper bound of the character count, so a short text needs no count.
    if text.len() <= *left {
        out.push_str(text);
        *left -= text.chars().count();
        return true;
    }
    let mut taken = 0;
    for (at, _) in text.char_indices() {
        if taken == *left {
            out.push_str(&text[..at]);
            *left = 0;
            return false;
        }
        taken += 1;
    }
    out.push_str(text);
    *left -= taken;
    true
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

    /// The old view: format and join every line, then cut the whole body.
    fn reference_view(lines: &[&str], first: usize) -> String {
        let body = number_lines(lines, first);
        if body.chars().count() <= VIEW_LIMIT_CHARS {
            return body;
        }
        let mut cut: String = body.chars().take(VIEW_LIMIT_CHARS).collect();
        cut.push('\n');
        cut.push_str(CUT_LINE);
        cut
    }

    #[test]
    fn the_bounded_view_matches_the_full_view_then_cut() {
        // One line of 7 characters with its number is "     1\t" plus the text.
        let exact = "y".repeat(VIEW_LIMIT_CHARS - 7);
        let over = "y".repeat(VIEW_LIMIT_CHARS - 6);
        let wide = "\u{e9}".repeat(VIEW_LIMIT_CHARS);
        let many: Vec<String> = (0..5_000).map(|n| format!("line {n} \u{1f600}")).collect();
        let many: Vec<&str> = many.iter().map(String::as_str).collect();
        let cases: Vec<(Vec<&str>, usize)> = vec![
            (vec!["a", "b"], 1),
            (vec![""], 1),
            (vec![exact.as_str()], 1),
            (vec![over.as_str()], 1),
            (vec![wide.as_str()], 3),
            (vec!["a", exact.as_str()], 1),
            (many, 1),
        ];
        for (lines, first) in cases {
            let got = number_lines_cut(&lines, first);
            assert_eq!(got, reference_view(&lines, first), "{} lines", lines.len());
        }
        assert!(!number_lines_cut(&[exact.as_str()], 1).contains(CUT_LINE));
        assert!(number_lines_cut(&[over.as_str()], 1).ends_with(CUT_LINE));
    }
}
