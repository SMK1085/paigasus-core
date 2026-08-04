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
            | Action::ListOutboxDeadLetters => false,
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
            | Action::InvokeModel => true,
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
                | Action::InvokeModel => {}
            }
        }
        for a in Action::ALL {
            assert_in_all(*a);
        }
        assert_eq!(
            Action::ALL.len(),
            40,
            "27 pre-existing + 7 M4 + 1 audit + 1 invoke-model + 3 outbox dead-letter + 1 SMA-481 RetireSystemPolicy"
        );
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
}
