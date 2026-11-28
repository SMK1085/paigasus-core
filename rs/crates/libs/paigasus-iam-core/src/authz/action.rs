// SPDX-License-Identifier: Apache-2.0
//! IAM action catalog (ADR-0013): one `Action` variant per Cedar action declared in the
//! embedded schema (`super::schema::SCHEMA_SRC`), kept 1:1 with it.

use cedar_policy::{EntityId, EntityTypeName, EntityUid};
use std::str::FromStr;

/// An IAM action, one variant per `Pgs::Iam::Action` entity declared in the embedded
/// Cedar schema (`super::schema::SCHEMA_SRC`). Keep this enum 1:1 with the schema's
/// `action` declaration — `IsAuthorized` is the authorization entry point, not itself
/// an action, and is deliberately excluded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    GetOrganization,
    ListOrganizations,
    GetTeam,
    ListTeams,
    GetProject,
    ListProjects,
    ListMemberships,
    CreateOrganization,
    RenameOrganization,
    ArchiveOrganization,
    RestoreOrganization,
    CreateTeam,
    RenameTeam,
    ArchiveTeam,
    RestoreTeam,
    CreateProject,
    RenameProject,
    ArchiveProject,
    RestoreProject,
    AttachMembership,
    DetachMembership,
    PutPolicy,
    DeletePolicy,
    ListPolicies,
    GrantRole,
    RevokeRole,
    ListRoleGrants,
    CreateServiceAccount,
    GetServiceAccount,
    ListServiceAccounts,
    ArchiveServiceAccount,
    IssueApiKey,
    RevokeApiKey,
    ListApiKeys,
    ListAuditLog,
    ListOutboxDeadLetters,
    ReplayOutboxDeadLetter,
    DiscardOutboxDeadLetter,
    /// Retire an orphaned system-owned policy row (and its `role` row, if any) whose id the
    /// code catalog no longer defines — SMA-481. Root-only, enforced in
    /// `SystemRetirementService` rather than the Cedar schema, exactly like the three
    /// dead-letter actions above.
    RetireSystemPolicy,
    InvokeModel,
    /// Write an event into an agent inbox of an organization (SMA-787 §6.1). Of the scoped starter
    /// roles, only the dedicated `agent_event_writer` grants it directly; `org_admin` never holds
    /// it. `platform_admin` permits every action, so it permits this one too.
    WriteInboxEvent,
    /// Create, update, archive and restore agent definitions and manage the agent inboxes of an
    /// organization (SMA-852). A non-restore write. Held by the `agent_admin` role.
    ManageAgents,
    /// Read the agent definitions and the agent inboxes of an organization (SMA-852). Held by
    /// `agent_admin` and `org_admin`.
    ReadAgents,
    /// Mint a user principal (SMA-584). Authorized at `Root` only: a user principal has no
    /// owner tenancy node (users attach to orgs/teams later, via memberships), so there is no
    /// narrower resource to scope against. Checked in `adapters::http::users` and
    /// `adapters::grpc::users`.
    CreateUser,
    /// Read one user with its external identities (SMA-712, `FindUserByEmail`). The action is in
    /// the Cedar schema (`SCHEMA_SRC`). It is Root-only because `UserIdentityService` passes
    /// `root_prn()` as the resource, not because of the schema.
    GetUser,
    /// Link an external `(issuer, subject)` to a user (SMA-712). Equal to `platform_admin` in
    /// power: a holder can attach an identity that it controls to any user.
    LinkExternalIdentity,
    /// Unlink an external identity from a user (SMA-712). A holder can lock out any user.
    UnlinkExternalIdentity,
    /// Change a user's email (SMA-712).
    ChangeUserEmail,
    /// Write the tool catalog (SMA-774).
    ManageToolCatalog,
    /// Read the tool catalog (SMA-774).
    ReadToolCatalog,
    /// Write tool policy sets (SMA-774).
    ManageToolPolicies,
    /// Read tool policy sets (SMA-774).
    ReadToolPolicies,
    /// Write the platform tool policy floor (SMA-774). Held by `platform_admin` only.
    ManageToolPolicyFloor,
    /// Read the platform tool policy floor (SMA-774). Held by `platform_admin` only.
    ReadToolPolicyFloor,
    /// Write the tool policy settings of an organization (SMA-775).
    ManageToolPolicySettings,
    /// Read the compiled tool policies of an organization or the floor (SMA-775).
    /// Of the scoped roles, only `tool_policy_reader` holds it; `platform_admin` also holds it.
    ReadCompiledToolPolicies,
    /// Run the tool policy re-compile job and read its outcome (SMA-781). Held by `platform_admin`
    /// only, through its every-action template.
    ManageToolPolicyRecompile,
}

impl Action {
    /// Every action declared in the schema, in schema-declaration order.
    pub const ALL: &[Action] = &[
        Action::GetOrganization,
        Action::ListOrganizations,
        Action::GetTeam,
        Action::ListTeams,
        Action::GetProject,
        Action::ListProjects,
        Action::ListMemberships,
        Action::CreateOrganization,
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
        Action::PutPolicy,
        Action::DeletePolicy,
        Action::ListPolicies,
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
        Action::ListAuditLog,
        Action::ListOutboxDeadLetters,
        Action::ReplayOutboxDeadLetter,
        Action::DiscardOutboxDeadLetter,
        Action::RetireSystemPolicy,
        Action::InvokeModel,
        Action::WriteInboxEvent,
        Action::ManageAgents,
        Action::ReadAgents,
        Action::CreateUser,
        Action::GetUser,
        Action::LinkExternalIdentity,
        Action::UnlinkExternalIdentity,
        Action::ChangeUserEmail,
        Action::ManageToolCatalog,
        Action::ReadToolCatalog,
        Action::ManageToolPolicies,
        Action::ReadToolPolicies,
        Action::ManageToolPolicyFloor,
        Action::ReadToolPolicyFloor,
        Action::ManageToolPolicySettings,
        Action::ReadCompiledToolPolicies,
        Action::ManageToolPolicyRecompile,
    ];

    /// The exact Cedar action id, verbatim from `SCHEMA_SRC` — this string doubles as
    /// the wire `action` field over HTTP/gRPC, so it must never be reformatted
    /// (e.g. no case conversion) independently of the schema.
    pub fn as_wire(&self) -> &'static str {
        match self {
            Action::GetOrganization => "GetOrganization",
            Action::ListOrganizations => "ListOrganizations",
            Action::GetTeam => "GetTeam",
            Action::ListTeams => "ListTeams",
            Action::GetProject => "GetProject",
            Action::ListProjects => "ListProjects",
            Action::ListMemberships => "ListMemberships",
            Action::CreateOrganization => "CreateOrganization",
            Action::RenameOrganization => "RenameOrganization",
            Action::ArchiveOrganization => "ArchiveOrganization",
            Action::RestoreOrganization => "RestoreOrganization",
            Action::CreateTeam => "CreateTeam",
            Action::RenameTeam => "RenameTeam",
            Action::ArchiveTeam => "ArchiveTeam",
            Action::RestoreTeam => "RestoreTeam",
            Action::CreateProject => "CreateProject",
            Action::RenameProject => "RenameProject",
            Action::ArchiveProject => "ArchiveProject",
            Action::RestoreProject => "RestoreProject",
            Action::AttachMembership => "AttachMembership",
            Action::DetachMembership => "DetachMembership",
            Action::PutPolicy => "PutPolicy",
            Action::DeletePolicy => "DeletePolicy",
            Action::ListPolicies => "ListPolicies",
            Action::GrantRole => "GrantRole",
            Action::RevokeRole => "RevokeRole",
            Action::ListRoleGrants => "ListRoleGrants",
            Action::CreateServiceAccount => "CreateServiceAccount",
            Action::GetServiceAccount => "GetServiceAccount",
            Action::ListServiceAccounts => "ListServiceAccounts",
            Action::ArchiveServiceAccount => "ArchiveServiceAccount",
            Action::IssueApiKey => "IssueApiKey",
            Action::RevokeApiKey => "RevokeApiKey",
            Action::ListApiKeys => "ListApiKeys",
            Action::ListAuditLog => "ListAuditLog",
            Action::ListOutboxDeadLetters => "ListOutboxDeadLetters",
            Action::ReplayOutboxDeadLetter => "ReplayOutboxDeadLetter",
            Action::DiscardOutboxDeadLetter => "DiscardOutboxDeadLetter",
            Action::RetireSystemPolicy => "RetireSystemPolicy",
            Action::InvokeModel => "InvokeModel",
            Action::WriteInboxEvent => "WriteInboxEvent",
            Action::ManageAgents => "ManageAgents",
            Action::ReadAgents => "ReadAgents",
            Action::CreateUser => "CreateUser",
            Action::GetUser => "GetUser",
            Action::LinkExternalIdentity => "LinkExternalIdentity",
            Action::UnlinkExternalIdentity => "UnlinkExternalIdentity",
            Action::ChangeUserEmail => "ChangeUserEmail",
            Action::ManageToolCatalog => "ManageToolCatalog",
            Action::ReadToolCatalog => "ReadToolCatalog",
            Action::ManageToolPolicies => "ManageToolPolicies",
            Action::ReadToolPolicies => "ReadToolPolicies",
            Action::ManageToolPolicyFloor => "ManageToolPolicyFloor",
            Action::ReadToolPolicyFloor => "ReadToolPolicyFloor",
            Action::ManageToolPolicySettings => "ManageToolPolicySettings",
            Action::ReadCompiledToolPolicies => "ReadCompiledToolPolicies",
            Action::ManageToolPolicyRecompile => "ManageToolPolicyRecompile",
        }
    }

    /// Parse a wire action id (as returned by [`Action::as_wire`]) back into an `Action`.
    pub fn parse(s: &str) -> Option<Action> {
        Action::ALL.iter().copied().find(|a| a.as_wire() == s)
    }

    /// `true` for the three `Restore*` actions (`RestoreOrganization`/`RestoreTeam`/
    /// `RestoreProject`); `false` for everything else. Restores are writes (see
    /// [`Action::is_write`]) but are the one legitimate write on an archived resource — the
    /// `forbid-archived-writes` starter policy (`super::roles`) excludes them via this method.
    pub fn is_restore(&self) -> bool {
        matches!(self, Action::RestoreOrganization | Action::RestoreTeam | Action::RestoreProject)
    }

    /// `true` for mutating actions (Create/Rename/Archive/Restore/Attach/Detach/Put/
    /// Delete/Grant/Revoke/Issue); `false` for read-only Get/List actions. Exhaustive over
    /// every variant so a newly added action must be explicitly classified.
    pub fn is_write(&self) -> bool {
        match self {
            Action::GetOrganization
            | Action::ListOrganizations
            | Action::GetTeam
            | Action::ListTeams
            | Action::GetProject
            | Action::ListProjects
            | Action::ListMemberships
            | Action::ListPolicies
            | Action::ListRoleGrants
            | Action::GetServiceAccount
            | Action::ListServiceAccounts
            | Action::ListApiKeys
            | Action::ListAuditLog
            | Action::ListOutboxDeadLetters
            | Action::GetUser
            | Action::ReadToolCatalog
            | Action::ReadToolPolicies
            | Action::ReadToolPolicyFloor
            | Action::ReadCompiledToolPolicies
            | Action::ReadAgents => false,
            Action::CreateOrganization
            | Action::RenameOrganization
            | Action::ArchiveOrganization
            | Action::RestoreOrganization
            | Action::CreateTeam
            | Action::RenameTeam
            | Action::ArchiveTeam
            | Action::RestoreTeam
            | Action::CreateProject
            | Action::RenameProject
            | Action::ArchiveProject
            | Action::RestoreProject
            | Action::AttachMembership
            | Action::DetachMembership
            | Action::PutPolicy
            | Action::DeletePolicy
            | Action::GrantRole
            | Action::RevokeRole
            | Action::CreateServiceAccount
            | Action::ArchiveServiceAccount
            | Action::IssueApiKey
            | Action::RevokeApiKey
            | Action::ReplayOutboxDeadLetter
            | Action::DiscardOutboxDeadLetter
            | Action::RetireSystemPolicy
            | Action::InvokeModel
            | Action::WriteInboxEvent
            | Action::ManageAgents
            | Action::CreateUser
            | Action::LinkExternalIdentity
            | Action::UnlinkExternalIdentity
            | Action::ChangeUserEmail
            | Action::ManageToolCatalog
            | Action::ManageToolPolicies
            | Action::ManageToolPolicyFloor
            | Action::ManageToolPolicySettings
            | Action::ManageToolPolicyRecompile => true,
        }
    }

    /// The `Pgs::Iam::Action::"<wire>"` entity UID for this action, built from typed
    /// components (not string concatenation + parsing, which the `cedar-policy` API
    /// docs warn against).
    pub fn cedar_uid(&self) -> EntityUid {
        let type_name = EntityTypeName::from_str("Pgs::Iam::Action").expect("Pgs::Iam::Action is a valid Cedar entity type name");
        EntityUid::from_type_name_and_id(type_name, EntityId::new(self.as_wire()))
    }
}

#[cfg(test)]
mod tests {
    use super::Action;
    #[test]
    fn wire_roundtrip_all_variants() {
        for a in Action::ALL {
            assert_eq!(Action::parse(a.as_wire()), Some(*a), "{}", a.as_wire());
            assert_eq!(a.cedar_uid().type_name().to_string(), "Pgs::Iam::Action");
        }
    }
    #[test]
    fn write_classification() {
        assert!(Action::CreateTeam.is_write());
        assert!(!Action::GetTeam.is_write());
    }
    #[test]
    fn issue_api_key_is_a_write() {
        assert!(Action::IssueApiKey.is_write());
    }
    /// `Action::ALL` must contain every enum variant. `ALL` is hand-maintained (no `strum`), so
    /// this is guarded two ways: the inner `match` is *exhaustive* over the `Action` type
    /// itself (not just over what's in `ALL`) — rustc statically requires an arm for every
    /// variant regardless of which values actually reach it at runtime, so adding a variant
    /// without adding a match arm here is a compile error (`E0004: non-exhaustive patterns`),
    /// independent of whether `ALL` was updated. The `len()` assertion then catches a variant
    /// that got a match arm but was never added to `ALL` itself.
    #[test]
    fn all_covers_every_variant() {
        fn assert_in_all(a: Action) {
            assert!(Action::ALL.contains(&a), "{} is missing from Action::ALL", a.as_wire());
            match a {
                Action::GetOrganization
                | Action::ListOrganizations
                | Action::GetTeam
                | Action::ListTeams
                | Action::GetProject
                | Action::ListProjects
                | Action::ListMemberships
                | Action::CreateOrganization
                | Action::RenameOrganization
                | Action::ArchiveOrganization
                | Action::RestoreOrganization
                | Action::CreateTeam
                | Action::RenameTeam
                | Action::ArchiveTeam
                | Action::RestoreTeam
                | Action::CreateProject
                | Action::RenameProject
                | Action::ArchiveProject
                | Action::RestoreProject
                | Action::AttachMembership
                | Action::DetachMembership
                | Action::PutPolicy
                | Action::DeletePolicy
                | Action::ListPolicies
                | Action::GrantRole
                | Action::RevokeRole
                | Action::ListRoleGrants
                | Action::CreateServiceAccount
                | Action::GetServiceAccount
                | Action::ListServiceAccounts
                | Action::ArchiveServiceAccount
                | Action::IssueApiKey
                | Action::RevokeApiKey
                | Action::ListApiKeys
                | Action::ListAuditLog
                | Action::ListOutboxDeadLetters
                | Action::ReplayOutboxDeadLetter
                | Action::DiscardOutboxDeadLetter
                | Action::RetireSystemPolicy
                | Action::InvokeModel
                | Action::WriteInboxEvent
                | Action::CreateUser
                | Action::GetUser
                | Action::LinkExternalIdentity
                | Action::UnlinkExternalIdentity
                | Action::ChangeUserEmail
                | Action::ManageToolCatalog
                | Action::ReadToolCatalog
                | Action::ManageToolPolicies
                | Action::ReadToolPolicies
                | Action::ManageToolPolicyFloor
                | Action::ReadToolPolicyFloor
                | Action::ManageToolPolicySettings
                | Action::ReadCompiledToolPolicies
                | Action::ManageToolPolicyRecompile
                | Action::ManageAgents
                | Action::ReadAgents => {}
            }
        }
        for a in Action::ALL {
            assert_in_all(*a);
        }
        assert_eq!(
            Action::ALL.len(),
            57,
            "27 pre-existing + 7 M4 + 1 audit + 1 invoke-model + 3 outbox dead-letter + 1 SMA-481 RetireSystemPolicy + 1 SMA-584 CreateUser + 4 SMA-712 user identity + 6 SMA-774 tool policies + 2 SMA-775 tool policy settings + 1 SMA-781 tool policy re-compile + 1 SMA-787 WriteInboxEvent + 2 SMA-852 agent registry"
        );
    }
    #[test]
    fn the_agent_registry_actions_are_classified_and_round_trip() {
        assert_eq!(Action::parse("ManageAgents"), Some(Action::ManageAgents));
        assert_eq!(Action::parse("ReadAgents"), Some(Action::ReadAgents));
        assert!(Action::ManageAgents.is_write() && !Action::ManageAgents.is_restore());
        assert!(!Action::ReadAgents.is_write());
    }
    #[test]
    fn write_inbox_event_is_a_write_action() {
        assert_eq!(Action::parse("WriteInboxEvent"), Some(Action::WriteInboxEvent));
        assert!(Action::WriteInboxEvent.is_write());
    }
    #[test]
    fn list_audit_log_is_a_read_action() {
        assert!(!Action::ListAuditLog.is_write());
        assert_eq!(Action::parse("ListAuditLog"), Some(Action::ListAuditLog));
    }
    #[test]
    fn outbox_dead_letter_actions_are_classified_and_round_trip() {
        assert!(!Action::ListOutboxDeadLetters.is_write(), "listing dead letters is a read");
        assert!(Action::ReplayOutboxDeadLetter.is_write(), "replay mutates the outbox");
        assert!(Action::DiscardOutboxDeadLetter.is_write(), "discard deletes a row");
        for a in [Action::ListOutboxDeadLetters, Action::ReplayOutboxDeadLetter, Action::DiscardOutboxDeadLetter] {
            assert_eq!(Action::parse(a.as_wire()), Some(a), "{} must round-trip", a.as_wire());
            assert!(Action::ALL.contains(&a), "{} must be in ALL", a.as_wire());
        }
        // None of the three is a restore, so all three land in the generated
        // `forbid_archived_writes` list (harmless: they are Root-scoped and `Root` has no
        // `effective_status` attribute, so the clause can never match them).
        assert!(!Action::ReplayOutboxDeadLetter.is_restore());
    }
    #[test]
    fn restore_classification() {
        assert!(Action::RestoreOrganization.is_restore());
        assert!(Action::RestoreTeam.is_restore());
        assert!(Action::RestoreProject.is_restore());
        assert!(!Action::CreateTeam.is_restore());
        assert!(!Action::ArchiveTeam.is_restore());
        assert!(!Action::GetTeam.is_restore());
        for a in Action::ALL {
            if a.is_restore() {
                assert!(a.is_write(), "{}: every restore action must also be a write", a.as_wire());
            }
        }
    }
    #[test]
    fn retire_system_policy_is_a_non_restore_write() {
        assert_eq!(Action::RetireSystemPolicy.as_wire(), "RetireSystemPolicy");
        assert!(Action::RetireSystemPolicy.is_write(), "retirement deletes policy and role rows");
        assert!(!Action::RetireSystemPolicy.is_restore());
        assert!(Action::ALL.contains(&Action::RetireSystemPolicy), "must be in the catalog or the forbid list misses it");
    }
    /// SMA-712: `GetUser` is a read; the three identity writes are writes, not restores, so
    /// they reach the generated `forbid-archived-writes` list.
    #[test]
    fn the_user_identity_actions_are_classified_and_round_trip() {
        assert!(!Action::GetUser.is_write(), "finding a user is a read");
        for a in [Action::LinkExternalIdentity, Action::UnlinkExternalIdentity, Action::ChangeUserEmail] {
            assert!(a.is_write(), "{} changes a user", a.as_wire());
            assert!(!a.is_restore(), "{} is not a restore", a.as_wire());
        }
        for a in [Action::GetUser, Action::LinkExternalIdentity, Action::UnlinkExternalIdentity, Action::ChangeUserEmail] {
            assert_eq!(Action::parse(a.as_wire()), Some(a), "{} must round-trip", a.as_wire());
            assert!(Action::ALL.contains(&a), "{} must be in ALL", a.as_wire());
        }
    }
    /// SMA-774: `Manage*` are non-restore writes, `Read*` are reads, and all six round-trip.
    #[test]
    fn the_tool_policy_actions_are_classified_and_round_trip() {
        for a in [Action::ManageToolCatalog, Action::ManageToolPolicies, Action::ManageToolPolicyFloor] {
            assert!(a.is_write(), "{} changes state", a.as_wire());
            assert!(!a.is_restore(), "{} is not a restore", a.as_wire());
        }
        for a in [Action::ReadToolCatalog, Action::ReadToolPolicies, Action::ReadToolPolicyFloor] {
            assert!(!a.is_write(), "{} is a read", a.as_wire());
        }
        for a in [
            Action::ManageToolCatalog,
            Action::ReadToolCatalog,
            Action::ManageToolPolicies,
            Action::ReadToolPolicies,
            Action::ManageToolPolicyFloor,
            Action::ReadToolPolicyFloor,
        ] {
            assert_eq!(Action::parse(a.as_wire()), Some(a), "{} must round-trip", a.as_wire());
            assert!(Action::ALL.contains(&a), "{} must be in ALL", a.as_wire());
        }
    }
    /// SMA-781: the re-compile action is a non-restore write and round-trips.
    #[test]
    fn the_recompile_action_is_classified_and_round_trips() {
        let a = Action::ManageToolPolicyRecompile;
        assert!(a.is_write(), "the job writes new versions");
        assert!(!a.is_restore());
        assert_eq!(a.as_wire(), "ManageToolPolicyRecompile");
        assert_eq!(Action::parse(a.as_wire()), Some(a));
        assert!(Action::ALL.contains(&a));
    }
    /// SMA-775: the settings write is a non-restore write, the compiled read is a read, and both round-trip.
    #[test]
    fn the_tool_policy_settings_actions_are_classified_and_round_trip() {
        assert!(Action::ManageToolPolicySettings.is_write());
        assert!(!Action::ManageToolPolicySettings.is_restore());
        assert!(!Action::ReadCompiledToolPolicies.is_write());
        for a in [Action::ManageToolPolicySettings, Action::ReadCompiledToolPolicies] {
            assert_eq!(Action::parse(a.as_wire()), Some(a), "{} must round-trip", a.as_wire());
            assert!(Action::ALL.contains(&a), "{} must be in ALL", a.as_wire());
        }
        assert_eq!(Action::ManageToolPolicySettings.as_wire(), "ManageToolPolicySettings");
        assert_eq!(Action::ReadCompiledToolPolicies.as_wire(), "ReadCompiledToolPolicies");
    }
}
