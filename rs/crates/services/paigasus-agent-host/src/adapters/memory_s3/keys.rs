// SPDX-License-Identifier: Apache-2.0

//! The object keys of the S3 memory store (spec §6.1 and §6.2).
//!
//! A file key is `<prefix>/<org>/<agent>/<principal><memory path>`. The three ids are
//! hyphenated lower-case UUIDs. The key parts are valid by construction, so no part is
//! percent-encoded.

use object_store::path::Path;
use paigasus_agent_host_core::memory::{MemoryDir, MemoryPath, MemoryScope};
use uuid::Uuid;

/// The most bytes in a key prefix.
const MAX_PREFIX_BYTES: usize = 128;
/// The most segments in a key prefix.
const MAX_PREFIX_SEGMENTS: usize = 8;
/// The most bytes in one segment of a key prefix.
const MAX_SEGMENT_BYTES: usize = 64;
/// The segment under the prefix that holds the probe objects. It is not a UUID, so no scope
/// key can start with it.
const PROBE_SEGMENT: &str = "_probe";

/// The key prefix that the operator sets. It is valid by construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyPrefix(String);

/// The reason why a key prefix is not valid.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PrefixError {
    /// The prefix is empty.
    #[error("the key prefix is empty")]
    Empty,
    /// The prefix has more than 128 bytes.
    #[error("the key prefix has {bytes} bytes. The maximum is 128 bytes")]
    TooLong {
        /// The size of the prefix.
        bytes: usize,
    },
    /// The prefix has more than 8 segments.
    #[error("the key prefix has {segments} segments. The maximum is 8 segments")]
    TooManySegments {
        /// The number of segments.
        segments: usize,
    },
    /// The prefix has two `/` characters together.
    #[error("the key prefix has an empty segment")]
    EmptySegment,
    /// The prefix starts or ends with `/`.
    #[error("the key prefix must not start or end with /")]
    LeadingOrTrailingSlash,
    /// A segment has a character or a length that is not allowed.
    #[error("each segment of the key prefix has 1 to 64 characters from A-Z a-z 0-9 . _ - and does not start with .")]
    BadSegment,
}

impl KeyPrefix {
    /// Parse a key prefix. The length is checked first, then the slashes, then the segments.
    pub fn parse(raw: &str) -> Result<KeyPrefix, PrefixError> {
        if raw.is_empty() {
            return Err(PrefixError::Empty);
        }
        if raw.len() > MAX_PREFIX_BYTES {
            return Err(PrefixError::TooLong { bytes: raw.len() });
        }
        if raw.starts_with('/') || raw.ends_with('/') {
            return Err(PrefixError::LeadingOrTrailingSlash);
        }
        let segments = raw.split('/').count();
        if segments > MAX_PREFIX_SEGMENTS {
            return Err(PrefixError::TooManySegments { segments });
        }
        for segment in raw.split('/') {
            if segment.is_empty() {
                return Err(PrefixError::EmptySegment);
            }
            let chars_ok = segment.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
            if segment.len() > MAX_SEGMENT_BYTES || segment.starts_with('.') || !chars_ok {
                return Err(PrefixError::BadSegment);
            }
        }
        Ok(KeyPrefix(raw.to_string()))
    }

    /// The prefix as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Build a key from a string that is valid by construction.
fn key(text: &str) -> Path {
    Path::parse(text).expect("a key part that is valid by construction")
}

/// The key base of one scope: `<prefix>/<org>/<agent>/<principal>`.
#[must_use]
pub fn scope_path(prefix: &KeyPrefix, scope: &MemoryScope) -> Path {
    key(&format!("{}/{}/{}/{}", prefix.as_str(), scope.org(), scope.agent().uuid(), scope.principal().uuid()))
}

/// The key of one file of a scope.
#[must_use]
pub fn file_path(prefix: &KeyPrefix, scope: &MemoryScope, path: &MemoryPath) -> Path {
    key(&format!("{}{}", scope_path(prefix, scope).as_ref(), path.as_str()))
}

/// The key base of one directory of a scope. The root directory gives the scope path.
#[must_use]
pub fn dir_path(prefix: &KeyPrefix, scope: &MemoryScope, dir: &MemoryDir) -> Path {
    if dir.is_root() {
        scope_path(prefix, scope)
    } else {
        key(&format!("{}{}", scope_path(prefix, scope).as_ref(), dir.as_str()))
    }
}

/// The key of one probe object: `<prefix>/_probe/<uuid>`.
#[must_use]
pub fn probe_path(prefix: &KeyPrefix, id: Uuid) -> Path {
    key(&format!("{}/{PROBE_SEGMENT}/{id}", prefix.as_str()))
}

/// The key base of the probe objects: `<prefix>/_probe`.
#[must_use]
pub fn probe_dir(prefix: &KeyPrefix) -> Path {
    key(&format!("{}/{PROBE_SEGMENT}", prefix.as_str()))
}

/// A listed key is not under the base of the listing. The store broke its contract (AG6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutsideBase;

/// The result of the read of one listed key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListedKey {
    /// The key is a memory file.
    File(MemoryPath),
    /// The key is under the base, but the rest is not a valid memory path.
    Refused,
}

/// Read a listed key against the base of the listing. The base is a scope path or a directory
/// path.
pub fn listed_key(base: &Path, key: &Path) -> Result<ListedKey, OutsideBase> {
    let rest = key.as_ref().strip_prefix(base.as_ref()).and_then(|rest| rest.strip_prefix('/')).ok_or(OutsideBase)?;
    match MemoryPath::parse(&format!("/{rest}")) {
        Ok(path) => Ok(ListedKey::File(path)),
        Err(_) => Ok(ListedKey::Refused),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paigasus_agent_host_core::ids::{AgentId, PrincipalId};
    use paigasus_agent_host_core::memory::{MemoryDir, MemoryPath, MemoryScope};
    use uuid::Uuid;

    fn prefix(raw: &str) -> KeyPrefix {
        KeyPrefix::parse(raw).expect("a valid prefix")
    }

    fn scope() -> MemoryScope {
        let org = Uuid::from_u128(1);
        MemoryScope::new(AgentId::new(org, Uuid::from_u128(2)), PrincipalId::from_uuid(Uuid::from_u128(3)))
    }

    fn scope_text() -> String {
        format!("{}/{}/{}", Uuid::from_u128(1), Uuid::from_u128(2), Uuid::from_u128(3))
    }

    #[test]
    fn a_valid_prefix_parses() {
        let eight = "a/b/c/d/e/f/g/h";
        let long = format!("{}/{}", "a".repeat(63), "b".repeat(64));
        assert_eq!(long.len(), 128);
        for raw in ["mem", "a/b.c/d-e_f", eight, long.as_str()] {
            assert_eq!(KeyPrefix::parse(raw).expect(raw).as_str(), raw);
        }
    }

    #[test]
    fn the_prefix_table_refuses() {
        let nine = "a/b/c/d/e/f/g/h/i";
        let too_long = "a".repeat(129);
        let long_segment = "a".repeat(65);
        let rows: Vec<(&str, PrefixError)> = vec![
            ("", PrefixError::Empty),
            ("..", PrefixError::BadSegment),
            (".x", PrefixError::BadSegment),
            ("a/.b", PrefixError::BadSegment),
            ("/a", PrefixError::LeadingOrTrailingSlash),
            ("a/", PrefixError::LeadingOrTrailingSlash),
            ("a//b", PrefixError::EmptySegment),
            ("%2e", PrefixError::BadSegment),
            ("a b", PrefixError::BadSegment),
            (too_long.as_str(), PrefixError::TooLong { bytes: 129 }),
            (nine, PrefixError::TooManySegments { segments: 9 }),
            (long_segment.as_str(), PrefixError::BadSegment),
        ];
        for (raw, want) in rows {
            assert_eq!(KeyPrefix::parse(raw), Err(want), "prefix {raw:?}");
        }
    }

    #[test]
    fn a_file_key_round_trips() {
        let mem = prefix("mem");
        let file = MemoryPath::parse("/tasks/t1.md").unwrap();
        let key = file_path(&mem, &scope(), &file);
        assert_eq!(key.as_ref(), format!("mem/{}/tasks/t1.md", scope_text()));
        let base = scope_path(&mem, &scope());
        assert_eq!(base.as_ref(), format!("mem/{}", scope_text()));
        assert_eq!(listed_key(&base, &key), Ok(ListedKey::File(file)));
    }

    #[test]
    fn the_root_dir_is_the_scope_path() {
        let mem = prefix("mem");
        assert_eq!(dir_path(&mem, &scope(), &MemoryDir::root()), scope_path(&mem, &scope()));
        let dir = MemoryDir::parse("/tasks").unwrap();
        assert_eq!(dir_path(&mem, &scope(), &dir).as_ref(), format!("mem/{}/tasks", scope_text()));
    }

    #[test]
    fn a_key_outside_the_base_is_a_fault() {
        let base = Path::parse("mem/s/a").unwrap();
        let other = Path::parse("mem/s/ab/x").unwrap();
        assert_eq!(listed_key(&base, &other), Err(OutsideBase));
        assert_eq!(listed_key(&base, &base), Err(OutsideBase));
    }

    #[test]
    fn a_key_that_memory_path_refuses_is_refused() {
        let base = Path::parse("mem/s/a").unwrap();
        let long = Path::parse(format!("mem/s/a/{}", "x".repeat(65))).unwrap();
        let tilde = Path::parse("mem/s/a/~").unwrap();
        assert_eq!(listed_key(&base, &long), Ok(ListedKey::Refused));
        assert_eq!(listed_key(&base, &tilde), Ok(ListedKey::Refused));
    }

    #[test]
    fn the_probe_key_is_not_a_scope_key() {
        let mem = prefix("mem");
        let id = Uuid::from_u128(9);
        assert!(probe_path(&mem, id).as_ref().starts_with("mem/_probe/"));
        assert_eq!(probe_dir(&mem).as_ref(), "mem/_probe");
        assert!(Uuid::parse_str("_probe").is_err());
        assert!(!scope_path(&mem, &scope()).as_ref().starts_with("mem/_probe/"));
    }

    #[test]
    fn the_longest_key_is_under_the_s3_limit() {
        let long = format!("{}/{}", "a".repeat(63), "b".repeat(64));
        let mem = prefix(&long);
        // 256 bytes in 8 segments: eight segments of 31 bytes, each with a leading "/".
        let seg = "s".repeat(31);
        let raw = format!("/{}", [seg.as_str(); 8].join("/"));
        assert_eq!(raw.len(), 8 * 32);
        let file = MemoryPath::parse(&raw).unwrap();
        let key = file_path(&mem, &scope(), &file);
        assert!(key.as_ref().len() < 1024, "key has {} bytes", key.as_ref().len());
    }
}
