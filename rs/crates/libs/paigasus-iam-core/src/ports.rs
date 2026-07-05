// SPDX-License-Identifier: Apache-2.0

//! Hexagonal ports (traits) the service's adapters implement. Kept in the pure core so
//! use cases depend on abstractions, not on SeaORM/axum (ADR-0005).

use crate::principal::Principal;
use crate::user::User;
use crate::value::PrincipalId;
use async_trait::async_trait;
use chrono::{DateTime, Utc};

/// Persistence errors, source-preserving. The adapter maps its backend error (e.g. SeaORM
/// `DbErr`) into these; the core never imports the backend.
#[derive(Debug, thiserror::Error)]
pub enum RepositoryError {
    #[error("conflict: {0}")]
    Conflict(String),
    #[error(transparent)]
    Backend(#[from] Box<dyn std::error::Error + Send + Sync>),
}

/// Persistence port for user-principals.
#[async_trait]
pub trait PrincipalRepository: Send + Sync {
    async fn create_user(&self, principal: &Principal, user: &User) -> Result<(), RepositoryError>;
    async fn find_user(&self, id: &PrincipalId) -> Result<Option<(Principal, User)>, RepositoryError>;
}

/// Mints new principal identities (UUIDv7 + PRN). Impure (clock + entropy) — hence a port.
pub trait IdGenerator: Send + Sync {
    fn new_principal_id(&self) -> PrincipalId;
}

/// A source of the current time, truncated to microseconds so values round-trip through
/// Postgres `TIMESTAMPTZ` (µs resolution) bit-for-bit.
pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

#[cfg(test)]
mod tests {
    use super::*;

    // Compile-time proof the repository port is object-safe (injected as a trait object).
    #[allow(dead_code)]
    fn assert_object_safe(_: &dyn PrincipalRepository) {}

    #[test]
    fn repository_error_wraps_a_source_error() {
        let e: RepositoryError = Box::<dyn std::error::Error + Send + Sync>::from("boom").into();
        assert!(matches!(e, RepositoryError::Backend(_)));
    }
}
