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
        }
    }

    /// Parse a wire action id (as returned by [`Action::as_wire`]) back into an `Action`.
    pub fn parse(s: &str) -> Option<Action> {
        Action::ALL.iter().copied().find(|a| a.as_wire() == s)
    }

    /// `true` for mutating actions (Create/Rename/Archive/Restore/Attach/Detach/Put/
    /// Delete/Grant/Revoke); `false` for read-only Get/List actions. Exhaustive over
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
            | Action::ListRoleGrants => false,
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
            | Action::RevokeRole => true,
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
}
