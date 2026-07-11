// SPDX-License-Identifier: Apache-2.0

//! `HmacSecretHasher` — the `SecretHasher` port's v1 implementation: HMAC-SHA-256 keyed by an
//! operator-configured pepper (never persisted, never logged). `hash` computes the MAC over
//! the presented secret bytes; `verify` recomputes it and compares against the stored tag via
//! `Mac::verify_slice`, which is constant-time in the tag length (mirrors the test-local
//! `TestHasher` in `paigasus-iam-core`'s `tests/api_key_props.rs`, SMA-445 Task 6). The choice
//! of HMAC-SHA-256+pepper over argon2 (and the single-pepper rotation caveat) is recorded in
//! the API-key & secret handling ADR (Notion).

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use hmac::{Hmac, Mac};
use paigasus_iam_core::SecretHasher;
use sha2::Sha256;

/// Minimum accepted pepper length after decoding, in bytes. 32 bytes (256 bits) matches
/// HMAC-SHA-256's ideal key size and rules out trivially short/guessable peppers.
const MIN_PEPPER_BYTES: usize = 32;

/// A REDACTING newtype around the HMAC pepper's raw bytes. `Debug` is hand-rolled to print a
/// fixed placeholder — never the bytes — mirroring `paigasus_iam_core::api_key::ApiKey`'s
/// redacted plaintext-token `Debug`. Deliberately NOT `Serialize`/`Deserialize`: if this is
/// ever embedded in a config struct that derives those, add `#[serde(skip)]` (or a bespoke
/// serializer) rather than deriving them here, so the pepper can never round-trip through a
/// logged/dumped config snapshot. `Clone` IS derived (cheap — a small `Vec<u8>` copy): SMA-445
/// Task 19's `AppState` wiring needs `HmacSecretHasher` (and therefore `Pepper`) to be `Clone`
/// so the `AuthenticateApiKey`/`ApiKeyService` use cases it's embedded in can satisfy their own
/// `#[derive(Clone)]`, which `AppState`'s own `#[derive(Clone)]` requires transitively —
/// `Clone` never round-trips through `Debug`/`Serialize`, so this doesn't weaken redaction.
#[derive(Clone)]
pub struct Pepper(Vec<u8>);

impl std::fmt::Debug for Pepper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Pepper").field(&"<redacted>").finish()
    }
}

/// Why a configured pepper was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PepperConfigError {
    #[error("pepper is not valid base64")]
    InvalidEncoding,
    #[error("pepper must decode to at least {MIN_PEPPER_BYTES} bytes")]
    TooShort,
}

impl Pepper {
    /// Decodes `s` as standard (padded) base64 — the encoding this service already depends on
    /// (`base64`, pulled in by the OIDC validator and the core's api-key token codec) — and
    /// requires at least [`MIN_PEPPER_BYTES`] decoded bytes. Hex is deliberately NOT accepted
    /// alongside it: one encoding keeps the operator-facing contract (config file / `IAM_*`
    /// env var) unambiguous, and base64 is already a dependency this crate carries.
    pub fn from_config(s: &str) -> Result<Self, PepperConfigError> {
        let bytes = STANDARD.decode(s.trim()).map_err(|_| PepperConfigError::InvalidEncoding)?;
        if bytes.len() < MIN_PEPPER_BYTES {
            return Err(PepperConfigError::TooShort);
        }
        Ok(Self(bytes))
    }
}

/// The `SecretHasher` port's v1 implementation (spec M4). `Clone` mirrors `Pepper`'s own
/// (SMA-445 Task 19 `AppState` wiring, see `Pepper`'s doc).
#[derive(Debug, Clone)]
pub struct HmacSecretHasher {
    pepper: Pepper,
}

impl HmacSecretHasher {
    pub fn new(pepper: Pepper) -> Self {
        Self { pepper }
    }
}

impl SecretHasher for HmacSecretHasher {
    fn hash(&self, secret: &[u8]) -> Vec<u8> {
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.pepper.0).expect("HMAC-SHA-256 accepts a key of any length");
        mac.update(secret);
        mac.finalize().into_bytes().to_vec()
    }

    fn verify(&self, secret: &[u8], expected: &[u8]) -> bool {
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.pepper.0).expect("HMAC-SHA-256 accepts a key of any length");
        mac.update(secret);
        mac.verify_slice(expected).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A >=32-byte pepper, base64-encoded.
    fn test_pepper_b64() -> String {
        STANDARD.encode([0x5au8; 32])
    }

    #[test]
    fn hash_verify_roundtrip_and_reject() {
        let h = HmacSecretHasher::new(Pepper::from_config(&test_pepper_b64()).unwrap());
        let tag = h.hash(b"secret-bytes");
        assert!(h.verify(b"secret-bytes", &tag));
        assert!(!h.verify(b"other", &tag));
    }

    #[test]
    fn pepper_debug_is_redacted() {
        let p = Pepper::from_config(&test_pepper_b64()).unwrap();
        let debug = format!("{p:?}");
        assert_eq!(debug, "Pepper(\"<redacted>\")");
        // Belt-and-braces: the raw pepper (in its configured base64 form) must not leak into
        // the Debug output.
        assert!(!debug.contains(&test_pepper_b64()));
    }

    #[test]
    fn hasher_debug_is_also_redacted() {
        let h = HmacSecretHasher::new(Pepper::from_config(&test_pepper_b64()).unwrap());
        let debug = format!("{h:?}");
        assert!(debug.contains("<redacted>"));
        assert!(!debug.contains(&test_pepper_b64()));
    }

    #[test]
    fn from_config_rejects_short_pepper() {
        let short = STANDARD.encode([0x5au8; 16]); // 16 decoded bytes < the 32 minimum
        assert_eq!(Pepper::from_config(&short).unwrap_err(), PepperConfigError::TooShort);
    }

    #[test]
    fn from_config_rejects_invalid_base64() {
        assert_eq!(Pepper::from_config("not-valid-base64!!").unwrap_err(), PepperConfigError::InvalidEncoding);
    }

    #[test]
    fn from_config_accepts_exactly_32_bytes() {
        assert!(Pepper::from_config(&STANDARD.encode([0x5au8; 32])).is_ok());
    }

    #[test]
    fn different_peppers_reject_each_others_tag() {
        let a = HmacSecretHasher::new(Pepper::from_config(&STANDARD.encode([0xAAu8; 32])).unwrap());
        let b = HmacSecretHasher::new(Pepper::from_config(&STANDARD.encode([0xBBu8; 32])).unwrap());
        let tag = a.hash(b"secret-bytes");
        assert!(!b.verify(b"secret-bytes", &tag));
    }
}
