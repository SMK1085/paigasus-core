// SPDX-License-Identifier: Apache-2.0
//! The one typed walk over the arguments of a tool call (§ 6.1.4). `map_input` builds
//! `context.input` from it. `history::extract_fields` builds the stored fields from it. Because
//! both read the same walk, the stored fields and `context.input` cannot differ.

use serde_json::value::RawValue;

use super::ToolInput;
use crate::detect::input::{NumberClass, Unparsed, classify_number, elements, first_byte, nesting_ok, strict_entries};

/// A value nested deeper than this becomes a string of its raw JSON text (§ 6.1.4).
const MAX_DEPTH: usize = 16;

/// One argument value, typed as `context.input` types it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Arg {
    Record(Vec<(String, Arg)>),
    Set(Vec<Arg>),
    Str(String),
    Bool(bool),
    Long(i64),
    /// The kernel decimal text, for example `1.5` or `-0.0005`.
    Decimal(String),
    /// A number that `Long` and `decimal` cannot hold. It keeps its text.
    TextNumber(String),
    /// A value below depth 16. It is the raw JSON text.
    RawText(String),
}

/// The result of the walk over the arguments of one call.
pub(crate) struct Parsed {
    pub args: Option<Arg>,
    pub input_text: Option<String>,
    pub unparsed: bool,
}

/// `Ok(None)` is a JSON null: the key is left out.
type Walk = Result<Option<Arg>, Unparsed>;

/// Check a container that is too deep to map: a duplicate key at any depth still makes the
/// arguments unparsed.
fn validate(raw: &RawValue) -> Result<(), Unparsed> {
    let text = raw.get();
    match first_byte(text) {
        b'{' => strict_entries(text)?.into_iter().try_for_each(|(_, v)| validate(v)),
        b'[' => elements(text)?.into_iter().try_for_each(validate),
        _ => Ok(()),
    }
}

fn walk(raw: &RawValue, depth: usize) -> Walk {
    let text = raw.get();
    let first = first_byte(text);
    // A null member is left out at every depth. Any other value deeper than MAX_DEPTH, a scalar
    // too, becomes its raw JSON text.
    if depth > MAX_DEPTH && first != b'n' {
        validate(raw)?;
        return Ok(Some(Arg::RawText(text.to_owned())));
    }
    match first {
        b'{' => {
            let mut fields = Vec::new();
            for (key, value) in strict_entries(text)? {
                if let Some(arg) = walk(value, depth + 1)? {
                    fields.push((key, arg));
                }
            }
            Ok(Some(Arg::Record(fields)))
        }
        b'[' => {
            let mut items = Vec::new();
            for value in elements(text)? {
                if let Some(arg) = walk(value, depth + 1)? {
                    items.push(arg);
                }
            }
            Ok(Some(Arg::Set(items)))
        }
        b'"' => serde_json::from_str::<String>(text).map(|s| Some(Arg::Str(s))).map_err(|_| Unparsed),
        b't' => Ok(Some(Arg::Bool(true))),
        b'f' => Ok(Some(Arg::Bool(false))),
        b'n' => Ok(None),
        _ => Ok(Some(match classify_number(text) {
            NumberClass::Long(n) => Arg::Long(n),
            NumberClass::Decimal(d) => Arg::Decimal(d),
            NumberClass::Text(t) => Arg::TextNumber(t),
        })),
    }
}

fn parse_object(raw: &str) -> Option<Arg> {
    let value: &RawValue = serde_json::from_str(raw).ok()?;
    if first_byte(value.get()) != b'{' || !nesting_ok(value.get()) {
        return None;
    }
    walk(value, 1).ok().flatten()
}

/// Walk the arguments of one call.
///
/// Anything but one strict JSON object (invalid text, another JSON type, a duplicate key at any
/// depth, or free text) gives no `args` and `unparsed`. `input_text` is set only for free text.
pub(crate) fn parse_arguments(input: &ToolInput) -> Parsed {
    match input {
        ToolInput::Text(text) => Parsed {
            args: None,
            input_text: Some(text.clone()),
            unparsed: true,
        },
        ToolInput::Json(raw) => match parse_object(raw) {
            Some(args) => Parsed {
                args: Some(args),
                input_text: None,
                unparsed: false,
            },
            None => Parsed {
                args: None,
                input_text: None,
                unparsed: true,
            },
        },
    }
}

/// Whether the arguments of a call pass the strict parse: one JSON object with no duplicate key at
/// any depth. It is the exact opposite of the `unparsed` flag of the walk. A webhook gets the
/// arguments byte for byte only when this is true (SMA-856).
#[must_use]
pub fn parses_strictly(input: &ToolInput) -> bool {
    !parse_arguments(input).unparsed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_strictly_follows_the_unparsed_rule() {
        let json = |s: &str| ToolInput::Json(s.to_owned());
        assert!(parses_strictly(&json(r#"{"a":1}"#)));
        assert!(!parses_strictly(&json(r#"{"a":1,"a":2}"#)));
        assert!(!parses_strictly(&json("[1]")));
        assert!(!parses_strictly(&json("not json")));
        assert!(!parses_strictly(&ToolInput::Text(r#"{"a":1}"#.to_owned())));
        // A big integer is kept as text, so it still parses strictly (SMA-856 P1).
        assert!(parses_strictly(&json(r#"{"n":1234567890123456789012345}"#)));
    }
}
