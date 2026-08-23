// SPDX-License-Identifier: Apache-2.0

//! `AuditGrpc`: the `AuditService` gRPC server (SMA-446 Task A10) — a thin adapter over
//! `AppState.audit_query`: parse the wire request -> `AuditQueryService::list` -> convert the
//! result, no business logic in this layer (mirrors `grpc::authz`/`grpc::service_accounts`'s
//! posture). `ListAuditEntries` is Root-only, enforced INSIDE `AuditQueryService` itself (see
//! its module doc) — this adapter just forwards the bearer-resolved actor and lets that check
//! do the work, exactly like `ServiceAccountGrpc`'s posture toward its own application
//! services.
//!
//! **Actor extraction:** every RPC here is bearer-enforced by `AuthLayer`/`AuthEnforce`
//! (`grpc::authn`) — `AuditService` is NOT on the `:path` exemption list, so `AuthEnforce`
//! always resolves the caller and inserts an [`AuthContext`] into the request's extensions
//! before this service ever runs (mirrors `grpc::authz`'s identical module doc).

use std::time::Instant;

use paigasus_iam_core::{AuditEntry, AuditFilter, AuditOutcome};
use paigasus_observability::record_grpc;
use paigasus_proto::paigasus::iam::v1::audit_service_server::AuditService;
use paigasus_proto::paigasus::iam::v1::{AuditEntry as ProtoAuditEntry, ListAuditEntriesRequest, ListAuditEntriesResponse};
use tonic::{Request, Response, Status};
use uuid::Uuid;

use super::convert;
use crate::adapters::auth::AuthContext;
use crate::adapters::http::AppState;
use crate::application::error::TenancyError;
use crate::application::pagination::DEFAULT_LIMIT;

/// The `AuditService` gRPC server — a thin adapter over `AppState.audit_query` (module docs).
pub struct AuditGrpc {
    state: AppState,
}

impl AuditGrpc {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

/// Extracts the bearer-resolved [`AuthContext`] from a gRPC request's extensions — mirrors
/// `grpc::authz::actor_context`/`grpc::service_accounts::actor_context`/
/// `grpc::tenancy::actor_context` exactly (duplicated rather than shared across a
/// transport-internal module boundary, the same posture as those).
fn actor_context<T>(request: &Request<T>) -> Result<AuthContext, Status> {
    request.extensions().get::<AuthContext>().cloned().ok_or_else(convert::missing_auth_context)
}

/// An empty wire string means "unfiltered" on that field (proto doc on
/// `ListAuditEntriesRequest`).
fn opt_string(raw: String) -> Option<String> {
    if raw.is_empty() { None } else { Some(raw) }
}

/// Parses the wire `outcome` filter: empty means unfiltered; a non-empty value must name a
/// known [`AuditOutcome`]. `InvalidPrn`-as-sentinel (mirrors `RevokeApiKeyRequest.id`'s/
/// `IssueApiKeyRequest.expires_at`'s identical posture in `grpc::service_accounts`) — there is
/// no dedicated error code for "not a valid outcome" either.
fn parse_outcome(raw: &str) -> Result<Option<AuditOutcome>, TenancyError> {
    if raw.is_empty() {
        return Ok(None);
    }
    AuditOutcome::parse(raw).map(Some).ok_or_else(|| TenancyError::InvalidPrn(format!("unknown audit outcome: {raw}")))
}

/// Parses the wire `cursor`: empty means "first page" (`None`); a non-empty value must be a
/// valid uuid. `InvalidPrn`-as-sentinel, mirrors `RevokeRoleRequest.id`/`RevokeApiKeyRequest.id`
/// 's identical "not a uuid" posture (`grpc::authz`/`grpc::service_accounts`).
fn parse_cursor(raw: &str) -> Result<Option<Uuid>, TenancyError> {
    if raw.is_empty() {
        return Ok(None);
    }
    Uuid::parse_str(raw).map(Some).map_err(|_| TenancyError::InvalidPrn("cursor must be a uuid".to_string()))
}

/// Maps the wire request into the kernel [`AuditFilter`] `AuditQueryService::list` consumes.
/// `limit == 0` is the wire's "server default" sentinel (proto doc on
/// `ListAuditEntriesRequest.limit`) — mapped to [`DEFAULT_LIMIT`] (50) HERE, mirroring
/// `convert::to_page`'s identical `limit == 0 => None => Page`'s-own-default` translation for
/// every other list RPC in this crate. Passing a bare `0` straight through would silently hit
/// `AuditFilter::capped_limit`'s OWN floor instead (`clamp(1, MAX_LIMIT)` treats a raw `0` as
/// "at least 1", not "unset") — a real default request would then get back a single row,
/// contradicting the wire's documented contract.
///
/// Timestamps go through [`convert::parse_opt_ts`], NOT `and_then(convert::from_ts)`: the
/// latter maps an unrepresentable value to `None`, which on a filter field means UNFILTERED
/// (SMA-583).
fn to_filter(req: ListAuditEntriesRequest) -> Result<AuditFilter, TenancyError> {
    Ok(AuditFilter {
        actor_prn: opt_string(req.actor_prn),
        resource_prn: opt_string(req.resource_prn),
        action: opt_string(req.action),
        outcome: parse_outcome(&req.outcome)?,
        from: convert::parse_opt_ts(req.from, "from")?,
        to: convert::parse_opt_ts(req.to, "to")?,
        cursor: parse_cursor(&req.cursor)?,
        limit: if req.limit == 0 { DEFAULT_LIMIT } else { u64::from(req.limit) },
    })
}

/// Projects a domain [`AuditEntry`] into its wire message: `detail_json` is `detail`'s plain
/// `to_string()` (the proto's own doc comment — the persistence layer stores `detail` as
/// serialized TEXT, mirrors `pg_audit_log.rs`'s doc), `id` a canonical uuid string,
/// `correlation_id`/`actor_prn`/`resource_prn` empty-string when `None` (the wire's own
/// "unfiltered"/"absent" sentinel, symmetric with [`to_filter`]'s inverse mapping).
fn to_proto_entry(e: &AuditEntry) -> ProtoAuditEntry {
    ProtoAuditEntry {
        id: e.id.to_string(),
        occurred_at: Some(convert::ts(e.occurred_at)),
        actor_prn: e.actor_prn.clone().unwrap_or_default(),
        action: e.action.clone(),
        resource_prn: e.resource_prn.clone().unwrap_or_default(),
        outcome: e.outcome.as_str().to_string(),
        determining_policies: e.determining_policies.clone(),
        detail_json: e.detail.to_string(),
        correlation_id: e.correlation_id.map(|id| id.to_string()).unwrap_or_default(),
    }
}

#[tonic::async_trait]
impl AuditService for AuditGrpc {
    /// `ListAuditEntries`: Root-only (enforced inside `AuditQueryService::list`, see its module
    /// doc — a non-Root-authorized actor gets `PermissionDenied` via `convert::status_to_grpc`,
    /// nothing about the audit log's contents ever reaching the wire). `next_cursor` is the
    /// last returned entry's id when the page came back FULL (a full page implies more rows may
    /// follow), else empty — the standard keyset-pagination "under-full page proves there is no
    /// next page" convention (mirrors `PgAuditLog::query`'s own keyset-paging doc).
    async fn list_audit_entries(&self, request: Request<ListAuditEntriesRequest>) -> Result<Response<ListAuditEntriesResponse>, Status> {
        let started = Instant::now();
        let result: Result<Response<ListAuditEntriesResponse>, Status> = async {
            let actor = actor_context(&request)?.principal_id.prn().clone();
            let req = request.into_inner();
            let filter = to_filter(req).map_err(convert::status_to_grpc)?;
            let limit = filter.capped_limit();
            let entries = self.state.audit_query.list(&actor, filter).await.map_err(convert::status_to_grpc)?;
            let next_cursor = if entries.len() as u64 == limit {
                entries.last().map_or_else(String::new, |e| e.id.to_string())
            } else {
                String::new()
            };
            Ok(Response::new(ListAuditEntriesResponse {
                entries: entries.iter().map(to_proto_entry).collect(),
                next_cursor,
            }))
        }
        .await;
        record_grpc("Audit", "ListAuditEntries", started, &result);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_filter_treats_empty_wire_fields_as_unfiltered() {
        let filter = to_filter(ListAuditEntriesRequest {
            actor_prn: String::new(),
            resource_prn: String::new(),
            action: String::new(),
            outcome: String::new(),
            from: None,
            to: None,
            cursor: String::new(),
            limit: 0,
        })
        .unwrap();
        assert_eq!(filter.actor_prn, None);
        assert_eq!(filter.resource_prn, None);
        assert_eq!(filter.action, None);
        assert_eq!(filter.outcome, None);
        assert_eq!(filter.cursor, None);
        // The `from`/`to` half of "absent means unfiltered" — untested before SMA-583, which is
        // how `and_then(from_ts)` mapped an INVALID bound to the same `None` unnoticed.
        assert_eq!(filter.from, None);
        assert_eq!(filter.to, None);
    }

    #[test]
    fn to_filter_maps_a_zero_wire_limit_to_the_server_default_not_the_kernel_floor() {
        let filter = to_filter(default_request()).unwrap();
        assert_eq!(filter.limit, DEFAULT_LIMIT);
        // Sanity: `capped_limit`'s own floor for a literal `0` is 1, not `DEFAULT_LIMIT` — this
        // test is only meaningful because `to_filter` intercepts the sentinel first.
        assert_ne!(DEFAULT_LIMIT, 1);
    }

    #[test]
    fn to_filter_parses_a_present_outcome_and_cursor() {
        let id = Uuid::from_u128(7);
        let filter = to_filter(ListAuditEntriesRequest {
            outcome: "denied".to_string(),
            cursor: id.to_string(),
            ..default_request()
        })
        .unwrap();
        assert_eq!(filter.outcome, Some(AuditOutcome::Denied));
        assert_eq!(filter.cursor, Some(id));
    }

    #[test]
    fn to_filter_rejects_an_unknown_outcome() {
        let err = to_filter(ListAuditEntriesRequest {
            outcome: "not-a-real-outcome".to_string(),
            ..default_request()
        })
        .unwrap_err();
        assert!(matches!(err, TenancyError::InvalidPrn(_)));
    }

    #[test]
    fn to_filter_rejects_a_malformed_cursor() {
        let err = to_filter(ListAuditEntriesRequest {
            cursor: "not-a-uuid".to_string(),
            ..default_request()
        })
        .unwrap_err();
        assert!(matches!(err, TenancyError::InvalidPrn(_)));
    }

    /// The case that catches a fix which rejects malformed bounds but drops VALID ones: both
    /// bounds must survive as the exact instant the wire asked for. Mirrors
    /// `http::audit::to_filter_parses_valid_rfc3339_from_and_to`, but asserts the instant
    /// rather than just `is_some()` — the gRPC side converts, it does not parse a string.
    #[test]
    fn to_filter_parses_a_present_valid_from_and_to() {
        let from = chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let to = chrono::DateTime::from_timestamp(1_700_003_600, 500).unwrap();
        let filter = to_filter(ListAuditEntriesRequest {
            from: Some(convert::ts(from)),
            to: Some(convert::ts(to)),
            ..default_request()
        })
        .unwrap();
        assert_eq!(filter.from, Some(from));
        assert_eq!(filter.to, Some(to));
    }

    /// SMA-583: a PRESENT but unrepresentable `from` (`nanos: -1` is outside `Timestamp`'s
    /// valid `[0, 999_999_999]`) is a client error, NOT silently unfiltered. `to` is left
    /// absent deliberately — setting both would pass even if only the `from` line were fixed.
    #[test]
    fn to_filter_rejects_a_present_but_unrepresentable_from_instead_of_unfiltering() {
        let err = to_filter(ListAuditEntriesRequest {
            from: Some(prost_types::Timestamp { seconds: 0, nanos: -1 }),
            to: None,
            ..default_request()
        })
        .unwrap_err();
        assert!(matches!(err, TenancyError::InvalidPrn(_)), "{err:?}");
    }

    /// The `to` half, with `from` absent for the same reason the previous test leaves `to`
    /// absent: this is what fails if only one of the two call sites is fixed.
    #[test]
    fn to_filter_rejects_a_present_but_unrepresentable_to_instead_of_unfiltering() {
        let err = to_filter(ListAuditEntriesRequest {
            from: None,
            to: Some(prost_types::Timestamp { seconds: 0, nanos: -1 }),
            ..default_request()
        })
        .unwrap_err();
        assert!(matches!(err, TenancyError::InvalidPrn(_)), "{err:?}");
    }

    #[test]
    fn to_proto_entry_maps_none_fields_to_empty_strings() {
        let entry = AuditEntry {
            id: Uuid::from_u128(1),
            occurred_at: chrono::Utc::now(),
            actor_prn: None,
            action: "ListOrganizations".to_string(),
            resource_prn: None,
            outcome: AuditOutcome::Denied,
            determining_policies: vec![],
            detail: serde_json::json!({}),
            correlation_id: None,
        };
        let wire = to_proto_entry(&entry);
        assert_eq!(wire.actor_prn, "");
        assert_eq!(wire.resource_prn, "");
        assert_eq!(wire.correlation_id, "");
        assert_eq!(wire.detail_json, "{}");
    }

    fn default_request() -> ListAuditEntriesRequest {
        ListAuditEntriesRequest {
            actor_prn: String::new(),
            resource_prn: String::new(),
            action: String::new(),
            outcome: String::new(),
            from: None,
            to: None,
            cursor: String::new(),
            limit: 0,
        }
    }
}
