# SMA-558 — Private-CA OIDC issuer trust — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let an operator make `paigasus-iam` (and `paigasus-gateway`) trust a private-CA TLS endpoint without disabling certificate verification.

**Architecture:** Enable `reqwest`'s `rustls-tls-native-roots` feature *alongside* the existing `rustls-tls`, so the rustls root store becomes webpki Mozilla roots ∪ the image's `/etc/ssl/certs` ∪ an optional operator-supplied PEM bundle — all additive. Each service gains one `extra_ca_bundle_path` config field that folds its certificates into the client via `add_root_certificate`. IAM's fetcher takes an `IdpTls` enum rather than a `bool` + `Option` pair, so "verification disabled AND a bundle configured" is unrepresentable.

**Tech Stack:** Rust (edition 2024, rust-version 1.95), `reqwest 0.12.28`, `rustls`, `rcgen 0.13` + `axum-server` (test fixtures), `tempfile`, `figment`, Moon.

**Spec:** `docs/superpowers/specs/2026-08-20-sma-558-private-ca-oidc-issuer-design.md`

## Global Constraints

- Every source file opens with `// SPDX-License-Identifier: Apache-2.0`.
- Rust crates are **edition 2024 + rust-version 1.95**.
- `[workspace.lints.rust] warnings = "deny"` — dead code and unused imports are **hard compile errors**, not warnings. Never add an item "to wire up later".
- Conventional commits with a workspace scope: `feat(rs): …`, `docs(repo): …`.
- Commit message bodies must **never** contain a line beginning `word:` (e.g. `error:`, `use:`) — commitlint parses it as a trailer and fails `footer-leading-blank`. Also: subject starts lowercase, header ≤100 chars, and write "owner/repo PR NNN" rather than `#NNN`.
- Prefix every shell command with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` so `moon`/`cargo-nextest` resolve to the repo-pinned versions.
- Work exclusively in `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-558` on branch `feature/sma-558-iam-private-ca-oidc-issuer`. Never `cd` to the main checkout.
- The field is named `extra_ca_bundle_path` in **both** services. The `extra_` prefix is load-bearing (spec D2) — it marks additive semantics against two existing knobs that *replace*. Do not "simplify" it to `ca_bundle_path`.
- **Roots only:** every certificate in a bundle becomes an unconstrained trust anchor (no `cA` check). Never document or suggest adding intermediates.
- Neither `AuthnConfig` nor `OpenAiConfig` derives `Default`, so `..Default::default()` is unavailable — every struct literal must gain the new field explicitly.

---

## File Structure

| File | Responsibility |
| --- | --- |
| `rs/Cargo.toml` | The one-line feature change that makes the system trust store reachable |
| `rs/crates/services/paigasus-iam/src/adapters/oidc/jwks.rs` | `IdpTls` enum + bundle-folding + the three D4 error cases + D8 log |
| `rs/crates/services/paigasus-iam/src/config.rs` | `AuthnConfig` field + `validate()` rules |
| `rs/crates/services/paigasus-iam/tests/support/mod.rs` | `start_mock_idp_private_ca()` — the CA-signed chain fixture |
| `rs/crates/services/paigasus-iam/tests/authn_private_ca.rs` | The AC3 proof: positive + negative control |
| `rs/crates/services/paigasus-gateway/src/adapters/openai/client.rs` | Gateway's mirror of the fold + `CaBundle` variant |
| `rs/crates/services/paigasus-gateway/src/adapters/http/error.rs` | `CaBundle` → `UpstreamUnavailable` mapping |
| `docs/ops/RUNBOOK-containers.md` | AC2: the operator-facing mechanism |

**Task order rationale:** Task 1 (the feature flip) is a prerequisite for everything — without it, `rustls-tls-native-roots` is not compiled in and nothing else can be tested. Tasks 2-4 build IAM bottom-up (adapter → config → integration proof). Task 5 does the gateway. Task 6 is docs. Task 7 is the full gate run.

---

## Task 1: Enable native roots in the workspace reqwest pin

**Files:**
- Modify: `rs/Cargo.toml:74-76`
- Modify: `rs/Cargo.lock` (regenerated)

**Interfaces:**
- Consumes: nothing.
- Produces: the `rustls-tls-native-roots` feature, which every later task's `reqwest::Client` depends on. No Rust API change.

- [ ] **Step 1: Make the feature change**

Replace the comment block and pin at `rs/Cargo.toml:74-76`. The current text is:

```toml
# TLS baseline is rustls + json. If a service needs gzip/brotli/cookies, opt in PER CRATE —
# do NOT set default-features = true here (it re-introduces openssl across the workspace).
reqwest = { version = "0.12", default-features = false, features = ["rustls-tls", "json"] }
```

Replace with:

```toml
# TLS baseline is rustls + json. If a service needs gzip/brotli/cookies, opt in PER CRATE —
# do NOT set default-features = true here (it re-introduces openssl across the workspace).
#
# BOTH root-store features are on deliberately (SMA-558 D1). reqwest builds ONE RootCertStore by
# UNION: `add_root_certificate()` calls, then webpki roots if `rustls-tls-webpki-roots`, then the
# platform store if `rustls-tls-native-roots` (async_impl/client.rs:687-732). So this yields
# webpki ∪ /etc/ssl/certs ∪ any explicitly configured bundle, not a choice between them — which is
# what lets an operator trust a private-CA IdP without `accept_invalid_tls`.
#
# Keeping `rustls-tls` alongside is NOT redundancy. reqwest errors on a native store only when it
# finds certificates that ALL fail to parse (client.rs:715); an ABSENT or EMPTY store falls through
# and yields a client with zero anchors that fails every handshake at request time. The webpki
# roots are the floor that makes that unrepresentable.
#
# Both features alias through to `__rustls-ring` (reqwest/Cargo.toml:148,154-172), so there is no
# second CryptoProvider — cf. the aws-lc-rs note on `async-nats` below.
reqwest = { version = "0.12", default-features = false, features = ["rustls-tls", "rustls-tls-native-roots", "json"] }
```

- [ ] **Step 2: Regenerate the lockfile and verify it adds no packages**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs
cargo metadata --format-version 1 > /dev/null
git diff --stat Cargo.lock
grep -c '^\[\[package\]\]' Cargo.lock
```

Expected: `Cargo.lock | 2 ++`, and the package count is **543** — unchanged. The only two insertions are `rustls-native-certs` edges under `hyper-rustls` and `reqwest`.

If the package count changed, STOP and report — the spec's `:deny`-is-clean argument (§ 2.4) depended on it.

- [ ] **Step 3: Verify the workspace still builds**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo build --workspace --locked 2>&1 | tail -5
```

Expected: success.

- [ ] **Step 4: Commit**

```bash
git add rs/Cargo.toml rs/Cargo.lock
git commit -m "build(rs): union the platform trust store into reqwest's roots (SMA-558)"
```

---

## Task 2: `IdpTls` enum and bundle loading in `HttpJwksFetcher`

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/oidc/jwks.rs:83-104` (and its `#[cfg(test)] mod tests`)
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/mod.rs:668` (the one call site)

**Interfaces:**
- Consumes: Task 1's `rustls-tls-native-roots` feature.
- Produces:
  - `pub enum IdpTls<'a> { AcceptInvalid, Verify { extra_bundle: Option<&'a str> } }` in `adapters::oidc::jwks`
  - `HttpJwksFetcher::new(timeout: Duration, tls: IdpTls<'_>) -> Result<Self, AuthnError>` — replaces the old `new(Duration, bool)`.

Task 3 constructs `IdpTls` from config; Task 4's integration test calls `HttpJwksFetcher::new` directly.

- [ ] **Step 1: Write the three failing tests**

Add to the existing `#[cfg(test)] mod tests` at the bottom of `jwks.rs` (it already has `use super::*;`):

```rust
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
            IdpTls::Verify { extra_bundle: Some("/nonexistent/paigasus-sma558/ca.pem") },
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
            IdpTls::Verify { extra_bundle: Some(f.path().to_str().unwrap()) },
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
            IdpTls::Verify { extra_bundle: Some(f.path().to_str().unwrap()) },
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
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --lib -E 'test(bundle) or test(accept_invalid_both)' --no-tests=pass
```

Expected: FAIL to **compile** — `cannot find type IdpTls in this scope` and `this function takes 2 arguments`. A compile failure is the correct "red" here.

- [ ] **Step 3: Implement `IdpTls` and the new constructor**

In `jwks.rs`, replace the whole `impl HttpJwksFetcher { pub fn new(...) }` block at lines 88-104 (keep `unavailable` below it), and add the enum immediately above `pub struct HttpJwksFetcher`:

```rust
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
                let pem = std::fs::read(path)
                    .map_err(|e| backend(format!("failed to read authn.extra_ca_bundle_path {path:?}: {e}")))?;

                // `from_pem_bundle`, NOT `from_pem`: a bundle may legitimately carry more than one
                // ROOT (a cross-signed CA, or two corporate roots mid-rotation) and `from_pem`
                // reads only the first. This is NOT an invitation to add intermediates — every
                // certificate here becomes an UNCONSTRAINED trust anchor (rustls performs no `cA`
                // basic-constraints check on an anchor), so an intermediate would be promoted to a
                // root for every HTTPS call this process makes.
                let certs = reqwest::Certificate::from_pem_bundle(&pem).map_err(|e| {
                    backend(format!("authn.extra_ca_bundle_path {path:?} is not a valid PEM certificate bundle: {e}"))
                })?;

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
```

Add this free function just above `pub enum IdpTls` (module-private):

```rust
/// A config/wiring fault, carrying its cause. `AuthnError::Backend` takes a boxed error and std
/// supplies `From<String> for Box<dyn Error + Send + Sync>`, which is the idiom
/// `adapters/http/mod.rs:605` already uses.
fn backend(message: String) -> AuthnError {
    AuthnError::Backend(message.into())
}
```

- [ ] **Step 4: Update the one production call site**

In `rs/crates/services/paigasus-iam/src/adapters/http/mod.rs`, line 668 currently reads:

```rust
        let fetcher = HttpJwksFetcher::new(Duration::from_secs(authn_cfg.http_timeout_secs), authn_cfg.accept_invalid_tls)?;
```

Replace with:

```rust
        // `validate()` rejects accept_invalid_tls + a bundle, so this collapse is not lossy: at
        // most one arm's data is ever meaningful. `AcceptInvalid` wins if both somehow arrive
        // (an embedder that skipped validate()), which is the safe direction — it cannot silently
        // pretend a bundle is in force.
        let idp_tls = if authn_cfg.accept_invalid_tls {
            IdpTls::AcceptInvalid
        } else {
            IdpTls::Verify { extra_bundle: authn_cfg.extra_ca_bundle_path.as_deref() }
        };
        let fetcher = HttpJwksFetcher::new(Duration::from_secs(authn_cfg.http_timeout_secs), idp_tls)?;
```

Add `IdpTls` to the existing `adapters::oidc::jwks` import group in that file's `use` block.

**Note:** `authn_cfg.extra_ca_bundle_path` does not exist yet — Task 3 adds it. Do Step 4 and Task 3 Step 3 together if the compiler blocks you; they are one logical change split for review clarity.

- [ ] **Step 5: Add the `tempfile` import guard**

`tempfile` is already an IAM dev-dependency (`Cargo.toml:170`), so no manifest change. The tests reference it as `tempfile::NamedTempFile` with no `use`, which works.

- [ ] **Step 6: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --lib -E 'test(bundle) or test(accept_invalid_both)' --no-tests=pass
```

Expected: 4 PASS.

- [ ] **Step 7: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/adapters/oidc/jwks.rs rs/crates/services/paigasus-iam/src/adapters/http/mod.rs
git commit -m "feat(rs): load extra IdP trust anchors from a PEM bundle (SMA-558)"
```

---

## Task 3: `authn.extra_ca_bundle_path` config field and validation

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/config.rs` — `AuthnConfig` (~line 117), `AuthnDefaults` comment (~line 657), `validate()` (~line 985)
- Modify: `rs/crates/services/paigasus-iam/src/service_info.rs:137-149`
- Modify: `rs/crates/services/paigasus-iam/tests/support/mod.rs:307-326`
- Modify: `rs/crates/services/paigasus-iam/tests/keycloak_e2e.rs:199-215`

**Interfaces:**
- Consumes: Task 2's `IdpTls`.
- Produces: `AuthnConfig.extra_ca_bundle_path: Option<String>`, read by Task 2's call site and Task 4's tests. Env var `IAM_AUTHN__EXTRA_CA_BUNDLE_PATH`.

- [ ] **Step 1: Write the failing validation tests**

Add to `config.rs`'s `#[cfg(test)] mod tests`, following the existing `figment::Jail` style used by its neighbours:

```rust
    #[test]
    fn validate_rejects_accept_invalid_tls_together_with_a_ca_bundle() {
        figment::Jail::expect_with(|jail| {
            jail.create_file(
                "iam.toml",
                r#"
                    database_url = "postgres://u:p@localhost/db"
                    [api_keys]
                    pepper = "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY="
                    [authn]
                    accept_invalid_tls = true
                    extra_ca_bundle_path = "/etc/paigasus/corp-ca.pem"
                    [[authn.issuers]]
                    issuer = "https://idp.example.com"
                    audiences = ["paigasus"]
                "#,
            )?;
            let cfg: IamConfig = IamConfig::figment().extract()?;
            let err = cfg.validate().expect_err("the combination must be rejected");
            assert!(err.contains("extra_ca_bundle_path"), "the message must name the field: {err}");
            assert!(err.contains("accept_invalid_tls"), "the message must name the other field: {err}");
            Ok(())
        });
    }

    #[test]
    fn validate_rejects_an_empty_ca_bundle_path() {
        // `IAM_AUTHN__EXTRA_CA_BUNDLE_PATH=` deserializes to Some(""), not None, which would
        // otherwise reach std::fs::read("") and fail with a confusing empty path.
        figment::Jail::expect_with(|jail| {
            jail.create_file(
                "iam.toml",
                r#"
                    database_url = "postgres://u:p@localhost/db"
                    [api_keys]
                    pepper = "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY="
                    [authn]
                    extra_ca_bundle_path = ""
                    [[authn.issuers]]
                    issuer = "https://idp.example.com"
                    audiences = ["paigasus"]
                "#,
            )?;
            let cfg: IamConfig = IamConfig::figment().extract()?;
            assert!(cfg.validate().is_err(), "an empty bundle path must be rejected");
            Ok(())
        });
    }

    #[test]
    fn extra_ca_bundle_path_defaults_to_none() {
        figment::Jail::expect_with(|jail| {
            jail.create_file(
                "iam.toml",
                r#"
                    database_url = "postgres://u:p@localhost/db"
                    [api_keys]
                    pepper = "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY="
                    [[authn.issuers]]
                    issuer = "https://idp.example.com"
                    audiences = ["paigasus"]
                "#,
            )?;
            let cfg: IamConfig = IamConfig::figment().extract()?;
            assert!(cfg.authn.extra_ca_bundle_path.is_none(), "absent config must yield None");
            Ok(())
        });
    }
```

In that third test, the assertion line is:

```rust
            assert!(cfg.authn.extra_ca_bundle_path.is_none(), "absent config must yield None");
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --lib -E 'test(ca_bundle)' --no-tests=pass
```

Expected: FAIL to compile — `no field extra_ca_bundle_path on type AuthnConfig`.

- [ ] **Step 3: Add the field**

In `config.rs`, immediately after the `accept_invalid_tls` field (which ends at line 124 with `pub accept_invalid_tls: bool,`), add:

```rust
    /// Extra trust anchors for the IdP discovery/JWKS fetches, as a path to a PEM bundle.
    ///
    /// **This ADDS to the trust store, it does not replace it.** The client trusts the
    /// compiled-in Mozilla roots, the image's own store (`/etc/ssl/certs`), AND every
    /// certificate in this bundle. That is the OPPOSITE of the sibling
    /// [`PublisherConfig::root_ca_bundle`] and of the gateway's `iam.tls.ca_cert_path`, both of
    /// which REPLACE — hence the `extra_` prefix (cf. `NODE_EXTRA_CA_CERTS`, which is additive,
    /// against `REQUESTS_CA_BUNDLE`, which is not).
    ///
    /// **ROOTS ONLY.** Every certificate here becomes an UNCONSTRAINED trust anchor for every
    /// HTTPS call this process makes — rustls performs no `cA` basic-constraints check on an
    /// anchor. An intermediate placed here is silently promoted to a root.
    ///
    /// Read ONCE at boot, so a rotated bundle needs a restart (unlike `root_ca_bundle`, which is
    /// re-read per connection attempt). An unreadable, malformed, or certificate-FREE bundle is a
    /// hard boot failure, never a warning. Mutually exclusive with `accept_invalid_tls`, which
    /// would make it dead — `validate()` rejects the pair.
    ///
    /// Does NOT help a self-signed LEAF certificate: webpki has no support for self-signed
    /// certificates, so that case still needs `accept_invalid_tls` (SMA-558 § 9).
    #[serde(default)]
    pub extra_ca_bundle_path: Option<String>,
```

- [ ] **Step 4: Update the `AuthnDefaults` comment**

`AuthnDefaults` (line ~657) is deliberately NOT given the new field — an `Option` with `#[serde(default)]` resolves to `None` without a defaults-layer entry. Update its comment from:

```rust
// Mirrors `AuthnConfig` minus `issuers` — deliberately absent, see `AuthnConfig` doc.
```

to:

```rust
// Mirrors `AuthnConfig` minus TWO fields, for different reasons. `issuers` is absent so a missing
// issuer list is a hard error rather than silently defaulting to empty (see `AuthnConfig`'s doc).
// `extra_ca_bundle_path` is absent because it is an `Option` carrying `#[serde(default)]`, which
// already resolves to `None` without a defaults-layer entry; adding one would serialize a null
// into the layer for no gain.
```

- [ ] **Step 5: Add the two validation rules**

In `validate()`, immediately after the `jwks_ttl_secs == 0` check (~line 998), add:

```rust
        // `accept_invalid_tls` disables verification outright, so a configured bundle could never
        // be consulted. The pair is always an operator mistake; the adapter seam's `IdpTls` enum
        // makes it unrepresentable in code, and this makes it a readable message in config
        // (SMA-558 D5).
        if self.authn.accept_invalid_tls && self.authn.extra_ca_bundle_path.is_some() {
            return Err(
                "authn.accept_invalid_tls = true disables certificate verification entirely, so \
                 authn.extra_ca_bundle_path can never take effect — set one or the other, not both"
                    .to_string(),
            );
        }

        // `IAM_AUTHN__EXTRA_CA_BUNDLE_PATH=` yields Some(""), not None.
        if self.authn.extra_ca_bundle_path.as_deref().is_some_and(str::is_empty) {
            return Err("authn.extra_ca_bundle_path must not be empty (omit the key entirely to use the default trust store)".to_string());
        }
```

Also extend `validate()`'s doc comment — append to its existing prose: `Also (SMA-558): 'authn.accept_invalid_tls' and 'authn.extra_ca_bundle_path' are mutually exclusive, and the latter is non-empty when present.`

- [ ] **Step 6: Update the three `AuthnConfig` struct literals**

`AuthnConfig` does not derive `Default`, so each literal needs the field. Add `extra_ca_bundle_path: None,` immediately after the `accept_invalid_tls: true,` line in each of:

- `src/service_info.rs:143`
- `tests/support/mod.rs:313`
- `tests/keycloak_e2e.rs:205`

- [ ] **Step 7: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --lib --no-tests=pass 2>&1 | tail -20
```

Expected: all lib tests PASS, including the three new ones.

- [ ] **Step 8: Verify the whole IAM crate still compiles, tests included**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo build --tests -p paigasus-iam --locked 2>&1 | tail -10
```

Expected: success. This is what catches a missed struct literal in a Docker-gated suite (`keycloak_e2e.rs`) you would not otherwise build.

- [ ] **Step 9: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/config.rs rs/crates/services/paigasus-iam/src/service_info.rs rs/crates/services/paigasus-iam/tests/support/mod.rs rs/crates/services/paigasus-iam/tests/keycloak_e2e.rs
git commit -m "feat(rs): add authn.extra_ca_bundle_path with mutual-exclusion validation (SMA-558)"
```

---

## Task 4: The private-CA integration proof (AC3)

**Files:**
- Modify: `rs/crates/services/paigasus-iam/tests/support/mod.rs` (add `start_mock_idp_private_ca`)
- Create: `rs/crates/services/paigasus-iam/tests/authn_private_ca.rs`

**Interfaces:**
- Consumes: `HttpJwksFetcher::new`, `IdpTls` (Task 2); `AuthnConfig.extra_ca_bundle_path` (Task 3).
- Produces: `support::start_mock_idp_private_ca() -> (MockIdp, String)` — the `MockIdp` plus the CA certificate as a PEM string.

- [ ] **Step 1: Add the CA-signed fixture to `tests/support/mod.rs`**

Add immediately after the existing `start_mock_idp` (which ends at line 285). It duplicates that function's server-wiring rather than refactoring it, because the two differ only in TLS material and `start_mock_idp` is used by ~50 existing tests that must not change behaviour.

```rust
/// Like [`start_mock_idp`], but served with a leaf certificate signed by a freshly minted
/// PRIVATE CA, and returns that CA's certificate PEM alongside the IdP (SMA-558 AC3).
///
/// The server is configured with the **leaf alone**, not the chain. That is correct TLS practice
/// for a root the client is expected to hold, and it is what makes the test strict: the client
/// cannot learn the CA from the handshake, so it can only succeed if `extra_ca_bundle_path`
/// genuinely loaded.
///
/// Three details are load-bearing, because `CertificateParams::default()` leaves the
/// distinguished name EMPTY and the pre-existing `start_mock_idp` fixture has never been
/// exercised against real verification (every one of its call sites runs with
/// `accept_invalid_tls: true`):
///   - the CA and the leaf get DISTINCT, non-empty CNs — otherwise both carry an empty subject
///     DN and path building has nothing to match on;
///   - the leaf carries both `localhost` and `127.0.0.1` SANs, since the server binds an
///     ephemeral `127.0.0.1` port and the issuer URL is `https://127.0.0.1:<port>`;
///   - `CertificateParams::signed_by` CONSUMES `self`, so the params must be built and passed by
///     value.
#[allow(dead_code)]
pub async fn start_mock_idp_private_ca() -> (MockIdp, String) {
    use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, KeyPair};

    // --- the private CA ---
    // `DistinguishedName::push` takes `impl Into<DnValue>` and rcgen has a blanket
    // `impl<T: Into<String>> From<T> for DnValue`, so a bare &str is enough (it becomes a
    // Utf8String). No PrintableString conversion needed.
    let mut ca_params = CertificateParams::new(Vec::new()).expect("ca params");
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params.distinguished_name.push(DnType::CommonName, "paigasus-test-private-ca");
    let ca_key = KeyPair::generate().expect("ca keypair");
    let ca_cert = ca_params.self_signed(&ca_key).expect("self-signed ca");
    let ca_pem = ca_cert.pem();

    // --- the leaf, signed BY the CA ---
    let mut leaf_params = CertificateParams::new(vec!["localhost".to_string(), "127.0.0.1".to_string()]).expect("leaf params");
    leaf_params.distinguished_name.push(DnType::CommonName, "paigasus-mock-idp");
    let leaf_key = KeyPair::generate().expect("leaf keypair");
    let leaf_cert = leaf_params.signed_by(&leaf_key, &ca_cert, &ca_key).expect("ca-signed leaf");

    let kid = "mock-idp-es256-initial".to_string();
    let (sign, jwk) = es256_keypair(&kid);

    let tls = axum_server::tls_rustls::RustlsConfig::from_pem(leaf_cert.pem().into_bytes(), leaf_key.serialize_pem().into_bytes())
        .await
        .expect("rustls config from generated pem");

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("ephemeral port");
    listener.set_nonblocking(true).expect("nonblocking listener");
    let addr = listener.local_addr().unwrap();
    let issuer = format!("https://{addr}");

    let discovery_body = serde_json::json!({ "issuer": issuer, "jwks_uri": format!("{issuer}/jwks") }).to_string();
    let jwks_body = Arc::new(RwLock::new(serde_json::to_string(&JwkSet { keys: vec![jwk] }).expect("jwks serializes")));

    let jwks_for_route = jwks_body.clone();
    let idp_routes = Router::new()
        .route(
            "/.well-known/openid-configuration",
            get(move || {
                let body = discovery_body.clone();
                async move { ([("content-type", "application/json")], body) }
            }),
        )
        .route(
            "/jwks",
            get(move || {
                let shared = jwks_for_route.clone();
                let body = shared.read().expect("jwks lock not poisoned").clone();
                async move { ([("content-type", "application/json")], body) }
            }),
        );

    let handle = tokio::spawn(async move {
        axum_server::from_tcp_rustls(listener, tls).serve(idp_routes.into_make_service()).await.expect("mock idp server");
    });

    (MockIdp { issuer, sign, kid, jwks_body, handle }, ca_pem)
}
```

- [ ] **Step 2: Write the failing integration test**

Create `rs/crates/services/paigasus-iam/tests/authn_private_ca.rs`:

```rust
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
    let provider = JwksProvider::new(
        fetcher,
        InMemoryJwksCache::new(),
        SystemClock,
        Duration::from_secs(3600),
        Duration::from_secs(30),
    );
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
```

- [ ] **Step 3: Run the tests to verify the pair behaves**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --test authn_private_ca --no-tests=pass
```

Expected: 2 PASS.

**If the positive test fails with `Unavailable`**, the chain is not verifying. Diagnose in this order — do NOT reach for `accept_invalid_tls`:
1. Confirm the CN values are distinct and non-empty (empty DNs on both certs is the classic cause).
2. Confirm the leaf carries the `127.0.0.1` **IP** SAN, not just a DNS SAN — the issuer URL is an IP literal.
3. Confirm the server was given the leaf's PEM, not the CA's.

**If the negative test PASSES the token** (i.e. no error), stop — that means something other than the bundle is trusting the certificate, and the positive test is vacuous. Do not proceed.

- [ ] **Step 4: Verify the suite is genuinely Docker-free**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && env -u CI PAIGASUS_SKIP_DOCKER=1 cargo nextest run -p paigasus-iam --test authn_private_ca --no-tests=pass
```

Expected: 2 PASS — the same two, not skipped. Also confirm the file contains no `CI` env read and no `AsyncRunner` import, which `repo:iam-docker-policy-single-site` gates:

```bash
grep -n 'var_os("CI")\|var("CI")\|option_env!("CI")\|AsyncRunner' rs/crates/services/paigasus-iam/tests/authn_private_ca.rs || echo "clean"
```

Expected: `clean`.

- [ ] **Step 5: Commit**

```bash
git add rs/crates/services/paigasus-iam/tests/support/mod.rs rs/crates/services/paigasus-iam/tests/authn_private_ca.rs
git commit -m "test(rs): prove a private-CA issuer validates with verification on (SMA-558)"
```

---

## Task 5: The gateway's `extra_ca_bundle_path`

**Files:**
- Modify: `rs/crates/services/paigasus-gateway/src/config.rs` — `OpenAiConfig` (~line 99), `OpenAiDefaults` comment (~line 159), `validate()` (~line 260)
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/openai/client.rs` — `OpenAiError`, `OpenAiClient::new`, test module
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/http/error.rs:85-92` and its parity test at `:204-213`
- Modify: `rs/crates/services/paigasus-gateway/Cargo.toml` (dev-dependency)
- Modify: 7 further `OpenAiConfig` literal sites (listed in Step 5)

**Interfaces:**
- Consumes: Task 1's feature.
- Produces: `OpenAiConfig.extra_ca_bundle_path: Option<String>`; `OpenAiError::CaBundle { path, source }`. Env var `GATEWAY_UPSTREAM__OPENAI__EXTRA_CA_BUNDLE_PATH`.

- [ ] **Step 1: Add the `tempfile` dev-dependency**

In `rs/crates/services/paigasus-gateway/Cargo.toml`, in the existing `[dev-dependencies]` block:

```toml
# Writes throwaway CA-bundle fixtures for `adapters::openai::client`'s trust-anchor tests
# (SMA-558) — the same dev-dep paigasus-iam already carries for its own fixtures.
tempfile = "3"
# Mints those fixtures' CA certificate at RUNTIME rather than committing a PEM blob, matching
# the convention paigasus-iam records in tests/support/mod.rs ("no committed PEM/JWK fixtures").
# Only the cert minter — deliberately NOT axum-server, since no gateway test starts a TLS
# listener (SMA-558 D6).
rcgen = { version = "0.13", default-features = false, features = ["crypto", "pem", "ring"] }
```

- [ ] **Step 2: Write the failing tests**

Add to `client.rs`'s `#[cfg(test)] mod tests`, and extend its existing `test_client` helper. First replace the helper (currently at `:202-208`) with:

```rust
    fn test_client(api_key: &str) -> OpenAiClient {
        client_with_bundle(api_key, None).expect("client builds")
    }

    fn client_with_bundle(api_key: &str, extra_ca_bundle_path: Option<String>) -> Result<OpenAiClient, OpenAiError> {
        let cfg = OpenAiConfig {
            base_url: "https://api.openai.com/".to_string(),
            api_key: SecretString::from(api_key.to_string()),
            extra_ca_bundle_path,
        };
        OpenAiClient::new(&cfg, Duration::from_secs(10), Duration::from_secs(30), Duration::from_secs(300))
    }

    fn tmp_file_with(contents: &[u8]) -> tempfile::NamedTempFile {
        use std::io::Write;
        let mut f = tempfile::NamedTempFile::new().expect("temp file");
        f.write_all(contents).expect("write");
        f.flush().expect("flush");
        f
    }
```

Then add the four tests:

```rust
    // ---- extra_ca_bundle_path plumbing (SMA-558 D6) ------------------------------------------
    // These prove the gateway's OWN wiring: the config field reaches reqwest and each failure
    // mode maps to CaBundle. They deliberately prove NOTHING about whether a handshake against a
    // private-CA upstream succeeds — `reqwest::Client` exposes no trust-store accessor, so no
    // test here could observe that. IAM's `tests/authn_private_ca.rs` proves it once for the
    // shared reqwest mechanism.

    #[test]
    fn valid_ca_bundle_builds_the_client() {
        let f = tmp_file_with(test_ca_pem().as_bytes());
        client_with_bundle("sk-x", Some(f.path().to_str().unwrap().to_string())).expect("a valid bundle must build");
    }

    #[test]
    fn missing_ca_bundle_path_is_a_build_error() {
        let err = client_with_bundle("sk-x", Some("/nonexistent/paigasus-sma558/ca.pem".to_string()))
            .expect_err("a nonexistent bundle path must fail");
        assert!(matches!(err, OpenAiError::CaBundle { .. }), "expected CaBundle, got {err:?}");
    }

    #[test]
    fn certificate_free_ca_bundle_is_a_build_error() {
        // `from_pem_bundle` returns Ok(vec![]) for a file with no CERTIFICATE section, so only an
        // explicit is_empty() check catches this.
        let f = tmp_file_with(b"-----BEGIN PRIVATE KEY-----\nMC4CAQAwBQYDK2VwBCIEIA==\n-----END PRIVATE KEY-----\n");
        let err = client_with_bundle("sk-x", Some(f.path().to_str().unwrap().to_string()))
            .expect_err("a certificate-free bundle must fail");
        assert!(matches!(err, OpenAiError::CaBundle { .. }), "expected CaBundle, got {err:?}");
    }

    #[test]
    fn undecodable_ca_bundle_is_a_build_error() {
        let f = tmp_file_with(b"-----BEGIN CERTIFICATE-----\n!!!not base64!!!\n-----END CERTIFICATE-----\n");
        let err = client_with_bundle("sk-x", Some(f.path().to_str().unwrap().to_string()))
            .expect_err("an undecodable bundle must fail");
        assert!(matches!(err, OpenAiError::CaBundle { .. }), "expected CaBundle, got {err:?}");
    }
```

`test_ca_pem()` mints the certificate at runtime rather than committing a PEM blob — matching the
convention `paigasus-iam/tests/support/mod.rs` records on `es256_keypair` ("no committed PEM/JWK
fixtures"). Add it to the same test module:

```rust
    /// A throwaway CA certificate, minted fresh per test run. Used ONLY to prove the bundle path
    /// parses and reaches the builder — nothing here is ever trusted by a real handshake, and no
    /// gateway test starts a TLS listener (SMA-558 D6). Minted rather than committed so there is
    /// no fixture to rot, and none for a future reader to mistake for a real trust anchor.
    fn test_ca_pem() -> String {
        let mut params = rcgen::CertificateParams::new(Vec::new()).expect("ca params");
        params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        params.distinguished_name.push(rcgen::DnType::CommonName, "paigasus-gateway-test-ca");
        let key = rcgen::KeyPair::generate().expect("ca keypair");
        params.self_signed(&key).expect("self-signed ca").pem()
    }
```

- [ ] **Step 3: Run the tests to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-gateway --lib -E 'test(ca_bundle)' --no-tests=pass
```

Expected: FAIL to compile — missing field `extra_ca_bundle_path`, unknown variant `CaBundle`.

- [ ] **Step 4: Add the config field, the error variant, and the fold**

In `config.rs`, add to `OpenAiConfig` after `api_key`:

```rust
    /// Extra trust anchors for the outbound upstream calls, as a path to a PEM bundle.
    ///
    /// **This ADDS to the trust store, it does not replace it** — the client trusts the
    /// compiled-in Mozilla roots, the image's own store, AND every certificate here. The
    /// opposite of the sibling `iam.tls.ca_cert_path`, which PINS; hence the `extra_` prefix.
    /// For a self-hosted vLLM/LiteLLM upstream behind a corporate CA (SMA-558).
    ///
    /// **ROOTS ONLY** — every certificate here becomes an unconstrained trust anchor for every
    /// HTTPS call this process makes. Read once at boot; an unreadable, malformed or
    /// certificate-free bundle is a hard boot failure. Mirrors
    /// `paigasus-iam`'s `authn.extra_ca_bundle_path`.
    #[serde(default)]
    pub extra_ca_bundle_path: Option<String>,
```

Update the `OpenAiDefaults` comment (line ~155-158) to note the second omission, exactly as Task 3 Step 4 did for `AuthnDefaults`, and add to `validate()` after the empty-API-key check:

```rust
        if self.upstream.openai.extra_ca_bundle_path.as_deref().is_some_and(str::is_empty) {
            return Err("upstream.openai.extra_ca_bundle_path must not be empty (omit the key entirely to use the default trust store)".to_string());
        }
```

In `client.rs`, add the variant to `OpenAiError`:

```rust
    /// The configured CA bundle could not be read, parsed, or contained no certificates — a
    /// boot-time fault like `Build`, never a request-time one, so G7's status mapping is
    /// unaffected.
    #[error("failed to load upstream.openai.extra_ca_bundle_path {path:?}")]
    CaBundle {
        path: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
```

and the fold in `OpenAiClient::new`, replacing the current builder chain:

```rust
        let mut builder = reqwest::Client::builder().connect_timeout(connect_timeout).read_timeout(stream_idle_timeout);

        if let Some(path) = cfg.extra_ca_bundle_path.as_deref() {
            let ca_bundle = |source: Box<dyn std::error::Error + Send + Sync>| OpenAiError::CaBundle { path: path.to_string(), source };

            let pem = std::fs::read(path).map_err(|e| ca_bundle(Box::new(e)))?;
            // `from_pem_bundle`, NOT `from_pem`: a bundle may carry more than one ROOT and
            // `from_pem` reads only the first. Not an invitation to add intermediates — see the
            // ROOTS ONLY note on the config field. Mirrors
            // `paigasus-iam/src/adapters/oidc/jwks.rs`'s copy; see SMA-558 D7 for why the two are
            // duplicated rather than extracted.
            let certs = reqwest::Certificate::from_pem_bundle(&pem).map_err(|e| ca_bundle(Box::new(e)))?;
            // Ok(vec![]) is what a DER .crt, a key-only PEM or an empty file parses to, so
            // without this the likeliest operator mistake boots green having added nothing.
            if certs.is_empty() {
                return Err(ca_bundle(
                    "contained no PEM certificates — a DER file, a key-only PEM or an empty file parses as an empty bundle".into(),
                ));
            }
            tracing::info!(path = %path, count = certs.len(), "loaded extra upstream trust anchors from upstream.openai.extra_ca_bundle_path");
            for cert in certs {
                builder = builder.add_root_certificate(cert);
            }
        }

        let http = builder.build().map_err(OpenAiError::Build)?;
```

- [ ] **Step 5: Fix the exhaustive match and every `OpenAiConfig` literal**

`impl From<OpenAiError> for GatewayError` (`adapters/http/error.rs:87-92`) is an exhaustive `match` with no wildcard, and `warnings = "deny"` makes a new variant a **build failure**. Add `CaBundle` to the 502 arm:

```rust
            OpenAiError::Connect(_) | OpenAiError::Transport(_) | OpenAiError::Build(_) | OpenAiError::CaBundle { .. } => GatewayError::UpstreamUnavailable,
```

`UpstreamUnavailable` reuses an existing canonical registry code, so no error-registry change is needed. Extend that function's doc comment to mention the bundle case, and add a row to the parity test at `:209-212`:

```rust
        assert_eq!(
            GatewayError::from(OpenAiError::CaBundle { path: "/x".to_string(), source: "boom".into() }),
            GatewayError::UpstreamUnavailable
        );
```

Then add `extra_ca_bundle_path: None,` to each remaining `OpenAiConfig` literal:

- `src/service_info.rs:103`
- `src/adapters/http/mod.rs:233`
- `tests/openai_egress.rs:24`
- `tests/service_info.rs:147`
- `tests/chat_proxy.rs:130`
- `tests/metrics.rs:93`
- `tests/metrics.rs:145`

- [ ] **Step 6: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo build --tests -p paigasus-gateway --locked 2>&1 | tail -10
cd rs && cargo nextest run -p paigasus-gateway --no-tests=pass 2>&1 | tail -20
```

Expected: builds clean, all gateway tests PASS including the four new ones.

- [ ] **Step 7: Commit**

```bash
git add rs/crates/services/paigasus-gateway
git commit -m "feat(rs): add upstream.openai.extra_ca_bundle_path to the gateway (SMA-558)"
```

---

## Task 6: Operator documentation (AC2)

**Files:**
- Modify: `docs/ops/RUNBOOK-containers.md:121-123`
- Modify: `rs/crates/services/paigasus-iam/iam.toml.example` (the `[authn]` block, ~line 25)
- Modify: `rs/crates/services/paigasus-gateway/gateway.toml.example` (the `[upstream.openai]` block, ~line 46)
- Modify: `CLAUDE.md` (Gotchas section)

**Interfaces:**
- Consumes: the field names from Tasks 3 and 5.
- Produces: nothing consumed by code.

- [ ] **Step 1: Verify the chiseled image actually exposes a trust-store path**

This gates what Step 2 may claim. `rustls-native-certs` probes specific candidate paths; if the `ca-certificates_data` slice ships only `/usr/share/ca-certificates/**`, then mounting into `/etc/ssl/certs` is a **no-op** and the runbook must say the config field is the only working route.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
ci/images/run.sh build 2>&1 | tail -5
docker run --rm --entrypoint /usr/local/bin/paigasus-service \
  ghcr.io/paigasus/paigasus-iam:dev --help >/dev/null 2>&1 || true
docker create --name sma558probe ghcr.io/paigasus/paigasus-iam:dev
docker export sma558probe | tar -tv | grep -E 'etc/ssl|ca-certificates' | head -20
docker rm sma558probe
```

(The image has no shell, so inspect the exported filesystem rather than running `ls` inside it. Adjust the image tag to whatever `ci/images/run.sh build` actually produced.)

Record the finding. If `/etc/ssl/certs/ca-certificates.crt` is present, all three routes are real. If it is absent or a dangling symlink, Step 2 must demote route 2 to "does not work in this image".

- [ ] **Step 2: Replace the RUNBOOK bullet**

`docs/ops/RUNBOOK-containers.md:121-123` currently reads:

```markdown
- **A private-CA identity provider is not supported.** IAM's `reqwest`-based path to the IdP
  (discovery/JWKS) carries compiled-in webpki roots, so mounting a CA certificate into the
  container does not make IAM trust it.
```

Replace with (adjusting route 2 per Step 1's finding):

```markdown
- **A private-CA identity provider is supported (SMA-558), and the routes are not equivalent.**
  Both services' `reqwest` clients now trust the compiled-in Mozilla roots, the image's own store,
  **and** any bundle you name — unioned, so no route costs you the public roots. Prefer them in
  this order:

  1. **`authn.extra_ca_bundle_path`** (`IAM_AUTHN__EXTRA_CA_BUNDLE_PATH`), and
     `upstream.openai.extra_ca_bundle_path` for the gateway's upstream. **Recommended** — the only
     route that fails loudly at boot when it is wrong, and the only one with an auditable record
     in config. A rotated bundle needs a restart.
  2. **Mount your CA into `/etc/ssl/certs`.** Works, but the image has no shell, so
     `update-ca-certificates` is unavailable — you must overwrite the bundle with one you
     assembled yourself.
  3. **`SSL_CERT_FILE` / `SSL_CERT_DIR` — last resort.** Setting either makes the process read
     *only* those paths and **ignore the image's own store**, so it replaces rather than adds. A
     path that does not exist, or a file that is not PEM, is silently ignored — no boot error, no
     request error against public hosts, and a still-broken private IdP.

  **Put roots in the bundle, never intermediates.** Every certificate in it becomes an
  unconstrained trust anchor for every outbound HTTPS call the process makes — TLS performs no
  `cA` check on an anchor, so an intermediate is silently promoted to a root.

  **A self-signed *leaf* is still not supported.** Webpki has no support for self-signed
  certificates, so an IdP presenting a bare self-signed certificate (rather than one issued by a
  CA you can name) still requires `authn.accept_invalid_tls`, which disables verification
  entirely. Mint a small private CA and issue the IdP a certificate from it instead.
```

- [ ] **Step 3: Document the field in both example files**

In `iam.toml.example`, after the `accept_invalid_tls = false` line in the `[authn]` block:

```toml
#
# Extra trust anchors for the IdP discovery/JWKS fetches — a path to a PEM bundle. ADDS to the
# trust store (compiled-in Mozilla roots + the image's /etc/ssl/certs); it does NOT replace it.
# Note this is the OPPOSITE of [outbox.publisher].root_ca_bundle below, which REPLACES — hence
# the `extra_` prefix. ROOTS ONLY: every certificate here becomes an unconstrained trust anchor
# for every outbound HTTPS call. Read once at boot, so a rotated bundle needs a restart. Cannot
# be combined with accept_invalid_tls (boot refuses). Does not help a self-signed LEAF.
# extra_ca_bundle_path = "/etc/paigasus/corp-ca.pem"
```

In `gateway.toml.example`, in the `[upstream.openai]` block:

```toml
#
# Extra trust anchors for the upstream calls — a path to a PEM bundle, for a self-hosted
# vLLM/LiteLLM endpoint behind a corporate CA. ADDS to the trust store; the OPPOSITE of
# [iam.tls] ca_cert_path, which PINS. ROOTS ONLY. Read once at boot.
# extra_ca_bundle_path = "/etc/paigasus/corp-ca.pem"
```

- [ ] **Step 4: Add the CLAUDE.md gotcha**

Append to the Gotchas section. **Do not paste the `moon ci` target list or the `<!-- ci-targets:begin/end -->` markers into this entry** — a second copy of either anywhere in the file reds `repo:affected-smoke` (SMA-541).

```markdown
- This repo now has **three** CA-bundle config knobs and they do NOT share semantics. `authn.extra_ca_bundle_path`
  and `upstream.openai.extra_ca_bundle_path` (SMA-558) **ADD** to the trust store — reqwest builds one
  `RootCertStore` by unioning `add_root_certificate` calls with the webpki roots and the platform store, so
  the workspace pins BOTH `rustls-tls` and `rustls-tls-native-roots` (dropping the former is not a
  simplification: reqwest accepts an EMPTY platform store silently, and webpki is the floor that stops a bad
  mount becoming a per-request failure). `outbox.publisher.root_ca_bundle` and the gateway's
  `iam.tls.ca_cert_path` **REPLACE** it. The `extra_` prefix is the marker — a fourth knob must pick a side
  and say which in its doc. Anything in an added bundle becomes an **unconstrained** anchor (no `cA` check),
  so it must contain roots only; and a self-signed LEAF still needs `accept_invalid_tls`, since webpki has no
  support for self-signed certificates.
```

- [ ] **Step 5: Verify no marker duplication**

```bash
grep -c 'ci-targets:begin' CLAUDE.md
```

Expected: `1`.

- [ ] **Step 6: Commit**

```bash
git add docs/ops/RUNBOOK-containers.md rs/crates/services/paigasus-iam/iam.toml.example rs/crates/services/paigasus-gateway/gateway.toml.example CLAUDE.md
git commit -m "docs(repo): document private-CA trust for both services (SMA-558)"
```

---

## Task 7: Full gate run

**Files:** none modified unless a gate fails.

**Interfaces:**
- Consumes: everything.
- Produces: a green graph.

- [ ] **Step 1: Run the full CI target list**

Because `rs/Cargo.toml` and `rs/Cargo.lock` changed, per-project tasks are not sufficient — this schedules the wide Rust graph the way CI does.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :input-liveness :promtool :observability-drift :nats-permissions :release-parity \
  :release-parity-py :release-parity-ts :publish-metadata --base origin/main --include-relations
```

Expected: all green. Per the spec's § 7, `:redis-connect-single-site` and `:iam-docker-policy-single-site` **will** be scheduled by the IAM `src/`+`tests/` changes and are expected to pass.

- [ ] **Step 2: Diagnose any unattributed failure**

Moon reports "N failed" without naming the task. Resolve it with:

```bash
jq '.actions[] | select(.status=="failed") | .label' .moon/cache/ciReport.json
```

- [ ] **Step 3: Confirm `:deny` stayed clean for the reason the spec claimed**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && grep -c '^\[\[package\]\]' Cargo.lock
```

Expected: `543`. If a package was added at any point during implementation, the spec's "no new license to except" argument no longer holds and `rs/deny.toml` may need an exception.

- [ ] **Step 4: Commit any gate fixes**

```bash
git add -A
git commit -m "fix(repo): satisfy the affected-graph gates for SMA-558"
```

Skip if nothing changed.

---

## Self-Review

**Spec coverage:**

| Spec item | Task |
| --- | --- |
| D1 additive trust model | 1 |
| D2 `extra_ca_bundle_path` naming | 3, 5 |
| D3 global, not per-issuer | 3 (single field on `AuthnConfig`) |
| D4 hard boot failure, incl. zero-cert case | 2 (impl + tests 3/4/4b), 5 (gateway mirror) |
| D5 `IdpTls` enum + `validate()` rule | 2 (enum), 3 (rule) |
| D6 gateway plumbing tests only | 5 |
| D7 duplicated fold, cross-referenced | 2, 5 (comments cite each other) |
| D8 success-path log | 2, 5 |
| § 5.1 fixture (distinct CNs, IP SAN, `signed_by` consumes self) | 4 Step 1 |
| § 5.2 tests 1-8b | 2 (3/4/4b), 3 (5), 4 (1/2), 5 (6/7/8/8b) |
| § 5.3 Docker-free seam | 4 Steps 2-4 |
| § 2.7 chiseled-slice verification | 6 Step 1 |
| § 6 ranked routes, roots-only, self-signed limit | 6 Step 2 |
| § 7 gate table | 7 |
| AC1 / AC2 / AC3 / AC4 | 3+4 / 6 / 4 / 3 (rule only forbids the pair) |

**Type consistency:** `IdpTls::Verify { extra_bundle }` is spelled identically in Tasks 2, 3 and 4. `extra_ca_bundle_path` is the field name in both services throughout. `OpenAiError::CaBundle { path, source }` is constructed in Task 5 Step 4 and matched in Step 5 with the same field names.

**Known ordering coupling:** Task 2 Step 4 references `authn_cfg.extra_ca_bundle_path`, which Task 3 Step 3 creates. Flagged inline in Task 2 Step 4 — an implementer hitting a compile error there should pull Task 3 Step 3 forward rather than inventing a placeholder.
