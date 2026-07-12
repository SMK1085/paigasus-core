// SPDX-License-Identifier: Apache-2.0

//! Audit-log value types (SMA-446): the append-only record of security-relevant events
//! (authz denials in Slice A; committed mutations in Slice B). Pure/kernel-friendly — ids and
//! timestamps are injected by the caller (no `getrandom`, no ambient clock).
use chrono::{DateTime, Utc};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditOutcome {
    Committed,
    Denied,
}
impl AuditOutcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Committed => "committed",
            Self::Denied => "denied",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "committed" => Some(Self::Committed),
            "denied" => Some(Self::Denied),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AuditEntry {
    pub id: Uuid,
    pub occurred_at: DateTime<Utc>,
    pub actor_prn: Option<String>,
    pub action: String,
    pub resource_prn: Option<String>,
    pub outcome: AuditOutcome,
    pub determining_policies: Vec<String>,
    pub detail: serde_json::Value,
    pub correlation_id: Option<Uuid>,
}

#[derive(Debug, Clone)]
pub struct AuditFilter {
    pub actor_prn: Option<String>,
    pub resource_prn: Option<String>,
    pub action: Option<String>,
    pub outcome: Option<AuditOutcome>,
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
    pub cursor: Option<Uuid>,
    pub limit: u64,
}
impl AuditFilter {
    pub const MAX_LIMIT: u64 = 200;
    pub fn capped_limit(&self) -> u64 {
        self.limit.clamp(1, Self::MAX_LIMIT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn outcome_roundtrips_through_wire_strings() {
        for o in [AuditOutcome::Committed, AuditOutcome::Denied] {
            assert_eq!(AuditOutcome::parse(o.as_str()), Some(o));
        }
        assert_eq!(AuditOutcome::parse("nope"), None);
    }
    #[test]
    fn filter_limit_is_clamped_to_the_max_and_min() {
        let base = AuditFilter {
            actor_prn: None,
            resource_prn: None,
            action: None,
            outcome: None,
            from: None,
            to: None,
            cursor: None,
            limit: 0,
        };
        assert_eq!(AuditFilter { limit: 0, ..base.clone() }.capped_limit(), 1);
        assert_eq!(AuditFilter { limit: 10_000, ..base }.capped_limit(), AuditFilter::MAX_LIMIT);
    }
}
