// SPDX-License-Identifier: Apache-2.0

//! Boot-time reconciliation of the starter Cedar policy set + the seven system roles
//! (SMA-444 Task 17, design §3.2/D6/M5). [`reconcile_starter`] runs once per `AppState::new`
//! (the composition root), AFTER the policy store is constructed but BEFORE the initial
//! `PolicySnapshot` compiles — so a fresh/unseeded database gets the starter policies (and
//! role catalog) seeded before the first request is ever decided, and every process restart
//! is idempotent: an unchanged starter policy is a no-op, a changed one is a WARN (never a
//! silent overwrite — the store itself refuses to edit a persisted `system = true` row, so
//! attempting one would error, not drift), and an already-present role row is left alone.
//! This is boot code, so it touches the `role` SeaORM entity directly rather than going
//! through a port — there is no `RoleRepository` port (roles are code-defined; this table is
//! only their persisted/introspectable form and the `role_grant.role_key` FK target).

use crate::adapters::persistence::entities::role;
use chrono::Utc;
use paigasus_iam_core::authz::model::NodeKind;
use paigasus_iam_core::authz::roles as authz_roles;
use paigasus_iam_core::{AuthzError, PolicyStore, Role};
use sea_orm::{ActiveModelTrait, DatabaseConnection, DbErr, EntityTrait, Set, SqlErr};

fn map_err(e: DbErr) -> AuthzError {
    AuthzError::Backend(Box::new(e))
}

fn node_kind_str(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::Root => "root",
        NodeKind::Organization => "organization",
        NodeKind::Team => "team",
        NodeKind::Project => "project",
    }
}

/// Renders a `Role::scope_kinds` list as the JSON-array-of-strings the `role.scope_kinds`
/// column stores (e.g. `["organization"]`, mirroring the encoding `authz_role_grants.rs`'s
/// test fixtures assume) — nothing currently parses this column back at runtime; the
/// `role_key -> Role` catalog lookup is always code-defined (`authz::roles::role`), so this
/// column is introspectable-only.
fn scope_kinds_json(kinds: &[NodeKind]) -> String {
    let items: Vec<String> = kinds.iter().map(|k| format!("\"{}\"", node_kind_str(*k))).collect();
    format!("[{}]", items.join(","))
}

/// Inserts `role_def`'s row if the key is absent; already-present is left untouched
/// (idempotent). A unique-constraint violation on insert (a concurrent replica's boot won
/// the race between our existence check and our insert) is treated as an idempotent no-op,
/// not an error — the row exists either way.
async fn seed_role_row(db: &DatabaseConnection, role_def: &Role) -> Result<(), AuthzError> {
    if role::Entity::find_by_id(role_def.key.clone()).one(db).await.map_err(map_err)?.is_some() {
        return Ok(());
    }
    let active = role::ActiveModel {
        key: Set(role_def.key.clone()),
        template_id: Set(role_def.template_id.clone()),
        scope_kinds: Set(scope_kinds_json(&role_def.scope_kinds)),
        description: Set(if role_def.description.is_empty() { None } else { Some(role_def.description.clone()) }),
        system: Set(role_def.system),
        created_at: Set(Utc::now()),
    };
    match active.insert(db).await {
        Ok(_) => Ok(()),
        Err(e) if matches!(e.sql_err(), Some(SqlErr::UniqueConstraintViolation(_))) => Ok(()),
        Err(e) => Err(map_err(e)),
    }
}

/// Compare-and-warn reconciliation of `authz::roles::starter_policies()` against what's
/// currently persisted, then seeds the `role` table from `authz::roles::system_roles()`.
/// Policies are reconciled FIRST: every role template's `policy_id == Role::key ==
/// Role::template_id` (see `authz::roles` module docs), and `role.template_id` carries an
/// FK to `policy.policy_id` (`fk_role_template`) — the referenced policy row must exist
/// before the role row can be inserted.
pub async fn reconcile_starter(policies: &dyn PolicyStore, db: &DatabaseConnection) -> Result<(), AuthzError> {
    let current = policies.list_all().await?;
    for doc in authz_roles::starter_policies() {
        match current.iter().find(|d| d.policy_id == doc.policy_id) {
            None => policies.put(&doc).await?,
            Some(existing) if existing.source != doc.source => {
                tracing::warn!(
                    policy_id = %doc.policy_id,
                    "starter policy drift: the stored source differs from the code-defined source; not overwriting a system-owned row"
                );
            }
            Some(_) => {}
        }
    }

    for role_def in authz_roles::system_roles() {
        seed_role_row(db, &role_def).await?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // `reconcile_starter` itself needs a real `DatabaseConnection` for the role-table half
    // (no `RoleStore` port to fake against — see the module docs) — its full seed +
    // idempotent-second-run + drift-warning behavior is covered by the Docker integration
    // test (`tests/authz_bootstrap.rs`). These unit tests cover the pure helpers only.

    #[test]
    fn scope_kinds_json_renders_a_json_string_array() {
        assert_eq!(scope_kinds_json(&[NodeKind::Root]), r#"["root"]"#);
        assert_eq!(scope_kinds_json(&[NodeKind::Organization, NodeKind::Team]), r#"["organization","team"]"#);
    }

    #[test]
    fn node_kind_str_covers_every_variant() {
        assert_eq!(node_kind_str(NodeKind::Root), "root");
        assert_eq!(node_kind_str(NodeKind::Organization), "organization");
        assert_eq!(node_kind_str(NodeKind::Team), "team");
        assert_eq!(node_kind_str(NodeKind::Project), "project");
    }

    #[test]
    fn starter_policies_are_stable_across_calls_on_id_source_and_kind() {
        // Sanity check `reconcile_starter`'s `existing.source != doc.source` compare-and-warn
        // logic isn't comparing an accidentally-unstable field: two `starter_policies()`
        // calls must agree on every field except the deliberately-fresh timestamps.
        let a = authz_roles::starter_policies();
        let b = authz_roles::starter_policies();
        for (x, y) in a.iter().zip(b.iter()) {
            assert_eq!(x.policy_id, y.policy_id);
            assert_eq!(x.source, y.source);
            assert_eq!(x.kind, y.kind);
        }
    }
}
