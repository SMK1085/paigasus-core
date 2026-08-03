// SPDX-License-Identifier: Apache-2.0

//! `/v1/outbox/dead-letters` handlers (SMA-469): a thin adapter over `AppState.dead_letters` —
//! parse -> `DeadLetterService` -> DTO, no business logic here (mirrors `http::audit`).
//!
//! All three Cedar actions are Root-only, enforced INSIDE `DeadLetterService` itself, so a
//! non-Root caller gets `403` with nothing about the dead-letter contents reaching the
//! response. Sits on the bearer-gated `protected` sub-router; the caller's PRN comes from the
//! auth middleware's `AuthContext`, never a client-supplied value.
//!
//! **A caveat for time filters, not a bug** (mirrors `PgDeadLetters`'s own module doc): a row
//! with `parked_at IS NULL` can never satisfy `parked_from`/`parked_to` — Postgres never
//! evaluates a `NULL` comparison as true — so such a row is invisible to `GET
//! /v1/outbox/dead-letters` whenever either bound is set, and to `POST .../replay` whenever a
//! time bound narrows the match. It remains reachable via an unfiltered `list` and an
//! unfiltered (or only `event_type`-filtered) bulk replay, so nothing is permanently lost — but
//! an operator triaging why a known-parked row is missing from a windowed query should know
//! this before assuming a bug.
//!
//! This is an operator-only break-glass surface and is deliberately HTTP-only: unlike the
//! audit read API it has no gRPC mirror, which keeps `contracts/` untouched. That is a scope
//! decision, not an API-boundary principle.

use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use chrono::{DateTime, Utc};
use paigasus_iam_core::{BulkReplayRequest, DeadLetterFilter};
use paigasus_kernel::Prn;
use uuid::Uuid;

use super::AppState;
use super::dto::{BulkReplayBody, BulkReplayResponseDto, DeadLetterEntryDto, DeadLetterListResponseDto, DeadLetterQuery};
use super::error::ApiError;
use crate::adapters::auth::AuthContext;
use crate::application::error::TenancyError;
use crate::application::pagination::DEFAULT_LIMIT;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/outbox/dead-letters", get(list))
        // The literal `/replay` and `/{id}/replay` below differ in segment count, so axum's
        // router has no ambiguity between them.
        .route("/v1/outbox/dead-letters/replay", post(replay_matching))
        .route("/v1/outbox/dead-letters/{id}/replay", post(replay_one))
        .route("/v1/outbox/dead-letters/{id}/discard", post(discard_one))
}

/// The acting principal's canonical `Prn`, from the bearer-resolved `AuthContext` — mirrors
/// `http::audit::actor_prn`.
fn actor_prn(ctx: &AuthContext) -> Prn {
    ctx.principal_id.prn().clone()
}

fn opt_non_empty(raw: Option<String>) -> Option<String> {
    raw.filter(|s| !s.is_empty())
}

/// Absent/empty means unfiltered; a present value must parse as RFC3339.
/// `InvalidPrn`-as-sentinel, mirroring `http::audit::parse_ts` exactly.
fn parse_ts(raw: Option<String>) -> Result<Option<DateTime<Utc>>, TenancyError> {
    match opt_non_empty(raw) {
        None => Ok(None),
        Some(s) => DateTime::parse_from_rfc3339(&s)
            .map(|dt| Some(dt.with_timezone(&Utc)))
            .map_err(|_| TenancyError::InvalidPrn(format!("invalid RFC3339 timestamp: {s}"))),
    }
}

fn parse_cursor(raw: Option<String>) -> Result<Option<Uuid>, TenancyError> {
    match opt_non_empty(raw) {
        None => Ok(None),
        Some(s) => Uuid::parse_str(&s).map(Some).map_err(|_| TenancyError::InvalidPrn("cursor must be a uuid".to_string())),
    }
}

/// `limit` absent or `0` maps to [`DEFAULT_LIMIT`] HERE — passing a bare `0` through would hit
/// `DeadLetterFilter::capped_limit`'s own floor of 1 instead, so a default request would return
/// a single row (the same trap `http::audit::to_filter` documents).
fn to_filter(q: DeadLetterQuery) -> Result<DeadLetterFilter, TenancyError> {
    Ok(DeadLetterFilter {
        event_type: opt_non_empty(q.event_type),
        parked_from: parse_ts(q.parked_from)?,
        parked_to: parse_ts(q.parked_to)?,
        cursor: parse_cursor(q.cursor)?,
        limit: match q.limit {
            None | Some(0) => DEFAULT_LIMIT,
            Some(l) => l,
        },
    })
}

impl BulkReplayBody {
    /// An absent `max_rows` becomes `0`, which `BulkReplayRequest::is_valid` rejects — the
    /// service turns that into `TenancyError::InvalidBulkReplay` (a 400). It is deliberately
    /// NOT defaulted to anything usable: the explicit row budget is the guard.
    pub fn into_request(self) -> Result<BulkReplayRequest, TenancyError> {
        Ok(BulkReplayRequest {
            event_type: opt_non_empty(self.event_type),
            parked_from: parse_ts(self.parked_from)?,
            parked_to: parse_ts(self.parked_to)?,
            max_rows: self.max_rows.unwrap_or(0),
        })
    }
}

/// `GET /v1/outbox/dead-letters`: Root-only (enforced inside `DeadLetterService::list`).
/// `next_cursor` is the last returned entry's id when the page came back FULL, else `None` —
/// mirrors `http::audit::list`'s identical keyset-pagination convention.
async fn list(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, Query(q): Query<DeadLetterQuery>) -> Result<Json<DeadLetterListResponseDto>, ApiError> {
    let filter = to_filter(q)?;
    let limit = filter.capped_limit();
    let entries = s.dead_letters.list(&actor_prn(&ctx), filter).await?;
    let next_cursor = if entries.len() as u64 == limit { entries.last().map(|e| e.id.to_string()) } else { None };
    Ok(Json(DeadLetterListResponseDto {
        entries: entries.into_iter().map(Into::into).collect(),
        next_cursor,
    }))
}

/// `POST /v1/outbox/dead-letters/{id}/replay`: Root-only. `404` covers an absent id, a live
/// row, and a row another actor already replayed or discarded.
async fn replay_one(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>) -> Result<Json<DeadLetterEntryDto>, ApiError> {
    Ok(Json(s.dead_letters.replay(&actor_prn(&ctx), id).await?.into()))
}

/// `POST /v1/outbox/dead-letters/{id}/discard`: Root-only. A discarded row is gone forever —
/// its audit entry is its only remaining trace (`DeadLetterService::discard`'s own doc).
async fn discard_one(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>) -> Result<Json<DeadLetterEntryDto>, ApiError> {
    Ok(Json(s.dead_letters.discard(&actor_prn(&ctx), id).await?.into()))
}

/// `POST /v1/outbox/dead-letters/replay`: Root-only. A missing or zero `max_rows` is rejected
/// with `400 invalid-bulk-replay` before any store access (`DeadLetterService::replay_matching`)
/// — the explicit row budget is the guard on blast radius, never defaulted to anything usable.
async fn replay_matching(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, Json(body): Json<BulkReplayBody>) -> Result<Json<BulkReplayResponseDto>, ApiError> {
    let req = body.into_request()?;
    let replayed = s.dead_letters.replay_matching(&actor_prn(&ctx), req).await?;
    Ok(Json(BulkReplayResponseDto { replayed }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q() -> DeadLetterQuery {
        DeadLetterQuery {
            event_type: None,
            parked_from: None,
            parked_to: None,
            cursor: None,
            limit: None,
        }
    }

    #[test]
    fn to_filter_treats_absent_and_empty_fields_as_unfiltered() {
        let f = to_filter(DeadLetterQuery {
            event_type: Some(String::new()),
            ..q()
        })
        .unwrap();
        assert_eq!(f.event_type, None);
        assert_eq!(f.parked_from, None);
        assert_eq!(f.parked_to, None);
        assert_eq!(f.cursor, None);
    }

    #[test]
    fn to_filter_maps_an_absent_or_zero_limit_to_the_server_default() {
        assert_eq!(to_filter(q()).unwrap().limit, DEFAULT_LIMIT);
        assert_eq!(to_filter(DeadLetterQuery { limit: Some(0), ..q() }).unwrap().limit, DEFAULT_LIMIT);
        // Sanity: capped_limit's own floor for a literal 0 is 1, not DEFAULT_LIMIT — this test
        // is only meaningful because to_filter intercepts the sentinel first.
        assert_ne!(DEFAULT_LIMIT, 1);
    }

    #[test]
    fn to_filter_parses_rfc3339_park_times_and_a_uuid_cursor() {
        let f = to_filter(DeadLetterQuery {
            parked_from: Some("2026-08-01T00:00:00Z".to_string()),
            parked_to: Some("2026-08-02T00:00:00Z".to_string()),
            cursor: Some(Uuid::from_u128(7).to_string()),
            ..q()
        })
        .unwrap();
        assert!(f.parked_from.is_some());
        assert!(f.parked_to.is_some());
        assert_eq!(f.cursor, Some(Uuid::from_u128(7)));
    }

    #[test]
    fn to_filter_rejects_a_malformed_timestamp_and_cursor() {
        assert!(matches!(
            to_filter(DeadLetterQuery {
                parked_from: Some("nope".to_string()),
                ..q()
            }),
            Err(TenancyError::InvalidPrn(_))
        ));
        assert!(matches!(
            to_filter(DeadLetterQuery {
                cursor: Some("nope".to_string()),
                ..q()
            }),
            Err(TenancyError::InvalidPrn(_))
        ));
    }

    #[test]
    fn bulk_body_without_max_rows_becomes_an_invalid_bulk_replay_request() {
        let req = BulkReplayBody {
            event_type: None,
            parked_from: None,
            parked_to: None,
            max_rows: None,
        }
        .into_request()
        .unwrap();
        assert!(!req.is_valid(), "an absent max_rows must produce an invalid request, not a default");
    }
}
