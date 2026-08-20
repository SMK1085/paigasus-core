# SMA-558 — A private-CA OIDC issuer must be trustable without disabling verification

**Status:** draft for adversarial review (2026-08-20)
**Linear:** [SMA-558](https://linear.app/smaschek/issue/SMA-558/iam-a-private-ca-oidc-issuer-is-not-trustable-reqwest-carries-compiled)
**Supersedes:** SMA-500 design § 2.2's `reqwest` trust-source row, and its deferred item 3
**Related:** SMA-493 D7 (`outbox.publisher.root_ca_bundle`), SMA-504 D8 (`iam.tls.ca_cert_path`)

## 1. Problem

A self-hoster whose OIDC issuer sits behind a private or corporate CA cannot make `paigasus-iam`
trust it. IAM's discovery and JWKS fetches go through a `reqwest::Client` whose rustls trust
anchors are the Mozilla roots compiled into `webpki-roots`. Mounting a CA certificate into the
container does nothing, `SSL_CERT_FILE` does not apply, and no configuration field exists.

The only escape today is `authn.accept_invalid_tls`, which `config.rs:119-124` documents as
disabling certificate verification outright — "it lets any on-path attacker serve a forged JWKS,
which is a full authentication bypass". Telling an enterprise operator to run production that way
is not an answer.

The behaviour is pre-existing; containerization (SMA-500) is what made it operator-visible, and
that issue deferred the fix here.

`paigasus-gateway` has the identical hole on its outbound upstream client, for a self-hosted
vLLM/LiteLLM endpoint behind the same corporate CA. Because `reqwest` is a *workspace* dependency
consumed by both services, the mechanism half of the fix reaches the gateway whether or not this
issue addresses it. This spec therefore covers both services rather than leaving the gateway with
a silently-changed trust store and no config surface of its own.

## 2. Evidence

Measured on 2026-08-20 against `origin/main` (`af5a302`), `reqwest 0.12.28`, `rcgen 0.13.2`.

### 2.1 The trust sources, per crate

| Consumer | Trust source | Reads `/etc/ssl/certs`? |
| --- | --- | --- |
| `tonic` (gateway → IAM) | `tls-native-roots` → `rustls-native-certs 0.8.4` | Yes |
| `async-nats` (IAM outbox) | native roots, unless `root_ca_bundle` is set | Yes |
| `reqwest` (IAM → IdP, gateway → upstream) | `rustls-tls` → `webpki-roots 1.0.8`, compiled in | **No** |
| `sqlx`/`sea-orm` (Postgres) | `webpki-roots 0.26.11`, compiled in | No |
| `redis` | `tls-rustls-webpki-roots`, compiled in | No |

This asymmetry is what makes the failure confusing to diagnose: two links in the same process read
the image trust store and one does not.

One correction to the issue text, which claims "only the reqwest and sqlx/redis paths are
webpki-pinned": `redis 1.3.0` resolves **both** `rustls-native-certs` and `webpki-roots` in the
graph. Which one it *uses* is decided by the feature selection at `rs/Cargo.toml:161`
(`tls-rustls-webpki-roots`), not by what appears in `cargo tree`. The table above states the
effective behaviour. Neither `redis` nor `sqlx` is in scope here.

### 2.2 `reqwest` composes its root store additively, from three independent sources

From `reqwest-0.12.28/src/async_impl/client.rs:688-730`, the rustls branch builds one
`RootCertStore` in this order:

1. every certificate passed to `add_root_certificate()` (`:688-690`), unconditionally;
2. the webpki Mozilla roots, if `tls_built_in_certs_webpki` (`:692-695`);
3. the native/system certs, if `tls_built_in_certs_native` (`:697-730`).

Both feature-gated blocks default to `true` when their feature is enabled (`:319-322`), and
`tls_built_in_webpki_certs(bool)` / `tls_built_in_native_certs(bool)` toggle them independently
(`:1935`, `:1945`). Enabling both features therefore yields **webpki ∪ system ∪ explicit**, not a
choice between them.

### 2.3 An empty system trust store is silently accepted

`:714` errors only when `valid_count == 0 && invalid_count > 0` — that is, when certificates were
found but none parsed. A store that is entirely *absent* or *empty* produces zero of each, falls
through the condition, and yields a working `Client` with no trust anchors whose every handshake
then fails at request time.

This is decisive for D1. A native-roots-*only* posture would convert a bad or missing bundle mount
into a late, silent, per-request failure. Keeping `webpki-roots` enabled alongside makes an empty
store unrepresentable: there is always a floor.

### 2.4 The feature change costs zero new packages

Measured by editing `rs/Cargo.toml`, running `cargo metadata`, and diffing the lockfile:

```
package count before: 543
package count after:  543
lockfile delta:       2 insertions
```

Both insertions are a `rustls-native-certs` dependency *edge* — one under `hyper-rustls`, one
under `reqwest`. The crate is already vendored via `tonic`, `async-nats` and `redis`. So there is
no new license to except in `rs/deny.toml`, no new advisory surface, and no supply-chain
expansion. The probe was reverted; the lockfile in this branch is unmodified until implementation.

### 2.5 The repo already has two CA-path knobs, and both REPLACE the trust store

`outbox.publisher.root_ca_bundle` (`config.rs:567-579`, SMA-493 D7) — in this very config struct:

> **This REPLACES the system trust store, it does not extend it.** […] Concatenate every CA the
> client needs into one file — naming only a private CA and later moving the broker behind a public
> one is a total outage that presents as a bare TLS error.

`iam.tls.ca_cert_path` (gateway `config.rs:57-60`, consumed at `adapters/iam/client.rs:225-230`,
SMA-504 D8) — "the client PINS to this CA alone — it REPLACES the system trust store, not adds to
it", argued as "narrower, and therefore stronger".

Both are correct for what they secure: a single internal endpoint the operator controls. The OIDC
case differs materially, which is D3.

### 2.6 The test fixture exists in IAM and does not exist in the gateway

`paigasus-iam` carries `rcgen`, `axum-server` and `tempfile` as dev-dependencies
(`Cargo.toml:155-156,170`), and `tests/support/mod.rs:238-285` already starts an in-process HTTPS
mock IdP on an ephemeral port using `rcgen::generate_simple_self_signed`. It is Docker-free.

`rcgen 0.13.2` supports the two-level chain this needs: `IsCa::Ca(BasicConstraints::Unconstrained)`
(`certificate.rs:1197-1211`) and `CertificateParams::signed_by()` (`certificate.rs:150`).

`paigasus-gateway` carries none of those dev-dependencies and has no HTTPS test infrastructure —
its suite drives the router via `ServiceExt::oneshot`, with one plain-TCP listener in
`tests/chat_proxy.rs`. This asymmetry drives D6.

### 2.7 The runtime image ships a CA store but no way to update it

`rs/Dockerfile:50-52` chisels `ca-certificates_data` into the rootfs, so `/etc/ssl/certs` exists
and a mounted bundle is meaningful. But the final stage is `FROM scratch` with no shell, so
`update-ca-certificates` — the normal way to *add* a CA to a Debian/Ubuntu store — cannot be run.
An operator's only file-based routes are to overwrite `ca-certificates.crt` wholesale with a
bundle they assembled themselves, or to point `SSL_CERT_FILE` at one. Both replace rather than
extend. This is the strongest argument for a config field existing at all.

## 3. Decisions

### D1 — trust anchors are webpki ∪ system ∪ optional explicit bundle, all additive

```toml
reqwest = { version = "0.12", default-features = false,
            features = ["rustls-tls", "rustls-tls-native-roots", "json"] }
```

Both root features stay on. Per § 2.2 this unions the three sources rather than choosing between
them, and per § 2.3 keeping `rustls-tls` is what makes an empty system store harmless instead of
silently fatal.

The operator gains three composable routes, and no route can lose the public roots:

1. mount a CA into `/etc/ssl/certs`;
2. `SSL_CERT_FILE=/etc/paigasus/corp-ca.pem` (honoured by `rustls-native-certs`);
3. `authn.extra_ca_bundle_path = "/etc/paigasus/corp-ca.pem"`.

Rejected — **native roots only, no config field**: § 2.3's silent-empty-store failure, § 2.7's
no-shell constraint forcing operators to hand-assemble a complete bundle, and it leaves AC3 with
nothing to test but a process-wide `SSL_CERT_FILE` mutation that races every other test in the
binary.

Rejected — **explicit field only, keeping webpki as the sole base**: mounting a CA into the
container would still do nothing and `SSL_CERT_FILE` would still not apply, so the runbook's
complaint is only half-answered.

### D2 — the field is named `extra_ca_bundle_path`, and the `extra_` is load-bearing

Per § 2.5 this repo already has two CA-path knobs and both *replace* the trust store, one of them
in the same `AuthnConfig` file. A third field named `ca_bundle_path` or `root_ca_bundle` — the
issue's own suggestion — would carry their name shape with inverted semantics, which is a trap for
the next reader and a plausible production misconfiguration.

`extra_` is the ecosystem's own marker for this exact distinction: `NODE_EXTRA_CA_CERTS` is
additive, while `REQUESTS_CA_BUNDLE` and git's `http.sslCAInfo` replace.

| Field | Semantics |
| --- | --- |
| `authn.extra_ca_bundle_path` (new) | **adds** to webpki + system |
| `upstream.openai.extra_ca_bundle_path` (new) | **adds** to webpki + system |
| `outbox.publisher.root_ca_bundle` | replaces the system store |
| `iam.tls.ca_cert_path` (gateway) | replaces / pins |

### D3 — one global field per service, not per-issuer

`authn.issuers` is a `Vec`, so `authn.issuers[].extra_ca_bundle_path` was a real option and the
issue floats it. Rejected on YAGNI: additive semantics (D1) already make a mixed public + private
issuer set work, so per-issuer buys only trust *narrowing*, at the cost of a `reqwest::Client` per
issuer, a per-issuer connection pool, a lookup in `fetch()`, and a decision about what an
unconfigured issuer gets — all inside `jwks.rs`, the file that hosts the rotation and single-flight
logic.

The accepted cost: a bundle named here is trusted for *every* configured issuer, including a
publicly-issued one. That is precisely what adding a CA to a system trust store already means, and
`HttpJwksFetcher` keeps its single client unchanged.

### D4 — a bad bundle is a hard boot failure, not a warning

An unreadable or unparseable bundle aborts boot, matching the gateway's tonic path, which fails at
boot today (`adapters/iam/client.rs:227`). Failing late instead would present as every token
validation returning `Unavailable` with no indication that a config path is at fault.

This requires fixing a defect on the way: `HttpJwksFetcher::new` currently maps *every*
client-build failure to `AuthnError::Unavailable` (`jwks.rs:102`), discarding the cause. Once a
config-supplied path can fail there, "authentication backend unavailable" with no further detail is
not a diagnosable boot error. The variant becomes `AuthnError::Backend`, which already carries a
boxed source and is already used for a wiring defect two lines below (`adapters/http/mod.rs:686`).

### D5 — `accept_invalid_tls` + a bundle is rejected at `validate()`

`danger_accept_invalid_certs(true)` makes the root store irrelevant, so the combination is always
an operator mistake and the bundle is provably dead. Rejecting it follows D8's own reasoning about
making invalid states unrepresentable rather than documenting a footgun.

AC4 is unaffected: `accept_invalid_tls` on its own keeps working exactly as it does today, and the
existing boot warning (`adapters/http/mod.rs:665-667`) stays. The new field is new, so nothing
currently valid becomes invalid.

### D6 — the gateway gets the field and plumbing-level tests, not a TLS mock upstream

Per § 2.6 the gateway has no HTTPS test infrastructure. Full parity would mean three new
dev-dependencies, a second copy of the chain-minting fixture in a crate with no `support/` module
to host it, and a TLS listener in a suite that has deliberately avoided real listeners.

Instead the gateway gets `tempfile` and three unit tests over `OpenAiClient::new` covering a valid
static PEM fixture, a missing path, and a malformed file. What that proves is the gateway's own
plumbing: the config field reaches `reqwest`, and both failure modes map to the new error variant.
What it does not prove — that the resulting client completes a handshake against a private-CA
server — is proven once by IAM's § 5 test pair, over the same crate and the same builder API.

Stated plainly so nobody later mistakes the coverage for more than it is.

### D7 — the fold is duplicated in both services rather than extracted

The shared part is three lines (`read` → `from_pem_bundle` → fold `add_root_certificate`); the
error mapping is the bulk and differs by service (`AuthnError` vs `OpenAiError`, with different
config-key names in the messages). Against that, a new `libs/` crate reds `repo:affected-smoke`
until it is added to the `lockfile->all-lint` expected set in `ci/affected-graph/run.sh`, and
`paigasus-kernel` is disqualified outright — it is wasm-bound and must stay dependency-light
(SMA-448). The two copies cross-reference each other by path in their doc comments.

## 4. Architecture

### 4.1 Files

| File | Change |
| --- | --- |
| `rs/Cargo.toml` | `reqwest` features += `rustls-tls-native-roots` |
| `rs/Cargo.lock` | regenerated (2 edges, 0 packages) |
| `iam/src/config.rs` | `AuthnConfig.extra_ca_bundle_path`; `validate()` D5 rule; `AuthnDefaults` comment |
| `iam/src/adapters/oidc/jwks.rs` | `HttpJwksFetcher::new` signature + fold + D4 error fix; unit tests 3/4 |
| `iam/src/adapters/http/mod.rs` | pass the new argument at the call site |
| `iam/tests/support/mod.rs` | `start_mock_idp_private_ca()` fixture |
| `iam/tests/authn_private_ca.rs` | new, Docker-free: tests 1 and 2 |
| `iam/iam.toml.example` | document the field under `[authn]` |
| `gateway/src/config.rs` | `OpenAiConfig.extra_ca_bundle_path` |
| `gateway/src/adapters/openai/client.rs` | fold + `OpenAiError::CaBundle` + three unit tests |
| `gateway/Cargo.toml` | `tempfile` dev-dependency |
| `gateway/gateway.toml.example` | document the field under `[upstream.openai]` |
| `docs/ops/RUNBOOK-containers.md` | replace the § 5 "not supported" bullet (AC2) |
| `CLAUDE.md` | gotcha: three CA knobs, two replace and one extends |

### 4.2 IAM config

`AuthnConfig` gains:

```rust
/// Extra trust anchors for the IdP discovery/JWKS fetches, as a path to a PEM bundle.
///
/// **This ADDS to the trust store, it does not replace it.** The client trusts the compiled-in
/// Mozilla roots, the image's own store (`/etc/ssl/certs`, honouring `SSL_CERT_FILE`), AND
/// every certificate in this bundle. Note this is the OPPOSITE of the sibling
/// `outbox.publisher.root_ca_bundle` and of the gateway's `iam.tls.ca_cert_path`, both of
/// which REPLACE — hence the `extra_` prefix (cf. `NODE_EXTRA_CA_CERTS`).
///
/// Read once at boot. An unreadable or malformed bundle is a hard boot failure, never a
/// warning. Mutually exclusive with `accept_invalid_tls`, which would make it dead.
#[serde(default)]
pub extra_ca_bundle_path: Option<String>,
```

It is deliberately **absent** from `AuthnDefaults`. That struct feeds figment's defaults layer, and
an `Option` carrying `#[serde(default)]` resolves to `None` without an entry — the same reason
`issuers` is omitted. `AuthnDefaults`' "mirrors `AuthnConfig` minus `issuers`" comment is updated to
name both omissions and say why they differ.

### 4.3 The fetcher

```rust
pub fn new(timeout: Duration, accept_invalid_tls: bool, extra_ca_bundle_path: Option<&str>)
    -> Result<Self, AuthnError>
{
    let mut builder = reqwest::Client::builder()
        .timeout(timeout)
        .danger_accept_invalid_certs(accept_invalid_tls);

    if let Some(path) = extra_ca_bundle_path {
        let pem = std::fs::read(path).map_err(|e| backend(format!(
            "failed to read authn.extra_ca_bundle_path {path:?}: {e}")))?;
        // from_pem_bundle, NOT from_pem: the latter reads only the FIRST certificate, which
        // would silently ignore an intermediate in a two-cert chain.
        for cert in reqwest::Certificate::from_pem_bundle(&pem).map_err(|e| backend(format!(
            "authn.extra_ca_bundle_path {path:?} is not a valid PEM certificate bundle: {e}")))?
        {
            builder = builder.add_root_certificate(cert);
        }
    }

    let client = builder.build().map_err(|e| backend(format!(
        "failed to build the IdP HTTP client: {e}")))?;
    Ok(Self { client, clock: SystemClock })
}
```

`backend(..)` is shorthand for the idiom this file already uses at `adapters/http/mod.rs:605` —
`AuthnError::Backend(format!("…").into())`, which works because std provides
`From<String> for Box<dyn Error + Send + Sync>`. The implementation may inline it rather than
introduce a helper. The final `map_err` is D4's defect fix.

### 4.4 The `validate()` rule

```
authn.accept_invalid_tls = true disables certificate verification entirely, so
authn.extra_ca_bundle_path can never take effect — set one or the other, not both.
```

### 4.5 Gateway

`OpenAiConfig` gains the same field with the same doc shape. `OpenAiClient::new` performs the same
fold, and `OpenAiError` gains one variant, because `std::fs::read` yields `std::io::Error` while
today's `Build` wraps only `reqwest::Error`:

```rust
/// The configured CA bundle could not be read or parsed — a boot-time fault like `Build`,
/// never a request-time one, so G7's HTTP status mapping is unaffected.
#[error("failed to load upstream.openai.extra_ca_bundle_path {path:?}")]
CaBundle { path: String, #[source] source: Box<dyn std::error::Error + Send + Sync> },
```

The gateway's `validate()` needs no D5 rule — it has no `accept_invalid_tls` equivalent.

## 5. Testing

### 5.1 The fixture

`tests/support/mod.rs` gains, beside the existing `start_mock_idp`:

```rust
pub async fn start_mock_idp_private_ca() -> (MockIdp, String /* CA cert PEM */)
```

It mints a genuine two-level chain: a self-signed CA with
`IsCa::Ca(BasicConstraints::Unconstrained)`, then a leaf for `localhost` / `127.0.0.1` via
`CertificateParams::signed_by(&leaf_key, &ca_cert, &ca_key)`.

The server is configured with **the leaf alone**, not the chain. That is correct TLS practice for a
root the client is expected to hold, and it makes the test strict: the client cannot learn the CA
from the handshake, so it can only succeed if the bundle genuinely loaded.

### 5.2 Assertions

| # | Test | Location | Asserts |
| --- | --- | --- | --- |
| 1 | `private_ca_issuer_validates_with_extra_ca_bundle` | `tests/authn_private_ca.rs` | CA PEM → `NamedTempFile`, `accept_invalid_tls: false`, bundle set → authentication **succeeds** |
| 2 | `private_ca_issuer_fails_without_extra_ca_bundle` | `tests/authn_private_ca.rs` | byte-identical setup, bundle `None` → `AuthnError::Unavailable` |
| 3 | `missing_bundle_path_is_a_boot_error` | `jwks.rs` unit | nonexistent path → `Backend`, not `Unavailable` |
| 4 | `malformed_bundle_is_a_boot_error` | `jwks.rs` unit | file of garbage → `Backend` |
| 5 | `accept_invalid_tls_with_bundle_is_rejected` | `config.rs` unit | `validate()` returns `Err` |
| 6 | `ca_bundle_is_loaded_into_the_client` | gateway `client.rs` unit | static CA PEM fixture → `Ok` |
| 7 | `missing_ca_bundle_path_is_a_build_error` | gateway `client.rs` unit | → `CaBundle` |
| 8 | `malformed_ca_bundle_is_a_build_error` | gateway `client.rs` unit | → `CaBundle` |

**Test 2 is load-bearing and must not be dropped as redundant.** Test 1 alone would pass vacuously
if anything else in the trust path happened to accept that certificate. Because 1 and 2 differ in
exactly one field and talk to the same mock IdP, the pair is a controlled comparison and the
failure is attributable to the trust anchor.

Tests 1 and 2 are Docker-free — the mock IdP is in-process axum — so they run on every machine and
in every CI leg, and need no `docker.rs` policy participation.

## 6. Documentation

`docs/ops/RUNBOOK-containers.md` § 5, lines 121-123, currently reads "**A private-CA identity
provider is not supported.**" It is replaced (AC2) by the three routes of D1, stated as composable,
with two caveats that matter in this image specifically: there is no shell, so
`update-ca-certificates` is unavailable (§ 2.7) — which is why the config field exists; and unlike
`outbox.publisher.root_ca_bundle`, none of the three routes costs you the public roots.

`iam.toml.example` and `gateway.toml.example` document the field with the same
`extra_` vs `root_ca_bundle` contrast, since an operator reading one will assume the other.

`CLAUDE.md` gains a gotcha recording that the repo now has three CA-path knobs, two replacing and
one extending, and which convention a fourth should join.

`docs/superpowers/specs/2026-08-19-sma-500-...-design.md` is **not** edited. Its § 2.2 row and
deferred item 3 become false, but dated design docs are records of what was true when written; this
spec's header states the supersession.

## 7. CI gates

The `rs/Cargo.toml` + `rs/Cargo.lock` touch schedules a wide Rust graph, so the full CI line runs
before push, not per-project tasks.

| Gate | Expectation | Basis |
| --- | --- | --- |
| `:deny` | clean | § 2.4 — zero new packages, so no license exception and no advisory surface |
| `:machete` | clean | `tempfile` is consumed in the same commit, so no `ignored` allowlist entry |
| `:affected-smoke` | clean | no new crate, so `lockfile->all-lint`'s expected set is untouched |
| `:error-code-single-site` | clean | neither `jwks.rs` nor `openai/client.rs` is in `check.py`'s `MANIFEST`; `AuthnError::Backend` is an internal type, not a wire code |
| `:input-liveness` | clean | no file moves or renames |
| `:observability-drift` | clean | no metrics touched |
| `:release-parity*`, `:publish-metadata` | clean | no version or manifest changes |

Every row is an expectation to *verify by running*, not to assume.

## 8. Risks

1. **Enabling native roots widens the trust store for every `reqwest` call in both services,
   permanently and by default.** An operator with a publicly-issued IdP now also trusts whatever
   their image's CA bundle contains, including anything a base-image update adds. This is the
   accepted cost of D1 and the standard posture across the ecosystem, but it is a real trust-surface
   expansion and not merely a convenience.
2. **No pinning story.** D1 can only widen trust, never narrow it. An operator wanting "trust my
   corporate CA *and nothing else*" is not served — `tls_built_in_webpki_certs(false)` /
   `tls_built_in_native_certs(false)` make it reachable later, but no field exposes them here. There
   is no pinning today either, so this is unaddressed rather than regressed.
3. **D3's global scope means a bundle is trusted for every issuer.** A mixed public + private issuer
   set works, but the private CA could vouch for the public issuer's hostname. Per-issuer clients
   remain a later refinement.
4. **The gateway's handshake path is proven only by inference** (D6) — via IAM's test pair over the
   same crate and builder API, not by a gateway-side TLS test.
5. **`rustls-native-certs` reads the OS store at client-build time**, which on developer macOS means
   the system keychain. Slower than a compiled-in slice and platform-dependent, though bounded to
   client construction rather than per-request.

## 9. Out of scope

- `sqlx`/`sea-orm` and `redis` trust stores (§ 2.1) — still webpki-pinned. Postgres and Redis are
  operator-controlled infrastructure links, not third-party issuers, and neither is named by
  SMA-558's acceptance criteria.
- Certificate *pinning* for OIDC issuers (risk 2).
- Per-issuer trust anchors (D3, risk 3).
- mTLS to the IdP. `iam.tls` offers client certificates for the gateway→IAM link; nothing requests
  the same for IdP fetches.

## 10. Acceptance criteria mapping

| AC | Where |
| --- | --- |
| 1. Trust a private-CA issuer without disabling verification | D1's three routes; proven by test 1 with `accept_invalid_tls: false` |
| 2. Documented in `RUNBOOK-containers.md`, replacing "not supported" | § 6 |
| 3. A test covers the trusted-private-CA path | § 5, tests 1 + 2, using the § 2.6 fixture shape |
| 4. `accept_invalid_tls` remains available, no longer the only route | Unchanged behaviour; D5 only forbids combining it with the new field |
