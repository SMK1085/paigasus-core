// SPDX-License-Identifier: Apache-2.0

//! Pure IAM domain (M0 walking skeleton): entities, value objects, and port traits.
//! No I/O, no SeaORM, no axum/tonic — the service crate provides adapters (ADR-0005,
//! hexagonal). IDs/PRNs come from `paigasus-kernel`; time/entropy are injected via ports.

pub mod value;

pub use value::{DomainError, Email, PrincipalId};
