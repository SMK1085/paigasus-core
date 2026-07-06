// SPDX-License-Identifier: Apache-2.0

//! Core authentication domain types for OIDC BYO-IdP login (SMA-443, M2). Pure data +
//! parsing — no I/O, no JWT/JWKS handling (that lives in the service's adapters, ADR-0005).

use crate::ports::MembershipRecord;
use crate::principal::{PrincipalKind, PrincipalStatus};
use crate::value::{DomainError, PrincipalId};
use chrono::{DateTime, Utc};
use paigasus_kernel::Prn;
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

/// A principal resolved from a validated token: the local identity plus the external
/// (issuer, subject) pair that authenticated this request.
#[derive(Debug, Clone, PartialEq)]
pub struct AuthnPrincipal {
    pub principal_id: PrincipalId,
    pub kind: PrincipalKind,
    pub status: PrincipalStatus,
    pub issuer: Issuer,
    pub subject: String,
    pub expires_at: DateTime<Utc>,
}

/// The full authorization context for a request: the authenticated principal plus the
/// tenancy memberships and role groups it carries. `role_groups` is always empty until M3.
#[derive(Debug, Clone, PartialEq)]
pub struct PrincipalContext {
    pub principal: AuthnPrincipal,
    pub memberships: Vec<MembershipRecord>,
    pub role_groups: Vec<Prn>,
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
