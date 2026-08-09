// SPDX-License-Identifier: Apache-2.0

//! NATS credential loading (SMA-493 D8) — the piece that makes a rotated credential take effect
//! without a process restart.
//!
//! **Why this exists at all.** `ConnectOptions::with_credentials_file` reads the file exactly
//! once (`options.rs:429`), caches the JWT string and the parsed `KeyPair`, and every reconnect
//! rebuilds its `CONNECT` from that cache (`connector.rs:666`). A NATS user JWT can carry an
//! expiry; when the cached one lapses, every reconnect fails `AuthorizationViolation` and the
//! process cannot recover. `ConnectOptions::with_auth_callback` is invoked on EVERY connection
//! attempt (`connector.rs:681`), so a callback that re-reads the file closes that gap.
//!
//! **Two file shapes, one code path.** A `.creds` (JWT + seed) authenticates by JWT — the
//! production shape. A bare seed file authenticates by nkey, which is what lets an integration
//! test run the real adapter against a static-account broker whose users are declared as
//! `{ nkey: "U…", permissions: {…} }` (SMA-493 D2). The shape is read from the file, not
//! configured, because it is a property of the credential rather than of the deployment.
//!
//! **Stricter than upstream, deliberately.** `async-nats`' own parser takes the first and second
//! `-----`-delimited blocks regardless of their labels (`auth_utils.rs:74-91`). This one keys on
//! `BEGIN NATS USER JWT` and `BEGIN USER NKEY SEED`, so a mislabelled or reordered file is
//! rejected instead of silently misread.

use async_nats::{Auth, AuthError};
use nkeys::KeyPair;

const JWT_LABEL: &str = "NATS USER JWT";
const SEED_LABEL: &str = "USER NKEY SEED";

/// Why a credential file could not be turned into an [`Auth`].
#[derive(Debug, thiserror::Error)]
pub enum CredsError {
    #[error("no `-----BEGIN {SEED_LABEL}-----` block found")]
    MissingSeed,
    #[error("a `-----BEGIN {JWT_LABEL}-----` block was opened but never closed")]
    MissingJwt,
    #[error("the nkey seed could not be parsed: {0}")]
    BadSeed(String),
    #[error("the server nonce could not be signed: {0}")]
    Sign(String),
}

/// A parsed credential: always a key pair, and a JWT when the file carries one.
#[derive(Debug)]
pub struct ParsedCredentials {
    pub jwt: Option<String>,
    pub key_pair: KeyPair,
}

/// Extracts the single line inside the `-----BEGIN {label}-----` / `------END {label}------`
/// block, if present. Hand-rolled rather than regex-backed: two delimited blocks do not need a
/// regex engine, and keying on the label is the strictness this module wants.
fn block(raw: &str, label: &str) -> Option<String> {
    let begin = raw.find(&format!("BEGIN {label}"))?;
    let after_begin = raw[begin..].find('\n')? + begin + 1;
    let end = raw[after_begin..].find("---")? + after_begin;
    let body: String = raw[after_begin..end].split_whitespace().collect();
    if body.is_empty() { None } else { Some(body) }
}

/// Parses a `.creds` (JWT + seed) or a bare seed file.
///
/// # Errors
///
/// [`CredsError::MissingSeed`] when no seed block is present — including an empty file and a
/// file whose blocks carry other labels — and [`CredsError::BadSeed`] when the seed is present
/// but not a valid nkey.
pub fn parse_credentials(raw: &str) -> Result<ParsedCredentials, CredsError> {
    let seed = block(raw, SEED_LABEL).ok_or(CredsError::MissingSeed)?;
    let key_pair = KeyPair::from_seed(&seed).map_err(|e| CredsError::BadSeed(e.to_string()))?;
    // A JWT block that is opened but unterminated is an error rather than "no JWT": silently
    // downgrading a production `.creds` to nkey auth would fail against an operator-mode broker
    // with a message about the wrong thing.
    let jwt = if raw.contains(&format!("BEGIN {JWT_LABEL}")) {
        Some(block(raw, JWT_LABEL).ok_or(CredsError::MissingJwt)?)
    } else {
        None
    };
    Ok(ParsedCredentials { jwt, key_pair })
}

/// Reads `path`, parses it, and signs `nonce` — the body of the auth callback
/// `NatsEventPublisher::connect` installs, called once per connection attempt.
///
/// Returns raw signature bytes: async-nats base64url-encodes them itself
/// (`connector.rs:694-696`), so encoding here would double-encode.
///
/// Holds nothing across an await beyond the file's contents — `ConnectOptions::with_auth_callback`
/// requires the returned future to be `Send + Sync + 'static` (`options.rs:207`), and `KeyPair`
/// is constructed and used entirely after the read completes.
///
/// # Errors
///
/// An [`AuthError`] naming the path, for a file that cannot be read, cannot be parsed, or whose
/// key cannot sign. async-nats preserves this as the source of its `Authentication`
/// `ConnectError` (`connector.rs:685-688`) and logs it, so the path reaches the operator.
pub async fn auth_from_credentials(path: &str, nonce: &[u8]) -> Result<Auth, AuthError> {
    let raw = tokio::fs::read_to_string(path)
        .await
        .map_err(|e| AuthError::new(format!("nats credentials file {path} could not be read: {e}")))?;
    let parsed = parse_credentials(&raw).map_err(|e| AuthError::new(format!("nats credentials file {path} is malformed: {e}")))?;
    let signature = parsed
        .key_pair
        .sign(nonce)
        .map_err(|e| AuthError::new(format!("nats credentials file {path}: {}", CredsError::Sign(e.to_string()))))?;

    let mut auth = Auth::new();
    auth.signature = Some(signature);
    match parsed.jwt {
        Some(jwt) => auth.jwt = Some(jwt),
        // No JWT: authenticate by nkey. The server matches the public key against its configured
        // `users: [{ nkey: … }]` and verifies the signature over the nonce it sent.
        None => auth.nkey = Some(parsed.key_pair.public_key()),
    }
    Ok(auth)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Renders a `.creds` file body the way `nsc generate creds` does: a decorated JWT block
    /// followed by a decorated seed block.
    fn creds_file(jwt: &str, seed: &str) -> String {
        format!(
            "-----BEGIN NATS USER JWT-----\n{jwt}\n------END NATS USER JWT------\n\n\
             *************************** IMPORTANT ***************************\n\
             NKEY Seed printed below can be used sign and prove identity.\n\n\
             -----BEGIN USER NKEY SEED-----\n{seed}\n------END USER NKEY SEED------\n"
        )
    }

    fn a_seed() -> String {
        nkeys::KeyPair::new_user().seed().expect("a fresh user keypair exposes its seed")
    }

    #[test]
    fn a_two_block_creds_file_yields_jwt_auth() {
        let seed = a_seed();
        let parsed = parse_credentials(&creds_file("header.payload.signature", &seed)).expect("valid creds");
        assert_eq!(parsed.jwt.as_deref(), Some("header.payload.signature"));
        assert_eq!(parsed.key_pair.seed().unwrap(), seed);
    }

    /// A bare seed file (`.nk`) authenticates by nkey instead — the fixture shape SMA-493 D2's
    /// static-account test broker uses, and the reason this loader keys off the file's contents
    /// rather than a config flag.
    #[test]
    fn a_seed_only_file_yields_nkey_auth() {
        let seed = a_seed();
        let parsed = parse_credentials(&format!("-----BEGIN USER NKEY SEED-----\n{seed}\n------END USER NKEY SEED------\n")).expect("valid seed file");
        assert!(parsed.jwt.is_none(), "a seed-only file has no JWT to present");
        assert_eq!(parsed.key_pair.seed().unwrap(), seed);
    }

    /// Deliberately stricter than async-nats' own parser, which takes the first and second
    /// `-----`-delimited blocks REGARDLESS of their labels (`auth_utils.rs:74-91`). Keying on the
    /// labels means a file whose blocks are reordered or mislabelled is rejected rather than
    /// silently misread as a JWT (SMA-493 D8).
    #[test]
    fn a_mislabelled_block_is_not_read_as_a_seed() {
        let err = parse_credentials("-----BEGIN SOMETHING ELSE-----\nSUAAAA\n------END SOMETHING ELSE------\n").expect_err("must not accept an unlabelled block");
        assert!(matches!(err, CredsError::MissingSeed), "got {err:?}");
    }

    #[test]
    fn an_empty_file_is_rejected() {
        assert!(matches!(parse_credentials("").expect_err("empty is not credentials"), CredsError::MissingSeed));
    }

    #[test]
    fn a_corrupt_seed_is_rejected() {
        let err = parse_credentials("-----BEGIN USER NKEY SEED-----\nNOTASEED\n------END USER NKEY SEED------\n").expect_err("a malformed seed must not parse");
        assert!(matches!(err, CredsError::BadSeed(_)), "got {err:?}");
    }

    /// The property D8 exists for: the file is read on EVERY call, so a rotated credential is
    /// picked up by the next connection attempt without a restart.
    #[tokio::test]
    async fn each_call_re_reads_the_file_from_disk() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("iam.creds");
        let path_str = path.to_string_lossy().to_string();

        std::fs::write(&path, creds_file("jwt.one", &a_seed())).unwrap();
        let first = auth_from_credentials(&path_str, b"nonce").await.expect("first load");

        std::fs::write(&path, creds_file("jwt.two", &a_seed())).unwrap();
        let second = auth_from_credentials(&path_str, b"nonce").await.expect("second load");

        assert_eq!(first.jwt.as_deref(), Some("jwt.one"));
        assert_eq!(second.jwt.as_deref(), Some("jwt.two"), "the rotated file must be re-read, not cached");
    }

    /// The signature must verify against the seed's own public key — async-nats base64url-encodes
    /// what we hand back (`connector.rs:694-696`), so we return RAW bytes.
    #[tokio::test]
    async fn the_signature_verifies_against_the_seeds_public_key() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("iam.creds");
        let seed = a_seed();
        std::fs::write(&path, creds_file("jwt.one", &seed)).unwrap();

        let auth = auth_from_credentials(&path.to_string_lossy(), b"the-server-nonce").await.expect("load");
        let kp = nkeys::KeyPair::from_seed(&seed).unwrap();
        kp.verify(b"the-server-nonce", &auth.signature.expect("a signature is always returned"))
            .expect("the signature must verify against the same seed");
    }

    #[tokio::test]
    async fn a_missing_file_names_the_path() {
        // `.expect_err(..)` would require `Result::Ok`'s type (`async_nats::Auth`) to implement
        // `Debug`, which it does not (`auth.rs:3-4` derives only `Clone, Default`). `.err()` +
        // `Option::expect` carries the same "panic with this message unless we got the branch we
        // wanted" semantics without that bound.
        let err = auth_from_credentials("/nonexistent/iam.creds", b"nonce").await.err().expect("missing file");
        assert!(format!("{err}").contains("/nonexistent/iam.creds"), "{err}");
    }
}
