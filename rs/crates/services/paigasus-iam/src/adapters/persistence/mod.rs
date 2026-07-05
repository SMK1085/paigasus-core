// SPDX-License-Identifier: Apache-2.0

//! Postgres persistence adapter: entities, migrations, and the repository impl.

pub mod entities;
pub mod migration;
pub mod pg_repository;

pub use migration::Migrator;
pub use pg_repository::PgPrincipalRepository;
