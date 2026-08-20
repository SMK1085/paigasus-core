// SPDX-License-Identifier: Apache-2.0

//! SMA-558 AC3: an OIDC issuer behind a PRIVATE CA validates when — and only when —
//! `authn.extra_ca_bundle_path` names that CA, with certificate verification left ON.
//!
//! **Docker-free by construction.** These bind at the `OidcAuthenticator` seam rather than at
//! `AppState`/router: `AppState::new` needs a `DatabaseConnection` from the Docker-gated
//! `start_migrated_postgres()`, which every other mock-IdP suite calls first. `authenticate()`
//! touches no database, so this suite runs on every machine and in every CI leg — and needs no
//! participation in `tests/support/docker.rs`'s policy.
//!
//! It therefore covers the TLS trust path and NOTHING about identity resolution, which
//! `tests/http_authn.rs` already covers against the same mock IdP.
//!
//! The two tests are a matched pair and the negative control is load-bearing: the positive test
//! alone would pass vacuously if anything else in the trust path happened to accept that
//! certificate. They differ in exactly one field.

use paigasus_iam::adapters::clock::SystemClock;
use paigasus_iam::adapters::oidc::jwks::{HttpJwksFetcher, IdpTls, InMemoryJwksCache, JwksProvider};
use paigasus_iam::adapters::oidc::validator::OidcAuthenticator;
use paigasus_iam::config::IssuerConfig;
use paigasus_iam_core::{Authenticator, AuthnError};
use std::io::Write;
use std::time::Duration;

mod support;

/// Builds the authenticator under test at the DB-free seam. `extra_bundle` is the only
/// difference between the two tests below; verification is ON in both.
fn authenticator_for(issuer: &str, extra_bundle: Option<&str>) -> impl Authenticator {
    let fetcher = HttpJwksFetcher::new(Duration::from_secs(5), IdpTls::Verify { extra_bundle }).expect("fetcher builds");
    let provider = JwksProvider::new(fetcher, InMemoryJwksCache::new(), SystemClock, Duration::from_secs(3600), Duration::from_secs(30));
    OidcAuthenticator::new(
        vec![IssuerConfig {
            issuer: issuer.to_string(),
            audiences: vec!["paigasus".to_string()],
            jit_provisioning: true,
        }],
        provider,
        60,
        16384,
    )
    .expect("authenticator builds")
}

#[tokio::test]
async fn private_ca_issuer_validates_with_extra_ca_bundle() {
    let (idp, ca_pem) = support::start_mock_idp_private_ca().await;

    let mut bundle = tempfile::NamedTempFile::new().expect("temp file");
    bundle.write_all(ca_pem.as_bytes()).expect("write ca pem");
    bundle.flush().expect("flush");

    let authn = authenticator_for(&idp.issuer, Some(bundle.path().to_str().unwrap()));
    let token = idp.bearer("sub-alice", Some("alice@example.com"), "paigasus", 3600);

    let claims = authn.authenticate(&token).await.expect("a private-CA issuer must validate when its CA is trusted");
    assert_eq!(claims.subject, "sub-alice");
}

#[tokio::test]
async fn private_ca_issuer_fails_without_extra_ca_bundle() {
    // The negative control. Identical to the test above except `extra_bundle: None`, so the
    // failure is attributable to the trust anchor and nothing else. Without this, the positive
    // test proves only that SOMETHING accepted the certificate.
    let (idp, _ca_pem) = support::start_mock_idp_private_ca().await;

    let authn = authenticator_for(&idp.issuer, None);
    let token = idp.bearer("sub-alice", Some("alice@example.com"), "paigasus", 3600);

    let err = authn.authenticate(&token).await.expect_err("an untrusted private CA must not validate");
    assert!(
        matches!(err, AuthnError::Unavailable),
        "a TLS trust failure surfaces as Unavailable (the JWKS fetch failed), got {err:?}"
    );
}

#[tokio::test]
async fn private_ca_issuer_fails_with_the_wrong_ca_bundle() {
    // The second negative control, and it catches a regression the other two cannot. Those two
    // distinguish only "a bundle was loaded" from "no bundle at all", so anything that made a
    // non-empty bundle RELAX verification rather than EXTEND it — `danger_accept_invalid_certs`
    // added alongside `add_root_certificate`, or a custom verifier that returns Ok once any extra
    // anchor is present — would leave both of them green. Here the bundle is non-empty and
    // well-formed but carries the WRONG root, so it must still fail. (Observed at the reqwest
    // layer, this handshake dies with `InvalidCertificate(BadSignature)`, where an absent bundle
    // gives `InvalidCertificate(UnknownIssuer)` — both surface here as `Unavailable`.)
    let (idp_a, _ca_a_pem) = support::start_mock_idp_private_ca().await;
    let (_idp_b, ca_b_pem) = support::start_mock_idp_private_ca().await;

    let mut bundle = tempfile::NamedTempFile::new().expect("temp file");
    bundle.write_all(ca_b_pem.as_bytes()).expect("write the OTHER ca's pem");
    bundle.flush().expect("flush");

    let authn = authenticator_for(&idp_a.issuer, Some(bundle.path().to_str().unwrap()));
    let token = idp_a.bearer("sub-alice", Some("alice@example.com"), "paigasus", 3600);

    let err = authn.authenticate(&token).await.expect_err("a bundle holding an unrelated CA must not validate");
    assert!(
        matches!(err, AuthnError::Unavailable),
        "a TLS trust failure surfaces as Unavailable (the JWKS fetch failed), got {err:?}"
    );
}
