// SPDX-License-Identifier: Apache-2.0

//! SMA-443 AC 1 (Task 13): the config-only, end-to-end acceptance proof against a REAL OIDC
//! IdP. A Keycloak container (HTTPS, self-signed dev cert, `--import-realm`) authenticates a
//! password-grant access token all the way through the wired service — with NO production
//! code changes, only an `IamConfig` pointed at the container's issuer.
//!
//! The realm fixture (`fixtures/keycloak-realm.json`) forces `aud: paigasus` and the `email`
//! claim into the ACCESS token via client protocol mappers (decision D11 — vanilla Keycloak
//! access tokens carry neither). The token is RS256 (Keycloak's realm default), which closes
//! the RS256 accept-path coverage the ES256-only in-process mock IdP could not exercise
//! (spec §8).
//!
//! SMA-690: the test also gets a DPoP-bound token from the same client and asserts that IAM
//! refuses it as SenderConstrained.
//!
//! Docker gating is the single policy owned by `tests/support/docker.rs`'s `start_or_skip`
//! (SMA-538), not restated here — notably, this suite's 240-second Keycloak startup timeout is
//! now a hard failure locally too, not the fast skip it used to be: a container failure against
//! a reachable daemon is never a skip.
//!
//! Keycloak's HTTPS listener uses a runtime self-signed cert, so both the test's own token
//! fetch (`danger_accept_invalid_certs`) and the service's JWKS fetch (`accept_invalid_tls`)
//! trust it — the latter is still config-only. Because Keycloak derives a token's `iss` from
//! the request host in dev mode, the config issuer and the token-endpoint call MUST use the
//! same `127.0.0.1:{mapped}` form so `iss` matches the configured issuer byte-for-byte.
//!
//! The realm fixture also sets `"sslRequired": "none"` deliberately — that only relaxes
//! Keycloak's OWN internal requirement that its clients connect over TLS; this test never
//! reaches the container any other way than through its mapped HTTPS port (JSON has no
//! comments, hence this note living here instead).

mod support;

use axum::http::StatusCode;
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use jsonwebtoken::jwk::Jwk;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use p256::elliptic_curve::Generate;
use p256::elliptic_curve::sec1::ToSec1Point;
use p256::pkcs8::{EncodePrivateKey, LineEnding};
use paigasus_iam::adapters::http::{AppState, router};
use paigasus_iam::adapters::persistence::entities::user;
use paigasus_iam::application::authenticate_token::Provisioning;
use paigasus_iam::config::{ApiKeyConfig, AuditConfig, AuthnConfig, AuthzConfig, IamConfig, IssuerConfig, JwksCacheBackend, JwksCacheConfig, MetricsConfig, MigrationConfig, OutboxConfig};
use paigasus_iam_core::{AuthnError, TokenDefect};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde_json::{Value, json};
use sha2::Digest;
use std::time::Duration;
use support::{provision_platform_admin, send, start_migrated_postgres};
use testcontainers_modules::testcontainers::core::IntoContainerPort;
use testcontainers_modules::testcontainers::{ContainerAsync, GenericImage, ImageExt};
use uuid::Uuid;

const KEYCLOAK_IMAGE: &str = "quay.io/keycloak/keycloak";
/// Pinned to a current stable Keycloak (verified pullable at implementation time). Newer
/// tags use the `KC_BOOTSTRAP_ADMIN_*` bootstrap env vars, set below.
const KEYCLOAK_TAG: &str = "26.4";
const REALM: &str = "paigasus-test";
const HTTPS_PORT: u16 = 8443;
/// Keycloak's dev-mode boot (JVM + Quarkus + realm import) is slow; give the readiness poll a
/// generous budget rather than a fragile stdout-message wait.
const READINESS_ATTEMPTS: u32 = 120;

#[tokio::test]
async fn keycloak_end_to_end_config_only_oidc() {
    // Postgres first (Docker-gated: CI hard-fail, local skip). If Docker is missing locally,
    // this returns `None` and the whole test skips before we ever reach for Keycloak.
    let Some((_pg, db)) = start_migrated_postgres().await else {
        return;
    };

    // A runtime self-signed cert for Keycloak's HTTPS listener — copied into the container,
    // never committed. rcgen emits a PEM cert + a PKCS#8 PEM key, exactly what Keycloak's
    // `KC_HTTPS_CERTIFICATE_*` expect.
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string(), "127.0.0.1".to_string()]).expect("self-signed cert");
    let cert_pem = cert.cert.pem().into_bytes();
    let key_pem = cert.signing_key.serialize_pem().into_bytes();
    let realm_json = include_bytes!("fixtures/keycloak-realm.json").to_vec();

    let image = GenericImage::new(KEYCLOAK_IMAGE, KEYCLOAK_TAG)
        .with_exposed_port(HTTPS_PORT.tcp())
        .with_env_var("KC_BOOTSTRAP_ADMIN_USERNAME", "admin")
        .with_env_var("KC_BOOTSTRAP_ADMIN_PASSWORD", "admin")
        .with_env_var("KC_HTTPS_CERTIFICATE_FILE", "/opt/keycloak/conf/server.crt.pem")
        .with_env_var("KC_HTTPS_CERTIFICATE_KEY_FILE", "/opt/keycloak/conf/server.key.pem")
        .with_copy_to("/opt/keycloak/conf/server.crt.pem", cert_pem)
        .with_copy_to("/opt/keycloak/conf/server.key.pem", key_pem)
        .with_copy_to("/opt/keycloak/data/import/paigasus-test-realm.json", realm_json)
        .with_cmd(["start-dev", "--import-realm", "--https-port=8443"])
        .with_startup_timeout(Duration::from_secs(240));

    let Some(keycloak) = support::docker::start_or_skip(image, "keycloak_e2e").await else {
        return;
    };

    // Keycloak issues `iss` from the request host in dev mode, so the config issuer and every
    // call below share this exact `127.0.0.1:{mapped}` form.
    let https_port = support::docker::mapped_port(&keycloak, HTTPS_PORT, "keycloak https").await;
    let issuer = format!("https://127.0.0.1:{https_port}/realms/{REALM}");

    // One reqwest client for the test's own IdP calls, trusting the self-signed cert (the test
    // playing the role of the CLI — distinct from the service's JWKS fetcher).
    let http = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .timeout(Duration::from_secs(10))
        .build()
        .expect("reqwest client");

    // Poll discovery until the realm is live (boot + import take ~30–60s). A 404 here would
    // mean the realm import failed, so a never-ready discovery dumps the container logs.
    let discovery_url = format!("{issuer}/.well-known/openid-configuration");
    let mut ready = false;
    for _ in 0..READINESS_ATTEMPTS {
        if let Ok(response) = http.get(&discovery_url).send().await
            && response.status().is_success()
        {
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    if !ready {
        panic!("keycloak discovery never became ready at {discovery_url}\n{}", dump_logs(&keycloak).await);
    }

    // Real password-grant token from Keycloak (direct access grant, public client).
    let token_url = format!("{issuer}/protocol/openid-connect/token");
    let token_response = http
        .post(&token_url)
        .form(&[
            ("grant_type", "password"),
            ("client_id", "paigasus-cli"),
            ("username", "alice"),
            ("password", "alice-password"),
            ("scope", "openid"),
        ])
        .send()
        .await
        .expect("token request");
    let token_status = token_response.status();
    let token_body: Value = token_response.json().await.expect("token response json");
    assert!(token_status.is_success(), "password grant failed ({token_status}): {token_body}\n{}", dump_logs(&keycloak).await);
    let access_token = token_body["access_token"].as_str().expect("access_token in token response").to_string();

    // SMA-686: `scope=openid` also returns an ID token. Pin the Keycloak markers this change
    // relies on, so a Keycloak upgrade that drops them reds here rather than silently
    // re-opening the gap (spec § 5.2).
    let id_token = token_body["id_token"].as_str().expect("id_token in token response (scope=openid)").to_string();
    let id_claims = jwt_payload(&id_token);
    assert_eq!(id_claims["typ"], "ID", "keycloak ID token must carry typ=ID: {id_claims}");
    assert!(aud_contains(&id_claims, "paigasus-cli"), "keycloak ID token aud must contain the client id: {id_claims}");
    let access_claims = jwt_payload(&access_token);
    assert_eq!(access_claims["typ"], "Bearer", "keycloak access token must carry typ=Bearer: {access_claims}");
    assert!(aud_contains(&access_claims, "paigasus"), "keycloak access token aud must contain paigasus: {access_claims}");
    // The access-token assertions below must prove the `paigasus` audience path (SMA-678
    // `oidc.audience`), not ride on the client id that the ID-token check needs in the config.
    assert!(
        !aud_contains(&access_claims, "paigasus-cli"),
        "keycloak access token aud must NOT contain the client id: {access_claims}"
    );

    // The access token is RS256 — closes the RS256 end-to-end accept-path coverage (the mock
    // IdP is ES256-only, spec §8).
    let header_segment = access_token.split('.').next().expect("jwt has a header segment");
    let header_bytes = URL_SAFE_NO_PAD.decode(header_segment).expect("base64url-decodable header");
    let header: Value = serde_json::from_slice(&header_bytes).expect("json header");
    assert_eq!(header["alg"], "RS256", "keycloak access token must be RS256");

    // SMA-690 AC 2: Keycloak's plain Bearer access token has no `cnf`.
    assert!(access_claims.get("cnf").is_none(), "keycloak Bearer access token must carry no cnf: {access_claims}");

    // SMA-690 AC 1: a DPoP-bound token from the SAME client. Keycloak binds a token when the
    // client sends a DPoP proof, with no client setting (measurement M2).
    let (dpop_key, dpop_x, dpop_y) = dpop_keypair();
    let dpop_response = http
        .post(&token_url)
        .header("DPoP", dpop_proof(&dpop_key, &dpop_x, &dpop_y, "POST", &token_url))
        .form(&[
            ("grant_type", "password"),
            ("client_id", "paigasus-cli"),
            ("username", "alice"),
            ("password", "alice-password"),
            ("scope", "openid"),
        ])
        .send()
        .await
        .expect("DPoP token request");
    let dpop_status = dpop_response.status();
    let dpop_body: Value = dpop_response.json().await.expect("DPoP token response json");
    // A refused grant has an OAuth error body with no token, so this message prints the body.
    assert!(dpop_status.is_success(), "DPoP password grant failed ({dpop_status}): {dpop_body}\n{}", dump_logs(&keycloak).await);
    // From here the body holds a live token. The messages print only the field under test.
    assert_eq!(dpop_body["token_type"], "DPoP", "keycloak must answer a DPoP proof with token_type DPoP");
    let dpop_token = dpop_body["access_token"].as_str().expect("access_token in DPoP token response").to_string();
    let dpop_claims = jwt_payload(&dpop_token);
    assert_eq!(dpop_claims["typ"], "DPoP", "keycloak DPoP-bound access token must carry typ=DPoP");
    assert_eq!(dpop_claims["cnf"]["jkt"], jwk_thumbprint(&dpop_x, &dpop_y), "cnf.jkt must be the RFC 7638 thumbprint of the proof key");

    // Config-only: point the wired service at the container's issuer. `accept_invalid_tls` is
    // the sole concession to the self-signed dev cert — it is still a plain config flag.
    let cfg = keycloak_config(&issuer);
    let state = AppState::new(db, &cfg).await.expect("AppState::new");
    let app = router(state.clone());

    // SMA-686 AC1: the configured audiences include the client id (`paigasus-cli`, see
    // `keycloak_config`), like the chart default, which accepts the client id; this test
    // accepts two so that both paths run — so the ID token passes the issuer, signature,
    // audience and expiry checks and reaches the `typ` check. Assert the DEFECT
    // through the use case: every defect renders the same 401, so a 401 alone cannot prove
    // which check refused the token.
    let err = state.authn.resolve(&id_token, Provisioning::Enabled).await.expect_err("an ID token must not authenticate");
    assert!(
        matches!(err, AuthnError::InvalidToken(TokenDefect::NotAnAccessToken)),
        "ID token must be refused as NotAnAccessToken, got {err:?}"
    );

    // The wire contract on both HTTP paths: the bearer middleware and the exempt introspect.
    let (status, body) = send(&app, "POST", "/v1/organizations", Some(json!({ "slug": "idtok", "name": "ID token" })), Some(&id_token)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(body["error"]["code"], "invalid-token", "{body}");
    let (status, body) = send(&app, "POST", "/v1/authn/introspect", Some(json!({ "token": id_token })), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(body["error"]["code"], "invalid-token", "{body}");

    // SMA-690 AC 1: the bound token is refused as SenderConstrained (the defect proves the cause;
    // every defect renders the same 401).
    let err = state.authn.resolve(&dpop_token, Provisioning::Enabled).await.expect_err("a DPoP-bound token must not authenticate");
    assert!(
        matches!(err, AuthnError::InvalidToken(TokenDefect::SenderConstrained)),
        "DPoP-bound token must be refused as SenderConstrained, got {err:?}"
    );
    let (status, body) = send(&app, "POST", "/v1/organizations", Some(json!({ "slug": "dpop", "name": "DPoP token" })), Some(&dpop_token)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(body["error"]["code"], "invalid-token", "{body}");
    let (status, body) = send(&app, "POST", "/v1/authn/introspect", Some(json!({ "token": dpop_token })), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(body["error"]["code"], "invalid-token", "{body}");

    // A protected write with the real Keycloak bearer: the middleware's `resolve(.., Enabled)`
    // JIT-provisions the principal (which REQUIRES the `email` claim, spec §6.2), so a 201 here
    // is itself proof the email mapper landed its claim in the ACCESS token (the audience is
    // asserted on the decoded payload above).
    // SMA-444 Task 20: `POST /v1/organizations` is now also authorization-enforced — seed a
    // `platform_admin` grant up front (`provision_platform_admin`'s `state.authn.resolve` runs
    // the exact same JIT path against the REAL Keycloak issuer the middleware itself would).
    provision_platform_admin(&state, &access_token).await;
    let (status, created) = send(&app, "POST", "/v1/organizations", Some(json!({ "slug": "acme", "name": "Acme" })), Some(&access_token)).await;
    assert_eq!(status, StatusCode::CREATED, "JIT-authenticated org create must succeed: {created}");

    // Introspect the same token (bearer-free — the endpoint is middleware-exempt): the full
    // principal context resolves, with the issuer + subject read off the verified token.
    let (status, first) = send(&app, "POST", "/v1/authn/introspect", Some(json!({ "token": access_token })), None).await;
    assert_eq!(status, StatusCode::OK, "{first}");
    let principal_prn = first["principal_prn"].as_str().expect("principal_prn").to_string();
    assert_eq!(first["status"], "active");
    assert_eq!(first["issuer"], issuer, "introspect issuer must equal the configured Keycloak issuer");
    let subject = first["subject"].as_str().expect("subject").to_string();
    assert!(!subject.is_empty(), "keycloak sub must be present on the token: {first}");

    // The email-derived user was actually persisted: exactly one `user` row for alice's email,
    // whose principal_id is the very principal introspect resolved.
    let principal_uuid = principal_prn.rsplit('/').next().and_then(|s| Uuid::parse_str(s).ok()).expect("principal uuid parsed from prn");
    let user_row = user::Entity::find()
        .filter(user::Column::Email.eq("alice@example.com"))
        .one(&state.db)
        .await
        .expect("user query")
        .expect("email-derived user must exist after JIT provisioning");
    assert_eq!(user_row.principal_id, principal_uuid, "the provisioned user must be the principal introspect resolved");

    // A second introspect yields the SAME principal_prn — the (issuer, subject) → principal
    // mapping is stable, not re-minted per call.
    let (status, second) = send(&app, "POST", "/v1/authn/introspect", Some(json!({ "token": access_token })), None).await;
    assert_eq!(status, StatusCode::OK, "{second}");
    assert_eq!(second["principal_prn"], principal_prn, "principal_prn must be stable across introspect calls");
    assert_eq!(second["issuer"], issuer);
    assert_eq!(second["subject"], subject);
}

/// An `IamConfig` pointed at the running Keycloak: a single issuer (audiences `paigasus` and
/// the client id `paigasus-cli`, JIT on) with `accept_invalid_tls` for the self-signed dev
/// cert. Standard test defaults otherwise — this is the ENTIRE production-facing surface
/// exercised by AC 1 (config only).
fn keycloak_config(issuer: &str) -> IamConfig {
    IamConfig {
        http_addr: "127.0.0.1:0".parse().unwrap(),
        grpc_addr: "127.0.0.1:0".parse().unwrap(),
        database_url: "unused-in-tests".into(),
        log_level: "info".to_string(),
        authn: AuthnConfig {
            leeway_secs: 60,
            http_timeout_secs: 10,
            jwks_ttl_secs: 3600,
            jwks_refresh_cooldown_secs: 30,
            max_token_bytes: 16384,
            accept_invalid_tls: true,
            extra_ca_bundle_path: None,
            jwks_cache: JwksCacheConfig {
                backend: JwksCacheBackend::Memory,
                redis_url: None,
            },
            issuers: vec![IssuerConfig {
                issuer: issuer.to_string(),
                // `paigasus` is the access token's aud (audience mapper); `paigasus-cli` is the
                // client id, which the chart accepts by default — and which is the ID token's
                // aud. Both are accepted so the ID token reaches the SMA-686 `typ` check.
                audiences: vec!["paigasus".to_string(), "paigasus-cli".to_string()],
                jit_provisioning: true,
            }],
        },
        authz: AuthzConfig::default(),
        // SMA-445 Task 19: `AppState::new` now calls `cfg.api_keys.pepper()` unconditionally —
        // `ApiKeyConfig::default()`'s pepper is deliberately invalid (empty), so this test's
        // `AppState::new(db, &keycloak_config(..))` needs a real one. Mirrors
        // `support::test_api_key_pepper`'s identical fixed test pepper.
        api_keys: ApiKeyConfig::with_test_pepper(STANDARD.encode([0x5Au8; 32])),
        audit: AuditConfig::default(),
        outbox: OutboxConfig::default(),
        metrics: MetricsConfig::default(),
        migration: MigrationConfig::default(),
    }
}

/// `aud` per RFC 7519 §4.1.3 is a string or an array of strings.
fn aud_contains(claims: &Value, audience: &str) -> bool {
    match &claims["aud"] {
        Value::String(aud) => aud == audience,
        Value::Array(auds) => auds.iter().any(|aud| aud == audience),
        _ => false,
    }
}

/// Decodes a JWT's payload segment WITHOUT verifying it — test inspection only.
fn jwt_payload(token: &str) -> Value {
    let segment = token.split('.').nth(1).expect("jwt has a payload segment");
    let bytes = URL_SAFE_NO_PAD.decode(segment).expect("base64url-decodable payload");
    serde_json::from_slice(&bytes).expect("json payload")
}

/// An EC P-256 key for a DPoP proof (SMA-690 spec § 5.3): the signing key and the public key's
/// base64url `x` and `y` coordinates.
fn dpop_keypair() -> (EncodingKey, String, String) {
    let secret_key = p256::SecretKey::generate();
    let pem = secret_key.to_pkcs8_pem(LineEnding::LF).expect("valid pkcs8 pem");
    let encoding_key = EncodingKey::from_ec_pem(pem.as_bytes()).expect("valid ec pem");
    let point = secret_key.public_key().to_sec1_point(false);
    let x = URL_SAFE_NO_PAD.encode(point.x().expect("uncompressed point has x"));
    let y = URL_SAFE_NO_PAD.encode(point.y().expect("uncompressed point has y"));
    (encoding_key, x, y)
}

/// A DPoP proof (RFC 9449 § 4.2) for one request: header `typ: dpop+jwt`, `alg: ES256` and the
/// public `jwk`; payload `jti`, `htm`, `htu` and `iat`.
fn dpop_proof(key: &EncodingKey, x: &str, y: &str, htm: &str, htu: &str) -> String {
    let jwk: Jwk = serde_json::from_value(json!({ "kty": "EC", "crv": "P-256", "x": x, "y": y })).expect("public EC jwk");
    let mut header = Header::new(Algorithm::ES256);
    header.typ = Some("dpop+jwt".to_string());
    header.jwk = Some(jwk);
    let jti = URL_SAFE_NO_PAD.encode(rand::random::<[u8; 16]>());
    let claims = json!({ "jti": jti, "htm": htm, "htu": htu, "iat": chrono::Utc::now().timestamp() });
    jsonwebtoken::encode(&header, &claims, key).expect("sign the DPoP proof")
}

/// The RFC 7638 SHA-256 thumbprint of a P-256 public key, from the literal canonical JSON.
fn jwk_thumbprint(x: &str, y: &str) -> String {
    let canonical = format!(r#"{{"crv":"P-256","kty":"EC","x":"{x}","y":"{y}"}}"#);
    URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(canonical.as_bytes()))
}

/// Best-effort container stdout+stderr, for the failure panics only (realm-import errors and
/// a token grant that never succeeds surface here).
async fn dump_logs(container: &ContainerAsync<GenericImage>) -> String {
    let stdout = container.stdout_to_vec().await.map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
    let stderr = container.stderr_to_vec().await.map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
    format!("--- keycloak stdout ---\n{stdout}\n--- keycloak stderr ---\n{stderr}")
}
