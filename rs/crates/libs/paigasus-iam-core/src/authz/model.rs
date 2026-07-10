// SPDX-License-Identifier: Apache-2.0

//! Authorization domain model (ADR-0013): value objects, entities, and the `AuthzError`
//! error taxonomy shared by the Cedar engine, ports, stores, caches, and use cases (later
//! M3 tasks).

use super::action::Action;
use crate::tenancy::TenancyNodeRef;
use crate::value::PrincipalId;
use chrono::{DateTime, Utc};
use paigasus_kernel::Prn;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use uuid::Uuid;

/// The synthetic Cedar entity that sits above every organization (D4): `(entity_type,
/// entity_id)`, pinned to equal [`root_prn`]'s Cedar uid by a unit test below. Still
/// injected directly into every `EntitySlice` — there is no tenancy store row for it —
/// but as of [`root_prn`] it is also expressible as an ordinary `Prn`, so callers that
/// need a `Cedar` uid without building a `Prn` first (test fixtures, mostly) can use this
/// constant interchangeably with `to_cedar_uid(&root_prn())`.
pub const ROOT_ENTITY: (&str, &str) = ("Pgs::Iam::Root", "00000000-0000-0000-0000-000000000000");

/// The canonical sentinel `Prn` for the synthetic Cedar `Root` entity (D4): a well-known
/// nil-UUID `Prn` (`resource_type = "root"`) rather than a non-`Prn` special case, so every
/// PRN-shaped surface (`AccessRequest::resource`, wire `resource_prn`, `GrantScope`) can
/// express Root uniformly through `paigasus_kernel::to_cedar_uid` — no special-casing
/// anywhere. Its Cedar uid is pinned to equal [`ROOT_ENTITY`] by a unit test below.
#[must_use]
pub fn root_prn() -> Prn {
    Prn::build("iam", "", None, "root", Uuid::nil()).expect("root sentinel prn parts are valid")
}

/// The outcome of an authorization decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Effect {
    Allow,
    Deny,
}

/// An authorization decision: the [`Effect`] plus the id(s) of the policy/policies that
/// determined it (Cedar diagnostics, or a synthetic marker for default-deny / evaluation
/// errors — see `authz::engine`). `Serialize`/`Deserialize` so adapters can round-trip a
/// `Decision` through an external cache payload (`adapters::authz::decision_cache`, SMA-444
/// Task 14) without a bespoke wire format.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decision {
    pub effect: Effect,
    pub determining_policies: Vec<String>,
}

/// A Cedar request-context attribute value. Deliberately minimal — only the primitive
/// kinds the embedded schema's actions currently need.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContextValue {
    Str(String),
    Long(i64),
    Bool(bool),
}

/// Extra attributes for an [`AccessRequest`], beyond principal/action/resource. A
/// `BTreeMap` for deterministic ordering (stable test/log output, and a stable byte
/// sequence when serialized as part of `adapters::authz::decision_cache::decision_key`'s
/// hash input).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RequestContext(pub BTreeMap<String, ContextValue>);

impl RequestContext {
    /// An empty request context (the common case — most actions carry none).
    #[must_use]
    pub fn empty() -> Self {
        Self(BTreeMap::new())
    }
}

/// A single authorization question: "can `principal` perform `action` on `resource`,
/// given `context`?"
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessRequest {
    pub principal: Prn,
    pub action: Action,
    pub resource: Prn,
    pub context: RequestContext,
}

/// The kind of node a [`GrantScope`] or [`Role`] targets. `Root` is the synthetic
/// platform-wide scope (D4); the rest mirror [`TenancyNodeRef`]'s variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    Root,
    Organization,
    Team,
    Project,
}

/// Where a [`RoleGrant`] applies: the synthetic platform [`Root`](GrantScope::Root), or a
/// concrete tenancy node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GrantScope {
    Root,
    Node(TenancyNodeRef),
}

impl GrantScope {
    /// The scope's canonical identity string: [`root_prn`]'s canonical PRN string for
    /// [`GrantScope::Root`], or the node's canonical PRN — delegates to
    /// [`TenancyNodeRef::canonical`] for [`GrantScope::Node`].
    #[must_use]
    pub fn canonical_prn(&self) -> String {
        match self {
            GrantScope::Root => root_prn().canonical(),
            GrantScope::Node(node) => node.canonical(),
        }
    }

    /// The scope's [`NodeKind`].
    #[must_use]
    pub fn kind(&self) -> NodeKind {
        match self {
            GrantScope::Root => NodeKind::Root,
            GrantScope::Node(TenancyNodeRef::Organization(_)) => NodeKind::Organization,
            GrantScope::Node(TenancyNodeRef::Team(_)) => NodeKind::Team,
            GrantScope::Node(TenancyNodeRef::Project(_)) => NodeKind::Project,
        }
    }
}

/// A role definition: a stable `key`, the Cedar policy template it links (`template_id`),
/// the node kinds it may be granted at, a human `description`, and whether it's a
/// system-seeded role (immutable via the policy/role CRUD API).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Role {
    pub key: String,
    pub template_id: String,
    pub scope_kinds: Vec<NodeKind>,
    pub description: String,
    pub system: bool,
}

/// A materialized grant of a [`Role`] to a principal at a [`GrantScope`], linked to a
/// Cedar template-linked policy (`linked_policy_id`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleGrant {
    pub id: Uuid,
    pub principal: PrincipalId,
    pub role_key: String,
    pub scope: GrantScope,
    pub linked_policy_id: String,
    pub created_at: DateTime<Utc>,
}

/// A lightweight, wire-friendly reference to a [`RoleGrant`]'s scope + role — the scope's
/// canonical identity string (a tenancy PRN, or [`root_prn`]'s canonical PRN for `Root`)
/// plus the role key, without the grant's own id/timestamps/linked-policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleGrantRef {
    pub scope_prn: String,
    pub role_key: String,
}

/// Whether a [`PolicyDocument`] is a standalone static policy or a Cedar policy template
/// (linked per-grant, see `authz::engine::link_grant`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyKind {
    Static,
    Template,
}

/// An authored Cedar policy (or template), as stored/served by the policy CRUD API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyDocument {
    pub policy_id: String,
    pub kind: PolicyKind,
    pub source: String,
    pub description: String,
    pub system: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// One entity in an [`EntitySlice`]: its Cedar uid (`(entity_type, entity_id)`), parent
/// uids (membership-hierarchy edges), and attributes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SliceEntity {
    pub uid: (String, String),
    pub parents: Vec<(String, String)>,
    pub attrs: BTreeMap<String, ContextValue>,
}

/// The minimal set of Cedar entities (principal, resource, ancestor chain, synthetic
/// `Root`) needed to decide one [`AccessRequest`]. `Serialize`/`Deserialize` so the
/// entity-slice cache (`adapters::authz::entity_cache`, SMA-444 Task 14) can round-trip a
/// slice through Redis.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct EntitySlice {
    pub entities: Vec<SliceEntity>,
}

/// An audit record of a single authorization decision (logged by callers, not the engine
/// itself — see `authz::engine::PolicyEngine::decide`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthzDecisionEvent {
    pub principal_prn: String,
    pub action: String,
    pub resource_prn: String,
    pub effect: Effect,
    pub determining_policies: Vec<String>,
    pub at: DateTime<Utc>,
}

/// The authorization error taxonomy — shared by the Cedar engine, ports, stores, caches,
/// and use cases (later M3 tasks).
#[derive(Debug, thiserror::Error)]
pub enum AuthzError {
    #[error("policy failed to parse: {0}")]
    PolicyParse(String),
    #[error("policy failed schema validation: {0}")]
    SchemaValidation(String),
    #[error("template link failed: {0}")]
    TemplateLink(String),
    #[error("policy evaluation failed: {0}")]
    Evaluation(String),
    #[error("unknown role: {0}")]
    UnknownRole(String),
    #[error("invalid grant scope: {0}")]
    InvalidScope(String),
    #[error("system-owned resource is immutable: {0}")]
    SystemImmutable(String),
    /// A tenancy node an [`EntitySliceLoader`](super::ports::EntitySliceLoader) needed to
    /// slice — typically the request's resource, sometimes an ancestor — does not exist.
    /// Surfaced as its own variant, distinct from [`Self::Backend`], so `CedarAuthorizer` can
    /// tell "the resource is genuinely missing" apart from "the backend broke" and fail
    /// CLOSED as a `Deny` for the former instead of a 500 (SMA-444 review fix) — never a
    /// silent existence oracle, and never a false deny for a real outage.
    #[error("resource not found: {0}")]
    ResourceNotFound(String),
    /// A mutation lost a concurrent-create race against a DIFFERENT document for the same id
    /// (e.g. `PolicyStore::put`, SMA-444 review fix): the caller's write was NOT applied — the
    /// stored row belongs to the race's winner, not this caller.
    #[error("conflict: {0}")]
    Conflict(String),
    /// A backend (storage/transport) failure. `#[error(transparent)]` forwards `Display`
    /// (and `source()`) to the wrapped error verbatim — callers never see more than what
    /// the underlying source already exposes.
    #[error(transparent)]
    Backend(#[from] Box<dyn std::error::Error + Send + Sync>),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tenancy::OrganizationId;

    fn u(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    #[test]
    fn role_grant_ref_round_trips_its_fields() {
        let r = RoleGrantRef {
            scope_prn: "prn:pgs:iam:::organization/00000000-0000-7000-8000-000000000001".to_string(),
            role_key: "org_admin".to_string(),
        };
        let round_tripped = RoleGrantRef {
            scope_prn: r.scope_prn.clone(),
            role_key: r.role_key.clone(),
        };
        assert_eq!(r, round_tripped);
    }

    #[test]
    fn grant_scope_canonical_prn_for_root_is_the_sentinel_prns_canonical_string() {
        assert_eq!(GrantScope::Root.canonical_prn(), root_prn().canonical());
        assert_eq!(GrantScope::Root.kind(), NodeKind::Root);
    }

    /// Pins the two Root representations together: [`root_prn`]'s Cedar uid (via
    /// `paigasus_kernel::to_cedar_uid`) must equal [`ROOT_ENTITY`] exactly, so every call
    /// site free to choose either one gets the identical Cedar entity.
    #[test]
    fn to_cedar_uid_of_root_prn_matches_root_entity_constant() {
        let uid = paigasus_kernel::to_cedar_uid(&root_prn());
        assert_eq!(uid.entity_type, ROOT_ENTITY.0);
        assert_eq!(uid.entity_id, ROOT_ENTITY.1);
    }

    #[test]
    fn grant_scope_canonical_prn_for_node_delegates_to_tenancy_node_ref() {
        let org = TenancyNodeRef::Organization(OrganizationId::from_uuid(u(1)));
        let scope = GrantScope::Node(org.clone());
        assert_eq!(scope.canonical_prn(), org.canonical());
        assert_eq!(scope.kind(), NodeKind::Organization);
    }

    #[test]
    fn request_context_empty_has_no_entries() {
        assert!(RequestContext::empty().0.is_empty());
    }

    #[test]
    fn authz_error_display_is_non_empty_for_every_string_variant() {
        let variants = [
            AuthzError::PolicyParse("x".to_string()),
            AuthzError::SchemaValidation("x".to_string()),
            AuthzError::TemplateLink("x".to_string()),
            AuthzError::Evaluation("x".to_string()),
            AuthzError::UnknownRole("x".to_string()),
            AuthzError::InvalidScope("x".to_string()),
            AuthzError::SystemImmutable("x".to_string()),
            AuthzError::ResourceNotFound("x".to_string()),
            AuthzError::Conflict("x".to_string()),
        ];
        for v in &variants {
            assert!(!v.to_string().is_empty(), "{v:?}");
        }
    }

    #[test]
    fn authz_error_backend_display_is_exactly_the_wrapped_source_no_more_no_less() {
        let source: Box<dyn std::error::Error + Send + Sync> = "boom".into();
        let err = AuthzError::Backend(source);
        assert_eq!(err.to_string(), "boom");
    }
}
