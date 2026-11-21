// SPDX-License-Identifier: Apache-2.0

//! The delimiter scheme for text that the model must treat as data.
//!
//! A block has the form `<{kind}-{nonce} k="v">` ... `</{kind}-{nonce}>`. The caller makes the
//! nonce, one random value per run. The content cannot close the block, because `escape` writes
//! `&lt;` in place of `<` for every open or close prefix that holds the same kind and nonce.
//!
//! The ADR (m) event renderer (SMA-787/SMA-790) must use these same functions, so that there is
//! one scheme for all untrusted text.

/// The open tag of a block. An attribute value has `"` written as `&quot;`.
///
/// The ADR (m) event renderer (SMA-787/SMA-790) must use this function.
#[must_use]
pub fn open_tag(kind: &str, nonce: &str, attrs: &[(&str, &str)]) -> String {
    let mut tag = format!("<{kind}-{nonce}");
    for (key, value) in attrs {
        tag.push_str(&format!(" {key}=\"{}\"", value.replace('"', "&quot;")));
    }
    tag.push('>');
    tag
}

/// The close tag of a block.
///
/// The ADR (m) event renderer (SMA-787/SMA-790) must use this function.
#[must_use]
pub fn close_tag(kind: &str, nonce: &str) -> String {
    format!("</{kind}-{nonce}>")
}

/// Neutralise every open and close tag prefix of `kind` and `nonce` in `content`. The match is
/// case-sensitive. The function does not change any other text.
///
/// The ADR (m) event renderer (SMA-787/SMA-790) must use this function.
#[must_use]
pub fn escape(kind: &str, content: &str, nonce: &str) -> String {
    let close = format!("</{kind}-{nonce}");
    let open = format!("<{kind}-{nonce}");
    content.replace(&close, &format!("&lt;/{kind}-{nonce}")).replace(&open, &format!("&lt;{kind}-{nonce}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_have_the_documented_form() {
        assert_eq!(open_tag("memory", "N1", &[("path", "/memories/a.md")]), "<memory-N1 path=\"/memories/a.md\">");
        assert_eq!(open_tag("memory", "N1", &[]), "<memory-N1>");
        assert_eq!(close_tag("memory", "N1"), "</memory-N1>");
    }

    #[test]
    fn attribute_quote_is_escaped() {
        assert_eq!(open_tag("m", "N", &[("k", "a\"b")]), "<m-N k=\"a&quot;b\">");
    }

    #[test]
    fn escape_neutralises_open_and_close_prefix() {
        let out = escape("memory", "a <memory-N1 x> b </memory-N1> c", "N1");
        assert_eq!(out, "a &lt;memory-N1 x> b &lt;/memory-N1> c");
    }

    #[test]
    fn escape_leaves_other_text_alone() {
        let text = "<memory-N2> </memory-N2> <Memory-N1> <other-N1> </memory> <b>x</b> &lt;";
        assert_eq!(escape("memory", text, "N1"), text);
    }
}
