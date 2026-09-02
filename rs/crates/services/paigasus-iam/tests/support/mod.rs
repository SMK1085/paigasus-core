// SPDX-License-Identifier: Apache-2.0

//! Shared integration-test support: an ephemeral, migrated Postgres via Docker, an
//! in-process mock OIDC IdP, plus the axum-`oneshot` HTTP test harness (`app`/`send`).
//!
//! `start_migrated_postgres` runs against an ephemeral Postgres in Docker; the Docker-unavailable
//! skip-versus-panic decision lives once, in `tests/support/docker.rs`'s `start_or_skip`
//! (SMA-538), rather than being restated here. Used by every integration test file that needs a
//! real database. `start_raw_postgres` is the same container/policy posture but skips the
//! `Migrator::up(&db, None)` step, for tests that must drive migrations one step at a time
//! (pinning the schema to an exact migration count) instead of always migrating to the tip.
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
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use chrono::Utc;
use jsonwebtoken::jwk::{AlgorithmParameters, CommonParameters, EllipticCurve, EllipticCurveKeyParameters, EllipticCurveKeyType, Jwk, JwkSet, KeyAlgorithm};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use p256::elliptic_curve::Generate;
use p256::elliptic_curve::sec1::ToSec1Point;
use p256::pkcs8::{EncodePrivateKey, LineEnding};
use paigasus_iam::adapters::clock::SystemClock;
use paigasus_iam::adapters::http::{AppState, router};
use paigasus_iam::adapters::id::KernelIdGenerator;
use paigasus_iam::adapters::persistence::Migrator;
use paigasus_iam::application::authenticate_token::Provisioning;
use paigasus_iam::config::{ApiKeyConfig, AuditConfig, AuthnConfig, AuthzConfig, IamConfig, IssuerConfig, JwksCacheBackend, JwksCacheConfig, MetricsConfig, MigrationConfig, OutboxConfig};
use paigasus_iam_core::{
    ApiKey, ApiKeyStatus, Clock, GrantScope, IdGenerator, OrganizationId, Principal, PrincipalId, PrincipalKind, PrincipalStatus, RoleGrant, ServiceAccount, TenancyNodeRef, display_prefix,
};
use paigasus_kernel::Prn;
use sea_orm::{ConnectOptions, ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement};
use sea_orm_migration::MigratorTrait;
use serde_json::Value;
use std::sync::{Arc, RwLock};
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::ContainerAsync;
use testcontainers_modules::testcontainers::ImageExt;
use tokio::task::JoinHandle;
use tower::ServiceExt;
use uuid::Uuid;

/// Standalone container helpers — see `support/docker.rs`. Declared `pub` so the ~59 files that
/// carry `mod support;` reach it as `support::docker::*`; the four Redis-only files that have no
/// `mod support;` include the same file directly via `#[path = "support/docker.rs"]`.
pub mod docker;

/// Starts an ephemeral Postgres container, connects, and runs migrations.
///
/// The skip-versus-panic decision lives once, in `docker::start_or_skip` (SMA-538).
///
/// `#[allow(dead_code)]` for the same reason as the helpers below: `tests/authn_private_ca.rs`
/// (SMA-558) is the first binary carrying `mod support;` that needs NO database at all — it binds
/// at the `OidcAuthenticator` seam so it stays Docker-free — so this is unused there.
#[allow(dead_code)]
pub async fn start_migrated_postgres() -> Option<(ContainerAsync<Postgres>, DatabaseConnection)> {
    let node = docker::start_or_skip(Postgres::default().with_tag("16-alpine"), "start_migrated_postgres").await?;

    let url = connection_url(&node).await;
    let db = connect_when_ready(&url).await;
    Migrator::up(&db, None).await.unwrap();

    Some((node, db))
}

/// The `postgres://` URL of an already-started container from [`start_migrated_postgres`] /
/// [`start_raw_postgres`]. The SINGLE definition of that URL: `start_migrated_postgres` builds its
/// own connection through this too, so a caller that needs the string and a caller that needs the
/// pool can never drift apart.
///
/// SeaORM's `DatabaseConnection` deliberately does not expose the URL it was built from, and the
/// SMA-489 nudge tests need one for components that take a connection string rather than a pool
/// handle — `PgOutboxListener::new(url, ..)` and a bare `sqlx::PgListener::connect(&url)` used as
/// an independent observer of `pg_notify`. Both must reach the SAME database as `db`.
pub async fn connection_url(pg: &ContainerAsync<Postgres>) -> String {
    let port = docker::mapped_port(pg, 5432, "postgres").await;
    format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres")
}

/// How long [`connect_when_ready`] waits for a freshly-started Postgres to accept connections.
/// A LOAD BUDGET, not an expectation — it returns on the first successful connect, which on an
/// idle machine is immediate.
const PG_READY_BUDGET: std::time::Duration = std::time::Duration::from_secs(60);

/// Connects to a just-started Postgres container, retrying until it actually accepts.
///
/// **Why this is not a bare `Database::connect(..).unwrap()`** (which is what it replaced): the
/// `testcontainers` Postgres ready-condition is log-based, and the official image logs
/// "ready to accept connections" *twice* — once at the end of its `initdb`/init-scripts phase,
/// then again after the real startup that follows. A container matched on the first line is
/// therefore reported ready while the server is still about to restart, so an immediate connect
/// races that restart and fails with a connection-refused/reset.
///
/// The race is old and pre-existing, but it is load-sensitive: it fires when many containers
/// start concurrently, and SMA-471 added twelve more Docker-backed tests to this crate, which
/// makes it fire measurably more often (observed here: `api_key_auth` failing 1 run in 3 at
/// `Database::connect`). Retrying is the standard fix and costs nothing on the happy path.
async fn connect_when_ready(opts: impl Into<ConnectOptions>) -> DatabaseConnection {
    let opts = opts.into();
    let deadline = std::time::Instant::now() + PG_READY_BUDGET;
    loop {
        match Database::connect(opts.clone()).await {
            Ok(db) => return db,
            Err(e) => {
                assert!(std::time::Instant::now() < deadline, "postgres did not accept connections within {PG_READY_BUDGET:?}: {e}");
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }
    }
}

/// Starts a raw, ephemeral Postgres container WITHOUT running any migrations — same
/// image/tag/CI-gating posture as [`start_migrated_postgres`], but stops short of
/// `Migrator::up(&db, None)` so the caller can drive `Migrator::up`/`Migrator::down` step by
/// step. Needed whenever a test must pin the schema to an EXACT migration count (e.g. seed
/// pre-existing data into the plain, pre-partition `audit_log` shape before m0008 ever runs —
/// SMA-467 — or pin a migration's `up`/`down` under test so it stays meaningful regardless of
/// how many migrations land on top of it later — SMA-469) rather than always migrating all the
/// way to the tip the way [`start_migrated_postgres`] does.
///
/// Pins the pool to a SINGLE connection so per-session state — notably a `SET TimeZone` a
/// caller issues before migrating — is guaranteed to apply to the same physical connection the
/// subsequent `Migrator::up`/`Migrator::down` runs on (a default multi-connection pool could
/// migrate on a different, unaffected session, making such a test non-deterministic —
/// CodeRabbit SMA-467 round 2).
#[allow(dead_code)]
pub async fn start_raw_postgres() -> Option<(ContainerAsync<Postgres>, DatabaseConnection)> {
    let node = docker::start_or_skip(Postgres::default().with_tag("16-alpine"), "start_raw_postgres").await?;
    // Through `connection_url`, not a second inline `format!` — one definition of the URL, as
    // that helper's doc claims (CodeRabbit SMA-489 round 1).
    let mut opts = ConnectOptions::new(connection_url(&node).await);
    opts.max_connections(1).min_connections(1);
    // Same startup race as `start_migrated_postgres` — see `connect_when_ready`'s doc.
    let db = connect_when_ready(opts).await;
    Some((node, db))
}

/// An in-process mock OIDC IdP: an HTTPS axum server (self-signed runtime cert) serving
/// the discovery document + JWKS, plus the ES256 signing key to mint bearer tokens whose
/// signatures verify against that JWKS. The served JWKS lives behind an `Arc<RwLock<..>>`
/// shared with the server task so [`MockIdp::rotate`] can swap the keypair at runtime
/// (the key-rotation integration test, spec §8). The server task is aborted on drop.
#[allow(dead_code)]
pub struct MockIdp {
    pub issuer: String,
    sign: EncodingKey,
    kid: String,
    /// The serialized JWKS the `/jwks` route serves; `rotate` replaces it in place, and the
    /// server reads it (under the same lock) on every request.
    jwks_body: Arc<RwLock<String>>,
    handle: JoinHandle<()>,
}

#[allow(dead_code)]
impl MockIdp {
    /// Mints a signed ES256 bearer token: `iss` = this IdP, plus the given `sub`/`aud`,
    /// `exp` = now + `exp_offset_secs` (negative for an already-expired token), and an
    /// optional `email` claim (JIT provisioning requires one, spec §6.2). Signs with the
    /// CURRENT keypair, so a token minted after [`rotate`](Self::rotate) carries the new `kid`.
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

    /// Rotates the IdP's signing key: mints a fresh ES256 keypair under a NEW `kid`, swaps
    /// it into the served JWKS, and switches `bearer` to sign with it. A token minted after
    /// this carries the new `kid`, so the validator's cached JWKS (keyed by the old `kid`)
    /// misses and must refetch (§4.3) — the key-rotation integration test. The timestamp
    /// suffix guarantees the new `kid` differs from the old one across repeated rotations.
    pub fn rotate(&mut self) {
        let kid = format!("mock-idp-es256-{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default());
        let (sign, jwk) = es256_keypair(&kid);
        let body = serde_json::to_string(&JwkSet { keys: vec![jwk] }).expect("jwks serializes");
        *self.jwks_body.write().expect("jwks lock not poisoned") = body;
        self.sign = sign;
        self.kid = kid;
    }
}

impl Drop for MockIdp {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

/// Mints a runtime EC P-256 keypair under the given `kid` (test-support copy of the
/// validator's inline helper — spec §8 mock-IdP refinement: no committed PEM/JWK fixtures,
/// no `rsa` crate). Returns the signing key and the corresponding public JWK, both tagged
/// with `kid`. The caller owns `kid` so `rotate` can mint a distinct one per rotation.
fn es256_keypair(kid: &str) -> (EncodingKey, Jwk) {
    let secret_key = p256::SecretKey::generate();
    let pem = secret_key.to_pkcs8_pem(LineEnding::LF).expect("valid pkcs8 pem");
    let encoding_key = EncodingKey::from_ec_pem(pem.as_bytes()).expect("valid ec pem");

    let encoded_point = secret_key.public_key().to_sec1_point(false);
    let x = URL_SAFE_NO_PAD.encode(encoded_point.x().expect("uncompressed point has x"));
    let y = URL_SAFE_NO_PAD.encode(encoded_point.y().expect("uncompressed point has y"));

    let jwk = Jwk {
        common: CommonParameters {
            key_algorithm: Some(KeyAlgorithm::ES256),
            key_id: Some(kid.to_string()),
            ..Default::default()
        },
        algorithm: AlgorithmParameters::EllipticCurve(EllipticCurveKeyParameters {
            key_type: EllipticCurveKeyType::EC,
            curve: EllipticCurve::P256,
            x,
            y,
        }),
    };

    (encoding_key, jwk)
}

/// Starts the mock IdP on an ephemeral 127.0.0.1 port, HTTPS via a runtime self-signed
/// certificate (`rcgen`), serving `/.well-known/openid-configuration` + `/jwks`.
#[allow(dead_code)]
pub async fn start_mock_idp() -> MockIdp {
    let kid = "mock-idp-es256-initial".to_string();
    let (sign, jwk) = es256_keypair(&kid);

    let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string(), "127.0.0.1".to_string()]).expect("self-signed cert");
    let tls = axum_server::tls_rustls::RustlsConfig::from_pem(cert.cert.pem().into_bytes(), cert.key_pair.serialize_pem().into_bytes())
        .await
        .expect("rustls config from generated pem");

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("ephemeral port");
    listener.set_nonblocking(true).expect("nonblocking listener");
    let addr = listener.local_addr().unwrap();
    let issuer = format!("https://{addr}");

    // The discovery doc is static (the issuer never changes); the JWKS body lives behind a
    // shared lock so `rotate` can swap it while the server keeps serving from the same Arc.
    let discovery_body = serde_json::json!({ "issuer": issuer, "jwks_uri": format!("{issuer}/jwks") }).to_string();
    let jwks_body = Arc::new(RwLock::new(serde_json::to_string(&JwkSet { keys: vec![jwk] }).expect("jwks serializes")));

    let jwks_for_route = jwks_body.clone();
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
                let shared = jwks_for_route.clone();
                // Snapshot the current JWKS under the read lock, then drop it before
                // returning — the guard is never held across an `.await`.
                let body = shared.read().expect("jwks lock not poisoned").clone();
                async move { ([("content-type", "application/json")], body) }
            }),
        );

    let handle = tokio::spawn(async move {
        axum_server::from_tcp_rustls(listener, tls)
            .expect("server from tcp listener")
            .serve(idp_routes.into_make_service())
            .await
            .expect("mock idp server");
    });

    MockIdp { issuer, sign, kid, jwks_body, handle }
}

/// Like [`start_mock_idp`], but served with a leaf certificate signed by a freshly minted
/// PRIVATE CA, and returns that CA's certificate PEM alongside the IdP (SMA-558 AC3).
///
/// The server is configured with the **leaf alone**, not the chain. That is correct TLS practice
/// for a root the client is expected to hold, and it is what makes the test strict: the client
/// cannot learn the CA from the handshake, so it can only succeed if `extra_ca_bundle_path`
/// genuinely loaded.
///
/// Three details are load-bearing, because `CertificateParams::default()` leaves the
/// distinguished name EMPTY and the pre-existing `start_mock_idp` fixture has never been
/// exercised against real verification (every one of its call sites runs with
/// `accept_invalid_tls: true`):
///   - the CA and the leaf get DISTINCT, non-empty CNs — otherwise both carry an empty subject
///     DN and path building has nothing to match on;
///   - the leaf carries both `localhost` and `127.0.0.1` SANs, since the server binds an
///     ephemeral `127.0.0.1` port and the issuer URL is `https://127.0.0.1:<port>`;
///   - `CertificateParams::signed_by` CONSUMES `self`, so the params must be built and passed by
///     value.
#[allow(dead_code)]
pub async fn start_mock_idp_private_ca() -> (MockIdp, String) {
    use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, KeyPair};

    // --- the private CA ---
    // `DistinguishedName::push` takes `impl Into<DnValue>` and rcgen has a blanket
    // `impl<T: Into<String>> From<T> for DnValue`, so a bare &str is enough (it becomes a
    // Utf8String). No PrintableString conversion needed.
    let mut ca_params = CertificateParams::new(Vec::new()).expect("ca params");
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params.distinguished_name.push(DnType::CommonName, "paigasus-test-private-ca");
    let ca_key = KeyPair::generate().expect("ca keypair");
    let ca_cert = ca_params.self_signed(&ca_key).expect("self-signed ca");
    let ca_pem = ca_cert.pem();

    // --- the leaf, signed BY the CA ---
    let mut leaf_params = CertificateParams::new(vec!["localhost".to_string(), "127.0.0.1".to_string()]).expect("leaf params");
    leaf_params.distinguished_name.push(DnType::CommonName, "paigasus-mock-idp");
    let leaf_key = KeyPair::generate().expect("leaf keypair");
    let leaf_cert = leaf_params.signed_by(&leaf_key, &ca_cert, &ca_key).expect("ca-signed leaf");

    let kid = "mock-idp-es256-initial".to_string();
    let (sign, jwk) = es256_keypair(&kid);

    let tls = axum_server::tls_rustls::RustlsConfig::from_pem(leaf_cert.pem().into_bytes(), leaf_key.serialize_pem().into_bytes())
        .await
        .expect("rustls config from generated pem");

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("ephemeral port");
    listener.set_nonblocking(true).expect("nonblocking listener");
    let addr = listener.local_addr().unwrap();
    let issuer = format!("https://{addr}");

    let discovery_body = serde_json::json!({ "issuer": issuer, "jwks_uri": format!("{issuer}/jwks") }).to_string();
    let jwks_body = Arc::new(RwLock::new(serde_json::to_string(&JwkSet { keys: vec![jwk] }).expect("jwks serializes")));

    let jwks_for_route = jwks_body.clone();
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
                let shared = jwks_for_route.clone();
                let body = shared.read().expect("jwks lock not poisoned").clone();
                async move { ([("content-type", "application/json")], body) }
            }),
        );

    let handle = tokio::spawn(async move {
        axum_server::from_tcp_rustls(listener, tls)
            .expect("server from tcp listener")
            .serve(idp_routes.into_make_service())
            .await
            .expect("mock idp server");
    });

    (MockIdp { issuer, sign, kid, jwks_body, handle }, ca_pem)
}

/// Like [`start_mock_idp`], but also returns the leaf's own certificate PEM (SMA-558 Finding 1
/// regression fixture).
///
/// This exists to keep a FALSE claim from silently becoming true again. Four sites (this
/// crate's `config.rs` doc comment, `CLAUDE.md`, `RUNBOOK-containers.md`, and
/// `iam.toml.example`) used to say a self-signed *leaf* certificate placed in
/// `extra_ca_bundle_path` would NOT validate, and that `accept_invalid_tls` was still needed.
/// That was wrong: rustls applies no `cA` basic-constraints check to a trust anchor
/// (`anchor_from_trusted_cert`), and `verify_cert.rs` only rejects `CaUsedAsEndEntity` when the
/// LEAF BEING VERIFIED asserts CA:TRUE — which `generate_simple_self_signed`'s output never
/// does. A self-signed leaf's own PEM in the bundle verifies fine. Regressing this silently
/// would push an operator with a self-signed IdP toward `accept_invalid_tls`, which disables
/// certificate verification entirely — the exact bypass SMA-558 exists to make unnecessary. See
/// `tests/authn_private_ca.rs`'s `self_signed_leaf_in_the_bundle_also_validates`.
#[allow(dead_code)]
pub async fn start_mock_idp_self_signed() -> (MockIdp, String) {
    let kid = "mock-idp-es256-initial".to_string();
    let (sign, jwk) = es256_keypair(&kid);

    let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string(), "127.0.0.1".to_string()]).expect("self-signed cert");
    let leaf_pem = cert.cert.pem();
    let tls = axum_server::tls_rustls::RustlsConfig::from_pem(cert.cert.pem().into_bytes(), cert.key_pair.serialize_pem().into_bytes())
        .await
        .expect("rustls config from generated pem");

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("ephemeral port");
    listener.set_nonblocking(true).expect("nonblocking listener");
    let addr = listener.local_addr().unwrap();
    let issuer = format!("https://{addr}");

    let discovery_body = serde_json::json!({ "issuer": issuer, "jwks_uri": format!("{issuer}/jwks") }).to_string();
    let jwks_body = Arc::new(RwLock::new(serde_json::to_string(&JwkSet { keys: vec![jwk] }).expect("jwks serializes")));

    let jwks_for_route = jwks_body.clone();
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
                let shared = jwks_for_route.clone();
                let body = shared.read().expect("jwks lock not poisoned").clone();
                async move { ([("content-type", "application/json")], body) }
            }),
        );

    let handle = tokio::spawn(async move {
        axum_server::from_tcp_rustls(listener, tls)
            .expect("server from tcp listener")
            .serve(idp_routes.into_make_service())
            .await
            .expect("mock idp server");
    });

    (MockIdp { issuer, sign, kid, jwks_body, handle }, leaf_pem)
}

/// An `IamConfig` wired to the mock IdP: test defaults, the mock issuer with audience
/// `"paigasus"` + JIT enabled, and `accept_invalid_tls` set (the mock's cert is
/// self-signed — the flag exists exactly for this, and is `false` by default in prod).
#[allow(dead_code)]
pub fn test_config(idp: &MockIdp) -> IamConfig {
    test_config_with(&[(idp, true)], 30)
}

/// A flexible `test_config`: each `(idp, jit_provisioning)` pair becomes a configured
/// issuer (audience `"paigasus"`), and `jwks_refresh_cooldown_secs` is caller-controlled so
/// the key-rotation test can drop it to `0` — otherwise the cooldown from the first JWKS
/// fetch would suppress the kid-miss refetch that a post-swap token needs (spec §4.3). All
/// other fields are the standard test defaults (`accept_invalid_tls` on, memory cache).
#[allow(dead_code)]
pub fn test_config_with(idps: &[(&MockIdp, bool)], jwks_refresh_cooldown_secs: u64) -> IamConfig {
    IamConfig {
        http_addr: "127.0.0.1:0".parse().unwrap(),
        grpc_addr: "127.0.0.1:0".parse().unwrap(),
        database_url: "unused-in-tests".into(),
        log_level: "info".to_string(),
        authn: AuthnConfig {
            leeway_secs: 60,
            http_timeout_secs: 5,
            jwks_ttl_secs: 3600,
            jwks_refresh_cooldown_secs,
            max_token_bytes: 16384,
            accept_invalid_tls: true,
            extra_ca_bundle_path: None,
            jwks_cache: JwksCacheConfig {
                backend: JwksCacheBackend::Memory,
                redis_url: None,
            },
            issuers: idps
                .iter()
                .map(|(idp, jit_provisioning)| IssuerConfig {
                    issuer: idp.issuer.clone(),
                    audiences: vec!["paigasus".to_string()],
                    jit_provisioning: *jit_provisioning,
                })
                .collect(),
        },
        authz: AuthzConfig::default(),
        api_keys: ApiKeyConfig::with_test_pepper(test_api_key_pepper()),
        audit: AuditConfig::default(),
        outbox: OutboxConfig::default(),
        metrics: MetricsConfig::default(),
        migration: MigrationConfig::default(),
    }
}

/// A >=32-byte, base64-encoded API-key pepper for test-support `IamConfig`s (SMA-445 Task 19):
/// `AppState::new` now calls `cfg.api_keys.pepper()` unconditionally, and `ApiKeyConfig::
/// default()`'s pepper is deliberately invalid (empty) — see `ApiKeyConfig::with_test_pepper`'s
/// doc. Re-derives `adapters::api_keys::hasher`'s own test pepper (`[0x5A; 32]`, also mirrored
/// by `config.rs`'s `valid_pepper_b64`) rather than reaching into a sibling module's private
/// test helpers.
#[allow(dead_code)]
fn test_api_key_pepper() -> String {
    STANDARD.encode([0x5Au8; 32])
}

/// Builds the real `router(AppState::new(db, &test_config(&idp)))` for
/// `tower::ServiceExt::oneshot` HTTP tests, plus the mock IdP handle to mint tokens with.
#[allow(dead_code)]
pub async fn app(db: DatabaseConnection) -> (Router, MockIdp) {
    let (router, _state, idp) = app_with_state(db).await;
    (router, idp)
}

/// Like [`app`], but also hands back the `AppState` itself — needed by tests that must reach
/// a raw store directly (e.g. `tests/http_authz.rs` seeding a bootstrap `platform_admin`
/// role-grant through `AppState.role_grant_store`, bypassing `RoleService::grant`'s
/// anti-escalation check — there is necessarily no prior authority to authorize the very
/// first grant against, ahead of SMA-444 Task 21's config-driven bootstrap-admin seeding).
#[allow(dead_code)]
pub async fn app_with_state(db: DatabaseConnection) -> (Router, AppState, MockIdp) {
    let idp = start_mock_idp().await;
    let state = AppState::new(db, &test_config(&idp)).await.expect("AppState::new");
    (router(state.clone()), state, idp)
}

/// Like [`app_with_state`], but the caller supplies the `IamConfig` directly instead of
/// getting `test_config(&idp)` — needed by tests that flip a config toggle away from the
/// ordinary test defaults (e.g. `authz.enforce_tenancy = false`, SMA-444 Task 21). The caller
/// still owns whichever `MockIdp` (if any) it wired into `cfg.authn.issuers`.
#[allow(dead_code)]
pub async fn app_with_config(db: DatabaseConnection, cfg: &IamConfig) -> (Router, AppState) {
    let state = AppState::new(db, cfg).await.expect("AppState::new");
    (router(state.clone()), state)
}

/// Lowest-level request driver: full control over the `Authorization` value, the
/// `content-type`, and the raw body bytes — for tests that need a non-`Bearer {token}`
/// credential shape (scheme casing, fused scheme, foreign scheme) or a deliberately
/// broken/oversized body. Everything else goes through `send_raw`/`send`.
#[allow(dead_code)]
pub async fn send_raw_parts(app: &Router, method: &str, uri: &str, authorization: Option<&str>, content_type: Option<&str>, body: Option<Vec<u8>>) -> Response {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(authorization) = authorization {
        builder = builder.header("authorization", authorization);
    }
    if let Some(content_type) = content_type {
        builder = builder.header("content-type", content_type);
    }
    let request = builder.body(body.map_or_else(Body::empty, Body::from)).unwrap();
    app.clone().oneshot(request).await.unwrap()
}

/// Drives one request through the router and returns the raw response — for tests that
/// assert on headers (e.g. `WWW-Authenticate`). `token` sets `Authorization: Bearer …`.
#[allow(dead_code)]
pub async fn send_raw(app: &Router, method: &str, uri: &str, body: Option<Value>, token: Option<&str>) -> Response {
    let authorization = token.map(|token| format!("Bearer {token}"));
    let (content_type, body) = match body {
        Some(b) => (Some("application/json"), Some(serde_json::to_vec(&b).unwrap())),
        None => (None, None),
    };
    send_raw_parts(app, method, uri, authorization.as_deref(), content_type, body).await
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

/// Drives one request with a RAW body through the router and returns `(status, json body)` —
/// for tests that must send bytes `serde_json::Value` cannot represent (malformed JSON), or a
/// deliberate wrong `Content-Type`. `send` cannot: it serializes a `Value`, which is always
/// valid JSON. An empty response body yields `Value::Null`.
#[allow(dead_code)]
pub async fn send_bytes(app: &Router, method: &str, uri: &str, content_type: Option<&str>, body: &[u8], token: Option<&str>) -> (StatusCode, Value) {
    let authorization = token.map(|token| format!("Bearer {token}"));
    let response = send_raw_parts(app, method, uri, authorization.as_deref(), content_type, Some(body.to_vec())).await;
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let value = if bytes.is_empty() { Value::Null } else { serde_json::from_slice(&bytes).unwrap() };
    (status, value)
}

/// Attaches an `authorization: Bearer <token>` metadata entry to a gRPC request — the gRPC
/// surface's bearer credential (Task 12 enforcement), mirroring the HTTP `Authorization`
/// header the axum middleware reads. Protected `TenancyService` calls carry it; the exempt
/// routes (`Introspect`, health) are called without it.
#[allow(dead_code)]
pub fn grpc_bearer<T>(req: &mut tonic::Request<T>, token: &str) {
    let value: tonic::metadata::MetadataValue<tonic::metadata::Ascii> = format!("Bearer {token}").parse().expect("bearer token is valid ascii metadata");
    req.metadata_mut().insert("authorization", value);
}

// --- SMA-444 Task 20: tenancy-retrofit test-migration helpers -------------------------------
//
// Every M1/M2 tenancy test that drives an ENFORCED HTTP/gRPC route now needs its acting
// principal authorized. `provision` + `seed_platform_admin` (or the `provision_platform_admin`
// combinator) are the shared, one-line-per-test fix: `platform_admin`@`Root` covers every
// `Action` at every resource (the `platform_admin` template is `permit(principal ==
// ?principal, action, resource in ?resource)`, spec §3.2), so seeding it for a test's actor is
// enough to satisfy `ENFORCE_TENANCY` everywhere that actor calls, without picking apart which
// specific action/resource each pre-authorization test happens to exercise.

/// A monotonic, process-local grant-id source: the crate's own `uuid` feature set is
/// intentionally `v4`/`v7`-free everywhere except `serde` (kernel/wasm stay rng-free; see
/// `paigasus-iam`'s `Cargo.toml`), so test-only grant ids can't call `Uuid::new_v4()` /
/// `Uuid::now_v7()` either — this mirrors the crate's own `SeqIds` fake's posture. Each test
/// runs against its OWN freshly migrated Postgres (`start_migrated_postgres` per test), so
/// uniqueness only needs to hold within one test's calls, which a per-binary counter satisfies.
static NEXT_GRANT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

#[allow(dead_code)]
fn next_grant_id() -> Uuid {
    Uuid::from_u128(u128::from(NEXT_GRANT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)))
}

/// Resolves (and JIT-provisions, if unknown) `token`'s principal directly through
/// `state.authn` — bypassing HTTP/gRPC entirely, so provisioning never itself depends on
/// whatever the tenancy authorization gate (SMA-444 Task 20) decides for a first protected
/// call (the pre-Task-20 pattern of triggering provisioning via `GET /v1/organizations` /
/// `ListOrganizations` and asserting/relying on its result no longer holds once that route
/// itself requires a grant). Returns the principal's canonical PRN.
#[allow(dead_code)]
pub async fn provision(state: &AppState, token: &str) -> String {
    let principal = state.authn.resolve(token, Provisioning::Enabled).await.expect("resolve(Enabled) JIT-provisions");
    principal.principal_id.canonical()
}

/// Seeds a `platform_admin`-at-`Root` grant for `principal_prn` directly through
/// `state.role_grant_store` — bypassing `RoleService::grant`'s anti-escalation authorize
/// check (there is necessarily no prior authority to authorize the very first grant against).
/// Sharing this exact store (rather than a freshly constructed one) matters: `PgRoleGrantStore
/// ::grant` bumps `policy_gen` via the `Generations` handle `AppState::new` shares with
/// `CedarAuthorizer`, so the grant is visible to the very next decision (AC1) with no extra
/// wait. Mirrors `tests/http_authz.rs`'s (pre-existing, SMA-444 Task 18) bootstrap-grant
/// pattern, generalized here as the canonical, shared helper the brief asks for.
#[allow(dead_code)]
pub async fn seed_platform_admin(state: &AppState, principal_prn: &str) {
    let principal = PrincipalId::from_prn(Prn::parse(principal_prn).expect("valid principal prn"));
    let grant_id = next_grant_id();
    let grant = RoleGrant {
        id: grant_id,
        principal,
        role_key: "platform_admin".to_string(),
        scope: GrantScope::Root,
        linked_policy_id: format!("grant:{grant_id}"),
        created_at: Utc::now(),
    };
    state.role_grant_store.grant(&grant).await.expect("seed platform_admin grant");
}

/// Seeds an `org_admin` grant for `principal_prn`, scoped to `org_prn` (a canonical
/// organization PRN) — narrower than [`seed_platform_admin`], for a test that wants to prove
/// an org-scoped grant (rather than blanket platform authority) is enough. Mirrors
/// `seed_platform_admin`'s bypass-`RoleService::grant` posture.
#[allow(dead_code)]
pub async fn seed_org_admin(state: &AppState, principal_prn: &str, org_prn: &str) {
    let principal = PrincipalId::from_prn(Prn::parse(principal_prn).expect("valid principal prn"));
    let scope = TenancyNodeRef::from_prn(Prn::parse(org_prn).expect("valid org prn")).expect("org_prn names a tenancy node");
    let grant_id = next_grant_id();
    let grant = RoleGrant {
        id: grant_id,
        principal,
        role_key: "org_admin".to_string(),
        scope: GrantScope::Node(scope),
        linked_policy_id: format!("grant:{grant_id}"),
        created_at: Utc::now(),
    };
    state.role_grant_store.grant(&grant).await.expect("seed org_admin grant");
}

/// Convenience: [`provision`] + [`seed_platform_admin`] in one call — the common case for a
/// migrated tenancy test's one-line setup. Returns the principal's canonical PRN.
#[allow(dead_code)]
pub async fn provision_platform_admin(state: &AppState, token: &str) -> String {
    let principal_prn = provision(state, token).await;
    seed_platform_admin(state, &principal_prn).await;
    principal_prn
}

// --- SMA-444 Task 20b: direct-`PgOrganizationRepository`-driven owner-grant helpers --------
//
// `PgOrganizationRepository::create` now takes a third `owner_grant: &RoleGrant` argument
// (spec D8): the `org_admin` grant seeded for the creating principal, inserted in the SAME
// transaction as the org + default team. Tests that drive `PgOrganizationRepository`
// directly (not through `AppState::new`, so `bootstrap::reconcile_starter` never ran) need
// that grant's two FK targets to exist first: an `org_admin` `role` row (+ its backing
// `policy` template row, `fk_role_template`), and a `principal` row for the owner
// (`fk_role_grant_principal`). These three helpers are the direct-Pg-repo analog of
// `seed_platform_admin`/`seed_org_admin` above (which seed through an `AppState`'s
// `role_grant_store` instead).

/// Seeds the `org_admin`-keyed `role` row (and its backing `policy` template row) if not
/// already present — idempotent, so callers can invoke it once per test or once per org
/// creation without a duplicate-key error.
#[allow(dead_code)]
pub async fn seed_org_admin_role_row(db: &DatabaseConnection) {
    use paigasus_iam::adapters::persistence::entities::{policy, role};
    use sea_orm::{ActiveModelTrait, ActiveValue::NotSet, EntityTrait, Set};

    if role::Entity::find_by_id("org_admin".to_string()).one(db).await.unwrap().is_some() {
        return;
    }
    let now = Utc::now();
    policy::ActiveModel {
        policy_id: Set("org_admin".to_string()),
        kind: Set("template".to_string()),
        source: Set("permit(principal == ?principal, action, resource in ?resource);".to_string()),
        description: Set(None),
        system: Set(false),
        created_at: Set(now),
        updated_at: Set(now),
        content_fingerprint: NotSet,
        starter_revision: NotSet,
    }
    .insert(db)
    .await
    .unwrap();
    role::ActiveModel {
        key: Set("org_admin".to_string()),
        template_id: Set("org_admin".to_string()),
        scope_kinds: Set(r#"["organization"]"#.to_string()),
        description: Set(None),
        system: Set(false),
        created_at: Set(now),
    }
    .insert(db)
    .await
    .unwrap();
}

/// Seeds a bare `principal` row (no `user`/email — this owner only needs to exist for the
/// `RoleGrant`'s `fk_role_grant_principal` target) if not already present — idempotent, so
/// the same owner can be reused across several org creations within one test.
#[allow(dead_code)]
pub async fn seed_bare_principal_row(db: &DatabaseConnection, principal_id: Uuid) {
    use paigasus_iam::adapters::persistence::entities::principal;
    use sea_orm::{ActiveModelTrait, EntityTrait, Set};

    if principal::Entity::find_by_id(principal_id).one(db).await.unwrap().is_some() {
        return;
    }
    let now = Utc::now();
    principal::ActiveModel {
        id: Set(principal_id),
        prn: Set(format!("prn:pgs:iam:::principal/{principal_id}")),
        kind: Set("user".to_string()),
        status: Set("active".to_string()),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(db)
    .await
    .unwrap();
}

/// Seeds both FK prerequisites (the `org_admin` role row, and a bare principal row for
/// `owner`) and returns the `org_admin` owner [`RoleGrant`] for `org` — the one-line-per-call
/// helper direct-`PgOrganizationRepository::create`-driven tests need to build its third
/// argument (SMA-444 Task 20b, spec D8).
#[allow(dead_code)]
pub async fn pg_owner_grant(db: &DatabaseConnection, owner: &PrincipalId, grant_id: Uuid, org: &OrganizationId) -> RoleGrant {
    seed_org_admin_role_row(db).await;
    seed_bare_principal_row(db, owner.uuid()).await;
    RoleGrant {
        id: grant_id,
        principal: owner.clone(),
        role_key: "org_admin".to_string(),
        scope: GrantScope::Node(TenancyNodeRef::Organization(org.clone())),
        linked_policy_id: format!("grant:{grant_id}"),
        created_at: Utc::now(),
    }
}

// --- SMA-445 Task 9: `PgServiceAccountRepository`-driven test helpers ----------------------

/// Seeds a fresh, bare `organization` row via raw SQL (mirrors `authz_bootstrap.rs::seed_org`,
/// duplicated here rather than shared across test binaries — each `tests/*.rs` file compiles
/// its own copy of `mod support;`) and returns a `TenancyNodeRef` naming it — the minimal
/// owner a `service_account` row's `fk_service_account_org` FK needs, without the heavier
/// `PgOrganizationRepository::create` (which also demands a default team + owner grant, D8).
/// The slug is derived from the minted uuid, so repeat calls within one test never collide on
/// `uq_organization_slug`.
#[allow(dead_code)]
pub async fn seed_org_ref(db: &DatabaseConnection) -> TenancyNodeRef {
    let id = KernelIdGenerator.new_organization_id();
    let uuid = id.uuid();
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!(
            r#"INSERT INTO "organization" (id, prn, slug, name, status, created_at, updated_at)
               VALUES ('{uuid}', '{prn}', 'org-{slug}', 'Test Org', 'active', now(), now())"#,
            prn = id.canonical(),
            slug = uuid.simple(),
        ),
        [],
    ))
    .await
    .unwrap();
    TenancyNodeRef::Organization(id)
}

/// Builds a fresh `(Principal, ServiceAccount)` pair for `PgServiceAccountRepository::create`
/// tests — mints via the real `KernelIdGenerator`/`SystemClock` adapters (mirrors
/// `tenancy_orgs.rs::new_org_and_default_team`'s precedent). `owner` must already exist as a
/// real row (its owning `organization`/`team`/`project` FK) — callers seed it first, e.g. via
/// [`seed_org_ref`].
#[allow(dead_code)]
pub fn sample_sa(name: &str, owner: TenancyNodeRef) -> (Principal, ServiceAccount) {
    let ids = KernelIdGenerator;
    let now = SystemClock.now();
    let principal_id = ids.new_service_account_id();
    let principal = Principal::new(principal_id.clone(), PrincipalKind::ServiceAccount, PrincipalStatus::Active, now, now);
    let sa = ServiceAccount::new(principal_id, owner, name, now).expect("valid service account name");
    (principal, sa)
}

// --- SMA-445 Task 10: `PgApiKeyRepository`-driven test helpers ----------------------------

// --- SMA-471 Task 6: `event_outbox`-driven test helpers -----------------------------------
//
// Lifted from `tests/relay_pg.rs::seed_row` (which stays as-is — its callers need a
// caller-controlled `occurred_at` for age-based assertions) for `tests/nats_publisher.rs`'s
// end-to-end relay test, which only needs a fresh unpublished row inserted "now" plus the two
// read-back queries below (`event_outbox` itself has no `pub` port in `paigasus_iam_core`, so a
// test driving the real relay against a real broker has no way to read it back except direct
// SeaORM entity queries).

/// Inserts one fresh, unpublished `event_outbox` row with the given `id` (`occurred_at =
/// Utc::now()`) — bypassing the `Outbox`/`UnitOfWork` ports, mirroring `relay_pg.rs::seed_row`'s
/// field values exactly (same fixed `event_type`/`payload`), just without a caller-controlled
/// `occurred_at`.
#[allow(dead_code)]
pub async fn insert_outbox_row(db: &DatabaseConnection, id: Uuid) {
    use paigasus_iam::adapters::persistence::entities::event_outbox;
    use paigasus_iam_core::EventType;
    use sea_orm::{ActiveModelTrait, Set};

    event_outbox::ActiveModel {
        id: Set(id),
        occurred_at: Set(Utc::now()),
        event_type: Set(EventType::PrincipalCreated.as_wire().to_string()),
        schema_version: Set(1),
        aggregate_prn: Set(format!("prn:pgs:iam:::principal/{id}")),
        actor_prn: Set(None),
        payload: Set(serde_json::json!({"kind": "user"}).to_string()),
        correlation_id: Set(None),
        published_at: Set(None),
        attempts: Set(0),
        parked: Set(false),
        parked_at: Set(None),
        last_error: Set(None),
    }
    .insert(db)
    .await
    .expect("insert event_outbox row");
}

/// Count of `event_outbox` rows with `published_at IS NULL` — the "still needs delivery" set a
/// healthy drain must empty.
#[allow(dead_code)]
pub async fn unpublished_count(db: &DatabaseConnection) -> u64 {
    use paigasus_iam::adapters::persistence::entities::event_outbox;
    use sea_orm::{ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter};

    event_outbox::Entity::find()
        .filter(event_outbox::Column::PublishedAt.is_null())
        .count(db)
        .await
        .expect("count unpublished event_outbox rows")
}

/// The `last_error` column of the `event_outbox` row `id` — `None` if the row has never
/// recorded a failed attempt (or does not exist).
#[allow(dead_code)]
pub async fn last_error(db: &DatabaseConnection, id: Uuid) -> Option<String> {
    use paigasus_iam::adapters::persistence::entities::event_outbox;
    use sea_orm::EntityTrait;

    event_outbox::Entity::find_by_id(id).one(db).await.expect("query event_outbox row").and_then(|row| row.last_error)
}

// --- Prometheus-exposition assertion helper (SMA-489) -------------------------------------

/// Sums the sample values of every `name`-named series in a Prometheus text exposition body
/// (`PrometheusHandle::render()` output), skipping `# HELP`/`# TYPE` comment lines and every other
/// metric family. `f64` so it reads gauges and histogram `_sum`s as faithfully as counters.
///
/// **The `!l.starts_with('#')` filter is the whole point, not a tidy-up.** `PrometheusHandle::
/// render` writes a `# TYPE <name> counter` line for every REGISTERED metric
/// (`metrics-exporter-prometheus`'s `recorder.rs`, `write_type_line`, unconditionally and ahead of
/// any samples), and several call sites register their counters at zero on startup (SMA-489 D12
/// priming). So a `rendered.contains("iam_outbox_listener_reconnects_total")`-style assertion —
/// even one that also excludes lines ending in `" 0"` — is satisfied by the TYPE COMMENT alone,
/// with no sample present at all, and can never fail. Verified: with the `increment(1)` at
/// `pg_outbox_listener.rs:155` removed, the string-matching form of `relay_nudge_pg.rs`'s
/// reconnect assertion still passed. Parse the value, do not grep the name.
///
/// NOTE: `paigasus_observability::init` installs one PROCESS-GLOBAL recorder, so these sums are
/// only meaningful under `cargo nextest`'s process-per-test isolation — the same assumption the
/// `result="ok"`/`result="error"` exclusivity assertions in `relay_pg.rs` already make.
#[allow(dead_code)]
pub fn sum_metric_from(rendered: &str, name: &str) -> f64 {
    rendered
        .lines()
        .filter(|l| !l.starts_with('#'))
        .filter(|l| l.split(['{', ' ']).next() == Some(name))
        .filter_map(|l| l.rsplit(' ').next())
        .filter_map(|v| v.trim().parse::<f64>().ok())
        .sum()
}

/// Builds a fresh `(ApiKey, Vec<u8>)` pair for `PgApiKeyRepository::issue` tests — mints the
/// key id via the real `KernelIdGenerator`/`SystemClock` adapters (mirrors `sample_sa`'s
/// precedent). `scope` must already exist as a real row (its owning `organization`/`team`/
/// `project` FK, `fk_api_key_scope_*`) — callers seed it first, e.g. via [`seed_org_ref`],
/// typically reusing the SAME ref passed to the owning service account's `sample_sa`. The
/// hash stands in for a real `SecretHasher` output (a later task, not yet wired here) —
/// derived deterministically from the freshly minted key id (blake3, already a
/// `paigasus-iam` dependency) so distinct calls never collide by accident; a test that wants
/// to force a collision (`ApiKeyHashCollision`) reuses one call's returned hash bytes
/// directly on a second `issue`, rather than asking this helper to produce one.
#[allow(dead_code)]
pub fn sample_key(sa: &PrincipalId, scope: TenancyNodeRef) -> (ApiKey, Vec<u8>) {
    let ids = KernelIdGenerator;
    let now = SystemClock.now();
    let id = ids.new_api_key_id();
    let hash = blake3::hash(id.uuid().as_bytes()).as_bytes().to_vec();
    let key = ApiKey {
        id,
        service_account_id: sa.clone(),
        scope,
        prefix: display_prefix("pgs_sk_", id),
        status: ApiKeyStatus::Active,
        expires_at: None,
        last_used_at: None,
        created_at: now,
        revoked_at: None,
        scope_actions: Vec::new(),
        scope_roles: Vec::new(),
    };
    (key, hash)
}
