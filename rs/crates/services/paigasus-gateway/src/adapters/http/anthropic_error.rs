// SPDX-License-Identifier: Apache-2.0

//! The Anthropic error envelope of the `/v1/messages` route group (SMA-750 D7, § 6).
//!
//! Every gateway-made error on those routes has the shape
//! `{"type":"error","error":{"type","message","code"},"request_id"}`. The handlers and the auth
//! middleware still return a [`GatewayError`]; its `IntoResponse` renders the OpenAI envelope and
//! tags the response with [`RenderedError`]. [`rerender`] then swaps the body, keeps the status
//! and every header (`WWW-Authenticate`, `paigasus-*`), and adds the two Anthropic SDK headers.
//! A response without the tag (an upstream body) passes through byte for byte.

use axum::body::Body;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::Response;
use serde::Serialize;

use crate::adapters::http::error::{GatewayError, RenderedError, SHOULD_RETRY_HEADER};

/// The least `retry-after` of a budget refusal: a retry before the next minute cannot succeed.
const BUDGET_RETRY_AFTER_FLOOR_SECS: u32 = 61;

/// The `MissingBearer` message on the Anthropic routes, which accept two credential headers.
const MISSING_CREDENTIALS_MESSAGE: &str = "Missing credentials. Send the x-api-key header or an Authorization: Bearer header.";

#[derive(Serialize)]
struct Envelope<'a> {
    r#type: &'static str,
    error: Inner<'a>,
    request_id: Option<&'a str>,
}

#[derive(Serialize)]
struct Inner<'a> {
    r#type: &'static str,
    message: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<&'static str>,
}

/// The Anthropic error `type` for a gateway error (§ 6.1).
pub fn anthropic_type(err: GatewayError) -> &'static str {
    match err {
        GatewayError::MissingBearer | GatewayError::InvalidCredential | GatewayError::InvalidDpopProof => "authentication_error",
        GatewayError::AuthzDenied => "permission_error",
        GatewayError::RequestTooLarge => "request_too_large",
        GatewayError::RateLimited { .. } => "rate_limit_error",
        GatewayError::BudgetExhausted { .. } => "billing_error",
        GatewayError::UpstreamTimeout => "timeout_error",
        GatewayError::MissingScope | GatewayError::IamUnavailable | GatewayError::Internal | GatewayError::UpstreamUnavailable | GatewayError::ToolInterceptionMalformedResponse => "api_error",
        GatewayError::BadRequestBody
        | GatewayError::InvalidRequestSchema
        | GatewayError::StreamingDisabled
        | GatewayError::InvalidOrgHeader
        | GatewayError::OrgRequired
        | GatewayError::ResponsesStatefulNotSupported { .. }
        | GatewayError::ToolResultUnreplaceable => "invalid_request_error",
    }
}

fn body_bytes(envelope: &Envelope<'_>) -> Vec<u8> {
    // A struct of strings and options always serializes.
    serde_json::to_vec(envelope).unwrap_or_else(|_| br#"{"type":"error","error":{"type":"api_error","message":"Internal error."},"request_id":null}"#.to_vec())
}

/// The Anthropic envelope of `err`. `request_id` is the `paigasus-request-id` of the request, or
/// `None` (a JSON `null`) outside a request scope.
pub fn render(err: GatewayError, request_id: Option<&str>) -> Vec<u8> {
    let message = match err {
        GatewayError::MissingBearer => MISSING_CREDENTIALS_MESSAGE.to_owned(),
        other => other.message(),
    };
    let (_, _, code, _, _) = err.parts();
    body_bytes(&Envelope {
        r#type: "error",
        error: Inner {
            r#type: anthropic_type(err),
            message: &message,
            code,
        },
        request_id,
    })
}

fn request_id() -> Option<String> {
    paigasus_observability::current_ids().map(|ids| ids.request_id.to_string())
}

fn with_body(mut response: Response, bytes: Vec<u8>) -> Response {
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(bytes.len()));
    *response.body_mut() = Body::from(bytes);
    response
}

/// For `axum::middleware::map_response`: re-renders a tagged gateway error in the Anthropic
/// envelope. The status and every other header stay. A response with no [`RenderedError`] is
/// returned unchanged.
pub async fn rerender(resp: Response) -> Response {
    let Some(RenderedError(err)) = resp.extensions().get::<RenderedError>().copied() else {
        return resp;
    };
    let id = request_id();
    let bytes = render(err, id.as_deref());
    let mut response = with_body(resp, bytes);
    let headers = response.headers_mut();
    match err {
        GatewayError::BudgetExhausted { retry_after_secs, .. } => {
            headers.insert(header::RETRY_AFTER, HeaderValue::from(retry_after_secs.max(BUDGET_RETRY_AFTER_FLOOR_SECS)));
        }
        GatewayError::ToolInterceptionMalformedResponse => {
            headers.insert(SHOULD_RETRY_HEADER, HeaderValue::from_static("false"));
        }
        _ => {}
    }
    response
}

fn fallback(status: StatusCode, error_type: &'static str, message: &str) -> Response {
    let id = request_id();
    let bytes = body_bytes(&Envelope {
        r#type: "error",
        error: Inner {
            r#type: error_type,
            message,
            code: None,
        },
        request_id: id.as_deref(),
    });
    let mut response = with_body(Response::new(Body::empty()), bytes);
    *response.status_mut() = status;
    response
}

/// The router fallback of the Anthropic group: 404 `not_found_error`, with no `code`.
pub async fn not_found() -> Response {
    fallback(StatusCode::NOT_FOUND, "not_found_error", "Not found.")
}

/// The method fallback of the Anthropic group: 405 `invalid_request_error`, with no `code`.
pub async fn method_not_allowed() -> Response {
    fallback(StatusCode::METHOD_NOT_ALLOWED, "invalid_request_error", "Method not allowed.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::to_bytes;
    use axum::http::Request;
    use axum::middleware::map_response;
    use axum::response::IntoResponse;
    use axum::routing::get;
    use strum::IntoEnumIterator;
    use tower::ServiceExt;

    use crate::domain::limits::BudgetPeriod;

    fn json(bytes: &[u8]) -> serde_json::Value {
        serde_json::from_slice(bytes).expect("a JSON body")
    }

    async fn body_of(resp: Response) -> Vec<u8> {
        to_bytes(resp.into_body(), usize::MAX).await.unwrap().to_vec()
    }

    fn keys(value: &serde_json::Value) -> Vec<&str> {
        let mut keys: Vec<&str> = value.as_object().expect("an object").keys().map(String::as_str).collect();
        keys.sort_unstable();
        keys
    }

    fn budget(retry_after_secs: u32) -> GatewayError {
        GatewayError::BudgetExhausted {
            resets_at_unix: 1_793_491_200,
            period: BudgetPeriod::Monthly,
            retry_after_secs,
        }
    }

    /// The § 6.1 table, keyed by status; the two 429 cases differ by variant.
    fn table(err: GatewayError) -> &'static str {
        match (err.status().as_u16(), err) {
            (400, _) => "invalid_request_error",
            (401, _) => "authentication_error",
            (403, _) => "permission_error",
            (413, _) => "request_too_large",
            (429, GatewayError::RateLimited { .. }) => "rate_limit_error",
            (429, GatewayError::BudgetExhausted { .. }) => "billing_error",
            (500 | 502 | 503, _) => "api_error",
            (504, _) => "timeout_error",
            (status, err) => panic!("{err:?} has status {status}, which the § 6.1 table does not list"),
        }
    }

    #[test]
    fn each_case_maps_to_its_anthropic_type() {
        for err in GatewayError::iter() {
            assert_eq!(anthropic_type(err), table(err), "{err:?}");
        }
        assert_eq!(anthropic_type(budget(5)), "billing_error");
        assert_eq!(anthropic_type(GatewayError::RateLimited { retry_after_secs: 1 }), "rate_limit_error");
        assert_eq!(anthropic_type(GatewayError::UpstreamTimeout), "timeout_error");
    }

    #[test]
    fn the_body_is_the_anthropic_envelope() {
        for err in GatewayError::iter() {
            let body = json(&render(err, Some("req-1")));
            assert_eq!(keys(&body), ["error", "request_id", "type"], "{err:?}");
            assert_eq!(body["type"], "error");
            assert_eq!(body["request_id"], "req-1");
            assert_eq!(keys(&body["error"]), ["code", "message", "type"], "{err:?}");
            let (_, _, code, _, _) = err.parts();
            assert_eq!(body["error"]["code"], code.expect("a registry code"), "{err:?}");
            assert_eq!(body["error"]["type"], anthropic_type(err));
            if matches!(err, GatewayError::MissingBearer) {
                assert_eq!(body["error"]["message"], "Missing credentials. Send the x-api-key header or an Authorization: Bearer header.");
            } else {
                assert_eq!(body["error"]["message"], err.message(), "{err:?}");
            }
        }
        assert!(json(&render(GatewayError::Internal, None))["request_id"].is_null());
    }

    async fn rerendered(handler_err: GatewayError) -> Response {
        let app = Router::new()
            .route(
                "/",
                get(move || async move {
                    let mut resp = handler_err.into_response();
                    resp.headers_mut().append("www-authenticate", HeaderValue::from_static("Bearer realm=\"paigasus\""));
                    resp.headers_mut().append("paigasus-tool-decisions", HeaderValue::from_static("blocked=1"));
                    resp
                }),
            )
            .layer(map_response(rerender));
        app.oneshot(Request::get("/").body(Body::empty()).unwrap()).await.unwrap()
    }

    #[tokio::test]
    async fn the_rerender_keeps_status_and_headers() {
        let resp = rerendered(GatewayError::MissingBearer).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(resp.headers()["www-authenticate"], "Bearer realm=\"paigasus\"");
        assert_eq!(resp.headers()["paigasus-tool-decisions"], "blocked=1");
        assert_eq!(resp.headers()["paigasus-retryable"], "false");
        assert_eq!(resp.headers()["content-type"], "application/json");
        let length: usize = resp.headers()["content-length"].to_str().unwrap().parse().unwrap();
        let bytes = body_of(resp).await;
        assert_eq!(length, bytes.len());
        assert_eq!(bytes, render(GatewayError::MissingBearer, None));
        assert_eq!(json(&bytes)["error"]["type"], "authentication_error");
    }

    /// R5: an upstream error body is the upstream's, not the gateway's, so it is never rewritten.
    #[tokio::test]
    async fn an_upstream_body_is_never_rerendered() {
        let upstream = br#"{"type":"error","error":{"type":"invalid_request_error","message":"max_tokens: Field required"}}"#;
        let app = Router::new()
            .route(
                "/",
                get(|| async {
                    let mut resp = Response::new(Body::from(upstream.as_slice()));
                    *resp.status_mut() = StatusCode::BAD_REQUEST;
                    resp.headers_mut().insert("content-type", HeaderValue::from_static("application/json"));
                    resp.headers_mut().insert("request-id", HeaderValue::from_static("req_upstream"));
                    resp
                }),
            )
            .layer(map_response(rerender));
        let resp = app.oneshot(Request::get("/").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert_eq!(resp.headers()["request-id"], "req_upstream");
        assert_eq!(body_of(resp).await, upstream);
    }

    #[tokio::test]
    async fn a_budget_refusal_sends_at_least_61_seconds() {
        let short = rerendered(budget(5)).await;
        assert_eq!(short.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(short.headers()["retry-after"], "61");
        assert_eq!(short.headers()["x-should-retry"], "false");
        assert_eq!(json(&body_of(short).await)["error"]["type"], "billing_error");
        let long = rerendered(budget(3600)).await;
        assert_eq!(long.headers()["retry-after"], "3600");
        assert_eq!(long.headers()["x-should-retry"], "false");
    }

    #[tokio::test]
    async fn a_rate_refusal_keeps_its_retry_after() {
        let resp = rerendered(GatewayError::RateLimited { retry_after_secs: 17 }).await;
        assert_eq!(resp.headers()["retry-after"], "17");
        assert_eq!(resp.headers()["x-should-retry"], "true");
        assert_eq!(json(&body_of(resp).await)["error"]["type"], "rate_limit_error");
        let floor = rerendered(GatewayError::RateLimited { retry_after_secs: 0 }).await;
        assert_eq!(floor.headers()["retry-after"], "1");
    }

    #[tokio::test]
    async fn the_malformed_502_says_do_not_retry() {
        let resp = rerendered(GatewayError::ToolInterceptionMalformedResponse).await;
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
        assert_eq!(resp.headers()["x-should-retry"], "false");
        let other = rerendered(GatewayError::UpstreamUnavailable).await;
        assert!(other.headers().get("x-should-retry").is_none());
    }

    /// The body's `request_id` is the `paigasus-request-id` of the scope the request runs in.
    #[tokio::test]
    async fn the_request_id_is_the_one_of_the_request_scope() {
        use paigasus_observability::RequestIds;
        use paigasus_observability::correlation::scope_for_test;

        let ids = RequestIds {
            request_id: uuid::Uuid::parse_str("0198f2c1-3333-7000-8000-000000000042").unwrap(),
            correlation_id: uuid::Uuid::parse_str("0198f2c1-4444-7000-8000-000000000042").unwrap(),
        };
        let want = ids.request_id.to_string();
        let resp = scope_for_test(ids, rerendered(GatewayError::MissingBearer)).await;
        assert_eq!(json(&body_of(resp).await)["request_id"], want.as_str());
        let resp = scope_for_test(ids, not_found()).await;
        assert_eq!(json(&body_of(resp).await)["request_id"], want.as_str());
        let resp = scope_for_test(ids, method_not_allowed()).await;
        assert_eq!(json(&body_of(resp).await)["request_id"], want.as_str());
    }

    #[tokio::test]
    async fn the_fallbacks_have_no_code() {
        let resp = not_found().await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let body = json(&body_of(resp).await);
        assert_eq!(body["error"]["type"], "not_found_error");
        assert!(body["error"].get("code").is_none());
        assert_eq!(body["type"], "error");

        let resp = method_not_allowed().await;
        assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
        let body = json(&body_of(resp).await);
        assert_eq!(body["error"]["type"], "invalid_request_error");
        assert!(body["error"].get("code").is_none());
    }
}
