// SPDX-License-Identifier: Apache-2.0

//! The gRPC authn surface (spec §7.3/§7.4, D12/D14): `AuthnGrpc` implements the generated
//! `AuthnService.Introspect`/`IntrospectApiKey` (the latter since SMA-445 Task 21), and
//! `AuthLayer` is the bearer-enforcement tower layer wrapping the whole `grpc::router` (health
//! + tenancy + authn + authz + service-accounts) via `Server::builder().layer(..)`.
//!
//! Interceptors in tonic are sync-only, but `resolve` is async (a JWKS fetch may await), so
//! enforcement is a small tower `Service` rather than an interceptor. The layer wraps ALL
//! services on the server — including health — so the `:path` exemption check
//! (`/grpc.health.v1.Health/`, `/paigasus.iam.v1.AuthnService/Introspect`,
//! `/paigasus.iam.v1.AuthnService/IntrospectApiKey`) runs BEFORE any token extraction. A
//! rejection renders a proper trailers-only gRPC response (HTTP 200, `content-type:
//! application/grpc`, `grpc-status` + ASCII-safe `grpc-message`) via `tonic::Status::into_http`
//! — never a bare HTTP 401, which a gRPC client can't interpret.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Instant;

use paigasus_iam_core::{AuthnError, Credential, TokenDefect};
use paigasus_observability::record_grpc;
use paigasus_proto::paigasus::iam::v1::authn_service_server::AuthnService;
use paigasus_proto::paigasus::iam::v1::{IntrospectApiKeyRequest, IntrospectApiKeyResponse, IntrospectRequest, IntrospectResponse};
use tonic::body::Body;
use tonic::codegen::http;
use tonic::{Request, Response, Status};
use tower::{Layer, Service};

use super::convert;
use crate::adapters::auth::{AuthContext, bearer_from_headers};
use crate::adapters::http::AppState;
use crate::application::authenticate_token::Provisioning;

/// The `AuthnService` gRPC server — a thin adapter over the same `AppState.authn` use case
/// the HTTP `/v1/authn/introspect` handler drives.
pub struct AuthnGrpc {
    state: AppState,
}

impl AuthnGrpc {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

#[tonic::async_trait]
impl AuthnService for AuthnGrpc {
    /// `Introspect` (spec §7.2): the full `PrincipalContext` for a presented token. READ-ONLY
    /// (D10) — `AuthenticateToken::introspect` resolves with `Provisioning::Disabled`, so an
    /// unknown identity is `PermissionDenied` and this exempt RPC never has a user-creation
    /// side effect. The token IS the credential (in the request body, never the metadata):
    /// nothing here logs it, and errors funnel through `authn_status` (static messages only).
    async fn introspect(&self, request: Request<IntrospectRequest>) -> Result<Response<IntrospectResponse>, Status> {
        let started = Instant::now();
        let result: Result<Response<IntrospectResponse>, Status> = async {
            let token = request.into_inner().token;
            let ctx = self.state.authn.introspect(&token).await.map_err(|e| convert::authn_status(&e))?;
            Ok(Response::new(convert::to_introspect_response(&ctx)))
        }
        .await;
        record_grpc("Authentication", "Introspect", started, &result);
        result
    }

    /// `IntrospectApiKey` (spec §10.1, SMA-445 Task 21): API-key introspection, the peer of
    /// `Introspect` on `AuthnService`. Unauthenticated by design (the credential travels in
    /// the request body, never a bearer header) — mirrors `introspect` field-for-field, but
    /// delegates to `AppState.api_key_auth` (the `AuthenticateApiKey` use case, Task 18)
    /// instead of `AppState.authn`. Never logs the token; errors funnel through the same
    /// `authn_status` (static messages only).
    async fn introspect_api_key(&self, request: Request<IntrospectApiKeyRequest>) -> Result<Response<IntrospectApiKeyResponse>, Status> {
        let started = Instant::now();
        let result: Result<Response<IntrospectApiKeyResponse>, Status> = async {
            let token = request.into_inner().token;
            let ctx = self.state.api_key_auth.introspect(&token).await.map_err(|e| convert::authn_status(&e))?;
            Ok(Response::new(convert::to_introspect_api_key_response(&ctx)))
        }
        .await;
        record_grpc("Authentication", "IntrospectApiKey", started, &result);
        result
    }
}

/// The bearer-enforcement tower `Layer`, applied to the whole gRPC server via
/// `Server::builder().layer(AuthLayer::new(state))` (D14 — wrapping the router is where the
/// integration tests exercise it). Clones cheaply (`AppState` is `Clone`).
#[derive(Clone)]
pub struct AuthLayer {
    state: AppState,
}

impl AuthLayer {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

impl<S> Layer<S> for AuthLayer {
    type Service = AuthEnforce<S>;

    fn layer(&self, inner: S) -> Self::Service {
        AuthEnforce { inner, state: self.state.clone() }
    }
}

/// The `Service` `AuthLayer` produces: on every request it runs the `:path` exemption check,
/// then (for a protected path) extracts the bearer, `resolve`s it with `Provisioning::Enabled`
/// (JIT-provisioning an unknown identity, mirroring the HTTP middleware), and either forwards
/// with an [`AuthContext`] extension attached or short-circuits with a trailers-only gRPC
/// error WITHOUT calling the inner service.
#[derive(Clone)]
pub struct AuthEnforce<S> {
    inner: S,
    state: AppState,
}

/// Requests whose `:path` bypasses bearer enforcement (spec §7.4): the well-known health
/// service (all its methods, hence the prefix) and the unauthenticated `Introspect`/
/// `IntrospectApiKey` RPCs (SMA-445 Task 21 adds the latter — both credential-introspection
/// RPCs carry their token in the request body, not a bearer header, so neither can require
/// one). Every `ServiceAccountService` management RPC is deliberately NOT here — proven by
/// `tests/api_keys_grpc.rs::management_rpcs_not_exempt`.
fn is_exempt(path: &str) -> bool {
    path.starts_with("/grpc.health.v1.Health/") || path == "/paigasus.iam.v1.AuthnService/Introspect" || path == "/paigasus.iam.v1.AuthnService/IntrospectApiKey"
}

impl<S> Service<http::Request<Body>> for AuthEnforce<S>
where
    S: Service<http::Request<Body>, Response = http::Response<Body>> + Clone + Send + 'static,
    S::Future: Send + 'static,
{
    type Response = http::Response<Body>;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: http::Request<Body>) -> Self::Future {
        // Swap the `poll_ready`-readied inner out and leave a clone behind, so the instance
        // moved into the boxed future is exactly the one that was readied (the canonical tower
        // middleware pattern). The clone will be readied on the next `poll_ready`.
        let clone = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);
        let state = self.state.clone();
        Box::pin(async move {
            if is_exempt(req.uri().path()) {
                return inner.call(req).await;
            }
            // A missing or malformed `authorization` header is treated exactly like a rejected
            // token (D12): both are `Unauthenticated`.
            let Some(token) = bearer_from_headers(req.headers()) else {
                return Ok(reject(&AuthnError::InvalidToken(TokenDefect::Malformed)));
            };
            // Credential router (SMA-445 Task 19), mirroring the HTTP `require_bearer`
            // middleware's identical branch: a token carrying the configured API-key prefix
            // resolves through the API-key authenticator; everything else is an OIDC bearer.
            let resolved = if token.starts_with(&state.api_key_prefix) {
                state.api_key_auth.resolve(&token).await
            } else {
                state.authn.resolve(&token, Provisioning::Enabled).await
            };
            match resolved {
                Ok(principal) => {
                    // Cold-start bootstrap-admin seeding (SMA-444 Task 21b, D9/M4): mirrors
                    // the HTTP `require_bearer` middleware's call site exactly — only ever a
                    // no-op HashSet lookup for a non-bootstrap identity, and never reached by
                    // the exempt `Introspect` RPC (D10, `is_exempt` above). KEPT INSIDE THE
                    // OIDC ARM ONLY (SMA-445 Task 19): a service account has no
                    // (issuer, subject) pair and must never be JIT-granted platform_admin.
                    if let Credential::Oidc { issuer, subject, .. } = &principal.credential {
                        state.bootstrap_seeder.ensure_platform_admin(&principal.principal_id, issuer, subject).await;
                    }
                    req.extensions_mut().insert(AuthContext {
                        principal_id: principal.principal_id,
                        credential: principal.credential,
                    });
                    inner.call(req).await
                }
                Err(err) => Ok(reject(&err)),
            }
        })
    }
}

/// Renders an `AuthnError` as a trailers-only gRPC error response (HTTP 200,
/// `content-type: application/grpc`, `grpc-status` + ASCII-safe `grpc-message`) via
/// `Status::into_http` — never a bare HTTP 401, which a gRPC client can't interpret.
fn reject(err: &AuthnError) -> http::Response<Body> {
    convert::authn_status(err).into_http()
}
