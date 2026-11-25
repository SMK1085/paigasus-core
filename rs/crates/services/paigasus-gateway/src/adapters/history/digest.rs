// SPDX-License-Identifier: Apache-2.0

//! The keyed digest of a history field value (SMA-753 spec P3). The digest is
//! `HMAC-SHA-256(key, org || 0x00 || path || 0x00 || tag || value)` cut to 16 bytes. A key has a
//! public id, the first 8 bytes of `HMAC-SHA-256(key, "paigasus-history-kid")`. The gateway can
//! hold a current key and a previous key, so a key can rotate without a gap.

use std::fmt;

use hmac::{Hmac, KeyInit, Mac};
use paigasus_kernel::tool_policy::history::{Digest, FieldPath, HistoryDigester, KeyId, Scalar};
use secrecy::{ExposeSecret, SecretString};
use sha2::Sha256;
use uuid::Uuid;

use crate::config::ToolHistoryConfig;

/// The least key length in bytes (the SHA-256 output size).
const MIN_KEY_BYTES: usize = 32;
/// The text that the key id is the HMAC of.
const KEY_ID_LABEL: &[u8] = b"paigasus-history-kid";

/// A digest key and its public id.
pub struct HistoryKey {
    id: KeyId,
    key: SecretString,
}

impl fmt::Debug for HistoryKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HistoryKey").field("id", &self.id.to_hex()).finish_non_exhaustive()
    }
}

impl HistoryKey {
    /// # Errors
    /// The key is shorter than 32 bytes.
    pub fn new(key: SecretString) -> Result<Self, String> {
        if key.expose_secret().len() < MIN_KEY_BYTES {
            return Err(format!("the key must hold at least {MIN_KEY_BYTES} bytes"));
        }
        let full = mac(key.expose_secret().as_bytes(), &[KEY_ID_LABEL]);
        let mut id = [0u8; 8];
        id.copy_from_slice(&full[..8]);
        Ok(Self { id: KeyId::new(id), key })
    }

    #[must_use]
    pub fn id(&self) -> KeyId {
        self.id
    }

    fn digest(&self, org: Uuid, path: &FieldPath, value: &Scalar) -> (KeyId, Digest) {
        let org = org.hyphenated().to_string();
        let path = path.joined();
        let value = value.canonical_bytes();
        let full = mac(self.key.expose_secret().as_bytes(), &[org.as_bytes(), &[0], path.as_bytes(), &[0], &value]);
        let mut out = [0u8; 16];
        out.copy_from_slice(&full[..16]);
        (self.id, Digest::new(out))
    }
}

fn mac(key: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(key).expect("HMAC accepts a key of any length");
    for part in parts {
        mac.update(part);
    }
    mac.finalize().into_bytes().into()
}

/// The digester of the gateway: a current key, an optional previous key, or no key.
#[derive(Debug)]
pub struct HmacHistoryDigester {
    current: Option<HistoryKey>,
    previous: Option<HistoryKey>,
}

impl HmacHistoryDigester {
    /// # Errors
    /// A previous key without a current key, or two keys with the same id.
    pub fn new(current: Option<HistoryKey>, previous: Option<HistoryKey>) -> Result<Self, String> {
        match (&current, &previous) {
            (None, Some(_)) => Err("a previous key needs a current key".to_owned()),
            (Some(c), Some(p)) if c.id() == p.id() => Err("the previous key has the same id as the current key".to_owned()),
            _ => Ok(Self { current, previous }),
        }
    }

    /// A digester with no key: it makes no digest.
    #[must_use]
    pub fn keyless() -> Self {
        Self { current: None, previous: None }
    }

    /// # Errors
    /// A message that names the key of the first invalid value.
    pub fn from_config(c: &ToolHistoryConfig) -> Result<Self, String> {
        let current = key_of(c.digest_key.as_ref(), "tool_history.digest_key")?;
        let previous = key_of(c.previous_digest_key.as_ref(), "tool_history.previous_digest_key")?;
        Self::new(current, previous).map_err(|e| format!("tool_history.previous_digest_key: {e}"))
    }
}

fn key_of(secret: Option<&SecretString>, name: &str) -> Result<Option<HistoryKey>, String> {
    secret.map(|s| HistoryKey::new(s.clone()).map_err(|e| format!("{name}: {e}"))).transpose()
}

impl HistoryDigester for HmacHistoryDigester {
    fn current(&self, org: Uuid, path: &FieldPath, value: &Scalar) -> Option<(KeyId, Digest)> {
        self.current.as_ref().map(|k| k.digest(org, path, value))
    }

    fn all(&self, org: Uuid, path: &FieldPath, value: &Scalar) -> Vec<(KeyId, Digest)> {
        [&self.current, &self.previous].into_iter().flatten().map(|k| k.digest(org, path, value)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const K1: &str = "0123456789abcdef0123456789abcdef-one";
    const K2: &str = "0123456789abcdef0123456789abcdef-two";

    fn key(text: &str) -> HistoryKey {
        HistoryKey::new(SecretString::from(text.to_owned())).expect("a valid key")
    }

    fn path(s: &[&str]) -> FieldPath {
        FieldPath::new(s.iter().map(|x| (*x).to_owned()).collect()).unwrap()
    }

    fn org(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    fn single() -> HmacHistoryDigester {
        HmacHistoryDigester::new(Some(key(K1)), None).unwrap()
    }

    #[test]
    fn the_key_id_is_derived_from_the_key_bytes() {
        assert_ne!(key(K1).id(), key(K2).id());
        assert_eq!(key(K1).id(), key(K1).id());
        assert_eq!(key(K1).id().to_hex().len(), 16);
    }

    #[test]
    fn the_digest_binds_the_org_the_path_and_the_value() {
        let d = single();
        let base = d.current(org(1), &path(&["a"]), &Scalar::Str("x".into())).unwrap().1;
        let other_org = d.current(org(2), &path(&["a"]), &Scalar::Str("x".into())).unwrap().1;
        let other_path = d.current(org(1), &path(&["b"]), &Scalar::Str("x".into())).unwrap().1;
        let other_value = d.current(org(1), &path(&["a"]), &Scalar::Str("y".into())).unwrap().1;
        assert_eq!(base.0.len(), 16);
        assert!(base != other_org && base != other_path && base != other_value);
        let again = d.current(org(1), &path(&["a"]), &Scalar::Str("x".into())).unwrap().1;
        assert_eq!(base, again);
    }

    fn hmac_of(key: &str, parts: &[&[u8]]) -> Vec<u8> {
        let mut m = <Hmac<Sha256> as KeyInit>::new_from_slice(key.as_bytes()).unwrap();
        for p in parts {
            m.update(p);
        }
        m.finalize().into_bytes().to_vec()
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn the_digest_bytes_match_an_independent_hmac() {
        let (id, digest) = single().current(org(1), &path(&["a"]), &Scalar::Str("x".into())).unwrap();
        let expected = hmac_of(K1, &[b"00000000-0000-0000-0000-000000000001", &[0], b"a", &[0], b"s", b"x"]);
        assert_eq!(digest.0.as_slice(), &expected[..16]);
        assert_eq!(hex(&digest.0), "dae3046d4be4d264f56d81c99947980d");
        assert_eq!(id, key(K1).id());
    }

    #[test]
    fn the_key_id_bytes_match_an_independent_hmac_of_the_label() {
        let expected = hmac_of(K1, &[b"paigasus-history-kid"]);
        assert_eq!(key(K1).id().0.as_slice(), &expected[..8]);
        assert_eq!(key(K1).id().to_hex(), "85ada7abdf64ec57");
    }

    #[test]
    fn the_scalar_kinds_give_different_digests() {
        let d = single();
        let at = |v: Scalar| d.current(org(1), &path(&["a"]), &v).unwrap().1;
        let (s, b, n) = (at(Scalar::Str("1".into())), at(Scalar::Bool(true)), at(Scalar::Decimal(10_000)));
        assert!(s != b && s != n && b != n);
    }

    #[test]
    fn the_key_length_boundary_is_32_bytes() {
        assert!(HistoryKey::new(SecretString::from("k".repeat(31))).is_err());
        assert!(HistoryKey::new(SecretString::from("k".repeat(32))).is_ok());
    }

    #[test]
    fn a_literal_gets_both_digests_and_a_call_value_gets_the_current_one() {
        let d = HmacHistoryDigester::new(Some(key(K1)), Some(key(K2))).unwrap();
        let (p, v) = (path(&["a"]), Scalar::Bool(true));
        let all = d.all(org(1), &p, &v);
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].0, key(K1).id());
        assert_eq!(all[1].0, key(K2).id());
        assert_ne!(all[0].1, all[1].1);
        assert_eq!(d.current(org(1), &p, &v), Some(all[0]));
    }

    #[test]
    fn keyless_gives_no_digest() {
        let d = HmacHistoryDigester::keyless();
        assert!(d.current(org(1), &path(&["a"]), &Scalar::Bool(true)).is_none());
        assert!(d.all(org(1), &path(&["a"]), &Scalar::Bool(true)).is_empty());
    }

    #[test]
    fn a_short_key_a_previous_without_current_and_equal_ids_are_refused() {
        assert!(HistoryKey::new(SecretString::from("short".to_owned())).is_err());
        assert!(HmacHistoryDigester::new(None, Some(key(K1))).is_err());
        assert!(HmacHistoryDigester::new(Some(key(K1)), Some(key(K1))).is_err());
    }

    #[test]
    fn debug_never_prints_a_key() {
        let d = HmacHistoryDigester::new(Some(key(K1)), Some(key(K2))).unwrap();
        let text = format!("{d:?}");
        assert!(!text.contains(K1) && !text.contains(K2) && !text.contains("one") && !text.contains("two"), "{text}");
        assert!(text.contains(&key(K1).id().to_hex()), "{text}");
    }
}
