// SPDX-License-Identifier: Apache-2.0

//! Postgres-backed [`SystemRoleReconciler`] (SMA-477). Boot converges the `role` table to
//! `authz::roles::system_roles()`, where it previously only inserted missing rows — so a code
//! change to a role's description or scope kinds drifted forever, and silently.
//!
//! There is no `RoleRepository` port and this is not one: roles are code-defined, and this
//! table is only their persisted/introspectable form plus the `role_grant.role_key` FK target.

use super::entities::role;
use crate::adapters::persistence::pg_policies::map_db_err;
use async_trait::async_trait;
use chrono::Utc;
use paigasus_iam_core::authz::reconcile::{RoleOutcome, StoredRoleRow, role_row_matches, scope_kinds_json};
use paigasus_iam_core::{AuthzError, Role, SystemRoleReconciler};
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, QuerySelect, Set, SqlErr};

#[derive(Clone)]
pub struct PgSystemRoleReconciler {
    db: DatabaseConnection,
}

impl PgSystemRoleReconciler {
    #[must_use]
    pub fn new(db: DatabaseConnection) -> Self {
        PgSystemRoleReconciler { db }
    }
}

#[async_trait]
impl SystemRoleReconciler for PgSystemRoleReconciler {
    async fn reconcile_role(&self, role_def: &Role) -> Result<RoleOutcome, AuthzError> {
        let description = if role_def.description.is_empty() { None } else { Some(role_def.description.clone()) };
        let existing = role::Entity::find_by_id(role_def.key.clone()).one(&self.db).await.map_err(map_db_err)?;

        let Some(existing) = existing else {
            let active = role::ActiveModel {
                key: Set(role_def.key.clone()),
                template_id: Set(role_def.template_id.clone()),
                scope_kinds: Set(scope_kinds_json(&role_def.scope_kinds)),
                description: Set(description),
                system: Set(role_def.system),
                created_at: Set(Utc::now()),
            };
            return match active.insert(&self.db).await {
                Ok(_) => Ok(RoleOutcome::Inserted),
                // A concurrent replica's boot won the race between our check and our insert.
                // The row exists either way, so this is an idempotent no-op, not an error.
                Err(e) if matches!(e.sql_err(), Some(SqlErr::UniqueConstraintViolation(_))) => Ok(RoleOutcome::Unchanged),
                Err(e) => Err(map_db_err(e)),
            };
        };

        let stored = StoredRoleRow {
            template_id: &existing.template_id,
            scope_kinds: &existing.scope_kinds,
            description: existing.description.as_deref(),
            system: existing.system,
        };
        if role_row_matches(&stored, role_def) {
            return Ok(RoleOutcome::Unchanged);
        }

        // `created_at` is preserved: only the code-defined columns converge.
        let active = role::ActiveModel {
            key: Set(role_def.key.clone()),
            template_id: Set(role_def.template_id.clone()),
            scope_kinds: Set(scope_kinds_json(&role_def.scope_kinds)),
            description: Set(description),
            system: Set(role_def.system),
            created_at: Set(existing.created_at),
        };
        active.update(&self.db).await.map_err(map_db_err)?;
        Ok(RoleOutcome::Updated)
    }

    async fn orphaned_system_role_keys(&self, known: &[&str]) -> Result<Vec<String>, AuthzError> {
        // `ORDER BY key`, exactly as `orphaned_system_policy_ids` orders by `policy_id`: boot
        // logs one WARN line per orphan, so an unordered scan would reshuffle a boot's lines run
        // to run (and make any multi-orphan test flaky). Contract, not implementation detail.
        // Projected to the key column, matching `orphaned_system_policy_ids`: only the key is used.
        let keys: Vec<String> = role::Entity::find()
            .select_only()
            .column(role::Column::Key)
            .filter(role::Column::System.eq(true))
            .order_by_asc(role::Column::Key)
            .into_tuple()
            .all(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(keys.into_iter().filter(|k| !known.contains(&k.as_str())).collect())
    }

    async fn existing_role_keys(&self) -> Result<Vec<String>, AuthzError> {
        // EVERY row, deliberately unfiltered by `system` — this feeds boot's fatal-vs-survivable
        // decision, and an operator's own non-system row at a code-defined key still means the
        // key exists (the INSERT would collide, not vanish). Narrowing it would report a present
        // row as missing and turn a survivable failure fatal.
        role::Entity::find().select_only().column(role::Column::Key).into_tuple().all(&self.db).await.map_err(map_db_err)
    }
}
