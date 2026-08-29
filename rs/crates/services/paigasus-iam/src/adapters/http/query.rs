// SPDX-License-Identifier: Apache-2.0

//! The house query-string extractor: `EnvelopeQuery<T>` answers a refused query string inside
//! IAM's stable `{"error":{code,message}}` envelope with a registered reason, instead of letting
//! axum's plain-text rejection escape the error contract (SMA-588).
//!
//! One module per input kind, a sibling of `json.rs` and `path.rs`. It takes `path.rs`'s side of
//! the split those two deliberately make (SMA-587 D2.1): it renders through
//! `ApiError(TenancyError::…)` rather than building an envelope from literals, because every
//! route it serves returns `Result<_, ApiError>`. `json.rs` must hand-build because it also
//! serves `api_keys::introspect`, whose funnel is `AuthnApiError`; that constraint does not
//! reach here. The payoff is that this file carries NO code literal, so it stays off
//! `ci/error-registry/check.py`'s MANIFEST and inherits `retryable` classification for free.
//!
//! **Two failure classes reach this extractor**, both measured against a real router:
//! a value that will not parse into its target type (`?limit=abc`), and a key supplied more
//! than once (`?limit=1&limit=2`). The second reaches EVERY field on every route, including
//! `Option<String>` ones, because a derived struct visitor raises `duplicate field` regardless
//! of type — axum directs callers wanting repeats to `axum_extra::extract::Query`. An unknown
//! key is not a failure at all; it is ignored.
//!
//! The message is static and names no parameter. axum's rejection DOES carry the key (it wraps
//! the deserializer in `serde_path_to_error`), but `TenancyError`'s payload is `&'static str` so
//! that untrusted input is structurally unable to reach an error body, and a runtime key cannot
//! become one without the `Box::leak` `application/error.rs` forbids.

use axum::extract::rejection::QueryRejection;
use axum::extract::{FromRequestParts, Query};
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use serde::de::DeserializeOwned;

use crate::adapters::http::error::ApiError;
use crate::application::error::TenancyError;

/// Was this rejection the CALLER's fault, or ours?
///
/// `QueryRejection` is `#[non_exhaustive]` and carries a single variant today
/// (`FailedToDeserializeQueryString`, a 400). This admits only `BAD_REQUEST` — the identical
/// rule `path.rs`'s `is_client_error` and `json.rs`'s `classify` follow — rather than any
/// `is_client_error()` status, so an axum variant added later with a DIFFERENT client status is
/// handed back to axum unchanged instead of being answered as `invalid-query-parameter`, a
/// reason that would no longer name the real condition. Anything not `BAD_REQUEST`, client or
/// server, is handed back to axum unchanged — three extractors answering unnamed statuses
/// differently would be worse than one plain-text 500.
fn is_client_error(rejection: &QueryRejection) -> bool {
    rejection.status() == StatusCode::BAD_REQUEST
}

/// `Query<T>` with the IAM error envelope on rejection.
///
/// This is the house extractor for EVERY query string on this adapter. A handler taking a bare
/// `axum::Query` in request position is a bug, and `repo:http-extractor-envelope` fails the
/// build on one.
#[derive(Debug)]
pub(crate) struct EnvelopeQuery<T>(pub(crate) T);

impl<S: Send + Sync, T: DeserializeOwned> FromRequestParts<S> for EnvelopeQuery<T> {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match Query::<T>::from_request_parts(parts, state).await {
            Ok(Query(value)) => Ok(EnvelopeQuery(value)),
            Err(rejection) if is_client_error(&rejection) => Err(ApiError(TenancyError::InvalidQueryParameter).into_response()),
            Err(rejection) => Err(rejection.into_response()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::to_bytes;
    use axum::http::StatusCode;
    use axum::routing::get;
    use serde::Deserialize;
    use tower::ServiceExt;

    #[derive(Debug, Deserialize)]
    struct Probe {
        limit: Option<i64>,
        name: Option<String>,
    }

    async fn ok(EnvelopeQuery(q): EnvelopeQuery<Probe>) -> String {
        format!("{:?}/{:?}", q.limit, q.name)
    }

    async fn probe(uri: &str) -> (StatusCode, Vec<u8>) {
        let app = Router::new().route("/x", get(ok));
        let resp = app.oneshot(axum::http::Request::builder().uri(uri).body(axum::body::Body::empty()).unwrap()).await.unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        (status, bytes.to_vec())
    }

    /// The registry's wire string, resolved through the enum — never a kebab literal, which
    /// would put this production module on `ci/error-registry/check.py`'s MANIFEST.
    fn invalid_query_parameter_wire() -> String {
        use paigasus_proto::paigasus::common::v1::ErrorReason;
        ErrorReason::InvalidQueryParameter.as_wire_reason().expect("not the Unspecified sentinel")
    }

    async fn assert_envelope(uri: &str) {
        let (status, bytes) = probe(uri).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}");
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"]["code"], invalid_query_parameter_wire(), "{uri}");
        assert_eq!(body["error"]["message"], "invalid query parameter", "{uri}");
    }

    /// Class 1: a value that will not parse into its target type.
    #[tokio::test]
    async fn an_unparseable_value_answers_in_the_error_envelope() {
        assert_envelope("/x?limit=abc").await;
    }

    /// Class 2: a REPEATED key — and on an `Option<String>` field, which no amount of type
    /// checking would predict. A derived struct visitor raises `duplicate field` regardless of
    /// the field's type, so this class reaches every field on every route. Missing it is what
    /// made the first draft of this ticket's spec conclude one route was unreachable.
    #[tokio::test]
    async fn a_repeated_key_answers_in_the_error_envelope() {
        assert_envelope("/x?limit=1&limit=2").await;
        assert_envelope("/x?name=a&name=b").await;
    }

    /// A well-formed query still reaches the handler, and an UNKNOWN key is ignored rather than
    /// refused — so the assertions above are about the query's shape, not about the route.
    #[tokio::test]
    async fn a_well_formed_query_extracts_and_unknown_keys_are_ignored() {
        let (status, bytes) = probe("/x?limit=7&name=n").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(String::from_utf8(bytes).unwrap(), "Some(7)/Some(\"n\")");

        let (status, bytes) = probe("/x?nosuchkey=abc").await;
        assert_eq!(status, StatusCode::OK, "an unknown key is not a rejection");
        assert_eq!(String::from_utf8(bytes).unwrap(), "None/None");
    }

    /// An absent query string is not a rejection either — every list route's params are
    /// `Option`, so `GET /x` must reach the handler.
    #[tokio::test]
    async fn an_absent_query_string_extracts() {
        let (status, _) = probe("/x").await;
        assert_eq!(status, StatusCode::OK);
    }

    /// Pins `is_client_error`'s equality rule directly against a real rejection, not only
    /// through the extractor's observable behaviour. `QueryRejection` is `#[non_exhaustive]`
    /// and carries exactly one variant today (`FailedToDeserializeQueryString`, always
    /// `BAD_REQUEST`), so this is the only status a REAL rejection can carry — unlike
    /// `bytes.rs`, this module has no free `classify` function and `QueryRejection`'s variant
    /// cannot be constructed from outside axum, so the negative branch (an unexpected client
    /// status handed back to axum unchanged) is not unit-testable here today. It is pinned
    /// directly for `bytes.rs::classify`, which carries no such construction restriction.
    #[tokio::test]
    async fn is_client_error_matches_the_only_real_rejection_today() {
        let request = axum::http::Request::builder().uri("/x?limit=abc").body(()).unwrap();
        let (mut parts, ()) = request.into_parts();
        let rejection = Query::<Probe>::from_request_parts(&mut parts, &()).await.unwrap_err();
        assert_eq!(rejection.status(), StatusCode::BAD_REQUEST);
        assert!(is_client_error(&rejection));
    }
}
