// SPDX-License-Identifier: Apache-2.0

//! `AuthzLayer`: a reusable, coarse `tower` layer that authorizes every request it wraps
//! against a FIXED [`Action`] and a per-request resource [`Prn`] extracted by a
//! caller-supplied closure — denying with 403 before the inner service ever runs. Mirrors
//! `adapters::grpc::authn::AuthLayer`/`AuthEnforce`'s `Layer`/`Service` split (the same
//! shape, over axum's `Request`/`Response` instead of tonic's raw `http` types) — the form
//! a later M5 gateway reuses (SMA-444 Task 20 brief).
//!
//! **Not wired onto the tenancy routes today.** The per-handler `Authorize::check` calls in
//! `organizations.rs`/`teams.rs`/`projects.rs`/`memberships.rs`/`adapters::grpc::tenancy` are
//! what actually enforces `/v1/organizations`, `/v1/teams`, `/v1/projects`, and
//! `/v1/memberships` (Part 1 of the task-20 brief) — a per-node-type resource shape (parent
//! org for a create, the node itself for a get/rename/archive/restore, the target/queried
//! node for a membership op, `Root` for the two Root-only actions) doesn't reduce to ONE
//! fixed `Action` shared across a whole sub-router the way this layer wants. `AuthzLayer` is
//! shipped standalone, unit-tested, ready for a coarser surface (a gateway proxying whole
//! sub-trees behind one action) to reuse.
//!
//! **Resource extraction is coarse and cheap** — the extractor closure sees the request's
//! method/URI/headers (never the body), mirroring every other `AccessRequest` this crate
//! builds (empty `RequestContext`, no body inspection). It runs against the [`AuthContext`]
//! `auth_middleware::require_bearer` attaches — this layer MUST sit downstream of that
//! middleware in the stack (a missing `AuthContext` is treated as `Forbidden`, fail-closed,
//! never a panic).

use axum::extract::Request;
use axum::response::{IntoResponse, Response};
use paigasus_iam_core::Action;
use paigasus_kernel::Prn;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tower::{Layer, Service};

use super::error::ApiError;
use crate::adapters::auth::AuthContext;
use crate::application::authorize::Authorize;
use crate::application::error::TenancyError;

/// Extracts the resource [`Prn`] a request should be authorized against, given the request
/// itself (path/query/headers — never the body). `None` means the request doesn't carry
/// what the extractor needs (e.g. a malformed path param) — [`AuthzLayer`] treats that as
/// `Forbidden` (fail closed), never a panic or a silent pass-through.
pub trait ResourceExtractor: Send + Sync + 'static {
    fn resource(&self, req: &Request) -> Option<Prn>;
}

impl<F> ResourceExtractor for F
where
    F: Fn(&Request) -> Option<Prn> + Send + Sync + 'static,
{
    fn resource(&self, req: &Request) -> Option<Prn> {
        self(req)
    }
}

/// A `tower::Layer` that authorizes every request it wraps against a fixed [`Action`] plus a
/// per-request resource extracted by `R`. `Clone` is cheap: `Authorize` is an `Arc`-backed
/// handle and the extractor is `Arc`-wrapped here, mirroring `adapters::grpc::authn::AuthLayer`.
pub struct AuthzLayer<R> {
    authorize: Authorize,
    action: Action,
    extractor: Arc<R>,
}

impl<R> Clone for AuthzLayer<R> {
    fn clone(&self) -> Self {
        Self {
            authorize: self.authorize.clone(),
            action: self.action,
            extractor: self.extractor.clone(),
        }
    }
}

impl<R: ResourceExtractor> AuthzLayer<R> {
    /// Builds a layer that authorizes `actor` (the request's [`AuthContext`]) for `action`
    /// against whatever `extractor` resolves for the request, via `authorize.check` (D-per
    /// `Authorize::check`'s doc: `Effect::Deny` -> `TenancyError::Forbidden` -> 403; a
    /// genuine evaluation/backend failure -> `TenancyError::Internal` -> 500, never conflated
    /// with a deny).
    pub fn new(authorize: Authorize, action: Action, extractor: R) -> Self {
        Self {
            authorize,
            action,
            extractor: Arc::new(extractor),
        }
    }
}

impl<S, R: ResourceExtractor> Layer<S> for AuthzLayer<R> {
    type Service = AuthzEnforce<S, R>;

    fn layer(&self, inner: S) -> Self::Service {
        AuthzEnforce {
            inner,
            authorize: self.authorize.clone(),
            action: self.action,
            extractor: self.extractor.clone(),
        }
    }
}

/// The `Service` `AuthzLayer` produces. On every request: resolve the [`AuthContext`] +
/// resource, `authorize.check` them against the fixed `Action`, and either forward to the
/// inner service on `Ok(())` or short-circuit with the mapped error response — never calling
/// the inner service on a deny.
pub struct AuthzEnforce<S, R> {
    inner: S,
    authorize: Authorize,
    action: Action,
    extractor: Arc<R>,
}

impl<S: Clone, R> Clone for AuthzEnforce<S, R> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            authorize: self.authorize.clone(),
            action: self.action,
            extractor: self.extractor.clone(),
        }
    }
}

impl<S, R> Service<Request> for AuthzEnforce<S, R>
where
    S: Service<Request, Response = Response> + Clone + Send + 'static,
    S::Future: Send + 'static,
    R: ResourceExtractor,
{
    type Response = Response;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request) -> Self::Future {
        // Swap the `poll_ready`-readied inner out and leave a clone behind, so the instance
        // moved into the boxed future is exactly the one that was readied (the canonical
        // tower middleware pattern — mirrors `adapters::grpc::authn::AuthEnforce::call`).
        let clone = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);
        let authorize = self.authorize.clone();
        let action = self.action;
        let extractor = self.extractor.clone();

        Box::pin(async move {
            let ctx = req.extensions().get::<AuthContext>().cloned();
            let resource = extractor.resource(&req);
            let (Some(ctx), Some(resource)) = (ctx, resource) else {
                // No `AuthContext` means this layer ran ahead of `require_bearer` (a wiring
                // defect); no resolvable resource means the extractor couldn't place this
                // request. Both fail closed as `Forbidden` rather than panicking or
                // forwarding an unauthorized request.
                return Ok(ApiError(TenancyError::Forbidden).into_response());
            };
            match authorize.check(ctx.principal_id.prn(), action, &resource).await {
                Ok(()) => inner.call(req).await,
                Err(err) => Ok(ApiError(err).into_response()),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::fakes::FakeAuthorizer;
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request as HttpRequest, StatusCode};
    use axum::routing::get;
    use paigasus_iam_core::PrincipalId;
    use paigasus_kernel::Prn;
    use std::sync::Arc as StdArc;
    use tower::ServiceExt;
    use uuid::Uuid;

    async fn ok_handler() -> &'static str {
        "ok"
    }

    fn fixed_resource() -> Prn {
        Prn::build("iam", "", None, "organization", Uuid::from_u128(42)).unwrap()
    }

    fn ctx(n: u128) -> AuthContext {
        let prn = Prn::build("iam", "", None, "principal", Uuid::from_u128(n)).unwrap();
        AuthContext {
            principal_id: PrincipalId::from_prn(prn),
            issuer: paigasus_iam_core::Issuer::parse("https://idp.example.com/").unwrap(),
            subject: "test-subject".to_string(),
        }
    }

    fn app(authorize: Authorize) -> Router {
        let layer = AuthzLayer::new(authorize, Action::GetOrganization, |_req: &Request| Some(fixed_resource()));
        Router::new().route("/x", get(ok_handler)).layer(layer)
    }

    #[tokio::test]
    async fn allow_passes_through_to_the_inner_service() {
        let fake = FakeAuthorizer::default();
        fake.allow(Action::GetOrganization, &fixed_resource());
        let authorize = Authorize::new(StdArc::new(fake));

        let mut req = HttpRequest::builder().uri("/x").body(Body::empty()).unwrap();
        req.extensions_mut().insert(ctx(1));

        let response = app(authorize).oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn deny_short_circuits_with_403_and_never_reaches_the_inner_service() {
        let authorize = Authorize::new(StdArc::new(FakeAuthorizer::default()));

        let mut req = HttpRequest::builder().uri("/x").body(Body::empty()).unwrap();
        req.extensions_mut().insert(ctx(2));

        let response = app(authorize).oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn missing_auth_context_fails_closed_as_forbidden() {
        // No `AuthContext` extension inserted — simulates this layer running ahead of
        // `require_bearer` (a wiring defect this layer must never turn into a panic or an
        // unauthenticated pass-through).
        let fake = FakeAuthorizer::default();
        fake.allow(Action::GetOrganization, &fixed_resource());
        let authorize = Authorize::new(StdArc::new(fake));

        let req = HttpRequest::builder().uri("/x").body(Body::empty()).unwrap();
        let response = app(authorize).oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
}
