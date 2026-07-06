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
//! Docker gating mirrors `support::start_migrated_postgres`: a CI hard-fail, a local skip.
//! Keycloak's HTTPS listener uses a runtime self-signed cert, so both the test's own token
//! fetch (`danger_accept_invalid_certs`) and the service's JWKS fetch (`accept_invalid_tls`)
//! trust it — the latter is still config-only. Because Keycloak derives a token's `iss` from
//! the request host in dev mode, the config issuer and the token-endpoint call MUST use the
//! same `127.0.0.1:{mapped}` form so `iss` matches the configured issuer byte-for-byte.

mod support;

use axum::http::StatusCode;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use paigasus_iam::adapters::http::{AppState, router};
use paigasus_iam::adapters::persistence::entities::user;
use paigasus_iam::config::{AuthnConfig, IamConfig, IssuerConfig, JwksCacheBackend, JwksCacheConfig};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde_json::{Value, json};
use std::time::Duration;
use support::{send, start_migrated_postgres};
use testcontainers_modules::testcontainers::core::IntoContainerPort;
use testcontainers_modules::testcontainers::runners::AsyncRunner;
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
    let key_pem = cert.key_pair.serialize_pem().into_bytes();
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

    let keycloak = match image.start().await {
        Ok(container) => container,
        Err(e) => {
            if std::env::var_os("CI").is_some() {
                panic!("Docker/Keycloak is required for the keycloak e2e test in CI: {e}");
            }
            eprintln!("skipping keycloak e2e: container unavailable ({e})");
            return;
        }
    };

    // Keycloak issues `iss` from the request host in dev mode, so the config issuer and every
    // call below share this exact `127.0.0.1:{mapped}` form.
    let https_port = keycloak.get_host_port_ipv4(HTTPS_PORT).await.expect("mapped https port");
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

    // The access token is RS256 — closes the RS256 end-to-end accept-path coverage (the mock
    // IdP is ES256-only, spec §8).
    let header_segment = access_token.split('.').next().expect("jwt has a header segment");
    let header_bytes = URL_SAFE_NO_PAD.decode(header_segment).expect("base64url-decodable header");
    let header: Value = serde_json::from_slice(&header_bytes).expect("json header");
    assert_eq!(header["alg"], "RS256", "keycloak access token must be RS256");

    // Config-only: point the wired service at the container's issuer. `accept_invalid_tls` is
    // the sole concession to the self-signed dev cert — it is still a plain config flag.
    let cfg = keycloak_config(&issuer);
    let state = AppState::new(db, &cfg).await.expect("AppState::new");
    let app = router(state.clone());

    // A protected write with the real Keycloak bearer: the middleware's `resolve(.., Enabled)`
    // JIT-provisions the principal (which REQUIRES the `email` claim, spec §6.2), so a 201 here
    // is itself proof the audience + email mappers landed both claims in the ACCESS token.
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

/// An `IamConfig` pointed at the running Keycloak: a single issuer (audience `paigasus`, JIT
/// on) with `accept_invalid_tls` for the self-signed dev cert. Standard test defaults
/// otherwise — this is the ENTIRE production-facing surface exercised by AC 1 (config only).
fn keycloak_config(issuer: &str) -> IamConfig {
    IamConfig {
        http_addr: "127.0.0.1:0".parse().unwrap(),
        grpc_addr: "127.0.0.1:0".parse().unwrap(),
        database_url: "unused-in-tests".to_string(),
        log_level: "info".to_string(),
        authn: AuthnConfig {
            leeway_secs: 60,
            http_timeout_secs: 10,
            jwks_ttl_secs: 3600,
            jwks_refresh_cooldown_secs: 30,
            max_token_bytes: 16384,
            accept_invalid_tls: true,
            jwks_cache: JwksCacheConfig {
                backend: JwksCacheBackend::Memory,
                redis_url: None,
            },
            issuers: vec![IssuerConfig {
                issuer: issuer.to_string(),
                audiences: vec!["paigasus".to_string()],
                jit_provisioning: true,
            }],
        },
    }
}

/// Best-effort container stdout+stderr, for the failure panics only (realm-import errors and
/// a token grant that never succeeds surface here).
async fn dump_logs(container: &ContainerAsync<GenericImage>) -> String {
    let stdout = container.stdout_to_vec().await.map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
    let stderr = container.stderr_to_vec().await.map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
    format!("--- keycloak stdout ---\n{stdout}\n--- keycloak stderr ---\n{stderr}")
}
