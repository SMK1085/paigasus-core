// SPDX-License-Identifier: Apache-2.0

//! The `Principal` root entity and its kind/status value enums.

use crate::value::PrincipalId;
use chrono::{DateTime, Utc};

/// Principal subtype (SMA-445, M4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrincipalKind {
    User,
    ServiceAccount,
}

impl PrincipalKind {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            PrincipalKind::User => "user",
            PrincipalKind::ServiceAccount => "service_account",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "user" => Some(PrincipalKind::User),
            "service_account" => Some(PrincipalKind::ServiceAccount),
            _ => None,
        }
    }
}

/// Principal lifecycle status (SMA-445, M4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrincipalStatus {
    Active,
    Disabled,
}

impl PrincipalStatus {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            PrincipalStatus::Active => "active",
            PrincipalStatus::Disabled => "disabled",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "active" => Some(PrincipalStatus::Active),
            "disabled" => Some(PrincipalStatus::Disabled),
            _ => None,
        }
    }
}

/// The root identity. In M0 every principal is a `User` (see `crate::user::User`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Principal {
    pub id: PrincipalId,
    pub kind: PrincipalKind,
    pub status: PrincipalStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Principal {
    #[must_use]
    pub fn new(id: PrincipalId, kind: PrincipalKind, status: PrincipalStatus, created_at: DateTime<Utc>, updated_at: DateTime<Utc>) -> Self {
        Principal {
            id,
            kind,
            status,
            created_at,
            updated_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_and_status_round_trip_through_strings() {
        assert_eq!(PrincipalKind::parse(PrincipalKind::User.as_str()), Some(PrincipalKind::User));
        assert_eq!(PrincipalStatus::parse(PrincipalStatus::Active.as_str()), Some(PrincipalStatus::Active));
        assert_eq!(PrincipalKind::parse("nope"), None);
    }

    #[test]
    fn service_account_kind_roundtrips() {
        assert_eq!(PrincipalKind::parse("service_account"), Some(PrincipalKind::ServiceAccount));
        assert_eq!(PrincipalKind::ServiceAccount.as_str(), "service_account");
    }

    #[test]
    fn disabled_status_roundtrips() {
        assert_eq!(PrincipalStatus::parse("disabled"), Some(PrincipalStatus::Disabled));
        assert_eq!(PrincipalStatus::Disabled.as_str(), "disabled");
    }
}
