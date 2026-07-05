// SPDX-License-Identifier: Apache-2.0

use sea_orm_migration::prelude::*;

mod m0001_create_principal_and_user;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![Box::new(m0001_create_principal_and_user::Migration)]
    }
}
