// SPDX-License-Identifier: Apache-2.0

//! JWKS fetch + cache + rotation (spec §4.2/§4.3): a `CachedJwks` entry is fresh while its
//! `fetched_at` is within `authn.jwks_ttl_secs` of the injected `Clock`'s `now()`. An unknown
//! `kid` forces at most one refetch per issuer per `authn.jwks_refresh_cooldown_secs` cooldown
//! window, so a burst of tokens carrying an unrecognized `kid` cannot be used to hammer the
//! IdP. Concurrent callers needing a refetch for the same issuer coalesce onto a single HTTP
//! fetch via a per-issuer `tokio::sync::Mutex` (single-flight), which doubles as the cooldown
//! state's guard.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use jsonwebtoken::jwk::{Jwk, JwkSet};
use paigasus_iam_core::{AuthnError, Clock, Issuer, TokenDefect};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, RwLock};

use crate::adapters::clock::SystemClock;

/// A cached JWKS payload plus fetch bookkeeping (spec §4.3). The discovery doc's `jwks_uri`
/// is cached alongside the keys since discovery + JWKS share one TTL/refresh cycle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedJwks {
    pub jwks: JwkSet,
    pub jwks_uri: String,
    pub fetched_at: DateTime<Utc>,
}

/// Per-issuer JWKS storage (D2: an infra detail of the OIDC adapter, not a domain port).
#[async_trait]
pub trait JwksCache: Send + Sync {
    async fn get(&self, issuer: &Issuer) -> Result<Option<CachedJwks>, AuthnError>;
    async fn put(&self, issuer: &Issuer, jwks: CachedJwks) -> Result<(), AuthnError>;
}

/// In-process `JwksCache` (default backend, D2). Never fails: this is the fallback used
/// even when no external cache is configured.
#[derive(Default)]
pub struct InMemoryJwksCache(RwLock<HashMap<Issuer, CachedJwks>>);

impl InMemoryJwksCache {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl JwksCache for InMemoryJwksCache {
    async fn get(&self, issuer: &Issuer) -> Result<Option<CachedJwks>, AuthnError> {
        Ok(self.0.read().await.get(issuer).cloned())
    }

    async fn put(&self, issuer: &Issuer, jwks: CachedJwks) -> Result<(), AuthnError> {
        self.0.write().await.insert(issuer.clone(), jwks);
        Ok(())
    }
}

/// Fetches a fresh discovery + JWKS document pair for an issuer. A seam so `JwksProvider`'s
/// cache/rotation logic can be unit-tested without real HTTP (spec §4.3).
#[async_trait]
pub trait JwksFetcher: Send + Sync {
    async fn fetch(&self, issuer: &Issuer) -> Result<CachedJwks, AuthnError>;
}

/// Response bodies (discovery doc and JWKS document) are capped at 1 MiB, read via bounded
/// `chunk()` streaming rather than an unbounded `.text()`/`.bytes()` read (spec §4.2).
const MAX_RESPONSE_BODY_BYTES: usize = 1024 * 1024;

#[derive(Deserialize)]
struct DiscoveryDocument {
    issuer: String,
    jwks_uri: String,
}

/// A config/wiring fault, carrying its cause. `AuthnError::Backend` takes a boxed error and std
/// supplies `From<String> for Box<dyn Error + Send + Sync>`, which is the idiom
/// `adapters/http/mod.rs:605` already uses.
fn backend(message: String) -> AuthnError {
    AuthnError::Backend(message.into())
}

/// How the IdP HTTP client establishes TLS trust.
///
/// An enum rather than a `bool` + `Option<&str>` pair so that "certificate verification disabled
/// AND a trust bundle configured" — always an operator mistake, since a disabled verifier can
/// never consult the bundle — is unrepresentable at the type level. Same reasoning as the
/// gateway's `IamTlsConfig` (SMA-504 D8); it also removes a transposable positional pair.
///
/// `IamConfig::validate` rejects the same combination at the config-file level, so the operator
/// gets a readable message rather than a type error they cannot see.
pub enum IdpTls<'a> {
    /// TEST-ONLY: `danger_accept_invalid_certs`. See `AuthnConfig::accept_invalid_tls` — this
    /// DISABLES verification for every fetch the client makes, which is a full authentication
    /// bypass in production.
    AcceptInvalid,
    /// Verify normally. The client's trust anchors are the compiled-in webpki Mozilla roots, the
    /// platform store (`/etc/ssl/certs`), AND every certificate in `extra_bundle` if set — all
    /// three unioned (SMA-558 D1).
    Verify { extra_bundle: Option<&'a str> },
}

/// Live `JwksFetcher`: `GET {issuer}/.well-known/openid-configuration`, verify the document's
/// `issuer` field exactly matches and its `jwks_uri` is `https`, then `GET` the JWKS itself
/// (spec §4.2).
#[derive(Debug)]
pub struct HttpJwksFetcher {
    client: reqwest::Client,
    clock: SystemClock,
}

impl HttpJwksFetcher {
    /// Builds the fetcher's `reqwest::Client` with the given request timeout and TLS posture.
    /// No custom redirect policy is needed — reqwest's default is fine for a discovery endpoint
    /// operators configure directly.
    ///
    /// Every failure here is a BOOT failure carrying its cause (SMA-558 D4). It returns
    /// `AuthnError::Backend`, never `Unavailable`: a misconfigured bundle path must be
    /// diagnosable, not indistinguishable from the IdP being down.
    pub fn new(timeout: Duration, tls: IdpTls<'_>) -> Result<Self, AuthnError> {
        let mut builder = reqwest::Client::builder().timeout(timeout);

        match tls {
            IdpTls::AcceptInvalid => builder = builder.danger_accept_invalid_certs(true),
            IdpTls::Verify { extra_bundle: None } => {}
            IdpTls::Verify { extra_bundle: Some(path) } => {
                let pem = std::fs::read(path).map_err(|e| backend(format!("failed to read authn.extra_ca_bundle_path {path:?}: {e}")))?;

                // `from_pem_bundle`, NOT `from_pem`: a bundle may legitimately carry more than one
                // ROOT (a cross-signed CA, or two corporate roots mid-rotation) and `from_pem`
                // reads only the first. This is NOT an invitation to add intermediates — every
                // certificate here becomes an UNCONSTRAINED trust anchor (rustls performs no `cA`
                // basic-constraints check on an anchor), so an intermediate would be promoted to a
                // root for every HTTPS call this process makes.
                let certs = reqwest::Certificate::from_pem_bundle(&pem).map_err(|e| backend(format!("authn.extra_ca_bundle_path {path:?} is not a valid PEM certificate bundle: {e}")))?;

                // `from_pem_bundle` returns Ok(vec![]) — not an error — for any file with no PEM
                // CERTIFICATE section: a DER-encoded .crt, a key-only PEM, an empty file, a
                // truncated mount. Without this guard the likeliest operator mistake boots green
                // having added nothing at all (SMA-558 § 2.8).
                if certs.is_empty() {
                    return Err(backend(format!(
                        "authn.extra_ca_bundle_path {path:?} contained no PEM certificates — a DER file, \
                         a key-only PEM or an empty file parses as an empty bundle"
                    )));
                }

                tracing::info!(
                    path = %path,
                    count = certs.len(),
                    "loaded extra IdP trust anchors from authn.extra_ca_bundle_path"
                );

                for cert in certs {
                    builder = builder.add_root_certificate(cert);
                }
            }
        }

        let client = builder.build().map_err(|e| {
            backend(format!(
                "failed to build the IdP HTTP client: {e} — this can also mean the platform trust store \
                 contains no parseable certificates"
            ))
        })?;
        Ok(Self { client, clock: SystemClock })
    }

    /// Logs the issuer and a static defect-kind tag — never response bodies or full URLs
    /// beyond the issuer itself (spec §4.2's "no bodies in logs" constraint).
    fn unavailable(&self, issuer: &Issuer, defect: &'static str) -> AuthnError {
        tracing::warn!(issuer = %issuer, defect, "jwks fetch failed");
        AuthnError::Unavailable
    }
}

/// Reads a response body capped at `MAX_RESPONSE_BODY_BYTES`, streaming via `chunk()` so an
/// oversized or slow-loris body never gets buffered unbounded. A non-success status is also
/// treated as a read failure (caller only cares "did we get a usable body").
async fn read_capped_body(mut response: reqwest::Response) -> Result<Vec<u8>, &'static str> {
    if !response.status().is_success() {
        return Err("non-success http status");
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| "body read error")? {
        body.extend_from_slice(&chunk);
        if body.len() > MAX_RESPONSE_BODY_BYTES {
            return Err("response body exceeded size cap");
        }
    }
    Ok(body)
}

#[async_trait]
impl JwksFetcher for HttpJwksFetcher {
    async fn fetch(&self, issuer: &Issuer) -> Result<CachedJwks, AuthnError> {
        let discovery_url = format!("{}/.well-known/openid-configuration", issuer.as_str());
        let discovery_response = self.client.get(&discovery_url).send().await.map_err(|_| self.unavailable(issuer, "discovery request failed"))?;
        let discovery_body = read_capped_body(discovery_response).await.map_err(|defect| self.unavailable(issuer, defect))?;
        let discovery: DiscoveryDocument = serde_json::from_slice(&discovery_body).map_err(|_| self.unavailable(issuer, "discovery body malformed"))?;

        if discovery.issuer != issuer.as_str() {
            return Err(self.unavailable(issuer, "discovery issuer mismatch"));
        }
        if !discovery.jwks_uri.starts_with("https://") {
            return Err(self.unavailable(issuer, "jwks_uri not https"));
        }

        let jwks_response = self.client.get(&discovery.jwks_uri).send().await.map_err(|_| self.unavailable(issuer, "jwks request failed"))?;
        let jwks_body = read_capped_body(jwks_response).await.map_err(|defect| self.unavailable(issuer, defect))?;
        let jwks: JwkSet = serde_json::from_slice(&jwks_body).map_err(|_| self.unavailable(issuer, "jwks body malformed"))?;

        Ok(CachedJwks {
            jwks,
            jwks_uri: discovery.jwks_uri,
            fetched_at: self.clock.now(),
        })
    }
}

/// The result of a cache lookup: whether an entry existed at all (regardless of freshness),
/// and whether it yielded a usable (fresh + matching-`kid`) key.
struct CacheState {
    entry: Option<CachedJwks>,
    hit: Option<Jwk>,
}

/// Per-issuer single-flight lock + "last forced refetch attempt" cooldown clock (see
/// `JwksProvider::refetch_state`).
type RefetchState = Mutex<HashMap<Issuer, Arc<Mutex<Option<DateTime<Utc>>>>>>;

/// Fetch + cache + rotation orchestration (spec §4.3). Generic-by-value over its three
/// collaborators (no `Arc<dyn Trait>`, per the hexagonal composition convention) so the
/// concrete production wiring (`HttpJwksFetcher` + `InMemoryJwksCache`/`RedisJwksCache` +
/// `SystemClock`) is chosen once, at the composition root.
pub struct JwksProvider<F: JwksFetcher, K: JwksCache, C: Clock> {
    fetcher: F,
    cache: K,
    clock: C,
    ttl: chrono::Duration,
    cooldown: chrono::Duration,
    /// Per-issuer single-flight lock, doubling as the "last forced refetch attempt" cooldown
    /// clock: whoever holds an issuer's inner `Mutex` guard owns that issuer's in-flight
    /// fetch, so concurrent callers needing a refetch simply await the same guard instead of
    /// racing separate HTTP requests (spec §4.3 "single-flight").
    refetch_state: RefetchState,
}

impl<F: JwksFetcher, K: JwksCache, C: Clock> JwksProvider<F, K, C> {
    pub fn new(fetcher: F, cache: K, clock: C, ttl: Duration, cooldown: Duration) -> Self {
        Self {
            fetcher,
            cache,
            clock,
            ttl: chrono::Duration::from_std(ttl).unwrap_or(chrono::Duration::MAX),
            cooldown: chrono::Duration::from_std(cooldown).unwrap_or(chrono::Duration::MAX),
            refetch_state: Mutex::new(HashMap::new()),
        }
    }

    /// Resolves the JWK identified by `kid` for `issuer`, per the rotation algorithm in
    /// spec §4.3: a fresh cache entry containing `kid` returns immediately; otherwise a
    /// (possibly single-flighted, cooldown-gated) refetch is attempted.
    ///
    /// Precondition: `issuer` must already be one of the configured, allowlisted issuers —
    /// the validator checks `AuthnError::IssuerNotConfigured` before ever reaching this call.
    /// The per-issuer cache and refetch-cooldown maps grow one entry per distinct issuer seen
    /// here and never evict, so passing arbitrary/unconfigured issuers would leak memory.
    pub async fn key_for(&self, issuer: &Issuer, kid: &str) -> Result<Jwk, AuthnError> {
        if let Some(jwk) = self.cache_state(issuer, kid).await?.hit {
            return Ok(jwk);
        }

        let issuer_lock = self.lock_for(issuer).await;
        let mut last_refetch = issuer_lock.lock().await;

        // Double-check: another caller may have refreshed the cache while we waited for the
        // per-issuer lock (spec §4.3's "double-check the cache after acquiring").
        let state = self.cache_state(issuer, kid).await?;
        if let Some(jwk) = state.hit {
            return Ok(jwk);
        }
        let entry_is_fresh = state.entry.as_ref().is_some_and(|entry| self.is_fresh(entry));

        let cooldown_active = last_refetch.is_some_and(|attempted_at| self.clock.now().signed_duration_since(attempted_at) < self.cooldown);
        if cooldown_active {
            return if entry_is_fresh {
                // The cached entry was refreshed recently and genuinely lacks this kid;
                // the cooldown just prevents a needless repeat refetch.
                Err(AuthnError::InvalidToken(TokenDefect::UnknownKid))
            } else {
                // No entry, or a stale one: we cannot validate right now, and the cooldown
                // blocks retrying the fetch immediately. A stale key is never served.
                Err(AuthnError::Unavailable)
            };
        }

        *last_refetch = Some(self.clock.now());
        match self.fetcher.fetch(issuer).await {
            Ok(fetched) => {
                let hit = find_kid(&fetched.jwks, kid);
                self.cache.put(issuer, fetched).await?;
                hit.ok_or(AuthnError::InvalidToken(TokenDefect::UnknownKid))
            }
            // A refetch failure is always `Unavailable`, whether or not a stale cached
            // entry exists — a stale key is never served as a substitute (spec §4.3).
            Err(err) => Err(err),
        }
    }

    async fn cache_state(&self, issuer: &Issuer, kid: &str) -> Result<CacheState, AuthnError> {
        let entry = self.cache.get(issuer).await?;
        let hit = entry.as_ref().filter(|cached| self.is_fresh(cached)).and_then(|cached| find_kid(&cached.jwks, kid));
        Ok(CacheState { entry, hit })
    }

    fn is_fresh(&self, entry: &CachedJwks) -> bool {
        self.clock.now().signed_duration_since(entry.fetched_at) < self.ttl
    }

    async fn lock_for(&self, issuer: &Issuer) -> Arc<Mutex<Option<DateTime<Utc>>>> {
        let mut locks = self.refetch_state.lock().await;
        locks.entry(issuer.clone()).or_insert_with(|| Arc::new(Mutex::new(None))).clone()
    }
}

fn find_kid(jwks: &JwkSet, kid: &str) -> Option<Jwk> {
    jwks.keys.iter().find(|jwk| jwk.common.key_id.as_deref() == Some(kid)).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::sync::Mutex as StdMutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn test_issuer() -> Issuer {
        Issuer::parse("https://idp.example.com").unwrap()
    }

    fn base_time() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap()
    }

    fn jwk_set_with_kid(kid: &str) -> JwkSet {
        serde_json::from_value(serde_json::json!({
            "keys": [{
                "kty": "RSA",
                "kid": kid,
                "use": "sig",
                "alg": "RS256",
                "n": "test-modulus",
                "e": "AQAB",
            }]
        }))
        .expect("valid jwk set fixture")
    }

    fn cached(kid: &str, fetched_at: DateTime<Utc>) -> CachedJwks {
        CachedJwks {
            jwks: jwk_set_with_kid(kid),
            jwks_uri: "https://idp.example.com/jwks".to_string(),
            fetched_at,
        }
    }

    /// Adjustable clock so tests can drive TTL expiry and cooldown windows deterministically.
    #[derive(Clone)]
    struct FakeClock(Arc<StdMutex<DateTime<Utc>>>);

    impl FakeClock {
        fn at(t: DateTime<Utc>) -> Self {
            Self(Arc::new(StdMutex::new(t)))
        }

        fn advance(&self, delta: chrono::Duration) {
            let mut guard = self.0.lock().unwrap();
            *guard += delta;
        }
    }

    impl Clock for FakeClock {
        fn now(&self) -> DateTime<Utc> {
            *self.0.lock().unwrap()
        }
    }

    /// Fake `JwksFetcher`: counts calls, optionally sleeps (to force single-flight overlap),
    /// optionally fails, and otherwise "serves" a fresh `JwkSet` containing exactly one kid.
    struct FakeFetcher {
        calls: Arc<AtomicUsize>,
        clock: FakeClock,
        serves_kid: &'static str,
        fail: bool,
        delay: Duration,
    }

    #[async_trait]
    impl JwksFetcher for FakeFetcher {
        async fn fetch(&self, issuer: &Issuer) -> Result<CachedJwks, AuthnError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if !self.delay.is_zero() {
                tokio::time::sleep(self.delay).await;
            }
            if self.fail {
                return Err(AuthnError::Unavailable);
            }
            let _ = issuer;
            Ok(cached(self.serves_kid, self.clock.now()))
        }
    }

    #[tokio::test]
    async fn fresh_cache_hit_does_not_fetch() {
        let issuer = test_issuer();
        let clock = FakeClock::at(base_time());
        let cache = InMemoryJwksCache::new();
        cache.put(&issuer, cached("kid-a", clock.now())).await.unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let fetcher = FakeFetcher {
            calls: calls.clone(),
            clock: clock.clone(),
            serves_kid: "kid-a",
            fail: false,
            delay: Duration::ZERO,
        };
        let provider = JwksProvider::new(fetcher, cache, clock, Duration::from_secs(3600), Duration::from_secs(30));

        let jwk = provider.key_for(&issuer, "kid-a").await.unwrap();

        assert_eq!(jwk.common.key_id.as_deref(), Some("kid-a"));
        assert_eq!(calls.load(Ordering::SeqCst), 0, "a fresh hit must not fetch");
    }

    #[tokio::test]
    async fn ttl_expiry_triggers_refetch() {
        let issuer = test_issuer();
        let clock = FakeClock::at(base_time());
        let cache = InMemoryJwksCache::new();
        cache.put(&issuer, cached("kid-a", clock.now())).await.unwrap();
        clock.advance(chrono::Duration::seconds(3601));
        let calls = Arc::new(AtomicUsize::new(0));
        let fetcher = FakeFetcher {
            calls: calls.clone(),
            clock: clock.clone(),
            serves_kid: "kid-a",
            fail: false,
            delay: Duration::ZERO,
        };
        let provider = JwksProvider::new(fetcher, cache, clock, Duration::from_secs(3600), Duration::from_secs(30));

        let jwk = provider.key_for(&issuer, "kid-a").await.unwrap();

        assert_eq!(jwk.common.key_id.as_deref(), Some("kid-a"));
        assert_eq!(calls.load(Ordering::SeqCst), 1, "a stale entry must trigger exactly one refetch");
    }

    #[tokio::test]
    async fn kid_miss_triggers_one_refetch_then_unknown_kid() {
        let issuer = test_issuer();
        let clock = FakeClock::at(base_time());
        let cache = InMemoryJwksCache::new();
        cache.put(&issuer, cached("kid-old", clock.now())).await.unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        // The upstream JWKS hasn't actually rotated yet, so `kid-new` stays unknown even
        // after the forced refetch.
        let fetcher = FakeFetcher {
            calls: calls.clone(),
            clock: clock.clone(),
            serves_kid: "kid-old",
            fail: false,
            delay: Duration::ZERO,
        };
        let provider = JwksProvider::new(fetcher, cache, clock, Duration::from_secs(3600), Duration::from_secs(30));

        let err = provider.key_for(&issuer, "kid-new").await.unwrap_err();

        assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::UnknownKid)));
        assert_eq!(calls.load(Ordering::SeqCst), 1, "an unknown kid must force exactly one refetch");
    }

    #[tokio::test]
    async fn cooldown_suppresses_repeated_kid_miss_refetch() {
        let issuer = test_issuer();
        let clock = FakeClock::at(base_time());
        let cache = InMemoryJwksCache::new();
        cache.put(&issuer, cached("kid-old", clock.now())).await.unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let fetcher = FakeFetcher {
            calls: calls.clone(),
            clock: clock.clone(),
            serves_kid: "kid-old",
            fail: false,
            delay: Duration::ZERO,
        };
        let provider = JwksProvider::new(fetcher, cache, clock.clone(), Duration::from_secs(3600), Duration::from_secs(30));

        let first = provider.key_for(&issuer, "kid-new").await.unwrap_err();
        assert!(matches!(first, AuthnError::InvalidToken(TokenDefect::UnknownKid)));
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // Still well within the 30s cooldown window: the second miss must not fetch again.
        clock.advance(chrono::Duration::seconds(5));
        let second = provider.key_for(&issuer, "kid-new").await.unwrap_err();

        assert!(matches!(second, AuthnError::InvalidToken(TokenDefect::UnknownKid)));
        assert_eq!(calls.load(Ordering::SeqCst), 1, "the cooldown must suppress the second forced refetch");
    }

    #[tokio::test]
    async fn fetch_failure_without_cache_is_unavailable() {
        let issuer = test_issuer();
        let clock = FakeClock::at(base_time());
        let cache = InMemoryJwksCache::new();
        let calls = Arc::new(AtomicUsize::new(0));
        let fetcher = FakeFetcher {
            calls: calls.clone(),
            clock: clock.clone(),
            serves_kid: "kid-a",
            fail: true,
            delay: Duration::ZERO,
        };
        let provider = JwksProvider::new(fetcher, cache, clock, Duration::from_secs(3600), Duration::from_secs(30));

        let err = provider.key_for(&issuer, "kid-a").await.unwrap_err();

        assert!(matches!(err, AuthnError::Unavailable));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn fetch_failure_with_stale_entry_is_unavailable() {
        let issuer = test_issuer();
        let clock = FakeClock::at(base_time());
        let cache = InMemoryJwksCache::new();
        cache.put(&issuer, cached("kid-a", clock.now())).await.unwrap();
        clock.advance(chrono::Duration::seconds(3601));
        let calls = Arc::new(AtomicUsize::new(0));
        let fetcher = FakeFetcher {
            calls: calls.clone(),
            clock: clock.clone(),
            serves_kid: "kid-a",
            fail: true,
            delay: Duration::ZERO,
        };
        let provider = JwksProvider::new(fetcher, cache, clock, Duration::from_secs(3600), Duration::from_secs(30));

        let err = provider.key_for(&issuer, "kid-a").await.unwrap_err();

        assert!(matches!(err, AuthnError::Unavailable), "a stale entry must never be served in place of a failed refetch");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn cooldown_blocked_stale_entry_is_unavailable() {
        let issuer = test_issuer();
        let clock = FakeClock::at(base_time());
        let cache = InMemoryJwksCache::new();
        cache.put(&issuer, cached("kid-old", clock.now())).await.unwrap();
        clock.advance(chrono::Duration::seconds(3601));
        let calls = Arc::new(AtomicUsize::new(0));
        let fetcher = FakeFetcher {
            calls: calls.clone(),
            clock: clock.clone(),
            serves_kid: "kid-old",
            fail: true,
            delay: Duration::ZERO,
        };
        let provider = JwksProvider::new(fetcher, cache, clock.clone(), Duration::from_secs(3600), Duration::from_secs(30));

        let first = provider.key_for(&issuer, "kid-old").await.unwrap_err();
        assert!(matches!(first, AuthnError::Unavailable));
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // Still within the 30s cooldown: the second call must not fetch again. Since the
        // failed refetch never updated the cache, the entry is still stale, so this must
        // stay `Unavailable` rather than falling back to `UnknownKid`.
        clock.advance(chrono::Duration::seconds(5));
        let second = provider.key_for(&issuer, "kid-old").await.unwrap_err();

        assert!(matches!(second, AuthnError::Unavailable));
        assert_eq!(calls.load(Ordering::SeqCst), 1, "the cooldown must suppress the second refetch attempt");
    }

    // ---- extra_ca_bundle_path loading (SMA-558 D4) -------------------------------------------
    // Three distinct failure modes with three distinct operator fixes, so three tests. The
    // certificate-FREE case is the one that does not come free: `from_pem_bundle` returns
    // Ok(vec![]) rather than erroring for any file with no BEGIN CERTIFICATE section, so only
    // an explicit is_empty() check catches it (spec § 2.8).

    fn tmp_file_with(contents: &[u8]) -> tempfile::NamedTempFile {
        use std::io::Write;
        let mut f = tempfile::NamedTempFile::new().expect("temp file");
        f.write_all(contents).expect("write");
        f.flush().expect("flush");
        f
    }

    #[test]
    fn missing_bundle_path_is_a_boot_error() {
        let err = HttpJwksFetcher::new(
            Duration::from_secs(5),
            IdpTls::Verify {
                extra_bundle: Some("/nonexistent/paigasus-sma558/ca.pem"),
            },
        )
        .expect_err("a nonexistent bundle path must fail");

        // Backend, NOT Unavailable: a config fault must be diagnosable, not look like the IdP
        // being down.
        assert!(matches!(err, AuthnError::Backend(_)), "expected Backend, got {err:?}");
        assert!(format!("{err:?}").contains("extra_ca_bundle_path"), "the error must name the config key: {err:?}");
    }

    #[test]
    fn certificate_free_bundle_is_a_boot_error() {
        // A key-only PEM: well-formed, parses cleanly, contains zero CERTIFICATE sections.
        // Without the is_empty() guard this loads silently and adds no anchors at all.
        let f = tmp_file_with(b"-----BEGIN PRIVATE KEY-----\nMC4CAQAwBQYDK2VwBCIEIA==\n-----END PRIVATE KEY-----\n");
        let err = HttpJwksFetcher::new(
            Duration::from_secs(5),
            IdpTls::Verify {
                extra_bundle: Some(f.path().to_str().unwrap()),
            },
        )
        .expect_err("a bundle with no certificates must fail");

        assert!(matches!(err, AuthnError::Backend(_)), "expected Backend, got {err:?}");
        assert!(format!("{err:?}").contains("no PEM certificates"), "the error must say the bundle was empty: {err:?}");
    }

    #[test]
    fn undecodable_bundle_is_a_boot_error() {
        // A well-framed CERTIFICATE section whose body is not valid base64/DER. Unlike the case
        // above this DOES fail inside the PEM/DER decode rather than at the is_empty() guard.
        let f = tmp_file_with(b"-----BEGIN CERTIFICATE-----\n!!!not base64!!!\n-----END CERTIFICATE-----\n");
        let err = HttpJwksFetcher::new(
            Duration::from_secs(5),
            IdpTls::Verify {
                extra_bundle: Some(f.path().to_str().unwrap()),
            },
        )
        .expect_err("an undecodable bundle must fail");

        assert!(matches!(err, AuthnError::Backend(_)), "expected Backend, got {err:?}");
    }

    #[test]
    fn no_bundle_and_accept_invalid_both_build() {
        // The two non-bundle postures must still construct a client.
        HttpJwksFetcher::new(Duration::from_secs(5), IdpTls::Verify { extra_bundle: None }).expect("verify without a bundle builds");
        HttpJwksFetcher::new(Duration::from_secs(5), IdpTls::AcceptInvalid).expect("accept-invalid builds");
    }

    #[tokio::test]
    async fn single_flight_coalesces_concurrent_refetches() {
        let issuer = test_issuer();
        let clock = FakeClock::at(base_time());
        let cache = InMemoryJwksCache::new();
        let calls = Arc::new(AtomicUsize::new(0));
        let fetcher = FakeFetcher {
            calls: calls.clone(),
            clock: clock.clone(),
            serves_kid: "kid-a",
            fail: false,
            delay: Duration::from_millis(50),
        };
        let provider = Arc::new(JwksProvider::new(fetcher, cache, clock, Duration::from_secs(3600), Duration::from_secs(30)));

        let mut tasks = Vec::new();
        for _ in 0..10 {
            let provider = provider.clone();
            let issuer = issuer.clone();
            tasks.push(tokio::spawn(async move { provider.key_for(&issuer, "kid-a").await }));
        }

        for task in tasks {
            let jwk = task.await.unwrap().unwrap();
            assert_eq!(jwk.common.key_id.as_deref(), Some("kid-a"));
        }

        assert_eq!(calls.load(Ordering::SeqCst), 1, "concurrent cold-cache callers must coalesce onto one fetch");
    }
}
