# SMA-558 — A private-CA OIDC issuer must be trustable without disabling verification

**Status:** revised after adversarial review (2026-08-20)
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

Measured on 2026-08-20 against `origin/main` (`af5a302`), `reqwest 0.12.28`, `rcgen 0.13.2`,
`rustls-native-certs 0.8.4`, `rustls-webpki 0.103.14`, `rustls-pki-types 1.15.0` (source read from
the vendored 1.14.0 copy; the PEM reader is unchanged between them).

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

From `reqwest-0.12.28/src/async_impl/client.rs:687-732`, the rustls branch builds one
`RootCertStore` in this order:

1. every certificate passed to `add_root_certificate()` (`:687-690`), unconditionally;
2. the webpki Mozilla roots, if `tls_built_in_certs_webpki` (`:692-695`);
3. the native/system certs, if `tls_built_in_certs_native` (`:697-732`).

Both feature-gated blocks default to `true` when their feature is enabled (`:319-322`), and
`tls_built_in_webpki_certs(bool)` / `tls_built_in_native_certs(bool)` toggle them independently
(`:1935`, `:1945`). Enabling both features therefore yields **webpki ∪ system ∪ explicit**, not a
choice between them.

Enabling both is safe with respect to the crypto provider: `reqwest/Cargo.toml:148,154-172` shows
`rustls-tls` and `rustls-tls-native-roots` both alias through to `__rustls-ring`, so there is no
second `CryptoProvider` and no repeat of the `aws-lc-rs` panic that `rs/Cargo.toml:206-210`
records.

### 2.3 An empty system trust store is silently accepted; an all-invalid one is fatal

`client.rs:715` errors only when `valid_count == 0 && invalid_count > 0` — that is, when
certificates were found but none parsed. Two consequences, and the spec needs both:

- A store that is entirely *absent* or *empty* produces zero of each, falls through the condition,
  and yields a working `Client` with no trust anchors whose every handshake then fails at request
  time. This is decisive for D1: a native-roots-*only* posture would convert a bad or missing
  bundle mount into a late, silent, per-request failure. Keeping `webpki-roots` enabled alongside
  makes an empty store unrepresentable — there is always a floor.
- A store whose certificates *all* fail to parse is a hard `Err` from `ClientBuilder::build()`.
  Under D4 that becomes a **boot failure of both services**, triggered by the host or image trust
  store rather than by any Paigasus config. This is a new failure mode the feature introduces; see
  risk 5 and D4's error wording.

### 2.4 The feature change costs zero new packages

Measured by editing `rs/Cargo.toml`, running `cargo metadata`, and diffing the lockfile:

```
package count before: 543
package count after:  543
lockfile delta:       2 insertions
```

Both insertions are a `rustls-native-certs` dependency *edge* — one under `hyper-rustls`, one
under `reqwest`. The crate is already vendored via `tonic`, `async-nats` and `redis`
(`rs/Cargo.lock:3575-3585`), and `rs/deny.toml:2` sets `all-features = true`, so cargo-deny
already audited it before this change. No new license to except, no new advisory surface, no
supply-chain expansion. The probe was reverted; the lockfile in this branch is unmodified until
implementation.

### 2.5 The repo already has two CA-path knobs, and both REPLACE the trust store

`outbox.publisher.root_ca_bundle` (`config.rs:567-579`, SMA-493 D7) — in this very config struct:

> **This REPLACES the system trust store, it does not extend it.** […] Concatenate every CA the
> client needs into one file — naming only a private CA and later moving the broker behind a public
> one is a total outage that presents as a bare TLS error.

`iam.tls.ca_cert_path` (gateway `config.rs:56-60`, consumed at `adapters/iam/client.rs:225-230`,
SMA-504 D8) — "the client PINS to this CA alone — it REPLACES the system trust store, not adds to
it", argued as "narrower, and therefore stronger".

Both are correct for what they secure: a single internal endpoint the operator controls. The OIDC
case differs materially, which is D3.

### 2.6 The test fixture exists in IAM and does not exist in the gateway

`paigasus-iam` carries `rcgen`, `axum-server` and `tempfile` as dev-dependencies
(`Cargo.toml:155-156,170`), and `tests/support/mod.rs:238-285` already starts an in-process HTTPS
mock IdP on an ephemeral port using `rcgen::generate_simple_self_signed`. `rcgen 0.13.2` supports
the two-level chain this needs: `IsCa::Ca(BasicConstraints::Unconstrained)`
(`certificate.rs:1197-1211`) and `CertificateParams::signed_by()` (`certificate.rs:150`).

**But that fixture has never exercised real certificate verification** — every one of its call
sites runs under `accept_invalid_tls: true` (`tests/support/mod.rs:313`). § 5.1 therefore cannot
assume the existing chain shape is verifiable; it must specify one.

`paigasus-gateway` carries none of those dev-dependencies and has no HTTPS test infrastructure —
its suite drives the router via `ServiceExt::oneshot`, with one plain-TCP listener in
`tests/chat_proxy.rs`. This asymmetry drives D6.

### 2.7 The runtime image ships a CA store but no way to update it

`rs/Dockerfile:50-52` chisels `ca-certificates_data` into the rootfs, and the final stage is
`FROM scratch` with no shell, so `update-ca-certificates` — the normal way to *add* a CA to a
Debian/Ubuntu store — cannot be run. An operator's only file-based routes are to overwrite the
bundle wholesale or to point `SSL_CERT_FILE` at one. Both replace rather than extend. This is the
strongest argument for a config field existing at all.

**Unverified and load-bearing:** whether `ca-certificates_data` actually ships a path that
`openssl_probe` finds (`rustls-native-certs-0.8.4/src/unix.rs:3-10` probes specific candidates). If
the slice ships only `/usr/share/ca-certificates/**`, or a symlink dangling into a slice that was
not cut, then route 1 is a no-op and route 3 is the only working mechanism. **Implementation must
verify this against a built image** (§ 5.3) before § 6's runbook text is written.

### 2.8 A PEM bundle containing zero certificates loads silently

`reqwest::Certificate::from_pem_bundle` (`tls.rs:193-200`) delegates to `read_pem_certs`
(`:231-238`), which `collect()`s `CertificateDer::pem_reader_iter`. In `rustls-pki-types`
`src/pem.rs`, any line not starting with `-----BEGIN ` while no section is open is **skipped**
(`:332-334`), and EOF with no open section returns `Ok(ControlFlow::Break(None))` (`:275-278`) —
ending the iterator.

So `from_pem_bundle` returns **`Ok(vec![])`**, not an error, for a DER-encoded `.crt`, a key-only
PEM, an empty file, an HTML error page saved as `.pem`, or a truncated mount. Each would produce a
client with zero extra anchors and no complaint — reintroducing, through this feature's own field,
exactly the silent-failure mode § 2.3 uses to reject a native-roots-only posture. D4 handles it
explicitly.

A *well-framed* section with undecodable base64 does error, but at `builder.build()` via
`RootCertStore::add` (`tls.rs:207-229`), not at `from_pem_bundle` — so the two cases surface at
different call sites and need different error strings.

### 2.9 Anything in the bundle becomes an unconstrained trust anchor

`Certificate::add_to_rustls` (`tls.rs:207-229`) calls `RootCertStore::add` →
`anchor_from_trusted_cert`, documented at `rustls-webpki-0.103.14/src/trust_anchor.rs:14-15`:

> No additional checks on the content of the certificate, including whether it is self-signed, or
> has a basic constraints extension indicating the `cA` boolean is true, will be performed.

Two consequences. An *intermediate* placed in the bundle becomes a full unconstrained root for
every reqwest destination in the process — so operator guidance must say **roots only** (§ 6). And
per the same doc at `:23`, "Webpki has no support for self-signed certificates", so a self-signed
*leaf* dropped into the bundle will not verify itself — see § 9.

## 3. Decisions

### D1 — trust anchors are webpki ∪ system ∪ optional explicit bundle, all additive

```toml
reqwest = { version = "0.12", default-features = false,
            features = ["rustls-tls", "rustls-tls-native-roots", "json"] }
```

Both root features stay on. Per § 2.2 this unions the three sources rather than choosing between
them, and per § 2.3 keeping `rustls-tls` is what makes an empty system store harmless instead of
silently fatal.

The operator gains three routes. **They are not peers, and the runbook must rank them**, because
route 2's semantics contradict the additive model:

1. **`authn.extra_ca_bundle_path` — recommended.** The only route that reports a boot-time error
   when it is misconfigured (D4), and the only one with an auditable record in config.
2. **Mount a CA into `/etc/ssl/certs`.** Additive with respect to webpki, but § 2.7's verification
   is outstanding.
3. **`SSL_CERT_FILE` / `SSL_CERT_DIR` — last resort.** `rustls-native-certs`'
   `load_native_certs` (`lib.rs:119-124`) short-circuits to the env paths when either is set:
   documented at `:55-57` as "certificates are only loaded from the locations specified via
   environment variables and **not** the platform-native certificate store". So it *replaces* the
   image store. Worse, a missing path yields zero certs with a swallowed error and a non-PEM file
   yields zero certs with no error at all, while reqwest's guard (`client.rs:715`) counts only
   `RootCertStore::add` failures — so a typo is silently ignored.

Rejected — **native roots only, no config field**: § 2.3's silent-empty-store failure, § 2.7's
no-shell constraint, and it leaves AC3 with nothing to test but a process-wide `SSL_CERT_FILE`
mutation that races every other test in the binary.

Rejected — **explicit field only, keeping webpki as the sole base**: mounting a CA into the
container would still do nothing and `SSL_CERT_FILE` would still not apply.

### D2 — the field is named `extra_ca_bundle_path`, and the `extra_` is load-bearing

Per § 2.5 this repo already has two CA-path knobs and both *replace* the trust store, one of them
in the same `AuthnConfig` file. A third field named `ca_bundle_path` or `root_ca_bundle` — the
issue's own suggestion — would carry their name shape with inverted semantics, which is a trap for
the next reader and a plausible production misconfiguration.

`extra_` is the ecosystem's own marker for this exact distinction: `NODE_EXTRA_CA_CERTS` is
additive, while `REQUESTS_CA_BUNDLE` and git's `http.sslCAInfo` replace.

| Field | Semantics | Re-read on rotation? |
| --- | --- | --- |
| `authn.extra_ca_bundle_path` (new) | **adds** to webpki + system | No — boot only, restart required |
| `upstream.openai.extra_ca_bundle_path` (new) | **adds** to webpki + system | No — boot only, restart required |
| `outbox.publisher.root_ca_bundle` | replaces the system store | Yes, per connection attempt (`config.rs:577-578`) |
| `iam.tls.ca_cert_path` (gateway) | replaces / pins | No |

The rotation column matters: an operator reading D2's comparison would otherwise assume parity with
the NATS knob, which genuinely re-reads.

### D3 — one global field per service, not per-issuer

`authn.issuers` is a `Vec`, so `authn.issuers[].extra_ca_bundle_path` was a real option and the
issue floats it. Rejected on YAGNI: additive semantics (D1) already make a mixed public + private
issuer set work, so per-issuer buys only trust *narrowing*, at the cost of a `reqwest::Client` per
issuer, a per-issuer connection pool, a lookup in `fetch()`, and a decision about what an
unconfigured issuer gets — all inside `jwks.rs`, the file that hosts the rotation and single-flight
logic.

The accepted cost: a bundle named here is trusted for *every* configured issuer, including a
publicly-issued one. `HttpJwksFetcher` keeps its single client unchanged.

### D4 — a bad bundle is a hard boot failure, including the zero-certificate case

An unreadable, unparseable, **or certificate-free** bundle aborts boot, matching the gateway's
tonic path (`adapters/iam/client.rs:227`). Failing late instead would present as every token
validation returning `Unavailable` with no indication that a config path is at fault.

Per § 2.8 the zero-certificate case is the one that does *not* come free — `from_pem_bundle`
returns `Ok(vec![])` — so it needs an explicit `is_empty()` rejection. Without it, the single most
likely operator mistake boots green and does nothing, which is the exact defect this field exists
to prevent.

Three distinct failure strings, because they have three distinct fixes:

| Condition | Surfaces at | Message names |
| --- | --- | --- |
| path unreadable | `std::fs::read` | the path and the io error |
| section present, bad base64/DER | `builder.build()` | the config key — the error is otherwise anonymous |
| **zero certificates parsed** | explicit `is_empty()` check | that a DER/key-only/empty file parses as an empty bundle |

This also requires fixing a defect on the way: `HttpJwksFetcher::new` currently maps *every*
client-build failure to `AuthnError::Unavailable` (`jwks.rs:102`), discarding the cause. The
variant becomes `AuthnError::Backend`, already used for a wiring defect at
`adapters/http/mod.rs:686`. Per § 2.3 that same `build()` call can now also fail because the
*platform* store is entirely unparseable, so its message must say so — the operator would otherwise
hunt a config bug that is not there.

### D5 — the invalid combination is made unrepresentable at the adapter seam, not only validated

`accept_invalid_tls` makes the root store irrelevant, so combining it with a bundle is always an
operator mistake.

The first draft enforced this only in `IamConfig::validate()` while citing D8's
"unrepresentable" reasoning — but D8 achieved that with a *type* (the `IamTlsConfig` enum,
gateway `config.rs:53-69`), and `IamConfig::validate()` is called from exactly one production site,
`iam/src/main.rs:62`. `AppState::new` never calls it, so every integration test and any embedder
could construct the combination and get a verification-disabled client with no complaint. The
proposed `new(Duration, bool, Option<&str>)` signature also placed a `bool` and an `Option<&str>`
adjacent and positional — the transposable-argument defect class D8 exists to close.

So the adapter seam takes an enum instead:

```rust
/// How the IdP HTTP client establishes trust. An enum rather than a `bool` + `Option` pair so
/// "verification disabled AND a trust bundle configured" — which is always an operator mistake,
/// since the bundle can never be consulted — is unrepresentable (cf. gateway D8's IamTlsConfig).
pub enum IdpTls<'a> {
    /// TEST-ONLY: `danger_accept_invalid_certs`. See `AuthnConfig::accept_invalid_tls`.
    AcceptInvalid,
    Verify { extra_bundle: Option<&'a str> },
}
```

The two config fields stay as they are — the enum is constructed once, at the composition root.
`IamConfig::validate()` keeps the rule as the *config-file-level* diagnostic that produces § 4.4's
operator-facing message; the type is what makes the state unreachable everywhere else.

AC4 is unaffected: `accept_invalid_tls` on its own keeps working exactly as today, and the existing
boot warning (`adapters/http/mod.rs:665-667`) stays.

### D6 — the gateway gets the field and plumbing-level tests, not a TLS mock upstream

Per § 2.6 the gateway has no HTTPS test infrastructure. Full parity would mean three new
dev-dependencies, a second copy of the chain-minting fixture in a crate with no `support/` module,
and a TLS listener in a suite that has deliberately avoided real listeners.

Instead the gateway gets `tempfile` and unit tests over `OpenAiClient::new` covering a valid PEM
fixture, a missing path, a certificate-free file, and a malformed one. What that proves is the
gateway's own plumbing: the config field reaches `reqwest`, and each failure mode maps to the new
error variant.

**What it does not prove:** that the resulting client completes a handshake against a private-CA
server. `reqwest::Client` exposes no accessor for its trust store, so no gateway test can observe
it — which is why § 5.2's test 6 is named `valid_ca_bundle_builds_the_client` rather than anything
claiming the bundle was *loaded*. That is proven once by IAM's test pair, over the same crate and
builder API.

Splitting the gateway into a follow-up was considered and rejected: the feature flip widens the
gateway's trust store in this same commit regardless, and shipping that with no config surface and
no runbook entry is the worse outcome.

### D7 — the fold is duplicated in both services rather than extracted

The shared part is a handful of lines (`read` → `from_pem_bundle` → `is_empty` check → fold
`add_root_certificate`); the error mapping is the bulk and differs by service (`AuthnError` vs
`OpenAiError`, with different config-key names in the messages). Against that, a new `libs/` crate
reds `repo:affected-smoke` until it is added to the `lockfile->all-lint` expected set in
`ci/affected-graph/run.sh`, and `paigasus-kernel` is disqualified outright — it is wasm-bound and
must stay dependency-light (SMA-448). The two copies cross-reference each other by path.

### D8 — the success path logs, because every other signal in this feature is silent

Native-root loading is silent, `SSL_CERT_FILE` is silent (D1 route 3), and a zero-certificate
bundle would be silent without D4. An operator would have no way to confirm their CA was picked up
short of a successful token validation — reproducing, in a new shape, the diagnosability failure
this issue exists to fix.

Both services emit one `tracing::info!` on successful load, naming the path and the certificate
count. The site and idiom already exist: `adapters/http/mod.rs:665-667` logs the
`accept_invalid_tls` warning at exactly this point.

## 4. Architecture

### 4.1 Files

| File | Change |
| --- | --- |
| `rs/Cargo.toml` | `reqwest` features += `rustls-tls-native-roots` |
| `rs/Cargo.lock` | regenerated (2 edges, 0 packages) |
| `iam/src/config.rs` | `AuthnConfig.extra_ca_bundle_path`; `validate()` D5 rule; `AuthnDefaults` comment |
| `iam/src/adapters/oidc/jwks.rs` | `IdpTls` enum; `HttpJwksFetcher::new` signature + fold + D4 errors + D8 log; unit tests 3/4/4b |
| `iam/src/adapters/http/mod.rs` | construct `IdpTls` at the call site |
| `iam/src/service_info.rs` | `AuthnConfig` literal at `:137` gains the field |
| `iam/tests/support/mod.rs` | `start_mock_idp_private_ca()` fixture; `AuthnConfig` literal at `:307` |
| `iam/tests/keycloak_e2e.rs` | `AuthnConfig` literal at `:199` |
| `iam/tests/authn_private_ca.rs` | new, Docker-free: tests 1 and 2 |
| `iam/iam.toml.example` | document the field under `[authn]` |
| `gateway/src/config.rs` | `OpenAiConfig.extra_ca_bundle_path`; `OpenAiDefaults` comment |
| `gateway/src/adapters/openai/client.rs` | fold + `OpenAiError::CaBundle` + D8 log + tests 6-8b; `OpenAiConfig` literal at `:203` |
| `gateway/src/adapters/http/error.rs` | **`From<OpenAiError>` match arm + registry parity test row** |
| `gateway/src/adapters/http/mod.rs` | `OpenAiConfig` literal at `:233` |
| `gateway/src/service_info.rs` | `OpenAiConfig` literal at `:103` |
| `gateway/tests/{openai_egress,service_info,chat_proxy,metrics}.rs` | `OpenAiConfig` literals (5 sites) |
| `gateway/Cargo.toml` | `tempfile` dev-dependency |
| `gateway/gateway.toml.example` | document the field under `[upstream.openai]` |
| `docs/ops/RUNBOOK-containers.md` | replace the § 5 "not supported" bullet (AC2) |
| `CLAUDE.md` | gotcha: three CA knobs, two replace and one extends |

Neither `AuthnConfig` nor `OpenAiConfig` derives `Default`, so `..Default::default()` is
unavailable: **every** struct literal of either type must gain the field. The 11 sites are listed
above rather than left to discovery, because `keycloak_e2e.rs` sits in a Docker-gated suite an
implementer may not build locally.

**The CLAUDE.md edit must not paste the `moon ci` target list into the new gotcha** — per that
file's own SMA-541 rule, a second copy of the `<!-- ci-targets:begin/end -->` markers or their
content, even inside backticks, reds `repo:affected-smoke`.

### 4.2 IAM config

`AuthnConfig` gains:

```rust
/// Extra trust anchors for the IdP discovery/JWKS fetches, as a path to a PEM bundle.
///
/// **This ADDS to the trust store, it does not replace it.** The client trusts the compiled-in
/// Mozilla roots, the image's own store (`/etc/ssl/certs`), AND every certificate in this
/// bundle. Note this is the OPPOSITE of the sibling `outbox.publisher.root_ca_bundle` and of
/// the gateway's `iam.tls.ca_cert_path`, both of which REPLACE — hence the `extra_` prefix
/// (cf. `NODE_EXTRA_CA_CERTS`).
///
/// ROOTS ONLY. Every certificate here becomes an UNCONSTRAINED trust anchor for every HTTPS
/// call this process makes — rustls performs no `cA` basic-constraints check on an anchor
/// (rustls-webpki `trust_anchor.rs:14-15`). An intermediate placed here is promoted to a root.
///
/// Read ONCE at boot; a rotated bundle needs a restart (unlike `root_ca_bundle`). An
/// unreadable, malformed, or certificate-FREE bundle is a hard boot failure, never a warning.
/// Mutually exclusive with `accept_invalid_tls`, which would make it dead.
#[serde(default)]
pub extra_ca_bundle_path: Option<String>,
```

It is deliberately **absent** from `AuthnDefaults`. That struct feeds figment's defaults layer, and
an `Option` carrying `#[serde(default)]` resolves to `None` without an entry — the same reason
`issuers` is omitted. `AuthnDefaults`' "mirrors `AuthnConfig` minus `issuers`" comment is updated to
name both omissions and say why they differ. `OpenAiDefaults` (`gateway/src/config.rs:159-163`,
documented "Mirrors `OpenAiConfig` field-for-field") gets the identical treatment and comment
update, or its claim goes stale.

### 4.3 The fetcher

```rust
pub fn new(timeout: Duration, tls: IdpTls<'_>) -> Result<Self, AuthnError> {
    let mut builder = reqwest::Client::builder().timeout(timeout);

    match tls {
        IdpTls::AcceptInvalid => builder = builder.danger_accept_invalid_certs(true),
        IdpTls::Verify { extra_bundle: None } => {}
        IdpTls::Verify { extra_bundle: Some(path) } => {
            let pem = std::fs::read(path).map_err(|e| backend(format!(
                "failed to read authn.extra_ca_bundle_path {path:?}: {e}")))?;
            // from_pem_bundle, NOT from_pem: a bundle may legitimately carry MORE THAN ONE
            // ROOT (a cross-signed CA, or two corporate roots mid-rotation) and from_pem
            // reads only the first. It must NOT be read as an invitation to add
            // intermediates — see the `ROOTS ONLY` note on the config field.
            let certs = reqwest::Certificate::from_pem_bundle(&pem).map_err(|e| backend(format!(
                "authn.extra_ca_bundle_path {path:?} is not a valid PEM certificate bundle: {e}")))?;
            // from_pem_bundle returns Ok(vec![]) for any file with no PEM CERTIFICATE section
            // — a DER .crt, a key-only PEM, an empty file. Without this check the most likely
            // operator mistake boots green and silently adds nothing (spec § 2.8).
            if certs.is_empty() {
                return Err(backend(format!(
                    "authn.extra_ca_bundle_path {path:?} contained no PEM certificates — a DER \
                     file, a key-only PEM or an empty file parses as an empty bundle")));
            }
            tracing::info!(path = %path, count = certs.len(),
                "loaded extra IdP trust anchors from authn.extra_ca_bundle_path");
            for cert in certs {
                builder = builder.add_root_certificate(cert);
            }
        }
    }

    let client = builder.build().map_err(|e| backend(format!(
        "failed to build the IdP HTTP client: {e} — this can also mean the platform trust store \
         contains no parseable certificates")))?;
    Ok(Self { client, clock: SystemClock })
}
```

`backend(..)` is shorthand for the idiom this file already uses at `adapters/http/mod.rs:605` —
`AuthnError::Backend(format!("…").into())`, which works because std provides
`From<String> for Box<dyn Error + Send + Sync>`. The implementation may inline it.

### 4.4 The `validate()` rule

```
authn.accept_invalid_tls = true disables certificate verification entirely, so
authn.extra_ca_bundle_path can never take effect — set one or the other, not both.
```

`validate()` also rejects an empty-string path: `IAM_AUTHN__EXTRA_CA_BUNDLE_PATH=` deserializes to
`Some("")`, not `None`, which would otherwise reach `std::fs::read("")` and fail with a confusing
`""` in the message.

### 4.5 Gateway

`OpenAiConfig` gains the same field with the same doc shape, and `OpenAiClient::new` performs the
same fold including the `is_empty()` check and the D8 log. `OpenAiError` gains one variant, because
`std::fs::read` yields `std::io::Error` while today's `Build` wraps only `reqwest::Error`:

```rust
/// The configured CA bundle could not be read, parsed, or contained no certificates — a
/// boot-time fault like `Build`, never a request-time one.
#[error("failed to load upstream.openai.extra_ca_bundle_path {path:?}")]
CaBundle { path: String, #[source] source: Box<dyn std::error::Error + Send + Sync> },
```

**This is a compile-breaking edit, not a formality.** `impl From<OpenAiError> for GatewayError`
(`adapters/http/error.rs:85-92`) is an exhaustive `match` with no wildcard, and
`rs/Cargo.toml:224-225` sets `warnings = "deny"`, so a new variant fails the build until it is
handled. `CaBundle` maps to **`GatewayError::UpstreamUnavailable`**, joining `Build` under the same
502 "upstream unreachable/misbuilt" rationale. That reuses an existing canonical registry code, so
no registry change is needed and `:error-code-single-site` stays clean. The exhaustive parity test
at `error.rs:209-212` gains a fifth row.

The gateway's `validate()` needs no D5 rule — it has no `accept_invalid_tls` equivalent — but does
get the empty-string check.

## 5. Testing

### 5.1 The fixture

`tests/support/mod.rs` gains, beside the existing `start_mock_idp`:

```rust
pub async fn start_mock_idp_private_ca() -> (MockIdp, String /* CA cert PEM */)
```

It mints a genuine two-level chain: a self-signed CA with
`IsCa::Ca(BasicConstraints::Unconstrained)`, then a leaf signed by it. The server is configured
with **the leaf alone**, not the chain — correct TLS practice for a root the client holds, and it
makes the test strict: the client cannot learn the CA from the handshake, so it can only succeed if
the bundle genuinely loaded.

Three details are load-bearing, because per § 2.6 the existing fixture has **never** been verified
against a real trust path and `CertificateParams::default()` leaves the DN empty
(`certificate.rs:104`):

- the CA and the leaf get **distinct, non-empty** CNs — otherwise both carry an empty subject DN;
- the leaf carries both `localhost` and `127.0.0.1` SANs (`CertificateParams::new` parses the
  latter into `SanType::IpAddress`, `certificate.rs:122-137`), since the mock binds an ephemeral
  `127.0.0.1` port;
- `signed_by` **consumes** `self` (`certificate.rs:150-155`) — it is not a `&self` method.

### 5.2 Assertions

| # | Test | Location | Asserts |
| --- | --- | --- | --- |
| 1 | `private_ca_issuer_validates_with_extra_ca_bundle` | `tests/authn_private_ca.rs` | bundle set, verification ON → `authenticate()` returns `Ok` |
| 2 | `private_ca_issuer_fails_without_extra_ca_bundle` | `tests/authn_private_ca.rs` | byte-identical, bundle `None` → `AuthnError::Unavailable` |
| 3 | `missing_bundle_path_is_a_boot_error` | `jwks.rs` unit | nonexistent path → `Backend`, not `Unavailable` |
| 4 | `certificate_free_bundle_is_a_boot_error` | `jwks.rs` unit | file with no `-----BEGIN CERTIFICATE-----` → `Backend` |
| 4b | `undecodable_bundle_is_a_boot_error` | `jwks.rs` unit | well-framed section, bad base64 → `Backend` |
| 5 | `accept_invalid_tls_with_bundle_is_rejected` | `config.rs` unit | `validate()` returns `Err` |
| 6 | `valid_ca_bundle_builds_the_client` | gateway `client.rs` unit | valid PEM fixture → `Ok` |
| 7 | `missing_ca_bundle_path_is_a_build_error` | gateway `client.rs` unit | → `CaBundle` |
| 8 | `certificate_free_ca_bundle_is_a_build_error` | gateway `client.rs` unit | → `CaBundle` |
| 8b | `undecodable_ca_bundle_is_a_build_error` | gateway `client.rs` unit | → `CaBundle` |

**Tests 4 and 4b are separate because § 2.8 shows they surface at different call sites** — the
certificate-free case only fails because of D4's explicit `is_empty()` check, while the bad-base64
case fails inside `builder.build()`. A single "malformed" test would have missed the first entirely,
which is the BLOCKER the adversarial review caught.

**Test 2 is load-bearing and must not be dropped as redundant.** Test 1 alone would pass vacuously
if anything else in the trust path happened to accept that certificate. Because 1 and 2 differ in
exactly one field and talk to the same mock IdP, the failure is attributable to the trust anchor.

**Test 6 deliberately claims only that the client builds.** `reqwest::Client` exposes no trust-store
accessor, so a test named `..._is_loaded_into_the_client` would assert something it cannot observe
(D6).

### 5.3 The test seam — and why these are genuinely Docker-free

The first draft claimed tests 1 and 2 were Docker-free without showing it. They are not, at the
seam the existing suites use: every `start_mock_idp()` call site first calls
`support::start_migrated_postgres()` (e.g. `tests/http_authn.rs:23-26,45-49`), because
`AppState::new(db, &cfg)` requires a `DatabaseConnection`.

Tests 1 and 2 therefore bind one level lower, at the authenticator rather than the router:

```rust
let fetcher = HttpJwksFetcher::new(timeout, IdpTls::Verify { extra_bundle: Some(path) })?;
let provider = JwksProvider::new(fetcher, InMemoryJwksCache::new(), SystemClock, ttl, cooldown);
let authn  = OidcAuthenticator::new(issuers, provider, leeway, max_token_bytes)?;
assert!(authn.authenticate(&idp.bearer(..)).await.is_ok());
```

`OidcAuthenticator::authenticate` (`adapters/oidc/validator.rs:170-172`, the `Authenticator` port at
`paigasus-iam-core/src/ports.rs:190-193`) touches no database. This seam is genuinely Docker-free,
so the suite needs no participation in `tests/support/docker.rs`'s policy and adds no row to the
Docker-gated count.

It also covers *less* than the router seam, deliberately: it proves the TLS trust path and nothing
about identity resolution, which `http_authn.rs` already covers. Note it cannot reuse
`support::test_config_with`, which hardcodes `accept_invalid_tls: true` (`tests/support/mod.rs:313`)
— the whole point is to run with verification ON.

**One implementation-time verification, not a test:** § 2.7's open question about whether the
chiseled `ca-certificates_data` slice ships a path `openssl_probe` actually finds. Check it against
a built image (`ci/images/run.sh build`) before writing § 6's route-1 text; if it does not, route 1
is a no-op and the runbook must say so.

## 6. Documentation

`docs/ops/RUNBOOK-containers.md` § 5, lines 121-123, currently reads "**A private-CA identity
provider is not supported.**" It is replaced (AC2) by D1's three routes **in D1's ranked order**,
not as equivalents — with `extra_ca_bundle_path` first as the only one that fails loudly when
misconfigured, and `SSL_CERT_FILE` last carrying both of its caveats inline ("replaces the image's
own store rather than adding to it; a wrong path is silently ignored").

Three further points the operator needs:

- **Roots only** (§ 2.9) — anything in the bundle becomes an unconstrained anchor for every
  outbound HTTPS call the process makes, so an intermediate must not be pasted in.
- There is no shell, so `update-ca-certificates` is unavailable (§ 2.7) — which is why the config
  field exists.
- Unlike `outbox.publisher.root_ca_bundle`, none of the routes costs you the public roots; and
  unlike it, a rotated bundle needs a restart (D2's table).

`iam.toml.example` and `gateway.toml.example` document the field with the same `extra_` vs
`root_ca_bundle` contrast, since an operator reading one will assume the other.

`CLAUDE.md` gains a gotcha recording the three CA-path knobs, which of them replace vs extend, and
which convention a fourth should join — subject to § 4.1's marker warning.

`docs/superpowers/specs/2026-08-19-sma-500-...-design.md` is **not** edited. Its § 2.2 row and
deferred item 3 become false, but dated design docs are records of what was true when written; this
spec's header states the supersession.

## 7. CI gates

The `rs/Cargo.toml` + `rs/Cargo.lock` touch schedules a wide Rust graph, so the full CI line runs
before push, not per-project tasks.

| Gate | Expectation | Basis |
| --- | --- | --- |
| `:deny` | clean | § 2.4 — zero new packages, and `rs/deny.toml:2` `all-features = true` already audited `rustls-native-certs` |
| `:machete` | clean | `tempfile` is consumed in the same commit, so no `ignored` allowlist entry |
| `:affected-smoke` | clean | no new crate and no new in-tree dep, so `ci/affected-graph/run.sh`'s expected sets and `cargo_moon_parity.py` are untouched |
| `:error-code-single-site` | clean | `CaBundle` reuses `UpstreamUnavailable`'s existing registry code (§ 4.5); neither `jwks.rs` nor `openai/client.rs` is in `check.py`'s `MANIFEST` |
| `:redis-connect-single-site` | **fires**, expect clean | inputs cover `paigasus-iam/src/**` + `tests/**` (`moon.yml:271`); the new code names no gated redis constructor |
| `:iam-docker-policy-single-site` | **fires**, expect clean | inputs cover `paigasus-iam/tests/**` (`moon.yml:348`); the new suite must read no `CI` env var and import no `AsyncRunner` |
| `:input-liveness` | clean | no file moves or renames |
| `:observability-drift` | clean | D8 adds a log line, not a metric |
| `:release-parity*`, `:publish-metadata` | clean | no version or manifest changes |

Every row is an expectation to *verify by running*, not to assume.

## 8. Risks

1. **Enabling native roots widens the trust store for every `reqwest` call in both services,
   permanently and by default.** An operator with a publicly-issued IdP now also trusts whatever
   their image's CA bundle contains, including anything a base-image update adds. The accepted cost
   of D1, and the standard posture across the ecosystem — but a real trust-surface expansion.
2. **No pinning story.** D1 can only widen trust, never narrow it. An operator wanting "trust my
   corporate CA *and nothing else*" is not served — `tls_built_in_webpki_certs(false)` /
   `tls_built_in_native_certs(false)` make it reachable later, but no field exposes them. There is
   no pinning today either, so this is unaddressed rather than regressed.
3. **D3's global scope means a bundle is trusted for every issuer.** A mixed public + private issuer
   set works, but the private CA could vouch for the public issuer's hostname.
4. **The gateway's handshake path is proven only by inference** (D6) — via IAM's test pair over the
   same crate and builder API, not by a gateway-side TLS test.
5. **A wholly unparseable platform trust store now fails BOTH services at boot** (§ 2.3), triggered
   by the host or image rather than by Paigasus config, with no field to disable native roots.
   D4's error string names the possibility; there is no runtime escape hatch short of a rebuild.
6. **Every certificate in a bundle becomes an unconstrained anchor for every outbound HTTPS call**
   (§ 2.9) — including, in the gateway, the OpenAI/vLLM upstream. Mitigated by documentation
   (§ 6 "roots only"), not by code.
7. **`rustls-native-certs` reads the OS store at client-build time**, which on developer macOS means
   the system keychain. Slower than a compiled-in slice and platform-dependent, though bounded to
   client construction rather than per-request.
8. **`jwks.rs:142` requires only that `discovery.jwks_uri` starts with `https://`** — no same-origin
   check against the issuer. Pre-existing, but this change widens what "https" can now reach.

## 9. Out of scope

- **A self-signed IdP leaf certificate.** Per § 2.9, webpki has no support for self-signed
  certificates, so dropping a self-signed *leaf* into `extra_ca_bundle_path` will not verify.
  `accept_invalid_tls` remains the only route for that case. AC1 asks specifically for a
  private-**CA** issuer, which is satisfied — but this limit is real and belongs in the runbook.
- `sqlx`/`sea-orm` and `redis` trust stores (§ 2.1) — still webpki-pinned. Operator-controlled
  infrastructure links, not third-party issuers, and not named by SMA-558's acceptance criteria.
- Certificate *pinning* for OIDC issuers (risk 2), and per-issuer trust anchors (D3, risk 3).
- mTLS to the IdP.
- **Migrating the ~50 existing mock-IdP suites off `accept_invalid_tls` onto the new field.** This
  was raised in review and is genuinely attractive — it would turn the whole suite into continuous
  proof of AC1 and shrink `accept_invalid_tls`'s blast radius to nothing. Deferred deliberately: it
  changes the setup of every authentication-touching suite including the Docker-gated
  `keycloak_e2e`, whose container certificate is not under our control, and that risk does not
  belong in the same change as the mechanism itself. Worth its own issue.

## 10. Acceptance criteria mapping

| AC | Where |
| --- | --- |
| 1. Trust a private-CA issuer without disabling verification | D1's three routes; proven by test 1 with verification ON. Limit: self-signed leaves (§ 9) |
| 2. Documented in `RUNBOOK-containers.md`, replacing "not supported" | § 6, ranked per D1 |
| 3. A test covers the trusted-private-CA path | § 5, tests 1 + 2 at the § 5.3 Docker-free seam |
| 4. `accept_invalid_tls` remains available, no longer the only route | Unchanged behaviour; D5 only forbids combining it with the new field |
