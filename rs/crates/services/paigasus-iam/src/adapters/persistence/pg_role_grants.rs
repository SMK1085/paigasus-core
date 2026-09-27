// SPDX-License-Identifier: Apache-2.0

//! Postgres-backed `RoleGrantStore` (SeaORM). `grant`/`revoke` are thin one-shot-`UnitOfWork`
//! wrappers around [`RoleGrantStore::grant_in`]/[`RoleGrantStore::revoke_in`] (SMA-446, Slice
//! B): they open their own `SeaOrmTransaction`, run the txn-scoped insert/delete on it,
//! commit, then best-effort bump `policy_gen` via the shared `Generations` handle (spec
//! §7/D11: "bumped on any policy CRUD or role grant/revoke"). `BootstrapAdminSeeder` was the
//! last production caller of `grant`; since SMA-468 it drives its own `UnitOfWork` instead —
//! its grant, outbox event and audit row commit together on one transaction via `grant_in`,
//! not `grant` — so a seed failure has a single diagnosable path rather than the wrapper's
//! own bump-then-forget one. As of SMA-468, `grant`/`revoke` therefore have **zero production
//! callers**; every remaining call site is test/integration-only (`authz_role_grants.rs`,
//! `authz_bootstrap.rs`, `tests/support/mod.rs`, and `#[cfg(test)]` fixtures in
//! `cedar_authorizer.rs`/`policy_snapshot.rs`). `grant_in`/`revoke_in` are the txn-scoped
//! primitives `RoleService::grant`/`revoke` (the reference pattern) actually drive: they
//! persist exactly the caller-built `RoleGrant` — including its `linked_policy_id` (the
//! Cedar template-linked policy itself is materialized from grant rows at snapshot-compile
//! time, Task 12, so this store never touches `policy`/`role` rows) — on the caller's own
//! transaction, and deliberately never bump `policy_gen` themselves; the bump is the
//! caller's own awaited, post-commit responsibility (`application::roles::RoleService` via
//! `PolicyGenBumper`). The generation bump itself is logged and swallowed on error,
//! mirroring `pg_organizations.rs::bump_entity_gen`: the write already committed, so a
//! Redis-down bump failure must never fail it, it just means the change lands on the
//! snapshot's TTL backstop (`policy_cache_ttl_secs + refresh_interval_secs`) instead of
//! immediately — the decision cache follows for free, since its key's policy component is the
//! compiled set's `content_hash` (SMA-470 D4), which rotates the moment that reload installs.
//! `revoke`/`revoke_in` mirror
//! `PgPolicyStore::delete`'s idempotent-DELETE posture: a missing id is a no-op success (no
//! `AuthzError::NotFound` variant exists — see `authz::model::AuthzError`) and only a row
//! that actually existed bumps the generation (`revoke`) or is reported back to the caller
//! (`revoke_in`'s `bool`).

use super::entities::role_grant;
use super::uow::{SeaOrmTransaction, recover_txn};
use crate::adapters::authz::Generations;
use async_trait::async_trait;
use paigasus_iam_core::{AuthzError, GrantScope, PrincipalId, RepositoryError, RoleGrant, RoleGrantFilter, RoleGrantQuery, RoleGrantStore, TenancyNodeRef, Transaction};
use paigasus_kernel::Prn;
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, DbBackend, DbErr, EntityTrait, FromQueryResult, QueryFilter, Set, SqlErr, Statement, TransactionTrait};
use uuid::Uuid;

// `Clone` lets the composition root hold a store handle inside a `#[derive(Clone)]` service
// (mirrors `PgPolicyStore`'s precedent) — cheap: `DatabaseConnection` clones an `Arc`-backed
// pool handle, and `Generations` is `Arc`-backed too.
#[derive(Clone)]
pub struct PgRoleGrantStore {
    db: DatabaseConnection,
    gens: Generations,
}

impl PgRoleGrantStore {
    #[must_use]
    pub fn new(db: DatabaseConnection, gens: Generations) -> Self {
        PgRoleGrantStore { db, gens }
    }

    /// Best-effort `policy_gen` bump (spec §7/D11): logged and swallowed on error — mirrors
    /// `pg_organizations.rs::bump_entity_gen`/`bump_policy_gen` exactly: `grant`/`revoke`'s
    /// mutation already committed, so a Redis-down bump failure must never fail an
    /// already-successful write; it just means the change lands on the policy snapshot's TTL
    /// backstop — `policy_cache_ttl_secs + refresh_interval_secs`, NOT the decision cache's own
    /// TTL — instead of immediately (D11: a swallowed bump degrades to backstop-bounded
    /// staleness). The decision cache needs no expiry of its own to follow: its key's policy
    /// component is the compiled set's `content_hash` (SMA-470 D4), so that reload rotates the
    /// key space for it — which is also why the claim has to be phrased this way, since
    /// `MemoryDecisionCache` has no TTL at all.
    async fn bump_policy_gen_best_effort(&self) {
        if let Err(err) = self.gens.bump_policy_gen().await {
            tracing::warn!(error = %err, "pg_role_grants: policy_gen bump failed after a committed write — authz decisions may be stale until the policy snapshot's TTL backstop reloads");
        }
    }
}

fn map_err(e: DbErr) -> AuthzError {
    AuthzError::Backend(Box::new(e))
}

/// `grant_in`'s error mapping. The private [`map_err`] above collapses every `DbErr` into
/// `Backend`, which is right for the reads and deletes around it but wrong for exactly one of
/// `role_grant`'s five foreign keys: a violation of `fk_role_grant_role` specifically means the
/// `role` row this grant names does not exist, which is exactly what `RoleService::grant`
/// reports as `UnknownRole` before it ever reaches the database. The other four —
/// `fk_role_grant_principal`/`fk_role_grant_org`/`fk_role_grant_team`/`fk_role_grant_project` —
/// mean something else entirely (a missing principal or tenancy node) and must stay `Backend`;
/// matching on `SqlErr::ForeignKeyConstraintViolation` alone, without checking WHICH constraint
/// fired, would confidently mislabel all five as "the role is gone" even when the role is fine.
/// So this checks the constraint name embedded in the error text — mirroring `conflict_kind`'s
/// (`persistence/mod.rs`) own name-based attribution for unique violations, not the raw
/// message — before drawing the `UnknownRole` conclusion.
///
/// This is not a theoretical branch. SMA-481 D6: a retirement holds the role row `FOR UPDATE`
/// while a concurrent grant from a replica on an OLDER binary — one whose code catalog still
/// defines the retired role — blocks behind it. When the retirement commits with the row
/// deleted, that grant resumes, re-runs its FK check and fails. Without this mapping the caller
/// gets a `500 internal error` for a condition the service understands perfectly well.
///
/// SMA-676 adds the unique-violation arm: a duplicate (principal, role, scope) is
/// AuthzError::DuplicateGrant.
fn map_grant_err(e: DbErr, role_key: &str) -> AuthzError {
    match e.sql_err() {
        Some(SqlErr::ForeignKeyConstraintViolation(ref msg)) if msg.contains("fk_role_grant_role") => AuthzError::UnknownRole(role_key.to_string()),
        // SMA-676 D9. By constraint NAME, like the FK arm above: `uq_role_grant_linked_policy`
        // is a different defect and must stay `Backend`. The caller's transaction is now
        // aborted at the database; the caller must drop it and read the winner
        // (RoleService::grant, SMA-676 D9).
        Some(SqlErr::UniqueConstraintViolation(ref msg)) if msg.contains("uq_role_grant_principal_role_scope") => AuthzError::DuplicateGrant,
        _ => map_err(e),
    }
}

/// A stored-error helper for a corrupt/unparseable `scope_node_prn` or `principal_id` — a
/// data-integrity break (the row was written by this same adapter), surfaced as `Backend`,
/// never a silent default (mirrors `pg_memberships.rs::parse_status`'s posture).
fn backend_err(msg: impl std::fmt::Display) -> AuthzError {
    AuthzError::Backend(Box::new(std::io::Error::other(msg.to_string())))
}

/// Splits a [`GrantScope`] into the row's `scope_kind` string and its (at most one non-null)
/// `scope_org_id`/`scope_team_id`/`scope_project_id` columns — the inverse of
/// [`model_to_grant`]'s scope reconstruction. `GrantScope::Root` maps to all three NULL
/// (`ck_role_grant_scope`'s `root` arm).
fn scope_columns(scope: &GrantScope) -> (&'static str, Option<Uuid>, Option<Uuid>, Option<Uuid>) {
    match scope {
        GrantScope::Root => ("root", None, None, None),
        GrantScope::Node(TenancyNodeRef::Organization(id)) => ("organization", Some(id.uuid()), None, None),
        GrantScope::Node(TenancyNodeRef::Team(id)) => ("team", None, Some(id.uuid()), None),
        GrantScope::Node(TenancyNodeRef::Project(id)) => ("project", None, None, Some(id.uuid())),
    }
}

/// Builds the insertable `role_grant` row from a domain `RoleGrant` — every field is taken
/// as-is from the caller (the store never recomputes `linked_policy_id`; the use-case layer
/// sets it to `format!("grant:{}", g.id)`, mirroring `authz::engine::link_grant`).
fn grant_to_model(g: &RoleGrant) -> role_grant::ActiveModel {
    let (scope_kind, scope_org_id, scope_team_id, scope_project_id) = scope_columns(&g.scope);
    role_grant::ActiveModel {
        id: Set(g.id),
        principal_id: Set(g.principal.uuid()),
        role_key: Set(g.role_key.clone()),
        scope_kind: Set(scope_kind.to_string()),
        scope_node_prn: Set(g.scope.canonical_prn()),
        scope_org_id: Set(scope_org_id),
        scope_team_id: Set(scope_team_id),
        scope_project_id: Set(scope_project_id),
        linked_policy_id: Set(g.linked_policy_id.clone()),
        created_at: Set(g.created_at),
    }
}

/// Reconstructs the domain `RoleGrant` from a stored row: `scope_kind = 'root'` rebuilds
/// [`GrantScope::Root`] directly (there is no tenancy row for the synthetic Root sentinel);
/// otherwise `scope_node_prn` — the node's own canonical PRN, already carrying its org
/// context — round-trips through `Prn::parse` + `TenancyNodeRef::from_prn`, so the
/// `scope_*_id` FK columns (query/index-only) are never needed for this reconstruction.
/// `principal_id` is a bare uuid with no stored PRN of its own — a principal's PRN shape is
/// fully deterministic (`iam`, no org, `principal`, the uuid), so it's synthesized directly
/// rather than joining back to the `principal` table (mirrors
/// `application/memberships.rs`'s/`authenticate_token.rs`'s precedent).
fn model_to_grant(m: role_grant::Model) -> Result<RoleGrant, AuthzError> {
    let scope = if m.scope_kind == "root" {
        GrantScope::Root
    } else {
        let prn = Prn::parse(&m.scope_node_prn).map_err(backend_err)?;
        let node = TenancyNodeRef::from_prn(prn).map_err(backend_err)?;
        GrantScope::Node(node)
    };
    let principal_prn = Prn::build("iam", "", None, "principal", m.principal_id).map_err(backend_err)?;
    Ok(RoleGrant {
        id: m.id,
        principal: PrincipalId::from_prn(principal_prn),
        role_key: m.role_key,
        scope,
        linked_policy_id: m.linked_policy_id,
        created_at: m.created_at,
    })
}

/// Maps a [`recover_txn`] failure (an opaque `&dyn Transaction` that isn't a
/// `SeaOrmTransaction` — never happens in production, only a misbuilt fake could trigger it)
/// into `AuthzError::Backend`, mirroring [`map_err`]'s posture for row-level failures.
fn map_txn_err(e: RepositoryError) -> AuthzError {
    AuthzError::Backend(Box::new(e))
}

#[async_trait]
impl RoleGrantStore for PgRoleGrantStore {
    async fn grant(&self, g: &RoleGrant) -> Result<(), AuthzError> {
        // A thin one-shot-`UnitOfWork` wrapper (module docs): open a `SeaOrmTransaction`,
        // insert via `grant_in`, commit, then bump — the exact behavior this method had
        // before Slice B, just re-expressed over the txn-scoped primitive so the insert
        // logic lives in exactly one place.
        let txn = self.db.begin().await.map_err(map_err)?;
        let tx: Box<dyn Transaction> = Box::new(SeaOrmTransaction { txn });
        // A `uq_role_grant_principal_role_scope` violation surfaces here as
        // `AuthzError::DuplicateGrant` (SMA-676 D9; `map_grant_err`). A
        // `uq_role_grant_linked_policy` violation stays `AuthzError::Backend`. Either way
        // dropping `tx` without committing rolls the failed insert back.
        self.grant_in(&*tx, g).await?;
        tx.commit().await.map_err(map_txn_err)?;
        self.bump_policy_gen_best_effort().await;
        Ok(())
    }

    async fn revoke(&self, id: Uuid) -> Result<(), AuthzError> {
        // Thin one-shot-`UnitOfWork` wrapper, mirroring `grant`'s above.
        let txn = self.db.begin().await.map_err(map_err)?;
        let tx: Box<dyn Transaction> = Box::new(SeaOrmTransaction { txn });
        let existed = self.revoke_in(&*tx, id).await?;
        tx.commit().await.map_err(map_txn_err)?;
        // Idempotent: revoking an id that was never granted (or already revoked) is a no-op
        // success, mirroring `PgPolicyStore::delete`'s posture (no `NotFound` variant exists
        // on `AuthzError`) — and only a row that actually existed bumps the generation.
        if existed {
            self.bump_policy_gen_best_effort().await;
        }
        Ok(())
    }

    async fn grant_in(&self, tx: &dyn Transaction, g: &RoleGrant) -> Result<(), AuthzError> {
        let txn = recover_txn(tx).map_err(map_txn_err)?;
        grant_to_model(g).insert(txn).await.map_err(|e| map_grant_err(e, &g.role_key))?;
        Ok(())
    }

    async fn revoke_in(&self, tx: &dyn Transaction, id: Uuid) -> Result<bool, AuthzError> {
        let txn = recover_txn(tx).map_err(map_txn_err)?;
        let result = role_grant::Entity::delete_by_id(id).exec(txn).await.map_err(map_err)?;
        Ok(result.rows_affected > 0)
    }

    async fn list_all(&self) -> Result<Vec<RoleGrant>, AuthzError> {
        let models = role_grant::Entity::find().all(&self.db).await.map_err(map_err)?;
        models.into_iter().map(model_to_grant).collect()
    }

    async fn list_by_principal(&self, p: &PrincipalId) -> Result<Vec<RoleGrant>, AuthzError> {
        let models = role_grant::Entity::find().filter(role_grant::Column::PrincipalId.eq(p.uuid())).all(&self.db).await.map_err(map_err)?;
        models.into_iter().map(model_to_grant).collect()
    }

    async fn find(&self, id: Uuid) -> Result<Option<RoleGrant>, AuthzError> {
        let Some(model) = role_grant::Entity::find_by_id(id).one(&self.db).await.map_err(map_err)? else {
            return Ok(None);
        };
        Ok(Some(model_to_grant(model)?))
    }
}

/// The fixed head of `RoleGrantQuery::find`'s statement (SMA-676). The inner join on
/// `principal` supplies the kind (every grant has a principal row, `fk_role_grant_principal`,
/// so the join drops nothing unless a kind filter asks it to). The selected columns are exactly
/// `role_grant`'s own, in `role_grant::Model`'s field order, so
/// `role_grant::Model::find_by_statement` maps the row directly.
const FIND_SELECT: &str = r#"
SELECT g.id, g.principal_id, g.role_key, g.scope_kind, g.scope_node_prn, g.scope_org_id,
       g.scope_team_id, g.scope_project_id, g.linked_policy_id, g.created_at
  FROM "role_grant" g JOIN "principal" pr ON pr.id = g.principal_id"#;

/// The predicates and bound values of one `find` statement (SMA-699). Each set filter adds one
/// plain `col = $n` predicate. There is no `($n IS NULL OR col = $n)` form: under a generic plan
/// (sqlx caches `find_by_statement` as a named prepared statement) that form stops the planner
/// from using any index.
#[derive(Default)]
struct FindSql {
    predicates: Vec<String>,
    values: Vec<sea_orm::Value>,
}

impl FindSql {
    /// Adds `<column> = $n::<cast>` and binds `value` as `$n`.
    fn bind(&mut self, column: &str, cast: &str, value: sea_orm::Value) {
        self.values.push(value);
        self.predicates.push(format!("{column} = ${}::{cast}", self.values.len()));
    }

    /// Adds a predicate that binds no value.
    fn literal(&mut self, predicate: &str) {
        self.predicates.push(predicate.to_owned());
    }

    /// The full statement. `limit` and `offset` take the last two numbers (D6). No predicates
    /// gives `WHERE FALSE` (E7): D3 guarantees a principal or a scope, so no caller reaches it,
    /// and if one does it must list nothing, not every tenant's grants.
    fn render(mut self, limit: u64, offset: u64) -> Statement {
        let predicates = if self.predicates.is_empty() { "FALSE".to_owned() } else { self.predicates.join("\n   AND ") };
        self.values.push(limit.into());
        let limit_n = self.values.len();
        self.values.push(offset.into());
        let offset_n = self.values.len();
        let sql = format!("{FIND_SELECT}\n WHERE {predicates}\n ORDER BY g.principal_id, g.id\n LIMIT ${limit_n} OFFSET ${offset_n}");
        Statement::from_sql_and_values(DbBackend::Postgres, sql, self.values)
    }
}

/// `RoleGrantQuery::find`'s statement (SMA-676, SMA-699). One predicate per set filter, in a
/// fixed order: principal, scope, role, kind. D5: a node scope matches its stored canonical PRN
/// exactly (`ix_role_grant_scope_node_prn_principal_id`, m0012, serves it); the Root sentinel
/// matches `scope_kind = 'root'` and binds nothing (spec E3: no stored-PRN invariant for Root).
///
/// `pub` but `#[doc(hidden)]`, so `tests/authz_role_grant_query_plan.rs` can `EXPLAIN` the exact
/// statement, not a copy that can drift (the SMA-469 precedent, `published_sweep_sql`).
#[doc(hidden)]
#[must_use]
pub fn find_statement(f: &RoleGrantFilter, limit: u64, offset: u64) -> Statement {
    let mut q = FindSql::default();
    if let Some(p) = f.principal() {
        q.bind("g.principal_id", "uuid", p.uuid().into());
    }
    match f.scope() {
        None => {}
        Some(GrantScope::Root) => q.literal("g.scope_kind = 'root'"),
        Some(scope @ GrantScope::Node(_)) => q.bind("g.scope_node_prn", "text", scope.canonical_prn().into()),
    }
    if let Some(role_key) = f.role_key() {
        q.bind("g.role_key", "text", role_key.to_owned().into());
    }
    if let Some(kind) = f.principal_kind() {
        q.bind("pr.kind", "text", kind.as_str().to_owned().into());
    }
    q.render(limit, offset)
}

#[async_trait]
impl RoleGrantQuery for PgRoleGrantStore {
    async fn find(&self, f: &RoleGrantFilter, limit: u64, offset: u64) -> Result<Vec<RoleGrant>, AuthzError> {
        let models = role_grant::Model::find_by_statement(find_statement(f, limit, offset)).all(&self.db).await.map_err(map_err)?;
        models.into_iter().map(model_to_grant).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paigasus_iam_core::{OrganizationId, PrincipalKind};
    use sea_orm::Value;

    fn principal() -> PrincipalId {
        PrincipalId::from_prn(Prn::build("iam", "", None, "principal", Uuid::from_u128(7)).unwrap())
    }

    fn org_scope() -> GrantScope {
        GrantScope::Node(TenancyNodeRef::Organization(OrganizationId::from_uuid(Uuid::from_u128(9))))
    }

    /// Every `$n` in `sql`, in order of appearance.
    fn placeholders(sql: &str) -> Vec<usize> {
        let bytes = sql.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'$' {
                let start = i + 1;
                let mut j = start;
                while j < bytes.len() && bytes[j].is_ascii_digit() {
                    j += 1;
                }
                if j > start {
                    out.push(sql[start..j].parse().unwrap());
                }
                i = j;
            } else {
                i += 1;
            }
        }
        out
    }

    fn values(stmt: &Statement) -> Vec<Value> {
        stmt.values.clone().map(|v| v.0).unwrap_or_default()
    }

    /// Every filter shape `RoleGrantFilter::new` accepts: principal x scope {none, node, Root}
    /// x role x kind, minus the 4 shapes with no principal and no scope (D3). 20 shapes.
    fn all_shapes() -> Vec<RoleGrantFilter> {
        let mut out = Vec::new();
        for principal in [None, Some(principal())] {
            for scope in [None, Some(org_scope()), Some(GrantScope::Root)] {
                for role in [None, Some("gateway_user".to_string())] {
                    for kind in [None, Some(PrincipalKind::User)] {
                        if let Some(f) = RoleGrantFilter::new(principal.clone(), scope.clone(), role.clone(), kind) {
                            out.push(f);
                        }
                    }
                }
            }
        }
        assert_eq!(out.len(), 20);
        out
    }

    #[test]
    fn no_shape_keeps_an_is_null_or_predicate() {
        for f in all_shapes() {
            let stmt = find_statement(&f, 200, 0);
            assert!(!stmt.sql.contains("IS NULL"), "{f:?}: {}", stmt.sql);
            assert!(!stmt.sql.contains("IS FALSE"), "{f:?}: {}", stmt.sql);
        }
    }

    #[test]
    fn placeholders_are_numbered_one_to_k_with_no_gap() {
        for f in all_shapes() {
            let stmt = find_statement(&f, 200, 0);
            let mut seen = placeholders(&stmt.sql);
            seen.sort_unstable();
            seen.dedup();
            let k = values(&stmt).len();
            assert_eq!(seen, (1..=k).collect::<Vec<_>>(), "{f:?}: {}", stmt.sql);
        }
    }

    #[test]
    fn limit_and_offset_are_the_last_two_values_after_the_fixed_order() {
        for f in all_shapes() {
            let stmt = find_statement(&f, 17, 34);
            let vals = values(&stmt);
            let k = vals.len();
            assert_eq!(vals[k - 2], Value::from(17u64), "{f:?}");
            assert_eq!(vals[k - 1], Value::from(34u64), "{f:?}");
            assert!(stmt.sql.contains(&format!("ORDER BY g.principal_id, g.id\n LIMIT ${} OFFSET ${k}", k - 1)), "{f:?}: {}", stmt.sql);
        }
    }

    #[test]
    fn shape_b_binds_every_filter_in_the_fixed_order() {
        let f = RoleGrantFilter::new(Some(principal()), Some(org_scope()), Some("gateway_user".to_string()), Some(PrincipalKind::User)).unwrap();
        let stmt = find_statement(&f, 50, 0);
        assert!(stmt.sql.contains("g.principal_id = $1::uuid"), "{}", stmt.sql);
        assert!(stmt.sql.contains("g.scope_node_prn = $2::text"), "{}", stmt.sql);
        assert!(stmt.sql.contains("g.role_key = $3::text"), "{}", stmt.sql);
        assert!(stmt.sql.contains("pr.kind = $4::text"), "{}", stmt.sql);
        assert_eq!(
            values(&stmt),
            vec![
                Value::from(Uuid::from_u128(7)),
                Value::from(org_scope().canonical_prn()),
                Value::from("gateway_user".to_string()),
                Value::from("user".to_string()),
                Value::from(50u64),
                Value::from(0u64),
            ]
        );
    }

    #[test]
    fn shape_a_has_only_the_scope_and_kind_predicates() {
        let f = RoleGrantFilter::new(None, Some(org_scope()), None, Some(PrincipalKind::User)).unwrap();
        let stmt = find_statement(&f, 200, 800);
        assert!(stmt.sql.contains("g.scope_node_prn = $1::text"), "{}", stmt.sql);
        assert!(stmt.sql.contains("pr.kind = $2::text"), "{}", stmt.sql);
        assert!(!stmt.sql.contains("g.principal_id ="), "{}", stmt.sql);
        assert!(!stmt.sql.contains("g.role_key ="), "{}", stmt.sql);
        assert!(!stmt.sql.contains("g.scope_kind ="), "{}", stmt.sql);
        assert_eq!(values(&stmt).len(), 4);
    }

    #[test]
    fn root_filters_on_scope_kind_and_binds_no_scope_value() {
        let f = RoleGrantFilter::new(None, Some(GrantScope::Root), None, None).unwrap();
        let stmt = find_statement(&f, 200, 0);
        assert!(stmt.sql.contains("g.scope_kind = 'root'"), "{}", stmt.sql);
        assert!(!stmt.sql.contains("g.scope_node_prn ="), "{}", stmt.sql);
        assert_eq!(values(&stmt), vec![Value::from(200u64), Value::from(0u64)]);
    }

    #[test]
    fn the_select_list_and_the_join_do_not_change() {
        let f = RoleGrantFilter::new(Some(principal()), None, None, None).unwrap();
        let stmt = find_statement(&f, 1, 0);
        assert!(stmt.sql.contains(
            "SELECT g.id, g.principal_id, g.role_key, g.scope_kind, g.scope_node_prn, g.scope_org_id,\n       g.scope_team_id, g.scope_project_id, g.linked_policy_id, g.created_at\n  FROM \"role_grant\" g JOIN \"principal\" pr ON pr.id = g.principal_id"
        ));
    }

    /// E7: no caller can build an empty filter (`RoleGrantFilter::new` refuses it), so this
    /// drives the private core directly. An empty filter must return no rows, never every grant.
    #[test]
    fn an_empty_predicate_list_matches_no_row() {
        let stmt = FindSql::default().render(200, 0);
        assert!(stmt.sql.contains(" WHERE FALSE\n"), "{}", stmt.sql);
        assert_eq!(values(&stmt), vec![Value::from(200u64), Value::from(0u64)]);
    }
}
