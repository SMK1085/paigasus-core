// SPDX-License-Identifier: Apache-2.0

//! The `Authenticator` v1 implementation (spec §4.1): a provider-agnostic OIDC access token
//! validator. Pipeline: length cap -> header decode + alg allowlist + `kid` presence ->
//! unverified `iss` read -> exact issuer match -> JWKS `kid` lookup -> JWK/alg family
//! consistency -> signature + claims validation (issuer/audience/expiry) -> `ValidatedClaims`.
//! Never logs token or claim material (`TokenDefect` itself carries no payload).

use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Utc};
use jsonwebtoken::errors::ErrorKind;
use jsonwebtoken::jwk::{AlgorithmParameters, Jwk};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use paigasus_iam_core::{Authenticator, AuthnError, Clock, Issuer, TokenDefect, ValidatedClaims};
use serde::Deserialize;

use crate::adapters::oidc::jwks::{JwksCache, JwksFetcher, JwksProvider};
use crate::config::IssuerConfig;

/// Algorithms this validator accepts (spec §4.1) — RSA/EC signature algorithms only.
/// Deliberately excludes HMAC (a shared-secret alg would let anyone holding a *public*
/// verification artifact sign forged tokens) and `none` (which `jsonwebtoken` doesn't even
/// model as an `Algorithm` variant).
const ALLOWED_ALGORITHMS: [Algorithm; 2] = [Algorithm::RS256, Algorithm::ES256];

/// The `Authenticator` v1 implementation: validates a presented bearer token against a
/// fixed, operator-configured set of OIDC issuers (spec §4.1). Generic-by-value over its
/// `JwksProvider`'s three collaborators (fetcher/cache/clock), mirroring the provider's own
/// composition convention — the concrete adapters are chosen once at the composition root
/// (Task 14), not boxed as trait objects here.
pub struct OidcAuthenticator<F: JwksFetcher, K: JwksCache, C: Clock> {
    issuers: Vec<IssuerConfig>,
    provider: JwksProvider<F, K, C>,
    leeway_secs: u64,
    max_token_bytes: usize,
}

impl<F: JwksFetcher, K: JwksCache, C: Clock> OidcAuthenticator<F, K, C> {
    pub fn new(issuers: Vec<IssuerConfig>, provider: JwksProvider<F, K, C>, leeway_secs: u64, max_token_bytes: usize) -> Self {
        Self {
            issuers,
            provider,
            leeway_secs,
            max_token_bytes,
        }
    }

    /// Exact string match against the configured issuer list (spec §3.1's "no
    /// normalization" rule applies here too — this is compared byte-for-byte, same as
    /// `Issuer::parse`'s own equality semantics).
    fn find_issuer_config(&self, iss: &str) -> Option<&IssuerConfig> {
        self.issuers.iter().find(|cfg| cfg.issuer == iss)
    }
}

fn invalid(defect: TokenDefect) -> AuthnError {
    AuthnError::InvalidToken(defect)
}

/// The `iss` claim, read WITHOUT verifying the token's signature — used only to pick which
/// issuer's JWKS to check against (spec §4.1). Nothing else is ever read from this
/// unverified payload, and the payload/token is never logged (it's attacker-controlled
/// prior to signature verification).
#[derive(Deserialize)]
struct UnverifiedIss {
    iss: String,
}

fn read_unverified_issuer(token: &str) -> Result<String, AuthnError> {
    let mut parts = token.split('.');
    let (Some(_header), Some(payload)) = (parts.next(), parts.next()) else {
        return Err(invalid(TokenDefect::Malformed));
    };
    let decoded = URL_SAFE_NO_PAD.decode(payload).map_err(|_| invalid(TokenDefect::Malformed))?;
    let unverified: UnverifiedIss = serde_json::from_slice(&decoded).map_err(|_| invalid(TokenDefect::Malformed))?;
    Ok(unverified.iss)
}

/// `aud` per RFC 7519 §4.1.3 may be encoded as a single string or an array of strings.
#[derive(Deserialize)]
#[serde(untagged)]
enum WireAudience {
    Single(String),
    Multiple(Vec<String>),
}

impl WireAudience {
    fn into_vec(self) -> Vec<String> {
        match self {
            WireAudience::Single(aud) => vec![aud],
            WireAudience::Multiple(auds) => auds,
        }
    }
}

/// The claims this validator reads off a token, deserialized only AFTER `jsonwebtoken` has
/// verified the signature (spec §4.1). `sub`/`exp`/`aud` are required — their absence (or a
/// wrong-shaped value) is a serde failure, which `map_jwt_error` collapses to `Malformed`;
/// the profile claims are optional since an IdP may omit any of them.
#[derive(Deserialize)]
struct WireClaims {
    sub: String,
    exp: u64,
    aud: WireAudience,
    email: Option<String>,
    name: Option<String>,
    locale: Option<String>,
    zoneinfo: Option<String>,
}

/// Maps a `jsonwebtoken` decode/validation failure to a `TokenDefect` (spec §4.1). Every
/// kind this validator doesn't specifically distinguish (bad base64, malformed JSON, a
/// wrong-shaped claim, an unhandled `ErrorKind`) collapses to `Malformed`.
fn map_jwt_error(err: jsonwebtoken::errors::Error) -> AuthnError {
    match err.into_kind() {
        ErrorKind::ExpiredSignature => invalid(TokenDefect::Expired),
        ErrorKind::ImmatureSignature => invalid(TokenDefect::NotYetValid),
        ErrorKind::InvalidSignature => invalid(TokenDefect::BadSignature),
        ErrorKind::InvalidAudience => invalid(TokenDefect::AudienceMismatch),
        ErrorKind::InvalidIssuer => invalid(TokenDefect::IssuerNotConfigured),
        _ => invalid(TokenDefect::Malformed),
    }
}

/// The JWK's key type must match the family the header's algorithm belongs to (an RSA key
/// can't produce an ES256 signature, and vice versa). A mismatch here means this JWKS entry
/// could never have produced the token's signature — the same "this alg isn't usable" shape
/// as an unsupported algorithm, hence the shared `UnsupportedAlg` defect (spec §4.1 D-note).
fn check_kty_matches_alg(jwk: &Jwk, alg: Algorithm) -> Result<(), AuthnError> {
    let consistent = matches!(
        (&jwk.algorithm, alg),
        (AlgorithmParameters::RSA(_), Algorithm::RS256) | (AlgorithmParameters::EllipticCurve(_), Algorithm::ES256)
    );
    if consistent { Ok(()) } else { Err(invalid(TokenDefect::UnsupportedAlg)) }
}

#[async_trait]
impl<F: JwksFetcher, K: JwksCache, C: Clock> Authenticator for OidcAuthenticator<F, K, C> {
    async fn authenticate(&self, token: &str) -> Result<ValidatedClaims, AuthnError> {
        // 1. Length cap — before any parsing at all.
        if token.len() > self.max_token_bytes {
            return Err(invalid(TokenDefect::Oversized));
        }

        // 2. Header decode + alg allowlist + kid presence — header-only checks, no I/O and
        // no unverified-payload reads yet.
        let header = decode_header(token).map_err(|_| invalid(TokenDefect::Malformed))?;
        if !ALLOWED_ALGORITHMS.contains(&header.alg) {
            return Err(invalid(TokenDefect::UnsupportedAlg));
        }
        let kid = header.kid.ok_or_else(|| invalid(TokenDefect::UnknownKid))?;

        // 3. Unverified `iss` read, then exact match against configured issuers.
        let unverified_iss = read_unverified_issuer(token)?;
        let issuer_config = self.find_issuer_config(&unverified_iss).ok_or_else(|| invalid(TokenDefect::IssuerNotConfigured))?;
        let issuer = Issuer::parse(&issuer_config.issuer).map_err(|_| invalid(TokenDefect::IssuerNotConfigured))?;

        // 4. JWKS lookup + kty/alg consistency.
        let jwk = self.provider.key_for(&issuer, &kid).await?;
        check_kty_matches_alg(&jwk, header.alg)?;
        let decoding_key = DecodingKey::from_jwk(&jwk).map_err(|_| invalid(TokenDefect::BadSignature))?;

        // 5. Signature + claims validation, pinned to exactly the header's algorithm.
        let mut validation = Validation::new(header.alg);
        validation.set_issuer(&[issuer.as_str()]);
        validation.set_audience(&issuer_config.audiences);
        validation.leeway = self.leeway_secs;
        validation.validate_nbf = true;

        let token_data = decode::<WireClaims>(token, &decoding_key, &validation).map_err(map_jwt_error)?;

        let expires_at = i64::try_from(token_data.claims.exp)
            .ok()
            .and_then(|secs| DateTime::<Utc>::from_timestamp(secs, 0))
            .ok_or_else(|| invalid(TokenDefect::Malformed))?;

        Ok(ValidatedClaims {
            issuer,
            subject: token_data.claims.sub,
            audiences: token_data.claims.aud.into_vec(),
            expires_at,
            email: token_data.claims.email,
            name: token_data.claims.name,
            locale: token_data.claims.locale,
            zoneinfo: token_data.claims.zoneinfo,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::clock::SystemClock;
    use crate::adapters::oidc::jwks::{CachedJwks, InMemoryJwksCache};
    use jsonwebtoken::EncodingKey;
    use jsonwebtoken::jwk::{CommonParameters, EllipticCurve, EllipticCurveKeyParameters, EllipticCurveKeyType, JwkSet, KeyAlgorithm};
    use p256::elliptic_curve::rand_core::OsRng;
    use p256::elliptic_curve::sec1::ToEncodedPoint;
    use p256::pkcs8::{EncodePrivateKey, LineEnding};
    use serde::Serialize;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    /// Mints a runtime EC P-256 keypair (spec §8 mock-IdP refinement: no committed
    /// PEM/JWK fixtures, no `rsa` crate — RS256's accept path is covered end-to-end by the
    /// Keycloak integration test instead). Returns the signing key, the corresponding
    /// public JWK, and a fixed `kid` tying the two together.
    fn es256_keypair() -> (EncodingKey, Jwk, String) {
        let secret_key = p256::SecretKey::random(&mut OsRng);
        let pem = secret_key.to_pkcs8_pem(LineEnding::LF).expect("valid pkcs8 pem");
        let encoding_key = EncodingKey::from_ec_pem(pem.as_bytes()).expect("valid ec pem");

        let encoded_point = secret_key.public_key().to_encoded_point(false);
        let x = URL_SAFE_NO_PAD.encode(encoded_point.x().expect("uncompressed point has x"));
        let y = URL_SAFE_NO_PAD.encode(encoded_point.y().expect("uncompressed point has y"));

        let kid = "test-es256-key".to_string();
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

    #[derive(Serialize)]
    struct TestClaims {
        iss: String,
        sub: String,
        aud: String,
        exp: i64,
        #[serde(skip_serializing_if = "Option::is_none")]
        nbf: Option<i64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        email: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        locale: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        zoneinfo: Option<String>,
    }

    fn bare_claims(iss: &str, aud: &str, exp: i64) -> TestClaims {
        TestClaims {
            iss: iss.to_string(),
            sub: "sub-1".to_string(),
            aud: aud.to_string(),
            exp,
            nbf: None,
            email: None,
            name: None,
            locale: None,
            zoneinfo: None,
        }
    }

    fn sign(encoding_key: &EncodingKey, kid: Option<&str>, claims: &TestClaims) -> String {
        let mut header = jsonwebtoken::Header::new(Algorithm::ES256);
        header.kid = kid.map(str::to_string);
        jsonwebtoken::encode(&header, claims, encoding_key).expect("signing a test token")
    }

    /// Crafts a token from a raw header JSON string (bypassing `jsonwebtoken::Header`
    /// entirely, since it can't represent an unsupported `alg` like `"none"`) plus a
    /// minimal, otherwise-valid payload. The signature segment is never checked by the
    /// pipeline stages these tokens exercise (both are rejected before signature
    /// verification), so it's a fixed placeholder.
    fn manual_token(header_json: &str) -> String {
        let header_b64 = URL_SAFE_NO_PAD.encode(header_json.as_bytes());
        let payload = serde_json::json!({
            "iss": "https://idp.example.com",
            "sub": "sub-x",
            "aud": "aud",
            "exp": 9_999_999_999i64,
        });
        let payload_b64 = URL_SAFE_NO_PAD.encode(payload.to_string().as_bytes());
        format!("{header_b64}.{payload_b64}.deadbeef")
    }

    /// Stub `JwksFetcher`: serves a fixed `Jwk` and counts calls, so tests can assert the
    /// validator short-circuits before ever reaching the JWKS layer (no real HTTP).
    #[derive(Clone)]
    struct StubFetcher {
        calls: Arc<AtomicUsize>,
        jwks: JwkSet,
    }

    impl StubFetcher {
        fn new(jwk: Jwk) -> Self {
            StubFetcher {
                calls: Arc::new(AtomicUsize::new(0)),
                jwks: JwkSet { keys: vec![jwk] },
            }
        }
    }

    #[async_trait]
    impl JwksFetcher for StubFetcher {
        async fn fetch(&self, _issuer: &Issuer) -> Result<CachedJwks, AuthnError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(CachedJwks {
                jwks: self.jwks.clone(),
                jwks_uri: "https://idp.example.com/jwks".to_string(),
                fetched_at: Utc::now(),
            })
        }
    }

    fn make_authenticator(fetcher: StubFetcher, issuers: Vec<IssuerConfig>, leeway_secs: u64, max_token_bytes: usize) -> OidcAuthenticator<StubFetcher, InMemoryJwksCache, SystemClock> {
        let provider = JwksProvider::new(fetcher, InMemoryJwksCache::new(), SystemClock, Duration::from_secs(3600), Duration::from_secs(30));
        OidcAuthenticator::new(issuers, provider, leeway_secs, max_token_bytes)
    }

    fn issuer_config(issuer: &str, audiences: &[&str]) -> IssuerConfig {
        IssuerConfig {
            issuer: issuer.to_string(),
            audiences: audiences.iter().map(|a| (*a).to_string()).collect(),
            jit_provisioning: true,
        }
    }

    #[tokio::test]
    async fn valid_es256_token_yields_claims() {
        let (encoding_key, jwk, kid) = es256_keypair();
        let issuer = "https://idp.example.com";
        let now = Utc::now().timestamp();
        let claims = TestClaims {
            iss: issuer.to_string(),
            sub: "sub-1".to_string(),
            aud: "my-aud".to_string(),
            exp: now + 3600,
            nbf: None,
            email: Some("alice@example.com".to_string()),
            name: Some("Alice".to_string()),
            locale: Some("en-US".to_string()),
            zoneinfo: Some("America/Los_Angeles".to_string()),
        };
        let token = sign(&encoding_key, Some(&kid), &claims);

        let fetcher = StubFetcher::new(jwk);
        let authenticator = make_authenticator(fetcher, vec![issuer_config(issuer, &["my-aud"])], 60, 16_384);

        let validated = authenticator.authenticate(&token).await.expect("a well-formed, correctly signed token must authenticate");

        assert_eq!(validated.issuer.as_str(), issuer);
        assert_eq!(validated.subject, "sub-1");
        assert_eq!(validated.audiences, vec!["my-aud".to_string()]);
        assert_eq!(validated.email.as_deref(), Some("alice@example.com"));
        assert_eq!(validated.name.as_deref(), Some("Alice"));
        assert_eq!(validated.locale.as_deref(), Some("en-US"));
        assert_eq!(validated.zoneinfo.as_deref(), Some("America/Los_Angeles"));
        assert_eq!(validated.expires_at.timestamp(), now + 3600);
    }

    #[tokio::test]
    async fn alg_none_and_hs256_rejected_before_key_lookup() {
        let (_encoding_key, jwk, _kid) = es256_keypair();
        let fetcher = StubFetcher::new(jwk);
        let calls = fetcher.calls.clone();
        let authenticator = make_authenticator(fetcher, vec![issuer_config("https://idp.example.com", &["aud"])], 60, 16_384);

        let none_token = manual_token(r#"{"alg":"none","typ":"JWT"}"#);
        let err = authenticator.authenticate(&none_token).await.unwrap_err();
        assert!(matches!(err, AuthnError::InvalidToken(_)), "an alg=none token must be rejected");

        let hs256_token = manual_token(r#"{"alg":"HS256","typ":"JWT","kid":"whatever"}"#);
        let err = authenticator.authenticate(&hs256_token).await.unwrap_err();
        assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::UnsupportedAlg)), "an alg=HS256 token must be UnsupportedAlg");

        assert_eq!(calls.load(Ordering::SeqCst), 0, "a rejected alg must never reach the JWKS fetcher");
    }

    #[tokio::test]
    async fn unconfigured_issuer_rejected() {
        let (encoding_key, jwk, kid) = es256_keypair();
        let now = Utc::now().timestamp();
        let token = sign(&encoding_key, Some(&kid), &bare_claims("https://idp.example.com", "aud", now + 3600));

        let fetcher = StubFetcher::new(jwk);
        let calls = fetcher.calls.clone();
        // Configured issuer is a DIFFERENT issuer than the token's `iss`.
        let authenticator = make_authenticator(fetcher, vec![issuer_config("https://other-idp.example.com", &["aud"])], 60, 16_384);

        let err = authenticator.authenticate(&token).await.unwrap_err();
        assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::IssuerNotConfigured)));
        assert_eq!(calls.load(Ordering::SeqCst), 0, "an unconfigured issuer must never reach the JWKS fetcher");
    }

    #[tokio::test]
    async fn audience_mismatch_rejected() {
        let (encoding_key, jwk, kid) = es256_keypair();
        let issuer = "https://idp.example.com";
        let now = Utc::now().timestamp();
        let token = sign(&encoding_key, Some(&kid), &bare_claims(issuer, "wrong-aud", now + 3600));

        let fetcher = StubFetcher::new(jwk);
        let authenticator = make_authenticator(fetcher, vec![issuer_config(issuer, &["expected-aud"])], 60, 16_384);

        let err = authenticator.authenticate(&token).await.unwrap_err();
        assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::AudienceMismatch)));
    }

    #[tokio::test]
    async fn expired_token_rejected_and_leeway_honored() {
        let issuer = "https://idp.example.com";
        let now = Utc::now().timestamp();

        // Expired 30s ago, but a 60s leeway is configured -> still accepted.
        let (encoding_key, jwk, kid) = es256_keypair();
        let ok_token = sign(&encoding_key, Some(&kid), &bare_claims(issuer, "aud", now - 30));
        let ok_authenticator = make_authenticator(StubFetcher::new(jwk.clone()), vec![issuer_config(issuer, &["aud"])], 60, 16_384);
        ok_authenticator.authenticate(&ok_token).await.expect("a 30s-expired token within a 60s leeway must be accepted");

        // Expired 120s ago -> beyond the same 60s leeway, must be rejected.
        let expired_token = sign(&encoding_key, Some(&kid), &bare_claims(issuer, "aud", now - 120));
        let expired_authenticator = make_authenticator(StubFetcher::new(jwk), vec![issuer_config(issuer, &["aud"])], 60, 16_384);
        let err = expired_authenticator.authenticate(&expired_token).await.unwrap_err();
        assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::Expired)));
    }

    #[tokio::test]
    async fn oversized_token_rejected() {
        let (encoding_key, jwk, kid) = es256_keypair();
        let issuer = "https://idp.example.com";
        let now = Utc::now().timestamp();
        let token = sign(&encoding_key, Some(&kid), &bare_claims(issuer, "aud", now + 3600));

        let fetcher = StubFetcher::new(jwk);
        let calls = fetcher.calls.clone();
        let authenticator = make_authenticator(fetcher, vec![issuer_config(issuer, &["aud"])], 60, token.len() - 1);

        let err = authenticator.authenticate(&token).await.unwrap_err();
        assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::Oversized)));
        assert_eq!(calls.load(Ordering::SeqCst), 0, "an oversized token must be rejected before any key lookup");
    }

    #[tokio::test]
    async fn missing_kid_is_unknown_kid() {
        let (encoding_key, jwk, _kid) = es256_keypair();
        let issuer = "https://idp.example.com";
        let now = Utc::now().timestamp();
        let token = sign(&encoding_key, None, &bare_claims(issuer, "aud", now + 3600));

        let fetcher = StubFetcher::new(jwk);
        let calls = fetcher.calls.clone();
        let authenticator = make_authenticator(fetcher, vec![issuer_config(issuer, &["aud"])], 60, 16_384);

        let err = authenticator.authenticate(&token).await.unwrap_err();
        assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::UnknownKid)));
        assert_eq!(calls.load(Ordering::SeqCst), 0, "a missing kid must be rejected before any key lookup");
    }

    #[tokio::test]
    async fn not_yet_valid_token_rejected_and_leeway_honored() {
        let issuer = "https://idp.example.com";
        let now = Utc::now().timestamp();
        let (encoding_key, jwk, kid) = es256_keypair();

        // `nbf` 30s in the future, but a 60s leeway is configured -> still accepted
        // (jsonwebtoken applies the same leeway to `nbf` as it does to `exp`).
        let mut ok_claims = bare_claims(issuer, "aud", now + 3600);
        ok_claims.nbf = Some(now + 30);
        let ok_token = sign(&encoding_key, Some(&kid), &ok_claims);
        let ok_authenticator = make_authenticator(StubFetcher::new(jwk.clone()), vec![issuer_config(issuer, &["aud"])], 60, 16_384);
        ok_authenticator.authenticate(&ok_token).await.expect("an nbf 30s in the future within a 60s leeway must be accepted");

        // `nbf` 120s in the future -> beyond the same 60s leeway, must be rejected.
        let mut future_claims = bare_claims(issuer, "aud", now + 3600);
        future_claims.nbf = Some(now + 120);
        let future_token = sign(&encoding_key, Some(&kid), &future_claims);
        let future_authenticator = make_authenticator(StubFetcher::new(jwk), vec![issuer_config(issuer, &["aud"])], 60, 16_384);
        let err = future_authenticator.authenticate(&future_token).await.unwrap_err();
        assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::NotYetValid)));
    }

    #[tokio::test]
    async fn wrong_signing_key_under_the_same_kid_is_bad_signature() {
        // Two DIFFERENT keypairs — `es256_keypair()` always tags its JWK with the same fixed
        // `kid`, so signing with keypair A but serving keypair B's JWK reaches signature
        // verification (kid lookup and kty/alg consistency both succeed) and must fail there,
        // not earlier in the pipeline.
        let (encoding_key_a, _jwk_a, kid) = es256_keypair();
        let (_encoding_key_b, jwk_b, _kid_b) = es256_keypair();
        let issuer = "https://idp.example.com";
        let now = Utc::now().timestamp();
        let token = sign(&encoding_key_a, Some(&kid), &bare_claims(issuer, "aud", now + 3600));

        let fetcher = StubFetcher::new(jwk_b);
        let authenticator = make_authenticator(fetcher, vec![issuer_config(issuer, &["aud"])], 60, 16_384);

        let err = authenticator.authenticate(&token).await.unwrap_err();
        assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::BadSignature)));
    }
}
