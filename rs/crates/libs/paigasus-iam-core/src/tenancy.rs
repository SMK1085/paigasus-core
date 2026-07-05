// SPDX-License-Identifier: Apache-2.0

//! Tenancy value objects and entities (SMA-442, ADR-0014).

use crate::value::DomainError;

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
