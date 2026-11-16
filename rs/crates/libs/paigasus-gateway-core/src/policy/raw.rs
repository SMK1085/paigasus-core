// SPDX-License-Identifier: Apache-2.0
//! A JSON object and array splitter over `RawValue` slices (SMA-752 P-8). The request rewrite
//! edits a body through these splits: an edited container is written again from the original
//! text of every member it keeps, so a member that the rewrite does not change keeps its bytes
//! (escaped keys, white space, and number text such as `1e400`). No I/O.

use std::fmt;

use serde::de::{Deserialize, Deserializer, MapAccess, Visitor};
use serde_json::value::RawValue;

/// The byte offset of `inner` in `outer`. `inner` must borrow from `outer`.
fn offset(outer: &str, inner: &str) -> Option<usize> {
    let at = (inner.as_ptr() as usize).checked_sub(outer.as_ptr() as usize)?;
    (at + inner.len() <= outer.len()).then_some(at)
}

/// The members of an object, in order, with their raw values.
struct Pairs<'a>(Vec<(String, &'a RawValue)>);

impl<'de> Deserialize<'de> for Pairs<'de> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Pairs<'de>;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a JSON object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Pairs<'de>, A::Error> {
                let mut v = Vec::new();
                while let Some(k) = map.next_key::<String>()? {
                    v.push((k, map.next_value::<&'de RawValue>()?));
                }
                Ok(Pairs(v))
            }
        }
        d.deserialize_map(V)
    }
}

/// One object member: the white space and comma before the key (`prefix`), the raw key through the
/// colon and the white space after it (`head`), and the raw value.
pub(crate) struct Member<'a> {
    /// The decoded key.
    pub key: String,
    prefix: &'a str,
    head: &'a str,
    pub value: &'a RawValue,
}

/// A JSON object split into its members.
pub(crate) struct RawObject<'a> {
    pub members: Vec<Member<'a>>,
    /// The white space before the closing brace.
    tail: &'a str,
}

impl<'a> RawObject<'a> {
    /// Split the raw text of one JSON object. `None` when the text is not one object.
    pub fn parse(text: &'a str) -> Option<Self> {
        let Pairs(pairs) = serde_json::from_str(text).ok()?;
        let trimmed = text.trim_start();
        let open = text.len() - trimmed.len();
        let mut end = open + 1; // after `{`
        let mut members = Vec::with_capacity(pairs.len());
        for (key, value) in pairs {
            let start = offset(text, value.get())?;
            let between = text.get(end..start)?;
            let key_at = end + between.find('"')?;
            members.push(Member {
                key,
                prefix: text.get(end..key_at)?,
                head: text.get(key_at..start)?,
                value,
            });
            end = start + value.get().len();
        }
        let close = text.trim_end().len().checked_sub(1)?;
        Some(RawObject { members, tail: text.get(end..close)? })
    }

    /// The value of the first member with this key.
    pub fn get(&self, key: &str) -> Option<&'a RawValue> {
        self.members.iter().find(|m| m.key == key).map(|m| m.value)
    }

    /// The value of `key` as a string, or `None` when it is absent or not a string.
    pub fn str(&self, key: &str) -> Option<String> {
        self.get(key).and_then(|v| serde_json::from_str::<String>(v.get()).ok())
    }

    /// The value of `key` split as an object.
    pub fn object(&self, key: &str) -> Option<RawObject<'a>> {
        self.get(key).and_then(|v| RawObject::parse(v.get()))
    }

    /// The value of `key` split as an array.
    pub fn array(&self, key: &str) -> Option<RawArray<'a>> {
        self.get(key).and_then(|v| RawArray::parse(v.get()))
    }

    /// An edit that keeps every member.
    pub fn edit(&self) -> ObjectEdit<'_, 'a> {
        ObjectEdit {
            obj: self,
            slots: self.members.iter().map(|_| Slot::Keep).collect(),
            appended: Vec::new(),
        }
    }
}

/// One array element: the white space and comma before it, and the raw value.
pub(crate) struct Element<'a> {
    prefix: &'a str,
    pub value: &'a RawValue,
}

/// A JSON array split into its elements.
pub(crate) struct RawArray<'a> {
    pub elements: Vec<Element<'a>>,
    tail: &'a str,
}

impl<'a> RawArray<'a> {
    /// Split the raw text of one JSON array. `None` when the text is not one array.
    pub fn parse(text: &'a str) -> Option<Self> {
        let values: Vec<&'a RawValue> = serde_json::from_str(text).ok()?;
        let trimmed = text.trim_start();
        let open = text.len() - trimmed.len();
        let mut end = open + 1; // after `[`
        let mut elements = Vec::with_capacity(values.len());
        for value in values {
            let start = offset(text, value.get())?;
            elements.push(Element { prefix: text.get(end..start)?, value });
            end = start + value.get().len();
        }
        let close = text.trim_end().len().checked_sub(1)?;
        Some(RawArray {
            elements,
            tail: text.get(end..close)?,
        })
    }

    /// An edit that keeps every element.
    pub fn edit(&self) -> ArrayEdit<'_, 'a> {
        ArrayEdit {
            arr: self,
            slots: self.elements.iter().map(|_| Slot::Keep).collect(),
        }
    }
}

/// What happens to one member or element.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Slot {
    Keep,
    /// The new value, as JSON text.
    Replace(String),
    Drop,
}

/// Write `prefix` (white space and at most one comma) for an item that is first (`first`) or not.
fn push_prefix(out: &mut String, prefix: &str, first: bool) {
    match (first, prefix.contains(',')) {
        (true, true) => out.extend(prefix.chars().filter(|c| *c != ',')),
        (false, false) => {
            out.push(',');
            out.push_str(prefix);
        }
        _ => out.push_str(prefix),
    }
}

/// The edits of one object.
pub(crate) struct ObjectEdit<'o, 'a> {
    obj: &'o RawObject<'a>,
    slots: Vec<Slot>,
    /// New members, as (raw key, raw value) JSON text.
    appended: Vec<(String, String)>,
}

impl ObjectEdit<'_, '_> {
    /// Replace the value of every member with this key, or append the member when there is none.
    pub fn set(&mut self, key: &str, json: String) {
        if !self.replace(key, json.clone()) {
            self.appended.push((serde_json::Value::from(key).to_string(), json));
        }
    }

    /// Replace the value of every member with this key. `false` when there is none.
    pub fn replace(&mut self, key: &str, json: String) -> bool {
        let mut hit = false;
        for (m, s) in self.obj.members.iter().zip(&mut self.slots) {
            if m.key == key {
                *s = Slot::Replace(json.clone());
                hit = true;
            }
        }
        hit
    }

    /// Drop every member with this key.
    pub fn drop(&mut self, key: &str) {
        for (m, s) in self.obj.members.iter().zip(&mut self.slots) {
            if m.key == key {
                *s = Slot::Drop;
            }
        }
    }

    /// The edited object as JSON text.
    pub fn render(&self) -> String {
        let mut out = String::from("{");
        let mut first = true;
        for (m, s) in self.obj.members.iter().zip(&self.slots) {
            let value = match s {
                Slot::Keep => m.value.get(),
                Slot::Replace(v) => v.as_str(),
                Slot::Drop => continue,
            };
            push_prefix(&mut out, m.prefix, first);
            out.push_str(m.head);
            out.push_str(value);
            first = false;
        }
        for (k, v) in &self.appended {
            if !first {
                out.push(',');
            }
            out.push_str(k);
            out.push(':');
            out.push_str(v);
            first = false;
        }
        out.push_str(self.obj.tail);
        out.push('}');
        out
    }
}

/// The edits of one array.
pub(crate) struct ArrayEdit<'o, 'a> {
    arr: &'o RawArray<'a>,
    slots: Vec<Slot>,
}

impl ArrayEdit<'_, '_> {
    pub fn replace(&mut self, i: usize, json: String) {
        self.slots[i] = Slot::Replace(json);
    }

    pub fn drop(&mut self, i: usize) {
        self.slots[i] = Slot::Drop;
    }

    pub fn changed(&self) -> bool {
        self.slots.iter().any(|s| *s != Slot::Keep)
    }

    /// The number of elements the edit keeps.
    pub fn kept(&self) -> usize {
        self.slots.iter().filter(|s| **s != Slot::Drop).count()
    }

    /// The edited array as JSON text.
    pub fn render(&self) -> String {
        let mut out = String::from("[");
        let mut first = true;
        for (e, s) in self.arr.elements.iter().zip(&self.slots) {
            let value = match s {
                Slot::Keep => e.value.get(),
                Slot::Replace(v) => v.as_str(),
                Slot::Drop => continue,
            };
            push_prefix(&mut out, e.prefix, first);
            out.push_str(value);
            first = false;
        }
        out.push_str(self.arr.tail);
        out.push(']');
        out
    }
}

/// The byte range of the top-level value of `text`, and the value. `None` when `text` is not JSON.
pub(crate) fn top_level(text: &str) -> Option<(usize, &RawValue)> {
    let v: &RawValue = serde_json::from_str(text).ok()?;
    Some((offset(text, v.get())?, v))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ODD: &str = " {\n  \"a\" :1e400 ,\"b\\u0020c\":  [ 1 , {\"x\":12345678901234567890} ,3 ]\t,\n \"d\":\"\\\"}\" } ";

    fn obj(text: &str) -> RawObject<'_> {
        RawObject::parse(text).expect("object")
    }

    #[test]
    fn an_unchanged_edit_writes_the_same_bytes() {
        let o = obj(ODD);
        assert_eq!(o.members.iter().map(|m| m.key.as_str()).collect::<Vec<_>>(), ["a", "b c", "d"]);
        assert_eq!(o.edit().render(), ODD.trim());
        let a = o.array("b c").expect("array");
        assert_eq!(a.edit().render(), "[ 1 , {\"x\":12345678901234567890} ,3 ]");
    }

    #[test]
    fn dropping_a_member_keeps_the_others_and_their_separators() {
        let o = obj(ODD);
        let mut e = o.edit();
        e.drop("a");
        assert_eq!(e.render(), "{ \"b\\u0020c\":  [ 1 , {\"x\":12345678901234567890} ,3 ]\t,\n \"d\":\"\\\"}\" }");
        let mut e = o.edit();
        e.drop("b c");
        assert_eq!(e.render(), "{\n  \"a\" :1e400\t,\n \"d\":\"\\\"}\" }");
        let mut e = o.edit();
        e.drop("d");
        assert_eq!(e.render(), "{\n  \"a\" :1e400 ,\"b\\u0020c\":  [ 1 , {\"x\":12345678901234567890} ,3 ] }");
        let mut e = o.edit();
        for k in ["a", "b c", "d"] {
            e.drop(k);
        }
        assert_eq!(e.render(), "{ }");
    }

    #[test]
    fn replace_and_append() {
        let o = obj(ODD);
        let mut e = o.edit();
        assert!(e.replace("a", "2".to_owned()));
        assert!(!e.replace("zz", "2".to_owned()));
        e.set("new", "true".to_owned());
        assert_eq!(e.render(), "{\n  \"a\" :2 ,\"b\\u0020c\":  [ 1 , {\"x\":12345678901234567890} ,3 ]\t,\n \"d\":\"\\\"}\",\"new\":true }");
        let empty = obj("{}");
        let mut e = empty.edit();
        e.set("k", "1".to_owned());
        assert_eq!(e.render(), "{\"k\":1}");
    }

    #[test]
    fn array_edits() {
        let a = RawArray::parse("[ 1 , {\"x\":2} ,3 ]").expect("array");
        let mut e = a.edit();
        e.drop(0);
        assert_eq!(e.render(), "[  {\"x\":2} ,3 ]");
        assert_eq!(e.kept(), 2);
        let mut e = a.edit();
        e.drop(1);
        e.replace(2, "\"z\"".to_owned());
        assert_eq!(e.render(), "[ 1 ,\"z\" ]");
        let mut e = a.edit();
        (0..3).for_each(|i| e.drop(i));
        assert_eq!((e.render().as_str(), e.kept()), ("[ ]", 0));
        assert!(RawArray::parse("[]").expect("empty").elements.is_empty());
    }

    #[test]
    fn a_non_container_does_not_split() {
        assert!(RawObject::parse("[1]").is_none());
        assert!(RawArray::parse("{}").is_none());
        assert!(RawObject::parse("{\"a\":").is_none());
        let (at, v) = top_level("  {\"a\":1}\n").expect("top");
        assert_eq!((at, v.get()), (2, "{\"a\":1}"));
    }
}
