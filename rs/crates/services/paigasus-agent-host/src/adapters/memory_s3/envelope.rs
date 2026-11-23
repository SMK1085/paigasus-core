// SPDX-License-Identifier: Apache-2.0

//! The write-ID envelope of a memory object (spec §6.3).
//!
//! An object holds a header and then the file content. The header is the `MAGIC` text, a
//! write ID of 32 characters and one newline character. The write ID is new for each write.
//! So two writes of the same content give two different objects, and a writer can read an
//! object back to find out if its own write won.

use uuid::Uuid;

/// The size of the header in bytes.
pub const HEADER_LEN: u64 = 51;
/// The first bytes of each object.
pub const MAGIC: &str = "paigasus-memory/1 ";

/// The length of a write ID in characters.
const ID_LEN: usize = 32;

const _: () = assert!(MAGIC.len() + ID_LEN + 1 == HEADER_LEN as usize);

/// The ID of one write. It is a UUID v4 in the simple form, in lower-case hex.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteId(String);

impl WriteId {
    /// Make a new random write ID.
    pub fn random() -> WriteId {
        WriteId(Uuid::new_v4().simple().to_string())
    }

    /// The ID as text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The reason why an object is not a valid envelope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvelopeError {
    /// The object has less than 51 bytes.
    Short,
    /// The object does not start with `MAGIC`.
    Magic,
    /// The write ID is not 32 characters of lower-case hex.
    Id,
    /// The byte after the write ID is not a newline.
    Newline,
    /// The content is not valid UTF-8.
    Utf8,
}

/// Make the bytes of an object: `MAGIC`, the ID, a newline and the content.
pub fn encode(id: &WriteId, content: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN as usize + content.len());
    out.extend_from_slice(MAGIC.as_bytes());
    out.extend_from_slice(id.as_str().as_bytes());
    out.push(b'\n');
    out.extend_from_slice(content.as_bytes());
    out
}

/// Read the write ID and the content from the bytes of an object.
pub fn decode(bytes: &[u8]) -> Result<(WriteId, String), EnvelopeError> {
    if bytes.len() < HEADER_LEN as usize {
        return Err(EnvelopeError::Short);
    }
    let (magic, rest) = bytes.split_at(MAGIC.len());
    if magic != MAGIC.as_bytes() {
        return Err(EnvelopeError::Magic);
    }
    let (id, rest) = rest.split_at(ID_LEN);
    if !id.iter().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return Err(EnvelopeError::Id);
    }
    let (newline, content) = rest.split_at(1);
    if newline != b"\n" {
        return Err(EnvelopeError::Newline);
    }
    let id = std::str::from_utf8(id).map_err(|_| EnvelopeError::Id)?;
    let content = std::str::from_utf8(content).map_err(|_| EnvelopeError::Utf8)?;
    Ok((WriteId(id.to_owned()), content.to_owned()))
}

/// The size of the content, from the size of the object. It is `None` when the object is
/// smaller than the header.
pub fn content_size(object_size: u64) -> Option<u64> {
    object_size.checked_sub(HEADER_LEN)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_header_is_51_bytes() {
        assert_eq!(HEADER_LEN, 51);
        assert_eq!(MAGIC.len(), 18);
        let id = WriteId::random();
        assert_eq!(id.as_str().len(), 32);
        assert_eq!(encode(&id, "").len(), 51);
    }

    #[test]
    fn encode_then_decode() {
        for content in ["", "a", "über\n"] {
            let id = WriteId::random();
            let bytes = encode(&id, content);
            let (got_id, got_content) = decode(&bytes).unwrap();
            assert_eq!(got_id, id);
            assert_eq!(got_content, content);
            assert_eq!(content_size(bytes.len() as u64), Some(content.len() as u64));
        }
    }

    #[test]
    fn two_writes_of_one_content_differ() {
        assert_ne!(encode(&WriteId::random(), "same"), encode(&WriteId::random(), "same"));
    }

    #[test]
    fn decode_refuses() {
        let id = "0123456789abcdef0123456789abcdef";
        assert_eq!(decode(&[b'a'; 50]), Err(EnvelopeError::Short));
        let bad_magic = format!("paigasus-memory/2 {id}\n");
        assert_eq!(decode(bad_magic.as_bytes()), Err(EnvelopeError::Magic));
        let bad_g = format!("{MAGIC}g123456789abcdef0123456789abcdef\n");
        assert_eq!(decode(bad_g.as_bytes()), Err(EnvelopeError::Id));
        let bad_upper = format!("{MAGIC}0123456789ABCDEF0123456789abcdef\n");
        assert_eq!(decode(bad_upper.as_bytes()), Err(EnvelopeError::Id));
        let bad_newline = format!("{MAGIC}{id}x");
        assert_eq!(decode(bad_newline.as_bytes()), Err(EnvelopeError::Newline));
        let mut bad_utf8 = format!("{MAGIC}{id}\n").into_bytes();
        bad_utf8.push(0xff);
        assert_eq!(decode(&bad_utf8), Err(EnvelopeError::Utf8));
    }

    #[test]
    fn a_short_object_has_no_content_size() {
        assert_eq!(content_size(50), None);
        assert_eq!(content_size(51), Some(0));
    }
}
