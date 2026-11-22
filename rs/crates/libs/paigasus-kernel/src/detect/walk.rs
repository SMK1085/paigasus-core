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

/// One string of the arguments.
pub(crate) struct Leaf<'a> {
    /// The key of the object that holds the value. A key leaf has `None`.
    pub key: Option<&'a str>,
    /// The JSON pointer of the value or of the key. It is empty for raw and free text.
    pub pointer: &'a str,
    /// The decoded text.
    pub text: &'a str,
}

/// How the walk reads one input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(test)]
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

/// The parsed tree and whether an object repeats a key. `None` is invalid JSON. `serde_json`
/// stops at 128 levels, so a deeper text is `None` too.
fn parse(text: &str) -> Option<(Node, bool)> {
    let duplicate = Cell::new(false);
    let mut de = serde_json::Deserializer::from_str(text);
    let node = Seed(&duplicate).deserialize(&mut de).and_then(|n| de.end().map(|()| n)).ok()?;
    Some((node, duplicate.get()))
}

/// How the walk reads `input`.
// Only its test calls it for now. The detectors that read the mode take it over.
#[cfg(test)]
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

fn escape_into(out: &mut String, key: &str) {
    for c in key.chars() {
        match c {
            '~' => out.push_str("~0"),
            '/' => out.push_str("~1"),
            c => out.push(c),
        }
    }
}

fn visit(node: &Node, key: Option<&str>, pointer: &mut String, f: &mut dyn FnMut(Leaf<'_>)) {
    match node {
        Node::Str(text) => f(Leaf { key, pointer, text }),
        Node::Obj(members) => {
            for (k, v) in members {
                let len = pointer.len();
                pointer.push('/');
                escape_into(pointer, k);
                f(Leaf { key: None, pointer, text: k });
                visit(v, Some(k), pointer, f);
                pointer.truncate(len);
            }
        }
        Node::Arr(items) => {
            for (i, v) in items.iter().enumerate() {
                let len = pointer.len();
                pointer.push('/');
                pointer.push_str(&i.to_string());
                visit(v, key, pointer, f);
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
            Some((node, _)) => visit(&node, None, &mut String::new(), f),
            None => {
                let raw = text.replace("\\/", "/");
                f(Leaf { key: None, pointer: "", text: &raw });
            }
        },
        ToolInput::Text(text) => f(Leaf { key: None, pointer: "", text }),
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
    fn duplicates_are_all_walked_and_raw_is_unescaped() {
        let got = leaves(&ToolInput::Json(r#"{"a":"p","a":"q"}"#.into()));
        assert_eq!(got.iter().filter(|l| l.2 == "q").count(), 1);
        let got = leaves(&ToolInput::Json(r#"{"a":"x\/y"#.into()));
        assert_eq!(got, [(None, String::new(), r#"{"a":"x/y"#.into())]);
    }
}
