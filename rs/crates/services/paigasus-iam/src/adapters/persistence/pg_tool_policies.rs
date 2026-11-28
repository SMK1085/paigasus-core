// SPDX-License-Identifier: Apache-2.0

//! Postgres-backed `ToolPolicyRepository` (SeaORM, SMA-774). It stores the tool catalog and the
//! versions of the tool policy sets (spec sections 4.2, 4.4, 4.5 and 6).
//!
//! A writer calls `lock_state_in` first. That makes the `tool_policy_state` row if it is missing
//! and locks it with `SELECT … FOR UPDATE`. All writers of one set then run one after the other.
//! The `compiled` column holds the protobuf bytes that the service hands in. This adapter does
//! not decode them.
//!
//! Database errors map through `persistence::map_err`.

use super::entities::{tool_catalog_entry as catalog, tool_policy_set_version as versions, tool_policy_settings as settings_rows, tool_policy_state as state};
use super::map_err;
use super::uow::recover_txn;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use paigasus_iam_core::tool_policy::{
    DroppedField, FailMode, HeldReport, HeldStatus, HeldVersion, StoredTool, ToolEntry, ToolKind, ToolName, ToolPolicyMode, ToolPolicyRepository, ToolPolicySetKey, ToolPolicySettings,
    ToolPolicyState, ToolPolicyVersion, ToolSource, VersionOrigin, VersionStatus,
};
use paigasus_iam_core::{ConflictKind, RepositoryError, Transaction};
use sea_orm::sea_query::{Expr, OnConflict};
use sea_orm::{
    AccessMode, ActiveValue::Set, ColumnTrait, ConnectionTrait, DatabaseConnection, DbBackend, EntityTrait, IsolationLevel, QueryFilter, QueryOrder, QuerySelect, Statement, TransactionTrait,
};
use serde_json::{Value, json};
use uuid::Uuid;

/// Cheap to clone: `DatabaseConnection` is an `Arc`-backed pool handle.
#[derive(Clone)]
pub struct PgToolPolicies {
    db: DatabaseConnection,
}

impl PgToolPolicies {
    #[must_use]
    pub fn new(db: DatabaseConnection) -> Self {
        PgToolPolicies { db }
    }
}

fn corrupt(what: &str) -> RepositoryError {
    RepositoryError::Backend(Box::new(std::io::Error::other(format!("tool policy storage: {what}"))))
}

fn to_i64(v: u64) -> Result<i64, RepositoryError> {
    i64::try_from(v).map_err(|_| corrupt("a counter does not fit in a BIGINT"))
}

fn from_i64(v: i64) -> Result<u64, RepositoryError> {
    u64::try_from(v).map_err(|_| corrupt("a stored counter is negative"))
}

fn set_key(key: &ToolPolicySetKey) -> String {
    match key {
        ToolPolicySetKey::Floor => "floor".to_string(),
        ToolPolicySetKey::Tenant(org) => format!("tenant:{org}"),
    }
}

fn key_parts(key: &ToolPolicySetKey) -> (&'static str, Option<Uuid>) {
    match key {
        ToolPolicySetKey::Floor => ("floor", None),
        ToolPolicySetKey::Tenant(org) => ("tenant", Some(*org)),
    }
}

fn key_from_parts(kind: &str, org: Option<Uuid>) -> Result<ToolPolicySetKey, RepositoryError> {
    match (kind, org) {
        ("floor", None) => Ok(ToolPolicySetKey::Floor),
        ("tenant", Some(org)) => Ok(ToolPolicySetKey::Tenant(org)),
        _ => Err(corrupt("a version row has an unknown set kind")),
    }
}

fn kind_str(k: ToolKind) -> &'static str {
    match k {
        ToolKind::Function => "function",
        ToolKind::Custom => "custom",
    }
}

fn kind_from_str(s: &str) -> Result<ToolKind, RepositoryError> {
    match s {
        "function" => Ok(ToolKind::Function),
        "custom" => Ok(ToolKind::Custom),
        _ => Err(corrupt("a catalog row has an unknown kind")),
    }
}

fn source_str(s: ToolSource) -> &'static str {
    match s {
        ToolSource::Manual => "manual",
        ToolSource::OpenAi => "openai",
        ToolSource::Mcp => "mcp",
    }
}

fn source_from_str(s: &str) -> Result<ToolSource, RepositoryError> {
    match s {
        "manual" => Ok(ToolSource::Manual),
        "openai" => Ok(ToolSource::OpenAi),
        "mcp" => Ok(ToolSource::Mcp),
        _ => Err(corrupt("a catalog row has an unknown source")),
    }
}

fn status_str(s: VersionStatus) -> &'static str {
    match s {
        VersionStatus::Live => "live",
        VersionStatus::Superseded => "superseded",
    }
}

fn status_from_str(s: &str) -> Result<VersionStatus, RepositoryError> {
    match s {
        "live" => Ok(VersionStatus::Live),
        "superseded" => Ok(VersionStatus::Superseded),
        _ => Err(corrupt("a readable version row has an unexpected status")),
    }
}

fn origin_str(o: VersionOrigin) -> &'static str {
    match o {
        VersionOrigin::Save => "save",
        VersionOrigin::Starter => "starter",
        VersionOrigin::CatalogChange => "catalog_change",
        VersionOrigin::Recompile => "recompile",
        VersionOrigin::Accept => "accept",
    }
}

fn origin_from_str(s: &str) -> Result<VersionOrigin, RepositoryError> {
    match s {
        "save" => Ok(VersionOrigin::Save),
        "starter" => Ok(VersionOrigin::Starter),
        "catalog_change" => Ok(VersionOrigin::CatalogChange),
        "recompile" => Ok(VersionOrigin::Recompile),
        "accept" => Ok(VersionOrigin::Accept),
        _ => Err(corrupt("a readable version row has an unexpected origin")),
    }
}

fn dropped_to_json(fields: &[DroppedField]) -> Value {
    Value::Array(fields.iter().map(|f| json!({"path": f.path, "reason": f.reason})).collect())
}

fn dropped_from_json(v: &Value) -> Result<Vec<DroppedField>, RepositoryError> {
    let items = v.as_array().ok_or_else(|| corrupt("dropped_fields is not an array"))?;
    items
        .iter()
        .map(|item| {
            let path = item.get("path").and_then(Value::as_str);
            let reason = item.get("reason").and_then(Value::as_str);
            match (path, reason) {
                (Some(path), Some(reason)) => Ok(DroppedField {
                    path: path.to_string(),
                    reason: reason.to_string(),
                }),
                _ => Err(corrupt("a dropped field is malformed")),
            }
        })
        .collect()
}

fn model_to_tool(m: catalog::Model) -> Result<StoredTool, RepositoryError> {
    Ok(StoredTool {
        entry: ToolEntry {
            name: ToolName {
                display: m.display_name,
                name_key: m.name_key,
            },
            kind: kind_from_str(&m.kind)?,
            input_schema: m.input_schema,
            cedar_input_type: m.cedar_input_type,
            dropped_fields: dropped_from_json(&m.dropped_fields)?,
            source: source_from_str(&m.source)?,
        },
        created_at: m.created_at,
        updated_at: m.updated_at,
    })
}

fn model_to_version(m: versions::Model) -> Result<ToolPolicyVersion, RepositoryError> {
    Ok(ToolPolicyVersion {
        key: key_from_parts(&m.kind, m.org_id)?,
        version: from_i64(m.version)?,
        source: m.source,
        compiled: m.compiled.ok_or_else(|| corrupt("a live or superseded version row has no compiled bytes"))?,
        compiler_generation: u32::try_from(m.compiler_generation).map_err(|_| corrupt("negative compiler_generation"))?,
        schema_revision: u32::try_from(m.schema_revision).map_err(|_| corrupt("negative schema_revision"))?,
        catalog_revision: m.catalog_revision.map(from_i64).transpose()?,
        status: status_from_str(&m.status)?,
        origin: origin_from_str(&m.origin)?,
        created_by: m.created_by,
        created_at: m.created_at,
    })
}

fn model_to_state(m: &state::Model) -> Result<ToolPolicyState, RepositoryError> {
    Ok(ToolPolicyState {
        catalog_revision: from_i64(m.catalog_revision)?,
        live_version: from_i64(m.live_version)?,
        last_version: from_i64(m.last_version)?,
    })
}

fn held_status_from_str(s: &str) -> Result<HeldStatus, RepositoryError> {
    match s {
        "held" => Ok(HeldStatus::Held),
        "discarded" => Ok(HeldStatus::Discarded),
        _ => Err(corrupt("a held version row has an unexpected status")),
    }
}

fn model_to_held(m: versions::Model) -> Result<HeldVersion, RepositoryError> {
    let report = m.report.ok_or_else(|| corrupt("a held version row has no report"))?;
    let report: HeldReport = serde_json::from_value(report).map_err(|_| corrupt("a held version report is malformed"))?;
    Ok(HeldVersion {
        key: key_from_parts(&m.kind, m.org_id)?,
        version: from_i64(m.version)?,
        base_version: from_i64(m.base_version.ok_or_else(|| corrupt("a held version row has no base_version"))?)?,
        source: m.source,
        compiled: m.compiled,
        compiler_generation: u32::try_from(m.compiler_generation).map_err(|_| corrupt("negative compiler_generation"))?,
        schema_revision: u32::try_from(m.schema_revision).map_err(|_| corrupt("negative schema_revision"))?,
        catalog_revision: m.catalog_revision.map(from_i64).transpose()?,
        status: held_status_from_str(&m.status)?,
        report,
        created_by: m.created_by,
        created_at: m.created_at,
    })
}

/// Set the held row of a set to discarded, if there is one (only the row `only_version` when
/// it is given). Returns the number of rows changed.
async fn discard_held_rows(txn: &impl ConnectionTrait, key: &ToolPolicySetKey, only_version: Option<i64>) -> Result<u64, RepositoryError> {
    let (kind, org) = key_parts(key);
    let mut q = versions::Entity::update_many()
        .col_expr(versions::Column::Status, Expr::value("discarded"))
        .filter(versions::Column::Kind.eq(kind))
        .filter(versions::Column::Status.eq("held"));
    q = match org {
        Some(org) => q.filter(versions::Column::OrgId.eq(org)),
        None => q.filter(versions::Column::OrgId.is_null()),
    };
    if let Some(v) = only_version {
        q = q.filter(versions::Column::Version.eq(v));
    }
    Ok(q.exec(txn).await.map_err(map_err)?.rows_affected)
}

/// The filter of the version rows of one set.
fn version_filter(key: &ToolPolicySetKey) -> sea_orm::Select<versions::Entity> {
    let (kind, org) = key_parts(key);
    let q = versions::Entity::find().filter(versions::Column::Kind.eq(kind));
    match org {
        Some(org) => q.filter(versions::Column::OrgId.eq(org)),
        None => q.filter(versions::Column::OrgId.is_null()),
    }
}

/// Make the state row of a set if it is missing. A missing organization gives a foreign-key
/// violation, which `map_err` reports as `NotFound`.
async fn ensure_state_row(conn: &impl ConnectionTrait, key: &ToolPolicySetKey) -> Result<(), RepositoryError> {
    let (kind, org) = key_parts(key);
    conn.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO tool_policy_state (set_key, kind, org_id) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
        [set_key(key).into(), kind.into(), org.into()],
    ))
    .await
    .map_err(map_err)?;
    Ok(())
}

#[async_trait]
impl ToolPolicyRepository for PgToolPolicies {
    async fn state(&self, key: &ToolPolicySetKey) -> Result<ToolPolicyState, RepositoryError> {
        match state::Entity::find_by_id(set_key(key)).one(&self.db).await.map_err(map_err)? {
            Some(m) => model_to_state(&m),
            None => Ok(ToolPolicyState {
                catalog_revision: 0,
                live_version: 0,
                last_version: 0,
            }),
        }
    }

    async fn catalog(&self, org: Uuid) -> Result<Vec<StoredTool>, RepositoryError> {
        let rows = catalog::Entity::find()
            .filter(catalog::Column::OrgId.eq(org))
            .order_by_asc(catalog::Column::NameKey)
            .all(&self.db)
            .await
            .map_err(map_err)?;
        rows.into_iter().map(model_to_tool).collect()
    }

    async fn catalog_snapshot(&self, org: Uuid) -> Result<(Vec<StoredTool>, u64), RepositoryError> {
        // REPEATABLE READ: both SELECTs read one snapshot, so a catalog write that commits between
        // them is in neither. Two plain reads could return the new tools with the old revision.
        let txn = self.db.begin_with_config(Some(IsolationLevel::RepeatableRead), Some(AccessMode::ReadOnly)).await.map_err(map_err)?;
        let revision = match state::Entity::find_by_id(set_key(&ToolPolicySetKey::Tenant(org))).one(&txn).await.map_err(map_err)? {
            Some(m) => model_to_state(&m)?.catalog_revision,
            None => 0,
        };
        let rows = catalog::Entity::find()
            .filter(catalog::Column::OrgId.eq(org))
            .order_by_asc(catalog::Column::NameKey)
            .all(&txn)
            .await
            .map_err(map_err)?;
        txn.commit().await.map_err(map_err)?;
        let tools = rows.into_iter().map(model_to_tool).collect::<Result<Vec<_>, _>>()?;
        Ok((tools, revision))
    }

    async fn live(&self, key: &ToolPolicySetKey) -> Result<Option<ToolPolicyVersion>, RepositoryError> {
        let row = version_filter(key).filter(versions::Column::Status.eq("live")).one(&self.db).await.map_err(map_err)?;
        row.map(model_to_version).transpose()
    }

    async fn version(&self, key: &ToolPolicySetKey, version: u64) -> Result<Option<ToolPolicyVersion>, RepositoryError> {
        let row = version_filter(key)
            .filter(versions::Column::Version.eq(to_i64(version)?))
            .filter(versions::Column::Status.is_in(["live", "superseded"]))
            .one(&self.db)
            .await
            .map_err(map_err)?;
        row.map(model_to_version).transpose()
    }

    async fn lock_state_in(&self, tx: &dyn Transaction, key: &ToolPolicySetKey) -> Result<ToolPolicyState, RepositoryError> {
        let txn = recover_txn(tx)?;
        ensure_state_row(txn, key).await?;
        let row = state::Entity::find_by_id(set_key(key)).lock_exclusive().one(txn).await.map_err(map_err)?;
        model_to_state(&row.ok_or_else(|| corrupt("the state row vanished under its own lock"))?)
    }

    async fn apply_catalog_in(&self, tx: &dyn Transaction, org: Uuid, upserts: &[ToolEntry], deletes: &[String], now: DateTime<Utc>) -> Result<u64, RepositoryError> {
        let txn = recover_txn(tx)?;
        let key = ToolPolicySetKey::Tenant(org);
        ensure_state_row(txn, &key).await?;
        let bumped = txn
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "UPDATE tool_policy_state SET catalog_revision = catalog_revision + 1 WHERE set_key = $1 RETURNING catalog_revision",
                [set_key(&key).into()],
            ))
            .await
            .map_err(map_err)?
            .ok_or_else(|| corrupt("the state row vanished under its own lock"))?;
        let revision = from_i64(bumped.try_get::<i64>("", "catalog_revision").map_err(map_err)?)?;

        for name_key in deletes {
            catalog::Entity::delete_by_id((org, name_key.clone())).exec(txn).await.map_err(map_err)?;
        }
        for entry in upserts {
            let row = catalog::ActiveModel {
                org_id: Set(org),
                name_key: Set(entry.name.name_key.clone()),
                display_name: Set(entry.name.display.clone()),
                kind: Set(kind_str(entry.kind).to_string()),
                input_schema: Set(entry.input_schema.clone()),
                cedar_input_type: Set(entry.cedar_input_type.clone()),
                dropped_fields: Set(dropped_to_json(&entry.dropped_fields)),
                source: Set(source_str(entry.source).to_string()),
                created_at: Set(now),
                updated_at: Set(now),
            };
            // `created_at` is not in the update list: an upsert keeps the first time.
            catalog::Entity::insert(row)
                .on_conflict(
                    OnConflict::columns([catalog::Column::OrgId, catalog::Column::NameKey])
                        .update_columns([
                            catalog::Column::DisplayName,
                            catalog::Column::Kind,
                            catalog::Column::InputSchema,
                            catalog::Column::CedarInputType,
                            catalog::Column::DroppedFields,
                            catalog::Column::Source,
                            catalog::Column::UpdatedAt,
                        ])
                        .to_owned(),
                )
                .exec_without_returning(txn)
                .await
                .map_err(map_err)?;
        }
        Ok(revision)
    }

    async fn insert_live_version_in(&self, tx: &dyn Transaction, v: &ToolPolicyVersion) -> Result<(), RepositoryError> {
        // This method always writes a live row. Any other status is refused, not ignored.
        if v.status != VersionStatus::Live {
            return Err(corrupt("insert_live_version_in needs a version with the status Live"));
        }
        let txn = recover_txn(tx)?;
        let (kind, org) = key_parts(&v.key);
        let version = to_i64(v.version)?;
        // Compare-and-set on the state row, before any other write. It succeeds only if the
        // larger of the two counters is exactly one below the new version. It is the backstop: the
        // caller must still hold the row lock from `lock_state_in`. A stale writer, a skipped
        // version and a missing state row all change no row and give the version conflict.
        let swapped = state::Entity::update_many()
            .col_expr(state::Column::LiveVersion, Expr::value(version))
            .col_expr(state::Column::LastVersion, Expr::value(version))
            .filter(state::Column::SetKey.eq(set_key(&v.key)))
            .filter(Expr::cust_with_values("GREATEST(live_version, last_version) = $1", [version - 1]))
            .exec(txn)
            .await
            .map_err(map_err)?;
        if swapped.rows_affected == 0 {
            return Err(RepositoryError::Conflict(ConflictKind::ToolPolicyVersion));
        }
        // The old live row must go first: the live index allows one live row for each set.
        let supersede = versions::Entity::update_many()
            .col_expr(versions::Column::Status, sea_orm::sea_query::Expr::value("superseded"))
            .filter(versions::Column::Kind.eq(kind))
            .filter(versions::Column::Status.eq("live"));
        let supersede = match org {
            Some(org) => supersede.filter(versions::Column::OrgId.eq(org)),
            None => supersede.filter(versions::Column::OrgId.is_null()),
        };
        supersede.exec(txn).await.map_err(map_err)?;
        // Any new live version discards the held row of the set (plan H7).
        discard_held_rows(txn, &v.key, None).await?;

        let row = versions::ActiveModel {
            kind: Set(kind.to_string()),
            org_id: Set(org),
            version: Set(version),
            source: Set(v.source.clone()),
            compiled: Set(Some(v.compiled.clone())),
            compiler_generation: Set(i32::try_from(v.compiler_generation).map_err(|_| corrupt("compiler_generation does not fit in an INTEGER"))?),
            schema_revision: Set(i32::try_from(v.schema_revision).map_err(|_| corrupt("schema_revision does not fit in an INTEGER"))?),
            catalog_revision: Set(v.catalog_revision.map(to_i64).transpose()?),
            status: Set(status_str(v.status).to_string()),
            origin: Set(origin_str(v.origin).to_string()),
            base_version: Set(None),
            report: Set(None),
            created_by: Set(v.created_by.clone()),
            created_at: Set(v.created_at),
        };
        versions::Entity::insert(row).exec_without_returning(txn).await.map_err(map_err)?;

        Ok(())
    }

    async fn stale_live_versions(&self, compiler_generation: u32, schema_revision: u32) -> Result<Vec<ToolPolicyVersion>, RepositoryError> {
        let generation = i32::try_from(compiler_generation).map_err(|_| corrupt("compiler_generation does not fit in an INTEGER"))?;
        let revision = i32::try_from(schema_revision).map_err(|_| corrupt("schema_revision does not fit in an INTEGER"))?;
        let rows = versions::Entity::find()
            .filter(versions::Column::Status.eq("live"))
            .filter(
                sea_orm::Condition::any()
                    .add(versions::Column::CompilerGeneration.ne(generation))
                    .add(versions::Column::SchemaRevision.ne(revision)),
            )
            .order_by_asc(versions::Column::Kind)
            .order_by_asc(versions::Column::OrgId)
            .all(&self.db)
            .await
            .map_err(map_err)?;
        rows.into_iter().map(model_to_version).collect()
    }

    async fn max_stored_revisions(&self) -> Result<(u32, u32), RepositoryError> {
        let row = self
            .db
            .query_one_raw(Statement::from_string(
                DbBackend::Postgres,
                "SELECT COALESCE(MAX(compiler_generation), 0) AS g, COALESCE(MAX(schema_revision), 0) AS s FROM tool_policy_set_version",
            ))
            .await
            .map_err(map_err)?
            .ok_or_else(|| corrupt("an aggregate query returned no row"))?;
        let g = row.try_get::<i32>("", "g").map_err(map_err)?;
        let s = row.try_get::<i32>("", "s").map_err(map_err)?;
        Ok((
            u32::try_from(g).map_err(|_| corrupt("negative compiler_generation"))?,
            u32::try_from(s).map_err(|_| corrupt("negative schema_revision"))?,
        ))
    }

    async fn held(&self, key: &ToolPolicySetKey) -> Result<Option<HeldVersion>, RepositoryError> {
        let row = version_filter(key).filter(versions::Column::Status.eq("held")).one(&self.db).await.map_err(map_err)?;
        row.map(model_to_held).transpose()
    }

    async fn list_held(&self, org: Option<Uuid>, limit: u64, offset: u64) -> Result<Vec<HeldVersion>, RepositoryError> {
        let mut q = versions::Entity::find().filter(versions::Column::Status.eq("held"));
        if let Some(org) = org {
            q = q.filter(versions::Column::Kind.eq("tenant")).filter(versions::Column::OrgId.eq(org));
        }
        // ORDER BY kind, org_id: 'floor' sorts before 'tenant', so the floor comes first. The
        // page is in SQL, so a page does not read every held row with its blob and source.
        let rows = q
            .order_by_asc(versions::Column::Kind)
            .order_by_asc(versions::Column::OrgId)
            .limit(limit)
            .offset(offset)
            .all(&self.db)
            .await
            .map_err(map_err)?;
        rows.into_iter().map(model_to_held).collect()
    }

    async fn proposal_exists(&self, key: &ToolPolicySetKey, base_version: u64, compiler_generation: u32, schema_revision: u32) -> Result<bool, RepositoryError> {
        let generation = i32::try_from(compiler_generation).map_err(|_| corrupt("compiler_generation does not fit in an INTEGER"))?;
        let revision = i32::try_from(schema_revision).map_err(|_| corrupt("schema_revision does not fit in an INTEGER"))?;
        let row = version_filter(key)
            .filter(versions::Column::Status.is_in(["held", "discarded"]))
            .filter(versions::Column::BaseVersion.eq(to_i64(base_version)?))
            .filter(versions::Column::CompilerGeneration.eq(generation))
            .filter(versions::Column::SchemaRevision.eq(revision))
            .one(&self.db)
            .await
            .map_err(map_err)?;
        Ok(row.is_some())
    }

    async fn insert_held_in(&self, tx: &dyn Transaction, h: &HeldVersion) -> Result<(), RepositoryError> {
        if h.status != HeldStatus::Held {
            return Err(corrupt("insert_held_in needs a version with the status Held"));
        }
        let txn = recover_txn(tx)?;
        let (kind, org) = key_parts(&h.key);
        let version = to_i64(h.version)?;
        // Compare-and-set on the state row: the number must be `next_version()`. Only the
        // counter `last_version` moves.
        let swapped = state::Entity::update_many()
            .col_expr(state::Column::LastVersion, Expr::value(version))
            .filter(state::Column::SetKey.eq(set_key(&h.key)))
            .filter(Expr::cust_with_values("GREATEST(live_version, last_version) = $1", [version - 1]))
            .exec(txn)
            .await
            .map_err(map_err)?;
        if swapped.rows_affected == 0 {
            return Err(RepositoryError::Conflict(ConflictKind::ToolPolicyVersion));
        }
        // The one-held index allows one held row for each set: the older one goes first (H8).
        discard_held_rows(txn, &h.key, None).await?;
        let report = serde_json::to_value(&h.report).map_err(|_| corrupt("a held report does not serialize"))?;
        let row = versions::ActiveModel {
            kind: Set(kind.to_string()),
            org_id: Set(org),
            version: Set(version),
            source: Set(h.source.clone()),
            compiled: Set(h.compiled.clone()),
            compiler_generation: Set(i32::try_from(h.compiler_generation).map_err(|_| corrupt("compiler_generation does not fit in an INTEGER"))?),
            schema_revision: Set(i32::try_from(h.schema_revision).map_err(|_| corrupt("schema_revision does not fit in an INTEGER"))?),
            catalog_revision: Set(h.catalog_revision.map(to_i64).transpose()?),
            status: Set("held".to_string()),
            origin: Set(origin_str(VersionOrigin::Recompile).to_string()),
            base_version: Set(Some(to_i64(h.base_version)?)),
            report: Set(Some(report)),
            created_by: Set(h.created_by.clone()),
            created_at: Set(h.created_at),
        };
        versions::Entity::insert(row).exec_without_returning(txn).await.map_err(map_err)?;
        Ok(())
    }

    async fn discard_held_in(&self, tx: &dyn Transaction, key: &ToolPolicySetKey, version: u64) -> Result<bool, RepositoryError> {
        let txn = recover_txn(tx)?;
        Ok(discard_held_rows(txn, key, Some(to_i64(version)?)).await? > 0)
    }

    async fn settings(&self, org: Uuid) -> Result<Option<ToolPolicySettings>, RepositoryError> {
        let row = settings_rows::Entity::find_by_id(org).one(&self.db).await.map_err(map_err)?;
        row.map(|m| {
            Ok(ToolPolicySettings {
                mode: ToolPolicyMode::parse(&m.mode).ok_or_else(|| corrupt("a settings row has an unknown mode"))?,
                fail_mode: FailMode::parse(&m.fail_mode).ok_or_else(|| corrupt("a settings row has an unknown fail mode"))?,
                updated_at: Some(m.updated_at),
                updated_by: Some(m.updated_by),
                first_enabled_at: m.first_enabled_at,
                webhooks: parse_webhooks(&m.webhooks)?,
            })
        })
        .transpose()
    }

    async fn upsert_settings_in(&self, tx: &dyn Transaction, org: Uuid, s: &ToolPolicySettings, webhooks: Option<Vec<String>>) -> Result<Vec<String>, RepositoryError> {
        let (Some(updated_at), Some(updated_by)) = (s.updated_at, s.updated_by.as_ref()) else {
            return Err(corrupt("upsert_settings_in needs updated_at and updated_by"));
        };
        let txn = recover_txn(tx)?;
        let row = settings_rows::ActiveModel {
            org_id: Set(org),
            mode: Set(s.mode.as_str().to_string()),
            fail_mode: Set(s.fail_mode.as_str().to_string()),
            updated_at: Set(updated_at),
            updated_by: Set(updated_by.clone()),
            first_enabled_at: Set(s.first_enabled_at),
            // `NotSet` on an absent list: a new row takes the column default `'[]'`.
            webhooks: match &webhooks {
                Some(list) => Set(serde_json::to_string(list).map_err(|_| corrupt("the webhook opt-in list does not serialize"))?),
                None => sea_orm::ActiveValue::NotSet,
            },
        };
        let mut update = vec![
            settings_rows::Column::Mode,
            settings_rows::Column::FailMode,
            settings_rows::Column::UpdatedAt,
            settings_rows::Column::UpdatedBy,
        ];
        // An absent list stays out of the update columns, so a stale struct cannot undo a
        // concurrent write.
        if webhooks.is_some() {
            update.push(settings_rows::Column::Webhooks);
        }
        let stored = settings_rows::Entity::insert(row)
            .on_conflict(
                OnConflict::column(settings_rows::Column::OrgId)
                    .update_columns(update)
                    // Written once: a stored value wins over the new one, also under a race.
                    .value(
                        settings_rows::Column::FirstEnabledAt,
                        Expr::cust(r#"COALESCE("tool_policy_settings"."first_enabled_at", EXCLUDED."first_enabled_at")"#),
                    )
                    .to_owned(),
            )
            .exec_with_returning(txn)
            .await
            .map_err(map_err)?;
        parse_webhooks(&stored.webhooks)
    }

    async fn webhooks_for_update_in(&self, tx: &dyn Transaction, org: Uuid) -> Result<Vec<String>, RepositoryError> {
        let txn = recover_txn(tx)?;
        let row = settings_rows::Entity::find_by_id(org).lock_exclusive().one(txn).await.map_err(map_err)?;
        row.map(|m| parse_webhooks(&m.webhooks)).transpose().map(Option::unwrap_or_default)
    }
}

/// Reads the stored opt-in list. A value that is not a JSON array of strings is corrupt.
fn parse_webhooks(raw: &str) -> Result<Vec<String>, RepositoryError> {
    serde_json::from_str::<Vec<String>>(raw).map_err(|_| corrupt("a settings row has a malformed webhook opt-in list"))
}
