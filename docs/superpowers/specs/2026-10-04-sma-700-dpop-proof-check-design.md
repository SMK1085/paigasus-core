# SMA-700: The gateway accepts a DPoP-bound token, and IAM checks the proof (RFC 9449)

- Linear: SMA-700 (decision D9 of SMA-690,
  `2026-09-26-sma-690-refuse-sender-constrained-token-design.md`)
- Status: draft 2, waits for GATE 1. The spec challenge is in § 11.
- Path: architectural (a new authentication scheme, a proto contract change, new ports, a
  change in two services)
- ADR: ADR-0026 (Notion, status Proposed until GATE 1)
- History: a draft of 2026-09-27 was lost before it was committed. Draft 1 of this spec
  (commit `1016c0b3`) put the check on IAM's own API. The spec challenge showed that the chart
  gives no route to IAM's own API. Sven changed the scope to the gateway path on 2026-10-04
  (D1, D14).

## 1. Problem

SMA-690 made IAM refuse a sender-constrained access token: a token with a `cnf` claim, or with
a payload `typ` of `DPoP` in any letter case (`validator.rs`, step 7,
`sender_constraint_marker`). IAM cannot check the key binding, so a client that uses DPoP
(RFC 9449) cannot use Paigasus.

Keycloak 26.4 binds an access token when the client sends a `DPoP` header to the token
endpoint. The realm needs no setting for this (SMA-690 measurement M2). The bound token has
`typ: DPoP` and `cnf.jkt`. `cnf.jkt` is the RFC 7638 thumbprint of the proof key.

Measured facts about the request path (2026-10-04):

- The chart routes external traffic only to the consoles (`charts/paigasus/templates/ingress.yaml:41-45`,
  `httproute.yaml`). The IAM backend Service is `ClusterIP` (`backend-service.yaml:16`). No
  external client can reach IAM's own API with the shipped chart.
- The chart does not deploy the Rust gateway (`_helpers.tpl:100-101`). The operator deploys it
  and gives its URL (`values.yaml:111`). An API client reaches Paigasus through the gateway.
- The gateway has two protected HTTP routes (`paigasus-gateway/src/adapters/http/mod.rs:87-120`):
  `POST /v1/chat/completions` (`require_iam_auth`) and `GET /v1/service-info`
  (`require_authenticated`). It has no gRPC server.
- `require_iam_auth` (`adapters/http/auth.rs:60-130`) does this: parse `Bearer`
  (`bearer`, `auth.rs:402`); try `IntrospectApiKey`; then `Introspect`; resolve the org; then
  `IsAuthorized` with action `InvokeModel`.
- The gateway authenticates the `IsAuthorized` call with the client token as
  `authorization: Bearer <token>` metadata (`adapters/iam/client.rs:159-168`). `IsAuthorized` is
  not exempt from IAM's `AuthEnforce` (`paigasus-iam/src/adapters/grpc/authn.rs:181-211`). So a
  bound token passes `Introspect` and then fails at `IsAuthorized` (SMA-690 D8).
- The gateway sets no `WWW-Authenticate` header. It maps every `Unauthenticated` from IAM to
  401 `invalid-api-key` (`auth.rs:412-450`, `error.rs:172`). It has no introspect cache
  (`auth.rs:23`).
- `IntrospectRequest` has only `string token = 1` (`contracts/proto/paigasus/iam/v1/iam.proto:242`).
- IAM has a `Clock` port with `SystemClock` and `FixedClock`
  (`paigasus-iam-core/src/ports.rs:338`, `application/fakes.rs`). Token time checks use
  jsonwebtoken's own clock.
- The chart pins IAM to one replica (`backend-deployment.yaml:22-35`, SMA-559).

## 2. Goal and non-goals

Goal: an operator can turn on DPoP in IAM and in the gateway. Then a client that holds a
DPoP-bound token and its private key can call the gateway's protected routes with
`Authorization: DPoP <token>` and a valid `DPoP` proof. A bound token without a valid proof is
refused.

Non-goals:

- The `DPoP` scheme on IAM's own HTTP and gRPC API. IAM keeps refusing it there, except for the
  one `IsAuthorized` follow-up in § 4.7.
- Server-provided nonces (RFC 9449 § 8) and the `DPoP-Nonce` header.
- mTLS-bound tokens (RFC 8705). They stay refused (SMA-690 D4).
- A shared (Redis) replay store.
- Browser DPoP clients. Neither service has a CORS layer.
- A record in the audit log or in `AuthContext` that a request was DPoP-bound.
- A `Bearer` challenge on the gateway. The gateway sends none today, and this issue does not
  add one.
- A new metric.

## 3. Decisions

| # | Decision | Reason |
|---|---|---|
| D1 | Scope: IAM checks a DPoP context forwarded by the gateway in `Introspect`, and the gateway sends it. IAM's own API keeps refusing the `DPoP` scheme. | Sven chose this on 2026-10-04, after the challenge showed that no client can reach IAM's own API (§ 1). |
| D2 | The stateless proof checks are a core port, `DpopProofChecker`. Its jsonwebtoken adapter is in `adapters/oidc/dpop.rs`. An application service, `DpopProofVerifier`, adds the request checks, the time check and the replay check. | Sven approved approach B. The challenge showed that JOSE code belongs in an adapter (`paigasus-iam-core/src/authn.rs:3-4`). No module in `application/` imports `jsonwebtoken`. |
| D3 | The replay store is a port, `ReplayStore`, with one in-memory adapter. One instance is shared (one `Arc` in `AppState`). The port is synchronous. | Sven chose this on 2026-10-04. It is correct because the chart pins IAM to one replica. A Redis adapter would need an async port; that change comes with the adapter. |
| D4 | DPoP is off by default, in IAM and in the gateway. Each has its own switch. | Sven chose a global switch. Existing deployments do not change. |
| D5 | IAM checks `htu` against configured base URLs (`forwarded_base_urls`). The gateway sends the method and the path that it received. Neither service builds a URL from `Host`, `Forwarded` or `X-Forwarded-*`. | A client or a proxy controls those headers. The gateway does not know its public origin (§ 1). |
| D6 | A base URL can have a path prefix. | A proxy in front of the gateway can remove a prefix. The client then signs `https://h/prefix/v1/...` and the gateway sees `/v1/...`. |
| D7 | On the `Bearer` scheme, both SMA-690 markers stay refused, also when DPoP is on. | SMA-690 D9. A bound token is never a bearer token. |
| D8 | Allowed proof algorithms: ES256 and RS256. | The same list as `ALLOWED_ALGORITHMS` for access tokens. |
| D9 | Every proof defect maps to `invalid_dpop_proof`, also `ath` and the thumbprint. | RFC 9449 § 4.3 lists `ath` and the key binding as checks of the proof. One code is simpler for a client. |
| D10 | A full replay store fails closed with a retryable 503. The store is sized and has quotas for each key and each subject (§ 4.5). | Fail open makes replay possible. A 401 would tell the client that its proof is bad, which is false. The quotas stop one user from filling the store for all tenants. |
| D11 | When DPoP is off, the responses of both services do not change. | Existing deployments must see no change (§ 7). |
| D12 | No new metric. Each refusal writes one rate-limited `info` line. A full store also writes a `warn` line with the store occupancy. | The `observability-drift` gate stays unchanged. The occupancy lets an operator size the store. |
| D13 | ADR-0026 records the protocol decision. | Sven asked for it on 2026-10-04. |
| D14 | `IsAuthorized` accepts a one-time follow-up ticket. `Introspect` records the accepted proof. The gateway then sends the same token and proof on `IsAuthorized`. IAM accepts this once, only for `IsAuthorized`, only while the replay entry is live. | Sven chose this on 2026-10-04. The client sends one proof for one request, and the gateway makes two IAM calls for it (§ 1). |
| D15 | On the `DPoP` scheme, the gateway skips the API-key leg. | API keys are not bound to a key. The proof is then sent to IAM on the token leg only. |
| D16 | The proof `typ` is `dpop+jwt` or `application/dpop+jwt`, ASCII case-insensitive. | RFC 7515 § 4.1.9. The validator already accepts both forms for `logout+jwt` (`validator.rs:53`). |
| D17 | The `jwk` members `alg`, `use` and `key_ops` are ignored. | RFC 9449 does not require them. The header `alg` and the key type are checked. |

## 4. Change

### 4.1 Contracts

`contracts/proto/paigasus/iam/v1/iam.proto`:

```proto
// The DPoP context of a client request that the gateway authenticates (RFC 9449).
// IAM checks the proof against authn.dpop.forwarded_base_urls.
message DpopContext {
  // The value of the client's `DPoP` header: one JWS in compact form. At most 8192 bytes.
  string proof = 1;
  // The HTTP method of the client's request, for example "POST". At most 16 bytes.
  string method = 2;
  // The path of the client's request as the gateway received it, with no query. It starts
  // with "/". At most 2048 bytes.
  string path = 3;
}

message IntrospectRequest {
  string token = 1;
  // Present when the client used the DPoP scheme.
  DpopContext dpop = 2;
}
```

The comment on `rpc IsAuthorized` documents the follow-up (§ 4.7): the metadata
`authorization: DPoP <token>` plus `dpop: <proof>`.

`contracts/proto/paigasus/common/v1/error.proto`, in the IAM authn range:

```proto
// "invalid-dpop-proof" — the DPoP proof is missing, malformed, or does not match the
// request or the token.
ERROR_REASON_INVALID_DPOP_PROOF = 46;
```

Run `contracts:generate` and commit the generated Rust, Python and TypeScript output. The
`:breaking` gate must stay green: both changes only add.

Register the new code as `ci/error-registry/README.md` requires (`ci/error-registry/check.py`,
`MANIFEST`). The new enum value also changes:

- `ts/packages/paigasus-sdk/src/errors/presentation.ts:20` (a total `Record`, so
  `:typecheck` fails without an entry);
- the count 67 → 68 in `ts/packages/paigasus-sdk/tests/presentation.test.ts:13`,
  `ts/packages/paigasus-proto/src/error.test.ts:60` and
  `rs/crates/libs/paigasus-proto/src/error.rs` (with its `EXPECTED_REASONS` list).

### 4.2 `paigasus-iam-core`

In `authn.rs`:

- `TokenScheme { Bearer, Dpop }`.
- `Jkt`, a newtype over the base64url thumbprint string.
- `ValidatedClaims` gets `key_binding: Option<Jkt>`. It is `Some` only on the `Dpop` scheme.
- A new `TokenDefect::NotKeyBound`: the `Dpop` scheme with a token that has no `cnf.jkt`.
- A new `AuthnError::InvalidDpopProof(ProofDefect)`. `ProofDefect` is `Copy + Eq + Hash`, with
  one variant for each check: `Missing`, `Malformed`, `Typ`, `Alg`, `Jwk`, `Signature`, `Htm`,
  `Htu`, `Iat`, `Ath`, `Thumbprint`, `Replayed`, `FollowUp`.
- A new `AuthnError::ReplayStoreFull`.

In `ports.rs`:

- The authenticator port takes the scheme: `authenticate(token, scheme)`. Every caller and
  every test double changes. `paigasus-iam-core` has `publish = false`.
- `DpopProofChecker::check(proof, token, jkt) -> Result<ProofClaims, ProofDefect>`.
  `ProofClaims` holds `jti`, `iat`, `htm` and `htu` (a parsed URL).
- `ReplayStore` (§ 4.5).

`paigasus-iam/src/application/retryable.rs:45-63` (`all_authn_errors`) lists the new variants.

### 4.3 The validator (`adapters/oidc/validator.rs`)

Steps 1 to 6b do not change. Step 7 depends on the scheme:

- `Bearer`: no change. A token with `cnf` or `typ: DPoP` is refused with
  `TokenDefect::SenderConstrained` (D7).
- `Dpop`: the token must have `cnf` as a JSON object with a `jkt` member that is a non-empty
  string. Other `cnf` members are allowed. Else the token is refused with
  `TokenDefect::NotKeyBound`. Its log line has its own static message: "refused a DPoP request:
  the token is not bound to a key". On success, `key_binding` is `Some(jkt)`.

### 4.4 The proof check

**Adapter, `adapters/oidc/dpop.rs` (`JoseDpopProofChecker`), checks 1-9:**

1. Size: at most 8192 bytes. Else `Malformed`.
2. Form: three base64url parts with no padding. Decode the header and the payload as raw
   `serde_json::Map`. Else `Malformed`. (`decode_header` is not used: it fails on
   `alg: none` before check 4 can name it.)
3. Header `typ` (D16). Else `Typ`.
4. Header `alg` is the string `ES256` or `RS256` (D8). Else `Alg`. This refuses `none` and
   every HMAC algorithm.
5. Header `jwk`, read from the raw map (jsonwebtoken's `Jwk` drops private members):
   - It is a JSON object with none of the members `d`, `p`, `q`, `dp`, `dq`, `qi`, `oth`, `k`.
   - For `ES256`: `kty` is `EC`, `crv` is `P-256`, and `x` and `y` are strict base64url that
     decode to 32 bytes each. (`from_ec_components` ignores `crv`, so this check is necessary.)
   - For `RS256`: `kty` is `RSA`, `n` and `e` are strict base64url. `n` has no leading zero
     byte and is 2048 to 4096 bits. `e` is odd, at least 3 and at most 2^33 − 1 (the `rsa`
     0.9 limit).
   - Else `Jwk`.
6. Signature: `jsonwebtoken::decode::<serde_json::Map<..>>` with the key from the `jwk`, a
   `Validation` for the header `alg` with `required_spec_claims` empty and `validate_exp`,
   `validate_nbf` and `validate_aud` all false. Else `Signature`.
7. Claims: `jti` is a string of 1 to 256 bytes; `htm` and `htu` are strings; `iat` is a JSON
   integer in the `i64` range; `ath` is a string. A missing or wrong-type claim gives
   `Malformed`. A fractional `iat` gives `Malformed`.
8. `ath` equals base64url, with no padding, of SHA-256 over the ASCII bytes of the access
   token. Else `Ath`.
9. The RFC 7638 thumbprint of the `jwk` equals the `jkt` of the token. Use
   `Jwk::thumbprint(ThumbprintHash::SHA256)` only after check 5 (it writes the members into
   JSON with no escapes). Else `Thumbprint`.

`htu` is parsed with the `url` crate (WHATWG rules). A parse failure gives `Htu`.

**Application, `application/dpop.rs` (`DpopProofVerifier`), checks 10-13:**

10. `htm` equals the forwarded method, exactly. Else `Htm`.
11. `htu` matches (§ 4.6). Else `Htu`.
12. `iat`: with `now` from `Clock`, `|now − iat| ≤ iat_window_secs + leeway_secs`, with
    checked arithmetic. An overflow gives `Iat`. Else `Iat`.
13. Replay: `ReplayStore::record(...)` (§ 4.5). This is last, so a proof that fails another
    check does not use a `jti`.

A `nonce` claim is accepted and ignored.

The verifier owns the base-URL list. An adapter cannot pass another list.

### 4.5 The replay store (`adapters/dpop_replay.rs`)

Port:

```rust
pub trait ReplayStore: Send + Sync {
    fn record(&self, entry: NewProof, now: i64) -> RecordOutcome; // Fresh | Replayed | Full
    fn redeem_follow_up(&self, key: ProofKey, proof_digest: [u8; 32], now: i64) -> RedeemOutcome; // Redeemed | Refused
}
```

- `ProofKey` is the first 16 bytes of `blake3(jkt || 0x00 || jti)`. `blake3` is already an IAM
  dependency.
- `NewProof` holds the key, the subject (`issuer`, `sub`), the `jkt`, `expires_at` (unix
  seconds: `iat + iat_window_secs + leeway_secs`) and the follow-up digest
  (`blake3` of the whole proof string).
- An entry is live while `now <= expires_at`. Time comes only from `Clock`, through the `now`
  argument.
- An expiry index (`BTreeMap<i64, Vec<ProofKey>>`) holds the keys by expiry second. Each call
  removes only the expired heads. No call scans the whole map.
- Quotas: `replay_capacity` (global), `per_key_quota` (each `jkt`), `per_subject_quota` (each
  issuer and `sub`). The counts go down when an entry expires. A count at zero is removed.
- `record` returns `Full` when a quota or the capacity is reached after the expired heads are
  removed. `Full` gives `AuthnError::ReplayStoreFull`.
- `redeem_follow_up` returns `Redeemed` only if the entry is live, its follow-up is not used,
  and the digest matches. It then marks the follow-up as used.
- One `Mutex` protects the store. A restart clears it (R1).

### 4.6 `htu` comparison

- Each `forwarded_base_urls` entry is parsed at boot into scheme, host, port and a path
  prefix. The prefix has no trailing `/`; an empty prefix means the root.
- The expected URL is `scheme://host[:port]` + prefix + the forwarded `path`. It is parsed
  with the `url` crate.
- The proof `htu` and the expected URL are compared after both pass the same `url` parser:
  the scheme, the host and the port with a default port removed, and `Url::path()`. The query
  and the fragment of `htu` are ignored. `Url::path()` keeps percent-encoding, so a path that
  differs only by percent-encoding does not match. The parser removes dot segments on both
  sides. This is a deliberate deviation from the full RFC 3986 § 6.2.2 normalisation.
- The proof matches if it matches one entry.

### 4.7 IAM: `Introspect` and the `IsAuthorized` follow-up

`Introspect` (gRPC; the HTTP twin `POST /v1/authn/introspect` does not change):

- `dpop` absent: the `Bearer` scheme, as today (D7).
- `dpop` present and DPoP off: `InvalidToken(Malformed)` (D11).
- `dpop` present and DPoP on: check the sizes in § 4.1 (`Malformed` if over), validate the
  token with the `Dpop` scheme, then run the verifier. An empty `proof` gives `Missing`.

`IsAuthorized` follow-up, in `AuthEnforce` (`adapters/grpc/authn.rs`):

- The scheme parser accepts `DPoP` only when DPoP is on and the path is
  `/paigasus.iam.v1.AuthorizationService/IsAuthorized` (`iam.proto:411`). The code takes the
  path from the generated service constants, not from a string literal. On every other path, and when DPoP is off, the `DPoP` scheme is refused as
  `InvalidToken(Malformed)`, as today.
- The request must have exactly one `dpop` metadata entry. Else `ProofDefect::Missing` or
  `Malformed`.
- IAM validates the token with the `Dpop` scheme. It reads `jti` and `ath` from the proof
  payload without a signature check. It checks that `ath` matches the token. It then calls
  `redeem_follow_up` with the key from the token `jkt` and the proof `jti`, and the digest of
  the whole proof. The digest proves that the bytes are the same bytes that `Introspect`
  verified. A refusal gives `ProofDefect::FollowUp`.

Errors on gRPC:

| Error | Code | Reason | Message |
|---|---|---|---|
| `InvalidDpopProof(_)` | `Unauthenticated` | `INVALID_DPOP_PROOF` | "invalid DPoP proof" |
| `InvalidToken(NotKeyBound)` | `Unauthenticated` | `INVALID_TOKEN` | "invalid bearer token" (no change) |
| `ReplayStoreFull` | `Unavailable` | `AUTHN_UNAVAILABLE` | "authentication is temporarily unavailable" (retryable) |

IAM's HTTP routes do not change.

Logs: each refusal writes one `info` line through a `LogRateLimiter<ProofDefect>` owned by the
verifier. The message is static: "refused a DPoP proof". The line has the issuer and the static
defect name. It never contains the proof, the token, the `jti`, the `jkt`, the path or the URL.
`ReplayStoreFull` writes one rate-limited `warn` line with the entry count and which limit was
reached.

### 4.8 IAM configuration (`config.rs`)

```toml
[authn.dpop]
enabled = false
forwarded_base_urls = []
iat_window_secs = 60
replay_capacity = 200000
per_key_quota = 1000
per_subject_quota = 2000
```

- `DpopConfig` with `#[serde(default)]` and a `Default` implementation (the `MigrationConfig`
  pattern). Env names: `IAM_AUTHN__DPOP__ENABLED`, `IAM_AUTHN__DPOP__FORWARDED_BASE_URLS`
  (the figment inline `[...]` form, as `IAM_AUTHN__ISSUERS` uses), and the same pattern for the
  numbers.
- `validate()` refuses to boot when:
  - `enabled` is true and `forwarded_base_urls` is empty;
  - an entry has a query, a fragment or user info;
  - an entry has a scheme other than `https`, except `http` on a loopback host (`localhost`,
    `127.0.0.0/8` or `::1`);
  - a number is 0;
  - `iat_window_secs + authn.leeway_secs` is more than 300;
  - `per_key_quota` or `per_subject_quota` is more than `replay_capacity`.
- `test_config` and `test_config_with` get the default `DpopConfig`.

### 4.9 The gateway

**Config** (`config.rs`): `dpop: GatewayDpopConfig { enabled: bool }`, default false, env
`GATEWAY_DPOP__ENABLED`. The test constructor in `service_info.rs` changes.

**Parser** (`auth.rs:402`): `bearer` becomes `credentials(headers) -> Option<Credentials>`.
`Credentials` is `Bearer(token)` or `Dpop { token, proof: ProofHeader }`. `ProofHeader` is
`One(String)`, `Missing` or `Invalid` (two or more `DPoP` headers, or a value that is not
visible ASCII). When gateway DPoP is off, the `DPoP` scheme gives `None`, as today (D11). On the
`Bearer` scheme, a `DPoP` header is ignored.

**`Iam` trait** (`adapters/iam/client.rs:58-84`):

- `introspect_token(&self, token, dpop: Option<DpopContext>)`.
- `is_authorized_self(&self, caller: &CallerCredential, ...)`. `CallerCredential` is
  `ApiKey(key)`, `Bearer(token)` or `Dpop { token, proof }`. The `Dpop` case sends
  `authorization: DPoP <token>` and `dpop: <proof>` metadata.
- All implementers change: `IamClient`, `FakeIam` (`auth.rs:561`), `UnusedIam` and `ProbeIam`
  (`http/mod.rs:188`, `:219`), `tests/metrics.rs` (three), `tests/service_info.rs`,
  `tests/chat_proxy.rs`, `tests/support/limits.rs`.

**Flow** (`require_iam_auth` and `require_authenticated`):

- `Dpop` credentials with `ProofHeader::Missing` or `Invalid`: 401 `invalid-dpop-proof` at once,
  with no IAM call.
- `Dpop` credentials: skip the API-key leg (D15). Call `introspect_token` with
  `DpopContext { proof, method: req.method(), path: OriginalUri path }`.
- `require_iam_auth` then calls `is_authorized_self` with `CallerCredential::Dpop`.
- `Bearer` credentials: no change.

**Errors** (`error.rs`):

- `introspect_error` and `authz_error` read the IAM reason. `Unauthenticated` with
  `INVALID_DPOP_PROOF` gives a new `GatewayError::InvalidDpopProof`: 401,
  type `invalid_request_error`, code `invalid-dpop-proof`, message "Invalid DPoP proof.". Every
  other `Unauthenticated` gives `InvalidCredential`, as today. `Unavailable` gives
  `IamUnavailable` (503, retryable), as today.
- When gateway DPoP is on, every 401 appends a `WWW-Authenticate` line:
  - the client used `DPoP` and the error is `InvalidDpopProof`:
    `DPoP error="invalid_dpop_proof", algs="ES256 RS256"`;
  - the client used `DPoP` and the error is another 401: `DPoP error="invalid_token", algs="ES256 RS256"`;
  - the client used `Bearer` or sent no credential: `DPoP algs="ES256 RS256"` (no error
    attribute: RFC 6750 § 3 puts the error on the scheme the client used).
- One function builds the challenge from `(dpop_enabled, scheme_used, error)`. The
  `IntoResponse` path calls it. It uses `append`, not `insert`.
- When gateway DPoP is off, no `WWW-Authenticate` header is sent, as today (D11).

### 4.10 Chart and docs

- IAM values `zones.iam.backend.dpop.enabled` (default `false`) and
  `zones.iam.backend.dpop.forwardedBaseUrls`. `_iam-backend.tpl` projects them to env only
  when `enabled` is true. Add `IAM_AUTHN__DPOP__ENABLED` and
  `IAM_AUTHN__DPOP__FORWARDED_BASE_URLS` to `paigasus.iamReservedEnv`
  (`_iam-backend.tpl:16-18`). `paigasus.validateIamBackend` refuses `enabled: true` with an
  empty list; add the case to `tests/refusals.sh`. With the defaults, the rendered output does
  not change. Add one golden case with DPoP on.
- The gateway is not deployed by the chart, so the chart has no gateway DPoP value. The
  operator sets `GATEWAY_DPOP__ENABLED`.
- The comment at the IAM `replicas: 1` pin gets one more reason: the in-memory replay store
  (D3, SMA-700).
- `docs/ops/RUNBOOK-chart.md` (the SMA-690 text at about lines 230-246):
  - How to turn DPoP on: both switches, and `forwardedBaseUrls` = the public URLs at which
    clients reach the gateway, with any prefix that a proxy removes.
  - A client must make a new proof for each attempt. A retry with the same proof is refused as
    a replay.
  - The `Bearer` scheme still refuses a bound token. IAM's own API does not accept DPoP.
  - A 503 with a full replay store: read the `warn` line, then raise the limits.
- `CHANGELOG.md` of `paigasus-iam` and `paigasus-gateway`: one entry each.
- ADR-0026 is updated to this scope.

## 5. Tests

### 5.1 Unit tests (each asserts the exact `ProofDefect` or `GatewayError`)

- **Proof checker:** a valid ES256 proof and a valid RS256 proof pass. One test for each of
  checks 1-9, which changes only that field. Extra cases: `alg: none`; `alg: HS256`; a `jwk`
  with `d`; an RSA key of 2047 and 4097 bits; `e` even; a P-384 key with `ES256`; `typ`
  `application/DPoP+JWT` (pass); a fractional `iat`; `ath` of another token; the thumbprint of
  another key; a `nonce` claim (pass).
- **Thumbprint:** the RFC 7638 § 3.1 example key gives the published thumbprint.
- **Verifier:** checks 10-13 with `FixedClock`; `iat` at the edge (pass) and one second over
  (fail); `iat` near `i64::MAX` (`Iat`, no panic); `htm` `post` against `POST` (fail).
- **`htu`:** default port; host case; query and fragment ignored; a path prefix base URL; a
  path that differs by percent-encoding (fail); a trailing slash (fail); a base URL not in the
  list (fail).
- **Replay store:** fresh; replayed; live at `now == expires_at` and expired at
  `expires_at + 1`; each quota; the capacity; expired heads removed before `Full`; counts go to
  zero and are removed; follow-up redeemed once, refused the second time, refused with another
  digest, refused after expiry.
- **Validator:** the `Dpop` scheme with `cnf.jkt` (pass); with `cnf` but no `jkt`; with
  `typ: DPoP` only; with no binding (`NotKeyBound`). The SMA-690 `Bearer` cases stay.
- **Config:** each `validate()` refusal in § 4.8; the env names through `figment::Jail`.
- **IAM gRPC mapping:** each row of the table in § 4.7.
- **Gateway parser:** scheme case; two `DPoP` headers; a missing header; a non-ASCII value; the
  `Bearer` scheme with a `DPoP` header; DPoP off.
- **Gateway challenge builder:** each case in § 4.9, and two lines with `append`.
- **RS256 fixtures:** the validator tests do not use the `rsa` crate (`validator.rs:477-480`).
  Commit fixed RSA test keys as fixtures, or add `rsa` as an IAM dev-dependency. The plan
  chooses one and records it.

### 5.2 Integration tests

Extend `MockIdp` (`paigasus-iam/tests/support/mod.rs:165`) so it can mint a token with
`cnf.jkt` and `typ: DPoP`. Add a proof helper next to it (the shape of `keycloak_e2e.rs:358`).

- **IAM, `tests/grpc_authn.rs` and `tests/grpc_whoami.rs`** (their `IntrospectRequest`
  literals get `dpop: None`). New cases, over real gRPC:
  - the gateway sequence: `Introspect` with a valid `dpop` context, then `IsAuthorized` with the
    follow-up metadata. Both pass.
  - the same `Introspect` again: `INVALID_DPOP_PROOF` (replay).
  - the follow-up twice: the second gives `INVALID_DPOP_PROOF`.
  - the follow-up on another RPC (for example `WhoAmI`): `INVALID_TOKEN`.
  - the follow-up with no earlier `Introspect`: `INVALID_DPOP_PROOF`.
  - DPoP off: a `dpop` context gives `INVALID_TOKEN`, as for a malformed token.
  - no `dpop` context with a bound token: `INVALID_TOKEN`, as today.
  - a proof whose `htu` names a base URL that is not configured: `INVALID_DPOP_PROOF`.
- **IAM, `tests/keycloak_e2e.rs`:** with DPoP on, a real Keycloak DPoP-bound token and a new
  proof pass `Introspect` with a `dpop` context. The SMA-690 `Bearer` assertion stays.
- **Gateway, `auth.rs` tests with `FakeIam`:** a DPoP request sends the context to
  `introspect_token` with the right method and path, skips `introspect_api_key`, and sends
  `CallerCredential::Dpop` to `is_authorized_self`. An IAM `INVALID_DPOP_PROOF` gives 401
  `invalid-dpop-proof` with the challenge. Gateway DPoP off: the response is the same as today,
  except the per-request headers.
- **Gateway, `tests/chat_proxy.rs`:** one DPoP request end to end with a fake IAM that records
  both calls.

### 5.3 Proof that the tests bite

Each mutation below must compile. Run `cargo nextest run --no-fail-fast` on the IAM `--lib`,
`grpc_authn`, `grpc_whoami` and `keycloak_e2e` binaries and the gateway `--lib` and
`chat_proxy` binaries, with `PAIGASUS_REQUIRE_DOCKER=1`. Record which test fails for each
mutation.

- Checks 1-13: each one is replaced by `Ok(())` or the pass value. For check 2, the mutation
  accepts a header with no `typ`. For check 6, the mutation uses
  `jsonwebtoken::dangerous::insecure_decode`. For check 7, the mutation gives a default `jti`.
- Wiring: the verifier call is removed from `Introspect`; `Introspect` uses the `Bearer` scheme
  for a `dpop` context; `redeem_follow_up` always gives `Redeemed`; the follow-up path check
  accepts any RPC; the gateway sends `dpop: None`; the gateway calls the API-key leg first on
  `DPoP`.
- D7: the `Bearer` scheme accepts a bound token. D11: IAM accepts a `dpop` context when off.
- If a mutation fails no test, add a test and run the full battery again (the memory rule on
  re-running a battery whole).

Record the results in `docs/superpowers/plans/2026-10-04-sma-700-mutation-results.md`.

### 5.4 Acceptance criteria

| Criterion | Evidence |
|---|---|
| 1. With DPoP on in both services, a valid DPoP request to the gateway is accepted and authorized. | § 5.2 IAM gateway sequence; gateway `chat_proxy`; keycloak |
| 2. Each RFC 9449 § 4.3 check refuses a proof that fails it. | § 5.1 proof checker and verifier; § 5.3 |
| 3. A replayed proof is refused. The follow-up works once only. | § 5.1 replay store; § 5.2 |
| 4. The `Bearer` scheme still refuses a bound token (SMA-690 D9). | § 5.1 validator; § 5.2; § 5.3 D7 |
| 5. With DPoP off, the responses of both services are the same as before. | § 5.2 DPoP-off cases; § 5.3 D11 |
| 6. The gateway sends a `WWW-Authenticate: DPoP` challenge as § 4.9 says. | § 5.1 challenge builder; § 5.2 gateway |

## 6. Files

- Contracts: `contracts/proto/paigasus/iam/v1/iam.proto`,
  `contracts/proto/paigasus/common/v1/error.proto`, the generated output,
  `ci/error-registry/` (`MANIFEST`).
- TS: `ts/packages/paigasus-sdk/src/errors/presentation.ts`,
  `ts/packages/paigasus-sdk/tests/presentation.test.ts`,
  `ts/packages/paigasus-proto/src/error.test.ts`.
- Rust libs: `rs/crates/libs/paigasus-iam-core/src/{authn.rs,ports.rs}`,
  `rs/crates/libs/paigasus-proto/src/error.rs`.
- IAM: `src/adapters/oidc/{validator.rs,dpop.rs (new)}`, `src/adapters/dpop_replay.rs` (new),
  `src/application/{dpop.rs (new),authenticate_token.rs,retryable.rs}`,
  `src/adapters/grpc/{authn.rs,convert.rs}`, `src/config.rs`, the `AppState` wiring,
  `tests/support/mod.rs`, `tests/{grpc_authn.rs,grpc_whoami.rs,keycloak_e2e.rs}`,
  `CHANGELOG.md`.
- Gateway: `src/config.rs`, `src/service_info.rs`, `src/adapters/http/{auth.rs,error.rs,mod.rs}`,
  `src/adapters/iam/client.rs`, `tests/{chat_proxy.rs,service_info.rs,metrics.rs}`,
  `tests/support/limits.rs`, `CHANGELOG.md`.
- Chart: `charts/paigasus/values.yaml`, `templates/_iam-backend.tpl`,
  `templates/backend-deployment.yaml` (comment), golden files, `tests/refusals.sh`,
  `README.md`.
- Docs: `docs/ops/RUNBOOK-chart.md`.
- New crates: none, except possibly `rsa` as an IAM dev-dependency (§ 5.1).

## 7. Rollout and effect on operators

- The defaults are off. With the defaults, every IAM and gateway response is the same as
  before (D11), and the chart renders the same output.
- To turn DPoP on, the operator sets `zones.iam.backend.dpop.enabled`,
  `zones.iam.backend.dpop.forwardedBaseUrls` and `GATEWAY_DPOP__ENABLED`.
- Supported topology: TLS ends in front of the gateway. Each public URL at which a client
  reaches the gateway is in `forwardedBaseUrls`, with any prefix that a proxy removes. A wrong
  entry refuses DPoP requests (`Htu`); it does not accept a wrong request.
- IAM on DPoP off and the gateway on: DPoP requests get 401 with the DPoP challenge.
- More than one IAM replica with DPoP on is not safe (R2). The chart pin prevents it.

## 8. Verification before merge

- The full gate graph from the root `CLAUDE.md` (`moon ci … --base origin/main
  --include-relations`), with the bash rules for this machine.
- `contracts:generate` drift, `:breaking`, the error-registry gate, `:typecheck`,
  `:helm-render`.
- The mutation battery (§ 5.3).
- `keycloak_e2e.rs` locally with Docker.

## 9. Residuals

| # | Residual | Effect | Owner |
|---|---|---|---|
| R1 | A restart clears the replay store. | A proof used before the restart can be used again until its window ends (at most 120 s with the defaults). The attacker needs a captured proof for the same method and URL. | Accepted. |
| R2 | The in-memory store needs one IAM replica. | Two replicas would accept one proof once on each. The chart pin (SMA-559) prevents this today. | The comment at the pin (§ 4.10). |
| R3 | No server nonce (RFC 9449 § 8). | A client can make proofs in advance, within the `iat` window. | Accepted. |
| R4 | `Introspect` and the follow-up trust the caller for the method and the path. | A caller that can reach IAM's gRPC port can claim any path under a configured base URL. It still needs a proof signed by the client's private key for that URL. `Introspect` is already unauthenticated in-cluster. | Accepted. |
| R5 | The follow-up skips the signature check. | It relies on the digest of bytes that `Introspect` verified. A captured proof and token give one `IsAuthorized` answer for the token owner, within the window. | Accepted. |
| R6 | IAM's own API does not accept DPoP. | A client cannot use DPoP against IAM directly. No shipped route exists for that (§ 1). | Not tracked. A new issue if a route is added. |
| R7 | Many identities can still fill the global capacity. | DPoP requests get 503 until entries expire (at most 120 s). | The `warn` line; the operator raises `replay_capacity`. |

## 10. Open questions

None. Sven decided the scope, the replay store, the switches, the authz step and the ADR on
2026-10-04.

## 11. Spec challenge

Challenge 1 (2026-10-04, on draft 1): verdict NEEDS REWORK. The changes are in this draft.

| Finding | Severity | Action |
|---|---|---|
| The replay store can be filled by one user, its clock is not the `iat` clock, the expiry scan is O(n), the count map leaks, and `StoreFull` maps to a non-retryable 401. | BLOCKER | Folded in: § 4.5 (digest key, expiry index, per-key and per-subject quotas, one `Clock`, inclusive edge), D10 (retryable 503). The challenger's 429 for a quota was not taken: one code (503) is simpler, and a quota only affects its own key or subject. |
| The chart exposes no IAM origin. | MAJOR | Scope changed by Sven (D1): the gateway path. § 7 states the supported topology. |
| The verifier in `application/` imports JOSE code. | MAJOR | Folded in: D2, § 4.4 (port plus adapter). |
| The jsonwebtoken configuration and the parse step are not pinned. | MAJOR | Folded in: § 4.4 checks 2, 5, 6, 9. |
| The challenges have no place in the code, and the error attribute is on the wrong scheme. | MAJOR | Folded in: § 4.9 (one builder, `append`, error only on the scheme used). IAM's HTTP challenges are out of scope with the new scope. |
| The mutation battery leaves out the wiring and D7. | MAJOR | Folded in: § 5.3. |
| § 6 leaves out files that red CI, and cites a wrong gate path. | MAJOR | Folded in: § 4.1, § 4.2, § 6. |
| The clock statement is wrong. | MINOR | Folded in: § 1, § 4.4 check 12. |
| An unbound token on the DPoP scheme was named `SenderConstrained`. | MINOR | Folded in: `NotKeyBound` (§ 4.2, § 4.3). |
| The `htu` and origin model is not clear. | MINOR | Folded in: § 4.6, § 4.8 (loopback defined). |
| `typ` is too strict. | MINOR | Folded in: D16. |
| The RSA bounds are not stated. | MINOR | Folded in: § 4.4 check 5. |
| The claim types and the overflow are not defined. | MINOR | Folded in: § 4.4 checks 7, 12. |
| The HTTP introspect body cap is too small. | MINOR | Not needed: the HTTP twin does not change (§ 4.7). The gRPC sizes are in § 4.1. |
| The log line has no issuer. | MINOR | Folded in: § 4.7 (the verifier gets the issuer and owns its rate limiter). |
| An adapter can choose the origin list. | MINOR | Folded in: § 4.4 (the verifier owns the list). |
| Chart details (reserved env, refusal mechanism, list form, values path). | MINOR | Folded in: § 4.8, § 4.10. |
| The config bounds are incomplete. | MINOR | Folded in: § 4.8. |
| Test plan gaps (RS256 keys, byte-identical, precedence, `htm` case, two metadata entries). | MINOR | Folded in: § 5.1, § 5.2. |
| Client retries are refused as replays. | MINOR | Folded in: the runbook (§ 4.10). |
| Store sharing and sync or async port. | MINOR | Folded in: D3. |
| A cited line was off. | MINOR | Folded in: `auth.rs:402`. |
| Q: Who is the first client of IAM's own API? | QUESTION | Answered by the scope change (D1). |
| Q: `invalid_token` for `Thumbprint` and `Ath`? | QUESTION | Decided: `invalid_dpop_proof` (D9). |
| Q: an origin in both lists? | QUESTION | No longer applies: one list. |
| Q: send `dpop` once, on the token leg? | QUESTION | Decided: D15; the follow-up is the second, controlled use (D14). |
| Q: record DPoP in `AuthContext` or audit? | QUESTION | Non-goal (§ 2). |
| Q: `jwk` `alg`, `use`, `key_ops`? | QUESTION | Decided: ignored (D17). |
| Q: browser clients and CORS? | QUESTION | Non-goal (§ 2). |
