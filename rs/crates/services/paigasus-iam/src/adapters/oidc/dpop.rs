// SPDX-License-Identifier: Apache-2.0

//! `JoseDpopProofChecker` (SMA-700 D2, § 4.4): the stateless checks 1-9 of a DPoP proof
//! (RFC 9449 § 4.3) behind the core `DpopProofChecker` port. The proof is split and decoded by
//! hand, not with `jsonwebtoken::decode_header`: that call fails on `alg: none` before check 4 can
//! name it, and `jsonwebtoken`'s `Jwk` type drops private members that check 5 must see. The key is
//! built from the members that check 5 validated, and `jsonwebtoken::crypto::verify` checks the
//! raw signature, with no `Validation` flags. The request checks (`htm`, `htu`, `iat`, replay) are
//! in `application::dpop`. Nothing here logs.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use jsonwebtoken::{Algorithm, DecodingKey};
use paigasus_iam_core::{DpopProofChecker, FollowUpClaims, Jkt, MAX_PROOF_BYTES, ProofClaims, ProofDefect};
use serde::Deserialize;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

/// Check 3 (D16): compared ASCII case-insensitively (RFC 7515 § 4.1.9).
const PROOF_TYPES: [&str; 2] = ["dpop+jwt", "application/dpop+jwt"];
/// Check 5: a public key has none of these members.
const PRIVATE_JWK_MEMBERS: [&str; 8] = ["d", "p", "q", "dp", "dq", "qi", "oth", "k"];
/// Check 7: `jti` is 1 to 256 bytes.
const MAX_JTI_BYTES: usize = 256;
/// Check 5: the RSA modulus size.
const RSA_BITS: std::ops::RangeInclusive<usize> = 2048..=4096;
/// Check 5: the largest RSA public exponent that the `rsa` 0.9 crate accepts.
const MAX_RSA_EXPONENT: u64 = (1 << 33) - 1;

/// The `DpopProofChecker` v1 implementation. Stateless.
#[derive(Debug, Clone, Copy, Default)]
pub struct JoseDpopProofChecker;

/// The three parts of a proof after checks 1 and 2.
struct RawProof<'a> {
    /// `header.payload`, the bytes the signature covers.
    signing_input: &'a str,
    signature: &'a str,
    header: Map<String, Value>,
    payload: Map<String, Value>,
}

/// The public key of a proof, from the `jwk` members that check 5 validated. The string members
/// are strict base64url, so the RFC 7638 canonical JSON needs no escapes.
enum PublicJwk {
    Ec { x: String, y: String },
    Rsa { n: String, e: String, n_bytes: Vec<u8>, e_bytes: Vec<u8> },
}

/// The claims of check 7.
struct WireProofClaims {
    jti: String,
    iat: i64,
    htm: String,
    htu: String,
    ath: String,
}

impl DpopProofChecker for JoseDpopProofChecker {
    fn check(&self, proof: &str, token: &str, jkt: &Jkt) -> Result<ProofClaims, ProofDefect> {
        let raw = parse(proof)?; // checks 1 and 2
        check_typ(&raw.header)?; // check 3
        let alg = check_alg(&raw.header)?; // check 4
        let key = check_jwk(&raw.header, alg)?; // check 5
        verify_signature(&raw, &key, alg)?; // check 6
        let claims = read_claims(&raw.payload)?; // check 7
        if claims.ath != ath_of(token) {
            return Err(ProofDefect::Ath); // check 8
        }
        if thumbprint(&key) != jkt.as_str() {
            return Err(ProofDefect::Thumbprint); // check 9
        }
        Ok(ProofClaims {
            jti: claims.jti,
            iat: claims.iat,
            htm: claims.htm,
            htu: claims.htu,
        })
    }

    fn follow_up_claims(&self, proof: &str) -> Result<FollowUpClaims, ProofDefect> {
        let raw = parse(proof)?;
        Ok(FollowUpClaims {
            jti: read_jti(&raw.payload)?,
            ath: read_string(&raw.payload, "ath")?,
        })
    }

    fn ath_matches(&self, ath: &str, token: &str) -> bool {
        ath == ath_of(token)
    }
}

/// Checks 1 and 2: at most `MAX_PROOF_BYTES`; three non-empty base64url parts with no padding;
/// the header and the payload are JSON objects with unique member names (decision P10).
fn parse(proof: &str) -> Result<RawProof<'_>, ProofDefect> {
    if proof.len() > MAX_PROOF_BYTES {
        return Err(ProofDefect::Malformed);
    }
    let (signing_input, signature) = proof.rsplit_once('.').ok_or(ProofDefect::Malformed)?;
    let (header, payload) = signing_input.split_once('.').ok_or(ProofDefect::Malformed)?;
    // `is_base64url` refuses `.`, so a fourth part fails here, in `payload`.
    if [header, payload, signature].iter().any(|part| part.is_empty() || !part.bytes().all(is_base64url)) {
        return Err(ProofDefect::Malformed);
    }
    Ok(RawProof {
        signing_input,
        signature,
        header: decode_object(header)?,
        payload: decode_object(payload)?,
    })
}

fn is_base64url(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'
}

fn decode_object(part: &str) -> Result<Map<String, Value>, ProofDefect> {
    let bytes = URL_SAFE_NO_PAD.decode(part).map_err(|_| ProofDefect::Malformed)?;
    let UniqueObject(members) = serde_json::from_slice(&bytes).map_err(|_| ProofDefect::Malformed)?;
    Ok(members)
}

/// A JSON object that refuses a repeated top-level member name. A plain `serde_json::Map` keeps
/// the last of two equal names with no error (decision P10; the validator's `StrictPayload` has
/// the same rule for the access token).
struct UniqueObject(Map<String, Value>);

impl<'de> Deserialize<'de> for UniqueObject {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct UniqueMembers;

        impl<'de> serde::de::Visitor<'de> for UniqueMembers {
            type Value = Map<String, Value>;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a JSON object with unique member names")
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
                let mut members = Map::new();
                while let Some(name) = access.next_key::<String>()? {
                    // The error text names no member: the proof is client material.
                    if members.contains_key(&name) {
                        return Err(serde::de::Error::custom("a member name occurs twice"));
                    }
                    let value = access.next_value::<Value>()?;
                    members.insert(name, value);
                }
                Ok(members)
            }
        }

        deserializer.deserialize_map(UniqueMembers).map(UniqueObject)
    }
}

/// Check 3 (D16).
fn check_typ(header: &Map<String, Value>) -> Result<(), ProofDefect> {
    match header.get("typ").and_then(Value::as_str) {
        Some(typ) if PROOF_TYPES.iter().any(|allowed| typ.eq_ignore_ascii_case(allowed)) => Ok(()),
        _ => Err(ProofDefect::Typ),
    }
}

/// Check 4 (D8): the exact string `ES256` or `RS256`. This refuses `none` and every HMAC alg.
fn check_alg(header: &Map<String, Value>) -> Result<Algorithm, ProofDefect> {
    match header.get("alg").and_then(Value::as_str) {
        Some("ES256") => Ok(Algorithm::ES256),
        Some("RS256") => Ok(Algorithm::RS256),
        _ => Err(ProofDefect::Alg),
    }
}

/// Check 5: a public JWK of the header alg's family, with valid members (D17: `alg`, `use` and
/// `key_ops` are ignored).
fn check_jwk(header: &Map<String, Value>, alg: Algorithm) -> Result<PublicJwk, ProofDefect> {
    let jwk = header.get("jwk").and_then(Value::as_object).ok_or(ProofDefect::Jwk)?;
    if PRIVATE_JWK_MEMBERS.iter().any(|member| jwk.contains_key(*member)) {
        return Err(ProofDefect::Jwk);
    }
    let member = |name: &str| jwk.get(name).and_then(Value::as_str).ok_or(ProofDefect::Jwk);
    match alg {
        Algorithm::ES256 => {
            // `DecodingKey::from_ec_components` ignores `crv`, so this check is necessary.
            if member("kty")? != "EC" || member("crv")? != "P-256" {
                return Err(ProofDefect::Jwk);
            }
            let (x, y) = (member("x")?, member("y")?);
            if strict_b64(x)?.len() != 32 || strict_b64(y)?.len() != 32 {
                return Err(ProofDefect::Jwk);
            }
            Ok(PublicJwk::Ec { x: x.to_owned(), y: y.to_owned() })
        }
        Algorithm::RS256 => {
            if member("kty")? != "RSA" {
                return Err(ProofDefect::Jwk);
            }
            let (n, e) = (member("n")?, member("e")?);
            let (n_bytes, e_bytes) = (strict_b64(n)?, strict_b64(e)?);
            let first = *n_bytes.first().ok_or(ProofDefect::Jwk)?;
            if first == 0 {
                return Err(ProofDefect::Jwk);
            }
            let bits = n_bytes.len() * 8 - first.leading_zeros() as usize;
            if !RSA_BITS.contains(&bits) {
                return Err(ProofDefect::Jwk);
            }
            let exponent = rsa_exponent(&e_bytes).ok_or(ProofDefect::Jwk)?;
            if exponent < 3 || exponent.is_multiple_of(2) || exponent > MAX_RSA_EXPONENT {
                return Err(ProofDefect::Jwk);
            }
            Ok(PublicJwk::Rsa {
                n: n.to_owned(),
                e: e.to_owned(),
                n_bytes,
                e_bytes,
            })
        }
        _ => Err(ProofDefect::Alg),
    }
}

/// Strict base64url: no padding, no other alphabet, canonical trailing bits (`URL_SAFE_NO_PAD`
/// refuses all three).
fn strict_b64(value: &str) -> Result<Vec<u8>, ProofDefect> {
    URL_SAFE_NO_PAD.decode(value).map_err(|_| ProofDefect::Jwk)
}

/// The RSA public exponent as an integer: minimal octets (no leading zero, decision P12), at most
/// five bytes. `None` when the bytes break either rule.
fn rsa_exponent(bytes: &[u8]) -> Option<u64> {
    if bytes.is_empty() || bytes.len() > 5 || bytes[0] == 0 {
        return None;
    }
    Some(bytes.iter().fold(0u64, |acc, byte| (acc << 8) | u64::from(*byte)))
}

/// Check 6: the signature over `header.payload`, with the key from the validated members.
fn verify_signature(raw: &RawProof<'_>, key: &PublicJwk, alg: Algorithm) -> Result<(), ProofDefect> {
    let decoding_key = match key {
        PublicJwk::Ec { x, y } => DecodingKey::from_ec_components(x, y).map_err(|_| ProofDefect::Signature)?,
        PublicJwk::Rsa { n_bytes, e_bytes, .. } => DecodingKey::from_rsa_raw_components(n_bytes, e_bytes),
    };
    match jsonwebtoken::crypto::verify(raw.signature, raw.signing_input.as_bytes(), &decoding_key, alg) {
        Ok(true) => Ok(()),
        _ => Err(ProofDefect::Signature),
    }
}

/// Check 7. A missing or wrong-typed claim, and a fractional or out-of-range `iat`, are `Malformed`.
fn read_claims(payload: &Map<String, Value>) -> Result<WireProofClaims, ProofDefect> {
    Ok(WireProofClaims {
        jti: read_jti(payload)?,
        iat: payload.get("iat").and_then(Value::as_i64).ok_or(ProofDefect::Malformed)?,
        htm: read_string(payload, "htm")?,
        htu: read_string(payload, "htu")?,
        ath: read_string(payload, "ath")?,
    })
}

fn read_jti(payload: &Map<String, Value>) -> Result<String, ProofDefect> {
    let jti = read_string(payload, "jti")?;
    if jti.is_empty() || jti.len() > MAX_JTI_BYTES {
        return Err(ProofDefect::Malformed);
    }
    Ok(jti)
}

fn read_string(payload: &Map<String, Value>, name: &str) -> Result<String, ProofDefect> {
    payload.get(name).and_then(Value::as_str).map(str::to_owned).ok_or(ProofDefect::Malformed)
}

/// Check 8: base64url, with no padding, of SHA-256 over the token's ASCII bytes.
fn ath_of(token: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(token.as_bytes()))
}

/// Check 9: the RFC 7638 thumbprint, built by hand from the validated members, in the required
/// member order, with no whitespace. SHA-256, then base64url with no padding.
fn thumbprint(key: &PublicJwk) -> String {
    let canonical = match key {
        PublicJwk::Ec { x, y } => format!(r#"{{"crv":"P-256","kty":"EC","x":"{x}","y":"{y}"}}"#),
        PublicJwk::Rsa { n, e, .. } => format!(r#"{{"e":"{e}","kty":"RSA","n":"{n}"}}"#),
    };
    URL_SAFE_NO_PAD.encode(Sha256::digest(canonical.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::EncodingKey;
    use p256::elliptic_curve::Generate;
    use p256::elliptic_curve::sec1::ToSec1Point;
    use p256::pkcs8::{EncodePrivateKey, LineEnding};
    use serde_json::json;

    const TOKEN: &str = "eyJhbGciOiJFUzI1NiJ9.eyJzdWIiOiJhbGljZSJ9.bound-token-signature";
    const HTU: &str = "https://gw.example.test/v1/chat/completions";

    struct EsKey {
        sign: EncodingKey,
        x: String,
        y: String,
    }

    fn es_key() -> EsKey {
        let secret = p256::SecretKey::generate();
        let pem = secret.to_pkcs8_pem(LineEnding::LF).expect("pkcs8 pem");
        let point = secret.public_key().to_sec1_point(false);
        EsKey {
            sign: EncodingKey::from_ec_pem(pem.as_bytes()).expect("ec pem"),
            x: URL_SAFE_NO_PAD.encode(point.x().expect("x")),
            y: URL_SAFE_NO_PAD.encode(point.y().expect("y")),
        }
    }

    fn es_jwk(key: &EsKey) -> Value {
        json!({ "kty": "EC", "crv": "P-256", "x": key.x, "y": key.y })
    }

    fn es_jkt(key: &EsKey) -> Jkt {
        Jkt::new(thumbprint(&PublicJwk::Ec { x: key.x.clone(), y: key.y.clone() }))
    }

    /// A throwaway RSA-2048 key (PKCS#1) that signs only these unit tests (decision P1). It guards nothing.
    const RS256_TEST_KEY_PEM: &str = "\
-----BEGIN RSA PRIVATE KEY-----
MIIEpQIBAAKCAQEA0lAH+dtkLqjM8E3ndcuPr8V1HyFk35q12YRLSt0rjmF3H01k
LLNgFTlYlobtoABHiE01Xy+WBvyRPdQZXGtdArf95HGm43H9E/zydVRjlnwRbCMq
2y4vtUxV151TM8bnygSNEZeN6tOe+e2ptwslXssaJB1UR7jlpMOX8rhn+xeKSrFz
R4tT4L8VVSPSbo0U6XGqe1bGrJ7vSboNX1X5oSq8n9w6t/rIyW3miAeu6z2+9TQq
/nJdKNFK85k7QW9ZyFA4WVjYLCcCHqhaNFiJ6Det0Ps7RiD6lZEw5arLzCM+zacx
tutI5tjos10aLW0e8f1ahyuM6CB4yOlRcOXwIQIDAQABAoIBAE8o32+so88qKwUy
FXJRfdJHjLd8bsb5KQRn1p0llTzgs6EdFJz8oSgr7wutWqiUIliL0xByTVJw40w1
1pviL16UGWTQYGQQphTNawc9jcR5C2e77ugTwNJJGrBu33/IGLuBrgVWmYyvPZHN
4kjq0ZSV9s9sGKpsTkePdVRfE4g+27jVnvmK9kIguAgWKHQu2R+dcEVz4slmrT9X
5y3MibD5DFTJnMVyx2nUGIe9wMMCQgdT/0QZUWkogSgBhRtxUGJkJDGbcEP//ppr
BfPwche9Srt8sI9SKthEJVW46/pNiGcF9zVONkxrAQAdi13AME3KayLCC/+JOffA
QPN00ycCgYEA8TnFaC95r5/kJbr8DVyWoSGZHYvU5ZfQ2t8h6M+m16u5zaSAgnc9
xm+R1q6oh/+9xutzcYakhqWQQfoF1d6konWAD4fWttBq8SI1WKvLbUdbpwYOEkjj
MhGQmpYntaFX1JPgYrREQljU2W8sNyV/zbH0R12MxpTenBD/Lt2wMwMCgYEA3zGR
WLb7rbeYfS0R5yM/1ln+a89ewesTSLoECv4ukdrDDpBchKsThZh7BLhpz2uIjRd0
7XX+AeifT5zjRPR4E+ILkqFTs0jQEvl0ML+mM0wXyzxH/knpbyK0BFr6JCjb5fev
fJ8fb4L3HnwI3+Ij2nVQXhMVGjHD2iUYJi3hlQsCgYEAtN9QeZI/d8Q32WKe45Xt
C9yZZmIWvCBdZf+w+VPaEhSdOOiDw0+NbmDxxTso+vBzZ9fbs9/1NVCnHhFOltJe
N8JKx1pfUKxtw2iW/2mxGrtKqa4MlwE3+a7Z8k9sbvAPX0GSGfB4zha1YmPNj3v6
nE02kNxAVhYB5JuJ+6YWG+kCgYEAlwIdee2MCPQAGylERKNnzKpk5iKO1Rssl/cr
RxjE/3AIqzDnN+fbtHb/PKldBbaW1Ac72HINotL1/tKCPiQ9ng5BkDrQu6uXBE98
2oLAe1KPgrVNbHIrm0Lak1vOhGqUpVpYhDPQ/AybECgRhRCm+2aGMrAsheWHwm94
kFRYnRMCgYEAyUInCDFGpYHc88Iy0iyiNAxuabKwM2IsKWB2AITTHWST7xIEgrVC
X5WTLrgJdJ2zPpUqHrqzLd6W5pPQQRAmagh5zH8/Xs0Ar72gkuGOfzldsbeohFST
VJ6/mtjJ4EykrVcTEdQoCQC7J3NFUpOXZ2aaYOHgLSddm2Med29SXc8=
-----END RSA PRIVATE KEY-----
";

    /// The modulus of that key, base64url. The exponent is 65537 (`AQAB`).
    const RS256_TEST_KEY_N: &str = "0lAH-dtkLqjM8E3ndcuPr8V1HyFk35q12YRLSt0rjmF3H01kLLNgFTlYlobtoABHiE01Xy-WBvyRPdQZXGtdArf95HGm43H9E_zydVRjlnwRbCMq2y4vtUxV151TM8bnygSNEZeN6tOe-e2ptwslXssaJB1UR7jlpMOX8rhn-xeKSrFzR4tT4L8VVSPSbo0U6XGqe1bGrJ7vSboNX1X5oSq8n9w6t_rIyW3miAeu6z2-9TQq_nJdKNFK85k7QW9ZyFA4WVjYLCcCHqhaNFiJ6Det0Ps7RiD6lZEw5arLzCM-zacxtutI5tjos10aLW0e8f1ahyuM6CB4yOlRcOXwIQ";

    /// The inline RS256 test key (decision P1) and its public JWK.
    fn rs_key() -> (EncodingKey, Value) {
        let sign = EncodingKey::from_rsa_pem(RS256_TEST_KEY_PEM.as_bytes()).expect("rsa pem fixture");
        let jwk: Value = json!({ "kty": "RSA", "n": RS256_TEST_KEY_N, "e": "AQAB" });
        (sign, jwk)
    }

    fn rs_jkt(jwk: &Value) -> Jkt {
        let (n, e) = (jwk["n"].as_str().unwrap().to_owned(), jwk["e"].as_str().unwrap().to_owned());
        Jkt::new(thumbprint(&PublicJwk::Rsa {
            n,
            e,
            n_bytes: Vec::new(),
            e_bytes: Vec::new(),
        }))
    }

    fn header(alg: &str, jwk: Value) -> Value {
        json!({ "typ": "dpop+jwt", "alg": alg, "jwk": jwk })
    }

    fn payload() -> Value {
        json!({ "jti": "jti-1", "htm": "POST", "htu": HTU, "iat": 1_700_000_000i64, "ath": ath_of(TOKEN) })
    }

    fn b64_json(value: &str) -> String {
        URL_SAFE_NO_PAD.encode(value.as_bytes())
    }

    /// Signs the raw header and payload JSON text, so a test can write any member, also a repeated one.
    fn sign_raw(header_json: &str, payload_json: &str, key: &EncodingKey, alg: Algorithm) -> String {
        let message = format!("{}.{}", b64_json(header_json), b64_json(payload_json));
        let signature = jsonwebtoken::crypto::sign(message.as_bytes(), key, alg).expect("sign a test proof");
        format!("{message}.{signature}")
    }

    fn sign(header: &Value, payload: &Value, key: &EncodingKey, alg: Algorithm) -> String {
        sign_raw(&header.to_string(), &payload.to_string(), key, alg)
    }

    /// An unsigned proof with a fixed signature part, for the checks that run before check 6.
    fn unsigned(header: &Value, payload: &Value) -> String {
        format!("{}.{}.c2lnbmF0dXJl", b64_json(&header.to_string()), b64_json(&payload.to_string()))
    }

    fn es_proof(key: &EsKey, payload: &Value) -> String {
        sign(&header("ES256", es_jwk(key)), payload, &key.sign, Algorithm::ES256)
    }

    fn check(proof: &str, jkt: &Jkt) -> Result<ProofClaims, ProofDefect> {
        JoseDpopProofChecker.check(proof, TOKEN, jkt)
    }

    #[test]
    fn a_valid_es256_proof_passes_and_returns_its_claims() {
        let key = es_key();
        let claims = check(&es_proof(&key, &payload()), &es_jkt(&key)).expect("a valid ES256 proof");
        assert_eq!(
            claims,
            ProofClaims {
                jti: "jti-1".into(),
                iat: 1_700_000_000,
                htm: "POST".into(),
                htu: HTU.into()
            }
        );
    }

    #[test]
    fn a_valid_rs256_proof_passes() {
        let (sign_key, jwk) = rs_key();
        let proof = sign(&header("RS256", jwk.clone()), &payload(), &sign_key, Algorithm::RS256);
        check(&proof, &rs_jkt(&jwk)).expect("a valid RS256 proof");
    }

    #[test]
    fn check_1_an_oversized_proof_is_malformed() {
        let key = es_key();
        let big = json!({ "jti": "jti-1", "htm": "POST", "htu": HTU, "iat": 1_700_000_000i64, "ath": ath_of(TOKEN), "pad": "x".repeat(MAX_PROOF_BYTES) });
        let proof = es_proof(&key, &big);
        assert!(proof.len() > MAX_PROOF_BYTES);
        assert_eq!(check(&proof, &es_jkt(&key)), Err(ProofDefect::Malformed));
    }

    #[test]
    fn check_1_boundary_exactly_max_proof_bytes_passes_the_size_check() {
        let key = es_key();
        let head = format!("{}.{}.", b64_json(&header("ES256", es_jwk(&key)).to_string()), b64_json(&payload().to_string()));
        let at_limit = format!("{head}{}", "A".repeat(MAX_PROOF_BYTES - head.len()));
        assert_eq!(at_limit.len(), MAX_PROOF_BYTES);
        // Not refused for size: it reaches check 6 and fails there.
        assert_eq!(check(&at_limit, &es_jkt(&key)), Err(ProofDefect::Signature));
        let over = format!("{at_limit}A");
        assert_eq!(check(&over, &es_jkt(&key)), Err(ProofDefect::Malformed));
        assert_eq!(JoseDpopProofChecker.follow_up_claims(&at_limit).map(|c| c.jti), Ok("jti-1".to_owned()));
        assert_eq!(JoseDpopProofChecker.follow_up_claims(&over), Err(ProofDefect::Malformed));
    }

    #[test]
    fn check_2_a_proof_that_is_not_three_base64url_json_objects_is_malformed() {
        let key = es_key();
        let good = es_proof(&key, &payload());
        let (head, rest) = good.split_once('.').unwrap();
        let cases = [
            ("two parts", format!("{head}.{}", rest.split_once('.').unwrap().0)),
            ("four parts", format!("{good}.AAAA")),
            ("padding", format!("{head}=.{rest}")),
            ("a character outside base64url", format!("{head}+.{rest}")),
            ("an empty part", format!(".{rest}")),
            // An empty signature must be Malformed here, not a Signature defect from check 6.
            ("an empty signature part", format!("{}.", good.rsplit_once('.').unwrap().0)),
            ("a header that is not JSON", format!("{}.{rest}", b64_json("not json"))),
            ("a header that is a JSON array", format!("{}.{rest}", b64_json("[1,2]"))),
        ];
        for (name, proof) in cases {
            assert_eq!(check(&proof, &es_jkt(&key)), Err(ProofDefect::Malformed), "{name}");
        }
    }

    #[test]
    fn a_repeated_member_name_is_malformed() {
        // Review Focus 1, decision P10: the last value must not silently win.
        let key = es_key();
        let jwk = es_jwk(&key).to_string();
        let payload_json = payload().to_string();
        let twice_typ = format!(r#"{{"typ":"dpop+jwt","typ":"JWT","alg":"ES256","jwk":{jwk}}}"#);
        let twice_jti = format!(r#"{{"jti":"a","jti":"b","htm":"POST","htu":"{HTU}","iat":1700000000,"ath":"{}"}}"#, ath_of(TOKEN));
        let header_json = header("ES256", es_jwk(&key)).to_string();
        for (name, proof) in [
            ("typ twice in the header", sign_raw(&twice_typ, &payload_json, &key.sign, Algorithm::ES256)),
            ("jti twice in the payload", sign_raw(&header_json, &twice_jti, &key.sign, Algorithm::ES256)),
        ] {
            assert_eq!(check(&proof, &es_jkt(&key)), Err(ProofDefect::Malformed), "{name}");
        }
    }

    #[test]
    fn check_3_the_typ_must_be_dpop_jwt_in_either_form_and_any_case() {
        let key = es_key();
        for typ in ["dpop+jwt", "application/dpop+jwt", "application/DPoP+JWT", "DPOP+JWT"] {
            let proof = sign(&json!({ "typ": typ, "alg": "ES256", "jwk": es_jwk(&key) }), &payload(), &key.sign, Algorithm::ES256);
            check(&proof, &es_jkt(&key)).unwrap_or_else(|d| panic!("typ {typ:?} must pass, got {d:?}"));
        }
        for (name, hdr) in [
            ("no typ", json!({ "alg": "ES256", "jwk": es_jwk(&key) })),
            ("typ JWT", json!({ "typ": "JWT", "alg": "ES256", "jwk": es_jwk(&key) })),
            ("typ a number", json!({ "typ": 1, "alg": "ES256", "jwk": es_jwk(&key) })),
        ] {
            assert_eq!(check(&sign(&hdr, &payload(), &key.sign, Algorithm::ES256), &es_jkt(&key)), Err(ProofDefect::Typ), "{name}");
        }
    }

    #[test]
    fn check_4_only_es256_and_rs256_are_allowed() {
        let key = es_key();
        for alg in ["none", "HS256", "ES384", "PS256", "EdDSA", ""] {
            assert_eq!(check(&unsigned(&header(alg, es_jwk(&key)), &payload()), &es_jkt(&key)), Err(ProofDefect::Alg), "alg {alg:?}");
        }
        let no_alg = json!({ "typ": "dpop+jwt", "jwk": es_jwk(&key) });
        assert_eq!(check(&unsigned(&no_alg, &payload()), &es_jkt(&key)), Err(ProofDefect::Alg));
    }

    #[test]
    fn check_5_the_jwk_must_be_a_public_key_of_the_header_alg() {
        let key = es_key();
        let jkt = es_jkt(&key);
        let mut with_d = es_jwk(&key);
        with_d["d"] = json!("AAAA");
        let p384 = json!({ "kty": "EC", "crv": "P-384", "x": key.x, "y": key.y });
        let short_x = json!({ "kty": "EC", "crv": "P-256", "x": URL_SAFE_NO_PAD.encode([7u8; 31]), "y": key.y });
        let padded_x = json!({ "kty": "EC", "crv": "P-256", "x": format!("{}=", key.x), "y": key.y });
        let cases = [
            ("no jwk", json!({ "typ": "dpop+jwt", "alg": "ES256" })),
            ("jwk a string", header("ES256", json!("key"))),
            ("a private member d", header("ES256", with_d)),
            ("a symmetric member k", header("ES256", json!({ "kty": "oct", "k": "AAAA" }))),
            ("P-384 under ES256", header("ES256", p384)),
            ("x of 31 bytes", header("ES256", short_x)),
            ("x with padding", header("ES256", padded_x)),
            ("an EC key under RS256", header("RS256", es_jwk(&key))),
        ];
        for (name, hdr) in cases {
            assert_eq!(check(&unsigned(&hdr, &payload()), &jkt), Err(ProofDefect::Jwk), "{name}");
        }
    }

    #[test]
    fn check_5_rsa_bounds() {
        // Synthetic values: check 5 refuses them before any signature, so no key generation is needed.
        let n_bits = |bytes: usize, first: u8| {
            let mut n = vec![0xffu8; bytes];
            n[0] = first;
            URL_SAFE_NO_PAD.encode(n)
        };
        let jwk = |n: String, e: &[u8]| json!({ "kty": "RSA", "n": n, "e": URL_SAFE_NO_PAD.encode(e) });
        let cases = [
            ("n of 2047 bits", jwk(n_bits(256, 0x7f), &[1, 0, 1])),
            ("n of 4097 bits", jwk(n_bits(513, 0x01), &[1, 0, 1])),
            ("n with a leading zero byte", jwk(n_bits(257, 0x00), &[1, 0, 1])),
            ("e even (65536)", jwk(n_bits(256, 0xc0), &[1, 0, 0])),
            ("e of 1", jwk(n_bits(256, 0xc0), &[1])),
            ("e of 2^33 + 1", jwk(n_bits(256, 0xc0), &[2, 0, 0, 0, 1])),
            ("e with a leading zero byte (P12)", jwk(n_bits(256, 0xc0), &[0, 1, 0, 1])),
            ("kty EC under RS256", json!({ "kty": "EC", "n": n_bits(256, 0xc0), "e": "AQAB" })),
        ];
        for (name, key) in cases {
            assert_eq!(check(&unsigned(&header("RS256", key), &payload()), &Jkt::new("x")), Err(ProofDefect::Jwk), "{name}");
        }
        // The bounds themselves pass check 5 (then fail the signature, which is not under test here).
        for (name, key) in [("n of 2048 bits", jwk(n_bits(256, 0x80), &[1, 0, 1])), ("n of 4096 bits, e 3", jwk(n_bits(512, 0x80), &[3]))] {
            assert_eq!(check(&unsigned(&header("RS256", key), &payload()), &Jkt::new("x")), Err(ProofDefect::Signature), "{name}");
        }
    }

    #[test]
    fn check_6_the_signature_must_verify_with_the_header_jwk() {
        let key = es_key();
        let other = es_key();
        // Signed by another key, but the header names `key`.
        let forged = sign(&header("ES256", es_jwk(&key)), &payload(), &other.sign, Algorithm::ES256);
        assert_eq!(check(&forged, &es_jkt(&key)), Err(ProofDefect::Signature));
        // A changed payload under the old signature.
        let good = es_proof(&key, &payload());
        let mut parts: Vec<&str> = good.split('.').collect();
        let changed = b64_json(&json!({ "jti": "jti-2", "htm": "POST", "htu": HTU, "iat": 1_700_000_000i64, "ath": ath_of(TOKEN) }).to_string());
        parts[1] = &changed;
        assert_eq!(check(&parts.join("."), &es_jkt(&key)), Err(ProofDefect::Signature));
    }

    #[test]
    fn check_7_each_claim_must_be_present_and_of_its_type() {
        let key = es_key();
        let base = payload();
        let with = |name: &str, value: Value| {
            let mut p = base.clone();
            p[name] = value;
            p
        };
        let without = |name: &str| {
            let mut p = base.clone();
            p.as_object_mut().unwrap().remove(name);
            p
        };
        let cases = [
            ("no jti", without("jti")),
            ("jti empty", with("jti", json!(""))),
            ("jti of 257 bytes", with("jti", json!("j".repeat(257)))),
            ("jti a number", with("jti", json!(1))),
            ("no htm", without("htm")),
            ("htu a number", with("htu", json!(1))),
            ("no iat", without("iat")),
            ("iat fractional", with("iat", json!(1_700_000_000.5))),
            ("iat a string", with("iat", json!("1700000000"))),
            ("iat above i64", with("iat", json!(u64::MAX))),
            ("no ath", without("ath")),
        ];
        for (name, p) in cases {
            assert_eq!(check(&es_proof(&key, &p), &es_jkt(&key)), Err(ProofDefect::Malformed), "{name}");
        }
        // The edge: a 256-byte jti passes.
        check(&es_proof(&key, &with("jti", json!("j".repeat(256)))), &es_jkt(&key)).expect("a 256-byte jti passes");
    }

    #[test]
    fn check_8_ath_must_hash_this_token() {
        let key = es_key();
        let mut p = payload();
        p["ath"] = json!(ath_of("another.bound.token"));
        assert_eq!(check(&es_proof(&key, &p), &es_jkt(&key)), Err(ProofDefect::Ath));
    }

    #[test]
    fn check_9_the_thumbprint_must_equal_the_token_jkt() {
        let key = es_key();
        let other = es_key();
        assert_eq!(check(&es_proof(&key, &payload()), &es_jkt(&other)), Err(ProofDefect::Thumbprint));
    }

    #[test]
    fn a_nonce_claim_is_accepted_and_ignored() {
        let key = es_key();
        let mut p = payload();
        p["nonce"] = json!("server-nonce");
        check(&es_proof(&key, &p), &es_jkt(&key)).expect("a nonce claim is ignored");
    }

    #[test]
    fn the_rfc_7638_example_key_gives_the_published_thumbprint() {
        // RFC 7638 § 3.1.
        let n = "0vx7agoebGcQSuuPiLJXZptN9nndrQmbXEps2aiAFbWhM78LhWx4cbbfAAtVT86zwu1RK7aPFFxuhDR1L6tSoc_BJECPebWKRXjBZCiFV4n3oknjhMstn64tZ_2W-5JsGY4Hc5n9yBXArwl93lqt7_RN5w6Cf0h4QyQ5v-65YGjQR0_FDW2QvzqY368QQMicAtaSqzs8KJZgnYb9c7d0zgdAZHzu6qMQvRL5hajrn1n91CbOpbISD08qNLyrdkt-bFTWhAI4vMQFh6WeZu0fM4lFd2NcRwr3XPksINHaQ-G_xBniIqbw0Ls1jF44-csFCur-kEgU8awapJzKnqDKgw";
        let key = PublicJwk::Rsa {
            n: n.to_owned(),
            e: "AQAB".to_owned(),
            n_bytes: Vec::new(),
            e_bytes: Vec::new(),
        };
        assert_eq!(thumbprint(&key), "NzbLsXh8uDCcd-6MNwXF4W_7noWXFZAfHkxZsRGC9Xs");
    }

    #[test]
    fn follow_up_claims_reads_jti_and_ath_with_no_signature_check() {
        let key = es_key();
        let good = es_proof(&key, &payload());
        let (signed, _) = good.rsplit_once('.').unwrap();
        let resigned = format!("{signed}.Z2FyYmFnZQ");
        assert_eq!(
            JoseDpopProofChecker.follow_up_claims(&resigned),
            Ok(FollowUpClaims {
                jti: "jti-1".into(),
                ath: ath_of(TOKEN)
            })
        );
        assert_eq!(JoseDpopProofChecker.follow_up_claims(&"a".repeat(MAX_PROOF_BYTES + 1)), Err(ProofDefect::Malformed), "check 1");
        assert_eq!(JoseDpopProofChecker.follow_up_claims("only.two"), Err(ProofDefect::Malformed), "check 2");
        let mut no_ath = payload();
        no_ath.as_object_mut().unwrap().remove("ath");
        assert_eq!(JoseDpopProofChecker.follow_up_claims(&es_proof(&key, &no_ath)), Err(ProofDefect::Malformed), "check 7, ath");
    }

    #[test]
    fn ath_matches_compares_with_the_token_hash() {
        assert!(JoseDpopProofChecker.ath_matches(&ath_of(TOKEN), TOKEN));
        assert!(!JoseDpopProofChecker.ath_matches(&ath_of(TOKEN), "other.token.x"));
    }
}
