// SPDX-License-Identifier: Apache-2.0

//! SMA-485: which Redis connection the API-key introspection cache is wired to.
//!
//! `AppState::new` reuses the authz `RedisHandle` for `api_keys.introspect_cache` only when the
//! two configured URLs match textually after trimming (SMA-485 D1); otherwise it dials its own
//! with `RedisRole::ApiKeys`, which is what gives the split deployment its own circuit breaker
//! (SMA-476 D1) and its own `role="api_keys"` metrics.
//!
//! **Why this observes a metric rather than the data path.** Proving that cache *traffic* reaches
//! the API-key Redis would mean opening a Redis client here to inspect the `iam:apikey:*` keys —
//! and `repo:redis-connect-single-site` bans the unnamed-connection constructors in `tests/` just
//! as in `src/` (moon.yml). The breaker gauge is the sanctioned observation channel: it is set at
//! construction (`redis_conn::connect` -> `Breaker::new(role)`), so the presence or absence of
//! `iam_redis_breaker_state{role="api_keys"}` is exactly "was a second connection opened".
//! Accepted residual: this proves the connection was opened from the configured URL, not that
//! traffic flows through it. The unit test `shares_one_connection_is_trimmed_textual_equality`
//! pins the predicate; `AppState::new` passes the dialled handle straight into
//! `RedisApiKeyCache::from_connection` on the next line.
//!
//! Runs against ephemeral Postgres + Redis in Docker. In CI (`CI` env set) a missing Docker
//! daemon is a HARD FAILURE; on a Docker-less laptop the test returns with a note — the same
//! gating pattern as `tests/authz_cache_redis.rs` / `tests/redis_jwks_cache.rs`.

mod support;

use paigasus_iam::adapters::http::AppState;
use paigasus_iam::config::{ApiKeyCacheBackend, AuthzCacheBackend, IamConfig, RedactedUrl};
use testcontainers_modules::redis::Redis;
use testcontainers_modules::testcontainers::ContainerAsync;
use testcontainers_modules::testcontainers::runners::AsyncRunner;

/// Starts an ephemeral Redis container, returning its connection URL. Same CI-hard-fail /
/// local-skip gating as `support::start_migrated_postgres`; self-contained here, mirroring
/// `tests/authz_cache_redis.rs`.
async fn start_redis() -> Option<(ContainerAsync<Redis>, String)> {
    let node = match Redis::default().start().await {
        Ok(n) => n,
        Err(e) => {
            if std::env::var_os("CI").is_some() {
                panic!("Docker is required for the api-key cache connection test in CI: {e}");
            }
            eprintln!("skipping api_key_cache_connection: Docker unavailable ({e})");
            return None;
        }
    };

    let port = node.get_host_port_ipv4(6379).await.unwrap();
    let url = format!("redis://127.0.0.1:{port}");
    Some((node, url))
}

/// `authz.cache` on redis at `authz_url`, `api_keys.introspect_cache` on redis at
/// `api_key_url` (`None` = the field left unset, which `IamConfig::validate` rejects — phase d).
fn split_config(base: &IamConfig, authz_url: &str, api_key_url: Option<&str>) -> IamConfig {
    let mut cfg = base.clone();
    cfg.authz.cache.backend = AuthzCacheBackend::Redis;
    cfg.authz.cache.redis_url = Some(authz_url.into());
    cfg.api_keys.introspect_cache.backend = ApiKeyCacheBackend::Redis;
    cfg.api_keys.introspect_cache.redis_url = api_key_url.map(RedactedUrl::from);
    cfg
}

const API_KEYS_SERIES: &str = r#"iam_redis_breaker_state{role="api_keys"}"#;
const AUTHZ_SERIES: &str = r#"iam_redis_breaker_state{role="authz"}"#;

/// The error plus its whole `source()` chain, flattened.
///
/// `AuthnError::Backend`'s `Display` is the bare literal `"backend error"` — every detail lives
/// in the boxed source. So `err.to_string()` alone is the SAME string for a dial failure and for
/// a missing-URL wiring defect, which would make both discriminating assertions below vacuously
/// true. This is also what an operator sees at boot: `main` returns `anyhow::Result<()>`, whose
/// `Termination` prints the cause chain under "Caused by:".
fn error_chain(err: &(dyn std::error::Error + 'static)) -> String {
    let mut out = err.to_string();
    let mut source = err.source();
    while let Some(cause) = source {
        out.push_str(" | ");
        out.push_str(&cause.to_string());
        source = cause.source();
    }
    out
}

/// All four phases in ONE test function, deliberately. Two reasons, neither of them the
/// `OnceLock`: container reuse (four `AppState::new` boots against one Postgres + one Redis, not
/// four pairs), and correctness under a plain `cargo test`, where the whole file shares one
/// process and therefore one metrics registry — the `api_keys` gauge, once set, never disappears,
/// so the absence assertion is only meaningful before the presence one. Under `cargo nextest run`
/// (what `.moon/tasks/rust.yml` actually runs) each test is its own process and the ordering
/// would be moot; it is kept so the file is correct under both runners.
///
/// `AppState::new` runs four times against one Postgres. Boot reconciliation is converge-to-code
/// and idempotent since SMA-477, so repeated boots against one database are what production does
/// on every restart.
#[tokio::test]
async fn api_key_cache_shares_the_authz_connection_only_on_matching_urls() {
    // MUST be first: `metrics::gauge!` against a not-yet-installed global recorder is silently
    // dropped, so installing it after the first `AppState::new` (the order `tests/metrics.rs`
    // uses) would make every assertion below pass vacuously.
    let handle = paigasus_observability::init("test-iam-api-key-cache-conn");

    let Some((_pg, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let Some((_redis, redis_url)) = start_redis().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let base = support::test_config(&idp);

    // --- Phase (a): identical URLs -> ONE shared connection (SMA-444 Task 21, AC2) -----------
    let cfg = split_config(&base, &redis_url, Some(&redis_url));
    AppState::new(db.clone(), &cfg).await.expect("phase a: both caches on one reachable redis");
    let out = handle.render();
    // The positive control is what makes the negative assertion mean anything: without it, a
    // dead recorder / renamed metric / misspelled label would all read as "absent" and the
    // phase would pass while proving nothing.
    assert!(out.contains(AUTHZ_SERIES), "phase a: the authz breaker must be registered (control):\n{out}");
    assert!(!out.contains(API_KEYS_SERIES), "phase a: identical URLs must share ONE connection, so no api_keys breaker:\n{out}");

    // --- Phase (b): distinct URLs -> its OWN connection, role api_keys (AC1) -----------------
    // `/1` selects logical database 1 on the same server: a different URL string, so D1 splits
    // it, and a reachable endpoint, so the dial succeeds. Stock Redis ships `databases 16` and
    // the testcontainers module does not override it; a `SELECT 1` failure would red the
    // `expect` below rather than pass silently.
    let cfg = split_config(&base, &redis_url, Some(&format!("{redis_url}/1")));
    AppState::new(db.clone(), &cfg).await.expect("phase b: both redis endpoints are reachable");
    let out = handle.render();
    assert!(out.contains(API_KEYS_SERIES), "phase b: distinct URLs must open a second connection with role=api_keys:\n{out}");

    // --- Phase (c): the api-key URL is actually dialled (AC1/AC3) ----------------------------
    // The regression proof: `redis_conn::connect` is eager, so before SMA-485 this config boots
    // happily (the URL is never read) and after it refuses to start. `127.0.0.1:1` follows the
    // crate's own precedent (`adapters/redis_conn.rs`): unbindable by an unprivileged process,
    // so deterministically refused, and not racy against testcontainers' port mapping the way
    // bind-ephemeral-then-drop would be.
    //
    // Safe to run after (b): `connect` propagates the dial failure with `?` BEFORE
    // `Breaker::new(role)`, so a failed dial registers no gauge and cannot invalidate (a).
    //
    // `AppState` is not `Debug` (it derives `Clone` only), so `unwrap_err`/`expect_err` will not
    // compile — assert on `is_err()` instead. Same trap SMA-476 documented in `redis_conn.rs`.
    let cfg = split_config(&base, &redis_url, Some("redis://127.0.0.1:1"));
    let err = AppState::new(db.clone(), &cfg).await.err();
    let err = error_chain(err.as_ref().expect("phase c: an unreachable api_keys redis_url must fail boot — it is dialled now"));
    // Discriminates a DIAL failure from phase (d)'s wiring-defect error, and pins the context
    // that tells an operator WHICH of the two possible connections failed. Deliberately not
    // asserting on "Connection refused", which is OS-specific.
    assert!(
        err.contains("api_keys.introspect_cache.redis_url is unreachable"),
        "phase c: expected the dial failure to name the config key, got: {err}"
    );
    assert!(!err.contains("IamConfig::validate"), "phase c: expected a dial failure, not the missing-url wiring defect: {err}");
    // The URL must not reach the logs (SMA-476 D4's posture) — it can carry a password.
    assert!(!err.contains("127.0.0.1:1"), "phase c: the boot error must not echo the configured URL: {err}");

    // --- Phase (d): a missing URL is a wiring defect, not "inherit authz's" (D2) -------------
    // Before SMA-485 this booted, because the `Some(conn)` arm masked the absent URL.
    let cfg = split_config(&base, &redis_url, None);
    let err = AppState::new(db.clone(), &cfg).await.err();
    let err = error_chain(err.as_ref().expect("phase d: backend=redis without redis_url must fail boot"));
    assert!(err.contains("IamConfig::validate"), "phase d: expected the wiring-defect error naming validate, got: {err}");
}
