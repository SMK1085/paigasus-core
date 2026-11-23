// SPDX-License-Identifier: Apache-2.0

use sea_orm_migration::prelude::*;

mod m0001_create_inbox;
mod m0002_create_memory;
mod m0003_create_inbox_event_tables;
mod m0004_create_timer;
mod m0005_create_session_event;

/// The migration bookkeeping table. It differs from the default so that the agent host does not
/// share state with another service on the same server.
pub const MIGRATION_TABLE: &str = "agent_host_migrations";

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m0001_create_inbox::Migration),
            Box::new(m0002_create_memory::Migration),
            Box::new(m0003_create_inbox_event_tables::Migration),
            Box::new(m0004_create_timer::Migration),
            Box::new(m0005_create_session_event::Migration),
        ]
    }

    fn migration_table_name() -> DynIden {
        Alias::new(MIGRATION_TABLE).into_iden()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_migrator_uses_its_own_table() {
        let name = Migrator::migration_table_name();
        assert_eq!(name.to_string(), MIGRATION_TABLE);
        assert_ne!(MIGRATION_TABLE, "seaql_migrations");
    }
}
