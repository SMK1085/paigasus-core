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

use super::entities::{tool_catalog_entry as catalog, tool_policy_set_version as versions, tool_policy_state as state};
use super::map_err;
use super::uow::recover_txn;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use paigasus_iam_core::tool_policy::{
    DroppedField, StoredTool, ToolEntry, ToolKind, ToolName, ToolPolicyRepository, ToolPolicySetKey, ToolPolicyState, ToolPolicyVersion, ToolSource, VersionOrigin, VersionStatus,
};
use paigasus_iam_core::{RepositoryError, Transaction};
use sea_orm::sea_query::OnConflict;
use sea_orm::{ActiveValue::Set, ColumnTrait, ConnectionTrait, DatabaseConnection, DbBackend, EntityTrait, QueryFilter, QueryOrder, QuerySelect, Statement};
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
        VersionOrigin::CatalogChange => "catalog_change",
    }
}

fn origin_from_str(s: &str) -> Result<VersionOrigin, RepositoryError> {
    match s {
        "save" => Ok(VersionOrigin::Save),
        "catalog_change" => Ok(VersionOrigin::CatalogChange),
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
        compiled: m.compiled,
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
    })
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
            None => Ok(ToolPolicyState { catalog_revision: 0, live_version: 0 }),
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
        let txn = recover_txn(tx)?;
        let (kind, org) = key_parts(&v.key);
        let version = to_i64(v.version)?;
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

        let row = versions::ActiveModel {
            kind: Set(kind.to_string()),
            org_id: Set(org),
            version: Set(version),
            source: Set(v.source.clone()),
            compiled: Set(v.compiled.clone()),
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

        let updated = state::Entity::update_many()
            .col_expr(state::Column::LiveVersion, sea_orm::sea_query::Expr::value(version))
            .filter(state::Column::SetKey.eq(set_key(&v.key)))
            .exec(txn)
            .await
            .map_err(map_err)?;
        if updated.rows_affected == 0 {
            // The caller did not lock the state first. Roll back through the error.
            return Err(RepositoryError::NotFound);
        }
        Ok(())
    }
}
