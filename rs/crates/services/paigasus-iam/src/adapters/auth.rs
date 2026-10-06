// SPDX-License-Identifier: Apache-2.0

//! Transport-agnostic authentication plumbing shared by the HTTP bearer middleware and the
//! gRPC enforcement layer: bearer extraction from request headers, and the [`AuthContext`]
//! both surfaces attach for downstream handlers. Lives outside `adapters::http` so the gRPC
//! layer no longer reaches into a sibling transport adapter for it (SMA-454 C1); M3
//! (authorization) and M5 (audit) will consume both from here.

use axum::http::{HeaderMap, header};
use paigasus_iam_core::{Credential, PrincipalId, PrincipalKind, PrincipalStatus};

/// The bearer-resolved caller, attached to a request's extensions by the gRPC `AuthEnforce`
/// layer and by the HTTP `require_bearer` middleware. Both attach the SAME shape, which is what
/// "fixed" means here — a field added for one transport must be filled by both.
///
/// `kind` and `status` were added by SMA-632: `WhoAmI` returns the caller's principal, and
/// `WhoAmIResponse.status` needs the status. Both middlewares already hold a full
/// `AuthnPrincipal` and used to discard these two fields, so carrying them costs no extra query.
#[derive(Clone)]
pub struct AuthContext {
    pub principal_id: PrincipalId,
    /// Carried so a handler can rebuild a complete `AuthnPrincipal`. No mapper reads it
    /// directly — do not delete it as unused without checking `grpc::authn::who_am_i`.
    pub kind: PrincipalKind,
    pub status: PrincipalStatus,
    pub credential: Credential,
}

/// Extracts the bearer token from the `Authorization` header — the sole accepted
/// credential source on both surfaces (no cookies, no query parameters). Returns `None`
/// for an absent header, a non-UTF-8 value, a fused or non-`Bearer` scheme, or an empty
/// credential. The scheme match is ASCII-case-insensitive per RFC 7235 §2.1.
pub fn bearer_from_headers(headers: &HeaderMap) -> Option<String> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("Bearer") {
        return None;
    }
    let token = token.trim();
    if token.is_empty() {
        return None;
    }
    Some(token.to_string())
}

/// The `DPoP` scheme's token (SMA-700 § 4.8): `Authorization: DPoP <token>`, scheme
/// ASCII-case-insensitive, token trimmed and not empty. Only `AuthEnforce` reads it, and only for
/// the `IsAuthorized` follow-up while DPoP is on. `bearer_from_headers` keeps refusing the scheme,
/// so every other gRPC route and every HTTP route refuse it as before.
pub fn dpop_token_from_headers(headers: &HeaderMap) -> Option<String> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("DPoP") {
        return None;
    }
    let token = token.trim();
    (!token.is_empty()).then(|| token.to_string())
}

/// The `dpop` request header (gRPC metadata) of the follow-up. `Debug` prints no proof.
pub enum DpopProofHeader {
    One(String),
    Missing,
    /// Two or more entries, a value that is not visible ASCII, or an empty value.
    Invalid,
}

impl std::fmt::Debug for DpopProofHeader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            DpopProofHeader::One(_) => "One(..)",
            DpopProofHeader::Missing => "Missing",
            DpopProofHeader::Invalid => "Invalid",
        })
    }
}

/// The `dpop` header: exactly one entry (§ 4.8).
pub fn dpop_proof_from_headers(headers: &HeaderMap) -> DpopProofHeader {
    let mut values = headers.get_all("dpop").iter();
    let Some(first) = values.next() else {
        return DpopProofHeader::Missing;
    };
    if values.next().is_some() {
        return DpopProofHeader::Invalid;
    }
    match first.to_str() {
        Ok(proof) if !proof.is_empty() => DpopProofHeader::One(proof.to_string()),
        _ => DpopProofHeader::Invalid,
    }
}

/// Marks a request that `AuthEnforce` admitted as the DPoP `IsAuthorized` follow-up (SMA-700
/// § 4.8). `grpc::authz::is_authorized` then requires a self-query.
#[derive(Debug, Clone, Copy)]
pub struct DpopFollowUp;

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(value: Option<&str>) -> HeaderMap {
        let mut headers = HeaderMap::new();
        if let Some(value) = value {
            headers.insert(header::AUTHORIZATION, HeaderValue::from_str(value).unwrap());
        }
        headers
    }

    #[test]
    fn accepts_bearer_scheme_case_insensitively() {
        // RFC 7235 §2.1: the auth-scheme token is case-insensitive.
        assert_eq!(bearer_from_headers(&headers(Some("Bearer abc"))).as_deref(), Some("abc"));
        assert_eq!(bearer_from_headers(&headers(Some("bearer abc"))).as_deref(), Some("abc"));
        assert_eq!(bearer_from_headers(&headers(Some("BEARER abc"))).as_deref(), Some("abc"));
    }

    #[test]
    fn rejects_absent_fused_foreign_and_empty() {
        assert_eq!(bearer_from_headers(&headers(None)), None, "absent header");
        assert_eq!(bearer_from_headers(&headers(Some("Bearertoken"))), None, "scheme fused with credential (no space)");
        assert_eq!(bearer_from_headers(&headers(Some("Basic dXNlcjpwdw=="))), None, "non-Bearer scheme");
        assert_eq!(bearer_from_headers(&headers(Some("Bearer "))), None, "empty credential");
        assert_eq!(bearer_from_headers(&headers(Some("Bearer \t "))), None, "whitespace-only credential");
    }

    fn with_dpop(authorization: Option<&str>, proofs: &[&[u8]]) -> HeaderMap {
        let mut headers = headers(authorization);
        for proof in proofs {
            headers.append("dpop", HeaderValue::from_bytes(proof).unwrap());
        }
        headers
    }

    #[test]
    fn dpop_token_needs_the_dpop_scheme_in_any_case() {
        assert_eq!(dpop_token_from_headers(&headers(Some("DPoP abc"))).as_deref(), Some("abc"));
        assert_eq!(dpop_token_from_headers(&headers(Some("dpop abc"))).as_deref(), Some("abc"));
        assert_eq!(dpop_token_from_headers(&headers(Some("Bearer abc"))), None);
        assert_eq!(dpop_token_from_headers(&headers(Some("DPoP "))), None);
        assert_eq!(dpop_token_from_headers(&headers(Some("DPoPabc"))), None);
        assert_eq!(dpop_token_from_headers(&headers(None)), None);
        // The shared Bearer parser keeps refusing DPoP, so IAM's HTTP routes do too (§ 4.8).
        assert_eq!(bearer_from_headers(&headers(Some("DPoP abc"))), None);
    }

    #[test]
    fn the_proof_header_must_be_exactly_one_visible_ascii_value() {
        assert!(matches!(dpop_proof_from_headers(&with_dpop(None, &[b"a.b.c"])), DpopProofHeader::One(p) if p == "a.b.c"));
        assert!(matches!(dpop_proof_from_headers(&with_dpop(None, &[])), DpopProofHeader::Missing));
        assert!(matches!(dpop_proof_from_headers(&with_dpop(None, &[b"a.b.c", b"d.e.f"])), DpopProofHeader::Invalid));
        assert!(matches!(dpop_proof_from_headers(&with_dpop(None, &[b"a.\xffb.c"])), DpopProofHeader::Invalid));
        assert!(matches!(dpop_proof_from_headers(&with_dpop(None, &[b""])), DpopProofHeader::Invalid));
        assert_eq!(format!("{:?}", DpopProofHeader::One("secret".into())), "One(..)");
    }
}
