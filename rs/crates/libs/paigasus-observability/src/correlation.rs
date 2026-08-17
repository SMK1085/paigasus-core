// SPDX-License-Identifier: Apache-2.0

//! Request-scoped correlation: the two ids every Paigasus response carries, and the tower layer
//! that mints, adopts and stamps them.
//!
//! One `Layer`, generic over the body type, wraps IAM's axum API router, IAM's tonic server AND
//! the gateway's axum router — gRPC metadata *is* HTTP/2 headers, so both protocols read the same
//! header names and no second implementation is needed.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::{SystemTime, UNIX_EPOCH};

use http::{HeaderValue, Request, Response, StatusCode};
use tower::{Layer, Service};
use uuid::Uuid;

/// Server-minted, never read from the request (the guideline marks it Forbidden inbound).
pub const REQUEST_ID_HEADER: &str = "paigasus-request-id";
/// Adopted from the caller when it parses as a UUID, else minted.
pub const CORRELATION_ID_HEADER: &str = "paigasus-correlation-id";
/// `"true"` | `"false"` | `"unknown"` — set by an error renderer, or defaulted by the layer.
pub const RETRYABLE_HEADER: &str = "paigasus-retryable";

/// The two request-scoped ids.
///
/// `request_id` identifies THIS individual call — server-minted, never read from the client — so
/// repeated or retried calls stay separable. `correlation_id` tracks ONE logical invocation
/// end-to-end across services, adopted from the caller when it parses as a UUID (D6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestIds {
    pub request_id: Uuid,
    pub correlation_id: Uuid,
}

/// Whether a client should retry. Three states, not two, and deliberately so: an `internal`
/// error erases whether its source was a transient backend blip or a logic bug, and claiming
/// `false` there would be WORSE than the status-class guess this header replaces (ADR-0019 D7,
/// spec D4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Retryable {
    Yes,
    No,
    Unknown,
}

impl Retryable {
    /// The exact wire spelling. Never `"1"`/`"0"` — clients match these three literals.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Yes => "true",
            Self::No => "false",
            Self::Unknown => "unknown",
        }
    }

    /// The layer's fallback for an error response no renderer owns (axum's 404/405,
    /// `DefaultBodyLimit`'s 413, `TimeoutLayer`'s 408). This IS status-class inference — the very
    /// thing the header exists to remove — and it is used only where the alternative is an absent
    /// header a client would have to interpret.
    pub fn from_status(status: StatusCode) -> Self {
        match status {
            StatusCode::REQUEST_TIMEOUT | StatusCode::TOO_MANY_REQUESTS | StatusCode::BAD_GATEWAY | StatusCode::SERVICE_UNAVAILABLE | StatusCode::GATEWAY_TIMEOUT => Self::Yes,
            s if s.is_server_error() => Self::Unknown,
            _ => Self::No,
        }
    }
}

tokio::task_local! {
    static IDS: RequestIds;
}

/// The ids for the in-flight request HEAD, or `None` outside that scope.
///
/// `None` in three situations, not one: unit tests that render directly; background tasks; and
/// **response-body streaming** — a `task_local!` scope set in `Service::call` ends when the future
/// resolves to `Response<Body>`, and hyper polls the body afterwards. Callers omit the id fields
/// entirely rather than substituting a nil UUID that would read as a real id.
pub fn current_ids() -> Option<RequestIds> {
    IDS.try_with(|ids| *ids).ok()
}

/// Mints a UUIDv7 the one way this repo mints one: `paigasus-kernel` does the layout, the host
/// supplies clock and entropy. NOT `uuid`'s `v7` feature — enabling it anywhere in `rs/` enables
/// `uuid/rng` (and `getrandom`) for every `uuid` dependent under feature unification, including
/// `paigasus-kernel`, whose wasm story depends on staying feature-free.
fn mint() -> Uuid {
    let ms = SystemTime::now().duration_since(UNIX_EPOCH).expect("system clock before 1970").as_millis() as u64;
    paigasus_kernel::mint_uuid7(ms, rand::random::<[u8; 10]>())
}

/// Reads an inbound correlation id, accepting it ONLY if it parses as a UUID (D6).
fn adopt_or_mint<B>(req: &Request<B>) -> Uuid {
    // `HeaderMap::get` takes the FIRST value; duplicates are ignored, same posture as discarding
    // a malformed one.
    let Some(raw) = req.headers().get(CORRELATION_ID_HEADER) else {
        return mint();
    };
    match raw.to_str().ok().and_then(|s| Uuid::parse_str(s).ok()) {
        Some(id) => id,
        None => {
            // Names the rejection, NEVER the value: an adopted value would reach logs, response
            // headers, gRPC metadata and audit rows, which is the whole reason it is rejected.
            tracing::debug!("discarded a malformed inbound {CORRELATION_ID_HEADER}; minted a fresh one");
            mint()
        }
    }
}

/// The layer. See the module docs for why one implementation covers axum and tonic both.
#[derive(Debug, Clone, Copy, Default)]
pub struct CorrelationLayer;

impl<S> Layer<S> for CorrelationLayer {
    type Service = CorrelationService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        CorrelationService { inner }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CorrelationService<S> {
    inner: S,
}

impl<S, ReqBody, ResBody> Service<Request<ReqBody>> for CorrelationService<S>
where
    S: Service<Request<ReqBody>, Response = Response<ResBody>> + Clone + Send + 'static,
    S::Future: Send + 'static,
    ReqBody: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<ReqBody>) -> Self::Future {
        let ids = RequestIds {
            request_id: mint(),
            correlation_id: adopt_or_mint(&req),
        };
        // `Clone` + `replace`: the `self.inner` that was `poll_ready`ed is the one that must be
        // called, so swap a fresh clone in rather than calling the clone (the standard tower
        // readiness dance).
        let clone = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);
        Box::pin(async move {
            let mut resp = IDS.scope(ids, inner.call(req)).await?;
            let status = resp.status();
            let headers = resp.headers_mut();
            // Unconditional: a downstream value for either id is not authoritative.
            headers.insert(REQUEST_ID_HEADER, header_value(ids.request_id));
            headers.insert(CORRELATION_ID_HEADER, header_value(ids.correlation_id));
            // Retryable: fill in a default ONLY where a renderer supplied none, and only on an
            // error — `Retryable::as_wire` returns `&'static str`, which is what `from_static`
            // wants, so this allocates nothing.
            let is_error = status.is_client_error() || status.is_server_error();
            if is_error && !headers.contains_key(RETRYABLE_HEADER) {
                headers.insert(RETRYABLE_HEADER, HeaderValue::from_static(Retryable::from_status(status).as_wire()));
            }
            Ok(resp)
        })
    }
}

/// A `Uuid`'s hyphenated form is always a valid header value, so this never fails.
fn header_value(id: Uuid) -> HeaderValue {
    HeaderValue::from_str(&id.to_string()).expect("a hyphenated UUID is a valid header value")
}

/// Enters an id scope directly. Production code enters it via [`CorrelationLayer`]; this exists
/// for tests, which must be able to assert what a renderer emits INSIDE a request scope without
/// standing up a server. Tasks 7 and 8 both depend on it.
pub async fn scope_for_test<F: Future>(ids: RequestIds, f: F) -> F::Output {
    IDS.scope(ids, f).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, Response, StatusCode};
    use tower::{ServiceExt, service_fn};
    use uuid::Uuid;

    /// Runs one request through the layer and hands back the response.
    async fn through_layer(req: Request<Body>) -> Response<Body> {
        let inner = service_fn(|_req: Request<Body>| async {
            // The handler observes the ids the layer entered the scope with.
            let ids = current_ids().expect("the layer must have entered a scope");
            let mut resp = Response::new(Body::empty());
            resp.headers_mut().insert("x-observed-request-id", ids.request_id.to_string().parse().unwrap());
            resp.headers_mut().insert("x-observed-correlation-id", ids.correlation_id.to_string().parse().unwrap());
            Ok::<_, std::convert::Infallible>(resp)
        });
        tower::Layer::layer(&CorrelationLayer, inner).oneshot(req).await.unwrap()
    }

    fn get(headers: &[(&str, &str)]) -> Request<Body> {
        let mut b = Request::builder().uri("/x");
        for (k, v) in headers {
            b = b.header(*k, *v);
        }
        b.body(Body::empty()).unwrap()
    }

    #[tokio::test]
    async fn mints_both_ids_and_sets_both_headers() {
        let resp = through_layer(get(&[])).await;
        let req_id = resp.headers()[REQUEST_ID_HEADER].to_str().unwrap();
        let corr_id = resp.headers()[CORRELATION_ID_HEADER].to_str().unwrap();
        assert!(Uuid::parse_str(req_id).is_ok(), "request id must be a UUID: {req_id}");
        assert!(Uuid::parse_str(corr_id).is_ok(), "correlation id must be a UUID: {corr_id}");
        assert_ne!(req_id, corr_id, "the two ids are independent");
        // The handler saw exactly what the response advertises.
        assert_eq!(resp.headers()["x-observed-request-id"], req_id);
        assert_eq!(resp.headers()["x-observed-correlation-id"], corr_id);
    }

    #[tokio::test]
    async fn adopts_a_well_formed_inbound_correlation_id() {
        let supplied = "0198f2c1-1111-7000-8000-000000000042";
        let resp = through_layer(get(&[(CORRELATION_ID_HEADER, supplied)])).await;
        assert_eq!(resp.headers()[CORRELATION_ID_HEADER], supplied);
        assert_eq!(resp.headers()["x-observed-correlation-id"], supplied);
    }

    /// D6: an adopted value reaches logs, two response headers, outbound gRPC metadata and
    /// persisted audit rows, so anything that is not a UUID is replaced and NEVER echoed.
    #[tokio::test]
    async fn replaces_a_malformed_inbound_correlation_id_and_never_echoes_it() {
        let hostile = "not-a-uuid-<script>";
        let resp = through_layer(get(&[(CORRELATION_ID_HEADER, hostile)])).await;
        let corr_id = resp.headers()[CORRELATION_ID_HEADER].to_str().unwrap();
        assert!(Uuid::parse_str(corr_id).is_ok(), "a malformed id must be replaced, got {corr_id}");
        let rendered = format!("{:?}", resp.headers());
        assert!(!rendered.contains("script"), "the supplied value must never be echoed anywhere");
    }

    /// The guideline marks `paigasus-request-id` Forbidden on requests: server-set, ignored
    /// when sent by the client.
    #[tokio::test]
    async fn overwrites_a_client_supplied_request_id() {
        let supplied = "0198f2c1-2222-7000-8000-000000000042";
        let resp = through_layer(get(&[(REQUEST_ID_HEADER, supplied)])).await;
        assert_ne!(resp.headers()[REQUEST_ID_HEADER], supplied, "a client-sent request id must be overwritten");
    }

    #[test]
    fn current_ids_is_none_outside_a_scope() {
        assert!(current_ids().is_none(), "no scope means no ids — never a nil-UUID stand-in");
    }

    #[test]
    fn retryable_wire_values_are_the_three_documented_strings() {
        assert_eq!(Retryable::Yes.as_wire(), "true");
        assert_eq!(Retryable::No.as_wire(), "false");
        assert_eq!(Retryable::Unknown.as_wire(), "unknown");
    }

    /// The layer's status-class default (D4). It exists so the header is present on error
    /// responses no renderer owns — axum's 404/405, DefaultBodyLimit's 413, TimeoutLayer's 408.
    #[test]
    fn status_class_default_marks_transient_statuses_retryable() {
        for s in [
            StatusCode::REQUEST_TIMEOUT,
            StatusCode::TOO_MANY_REQUESTS,
            StatusCode::BAD_GATEWAY,
            StatusCode::SERVICE_UNAVAILABLE,
            StatusCode::GATEWAY_TIMEOUT,
        ] {
            assert_eq!(Retryable::from_status(s), Retryable::Yes, "{s} should default retryable");
        }
        assert_eq!(Retryable::from_status(StatusCode::INTERNAL_SERVER_ERROR), Retryable::Unknown);
        assert_eq!(Retryable::from_status(StatusCode::NOT_FOUND), Retryable::No);
        assert_eq!(Retryable::from_status(StatusCode::OK), Retryable::No);
    }

    #[tokio::test]
    async fn the_layer_defaults_retryable_only_when_the_inner_service_did_not_set_it() {
        // Inner sets its own value: the layer must not clobber it.
        let inner = service_fn(|_req: Request<Body>| async {
            let mut resp = Response::new(Body::empty());
            *resp.status_mut() = StatusCode::SERVICE_UNAVAILABLE;
            resp.headers_mut().insert(RETRYABLE_HEADER, "unknown".parse().unwrap());
            Ok::<_, std::convert::Infallible>(resp)
        });
        let resp = tower::Layer::layer(&CorrelationLayer, inner).oneshot(get(&[])).await.unwrap();
        assert_eq!(resp.headers()[RETRYABLE_HEADER], "unknown", "a renderer's value wins over the layer default");

        // Inner sets nothing: the layer fills in the status-class default.
        let bare = service_fn(|_req: Request<Body>| async {
            let mut resp = Response::new(Body::empty());
            *resp.status_mut() = StatusCode::SERVICE_UNAVAILABLE;
            Ok::<_, std::convert::Infallible>(resp)
        });
        let resp = tower::Layer::layer(&CorrelationLayer, bare).oneshot(get(&[])).await.unwrap();
        assert_eq!(resp.headers()[RETRYABLE_HEADER], "true", "an unowned 503 still carries a signal");
    }

    #[tokio::test]
    async fn a_success_carries_ids_but_no_retryable_header() {
        let resp = through_layer(get(&[])).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(resp.headers().get(RETRYABLE_HEADER).is_none(), "retryable is an error-response concern");
    }

    /// `scope_for_test` and `current_ids` must read the SAME task-local — Tasks 7 and 8 assert
    /// what a renderer emits by entering a scope with the former and reading it with the latter,
    /// and a mismatch would surface there as a silent `None` rather than as a failure here.
    #[tokio::test]
    async fn scope_for_test_is_visible_to_current_ids_and_ends_with_the_scope() {
        let ids = RequestIds {
            request_id: Uuid::parse_str("0198f2c1-3333-7000-8000-000000000042").unwrap(),
            correlation_id: Uuid::parse_str("0198f2c1-4444-7000-8000-000000000042").unwrap(),
        };
        let seen = scope_for_test(ids, async { current_ids() }).await;
        assert_eq!(seen, Some(ids));
        assert!(current_ids().is_none(), "the scope must not outlive the future it wrapped");
    }
}
