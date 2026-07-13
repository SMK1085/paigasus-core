// SPDX-License-Identifier: Apache-2.0

//! Outbound IAM gRPC adapter: a `tonic` client that (a) introspects a caller-presented API key
//! (bearer-EXEMPT — the token travels in the request body) and (b) performs the **self-query**
//! `IsAuthorized` (the D9 security invariant — the gateway asks IAM whether *its own caller* may
//! act, presenting that caller's own key as the bearer and that caller's own SA PRN as the
//! principal).
//!
//! The [`Iam`] port trait is the seam G5's auth middleware depends on: it lets G5's decision
//! table be unit-tested against a fake IAM (no live server), while G7 wires the real
//! [`IamClient`]. This module builds requests *faithfully from its arguments* and never invents
//! a principal — the self-query invariant (principal == the introspected caller SA **and**
//! bearer == that same caller's own key) is guaranteed at G5's call site, which passes both from
//! the same inbound request.

use async_trait::async_trait;
use paigasus_proto::paigasus::iam::v1::authn_service_client::AuthnServiceClient;
use paigasus_proto::paigasus::iam::v1::authorization_service_client::AuthorizationServiceClient;
use paigasus_proto::paigasus::iam::v1::{IntrospectApiKeyRequest, IntrospectApiKeyResponse, IsAuthorizedRequest};
use tonic::Request;
use tonic::metadata::MetadataValue;
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Identity};

use crate::config::{IamClientConfig, IamTlsConfig};

/// Errors from the outbound IAM adapter. Two variants by design — G5 maps each to an HTTP
/// status:
/// - [`IamError::Connect`] is a boot-/build-time channel or TLS-material failure (invalid URI,
///   unreadable/mismatched certs, a caller key that cannot form a valid metadata value). With
///   `connect_lazy` this is a startup-time fault, not a per-request one (a dead IAM surfaces
///   later as `Rpc(Status::Unavailable)`); G5 maps it to 503.
/// - [`IamError::Rpc`] preserves the whole `tonic::Status` a call returned, so G5 can branch on
///   `Status::code()` (`Unavailable`/`DeadlineExceeded` → 503, `Unauthenticated` → 401,
///   `PermissionDenied` → context-dependent). The status→HTTP mapping itself is G5's job — this
///   adapter only preserves the `Status` intact (never flattening it to a string).
#[derive(Debug, thiserror::Error)]
pub enum IamError {
    /// Channel/URI/TLS-material build failure (boot time under `connect_lazy`).
    #[error("failed to build the IAM gRPC channel: {0}")]
    Connect(String),

    /// An IAM call returned a gRPC `Status`. Preserved whole so G5 maps by `.code()`.
    #[error("IAM gRPC call failed: {0}")]
    Rpc(#[from] tonic::Status),
}

/// Port trait for the two IAM calls the gateway's auth middleware makes. Deliberately minimal —
/// exactly the operations G5 needs — so it can be faked in G5's decision-table unit tests (an
/// `Arc<dyn Iam>`), while G7 injects the real [`IamClient`]. `#[async_trait]` (rather than a
/// native async-fn-in-trait) because the trait is consumed as a trait object (`dyn Iam`), which
/// AFIT does not yet support object-safely.
#[async_trait]
pub trait Iam: Send + Sync {
    /// Introspect a caller-presented API key. **Bearer-EXEMPT**: the token is the request body,
    /// so NO `authorization` metadata is attached (IAM's introspect path does not read it —
    /// attaching a bearer would be a protocol error).
    async fn introspect_api_key(&self, token: &str) -> Result<IntrospectApiKeyResponse, IamError>;

    /// The self-query `IsAuthorized` (D9). The caller's own key rides as the `authorization`
    /// bearer AND `principal_prn` is that same caller's SA PRN — so IAM sees a principal asking
    /// about *itself* and applies no cross-principal exposure gate. This method builds the
    /// request faithfully from its arguments; the invariant that `principal_prn` really is the
    /// introspected caller (never an attacker-chosen principal) is enforced by G5's call site,
    /// which sources both `caller_key` and `principal_prn` from the same inbound request.
    async fn is_authorized_self(&self, caller_key: &str, principal_prn: &str, action: &str, resource_prn: &str) -> Result<bool, IamError>;
}

/// The real IAM adapter: one lazily-connected `tonic` [`Channel`] shared by both generated
/// service clients (cloning a `Channel` — and hence a generated client — is cheap; it shares the
/// underlying connection pool).
#[derive(Clone, Debug)]
pub struct IamClient {
    authn: AuthnServiceClient<Channel>,
    authz: AuthorizationServiceClient<Channel>,
}

impl IamClient {
    /// Build the IAM channel from config and return a client over it. Uses `connect_lazy` so IAM
    /// being unreachable at gateway boot does NOT block startup — the first RPC then surfaces
    /// `Status::Unavailable` (→ 503) and G8's `/readyz` probes reachability. TLS (D8) is derived
    /// from `cfg.tls`; see [`build_channel`].
    ///
    /// `async` even though `connect_lazy` performs no I/O: the signature is what G7 awaits, and a
    /// future eager-connect variant would slot in without a call-site change.
    pub async fn connect(cfg: &IamClientConfig) -> Result<Self, IamError> {
        let channel = build_channel(cfg)?;
        Ok(Self {
            authn: AuthnServiceClient::new(channel.clone()),
            authz: AuthorizationServiceClient::new(channel),
        })
    }
}

#[async_trait]
impl Iam for IamClient {
    async fn introspect_api_key(&self, token: &str) -> Result<IntrospectApiKeyResponse, IamError> {
        let resp = self.authn.clone().introspect_api_key(introspect_request(token)).await?;
        Ok(resp.into_inner())
    }

    async fn is_authorized_self(&self, caller_key: &str, principal_prn: &str, action: &str, resource_prn: &str) -> Result<bool, IamError> {
        let req = self_authorize_request(caller_key, principal_prn, action, resource_prn)?;
        let resp = self.authz.clone().is_authorized(req).await?;
        Ok(resp.into_inner().allowed)
    }
}

/// Build the `IntrospectApiKey` request. **No `authorization` metadata** — the introspect call is
/// bearer-exempt (the token is the body). Extracted so its bearer-exemption is unit-testable
/// without a live server.
fn introspect_request(token: &str) -> Request<IntrospectApiKeyRequest> {
    Request::new(IntrospectApiKeyRequest { token: token.to_owned() })
}

/// Build the self-query `IsAuthorized` request: the message carries the caller's own SA PRN as
/// `principal_prn`, and the caller's own key rides in the `authorization: Bearer <key>` metadata
/// (IAM resolves that bearer to the SAME principal — that pairing is exactly what makes it a
/// *self* query with no cross-principal gate). `context` is empty (M0 sends no ABAC attributes).
///
/// A `caller_key` that cannot form a valid metadata value → [`IamError::Connect`] (a plumbing
/// failure, not a live-call `Status`; it should not occur for a key that already passed the
/// inbound bearer parse). Extracted so the D9 wiring — metadata AND body — is unit-testable
/// without a live server.
fn self_authorize_request(caller_key: &str, principal_prn: &str, action: &str, resource_prn: &str) -> Result<Request<IsAuthorizedRequest>, IamError> {
    let mut req = Request::new(IsAuthorizedRequest {
        principal_prn: principal_prn.to_owned(),
        action: action.to_owned(),
        resource_prn: resource_prn.to_owned(),
        context: Default::default(),
    });
    let bearer = MetadataValue::try_from(format!("Bearer {caller_key}")).map_err(|e| IamError::Connect(format!("caller key is not a valid `authorization` metadata value: {e}")))?;
    req.metadata_mut().insert("authorization", bearer);
    Ok(req)
}

/// Build the shared tonic [`Channel`] from config. `connect_lazy` defers the TCP handshake to the
/// first RPC, so an IAM that is unreachable at boot doesn't fail startup. TLS material, however,
/// IS assembled eagerly here (tonic builds the TLS connector when `tls_config` is called), so a
/// malformed cert/key or missing trust store fails fast at boot rather than silently on the first
/// request.
fn build_channel(cfg: &IamClientConfig) -> Result<Channel, IamError> {
    let endpoint = Channel::from_shared(cfg.grpc_addr.clone()).map_err(|e| IamError::Connect(format!("invalid iam.grpc_addr {:?}: {e}", cfg.grpc_addr)))?;

    let endpoint = match &cfg.tls {
        // Explicit loopback opt-out (G3's `validate` already proved the host is loopback): plain
        // h2c, no TLS. The channel scheme still comes from `grpc_addr` (e.g. `http://`).
        IamTlsConfig::LoopbackInsecure => endpoint,
        IamTlsConfig::Tls {
            ca_cert_path,
            client_cert_path,
            client_key_path,
        } => {
            let tls = build_tls_config(ca_cert_path, client_cert_path, client_key_path)?;
            endpoint.tls_config(tls).map_err(|e| IamError::Connect(format!("invalid IAM TLS config: {e}")))?
        }
    };

    Ok(endpoint.connect_lazy())
}

/// Assemble the [`ClientTlsConfig`] (D8) — ring-backed rustls via the crate's `tls-ring` feature.
///
/// Trust anchors: a configured `ca_cert_path` **pins** trust to that CA alone — the typical
/// private-CA / self-signed posture for an internal IAM link carrying raw API keys (narrower, and
/// therefore stronger, than also honouring every public root). With no `ca_cert_path` we fall
/// back to the platform trust store (`with_native_roots`, the crate's `tls-native-roots` feature)
/// for a publicly-issued server cert.
///
/// mTLS: a client [`Identity`] is added only when BOTH the client cert and key are configured;
/// exactly one of the two is a misconfiguration and is rejected as [`IamError::Connect`]
/// (both-or-neither). The pairing is checked BEFORE any file I/O so a mismatched config fails the
/// same way whether or not the referenced files exist.
fn build_tls_config(ca_cert_path: &Option<String>, client_cert_path: &Option<String>, client_key_path: &Option<String>) -> Result<ClientTlsConfig, IamError> {
    // Both-or-neither client cert/key pairing — validated first (fail fast, before file I/O).
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

    Ok(tls)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- request-builder unit tests (no live IAM server; that's G7's mock) -------------------

    #[test]
    fn self_authorize_request_sets_bearer_and_body() {
        // The D9 self-query wiring proof at the unit level: the caller's OWN key becomes the
        // `authorization: Bearer <key>` metadata AND the message body carries exactly the args
        // (principal/action/resource) G5 passes — never a substituted principal.
        let caller_key = "sk-caller-abc123";
        let req = self_authorize_request(caller_key, "prn:paigasus:iam:default:sa/gw-caller", "InvokeModel", "prn:paigasus:iam:default:scope/team-a").expect("valid metadata");

        let bearer = req.metadata().get("authorization").expect("authorization metadata must be present").to_str().expect("ascii metadata");
        assert_eq!(bearer, format!("Bearer {caller_key}"));

        let body = req.get_ref();
        assert_eq!(body.principal_prn, "prn:paigasus:iam:default:sa/gw-caller");
        assert_eq!(body.action, "InvokeModel");
        assert_eq!(body.resource_prn, "prn:paigasus:iam:default:scope/team-a");
        assert!(body.context.is_empty(), "M0 sends no ABAC context attributes");
    }

    #[test]
    fn introspect_request_has_no_authorization_metadata() {
        // Introspect is bearer-EXEMPT — the token is the body, and NO authorization metadata is
        // attached (attaching it would be a protocol error against IAM's introspect path).
        let req = introspect_request("some-opaque-token");
        assert!(req.metadata().get("authorization").is_none(), "introspect must NOT carry authorization metadata (it is bearer-exempt)");
        assert_eq!(req.get_ref().token, "some-opaque-token");
    }

    // ---- channel / TLS-config construction (lazy — no network) -------------------------------

    #[tokio::test]
    async fn connect_succeeds_for_loopback_insecure() {
        let cfg = IamClientConfig {
            grpc_addr: "http://127.0.0.1:9090".to_string(),
            tls: IamTlsConfig::LoopbackInsecure,
        };
        assert!(IamClient::connect(&cfg).await.is_ok(), "loopback-insecure connect is lazy and must not fail without a live server");
    }

    #[tokio::test]
    async fn connect_succeeds_for_default_tls_system_trust() {
        // Default config = TLS with no custom CA and no mTLS identity → the platform trust store
        // (`with_native_roots`). `connect_lazy` defers the handshake, but tonic assembles the TLS
        // connector eagerly, so this also exercises native-root loading.
        let cfg = IamClientConfig::default();
        assert!(matches!(cfg.tls, IamTlsConfig::Tls { .. }));
        assert!(
            IamClient::connect(&cfg).await.is_ok(),
            "default (system-trust) TLS connect is lazy and must not fail without a live server"
        );
    }

    #[tokio::test]
    async fn connect_fails_for_mtls_with_only_client_cert() {
        // The both-or-neither mTLS pairing check: a client cert without its key is a
        // misconfiguration → IamError::Connect (independent of whether the file exists).
        let cfg = IamClientConfig {
            grpc_addr: "https://iam.internal.example.com:9090".to_string(),
            tls: IamTlsConfig::Tls {
                ca_cert_path: None,
                client_cert_path: Some("does-not-need-to-exist-client.pem".to_string()),
                client_key_path: None,
            },
        };
        let err = IamClient::connect(&cfg).await.expect_err("a client cert without a key must be rejected");
        assert!(matches!(err, IamError::Connect(_)), "expected IamError::Connect for the mTLS pairing violation, got {err:?}");
    }

    #[test]
    fn rpc_error_preserves_status_code() {
        // G5 maps by `.code()`, so the adapter must keep the Status intact (via `#[from]`),
        // never flatten it to a string.
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
