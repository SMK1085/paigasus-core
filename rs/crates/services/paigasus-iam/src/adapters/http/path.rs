// SPDX-License-Identifier: Apache-2.0

//! A uuid path-segment extractor that answers inside IAM's error envelope (SMA-586 D5.1).
//!
//! axum's own `Path<Uuid>` rejects a malformed segment with a plain-text 400 that is not the
//! `{"error":{code,message}}` contract every other IAM failure uses — so 26 path segments
//! (across 25 handlers) answered outside their own error contract, and `invalid-uuid` had no
//! HTTP emitter at all while its gRPC twins did (AC-1). This closes both.
//!
//! Mirrors `json::EnvelopeJson` (SMA-587), including the half that matters most: its
//! `envelope_rejection` does NOT flatten every rejection into one client error — it keeps
//! `rejection.status()` and branches on the kind. So does this module. A `PathRejection` axum
//! classes `500` — `MissingPathParams`, and the `WrongNumberOfParameters` / `UnsupportedType`
//! kinds inside `FailedToDeserializePathParams` — means the route's pattern stopped matching
//! its handler's arity, which is a SERVER bug. Answering that with `400 invalid-uuid` would
//! report our mistake as the caller's, the exact category of error this ticket exists to
//! remove, so those keep axum's own status and body.
//!
//! The field name is carried by a MARKER TYPE rather than a request extension: `&'static str`
//! is not a stable const-generic parameter, and an extension set by a route-level layer would
//! let a route that forgets the layer report a wrong name at runtime. A marker puts the name
//! in the handler signature, so a route cannot compile without choosing one.
//!
//! Each segment is extracted as a `String` and parsed by `Uuid::parse_str` HERE, rather than
//! handed to `Path<Uuid>`/`Path<(Uuid, Uuid)>`. That is what lets a two-segment route name the
//! field of the segment that actually failed: the parse happens positionally, so
//! [`UuidPathPair`] reports its first marker for the first segment and its second for the
//! second, instead of guessing one name for both.

use std::marker::PhantomData;

use axum::extract::rejection::PathRejection;
use axum::extract::{FromRequestParts, Path};
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use uuid::Uuid;

use crate::adapters::http::error::ApiError;
use crate::application::error::TenancyError;

/// Names the wire field a uuid path segment stands for, for `TenancyError::InvalidUuid`.
pub(crate) trait PathField {
    const NAME: &'static str;
}

/// Declares a marker type and its wire field name.
macro_rules! path_field {
    ($(#[$m:meta])* $name:ident => $wire:literal) => {
        $(#[$m])*
        pub(crate) struct $name;
        impl PathField for $name {
            const NAME: &'static str = $wire;
        }
    };
}

path_field!(/// `{id}` on an organization route.
    OrganizationId => "organization_id");
path_field!(/// `{id}` or `{team_id}` on a team route.
    TeamId => "team_id");
path_field!(/// `{id}` on a project route.
    ProjectId => "project_id");
path_field!(/// `{sa}` — a service account's bare uuid.
    ServiceAccountId => "service_account_id");
path_field!(/// `{id}` on an api-key route.
    ApiKeyId => "api_key_id");
path_field!(/// `{id}` on a membership route.
    MembershipId => "membership_id");
path_field!(/// `{id}` on a role-grant route.
    RoleGrantId => "role_grant_id");
path_field!(/// `{id}` on a dead-letter route.
    DeadLetterId => "dead_letter_id");
path_field!(/// `{policy_id}` on a policy route, and `{id}` on the system-policy retire route —
    /// both name the same wire field.
    PolicyId => "policy_id");

/// The `{field} must be a uuid` envelope response — the one construction point both extractors
/// below use, so they cannot drift apart on status, code or shape.
fn malformed_uuid(field: &'static str) -> Response {
    ApiError(TenancyError::InvalidUuid(field)).into_response()
}

/// The `{field} is not a valid path segment` envelope response — `malformed_uuid`'s sibling,
/// built the same way and for the same reason: one construction point, so status, code and
/// shape cannot drift between the extractors below.
fn malformed_segment(field: &'static str) -> Response {
    ApiError(TenancyError::InvalidPathSegment(field)).into_response()
}

/// Was this rejection raised because the CALLER's path was bad, or because the ROUTE is wrong?
///
/// axum answers that itself: `FailedToDeserializePathParams::status()` is `BAD_REQUEST` only
/// for the kinds a request can cause, and `INTERNAL_SERVER_ERROR` for `WrongNumberOfParameters`
/// / `UnsupportedType`; `MissingPathParams` is a 500 outright. Anything not `BAD_REQUEST` is
/// handed back to axum unchanged (module docs).
fn is_client_error(rejection: &PathRejection) -> bool {
    match rejection {
        PathRejection::FailedToDeserializePathParams(e) => e.status() == StatusCode::BAD_REQUEST,
        _ => false,
    }
}

/// A single uuid path segment, reported as `F::NAME` when it is malformed.
pub(crate) struct UuidPath<F: PathField> {
    pub id: Uuid,
    _marker: PhantomData<F>,
}

impl<S: Send + Sync, F: PathField> FromRequestParts<S> for UuidPath<F> {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        // `Path<String>` cannot fail to parse, so the only client-side rejection left here is a
        // segment whose percent-decoding is not UTF-8 — still a malformed segment, and on a
        // one-segment route `F::NAME` is the only field it can be, so it stays in the envelope.
        let raw = match Path::<String>::from_request_parts(parts, state).await {
            Ok(Path(raw)) => raw,
            Err(rejection) if is_client_error(&rejection) => return Err(malformed_uuid(F::NAME)),
            Err(rejection) => return Err(rejection.into_response()),
        };
        let id = Uuid::parse_str(&raw).map_err(|_| malformed_uuid(F::NAME))?;
        Ok(UuidPath { id, _marker: PhantomData })
    }
}

/// Two uuid path segments, for `/{sa}/api-keys/{id}` — one marker per segment.
///
/// Each segment is parsed on its own, in order, so a malformed FIRST segment reports `F1::NAME`
/// and a malformed second reports `F2::NAME`. The single-marker form this replaced named the
/// second field for both, which meant a bad `{sa}` answered `api_key_id must be a uuid` — a
/// guess presented as fact, on the branch whose thesis is that a misleading reason is a bug.
///
/// One case stays outside the envelope: a segment whose percent-decoding is not valid UTF-8.
/// axum raises that BEFORE deserialization, from the `UrlParams` extension, so the rejection
/// carries the offending segment's route key (`sa`) and no position — there is nothing here to
/// map it onto a marker with. Its own 400 names that key correctly, which beats inventing one
/// of the two field names inside the envelope.
pub(crate) struct UuidPathPair<F1: PathField, F2: PathField> {
    pub first: Uuid,
    pub second: Uuid,
    _marker: PhantomData<(F1, F2)>,
}

impl<S: Send + Sync, F1: PathField, F2: PathField> FromRequestParts<S> for UuidPathPair<F1, F2> {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let (raw_first, raw_second) = match Path::<(String, String)>::from_request_parts(parts, state).await {
            Ok(Path(pair)) => pair,
            // No `is_client_error` arm here, unlike `UuidPath` above: the one client-side
            // rejection `Path<(String, String)>` can raise is the non-UTF-8 segment, and this
            // extractor cannot tell WHICH of its two markers that segment belongs to (struct
            // docs). Naming one would be the guess this fix removed, so axum answers instead.
            Err(rejection) => return Err(rejection.into_response()),
        };
        let first = Uuid::parse_str(&raw_first).map_err(|_| malformed_uuid(F1::NAME))?;
        let second = Uuid::parse_str(&raw_second).map_err(|_| malformed_uuid(F2::NAME))?;
        Ok(UuidPathPair { first, second, _marker: PhantomData })
    }
}

/// A single NON-UUID path segment, reported as `F::NAME` when it is undecodable (SMA-588).
///
/// `UuidPath`'s sibling for the two routes whose `{id}` is an opaque policy id rather than a
/// uuid — `authz::delete_policy` and `system_retirement::retire`. Both took axum's plain
/// `Path<String>`, whose rejection escapes the error envelope entirely.
///
/// `Path<String>` cannot fail to PARSE, so the one client-side rejection reachable here is a
/// segment whose percent-decoding is not valid UTF-8. On a one-segment route `F::NAME` is the
/// only field it can be, so naming it is a fact rather than the guess `UuidPathPair` refuses to
/// make. Everything axum classes 5xx — `MissingPathParams`, `WrongNumberOfParameters`,
/// `UnsupportedType` — is a ROUTE bug and keeps axum's own response (module docs).
///
/// `FromRequestParts`, not `FromRequest`: `system_retirement::retire` takes this extractor
/// FOLLOWED BY an `Option<EnvelopeJson<RetireBody>>` body, and only one `FromRequest` extractor
/// is permitted per handler and it must come last.
pub(crate) struct StringPath<F: PathField> {
    pub value: String,
    _marker: PhantomData<F>,
}

impl<S: Send + Sync, F: PathField> FromRequestParts<S> for StringPath<F> {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match Path::<String>::from_request_parts(parts, state).await {
            Ok(Path(value)) => Ok(StringPath { value, _marker: PhantomData }),
            Err(rejection) if is_client_error(&rejection) => Err(malformed_segment(F::NAME)),
            Err(rejection) => Err(rejection.into_response()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::to_bytes;
    use axum::routing::get;
    use tower::ServiceExt;

    async fn ok(path: UuidPath<MembershipId>) -> String {
        path.id.to_string()
    }

    async fn ok_pair(path: UuidPathPair<ServiceAccountId, ApiKeyId>) -> String {
        format!("{} {}", path.first, path.second)
    }

    /// `GET uri` against a one-route router, returning `(status, raw body)`.
    async fn probe(app: Router, uri: &str) -> (StatusCode, Vec<u8>) {
        let resp = app.oneshot(axum::http::Request::builder().uri(uri).body(axum::body::Body::empty()).unwrap()).await.unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        (status, bytes.to_vec())
    }

    /// The registry's `invalid-uuid` wire string, resolved through `as_wire_reason()`.
    ///
    /// Never a bare kebab literal — a literal in a `src/` file would put this production module
    /// on `ci/error-registry/check.py`'s MANIFEST, blinding that gate to a future *production*
    /// code literal in this file.
    fn invalid_uuid_wire() -> String {
        use paigasus_proto::paigasus::common::v1::ErrorReason;
        ErrorReason::InvalidUuid.as_wire_reason().expect("not the Unspecified sentinel")
    }

    /// SMA-586 D5.1: a malformed uuid path segment answers inside the error envelope, with
    /// `invalid-uuid`. Before this, axum's own `Path<Uuid>` rejection produced a plain-text
    /// 400 that was not the `{"error":{code,message}}` contract at all — on all 26 segments.
    #[tokio::test]
    async fn a_malformed_uuid_segment_answers_in_the_error_envelope() {
        let (status, bytes) = probe(Router::new().route("/x/{id}", get(ok)), "/x/not-a-uuid").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"]["code"], invalid_uuid_wire());
        assert_eq!(body["error"]["message"], "membership_id must be a uuid");
    }

    /// A well-formed uuid still reaches the handler unchanged.
    #[tokio::test]
    async fn a_well_formed_uuid_segment_extracts() {
        let id = uuid::Uuid::from_u128(0x1234_5678_9abc_def0_1234_5678_9abc_def0);
        let (status, bytes) = probe(Router::new().route("/x/{id}", get(ok)), &format!("/x/{id}")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(String::from_utf8(bytes).unwrap(), id.to_string());
    }

    /// A ROUTER bug keeps its own 5xx instead of being relabelled as the caller's mistake.
    ///
    /// A one-segment handler mounted on a two-segment pattern is exactly the drift the module
    /// docs describe: axum raises `WrongNumberOfParameters`, whose status is 500. The extractor
    /// used to swallow every `PathRejection` variant and answer `400 invalid-uuid`, reporting a
    /// server bug as a client error — the category SMA-586 exists to remove.
    #[tokio::test]
    async fn a_router_arity_bug_keeps_its_own_server_error() {
        let valid = uuid::Uuid::from_u128(1);
        let (status, bytes) = probe(Router::new().route("/x/{a}/{b}", get(ok)), &format!("/x/{valid}/{valid}")).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(
            serde_json::from_slice::<serde_json::Value>(&bytes).is_err(),
            "axum's own plain-text rejection is preserved, not re-wrapped: {}",
            String::from_utf8_lossy(&bytes)
        );
    }

    /// SMA-586 fix round 2: each segment of a PAIR reports its OWN field.
    ///
    /// The first segment is the one this pins hardest — `revoke`'s `/{sa}/api-keys/{id}` used a
    /// single marker for both, so a malformed `{sa}` answered `api_key_id must be a uuid`.
    #[tokio::test]
    async fn each_segment_of_a_pair_names_its_own_field() {
        let valid = uuid::Uuid::from_u128(1);
        let route = || Router::new().route("/x/{sa}/y/{id}", get(ok_pair));

        let (status, bytes) = probe(route(), &format!("/x/not-a-uuid/y/{valid}")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"]["code"], invalid_uuid_wire());
        assert_eq!(body["error"]["message"], "service_account_id must be a uuid", "a malformed FIRST segment must name the first field");

        let (status, bytes) = probe(route(), &format!("/x/{valid}/y/not-a-uuid")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"]["message"], "api_key_id must be a uuid", "a malformed SECOND segment must name the second field");

        let (status, bytes) = probe(route(), &format!("/x/{valid}/y/{valid}")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(String::from_utf8(bytes).unwrap(), format!("{valid} {valid}"));
    }

    async fn ok_string(path: StringPath<PolicyId>) -> String {
        path.value
    }

    /// The registry's `invalid-path-segment` wire string, resolved through the enum rather
    /// than spelled as a literal — a literal in this `src/` file would put the module on
    /// `ci/error-registry/check.py`'s MANIFEST and blind that gate here.
    fn invalid_path_segment_wire() -> String {
        use paigasus_proto::paigasus::common::v1::ErrorReason;
        ErrorReason::InvalidPathSegment.as_wire_reason().expect("not the Unspecified sentinel")
    }

    /// SMA-588: a segment whose percent-decoding is not valid UTF-8 answers inside the error
    /// envelope, naming its field. `%FF` is not a valid UTF-8 sequence, so axum raises
    /// `FailedToDeserializePathParams` with a 400 status, which `is_client_error` admits.
    #[tokio::test]
    async fn an_undecodable_segment_answers_in_the_error_envelope() {
        let (status, bytes) = probe(Router::new().route("/x/{id}", get(ok_string)), "/x/%FF").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"]["code"], invalid_path_segment_wire());
        assert_eq!(body["error"]["message"], "policy_id is not a valid path segment");
    }

    /// An ordinary segment reaches the handler unchanged — including one that is not a uuid,
    /// which is the whole point of this extractor existing beside `UuidPath`.
    #[tokio::test]
    async fn an_ordinary_string_segment_extracts() {
        let (status, bytes) = probe(Router::new().route("/x/{id}", get(ok_string)), "/x/allow-root-read").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(String::from_utf8(bytes).unwrap(), "allow-root-read");
    }

    /// A ROUTER bug keeps its own 5xx rather than being relabelled as the caller's mistake —
    /// the same rule `UuidPath` and `json.rs`'s `classify` follow. Three extractors, one rule.
    #[tokio::test]
    async fn a_string_path_router_arity_bug_keeps_its_own_server_error() {
        let (status, bytes) = probe(Router::new().route("/x/{a}/{b}", get(ok_string)), "/x/one/two").await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(
            serde_json::from_slice::<serde_json::Value>(&bytes).is_err(),
            "axum's own plain-text rejection is preserved, not re-wrapped: {}",
            String::from_utf8_lossy(&bytes)
        );
    }

    /// A rename tripwire, nothing more: every marker's `NAME` is pinned to the literal its
    /// routes' errors carry, so renaming one is a deliberate edit here rather than a silent
    /// wire-contract change. It does NOT look at a route, so it cannot see a marker attached to
    /// the wrong handler — `each_segment_of_a_pair_names_its_own_field` and the integration
    /// coverage in `tests/` are what cover that.
    ///
    /// It also cannot see a marker with NO row here. `path_field!` is a `macro_rules!` macro, and
    /// `macro_rules!` cannot accumulate across invocations, so there is no list to count against
    /// — a count assertion here could only compare a literal to itself and would pass with every
    /// row below deleted. Closing that would mean collapsing the nine declarations into one
    /// registry-shaped invocation, which is a larger change than SMA-588 justifies. Stated rather
    /// than papered over with a tautology (SMA-588, controller Ruling 1).
    #[test]
    fn the_path_field_names_are_stable() {
        assert_eq!(OrganizationId::NAME, "organization_id");
        assert_eq!(TeamId::NAME, "team_id");
        assert_eq!(ProjectId::NAME, "project_id");
        assert_eq!(ServiceAccountId::NAME, "service_account_id");
        assert_eq!(ApiKeyId::NAME, "api_key_id");
        assert_eq!(MembershipId::NAME, "membership_id");
        assert_eq!(RoleGrantId::NAME, "role_grant_id");
        assert_eq!(DeadLetterId::NAME, "dead_letter_id");
        assert_eq!(PolicyId::NAME, "policy_id");
    }
}
