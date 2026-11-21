// SPDX-License-Identifier: Apache-2.0

//! The outbound IAM gRPC adapter (spec §6.2). It is a copy of the API-key and bearer parts of the
//! gateway adapter (`paigasus-gateway/src/adapters/iam/client.rs`). The DPoP parts are not copied.
//!
//! The adapter makes three calls:
//!
//! - `IntrospectApiKey` and `Introspect` carry the caller's token in the request body. They attach
//!   no `authorization` metadata.
//! - `IsAuthorized` is the self-query. The caller's own credential is the `authorization`
//!   metadata, and the caller's own principal PRN is the queried principal. So IAM sees a
//!   principal that asks about itself. This module builds the request from its arguments only.
//!   The call site in `http::auth` takes the credential and the principal from the same request.

use std::time::Duration;

use async_trait::async_trait;
use paigasus_proto::paigasus::iam::v1::authn_service_client::AuthnServiceClient;
use paigasus_proto::paigasus::iam::v1::authorization_service_client::AuthorizationServiceClient;
use paigasus_proto::paigasus::iam::v1::{IntrospectApiKeyRequest, IntrospectApiKeyResponse, IntrospectRequest, IntrospectResponse, IsAuthorizedRequest};
use tonic::Request;
use tonic::metadata::MetadataValue;
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Identity};

use crate::config::{IamClientConfig, IamTlsConfig};

/// An error of the IAM adapter.
///
/// - [`IamError::Connect`]: the channel, the URI or the TLS material is not valid, or a caller
///   credential cannot be a metadata value. The channel connects lazily, so an IAM that is down
///   shows as `Rpc(Status::Unavailable)` on the first call, not as this variant.
/// - [`IamError::Rpc`]: a call returned a gRPC `Status`. The whole `Status` is kept, so the
///   caller can map it by `Status::code()` and by its `ErrorInfo` reason.
#[derive(Debug, thiserror::Error)]
pub enum IamError {
    #[error("failed to build the IAM gRPC channel: {0}")]
    Connect(String),

    #[error("IAM gRPC call failed: {0}")]
    Rpc(#[from] tonic::Status),
}

/// The credential that the self-query presents to IAM. Both forms send
/// `authorization: Bearer <secret>`. `Debug` prints no secret.
#[derive(Clone, PartialEq, Eq)]
pub enum CallerCredential {
    /// An API key of a service account.
    ApiKey(String),
    /// An OIDC access token.
    Bearer(String),
}

impl std::fmt::Debug for CallerCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            CallerCredential::ApiKey(_) => "ApiKey(..)",
            CallerCredential::Bearer(_) => "Bearer(..)",
        })
    }
}

/// The IAM calls of the ingress. The port is a trait object (`dyn Iam`), so it uses
/// `#[async_trait]`. The unit tests of `http::auth` use a fake.
#[async_trait]
pub trait Iam: Send + Sync {
    /// Introspect an API key. The token is the request body. No `authorization` metadata.
    async fn introspect_api_key(&self, token: &str) -> Result<IntrospectApiKeyResponse, IamError>;

    /// Introspect an OIDC access token (`AuthnService.Introspect`). The token is the request body.
    /// No `authorization` metadata, and no DPoP context.
    async fn introspect_token(&self, token: &str) -> Result<IntrospectResponse, IamError>;

    /// The self-query `IsAuthorized`. `caller` is the `authorization` metadata, and
    /// `principal_prn` must be the principal of that same credential.
    async fn is_authorized_self(&self, caller: &CallerCredential, principal_prn: &str, action: &str, resource_prn: &str) -> Result<bool, IamError>;
}

/// The real IAM adapter. One lazy `tonic` channel serves both generated clients.
#[derive(Clone, Debug)]
pub struct IamClient {
    authn: AuthnServiceClient<Channel>,
    authz: AuthorizationServiceClient<Channel>,
    /// `iam.call_timeout_ms`. It bounds each whole call (see [`bounded`]).
    call_timeout: Duration,
}

impl IamClient {
    /// Build the channel from the config. The channel connects on the first call, so an IAM that
    /// is down does not stop the boot. The TLS material is read now, so a bad certificate fails at
    /// boot. `call_timeout_ms` bounds each whole call: the wait for a connection, the response
    /// headers, the response message and the trailers ([`bounded`]). It also bounds the TCP
    /// connect and the TLS handshake one by one. The channel makes the connection in its own
    /// task, so a call that stops at its bound does not stop the connect. Call it inside a Tokio
    /// runtime.
    pub fn connect(cfg: &IamClientConfig) -> Result<IamClient, IamError> {
        let channel = build_channel(cfg)?;
        Ok(IamClient {
            authn: AuthnServiceClient::new(channel.clone()),
            authz: AuthorizationServiceClient::new(channel),
            call_timeout: Duration::from_millis(cfg.call_timeout_ms),
        })
    }
}

#[async_trait]
impl Iam for IamClient {
    async fn introspect_api_key(&self, token: &str) -> Result<IntrospectApiKeyResponse, IamError> {
        let mut authn = self.authn.clone();
        let resp = bounded(self.call_timeout, authn.introspect_api_key(introspect_request(token))).await?;
        Ok(resp.into_inner())
    }

    async fn introspect_token(&self, token: &str) -> Result<IntrospectResponse, IamError> {
        let mut authn = self.authn.clone();
        let resp = bounded(self.call_timeout, authn.introspect(token_introspect_request(token))).await?;
        Ok(resp.into_inner())
    }

    async fn is_authorized_self(&self, caller: &CallerCredential, principal_prn: &str, action: &str, resource_prn: &str) -> Result<bool, IamError> {
        let req = self_authorize_request(caller, principal_prn, action, resource_prn)?;
        let mut authz = self.authz.clone();
        let resp = bounded(self.call_timeout, authz.is_authorized(req)).await?;
        Ok(resp.into_inner().allowed)
    }
}

/// Bound one whole unary call by `limit`. `Endpoint::timeout` is not enough: tonic's
/// `GrpcTimeout` stops when the response HEADERS arrive, and the generated client then decodes
/// the message and reads the trailers with no bound. So an IAM that sends the headers and then
/// stalls could hold a request with no limit. On expiry the call is dropped, and the result is
/// `Status::deadline_exceeded`. `http::auth` maps that code to `503 iam-unavailable`.
async fn bounded<T>(limit: Duration, call: impl std::future::Future<Output = Result<T, tonic::Status>>) -> Result<T, IamError> {
    match tokio::time::timeout(limit, call).await {
        Ok(result) => result.map_err(IamError::from),
        Err(_) => Err(IamError::Rpc(tonic::Status::deadline_exceeded(format!(
            "the IAM call did not end within iam.call_timeout_ms ({} ms)",
            limit.as_millis()
        )))),
    }
}

/// Attach the correlation id of the current request to an IAM call, so one id spans the logs of
/// both services. Outside a request scope it attaches nothing. The request id is not forwarded.
fn with_correlation<T>(mut req: Request<T>) -> Request<T> {
    if let Some(ids) = paigasus_observability::current_ids()
        && let Ok(value) = ids.correlation_id.to_string().parse()
    {
        req.metadata_mut().insert(paigasus_observability::correlation::CORRELATION_ID_HEADER, value);
    }
    req
}

/// The `IntrospectApiKey` request. No `authorization` metadata.
fn introspect_request(token: &str) -> Request<IntrospectApiKeyRequest> {
    with_correlation(Request::new(IntrospectApiKeyRequest { token: token.to_owned() }))
}

/// The `Introspect` request. No `authorization` metadata, and no DPoP context.
fn token_introspect_request(token: &str) -> Request<IntrospectRequest> {
    with_correlation(Request::new(IntrospectRequest { token: token.to_owned(), dpop: None }))
}

/// The self-query `IsAuthorized` request. The body carries the arguments as they are. The
/// caller's credential is the `authorization` metadata. `context` is empty.
///
/// A credential that cannot be a metadata value gives [`IamError::Connect`]. The message names
/// no credential.
fn self_authorize_request(caller: &CallerCredential, principal_prn: &str, action: &str, resource_prn: &str) -> Result<Request<IsAuthorizedRequest>, IamError> {
    let mut req = with_correlation(Request::new(IsAuthorizedRequest {
        principal_prn: principal_prn.to_owned(),
        action: action.to_owned(),
        resource_prn: resource_prn.to_owned(),
        context: Default::default(),
    }));
    let secret = match caller {
        CallerCredential::ApiKey(secret) | CallerCredential::Bearer(secret) => secret,
    };
    // The error text of `try_from` names no value, so no credential reaches the message.
    let authorization = MetadataValue::try_from(format!("Bearer {secret}")).map_err(|e| IamError::Connect(format!("the caller credential is not a valid `authorization` metadata value: {e}")))?;
    req.metadata_mut().insert("authorization", authorization);
    Ok(req)
}

/// Build the shared channel. `connect_lazy` does the TCP handshake on the first call. tonic builds
/// the TLS connector in `tls_config`, so bad TLS material fails here.
fn build_channel(cfg: &IamClientConfig) -> Result<Channel, IamError> {
    let endpoint = Channel::from_shared(cfg.grpc_addr.clone()).map_err(|e| IamError::Connect(format!("invalid iam.grpc_addr {:?}: {e}", cfg.grpc_addr)))?;
    // `timeout` starts only when a call starts on a ready connection. It does not bound the
    // connect, which the channel makes before that. So `connect_timeout` bounds the TCP connect:
    // without it, a black-holed IAM address holds each request for the TCP connect timeout of the
    // OS (about 75 s on macOS, about 2 min on Linux) before the 503.
    let call_timeout = Duration::from_millis(cfg.call_timeout_ms);
    let endpoint = endpoint.connect_timeout(call_timeout).timeout(call_timeout);

    let endpoint = match &cfg.tls {
        // `Config::validate` proved that the host is a loopback host. Plain h2c, no TLS.
        IamTlsConfig::LoopbackInsecure => endpoint,
        IamTlsConfig::Tls {
            ca_cert_path,
            client_cert_path,
            client_key_path,
        } => {
            let tls = build_tls_config(ca_cert_path, client_cert_path, client_key_path, call_timeout)?;
            endpoint.tls_config(tls).map_err(|e| IamError::Connect(format!("invalid IAM TLS config: {e}")))?
        }
    };

    Ok(endpoint.connect_lazy())
}

/// The TLS config: ring-backed rustls (the `tls-ring` feature).
///
/// A configured `ca_cert_path` replaces the trust store: the client trusts that CA only. With no
/// `ca_cert_path`, the client uses the platform trust store (the `tls-native-roots` feature).
///
/// mTLS: the client identity needs both the certificate and the key. Only one of the two is a
/// [`IamError::Connect`]. The pair is checked before any file is read.
///
/// `handshake_timeout` bounds the TLS handshake.
fn build_tls_config(ca_cert_path: &Option<String>, client_cert_path: &Option<String>, client_key_path: &Option<String>, handshake_timeout: Duration) -> Result<ClientTlsConfig, IamError> {
    let identity = match (client_cert_path, client_key_path) {
        (Some(cert_path), Some(key_path)) => {
            let cert = std::fs::read(cert_path).map_err(|e| IamError::Connect(format!("failed to read iam.tls client_cert_path {cert_path:?}: {e}")))?;
            let key = std::fs::read(key_path).map_err(|e| IamError::Connect(format!("failed to read iam.tls client_key_path {key_path:?}: {e}")))?;
            Some(Identity::from_pem(cert, key))
        }
        (None, None) => None,
        (Some(_), None) | (None, Some(_)) => {
            return Err(IamError::Connect(
                "iam.tls client_cert_path and client_key_path must be set together (mTLS needs \
                 both the client certificate and its private key, or neither)"
                    .to_string(),
            ));
        }
    };

    let mut tls = match ca_cert_path {
        Some(path) => {
            let pem = std::fs::read(path).map_err(|e| IamError::Connect(format!("failed to read iam.tls ca_cert_path {path:?}: {e}")))?;
            ClientTlsConfig::new().ca_certificate(Certificate::from_pem(pem))
        }
        None => ClientTlsConfig::new().with_native_roots(),
    };

    if let Some(identity) = identity {
        tls = tls.identity(identity);
    }

    // `connect_timeout` bounds the TCP connect only. This bounds the TLS handshake, so a server
    // that accepts TCP and then sends nothing cannot hold a request.
    Ok(tls.timeout(handshake_timeout))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(grpc_addr: &str, tls: IamTlsConfig) -> IamClientConfig {
        IamClientConfig {
            grpc_addr: grpc_addr.to_owned(),
            tls,
            call_timeout_ms: 2000,
        }
    }

    #[tokio::test]
    async fn self_authorize_request_sets_bearer_and_body() {
        // The self-query at the unit level: the caller's own key is the `authorization: Bearer`
        // metadata, and the body carries exactly the arguments. The call runs in a correlation
        // scope, so the correlation id is checked here too.
        let caller_key = "sk-caller-abc123";
        let ids = paigasus_observability::RequestIds {
            request_id: uuid::Uuid::from_u128(3),
            correlation_id: uuid::Uuid::from_u128(4),
        };
        let req = paigasus_observability::correlation::scope_for_test(ids, async {
            self_authorize_request(
                &CallerCredential::ApiKey(caller_key.to_owned()),
                "prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-0000000000e0",
                "WriteInboxEvent",
                "prn:pgs:iam:::organization/0190a100-0000-7000-8000-0000000000a1",
            )
            .expect("valid metadata")
        })
        .await;

        let bearer = req.metadata().get("authorization").expect("authorization metadata must be present").to_str().expect("ascii metadata");
        assert_eq!(bearer, format!("Bearer {caller_key}"));

        let body = req.get_ref();
        assert_eq!(body.principal_prn, "prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-0000000000e0");
        assert_eq!(body.action, "WriteInboxEvent");
        assert_eq!(body.resource_prn, "prn:pgs:iam:::organization/0190a100-0000-7000-8000-0000000000a1");
        assert!(body.context.is_empty(), "the self-query sends no context attributes");

        assert_eq!(req.metadata().get("paigasus-correlation-id").unwrap().to_str().unwrap(), ids.correlation_id.to_string());
        assert!(req.metadata().get("paigasus-request-id").is_none(), "the request id is per hop and is not forwarded");
    }

    #[test]
    fn a_bearer_caller_sends_the_bearer_scheme() {
        let req = self_authorize_request(&CallerCredential::Bearer("user-token".into()), "p", "WriteInboxEvent", "r").expect("valid metadata");
        assert_eq!(req.metadata().get("authorization").unwrap().to_str().unwrap(), "Bearer user-token");
        assert!(req.metadata().get("dpop").is_none());
    }

    #[test]
    fn a_credential_that_is_not_a_metadata_value_is_a_connect_error_without_the_secret() {
        let err = self_authorize_request(&CallerCredential::Bearer("bad\nkey-secret".into()), "p", "WriteInboxEvent", "r").expect_err("a newline is not valid metadata");
        assert!(matches!(err, IamError::Connect(_)));
        assert!(!err.to_string().contains("key-secret"), "{err}");
    }

    #[test]
    fn a_token_introspect_carries_no_authorization_and_no_dpop_context() {
        let req = token_introspect_request("tok");
        assert!(req.metadata().get("authorization").is_none(), "Introspect is bearer-exempt");
        assert_eq!(req.get_ref().token, "tok");
        assert_eq!(req.get_ref().dpop, None);
    }

    #[test]
    fn caller_credential_debug_prints_no_secret() {
        let printed = format!("{:?} {:?}", CallerCredential::ApiKey("key-secret".into()), CallerCredential::Bearer("tok-secret".into()));
        assert!(!printed.contains("secret"), "{printed}");
    }

    #[test]
    fn introspect_request_has_no_authorization_metadata() {
        let req = introspect_request("some-opaque-token");
        assert!(req.metadata().get("authorization").is_none(), "introspect must not carry authorization metadata");
        assert_eq!(req.get_ref().token, "some-opaque-token");
    }

    #[tokio::test]
    async fn outbound_requests_carry_the_ambient_correlation_id() {
        let ids = paigasus_observability::RequestIds {
            request_id: uuid::Uuid::from_u128(1),
            correlation_id: uuid::Uuid::from_u128(2),
        };
        let req = paigasus_observability::correlation::scope_for_test(ids, async { introspect_request("tok") }).await;
        assert_eq!(req.metadata().get("paigasus-correlation-id").unwrap().to_str().unwrap(), ids.correlation_id.to_string());
        assert!(req.metadata().get("paigasus-request-id").is_none(), "the request id is per hop and is not forwarded");
    }

    #[tokio::test]
    async fn connect_succeeds_for_loopback_insecure() {
        let cfg = cfg("http://127.0.0.1:9090", IamTlsConfig::LoopbackInsecure);
        assert!(IamClient::connect(&cfg).is_ok(), "a lazy loopback connect must not fail without a live server");
    }

    #[tokio::test]
    async fn connect_succeeds_for_default_tls_system_trust() {
        // TLS with no CA and no client identity: the platform trust store. tonic builds the TLS
        // connector now, so this also loads the native roots.
        let cfg = cfg(
            "https://127.0.0.1:9090",
            IamTlsConfig::Tls {
                ca_cert_path: None,
                client_cert_path: None,
                client_key_path: None,
            },
        );
        assert!(IamClient::connect(&cfg).is_ok(), "a lazy TLS connect must not fail without a live server");
    }

    #[tokio::test]
    async fn connect_fails_for_mtls_with_only_client_cert() {
        let cfg = cfg(
            "https://iam.internal.example.com:9090",
            IamTlsConfig::Tls {
                ca_cert_path: None,
                client_cert_path: Some("does-not-need-to-exist-client.pem".to_string()),
                client_key_path: None,
            },
        );
        let err = IamClient::connect(&cfg).expect_err("a client cert without a key must be rejected");
        assert!(matches!(err, IamError::Connect(_)), "expected IamError::Connect, got {err:?}");
    }

    #[tokio::test]
    async fn connect_fails_for_an_invalid_address() {
        let err = IamClient::connect(&cfg("not a uri", IamTlsConfig::LoopbackInsecure)).expect_err("an invalid URI must be rejected");
        assert!(matches!(err, IamError::Connect(_)), "expected IamError::Connect, got {err:?}");
    }

    // ---- the time bounds of one call (spec §6.2: IAM unreachable gives a fast 503) --------------

    /// `call_timeout_ms` in the bound tests. It is far below the TCP connect timeout of the OS.
    const SHORT_MS: u64 = 200;
    /// The time a bounded call may take: three steps of 200 ms, with a large margin.
    const BUDGET: Duration = Duration::from_secs(2);

    /// A TCP address whose connect hangs. The guards keep the listener and its queue alive.
    struct HangingTarget {
        addr: std::net::SocketAddr,
        _listener: Option<tokio::net::TcpListener>,
        _queued: Vec<std::net::TcpStream>,
    }

    /// Linux drops the SYN of a connect when the accept queue of a listener is full, so the
    /// connect hangs. This needs no network. (macOS answers a full queue with a reset instead.)
    #[cfg(target_os = "linux")]
    fn hanging_tcp_target() -> HangingTarget {
        let socket = tokio::net::TcpSocket::new_v4().expect("socket");
        socket.bind("127.0.0.1:0".parse().unwrap()).expect("bind");
        let listener = socket.listen(0).expect("listen");
        let addr = listener.local_addr().expect("addr");
        // Fill the accept queue. Nothing accepts, so the first connect that hangs marks it full.
        let mut queued = Vec::new();
        for _ in 0..16 {
            match std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(200)) {
                Ok(stream) => queued.push(stream),
                Err(_) => break,
            }
        }
        HangingTarget {
            addr,
            _listener: Some(listener),
            _queued: queued,
        }
    }

    /// Other hosts: TEST-NET-1 (RFC 5737). It is never routed, so with a network the SYN gets
    /// no answer and the connect hangs.
    #[cfg(not(target_os = "linux"))]
    fn hanging_tcp_target() -> HangingTarget {
        HangingTarget {
            addr: "192.0.2.1:1".parse().unwrap(),
            _listener: None,
            _queued: Vec::new(),
        }
    }

    /// A local listener that accepts each connection and then sends nothing.
    async fn silent_server() -> std::net::SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((stream, _)) = listener.accept().await {
                held.push(stream);
            }
        });
        addr
    }

    /// Run one IAM call with `call_timeout_ms = SHORT_MS` and a 2 s budget. It must end in time,
    /// and with an error.
    async fn bounded_call(mut cfg: IamClientConfig) -> tonic::Code {
        cfg.call_timeout_ms = SHORT_MS;
        let client = IamClient::connect(&cfg).expect("lazy connect");
        let outcome = tokio::time::timeout(BUDGET, client.introspect_api_key("tok")).await;
        let result = outcome.unwrap_or_else(|_| panic!("the IAM call to {} did not end within {BUDGET:?}", cfg.grpc_addr));
        match result {
            Err(IamError::Rpc(status)) => status.code(),
            other => panic!("expected an IAM Rpc error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn the_call_timeout_bounds_a_tcp_connect_that_hangs() {
        let target = hanging_tcp_target();
        // Precondition: a plain TCP connect to the target hangs on this host. Without it, this
        // test would pass with no bound at all.
        let probe = tokio::time::timeout(Duration::from_millis(500), tokio::net::TcpStream::connect(target.addr)).await;
        assert!(
            probe.is_err(),
            "precondition failed: a TCP connect to {} does not hang on this host ({probe:?}); the test cannot prove the bound here",
            target.addr
        );
        let code = bounded_call(cfg(&format!("http://{}", target.addr), IamTlsConfig::LoopbackInsecure)).await;
        assert!(matches!(code, tonic::Code::Unavailable | tonic::Code::DeadlineExceeded), "{code:?}");
    }

    #[tokio::test]
    async fn the_call_timeout_bounds_a_tls_handshake_that_hangs() {
        let addr = silent_server().await;
        let tls = IamTlsConfig::Tls {
            ca_cert_path: None,
            client_cert_path: None,
            client_key_path: None,
        };
        let code = bounded_call(cfg(&format!("https://{addr}"), tls)).await;
        assert!(matches!(code, tonic::Code::Unavailable | tonic::Code::DeadlineExceeded), "{code:?}");
    }

    #[tokio::test]
    async fn the_call_timeout_bounds_a_silent_h2c_server() {
        let addr = silent_server().await;
        let code = bounded_call(cfg(&format!("http://{addr}"), IamTlsConfig::LoopbackInsecure)).await;
        assert!(matches!(code, tonic::Code::Unavailable | tonic::Code::DeadlineExceeded | tonic::Code::Cancelled), "{code:?}");
    }

    /// An h2c server that answers every gRPC call with `200` and `application/grpc` headers, and
    /// then sends no message and no trailers. The endpoint timeout stops at the headers, so only
    /// the whole-call bound can end such a call.
    async fn headers_then_stall_server() -> std::net::SocketAddr {
        use axum::body::Body;
        use axum::http::{Response, header};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let app = axum::Router::new().fallback(|| async {
            Response::builder()
                .status(200)
                .header(header::CONTENT_TYPE, "application/grpc")
                .body(Body::from_stream(futures::stream::pending::<Result<axum::body::Bytes, std::io::Error>>()))
                .expect("a valid response")
        });
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        addr
    }

    #[tokio::test]
    async fn the_call_timeout_bounds_a_server_that_stalls_after_the_headers() {
        let addr = headers_then_stall_server().await;
        let mut config = cfg(&format!("http://{addr}"), IamTlsConfig::LoopbackInsecure);
        config.call_timeout_ms = SHORT_MS;
        let client = IamClient::connect(&config).expect("lazy connect");
        let caller = CallerCredential::ApiKey("k".into());
        // Each of the three calls must end within the budget, with `DeadlineExceeded`.
        let started = std::time::Instant::now();
        let key = tokio::time::timeout(BUDGET, client.introspect_api_key("tok"))
            .await
            .expect("IntrospectApiKey must end within the budget");
        let token = tokio::time::timeout(BUDGET, client.introspect_token("tok")).await.expect("Introspect must end within the budget");
        let authz = tokio::time::timeout(BUDGET, client.is_authorized_self(&caller, "p", "WriteInboxEvent", "r"))
            .await
            .expect("IsAuthorized must end within the budget");
        for (name, code) in [
            ("IntrospectApiKey", key.map(|_| ()).err()),
            ("Introspect", token.map(|_| ()).err()),
            ("IsAuthorized", authz.map(|_| ()).err()),
        ] {
            match code {
                Some(IamError::Rpc(status)) => assert_eq!(status.code(), tonic::Code::DeadlineExceeded, "{name}: {status:?}"),
                other => panic!("{name}: expected a DeadlineExceeded Rpc error, got {other:?}"),
            }
        }
        // Each call waited for its bound, so the headers did arrive and the stall was after them.
        assert!(started.elapsed() >= Duration::from_millis(3 * SHORT_MS), "{:?}", started.elapsed());
    }

    #[test]
    fn rpc_error_preserves_status_code() {
        let err: IamError = tonic::Status::unavailable("iam is down").into();
        match err {
            IamError::Rpc(status) => {
                assert_eq!(status.code(), tonic::Code::Unavailable);
                assert_eq!(status.message(), "iam is down");
            }
            other => panic!("expected IamError::Rpc, got {other:?}"),
        }
    }
}
