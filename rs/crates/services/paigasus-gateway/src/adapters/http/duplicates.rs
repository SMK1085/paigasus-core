// SPDX-License-Identifier: Apache-2.0

//! The duplicate keys of one request body (SMA-769 fix round 1, SMA-750 D19).
//!
//! serde_json keeps the last of two duplicate keys, and an upstream may keep the first. So a check
//! that reads the parsed value can see a different value than the upstream does. The routes that
//! decide on a field (`background` on `/v1/responses`, `stream` on `/v1/messages`) refuse such a
//! body. Keys compare after JSON unescaping (`"a"` and `"\u0061"` are one key). The walk holds one
//! key set per open object and no values.

use std::collections::HashSet;

/// The duplicate keys of one request body.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Duplicates {
    /// Some object, at any depth, has a key twice.
    pub any: bool,
    /// The keys that the top-level object has more than once.
    pub top: HashSet<String>,
}

/// Find the duplicate keys of `body`. `None` when the body is not JSON; the callers have already
/// parsed it, so that does not occur.
pub(crate) fn duplicates(body: &[u8]) -> Option<Duplicates> {
    let mut found = Duplicates::default();
    let mut de = serde_json::Deserializer::from_slice(body);
    serde::de::DeserializeSeed::deserialize(Walk { found: &mut found, top: true }, &mut de).ok()?;
    Some(found)
}

/// One value of the [`duplicates`] walk. `top` is true for the document's own value only.
struct Walk<'a> {
    found: &'a mut Duplicates,
    top: bool,
}

impl<'de> serde::de::DeserializeSeed<'de> for Walk<'_> {
    type Value = ();
    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        d.deserialize_any(self)
    }
}

impl<'de> serde::de::Visitor<'de> for Walk<'_> {
    type Value = ();
    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a JSON value")
    }
    fn visit_bool<E>(self, _: bool) -> Result<(), E> {
        Ok(())
    }
    fn visit_i64<E>(self, _: i64) -> Result<(), E> {
        Ok(())
    }
    fn visit_u64<E>(self, _: u64) -> Result<(), E> {
        Ok(())
    }
    fn visit_f64<E>(self, _: f64) -> Result<(), E> {
        Ok(())
    }
    fn visit_str<E>(self, _: &str) -> Result<(), E> {
        Ok(())
    }
    fn visit_unit<E>(self) -> Result<(), E> {
        Ok(())
    }
    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        while seq.next_element_seed(Walk { found: &mut *self.found, top: false })?.is_some() {}
        Ok(())
    }
    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        let mut seen = HashSet::new();
        while let Some(key) = map.next_key::<String>()? {
            map.next_value_seed(Walk { found: &mut *self.found, top: false })?;
            if seen.contains(&key) {
                self.found.any = true;
                if self.top {
                    self.found.top.insert(key);
                }
            } else {
                seen.insert(key);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn top(raw: &str) -> Vec<String> {
        let mut keys: Vec<String> = duplicates(raw.as_bytes()).expect("JSON").top.into_iter().collect();
        keys.sort();
        keys
    }

    /// The top set holds only the top-level keys that repeat; `any` sees every depth.
    #[test]
    fn the_walk_splits_top_level_and_nested_duplicates() {
        assert_eq!(top(r#"{"a":1,"b":2,"a":3,"b":4,"c":5}"#), ["a", "b"]);
        let nested = duplicates(br#"{"a":{"k":1,"k":2},"b":[{"x":1,"x":2}]}"#).expect("JSON");
        assert!(nested.any);
        assert!(nested.top.is_empty(), "a nested duplicate is not a top-level one");
        assert_eq!(duplicates(br#"{"a":1,"A":2}"#).expect("JSON"), Duplicates::default());
        assert_eq!(top(r#"{"a":1,"\u0061":2}"#), ["a"], "keys compare after unescaping");
        assert!(duplicates(b"{not json").is_none());
    }
}
