// SPDX-License-Identifier: Apache-2.0

//! A fake IAM gRPC server for the tests that need a real gRPC link (the latency test). It serves
//! `IntrospectApiKey` and `IsAuthorized` on plaintext h2c at `127.0.0.1:0`. Every other RPC
//! returns `Status::unimplemented`. (The in-process stub of the `Iam` port is `stub_iam.rs`.)

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use paigasus_proto::paigasus::iam::v1::authn_service_server::{AuthnService, AuthnServiceServer};
use paigasus_proto::paigasus::iam::v1::authorization_service_server::{AuthorizationService, AuthorizationServiceServer};
use paigasus_proto::paigasus::iam::v1::{
    DeletePolicyRequest, DeletePolicyResponse, GrantRoleRequest, GrantRoleResponse, IntrospectApiKeyRequest, IntrospectApiKeyResponse, IntrospectRequest, IntrospectResponse, IsAuthorizedRequest,
    IsAuthorizedResponse, ListPoliciesRequest, ListPoliciesResponse, ListRoleGrantsRequest, ListRoleGrantsResponse, PutPolicyRequest, PutPolicyResponse, RetireSystemPolicyRequest,
    RetireSystemPolicyResponse, RevokeRoleRequest, RevokeRoleResponse, WhoAmIRequest, WhoAmIResponse,
};
use tokio::task::JoinHandle;
use tonic::{Request, Response, Status};
use uuid::Uuid;

/// One active API key. The self-query allows every action.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub struct FakeIam {
    token: String,
    principal: Uuid,
    introspections: AtomicUsize,
    authorizations: AtomicUsize,
}

#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
impl FakeIam {
    /// A fake with one active API key `token` of `principal`.
    pub fn new(token: &str, principal: Uuid) -> Arc<FakeIam> {
        Arc::new(FakeIam {
            token: token.to_owned(),
            principal,
            introspections: AtomicUsize::new(0),
            authorizations: AtomicUsize::new(0),
        })
    }

    /// How many `IntrospectApiKey` calls arrived.
    pub fn introspections(&self) -> usize {
        self.introspections.load(Ordering::SeqCst)
    }

    /// How many `IsAuthorized` calls arrived.
    pub fn authorizations(&self) -> usize {
        self.authorizations.load(Ordering::SeqCst)
    }
}

/// The tonic service wrapper. The generated traits are foreign, so a local type implements them.
struct Svc(Arc<FakeIam>);

#[tonic::async_trait]
impl AuthnService for Svc {
    async fn introspect(&self, _request: Request<IntrospectRequest>) -> Result<Response<IntrospectResponse>, Status> {
        Err(Status::unimplemented("the fake IAM does not serve Introspect"))
    }

    async fn introspect_api_key(&self, request: Request<IntrospectApiKeyRequest>) -> Result<Response<IntrospectApiKeyResponse>, Status> {
        self.0.introspections.fetch_add(1, Ordering::SeqCst);
        if request.into_inner().token != self.0.token {
            return Err(Status::unauthenticated("unknown key"));
        }
        Ok(Response::new(IntrospectApiKeyResponse {
            status: "active".to_owned(),
            principal_prn: format!("prn:pgs:iam:::principal/{}", self.0.principal),
            scope_prn: "prn:pgs:iam:::root/00000000-0000-0000-0000-000000000000".to_owned(),
            key_id: "key-1".to_owned(),
            ..Default::default()
        }))
    }

    async fn who_am_i(&self, _request: Request<WhoAmIRequest>) -> Result<Response<WhoAmIResponse>, Status> {
        Err(Status::unimplemented("not served"))
    }
}

#[tonic::async_trait]
impl AuthorizationService for Svc {
    async fn is_authorized(&self, _request: Request<IsAuthorizedRequest>) -> Result<Response<IsAuthorizedResponse>, Status> {
        self.0.authorizations.fetch_add(1, Ordering::SeqCst);
        Ok(Response::new(IsAuthorizedResponse { allowed: true, ..Default::default() }))
    }

    async fn put_policy(&self, _request: Request<PutPolicyRequest>) -> Result<Response<PutPolicyResponse>, Status> {
        Err(Status::unimplemented("not served"))
    }

    async fn delete_policy(&self, _request: Request<DeletePolicyRequest>) -> Result<Response<DeletePolicyResponse>, Status> {
        Err(Status::unimplemented("not served"))
    }

    async fn list_policies(&self, _request: Request<ListPoliciesRequest>) -> Result<Response<ListPoliciesResponse>, Status> {
        Err(Status::unimplemented("not served"))
    }

    async fn grant_role(&self, _request: Request<GrantRoleRequest>) -> Result<Response<GrantRoleResponse>, Status> {
        Err(Status::unimplemented("not served"))
    }

    async fn revoke_role(&self, _request: Request<RevokeRoleRequest>) -> Result<Response<RevokeRoleResponse>, Status> {
        Err(Status::unimplemented("not served"))
    }

    async fn list_role_grants(&self, _request: Request<ListRoleGrantsRequest>) -> Result<Response<ListRoleGrantsResponse>, Status> {
        Err(Status::unimplemented("not served"))
    }

    async fn retire_system_policy(&self, _request: Request<RetireSystemPolicyRequest>) -> Result<Response<RetireSystemPolicyResponse>, Status> {
        Err(Status::unimplemented("not served"))
    }
}

/// Serves the fake on `127.0.0.1:0` over plaintext h2c. The task ends when the test calls
/// `abort`, or when the test process ends.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub async fn serve_grpc(iam: Arc<FakeIam>) -> (SocketAddr, JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind the fake IAM");
    let addr = listener.local_addr().expect("local addr");
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
    let handle = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(AuthnServiceServer::new(Svc(iam.clone())))
            .add_service(AuthorizationServiceServer::new(Svc(iam)))
            .serve_with_incoming(incoming)
            .await
            .expect("the fake IAM server");
    });
    (addr, handle)
}
