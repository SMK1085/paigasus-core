// SPDX-License-Identifier: Apache-2.0

//! API-key domain entity and token codec (SMA-445, M4). Pure data + parsing — secret
//! generation and HMAC hashing are ports (`KeyEntropy`/`SecretHasher`, ADR-0005 keeps them
//! out of this core crate) implemented by the service's `adapters::api_keys::{entropy,
//! hasher}`; this file only knows the `pgs_sk_<keyid_hex>_<secret_b64url>` token shape and
//! how to format/parse it. HMAC-SHA-256+pepper over argon2, shown-once token structure, and
//! constant-time verification are recorded in the API-key & secret handling ADR (Notion).

use crate::authz::Action;
use crate::tenancy::TenancyNodeRef;
use crate::value::{DomainError, PrincipalId};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Utc};
use std::fmt;
use std::str::FromStr;
use uuid::Uuid;

/// An API key's stable identifier: a bare UUID (API keys are not tenancy/authz resources,
/// so unlike `OrganizationId`/`TeamId`/etc. there's no PRN wrapper). Renders as 32-char
/// lowercase simple hex (`Uuid::as_simple`) — the exact `keyid` segment of the token format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ApiKeyId(Uuid);

impl ApiKeyId {
    #[must_use]
    pub fn from_uuid(id: Uuid) -> Self {
        Self(id)
    }

    #[must_use]
    pub fn uuid(&self) -> Uuid {
        self.0
    }
}

impl fmt::Display for ApiKeyId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0.as_simple())
    }
}

impl FromStr for ApiKeyId {
    type Err = DomainError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(s).map(ApiKeyId).map_err(|_| DomainError::InvalidApiKeyToken(s.to_string()))
    }
}

/// API-key lifecycle status (SMA-445, M4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiKeyStatus {
    Active,
    Revoked,
}

impl ApiKeyStatus {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            ApiKeyStatus::Active => "active",
            ApiKeyStatus::Revoked => "revoked",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "active" => Some(ApiKeyStatus::Active),
            "revoked" => Some(ApiKeyStatus::Revoked),
            _ => None,
        }
    }
}

/// A persisted API key: the hash of its secret is NOT modeled here (that's a port concern,
/// Task 5) — this is purely the metadata a store needs plus the scope it was minted with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiKey {
    pub id: ApiKeyId,
    pub service_account_id: PrincipalId,
    pub scope: TenancyNodeRef,
    pub prefix: String,
    pub status: ApiKeyStatus,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub scope_actions: Vec<Action>,
    pub scope_roles: Vec<String>,
}

/// Why a presented API key token was rejected. Detail only — `Display` is a generic,
/// constant message for every variant so no secret/token material or even which check
/// failed ever reaches a client-facing error message; `Debug` (tests/internal logs only)
/// still shows the variant, mirroring the `TokenDefect` scrub convention in `authn.rs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ApiKeyDefect {
    #[error("invalid api key")]
    Malformed,
    #[error("invalid api key")]
    BadSecret,
    #[error("invalid api key")]
    Revoked,
    #[error("invalid api key")]
    Expired,
}

/// A freshly minted API key: the record to persist plus the ONE-TIME plaintext token
/// (`format_token` output) handed back to the caller. Nothing re-derives the plaintext
/// after this — `Debug` is hand-rolled to redact it so an accidental `{:?}` (e.g. a stray
/// log line) can't leak it.
#[derive(Clone, PartialEq, Eq)]
pub struct NewApiKey {
    pub key: ApiKey,
    pub plaintext: String,
}

impl fmt::Debug for NewApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NewApiKey").field("key", &self.key).field("plaintext", &"<redacted>").finish()
    }
}

/// The two components extracted from a presented token by [`parse_token`]: the (public)
/// key id used to look up the stored `ApiKey`, and the (secret) bytes to verify against its
/// stored hash. `Debug` redacts `secret` for the same reason as `NewApiKey::plaintext`.
#[derive(Clone, PartialEq, Eq)]
pub struct ParsedToken {
    pub key_id: ApiKeyId,
    pub secret: Vec<u8>,
}

impl fmt::Debug for ParsedToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ParsedToken").field("key_id", &self.key_id).field("secret", &"<redacted>").finish()
    }
}

/// Formats an API-key token: `"{prefix}{keyid_hex}_{secret_b64url}"`, where `keyid_hex` is
/// the fixed-width 32-char lowercase simple hex form of `key_id` and the secret is base64url
/// (no padding). Pure formatting — `secret` is caller-supplied (generation is a port).
#[must_use]
pub fn format_token(prefix: &str, key_id: ApiKeyId, secret: &[u8]) -> String {
    let keyid_hex = key_id.uuid().as_simple().to_string();
    let secret_b64 = URL_SAFE_NO_PAD.encode(secret);
    format!("{prefix}{keyid_hex}_{secret_b64}")
}

/// Parses an API-key token produced by [`format_token`]. `max_bytes` is enforced FIRST (a
/// cheap length cap before any further work — defends against oversized inputs). The keyid
/// is then read by FIXED WIDTH (32 chars), never by splitting on `_`, because the base64url
/// alphabet itself contains `_` and a split would misparse a secret that happens to contain
/// one (see `parse_handles_underscore_in_secret`). Every slice is bounds-checked via
/// `str::get`/`slice::get` — this function never panics, regardless of input.
///
/// # Errors
/// Returns [`ApiKeyDefect::Malformed`] if the token doesn't match the `{prefix}{32 hex
/// chars}_{...}` shape (wrong/missing prefix, oversized, non-hex keyid, missing separator),
/// or [`ApiKeyDefect::BadSecret`] if the secret segment isn't strictly canonical
/// base64url-nopad (invalid characters, or valid-but-non-canonical trailing bits — caught by
/// requiring the decoded bytes to re-encode back to the exact input segment).
pub fn parse_token(prefix: &str, token: &str, max_bytes: usize) -> Result<ParsedToken, ApiKeyDefect> {
    if token.len() > max_bytes {
        return Err(ApiKeyDefect::Malformed);
    }
    let rest = token.strip_prefix(prefix).ok_or(ApiKeyDefect::Malformed)?;

    let keyid_hex = rest.get(..32).ok_or(ApiKeyDefect::Malformed)?;
    let key_uuid = Uuid::parse_str(keyid_hex).map_err(|_| ApiKeyDefect::Malformed)?;
    // `Uuid::parse_str` is case-insensitive, so an uppercase (or mixed-case) keyid would
    // parse to the same id — making the token string non-injective. Require the canonical
    // lowercase simple-hex rendering to equal the input, mirroring the secret's
    // re-encode-and-compare canonicalization check below.
    if key_uuid.as_simple().to_string() != keyid_hex {
        return Err(ApiKeyDefect::Malformed);
    }

    if rest.as_bytes().get(32) != Some(&b'_') {
        return Err(ApiKeyDefect::Malformed);
    }
    let secret_b64 = rest.get(33..).ok_or(ApiKeyDefect::Malformed)?;

    let secret = URL_SAFE_NO_PAD.decode(secret_b64).map_err(|_| ApiKeyDefect::BadSecret)?;
    // Reject non-canonical base64 (e.g. stray trailing bits the decoder tolerates) — the
    // only reliable check is that re-encoding lands back on the exact input segment.
    if URL_SAFE_NO_PAD.encode(&secret) != secret_b64 {
        return Err(ApiKeyDefect::BadSecret);
    }

    Ok(ParsedToken {
        key_id: ApiKeyId::from_uuid(key_uuid),
        secret,
    })
}

/// A safe-to-display prefix for storage/listing UIs: the token prefix plus the first 8 hex
/// chars of the key id — enough to disambiguate a key in a list without exposing the secret.
#[must_use]
pub fn display_prefix(prefix: &str, key_id: ApiKeyId) -> String {
    let keyid_hex = key_id.uuid().as_simple().to_string();
    format!("{prefix}{}", &keyid_hex[..8])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_roundtrips_via_fixed_width_parse() {
        let id = ApiKeyId::from_uuid(Uuid::from_u128(0x0192_f1c0_1234_7000_8000_0000_0000_0001));
        let secret = [7u8; 32];
        let tok = format_token("pgs_sk_", id, &secret);
        let parsed = parse_token("pgs_sk_", &tok, 512).unwrap();
        assert_eq!(parsed.key_id, id);
        assert_eq!(parsed.secret, secret.to_vec());
    }

    #[test]
    fn parse_rejects_wrong_prefix_and_overlong() {
        assert!(matches!(parse_token("pgs_sk_", "nope_abc", 512), Err(ApiKeyDefect::Malformed)));
        let huge = format!("pgs_sk_{}", "a".repeat(10_000));
        assert!(matches!(parse_token("pgs_sk_", &huge, 512), Err(ApiKeyDefect::Malformed)));
    }

    #[test]
    fn parse_handles_underscore_in_secret() {
        // secret whose base64url contains '_' must still parse (fixed-width keyid, not
        // split-on-'_'). All-`0xFF` bytes encode to an all-`_` base64url string, so this
        // genuinely exercises the underscore-in-secret path.
        let id = ApiKeyId::from_uuid(Uuid::from_u128(1));
        let secret = [0xFFu8; 32];
        let tok = format_token("pgs_sk_", id, &secret);
        let secret_b64 = tok.strip_prefix("pgs_sk_").and_then(|r| r.get(33..)).unwrap();
        assert!(secret_b64.contains('_'), "test precondition: secret's base64url must contain '_'");
        assert_eq!(parse_token("pgs_sk_", &tok, 512).unwrap().secret, secret.to_vec());
    }

    #[test]
    fn defect_display_scrubs_detail() {
        assert_eq!(ApiKeyDefect::BadSecret.to_string(), "invalid api key");
        // Every variant scrubs identically — Display must never distinguish them.
        assert_eq!(ApiKeyDefect::Malformed.to_string(), "invalid api key");
        assert_eq!(ApiKeyDefect::Revoked.to_string(), "invalid api key");
        assert_eq!(ApiKeyDefect::Expired.to_string(), "invalid api key");
        // Debug is still allowed to distinguish (tests/internal logs only).
        assert_eq!(format!("{:?}", ApiKeyDefect::BadSecret), "BadSecret");
    }

    #[test]
    fn status_roundtrips() {
        assert_eq!(ApiKeyStatus::parse("revoked"), Some(ApiKeyStatus::Revoked));
        assert_eq!(ApiKeyStatus::parse(ApiKeyStatus::Active.as_str()), Some(ApiKeyStatus::Active));
        assert_eq!(ApiKeyStatus::parse("bogus"), None);
    }

    #[test]
    fn parse_never_panics_on_short_or_malformed_input() {
        // Exercises the bounds-checking: shorter than the prefix, shorter than the fixed
        // 32-char keyid window, missing separator, missing secret, and multi-byte UTF-8
        // sitting exactly on the 32-byte cut point (which is not a valid char boundary).
        let cases = [
            "",
            "p",
            "pgs_sk_",
            "pgs_sk_short",
            "pgs_sk_00000000000000000000000000000000",    // 32 hex chars, no separator/secret
            "pgs_sk_0000000000000000000000000000000",     // 31 hex chars (one short)
            "pgs_sk_ZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZ_YQ", // non-hex keyid
            "pgs_sk_€€€€€€€€€€€€€€€€€€€€€€€€€€€€€€€€_YQ", // multi-byte chars around the cut
        ];
        for c in cases {
            let _ = parse_token("pgs_sk_", c, 512); // must not panic, result irrelevant
        }
        assert!(matches!(parse_token("pgs_sk_", "", 512), Err(ApiKeyDefect::Malformed)));
    }

    #[test]
    fn parse_rejects_non_canonical_base64_trailing_bits() {
        let id = ApiKeyId::from_uuid(Uuid::from_u128(42));
        let keyid_hex = id.uuid().as_simple().to_string();
        // "YR" decodes the same 1 byte as "YQ" but sets non-zero trailing bits — canonical
        // base64url-nopad must emit "YQ"; "YR" is a valid-but-non-canonical encoding.
        let tok = format!("pgs_sk_{keyid_hex}_YR");
        assert!(matches!(parse_token("pgs_sk_", &tok, 512), Err(ApiKeyDefect::BadSecret)));
    }

    #[test]
    fn parse_rejects_non_canonical_uppercase_keyid() {
        // `Uuid::parse_str` is case-insensitive, so an uppercase keyid parses to the same
        // id as its lowercase form — the token would not be injective. The canonical
        // lowercase form must be accepted and any uppercase/mixed-case form rejected.
        let id = ApiKeyId::from_uuid(Uuid::from_u128(0x0192_f1c0_1234_7000_8000_0000_0000_00ab));
        let secret = [7u8; 32];
        let tok = format_token("pgs_sk_", id, &secret);
        assert!(parse_token("pgs_sk_", &tok, 512).is_ok(), "canonical lowercase keyid must parse");
        // Uppercase only the keyid segment (leave prefix + secret untouched).
        let rest = tok.strip_prefix("pgs_sk_").unwrap();
        let upper_tok = format!("pgs_sk_{}{}", rest[..32].to_ascii_uppercase(), &rest[32..]);
        assert_ne!(upper_tok, tok, "test precondition: keyid must contain a hex letter to uppercase");
        assert!(matches!(parse_token("pgs_sk_", &upper_tok, 512), Err(ApiKeyDefect::Malformed)));
    }

    #[test]
    fn api_key_id_display_roundtrips_through_from_str() {
        let id = ApiKeyId::from_uuid(Uuid::from_u128(0x0192_f1c0_1234_7000_8000_0000_0000_0002));
        let s = id.to_string();
        assert_eq!(s.len(), 32);
        assert_eq!(ApiKeyId::from_str(&s).unwrap(), id);
        assert!(ApiKeyId::from_str("not-a-uuid").is_err());
    }

    #[test]
    fn display_prefix_is_prefix_plus_first_eight_hex_chars() {
        let id = ApiKeyId::from_uuid(Uuid::from_u128(0x0192_f1c0_1234_7000_8000_0000_0000_0003));
        let full_hex = id.uuid().as_simple().to_string();
        assert_eq!(display_prefix("pgs_sk_", id), format!("pgs_sk_{}", &full_hex[..8]));
    }

    #[test]
    fn new_api_key_debug_redacts_plaintext() {
        let id = ApiKeyId::from_uuid(Uuid::from_u128(9));
        let now = Utc::now();
        let key = ApiKey {
            id,
            service_account_id: test_principal_id(),
            scope: test_scope(),
            prefix: display_prefix("pgs_sk_", id),
            status: ApiKeyStatus::Active,
            expires_at: None,
            last_used_at: None,
            created_at: now,
            revoked_at: None,
            scope_actions: vec![],
            scope_roles: vec![],
        };
        let new_key = NewApiKey {
            key,
            plaintext: "pgs_sk_totally-secret-value".to_string(),
        };
        let debugged = format!("{new_key:?}");
        assert!(!debugged.contains("totally-secret-value"), "plaintext leaked into Debug: {debugged}");
        assert!(debugged.contains("redacted"));
    }

    #[test]
    fn parsed_token_debug_redacts_secret() {
        let parsed = ParsedToken {
            key_id: ApiKeyId::from_uuid(Uuid::from_u128(10)),
            secret: vec![0xAB, 0xCD, 0xEF],
        };
        let debugged = format!("{parsed:?}");
        assert!(!debugged.contains("171") && !debugged.contains("205")); // no raw byte values
        assert!(debugged.contains("redacted"));
    }

    fn test_principal_id() -> PrincipalId {
        let uuid = Uuid::parse_str("0192f1c0-0000-7000-8000-000000000009").unwrap();
        PrincipalId::from_prn(paigasus_kernel::Prn::build("iam", "", None, "principal", uuid).unwrap())
    }

    fn test_scope() -> TenancyNodeRef {
        TenancyNodeRef::Organization(crate::tenancy::OrganizationId::from_uuid(Uuid::from_u128(99)))
    }
}
