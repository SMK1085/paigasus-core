// SPDX-License-Identifier: Apache-2.0

//! Service-account domain entity (SMA-445, M4).

use crate::principal::PrincipalStatus;
use crate::tenancy::{TenancyNodeRef, validate_name};
use crate::value::{DomainError, PrincipalId};
use chrono::{DateTime, Utc};

/// A non-human `Principal` (D16: lifecycle status lives on the `Principal`, not here).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceAccount {
    pub principal_id: PrincipalId,
    pub owner: TenancyNodeRef,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl ServiceAccount {
    pub fn new(principal_id: PrincipalId, owner: TenancyNodeRef, name: &str, now: DateTime<Utc>) -> Result<Self, DomainError> {
        Ok(Self {
            principal_id,
            owner,
            name: validate_name(name)?,
            created_at: now,
            updated_at: now,
        })
    }
}

/// A [`ServiceAccount`] together with its owning `Principal`'s lifecycle status. A read view,
/// not a persisted shape: D16 keeps `status` on the `Principal` row, never on `ServiceAccount`
/// itself, so this is what read paths (`ServiceAccountRepository::find`/`list_by_owner`,
/// `ServiceAccountService::create`/`get`/`list`) hand back — callers that need to know whether
/// an SA is active or disabled don't have to issue a second, out-of-band `PrincipalRepository`
/// read to find out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceAccountRecord {
    pub account: ServiceAccount,
    pub status: PrincipalStatus,
}

#[cfg(test)]
mod tests {
    use crate::tenancy::{OrganizationId, TenancyNodeRef};
    use crate::value::PrincipalId;
    use chrono::Utc;
    use paigasus_kernel::Prn;
    use uuid::Uuid;

    fn pid() -> PrincipalId {
        let uuid = Uuid::parse_str("0192f1c0-0000-7000-8000-000000000001").unwrap();
        PrincipalId::from_prn(Prn::build("iam", "", None, "principal", uuid).unwrap())
    }

    fn owner_org_ref() -> TenancyNodeRef {
        TenancyNodeRef::Organization(OrganizationId::from_uuid(Uuid::from_u128(1)))
    }

    #[test]
    fn new_rejects_blank_name() {
        let id = pid();
        assert!(super::ServiceAccount::new(id, owner_org_ref(), "  ", Utc::now()).is_err());
    }

    #[test]
    fn new_sets_timestamps() {
        let now = Utc::now();
        let sa = super::ServiceAccount::new(pid(), owner_org_ref(), "ci-bot", now).unwrap();
        assert_eq!(sa.created_at, now);
        assert_eq!(sa.updated_at, now);
        assert_eq!(sa.name, "ci-bot");
    }
}
