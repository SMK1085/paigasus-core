// SPDX-License-Identifier: Apache-2.0

//! SMA-703: the end-to-end proof of `id_token_marker_claims` against a REAL Zitadel. It closes
//! the follow-up in § 10 of the spec (`2026-10-02-sma-703-zitadel-id-token-marker-claims-design.md`)
//! and the gap that the measurement used a public client only.
//!
//! The test sets up the paigasus console's own client type: a CONFIDENTIAL web app (client
//! secret, HTTP Basic), the code flow with PKCE, a refresh token, and JWT access tokens. A human
//! user logs in through the built-in Login v1 with plain HTTP requests, no browser: the test sends the
//! forms itself and keeps the cookies by hand. A machine user gets a token with the
//! client-credentials grant. A v1 Action (Complement Token, pre access token creation) adds
//! `email` to the access token, because Zitadel does not put it there (spec F12).
//!
//! What the test pins, for the pinned Zitadel version:
//! - Every ID token (human, refreshed, machine) has `at_hash` and `azp`, and no access token has
//!   either (spec F6). If a Zitadel upgrade changes this, this test fails.
//! - IAM, configured with `["at_hash", "azp"]`, refuses each ID token as `NotAnAccessToken`.
//! - IAM accepts the human access token, the refreshed access token and the machine access token.
//! - A control: with an EMPTY list, IAM does not refuse any of the three ID tokens as
//!   `NotAnAccessToken`. So the setting, not another check, does the refusal.
//!
//! Docker gating is the single policy of `tests/support/docker.rs`'s `start_or_skip` (SMA-538).
//! The SMA-703 test starts three containers: IAM's own Postgres, a second Postgres for Zitadel,
//! and Zitadel. The SMA-732 test starts the two Zitadel containers only. The two Zitadel
//! containers share a Docker network, so Zitadel reaches its Postgres by name.
//!
//! The issuer. MEASURED (2026-10-03, v4.15.3): Zitadel takes the HOST of `iss` from
//! `ZITADEL_EXTERNALDOMAIN`, and refuses a request whose `Host` header has a different host
//! (404). It takes the PORT of `iss` from the request `Host` header, not from
//! `ZITADEL_EXTERNALPORT`. So the test uses the port that Docker maps, and every call (the test's
//! own calls and IAM's JWKS fetch) uses the same `127.0.0.1:{mapped}` form. The readiness check
//! asserts that the discovery `issuer` is exactly that value.
//!
//! SMA-732: `zitadel_refresh_extends_access_token_exp` measures whether a refresh gives an access
//! token with a new `exp` (spec `2026-10-05-sma-732-zitadel-refresh-exp-design.md`). It pins, as
//! measured facts for the pinned version: `exp - iat` is `L` or up to 2 s less, and `nbf == iat`
//! (K1: two clock reads); `expires_in` is in the same range (K2); a refresh 7 s and 13 s after the
//! login moves `iat` and `exp` by at least 6 s each, before and after the first `exp`; each
//! refresh issues a new refresh token (K3). It has its own Zitadel instance, because it sets the
//! access token lifetime `L` to 10 s with an env var, and the SMA-703 test must keep the defaults
//! (spec D2, D6). Its panic message names the outcome of spec D5: "keeps" when `exp` did not move,
//! "refuses after exp" when the refresh after the first `exp` fails. Both outcomes stop the work
//! until Sven decides the fix scope. Any other red check is a defect in the test.

mod support;

use axum::http::StatusCode;
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use paigasus_iam::adapters::http::{AppState, router};
use paigasus_iam::adapters::persistence::entities::user;
use paigasus_iam::application::authenticate_token::Provisioning;
use paigasus_iam::config::{ApiKeyConfig, AuditConfig, AuthnConfig, AuthzConfig, DpopConfig, IamConfig, IssuerConfig, JwksCacheBackend, JwksCacheConfig, MetricsConfig, MigrationConfig, OutboxConfig};
use paigasus_iam_core::{AuthnError, ProvisioningDefect, TokenDefect};
use reqwest::header::{COOKIE, LOCATION, SET_COOKIE};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde_json::{Value, json};
use sha2::Digest;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;
use support::{provision_platform_admin, send, start_migrated_postgres};
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::core::IntoContainerPort;
use testcontainers_modules::testcontainers::{ContainerAsync, GenericImage, ImageExt};
use tokio::time::Instant;
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
/// 2 s on an idle Docker Desktop. The budget is for a loaded CI runner.
const READINESS_ATTEMPTS: u32 = 180;
/// The console's redirect URI. The test never follows the redirect: it reads the code from the
/// `Location` header. Login v1 accepts an `http` redirect URI only with `devMode` on the app.
const REDIRECT_URI: &str = "http://localhost:9999/callback";
const USER_EMAIL: &str = "zitadel-e2e@example.com";
const USER_PASSWORD: &str = "E2e-Passw0rd!x";
/// The IAM setting under test (spec D1, the runbook recipe).
const MARKER_CLAIMS: [&str; 2] = ["at_hash", "azp"];
/// The v1 Action of the reference install. It adds `email` to the access token of a human user.
/// This script must stay equal to the script in the Zitadel bullet of
/// `docs/ops/RUNBOOK-chart.md` section 6. Change both files together.
const ADD_EMAIL_CLAIM_SCRIPT: &str = r#"function addEmailClaim(ctx, api) {
  var user = ctx.v1.getUser();
  if (user.human === undefined || !user.human.email) {
    return;
  }
  // human.email is the Go type domain.EmailAddress. goja gives a named Go type
  // to JavaScript as an object, not as a primitive string, so convert it.
  api.v1.claims.setClaim('email', String(user.human.email));
}"#;
/// SMA-732: the env var that sets the access token lifetime of the default instance
/// (`cmd/defaults.yaml:1302` of Zitadel v4.15.3, spec D6).
const ACCESS_TOKEN_LIFETIME_ENV: &str = "ZITADEL_DEFAULTINSTANCE_OIDCSETTINGS_ACCESSTOKENLIFETIME";
/// SMA-732: the access token lifetime `L` of the refresh test's own instance, in seconds (spec D3).
/// If `iat1 < exp0` reds under load, the fix is a larger `L`, and only after Sven agrees.
const REFRESH_LIFETIME_SECS: i64 = 10;
/// SMA-732: the tolerance of A1 and A2 for the gap between two clock reads in one request (K1, K2).
const CLOCK_READ_TOLERANCE_SECS: i64 = 2;
/// SMA-732: the lower bound of A3 and A4, in seconds of Zitadel time.
const MIN_REFRESH_STEP_SECS: i64 = 6;
/// SMA-732 step 4: refresh 1 starts this long after the login response.
const REFRESH_1_AT: Duration = Duration::from_secs(7);
/// SMA-732 step 5: refresh 2 starts at the later of this time after the login response …
const REFRESH_2_AT: Duration = Duration::from_secs(13);
/// … and this time after the refresh 1 response.
const REFRESH_2_AFTER_REFRESH_1: Duration = Duration::from_secs(6);

#[tokio::test]
async fn zitadel_id_tokens_are_refused_by_the_marker_claims() {
    // IAM's own database first. If Docker is missing locally, the test skips here.
    let Some((_iam_pg, db)) = start_migrated_postgres().await else {
        return;
    };

    // Zitadel and its own Postgres (see `start_zitadel`). This test keeps the instance defaults.
    let Some(zitadel) = start_zitadel(&[]).await else {
        return;
    };
    // Owned copies, so that the rest of this test is unchanged. A `reqwest::Client` clone shares
    // the same connection pool.
    let http = zitadel.http.clone();
    let issuer = zitadel.issuer.clone();
    let setup = setup_zitadel(&zitadel.api()).await;

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

    // The control: with an EMPTY list (the default), IAM does not refuse any ID token as
    // NotAnAccessToken. So the setting does the refusal above, not the typ/logout check. The
    // human is provisioned already, so the human and the refreshed ID token resolve to the same
    // principal. The machine ID token passes the same checks as the machine access token.
    let open_cfg = zitadel_config(&issuer, &setup.project_id, &[]);
    let open_state = AppState::new(state.db.clone(), &open_cfg).await.expect("AppState::new (empty marker list)");
    for (label, token) in [("human ID token", &human_id), ("refreshed ID token", &refreshed_id)] {
        let principal = open_state
            .authn
            .resolve(token, Provisioning::Disabled)
            .await
            .unwrap_or_else(|err| panic!("with an empty marker list, IAM must accept the {label}, got {err:?}"));
        assert_eq!(principal.principal_id.canonical(), principal_prn, "the accepted {label} must resolve to the human principal");
    }
    let err = open_state
        .authn
        .resolve(&machine_id, Provisioning::Disabled)
        .await
        .expect_err("no principal exists for the machine user");
    assert!(
        matches!(err, AuthnError::IdentityNotProvisioned),
        "with an empty marker list, the machine ID token must reach the identity lookup, got {err:?}"
    );
    let err = open_state.authn.resolve(&machine_id, Provisioning::Enabled).await.expect_err("JIT needs an email");
    assert!(
        matches!(err, AuthnError::ProvisioningFailed(ProvisioningDefect::MissingEmail)),
        "with an empty marker list, the machine ID token must fail JIT only for the missing email, got {err:?}"
    );
}

/// SMA-732: does a refresh give an access token with a new `exp`? See the module doc and the
/// spec `2026-10-05-sma-732-zitadel-refresh-exp-design.md` (§ 4.2: the steps and A1-A8).
#[tokio::test]
async fn zitadel_refresh_extends_access_token_exp() {
    // Step 1: an instance of its own, with L = 10 s (spec D2, D6). No IAM Postgres (spec D7).
    let lifetime = format!("{REFRESH_LIFETIME_SECS}s");
    let Some(zitadel) = start_zitadel(&[(ACCESS_TOKEN_LIFETIME_ENV, lifetime.as_str())]).await else {
        return;
    };
    let setup = setup_zitadel(&zitadel.api()).await;
    let http = &zitadel.http;
    let issuer = zitadel.issuer.as_str();

    // Step 2: the login gives T0 and R0. `start` is the host base of every wait.
    let login = human_login(http, issuer, &setup).await;
    let start = Instant::now();
    let (t0, r0) = TokenTimes::read("T0", &login, start, None);
    println!("SMA-732 {t0}");

    // Step 3: an immediate refresh (K4: the M4a case of SMA-703). Only A1, A2, A6 and A7 read Tq.
    let tq_response = refresh(http, issuer, &setup, &r0)
        .await
        .unwrap_or_else(|(status, body)| panic!("the immediate refresh (Tq) failed ({status}): {body}\n{t0}"));
    let (tq, rq) = TokenTimes::read("Tq", &tq_response, start, Some(&r0));
    println!("SMA-732 {tq}");

    // Step 4: refresh 1, before the first exp.
    sleep_until(start + REFRESH_1_AT, "refresh 1 (before exp0)").await;
    let t1_response = refresh(http, issuer, &setup, &rq)
        .await
        .unwrap_or_else(|(status, body)| panic!("refresh 1 (T1) failed ({status}): {body}\n{t0}\n{tq}"));
    let t1_arrived = Instant::now();
    let (t1, r1) = TokenTimes::read("T1", &t1_response, start, Some(&rq));
    println!("SMA-732 {t1}");

    // Step 5: refresh 2, after the first exp. The wait is relative to t1 too, so A4 is a lower
    // bound under any load. Mutation M3 replaces `&r1` with `&r0` in the next call.
    sleep_until((start + REFRESH_2_AT).max(t1_arrived + REFRESH_2_AFTER_REFRESH_1), "refresh 2 (after exp0)").await;
    let t2_response = refresh(http, issuer, &setup, &r1)
        .await
        .unwrap_or_else(|(status, body)| panic!("A8: outcome \"refuses after exp\" (spec D5): refresh 2 (T2) failed ({status}): {body}\n{t0}\n{tq}\n{t1}"));
    let (t2, _) = TokenTimes::read("T2", &t2_response, start, Some(&r1));
    println!("SMA-732 {t2}");

    // Step 6: the checks. Each failure is collected, so one run shows every red check.
    let tokens = [&t0, &tq, &t1, &t2];
    let report = tokens.iter().map(|t| t.to_string()).collect::<Vec<_>>().join("\n");
    let mut failures: Vec<String> = Vec::new();

    // A1 (K1). Mutation M1 inserts ` + 3` after `REFRESH_LIFETIME_SECS` on the next line.
    let a1_lifetime = REFRESH_LIFETIME_SECS;
    for t in tokens {
        let lifetime = t.exp - t.iat;
        if !(a1_lifetime - CLOCK_READ_TOLERANCE_SECS..=a1_lifetime).contains(&lifetime) {
            failures.push(format!(
                "A1: {}: exp - iat = {lifetime}, expected {}..={a1_lifetime} (spec K1)",
                t.label,
                a1_lifetime - CLOCK_READ_TOLERANCE_SECS
            ));
        }
        if t.nbf != t.iat {
            failures.push(format!("A1: {}: nbf {} != iat {} (spec K1)", t.label, t.nbf, t.iat));
        }
    }

    // A2 (K2).
    for t in tokens {
        if !(REFRESH_LIFETIME_SECS - CLOCK_READ_TOLERANCE_SECS..=REFRESH_LIFETIME_SECS).contains(&t.expires_in) {
            failures.push(format!(
                "A2: {}: expires_in = {}, expected {}..={REFRESH_LIFETIME_SECS} (spec K2)",
                t.label,
                t.expires_in,
                REFRESH_LIFETIME_SECS - CLOCK_READ_TOLERANCE_SECS
            ));
        }
    }

    // A3-A5 compare T1 and T2 with T0. Mutation M2 inserts `.map(|_| &t0)` after `[&t1, &t2]`.
    let [later1, later2] = [&t1, &t2];
    let keeps = later1.exp == t0.exp || later2.exp == t0.exp;
    let keeps_note = if keeps { " Outcome \"keeps\" (spec D5): exp did not move." } else { "" };

    // A3.
    if later1.iat - t0.iat < MIN_REFRESH_STEP_SECS || later1.exp - t0.exp < MIN_REFRESH_STEP_SECS {
        failures.push(format!(
            "A3: {} vs T0: iat +{}, exp +{}, expected both >= {MIN_REFRESH_STEP_SECS}.{keeps_note}",
            later1.label,
            later1.iat - t0.iat,
            later1.exp - t0.exp
        ));
    }
    // A4.
    if later2.iat - later1.iat < MIN_REFRESH_STEP_SECS || later2.exp - later1.exp < MIN_REFRESH_STEP_SECS {
        failures.push(format!(
            "A4: {} vs {}: iat +{}, exp +{}, expected both >= {MIN_REFRESH_STEP_SECS}.{keeps_note}",
            later2.label,
            later1.label,
            later2.iat - later1.iat,
            later2.exp - later1.exp
        ));
    }
    // A5.
    let late_refresh_1 = later1.iat >= t0.exp;
    if late_refresh_1 {
        failures.push(format!(
            "A5: iat1 {} >= exp0 {}: refresh 1 came after the first exp. Spec D3: do not remove this check; a larger L needs Sven's decision.",
            later1.iat, t0.exp
        ));
    }
    if later2.iat <= t0.exp {
        failures.push(format!("A5: iat2 {} <= exp0 {}: refresh 2 came before the first exp.{keeps_note}", later2.iat, t0.exp));
    }

    // A6.
    let jtis: BTreeSet<&str> = tokens.iter().map(|t| t.jti.as_str()).collect();
    if jtis.len() != tokens.len() {
        failures.push(format!("A6: the jti values are not all different: {jtis:?}"));
    }

    // A7 (K3: rotation). Printed as booleans, never as tokens.
    for t in [&tq, &t1, &t2] {
        if t.refresh_rotated != Some(true) {
            failures.push(format!("A7: {}: refresh_rotated = {:?}, expected Some(true) (spec K3)", t.label, t.refresh_rotated));
        }
    }

    let verdict = if keeps {
        "Outcome \"keeps\" (spec D5): Zitadel did not extend exp. STOP: report to the coordinator; do not change L or the scope."
    } else if late_refresh_1 {
        "iat1 >= exp0 (spec D3): the step-4 refresh was too slow. STOP: report to the coordinator; do not change L."
    } else {
        "No D5 outcome matched: a red check here is a defect in the test (spec D5)."
    };
    assert!(
        failures.is_empty(),
        "SMA-732: {} check(s) failed. {verdict}\n{}\n--- tokens ---\n{report}",
        failures.len(),
        failures.join("\n")
    );
    println!("SMA-732 outcome: \"extends\" (spec D5): A1-A8 pass");
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

/// A running Zitadel with its own Postgres, and the client, issuer and admin PAT to call it.
/// The containers stop when this value is dropped, so a test keeps it alive to its end.
struct ZitadelInstance {
    /// Never read. It is here only so that the Postgres container lives as long as Zitadel.
    _zitadel_pg: ContainerAsync<Postgres>,
    zitadel: ContainerAsync<GenericImage>,
    /// One client for every call of a test. It never follows a redirect, because the login
    /// flow must read each `Location` header, and the last one points at a server that does not
    /// exist.
    http: reqwest::Client,
    /// `https://127.0.0.1:{mapped port}` (see the module doc).
    issuer: String,
    /// The first-instance admin PAT.
    pat: String,
}

impl ZitadelInstance {
    /// The management API of this instance.
    fn api(&self) -> ZitadelApi<'_> {
        ZitadelApi {
            http: &self.http,
            base: &self.issuer,
            pat: &self.pat,
        }
    }
}

/// Starts Zitadel's own Postgres and Zitadel on a Docker network of their own, waits until
/// discovery and the management API answer, and reads the admin PAT. `extra_env` adds env vars
/// to the Zitadel container after the fixed ones (an empty slice keeps the defaults). Returns
/// `None` when `start_or_skip` skips (no Docker, per `tests/support/docker.rs`).
async fn start_zitadel(extra_env: &[(&str, &str)]) -> Option<ZitadelInstance> {
    // Zitadel's own Postgres, on a network of its own. The suffix keeps parallel runs apart.
    let suffix = format!("{:016x}", rand::random::<u64>());
    let network = format!("zitadel-e2e-{suffix}");
    let pg_host = format!("zitadel-e2e-pg-{suffix}");
    let zitadel_pg_image = Postgres::default().with_tag("16-alpine").with_network(&network).with_container_name(&pg_host);
    let zitadel_pg = support::docker::start_or_skip(zitadel_pg_image, "zitadel_e2e postgres").await?;
    // The Postgres module reports ready on its first log line, while the init server (unix
    // socket only) still runs. Zitadel stops at once when its first connection fails, so wait
    // for a TCP connection, which only the real server accepts.
    wait_for_postgres(&zitadel_pg).await;

    // A runtime self-signed cert for Zitadel's TLS listener, copied into the container.
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string(), "127.0.0.1".to_string()]).expect("self-signed cert");
    let cert_pem = cert.cert.pem().into_bytes();
    let key_pem = cert.signing_key.serialize_pem().into_bytes();

    let mut image = GenericImage::new(ZITADEL_IMAGE, ZITADEL_TAG)
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
    for (name, value) in extra_env {
        image = image.with_env_var(*name, *value);
    }

    let zitadel = support::docker::start_or_skip(image, "zitadel_e2e").await?;
    let https_port = support::docker::mapped_port(&zitadel, HTTPS_PORT, "zitadel https").await;
    let issuer = format!("https://127.0.0.1:{https_port}");

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
    let instance = ZitadelInstance {
        _zitadel_pg: zitadel_pg,
        zitadel,
        http,
        issuer,
        pat,
    };
    // MEASURED: discovery answers before the management API does. The REST gateway first
    // returns 503 `dial tcp [::1]:8080: connect: connection refused` for a few seconds.
    if !instance.api().wait_until_ready().await {
        panic!("the zitadel management API never became ready\n{}", dump_logs(&instance.zitadel).await);
    }
    Some(instance)
}

/// The times of one access token of the SMA-732 test, from its JWT and from its token response.
/// It holds no token: only claims, `expires_in`, a host offset and a boolean.
struct TokenTimes {
    label: &'static str,
    iat: i64,
    nbf: i64,
    exp: i64,
    jti: String,
    expires_in: i64,
    /// Host milliseconds since the login response. Only the waits use host time; no check reads it.
    offset_ms: u128,
    /// True when the response's refresh token differs from the one that the request sent.
    /// `None` for the login, which sent no refresh token.
    refresh_rotated: Option<bool>,
}

impl TokenTimes {
    /// Reads the access token times of `response`. Returns them with the response's refresh
    /// token. The caller must never put that refresh token into a message.
    fn read(label: &'static str, response: &Value, start: Instant, previous_refresh: Option<&str>) -> (Self, String) {
        let access = secret_field(response, "access_token", label);
        let refresh = secret_field(response, "refresh_token", label);
        let claims = jwt_payload(&access);
        let times = Self {
            label,
            iat: int_claim(&claims, "iat", label),
            nbf: int_claim(&claims, "nbf", label),
            exp: int_claim(&claims, "exp", label),
            jti: claims["jti"]
                .as_str()
                .unwrap_or_else(|| panic!("{label}: the access token has no string jti claim: {claims}"))
                .to_string(),
            expires_in: response["expires_in"]
                .as_i64()
                .unwrap_or_else(|| panic!("{label}: the token response has no integer expires_in. Keys: {:?}", response_keys(response))),
            offset_ms: start.elapsed().as_millis(),
            refresh_rotated: previous_refresh.map(|previous| previous != refresh),
        };
        (times, refresh)
    }
}

impl std::fmt::Display for TokenTimes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let rotated = match self.refresh_rotated {
            Some(rotated) => rotated.to_string(),
            None => "n/a".to_string(),
        };
        write!(
            f,
            "{}: iat={} nbf={} exp={} exp-iat={} expires_in={} jti={} offset_ms={} refresh_rotated={rotated}",
            self.label,
            self.iat,
            self.nbf,
            self.exp,
            self.exp - self.iat,
            self.expires_in,
            self.jti,
            self.offset_ms
        )
    }
}

/// One refresh grant of the console's confidential web app (HTTP Basic client auth).
async fn refresh(http: &reqwest::Client, issuer: &str, setup: &ZitadelSetup, refresh_token: &str) -> Result<Value, (StatusCode, String)> {
    token_request(
        http,
        issuer,
        Some((&setup.client_id, &setup.client_secret)),
        &[("grant_type", "refresh_token"), ("refresh_token", refresh_token)],
    )
    .await
}

/// Sleeps until `deadline` and prints how long it waits, so that the run output shows the real
/// schedule. Returns at once when the deadline has passed.
async fn sleep_until(deadline: Instant, what: &str) {
    let wait = deadline.saturating_duration_since(Instant::now());
    println!("SMA-732 waiting {} ms for {what}", wait.as_millis());
    tokio::time::sleep_until(deadline).await;
}

/// A string field of a token response that holds a secret. The panic names the keys of the
/// response, never the response, because the response holds tokens.
fn secret_field(response: &Value, field: &str, label: &str) -> String {
    response[field]
        .as_str()
        .unwrap_or_else(|| panic!("{label}: no string {field} in the token response. Keys: {:?}", response_keys(response)))
        .to_string()
}

/// An integer claim of a decoded access token payload. Claims hold no secret, so the panic
/// prints them.
fn int_claim(claims: &Value, name: &str, label: &str) -> i64 {
    claims[name].as_i64().unwrap_or_else(|| panic!("{label}: the access token has no integer {name} claim: {claims}"))
}

/// The top-level keys of a JSON object, for the panic messages of `secret_field`.
fn response_keys(response: &Value) -> Vec<String> {
    response.as_object().map(|object| object.keys().cloned().collect()).unwrap_or_default()
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

/// The human flow with plain HTTP requests, no browser: `/oauth/v2/authorize` with PKCE S256, the Login v1 forms
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
    let mut last_page = String::from("(no page was rendered)");
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
        last_page = page_title(&page);
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
    let callback = callback.unwrap_or_else(|| panic!("login v1 never redirected to the redirect URI. Last URL: {url}. Last page: {last_page}"));
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
            dpop: DpopConfig::default(),
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
