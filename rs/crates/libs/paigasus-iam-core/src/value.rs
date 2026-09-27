// SPDX-License-Identifier: Apache-2.0

//! Domain value objects: `Email`, `PrincipalId`, `Stamp`, `AuditReason` and `ExternalSubject`.

use chrono::{DateTime, Utc};
use paigasus_kernel::Prn;
use uuid::Uuid;

/// A domain-validation error (invalid value object input).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DomainError {
    #[error("invalid email: {0}")]
    InvalidEmail(String),
    #[error("invalid slug: {0}")]
    InvalidSlug(String),
    #[error("invalid name: {0}")]
    InvalidName(String),
    #[error("invalid tenancy prn: {0}")]
    InvalidNodePrn(String),
    #[error("invalid issuer: {0}")]
    InvalidIssuer(String),
    #[error("invalid api key token: {0}")]
    InvalidApiKeyToken(String),
    /// SMA-712. A unit variant: the caller's reason text is never kept in the error.
    #[error("invalid audit reason")]
    InvalidReason,
    /// SMA-712. A unit variant: the caller's subject is never kept in the error.
    #[error("invalid external subject")]
    InvalidSubject,
}

/// A validated email address. M0 rule: non-empty, exactly one `@`, non-empty local
/// and domain parts. Deliberately minimal — full RFC 5322 is out of scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Email(String);

impl Email {
    pub fn parse(raw: &str) -> Result<Self, DomainError> {
        let s = raw.trim();
        let bad = |r: &str| DomainError::InvalidEmail(r.to_string());
        if s.is_empty() {
            return Err(bad(raw));
        }
        let (local, domain) = s.split_once('@').ok_or_else(|| bad(raw))?;
        if local.is_empty() || domain.is_empty() || domain.contains('@') {
            return Err(bad(raw));
        }
        Ok(Email(s.to_string()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The longest audit reason, in Unicode scalar values after trim (SMA-712 spec 5.5).
pub const AUDIT_REASON_MAX_CHARS: usize = 500;

/// The longest external subject, in Unicode scalar values (SMA-712 spec 5.2).
pub const EXTERNAL_SUBJECT_MAX_CHARS: usize = 255;

/// The operator's reason for an identity-link write (SMA-712). The value is trimmed. After the
/// trim it has 1 to [`AUDIT_REASON_MAX_CHARS`] characters. It goes into the audit record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditReason(String);

impl AuditReason {
    pub fn parse(raw: &str) -> Result<Self, DomainError> {
        let trimmed = raw.trim();
        let chars = trimmed.chars().count();
        if chars == 0 || chars > AUDIT_REASON_MAX_CHARS {
            return Err(DomainError::InvalidReason);
        }
        Ok(AuditReason(trimmed.to_string()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// An OIDC `sub` value that an operator links to a user (SMA-712). It is NOT trimmed: JIT stores
/// the `sub` claim with no change, and a changed value would never match a token. It has 1 to
/// [`EXTERNAL_SUBJECT_MAX_CHARS`] characters, and it does not start or end with whitespace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalSubject(String);

impl ExternalSubject {
    pub fn parse(raw: &str) -> Result<Self, DomainError> {
        let chars = raw.chars().count();
        if chars == 0 || chars > EXTERNAL_SUBJECT_MAX_CHARS || raw.starts_with(char::is_whitespace) || raw.ends_with(char::is_whitespace) {
            return Err(DomainError::InvalidSubject);
        }
        Ok(ExternalSubject(raw.to_string()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}

/// A principal's stable identity: its PRN (`prn:pgs:iam:::principal/<uuidv7>`). The UUID
/// (the PK/FK) is derived from the PRN's resource-id — stored once, never duplicated.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PrincipalId(Prn);

impl PrincipalId {
    #[must_use]
    pub fn from_prn(prn: Prn) -> Self {
        PrincipalId(prn)
    }

    #[must_use]
    pub fn uuid(&self) -> Uuid {
        self.0.resource_id()
    }

    #[must_use]
    pub fn prn(&self) -> &Prn {
        &self.0
    }

    #[must_use]
    pub fn canonical(&self) -> String {
        self.0.canonical()
    }
}

/// The who+when of a single write (SMA-440).
///
/// Carried as one value so a mutation cannot advance a timestamp without naming the actor.
/// Every mutating repository port takes one, and the application service is the only place
/// that constructs one — from its `Clock` port plus the actor the transport handed it.
///
/// `by` is a [`PrincipalId`] rather than a bare `Prn` because that is the type asserting the
/// PRN names a principal, and every caller already holds one. The *stored* columns are
/// `Option<PrincipalId>` instead, which models a row written before the columns existed —
/// not a missing actor at write time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stamp {
    pub at: DateTime<Utc>,
    pub by: PrincipalId,
}

impl Stamp {
    #[must_use]
    pub fn new(at: DateTime<Utc>, by: PrincipalId) -> Self {
        Stamp { at, by }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn email_accepts_a_simple_address() {
        assert_eq!(Email::parse("  a@b.com ").unwrap().as_str(), "a@b.com");
    }

    #[test]
    fn email_rejects_empty_missing_at_and_empty_parts() {
        for bad in ["", "  ", "nope", "@b.com", "a@", "a@@b", "a b"] {
            assert!(Email::parse(bad).is_err(), "expected {bad:?} to be rejected");
        }
    }

    #[test]
    fn principal_id_derives_uuid_and_canonical_from_prn() {
        let uuid = Uuid::parse_str("0192f1c0-0000-7000-8000-000000000000").unwrap();
        let prn = Prn::build("iam", "", None, "principal", uuid).unwrap();
        let id = PrincipalId::from_prn(prn);
        assert_eq!(id.uuid(), uuid);
        assert_eq!(id.canonical(), format!("prn:pgs:iam:::principal/{uuid}"));
    }

    #[test]
    fn stamp_carries_both_halves_of_a_write() {
        use chrono::TimeZone;
        let at = chrono::Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        let by = PrincipalId::from_prn(paigasus_kernel::Prn::build("iam", "", None, "principal", Uuid::from_u128(1)).unwrap());
        let stamp = Stamp::new(at, by.clone());
        assert_eq!(stamp.at, at);
        assert_eq!(stamp.by, by);
    }

    /// SMA-712 spec 5.5: the reason is trimmed, and the limit counts Unicode scalar values, not
    /// bytes. `é` is two bytes, so a byte count would refuse the 500-character value.
    #[test]
    fn audit_reason_is_trimmed_and_counts_characters_not_bytes() {
        assert_eq!(AuditReason::parse("  INC-42: same person  ").unwrap().as_str(), "INC-42: same person");
        let max = "é".repeat(AUDIT_REASON_MAX_CHARS);
        assert_eq!(AuditReason::parse(&format!("  {max}\n")).unwrap().as_str(), max);
        assert_eq!(AuditReason::parse(&"é".repeat(AUDIT_REASON_MAX_CHARS + 1)), Err(DomainError::InvalidReason));
    }

    /// A reason of whitespace only is empty after trim. `U+00A0` and `U+2003` are whitespace.
    #[test]
    fn audit_reason_rejects_empty_and_whitespace_only_values() {
        for bad in ["", "   ", "\n\t", "\u{00A0}\u{2003}"] {
            assert_eq!(AuditReason::parse(bad), Err(DomainError::InvalidReason), "{bad:?}");
        }
    }

    /// SMA-712 spec 5.2: the subject is stored exactly as given. JIT stores the `sub` claim with
    /// no change, so a trimmed value would never match a token. Interior spaces are legal.
    #[test]
    fn external_subject_is_stored_exactly_and_counts_characters_not_bytes() {
        assert_eq!(ExternalSubject::parse("f:1b2c:user 7").unwrap().as_str(), "f:1b2c:user 7");
        let max = "é".repeat(EXTERNAL_SUBJECT_MAX_CHARS);
        assert_eq!(ExternalSubject::parse(&max).unwrap().into_string(), max);
        assert_eq!(ExternalSubject::parse(&"é".repeat(EXTERNAL_SUBJECT_MAX_CHARS + 1)), Err(DomainError::InvalidSubject));
    }

    /// A subject that starts or ends with whitespace is refused, not trimmed.
    #[test]
    fn external_subject_rejects_empty_and_edge_whitespace() {
        for bad in ["", " ", " abc", "abc ", "\u{00A0}abc", "abc\n", "\tabc"] {
            assert_eq!(ExternalSubject::parse(bad), Err(DomainError::InvalidSubject), "{bad:?}");
        }
    }
}
