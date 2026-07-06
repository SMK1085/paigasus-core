// SPDX-License-Identifier: Apache-2.0

//! `KernelIdGenerator` — mints a UUIDv7 + PRN via `paigasus-kernel`, supplying the host's
//! clock and entropy (the kernel is pure and does neither).

use paigasus_iam_core::{IdGenerator, OrganizationId, PrincipalId, ProjectId, TeamId};
use paigasus_kernel::{Prn, mint_uuid7};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

#[derive(Debug, Default, Clone, Copy)]
pub struct KernelIdGenerator;

impl KernelIdGenerator {
    /// Mints a fresh UUIDv7 from the host clock + entropy (the kernel is pure and does neither).
    fn mint(&self) -> Uuid {
        let ms = SystemTime::now().duration_since(UNIX_EPOCH).expect("system clock before 1970").as_millis() as u64;
        mint_uuid7(ms, rand::random::<[u8; 10]>())
    }
}

impl IdGenerator for KernelIdGenerator {
    fn new_principal_id(&self) -> PrincipalId {
        let uuid = self.mint();
        // Statically infallible for these fixed, valid inputs (service/type are valid labels,
        // region empty, org none, id a valid UUID).
        let prn = Prn::build("iam", "", None, "principal", uuid).expect("valid IAM principal PRN");
        PrincipalId::from_prn(prn)
    }

    fn new_organization_id(&self) -> OrganizationId {
        OrganizationId::from_uuid(self.mint())
    }

    fn new_team_id(&self, org: Uuid) -> TeamId {
        TeamId::from_parts(org, self.mint())
    }

    fn new_project_id(&self, org: Uuid) -> ProjectId {
        ProjectId::from_parts(org, self.mint())
    }

    fn new_membership_id(&self) -> Uuid {
        self.mint()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mints_a_v7_principal_prn() {
        let id = KernelIdGenerator.new_principal_id();
        assert_eq!(id.uuid().get_version_num(), 7);
        let canonical = id.canonical();
        assert!(canonical.starts_with("prn:pgs:iam:::principal/"), "unexpected PRN: {canonical}");
        // Distinct calls mint distinct ids.
        assert_ne!(KernelIdGenerator.new_principal_id().uuid(), id.uuid());
    }

    #[test]
    fn mints_v7_tenancy_ids() {
        let org = KernelIdGenerator.new_organization_id();
        assert_eq!(org.uuid().get_version_num(), 7);
        assert!(org.canonical().starts_with("prn:pgs:iam:::organization/"), "unexpected PRN: {}", org.canonical());

        let team = KernelIdGenerator.new_team_id(org.uuid());
        assert_eq!(team.uuid().get_version_num(), 7);
        assert_eq!(team.org_uuid(), org.uuid());

        let project = KernelIdGenerator.new_project_id(org.uuid());
        assert_eq!(project.uuid().get_version_num(), 7);
        assert_eq!(project.org_uuid(), org.uuid());

        let membership_id = KernelIdGenerator.new_membership_id();
        assert_eq!(membership_id.get_version_num(), 7);

        // Distinct calls mint distinct ids.
        assert_ne!(KernelIdGenerator.new_organization_id().uuid(), org.uuid());
    }
}
