// SPDX-License-Identifier: Apache-2.0

//! The `Authenticator` v1 implementation (spec §4.1): a provider-agnostic OIDC access token
//! validator. Pipeline: length cap -> header decode + alg allowlist + `kid` presence ->
//! unverified `iss` read -> exact issuer match -> JWKS `kid` lookup -> JWK/alg family
//! consistency -> signature + claims validation (issuer/audience/expiry) -> payload `typ`
//! check -> sender-constraint check -> `ValidatedClaims`. The token-type check (SMA-686) refuses
//! an ID token or a logout token: a Keycloak payload `typ` (`ID`, `Logout`) or a standard
//! back-channel logout marker (header `typ: logout+jwt`, the `events` member). After those
//! markers, it refuses a token that carries a claim the operator named for the issuer in
//! `id_token_marker_claims` (SMA-703). Then it refuses a token that does not carry a claim the
//! operator named for the issuer in `access_token_required_claims` (SMA-731). For an issuer with
//! either list, the payload is also decoded as a map that refuses a repeated top-level member
//! (`StrictPayload`); `ClaimRules` holds the two lists. The
//! sender-constraint check (SMA-690) refuses a token bound to a key (a `cnf` claim, or a Keycloak
//! payload `typ: DPoP`), because IAM cannot check the binding on that scheme. On the DPoP scheme (SMA-700) step 7 instead requires a `cnf.jkt` binding and returns it; the proof check is the caller's job (`application::dpop`).
//! Four defects are logged, rate-limited (`log_refusal`); `NotAnAccessToken` has two messages, one
//! for a marker and one for a missing required claim. Never logs token or claim material
//! (`TokenDefect` itself carries no payload).

use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Utc};
use jsonwebtoken::errors::ErrorKind;
use jsonwebtoken::jwk::{AlgorithmParameters, Jwk};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use paigasus_iam_core::{Authenticator, AuthnError, Clock, Issuer, Jkt, TokenDefect, TokenScheme, ValidatedClaims};
use serde::Deserialize;
use std::time::Instant;

use crate::adapters::oidc::jwks::{JwksCache, JwksFetcher, JwksProvider};
use crate::application::log_rate_limit::{LOG_RATE_LIMIT_INTERVAL, LogRateLimiter};
use crate::config::IssuerConfig;

/// Algorithms this validator accepts (spec §4.1) — RSA/EC signature algorithms only.
/// Deliberately excludes HMAC (a shared-secret alg would let anyone holding a *public*
/// verification artifact sign forged tokens) and `none` (which `jsonwebtoken` doesn't even
/// model as an `Algorithm` variant).
const ALLOWED_ALGORITHMS: [Algorithm; 2] = [Algorithm::RS256, Algorithm::ES256];

/// Payload `typ` values that mark a token as NOT an access token (SMA-686 spec D2). Keycloak sets
/// `ID` on its ID token and `Logout` on its back-channel logout token; its access token carries
/// `Bearer`. A Keycloak DPoP-bound access token carries `DPoP`, which step 7 refuses (SMA-690).
/// Compared ASCII case-insensitively. A denylist, not an allowlist: an IdP that sets no `typ`
/// (Dex, measured) must keep working (spec D1).
const NON_ACCESS_TOKEN_TYPES: [&str; 2] = ["ID", "Logout"];

/// Payload `typ` values that mark a sender-constrained token (SMA-690 D3). Keycloak sets `DPoP` on
/// a DPoP-bound access token (measured, SMA-690 measurements M2). Compared ASCII
/// case-insensitively.
const SENDER_CONSTRAINED_TYPES: [&str; 1] = ["DPoP"];

/// Header `typ` values of a back-channel logout token (OIDC Back-Channel Logout 1.0 § 2.4; the
/// `application/` form per RFC 8725 § 3.11). Compared ASCII case-insensitively (SMA-686 D12).
const LOGOUT_TOKEN_HEADER_TYPES: [&str; 2] = ["logout+jwt", "application/logout+jwt"];

/// The `events` member every back-channel logout token carries (OIDC Back-Channel Logout 1.0
/// § 2.4). No access token carries it (SMA-686 D12).
const BACKCHANNEL_LOGOUT_EVENT: &str = "http://schemas.openid.net/event/backchannel-logout";

/// What a refusal log line names besides the issuer (SMA-686 D8, D11). Static or configured
/// values only — never a token claim. `Debug` and `PartialEq` serve the `ClaimRules` unit tests.
#[derive(Debug, PartialEq, Eq)]
enum RefusalDetail<'a> {
    /// The static marker that shows a verified token is not an access token.
    Marker(&'static str),
    /// The CONFIGURED audiences of the issuer.
    Accepted(&'a [String]),
    /// The static marker that shows a verified token is bound to a key (SMA-690 D8).
    Binding(&'static str),
    /// The CONFIGURED claim name that shows a verified token is not an access token (SMA-703
    /// D4). Logged as the marker `claim <name>`.
    Claim(&'a str),
    /// The CONFIGURED claim name that a verified token does not carry (SMA-731 D4). Logged as the
    /// marker `missing claim <name>`, with its own message.
    MissingClaim(&'a str),
    /// The DPoP scheme with a token that is not bound to exactly one key (SMA-700 § 4.3). Its own
    /// static message, with no marker.
    NotKeyBound,
}

/// One configured issuer, parsed once at construction — replacing the per-request
/// `Issuer::parse` the request path used to run after every issuer match.
/// `jit_provisioning` is deliberately absent: the validator never reads it.
struct ConfiguredIssuer {
    issuer: Issuer,
    audiences: Vec<String>,
    /// SMA-703 D1 and SMA-731 D1: the configured claim lists. Both empty keep the SMA-686 decode
    /// path unchanged.
    claim_rules: ClaimRules,
}

/// The configured claim rules of one issuer (SMA-731 D3): the SMA-703 marker claims and the
/// SMA-731 required claims. `authenticate` asks it two things: which decode to run, and whether
/// the verified payload is refused. The request path does not read `IssuerConfig` again.
struct ClaimRules {
    /// `id_token_marker_claims`: a token that carries one of them is refused (step 6b).
    id_token_markers: Vec<String>,
    /// `access_token_required_claims`: a token that does not carry one of them is refused (step 6c).
    required: Vec<String>,
}

impl ClaimRules {
    /// True when either list is not empty. Then the payload is decoded as a `StrictPayload`, so a
    /// repeated top-level member is `Malformed` (SMA-703 D3). Both empty keep the plain decode (G2).
    fn needs_strict_decode(&self) -> bool {
        !self.id_token_markers.is_empty() || !self.required.is_empty()
    }

    /// Step 6b, then step 6c, on the verified top-level members: `Claim` for the first configured
    /// marker that is present, else `MissingClaim` for the first required name that is missing,
    /// else `None`. A claim is present when its member exists and is not JSON `null`.
    fn refusal(&self, members: &serde_json::Map<String, serde_json::Value>) -> Option<RefusalDetail<'_>> {
        if let Some(name) = configured_marker(members, &self.id_token_markers) {
            return Some(RefusalDetail::Claim(name));
        }
        missing_required_claim(members, &self.required).map(RefusalDetail::MissingClaim)
    }
}

/// The `Authenticator` v1 implementation: validates a presented bearer token against a
/// fixed, operator-configured set of OIDC issuers (spec §4.1). Generic-by-value over its
/// `JwksProvider`'s three collaborators (fetcher/cache/clock), mirroring the provider's own
/// composition convention — the concrete adapters are chosen once at the composition root
/// (Task 14), not boxed as trait objects here.
pub struct OidcAuthenticator<F: JwksFetcher, K: JwksCache, C: Clock> {
    issuers: Vec<ConfiguredIssuer>,
    provider: JwksProvider<F, K, C>,
    leeway_secs: u64,
    max_token_bytes: usize,
    refusal_log: LogRateLimiter<TokenDefect>,
}

impl<F: JwksFetcher, K: JwksCache, C: Clock> OidcAuthenticator<F, K, C> {
    /// Parses every configured issuer once, up front. Fails when one doesn't parse —
    /// `IamConfig::validate` already rejects that at boot, so an `Err` here is a wiring
    /// defect, mirroring the `redis_url` guard in `AppState::new`.
    ///
    /// Writes one `info` line for each issuer with configured `id_token_marker_claims` (SMA-703
    /// D2), and one for each issuer with configured `access_token_required_claims` (SMA-731 D2).
    /// `AppState::new` calls this once at boot, after `paigasus_logging::init`;
    /// `IamConfig::validate` runs before the logger exists, so the line cannot live there.
    pub fn new(issuers: Vec<IssuerConfig>, provider: JwksProvider<F, K, C>, leeway_secs: u64, max_token_bytes: usize) -> Result<Self, AuthnError> {
        let issuers = issuers
            .into_iter()
            .map(|cfg| {
                let issuer = Issuer::parse(&cfg.issuer).map_err(|e| AuthnError::Backend(e.to_string().into()))?;
                if !cfg.id_token_marker_claims.is_empty() {
                    tracing::info!(
                        issuer = issuer.as_str(),
                        id_token_marker_claims = ?cfg.id_token_marker_claims,
                        "IAM refuses a verified token of this issuer that carries one of the configured ID-token marker claims"
                    );
                }
                if !cfg.access_token_required_claims.is_empty() {
                    tracing::info!(
                        issuer = issuer.as_str(),
                        access_token_required_claims = ?cfg.access_token_required_claims,
                        "IAM refuses a verified token of this issuer that does not carry every configured required claim"
                    );
                }
                Ok(ConfiguredIssuer {
                    issuer,
                    audiences: cfg.audiences,
                    claim_rules: ClaimRules {
                        id_token_markers: cfg.id_token_marker_claims,
                        required: cfg.access_token_required_claims,
                    },
                })
            })
            .collect::<Result<Vec<_>, AuthnError>>()?;
        Ok(Self {
            issuers,
            provider,
            leeway_secs,
            max_token_bytes,
            refusal_log: LogRateLimiter::new(LOG_RATE_LIMIT_INTERVAL),
        })
    }

    /// Exact string match against the configured issuer list (spec §3.1's "no
    /// normalization" rule applies here too — compared byte-for-byte, same as
    /// `Issuer::parse`'s own equality semantics). Matching against the PARSED (trimmed)
    /// form is equivalent to the raw config string because `IamConfig::validate` rejects
    /// padded issuers before any authenticator is constructed.
    fn find_issuer_config(&self, iss: &str) -> Option<&ConfiguredIssuer> {
        self.issuers.iter().find(|cfg| cfg.issuer.as_str() == iss)
    }

    /// The one place that decides which refusals are logged (SMA-686 D8, D11, D14, D15; SMA-690
    /// D8; SMA-700 § 4.3): only `NotAnAccessToken`, `AudienceMismatch`, `SenderConstrained` and `NotKeyBound`, each reachable
    /// only for a correctly signed token from a configured issuer, and each rate-limited per
    /// (issuer, defect). Logs the issuer and a static or configured detail — never a token claim.
    /// A missing required claim (SMA-731 D4) is a `NotAnAccessToken` refusal with its own
    /// message; it shares the rate limit of the other `NotAnAccessToken` refusals of the issuer.
    fn log_refusal(&self, issuer: &Issuer, defect: TokenDefect, detail: RefusalDetail<'_>) {
        let Some(suppressed) = self.refusal_log.admit_at(issuer.as_str(), defect, Instant::now()) else {
            return;
        };
        match detail {
            RefusalDetail::Marker(marker) => {
                tracing::info!(issuer = issuer.as_str(), marker, suppressed, "{}", NOT_AN_ACCESS_TOKEN_MESSAGE);
            }
            RefusalDetail::Accepted(accepted) => {
                tracing::info!(issuer = issuer.as_str(), accepted = ?accepted, suppressed, "refused a bearer token: its aud claim holds none of the accepted audiences");
            }
            RefusalDetail::Binding(marker) => {
                tracing::info!(
                    issuer = issuer.as_str(),
                    marker,
                    suppressed,
                    "refused a bearer token: it is bound to a key, and IAM cannot check the binding"
                );
            }
            RefusalDetail::Claim(name) => {
                // The same message as `Marker`, from one constant: an operator greps one text for
                // SMA-686 and SMA-703.
                let marker = format!("claim {name}");
                tracing::info!(issuer = issuer.as_str(), marker = marker.as_str(), suppressed, "{}", NOT_AN_ACCESS_TOKEN_MESSAGE);
            }
            RefusalDetail::MissingClaim(name) => {
                // Its own message (SMA-731 D4): the refused token can be a real access token after
                // an IdP change, so "not an access token" would point the operator at the client.
                let marker = format!("missing claim {name}");
                tracing::info!(issuer = issuer.as_str(), marker = marker.as_str(), suppressed, "{}", MISSING_CLAIM_MESSAGE);
            }
            RefusalDetail::NotKeyBound => {
                tracing::info!(
                    issuer = issuer.as_str(),
                    suppressed,
                    "refused a DPoP request: the token is not bound to a key, or is also bound to a certificate"
                );
            }
        }
    }
}

/// The log message of a `NotAnAccessToken` refusal. The `Marker` and `Claim` arms of
/// `log_refusal` both use it, so an operator greps one text. The `MissingClaim` arm uses
/// `MISSING_CLAIM_MESSAGE` (SMA-731 D4).
const NOT_AN_ACCESS_TOKEN_MESSAGE: &str = "refused a bearer token: a verified marker shows it is not an access token";

/// The log message of a `NotAnAccessToken` refusal for a missing required claim (SMA-731 D4).
const MISSING_CLAIM_MESSAGE: &str = "refused a bearer token: it does not carry a claim that the issuer configuration requires";

/// Manual, not derived: a `#[derive(Debug)]` here would require `F`/`K`/`C` (and in turn
/// `JwksProvider`) to implement `Debug` too. This exists solely so
/// `Result<Self, AuthnError>::unwrap_err()` type-checks in the constructor test below —
/// `unwrap_err` requires the `Ok` side to be `Debug` even though it's never printed here.
impl<F: JwksFetcher, K: JwksCache, C: Clock> std::fmt::Debug for OidcAuthenticator<F, K, C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OidcAuthenticator").finish_non_exhaustive()
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
/// verified the signature (spec §4.1). `sub`/`exp` are required — their absence (or a
/// wrong-shaped value) is a serde failure, which `map_jwt_error` collapses to `Malformed`. `aud`
/// is `Option` so that a token WITHOUT `aud` reaches `jsonwebtoken::validate`, which refuses it
/// with `MissingRequiredClaim("aud")` because `authenticate` puts `aud` in
/// `required_spec_claims` (SMA-686 D13); a wrong-typed `aud` still fails serde (`Malformed`); a
/// JSON `null` `aud` counts as missing (`AudienceMismatch`).
/// The profile claims are optional since an IdP may omit any of them. `typ`, `events` and `cnf`
/// are untyped `Value`s on purpose (SMA-686 D6, D12; SMA-690 D6): a non-string `typ`, a
/// non-object `events` or any shape of `cnf` must not become `Malformed`. A JSON `null` `cnf`
/// deserializes as `None`.
#[derive(Deserialize)]
struct WireClaims {
    sub: String,
    exp: u64,
    aud: Option<WireAudience>,
    email: Option<String>,
    name: Option<String>,
    locale: Option<String>,
    zoneinfo: Option<String>,
    typ: Option<serde_json::Value>,
    events: Option<serde_json::Value>,
    cnf: Option<serde_json::Value>,
}

/// The verified payload for an issuer with configured marker claims (SMA-703 D3): every top-level
/// member, plus the same `WireClaims` the plain path reads. A plain `serde_json::Map` keeps the
/// LAST value of a repeated key with no error, and a derived struct does not check a repeated
/// member it ignores, so `{"at_hash":"x","at_hash":null}` would pass both. This `Deserialize`
/// refuses a repeated top-level member name; `jsonwebtoken` reports that serde error as
/// `ErrorKind::Json`, and `map_jwt_error` maps it to `Malformed`.
///
/// `claims` is read INSIDE `deserialize`, not after `decode` returns. `jsonwebtoken::decode`
/// deserializes the caller's type before it validates `exp`/`aud`/`iss`, so a wrong-shaped claim
/// stays `Malformed` ahead of an expiry or audience defect, exactly as on the plain path.
struct StrictPayload {
    members: serde_json::Map<String, serde_json::Value>,
    claims: WireClaims,
}

impl<'de> Deserialize<'de> for StrictPayload {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct UniqueMembers;

        impl<'de> serde::de::Visitor<'de> for UniqueMembers {
            type Value = serde_json::Map<String, serde_json::Value>;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a JSON object with unique member names")
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
                let mut members = serde_json::Map::new();
                while let Some(name) = access.next_key::<String>()? {
                    // The error text names no member: the payload is token material (SMA-686 D8).
                    if members.contains_key(&name) {
                        return Err(serde::de::Error::custom("a top-level member name occurs twice"));
                    }
                    let value = access.next_value::<serde_json::Value>()?;
                    members.insert(name, value);
                }
                Ok(members)
            }
        }

        let members = deserializer.deserialize_map(UniqueMembers)?;
        // `&Map<String, Value>` is a `Deserializer`, so `WireClaims` reads the map without a copy.
        let claims = WireClaims::deserialize(&members).map_err(serde::de::Error::custom)?;
        Ok(StrictPayload { members, claims })
    }
}

/// The first configured claim name that the verified payload carries with a value other than
/// JSON `null`, or `None` (SMA-703 D3). Any other value is a marker: a string, a number, an
/// object, an array, an empty string (the SMA-690 rule for `cnf`). Names compare exactly, because
/// JSON member names are case-sensitive. The list order decides which name a log line shows.
fn configured_marker<'a>(members: &serde_json::Map<String, serde_json::Value>, names: &'a [String]) -> Option<&'a str> {
    names.iter().find(|name| members.get(name.as_str()).is_some_and(|value| !value.is_null())).map(String::as_str)
}

/// The first configured required claim name that the verified payload does not carry, or `None`
/// (SMA-731 D3). A member with the value JSON `null` counts as missing. Any other value counts as
/// present: a string, a number, `false`, an object, an array, an empty string. Names compare
/// exactly. The list order decides which name a log line shows.
fn missing_required_claim<'a>(members: &serde_json::Map<String, serde_json::Value>, names: &'a [String]) -> Option<&'a str> {
    names.iter().find(|name| members.get(name.as_str()).is_none_or(serde_json::Value::is_null)).map(String::as_str)
}

/// Maps a `jsonwebtoken` decode/validation failure to a `TokenDefect` (spec §4.1). Every
/// kind this validator doesn't specifically distinguish (bad base64, malformed JSON, a
/// wrong-shaped claim, an unhandled `ErrorKind`) collapses to `Malformed`. A missing `aud`
/// counts as an audience mismatch (SMA-686 D13).
fn map_jwt_error(err: jsonwebtoken::errors::Error) -> AuthnError {
    match err.into_kind() {
        ErrorKind::ExpiredSignature => invalid(TokenDefect::Expired),
        ErrorKind::ImmatureSignature => invalid(TokenDefect::NotYetValid),
        ErrorKind::InvalidSignature => invalid(TokenDefect::BadSignature),
        ErrorKind::InvalidAudience => invalid(TokenDefect::AudienceMismatch),
        ErrorKind::InvalidIssuer => invalid(TokenDefect::IssuerNotConfigured),
        ErrorKind::MissingRequiredClaim(claim) if claim == "aud" => invalid(TokenDefect::AudienceMismatch),
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

/// The static marker that shows a signature-verified token is NOT an access token, or `None`
/// (SMA-686 D2, D12). Markers: a back-channel logout header `typ`, the back-channel logout
/// `events` member, or a Keycloak payload `typ` of `ID`/`Logout`. A missing, `null`, non-string
/// `typ` and a non-object `events` are not markers (D6).
fn non_access_token_marker(header_typ: Option<&str>, claims: &WireClaims) -> Option<&'static str> {
    if header_typ.is_some_and(|typ| LOGOUT_TOKEN_HEADER_TYPES.iter().any(|logout| typ.eq_ignore_ascii_case(logout))) {
        return Some("logout+jwt");
    }
    if claims
        .events
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .is_some_and(|events| events.contains_key(BACKCHANNEL_LOGOUT_EVENT))
    {
        return Some("backchannel-logout event");
    }
    let Some(serde_json::Value::String(typ)) = &claims.typ else {
        return None;
    };
    NON_ACCESS_TOKEN_TYPES.iter().find(|marker| typ.eq_ignore_ascii_case(marker)).copied()
}

/// The static marker that shows a signature-verified token is bound to a key, or `None`
/// (SMA-690 D2, D3). Marker 1: a `cnf` claim with any value except `null`. Marker 2: a payload
/// `typ` of `DPoP`. Marker 1 is checked first.
fn sender_constraint_marker(claims: &WireClaims) -> Option<&'static str> {
    if claims.cnf.is_some() {
        return Some("cnf");
    }
    let Some(serde_json::Value::String(typ)) = &claims.typ else {
        return None;
    };
    SENDER_CONSTRAINED_TYPES.iter().any(|marker| typ.eq_ignore_ascii_case(marker)).then_some("typ DPoP")
}

/// The `cnf.jkt` of a signature-verified token on the DPoP scheme (SMA-700 § 4.3), or `None` when
/// the token is not bound to exactly one key: `cnf` must be a JSON object with a `jkt` member that
/// is a non-empty string, and no `x5t#S256` member (an mTLS binding stays refused, D4).
fn dpop_key_binding(claims: &WireClaims) -> Option<Jkt> {
    let cnf = claims.cnf.as_ref()?.as_object()?;
    if cnf.contains_key("x5t#S256") {
        return None;
    }
    let jkt = cnf.get("jkt")?.as_str()?;
    (!jkt.is_empty()).then(|| Jkt::new(jkt))
}

#[async_trait]
impl<F: JwksFetcher, K: JwksCache, C: Clock> Authenticator for OidcAuthenticator<F, K, C> {
    async fn authenticate(&self, token: &str, scheme: TokenScheme) -> Result<ValidatedClaims, AuthnError> {
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
        let issuer = issuer_config.issuer.clone();

        // 4. JWKS lookup + kty/alg consistency.
        let jwk = self.provider.key_for(&issuer, &kid).await?;
        check_kty_matches_alg(&jwk, header.alg)?;
        let decoding_key = DecodingKey::from_jwk(&jwk).map_err(|_| invalid(TokenDefect::BadSignature))?;

        // 5. Signature + claims validation, pinned to exactly the header's algorithm.
        let mut validation = Validation::new(header.alg);
        validation.set_issuer(&[issuer.as_str()]);
        validation.set_audience(&issuer_config.audiences);
        validation.set_required_spec_claims(&["exp", "aud"]);
        validation.leeway = self.leeway_secs;
        validation.validate_nbf = true;

        let on_decode_error = |err: jsonwebtoken::errors::Error| {
            let err = map_jwt_error(err);
            if matches!(err, AuthnError::InvalidToken(TokenDefect::AudienceMismatch)) {
                self.log_refusal(&issuer, TokenDefect::AudienceMismatch, RefusalDetail::Accepted(&issuer_config.audiences));
            }
            err
        };
        // SMA-703 D3 and SMA-731 D3: an issuer with no claim rules keeps the SMA-686 decode
        // exactly (G2). One with a marker list or a required list decodes the same verified bytes
        // as a `StrictPayload`; the signature is checked once either way.
        let (verified_header, claims, claim_refusal) = if !issuer_config.claim_rules.needs_strict_decode() {
            let token_data = decode::<WireClaims>(token, &decoding_key, &validation).map_err(on_decode_error)?;
            (token_data.header, token_data.claims, None)
        } else {
            let token_data = decode::<StrictPayload>(token, &decoding_key, &validation).map_err(on_decode_error)?;
            let refusal = issuer_config.claim_rules.refusal(&token_data.claims.members);
            (token_data.header, token_data.claims.claims, refusal)
        };

        // 6. Token-type check on the verified token (SMA-686): an ID token or a logout token.
        if let Some(marker) = non_access_token_marker(verified_header.typ.as_deref(), &claims) {
            self.log_refusal(&issuer, TokenDefect::NotAnAccessToken, RefusalDetail::Marker(marker));
            return Err(invalid(TokenDefect::NotAnAccessToken));
        }
        // 6b. A configured marker claim (SMA-703 D3), then 6c. a missing required claim (SMA-731
        // D3), after the SMA-686 markers and before the key binding. `ClaimRules::refusal` keeps
        // the 6b-then-6c order.
        if let Some(detail) = claim_refusal {
            self.log_refusal(&issuer, TokenDefect::NotAnAccessToken, detail);
            return Err(invalid(TokenDefect::NotAnAccessToken));
        }

        // 7. The key binding (SMA-690, SMA-700 § 4.3). Bearer: a bound token is refused, because
        //    IAM cannot check the binding on that scheme (D7). DPoP: the token must be bound to
        //    exactly one key, and the caller checks the proof against `key_binding`.
        let key_binding = match scheme {
            TokenScheme::Bearer => {
                if let Some(marker) = sender_constraint_marker(&claims) {
                    self.log_refusal(&issuer, TokenDefect::SenderConstrained, RefusalDetail::Binding(marker));
                    return Err(invalid(TokenDefect::SenderConstrained));
                }
                None
            }
            TokenScheme::Dpop => match dpop_key_binding(&claims) {
                Some(jkt) => Some(jkt),
                None => {
                    self.log_refusal(&issuer, TokenDefect::NotKeyBound, RefusalDetail::NotKeyBound);
                    return Err(invalid(TokenDefect::NotKeyBound));
                }
            },
        };

        let expires_at = i64::try_from(claims.exp)
            .ok()
            .and_then(|secs| DateTime::<Utc>::from_timestamp(secs, 0))
            .ok_or_else(|| invalid(TokenDefect::Malformed))?;

        Ok(ValidatedClaims {
            issuer,
            subject: claims.sub,
            // A second guard: `validate` already refuses a missing `aud` (D13); if that ever
            // regresses, the token is still refused, as Malformed.
            audiences: claims.aud.map(WireAudience::into_vec).ok_or_else(|| invalid(TokenDefect::Malformed))?,
            expires_at,
            email: claims.email,
            name: claims.name,
            locale: claims.locale,
            zoneinfo: claims.zoneinfo,
            key_binding,
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
    use p256::elliptic_curve::Generate;
    use p256::elliptic_curve::sec1::ToSec1Point;
    use p256::pkcs8::{EncodePrivateKey, LineEnding};
    use paigasus_logging::test_support::capture_logs;
    use serde::Serialize;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    /// Mints a runtime EC P-256 keypair (spec §8 mock-IdP refinement: no committed
    /// PEM/JWK fixtures, no `rsa` crate — RS256's accept path is covered end-to-end by the
    /// Keycloak integration test instead). Returns the signing key, the corresponding
    /// public JWK, and a fixed `kid` tying the two together.
    fn es256_keypair() -> (EncodingKey, Jwk, String) {
        let secret_key = p256::SecretKey::generate();
        let pem = secret_key.to_pkcs8_pem(LineEnding::LF).expect("valid pkcs8 pem");
        let encoding_key = EncodingKey::from_ec_pem(pem.as_bytes()).expect("valid ec pem");

        let encoded_point = secret_key.public_key().to_sec1_point(false);
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

    fn sign<C: serde::Serialize>(encoding_key: &EncodingKey, kid: Option<&str>, claims: &C) -> String {
        let mut header = jsonwebtoken::Header::new(Algorithm::ES256);
        header.kid = kid.map(str::to_string);
        jsonwebtoken::encode(&header, claims, encoding_key).expect("signing a test token")
    }

    fn sign_with_header_typ<C: serde::Serialize>(encoding_key: &EncodingKey, kid: &str, header_typ: &str, claims: &C) -> String {
        let mut header = jsonwebtoken::Header::new(Algorithm::ES256);
        header.kid = Some(kid.to_string());
        header.typ = Some(header_typ.to_string());
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
        OidcAuthenticator::new(issuers, provider, leeway_secs, max_token_bytes).expect("test issuers parse")
    }

    fn issuer_config(issuer: &str, audiences: &[&str]) -> IssuerConfig {
        IssuerConfig {
            issuer: issuer.to_string(),
            audiences: audiences.iter().map(|a| (*a).to_string()).collect(),
            jit_provisioning: true,
            id_token_marker_claims: Vec::new(),
            access_token_required_claims: Vec::new(),
        }
    }

    #[test]
    fn unparseable_configured_issuer_fails_construction() {
        // `IamConfig::validate` rejects this at boot, so an Err here is a wiring-defect
        // guard — but the constructor must still refuse rather than defer to per-request
        // parse failures (which this change removes).
        let (_encoding_key, jwk, _kid) = es256_keypair();
        let provider = JwksProvider::new(StubFetcher::new(jwk), InMemoryJwksCache::new(), SystemClock, Duration::from_secs(3600), Duration::from_secs(30));
        let err = OidcAuthenticator::new(vec![issuer_config("http://not-https.example.com", &["aud"])], provider, 60, 16_384).unwrap_err();
        assert!(matches!(err, AuthnError::Backend(_)));
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

        let validated = authenticator
            .authenticate(&token, TokenScheme::Bearer)
            .await
            .expect("a well-formed, correctly signed token must authenticate");

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
        let err = authenticator.authenticate(&none_token, TokenScheme::Bearer).await.unwrap_err();
        assert!(matches!(err, AuthnError::InvalidToken(_)), "an alg=none token must be rejected");

        let hs256_token = manual_token(r#"{"alg":"HS256","typ":"JWT","kid":"whatever"}"#);
        let err = authenticator.authenticate(&hs256_token, TokenScheme::Bearer).await.unwrap_err();
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

        let err = authenticator.authenticate(&token, TokenScheme::Bearer).await.unwrap_err();
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

        let err = authenticator.authenticate(&token, TokenScheme::Bearer).await.unwrap_err();
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
        ok_authenticator
            .authenticate(&ok_token, TokenScheme::Bearer)
            .await
            .expect("a 30s-expired token within a 60s leeway must be accepted");

        // Expired 120s ago -> beyond the same 60s leeway, must be rejected.
        let expired_token = sign(&encoding_key, Some(&kid), &bare_claims(issuer, "aud", now - 120));
        let expired_authenticator = make_authenticator(StubFetcher::new(jwk), vec![issuer_config(issuer, &["aud"])], 60, 16_384);
        let err = expired_authenticator.authenticate(&expired_token, TokenScheme::Bearer).await.unwrap_err();
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

        let err = authenticator.authenticate(&token, TokenScheme::Bearer).await.unwrap_err();
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

        let err = authenticator.authenticate(&token, TokenScheme::Bearer).await.unwrap_err();
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
        ok_authenticator
            .authenticate(&ok_token, TokenScheme::Bearer)
            .await
            .expect("an nbf 30s in the future within a 60s leeway must be accepted");

        // `nbf` 120s in the future -> beyond the same 60s leeway, must be rejected.
        let mut future_claims = bare_claims(issuer, "aud", now + 3600);
        future_claims.nbf = Some(now + 120);
        let future_token = sign(&encoding_key, Some(&kid), &future_claims);
        let future_authenticator = make_authenticator(StubFetcher::new(jwk), vec![issuer_config(issuer, &["aud"])], 60, 16_384);
        let err = future_authenticator.authenticate(&future_token, TokenScheme::Bearer).await.unwrap_err();
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

        let err = authenticator.authenticate(&token, TokenScheme::Bearer).await.unwrap_err();
        assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::BadSignature)));
    }

    #[tokio::test]
    async fn jwk_kty_mismatching_header_alg_is_unsupported_alg() {
        // The stub JWKS serves an EC P-256 key under `kid`, but the crafted header claims
        // RS256 under that SAME kid: allowlist passes (RS256 is allowed), kid lookup
        // succeeds, and the kty/alg consistency check must reject — an EC key can never
        // have produced an RS256 signature — BEFORE signature verification (the
        // placeholder signature is never inspected).
        let (_encoding_key, jwk, kid) = es256_keypair();
        let authenticator = make_authenticator(StubFetcher::new(jwk), vec![issuer_config("https://idp.example.com", &["aud"])], 60, 16_384);

        let token = manual_token(&format!(r#"{{"alg":"RS256","typ":"JWT","kid":"{kid}"}}"#));
        let err = authenticator.authenticate(&token, TokenScheme::Bearer).await.unwrap_err();
        assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::UnsupportedAlg)));
    }

    #[tokio::test]
    async fn empty_audience_array_is_audience_mismatch() {
        // RFC 7519 §4.1.3 allows `aud` as an array; an EMPTY array can never intersect the
        // configured audience set, so validation must reject it as an audience mismatch
        // (verified against jsonwebtoken 10.4's is_subset: empty intersection -> InvalidAudience).
        #[derive(Serialize)]
        struct EmptyAudClaims {
            iss: String,
            sub: String,
            aud: Vec<String>,
            exp: i64,
        }
        let (encoding_key, jwk, kid) = es256_keypair();
        let issuer = "https://idp.example.com";
        let now = Utc::now().timestamp();
        let claims = EmptyAudClaims {
            iss: issuer.to_string(),
            sub: "sub-1".to_string(),
            aud: vec![],
            exp: now + 3600,
        };
        let token = sign(&encoding_key, Some(&kid), &claims);

        let authenticator = make_authenticator(StubFetcher::new(jwk), vec![issuer_config(issuer, &["expected-aud"])], 60, 16_384);
        let err = authenticator.authenticate(&token, TokenScheme::Bearer).await.unwrap_err();
        assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::AudienceMismatch)));
    }

    // ---- SMA-686: the payload `typ` check -------------------------------------------------

    const ISSUER: &str = "https://idp.example.com";

    /// A token that passes every other check (issuer, audience `aud`, one hour of life),
    /// plus the claims in `extra`. So only the extra claims differ from an accepted token.
    fn claims_with(extra: serde_json::Value) -> serde_json::Value {
        let claims = serde_json::json!({
            "iss": ISSUER,
            "sub": "sub-1",
            "aud": "aud",
            "exp": Utc::now().timestamp() + 3600,
            "email": "alice@example.com",
        });
        merged(claims, extra)
    }

    /// Signs `claims` with a fresh ES256 key and runs the full validator pipeline on it.
    async fn authenticate_json(claims: &serde_json::Value) -> Result<ValidatedClaims, AuthnError> {
        let (encoding_key, jwk, kid) = es256_keypair();
        let token = sign(&encoding_key, Some(&kid), claims);
        let authenticator = make_authenticator(StubFetcher::new(jwk), vec![issuer_config(ISSUER, &["aud"])], 60, 16_384);
        authenticator.authenticate(&token, TokenScheme::Bearer).await
    }

    #[tokio::test]
    async fn refuses_keycloak_id_and_logout_typ() {
        // Spec § 5.1 tests 1-4: Keycloak sets `typ: ID` on the ID token and `typ: Logout` on the
        // back-channel logout token (measured, SMA-686 measurements). Case-insensitive.
        for typ in ["ID", "id", "Logout", "LOGOUT"] {
            let err = authenticate_json(&claims_with(serde_json::json!({ "typ": typ }))).await.unwrap_err();
            assert!(
                matches!(err, AuthnError::InvalidToken(TokenDefect::NotAnAccessToken)),
                "typ {typ:?} must be refused as NotAnAccessToken, got {err:?}"
            );
        }
    }

    #[tokio::test]
    async fn accepts_non_marker_typ_values() {
        // Spec § 5.1 tests 5-10 (SMA-686), and SMA-690 test 10: `DPoP` moved to
        // `refuses_sender_constrained_tokens`. Every other shape here passes.
        let cases = [
            ("Bearer (Keycloak access token)", serde_json::json!({ "typ": "Bearer" })),
            // Every Dex access token: no `typ`, but `at_hash` and `nonce` (measured). `c_hash`
            // added too: none of the three is a marker (spec D3).
            ("Dex shape", serde_json::json!({ "at_hash": "x", "c_hash": "y", "nonce": "abc123" })),
            ("typ null", serde_json::json!({ "typ": null })),
            ("typ number", serde_json::json!({ "typ": 1 })),
            ("typ with leading space", serde_json::json!({ "typ": " ID" })),
            ("typ DPoP with leading space", serde_json::json!({ "typ": " DPoP" })),
        ];
        for (name, extra) in cases {
            authenticate_json(&claims_with(extra)).await.unwrap_or_else(|err| panic!("{name}: must be accepted, got {err:?}"));
        }
    }

    #[tokio::test]
    async fn expired_id_token_reports_expired() {
        // Spec D5: the check runs after `decode`, so the expiry defect wins.
        let claims = claims_with(serde_json::json!({ "typ": "ID", "exp": Utc::now().timestamp() - 120 }));
        let err = authenticate_json(&claims).await.unwrap_err();
        assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::Expired)), "got {err:?}");
    }

    // ---- SMA-690: the sender-constraint check ---------------------------------------------

    #[tokio::test]
    async fn refuses_sender_constrained_tokens() {
        // Spec § 5.1 tests 1-8, plus an array `cnf` (Review Focus 1).
        let cases = [
            ("cnf.jkt (Keycloak DPoP, M2)", serde_json::json!({ "cnf": { "jkt": "abc" } })),
            ("cnf.x5t#S256 (RFC 8705)", serde_json::json!({ "cnf": { "x5t#S256": "abc" } })),
            ("cnf.jwk (RFC 7800)", serde_json::json!({ "cnf": { "jwk": { "kty": "EC" } } })),
            ("cnf empty object", serde_json::json!({ "cnf": {} })),
            ("cnf string", serde_json::json!({ "cnf": "x" })),
            ("cnf array", serde_json::json!({ "cnf": ["x"] })),
            ("typ DPoP", serde_json::json!({ "typ": "DPoP" })),
            ("typ dpop", serde_json::json!({ "typ": "dpop" })),
            ("full Keycloak shape", serde_json::json!({ "typ": "DPoP", "cnf": { "jkt": "abc" } })),
        ];
        for (name, extra) in cases {
            let err = authenticate_json(&claims_with(extra)).await.unwrap_err();
            assert!(
                matches!(err, AuthnError::InvalidToken(TokenDefect::SenderConstrained)),
                "{name}: must be refused as SenderConstrained, got {err:?}"
            );
        }
    }

    #[tokio::test]
    async fn accepts_null_cnf() {
        // Spec D2 / § 5.1 test 9: a `null` `cnf` confirms no key.
        authenticate_json(&claims_with(serde_json::json!({ "cnf": null }))).await.expect("cnf: null must be accepted");
    }

    #[tokio::test]
    async fn id_token_with_cnf_reports_not_an_access_token() {
        // Spec D5 / § 5.1 test 11: the SMA-686 check runs first.
        let err = authenticate_json(&claims_with(serde_json::json!({ "typ": "ID", "cnf": { "jkt": "abc" } }))).await.unwrap_err();
        assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::NotAnAccessToken)), "got {err:?}");
    }

    #[tokio::test]
    async fn expired_bound_token_reports_expired() {
        // Spec D5 / § 5.1 test 12: the check runs after `decode`.
        let claims = claims_with(serde_json::json!({ "cnf": { "jkt": "abc" }, "exp": Utc::now().timestamp() - 120 }));
        let err = authenticate_json(&claims).await.unwrap_err();
        assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::Expired)), "got {err:?}");
    }

    /// Builds a token whose payload holds a DUPLICATE `cnf` key. The `json!` macro cannot make
    /// one (a JSON object literal cannot repeat a key), so this signs a raw JSON string by hand:
    /// the header, then `claims_with`'s usual fields, then `cnf_fields` verbatim. Signs with
    /// `jsonwebtoken::crypto::sign` directly, since `jsonwebtoken::encode` takes a typed struct
    /// and so cannot emit a duplicate key either.
    fn token_with_duplicate_cnf(encoding_key: &EncodingKey, kid: &str, cnf_fields: &str) -> String {
        let exp = Utc::now().timestamp() + 3600;
        let payload_json = format!(r#"{{"iss":"{ISSUER}","sub":"sub-1","aud":"aud","exp":{exp},"email":"alice@example.com",{cnf_fields}}}"#);
        sign_raw_payload(encoding_key, kid, &payload_json)
    }

    #[tokio::test]
    async fn duplicate_cnf_claim_is_never_authenticated() {
        // Finding 2 of the final whole-branch review: no test pinned a DUPLICATE `cnf` key. Today
        // `serde`'s derived `Deserialize` for `WireClaims` refuses a repeated map key outright, so
        // `serde_json::from_slice` errors and `map_jwt_error`'s fallback arm reports `Malformed`.
        // A later refactor away from that derive (a map, `#[serde(flatten)]`, a `Value` pre-parse)
        // could take the last `cnf` value instead and let a sender-constrained token pass. This
        // test does not pin the exact defect, only that the token is never authenticated.
        let (encoding_key, jwk, kid) = es256_keypair();
        let authenticator = make_authenticator(StubFetcher::new(jwk), vec![issuer_config(ISSUER, &["aud"])], 60, 16_384);

        for (name, cnf_fields) in [
            ("cnf object then null", r#""cnf":{"jkt":"abc"},"cnf":null"#),
            ("cnf null then object", r#""cnf":null,"cnf":{"jkt":"abc"}"#),
        ] {
            let token = token_with_duplicate_cnf(&encoding_key, &kid, cnf_fields);
            let err = authenticator.authenticate(&token, TokenScheme::Bearer).await.unwrap_err();
            // Observed today: TokenDefect::Malformed (the duplicate key fails serde before the
            // sender-constraint check ever runs).
            assert!(matches!(err, AuthnError::InvalidToken(_)), "{name}: must never authenticate, got {err:?}");
        }
    }

    /// `WireClaims` from a JSON payload, for the direct marker test.
    fn wire_claims(extra: serde_json::Value) -> WireClaims {
        serde_json::from_value(claims_with(extra)).expect("test claims deserialize")
    }

    #[test]
    fn sender_constraint_marker_names_the_marker() {
        // Spec § 5.1 test 13: the marker text and the D3 order.
        assert_eq!(sender_constraint_marker(&wire_claims(serde_json::json!({ "cnf": { "jkt": "abc" } }))), Some("cnf"));
        assert_eq!(sender_constraint_marker(&wire_claims(serde_json::json!({ "typ": "DPoP", "cnf": { "jkt": "abc" } }))), Some("cnf"));
        assert_eq!(sender_constraint_marker(&wire_claims(serde_json::json!({ "typ": "dpop" }))), Some("typ DPoP"));
        assert_eq!(sender_constraint_marker(&wire_claims(serde_json::json!({ "cnf": null }))), None);
        assert_eq!(sender_constraint_marker(&wire_claims(serde_json::json!({ "typ": "Bearer" }))), None);
    }

    // ---- SMA-700: step 7 on the DPoP scheme ------------------------------------------------

    const JKT: &str = "0ZcOCORZNYy-DWpqq30jZyJGHTN0d2HglBV3uiguA4I";

    /// Signs `claims` with a fresh ES256 key and runs the full pipeline on the given scheme.
    async fn authenticate_json_as(claims: &serde_json::Value, scheme: TokenScheme) -> Result<ValidatedClaims, AuthnError> {
        let (encoding_key, jwk, kid) = es256_keypair();
        let token = sign(&encoding_key, Some(&kid), claims);
        let authenticator = make_authenticator(StubFetcher::new(jwk), vec![issuer_config(ISSUER, &["aud"])], 60, 16_384);
        authenticator.authenticate(&token, scheme).await
    }

    #[tokio::test]
    async fn dpop_scheme_accepts_a_jkt_bound_token_and_returns_the_binding() {
        // Spec § 5.1 validator case 1: the Keycloak shape (typ DPoP plus cnf.jkt), and cnf.jkt alone.
        for extra in [serde_json::json!({ "typ": "DPoP", "cnf": { "jkt": JKT } }), serde_json::json!({ "cnf": { "jkt": JKT } })] {
            let validated = authenticate_json_as(&claims_with(extra.clone()), TokenScheme::Dpop)
                .await
                .unwrap_or_else(|err| panic!("{extra}: a jkt-bound token must pass the DPoP scheme, got {err:?}"));
            assert_eq!(validated.key_binding, Some(Jkt::new(JKT)), "{extra}");
            assert_eq!(validated.subject, "sub-1");
        }
    }

    #[tokio::test]
    async fn dpop_scheme_refuses_a_token_that_is_not_bound_to_exactly_one_key() {
        // Spec § 5.1 validator cases 2-5 (§ 4.3): cnf with no jkt, jkt plus x5t#S256, typ DPoP
        // only, no binding, and the shapes of a jkt that is not a non-empty string.
        let cases = [
            ("cnf with no jkt", serde_json::json!({ "cnf": { "jwk": { "kty": "EC" } } })),
            ("cnf empty object", serde_json::json!({ "cnf": {} })),
            ("jkt and x5t#S256", serde_json::json!({ "cnf": { "jkt": JKT, "x5t#S256": "abc" } })),
            ("x5t#S256 only", serde_json::json!({ "cnf": { "x5t#S256": "abc" } })),
            ("typ DPoP only", serde_json::json!({ "typ": "DPoP" })),
            ("no binding", serde_json::json!({})),
            ("cnf null", serde_json::json!({ "cnf": null })),
            ("cnf a string", serde_json::json!({ "cnf": JKT })),
            ("jkt empty", serde_json::json!({ "cnf": { "jkt": "" } })),
            ("jkt a number", serde_json::json!({ "cnf": { "jkt": 1 } })),
            ("jkt null", serde_json::json!({ "cnf": { "jkt": null } })),
        ];
        for (name, extra) in cases {
            let err = authenticate_json_as(&claims_with(extra), TokenScheme::Dpop).await.unwrap_err();
            assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::NotKeyBound)), "{name}: want NotKeyBound, got {err:?}");
        }
    }

    #[tokio::test]
    async fn bearer_scheme_still_refuses_a_bound_token_and_binds_nothing() {
        // D7: with DPoP on, the Bearer scheme does not change.
        let err = authenticate_json_as(&claims_with(serde_json::json!({ "typ": "DPoP", "cnf": { "jkt": JKT } })), TokenScheme::Bearer)
            .await
            .unwrap_err();
        assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::SenderConstrained)), "got {err:?}");
        let plain = authenticate_json_as(&claims_with(serde_json::json!({})), TokenScheme::Bearer)
            .await
            .expect("a plain token passes Bearer");
        assert_eq!(plain.key_binding, None);
    }

    #[tokio::test]
    async fn the_dpop_scheme_keeps_the_earlier_defects_first() {
        // Steps 1-6b do not change (§ 4.3): an ID token stays NotAnAccessToken, an expired bound
        // token stays Expired.
        let id = authenticate_json_as(&claims_with(serde_json::json!({ "typ": "ID", "cnf": { "jkt": JKT } })), TokenScheme::Dpop)
            .await
            .unwrap_err();
        assert!(matches!(id, AuthnError::InvalidToken(TokenDefect::NotAnAccessToken)), "got {id:?}");
        let expired = claims_with(serde_json::json!({ "cnf": { "jkt": JKT }, "exp": Utc::now().timestamp() - 120 }));
        let err = authenticate_json_as(&expired, TokenScheme::Dpop).await.unwrap_err();
        assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::Expired)), "got {err:?}");
    }

    #[tokio::test]
    async fn a_not_key_bound_refusal_logs_its_own_static_message_and_no_claim() {
        let (logs, _guard) = capture_logs();
        let err = authenticate_json_as(&claims_with(serde_json::json!({ "cnf": { "jkt": JKT, "x5t#S256": "cert-thumb" } })), TokenScheme::Dpop)
            .await
            .unwrap_err();
        assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::NotKeyBound)));
        let text = logs.text();
        let lines: Vec<&str> = text
            .lines()
            .filter(|line| line.contains("refused a DPoP request: the token is not bound to a key, or is also bound to a certificate"))
            .collect();
        assert_eq!(lines.len(), 1, "exactly one NotKeyBound line expected, got:\n{text}");
        assert!(lines[0].contains("INFO") && lines[0].contains(ISSUER), "{}", lines[0]);
        for secret in [JKT, "cert-thumb", "sub-1", "alice@example.com"] {
            assert!(!text.contains(secret), "the log must not contain {secret:?}:\n{text}");
        }
    }

    #[tokio::test]
    async fn refusal_logs_issuer_and_marker_only() {
        // Spec D8 / § 5.1 test 12. `#[tokio::test]` is current-thread, so the thread-local
        // subscriber from `set_default` sees the validator's log line.
        let (logs, _guard) = capture_logs();
        let err = authenticate_json(&claims_with(serde_json::json!({ "typ": "id" }))).await.unwrap_err();
        assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::NotAnAccessToken)));

        let text = logs.text();
        let refusal_lines: Vec<&str> = text.lines().filter(|line| line.contains("not an access token")).collect();
        assert_eq!(refusal_lines.len(), 1, "exactly one refusal line expected, got:\n{text}");
        let line = refusal_lines[0];
        assert!(line.contains("INFO"), "the refusal logs at info: {line}");
        assert!(line.contains(ISSUER), "the refusal names the issuer: {line}");
        // The canonical marker, not the token's own spelling ("id").
        assert!(line.contains("\"ID\"") || line.contains("=ID"), "the refusal names the canonical marker: {line}");
        for secret in ["sub-1", "alice@example.com", "\"id\""] {
            assert!(!text.contains(secret), "the log must not contain {secret:?}:\n{text}");
        }
    }

    #[tokio::test]
    async fn audience_mismatch_logs_issuer_and_accepted_audiences_only() {
        // SMA-686 R2 / spec D11: the runbook tells operators a wrong audience shows in the IAM
        // log. One info line with the issuer and the CONFIGURED audiences — never the token's aud.
        let (logs, _guard) = capture_logs();
        let err = authenticate_json(&claims_with(serde_json::json!({ "aud": "token-aud-xyz" }))).await.unwrap_err();
        assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::AudienceMismatch)), "got {err:?}");

        let text = logs.text();
        let lines: Vec<&str> = text.lines().filter(|line| line.contains("holds none of the accepted audiences")).collect();
        assert_eq!(lines.len(), 1, "exactly one audience-mismatch line expected, got:\n{text}");
        let line = lines[0];
        assert!(line.contains("INFO"), "logs at info: {line}");
        assert!(line.contains(ISSUER), "names the issuer: {line}");
        assert!(line.contains("\"aud\""), "names the configured audience: {line}");
        for secret in ["token-aud-xyz", "sub-1", "alice@example.com"] {
            assert!(!text.contains(secret), "the log must not contain {secret:?}:\n{text}");
        }
    }

    #[tokio::test]
    async fn bad_signature_with_wrong_audience_logs_nothing() {
        // Spec D11 flood safety: jsonwebtoken verifies the signature before `aud`, so a forged
        // token with a wrong aud is BadSignature and never reaches the audience log line.
        let (logs, _guard) = capture_logs();
        let (signing_key, _jwk_a, kid) = es256_keypair();
        let (_other_key, served_jwk, _kid_b) = es256_keypair();
        let token = sign(&signing_key, Some(&kid), &claims_with(serde_json::json!({ "aud": "token-aud-xyz" })));
        let authenticator = make_authenticator(StubFetcher::new(served_jwk), vec![issuer_config(ISSUER, &["aud"])], 60, 16_384);

        let err = authenticator.authenticate(&token, TokenScheme::Bearer).await.unwrap_err();
        assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::BadSignature)), "got {err:?}");
        assert!(!logs.text().contains("accepted audiences"), "a forged token must not reach the audience log:\n{}", logs.text());
    }

    /// Signs `claims` with a fresh key under `header_typ` and authenticates it.
    async fn authenticate_with_header_typ(header_typ: &str, claims: &serde_json::Value) -> Result<ValidatedClaims, AuthnError> {
        let (encoding_key, jwk, kid) = es256_keypair();
        let token = sign_with_header_typ(&encoding_key, &kid, header_typ, claims);
        let authenticator = make_authenticator(StubFetcher::new(jwk), vec![issuer_config(ISSUER, &["aud"])], 60, 16_384);
        authenticator.authenticate(&token, TokenScheme::Bearer).await
    }

    #[tokio::test]
    async fn refuses_standard_logout_token_markers() {
        // SMA-686 D12: OIDC Back-Channel Logout 1.0 § 2.4 markers, for any IdP.
        for header_typ in ["logout+jwt", "application/logout+jwt", "LOGOUT+JWT"] {
            let err = authenticate_with_header_typ(header_typ, &claims_with(serde_json::json!({}))).await.unwrap_err();
            assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::NotAnAccessToken)), "header typ {header_typ:?}: got {err:?}");
        }
        let events = serde_json::json!({ "events": { "http://schemas.openid.net/event/backchannel-logout": {} } });
        let err = authenticate_json(&claims_with(events)).await.unwrap_err();
        assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::NotAnAccessToken)), "events member: got {err:?}");
    }

    #[tokio::test]
    async fn accepts_non_logout_header_typ_and_events() {
        // D1: access-token header types and unrelated `events` shapes pass.
        for header_typ in ["JWT", "at+jwt", "application/at+jwt"] {
            authenticate_with_header_typ(header_typ, &claims_with(serde_json::json!({})))
                .await
                .unwrap_or_else(|err| panic!("header typ {header_typ:?} must be accepted, got {err:?}"));
        }
        for events in [serde_json::json!({ "other-event": {} }), serde_json::json!("x"), serde_json::json!(null)] {
            authenticate_json(&claims_with(serde_json::json!({ "events": events.clone() })))
                .await
                .unwrap_or_else(|err| panic!("events {events} must be accepted, got {err:?}"));
        }
    }

    #[tokio::test]
    async fn missing_audience_is_audience_mismatch_and_logged() {
        // SMA-686 D13: a token with NO aud (Keycloak lightweight access token, measurement B5)
        // is refused and logged like a wrong aud. Without `aud` in required_spec_claims,
        // jsonwebtoken would ACCEPT it — this test also pins that.
        let (logs, _guard) = capture_logs();
        let mut claims = claims_with(serde_json::json!({}));
        claims.as_object_mut().expect("object").remove("aud");
        let err = authenticate_json(&claims).await.unwrap_err();
        assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::AudienceMismatch)), "got {err:?}");
        let text = logs.text();
        assert_eq!(
            text.lines().filter(|line| line.contains("holds none of the accepted audiences")).count(),
            1,
            "one audience line expected:\n{text}"
        );
    }

    #[tokio::test]
    async fn wrong_typed_audience_is_malformed() {
        // D13: a wrong-typed aud fails serde first — Malformed, as before.
        let err = authenticate_json(&claims_with(serde_json::json!({ "aud": 7 }))).await.unwrap_err();
        assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::Malformed)), "got {err:?}");
    }

    #[tokio::test]
    async fn null_audience_is_audience_mismatch() {
        // D13: `aud: null` deserializes as None and jsonwebtoken reports it as missing.
        let err = authenticate_json(&claims_with(serde_json::json!({ "aud": null }))).await.unwrap_err();
        assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::AudienceMismatch)), "got {err:?}");
    }

    #[tokio::test]
    async fn repeated_refusals_log_once_per_issuer_and_defect() {
        // SMA-686 D14: one authenticator, three ID tokens and two wrong-aud tokens -> exactly one
        // line of each kind; the returned error is unchanged every time.
        let (logs, _guard) = capture_logs();
        let (encoding_key, jwk, kid) = es256_keypair();
        let authenticator = make_authenticator(StubFetcher::new(jwk), vec![issuer_config(ISSUER, &["aud"])], 60, 16_384);
        for _ in 0..3 {
            let token = sign(&encoding_key, Some(&kid), &claims_with(serde_json::json!({ "typ": "ID" })));
            let err = authenticator.authenticate(&token, TokenScheme::Bearer).await.unwrap_err();
            assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::NotAnAccessToken)));
        }
        for _ in 0..2 {
            let token = sign(&encoding_key, Some(&kid), &claims_with(serde_json::json!({ "aud": "other" })));
            let err = authenticator.authenticate(&token, TokenScheme::Bearer).await.unwrap_err();
            assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::AudienceMismatch)));
        }
        let text = logs.text();
        assert_eq!(text.lines().filter(|line| line.contains("not an access token")).count(), 1, "typ lines:\n{text}");
        assert_eq!(text.lines().filter(|line| line.contains("holds none of the accepted audiences")).count(), 1, "aud lines:\n{text}");
    }

    /// The SMA-690 log message (spec D8). Filter on this text, not on the SMA-686 text.
    const BINDING_REFUSAL: &str = "it is bound to a key, and IAM cannot check the binding";

    #[tokio::test]
    async fn sender_constrained_refusal_logs_issuer_and_cnf_marker_only() {
        // Spec § 5.1 test 14.
        let (logs, _guard) = capture_logs();
        let err = authenticate_json(&claims_with(serde_json::json!({ "cnf": { "jkt": "jkt-secret-value" } }))).await.unwrap_err();
        assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::SenderConstrained)), "got {err:?}");

        let text = logs.text();
        let lines: Vec<&str> = text.lines().filter(|line| line.contains(BINDING_REFUSAL)).collect();
        assert_eq!(lines.len(), 1, "exactly one binding refusal line expected, got:\n{text}");
        let line = lines[0];
        assert!(line.contains("INFO"), "the refusal logs at info: {line}");
        assert!(line.contains(ISSUER), "the refusal names the issuer: {line}");
        assert!(line.contains("\"cnf\"") || line.contains("=cnf"), "the refusal names the marker cnf: {line}");
        assert!(!text.contains("not an access token"), "the SMA-686 line must not appear:\n{text}");
        for secret in ["jkt-secret-value", "sub-1", "alice@example.com"] {
            assert!(!text.contains(secret), "the log must not contain {secret:?}:\n{text}");
        }
    }

    #[tokio::test]
    async fn dpop_typ_refusal_logs_canonical_marker() {
        // Spec § 5.1 test 15: the canonical marker, not the token's own spelling.
        let (logs, _guard) = capture_logs();
        let err = authenticate_json(&claims_with(serde_json::json!({ "typ": "dpop" }))).await.unwrap_err();
        assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::SenderConstrained)), "got {err:?}");

        let text = logs.text();
        let lines: Vec<&str> = text.lines().filter(|line| line.contains(BINDING_REFUSAL)).collect();
        assert_eq!(lines.len(), 1, "exactly one binding refusal line expected, got:\n{text}");
        assert!(lines[0].contains("typ DPoP"), "the refusal names the marker typ DPoP: {}", lines[0]);
        // Case-sensitive on purpose: the canonical text is "typ DPoP", which does not contain
        // "dpop". A quoted check (`"\"dpop\""`) only catches the quoted spelling; an unquoted
        // future log line (for example `typ=dpop`) would slip past it and stay undetected.
        assert!(!text.contains("dpop"), "the log must not contain the token's own spelling:\n{text}");
    }

    #[tokio::test]
    async fn repeated_sender_constrained_refusals_log_once() {
        // Spec § 5.1 test 16: the SMA-686 D14 rate limit covers the new defect.
        let (logs, _guard) = capture_logs();
        let (encoding_key, jwk, kid) = es256_keypair();
        let authenticator = make_authenticator(StubFetcher::new(jwk), vec![issuer_config(ISSUER, &["aud"])], 60, 16_384);
        for _ in 0..3 {
            let token = sign(&encoding_key, Some(&kid), &claims_with(serde_json::json!({ "cnf": { "jkt": "abc" } })));
            let err = authenticator.authenticate(&token, TokenScheme::Bearer).await.unwrap_err();
            assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::SenderConstrained)));
        }
        let text = logs.text();
        assert_eq!(text.lines().filter(|line| line.contains(BINDING_REFUSAL)).count(), 1, "binding lines:\n{text}");
    }

    // ---- SMA-703: configured ID-token marker claims ----------------------------------------
    //
    // The Zitadel fixtures copy the claim NAMES and value shapes of the measured tokens
    // (docs/superpowers/specs/2026-10-02-sma-703-zitadel-measurements.md, M1, M4a, M5b). The
    // times are relative to `Utc::now()`, because the measured `exp` values end on 2026-10-03.
    // `iss` is the test issuer. `aud` stays an array, so `WireAudience::Multiple` runs.

    /// The runbook recipe for Zitadel (spec D6).
    const ZITADEL_MARKERS: [&str; 2] = ["at_hash", "azp"];
    /// Measured ids: project P, the extra `aud` id (INFERRED: the app id of A), the client id of
    /// web app A, the human subject, the machine client id and the machine subject.
    const ZITADEL_PROJECT_ID: &str = "393381921683406851";
    const ZITADEL_APP_ID: &str = "393381921700315139";
    const ZITADEL_CLIENT_ID: &str = "393381921750515715";
    const ZITADEL_HUMAN_SUB: &str = "393381921784070147";
    const ZITADEL_MACHINE_CLIENT_ID: &str = "sma703-svc";
    const ZITADEL_MACHINE_SUB: &str = "393381990419660803";
    /// A second configured issuer, for the per-issuer test (T14).
    const SECOND_ISSUER: &str = "https://idp2.example.com";
    /// The SMA-686 refusal message, shared by the configured-claim refusal (spec D4).
    const NOT_ACCESS_TOKEN_REFUSAL: &str = "a verified marker shows it is not an access token";
    /// The SMA-703 boot line (spec D2).
    const MARKER_BOOT_LINE: &str = "carries one of the configured ID-token marker claims";

    /// The human-flow access token of M1 (and of the M4a refresh, with its own `jti`).
    fn zitadel_human_access_token(jti: &str) -> serde_json::Value {
        let now = Utc::now().timestamp();
        serde_json::json!({
            "iss": ISSUER,
            "sub": ZITADEL_HUMAN_SUB,
            "aud": [ZITADEL_APP_ID, ZITADEL_CLIENT_ID, ZITADEL_PROJECT_ID],
            "exp": now + 3600,
            "iat": now,
            "nbf": now,
            "client_id": ZITADEL_CLIENT_ID,
            "jti": jti,
        })
    }

    /// The human-flow ID token of M1 (and of the M4a refresh, with its own `at_hash`).
    fn zitadel_human_id_token(at_hash: &str) -> serde_json::Value {
        let now = Utc::now().timestamp();
        serde_json::json!({
            "iss": ISSUER,
            "sub": ZITADEL_HUMAN_SUB,
            "aud": [ZITADEL_APP_ID, ZITADEL_CLIENT_ID, ZITADEL_PROJECT_ID],
            "exp": now + 3600,
            "iat": now,
            "auth_time": now - 4,
            "nonce": "58e866bad23abebb",
            "amr": ["pwd"],
            "azp": ZITADEL_CLIENT_ID,
            "client_id": ZITADEL_CLIENT_ID,
            "at_hash": at_hash,
            "sid": "V1_393381929921019907",
        })
    }

    fn zitadel_m1_access_token() -> serde_json::Value {
        zitadel_human_access_token("V2_393381935289729027-at_393381935289794563")
    }

    fn zitadel_m1_id_token() -> serde_json::Value {
        zitadel_human_id_token("FFPzlMOE6pZPHZWKKJOObg")
    }

    fn zitadel_m4_access_token() -> serde_json::Value {
        zitadel_human_access_token("V2_393381935289729027-at_393381935306571779")
    }

    fn zitadel_m4_id_token() -> serde_json::Value {
        zitadel_human_id_token("9yO4kdn1jViUdIJ1kQrkjw")
    }

    /// The machine (client-credentials) access token of M5b: scope `openid` plus the P aud scope.
    fn zitadel_m5b_access_token() -> serde_json::Value {
        let now = Utc::now().timestamp();
        serde_json::json!({
            "iss": ISSUER,
            "sub": ZITADEL_MACHINE_SUB,
            "aud": [ZITADEL_PROJECT_ID],
            "exp": now + 3600,
            "iat": now,
            "nbf": now,
            "client_id": ZITADEL_MACHINE_CLIENT_ID,
            "jti": "V2_393381990436569091-at_393381990436634627",
        })
    }

    /// The machine ID token of M5b. No `nonce` and no `sid`: the client sent no nonce (F7).
    fn zitadel_m5b_id_token() -> serde_json::Value {
        let now = Utc::now().timestamp();
        serde_json::json!({
            "iss": ISSUER,
            "sub": ZITADEL_MACHINE_SUB,
            "aud": [ZITADEL_PROJECT_ID, ZITADEL_MACHINE_CLIENT_ID],
            "exp": now + 3600,
            "iat": now,
            "auth_time": now,
            "amr": ["pwd"],
            "azp": ZITADEL_MACHINE_CLIENT_ID,
            "client_id": ZITADEL_MACHINE_CLIENT_ID,
            "at_hash": "kYHj1WCq97WeD6uAjPviug",
        })
    }

    /// `base` with the members of `extra` added or replaced.
    fn merged(mut base: serde_json::Value, extra: serde_json::Value) -> serde_json::Value {
        let extra = extra.as_object().expect("extra claims are a JSON object").clone();
        base.as_object_mut().expect("claims are a JSON object").extend(extra);
        base
    }

    /// `base` without the member `name`.
    fn without(mut base: serde_json::Value, name: &str) -> serde_json::Value {
        base.as_object_mut().expect("claims are a JSON object").remove(name);
        base
    }

    fn issuer_with_markers(issuer: &str, audiences: &[&str], markers: &[&str]) -> IssuerConfig {
        IssuerConfig {
            id_token_marker_claims: markers.iter().map(|marker| (*marker).to_string()).collect(),
            ..issuer_config(issuer, audiences)
        }
    }

    /// Signs `claims` with a fresh key and authenticates it against `ISSUER`, configured with the
    /// audience `ZITADEL_PROJECT_ID` (runbook option 1) and the marker claims `markers`.
    async fn authenticate_zitadel(markers: &[&str], claims: &serde_json::Value) -> Result<ValidatedClaims, AuthnError> {
        let (encoding_key, jwk, kid) = es256_keypair();
        let token = sign(&encoding_key, Some(&kid), claims);
        let authenticator = make_authenticator(StubFetcher::new(jwk), vec![issuer_with_markers(ISSUER, &[ZITADEL_PROJECT_ID], markers)], 60, 16_384);
        authenticator.authenticate(&token, TokenScheme::Bearer).await
    }

    fn assert_not_an_access_token(result: Result<ValidatedClaims, AuthnError>, name: &str) {
        match result {
            Err(AuthnError::InvalidToken(TokenDefect::NotAnAccessToken)) => {}
            other => panic!("{name}: must be refused as NotAnAccessToken, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn refuses_zitadel_human_id_token() {
        // Spec § 5 T1.
        assert_not_an_access_token(authenticate_zitadel(&ZITADEL_MARKERS, &zitadel_m1_id_token()).await, "M1 ID token");
    }

    #[tokio::test]
    async fn refuses_zitadel_machine_id_token() {
        // Spec § 5 T2. The machine ID token has no nonce, so only the configured names refuse it.
        let claims = zitadel_m5b_id_token();
        assert!(claims.get("nonce").is_none(), "the M5b fixture has no nonce");
        assert_not_an_access_token(authenticate_zitadel(&ZITADEL_MARKERS, &claims).await, "M5b ID token");
    }

    #[tokio::test]
    async fn accepts_zitadel_access_tokens() {
        // Spec § 5 T3.
        let human = authenticate_zitadel(&ZITADEL_MARKERS, &zitadel_m1_access_token()).await.expect("the M1 access token must be accepted");
        assert_eq!(human.subject, ZITADEL_HUMAN_SUB);
        assert_eq!(human.audiences, vec![ZITADEL_APP_ID.to_string(), ZITADEL_CLIENT_ID.to_string(), ZITADEL_PROJECT_ID.to_string()]);
        let machine = authenticate_zitadel(&ZITADEL_MARKERS, &zitadel_m5b_access_token())
            .await
            .expect("the M5b access token must be accepted");
        assert_eq!(machine.subject, ZITADEL_MACHINE_SUB);
        assert_eq!(machine.audiences, vec![ZITADEL_PROJECT_ID.to_string()]);
    }

    #[tokio::test]
    async fn refresh_grant_tokens_keep_their_kind() {
        // Spec § 5 T4: the M4a refresh returns a new ID token and a new access token.
        assert_not_an_access_token(authenticate_zitadel(&ZITADEL_MARKERS, &zitadel_m4_id_token()).await, "M4 ID token");
        authenticate_zitadel(&ZITADEL_MARKERS, &zitadel_m4_access_token()).await.expect("the M4 access token must be accepted");
    }

    #[tokio::test]
    async fn empty_marker_list_keeps_the_sma_686_behaviour() {
        // Spec § 5 T5 and T6 (G2): with no configured names, a Zitadel ID token is accepted (the
        // open state that the setting closes), and so is the Dex shape.
        authenticate_zitadel(&[], &zitadel_m1_id_token())
            .await
            .expect("with an empty list the M1 ID token is accepted (open state)");
        let dex = merged(zitadel_m1_access_token(), serde_json::json!({ "at_hash": "x", "c_hash": "y", "nonce": "abc123" }));
        authenticate_zitadel(&[], &dex).await.expect("with an empty list the Dex shape is accepted");
    }

    #[tokio::test]
    async fn null_value_is_not_a_marker_and_any_other_value_is() {
        // Spec § 5 T7: the SMA-690 `cnf` rule.
        let null = merged(zitadel_m1_access_token(), serde_json::json!({ "at_hash": null }));
        authenticate_zitadel(&["at_hash"], &null).await.expect("at_hash: null is not a marker");
        for value in [serde_json::json!(""), serde_json::json!(0), serde_json::json!({}), serde_json::json!([]), serde_json::json!(false)] {
            let claims = merged(zitadel_m1_access_token(), serde_json::json!({ "at_hash": value.clone() }));
            assert_not_an_access_token(authenticate_zitadel(&["at_hash"], &claims).await, &format!("at_hash: {value}"));
        }
    }

    #[tokio::test]
    async fn only_configured_names_count() {
        // Spec § 5 T8: `azp` is present, but only `at_hash` is configured.
        let claims = without(zitadel_m1_id_token(), "at_hash");
        assert!(claims.get("azp").is_some(), "the fixture keeps azp");
        authenticate_zitadel(&["at_hash"], &claims).await.expect("an unconfigured name is not a marker");
    }

    #[tokio::test]
    async fn signature_and_claims_defects_come_first() {
        // Spec § 5 T9 (D3): `decode` validates before the marker check.
        let expired = merged(zitadel_m1_id_token(), serde_json::json!({ "exp": Utc::now().timestamp() - 120 }));
        let err = authenticate_zitadel(&ZITADEL_MARKERS, &expired).await.unwrap_err();
        assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::Expired)), "got {err:?}");
        let wrong_aud = merged(zitadel_m1_id_token(), serde_json::json!({ "aud": ["other-project"] }));
        let err = authenticate_zitadel(&ZITADEL_MARKERS, &wrong_aud).await.unwrap_err();
        assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::AudienceMismatch)), "got {err:?}");
    }

    #[tokio::test]
    async fn keycloak_typ_marker_runs_before_the_configured_claims() {
        // Spec § 5 T10: the SMA-686 marker wins, and the log names `ID`, not `claim at_hash`.
        let (logs, _guard) = capture_logs();
        let claims = merged(zitadel_m1_access_token(), serde_json::json!({ "typ": "ID", "at_hash": "x" }));
        assert_not_an_access_token(authenticate_zitadel(&ZITADEL_MARKERS, &claims).await, "typ ID with at_hash");
        let text = logs.text();
        let lines: Vec<&str> = text.lines().filter(|line| line.contains(NOT_ACCESS_TOKEN_REFUSAL)).collect();
        assert_eq!(lines.len(), 1, "exactly one refusal line expected, got:\n{text}");
        assert!(lines[0].contains("\"ID\"") || lines[0].contains("=ID"), "the marker is ID: {}", lines[0]);
        assert!(!text.contains("claim at_hash"), "the configured claim must not be the logged marker:\n{text}");
    }

    #[tokio::test]
    async fn configured_marker_runs_before_the_sender_constraint_check() {
        // Step 6b runs before step 7: a token with `at_hash` and `cnf` is refused as
        // NotAnAccessToken, and the log names `claim at_hash`, not the binding.
        let (logs, _guard) = capture_logs();
        let claims = merged(zitadel_m1_access_token(), serde_json::json!({ "at_hash": "x", "cnf": { "jkt": "x" } }));
        assert_not_an_access_token(authenticate_zitadel(&ZITADEL_MARKERS, &claims).await, "at_hash with cnf");
        let text = logs.text();
        let lines: Vec<&str> = text.lines().filter(|line| line.contains(NOT_ACCESS_TOKEN_REFUSAL)).collect();
        assert_eq!(lines.len(), 1, "exactly one refusal line expected, got:\n{text}");
        assert!(text.contains("claim at_hash"), "the configured claim is the logged marker:\n{text}");
        assert!(!text.contains(BINDING_REFUSAL), "the binding refusal must not run:\n{text}");
    }

    #[tokio::test]
    async fn configured_claim_refusal_logs_issuer_and_claim_name_only() {
        // Spec § 5 T11 (D4): issuer and `claim at_hash`; no claim value, subject or email; one
        // line for three refusals (the SMA-686 D14 rate limit).
        let (logs, _guard) = capture_logs();
        let (encoding_key, jwk, kid) = es256_keypair();
        let authenticator = make_authenticator(StubFetcher::new(jwk), vec![issuer_with_markers(ISSUER, &[ZITADEL_PROJECT_ID], &ZITADEL_MARKERS)], 60, 16_384);
        let claims = merged(zitadel_m1_id_token(), serde_json::json!({ "email": "alice@example.com" }));
        for _ in 0..3 {
            let token = sign(&encoding_key, Some(&kid), &claims);
            assert_not_an_access_token(authenticator.authenticate(&token, TokenScheme::Bearer).await, "M1 ID token");
        }
        let text = logs.text();
        let lines: Vec<&str> = text.lines().filter(|line| line.contains(NOT_ACCESS_TOKEN_REFUSAL)).collect();
        assert_eq!(lines.len(), 1, "exactly one refusal line expected, got:\n{text}");
        let line = lines[0];
        assert!(line.contains("INFO"), "the refusal logs at info: {line}");
        assert!(line.contains(ISSUER), "the refusal names the issuer: {line}");
        assert!(line.contains("claim at_hash"), "the refusal names the first configured claim: {line}");
        for secret in ["FFPzlMOE6pZPHZWKKJOObg", ZITADEL_HUMAN_SUB, ZITADEL_CLIENT_ID, "58e866bad23abebb", "alice@example.com"] {
            assert!(!text.contains(secret), "the log must not contain {secret:?}:\n{text}");
        }
    }

    #[tokio::test]
    async fn wrong_recipe_fails_closed_and_names_the_claim() {
        // Review Focus 4 (spec § 6): the Zitadel recipe on an IdP whose access token carries `azp`
        // (a Keycloak access token: `typ: Bearer`, `azp`, no `at_hash`) refuses that access token.
        // The log names `claim azp`, so the operator sees which name is wrong.
        let (logs, _guard) = capture_logs();
        let keycloak_access = merged(zitadel_m1_access_token(), serde_json::json!({ "typ": "Bearer", "azp": "paigasus-console" }));
        assert_not_an_access_token(authenticate_zitadel(&ZITADEL_MARKERS, &keycloak_access).await, "Keycloak access token");
        let text = logs.text();
        assert!(text.contains("claim azp"), "the log names the matched claim:\n{text}");
        assert!(!text.contains("paigasus-console"), "the log must not contain the azp value:\n{text}");
    }

    /// Signs a raw payload JSON string by hand, so a test can repeat a member name (neither
    /// `json!` nor `jsonwebtoken::encode` can emit a repeated key).
    fn sign_raw_payload(encoding_key: &EncodingKey, kid: &str, payload_json: &str) -> String {
        let header_json = format!(r#"{{"alg":"ES256","typ":"JWT","kid":"{kid}"}}"#);
        let header_b64 = URL_SAFE_NO_PAD.encode(header_json.as_bytes());
        let payload_b64 = URL_SAFE_NO_PAD.encode(payload_json.as_bytes());
        let message = format!("{header_b64}.{payload_b64}");
        let signature = jsonwebtoken::crypto::sign(message.as_bytes(), encoding_key, Algorithm::ES256).expect("signing a test token");
        format!("{message}.{signature}")
    }

    /// Authenticates the raw `payload` JSON string against `ISSUER`, configured with the audience
    /// `aud` and the marker claims `markers`.
    async fn authenticate_raw_payload(markers: &[&str], payload: &str) -> Result<ValidatedClaims, AuthnError> {
        let (encoding_key, jwk, kid) = es256_keypair();
        let token = sign_raw_payload(&encoding_key, &kid, payload);
        let authenticator = make_authenticator(StubFetcher::new(jwk), vec![issuer_with_markers(ISSUER, &["aud"], markers)], 60, 16_384);
        authenticator.authenticate(&token, TokenScheme::Bearer).await
    }

    /// Authenticates a payload of the usual test fields (`ISSUER`, aud `aud`, one hour) followed
    /// by `members` verbatim, against `ISSUER` configured with `markers`.
    async fn authenticate_raw(markers: &[&str], members: &str) -> Result<ValidatedClaims, AuthnError> {
        let exp = Utc::now().timestamp() + 3600;
        let payload = format!(r#"{{"iss":"{ISSUER}","sub":"sub-1","aud":"aud","exp":{exp},"email":"alice@example.com",{members}}}"#);
        authenticate_raw_payload(markers, &payload).await
    }

    #[tokio::test]
    async fn duplicate_member_is_malformed_on_the_strict_path() {
        // Spec § 5 T12 (D3). A plain map would keep the last value, so `null` last would hide
        // the marker. Both orders are Malformed. A duplicate `cnf` is Malformed on both paths.
        for (name, markers, members) in [
            ("at_hash string then null", &ZITADEL_MARKERS[..], r#""at_hash":"x","at_hash":null"#),
            ("at_hash null then string", &ZITADEL_MARKERS[..], r#""at_hash":null,"at_hash":"x""#),
            ("cnf twice, list set", &ZITADEL_MARKERS[..], r#""cnf":{"jkt":"abc"},"cnf":null"#),
            ("cnf twice, list empty", &[][..], r#""cnf":{"jkt":"abc"},"cnf":null"#),
            ("sub twice, list set", &ZITADEL_MARKERS[..], r#""sub":"sub-2""#),
        ] {
            let err = authenticate_raw(markers, members).await.unwrap_err();
            assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::Malformed)), "{name}: got {err:?}");
        }
    }

    #[tokio::test]
    async fn duplicate_unknown_member_is_unchanged_on_the_plain_path() {
        // Spec § 7 / G2: with an empty list the SMA-686 decode runs unchanged, and that decode
        // ignores a repeated member it does not read. This pins that the new code did not move
        // an issuer without marker claims onto the strict path.
        authenticate_raw(&[], r#""at_hash":"x","at_hash":null"#).await.expect("the plain path is unchanged");
    }

    #[tokio::test]
    async fn strict_path_keeps_the_defect_order() {
        // Review Focus 3: `StrictPayload` reads `WireClaims` inside its own `Deserialize`, so a
        // wrong-shaped claim is Malformed BEFORE `jsonwebtoken` validates `exp` and `aud`, on both
        // paths. A parse after `decode` would turn `aud: 7` into AudienceMismatch (and log it),
        // and an expired token without `sub` into Expired.
        let future = Utc::now().timestamp() + 3600;
        let past = Utc::now().timestamp() - 120;
        let cases = [
            ("aud a number", format!(r#"{{"iss":"{ISSUER}","sub":"sub-1","aud":7,"exp":{future}}}"#), TokenDefect::Malformed),
            ("expired and no sub", format!(r#"{{"iss":"{ISSUER}","aud":"aud","exp":{past}}}"#), TokenDefect::Malformed),
            (
                "name a number",
                format!(r#"{{"iss":"{ISSUER}","sub":"sub-1","aud":"aud","exp":{future},"name":7}}"#),
                TokenDefect::Malformed,
            ),
            ("aud null", format!(r#"{{"iss":"{ISSUER}","sub":"sub-1","aud":null,"exp":{future}}}"#), TokenDefect::AudienceMismatch),
            ("expired", format!(r#"{{"iss":"{ISSUER}","sub":"sub-1","aud":"aud","exp":{past}}}"#), TokenDefect::Expired),
        ];
        for markers in [&ZITADEL_MARKERS[..], &[][..]] {
            for (name, payload, want) in &cases {
                let err = authenticate_raw_payload(markers, payload).await.unwrap_err();
                assert!(
                    matches!(&err, AuthnError::InvalidToken(defect) if defect == want),
                    "{name}, markers {markers:?}: want {want:?}, got {err:?}"
                );
            }
        }
    }

    #[tokio::test]
    async fn marker_names_are_case_sensitive() {
        // Spec § 5 T13 (D2).
        let claims = merged(zitadel_m1_access_token(), serde_json::json!({ "AT_HASH": "x" }));
        authenticate_zitadel(&["at_hash"], &claims).await.expect("AT_HASH is not at_hash");
    }

    #[tokio::test]
    async fn marker_list_is_per_issuer() {
        // Spec § 5 T14: one issuer with `["at_hash"]`, one with an empty list, one key for both.
        let (encoding_key, jwk, kid) = es256_keypair();
        let authenticator = make_authenticator(
            StubFetcher::new(jwk),
            vec![issuer_with_markers(ISSUER, &[ZITADEL_PROJECT_ID], &["at_hash"]), issuer_config(SECOND_ISSUER, &[ZITADEL_PROJECT_ID])],
            60,
            16_384,
        );
        let first = sign(&encoding_key, Some(&kid), &zitadel_m1_id_token());
        assert_not_an_access_token(authenticator.authenticate(&first, TokenScheme::Bearer).await, "ID token of the first issuer");
        let second = sign(&encoding_key, Some(&kid), &merged(zitadel_m1_id_token(), serde_json::json!({ "iss": SECOND_ISSUER })));
        let validated = authenticator.authenticate(&second, TokenScheme::Bearer).await.expect("the second issuer has no marker claims");
        assert_eq!(validated.issuer.as_str(), SECOND_ISSUER);
    }

    #[test]
    fn boot_line_names_the_issuer_and_the_marker_claims() {
        // Spec § 5 T15 (D2): one info line for an issuer with marker claims; none for an empty
        // list. `capture_logs` installs a thread-local subscriber; `new` is synchronous.
        let (logs, _guard) = capture_logs();
        let (_encoding_key, jwk, _kid) = es256_keypair();
        let _with = make_authenticator(
            StubFetcher::new(jwk.clone()),
            vec![issuer_with_markers(ISSUER, &["aud"], &ZITADEL_MARKERS), issuer_config(SECOND_ISSUER, &["aud"])],
            60,
            16_384,
        );
        let text = logs.text();
        let lines: Vec<&str> = text.lines().filter(|line| line.contains(MARKER_BOOT_LINE)).collect();
        assert_eq!(lines.len(), 1, "exactly one boot line expected, got:\n{text}");
        let line = lines[0];
        assert!(line.contains("INFO"), "the boot line logs at info: {line}");
        assert!(line.contains(ISSUER), "the boot line names the issuer: {line}");
        assert!(line.contains("at_hash") && line.contains("azp"), "the boot line names the claims: {line}");
        assert!(!line.contains(SECOND_ISSUER), "the boot line names only the issuer with claims: {line}");

        let (logs, _guard) = capture_logs();
        let _without = make_authenticator(StubFetcher::new(jwk), vec![issuer_config(ISSUER, &["aud"])], 60, 16_384);
        assert!(!logs.text().contains(MARKER_BOOT_LINE), "no boot line for an empty list:\n{}", logs.text());
    }

    // ---- SMA-731: configured required claims ------------------------------------------------
    //
    // The fixtures are the SMA-703 Zitadel fixtures above (M1, M4a, M5b). Every Zitadel access
    // token has `jti`, and no Zitadel ID token has it (SMA-731 spec § 3, F1).

    /// The runbook recipe for Zitadel (SMA-731 spec D6).
    const REQUIRED_JTI: [&str; 1] = ["jti"];
    /// The SMA-731 refusal message (spec D4). It differs from `NOT_ACCESS_TOKEN_REFUSAL`.
    const MISSING_CLAIM_REFUSAL: &str = "it does not carry a claim that the issuer configuration requires";
    /// The SMA-731 boot line (spec D2).
    const REQUIRED_BOOT_LINE: &str = "does not carry every configured required claim";

    fn issuer_with_rules(issuer: &str, audiences: &[&str], markers: &[&str], required: &[&str]) -> IssuerConfig {
        IssuerConfig {
            access_token_required_claims: required.iter().map(|name| (*name).to_string()).collect(),
            ..issuer_with_markers(issuer, audiences, markers)
        }
    }

    /// Signs `claims` with a fresh key and authenticates it on `scheme` against `ISSUER`,
    /// configured with the audience `ZITADEL_PROJECT_ID`, the marker claims `markers` and the
    /// required claims `required`.
    async fn authenticate_rules_as(markers: &[&str], required: &[&str], claims: &serde_json::Value, scheme: TokenScheme) -> Result<ValidatedClaims, AuthnError> {
        let (encoding_key, jwk, kid) = es256_keypair();
        let token = sign(&encoding_key, Some(&kid), claims);
        let authenticator = make_authenticator(StubFetcher::new(jwk), vec![issuer_with_rules(ISSUER, &[ZITADEL_PROJECT_ID], markers, required)], 60, 16_384);
        authenticator.authenticate(&token, scheme).await
    }

    async fn authenticate_rules(markers: &[&str], required: &[&str], claims: &serde_json::Value) -> Result<ValidatedClaims, AuthnError> {
        authenticate_rules_as(markers, required, claims, TokenScheme::Bearer).await
    }

    /// Authenticates a raw payload (the usual test fields, then `members` verbatim) against
    /// `ISSUER` with the audience `aud`, the marker claims `markers` and the required claims
    /// `required`. A raw payload can repeat a member name.
    async fn authenticate_raw_rules(markers: &[&str], required: &[&str], members: &str) -> Result<ValidatedClaims, AuthnError> {
        let exp = Utc::now().timestamp() + 3600;
        let payload = format!(r#"{{"iss":"{ISSUER}","sub":"sub-1","aud":"aud","exp":{exp},"email":"alice@example.com",{members}}}"#);
        let (encoding_key, jwk, kid) = es256_keypair();
        let token = sign_raw_payload(&encoding_key, &kid, &payload);
        let authenticator = make_authenticator(StubFetcher::new(jwk), vec![issuer_with_rules(ISSUER, &["aud"], markers, required)], 60, 16_384);
        authenticator.authenticate(&token, TokenScheme::Bearer).await
    }

    #[tokio::test]
    async fn required_claim_refuses_zitadel_id_tokens() {
        // SMA-731 T1: with ["jti"] and NO marker claims, each Zitadel ID token is refused.
        for (name, claims) in [("M1 ID token", zitadel_m1_id_token()), ("M4 ID token", zitadel_m4_id_token()), ("M5b ID token", zitadel_m5b_id_token())] {
            assert_not_an_access_token(authenticate_rules(&[], &REQUIRED_JTI, &claims).await, name);
        }
    }

    #[tokio::test]
    async fn required_claim_accepts_zitadel_access_tokens() {
        // SMA-731 T2.
        for (name, claims, subject) in [
            ("M1 access token", zitadel_m1_access_token(), ZITADEL_HUMAN_SUB),
            ("M4 access token", zitadel_m4_access_token(), ZITADEL_HUMAN_SUB),
            ("M5b access token", zitadel_m5b_access_token(), ZITADEL_MACHINE_SUB),
        ] {
            let validated = authenticate_rules(&[], &REQUIRED_JTI, &claims)
                .await
                .unwrap_or_else(|err| panic!("{name}: must be accepted, got {err:?}"));
            assert_eq!(validated.subject, subject, "{name}");
        }
    }

    #[tokio::test]
    async fn null_required_claim_is_missing_and_any_other_value_is_present() {
        // SMA-731 T3 (D3): the SMA-703 null rule with the opposite result.
        let null = merged(zitadel_m1_access_token(), serde_json::json!({ "jti": null }));
        assert_not_an_access_token(authenticate_rules(&[], &REQUIRED_JTI, &null).await, "jti: null");
        for value in [serde_json::json!(""), serde_json::json!(0), serde_json::json!(false), serde_json::json!({}), serde_json::json!([])] {
            let claims = merged(zitadel_m1_access_token(), serde_json::json!({ "jti": value.clone() }));
            authenticate_rules(&[], &REQUIRED_JTI, &claims)
                .await
                .unwrap_or_else(|err| panic!("jti: {value} must count as present, got {err:?}"));
        }
    }

    #[tokio::test]
    async fn the_first_missing_required_claim_is_logged() {
        // SMA-731 T4: with ["jti", "nbf"], a token with `jti` and no `nbf` is refused, and the log
        // names `nbf`.
        let (logs, _guard) = capture_logs();
        let claims = without(zitadel_m1_access_token(), "nbf");
        assert_not_an_access_token(authenticate_rules(&[], &["jti", "nbf"], &claims).await, "no nbf");
        let text = logs.text();
        assert!(text.contains("missing claim nbf"), "the log names the missing claim:\n{text}");
        assert!(!text.contains("missing claim jti"), "jti is present:\n{text}");
    }

    #[tokio::test]
    async fn required_claim_names_are_case_sensitive() {
        // SMA-731 T5 (D2): `JTI` is not `jti`.
        let claims = merged(without(zitadel_m1_access_token(), "jti"), serde_json::json!({ "JTI": "x" }));
        assert_not_an_access_token(authenticate_rules(&[], &REQUIRED_JTI, &claims).await, "JTI only");
    }

    #[tokio::test]
    async fn empty_claim_lists_accept_the_zitadel_id_token() {
        // SMA-731 T6 (G2): with both lists empty, nothing changes. This is the open state.
        authenticate_rules(&[], &[], &zitadel_m1_id_token())
            .await
            .expect("with both lists empty the M1 ID token is accepted (open state)");
    }

    #[tokio::test]
    async fn decode_defects_come_before_the_required_claim_check() {
        // SMA-731 T7 (D3): the decode runs before step 6c.
        let expired = merged(zitadel_m1_id_token(), serde_json::json!({ "exp": Utc::now().timestamp() - 120 }));
        let err = authenticate_rules(&[], &REQUIRED_JTI, &expired).await.unwrap_err();
        assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::Expired)), "got {err:?}");
        let wrong_aud = merged(zitadel_m1_id_token(), serde_json::json!({ "aud": ["other-project"] }));
        let err = authenticate_rules(&[], &REQUIRED_JTI, &wrong_aud).await.unwrap_err();
        assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::AudienceMismatch)), "got {err:?}");
    }

    #[tokio::test]
    async fn keycloak_typ_marker_runs_before_the_required_claims() {
        // SMA-731 T8: step 6 runs first, so a Keycloak `typ: ID` token without `jti` logs `ID`.
        let (logs, _guard) = capture_logs();
        let claims = merged(without(zitadel_m1_access_token(), "jti"), serde_json::json!({ "typ": "ID" }));
        assert_not_an_access_token(authenticate_rules(&[], &REQUIRED_JTI, &claims).await, "typ ID without jti");
        let text = logs.text();
        let lines: Vec<&str> = text.lines().filter(|line| line.contains(NOT_ACCESS_TOKEN_REFUSAL)).collect();
        assert_eq!(lines.len(), 1, "exactly one refusal line expected, got:\n{text}");
        assert!(lines[0].contains("\"ID\"") || lines[0].contains("=ID"), "the marker is ID: {}", lines[0]);
        assert!(!text.contains("missing claim"), "step 6c must not run:\n{text}");
    }

    #[tokio::test]
    async fn marker_claim_runs_before_the_required_claim() {
        // SMA-731 T9: with both settings, the M1 ID token logs `claim at_hash` (step 6b), not
        // `missing claim jti`, and the M1 access token passes.
        let (logs, _guard) = capture_logs();
        assert_not_an_access_token(authenticate_rules(&ZITADEL_MARKERS, &REQUIRED_JTI, &zitadel_m1_id_token()).await, "M1 ID token");
        let text = logs.text();
        assert!(text.contains("claim at_hash"), "step 6b names the marker claim:\n{text}");
        assert!(!text.contains("missing claim jti"), "step 6c must not run:\n{text}");
        authenticate_rules(&ZITADEL_MARKERS, &REQUIRED_JTI, &zitadel_m1_access_token())
            .await
            .expect("the M1 access token passes the full recipe");
    }

    #[tokio::test]
    async fn required_claim_runs_before_the_sender_constraint_check() {
        // SMA-731 T10: a bound token without `jti` is NotAnAccessToken, not SenderConstrained.
        let (logs, _guard) = capture_logs();
        let claims = merged(without(zitadel_m1_access_token(), "jti"), serde_json::json!({ "cnf": { "jkt": JKT } }));
        assert_not_an_access_token(authenticate_rules(&[], &REQUIRED_JTI, &claims).await, "cnf without jti");
        let text = logs.text();
        assert!(text.contains("missing claim jti"), "step 6c names the claim:\n{text}");
        assert!(!text.contains(BINDING_REFUSAL), "the binding refusal must not run:\n{text}");
    }

    #[tokio::test]
    async fn duplicate_required_claim_member_is_malformed() {
        // SMA-731 T11 (D3): the required list alone moves the issuer onto the strict decode, so a
        // `null` copy cannot hide or fake the claim. Both orders are Malformed.
        for members in [r#""jti":null,"jti":"x""#, r#""jti":"x","jti":null"#] {
            let err = authenticate_raw_rules(&[], &REQUIRED_JTI, members).await.unwrap_err();
            assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::Malformed)), "{members}: got {err:?}");
        }
    }

    #[tokio::test]
    async fn unread_duplicate_member_is_malformed_once_a_required_claim_is_set() {
        // Review Focus 3: a repeated member that IAM does not read passes the plain decode. With
        // only the required list set, the strict decode refuses it as Malformed.
        authenticate_raw_rules(&[], &[], r#""jti":"x","foo":1,"foo":2"#)
            .await
            .expect("the plain path ignores a repeated unread member");
        let err = authenticate_raw_rules(&[], &REQUIRED_JTI, r#""jti":"x","foo":1,"foo":2"#).await.unwrap_err();
        assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::Malformed)), "got {err:?}");
    }

    #[tokio::test]
    async fn required_claims_are_per_issuer() {
        // SMA-731 T12: one issuer with ["jti"], one with an empty list, one key for both.
        let (encoding_key, jwk, kid) = es256_keypair();
        let authenticator = make_authenticator(
            StubFetcher::new(jwk),
            vec![
                issuer_with_rules(ISSUER, &[ZITADEL_PROJECT_ID], &[], &REQUIRED_JTI),
                issuer_config(SECOND_ISSUER, &[ZITADEL_PROJECT_ID]),
            ],
            60,
            16_384,
        );
        let first = sign(&encoding_key, Some(&kid), &zitadel_m1_id_token());
        assert_not_an_access_token(authenticator.authenticate(&first, TokenScheme::Bearer).await, "ID token of the first issuer");
        let second = sign(&encoding_key, Some(&kid), &merged(zitadel_m1_id_token(), serde_json::json!({ "iss": SECOND_ISSUER })));
        let validated = authenticator.authenticate(&second, TokenScheme::Bearer).await.expect("the second issuer has no required claims");
        assert_eq!(validated.issuer.as_str(), SECOND_ISSUER);
    }

    #[tokio::test]
    async fn missing_claim_refusal_logs_its_own_message_issuer_and_name_only() {
        // SMA-731 T13 (D4): its own message, the issuer and `missing claim jti`; no claim value,
        // subject or email; one line for three refusals.
        let (logs, _guard) = capture_logs();
        let (encoding_key, jwk, kid) = es256_keypair();
        let authenticator = make_authenticator(StubFetcher::new(jwk), vec![issuer_with_rules(ISSUER, &[ZITADEL_PROJECT_ID], &[], &REQUIRED_JTI)], 60, 16_384);
        let claims = merged(zitadel_m1_id_token(), serde_json::json!({ "email": "alice@example.com" }));
        for _ in 0..3 {
            let token = sign(&encoding_key, Some(&kid), &claims);
            assert_not_an_access_token(authenticator.authenticate(&token, TokenScheme::Bearer).await, "M1 ID token");
        }
        let text = logs.text();
        let lines: Vec<&str> = text.lines().filter(|line| line.contains(MISSING_CLAIM_REFUSAL)).collect();
        assert_eq!(lines.len(), 1, "exactly one refusal line expected, got:\n{text}");
        let line = lines[0];
        assert!(line.contains("INFO"), "the refusal logs at info: {line}");
        assert!(line.contains(ISSUER), "the refusal names the issuer: {line}");
        assert!(line.contains("missing claim jti"), "the refusal names the missing claim: {line}");
        assert!(!text.contains(NOT_ACCESS_TOKEN_REFUSAL), "the MissingClaim arm has its own message:\n{text}");
        for secret in [
            "FFPzlMOE6pZPHZWKKJOObg",
            ZITADEL_HUMAN_SUB,
            ZITADEL_CLIENT_ID,
            "58e866bad23abebb",
            "V1_393381929921019907",
            "alice@example.com",
        ] {
            assert!(!text.contains(secret), "the log must not contain {secret:?}:\n{text}");
        }
    }

    #[tokio::test]
    async fn missing_claim_shares_the_rate_limit_of_the_marker_refusals() {
        // Review Focus 1 (D4): the rate-limit key is (issuer, NotAnAccessToken) for steps 6, 6b and
        // 6c. A marker refusal and then a missing-claim refusal within 10 s give ONE line, the
        // first. The runbook tells the operator that one line can stand for many refusals.
        let (logs, _guard) = capture_logs();
        let (encoding_key, jwk, kid) = es256_keypair();
        let authenticator = make_authenticator(
            StubFetcher::new(jwk),
            vec![issuer_with_rules(ISSUER, &[ZITADEL_PROJECT_ID], &ZITADEL_MARKERS, &REQUIRED_JTI)],
            60,
            16_384,
        );
        let id_token = sign(&encoding_key, Some(&kid), &zitadel_m1_id_token());
        assert_not_an_access_token(authenticator.authenticate(&id_token, TokenScheme::Bearer).await, "M1 ID token");
        let no_jti = sign(&encoding_key, Some(&kid), &without(zitadel_m1_access_token(), "jti"));
        assert_not_an_access_token(authenticator.authenticate(&no_jti, TokenScheme::Bearer).await, "access token without jti");
        let text = logs.text();
        let lines: Vec<&str> = text.lines().filter(|line| line.contains(NOT_ACCESS_TOKEN_REFUSAL) || line.contains(MISSING_CLAIM_REFUSAL)).collect();
        assert_eq!(lines.len(), 1, "one line for the two refusals, got:\n{text}");
        assert!(lines[0].contains("claim at_hash"), "the first refusal is the logged one: {}", lines[0]);
    }

    #[test]
    fn boot_line_names_the_issuer_and_the_required_claims() {
        // SMA-731 T14 (D2): one info line for an issuer with required claims; none for an empty
        // list. The SMA-703 boot line does not show for an empty marker list.
        let (logs, _guard) = capture_logs();
        let (_encoding_key, jwk, _kid) = es256_keypair();
        let _with = make_authenticator(
            StubFetcher::new(jwk.clone()),
            vec![issuer_with_rules(ISSUER, &["aud"], &[], &REQUIRED_JTI), issuer_config(SECOND_ISSUER, &["aud"])],
            60,
            16_384,
        );
        let text = logs.text();
        let lines: Vec<&str> = text.lines().filter(|line| line.contains(REQUIRED_BOOT_LINE)).collect();
        assert_eq!(lines.len(), 1, "exactly one boot line expected, got:\n{text}");
        let line = lines[0];
        assert!(line.contains("INFO"), "the boot line logs at info: {line}");
        assert!(line.contains(ISSUER), "the boot line names the issuer: {line}");
        assert!(line.contains("jti"), "the boot line names the claims: {line}");
        assert!(!line.contains(SECOND_ISSUER), "the boot line names only the issuer with claims: {line}");
        assert!(!text.contains(MARKER_BOOT_LINE), "no marker boot line for an empty marker list:\n{text}");

        let (logs, _guard) = capture_logs();
        let _without = make_authenticator(StubFetcher::new(jwk), vec![issuer_config(ISSUER, &["aud"])], 60, 16_384);
        assert!(!logs.text().contains(REQUIRED_BOOT_LINE), "no boot line for an empty list:\n{}", logs.text());
    }

    #[tokio::test]
    async fn dpop_scheme_refuses_a_bound_token_without_the_required_claim() {
        // SMA-731 T14a (D3): the check is in `authenticate`, so it applies to the Dpop scheme too.
        // The validator does not see the proof: the caller checks it after `authenticate` returns
        // the binding (`application::dpop`), so a refusal here comes before any proof check.
        let bound = merged(zitadel_m1_access_token(), serde_json::json!({ "cnf": { "jkt": JKT } }));
        assert_not_an_access_token(
            authenticate_rules_as(&[], &REQUIRED_JTI, &without(bound.clone(), "jti"), TokenScheme::Dpop).await,
            "bound token without jti",
        );
        let validated = authenticate_rules_as(&[], &REQUIRED_JTI, &bound, TokenScheme::Dpop)
            .await
            .expect("a bound token with jti passes the Dpop scheme");
        assert_eq!(validated.key_binding, Some(Jkt::new(JKT)));
    }

    fn claim_rules(markers: &[&str], required: &[&str]) -> ClaimRules {
        ClaimRules {
            id_token_markers: markers.iter().map(|name| (*name).to_string()).collect(),
            required: required.iter().map(|name| (*name).to_string()).collect(),
        }
    }

    fn members(value: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
        value.as_object().expect("members are a JSON object").clone()
    }

    #[test]
    fn claim_rules_need_a_strict_decode_when_either_list_is_set() {
        // SMA-731 T14b (D3).
        assert!(!claim_rules(&[], &[]).needs_strict_decode());
        assert!(claim_rules(&["at_hash"], &[]).needs_strict_decode());
        assert!(claim_rules(&[], &["jti"]).needs_strict_decode());
        assert!(claim_rules(&["at_hash"], &["jti"]).needs_strict_decode());
    }

    #[test]
    fn claim_rules_refusal_checks_markers_first_then_the_required_names() {
        // SMA-731 T14b (D3): marker-first order, the null rule and case-sensitivity, on a plain map.
        let rules = claim_rules(&["at_hash", "azp"], &["jti", "nbf"]);
        let cases = [
            ("a marker wins over a missing name", serde_json::json!({ "at_hash": "x" }), Some(RefusalDetail::Claim("at_hash"))),
            (
                "the first configured marker",
                serde_json::json!({ "azp": "c", "at_hash": "x", "jti": "j", "nbf": 1 }),
                Some(RefusalDetail::Claim("at_hash")),
            ),
            ("the first missing name", serde_json::json!({}), Some(RefusalDetail::MissingClaim("jti"))),
            ("the second missing name", serde_json::json!({ "jti": "j" }), Some(RefusalDetail::MissingClaim("nbf"))),
            ("a null marker is absent", serde_json::json!({ "at_hash": null, "jti": "j", "nbf": 1 }), None),
            (
                "a null required claim is missing",
                serde_json::json!({ "jti": null, "nbf": 1 }),
                Some(RefusalDetail::MissingClaim("jti")),
            ),
            ("any other value is present", serde_json::json!({ "jti": "", "nbf": false }), None),
            (
                "names are case-sensitive",
                serde_json::json!({ "AT_HASH": "x", "JTI": "j", "nbf": 1 }),
                Some(RefusalDetail::MissingClaim("jti")),
            ),
        ];
        for (name, value, want) in cases {
            assert_eq!(rules.refusal(&members(value)), want, "{name}");
        }
        assert_eq!(claim_rules(&[], &[]).refusal(&members(serde_json::json!({ "at_hash": "x" }))), None, "no rules, no refusal");
    }
}
