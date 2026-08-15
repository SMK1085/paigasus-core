// SPDX-License-Identifier: Apache-2.0

//! Domain-event value types (SMA-446, Slice B): the payload written to the transactional
//! outbox (`Outbox::enqueue`) in the same transaction as its triggering mutation, later
//! drained by the relay and handed to an `EventPublisher`. Pure value types, no I/O — ids,
//! timestamps and correlation are injected by the caller (ADR-0005).
use chrono::{DateTime, Utc};
use uuid::Uuid;

/// The kind of domain event recorded in the outbox. `as_wire`/`parse` give the stable wire
/// string persisted in the outbox row (and handed to external consumers) — renaming a variant
/// must not change its wire string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventType {
    PrincipalCreated,
    PrincipalArchived,
    RoleGranted,
    RoleRevoked,
    ApiKeyIssued,
    ApiKeyRevoked,
    PolicyPut,
    PolicyDeleted,
}

impl EventType {
    /// Every variant, in declaration order. Public because consumers outside this crate need to
    /// enumerate the event surface: `tests/nats_permissions.rs` (SMA-493) asserts the NATS
    /// publisher's `pub` grant covers every subject this service can emit, and an integration
    /// test cannot see a `#[cfg(test)]` constant. Kept exhaustive by
    /// `all_lists_every_event_type`, whose wildcard-free match stops compiling when a variant is
    /// added.
    pub const ALL: [EventType; 8] = [
        Self::PrincipalCreated,
        Self::PrincipalArchived,
        Self::RoleGranted,
        Self::RoleRevoked,
        Self::ApiKeyIssued,
        Self::ApiKeyRevoked,
        Self::PolicyPut,
        Self::PolicyDeleted,
    ];

    pub fn as_wire(&self) -> &'static str {
        match self {
            Self::PrincipalCreated => "iam.principal.created",
            Self::PrincipalArchived => "iam.principal.archived",
            Self::RoleGranted => "iam.role.granted",
            Self::RoleRevoked => "iam.role.revoked",
            Self::ApiKeyIssued => "iam.api_key.issued",
            Self::ApiKeyRevoked => "iam.api_key.revoked",
            Self::PolicyPut => "iam.policy.put",
            Self::PolicyDeleted => "iam.policy.deleted",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "iam.principal.created" => Some(Self::PrincipalCreated),
            "iam.principal.archived" => Some(Self::PrincipalArchived),
            "iam.role.granted" => Some(Self::RoleGranted),
            "iam.role.revoked" => Some(Self::RoleRevoked),
            "iam.api_key.issued" => Some(Self::ApiKeyIssued),
            "iam.api_key.revoked" => Some(Self::ApiKeyRevoked),
            "iam.policy.put" => Some(Self::PolicyPut),
            "iam.policy.deleted" => Some(Self::PolicyDeleted),
            _ => None,
        }
    }
}

/// One committed domain event, written to the outbox in the same transaction as the mutation
/// that produced it, then relayed to an [`EventPublisher`](crate::ports::EventPublisher) at
/// least once.
#[derive(Debug, Clone, PartialEq)]
pub struct DomainEvent {
    pub id: Uuid,
    pub event_type: EventType,
    pub schema_version: u16,
    pub aggregate_prn: String,
    pub actor_prn: Option<String>,
    pub occurred_at: DateTime<Utc>,
    pub payload: serde_json::Value,
    pub correlation_id: Option<Uuid>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_type_roundtrips_through_wire_strings() {
        for et in EventType::ALL {
            assert_eq!(EventType::parse(et.as_wire()), Some(et));
        }
        assert_eq!(EventType::parse("nope"), None);
    }

    #[test]
    fn wire_strings_are_namespaced_and_distinct() {
        let wires: Vec<&str> = EventType::ALL.iter().map(EventType::as_wire).collect();
        for w in &wires {
            assert!(w.starts_with("iam."), "unexpected wire string: {w}");
        }
        let mut sorted = wires.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), wires.len(), "duplicate wire string in {wires:?}");
    }

    /// `ALL` must stay exhaustive: this match has no wildcard arm, so a new variant fails to
    /// compile here rather than silently shrinking every consumer's coverage (SMA-493 §3.4 —
    /// `tests/nats_permissions.rs` iterates `ALL` to prove the publisher's grant covers every
    /// subject the service can emit).
    #[test]
    fn all_lists_every_event_type() {
        for et in EventType::ALL {
            match et {
                EventType::PrincipalCreated
                | EventType::PrincipalArchived
                | EventType::RoleGranted
                | EventType::RoleRevoked
                | EventType::ApiKeyIssued
                | EventType::ApiKeyRevoked
                | EventType::PolicyPut
                | EventType::PolicyDeleted => {}
            }
        }
        assert_eq!(EventType::ALL.len(), 8);
    }

    #[test]
    fn constructs_a_domain_event() {
        let id = Uuid::from_u128(1);
        let corr = Uuid::from_u128(2);
        let now = Utc::now();
        let ev = DomainEvent {
            id,
            event_type: EventType::RoleGranted,
            schema_version: 1,
            aggregate_prn: "prn:pgs:iam:::principal/0000".to_string(),
            actor_prn: Some("prn:pgs:iam:::principal/actor".to_string()),
            occurred_at: now,
            payload: serde_json::json!({"role_key": "org_admin"}),
            correlation_id: Some(corr),
        };
        assert_eq!(ev.id, id);
        assert_eq!(ev.event_type.as_wire(), "iam.role.granted");
        assert_eq!(ev.correlation_id, Some(corr));
    }
}
