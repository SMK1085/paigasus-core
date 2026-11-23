// SPDX-License-Identifier: Apache-2.0
//! The walk over the strings of one tool call (§ 6.3, "Common input handling").
//!
//! The walk reads every decoded string value and every object key. A lenient reader keeps each
//! duplicate member, so an escape behind a duplicate key cannot hide a value.

use std::cell::Cell;
use std::collections::HashSet;
use std::fmt;

use serde::de::{DeserializeSeed, Deserializer, MapAccess, SeqAccess, Visitor};

use super::ToolInput;
use super::input::{nesting_ok, zero_numbers};
use super::secrets::is_secret_token;

/// One string of the arguments.
pub(crate) struct Leaf<'a> {
    /// The key of the object that holds the value. A key leaf has `None`.
    pub key: Option<&'a str>,
    /// The JSON pointer of the value or of the key. It is empty for raw and free text.
    pub pointer: &'a str,
    /// The pointer cut before the first object key that is a secret. The walk tests each key
    /// once, when it pushes the key, so this costs nothing more per leaf.
    pub safe_pointer: &'a str,
    /// The decoded text.
    pub text: &'a str,
}

/// How the walk reads one input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InputMode {
    /// One JSON object with unique keys.
    Strict,
    /// Valid JSON, but an object repeats a key.
    DuplicateKeys,
    /// Valid JSON that is not an object.
    NotObject,
    /// Invalid JSON, or nested deeper than 128 levels: the raw text.
    Raw,
    /// Free text.
    Text,
}

enum Node {
    Str(String),
    Obj(Vec<(String, Node)>),
    Arr(Vec<Node>),
    Other,
}

struct Seed<'a>(&'a Cell<bool>);

impl<'de> DeserializeSeed<'de> for Seed<'_> {
    type Value = Node;
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Node, D::Error> {
        d.deserialize_any(NodeVisitor(self.0))
    }
}

struct NodeVisitor<'a>(&'a Cell<bool>);

impl<'de> Visitor<'de> for NodeVisitor<'_> {
    type Value = Node;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("any JSON value")
    }
    fn visit_bool<E>(self, _: bool) -> Result<Node, E> {
        Ok(Node::Other)
    }
    fn visit_i64<E>(self, _: i64) -> Result<Node, E> {
        Ok(Node::Other)
    }
    fn visit_u64<E>(self, _: u64) -> Result<Node, E> {
        Ok(Node::Other)
    }
    fn visit_f64<E>(self, _: f64) -> Result<Node, E> {
        Ok(Node::Other)
    }
    fn visit_unit<E>(self) -> Result<Node, E> {
        Ok(Node::Other)
    }
    fn visit_str<E>(self, v: &str) -> Result<Node, E> {
        Ok(Node::Str(v.to_owned()))
    }
    fn visit_string<E>(self, v: String) -> Result<Node, E> {
        Ok(Node::Str(v))
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Node, A::Error> {
        let mut out = Vec::new();
        while let Some(n) = seq.next_element_seed(Seed(self.0))? {
            out.push(n);
        }
        Ok(Node::Arr(out))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Node, A::Error> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        while let Some(key) = map.next_key::<String>()? {
            if !seen.insert(key.clone()) {
                self.0.set(true);
            }
            out.push((key, map.next_value_seed(Seed(self.0))?));
        }
        Ok(Node::Obj(out))
    }
}

/// The parsed tree and whether an object repeats a key. `None` is invalid JSON, or JSON that
/// nests deeper than 128 levels. The strict reader allows exactly 128 levels (`nesting_ok`), so
/// the walk checks that first and then lifts the 127 level limit of `serde_json`. The check
/// bounds the recursion. A number outside the `f64` range (`1e400`) is valid JSON: the walk then
/// reads a copy with every number as `0`, because it reads no number ([`zero_numbers`]).
fn parse(text: &str) -> Option<(Node, bool)> {
    if !nesting_ok(text) {
        return None;
    }
    tree(text).or_else(|| tree(&zero_numbers(text)?))
}

fn tree(text: &str) -> Option<(Node, bool)> {
    let duplicate = Cell::new(false);
    let mut de = serde_json::Deserializer::from_str(text);
    de.disable_recursion_limit();
    let node = Seed(&duplicate).deserialize(&mut de).and_then(|n| de.end().map(|()| n)).ok()?;
    Some((node, duplicate.get()))
}

/// How the walk reads `input`.
pub(crate) fn input_mode(input: &ToolInput) -> InputMode {
    match input {
        ToolInput::Json(text) => match parse(text) {
            None => InputMode::Raw,
            Some((_, true)) => InputMode::DuplicateKeys,
            Some((Node::Obj(_), false)) => InputMode::Strict,
            Some(_) => InputMode::NotObject,
        },
        _ => InputMode::Text,
    }
}

fn hex4(b: &[u8]) -> Option<u32> {
    let digits = b.get(..4)?;
    digits.iter().try_fold(0u32, |acc, d| Some(acc * 16 + char::from(*d).to_digit(16)?))
}

/// The low surrogate of a `\uXXXX` escape at `b[i..]`, if there is one.
fn low_surrogate(b: &[u8], i: usize) -> Option<u32> {
    if b.get(i) != Some(&b'\\') || b.get(i + 1) != Some(&b'u') {
        return None;
    }
    hex4(b.get(i + 2..)?).filter(|l| (0xDC00..0xE000).contains(l))
}

/// Decode `\/`, `\\` and `\uXXXX` in a text that is not valid JSON, in one pass. A surrogate pair
/// gives one character. A lone surrogate gives U+FFFD. Any other backslash stays as it is.
fn decode_raw(text: &str) -> String {
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'\\' {
            let run = text[i..].find('\\').unwrap_or(text.len() - i);
            out.push_str(&text[i..i + run]);
            i += run;
            continue;
        }
        let unit = (b.get(i + 1) == Some(&b'u')).then(|| b.get(i + 2..).and_then(hex4)).flatten();
        match (b.get(i + 1), unit) {
            (Some(b'/'), _) => {
                out.push('/');
                i += 2;
            }
            (Some(b'\\'), _) => {
                out.push('\\');
                i += 2;
            }
            (_, Some(unit)) => {
                i += 6;
                let ch = match unit {
                    0xD800..0xDC00 => low_surrogate(b, i).and_then(|low| {
                        i += 6;
                        char::from_u32(0x10000 + ((unit - 0xD800) << 10) + (low - 0xDC00))
                    }),
                    0xDC00..0xE000 => None,
                    u => char::from_u32(u),
                };
                out.push(ch.unwrap_or('\u{FFFD}'));
            }
            _ => {
                out.push('\\');
                i += 1;
            }
        }
    }
    out
}

/// Append `key` to a JSON pointer, with `~` as `~0` and `/` as `~1`.
pub(crate) fn escape_into(out: &mut String, key: &str) {
    for c in key.chars() {
        match c {
            '~' => out.push_str("~0"),
            '/' => out.push_str("~1"),
            c => out.push(c),
        }
    }
}

/// `cut` is the length of the pointer before the first key that is a secret, if there is one.
fn visit(node: &Node, key: Option<&str>, pointer: &mut String, cut: Option<usize>, f: &mut dyn FnMut(Leaf<'_>)) {
    match node {
        Node::Str(text) => {
            let safe_pointer = &pointer[..cut.unwrap_or(pointer.len())];
            f(Leaf { key, pointer, safe_pointer, text });
        }
        Node::Obj(members) => {
            for (k, v) in members {
                let len = pointer.len();
                pointer.push('/');
                escape_into(pointer, k);
                let cut = cut.or_else(|| is_secret_token(k).then_some(len));
                let safe_pointer = &pointer[..cut.unwrap_or(pointer.len())];
                f(Leaf {
                    key: None,
                    pointer,
                    safe_pointer,
                    text: k,
                });
                visit(v, Some(k), pointer, cut, f);
                pointer.truncate(len);
            }
        }
        Node::Arr(items) => {
            for (i, v) in items.iter().enumerate() {
                let len = pointer.len();
                pointer.push('/');
                pointer.push_str(&i.to_string());
                visit(v, key, pointer, cut, f);
                pointer.truncate(len);
            }
        }
        Node::Other => {}
    }
}

/// Call `f` for every decoded string value and every object key of the input.
pub(crate) fn walk_strings(input: &ToolInput, f: &mut dyn FnMut(Leaf<'_>)) {
    match input {
        ToolInput::Json(text) => match parse(text) {
            Some((node, _)) => visit(&node, None, &mut String::new(), None, f),
            None => {
                let raw = decode_raw(text);
                f(Leaf {
                    key: None,
                    pointer: "",
                    safe_pointer: "",
                    text: &raw,
                });
            }
        },
        ToolInput::Text(text) => f(Leaf {
            key: None,
            pointer: "",
            safe_pointer: "",
            text,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaves(input: &ToolInput) -> Vec<(Option<String>, String, String)> {
        let mut out = Vec::new();
        walk_strings(input, &mut |l| out.push((l.key.map(str::to_owned), l.pointer.to_owned(), l.text.to_owned())));
        out
    }
    fn mode(raw: &str) -> InputMode {
        input_mode(&ToolInput::Json(raw.to_owned()))
    }

    #[test]
    fn the_modes() {
        assert_eq!(mode(r#"{"a":1}"#), InputMode::Strict);
        assert_eq!(mode(r#"{"a":1,"a":2}"#), InputMode::DuplicateKeys);
        assert_eq!(mode(r#"{"x":{"a":1,"a":2}}"#), InputMode::DuplicateKeys);
        assert_eq!(mode(r#"["a"]"#), InputMode::NotObject);
        assert_eq!(mode(r#"{"a":"#), InputMode::Raw);
        assert_eq!(mode(r#"{"a":1} x"#), InputMode::Raw);
        // A number outside the f64 range is valid JSON, as for the Cedar mapping.
        assert_eq!(mode(r#"{"host":"docs.example","n":1e400}"#), InputMode::Strict);
        assert_eq!(mode(r#"{"a":1e400,"a":-1E+999}"#), InputMode::DuplicateKeys);
        assert_eq!(mode("[1e400]"), InputMode::NotObject);
        // An invalid number stays invalid: the copy with zeros is not read.
        assert_eq!(mode(r#"{"a":1.2.3}"#), InputMode::Raw);
        assert_eq!(mode(r#"{"a":01}"#), InputMode::Raw);
        assert_eq!(input_mode(&ToolInput::Text("x".into())), InputMode::Text);
        let deep = format!("{}1{}", "[".repeat(129), "]".repeat(129));
        assert_eq!(mode(&deep), InputMode::Raw);
        let ok = format!("{}1{}", "[".repeat(100), "]".repeat(100));
        assert_eq!(mode(&ok), InputMode::NotObject);
    }

    #[test]
    fn keys_values_and_pointers() {
        let got = leaves(&ToolInput::Json(r#"{"a/b":["x",{"k":"y"}]}"#.into()));
        assert_eq!(
            got,
            [
                (None, "/a~1b".into(), "a/b".into()),
                (Some("a/b".into()), "/a~1b/0".into(), "x".into()),
                (None, "/a~1b/1/k".into(), "k".into()),
                (Some("k".into()), "/a~1b/1/k".into(), "y".into()),
            ]
        );
    }

    #[test]
    fn depth_128_is_walked_and_129_is_raw() {
        let nest = |n: usize| format!("{}\"x\"{}", "[".repeat(n), "]".repeat(n));
        assert_eq!(mode(&nest(127)), InputMode::NotObject);
        assert_eq!(mode(&nest(128)), InputMode::NotObject);
        assert_eq!(mode(&nest(129)), InputMode::Raw);
        let got = leaves(&ToolInput::Json(nest(128)));
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].2, "x");
    }

    #[test]
    fn raw_text_decodes_slash_backslash_and_unicode_escapes() {
        let raw = |t: &str| leaves(&ToolInput::Json(format!("{t} \"")))[0].2.clone();
        assert_eq!(raw(r"a\/b e \\ \\u0065"), "a/b e \\ \\u0065 \"");
        assert_eq!(raw(r"😀"), "\u{1F600} \"");
        assert_eq!(raw(r"\ud800x \udc00 \u12 \q"), "\u{FFFD}x \u{FFFD} \\u12 \\q \"");
        assert_eq!(raw("é日本\\"), "é日本\\ \"");
    }

    #[test]
    fn duplicates_are_all_walked_and_raw_is_unescaped() {
        let got = leaves(&ToolInput::Json(r#"{"a":"p","a":"q"}"#.into()));
        assert_eq!(got.iter().filter(|l| l.2 == "q").count(), 1);
        let got = leaves(&ToolInput::Json(r#"{"a":"x\/y"#.into()));
        assert_eq!(got, [(None, String::new(), r#"{"a":"x/y"#.into())]);
    }
}
