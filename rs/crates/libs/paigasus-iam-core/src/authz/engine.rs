// SPDX-License-Identifier: Apache-2.0

//! The pure Cedar decision engine (ADR-0013): compiles stored [`PolicyDocument`]s +
//! [`RoleGrant`]s into a `cedar_policy::PolicySet`, and decides individual
//! [`AccessRequest`]s against an [`EntitySlice`]. No I/O, no clock, no RNG — every
//! input is a value the caller already has in hand.
//!
//! **Role/template linkage convention:** [`PolicyEngine::compile`] only receives
//! `&[PolicyDocument]` and `&[RoleGrant]` — no `Role` catalog — so it resolves which
//! template a grant links against by treating [`RoleGrant::role_key`] as the template's
//! `policy_id`. Whichever later task authors the system roles / starter policies (the
//! plan's "roles.rs") **must** give each role's Cedar template document a `policy_id`
//! equal to that role's `key` for grants to link correctly.

use super::model::{AccessRequest, AuthzError, ContextValue, Decision, Effect, EntitySlice, GrantScope, PolicyDocument, PolicyKind, RoleGrant, SliceEntity, root_prn};
use super::schema::schema;
use crate::tenancy::TenancyNodeRef;
use cedar_policy::entities_errors::EntitiesError;
use cedar_policy::{
    Authorizer, Context, Decision as CedarDecision, Entities, Entity, EntityId, EntityTypeName, EntityUid, ParseErrors, Policy, PolicyId, PolicySet, Request, RestrictedExpression, SlotId, Template,
};
use paigasus_kernel::to_cedar_uid;
use std::collections::{HashMap, HashSet};
use std::str::FromStr;

/// `determining_policies` marker when Cedar denies with an empty reason set — no policy
/// matched at all ("default deny").
pub const DEFAULT_DENY_MARKER: &str = "default-deny (no matching permit)";

/// `determining_policies` marker when building the Cedar request/entities fails, or the
/// authorizer itself surfaces an evaluation error. [`PolicyEngine::decide`] is pure and
/// never returns `Err`; this marker is how it reports the failure to the caller, who logs
/// it (this module does not).
const EVALUATION_ERROR_MARKER: &str = "evaluation-error";

/// A compiled, ready-to-evaluate policy set: every static policy, every template, and
/// every grant's template-linked policy, all folded into one `cedar_policy::PolicySet`.
/// `gen` is the storage generation this was compiled from — [`PolicyEngine::compile`]
/// always sets it to `0`; the policy-snapshot cache (a later M3 task) overwrites it with
/// the real generation counter after compiling from a live store read.
#[derive(Debug)]
pub struct CompiledPolicies {
    pub policy_set: PolicySet,
    /// `r#gen` — `gen` is a reserved keyword as of the 2024 edition.
    pub r#gen: u64,
    /// A blake3 hex digest over a canonical encoding of the documents + grants this was
    /// compiled from (SMA-470 D4). Unlike [`Self::r#gen`] — which is a Redis-sourced counter
    /// that can stall, reset to 0, or miss a swallowed bump — this is a pure function of the
    /// compiled content, so it is identical across replicas that compiled the same policy set
    /// and always changes when the policy set does. It is the decision cache key's policy
    /// component; `r#gen` is only the reload-freshness comparator.
    pub content_hash: String,
}

/// Namespace for the pure Cedar engine operations. Never constructed — every method is
/// an associated function.
pub struct PolicyEngine;

impl PolicyEngine {
    /// Compile a snapshot of stored policies/templates + role grants into a
    /// [`CompiledPolicies`]: every [`PolicyKind::Static`] document is parsed and added as
    /// a static policy; every [`PolicyKind::Template`] document is parsed and added as a
    /// template; then every grant whose role template is present in `policies` is linked
    /// via [`link_grant`] (a grant naming an absent template is silently skipped — it
    /// contributes no permission, which is the fail-safe outcome).
    ///
    /// # Errors
    /// [`AuthzError::PolicyParse`] if any document fails to parse (or is the wrong kind —
    /// e.g. a `Static` document containing template slots). [`AuthzError::TemplateLink`]
    /// if a present template fails to link for a grant (e.g. a malformed scope/principal).
    pub fn compile(policies: &[PolicyDocument], grants: &[RoleGrant]) -> Result<CompiledPolicies, AuthzError> {
        let mut policy_set = PolicySet::new();

        for doc in policies {
            let id = PolicyId::new(&doc.policy_id);
            match doc.kind {
                PolicyKind::Static => {
                    let policy = Policy::parse(Some(id), &doc.source).map_err(|e| AuthzError::PolicyParse(e.to_string()))?;
                    policy_set.add(policy).map_err(|e| AuthzError::PolicyParse(e.to_string()))?;
                }
                PolicyKind::Template => {
                    let template = Template::parse(Some(id), &doc.source).map_err(|e| AuthzError::PolicyParse(e.to_string()))?;
                    policy_set.add_template(template).map_err(|e| AuthzError::PolicyParse(e.to_string()))?;
                }
            }
        }

        for grant in grants {
            let template_id = PolicyId::new(&grant.role_key);
            if policy_set.template(&template_id).is_some() {
                link_grant(&mut policy_set, &grant.role_key, grant)?;
            }
        }

        Ok(CompiledPolicies {
            policy_set,
            r#gen: 0,
            content_hash: content_hash(policies, grants),
        })
    }

    /// Decide one [`AccessRequest`] against `policies`, given the [`EntitySlice`] needed
    /// to evaluate it (principal, resource, its ancestor chain, and the synthetic
    /// `Root`). Pure and infallible: any failure building the Cedar request/entities, or
    /// an evaluation error surfaced by the authorizer, maps to `Effect::Deny` with the
    /// `evaluation-error` marker rather than an `Err` — see the module docs.
    #[must_use]
    pub fn decide(policies: &PolicySet, slice: &EntitySlice, req: &AccessRequest) -> Decision {
        Self::try_decide(policies, slice, req).unwrap_or_else(|_| Decision {
            effect: Effect::Deny,
            determining_policies: vec![EVALUATION_ERROR_MARKER.to_string()],
        })
    }

    /// The fallible core of [`Self::decide`], kept separate so every Cedar construction
    /// step can use `?` and the outer function stays a one-line `unwrap_or_else`.
    fn try_decide(policies: &PolicySet, slice: &EntitySlice, req: &AccessRequest) -> Result<Decision, DecisionBuildError> {
        let built = slice.entities.iter().map(build_entity).collect::<Result<Vec<_>, _>>()?;
        let entities = Entities::from_entities(built, Some(schema()))?;

        let principal_cedar = to_cedar_uid(&req.principal);
        let principal_uid = entity_uid(&principal_cedar.entity_type, &principal_cedar.entity_id)?;
        let resource_cedar = to_cedar_uid(&req.resource);
        let resource_uid = entity_uid(&resource_cedar.entity_type, &resource_cedar.entity_id)?;

        let context_pairs = req.context.0.iter().map(|(k, v)| (k.clone(), restricted_expression(v)));
        let context = Context::from_pairs(context_pairs)?;

        let request = Request::new(principal_uid, req.action.cedar_uid(), resource_uid, context, Some(schema()))?;

        let response = Authorizer::new().is_authorized(&request, policies, &entities);

        if response.diagnostics().errors().next().is_some() {
            return Ok(Decision {
                effect: Effect::Deny,
                determining_policies: vec![EVALUATION_ERROR_MARKER.to_string()],
            });
        }

        let mut reasons: Vec<String> = response.diagnostics().reason().map(ToString::to_string).collect();
        reasons.sort_unstable();

        Ok(match response.decision() {
            CedarDecision::Allow => Decision {
                effect: Effect::Allow,
                determining_policies: reasons,
            },
            CedarDecision::Deny if reasons.is_empty() => Decision {
                effect: Effect::Deny,
                determining_policies: vec![DEFAULT_DENY_MARKER.to_string()],
            },
            CedarDecision::Deny => Decision {
                effect: Effect::Deny,
                determining_policies: reasons,
            },
        })
    }
}

/// Link `template_id` into a concrete `grant:<uuid>` policy for `grant`, binding
/// `?principal` to the grantee (via [`to_cedar_uid`] on [`RoleGrant::principal`]) and
/// `?resource` to the grant's [`GrantScope`] — both routed uniformly through
/// [`to_cedar_uid`] (see [`scope_entity_uid`]; `GrantScope::Root` uses [`root_prn`], not a
/// special case).
///
/// # Errors
/// [`AuthzError::TemplateLink`] if the principal/scope can't be turned into a Cedar
/// `EntityUid`, or if `cedar_policy::PolicySet::link` itself rejects the link (e.g.
/// `template_id` doesn't name a template, or `grant:<uuid>` is already used).
pub fn link_grant(pset: &mut PolicySet, template_id: &str, grant: &RoleGrant) -> Result<(), AuthzError> {
    let principal_cedar = to_cedar_uid(grant.principal.prn());
    let principal_uid = entity_uid(&principal_cedar.entity_type, &principal_cedar.entity_id).map_err(|e| AuthzError::TemplateLink(e.to_string()))?;
    let resource_uid = scope_entity_uid(&grant.scope).map_err(|e| AuthzError::TemplateLink(e.to_string()))?;

    let vals: HashMap<SlotId, EntityUid> = HashMap::from([(SlotId::principal(), principal_uid), (SlotId::resource(), resource_uid)]);
    let new_id = PolicyId::new(format!("grant:{}", grant.id));

    pset.link(PolicyId::new(template_id), new_id, vals).map_err(|e| AuthzError::TemplateLink(e.to_string()))
}

/// Canonical, order-independent blake3 digest of the inputs a [`CompiledPolicies`] was built
/// from (SMA-470 D4). Both slices are hashed through SORTED, length-prefixed field encodings
/// so the digest is independent of `list_all`'s row order and cannot be forged by a field
/// value that happens to contain the delimiter: every field is individually length-prefixed
/// (not joined into a delimited string first), so there is no shared separator for an
/// attacker-controlled `policy_id`/`role_key` to smuggle and shift field boundaries with —
/// decoding is unambiguous about where each field starts and ends regardless of its
/// contents.
///
/// `created_at` is deliberately excluded from both encodings: it never affects the compiled
/// policy set, so including it would mint a fresh decision-cache key space for a semantically
/// identical policy set (and make the digest non-reproducible across replicas that re-read
/// rows with differing timestamp precision).
fn content_hash(policies: &[PolicyDocument], grants: &[RoleGrant]) -> String {
    fn field(hasher: &mut blake3::Hasher, value: &str) {
        // Length-prefix every field so ("ab", "c") and ("a", "bc") cannot collide.
        hasher.update(&(value.len() as u64).to_le_bytes());
        hasher.update(value.as_bytes());
    }

    // Each row is a fixed-arity array of its OWN fields (never pre-joined into a delimited
    // string), so `Vec<[String; N]>::sort_unstable` gives the canonical field-by-field
    // lexicographic order and hashing loops over `field()` per element below.
    let mut doc_rows: Vec<[String; 3]> = policies
        .iter()
        .map(|d| {
            let kind = match d.kind {
                PolicyKind::Static => "static",
                PolicyKind::Template => "template",
            };
            [d.policy_id.clone(), kind.to_string(), d.source.clone()]
        })
        .collect();
    doc_rows.sort_unstable();

    let mut grant_rows: Vec<[String; 5]> = grants
        .iter()
        .map(|g| {
            [
                g.id.to_string(),
                g.principal.uuid().to_string(),
                g.role_key.clone(),
                g.scope.canonical_prn(),
                g.linked_policy_id.clone(),
            ]
        })
        .collect();
    grant_rows.sort_unstable();

    let mut hasher = blake3::Hasher::new();
    field(&mut hasher, "policies");
    hasher.update(&(doc_rows.len() as u64).to_le_bytes());
    for row in &doc_rows {
        for f in row {
            field(&mut hasher, f);
        }
    }
    field(&mut hasher, "grants");
    hasher.update(&(grant_rows.len() as u64).to_le_bytes());
    for row in &grant_rows {
        for f in row {
            field(&mut hasher, f);
        }
    }
    hasher.finalize().to_hex().to_string()
}

/// Every fallible step of turning an [`EntitySlice`] + [`AccessRequest`] into a Cedar
/// `Request`/`Entities` pair, unified so [`PolicyEngine::try_decide`] can use `?`
/// throughout. Never surfaced to callers of [`PolicyEngine::decide`] — every variant maps
/// to the same `evaluation-error` deny.
///
/// Every source error is boxed: `cedar_policy`'s own error types run into the hundreds of
/// bytes (`clippy::result_large_err`), and this type is only ever inspected for its
/// variant (never its payload), so there's no cost to indirecting it.
#[derive(Debug, thiserror::Error)]
enum DecisionBuildError {
    #[error(transparent)]
    EntityType(Box<ParseErrors>),
    #[error(transparent)]
    EntityAttrs(Box<cedar_policy::EntityAttrEvaluationError>),
    #[error(transparent)]
    Entities(Box<EntitiesError>),
    #[error(transparent)]
    Context(Box<cedar_policy::ContextCreationError>),
    #[error(transparent)]
    Request(Box<cedar_policy::RequestValidationError>),
}

impl From<Box<ParseErrors>> for DecisionBuildError {
    fn from(e: Box<ParseErrors>) -> Self {
        Self::EntityType(e)
    }
}
impl From<cedar_policy::EntityAttrEvaluationError> for DecisionBuildError {
    fn from(e: cedar_policy::EntityAttrEvaluationError) -> Self {
        Self::EntityAttrs(Box::new(e))
    }
}
impl From<EntitiesError> for DecisionBuildError {
    fn from(e: EntitiesError) -> Self {
        Self::Entities(Box::new(e))
    }
}
impl From<cedar_policy::ContextCreationError> for DecisionBuildError {
    fn from(e: cedar_policy::ContextCreationError) -> Self {
        Self::Context(Box::new(e))
    }
}
impl From<cedar_policy::RequestValidationError> for DecisionBuildError {
    fn from(e: cedar_policy::RequestValidationError) -> Self {
        Self::Request(Box::new(e))
    }
}

/// Build a Cedar `Entity` from a [`SliceEntity`]: its uid, parent uids, and attrs (each
/// [`ContextValue`] becomes a `RestrictedExpression`).
fn build_entity(se: &SliceEntity) -> Result<Entity, DecisionBuildError> {
    let uid = entity_uid(&se.uid.0, &se.uid.1)?;
    let parents = se.parents.iter().map(|(t, i)| entity_uid(t, i)).collect::<Result<HashSet<EntityUid>, _>>()?;
    let attrs: HashMap<String, RestrictedExpression> = se.attrs.iter().map(|(k, v)| (k.clone(), restricted_expression(v))).collect();
    Ok(Entity::new(uid, attrs, parents)?)
}

/// Build an `EntityUid` from typed components (not string concatenation + parsing, which
/// the `cedar-policy` API docs warn against — same rationale as `Action::cedar_uid`).
/// Boxes the (large) `ParseErrors` payload to satisfy `clippy::result_large_err`.
fn entity_uid(entity_type: &str, entity_id: &str) -> Result<EntityUid, Box<ParseErrors>> {
    let type_name = EntityTypeName::from_str(entity_type)?;
    Ok(EntityUid::from_type_name_and_id(type_name, EntityId::new(entity_id)))
}

/// The `EntityUid` for a [`GrantScope`]: [`root_prn`] for [`GrantScope::Root`], or the
/// node's own `Prn` for [`GrantScope::Node`] — both routed uniformly through
/// [`to_cedar_uid`], with no special-casing.
fn scope_entity_uid(scope: &GrantScope) -> Result<EntityUid, Box<ParseErrors>> {
    let prn = match scope {
        GrantScope::Root => root_prn(),
        GrantScope::Node(TenancyNodeRef::Organization(id)) => id.prn().clone(),
        GrantScope::Node(TenancyNodeRef::Team(id)) => id.prn().clone(),
        GrantScope::Node(TenancyNodeRef::Project(id)) => id.prn().clone(),
    };
    let cedar = to_cedar_uid(&prn);
    entity_uid(&cedar.entity_type, &cedar.entity_id)
}

/// Map a [`ContextValue`] to the `RestrictedExpression` Cedar needs for entity attrs and
/// request context.
fn restricted_expression(v: &ContextValue) -> RestrictedExpression {
    match v {
        ContextValue::Str(s) => RestrictedExpression::new_string(s.clone()),
        ContextValue::Long(n) => RestrictedExpression::new_long(*n),
        ContextValue::Bool(b) => RestrictedExpression::new_bool(*b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authz::action::Action;
    use crate::authz::model::{ROOT_ENTITY, RequestContext};
    use crate::tenancy::{OrganizationId, ProjectId, TeamId};
    use crate::value::PrincipalId;
    use chrono::Utc;
    use paigasus_kernel::Prn;
    use std::collections::BTreeMap;
    use uuid::Uuid;

    fn u(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    fn active_attrs() -> BTreeMap<String, ContextValue> {
        BTreeMap::from([("effective_status".to_string(), ContextValue::Str("active".to_string()))])
    }

    fn policy_doc(policy_id: &str, kind: PolicyKind, source: &str) -> PolicyDocument {
        let now = Utc::now();
        PolicyDocument {
            policy_id: policy_id.to_string(),
            kind,
            source: source.to_string(),
            description: "test fixture".to_string(),
            system: true,
            created_at: now,
            updated_at: now,
        }
    }

    fn role_grant(id: Uuid, principal: &PrincipalId, role_key: &str, scope: GrantScope) -> RoleGrant {
        RoleGrant {
            id,
            principal: principal.clone(),
            role_key: role_key.to_string(),
            scope,
            linked_policy_id: format!("grant:{id}"),
            created_at: Utc::now(),
        }
    }

    /// A `Root -> Organization(org) -> Team(team) -> Project(project)` slice, plus a
    /// `Principal(principal)` entity, with every node's `effective_status` set to
    /// `project_status` (org/team stay `"active"` — only the resource's own effective
    /// status matters for the `forbid-archived` scenario).
    fn slice(org: &OrganizationId, team: &TeamId, project: &ProjectId, principal_prn: &Prn, project_status: &str) -> EntitySlice {
        let org_uid = to_cedar_uid(org.prn());
        let team_uid = to_cedar_uid(team.prn());
        let project_uid = to_cedar_uid(project.prn());
        let principal_uid = to_cedar_uid(principal_prn);

        EntitySlice {
            entities: vec![
                SliceEntity {
                    uid: (ROOT_ENTITY.0.to_string(), ROOT_ENTITY.1.to_string()),
                    parents: vec![],
                    attrs: BTreeMap::new(),
                },
                SliceEntity {
                    uid: (org_uid.entity_type.clone(), org_uid.entity_id.clone()),
                    parents: vec![(ROOT_ENTITY.0.to_string(), ROOT_ENTITY.1.to_string())],
                    attrs: active_attrs(),
                },
                SliceEntity {
                    uid: (team_uid.entity_type.clone(), team_uid.entity_id.clone()),
                    parents: vec![(org_uid.entity_type.clone(), org_uid.entity_id.clone())],
                    attrs: active_attrs(),
                },
                SliceEntity {
                    uid: (project_uid.entity_type.clone(), project_uid.entity_id.clone()),
                    parents: vec![(team_uid.entity_type, team_uid.entity_id)],
                    attrs: BTreeMap::from([("effective_status".to_string(), ContextValue::Str(project_status.to_string()))]),
                },
                SliceEntity {
                    uid: (principal_uid.entity_type, principal_uid.entity_id),
                    parents: vec![],
                    attrs: BTreeMap::from([
                        ("kind".to_string(), ContextValue::Str("user".to_string())),
                        ("status".to_string(), ContextValue::Str("active".to_string())),
                    ]),
                },
            ],
        }
    }

    fn principal_prn(n: u128) -> Prn {
        Prn::build("iam", "", None, "principal", u(n)).expect("static test prn parts are valid")
    }

    #[test]
    fn allow_via_linked_org_admin_style_template() {
        let org = OrganizationId::from_uuid(u(1));
        let team = TeamId::from_parts(u(1), u(2));
        let project = ProjectId::from_parts(u(1), u(3));
        let p_prn = principal_prn(4);
        let principal = PrincipalId::from_prn(p_prn.clone());

        let template = policy_doc("org_admin", PolicyKind::Template, r#"permit(principal == ?principal, action, resource in ?resource);"#);
        let grant = role_grant(u(100), &principal, "org_admin", GrantScope::Node(TenancyNodeRef::Organization(org.clone())));

        let compiled = PolicyEngine::compile(&[template], std::slice::from_ref(&grant)).expect("compile succeeds");

        let s = slice(&org, &team, &project, &p_prn, "active");
        let req = AccessRequest {
            principal: p_prn,
            action: Action::CreateProject,
            resource: project.prn().clone(),
            context: RequestContext::empty(),
        };

        let decision = PolicyEngine::decide(&compiled.policy_set, &s, &req);

        assert_eq!(decision.effect, Effect::Allow);
        assert_eq!(decision.determining_policies, vec![format!("grant:{}", grant.id)]);
    }

    #[test]
    fn default_deny_when_no_policy_links_the_principal() {
        let org = OrganizationId::from_uuid(u(1));
        let team = TeamId::from_parts(u(1), u(2));
        let project = ProjectId::from_parts(u(1), u(3));
        let p_prn = principal_prn(4);

        let compiled = PolicyEngine::compile(&[], &[]).expect("compile succeeds with nothing to compile");

        let s = slice(&org, &team, &project, &p_prn, "active");
        let req = AccessRequest {
            principal: p_prn,
            action: Action::CreateProject,
            resource: project.prn().clone(),
            context: RequestContext::empty(),
        };

        let decision = PolicyEngine::decide(&compiled.policy_set, &s, &req);

        assert_eq!(decision.effect, Effect::Deny);
        assert_eq!(decision.determining_policies, vec![DEFAULT_DENY_MARKER.to_string()]);
    }

    #[test]
    fn forbid_archived_denies_even_an_org_admin_grant() {
        let org = OrganizationId::from_uuid(u(1));
        let team = TeamId::from_parts(u(1), u(2));
        let project = ProjectId::from_parts(u(1), u(3));
        let p_prn = principal_prn(4);
        let principal = PrincipalId::from_prn(p_prn.clone());

        let template = policy_doc("org_admin", PolicyKind::Template, r#"permit(principal == ?principal, action, resource in ?resource);"#);
        let forbid = policy_doc(
            "forbid-archived",
            PolicyKind::Static,
            r#"forbid(principal, action, resource) when { resource has effective_status && resource.effective_status == "archived" };"#,
        );
        let grant = role_grant(u(100), &principal, "org_admin", GrantScope::Node(TenancyNodeRef::Organization(org.clone())));

        let compiled = PolicyEngine::compile(&[template, forbid], &[grant]).expect("compile succeeds");

        let s = slice(&org, &team, &project, &p_prn, "archived");
        let req = AccessRequest {
            principal: p_prn,
            action: Action::RenameProject,
            resource: project.prn().clone(),
            context: RequestContext::empty(),
        };

        let decision = PolicyEngine::decide(&compiled.policy_set, &s, &req);

        assert_eq!(decision.effect, Effect::Deny);
        assert_eq!(decision.determining_policies, vec!["forbid-archived".to_string()]);
    }

    /// SMA-470 D4: the content hash must be a pure function of the compiled inputs, so two
    /// replicas compiling the same policy set produce the same decision-cache key space.
    #[test]
    fn content_hash_is_stable_for_identical_inputs() {
        let docs = vec![hash_fixture_template()];
        let grants = vec![hash_fixture_grant(Uuid::from_u128(1))];

        let a = PolicyEngine::compile(&docs, &grants).expect("compiles");
        let b = PolicyEngine::compile(&docs, &grants).expect("compiles");

        assert_eq!(a.content_hash, b.content_hash);
        assert_eq!(a.content_hash.len(), 64, "blake3 hex digest is 64 chars");
        assert!(a.content_hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    /// SMA-470 D4: input ORDER must not change the hash — `list_all` gives no ordering
    /// guarantee, and two replicas reading the same rows in different orders must still
    /// share a cache key space.
    #[test]
    fn content_hash_ignores_input_ordering() {
        let g1 = hash_fixture_grant(Uuid::from_u128(1));
        let g2 = hash_fixture_grant(Uuid::from_u128(2));

        let forward = PolicyEngine::compile(&[hash_fixture_template()], &[g1.clone(), g2.clone()]).expect("compiles");
        let reverse = PolicyEngine::compile(&[hash_fixture_template()], &[g2, g1]).expect("compiles");

        assert_eq!(forward.content_hash, reverse.content_hash);
    }

    /// SMA-470 D4: revoking a grant MUST change the hash — this is what moves the decision
    /// cache to a fresh key space and makes a lost `policy_gen` bump irrelevant to the cache.
    #[test]
    fn content_hash_changes_when_a_grant_is_revoked() {
        let docs = vec![hash_fixture_template()];
        let with_grant = PolicyEngine::compile(&docs, &[hash_fixture_grant(Uuid::from_u128(1))]).expect("compiles");
        let without = PolicyEngine::compile(&docs, &[]).expect("compiles");

        assert_ne!(with_grant.content_hash, without.content_hash);
    }

    /// SMA-470 D4: editing a policy's Cedar source must change the hash even though the
    /// policy id is unchanged.
    #[test]
    fn content_hash_changes_when_a_policy_source_changes() {
        let original = PolicyEngine::compile(&[hash_fixture_template()], &[]).expect("compiles");

        let mut edited_doc = hash_fixture_template();
        edited_doc.source = r#"permit(principal == ?principal, action, resource in ?resource) when { true };"#.to_string();
        let edited = PolicyEngine::compile(&[edited_doc], &[]).expect("compiles");

        assert_ne!(original.content_hash, edited.content_hash);
    }

    /// SMA-470: two DIFFERENT documents must never hash alike just because a field value
    /// contains the row delimiter. `policy_id` and `role_key` are arbitrary caller-chosen
    /// strings with no charset validation, so an unescaped join would let an attacker craft a
    /// policy edit that does NOT rotate the decision-cache key — silently serving stale
    /// authorization decisions. Every field is length-prefixed independently, so the encoding
    /// is unambiguous about where each field ends.
    ///
    /// Exercises `content_hash` directly rather than through `PolicyEngine::compile`: the
    /// crafted field values below aren't valid Cedar template source, so both sides would
    /// fail to parse — and `AuthzError` has no `PartialEq`, so `Result<String, AuthzError>`
    /// doesn't either, meaning `assert_ne!` on the `compile(..).map(..)` results wouldn't even
    /// compile, let alone exercise the encoding this test pins.
    ///
    /// `kind` is pinned to [`PolicyKind::Static`] (overriding `hash_fixture_template`'s
    /// `Template`), not incidentally: the pre-fix row format was
    /// `policy_id + DELIM + kind + DELIM + source`, so the crafted values below only
    /// reproduce the collision (identical row bytes despite different documents) when the
    /// embedded `"static"` literal lines up with the actual `kind` field's row position —
    /// i.e. when `kind == "static"`. Verified empirically against the pre-fix encoding: with
    /// `kind = "static"` the two rows are byte-identical; with `kind = "template"` they are
    /// not, which would make this test pass even against the bug it's meant to catch.
    #[test]
    fn content_hash_is_unambiguous_across_field_boundaries() {
        let mut shifted_into_source = hash_fixture_template();
        shifted_into_source.kind = PolicyKind::Static;
        shifted_into_source.policy_id = "a".to_string();
        shifted_into_source.source = "b\u{1f}static\u{1f}c".to_string();

        let mut shifted_into_id = hash_fixture_template();
        shifted_into_id.kind = PolicyKind::Static;
        shifted_into_id.policy_id = "a\u{1f}static\u{1f}b".to_string();
        shifted_into_id.source = "c".to_string();

        assert_ne!(
            content_hash(&[shifted_into_source], &[]),
            content_hash(&[shifted_into_id], &[]),
            "a delimiter inside a field value must not forge another document's digest"
        );
    }

    fn hash_fixture_template() -> PolicyDocument {
        let now = chrono::Utc::now();
        PolicyDocument {
            policy_id: "org_admin".to_string(),
            kind: PolicyKind::Template,
            source: r#"permit(principal == ?principal, action, resource in ?resource);"#.to_string(),
            description: "test fixture".to_string(),
            system: true,
            created_at: now,
            updated_at: now,
        }
    }

    fn hash_fixture_grant(id: Uuid) -> RoleGrant {
        RoleGrant {
            id,
            principal: PrincipalId::from_prn(Prn::build("iam", "", None, "principal", Uuid::from_u128(9)).expect("static test prn parts are valid")),
            role_key: "org_admin".to_string(),
            scope: GrantScope::Root,
            linked_policy_id: format!("grant:{id}"),
            created_at: chrono::Utc::now(),
        }
    }
}
