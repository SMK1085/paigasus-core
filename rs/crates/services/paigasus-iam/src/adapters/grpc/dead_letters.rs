// SPDX-License-Identifier: Apache-2.0

//! `OutboxGrpc`: the `OutboxService` gRPC server (SMA-501) — a thin adapter over
//! `AppState.dead_letters`: parse -> `DeadLetterService` -> project, no business logic here
//! (mirrors `grpc::audit`, and `http::dead_letters` on the other transport).
//!
//! All four RPCs are Root-only, enforced INSIDE `DeadLetterService` itself, so a non-Root
//! caller gets `PermissionDenied` with nothing about the dead-letter contents reaching the
//! response. Every RPC is bearer-enforced by `AuthLayer` — `OutboxService` is not on
//! `grpc::authn::is_exempt`'s allowlist — so the caller's PRN comes from the resolved
//! `AuthContext`, never a client-supplied value.
//!
//! **Registered unconditionally**, unlike the neighbouring `AuditService`, which is dropped
//! entirely when `iam.audit` is off. The asymmetry is deliberate: `iam.audit` gates a READ-ONLY
//! surface, while this one permanently discards events and bulk-replays up to 10 000 — a
//! break-glass surface must not be disable-able, because the moment you need it is the moment a
//! config flag is hardest to change. HTTP mounts `dead_letters::router()` ungated too, so
//! gating gRPC alone would itself be a divergence.
//!
//! **A caveat for time filters, not a bug** (mirrors `PgDeadLetters` and `http::dead_letters`):
//! a row whose `parked_at` is unset can never satisfy `parked_from`/`parked_to` — Postgres
//! never evaluates a NULL comparison as true — so it is invisible to `ListDeadLetters` whenever
//! either bound is set. It stays reachable via an unfiltered list.

use std::time::Instant;

#[cfg(test)]
use chrono::{DateTime, Utc};
use paigasus_iam_core::{BulkReplayRequest, DeadLetterFilter};
use paigasus_kernel::Prn;
use paigasus_observability::record_grpc;
use paigasus_proto::paigasus::iam::v1::outbox_service_server::OutboxService;
use paigasus_proto::paigasus::iam::v1::{
    BulkReplayDeadLettersRequest, BulkReplayDeadLettersResponse, DiscardDeadLetterRequest, DiscardDeadLetterResponse, ListDeadLettersRequest, ListDeadLettersResponse, ReplayDeadLetterRequest,
    ReplayDeadLetterResponse,
};
use tonic::{Request, Response, Status};
use uuid::Uuid;

use super::convert;
use crate::adapters::auth::AuthContext;
use crate::adapters::http::AppState;
use crate::application::error::TenancyError;
use crate::application::pagination::DEFAULT_LIMIT;

/// The `OutboxService` gRPC server — a thin adapter over `AppState.dead_letters`.
pub struct OutboxGrpc {
    state: AppState,
}

impl OutboxGrpc {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

/// Extracts the bearer-resolved [`AuthContext`] — mirrors `grpc::audit::actor_context`
/// exactly (duplicated rather than shared across a transport-internal boundary, the same
/// posture as every sibling).
fn actor_context<T>(request: &Request<T>) -> Result<AuthContext, Status> {
    request.extensions().get::<AuthContext>().cloned().ok_or_else(convert::missing_auth_context)
}

fn actor_prn(ctx: &AuthContext) -> Prn {
    ctx.principal_id.prn().clone()
}

/// An empty wire string means "unfiltered" on that field (the proto's own doc).
fn opt_string(raw: String) -> Option<String> {
    if raw.is_empty() { None } else { Some(raw) }
}

/// Empty means unfiltered; a non-empty value must parse as a uuid. `InvalidPrn`-as-sentinel,
/// mirroring `grpc::audit::parse_cursor`.
fn parse_cursor(raw: &str) -> Result<Option<Uuid>, TenancyError> {
    if raw.is_empty() {
        return Ok(None);
    }
    Uuid::parse_str(raw).map(Some).map_err(|_| TenancyError::InvalidPrn("cursor must be a uuid".to_string()))
}

/// `limit` `0` maps to [`DEFAULT_LIMIT`] HERE — passing a bare `0` through would hit
/// `DeadLetterFilter::capped_limit`'s own floor of 1, so a default request would return a
/// single row (the trap `http::dead_letters::to_filter` documents).
///
/// Timestamps go through [`convert::parse_opt_ts`], NOT `and_then(convert::from_ts)`: the
/// latter maps an unrepresentable value to `None`, which on a filter field means UNFILTERED.
fn to_filter(req: ListDeadLettersRequest) -> Result<DeadLetterFilter, TenancyError> {
    Ok(DeadLetterFilter {
        event_type: opt_string(req.event_type),
        parked_from: convert::parse_opt_ts(req.parked_from, "parked_from")?,
        parked_to: convert::parse_opt_ts(req.parked_to, "parked_to")?,
        cursor: parse_cursor(&req.cursor)?,
        limit: if req.limit == 0 { DEFAULT_LIMIT } else { u64::from(req.limit) },
    })
}

/// A `max_rows` of 0 — which an absent field collapses to — produces an INVALID request that
/// `DeadLetterService::replay_matching` rejects before any store access (design D5). It is
/// deliberately NOT defaulted to anything usable: the explicit row budget is the guard.
///
/// Same strict timestamp handling as [`to_filter`], and it matters more here: a silently
/// dropped bound turns a narrowly-scoped bulk replay into "replay everything up to `max_rows`".
fn to_bulk_request(req: BulkReplayDeadLettersRequest) -> Result<BulkReplayRequest, TenancyError> {
    Ok(BulkReplayRequest {
        event_type: opt_string(req.event_type),
        parked_from: convert::parse_opt_ts(req.parked_from, "parked_from")?,
        parked_to: convert::parse_opt_ts(req.parked_to, "parked_to")?,
        max_rows: req.max_rows,
    })
}

fn parse_id(raw: &str) -> Result<Uuid, TenancyError> {
    Uuid::parse_str(raw).map_err(|_| TenancyError::InvalidPrn("id must be a uuid".to_string()))
}

#[tonic::async_trait]
impl OutboxService for OutboxGrpc {
    /// Root-only. `next_cursor` is the last returned entry's id when the page came back FULL,
    /// else empty — the same keyset convention `grpc::audit::list_audit_entries` uses.
    async fn list_dead_letters(&self, request: Request<ListDeadLettersRequest>) -> Result<Response<ListDeadLettersResponse>, Status> {
        let started = Instant::now();
        let result: Result<Response<ListDeadLettersResponse>, Status> = async {
            let actor = actor_prn(&actor_context(&request)?);
            let filter = to_filter(request.into_inner()).map_err(convert::status_to_grpc)?;
            let limit = filter.capped_limit();
            let entries = self.state.dead_letters.list(&actor, filter).await.map_err(convert::status_to_grpc)?;
            let next_cursor = if entries.len() as u64 == limit {
                entries.last().map_or_else(String::new, |e| e.id.to_string())
            } else {
                String::new()
            };
            Ok(Response::new(ListDeadLettersResponse {
                entries: entries.iter().map(convert::to_proto_dead_letter_entry).collect(),
                next_cursor,
            }))
        }
        .await;
        record_grpc("Outbox", "ListDeadLetters", started, &result);
        result
    }

    /// Root-only. `NotFound` covers an absent id, a live row, and a row another actor already
    /// replayed or discarded.
    async fn replay_dead_letter(&self, request: Request<ReplayDeadLetterRequest>) -> Result<Response<ReplayDeadLetterResponse>, Status> {
        let started = Instant::now();
        let result: Result<Response<ReplayDeadLetterResponse>, Status> = async {
            let actor = actor_prn(&actor_context(&request)?);
            let id = parse_id(&request.into_inner().id).map_err(convert::status_to_grpc)?;
            let entry = self.state.dead_letters.replay(&actor, id).await.map_err(convert::status_to_grpc)?;
            Ok(Response::new(ReplayDeadLetterResponse {
                entry: Some(convert::to_proto_dead_letter_entry(&entry)),
            }))
        }
        .await;
        record_grpc("Outbox", "ReplayDeadLetter", started, &result);
        result
    }

    /// Root-only. A discarded row is gone forever — its audit entry is its only remaining trace.
    async fn discard_dead_letter(&self, request: Request<DiscardDeadLetterRequest>) -> Result<Response<DiscardDeadLetterResponse>, Status> {
        let started = Instant::now();
        let result: Result<Response<DiscardDeadLetterResponse>, Status> = async {
            let actor = actor_prn(&actor_context(&request)?);
            let id = parse_id(&request.into_inner().id).map_err(convert::status_to_grpc)?;
            let entry = self.state.dead_letters.discard(&actor, id).await.map_err(convert::status_to_grpc)?;
            Ok(Response::new(DiscardDeadLetterResponse {
                entry: Some(convert::to_proto_dead_letter_entry(&entry)),
            }))
        }
        .await;
        record_grpc("Outbox", "DiscardDeadLetter", started, &result);
        result
    }

    /// Root-only. A missing or zero `max_rows` is rejected before any store access — the
    /// explicit row budget is the guard on blast radius, never defaulted to anything usable.
    async fn bulk_replay_dead_letters(&self, request: Request<BulkReplayDeadLettersRequest>) -> Result<Response<BulkReplayDeadLettersResponse>, Status> {
        let started = Instant::now();
        let result: Result<Response<BulkReplayDeadLettersResponse>, Status> = async {
            let actor = actor_prn(&actor_context(&request)?);
            let req = to_bulk_request(request.into_inner()).map_err(convert::status_to_grpc)?;
            let replayed = self.state.dead_letters.replay_matching(&actor, req).await.map_err(convert::status_to_grpc)?;
            Ok(Response::new(BulkReplayDeadLettersResponse { replayed }))
        }
        .await;
        record_grpc("Outbox", "BulkReplayDeadLetters", started, &result);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req() -> ListDeadLettersRequest {
        ListDeadLettersRequest {
            event_type: String::new(),
            parked_from: None,
            parked_to: None,
            cursor: String::new(),
            limit: 0,
        }
    }

    #[test]
    fn to_filter_treats_empty_wire_fields_as_unfiltered() {
        let f = to_filter(req()).unwrap();
        assert_eq!(f.event_type, None);
        assert_eq!(f.parked_from, None);
        assert_eq!(f.parked_to, None);
        assert_eq!(f.cursor, None);
    }

    /// Mirrors `http::dead_letters`'s identical test. `limit` is mapped HERE, not left to
    /// `capped_limit` — whose floor for a literal 0 is 1, so a default request would otherwise
    /// return a single row.
    #[test]
    fn to_filter_maps_an_absent_limit_to_the_server_default() {
        assert_eq!(to_filter(req()).unwrap().limit, DEFAULT_LIMIT);
        assert_ne!(DEFAULT_LIMIT, 1);
    }

    /// A hardcoded `limit: DEFAULT_LIMIT` inside `to_filter` would pass every other test here.
    #[test]
    fn to_filter_passes_through_an_explicit_nonzero_limit() {
        assert_eq!(to_filter(ListDeadLettersRequest { limit: 5, ..req() }).unwrap().limit, 5);
    }

    #[test]
    fn to_filter_forwards_present_filters_with_their_exact_values() {
        let from = DateTime::parse_from_rfc3339("2026-08-01T00:00:00Z").unwrap().with_timezone(&Utc);
        let to = DateTime::parse_from_rfc3339("2026-08-02T00:00:00Z").unwrap().with_timezone(&Utc);
        let f = to_filter(ListDeadLettersRequest {
            event_type: "iam.principal.created".to_string(),
            parked_from: Some(convert::ts(from)),
            parked_to: Some(convert::ts(to)),
            cursor: Uuid::from_u128(7).to_string(),
            limit: 5,
        })
        .unwrap();
        assert_eq!(f.event_type, Some("iam.principal.created".to_string()));
        assert_eq!(f.parked_from, Some(from));
        assert_eq!(f.parked_to, Some(to));
        assert_eq!(f.cursor, Some(Uuid::from_u128(7)));
    }

    #[test]
    fn to_filter_rejects_a_malformed_cursor() {
        assert!(matches!(to_filter(ListDeadLettersRequest { cursor: "nope".to_string(), ..req() }), Err(TenancyError::InvalidPrn(_))));
    }

    /// The security-relevant case (design D10). A present-but-unrepresentable bound must be a
    /// client error — mapping it to `None` would mean UNFILTERED, silently widening the query.
    #[test]
    fn to_filter_rejects_a_present_but_invalid_timestamp_rather_than_unfiltering() {
        for t in [prost_types::Timestamp { seconds: 0, nanos: -1 }, prost_types::Timestamp { seconds: i64::MAX, nanos: 0 }] {
            assert!(matches!(to_filter(ListDeadLettersRequest { parked_from: Some(t), ..req() }), Err(TenancyError::InvalidPrn(_))));
            assert!(matches!(to_filter(ListDeadLettersRequest { parked_to: Some(t), ..req() }), Err(TenancyError::InvalidPrn(_))));
        }
    }

    fn bulk() -> BulkReplayDeadLettersRequest {
        BulkReplayDeadLettersRequest {
            event_type: String::new(),
            parked_from: None,
            parked_to: None,
            max_rows: 0,
        }
    }

    /// Design D5: proto3 cannot tell an absent `max_rows` from an explicit 0, and does not need
    /// to — both are rejected identically, before any store access. The explicit row budget is
    /// the guard on blast radius and must never default to anything usable.
    #[test]
    fn a_zero_max_rows_produces_an_invalid_bulk_replay_request() {
        assert!(!to_bulk_request(bulk()).unwrap().is_valid());
    }

    /// The one security-relevant mutation on this surface: silently dropping the filters turns
    /// a narrowly-scoped bulk replay into "replay everything up to max_rows". Asserts EXACT
    /// values — `is_some()` would pass even with `event_type` dropped or the instants swapped.
    #[test]
    fn to_bulk_request_forwards_every_filter_and_max_rows() {
        let from = DateTime::parse_from_rfc3339("2026-08-01T00:00:00Z").unwrap().with_timezone(&Utc);
        let to = DateTime::parse_from_rfc3339("2026-08-02T00:00:00Z").unwrap().with_timezone(&Utc);
        let r = to_bulk_request(BulkReplayDeadLettersRequest {
            event_type: "iam.principal.created".to_string(),
            parked_from: Some(convert::ts(from)),
            parked_to: Some(convert::ts(to)),
            max_rows: 500,
        })
        .unwrap();
        assert_eq!(r.event_type, Some("iam.principal.created".to_string()));
        assert_eq!(r.parked_from, Some(from));
        assert_eq!(r.parked_to, Some(to));
        assert_eq!(r.max_rows, 500);
    }

    /// The same D10 guard on the BULK path, where dropping a bound is worst.
    #[test]
    fn to_bulk_request_rejects_a_present_but_invalid_timestamp() {
        let bad = prost_types::Timestamp { seconds: 0, nanos: -1 };
        assert!(matches!(
            to_bulk_request(BulkReplayDeadLettersRequest {
                parked_from: Some(bad),
                max_rows: 500,
                ..bulk()
            }),
            Err(TenancyError::InvalidPrn(_))
        ));
        assert!(matches!(
            to_bulk_request(BulkReplayDeadLettersRequest {
                parked_to: Some(bad),
                max_rows: 500,
                ..bulk()
            }),
            Err(TenancyError::InvalidPrn(_))
        ));
    }
}
