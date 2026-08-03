// SPDX-License-Identifier: Apache-2.0

use sea_orm_migration::prelude::*;

mod m0001_create_principal_and_user;
mod m0002_create_tenancy;
mod m0003_create_external_identity;
mod m0004_create_authz;
mod m0005_create_service_accounts_and_api_keys;
mod m0006_create_audit_log;
mod m0007_create_event_outbox;
pub mod m0008_partition_audit_log;
mod m0009_outbox_dead_letter_columns;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m0001_create_principal_and_user::Migration),
            Box::new(m0002_create_tenancy::Migration),
            Box::new(m0003_create_external_identity::Migration),
            Box::new(m0004_create_authz::Migration),
            Box::new(m0005_create_service_accounts_and_api_keys::Migration),
            Box::new(m0006_create_audit_log::Migration),
            Box::new(m0007_create_event_outbox::Migration),
            Box::new(m0008_partition_audit_log::Migration),
            Box::new(m0009_outbox_dead_letter_columns::Migration),
        ]
    }
}
