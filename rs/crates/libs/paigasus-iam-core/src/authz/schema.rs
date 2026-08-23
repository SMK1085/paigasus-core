// SPDX-License-Identifier: Apache-2.0
//! Embedded Cedar schema (ADR-0013) + write-time policy validation. Parsed once.

use super::model::AuthzError;
use cedar_policy::Schema;
use std::str::FromStr;
use std::sync::OnceLock;

/// Cedar schema (human syntax). One `Principal` type (kind as an attribute); the tenancy
/// nodes form the resource hierarchy with a synthetic `Root` at the top.
pub const SCHEMA_SRC: &str = r#"
namespace Pgs::Iam {
  entity Root;
  entity Organization in [Root] { effective_status: String };
  entity Team in [Organization] { effective_status: String };
  entity Project in [Team] { effective_status: String };
  entity Principal { kind: String, status: String };

  action GetOrganization, ListOrganizations, GetTeam, ListTeams, GetProject, ListProjects,
         ListMemberships, CreateOrganization, RenameOrganization, ArchiveOrganization,
         RestoreOrganization, CreateTeam, RenameTeam, ArchiveTeam, RestoreTeam, CreateProject,
         RenameProject, ArchiveProject, RestoreProject, AttachMembership, DetachMembership,
         PutPolicy, DeletePolicy, ListPolicies, GrantRole, RevokeRole, ListRoleGrants,
         CreateServiceAccount, GetServiceAccount, ListServiceAccounts, ArchiveServiceAccount,
         IssueApiKey, RevokeApiKey, ListApiKeys, ListAuditLog, ListOutboxDeadLetters,
         ReplayOutboxDeadLetter, DiscardOutboxDeadLetter, RetireSystemPolicy, InvokeModel,
         CreateUser
    appliesTo { principal: [Principal], resource: [Root, Organization, Team, Project] };
}
"#;

pub fn schema() -> &'static Schema {
    static SCHEMA: OnceLock<Schema> = OnceLock::new();
    SCHEMA.get_or_init(|| Schema::from_str(SCHEMA_SRC).expect("embedded Cedar schema is valid"))
}

/// Validate a policy's syntax + schema conformance at write time.
pub fn validate_policy(src: &str) -> Result<(), AuthzError> {
    use cedar_policy::{PolicySet, ValidationMode, Validator};
    let pset = PolicySet::from_str(src).map_err(|e| AuthzError::PolicyParse(e.to_string()))?;
    let result = Validator::new(schema().clone()).validate(&pset, ValidationMode::default());
    if result.validation_passed() {
        Ok(())
    } else {
        Err(AuthzError::SchemaValidation(format!("{result:?}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn schema_parses() {
        let _ = schema();
    }
    #[test]
    fn a_wellformed_permit_validates() {
        assert!(validate_policy(r#"permit(principal, action == Pgs::Iam::Action::"GetOrganization", resource);"#).is_ok());
    }
    #[test]
    fn a_malformed_policy_is_rejected() {
        assert!(validate_policy("permit(this is not cedar);").is_err());
    }
    /// SCHEMA_SRC's action list is hand-maintained. If the new action is missing there,
    /// `validate_policy` rejects the newly-generated forbid-archived-writes source and boot fails.
    #[test]
    fn the_retire_action_validates_against_the_embedded_schema() {
        assert!(validate_policy(r#"permit(principal, action == Pgs::Iam::Action::"RetireSystemPolicy", resource);"#).is_ok());
    }
    /// SMA-584: the twin of the `RetireSystemPolicy` test above. `SCHEMA_SRC`'s action list is
    /// hand-maintained, so a `CreateUser` present in `Action::ALL` but missing here makes the
    /// generated `forbid-archived-writes` source fail validation.
    #[test]
    fn the_create_user_action_validates_against_the_embedded_schema() {
        assert!(validate_policy(r#"permit(principal, action == Pgs::Iam::Action::"CreateUser", resource);"#).is_ok());
    }
}
