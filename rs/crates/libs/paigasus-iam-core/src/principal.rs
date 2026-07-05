// SPDX-License-Identifier: Apache-2.0

//! The `Principal` root entity and its kind/status value enums.

use crate::value::PrincipalId;
use chrono::{DateTime, Utc};

/// Principal subtype. M0 mints only `User`; `ServiceAccount` arrives in a later milestone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrincipalKind {
    User,
}

impl PrincipalKind {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            PrincipalKind::User => "user",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "user" => Some(PrincipalKind::User),
            _ => None,
        }
    }
}

/// Principal lifecycle status. M0 only ever `Active`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrincipalStatus {
    Active,
}

impl PrincipalStatus {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            PrincipalStatus::Active => "active",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "active" => Some(PrincipalStatus::Active),
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
}
