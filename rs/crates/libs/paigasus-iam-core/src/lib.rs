// SPDX-License-Identifier: Apache-2.0

//! Pure IAM domain (M0 walking skeleton): entities, value objects, and port traits.
//! No I/O, no SeaORM, no axum/tonic — the service crate provides adapters (ADR-0005,
//! hexagonal). IDs/PRNs come from `paigasus-kernel`; time/entropy are injected via ports.

pub mod ports;
pub mod principal;
pub mod tenancy;
pub mod user;
pub mod value;

pub use ports::{Clock, IdGenerator, PrincipalRepository, RepositoryError};
pub use principal::{Principal, PrincipalKind, PrincipalStatus};
pub use tenancy::{Membership, NAME_MAX_CHARS, NodeStatus, Organization, OrganizationId, Project, ProjectId, Slug, Team, TeamId, TenancyNodeRef, validate_name};
pub use user::User;
pub use value::{DomainError, Email, PrincipalId};
