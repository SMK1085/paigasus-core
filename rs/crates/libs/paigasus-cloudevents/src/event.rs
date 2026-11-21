// SPDX-License-Identifier: Apache-2.0

use jiff::Timestamp;

use crate::SchemaError;

/// The maximum length of `id`, in characters.
pub const ID_MAX_CHARS: usize = 256;
/// The maximum length of `type`, in characters.
pub const TYPE_MAX_CHARS: usize = 256;
/// The maximum length of `source`, `subject` and `dataschema`, in characters.
pub const URI_MAX_CHARS: usize = 1024;
/// The maximum length of a string extension value, in characters.
pub const EXTENSION_STRING_MAX_CHARS: usize = 1024;
/// The maximum length of an extension name, in characters.
pub const EXTENSION_NAME_MAX_CHARS: usize = 20;

/// The names of the core attributes. An extension cannot use one of them.
pub(crate) const RESERVED: [&str; 9] = ["specversion", "id", "source", "type", "subject", "time", "datacontenttype", "dataschema", "data"];

/// The value of an extension attribute. CloudEvents JSON allows these three types here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtensionValue {
    String(String),
    Integer(i64),
    Boolean(bool),
}

/// One CloudEvents 1.0 event. Build it with [`CloudEvent::new`] or read it with [`crate::parse`].
#[derive(Debug, Clone, PartialEq)]
pub struct CloudEvent {
    pub(crate) id: String,
    pub(crate) source: String,
    pub(crate) ty: String,
    pub(crate) time: Option<Timestamp>,
    pub(crate) subject: Option<String>,
    pub(crate) datacontenttype: Option<String>,
    pub(crate) dataschema: Option<String>,
    pub(crate) data: Option<serde_json::Value>,
    pub(crate) extensions: Vec<(String, ExtensionValue)>,
}

impl CloudEvent {
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }
    #[must_use]
    pub fn ty(&self) -> &str {
        &self.ty
    }
    #[must_use]
    pub fn time(&self) -> Option<Timestamp> {
        self.time
    }
    #[must_use]
    pub fn subject(&self) -> Option<&str> {
        self.subject.as_deref()
    }
    #[must_use]
    pub fn datacontenttype(&self) -> Option<&str> {
        self.datacontenttype.as_deref()
    }
    #[must_use]
    pub fn dataschema(&self) -> Option<&str> {
        self.dataschema.as_deref()
    }
    #[must_use]
    pub fn data(&self) -> Option<&serde_json::Value> {
        self.data.as_ref()
    }
    #[must_use]
    pub fn extension(&self, name: &str) -> Option<&ExtensionValue> {
        self.extensions.iter().find(|(n, _)| n == name).map(|(_, v)| v)
    }
    /// The `schemaversion` extension, when it is an integer.
    #[must_use]
    pub fn schemaversion(&self) -> Option<i64> {
        match self.extension("schemaversion") {
            Some(ExtensionValue::Integer(i)) => Some(*i),
            _ => None,
        }
    }
    #[must_use]
    pub fn actorprn(&self) -> Option<&str> {
        self.string_extension("actorprn")
    }
    #[must_use]
    pub fn correlationid(&self) -> Option<&str> {
        self.string_extension("correlationid")
    }
    #[must_use]
    pub fn threadid(&self) -> Option<&str> {
        self.string_extension("threadid")
    }

    fn string_extension(&self, name: &str) -> Option<&str> {
        match self.extension(name) {
            Some(ExtensionValue::String(s)) => Some(s),
            _ => None,
        }
    }
}

/// Fail when `value` has more than `max` characters (not bytes), or when it has a character that
/// a CloudEvents `String` does not allow.
///
/// CloudEvents 1.0.2 (type system, `String`) does not allow the control characters U+0000 to
/// U+001F and U+007F to U+009F, and the Unicode noncharacters. A Rust `str` cannot hold a
/// surrogate, so that third rule needs no check. This applies to every context attribute and to
/// every string extension, not to `data`. A NUL also cannot be stored in a Postgres `text` or
/// `jsonb` value, so without this check such an event fails at the insert on every retry.
pub(crate) fn check_len(attribute: &str, value: &str, max: usize) -> Result<(), SchemaError> {
    if value.chars().count() > max {
        return Err(SchemaError::TooLong {
            attribute: attribute.to_string(),
            max,
        });
    }
    if value.chars().any(|c| c.is_control() || is_noncharacter(c)) {
        return Err(SchemaError::InvalidCharacter(attribute.to_string()));
    }
    Ok(())
}

/// A Unicode noncharacter: U+FDD0 to U+FDEF, and the last two code points of each plane.
fn is_noncharacter(c: char) -> bool {
    let n = u32::from(c);
    (0xFDD0..=0xFDEF).contains(&n) || (n & 0xFFFE) == 0xFFFE
}

/// Fail unless `value` is an absolute URI (RFC 3986 section 4.3: a scheme, then `:`). The rest
/// may use only the characters that RFC 3986 allows or a non-ASCII character (as in an IRI), and
/// each `%` starts a valid escape.
/// CloudEvents types `dataschema` as `URI`, not `URI-reference`, so a relative value is refused.
/// `source` is a `URI-reference` and does not use this check.
pub(crate) fn check_absolute_uri(name: &'static str, value: &str) -> Result<(), SchemaError> {
    let err = || SchemaError::NotAbsoluteUri(name);
    let (scheme, rest) = value.split_once(':').ok_or_else(err)?;
    let mut scheme_bytes = scheme.bytes();
    let scheme_ok = scheme_bytes.next().is_some_and(|b| b.is_ascii_alphabetic()) && scheme_bytes.all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'));
    if !scheme_ok || rest.is_empty() {
        return Err(err());
    }
    let b = rest.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' => {
                if !(i + 2 < b.len() && b[i + 1].is_ascii_hexdigit() && b[i + 2].is_ascii_hexdigit()) {
                    return Err(err());
                }
                i += 3;
                continue;
            }
            // A non-ASCII character: allowed as in an IRI (RFC 3987). `check_len` already refused
            // the control characters and the noncharacters.
            c if c.is_ascii_alphanumeric() || !c.is_ascii() => {}
            b'-' | b'.' | b'_' | b'~' | b':' | b'/' | b'?' | b'#' | b'[' | b']' | b'@' | b'!' | b'$' | b'&' | b'\'' | b'(' | b')' | b'*' | b'+' | b',' | b';' | b'=' => {}
            _ => return Err(err()),
        }
        i += 1;
    }
    Ok(())
}

/// Fail when a required text attribute is empty or too long.
pub(crate) fn check_text(name: &'static str, value: &str, max: usize) -> Result<(), SchemaError> {
    if value.is_empty() {
        return Err(SchemaError::Empty(name));
    }
    check_len(name, value, max)
}

/// An extension name is 1 to 20 characters of `a-z` and `0-9`, and is not a core attribute.
pub(crate) fn check_extension_name(name: &str) -> Result<(), SchemaError> {
    let valid = !name.is_empty() && name.len() <= EXTENSION_NAME_MAX_CHARS && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()) && !RESERVED.contains(&name);
    if valid { Ok(()) } else { Err(SchemaError::InvalidExtensionName(name.to_string())) }
}

pub(crate) fn check_extension_value(name: &str, value: &ExtensionValue) -> Result<(), SchemaError> {
    if let ExtensionValue::String(s) = value {
        check_len(name, s, EXTENSION_STRING_MAX_CHARS)?;
    }
    Ok(())
}
