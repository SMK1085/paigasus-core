# SMA-570 — CA-bundle doc scope and boot diagnosability

Two follow-ups deferred from SMA-558 (PR 151, shipped to `main` in `e357f62`). Neither is a
behavioural defect. Item 1 is an over-broad doc claim; item 2 is a diagnosability gap.

Revision 2, after adversarial review. Changes from revision 1 are listed in §7.

## 1. Item 1 — the blast-radius claim is wider than the truth

### The defect

**Five** sites say a certificate in `extra_ca_bundle_path` becomes an unconstrained trust anchor
for every HTTPS call the **process** makes:

| # | Site | Wording today |
|---|------|---------------|
| 1 | `rs/crates/services/paigasus-iam/src/config.rs:136-138` | "an UNCONSTRAINED trust anchor for every HTTPS call this process makes" |
| 2 | `rs/crates/services/paigasus-gateway/src/config.rs:116-117` | "an unconstrained trust anchor for every HTTPS call this process makes" |
| 3 | `rs/crates/services/paigasus-iam/src/adapters/oidc/jwks.rs:135-138` | "promoted to a root for every HTTPS call this process makes" |
| 4 | `docs/ops/RUNBOOK-containers.md:257-258` | "an unconstrained trust anchor for every outbound HTTPS call the process makes" |
| 5 | `rs/crates/services/paigasus-iam/iam.toml.example:30-31` | "every certificate here becomes an unconstrained trust anchor for every outbound HTTPS call" |

Site 5 was missed in revision 1 and is the **most operator-facing of the five** — it is the config
template an operator copies. The repo already treats this file as canonical for this knob: both
`tests/support/mod.rs:382-386` and `tests/authn_private_ca.rs:64-66` enumerate it when listing the
sites of SMA-558's earlier self-signed-leaf correction.

The anchors are added to **one `reqwest::ClientBuilder`** each — IAM's `HttpJwksFetcher` and the
gateway's `OpenAiClient`. The `tonic` (gateway → IAM), `async-nats` and `redis` links each build
their own TLS configuration and never consult these anchors.

The **unconstrained-anchor half is correct and stays.** rustls applies no `cA` basic-constraints
check to a trust anchor, which is exactly why the bundle must hold roots only. Only the *scope*
is wrong, and it is wrong in the cautious direction — which is why it did not block SMA-558.

### The fix

Narrow all five to **client** scope, using this phrasing:

> for every request this client makes, to any host it reaches

The trailing clause is load-bearing. Narrowing to "for the IdP connection" would **under**-claim:
`HttpJwksFetcher` dials whatever `jwks_uri` the discovery document names — checked only for an
`https://` prefix (`jwks.rs:209-211`) — and uses reqwest's default redirect policy (`jwks.rs:118-119`
declines a custom one on purpose). The anchor is therefore good for any host that client reaches,
which is precisely the property that makes roots-only load-bearing.

A sixth edit, **additive rather than corrective**: `gateway.toml.example:53-57` says `ROOTS ONLY.`
with no scope claim at all. It gains the same client-scoped sentence, for symmetry with the
corrected `iam.toml.example`.

`CLAUDE.md:218` already words this correctly and is **not** touched.

### Non-sites, checked and deliberately excluded

- `jwks.rs:97-99` (`IdpTls::AcceptInvalid`) — already client-scoped ("for every fetch the client
  makes"), correct as written.
- `openai/client.rs:156-160` — defers to the config field's note, makes no scope claim.
- `docs/superpowers/specs/2026-08-20-sma-558-*.md` and `docs/superpowers/plans/2026-08-20-sma-558-*.md`
  carry the same claim in nine places. **Historical design records are not amended** — they record
  what was decided at the time. See §6.

## 2. Item 2 — an invalid-DER bundle loses the config key in its error

### The defect, measured

The bundle loader has three failure paths. Two name the config key; the third does not.

| Failure | Where it surfaces | Names the key? |
|---------|-------------------|----------------|
| Unreadable path | `std::fs::read` | yes |
| Zero certificates parsed | explicit `is_empty()` guard | yes |
| Valid base64, invalid DER | later, inside `builder.build()` | **no** |

Measured **in this worktree, as a unit test inside `paigasus-iam`** — so under the workspace
feature set, not a scratch manifest — with a `-----BEGIN CERTIFICATE-----` section whose body is
`AAAAAAAA` (valid base64, six zero bytes, not valid DER):

```
from_pem_bundle ACCEPTED it, 1 cert(s)
builder.build() FAILED, Display: builder error
Debug: reqwest::Error { kind: Builder, source: InvalidCertificate(BadEncoding) }
  source[1]: invalid peer certificate: BadEncoding
```

The premise holds: the bad certificate passes `from_pem_bundle` and dies at `build()`. The
identifying token `BadEncoding` is reachable only through `Debug` or the `source()` chain —
reqwest's build-error `Display` is the bare string `"builder error"`.

What is missing from the operator's message is therefore **the config key**, not the cause. Both
`main.rs` files print `eprintln!("Error: {error:?}")` on an `anyhow::Error`
(`paigasus-iam/src/main.rs:29-30`, `paigasus-gateway/src/main.rs:29-30`), and anyhow's `Debug`
already walks `source()`, so `BadEncoding` is visible today. What the operator cannot learn is
that their configured bundle is what produced it — they are instead pointed at the platform trust
store, which is not where the problem is.

### Facts about reqwest that constrain the design

Read from `reqwest-0.12.28/src/async_impl/client.rs`, verified in the vendored source:

- **User roots are added first, and the first bad one short-circuits the whole build**
  (`:687-690`: `for cert in config.root_certs { cert.add_to_rustls(&mut root_cert_store)?; }`).
- **The native store block runs after** (`:697-732`) and errors **only** when
  `valid_count == 0 && invalid_count > 0` (`:715`). An **absent or empty** store yields zero of
  each and builds fine. `rs/Cargo.toml:91-94` and the CLAUDE.md CA-bundle entry already record
  this.

### D1 — attribution by control build

When a bundle is configured and `build()` fails, retry a **control** build with the same options
but no added anchors.

| Case | Inference | Message |
|------|-----------|---------|
| No bundle configured | nothing to attribute; control never runs | today's platform-store wording |
| Bundle, control **succeeds** | the two builds differ only in the added anchors, so the anchors caused this failure | names the config key outright |
| Bundle, control **fails** | the platform store is broken. Because user roots are added *first* and short-circuit, the real build never reached the native block — so the bundle may **also** be invalid | names **both**, store first |

The third row is the correction that revision 1 got wrong. Revision 1 emitted today's unchanged
platform-store message there, which sends the operator to fix the store and then fail boot again
on the still-invalid bundle.

Two wording corrections carried into the code comments, both of which revision 1 stated wrongly:

- Not "the control build proves the platform store is healthy" — an absent or empty store builds
  fine (`:715`). The sound inference is narrower: **the control build succeeded, so the platform
  store did not cause *this* failure; the added anchors did.**
- Not "the two builds differ in exactly one input" — the real IAM builder sets `.timeout()`
  (`jwks.rs:125`) and the gateway's sets `.connect_timeout().read_timeout()`
  (`openai/client.rs:150`), which a bare control builder would drop. D6 makes the one-input claim
  true by construction instead of by inspection.

Rejected alternatives:

- **Hedged message** (what the issue's Fix section specifies): name the bundle path alongside the
  platform-store hint without determining which is at fault. Smaller, but leaves the operator two
  suspects — it relabels the symptom instead of diagnosing it.
- **Per-certificate probe**: name *which* entry is malformed. One client build per certificate,
  each reloading the native root store, for a bundle that is realistically one or two roots.

The control build runs **only on the already-fatal failure path**, and only when a bundle is
configured. Both constructors are boot-only (`paigasus-iam/src/adapters/http/mod.rs:737` via
`AppState::new`; `paigasus-gateway/src/main.rs:80`), so its cost is irrelevant.

### D2 — decide with a pure function, inject the probe

Rendering and deciding are split. The decision is a pure function over an injected probe:

```rust
/// The three REACHABLE outcomes. A bundle-less build can never be attributed to a bundle,
/// so that combination is unrepresentable rather than merely untested.
enum Attribution<'a> {
    NoBundle,
    Bundle { path: &'a str },
    BundleAndStore { path: &'a str },
}

/// `control_build_ok` is called ONLY when `bundle` is `Some` — a bundle-less failure has
/// nothing to attribute, and probing would cost a second `load_native_certs()` for nothing.
fn attribute_build_failure(bundle: Option<&str>, control_build_ok: impl FnOnce() -> bool) -> Attribution<'_>
```

Passing the probe as `impl FnOnce() -> bool` gives three things at once: dependency injection, so
both failure arms are reachable in tests without touching the environment; laziness, so the
no-bundle arm provably never probes (a test asserts the closure was not called); and a pure
decision that needs no host state.

This replaces revision 1's `describe_build_failure(...) -> String`, which was unusable: the
gateway's no-bundle message lives in a fixed `#[error(...)]` attribute on `OpenAiError::Build`
(`openai/client.rs:90`) that no returned `String` can reach. Revision 1 also claimed its
four-quadrant unit tests verified AC4 for both services; for the gateway they would have verified
nothing, because that message is not produced by the function.

Revision 1 further asserted "a broken platform trust store is not something a test can force".
That is **false** — `rustls-native-certs` honours `SSL_CERT_FILE`/`SSL_CERT_DIR` in preference to
the platform store, so a file holding one `AAAAAAAA` certificate makes a bare build fail with
`zero valid certificates found in native root store`. Injection is still preferred over that: it
needs no environment mutation (`std::env::set_var` is `unsafe` in edition 2024) and no assumption
about test-process isolation.

### D3 — reuse IAM's existing chain renderer; add one to the gateway

`paigasus-iam/src/adapters/events/relay.rs:57-72` already defines
`fn describe_error(err: &(dyn Error + 'static)) -> String`, rendering the `source()` chain as
`"outer: middle: inner"`, with a doc comment making the same argument about a `Backend` variant
whose `Display` is a static string. It is promoted to `pub(crate)` in
`paigasus-iam/src/adapters/mod.rs` and reused; `relay.rs` keeps its single call site.

The two copies in IAM's integration tests (`tests/api_key_cache_connection.rs:61`,
`tests/nats_publisher.rs:276`) stay — separate test binaries cannot see a `pub(crate)` item.

The gateway gets its own copy. SMA-558 D7 keeps the two services' bundle handling duplicated
rather than extracted, and there is no shared crate that would fit.

### D4 — the gateway needs no new error variant, and `error.rs` is untouched

`OpenAiError` already carries `CaBundle { path, source }`, whose `Display` names
`upstream.openai.extra_ca_bundle_path`, and `adapters/http/error.rs:110` already maps it to
`GatewayError::UpstreamUnavailable` alongside `Build`. Both bundle-attributed arms return
`CaBundle`; the no-bundle arm returns `Build` unchanged.

So `adapters/http/error.rs` is not touched, `repo:http-extractor-envelope` is not scheduled, and
`GatewayError`'s `EnumIter` / `retryable()` / `parts()` / registry-membership test are all
untouched.

To carry the explanation *and* keep the typed error, the definitive arm boxes a small local error
that holds the `reqwest::Error` as its own `source`:

```rust
#[derive(Debug, thiserror::Error)]
#[error("contains a structurally invalid certificate: it decodes as base64 but is not valid DER. \
         A control client built without it succeeded, so the platform trust store is not the cause")]
struct InvalidBundleCertificate {
    #[source]
    source: reqwest::Error,
}
```

anyhow then renders the full picture and the `reqwest::Error` stays downcastable. Revision 1
proposed boxing a pre-rendered `String`, which discards the typed error for no gain.

### D5 — the `accept_invalid_tls` path is unaffected

`IdpTls::AcceptInvalid` can never carry a bundle: `IamConfig::validate` rejects the pair, and the
enum makes the combination unrepresentable. That arm always yields `Attribution::NoBundle`.

Note `build()` constructs the root store unconditionally, before the `danger_accept_invalid_certs`
branch at `:780` — so the reasoning holds for the right reason, not by accident.

### D6 — a shared base builder, so the control differs by exactly the anchors

Each service factors its builder construction into a function:

```rust
fn base_builder(/* the service's timeouts */) -> reqwest::ClientBuilder
```

The real client is `base_builder(..)` plus anchors; the control is `base_builder(..)` alone.
`ClientBuilder` is not `Clone`, so this must be a function, not a shared value. The "differs only
in the added anchors" claim is then true **by construction**, and stays true when a future option
is added to either client.

### D7 — log the attribution

Both success paths log (`jwks.rs:152-156`, `openai/client.rs:169`); neither failure path does.
`paigasus_logging::init` runs before both constructions, so each attributed failure emits a
`tracing::error!` carrying `path` and the attribution before the error is returned. This is what a
log aggregator indexes; the returned error is what the operator sees on stderr.

### D8 — literal message strings

**IAM** (`AuthnError::Backend`, whose own `Display` is the static `"backend error"`, so the whole
message must live in the boxed string). `{chain}` is `describe_error(&e)`.

- `NoBundle`:
  `failed to build the IdP HTTP client: {chain} — this can also mean the platform trust store contains no parseable certificates`
- `Bundle`:
  `authn.extra_ca_bundle_path {path:?} contains a structurally invalid certificate: it decodes as base64 but is not valid DER ({chain}). A control client built without it succeeded, so the platform trust store is not the cause.`
- `BundleAndStore`:
  `failed to build the IdP HTTP client: {chain}. A control client built WITHOUT authn.extra_ca_bundle_path {path:?} also failed, so the platform trust store contains no parseable certificates — fix that first, then re-check the bundle, which may also be invalid.`

**Gateway**:

- `NoBundle`: `OpenAiError::Build(e)` — the `#[error]` attribute at `openai/client.rs:90` is
  **byte-unchanged**.
- `Bundle`: `OpenAiError::CaBundle { path, source: Box::new(InvalidBundleCertificate { source: e }) }`
- `BundleAndStore`: `OpenAiError::CaBundle { path, source: <boxed string> }` reading
  `the platform trust store also contains no parseable certificates, which is the more likely cause — fix that first, then re-verify this bundle ({chain})`

**Constraint:** none of these strings may quote an error-registry code spelling
(`upstream-unavailable`, `internal`, …). `ci/error-registry/check.py` scans every
`rs/crates/**/src/**/*.rs` and neither file has a `MANIFEST` row, so a quoted code would red
`repo:error-code-single-site`.

### D9 — AC4 is met in substance, with one stated deviation

AC4 says "the no-bundle platform-trust-store message is unchanged".

- **Gateway: byte-exact.** The `#[error]` attribute is untouched.
- **IAM: the sentence is unchanged; the interpolated cause changes.** `{e}` rendered as the
  useless `"builder error"`; it now renders the chain (`"builder error: invalid peer certificate:
  BadEncoding"`). This is a deliberate consequence of D3, which the issue's ACs predate.

Flagged rather than buried: if byte-exactness is required for IAM too, D3 must be dropped from
the no-bundle arm.

## 3. Acceptance criteria

| AC | Covered by |
|----|-----------|
| 1. The sites scope the claim to the client, keeping the unconstrained-anchor / roots-only warning | Item 1 — **five** corrective edits plus one additive, reviewed by diff |
| 2. A valid-base64/invalid-DER bundle produces a boot error naming `authn.extra_ca_bundle_path` / `upstream.openai.extra_ca_bundle_path` | D1 `Bundle` arm; D8 strings |
| 3. A test covers that case in both services, asserting the message names the config key — distinct from the existing base64 tests | New test per service with an `AAAAAAAA` body. **Verified distinct**: the existing `!!!not base64!!!` fixtures fail in `read_pem_certs` → `"invalid certificate encoding"` (`reqwest-0.12.28/src/tls.rs:231-238`); the new one fails in `RootCertStore::add` → `BadEncoding` at build time |
| 4. The no-bundle platform-trust-store message is unchanged | D9 — byte-exact for the gateway, sentence-exact with a stated deviation for IAM |

## 4. Testing

Per service:

- `attribute_build_failure` unit tests over all three reachable outcomes, plus one asserting the
  probe closure is **not** called when no bundle is configured.
- A test per arm driving the real constructor with an injected probe: `Bundle` (probe returns
  true) and `BundleAndStore` (probe returns false), both with an `AAAAAAAA` certificate body,
  asserting the message names the config key (AC3) and, for `BundleAndStore`, that it names the
  store first.
- A byte-exact assertion on the `NoBundle` message (AC4).
- The existing `undecodable_*` tests stay unchanged — they cover the base64 path, which fails
  earlier and is already correctly attributed.

No test mutates `SSL_CERT_FILE`/`SSL_CERT_DIR` or depends on host trust-store state.

## 5. CI gates

Verified: **no registry work is needed.** No `repo:*` gate keys on `RUNBOOK-containers.md`,
`oidc/jwks.rs` or `openai/client.rs` specifically. `repo:redis-connect-single-site` and
`repo:error-code-single-site` are *scheduled* by the `src/` edits but cannot red on this change,
subject to D8's no-registry-codes constraint. `repo:http-extractor-envelope` keys on
`rs/crates/services/*/src/adapters/http/**/*.rs` and is not scheduled, because D4 leaves
`error.rs` untouched. No `ci.yml` `T=(…)`, `SELF_SCHEDULED_GATES`, `SELF_TASK_EXPECTED_GLOBS` or
`T_AFFECTED_SMOKE_REQUIRED_INPUTS` changes.

## 6. Out of scope

- Extracting the two duplicated bundle loaders into a shared crate — SMA-558 D7 decided against it.
- `CLAUDE.md:218`, already correct.
- The nine copies of the process-scoped claim in SMA-558's own spec and plan documents. Historical
  design records state what was decided at the time and are not retro-edited.
- The four CA-bundle knobs' add-vs-replace semantics.

## 7. Changes from revision 1

Folded in after adversarial review: the fifth site (`iam.toml.example`) and the additive
`gateway.toml.example` edit; the `BundleAndStore` arm, which revision 1 mis-attributed; corrected
soundness wording for both control-build inferences; `Attribution` enum replacing a `String`-returning
function that the gateway could not use; injected probe replacing an environment-dependent test
strategy; reuse of `describe_error`; the typed `InvalidBundleCertificate` source; `base_builder`;
failure-path logging; literal message strings; the AC4 deviation stated explicitly; and the
registry-code and historical-document scope notes.

## 8. Risks

- **The control build could itself fail for an unrelated future reason.** Consequence is a fall
  back to the `BundleAndStore` message, which names both causes — never a wrong single accusation.
- **The `AAAAAAAA` fixture depends on rustls continuing to reject that byte sequence as DER.** If
  a future rustls accepted it, the new tests fail loudly rather than passing silently.
- **The measurement is feature-dependent.** `AAAAAAAA` survives `from_pem_bundle` only because
  `default-tls` is off: `rs/Cargo.toml:98` pins `default-features = false` with
  `["rustls-tls", "rustls-tls-native-roots", "json"]`, so `Certificate::from_der` stores the bytes
  unparsed (`reqwest-0.12.28/src/tls.rs:142-149`). If `default-tls` were ever unified in,
  `from_der` would parse eagerly via native-tls and the new tests would silently collapse onto the
  *existing* base64 path — passing for the wrong reason.
- **The user-roots-first ordering is what makes any attribution scheme here work.** If reqwest
  ever added native roots first, the `BundleAndStore` arm's reasoning would need re-deriving.
