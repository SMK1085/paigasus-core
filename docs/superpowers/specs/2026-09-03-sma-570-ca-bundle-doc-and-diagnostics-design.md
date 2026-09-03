# SMA-570 — CA-bundle doc scope and boot diagnosability

Two follow-ups deferred from SMA-558 (PR 151, shipped to `main` in `e357f62`). Neither is a
behavioural defect. Item 1 is an over-broad doc claim; item 2 is a diagnosability gap.

## 1. Item 1 — the blast-radius claim is wider than the truth

### The defect

Four sites say a certificate in `extra_ca_bundle_path` becomes an unconstrained trust anchor
for **every HTTPS call the process makes**:

| # | Site | Wording |
|---|------|---------|
| 1 | `rs/crates/services/paigasus-iam/src/config.rs:136-138` | "an UNCONSTRAINED trust anchor for every HTTPS call this process makes" |
| 2 | `rs/crates/services/paigasus-gateway/src/config.rs:116-117` | "an unconstrained trust anchor for every HTTPS call this process makes" |
| 3 | `rs/crates/services/paigasus-iam/src/adapters/oidc/jwks.rs:135-138` | "promoted to a root for every HTTPS call this process makes" |
| 4 | `docs/ops/RUNBOOK-containers.md:257-258` | "an unconstrained trust anchor for every outbound HTTPS call the process makes" |

The anchors are added to **one `reqwest::ClientBuilder`** each — IAM's `HttpJwksFetcher` and the
gateway's `OpenAiClient`. The `tonic` (gateway → IAM), `async-nats` and `redis` links each build
their own TLS configuration and never consult these anchors.

The **unconstrained-anchor half is correct and stays.** rustls applies no `cA` basic-constraints
check to a trust anchor, which is exactly why the bundle must hold roots only. Only the *scope*
is wrong.

The error is in the cautious direction — it overstates how far trust widens — which is why it did
not block SMA-558's merge.

### The fix

Narrow all four to name the client rather than the process. Keep every roots-only warning intact.

`CLAUDE.md:218` already words this correctly and is **not** touched.

### Non-sites, checked and deliberately excluded

- `jwks.rs:97-99` (`IdpTls::AcceptInvalid`) says verification is disabled "for every fetch the
  client makes" — already client-scoped, correct as written.
- `openai/client.rs:155-158` defers to the config field's note and makes no scope claim of its own.

## 2. Item 2 — an invalid-DER bundle loses the config key in its error

### The defect, measured

The bundle loader has three failure paths. Two name the config key; the third does not.

| Failure | Where it surfaces | Names the key? |
|---------|-------------------|----------------|
| Unreadable path | `std::fs::read` | yes |
| Zero certificates parsed | explicit `is_empty()` guard | yes |
| Valid base64, invalid DER | later, inside `builder.build()` | **no** |

Measured on reqwest 0.12.28 with a `-----BEGIN CERTIFICATE-----` section whose body is
`AAAAAAAA` (valid base64, six zero bytes, not valid DER):

```
from_pem_bundle ACCEPTED it, 1 cert(s)
builder.build() FAILED, Display: builder error
Debug: reqwest::Error { kind: Builder, source: InvalidCertificate(BadEncoding) }
  source[1]: invalid peer certificate: BadEncoding
```

Two findings, the second of which the issue did not anticipate:

1. The premise holds — the bad certificate passes `from_pem_bundle` and dies at `build()`.
2. **reqwest's build-error `Display` is the string `"builder error"`.** IAM's current message
   interpolates `{e}`, so it contributes nothing. The operator reads
   `failed to build the IdP HTTP client: builder error — this can also mean the platform trust
   store contains no parseable certificates`, and is sent to inspect the platform trust store,
   which is not where the problem is. The one identifying token, `BadEncoding`, is reachable only
   through `Debug` or the source chain.

### D1 — attribution by control build

On a `build()` failure, retry a **bare** client build with no added anchors.

- Control **succeeds** → the platform store is provably healthy. The only difference between the
  two builds is the added anchors, so the bundle is definitively at fault. Emit a message naming
  the config key outright.
- Control **fails** → the platform store is the fault. Emit today's message, unchanged.

This is sound rather than heuristic: the two builds differ in exactly one input, so a
success/failure split across them isolates that input. It needs no string-matching on rustls's
wording, which is an unstable private detail of a transitive dependency.

Rejected alternatives:

- **Hedged message** (what the issue's Fix section specifies): name the bundle path alongside the
  platform-store hint without determining which is at fault. Smaller, but leaves the operator two
  suspects — it relabels the symptom instead of diagnosing it.
- **Per-certificate probe**: after the control build, probe each certificate to name *which* entry
  is malformed. Most precise, but costs one client build per certificate (each reloading the
  native root store) for a bundle that is realistically one or two roots. Not worth it.

The control build runs **only on the already-fatal failure path**, so its cost is irrelevant.

### D2 — a pure policy function, so all four quadrants are testable

The message choice is extracted into a pure function per service:

```rust
fn describe_build_failure(bundle: Option<&str>, store_healthy: bool, chain: &str) -> String
```

The alternative — testing through `HttpJwksFetcher::new` — can only reach the arms a healthy
machine can produce. A broken platform trust store is not something a test can force, so
AC4 ("the no-bundle platform-trust-store message is unchanged") would otherwise be verified by
inspection alone. With the policy extracted, all four `(bundle, store_healthy)` combinations get
a real unit test.

### D3 — render the source chain, not `Display`

`{e}` provably renders as `"builder error"`. Both services walk the chain instead:

```rust
fn causes(e: &dyn std::error::Error) -> String   // "builder error: invalid peer certificate: BadEncoding"
```

This is the same defect class the issue is about — a message that does not say what went wrong —
so it is fixed here rather than filed separately.

### D4 — the gateway needs no new error variant

`OpenAiError` already carries `CaBundle { path, source }`, whose `Display` names
`upstream.openai.extra_ca_bundle_path`, and `adapters/http/error.rs:110` already maps it to
`GatewayError::UpstreamUnavailable` identically to `Build`. So the definitive arm returns
`CaBundle` and the fallback arm returns `Build` — the enum, its `Display` strings, and the
status mapping are all untouched.

The `source` carries the rendered chain as a boxed string, matching how the existing
certificate-free case at `openai/client.rs:158-165` already constructs `CaBundle`.

This keeps the two services parallel in behaviour while respecting SMA-558 D7's decision that the
loaders are **duplicated, not extracted** — `causes` and `describe_build_failure` are written once
per service.

### D5 — the `accept_invalid_tls` path is unaffected

`IdpTls::AcceptInvalid` can never carry a bundle: `IamConfig::validate` rejects the pair, and the
enum makes the combination unrepresentable. That arm therefore always takes the fallback message.

## 3. Acceptance criteria

| AC | Covered by |
|----|-----------|
| 1. The four sites scope the claim to the client, keeping the unconstrained-anchor / roots-only warning | Item 1 fix; reviewed by diff |
| 2. A valid-base64/invalid-DER bundle produces a boot error naming `authn.extra_ca_bundle_path` / `upstream.openai.extra_ca_bundle_path` | D1 definitive arm |
| 3. A test covers that case in both services, asserting the message names the config key — distinct from the existing base64 tests | New test per service, using an `AAAAAAAA` body (the existing tests use `!!!not base64!!!`, which fails a different path) |
| 4. The no-bundle platform-trust-store message is unchanged | D2 policy-function unit tests, all four quadrants |

## 4. Testing

Per service:

- `describe_build_failure` unit tests over all four `(bundle, store_healthy)` combinations,
  including the byte-exact unchanged no-bundle message (AC4).
- An integration-level test driving `HttpJwksFetcher::new` / `OpenAiClient::new` with an
  `AAAAAAAA` certificate body, asserting the error names the config key (AC3).
- The existing `undecodable_*` tests stay as they are — they cover the base64 path, which fails
  earlier and is already correctly attributed.

## 5. Out of scope

- Extracting the two duplicated bundle loaders into a shared crate — SMA-558 D7 decided against it.
- Any change to `CLAUDE.md:218`, which is already correct.
- Any change to the four CA-bundle knobs' add-vs-replace semantics.

## 6. Risks

- **The control build could itself be affected by a future reqwest change** that makes a bare
  build fail for an unrelated reason. The consequence is a fall back to today's message — a
  regression to the status quo, never a wrong accusation.
- **The `AAAAAAAA` fixture depends on rustls continuing to reject that byte sequence as DER.**
  If a future rustls accepted it, the new tests fail loudly rather than silently passing.
