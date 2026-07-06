// SPDX-License-Identifier: Apache-2.0

//! Transport-agnostic authentication plumbing shared by the HTTP bearer middleware and the
//! gRPC enforcement layer: bearer extraction from request headers, and the [`AuthContext`]
//! both surfaces attach for downstream handlers. Lives outside `adapters::http` so the gRPC
//! layer no longer reaches into a sibling transport adapter for it (SMA-454 C1); M3
//! (authorization) and M5 (audit) will consume both from here.

use axum::http::{HeaderMap, header};
use paigasus_iam_core::{Issuer, PrincipalId};

/// The authenticated request context the enforcement layers attach on success (D13: the
/// hot path resolves the principal only — no membership fetch; that stays in `Introspect`).
/// M2 handlers don't read it yet; M3 (authorization) and M5 (audit) will. The HTTP
/// middleware and the gRPC layer attach this exact same shape, so the field set is
/// deliberately fixed here.
#[derive(Clone)]
pub struct AuthContext {
    pub principal_id: PrincipalId,
    pub issuer: Issuer,
    pub subject: String,
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
}
