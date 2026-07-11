// SPDX-License-Identifier: Apache-2.0

//! Core authentication domain types for OIDC BYO-IdP login (SMA-443, M2). Pure data +
//! parsing — no I/O, no JWT/JWKS handling (that lives in the service's adapters, ADR-0005).

use crate::api_key::ApiKeyId;
use crate::authz::model::RoleGrantRef;
use crate::ports::MembershipRecord;
use crate::principal::{PrincipalKind, PrincipalStatus};
use crate::value::{DomainError, PrincipalId};
use chrono::{DateTime, Utc};
use std::fmt;
use uuid::Uuid;

/// A validated OIDC issuer URL. Compared byte-for-byte against the value configured for the
/// tenant — no normalization, since IdPs are inconsistent about trailing slashes and a
/// mismatch there is exactly the kind of ambiguity we want callers to catch (spec §3.1).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Issuer(String);

impl Issuer {
    /// Parses and validates an issuer string: trims surrounding whitespace, then requires an
    /// `https://` scheme, a non-empty host, no fragment (`#`), and no interior whitespace. This
    /// is deliberately a "parse-lite" check (no URL crate) — full URL semantics aren't needed
    /// for an exact-match comparison key.
    pub fn parse(raw: &str) -> Result<Self, DomainError> {
        let bad = || DomainError::InvalidIssuer(raw.to_string());
        let trimmed = raw.trim();
        let rest = trimmed.strip_prefix("https://").ok_or_else(bad)?;
        if rest.contains('#') || trimmed.contains(char::is_whitespace) {
            return Err(bad());
        }
        let host = rest.split_once('/').map_or(rest, |(host, _)| host);
        if host.is_empty() {
            return Err(bad());
        }
        Ok(Issuer(trimmed.to_string()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Issuer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The claims extracted from a verified access token, after signature/expiry/audience checks
/// have already passed (spec §3.2). Optional profile fields are best-effort — the IdP may
/// omit any of them.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidatedClaims {
    pub issuer: Issuer,
    pub subject: String,
    pub audiences: Vec<String>,
    pub expires_at: DateTime<Utc>,
    pub email: Option<String>,
    pub name: Option<String>,
    pub locale: Option<String>,
    pub zoneinfo: Option<String>,
}

/// The credential that authenticated a request: either a validated OIDC token (M2) or an
/// API key (M4, Task 19 wires the producer — every principal this crate's consumers build
/// today is still `Oidc`). Modeled as an enum rather than optional flat fields because the
/// two credential kinds carry genuinely different data (an OIDC subject has no equivalent
/// for an API key, and vice versa) — a shared flat shape would let callers read a
/// `subject`/`issuer` that never meaningfully applied to an `ApiKey`-authenticated request.
#[derive(Debug, Clone, PartialEq)]
pub enum Credential {
    Oidc { issuer: Issuer, subject: String, expires_at: DateTime<Utc> },
    ApiKey { key_id: ApiKeyId, expires_at: Option<DateTime<Utc>> },
}

/// A principal resolved from a validated credential: the local identity plus the
/// credential that authenticated this request.
#[derive(Debug, Clone, PartialEq)]
pub struct AuthnPrincipal {
    pub principal_id: PrincipalId,
    pub kind: PrincipalKind,
    pub status: PrincipalStatus,
    pub credential: Credential,
}

impl AuthnPrincipal {
    /// The credential's expiry, when it has one — `Oidc` always does (the token's `exp`
    /// claim); `ApiKey` only if the key itself was minted with an expiry.
    #[must_use]
    pub fn expires_at(&self) -> Option<DateTime<Utc>> {
        match &self.credential {
            Credential::Oidc { expires_at, .. } => Some(*expires_at),
            Credential::ApiKey { expires_at, .. } => *expires_at,
        }
    }

    /// The OIDC issuer that authenticated this request — `None` for an `ApiKey` credential
    /// (API keys have no issuer).
    #[must_use]
    pub fn issuer(&self) -> Option<&Issuer> {
        match &self.credential {
            Credential::Oidc { issuer, .. } => Some(issuer),
            Credential::ApiKey { .. } => None,
        }
    }

    /// The OIDC subject that authenticated this request — `None` for an `ApiKey` credential
    /// (API keys have no subject).
    #[must_use]
    pub fn subject(&self) -> Option<&str> {
        match &self.credential {
            Credential::Oidc { subject, .. } => Some(subject.as_str()),
            Credential::ApiKey { .. } => None,
        }
    }
}

/// The full authorization context for a request: the authenticated principal plus the
/// tenancy memberships and role grants it carries. `role_grants` is always empty until a
/// later M3 task populates it from the `RoleGrantStore`.
#[derive(Debug, Clone, PartialEq)]
pub struct PrincipalContext {
    pub principal: AuthnPrincipal,
    pub memberships: Vec<MembershipRecord>,
    pub role_grants: Vec<RoleGrantRef>,
}

/// A persisted link between a principal and one external IdP identity (issuer, subject).
/// A principal may accumulate one per IdP it has authenticated through.
#[derive(Debug, Clone, PartialEq)]
pub struct ExternalIdentity {
    pub id: Uuid,
    pub principal_id: PrincipalId,
    pub issuer: Issuer,
    pub subject: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Why a presented token was rejected. Detail only — never surfaced in `AuthnError`'s
/// `Display` (no token/claim material in logs); useful for tests and internal diagnostics
/// via `Debug` (spec §3.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenDefect {
    Malformed,
    UnsupportedAlg,
    UnknownKid,
    BadSignature,
    Expired,
    NotYetValid,
    IssuerNotConfigured,
    AudienceMismatch,
    Oversized,
}

/// Why just-in-time provisioning of a new identity failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProvisioningDefect {
    MissingEmail,
    EmailConflict,
}

/// Authentication use-case errors. `Display` never includes token or claim values —
/// `TokenDefect`/`ProvisioningDefect` detail is exposed only through `{:?}` for logs/tests.
#[derive(Debug, thiserror::Error)]
pub enum AuthnError {
    #[error("invalid token: {0:?}")]
    InvalidToken(TokenDefect),
    #[error("identity not provisioned")]
    IdentityNotProvisioned,
    #[error("provisioning failed: {0:?}")]
    ProvisioningFailed(ProvisioningDefect),
    #[error("principal inactive")]
    PrincipalInactive,
    #[error("authentication backend unavailable")]
    Unavailable,
    #[error("backend error")]
    Backend(#[from] Box<dyn std::error::Error + Send + Sync>),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pid() -> PrincipalId {
        let uuid = Uuid::parse_str("0192f1c0-0000-7000-8000-000000000009").unwrap();
        PrincipalId::from_prn(paigasus_kernel::Prn::build("iam", "", None, "principal", uuid).unwrap())
    }

    #[test]
    fn api_key_principal_has_no_issuer() {
        let p = AuthnPrincipal {
            principal_id: pid(),
            kind: PrincipalKind::ServiceAccount,
            status: PrincipalStatus::Active,
            credential: Credential::ApiKey {
                key_id: ApiKeyId::from_uuid(Uuid::from_u128(1)),
                expires_at: None,
            },
        };
        assert!(p.issuer().is_none());
        assert!(p.subject().is_none());
        assert_eq!(p.expires_at(), None);
    }

    #[test]
    fn oidc_principal_exposes_issuer_subject_and_expiry() {
        let issuer = Issuer::parse("https://idp.example.com").unwrap();
        let expires_at = Utc::now();
        let p = AuthnPrincipal {
            principal_id: pid(),
            kind: PrincipalKind::User,
            status: PrincipalStatus::Active,
            credential: Credential::Oidc {
                issuer: issuer.clone(),
                subject: "sub-1".to_string(),
                expires_at,
            },
        };
        assert_eq!(p.issuer(), Some(&issuer));
        assert_eq!(p.subject(), Some("sub-1"));
        assert_eq!(p.expires_at(), Some(expires_at));
    }

    #[test]
    fn issuer_accepts_https_urls_verbatim() {
        let i = Issuer::parse("https://idp.example.com/realms/acme").unwrap();
        assert_eq!(i.as_str(), "https://idp.example.com/realms/acme");
        // No normalization: trailing slash is a DIFFERENT issuer (exact-match rule, spec §3.1).
        let j = Issuer::parse("https://idp.example.com/realms/acme/").unwrap();
        assert_ne!(i, j);
    }

    #[test]
    fn issuer_rejects_non_https_fragments_and_garbage() {
        for bad in ["", "http://idp.example.com", "idp.example.com", "https://", "https://idp.example.com/#frag", "not a url"] {
            assert!(Issuer::parse(bad).is_err(), "expected {bad:?} rejected");
        }
    }

    #[test]
    fn issuer_rejects_interior_whitespace() {
        // The whitespace check must catch spaces/tabs INSIDE the trimmed string — both in
        // the host and in the path — not just the surrounding padding `parse` trims away.
        for bad in ["https://idp.example .com", "https://idp.example.com/realms acme", "https://idp.example.com/realms\tacme"] {
            assert!(Issuer::parse(bad).is_err(), "expected {bad:?} rejected");
        }
    }
}
