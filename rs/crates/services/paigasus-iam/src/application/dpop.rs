// SPDX-License-Identifier: Apache-2.0

//! `DpopProofVerifier` (SMA-700 D2, § 4.4 checks 10-13, § 4.6, § 4.8): the request checks of a
//! DPoP proof that the gateway forwards in `Introspect`, the `iat` window, the replay store, and
//! the one-time `IsAuthorized` follow-up. Checks 1-9 are behind the `DpopProofChecker` port; this
//! module has no JOSE code. The verifier owns the configured base-URL list: no adapter can pass
//! another one.
//!
//! Logs: one rate-limited `info` line for each refusal, "refused a DPoP proof", with the issuer and
//! the static defect name. A request refusal before the token is verified (§ 4.8, decision P5)
//! logs the issuer `-`. A quota hit writes one rate-limited `info` line, and a full store one
//! rate-limited `warn` line with the entry count. No line carries the proof, the token, the `jti`,
//! the `jkt`, the path or a URL.

use std::sync::Arc;
use std::time::Instant;

use paigasus_iam_core::{AuthnError, Clock, DpopProofChecker, Jkt, MAX_PROOF_BYTES, NewProof, ProofDefect, ProofKey, RecordOutcome, RedeemOutcome, ReplayStore, TokenDefect, ValidatedClaims};
use url::Url;

use crate::application::log_rate_limit::{LOG_RATE_LIMIT_INTERVAL, LogRateLimiter};

/// The largest forwarded HTTP method (§ 4.1).
pub const MAX_METHOD_BYTES: usize = 16;
/// The largest forwarded path (§ 4.1).
pub const MAX_PATH_BYTES: usize = 2048;
/// The follow-up ticket lives at least this long after `record` (§ 4.5).
const MIN_FOLLOW_UP_SECS: i64 = 30;
/// The issuer field of a refusal that happens before the token is verified (decision P5).
const UNVERIFIED_ISSUER: &str = "-";

/// The DPoP context of one `Introspect` (SMA-700 § 4.1). `Debug` prints no field: the proof is
/// a credential and the path is request data (§ 4.8).
#[derive(Clone, PartialEq, Eq)]
pub struct DpopRequest {
    pub proof: String,
    pub method: String,
    pub path: String,
}

impl std::fmt::Debug for DpopRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DpopRequest").finish_non_exhaustive()
    }
}

/// One configured `forwarded_base_urls` entry, parsed at boot (§ 4.6): scheme, host, port (a
/// default port is `None`) and a path prefix with no trailing `/` (empty for the root).
#[derive(Debug, Clone, PartialEq, Eq)]
struct ForwardedBase {
    scheme: String,
    host: String,
    port: Option<u16>,
    prefix: String,
}

impl ForwardedBase {
    fn parse(raw: &str) -> Result<Self, String> {
        let url = Url::parse(raw).map_err(|e| format!("authn.dpop.forwarded_base_urls entry {raw:?} does not parse: {e}"))?;
        let host = url.host_str().ok_or_else(|| format!("authn.dpop.forwarded_base_urls entry {raw:?} has no host"))?.to_owned();
        Ok(ForwardedBase {
            scheme: url.scheme().to_owned(),
            host,
            port: url.port(),
            prefix: url.path().trim_end_matches('/').to_owned(),
        })
    }

    /// The URL that the client signed for `path` through this base, or `None` when the parse
    /// moves the scheme, the host or the port away from the base (§ 4.6).
    fn expected(&self, path: &str) -> Option<Url> {
        let port = self.port.map(|p| format!(":{p}")).unwrap_or_default();
        let url = Url::parse(&format!("{}://{}{}{}{}", self.scheme, self.host, port, self.prefix, path)).ok()?;
        (url.scheme() == self.scheme && url.host_str() == Some(self.host.as_str()) && url.port() == self.port).then_some(url)
    }
}

/// § 4.6: after the same `url` parser, the scheme, the host, the port (a default port removed) and
/// `path()` are equal. The query and the fragment of `htu` are ignored. `path()` keeps the
/// percent-encoding, so a path that differs only by it does not match (a deliberate deviation from
/// RFC 3986 § 6.2.2).
fn same_target(htu: &Url, expected: &Url) -> bool {
    htu.scheme() == expected.scheme() && htu.host_str() == expected.host_str() && htu.port() == expected.port() && htu.path() == expected.path()
}

/// § 4.8: the forwarded path starts with `/`, has no `?`, `#`, `\` or control character, and no
/// segment that is `.` or `..`, also percent-encoded (`%2e`, in any case). So a forwarded path
/// cannot leave the base prefix.
fn path_is_well_formed(path: &str) -> bool {
    path.starts_with('/') && !path.chars().any(|c| matches!(c, '?' | '#' | '\\') || c.is_control()) && !path.split('/').any(is_dot_segment)
}

/// A segment that is `.` or `..` after the percent-encoded dots are decoded.
fn is_dot_segment(segment: &str) -> bool {
    matches!(segment.to_ascii_lowercase().replace("%2e", ".").as_str(), "." | "..")
}

/// The two replay-store events with their own log line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum StoreEvent {
    Quota,
    Capacity,
}

fn first_16(hash: &blake3::Hash) -> [u8; 16] {
    let mut out = [0u8; 16];
    out.copy_from_slice(&hash.as_bytes()[..16]);
    out
}

/// § 4.5: the first 16 bytes of `blake3(jkt || 0x00 || jti)`.
fn proof_key(jkt: &Jkt, jti: &str) -> ProofKey {
    let mut hasher = blake3::Hasher::new();
    hasher.update(jkt.as_str().as_bytes());
    hasher.update(&[0]);
    hasher.update(jti.as_bytes());
    ProofKey(first_16(&hasher.finalize()))
}

/// § 4.5: the first 16 bytes of `blake3(issuer || 0x00 || sub)`.
fn subject_hash(claims: &ValidatedClaims) -> [u8; 16] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(claims.issuer.as_str().as_bytes());
    hasher.update(&[0]);
    hasher.update(claims.subject.as_bytes());
    first_16(&hasher.finalize())
}

fn jkt_hash(jkt: &Jkt) -> [u8; 16] {
    first_16(&blake3::hash(jkt.as_str().as_bytes()))
}

/// § 4.5: `blake3` of the whole proof string as received, with no trim.
fn proof_digest(proof: &str) -> [u8; 32] {
    *blake3::hash(proof.as_bytes()).as_bytes()
}

/// The request checks, the window, the replay store and the follow-up (SMA-700 § 4.4 checks
/// 10-13, § 4.8). One instance per process behind an `Arc`; it shares the one replay store.
pub struct DpopProofVerifier {
    checker: Arc<dyn DpopProofChecker>,
    store: Arc<dyn ReplayStore>,
    clock: Arc<dyn Clock>,
    bases: Vec<ForwardedBase>,
    iat_window_secs: i64,
    refusal_log: LogRateLimiter<ProofDefect>,
    store_log: LogRateLimiter<StoreEvent>,
}

impl DpopProofVerifier {
    /// `IamConfig::validate` has already checked the list and the window, so an `Err` here is a
    /// wiring defect.
    pub fn new(checker: Arc<dyn DpopProofChecker>, store: Arc<dyn ReplayStore>, clock: Arc<dyn Clock>, forwarded_base_urls: &[String], iat_window_secs: u64) -> Result<Self, String> {
        let bases = forwarded_base_urls.iter().map(|raw| ForwardedBase::parse(raw)).collect::<Result<Vec<_>, _>>()?;
        Ok(DpopProofVerifier {
            checker,
            store,
            clock,
            bases,
            iat_window_secs: i64::try_from(iat_window_secs).map_err(|_| "authn.dpop.iat_window_secs is too large".to_string())?,
            refusal_log: LogRateLimiter::new(LOG_RATE_LIMIT_INTERVAL),
            store_log: LogRateLimiter::new(LOG_RATE_LIMIT_INTERVAL),
        })
    }

    /// § 4.8, before the token is verified (decision P5): the sizes, an empty proof, the
    /// forwarded path.
    pub fn check_request(&self, request: &DpopRequest) -> Result<(), AuthnError> {
        let defect = if request.proof.len() > MAX_PROOF_BYTES || request.method.len() > MAX_METHOD_BYTES || request.path.len() > MAX_PATH_BYTES {
            Some(ProofDefect::Malformed)
        } else if request.proof.is_empty() {
            Some(ProofDefect::Missing)
        } else if !path_is_well_formed(&request.path) {
            Some(ProofDefect::Malformed)
        } else {
            None
        };
        match defect {
            Some(defect) => Err(self.refuse(UNVERIFIED_ISSUER, defect)),
            None => Ok(()),
        }
    }

    /// Checks 1-13 for one `Introspect`. `claims` came from `authenticate(.., TokenScheme::Dpop)`.
    /// The replay record is last, so a proof that fails another check uses no `jti` (§ 4.4).
    pub fn verify(&self, claims: &ValidatedClaims, token: &str, request: &DpopRequest) -> Result<(), AuthnError> {
        let issuer = claims.issuer.as_str();
        let jkt = claims.key_binding.as_ref().ok_or(AuthnError::InvalidToken(TokenDefect::NotKeyBound))?;
        let proof = self.checker.check(&request.proof, token, jkt).map_err(|defect| self.refuse(issuer, defect))?;
        if proof.htm != request.method {
            return Err(self.refuse(issuer, ProofDefect::Htm)); // check 10
        }
        if !self.htu_matches(&proof.htu, &request.path) {
            return Err(self.refuse(issuer, ProofDefect::Htu)); // check 11
        }
        let now = self.now();
        let Some(expires_at) = self.iat_window_end(proof.iat, now) else {
            return Err(self.refuse(issuer, ProofDefect::Iat)); // check 12
        };
        let entry = NewProof {
            key: proof_key(jkt, &proof.jti),
            subject: subject_hash(claims),
            jkt: jkt_hash(jkt),
            expires_at,
            follow_up_deadline: expires_at.max(now.saturating_add(MIN_FOLLOW_UP_SECS)),
            follow_up_digest: proof_digest(&request.proof),
        };
        match self.store.record(entry, now) {
            RecordOutcome::Fresh => Ok(()), // check 13
            RecordOutcome::Replayed => Err(self.refuse(issuer, ProofDefect::Replayed)),
            RecordOutcome::QuotaExceeded { retry_after_secs } => {
                if let Some(suppressed) = self.store_log.admit_at(issuer, StoreEvent::Quota, Instant::now()) {
                    tracing::info!(issuer, retry_after_secs, suppressed, "refused a DPoP proof: a key or subject quota of the replay store is full");
                }
                Err(AuthnError::DpopQuotaExceeded { retry_after_secs })
            }
            RecordOutcome::CapacityFull { entries } => {
                if let Some(suppressed) = self.store_log.admit_at(issuer, StoreEvent::Capacity, Instant::now()) {
                    tracing::warn!(issuer, entries, suppressed, "the DPoP replay store is full; raise authn.dpop.replay_capacity");
                }
                Err(AuthnError::Unavailable)
            }
        }
    }

    /// The `IsAuthorized` follow-up (§ 4.8): `jti` and `ath` with no signature check, the `ath` of
    /// this token, then the one-time ticket for the digest of these exact bytes.
    pub fn redeem(&self, claims: &ValidatedClaims, token: &str, proof: &str) -> Result<(), AuthnError> {
        let issuer = claims.issuer.as_str();
        let jkt = claims.key_binding.as_ref().ok_or(AuthnError::InvalidToken(TokenDefect::NotKeyBound))?;
        if proof.is_empty() {
            return Err(self.refuse(issuer, ProofDefect::Missing));
        }
        let follow_up = self.checker.follow_up_claims(proof).map_err(|defect| self.refuse(issuer, defect))?;
        if !self.checker.ath_matches(&follow_up.ath, token) {
            return Err(self.refuse(issuer, ProofDefect::Ath)); // decision P6
        }
        match self.store.redeem_follow_up(proof_key(jkt, &follow_up.jti), proof_digest(proof), self.now()) {
            RedeemOutcome::Redeemed => Ok(()),
            RedeemOutcome::Refused => Err(self.refuse(issuer, ProofDefect::FollowUp)),
        }
    }

    fn now(&self) -> i64 {
        self.clock.now().timestamp()
    }

    /// Check 11: the proof matches when it matches one configured base.
    fn htu_matches(&self, htu: &str, path: &str) -> bool {
        if !path_is_well_formed(path) {
            return false;
        }
        let Ok(htu) = Url::parse(htu) else {
            return false;
        };
        self.bases.iter().filter_map(|base| base.expected(path)).any(|expected| same_target(&htu, &expected))
    }

    /// Check 12: `|now - iat| <= iat_window_secs` with checked arithmetic. `Some(iat + window)`
    /// (the entry's `expires_at`) on a pass.
    fn iat_window_end(&self, iat: i64, now: i64) -> Option<i64> {
        let skew = now.checked_sub(iat)?.checked_abs()?;
        (skew <= self.iat_window_secs).then(|| iat.saturating_add(self.iat_window_secs))
    }

    fn refuse(&self, issuer: &str, defect: ProofDefect) -> AuthnError {
        if let Some(suppressed) = self.refusal_log.admit_at(issuer, defect, Instant::now()) {
            tracing::info!(issuer, defect = defect.as_str(), suppressed, "refused a DPoP proof");
        }
        AuthnError::InvalidDpopProof(defect)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::dpop_replay::InMemoryReplayStore;
    use crate::application::fakes::FixedClock;
    use chrono::{TimeZone, Utc};
    use paigasus_iam_core::{FollowUpClaims, Issuer, ProofClaims};
    use paigasus_logging::test_support::capture_logs;
    use std::sync::Mutex;

    const NOW: i64 = 1_700_000_000;
    const BASE: &str = "https://gw.example.test";
    const URL: &str = "https://gw.example.test/v1/chat/completions";
    const PATH: &str = "/v1/chat/completions";
    const ISSUER: &str = "https://idp.example.test";

    /// A `DpopProofChecker` that returns scripted answers, so each verifier test changes one
    /// field of the claims (checks 1-9 have their own tests in `adapters::oidc::dpop`).
    struct ScriptedChecker {
        check: Mutex<Result<ProofClaims, ProofDefect>>,
        follow_up: Result<FollowUpClaims, ProofDefect>,
        ath_ok: bool,
    }

    impl DpopProofChecker for ScriptedChecker {
        fn check(&self, _proof: &str, _token: &str, _jkt: &Jkt) -> Result<ProofClaims, ProofDefect> {
            self.check.lock().unwrap().clone()
        }
        fn follow_up_claims(&self, _proof: &str) -> Result<FollowUpClaims, ProofDefect> {
            self.follow_up.clone()
        }
        fn ath_matches(&self, _ath: &str, _token: &str) -> bool {
            self.ath_ok
        }
    }

    fn proof_claims(htm: &str, htu: &str, iat: i64) -> ProofClaims {
        ProofClaims {
            jti: "jti-1".into(),
            iat,
            htm: htm.into(),
            htu: htu.into(),
        }
    }

    fn checker(check: Result<ProofClaims, ProofDefect>) -> ScriptedChecker {
        ScriptedChecker {
            check: Mutex::new(check),
            follow_up: Ok(FollowUpClaims {
                jti: "jti-1".into(),
                ath: "ath".into(),
            }),
            ath_ok: true,
        }
    }

    fn verifier_with(checker: ScriptedChecker, bases: &[&str], store: Arc<InMemoryReplayStore>) -> (DpopProofVerifier, FixedClock) {
        let clock = FixedClock::default();
        clock.set(Utc.timestamp_opt(NOW, 0).unwrap());
        let bases: Vec<String> = bases.iter().map(|b| (*b).to_string()).collect();
        let verifier = DpopProofVerifier::new(Arc::new(checker), store, Arc::new(clock.clone()), &bases, 60).expect("test bases parse");
        (verifier, clock)
    }

    fn verifier(check: Result<ProofClaims, ProofDefect>) -> (DpopProofVerifier, FixedClock) {
        verifier_with(checker(check), &[BASE], Arc::new(InMemoryReplayStore::new(100, 100, 100)))
    }

    fn bound(sub: &str, jkt: &str) -> ValidatedClaims {
        ValidatedClaims {
            issuer: Issuer::parse(ISSUER).unwrap(),
            subject: sub.into(),
            audiences: vec!["aud".into()],
            expires_at: Utc.timestamp_opt(NOW + 3600, 0).unwrap(),
            email: None,
            name: None,
            locale: None,
            zoneinfo: None,
            key_binding: Some(Jkt::new(jkt)),
        }
    }

    fn request(path: &str) -> DpopRequest {
        DpopRequest {
            proof: "p.r.oof".into(),
            method: "POST".into(),
            path: path.into(),
        }
    }

    fn defect(result: Result<(), AuthnError>) -> ProofDefect {
        match result {
            Err(AuthnError::InvalidDpopProof(defect)) => defect,
            other => panic!("want InvalidDpopProof, got {other:?}"),
        }
    }

    #[test]
    fn a_valid_proof_for_the_request_passes() {
        let (v, _) = verifier(Ok(proof_claims("POST", URL, NOW)));
        v.verify(&bound("alice", "jkt-a"), "tok", &request(PATH)).expect("passes");
    }

    #[test]
    fn checks_1_to_9_pass_through_their_defect() {
        let (v, _) = verifier(Err(ProofDefect::Thumbprint));
        assert_eq!(defect(v.verify(&bound("alice", "jkt-a"), "tok", &request(PATH))), ProofDefect::Thumbprint);
    }

    #[test]
    fn check_10_htm_is_exact() {
        // § 5.1: `post` against `POST` fails.
        let (v, _) = verifier(Ok(proof_claims("post", URL, NOW)));
        assert_eq!(defect(v.verify(&bound("alice", "jkt-a"), "tok", &request(PATH))), ProofDefect::Htm);
    }

    #[test]
    fn check_11_htu_rules() {
        // § 5.1 `htu` cases. Each (base list, htu, forwarded path, passes).
        let cases: [(&[&str], &str, &str, bool); 11] = [
            (&[BASE], "https://gw.example.test:443/v1/chat/completions", PATH, true),
            (&[BASE], "https://GW.Example.Test/v1/chat/completions", PATH, true),
            (&[BASE], "https://gw.example.test/v1/chat/completions?x=1#f", PATH, true),
            (&["https://edge.example.test/api"], "https://edge.example.test/api/v1/chat/completions", PATH, true),
            (&["https://edge.example.test/api"], "https://edge.example.test/v1/chat/completions", PATH, false),
            (&[BASE], "https://gw.example.test/v1/chat/%63ompletions", PATH, false),
            (&[BASE], "https://gw.example.test/v1/chat/completions/", PATH, false),
            (&[BASE], "https://other.example.test/v1/chat/completions", PATH, false),
            (&[BASE], "http://gw.example.test/v1/chat/completions", PATH, false),
            (&[BASE], "not a url", PATH, false),
            (&["https://one.example.test", BASE], URL, PATH, true),
        ];
        for (bases, htu, path, passes) in cases {
            let store = Arc::new(InMemoryReplayStore::new(100, 100, 100));
            let (v, _) = verifier_with(checker(Ok(proof_claims("POST", htu, NOW))), bases, store);
            let result = v.verify(&bound("alice", "jkt-a"), "tok", &request(path));
            if passes {
                result.unwrap_or_else(|e| panic!("{bases:?} {htu}: want a pass, got {e:?}"));
            } else {
                assert_eq!(defect(result), ProofDefect::Htu, "{bases:?} {htu}");
            }
        }
    }

    #[test]
    fn a_base_url_with_a_trailing_slash_still_matches() {
        // Review Focus 2.
        for (base, htu) in [
            ("https://gw.example.test/", URL),
            ("https://edge.example.test/api/", "https://edge.example.test/api/v1/chat/completions"),
        ] {
            let (v, _) = verifier_with(checker(Ok(proof_claims("POST", htu, NOW))), &[base], Arc::new(InMemoryReplayStore::new(100, 100, 100)));
            v.verify(&bound("alice", "jkt-a"), "tok", &request(PATH)).unwrap_or_else(|e| panic!("{base}: {e:?}"));
        }
    }

    #[test]
    fn an_ipv6_loopback_base_url_matches() {
        // Review Focus 5.
        let htu = "http://[::1]:8088/v1/chat/completions";
        let (v, _) = verifier_with(checker(Ok(proof_claims("POST", htu, NOW))), &["http://[::1]:8088"], Arc::new(InMemoryReplayStore::new(100, 100, 100)));
        v.verify(&bound("alice", "jkt-a"), "tok", &request(PATH)).expect("passes");
    }

    #[test]
    fn check_12_iat_window_edges() {
        for (iat, passes) in [(NOW - 60, true), (NOW + 60, true), (NOW - 61, false), (NOW + 61, false), (i64::MAX, false), (i64::MIN, false)] {
            let (v, _) = verifier(Ok(proof_claims("POST", URL, iat)));
            let result = v.verify(&bound("alice", "jkt-a"), "tok", &request(PATH));
            if passes {
                result.unwrap_or_else(|e| panic!("iat {iat}: {e:?}"));
            } else {
                assert_eq!(defect(result), ProofDefect::Iat, "iat {iat}");
            }
        }
    }

    #[test]
    fn check_13_a_second_use_is_a_replay_and_a_failed_check_uses_no_jti() {
        let store = Arc::new(InMemoryReplayStore::new(100, 100, 100));
        // A proof that fails check 10 records nothing.
        let (bad, _) = verifier_with(checker(Ok(proof_claims("GET", URL, NOW))), &[BASE], store.clone());
        assert_eq!(defect(bad.verify(&bound("alice", "jkt-a"), "tok", &request(PATH))), ProofDefect::Htm);
        let (v, _) = verifier_with(checker(Ok(proof_claims("POST", URL, NOW))), &[BASE], store);
        v.verify(&bound("alice", "jkt-a"), "tok", &request(PATH)).expect("the first use passes");
        assert_eq!(defect(v.verify(&bound("alice", "jkt-a"), "tok", &request(PATH))), ProofDefect::Replayed);
    }

    #[test]
    fn the_same_jti_under_two_keys_is_not_a_replay() {
        // Review Focus 4: the replay key includes the jkt.
        let (v, _) = verifier(Ok(proof_claims("POST", URL, NOW)));
        v.verify(&bound("alice", "jkt-a"), "tok-a", &request(PATH)).expect("key a");
        v.verify(&bound("bob", "jkt-b"), "tok-b", &request(PATH)).expect("key b, same jti");
    }

    #[test]
    fn a_quota_hit_is_dpop_quota_exceeded_and_a_full_store_is_unavailable() {
        let alice = bound("alice", "jkt-a");
        // One store with a key quota of 1, shared by two verifiers whose proofs have two jtis.
        let shared = Arc::new(InMemoryReplayStore::new(100, 1, 100));
        let (first, _) = verifier_with(checker(Ok(proof_claims("POST", URL, NOW))), &[BASE], shared.clone());
        first.verify(&alice, "tok", &request(PATH)).expect("the first proof of jkt-a");
        let second_jti = ScriptedChecker {
            check: Mutex::new(Ok(ProofClaims {
                jti: "jti-2".into(),
                ..proof_claims("POST", URL, NOW)
            })),
            ..checker(Ok(proof_claims("POST", URL, NOW)))
        };
        let (second, _) = verifier_with(second_jti, &[BASE], shared);
        // iat NOW, window 60: the oldest entry goes at NOW + 61, so retry after 61 s.
        assert!(matches!(second.verify(&alice, "tok", &request(PATH)), Err(AuthnError::DpopQuotaExceeded { retry_after_secs: 61 })));

        let (full, _) = verifier_with(checker(Ok(proof_claims("POST", URL, NOW))), &[BASE], Arc::new(InMemoryReplayStore::new(1, 1, 1)));
        full.verify(&alice, "tok", &request(PATH)).expect("fills the store");
        assert!(matches!(full.verify(&bound("bob", "jkt-b"), "tok", &request(PATH)), Err(AuthnError::Unavailable)));
    }

    #[test]
    fn htu_matches_refuses_a_malformed_path() {
        let (v, _) = verifier(Err(ProofDefect::Signature));
        assert!(v.htu_matches(URL, PATH));
        assert!(!v.htu_matches(URL, "/v1/../chat/completions"));
        assert!(!v.htu_matches(URL, "x"));
        // The URL parser folds `/v1/../chat/completions` to `/chat/completions`. Only the guard
        // refuses this pair, so `htu_matches` must hold the guard itself, not only `check_request`.
        assert!(!v.htu_matches("https://gw.example.test/chat/completions", "/v1/../chat/completions"));
    }

    #[test]
    fn the_forwarded_path_must_not_move_the_origin() {
        // § 5.1 forwarded path cases: each gives Malformed, before any proof check.
        let (v, _) = verifier(Err(ProofDefect::Signature));
        for path in [".evil.example/x", "@evil.example/x", ":8443/x", "x", "/v1?x=1", "/v1#f", "/v1\\x", "/v1\u{0}x", "/v1\nx", ""] {
            assert_eq!(defect(v.check_request(&request(path))), ProofDefect::Malformed, "{path:?}");
        }
        for path in ["/../admin", "/v1/./x", "/v1/%2e%2e/x", "/v1/%2E./x", "/v1/.%2e/x", "/v1/%2e./x", "/v1/%2E/x", "/v1/..", "/v1/%2E%2E"] {
            assert_eq!(defect(v.check_request(&request(path))), ProofDefect::Malformed, "{path:?}");
        }
        v.check_request(&request("/v1/a.b/..c/.d/%2e%2e%2e")).expect("dots inside a segment are not dot segments");
        v.check_request(&request(PATH)).expect("a plain path passes");
        v.check_request(&request("//evil.example/x"))
            .expect("a double slash stays on the base host; the host check after the parse holds");
    }

    #[test]
    fn the_request_sizes_and_an_empty_proof() {
        let (v, _) = verifier(Err(ProofDefect::Signature));
        let big_proof = DpopRequest {
            proof: "a".repeat(MAX_PROOF_BYTES + 1),
            ..request(PATH)
        };
        let big_method = DpopRequest {
            method: "M".repeat(MAX_METHOD_BYTES + 1),
            ..request(PATH)
        };
        let big_path = DpopRequest {
            path: format!("/{}", "a".repeat(MAX_PATH_BYTES)),
            ..request(PATH)
        };
        let empty = DpopRequest {
            proof: String::new(),
            ..request(PATH)
        };
        assert_eq!(defect(v.check_request(&big_proof)), ProofDefect::Malformed);
        assert_eq!(defect(v.check_request(&big_method)), ProofDefect::Malformed);
        assert_eq!(defect(v.check_request(&big_path)), ProofDefect::Malformed);
        assert_eq!(defect(v.check_request(&empty)), ProofDefect::Missing);
        let edge = DpopRequest {
            proof: "a".repeat(MAX_PROOF_BYTES),
            method: "M".repeat(MAX_METHOD_BYTES),
            path: format!("/{}", "a".repeat(MAX_PATH_BYTES - 1)),
        };
        v.check_request(&edge).expect("the sizes themselves pass");
    }

    #[test]
    fn the_follow_up_redeems_once_with_the_same_proof_bytes() {
        let (v, _) = verifier(Ok(proof_claims("POST", URL, NOW)));
        let alice = bound("alice", "jkt-a");
        let introspected = request(PATH);
        assert_eq!(defect(v.redeem(&alice, "tok", &introspected.proof)), ProofDefect::FollowUp, "no ticket before Introspect");
        v.verify(&alice, "tok", &introspected).expect("Introspect");
        assert_eq!(defect(v.redeem(&alice, "tok", "other.proof.bytes")), ProofDefect::FollowUp, "other bytes");
        v.redeem(&alice, "tok", &introspected.proof).expect("the follow-up, still redeemable after the wrong digest");
        assert_eq!(defect(v.redeem(&alice, "tok", &introspected.proof)), ProofDefect::FollowUp, "once only");
    }

    #[test]
    fn the_follow_up_checks_ath_and_the_proof_shape() {
        let store = Arc::new(InMemoryReplayStore::new(100, 100, 100));
        let (v, _) = verifier_with(
            ScriptedChecker {
                ath_ok: false,
                ..checker(Ok(proof_claims("POST", URL, NOW)))
            },
            &[BASE],
            store,
        );
        assert_eq!(defect(v.redeem(&bound("alice", "jkt-a"), "tok", "p.r.oof")), ProofDefect::Ath);
        let (v, _) = verifier_with(
            ScriptedChecker {
                follow_up: Err(ProofDefect::Malformed),
                ..checker(Ok(proof_claims("POST", URL, NOW)))
            },
            &[BASE],
            Arc::new(InMemoryReplayStore::new(100, 100, 100)),
        );
        assert_eq!(defect(v.redeem(&bound("alice", "jkt-a"), "tok", "p.r.oof")), ProofDefect::Malformed);
        assert_eq!(defect(v.redeem(&bound("alice", "jkt-a"), "tok", "")), ProofDefect::Missing);
    }

    #[test]
    fn the_follow_up_ticket_expires() {
        let (v, clock) = verifier(Ok(proof_claims("POST", URL, NOW)));
        let alice = bound("alice", "jkt-a");
        v.verify(&alice, "tok", &request(PATH)).expect("Introspect at NOW");
        // follow_up_deadline = max(NOW + 60, NOW + 30) = NOW + 60.
        clock.set(Utc.timestamp_opt(NOW + 61, 0).unwrap());
        assert_eq!(defect(v.redeem(&alice, "tok", &request(PATH).proof)), ProofDefect::FollowUp);
    }

    #[test]
    fn a_refusal_logs_the_issuer_and_the_static_defect_only() {
        let (logs, _guard) = capture_logs();
        let (v, _) = verifier(Ok(proof_claims("POST", "https://other.example.test/secret-path", NOW)));
        let _ = v.verify(
            &bound("alice-subject", "jkt-secret"),
            "token-secret",
            &DpopRequest {
                proof: "proof-secret".into(),
                ..request("/v1/secret-path")
            },
        );
        let text = logs.text();
        let lines: Vec<&str> = text.lines().filter(|l| l.contains("refused a DPoP proof")).collect();
        assert_eq!(lines.len(), 1, "{text}");
        assert!(lines[0].contains("INFO") && lines[0].contains(ISSUER) && lines[0].contains("htu"), "{}", lines[0]);
        for secret in ["alice-subject", "jkt-secret", "token-secret", "proof-secret", "secret-path", "jti-1", "other.example.test"] {
            assert!(!text.contains(secret), "the log must not contain {secret:?}:\n{text}");
        }
    }

    #[test]
    fn a_full_store_writes_a_warn_line_with_the_entry_count() {
        let (logs, _guard) = capture_logs();
        let (v, _) = verifier_with(checker(Ok(proof_claims("POST", URL, NOW))), &[BASE], Arc::new(InMemoryReplayStore::new(1, 1, 1)));
        v.verify(&bound("alice", "jkt-a"), "tok", &request(PATH)).expect("fills the store");
        let _ = v.verify(&bound("bob", "jkt-b"), "tok", &request(PATH));
        let text = logs.text();
        let line = text.lines().find(|l| l.contains("the DPoP replay store is full")).unwrap_or_else(|| panic!("{text}"));
        assert!(line.contains("WARN") && line.contains("entries=1"), "{line}");
    }

    #[test]
    fn new_refuses_a_base_url_it_cannot_parse() {
        let bases = vec!["not a url".to_string()];
        let err = DpopProofVerifier::new(
            Arc::new(checker(Err(ProofDefect::Signature))),
            Arc::new(InMemoryReplayStore::new(1, 1, 1)),
            Arc::new(FixedClock::default()),
            &bases,
            60,
        )
        .err()
        .expect("an unparseable base URL is a wiring defect");
        assert!(err.contains("forwarded_base_urls"), "{err}");
    }

    #[test]
    fn dpop_request_debug_prints_no_field() {
        let printed = format!(
            "{:?}",
            DpopRequest {
                proof: "proof-secret".into(),
                method: "POST".into(),
                path: "/secret".into()
            }
        );
        assert_eq!(printed, "DpopRequest { .. }");
    }

    #[test]
    fn the_follow_up_floor_is_thirty_seconds_after_the_record() {
        // PF-C3: iat = NOW - 60 gives expires_at = NOW, so the floor (NOW + 30) is the deadline.
        let (v, clock) = verifier(Ok(proof_claims("POST", URL, NOW - 60)));
        let alice = bound("alice", "jkt-a");
        v.verify(&alice, "tok", &request(PATH)).expect("Introspect at expires_at");
        clock.set(Utc.timestamp_opt(NOW + 30, 0).unwrap());
        v.redeem(&alice, "tok", &request(PATH).proof).expect("30 s later still redeems");

        let (v, clock) = verifier(Ok(proof_claims("POST", URL, NOW - 60)));
        v.verify(&alice, "tok", &request(PATH)).expect("Introspect at expires_at");
        clock.set(Utc.timestamp_opt(NOW + 31, 0).unwrap());
        assert_eq!(defect(v.redeem(&alice, "tok", &request(PATH).proof)), ProofDefect::FollowUp, "31 s later is refused");
    }

    #[test]
    fn the_post_parse_check_refuses_a_path_that_moves_the_authority() {
        // PF-C4: these paths skip the `starts_with('/')` pre-check by calling `expected` directly.
        // Only the equality check after the parse refuses them.
        let base = ForwardedBase::parse(BASE).unwrap();
        for path in [".evil.example/x", "@evil.example/x", ":8443/x"] {
            assert!(base.expected(path).is_none(), "{path:?} must be refused by the post-parse check");
        }
        assert!(base.expected(PATH).is_some(), "a plain path is built");
        let with_port = ForwardedBase::parse("https://gw.example.test:8443").unwrap();
        assert!(with_port.expected("@evil.example/x").is_none());
        assert!(with_port.expected(PATH).is_some());
    }

    #[test]
    fn a_path_that_moves_the_authority_never_matches_the_htu_it_builds() {
        // PF-C4 end to end: a proof signed for the moved host is refused as Htu, not accepted.
        for (path, htu) in [
            (".evil.example/x", "https://gw.example.test.evil.example/x"),
            ("@evil.example/x", "https://evil.example/x"),
            (":8443/x", "https://gw.example.test:8443/x"),
        ] {
            let (v, _) = verifier(Ok(proof_claims("POST", htu, NOW)));
            assert_eq!(defect(v.verify(&bound("alice", "jkt-a"), "tok", &request(path))), ProofDefect::Htu, "{path:?}");
        }
    }
}
