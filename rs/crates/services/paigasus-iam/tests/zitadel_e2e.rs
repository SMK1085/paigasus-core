// SPDX-License-Identifier: Apache-2.0

//! SMA-703: the end-to-end proof of `id_token_marker_claims` against a REAL Zitadel. It closes
//! the follow-up in § 10 of the spec (`2026-10-02-sma-703-zitadel-id-token-marker-claims-design.md`)
//! and the gap that the measurement used a public client only.
//!
//! The test sets up the paigasus console's own client type: a CONFIDENTIAL web app (client
//! secret, HTTP Basic), the code flow with PKCE, a refresh token, and JWT access tokens. A human
//! user logs in through the built-in Login v1 over plain HTTP (no browser): the test sends the
//! forms itself and keeps the cookies by hand. A machine user gets a token with the
//! client-credentials grant. A v1 Action (Complement Token, pre access token creation) adds
//! `email` to the access token, because Zitadel does not put it there (spec F12).
//!
//! What the test pins, for the pinned Zitadel version:
//! - Every ID token (human, refreshed, machine) has `at_hash` and `azp`, and no access token has
//!   either (spec F6). If a Zitadel upgrade changes this, this test fails.
//! - IAM, configured with `["at_hash", "azp"]`, refuses each ID token as `NotAnAccessToken`.
//! - IAM accepts the human access token, the refreshed access token and the machine access token.
//! - A control: with an EMPTY list, IAM accepts the human ID token. So the setting, not another
//!   check, does the refusal.
//!
//! Docker gating is the single policy of `tests/support/docker.rs`'s `start_or_skip` (SMA-538).
//! Three containers start: IAM's own Postgres, a second Postgres for Zitadel, and Zitadel. The
//! two Zitadel containers share a Docker network, so Zitadel reaches its Postgres by name.
//!
//! The issuer. MEASURED (2026-10-03, v4.15.3): Zitadel takes the HOST of `iss` from
//! `ZITADEL_EXTERNALDOMAIN`, and refuses a request whose `Host` header has a different host
//! (404). It takes the PORT of `iss` from the request `Host` header, not from
//! `ZITADEL_EXTERNALPORT`. So the test uses the port that Docker maps, and every call (the test's
//! own calls and IAM's JWKS fetch) uses the same `127.0.0.1:{mapped}` form. The readiness check
//! asserts that the discovery `issuer` is exactly that value.

mod support;

use axum::http::StatusCode;
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use paigasus_iam::adapters::http::{AppState, router};
use paigasus_iam::adapters::persistence::entities::user;
use paigasus_iam::application::authenticate_token::Provisioning;
use paigasus_iam::config::{ApiKeyConfig, AuditConfig, AuthnConfig, AuthzConfig, IamConfig, IssuerConfig, JwksCacheBackend, JwksCacheConfig, MetricsConfig, MigrationConfig, OutboxConfig};
use paigasus_iam_core::{AuthnError, ProvisioningDefect, TokenDefect};
use reqwest::header::{COOKIE, LOCATION, SET_COOKIE};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde_json::{Value, json};
use sha2::Digest;
use std::collections::BTreeMap;
use std::time::Duration;
use support::{provision_platform_admin, send, start_migrated_postgres};
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::core::IntoContainerPort;
use testcontainers_modules::testcontainers::{ContainerAsync, GenericImage, ImageExt};
use uuid::Uuid;

const ZITADEL_IMAGE: &str = "ghcr.io/zitadel/zitadel";
/// Pinned to the version that the SMA-703 measurement used. A new tag must pass this test
/// before the runbook recipe can name it.
const ZITADEL_TAG: &str = "v4.15.3";
/// Zitadel's one listener (HTTP/2, gRPC and HTTP/1.1 on one port, TLS on with `--tlsMode enabled`).
const HTTPS_PORT: u16 = 8080;
/// A directory in the container for the TLS files and the admin PAT. The image has no `/tmp`
/// (MEASURED: `open /tmp/admin.pat: no such file or directory`), and `with_copy_to` makes this
/// one.
const STATE_DIR: &str = "/zitadel-e2e";
/// `start-from-init` creates the schema and the first instance before it serves. MEASURED: about
/// 5 s on an idle Docker Desktop. The budget is for a loaded CI runner.
const READINESS_ATTEMPTS: u32 = 180;
/// The console's redirect URI. The test never follows the redirect: it reads the code from the
/// `Location` header. Login v1 accepts an `http` redirect URI only with `devMode` on the app.
const REDIRECT_URI: &str = "http://localhost:9999/callback";
const USER_EMAIL: &str = "zitadel-e2e@example.com";
const USER_PASSWORD: &str = "E2e-Passw0rd!x";
/// The IAM setting under test (spec D1, the runbook recipe).
const MARKER_CLAIMS: [&str; 2] = ["at_hash", "azp"];
/// The v1 Action of the reference install. It adds `email` to the access token of a human user.
const ADD_EMAIL_CLAIM_SCRIPT: &str = r#"function addEmailClaim(ctx, api) {
  var user = ctx.v1.getUser();
  if (user.human === undefined || !user.human.email) {
    return;
  }
  // human.email is the Go type domain.EmailAddress. goja gives a named Go type
  // to JavaScript as an object, not as a primitive string, so convert it.
  api.v1.claims.setClaim('email', String(user.human.email));
}"#;

#[tokio::test]
async fn zitadel_id_tokens_are_refused_by_the_marker_claims() {
    // IAM's own database first. If Docker is missing locally, the test skips here.
    let Some((_iam_pg, db)) = start_migrated_postgres().await else {
        return;
    };

    // Zitadel's own Postgres, on a network of its own. The suffix keeps parallel runs apart.
    let suffix = format!("{:016x}", rand::random::<u64>());
    let network = format!("zitadel-e2e-{suffix}");
    let pg_host = format!("zitadel-e2e-pg-{suffix}");
    let zitadel_pg_image = Postgres::default().with_tag("16-alpine").with_network(&network).with_container_name(&pg_host);
    let Some(zitadel_pg) = support::docker::start_or_skip(zitadel_pg_image, "zitadel_e2e postgres").await else {
        return;
    };
    // The Postgres module reports ready on its first log line, while the init server (unix
    // socket only) still runs. Zitadel stops at once when its first connection fails, so wait
    // for a TCP connection, which only the real server accepts.
    wait_for_postgres(&zitadel_pg).await;

    // A runtime self-signed cert for Zitadel's TLS listener, copied into the container.
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string(), "127.0.0.1".to_string()]).expect("self-signed cert");
    let cert_pem = cert.cert.pem().into_bytes();
    let key_pem = cert.signing_key.serialize_pem().into_bytes();

    let image = GenericImage::new(ZITADEL_IMAGE, ZITADEL_TAG)
        .with_exposed_port(HTTPS_PORT.tcp())
        .with_network(&network)
        // The image user cannot write to a directory that `with_copy_to` makes (root owns it),
        // and Zitadel must write the admin PAT there.
        .with_user("0")
        .with_env_var("ZITADEL_MASTERKEY", "MasterkeyNeedsToHave32Characters")
        .with_env_var("ZITADEL_DATABASE_POSTGRES_HOST", &pg_host)
        .with_env_var("ZITADEL_DATABASE_POSTGRES_PORT", "5432")
        .with_env_var("ZITADEL_DATABASE_POSTGRES_DATABASE", "zitadel")
        .with_env_var("ZITADEL_DATABASE_POSTGRES_USER_USERNAME", "zitadel")
        .with_env_var("ZITADEL_DATABASE_POSTGRES_USER_PASSWORD", "zitadel")
        .with_env_var("ZITADEL_DATABASE_POSTGRES_USER_SSL_MODE", "disable")
        .with_env_var("ZITADEL_DATABASE_POSTGRES_ADMIN_USERNAME", "postgres")
        .with_env_var("ZITADEL_DATABASE_POSTGRES_ADMIN_PASSWORD", "postgres")
        .with_env_var("ZITADEL_DATABASE_POSTGRES_ADMIN_SSL_MODE", "disable")
        // See the module doc: the host of `iss` comes from here, the port from the request.
        .with_env_var("ZITADEL_EXTERNALDOMAIN", "127.0.0.1")
        .with_env_var("ZITADEL_EXTERNALPORT", HTTPS_PORT.to_string())
        .with_env_var("ZITADEL_EXTERNALSECURE", "true")
        .with_env_var("ZITADEL_TLS_ENABLED", "true")
        .with_env_var("ZITADEL_TLS_CERTPATH", format!("{STATE_DIR}/tls.crt"))
        .with_env_var("ZITADEL_TLS_KEYPATH", format!("{STATE_DIR}/tls.key"))
        .with_env_var("ZITADEL_FIRSTINSTANCE_PATPATH", format!("{STATE_DIR}/admin.pat"))
        .with_env_var("ZITADEL_FIRSTINSTANCE_ORG_MACHINE_MACHINE_USERNAME", "e2e-admin")
        .with_env_var("ZITADEL_FIRSTINSTANCE_ORG_MACHINE_MACHINE_NAME", "e2e-admin")
        .with_env_var("ZITADEL_FIRSTINSTANCE_ORG_MACHINE_PAT_EXPIRATIONDATE", "2099-01-01T00:00:00Z")
        // The measurement used Login v1. This keeps the instance on it.
        .with_env_var("ZITADEL_DEFAULTINSTANCE_FEATURES_LOGINV2_REQUIRED", "false")
        .with_copy_to(format!("{STATE_DIR}/tls.crt"), cert_pem)
        .with_copy_to(format!("{STATE_DIR}/tls.key"), key_pem)
        .with_cmd(["start-from-init", "--masterkeyFromEnv", "--tlsMode", "enabled"])
        .with_startup_timeout(Duration::from_secs(240));

    let Some(zitadel) = support::docker::start_or_skip(image, "zitadel_e2e").await else {
        return;
    };
    let https_port = support::docker::mapped_port(&zitadel, HTTPS_PORT, "zitadel https").await;
    let issuer = format!("https://127.0.0.1:{https_port}");

    // One client for every call of the test. It never follows a redirect, because the login
    // flow must read each `Location` header, and the last one points at a server that does not
    // exist.
    let http = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(20))
        .build()
        .expect("reqwest client");

    // Poll discovery until Zitadel serves, then pin the issuer form (see the module doc).
    let discovery_url = format!("{issuer}/.well-known/openid-configuration");
    let mut discovery = None;
    for _ in 0..READINESS_ATTEMPTS {
        if let Ok(response) = http.get(&discovery_url).send().await
            && response.status().is_success()
            && let Ok(body) = response.json::<Value>().await
        {
            discovery = Some(body);
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    let Some(discovery) = discovery else {
        panic!("zitadel discovery never became ready at {discovery_url}\n{}", dump_logs(&zitadel).await);
    };
    assert_eq!(discovery["issuer"], issuer, "zitadel must derive the issuer from the request Host: {discovery}");

    let pat = read_admin_pat(&zitadel).await;
    let zitadel_api = ZitadelApi {
        http: &http,
        base: &issuer,
        pat: &pat,
    };
    // MEASURED: discovery answers before the management API does. The REST gateway first
    // returns 503 `dial tcp [::1]:8080: connect: connection refused` for a few seconds.
    if !zitadel_api.wait_until_ready().await {
        panic!("the zitadel management API never became ready\n{}", dump_logs(&zitadel).await);
    }
    let setup = setup_zitadel(&zitadel_api).await;

    // The human flow: code + PKCE through Login v1, then the code exchange with HTTP Basic.
    let human = human_login(&http, &issuer, &setup).await;
    let human_access = token_field(&human, "access_token");
    let human_id = token_field(&human, "id_token");
    let human_refresh = token_field(&human, "refresh_token");

    // The refresh grant (spec F9: Zitadel returns a new ID token too).
    let refreshed = token_request(
        &http,
        &issuer,
        Some((&setup.client_id, &setup.client_secret)),
        &[("grant_type", "refresh_token"), ("refresh_token", human_refresh.as_str())],
    )
    .await
    .unwrap_or_else(|(status, body)| panic!("refresh grant failed ({status}): {body}"));
    let refreshed_access = token_field(&refreshed, "access_token");
    let refreshed_id = token_field(&refreshed, "id_token");

    // The machine flow: client credentials with `openid` and the project audience scope.
    let machine = machine_token(&http, &issuer, &setup).await;
    let machine_access = token_field(&machine, "access_token");
    let machine_id = token_field(&machine, "id_token");

    // --- The token shapes (decoded without verification, like keycloak_e2e) ---

    let id_tokens = [("human ID token", &human_id), ("refreshed ID token", &refreshed_id), ("machine ID token", &machine_id)];
    let access_tokens = [
        ("human access token", &human_access),
        ("refreshed access token", &refreshed_access),
        ("machine access token", &machine_access),
    ];
    for (label, token) in id_tokens.iter().chain(access_tokens.iter()) {
        let claims = jwt_payload(token);
        assert_eq!(claims["iss"], issuer, "{label}: iss must be the configured issuer: {claims}");
    }
    // Spec F6: both markers on every ID token, neither on any access token.
    for (label, token) in id_tokens {
        let claims = jwt_payload(token);
        for marker in MARKER_CLAIMS {
            assert!(has_claim(&claims, marker), "{label} must carry {marker} (spec F6): {claims}");
        }
    }
    for (label, token) in access_tokens {
        let claims = jwt_payload(token);
        for marker in MARKER_CLAIMS {
            assert!(!has_claim(&claims, marker), "{label} must NOT carry {marker} (spec F6): {claims}");
        }
        assert!(aud_contains(&claims, &setup.project_id), "{label} aud must contain the project id: {claims}");
    }
    // The Action works: the human access tokens carry the user's email. The machine user is not
    // human, so the Action adds nothing to its token.
    assert_eq!(jwt_payload(&human_access)["email"], USER_EMAIL, "the addEmailClaim Action must add email to the access token");
    assert_eq!(jwt_payload(&refreshed_access)["email"], USER_EMAIL, "the Action must also run on the refresh grant");
    assert!(jwt_payload(&machine_access).get("email").is_none(), "the machine access token must carry no email");

    // --- IAM with the Zitadel recipe ---

    let cfg = zitadel_config(&issuer, &setup.project_id, &MARKER_CLAIMS);
    let state = AppState::new(db, &cfg).await.expect("AppState::new");
    let app = router(state.clone());

    // Every ID token is refused as NotAnAccessToken. Assert the defect through the use case,
    // because every defect renders the same 401.
    for (label, token) in id_tokens {
        let err = state.authn.resolve(token, Provisioning::Enabled).await.expect_err("an ID token must not authenticate");
        assert!(
            matches!(err, AuthnError::InvalidToken(TokenDefect::NotAnAccessToken)),
            "{label} must be refused as NotAnAccessToken, got {err:?}"
        );
        let (status, body) = send(&app, "POST", "/v1/organizations", Some(json!({ "slug": "idtok", "name": "ID token" })), Some(token)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{label}: {body}");
        assert_eq!(body["error"]["code"], "invalid-token", "{label}: {body}");
    }

    // The human access token provisions a user (JIT needs the email that the Action added) and
    // reaches an authorization-enforced write, as in keycloak_e2e.
    provision_platform_admin(&state, &human_access).await;
    let (status, created) = send(&app, "POST", "/v1/organizations", Some(json!({ "slug": "acme", "name": "Acme" })), Some(&human_access)).await;
    assert_eq!(status, StatusCode::CREATED, "JIT-authenticated org create must succeed: {created}");

    let (status, first) = send(&app, "POST", "/v1/authn/introspect", Some(json!({ "token": human_access })), None).await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(first["status"], "active");
    assert_eq!(first["issuer"], issuer, "introspect issuer must equal the configured Zitadel issuer");
    let principal_prn = first["principal_prn"].as_str().expect("principal_prn").to_string();
    let principal_uuid = principal_prn.rsplit('/').next().and_then(|s| Uuid::parse_str(s).ok()).expect("principal uuid parsed from prn");
    let user_row = user::Entity::find()
        .filter(user::Column::Email.eq(USER_EMAIL))
        .one(&state.db)
        .await
        .expect("user query")
        .expect("email-derived user must exist after JIT provisioning");
    assert_eq!(user_row.principal_id, principal_uuid, "the provisioned user must be the principal introspect resolved");

    // The refreshed access token is accepted and resolves to the same principal.
    let (status, again) = send(&app, "POST", "/v1/authn/introspect", Some(json!({ "token": refreshed_access })), None).await;
    assert_eq!(status, StatusCode::OK, "the refreshed access token must be accepted: {again}");
    assert_eq!(again["principal_prn"], principal_prn, "the refreshed access token must resolve to the same principal");

    // The machine access token passes the authenticator: the use case reaches the identity
    // lookup, which runs only after a token is verified. It has no email, so IAM cannot
    // provision a principal for it by JIT (spec § 6.2 of SMA-443). This is the current state for
    // every machine token without email, not a Zitadel defect.
    let err = state
        .authn
        .resolve(&machine_access, Provisioning::Disabled)
        .await
        .expect_err("no principal exists for the machine user");
    assert!(matches!(err, AuthnError::IdentityNotProvisioned), "the machine access token must pass the authenticator, got {err:?}");
    let err = state.authn.resolve(&machine_access, Provisioning::Enabled).await.expect_err("JIT needs an email");
    assert!(
        matches!(err, AuthnError::ProvisioningFailed(ProvisioningDefect::MissingEmail)),
        "the machine access token must fail JIT only for the missing email, got {err:?}"
    );

    // The control: with an EMPTY list (the default), IAM accepts the human ID token. So the
    // setting does the refusal above, not some other check. The human is provisioned already,
    // so the token resolves to the same principal.
    let open_cfg = zitadel_config(&issuer, &setup.project_id, &[]);
    let open_state = AppState::new(state.db.clone(), &open_cfg).await.expect("AppState::new (empty marker list)");
    let principal = open_state
        .authn
        .resolve(&human_id, Provisioning::Disabled)
        .await
        .expect("with an empty marker list, IAM accepts a Zitadel ID token (the gap SMA-703 closes)");
    assert_eq!(principal.principal_id.canonical(), principal_prn, "the accepted ID token must resolve to the human principal");
}

/// The ids and secrets that [`setup_zitadel`] creates.
struct ZitadelSetup {
    project_id: String,
    client_id: String,
    client_secret: String,
    machine_client_id: String,
    machine_client_secret: String,
}

/// The management API of the running Zitadel, called with the first-instance admin PAT.
struct ZitadelApi<'a> {
    http: &'a reqwest::Client,
    base: &'a str,
    pat: &'a str,
}

impl ZitadelApi<'_> {
    /// Polls a read call with the PAT until it succeeds. False when the budget runs out.
    async fn wait_until_ready(&self) -> bool {
        for _ in 0..READINESS_ATTEMPTS {
            if let Ok(response) = self.http.get(format!("{}/management/v1/orgs/me", self.base)).bearer_auth(self.pat).send().await
                && response.status().is_success()
            {
                return true;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        false
    }

    /// One API call. Panics with the response body on any status other than 2xx.
    async fn call(&self, method: reqwest::Method, path: &str, body: Value) -> Value {
        let response = self
            .http
            .request(method.clone(), format!("{}{path}", self.base))
            .bearer_auth(self.pat)
            .json(&body)
            .send()
            .await
            .unwrap_or_else(|e| panic!("zitadel {method} {path}: {e}"));
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        assert!(status.is_success(), "zitadel {method} {path} failed ({status}): {text}");
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("zitadel {method} {path}: response is not JSON ({e}): {text}"))
    }
}

/// Creates, through the management API: project P; the confidential web app A of the paigasus
/// console; a human user with a password and a verified email; a machine user with a client
/// secret and JWT access tokens; and the `addEmailClaim` Action on the Complement Token flow.
async fn setup_zitadel(api: &ZitadelApi<'_>) -> ZitadelSetup {
    use reqwest::Method;

    let project = api.call(Method::POST, "/management/v1/projects", json!({ "name": "paigasus-e2e" })).await;
    let project_id = string_field(&project, "id");

    let app = api
        .call(
            Method::POST,
            &format!("/management/v1/projects/{project_id}/apps/oidc"),
            json!({
                "name": "paigasus-console",
                "redirectUris": [REDIRECT_URI],
                "responseTypes": ["OIDC_RESPONSE_TYPE_CODE"],
                "grantTypes": ["OIDC_GRANT_TYPE_AUTHORIZATION_CODE", "OIDC_GRANT_TYPE_REFRESH_TOKEN"],
                "appType": "OIDC_APP_TYPE_WEB",
                "authMethodType": "OIDC_AUTH_METHOD_TYPE_BASIC",
                "accessTokenType": "OIDC_TOKEN_TYPE_JWT",
                "idTokenUserinfoAssertion": true,
                // Login v1 refuses an `http` redirect URI on a web app without dev mode.
                "devMode": true
            }),
        )
        .await;
    let client_id = string_field(&app, "clientId");
    let client_secret = string_field(&app, "clientSecret");

    api.call(
        Method::POST,
        "/v2/users/human",
        json!({
            "username": USER_EMAIL,
            "profile": { "givenName": "Zitadel", "familyName": "E2E" },
            "email": { "email": USER_EMAIL, "isVerified": true },
            "password": { "password": USER_PASSWORD, "changeRequired": false }
        }),
    )
    .await;

    let machine = api
        .call(
            Method::POST,
            "/management/v1/users/machine",
            json!({ "userName": "paigasus-e2e-svc", "name": "paigasus-e2e-svc", "accessTokenType": "ACCESS_TOKEN_TYPE_JWT" }),
        )
        .await;
    let machine_user_id = string_field(&machine, "userId");
    let secret = api.call(Method::PUT, &format!("/management/v1/users/{machine_user_id}/secret"), json!({})).await;
    let machine_client_id = string_field(&secret, "clientId");
    let machine_client_secret = string_field(&secret, "clientSecret");

    let action = api
        .call(
            Method::POST,
            "/management/v1/actions",
            json!({ "name": "addEmailClaim", "script": ADD_EMAIL_CLAIM_SCRIPT, "timeout": "10s", "allowedToFail": false }),
        )
        .await;
    let action_id = string_field(&action, "id");
    // Flow type 2 is Complement Token. Trigger type 5 is Pre access token creation.
    api.call(Method::POST, "/management/v1/flows/2/trigger/5", json!({ "actionIds": [action_id] })).await;

    ZitadelSetup {
        project_id,
        client_id,
        client_secret,
        machine_client_id,
        machine_client_secret,
    }
}

/// The human flow over plain HTTP: `/oauth/v2/authorize` with PKCE S256, the Login v1 forms
/// (login name, password, skip the MFA setup prompt), the code from the last redirect, and the
/// code exchange with HTTP Basic client auth. Returns the token response.
async fn human_login(http: &reqwest::Client, issuer: &str, setup: &ZitadelSetup) -> Value {
    let verifier = URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>());
    let challenge = URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(verifier.as_bytes()));
    let state = format!("{:016x}", rand::random::<u64>());
    let nonce = format!("{:016x}", rand::random::<u64>());
    let scope = format!("openid profile email offline_access {}", project_aud_scope(&setup.project_id));
    let authorize = url::Url::parse_with_params(
        &format!("{issuer}/oauth/v2/authorize"),
        &[
            ("client_id", setup.client_id.as_str()),
            ("redirect_uri", REDIRECT_URI),
            ("response_type", "code"),
            ("scope", scope.as_str()),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
            ("nonce", nonce.as_str()),
            ("state", state.as_str()),
        ],
    )
    .expect("authorize url");

    let mut browser = Browser { http, cookies: BTreeMap::new() };
    let mut url = authorize;
    let mut form: Option<Vec<(String, String)>> = None;
    let mut callback = None;
    // The measured flow has five hops. The bound stops a loop if Zitadel shows a page twice.
    for _ in 0..12 {
        let response = browser.send(&url, form.take()).await;
        let status = response.status();
        if status.is_redirection() {
            let location = response.headers().get(LOCATION).and_then(|v| v.to_str().ok()).expect("a redirect has a Location").to_string();
            let next = url.join(&location).expect("Location is a valid URL");
            if next.as_str().starts_with(REDIRECT_URI) {
                callback = Some(next);
                break;
            }
            url = next;
            continue;
        }
        let page = response.text().await.unwrap_or_default();
        assert!(status.is_success(), "login v1 returned {status} at {url}: {}", page_title(&page));
        let (action, mut fields) = first_form(&page).unwrap_or_else(|| panic!("login v1 page at {url} has no form: {}", page_title(&page)));
        match action.as_str() {
            "/ui/login/loginname" => fields.push(("loginName".to_string(), USER_EMAIL.to_string())),
            "/ui/login/password" => {
                fields.push(("loginName".to_string(), USER_EMAIL.to_string()));
                fields.push(("password".to_string(), USER_PASSWORD.to_string()));
            }
            // Zitadel asks to set up a second factor after the password. Skip it.
            "/ui/login/mfa/prompt" => fields.push(("skip".to_string(), "true".to_string())),
            other => panic!("unexpected login v1 form {other} at {url}: {}", page_title(&page)),
        }
        url = url.join(&action).expect("form action is a valid URL");
        form = Some(fields);
    }
    let callback = callback.expect("login v1 never redirected to the redirect URI");
    let params: BTreeMap<String, String> = callback.query_pairs().into_owned().collect();
    assert_eq!(params.get("state"), Some(&state), "the callback must return the state: {callback}");
    let code = params.get("code").unwrap_or_else(|| panic!("the callback has no code: {callback}"));

    let tokens = token_request(
        http,
        issuer,
        Some((&setup.client_id, &setup.client_secret)),
        &[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", REDIRECT_URI),
            ("code_verifier", verifier.as_str()),
        ],
    )
    .await
    .unwrap_or_else(|(status, body)| panic!("code exchange failed ({status}): {body}"));
    assert_eq!(jwt_payload(&token_field(&tokens, "id_token"))["nonce"], nonce, "the human ID token must return the nonce");
    tokens
}

/// The client-credentials grant of the machine user. MEASURED: a secret used less than one
/// second after it was made can fail once with `Errors.User.Machine.Secret.NotExisting`, so that
/// one error is retried for a short time.
async fn machine_token(http: &reqwest::Client, issuer: &str, setup: &ZitadelSetup) -> Value {
    let scope = format!("openid {}", project_aud_scope(&setup.project_id));
    let mut last = None;
    for _ in 0..10 {
        let form = [
            ("grant_type", "client_credentials"),
            ("client_id", setup.machine_client_id.as_str()),
            ("client_secret", setup.machine_client_secret.as_str()),
            ("scope", scope.as_str()),
        ];
        match token_request(http, issuer, None, &form).await {
            Ok(tokens) => return tokens,
            Err((status, body)) if body.contains("Errors.User.Machine.Secret.NotExisting") => {
                last = Some((status, body));
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            Err((status, body)) => panic!("client credentials grant failed ({status}): {body}"),
        }
    }
    panic!("client credentials grant kept failing: {last:?}");
}

/// One call to the token endpoint. `basic` is the client id and secret for HTTP Basic client
/// auth. Returns the JSON body, or the status and the body text on an error.
async fn token_request(http: &reqwest::Client, issuer: &str, basic: Option<(&str, &str)>, form: &[(&str, &str)]) -> Result<Value, (StatusCode, String)> {
    let mut request = http.post(format!("{issuer}/oauth/v2/token")).form(form);
    if let Some((client_id, secret)) = basic {
        request = request.basic_auth(client_id, Some(secret));
    }
    let response = request.send().await.expect("token request");
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err((status, text));
    }
    Ok(serde_json::from_str(&text).expect("token response is JSON"))
}

/// A minimal browser for Login v1: it sends one request at a time and keeps the cookies by hand
/// (the workspace `reqwest` has no `cookies` feature, and the flow needs only name and value).
struct Browser<'a> {
    http: &'a reqwest::Client,
    cookies: BTreeMap<String, String>,
}

impl Browser<'_> {
    /// A GET, or a form POST when `form` is set. Stores each `Set-Cookie` of the response.
    async fn send(&mut self, url: &url::Url, form: Option<Vec<(String, String)>>) -> reqwest::Response {
        let mut request = match &form {
            Some(fields) => self.http.post(url.clone()).form(fields),
            None => self.http.get(url.clone()),
        };
        if !self.cookies.is_empty() {
            let header = self.cookies.iter().map(|(name, value)| format!("{name}={value}")).collect::<Vec<_>>().join("; ");
            request = request.header(COOKIE, header);
        }
        let response = request.send().await.unwrap_or_else(|e| panic!("login v1 request to {url}: {e}"));
        for value in response.headers().get_all(SET_COOKIE) {
            let Ok(text) = value.to_str() else { continue };
            let pair = text.split(';').next().unwrap_or_default();
            if let Some((name, value)) = pair.split_once('=') {
                self.cookies.insert(name.trim().to_string(), value.trim().to_string());
            }
        }
        response
    }
}

/// The first `<form>` of a page: its `action` and its hidden inputs (the CSRF token and the
/// auth request id). Enough for the Login v1 pages; this is not a general HTML parser.
fn first_form(page: &str) -> Option<(String, Vec<(String, String)>)> {
    let start = page.find("<form")?;
    let end = start + page[start..].find("</form>")?;
    let form = &page[start..end];
    let open_tag = &form[..form.find('>')?];
    let action = attribute(open_tag, "action")?;
    let mut fields = Vec::new();
    for (index, _) in form.match_indices("<input") {
        let tag = &form[index..index + form[index..].find('>').unwrap_or(form.len() - index)];
        if attribute(tag, "type").as_deref() == Some("hidden")
            && let Some(name) = attribute(tag, "name")
        {
            fields.push((name, attribute(tag, "value").unwrap_or_default()));
        }
    }
    Some((action, fields))
}

/// The value of an attribute in one tag. The name must follow whitespace, so `name` does not
/// match `data-name`. Decodes the HTML character references that Go's `html/template` writes.
fn attribute(tag: &str, name: &str) -> Option<String> {
    let needle = format!("{name}=\"");
    let mut from = 0;
    while let Some(found) = tag[from..].find(&needle) {
        let at = from + found;
        if tag[..at].ends_with(char::is_whitespace) {
            let value_start = at + needle.len();
            let value_end = value_start + tag[value_start..].find('"')?;
            return Some(html_unescape(&tag[value_start..value_end]));
        }
        from = at + needle.len();
    }
    None
}

/// Decodes `&amp;`, `&lt;`, `&gt;`, `&quot;` and numeric references such as `&#43;`.
fn html_unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let tail = &rest[amp..];
        let Some(semi) = tail.find(';') else {
            out.push_str(tail);
            return out;
        };
        let entity = &tail[1..semi];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            _ => entity.strip_prefix('#').and_then(|n| n.parse::<u32>().ok()).and_then(char::from_u32),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &tail[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// The `<title>` of a page, for the failure messages (Login v1 shows its errors as a page).
fn page_title(page: &str) -> String {
    let error = page.find("lgn-error-message").map(|at| page[at..].chars().take(300).collect::<String>());
    let title = page.find("<title>").and_then(|at| page[at + 7..].find("</title>").map(|end| page[at + 7..at + 7 + end].to_string()));
    format!("title={title:?} error={error:?}")
}

/// Zitadel's scope that adds a project id to `aud` (spec F4).
fn project_aud_scope(project_id: &str) -> String {
    format!("urn:zitadel:iam:org:project:id:{project_id}:aud")
}

/// Reads the first-instance admin PAT out of the container. Zitadel writes it during
/// `start-from-init`, before it serves, so it exists once discovery answers; the loop covers a
/// slow file system.
async fn read_admin_pat(zitadel: &ContainerAsync<GenericImage>) -> String {
    for _ in 0..30 {
        if let Ok(bytes) = zitadel.copy_file_from(format!("{STATE_DIR}/admin.pat"), Vec::new()).await {
            let pat = String::from_utf8(bytes).expect("the PAT is UTF-8").trim().to_string();
            if !pat.is_empty() {
                return pat;
            }
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    panic!("zitadel never wrote the admin PAT\n{}", dump_logs(zitadel).await);
}

/// Waits until Zitadel's Postgres accepts a TCP connection from the host (see the call site).
async fn wait_for_postgres(pg: &ContainerAsync<Postgres>) {
    let url = support::connection_url(pg).await;
    for _ in 0..120 {
        if sea_orm::Database::connect(url.as_str()).await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    panic!("zitadel's postgres did not accept connections at {url}");
}

/// An `IamConfig` pointed at the running Zitadel: one issuer whose audience is project P, JIT
/// on, `accept_invalid_tls` for the self-signed cert, and the marker claims under test. Standard
/// test defaults otherwise, as in keycloak_e2e.
fn zitadel_config(issuer: &str, project_id: &str, markers: &[&str]) -> IamConfig {
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
                // The runbook's option 1: the project id. Both tokens of both flows carry it
                // (spec F2-F4), so an ID token passes the audience check and reaches the
                // marker check.
                audiences: vec![project_id.to_string()],
                jit_provisioning: true,
                id_token_marker_claims: markers.iter().map(|name| (*name).to_string()).collect(),
            }],
        },
        authz: AuthzConfig::default(),
        // `AppState::new` needs a valid pepper; the same fixed test pepper as keycloak_e2e.
        api_keys: ApiKeyConfig::with_test_pepper(STANDARD.encode([0x5Au8; 32])),
        audit: AuditConfig::default(),
        outbox: OutboxConfig::default(),
        metrics: MetricsConfig::default(),
        migration: MigrationConfig::default(),
    }
}

/// A string field of a token response, with the response in the panic message when it is
/// missing (an OAuth error body has no token).
fn token_field(response: &Value, field: &str) -> String {
    response[field].as_str().unwrap_or_else(|| panic!("no {field} in the token response: {response}")).to_string()
}

/// A string field of a management API response.
fn string_field(response: &Value, field: &str) -> String {
    response[field].as_str().unwrap_or_else(|| panic!("no {field} in the zitadel response: {response}")).to_string()
}

/// True when the payload has a top-level member `name` whose value is not JSON `null` (the
/// marker rule of spec D3).
fn has_claim(claims: &Value, name: &str) -> bool {
    claims.get(name).is_some_and(|value| !value.is_null())
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

/// Best-effort container stdout+stderr, for the failure panics only.
async fn dump_logs(container: &ContainerAsync<GenericImage>) -> String {
    let stdout = container.stdout_to_vec().await.map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
    let stderr = container.stderr_to_vec().await.map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
    format!("--- zitadel stdout ---\n{stdout}\n--- zitadel stderr ---\n{stderr}")
}
