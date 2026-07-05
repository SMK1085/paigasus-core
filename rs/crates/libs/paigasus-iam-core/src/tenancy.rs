// SPDX-License-Identifier: Apache-2.0

//! Tenancy value objects and entities (SMA-442, ADR-0014).

use crate::value::DomainError;
use paigasus_kernel::Prn;
use uuid::Uuid;

pub const SLUG_MAX_LEN: usize = 64;

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

#[cfg(test)]
mod tests {
    use super::*;

    fn u(n: u128) -> Uuid {
        Uuid::from_u128(n)
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
}
