// SPDX-License-Identifier: Apache-2.0

//! Domain value objects: `Email` and `PrincipalId`.

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
}
