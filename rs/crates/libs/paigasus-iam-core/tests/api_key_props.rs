// SPDX-License-Identifier: Apache-2.0
//! Property-based coverage of the api-key token codec (`format_token`/`parse_token`, Task 3)
//! and the `SecretHasher` port contract (SMA-445, AC-1 "constant-time validation").
//!
//! `TestHasher` is a minimal HMAC-SHA-256 `SecretHasher` implementation, local to this test
//! file only — it stands in for the real adapter (a later SMA-445 task) to prove the port's
//! round-trip/rejection contract without pulling crypto into `paigasus-iam-core`'s runtime
//! dependencies (ADR-0005 keeps the domain crate pure; `hmac`/`sha2` are dev-dependencies
//! only, never shipped).

use hmac::{Hmac, Mac};
use paigasus_iam_core::{ApiKeyId, SecretHasher, format_token, parse_token};
use proptest::prelude::*;
use sha2::Sha256;
use uuid::Uuid;

/// Test-local `SecretHasher`: HMAC-SHA-256 keyed by a caller-supplied pepper. Not the real
/// adapter — just enough to exercise the port contract (`hash` then `verify` round-trips;
/// any secret/pepper mismatch is rejected).
struct TestHasher {
    pepper: Vec<u8>,
}

impl TestHasher {
    fn new(pepper: &[u8]) -> Self {
        Self { pepper: pepper.to_vec() }
    }
}

impl SecretHasher for TestHasher {
    fn hash(&self, secret: &[u8]) -> Vec<u8> {
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.pepper).expect("HMAC-SHA-256 accepts a key of any length");
        mac.update(secret);
        mac.finalize().into_bytes().to_vec()
    }

    fn verify(&self, secret: &[u8], expected: &[u8]) -> bool {
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.pepper).expect("HMAC-SHA-256 accepts a key of any length");
        mac.update(secret);
        mac.verify_slice(expected).is_ok()
    }
}

proptest! {
    // (a) issue -> parse round-trips for any secret bytes.
    #[test]
    fn issue_parse_roundtrip(secret in proptest::array::uniform32(any::<u8>()), lo in any::<u128>()) {
        let id = ApiKeyId::from_uuid(Uuid::from_u128(lo));
        let tok = format_token("pgs_sk_", id, &secret);
        let p = parse_token("pgs_sk_", &tok, 4096).unwrap();
        prop_assert_eq!(p.key_id, id);
        prop_assert_eq!(p.secret, secret.to_vec());
    }

    // (b) any single-bit flip of the secret fails HMAC verify against the original hash.
    #[test]
    fn bitflip_secret_rejected(secret in proptest::array::uniform32(any::<u8>()), idx in 0usize..32, bit in 0u8..8) {
        let h = TestHasher::new(b"peppered-pepper-32-bytes-minimum!!");
        let good = h.hash(&secret);
        let mut bad = secret;
        bad[idx] ^= 1 << bit;
        prop_assert!(h.verify(&secret, &good));
        prop_assert!(!h.verify(&bad, &good));
    }

    // (c) a hash produced under one pepper never verifies under a different pepper.
    #[test]
    fn wrong_pepper_rejected(secret in proptest::array::uniform32(any::<u8>())) {
        let a = TestHasher::new(b"pepper-aaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        let b = TestHasher::new(b"pepper-bbbbbbbbbbbbbbbbbbbbbbbbbbbb");
        prop_assert!(!b.verify(&secret, &a.hash(&secret)));
    }

    // (d) arbitrary bytes fed into parse_token never panic, regardless of shape.
    #[test]
    fn parse_never_panics(s in ".*") {
        let _ = parse_token("pgs_sk_", &s, 4096);
    }
}
