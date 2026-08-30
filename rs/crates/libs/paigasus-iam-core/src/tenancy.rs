// SPDX-License-Identifier: Apache-2.0

//! Tenancy value objects and entities (SMA-442, ADR-0014).

use crate::value::{DomainError, PrincipalId, Stamp};
use chrono::{DateTime, Utc};
use paigasus_kernel::Prn;
use uuid::Uuid;

pub const SLUG_MAX_LEN: usize = 64;
pub const NAME_MAX_CHARS: usize = 256;

/// URL-safe mutable display token, unique within parent scope (spec D2).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Slug(String);

impl Slug {
    pub fn parse(input: &str) -> Result<Self, DomainError> {
        let ok_len = !input.is_empty() && input.len() <= SLUG_MAX_LEN;
        let ok_chars = input.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        let ok_edges = !input.starts_with('-') && !input.ends_with('-') && !input.contains("--");
        if ok_len && ok_chars && ok_edges {
            Ok(Self(input.to_owned()))
        } else {
            Err(DomainError::InvalidSlug(input.to_owned()))
        }
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeStatus {
    Active,
    Archived,
}

impl NodeStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Archived => "archived",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "active" => Some(Self::Active),
            "archived" => Some(Self::Archived),
            _ => None,
        }
    }
    /// D1/D10: effective = own ∨ any ancestor archived. The single source of truth —
    /// application guards, test fakes, and persistence adapters all call this.
    pub fn effective(own: NodeStatus, ancestors: &[NodeStatus]) -> NodeStatus {
        if own == Self::Archived || ancestors.contains(&Self::Archived) {
            Self::Archived
        } else {
            Self::Active
        }
    }
}

const IAM: &str = "iam";

fn check(prn: &Prn, resource_type: &str, wants_org: bool) -> Result<(), DomainError> {
    let ok = prn.service() == IAM && prn.resource_type() == resource_type && prn.org().is_some() == wants_org;
    if ok { Ok(()) } else { Err(DomainError::InvalidNodePrn(prn.canonical())) }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OrganizationId(Prn);
impl OrganizationId {
    pub fn from_prn(prn: Prn) -> Result<Self, DomainError> {
        check(&prn, "organization", false)?;
        Ok(Self(prn))
    }
    pub fn from_uuid(id: Uuid) -> Self {
        Self(Prn::build(IAM, "", None, "organization", id).expect("static org prn parts are valid"))
    }
    pub fn uuid(&self) -> Uuid {
        self.0.resource_id()
    }
    pub fn prn(&self) -> &Prn {
        &self.0
    }
    pub fn canonical(&self) -> String {
        self.0.canonical()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TeamId(Prn);
impl TeamId {
    pub fn from_prn(prn: Prn) -> Result<Self, DomainError> {
        check(&prn, "team", true)?;
        Ok(Self(prn))
    }
    pub fn from_parts(org: Uuid, id: Uuid) -> Self {
        Self(Prn::build(IAM, "", Some(org), "team", id).expect("static team prn parts are valid"))
    }
    pub fn uuid(&self) -> Uuid {
        self.0.resource_id()
    }
    pub fn org_uuid(&self) -> Uuid {
        self.0.org().expect("team prn always carries org")
    }
    pub fn prn(&self) -> &Prn {
        &self.0
    }
    pub fn canonical(&self) -> String {
        self.0.canonical()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProjectId(Prn);
impl ProjectId {
    pub fn from_prn(prn: Prn) -> Result<Self, DomainError> {
        check(&prn, "project", true)?;
        Ok(Self(prn))
    }
    pub fn from_parts(org: Uuid, id: Uuid) -> Self {
        Self(Prn::build(IAM, "", Some(org), "project", id).expect("static project prn parts are valid"))
    }
    pub fn uuid(&self) -> Uuid {
        self.0.resource_id()
    }
    pub fn org_uuid(&self) -> Uuid {
        self.0.org().expect("project prn always carries org")
    }
    pub fn prn(&self) -> &Prn {
        &self.0
    }
    pub fn canonical(&self) -> String {
        self.0.canonical()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TenancyNodeRef {
    Organization(OrganizationId),
    Team(TeamId),
    Project(ProjectId),
}
impl TenancyNodeRef {
    pub fn from_prn(prn: Prn) -> Result<Self, DomainError> {
        match prn.resource_type() {
            "organization" => Ok(Self::Organization(OrganizationId::from_prn(prn)?)),
            "team" => Ok(Self::Team(TeamId::from_prn(prn)?)),
            "project" => Ok(Self::Project(ProjectId::from_prn(prn)?)),
            _ => Err(DomainError::InvalidNodePrn(prn.canonical())),
        }
    }
    pub fn canonical(&self) -> String {
        match self {
            Self::Organization(i) => i.canonical(),
            Self::Team(i) => i.canonical(),
            Self::Project(i) => i.canonical(),
        }
    }
    pub fn resource_uuid(&self) -> Uuid {
        match self {
            Self::Organization(i) => i.uuid(),
            Self::Team(i) => i.uuid(),
            Self::Project(i) => i.uuid(),
        }
    }
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Organization(_) => "organization",
            Self::Team(_) => "team",
            Self::Project(_) => "project",
        }
    }
}

pub fn validate_name(input: &str) -> Result<String, DomainError> {
    let trimmed = input.trim();
    if trimmed.is_empty() || trimmed.chars().count() > NAME_MAX_CHARS {
        return Err(DomainError::InvalidName(input.to_owned()));
    }
    Ok(trimmed.to_owned())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Organization {
    pub id: OrganizationId,
    pub slug: Slug,
    pub name: String,
    pub status: NodeStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Who created the row. `None` only for a row written before SMA-440's `m0011` — the
    /// absent `Actor` that `actor.proto` defines as unknown-or-system.
    pub created_by: Option<PrincipalId>,
    /// Who last modified the row. Equals `created_by` on the first write.
    pub modified_by: Option<PrincipalId>,
}
impl Organization {
    pub fn new(id: OrganizationId, slug: Slug, name: &str, stamp: &Stamp) -> Result<Self, DomainError> {
        Ok(Self {
            id,
            slug,
            name: validate_name(name)?,
            status: NodeStatus::Active,
            created_at: stamp.at,
            updated_at: stamp.at,
            created_by: Some(stamp.by.clone()),
            modified_by: Some(stamp.by.clone()),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Team {
    pub id: TeamId,
    pub slug: Slug,
    pub name: String,
    pub status: NodeStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Who created the row. `None` only for a row written before SMA-440's `m0011` — the
    /// absent `Actor` that `actor.proto` defines as unknown-or-system.
    pub created_by: Option<PrincipalId>,
    /// Who last modified the row. Equals `created_by` on the first write.
    pub modified_by: Option<PrincipalId>,
}
impl Team {
    pub fn new(id: TeamId, slug: Slug, name: &str, stamp: &Stamp) -> Result<Self, DomainError> {
        Ok(Self {
            id,
            slug,
            name: validate_name(name)?,
            status: NodeStatus::Active,
            created_at: stamp.at,
            updated_at: stamp.at,
            created_by: Some(stamp.by.clone()),
            modified_by: Some(stamp.by.clone()),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub id: ProjectId,
    pub team_id: TeamId,
    pub slug: Slug,
    pub name: String,
    pub status: NodeStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Who created the row. `None` only for a row written before SMA-440's `m0011` — the
    /// absent `Actor` that `actor.proto` defines as unknown-or-system.
    pub created_by: Option<PrincipalId>,
    /// Who last modified the row. Equals `created_by` on the first write.
    pub modified_by: Option<PrincipalId>,
}
impl Project {
    pub fn new(id: ProjectId, team_id: TeamId, slug: Slug, name: &str, stamp: &Stamp) -> Result<Self, DomainError> {
        if id.org_uuid() != team_id.org_uuid() {
            return Err(DomainError::InvalidNodePrn(id.canonical()));
        }
        Ok(Self {
            id,
            team_id,
            slug,
            name: validate_name(name)?,
            status: NodeStatus::Active,
            created_at: stamp.at,
            updated_at: stamp.at,
            created_by: Some(stamp.by.clone()),
            modified_by: Some(stamp.by.clone()),
        })
    }
}

/// Pure belongs-to relationship (roles arrive in M3). Plain UUIDv7 id, no PRN (D5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Membership {
    pub id: Uuid,
    pub principal_id: PrincipalId,
    pub node: TenancyNodeRef,
    pub created_at: DateTime<Utc>,
}
impl Membership {
    pub fn new(id: Uuid, principal_id: PrincipalId, node: TenancyNodeRef, created_at: DateTime<Utc>) -> Self {
        Self { id, principal_id, node, created_at }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn u(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    /// Named `stamp_at_secs` rather than `test_stamp` deliberately: Task 3 adds a `pub
    /// test_stamp(at: DateTime<Utc>, actor: u128)` to the service crate's `fakes.rs` with a
    /// different first parameter, and two helpers sharing a name across crates invites a
    /// wrong-argument mistake that still compiles.
    fn stamp_at_secs(secs: i64, actor: u128) -> Stamp {
        Stamp::new(Utc.timestamp_opt(secs, 0).unwrap(), PrincipalId::from_prn(Prn::build("iam", "", None, "principal", Uuid::from_u128(actor)).unwrap()))
    }

    /// SMA-440: the first write sets `modified_by` equal to `created_by`, mirroring the rule
    /// `AuditMetadata` already states for `modified_at` vs `created_at`.
    #[test]
    fn a_new_org_records_its_creator_as_its_first_modifier() {
        let stamp = stamp_at_secs(1_700_000_000, 1);
        let org = Organization::new(OrganizationId::from_uuid(Uuid::from_u128(10)), Slug::parse("acme").unwrap(), "Acme", &stamp).unwrap();
        assert_eq!(org.created_by.as_ref(), Some(&stamp.by));
        assert_eq!(org.modified_by.as_ref(), Some(&stamp.by));
        assert_eq!(org.created_at, stamp.at);
        assert_eq!(org.updated_at, stamp.at);
    }

    #[test]
    fn slug_accepts_valid_forms() {
        for s in ["a", "acme", "acme-corp", "a1-b2-c3", "x".repeat(64).as_str()] {
            assert!(Slug::parse(s).is_ok(), "{s}");
        }
    }
    #[test]
    fn slug_rejects_invalid_forms() {
        for s in ["", "-a", "a-", "a--b", "A", "a_b", "a b", "ä", "x".repeat(65).as_str()] {
            assert!(Slug::parse(s).is_err(), "{s}");
        }
    }
    #[test]
    fn node_status_roundtrips() {
        assert_eq!(NodeStatus::parse("active"), Some(NodeStatus::Active));
        assert_eq!(NodeStatus::parse("archived"), Some(NodeStatus::Archived));
        assert_eq!(NodeStatus::parse("bogus"), None);
        assert_eq!(NodeStatus::Active.as_str(), "active");
    }
    #[test]
    fn effective_status_truth_table() {
        use NodeStatus::*;
        assert_eq!(NodeStatus::effective(Active, &[]), Active);
        assert_eq!(NodeStatus::effective(Archived, &[]), Archived);
        assert_eq!(NodeStatus::effective(Active, &[Active]), Active);
        assert_eq!(NodeStatus::effective(Active, &[Archived]), Archived);
        assert_eq!(NodeStatus::effective(Archived, &[Active]), Archived);
        assert_eq!(NodeStatus::effective(Active, &[Active, Archived]), Archived);
    }
    #[test]
    fn organization_id_roundtrip_and_rejections() {
        let id = OrganizationId::from_uuid(u(1));
        assert_eq!(id.canonical(), format!("prn:pgs:iam:::organization/{}", u(1)));
        assert_eq!(id.uuid(), u(1));
        assert!(OrganizationId::from_prn(Prn::build("iam", "", Some(u(9)), "organization", u(1)).unwrap()).is_err()); // org slot must be empty
        assert!(OrganizationId::from_prn(Prn::build("iam", "", None, "team", u(1)).unwrap()).is_err()); // wrong type
        assert!(OrganizationId::from_prn(Prn::build("gateway", "", None, "organization", u(1)).unwrap()).is_err()); // wrong service
    }
    #[test]
    fn team_and_project_ids_carry_org() {
        let t = TeamId::from_parts(u(7), u(2));
        assert_eq!(t.org_uuid(), u(7));
        assert_eq!(t.canonical(), format!("prn:pgs:iam::{}:team/{}", u(7), u(2)));
        assert!(TeamId::from_prn(Prn::build("iam", "", None, "team", u(2)).unwrap()).is_err()); // org slot required
        let p = ProjectId::from_parts(u(7), u(3));
        assert_eq!(p.canonical(), format!("prn:pgs:iam::{}:project/{}", u(7), u(3)));
    }
    #[test]
    fn node_ref_dispatches_by_resource_type() {
        let r = TenancyNodeRef::from_prn(Prn::parse(&TeamId::from_parts(u(7), u(2)).canonical()).unwrap()).unwrap();
        assert!(matches!(r, TenancyNodeRef::Team(_)));
        assert_eq!(r.resource_uuid(), u(2));
        assert_eq!(r.kind(), "team");
        assert!(TenancyNodeRef::from_prn(Prn::build("iam", "", None, "user", u(1)).unwrap()).is_err());
    }
    #[test]
    fn name_validation() {
        assert_eq!(validate_name("  Acme Corp.  ").unwrap(), "Acme Corp.");
        assert!(validate_name("   ").is_err());
        assert!(validate_name(&"x".repeat(257)).is_err());
        assert!(validate_name(&"ü".repeat(256)).is_ok()); // scalar values, not bytes
    }
    #[test]
    fn project_rejects_cross_org_team() {
        let stamp = stamp_at_secs(1_700_000_000, 1);
        let team = TeamId::from_parts(u(7), u(2));
        assert!(Project::new(ProjectId::from_parts(u(8), u(3)), team.clone(), Slug::parse("p").unwrap(), "P", &stamp).is_err());
        assert!(Project::new(ProjectId::from_parts(u(7), u(3)), team, Slug::parse("p").unwrap(), "P", &stamp).is_ok());
    }
    #[test]
    fn new_nodes_start_active() {
        let stamp = stamp_at_secs(1_700_000_000, 1);
        let org = Organization::new(OrganizationId::from_uuid(u(1)), Slug::parse("acme").unwrap(), "Acme", &stamp).unwrap();
        assert_eq!(org.status, NodeStatus::Active);
        assert_eq!(org.created_at, stamp.at);
        assert_eq!(org.updated_at, stamp.at);
    }
}
