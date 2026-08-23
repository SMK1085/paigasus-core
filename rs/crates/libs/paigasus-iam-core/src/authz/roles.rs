// SPDX-License-Identifier: Apache-2.0

//! Starter Cedar policy set + the eight system roles (ADR-0013, design §3.2): the base
//! `forbid-archived-writes` static policy, one policy template per system role, and the
//! `Role` catalog those templates back.
//!
//! **Template `policy_id` == role `key` (load-bearing):** [`super::engine::PolicyEngine::compile`]
//! resolves which template a [`super::model::RoleGrant`] links against by treating
//! `RoleGrant::role_key` as the template's `policy_id` — see that module's docs. So every
//! template [`PolicyDocument`] produced here has `policy_id == Role::key == Role::template_id`
//! for the role it backs; getting this wrong makes the engine silently skip the grant (no
//! error surfaces — it just contributes no permission).
//!
//! **Action-set allow-lists, not exclude-lists:** each non-`platform_admin` role's action set
//! is spelled out explicitly (the `*_ACTIONS` consts below) rather than derived as "every
//! action except the ones that don't apply" (e.g. "every action except the five Root-only
//! ones" for `org_admin`). An allow-list fails closed: a newly added `Action` variant grants
//! nothing to any role until someone deliberately adds it here. An exclude-list fails open: it
//! would silently hand the new action to every role unless whoever added the action remembered
//! to add it to every exclusion list too — backwards for an authorization boundary. Only
//! `forbid-archived-writes`'s action list is *derived* (`Action::ALL` filtered by
//! `Action::is_write` and then `!Action::is_restore`, per the design's explicit instruction,
//! with `Restore*` carved back out because restoring is the one legitimate write on an
//! archived node): that direction is safe because it's a `forbid`, so a missed action only
//! weakens the belt-and-braces guard — it can never over-grant.
//!
//! Action sets are derived from design §3.2's role table together with §9.4's action→resource
//! binding table, using Cedar's `resource in ?resource` semantics (true iff `?resource` is
//! `resource` itself or one of its Cedar-hierarchy ancestors). An action whose §9.4-authorized
//! resource can never be a descendant-or-self of a role's scope kind is excluded — e.g.
//! `CreateTeam`/`ListTeams` authorize against the *parent org*, which is never "in" a
//! `Team`-scoped role, so `team_admin`/`team_member` omit them; `CreateProject`/`ListProjects`
//! authorize against the *parent team*, which is never "in" a `Project`-scoped role, so
//! `project_admin`/`project_member` omit them.

use super::action::Action;
use super::model::{NodeKind, PolicyDocument, PolicyKind, Role};
use chrono::Utc;

/// The base static policy id (design §3.2).
pub const FORBID_ARCHIVED_WRITES_ID: &str = "forbid-archived-writes";

/// Bumped by hand whenever any starter policy's content changes — guarded by
/// `starter_policy_content_is_pinned_to_the_declared_revision` below, which reds until it is.
///
/// Persisted per row as `policy.starter_revision`, and compared on every boot: a replica whose
/// `STARTER_POLICY_REVISION` is LOWER than a stored row's leaves that row alone (SMA-477 D11).
/// There is one `policy` table for the whole fleet, so without this an older replica booting
/// mid-deploy — a rollback, a crashloop restart, an HPA scale-up, a held canary — would
/// rewrite the shared row to its own older policy set and, via the `policy_gen` bump, push it
/// onto every already-serving newer replica.
///
/// `CARGO_PKG_VERSION` cannot serve here: the crate is version `0.0.0`.
///
/// `2`: SMA-481 added the `RetireSystemPolicy` action, which — being a non-restore write —
/// joins `forbid-archived-writes`'s generated action list and so changes its `source`.
///
/// `3`: SMA-584 added `CreateUser` for the same reason. The forbid can never actually bite on
/// it (`entity Root;` declares no attributes, so `resource has effective_status` is
/// unsatisfiable at `Root`), but the action list is *derived*, not hand-written, so the
/// content moves and every deployed database now holds an older set.
pub const STARTER_POLICY_REVISION: u32 = 3;

/// Every `policy_id` [`starter_policies`] produces, in the order it produces them. A `const`
/// so the reserved-namespace check in `PolicyStore::put_in` is a slice scan rather than nine
/// `PolicyDocument` allocations per call; kept honest by
/// `starter_policy_ids_matches_what_starter_policies_actually_produces`.
pub const STARTER_POLICY_IDS: &[&str] = &[
    FORBID_ARCHIVED_WRITES_ID,
    PLATFORM_ADMIN_KEY,
    "org_admin",
    "org_member",
    "team_admin",
    "team_member",
    "project_admin",
    "project_member",
    "gateway_user",
];

/// Whether `id` is one of the code-owned starter policy ids. The public `PutPolicy` API
/// rejects these outright (SMA-477 D6): the ids are reserved even before they are seeded, so
/// an operator cannot occupy one and thereby exempt it from boot-time convergence.
#[must_use]
pub fn is_starter_policy_id(id: &str) -> bool {
    STARTER_POLICY_IDS.contains(&id)
}

/// The pinned content hash guarding [`STARTER_POLICY_REVISION`] — see the test that reads it.
#[cfg(test)]
const EXPECTED_STARTER_CONTENT_HASH: &str = "b116dc14f23bf3dc658b333d17e1a79e6da800859d8d3ec7dab28b2de0f84cd5";

/// `platform_admin`'s role key — also its template's `policy_id` (see module docs).
const PLATFORM_ADMIN_KEY: &str = "platform_admin";

/// `org_admin`: every action except the five Root-only ones (`CreateOrganization`,
/// `ListOrganizations`, `PutPolicy`, `DeletePolicy`, `ListPolicies` — design §3.2, D4).
/// Grantable at `Organization`; because of the Cedar `in`-hierarchy, a grant here also covers
/// every team/project under the org.
///
/// Includes the seven service-account/API-key management actions (design §3.2 D9): SA
/// ownership can be any tenancy node (D10), so — mirroring how `AttachMembership`/`GrantRole`
/// are distributed across every non-Root admin template rather than confined to one level like
/// `CreateTeam` — `org_admin` can manage SAs/keys owned anywhere in its own subtree via the
/// same `resource in ?resource` scoping.
const ORG_ADMIN_ACTIONS: &[Action] = &[
    Action::GetOrganization,
    Action::GetTeam,
    Action::ListTeams,
    Action::GetProject,
    Action::ListProjects,
    Action::ListMemberships,
    Action::RenameOrganization,
    Action::ArchiveOrganization,
    Action::RestoreOrganization,
    Action::CreateTeam,
    Action::RenameTeam,
    Action::ArchiveTeam,
    Action::RestoreTeam,
    Action::CreateProject,
    Action::RenameProject,
    Action::ArchiveProject,
    Action::RestoreProject,
    Action::AttachMembership,
    Action::DetachMembership,
    Action::GrantRole,
    Action::RevokeRole,
    Action::ListRoleGrants,
    Action::CreateServiceAccount,
    Action::GetServiceAccount,
    Action::ListServiceAccounts,
    Action::ArchiveServiceAccount,
    Action::IssueApiKey,
    Action::RevokeApiKey,
    Action::ListApiKeys,
];

/// `org_member`: org/team/project reads only (design §3.2) — no membership rosters, no grant
/// management.
const ORG_MEMBER_ACTIONS: &[Action] = &[Action::GetOrganization, Action::GetTeam, Action::ListTeams, Action::GetProject, Action::ListProjects];

/// `team_admin`: team + project writes/reads, membership, and grant management within the
/// team subtree (design §3.2). Also carries the seven service-account/API-key management
/// actions (design §3.2 D9/D10) — see [`ORG_ADMIN_ACTIONS`]'s doc for why they're distributed
/// like `AttachMembership`/`GrantRole` rather than confined to one level.
const TEAM_ADMIN_ACTIONS: &[Action] = &[
    Action::GetTeam,
    Action::GetProject,
    Action::ListProjects,
    Action::ListMemberships,
    Action::RenameTeam,
    Action::ArchiveTeam,
    Action::RestoreTeam,
    Action::CreateProject,
    Action::RenameProject,
    Action::ArchiveProject,
    Action::RestoreProject,
    Action::AttachMembership,
    Action::DetachMembership,
    Action::GrantRole,
    Action::RevokeRole,
    Action::ListRoleGrants,
    Action::CreateServiceAccount,
    Action::GetServiceAccount,
    Action::ListServiceAccounts,
    Action::ArchiveServiceAccount,
    Action::IssueApiKey,
    Action::RevokeApiKey,
    Action::ListApiKeys,
];

/// `team_member`: team/project reads only (design §3.2).
const TEAM_MEMBER_ACTIONS: &[Action] = &[Action::GetTeam, Action::GetProject, Action::ListProjects];

/// `project_admin`: project writes/reads, membership, and grant management within the project
/// itself (design §3.2) — `Project` is a leaf node, so no `Create*`/`List*` of children. Also
/// carries the seven service-account/API-key management actions (design §3.2 D9/D10) — see
/// [`ORG_ADMIN_ACTIONS`]'s doc for why they're distributed like `AttachMembership`/`GrantRole`
/// rather than confined to one level.
const PROJECT_ADMIN_ACTIONS: &[Action] = &[
    Action::GetProject,
    Action::RenameProject,
    Action::ArchiveProject,
    Action::RestoreProject,
    Action::ListMemberships,
    Action::AttachMembership,
    Action::DetachMembership,
    Action::GrantRole,
    Action::RevokeRole,
    Action::ListRoleGrants,
    Action::CreateServiceAccount,
    Action::GetServiceAccount,
    Action::ListServiceAccounts,
    Action::ArchiveServiceAccount,
    Action::IssueApiKey,
    Action::RevokeApiKey,
    Action::ListApiKeys,
];

/// `project_member`: project reads only (design §3.2).
const PROJECT_MEMBER_ACTIONS: &[Action] = &[Action::GetProject];

/// `gateway_user`: only [InvokeModel] on its scope subtree (SMA-446, D10 — a dedicated role so
/// a spend-capable action never dilutes the read-only `*_member` roles).
const GATEWAY_USER_ACTIONS: &[Action] = &[Action::InvokeModel];

/// The eight system roles (design §3.2), each `system = true` and immutable via the policy/role
/// CRUD API. Every `template_id` equals the role's own `key` (see module docs).
#[must_use]
pub fn system_roles() -> Vec<Role> {
    vec![
        Role {
            key: PLATFORM_ADMIN_KEY.to_string(),
            template_id: PLATFORM_ADMIN_KEY.to_string(),
            scope_kinds: vec![NodeKind::Root],
            description: "Full platform authority: every action, anywhere in the hierarchy.".to_string(),
            system: true,
        },
        Role {
            key: "org_admin".to_string(),
            template_id: "org_admin".to_string(),
            scope_kinds: vec![NodeKind::Organization],
            description: "Manage an organization and everything under it, including role grants within it.".to_string(),
            system: true,
        },
        Role {
            key: "org_member".to_string(),
            template_id: "org_member".to_string(),
            scope_kinds: vec![NodeKind::Organization],
            description: "Read an organization and everything under it.".to_string(),
            system: true,
        },
        Role {
            key: "team_admin".to_string(),
            template_id: "team_admin".to_string(),
            scope_kinds: vec![NodeKind::Team],
            description: "Manage a team and its projects, including role grants within it.".to_string(),
            system: true,
        },
        Role {
            key: "team_member".to_string(),
            template_id: "team_member".to_string(),
            scope_kinds: vec![NodeKind::Team],
            description: "Read a team and its projects.".to_string(),
            system: true,
        },
        Role {
            key: "project_admin".to_string(),
            template_id: "project_admin".to_string(),
            scope_kinds: vec![NodeKind::Project],
            description: "Manage a single project, including role grants within it.".to_string(),
            system: true,
        },
        Role {
            key: "project_member".to_string(),
            template_id: "project_member".to_string(),
            scope_kinds: vec![NodeKind::Project],
            description: "Read a single project.".to_string(),
            system: true,
        },
        Role {
            key: "gateway_user".to_string(),
            template_id: "gateway_user".to_string(),
            scope_kinds: vec![NodeKind::Organization, NodeKind::Team, NodeKind::Project],
            description: "Invoke models within a scope subtree (org/team/project).".to_string(),
            system: true,
        },
    ]
}

/// Look up a system role by its `key`.
#[must_use]
pub fn role(key: &str) -> Option<Role> {
    system_roles().into_iter().find(|r| r.key == key)
}

/// The starter Cedar policy set (design §3.2): the base `forbid-archived-writes` static
/// policy, plus one `template` per [`system_roles`] entry (`policy_id == Role::key`).
#[must_use]
pub fn starter_policies() -> Vec<PolicyDocument> {
    let now = Utc::now();
    let mut docs = vec![PolicyDocument {
        policy_id: FORBID_ARCHIVED_WRITES_ID.to_string(),
        kind: PolicyKind::Static,
        source: forbid_archived_writes_source(),
        description: "Forbid every write action, except Restore*, on a resource whose effective_status is archived (belt-and-braces over M1's in-txn guards). Restores are exempt: restoring is the one legitimate write on an archived node — its whole purpose — and forbidding it here would make an archived node permanently un-restorable; M1's in-txn guards remain the real gate on restore ordering/validity."
            .to_string(),
        system: true,
        created_at: now,
        updated_at: now,
    }];
    docs.extend(system_roles().into_iter().map(|r| PolicyDocument {
        policy_id: r.template_id.clone(),
        kind: PolicyKind::Template,
        source: template_source(&r.key),
        description: r.description.clone(),
        system: true,
        created_at: now,
        updated_at: now,
    }));
    docs
}

/// `forbid(principal, action in [<every write action except Restore*>], resource) when {
/// resource has effective_status && resource.effective_status == "archived" };` — the
/// action list is generated from [`Action::ALL`] filtered by [`Action::is_write`] and then
/// [`Action::is_restore`] so it can never drift from the action catalog (design §3.2).
/// `Restore*` actions are deliberately excluded: restoring is the one legitimate write on an
/// archived resource (that's the whole point of restoring one), so forbidding it here would
/// make an archived node permanently stuck — M1's in-txn guards remain the real gate on
/// whether a given restore is valid/ordered correctly.
fn forbid_archived_writes_source() -> String {
    let write_actions = Action::ALL.iter().copied().filter(|a| a.is_write() && !a.is_restore()).collect::<Vec<_>>();
    let actions = action_refs(&write_actions);
    format!(r#"forbid(principal, action in [{actions}], resource) when {{ resource has effective_status && resource.effective_status == "archived" }};"#)
}

/// The Cedar template source for a system role's `key`: `platform_admin` omits the `action in
/// [...]` clause entirely (every action); every other role lists its allow-listed actions
/// explicitly.
///
/// # Panics
/// If `key` doesn't name one of the eight roles this module defines. Every call site in this
/// module passes a key from [`system_roles`]'s own output, so this can never actually happen.
fn template_source(key: &str) -> String {
    if key == PLATFORM_ADMIN_KEY {
        return "permit(principal == ?principal, action, resource in ?resource);".to_string();
    }
    let actions = match key {
        "org_admin" => ORG_ADMIN_ACTIONS,
        "org_member" => ORG_MEMBER_ACTIONS,
        "team_admin" => TEAM_ADMIN_ACTIONS,
        "team_member" => TEAM_MEMBER_ACTIONS,
        "project_admin" => PROJECT_ADMIN_ACTIONS,
        "project_member" => PROJECT_MEMBER_ACTIONS,
        "gateway_user" => GATEWAY_USER_ACTIONS,
        other => panic!("template_source: unknown system role key {other:?}"),
    };
    format!("permit(principal == ?principal, action in [{}], resource in ?resource);", action_refs(actions))
}

/// Render a comma-joined `Pgs::Iam::Action::"A", Pgs::Iam::Action::"B"` list for an `action in
/// [...]` clause.
fn action_refs(actions: &[Action]) -> String {
    actions.iter().map(|a| format!(r#"Pgs::Iam::Action::"{}""#, a.as_wire())).collect::<Vec<_>>().join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authz::engine::PolicyEngine;
    use crate::authz::model::{AccessRequest, ContextValue, Decision, Effect, EntitySlice, GrantScope, ROOT_ENTITY, RequestContext, RoleGrant, SliceEntity, root_prn};
    use crate::authz::schema::validate_policy;
    use crate::tenancy::{OrganizationId, ProjectId, TeamId, TenancyNodeRef};
    use crate::value::PrincipalId;
    use paigasus_kernel::{Prn, to_cedar_uid};
    use std::collections::BTreeMap;
    use uuid::Uuid;

    fn u(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    fn principal_prn(n: u128) -> Prn {
        Prn::build("iam", "", None, "principal", u(n)).expect("static test prn parts are valid")
    }

    fn cedar_tuple(prn: &Prn) -> (String, String) {
        let uid = to_cedar_uid(prn);
        (uid.entity_type, uid.entity_id)
    }

    fn root_tuple() -> (String, String) {
        (ROOT_ENTITY.0.to_string(), ROOT_ENTITY.1.to_string())
    }

    /// A tenancy-node `SliceEntity`: `prn`'s Cedar uid, parented on `parent`, with
    /// `effective_status` set to `status`.
    fn node_entity(prn: &Prn, parent: (String, String), status: &str) -> SliceEntity {
        SliceEntity {
            uid: cedar_tuple(prn),
            parents: vec![parent],
            attrs: BTreeMap::from([("effective_status".to_string(), ContextValue::Str(status.to_string()))]),
        }
    }

    /// One shared entity universe for the whole table: the synthetic `Root`, two orgs
    /// (`org_o`, `org_other`) each with one team (`team_o`, `team_other`) and one active
    /// project under it, a third ("archived") project under `org_o`'s team, and the one
    /// principal every case grants roles to.
    struct Universe {
        org_o: OrganizationId,
        team_o: TeamId,
        team_other: TeamId,
        project_in_o: ProjectId,
        project_in_other: ProjectId,
        archived_project_in_o: ProjectId,
        principal: Prn,
        slice: EntitySlice,
    }

    fn universe() -> Universe {
        let org_o = OrganizationId::from_uuid(u(1));
        let org_other = OrganizationId::from_uuid(u(2));
        let team_o = TeamId::from_parts(org_o.uuid(), u(10));
        let team_other = TeamId::from_parts(org_other.uuid(), u(11));
        let project_in_o = ProjectId::from_parts(org_o.uuid(), u(20));
        let project_in_other = ProjectId::from_parts(org_other.uuid(), u(21));
        let archived_project_in_o = ProjectId::from_parts(org_o.uuid(), u(22));
        let principal = principal_prn(100);

        let mut entities = vec![SliceEntity {
            uid: root_tuple(),
            parents: vec![],
            attrs: BTreeMap::new(),
        }];
        entities.push(node_entity(org_o.prn(), root_tuple(), "active"));
        entities.push(node_entity(org_other.prn(), root_tuple(), "active"));
        entities.push(node_entity(team_o.prn(), cedar_tuple(org_o.prn()), "active"));
        entities.push(node_entity(team_other.prn(), cedar_tuple(org_other.prn()), "active"));
        entities.push(node_entity(project_in_o.prn(), cedar_tuple(team_o.prn()), "active"));
        entities.push(node_entity(project_in_other.prn(), cedar_tuple(team_other.prn()), "active"));
        entities.push(node_entity(archived_project_in_o.prn(), cedar_tuple(team_o.prn()), "archived"));

        let principal_uid = to_cedar_uid(&principal);
        entities.push(SliceEntity {
            uid: (principal_uid.entity_type, principal_uid.entity_id),
            parents: vec![],
            attrs: BTreeMap::from([
                ("kind".to_string(), ContextValue::Str("user".to_string())),
                ("status".to_string(), ContextValue::Str("active".to_string())),
            ]),
        });

        Universe {
            org_o,
            team_o,
            team_other,
            project_in_o,
            project_in_other,
            archived_project_in_o,
            principal,
            slice: EntitySlice { entities },
        }
    }

    /// A `RoleGrant` of `role_key` to `principal` at `scope`, with a deterministic id (`u(id)`)
    /// so `grant:<uuid>` determining-policy ids are stable across test runs.
    fn grant(id: u128, principal: &Prn, role_key: &str, scope: GrantScope) -> RoleGrant {
        RoleGrant {
            id: u(id),
            principal: PrincipalId::from_prn(principal.clone()),
            role_key: role_key.to_string(),
            scope,
            linked_policy_id: format!("grant:{}", u(id)),
            created_at: Utc::now(),
        }
    }

    /// One row of the Cedar policy CI test suite (the issue's explicit deliverable): given
    /// `grants` compiled against [`starter_policies`], does `action` on `resource` decide as
    /// `expect`?
    struct Case {
        name: &'static str,
        grants: Vec<RoleGrant>,
        action: Action,
        resource: Prn,
        expect: Effect,
    }

    /// The table-driven Cedar policy test suite: compiles [`starter_policies`] + each case's
    /// grants via [`PolicyEngine::compile`], decides the case's request against the shared
    /// [`universe`], and asserts the resulting [`Effect`]. This is the CI-facing "Cedar policy
    /// test suite" deliverable — every scenario here must keep passing as the starter policy
    /// set evolves.
    #[test]
    fn starter_policy_table() {
        let uni = universe();

        let cases = vec![
            Case {
                name: "an ungranted principal is default-denied",
                grants: vec![],
                action: Action::GetProject,
                resource: uni.project_in_o.prn().clone(),
                expect: Effect::Deny,
            },
            Case {
                name: "platform_admin at Root allows CreateOrganization at Root itself",
                grants: vec![grant(1, &uni.principal, "platform_admin", GrantScope::Root)],
                action: Action::CreateOrganization,
                resource: root_prn(),
                expect: Effect::Allow,
            },
            Case {
                name: "org_admin allows CreateProject on a project under its own org",
                grants: vec![grant(2, &uni.principal, "org_admin", GrantScope::Node(TenancyNodeRef::Organization(uni.org_o.clone())))],
                action: Action::CreateProject,
                resource: uni.project_in_o.prn().clone(),
                expect: Effect::Allow,
            },
            Case {
                name: "org_admin denies CreateProject on a project under a different org",
                grants: vec![grant(3, &uni.principal, "org_admin", GrantScope::Node(TenancyNodeRef::Organization(uni.org_o.clone())))],
                action: Action::CreateProject,
                resource: uni.project_in_other.prn().clone(),
                expect: Effect::Deny,
            },
            Case {
                name: "org_member allows GetProject on a project under its org",
                grants: vec![grant(4, &uni.principal, "org_member", GrantScope::Node(TenancyNodeRef::Organization(uni.org_o.clone())))],
                action: Action::GetProject,
                resource: uni.project_in_o.prn().clone(),
                expect: Effect::Allow,
            },
            Case {
                name: "org_member denies RenameProject (a write) on a project under its org",
                grants: vec![grant(5, &uni.principal, "org_member", GrantScope::Node(TenancyNodeRef::Organization(uni.org_o.clone())))],
                action: Action::RenameProject,
                resource: uni.project_in_o.prn().clone(),
                expect: Effect::Deny,
            },
            Case {
                name: "forbid-archived-writes denies a non-restore write on an archived project even for org_admin",
                grants: vec![grant(6, &uni.principal, "org_admin", GrantScope::Node(TenancyNodeRef::Organization(uni.org_o.clone())))],
                action: Action::RenameProject,
                resource: uni.archived_project_in_o.prn().clone(),
                expect: Effect::Deny,
            },
            Case {
                name: "forbid-archived-writes does not fire on RestoreProject for an archived project: org_admin is allowed to restore it",
                grants: vec![grant(19, &uni.principal, "org_admin", GrantScope::Node(TenancyNodeRef::Organization(uni.org_o.clone())))],
                action: Action::RestoreProject,
                resource: uni.archived_project_in_o.prn().clone(),
                expect: Effect::Allow,
            },
            Case {
                name: "forbid-archived-writes does not fire on RestoreProject for an archived project: platform_admin is allowed to restore it",
                grants: vec![grant(20, &uni.principal, "platform_admin", GrantScope::Root)],
                action: Action::RestoreProject,
                resource: uni.archived_project_in_o.prn().clone(),
                expect: Effect::Allow,
            },
            // -- GrantRole as the requested action: which principals may perform GrantRole
            // itself (the use-case-layer anti-escalation check on *which* role a grant
            // confers is separate, tested elsewhere).
            Case {
                name: "org_admin allows GrantRole on a project under its own org",
                grants: vec![grant(7, &uni.principal, "org_admin", GrantScope::Node(TenancyNodeRef::Organization(uni.org_o.clone())))],
                action: Action::GrantRole,
                resource: uni.project_in_o.prn().clone(),
                expect: Effect::Allow,
            },
            Case {
                name: "org_member denies GrantRole on a project under its own org",
                grants: vec![grant(8, &uni.principal, "org_member", GrantScope::Node(TenancyNodeRef::Organization(uni.org_o.clone())))],
                action: Action::GrantRole,
                resource: uni.project_in_o.prn().clone(),
                expect: Effect::Deny,
            },
            Case {
                name: "team_admin allows GrantRole on a project within its own team",
                grants: vec![grant(9, &uni.principal, "team_admin", GrantScope::Node(TenancyNodeRef::Team(uni.team_o.clone())))],
                action: Action::GrantRole,
                resource: uni.project_in_o.prn().clone(),
                expect: Effect::Allow,
            },
            Case {
                name: "team_member denies GrantRole on a project within its own team",
                grants: vec![grant(10, &uni.principal, "team_member", GrantScope::Node(TenancyNodeRef::Team(uni.team_o.clone())))],
                action: Action::GrantRole,
                resource: uni.project_in_o.prn().clone(),
                expect: Effect::Deny,
            },
            // -- team_admin / team_member behavioral coverage (previously untested roles).
            Case {
                name: "team_admin allows RenameTeam on its own team",
                grants: vec![grant(11, &uni.principal, "team_admin", GrantScope::Node(TenancyNodeRef::Team(uni.team_o.clone())))],
                action: Action::RenameTeam,
                resource: uni.team_o.prn().clone(),
                expect: Effect::Allow,
            },
            Case {
                name: "team_admin denies RenameTeam on a team outside its own subtree",
                grants: vec![grant(12, &uni.principal, "team_admin", GrantScope::Node(TenancyNodeRef::Team(uni.team_o.clone())))],
                action: Action::RenameTeam,
                resource: uni.team_other.prn().clone(),
                expect: Effect::Deny,
            },
            Case {
                name: "team_member allows GetProject on a project within its own team",
                grants: vec![grant(13, &uni.principal, "team_member", GrantScope::Node(TenancyNodeRef::Team(uni.team_o.clone())))],
                action: Action::GetProject,
                resource: uni.project_in_o.prn().clone(),
                expect: Effect::Allow,
            },
            Case {
                name: "team_member denies RenameProject (a write) on a project within its own team",
                grants: vec![grant(14, &uni.principal, "team_member", GrantScope::Node(TenancyNodeRef::Team(uni.team_o.clone())))],
                action: Action::RenameProject,
                resource: uni.project_in_o.prn().clone(),
                expect: Effect::Deny,
            },
            // -- project_admin / project_member behavioral coverage (previously untested roles).
            Case {
                name: "project_admin allows RenameProject on its own project",
                grants: vec![grant(15, &uni.principal, "project_admin", GrantScope::Node(TenancyNodeRef::Project(uni.project_in_o.clone())))],
                action: Action::RenameProject,
                resource: uni.project_in_o.prn().clone(),
                expect: Effect::Allow,
            },
            Case {
                name: "project_admin denies RenameProject on a different project outside its scope",
                grants: vec![grant(16, &uni.principal, "project_admin", GrantScope::Node(TenancyNodeRef::Project(uni.project_in_o.clone())))],
                action: Action::RenameProject,
                resource: uni.project_in_other.prn().clone(),
                expect: Effect::Deny,
            },
            Case {
                name: "project_member allows GetProject on its own project",
                grants: vec![grant(17, &uni.principal, "project_member", GrantScope::Node(TenancyNodeRef::Project(uni.project_in_o.clone())))],
                action: Action::GetProject,
                resource: uni.project_in_o.prn().clone(),
                expect: Effect::Allow,
            },
            Case {
                name: "project_member denies RenameProject (a write) on its own project",
                grants: vec![grant(18, &uni.principal, "project_member", GrantScope::Node(TenancyNodeRef::Project(uni.project_in_o.clone())))],
                action: Action::RenameProject,
                resource: uni.project_in_o.prn().clone(),
                expect: Effect::Deny,
            },
            // -- gateway_user coverage (SMA-446): a dedicated role for InvokeModel, granted at
            // the Organization scope so it covers every team/project under it.
            Case {
                name: "gateway_user allows InvokeModel on a project under its granted org",
                grants: vec![grant(21, &uni.principal, "gateway_user", GrantScope::Node(TenancyNodeRef::Organization(uni.org_o.clone())))],
                action: Action::InvokeModel,
                resource: uni.project_in_o.prn().clone(),
                expect: Effect::Allow,
            },
            Case {
                name: "gateway_user denies InvokeModel on a project under a different org",
                grants: vec![grant(22, &uni.principal, "gateway_user", GrantScope::Node(TenancyNodeRef::Organization(uni.org_o.clone())))],
                action: Action::InvokeModel,
                resource: uni.project_in_other.prn().clone(),
                expect: Effect::Deny,
            },
            Case {
                name: "forbid-archived-writes denies InvokeModel on an archived project even for gateway_user",
                grants: vec![grant(23, &uni.principal, "gateway_user", GrantScope::Node(TenancyNodeRef::Organization(uni.org_o.clone())))],
                action: Action::InvokeModel,
                resource: uni.archived_project_in_o.prn().clone(),
                expect: Effect::Deny,
            },
            Case {
                name: "platform_admin at Root allows CreateUser at Root itself",
                grants: vec![grant(90, &uni.principal, "platform_admin", GrantScope::Root)],
                action: Action::CreateUser,
                resource: root_prn(),
                expect: Effect::Allow,
            },
            Case {
                name: "org_admin denies CreateUser at Root (Root is the ancestor, never a descendant)",
                grants: vec![grant(91, &uni.principal, "org_admin", GrantScope::Node(TenancyNodeRef::Organization(uni.org_o.clone())))],
                action: Action::CreateUser,
                resource: root_prn(),
                expect: Effect::Deny,
            },
        ];

        for case in cases {
            let compiled = PolicyEngine::compile(&starter_policies(), &case.grants).unwrap_or_else(|e| panic!("{}: compile failed: {e}", case.name));
            let req = AccessRequest {
                principal: uni.principal.clone(),
                action: case.action,
                resource: case.resource.clone(),
                context: RequestContext::empty(),
            };
            let decision: Decision = PolicyEngine::decide(&compiled.policy_set, &uni.slice, &req);
            assert_eq!(decision.effect, case.expect, "{}: got {:?}", case.name, decision);
        }
    }

    /// The write-time-validation half of the CI deliverable: every entry produced by
    /// [`starter_policies`] (the base static policy and every role template) must itself pass
    /// [`validate_policy`] — i.e. parse and validate against the embedded schema.
    #[test]
    fn every_starter_policy_passes_schema_validation() {
        for doc in starter_policies() {
            validate_policy(&doc.source).unwrap_or_else(|e| panic!("{} failed schema validation: {e}", doc.policy_id));
        }
    }

    #[test]
    fn system_roles_have_matching_key_and_template_id() {
        for role in system_roles() {
            assert_eq!(role.key, role.template_id, "role {} must have template_id == key (engine::PolicyEngine::compile convention)", role.key);
            assert!(role.system);
        }
    }

    #[test]
    fn role_looks_up_a_known_key_and_rejects_an_unknown_one() {
        assert_eq!(role("org_admin").map(|r| r.key), Some("org_admin".to_string()));
        assert_eq!(role("no-such-role"), None);
    }

    #[test]
    fn starter_policies_are_all_system_flagged_and_org_admin_template_is_present() {
        assert!(starter_policies().iter().all(|d| d.system));
        assert!(starter_policies().iter().any(|d| d.policy_id == "org_admin" && d.kind == PolicyKind::Template));
        assert!(starter_policies().iter().any(|d| d.policy_id == FORBID_ARCHIVED_WRITES_ID && d.kind == PolicyKind::Static));
    }

    #[test]
    fn starter_policy_ids_matches_what_starter_policies_actually_produces() {
        // The const exists so `put_in`'s reserved-namespace check is a slice scan rather than
        // nine `PolicyDocument` allocations per call. This test is what stops it drifting.
        let actual: Vec<String> = starter_policies().into_iter().map(|d| d.policy_id).collect();
        let declared: Vec<String> = STARTER_POLICY_IDS.iter().map(|s| (*s).to_string()).collect();
        assert_eq!(declared, actual, "STARTER_POLICY_IDS must list exactly the ids starter_policies() produces, in order");
    }

    #[test]
    fn is_starter_policy_id_recognizes_every_starter_id_and_nothing_else() {
        for id in STARTER_POLICY_IDS {
            assert!(is_starter_policy_id(id), "{id} must be recognized");
        }
        assert!(!is_starter_policy_id("some-operator-policy"));
        assert!(!is_starter_policy_id(""));
    }

    /// SMA-477 D11: `STARTER_POLICY_REVISION` is hand-maintained, so this pin is what stops it
    /// being forgotten. It hashes the canonical content of every starter policy; any change to
    /// a generated source, a role's action list, a description, or a kind reds it.
    #[test]
    fn starter_policy_content_is_pinned_to_the_declared_revision() {
        let mut hasher = blake3::Hasher::new();
        for doc in starter_policies() {
            hasher.update(doc.policy_id.as_bytes());
            hasher.update(crate::authz::reconcile::content_fingerprint(doc.kind, &doc.source, &doc.description).as_bytes());
        }
        let actual = hasher.finalize().to_hex().to_string();

        assert_eq!(
            actual, EXPECTED_STARTER_CONTENT_HASH,
            "\n\nThe starter policy set's content changed.\n\
             This is expected when you add an Action, edit a role's action list, or reword a \
             description — but it means every deployed database now holds an older set.\n\n\
             Do BOTH of these, in this order:\n\
             1. Bump `STARTER_POLICY_REVISION` (currently {STARTER_POLICY_REVISION}) by one.\n\
             2. Replace `EXPECTED_STARTER_CONTENT_HASH` with:\n     {actual}\n\n\
             Skipping step 1 lets an older binary overwrite this release's policy set \
             fleet-wide (SMA-477 D11).\n"
        );
    }

    /// The new action must actually reach the generated forbid list — the whole reason
    /// STARTER_POLICY_REVISION has to move. A hand-updated hash with the action missing from
    /// `Action::ALL` would otherwise look green.
    #[test]
    fn the_retire_action_is_in_the_generated_forbid_source() {
        assert!(
            forbid_archived_writes_source().contains(r#"Pgs::Iam::Action::"RetireSystemPolicy""#),
            "RetireSystemPolicy is a write action, so it must appear in forbid-archived-writes"
        );
    }

    /// SMA-584: `CreateUser` is a non-restore write, so it must reach the generated forbid
    /// list — the reason `STARTER_POLICY_REVISION` has to move. A hand-updated content hash
    /// with the action missing from `Action::ALL` would otherwise look green.
    #[test]
    fn the_create_user_action_is_in_the_generated_forbid_source() {
        assert!(
            forbid_archived_writes_source().contains(r#"Pgs::Iam::Action::"CreateUser""#),
            "CreateUser is a write action, so it must appear in forbid-archived-writes"
        );
    }
}
