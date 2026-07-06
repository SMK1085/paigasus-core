// SPDX-License-Identifier: Apache-2.0

//! Shared integration-test support: an ephemeral, migrated Postgres via Docker, an
//! in-process mock OIDC IdP, plus the axum-`oneshot` HTTP test harness (`app`/`send`).
//!
//! `start_migrated_postgres` runs against an ephemeral Postgres in Docker. In CI (`CI` env
//! set) a missing Docker daemon is a HARD FAILURE; on a Docker-less laptop the test skips
//! (returns `None`) with a note. Used by every integration test file that needs a real
//! database.
//!
//! `start_mock_idp` serves an OIDC discovery document + JWKS from an in-process axum
//! server over HTTPS with a runtime self-signed certificate (the JWKS fetcher
//! hard-requires `https`, spec §4.2; `test_config` sets the `accept_invalid_tls`
//! test-only escape so the fetcher trusts it). Keys are runtime ES256 per the spec §8
//! mock-IdP refinement — no committed key fixtures; RS256's accept path is covered by
//! the Keycloak end-to-end test (Task 13).
//!
//! `app`/`send` are shared by the HTTP integration suites (`http_tenancy.rs`,
//! `http_memberships.rs`, `http_authn.rs`); this file is compiled per test binary via
//! `mod support;`, so binaries that only need `start_migrated_postgres` (e.g.
//! `roundtrip.rs`, `tenancy_*.rs`) would otherwise warn on the unused helpers under
//! `clippy --all-targets -D warnings` — hence `#[allow(dead_code)]` on them.

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use axum::response::Response;
use axum::routing::get;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use jsonwebtoken::jwk::{AlgorithmParameters, CommonParameters, EllipticCurve, EllipticCurveKeyParameters, EllipticCurveKeyType, Jwk, JwkSet, KeyAlgorithm};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use p256::elliptic_curve::rand_core::OsRng;
use p256::elliptic_curve::sec1::ToEncodedPoint;
use p256::pkcs8::{EncodePrivateKey, LineEnding};
use paigasus_iam::adapters::http::{AppState, router};
use paigasus_iam::adapters::persistence::Migrator;
use paigasus_iam::config::{AuthnConfig, IamConfig, IssuerConfig, JwksCacheBackend, JwksCacheConfig};
use sea_orm::{Database, DatabaseConnection};
use sea_orm_migration::MigratorTrait;
use serde_json::Value;
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::ContainerAsync;
use testcontainers_modules::testcontainers::ImageExt;
use testcontainers_modules::testcontainers::runners::AsyncRunner;
use tokio::task::JoinHandle;
use tower::ServiceExt;

/// Starts an ephemeral Postgres container, connects, and runs migrations.
///
/// Returns `None` when Docker is unavailable and `CI` is unset (local skip path). Panics
/// when `CI` is set and Docker is unreachable — Docker must be present in CI.
pub async fn start_migrated_postgres() -> Option<(ContainerAsync<Postgres>, DatabaseConnection)> {
    let node = match Postgres::default().with_tag("16-alpine").start().await {
        Ok(n) => n,
        Err(e) => {
            if std::env::var_os("CI").is_some() {
                panic!("Docker is required for the round-trip test in CI: {e}");
            }
            eprintln!("skipping round-trip: Docker unavailable ({e})");
            return None;
        }
    };

    let port = node.get_host_port_ipv4(5432).await.unwrap();
    let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let db = Database::connect(&url).await.unwrap();
    Migrator::up(&db, None).await.unwrap();

    Some((node, db))
}

/// An in-process mock OIDC IdP: an HTTPS axum server (self-signed runtime cert) serving
/// the discovery document + JWKS, plus the ES256 signing key to mint bearer tokens whose
/// signatures verify against that JWKS. The server task is aborted on drop.
#[allow(dead_code)]
pub struct MockIdp {
    pub issuer: String,
    sign: EncodingKey,
    kid: String,
    handle: JoinHandle<()>,
}

#[allow(dead_code)]
impl MockIdp {
    /// Mints a signed ES256 bearer token: `iss` = this IdP, plus the given `sub`/`aud`,
    /// `exp` = now + `exp_offset_secs` (negative for an already-expired token), and an
    /// optional `email` claim (JIT provisioning requires one, spec §6.2).
    pub fn bearer(&self, sub: &str, email: Option<&str>, aud: &str, exp_offset_secs: i64) -> String {
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some(self.kid.clone());
        let mut claims = serde_json::json!({
            "iss": self.issuer,
            "sub": sub,
            "aud": aud,
            "exp": chrono::Utc::now().timestamp() + exp_offset_secs,
        });
        if let Some(email) = email {
            claims["email"] = Value::String(email.to_string());
        }
        jsonwebtoken::encode(&header, &claims, &self.sign).expect("signing a test token")
    }
}

impl Drop for MockIdp {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

/// Mints a runtime EC P-256 keypair (test-support copy of the validator's inline helper —
/// spec §8 mock-IdP refinement: no committed PEM/JWK fixtures, no `rsa` crate). Returns
/// the signing key, the corresponding public JWK, and a fixed `kid` tying the two together.
fn es256_keypair() -> (EncodingKey, Jwk, String) {
    let secret_key = p256::SecretKey::random(&mut OsRng);
    let pem = secret_key.to_pkcs8_pem(LineEnding::LF).expect("valid pkcs8 pem");
    let encoding_key = EncodingKey::from_ec_pem(pem.as_bytes()).expect("valid ec pem");

    let encoded_point = secret_key.public_key().to_encoded_point(false);
    let x = URL_SAFE_NO_PAD.encode(encoded_point.x().expect("uncompressed point has x"));
    let y = URL_SAFE_NO_PAD.encode(encoded_point.y().expect("uncompressed point has y"));

    let kid = "mock-idp-es256".to_string();
    let jwk = Jwk {
        common: CommonParameters {
            key_algorithm: Some(KeyAlgorithm::ES256),
            key_id: Some(kid.clone()),
            ..Default::default()
        },
        algorithm: AlgorithmParameters::EllipticCurve(EllipticCurveKeyParameters {
            key_type: EllipticCurveKeyType::EC,
            curve: EllipticCurve::P256,
            x,
            y,
        }),
    };

    (encoding_key, jwk, kid)
}

/// Starts the mock IdP on an ephemeral 127.0.0.1 port, HTTPS via a runtime self-signed
/// certificate (`rcgen`), serving `/.well-known/openid-configuration` + `/jwks`.
#[allow(dead_code)]
pub async fn start_mock_idp() -> MockIdp {
    let (sign, jwk, kid) = es256_keypair();

    let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string(), "127.0.0.1".to_string()]).expect("self-signed cert");
    let tls = axum_server::tls_rustls::RustlsConfig::from_pem(cert.cert.pem().into_bytes(), cert.key_pair.serialize_pem().into_bytes())
        .await
        .expect("rustls config from generated pem");

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("ephemeral port");
    listener.set_nonblocking(true).expect("nonblocking listener");
    let addr = listener.local_addr().unwrap();
    let issuer = format!("https://{addr}");

    // Both bodies are static per IdP instance — precomputed strings, served verbatim.
    let discovery_body = serde_json::json!({ "issuer": issuer, "jwks_uri": format!("{issuer}/jwks") }).to_string();
    let jwks_body = serde_json::to_string(&JwkSet { keys: vec![jwk] }).expect("jwks serializes");

    let idp_routes = Router::new()
        .route(
            "/.well-known/openid-configuration",
            get(move || {
                let body = discovery_body.clone();
                async move { ([("content-type", "application/json")], body) }
            }),
        )
        .route(
            "/jwks",
            get(move || {
                let body = jwks_body.clone();
                async move { ([("content-type", "application/json")], body) }
            }),
        );

    let handle = tokio::spawn(async move {
        axum_server::from_tcp_rustls(listener, tls).serve(idp_routes.into_make_service()).await.expect("mock idp server");
    });

    MockIdp { issuer, sign, kid, handle }
}

/// An `IamConfig` wired to the mock IdP: test defaults, the mock issuer with audience
/// `"paigasus"` + JIT enabled, and `accept_invalid_tls` set (the mock's cert is
/// self-signed — the flag exists exactly for this, and is `false` by default in prod).
#[allow(dead_code)]
pub fn test_config(idp: &MockIdp) -> IamConfig {
    IamConfig {
        http_addr: "127.0.0.1:0".parse().unwrap(),
        grpc_addr: "127.0.0.1:0".parse().unwrap(),
        database_url: "unused-in-tests".to_string(),
        log_level: "info".to_string(),
        authn: AuthnConfig {
            leeway_secs: 60,
            http_timeout_secs: 5,
            jwks_ttl_secs: 3600,
            jwks_refresh_cooldown_secs: 30,
            max_token_bytes: 16384,
            accept_invalid_tls: true,
            jwks_cache: JwksCacheConfig {
                backend: JwksCacheBackend::Memory,
                redis_url: None,
            },
            issuers: vec![IssuerConfig {
                issuer: idp.issuer.clone(),
                audiences: vec!["paigasus".to_string()],
                jit_provisioning: true,
            }],
        },
    }
}

/// Builds the real `router(AppState::new(db, &test_config(&idp)))` for
/// `tower::ServiceExt::oneshot` HTTP tests, plus the mock IdP handle to mint tokens with.
#[allow(dead_code)]
pub async fn app(db: DatabaseConnection) -> (Router, MockIdp) {
    let idp = start_mock_idp().await;
    let state = AppState::new(db, &test_config(&idp)).await.expect("AppState::new");
    (router(state), idp)
}

/// Drives one request through the router and returns the raw response — for tests that
/// assert on headers (e.g. `WWW-Authenticate`). `token` sets `Authorization: Bearer …`.
#[allow(dead_code)]
pub async fn send_raw(app: &Router, method: &str, uri: &str, body: Option<Value>, token: Option<&str>) -> Response {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    let body = match body {
        Some(b) => {
            builder = builder.header("content-type", "application/json");
            Body::from(serde_json::to_vec(&b).unwrap())
        }
        None => Body::empty(),
    };
    let request = builder.body(body).unwrap();
    app.clone().oneshot(request).await.unwrap()
}

/// Drives one request through the router and returns `(status, json body)`. `Value::Null`
/// stands in for an empty body (e.g. the archive/restore/health endpoints and `DELETE` 204
/// responses under test don't all have one). `token` sets `Authorization: Bearer …`.
#[allow(dead_code)]
pub async fn send(app: &Router, method: &str, uri: &str, body: Option<Value>, token: Option<&str>) -> (StatusCode, Value) {
    let response = send_raw(app, method, uri, body, token).await;
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let value = if bytes.is_empty() { Value::Null } else { serde_json::from_slice(&bytes).unwrap() };
    (status, value)
}
