// SPDX-License-Identifier: Apache-2.0

//! `GET /v1/audit` handler (SMA-446 Task A11): a thin adapter over `AppState.audit_query` —
//! parse query params -> `AuditQueryService::list` -> DTO projection, no business logic here
//! (mirrors `grpc::audit`'s posture, and `http::authz`/`http::service_accounts`'s thin
//! extract -> service call -> map shape). `Action::ListAuditLog` is Root-only, enforced
//! INSIDE `AuditQueryService` itself (see its module doc) — a non-Root-authorized caller gets
//! `403 Forbidden` via `ApiError`, nothing about the audit log's contents ever reaching the
//! response.
//!
//! Sits on the bearer-gated `protected` sub-router (merged in `mod.rs`) — the caller's PRN
//! comes from the auth middleware's `AuthContext` extension, never a client-supplied value
//! (mirrors every other handler in this module).

use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Extension, Json, Router};
use chrono::{DateTime, Utc};
use paigasus_iam_core::{AuditFilter, AuditOutcome};
use paigasus_kernel::Prn;
use uuid::Uuid;

use super::AppState;
use super::dto::{AuditListResponseDto, AuditQuery};
use super::error::ApiError;
use crate::adapters::auth::AuthContext;
use crate::application::error::TenancyError;
use crate::application::pagination::DEFAULT_LIMIT;

pub fn router() -> Router<AppState> {
    Router::new().route("/v1/audit", get(list))
}

/// The acting principal's canonical `Prn`, from the bearer-resolved `AuthContext` — mirrors
/// `adapters::http::authz::actor_prn`/`service_accounts::actor_prn`.
fn actor_prn(ctx: &AuthContext) -> Prn {
    ctx.principal_id.prn().clone()
}

/// An absent or empty query param means "unfiltered" — mirrors `grpc::audit::opt_string`'s
/// identical empty-wire-value sentinel, generalized to HTTP's native `Option` absence too.
fn opt_non_empty(raw: Option<String>) -> Option<String> {
    raw.filter(|s| !s.is_empty())
}

/// Parses the `outcome` query param: absent/empty means unfiltered; a present value must name
/// a known [`AuditOutcome`]. `InvalidPrn`-as-sentinel — mirrors `grpc::audit::parse_outcome`'s
/// identical posture (there is no dedicated error code for "not a valid outcome" either).
fn parse_outcome(raw: Option<String>) -> Result<Option<AuditOutcome>, TenancyError> {
    match opt_non_empty(raw) {
        None => Ok(None),
        Some(s) => AuditOutcome::parse(&s).map(Some).ok_or_else(|| TenancyError::InvalidPrn(format!("unknown audit outcome: {s}"))),
    }
}

/// Parses the `cursor` query param: absent/empty means "first page" (`None`); a present value
/// must be a valid uuid. Mirrors `grpc::audit::parse_cursor`'s identical "not a uuid" posture.
fn parse_cursor(raw: Option<String>) -> Result<Option<Uuid>, TenancyError> {
    match opt_non_empty(raw) {
        None => Ok(None),
        Some(s) => Uuid::parse_str(&s).map(Some).map_err(|_| TenancyError::InvalidPrn("cursor must be a uuid".to_string())),
    }
}

/// Parses an RFC3339 `from`/`to` query param: absent/empty means unfiltered; a present value
/// must parse as RFC3339. `InvalidPrn`-as-sentinel, mirroring `parse_outcome`/`parse_cursor`
/// above — there is no dedicated error code for "not a valid timestamp" either, and the gRPC
/// wire has no equivalent failure mode to mirror here (its `from`/`to` are already-structured
/// `prost_types::Timestamp`, never a string that can fail to parse).
fn parse_ts(raw: Option<String>) -> Result<Option<DateTime<Utc>>, TenancyError> {
    match opt_non_empty(raw) {
        None => Ok(None),
        Some(s) => DateTime::parse_from_rfc3339(&s)
            .map(|dt| Some(dt.with_timezone(&Utc)))
            .map_err(|_| TenancyError::InvalidPrn(format!("invalid RFC3339 timestamp: {s}"))),
    }
}

/// Maps the query params into the kernel [`AuditFilter`] `AuditQueryService::list` consumes.
/// `limit` absent or `0` maps to [`DEFAULT_LIMIT`] (50) HERE — the SAME "unset sentinel ->
/// server default" mapping `grpc::audit::to_filter` applies to its own wire's `limit == 0`
/// sentinel (see that function's doc for why: passing a bare `0`/`None` straight through
/// would silently hit `AuditFilter::capped_limit`'s OWN floor instead, `clamp(1, MAX_LIMIT)`,
/// which treats "at least 1" as "unset" — a real default request would get back a single row).
///
/// Deliberately duplicated rather than shared with `grpc::audit::to_filter`: the two wire
/// shapes differ enough (proto's empty-string/`u32`-zero sentinels vs. HTTP's `Option`-native
/// query params, plus this transport's own RFC3339-string parsing with no gRPC equivalent)
/// that a shared helper would need its own translation layer on both sides anyway — mirrors
/// `grpc::audit::actor_context`'s explicit "duplicated rather than shared across a
/// transport-internal module boundary" posture for the identical reason.
fn to_filter(q: AuditQuery) -> Result<AuditFilter, TenancyError> {
    Ok(AuditFilter {
        actor_prn: opt_non_empty(q.actor),
        resource_prn: opt_non_empty(q.resource),
        action: opt_non_empty(q.action),
        outcome: parse_outcome(q.outcome)?,
        from: parse_ts(q.from)?,
        to: parse_ts(q.to)?,
        cursor: parse_cursor(q.cursor)?,
        limit: match q.limit {
            None | Some(0) => DEFAULT_LIMIT,
            Some(l) => l,
        },
    })
}

/// `GET /v1/audit`: Root-only (enforced inside `AuditQueryService::list`, see its module doc —
/// a non-Root-authorized caller gets `403 Forbidden` via `ApiError`, nothing about the audit
/// log's contents ever reaching the response). `next_cursor` is the last returned entry's id
/// when the page came back FULL (a full page implies more rows may follow), else `None` — the
/// standard keyset-pagination "under-full page proves there is no next page" convention
/// (mirrors `grpc::audit::list_audit_entries`'s identical doc).
async fn list(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, Query(q): Query<AuditQuery>) -> Result<Json<AuditListResponseDto>, ApiError> {
    let actor = actor_prn(&ctx);
    let filter = to_filter(q)?;
    let limit = filter.capped_limit();
    let entries = s.audit_query.list(&actor, filter).await?;
    let next_cursor = if entries.len() as u64 == limit { entries.last().map(|e| e.id.to_string()) } else { None };
    Ok(Json(AuditListResponseDto {
        entries: entries.into_iter().map(Into::into).collect(),
        next_cursor,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_query() -> AuditQuery {
        AuditQuery {
            actor: None,
            resource: None,
            action: None,
            outcome: None,
            from: None,
            to: None,
            cursor: None,
            limit: None,
        }
    }

    #[test]
    fn to_filter_treats_absent_and_empty_fields_as_unfiltered() {
        let filter = to_filter(AuditQuery {
            actor: Some(String::new()),
            resource: Some(String::new()),
            action: Some(String::new()),
            ..default_query()
        })
        .unwrap();
        assert_eq!(filter.actor_prn, None);
        assert_eq!(filter.resource_prn, None);
        assert_eq!(filter.action, None);
        assert_eq!(filter.outcome, None);
        assert_eq!(filter.from, None);
        assert_eq!(filter.to, None);
        assert_eq!(filter.cursor, None);
    }

    #[test]
    fn to_filter_maps_an_absent_or_zero_limit_to_the_server_default_not_the_kernel_floor() {
        assert_eq!(to_filter(default_query()).unwrap().limit, DEFAULT_LIMIT);
        assert_eq!(to_filter(AuditQuery { limit: Some(0), ..default_query() }).unwrap().limit, DEFAULT_LIMIT);
        // Sanity: `capped_limit`'s own floor for a literal `0` is 1, not `DEFAULT_LIMIT` —
        // this test is only meaningful because `to_filter` intercepts the sentinel first.
        assert_ne!(DEFAULT_LIMIT, 1);
    }

    #[test]
    fn to_filter_passes_through_an_explicit_nonzero_limit() {
        assert_eq!(to_filter(AuditQuery { limit: Some(5), ..default_query() }).unwrap().limit, 5);
    }

    #[test]
    fn to_filter_parses_a_present_outcome_and_cursor() {
        let id = Uuid::from_u128(7);
        let filter = to_filter(AuditQuery {
            outcome: Some("denied".to_string()),
            cursor: Some(id.to_string()),
            ..default_query()
        })
        .unwrap();
        assert_eq!(filter.outcome, Some(AuditOutcome::Denied));
        assert_eq!(filter.cursor, Some(id));
    }

    #[test]
    fn to_filter_rejects_an_unknown_outcome() {
        let err = to_filter(AuditQuery {
            outcome: Some("not-a-real-outcome".to_string()),
            ..default_query()
        })
        .unwrap_err();
        assert!(matches!(err, TenancyError::InvalidPrn(_)));
    }

    #[test]
    fn to_filter_rejects_a_malformed_cursor() {
        let err = to_filter(AuditQuery {
            cursor: Some("not-a-uuid".to_string()),
            ..default_query()
        })
        .unwrap_err();
        assert!(matches!(err, TenancyError::InvalidPrn(_)));
    }

    #[test]
    fn to_filter_rejects_a_malformed_timestamp() {
        let err = to_filter(AuditQuery {
            from: Some("not-a-timestamp".to_string()),
            ..default_query()
        })
        .unwrap_err();
        assert!(matches!(err, TenancyError::InvalidPrn(_)));
    }

    #[test]
    fn to_filter_parses_valid_rfc3339_from_and_to() {
        let filter = to_filter(AuditQuery {
            from: Some("2024-01-01T00:00:00Z".to_string()),
            to: Some("2024-12-31T23:59:59Z".to_string()),
            ..default_query()
        })
        .unwrap();
        assert!(filter.from.is_some());
        assert!(filter.to.is_some());
    }
}
