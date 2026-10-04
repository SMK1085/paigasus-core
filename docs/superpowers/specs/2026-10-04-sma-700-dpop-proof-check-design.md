# SMA-700: IAM checks a DPoP proof (RFC 9449)

- Linear: SMA-700 (decision D9 of SMA-690,
  `2026-09-26-sma-690-refuse-sender-constrained-token-design.md`)
- Status: draft, waits for GATE 1. The spec challenge is in § 11.
- Path: architectural (a new authentication scheme, a proto contract change, a new port)
- ADR: ADR-0026, "IAM checks DPoP proofs (RFC 9449) on its own API, off by default"
  (Notion, status Proposed until GATE 1)
- History: an earlier draft of this spec (2026-09-27) was lost before it was committed. This
  spec starts again from the Linear issue and the decisions in its Linear comment. Sven
  confirmed or changed each decision again on 2026-10-04 (§ 3).

## 1. Problem

SMA-690 made IAM refuse a sender-constrained access token: a token with a `cnf` claim, or with
a payload `typ` of `DPoP` in any letter case (`validator.rs`, step 7, `sender_constraint_marker`).
IAM cannot check the key binding, so a client that uses DPoP (RFC 9449) cannot use Paigasus.

Keycloak 26.4 binds an access token when the client sends a `DPoP` header to the token
endpoint. The realm needs no setting for this (SMA-690 measurement M2). The bound token has
`typ: DPoP` and `cnf.jkt`, and `cnf.jkt` is the RFC 7638 thumbprint of the proof key.

Today:

- `bearer_from_headers` (`rs/crates/services/paigasus-iam/src/adapters/auth.rs:33`) accepts
  only the `Bearer` scheme. `Authorization: DPoP <token>` gives `None`, and the caller refuses
  it as `InvalidToken(Malformed)`.
- The HTTP middleware (`adapters/http/auth_middleware.rs:35`) and the gRPC `AuthEnforce`
  service (`adapters/grpc/authn.rs:211`) have the request in scope. They send only the token
  to `AuthenticateToken::resolve`.
- IAM has no configured public origin, and no code reads `Host`, `Forwarded` or
  `X-Forwarded-*`.
- `IntrospectRequest` (`contracts/proto/paigasus/iam/v1/iam.proto:242`) has only
  `string token = 1`. The gateway calls `Introspect` at two sites
  (`rs/crates/services/paigasus-gateway/src/adapters/http/auth.rs:102`, `:284`).
- The chart pins IAM to one replica
  (`charts/paigasus/templates/backend-deployment.yaml:22-35`, SMA-559).

## 2. Goal and non-goals

Goal: an operator can turn on DPoP. Then a client that holds a DPoP-bound token and its private
key can call IAM's own HTTP and gRPC API with `Authorization: DPoP <token>` and a valid proof.
A bound token without a valid proof is refused. The contract that the gateway needs is in place.

Non-goals:

- The gateway does not send the DPoP context in this issue (follow-up issue, § 4.10).
- Server-provided nonces (RFC 9449 § 8) and the `DPoP-Nonce` header.
- mTLS-bound tokens (RFC 8705). They stay refused (SMA-690 D4).
- A shared (Redis) replay store.
- A new metric.

## 3. Decisions

| # | Decision | Reason |
|---|---|---|
| D1 | Scope: IAM's own HTTP and gRPC API, plus a new optional `dpop` field on `IntrospectRequest` (and on the HTTP twin `POST /v1/authn/introspect`). The gateway wiring is a follow-up issue. | Sven chose this on 2026-10-04. The contract changes once. The follow-up then changes only gateway code. |
| D2 | The proof check is a separate application service, `DpopProofVerifier`. The validator gets only a scheme input. | Sven approved approach B on 2026-10-04. Token validation and request checks stay apart. Each unit has its own tests. `validator.rs` does not grow a replay store. |
| D3 | The `jti` replay store is a port, `ReplayStore`, with one in-memory adapter. | Sven chose this on 2026-10-04. It is correct because the chart pins IAM to one replica. A Redis adapter can come later through the same port. |
| D4 | One global config section, `authn.dpop`, off by default. | Sven chose this on 2026-10-04. Existing deployments do not change. |
| D5 | IAM checks `htu` against configured origins (`origins`, `forwarded_origins`). It never builds `htu` from `Host`, `Forwarded` or `X-Forwarded-*`. | A client or a proxy controls those headers. IAM has no trusted public URL today (§ 1). |
| D6 | For a gRPC call, `htm` is `POST` and `htu` is `<origin>/<package.Service>/<Method>`. | A gRPC call is an HTTP/2 POST to that path. RFC 9449 does not define gRPC. The rule lets a client use its normal DPoP code with the gRPC URL. |
| D7 | A direct request is checked against `origins`. An `Introspect` DPoP context is checked against `forwarded_origins`. | A proof for the gateway then cannot be used on IAM's own API, and the reverse. |
| D8 | On the `Bearer` scheme, both SMA-690 markers stay refused, also when DPoP is on. | SMA-690 D9 and the Linear issue. A bound token is never a bearer token. |
| D9 | Allowed proof algorithms: ES256 and RS256. | The same list as `ALLOWED_ALGORITHMS` for access tokens. `jsonwebtoken` 11 with `rust_crypto` supports both. More algorithms are a later change. |
| D10 | A full replay store fails closed. | Fail open makes replay possible under load. Each stored entry needs a valid access token first (§ 4.5), so an attacker without a token cannot fill the store. |
| D11 | When DPoP is off, the wire response for the `DPoP` scheme does not change. | Existing deployments must see no change (§ 7). |
| D12 | No new metric. Each refusal writes one rate-limited `info` line. | The `observability-drift` gate stays unchanged. The SMA-690 refusal has only a log line too. |
| D13 | ADR-0026 records the protocol decision. | Sven asked for it on 2026-10-04. SMA-690 D11 recorded "no DPoP support" with no ADR. This issue reverses that. |

## 4. Change

### 4.1 Contracts

`contracts/proto/paigasus/iam/v1/iam.proto`:

```proto
// The DPoP context of the request that a caller (the gateway) authenticates for a client.
// RFC 9449. IAM checks the proof against authn.dpop.forwarded_origins.
message DpopContext {
  // The value of the client's `DPoP` header: one JWS in compact form.
  string proof = 1;
  // The HTTP method of the client's request, for example "POST".
  string method = 2;
  // The absolute URL of the client's request, with no fragment.
  string url = 3;
}

message IntrospectRequest {
  string token = 1;
  // Present when the client used the DPoP scheme.
  DpopContext dpop = 2;
}
```

`contracts/proto/paigasus/common/v1/error.proto`, in the IAM authn range:

```proto
// "invalid-dpop-proof" — the DPoP proof is missing, malformed, or does not match the
// request or the token. The WWW-Authenticate error value is `invalid_dpop_proof`.
ERROR_REASON_INVALID_DPOP_PROOF = 46;
```

Run `contracts:generate` and commit the generated Rust, Python and TypeScript output. The
`:breaking` gate must stay green: both changes only add.

Before the error reason is added, read `ci/error-code-single-site/README.md` and register the
new code the way that gate requires.

### 4.2 `paigasus-iam-core`

- A new type `TokenScheme { Bearer, Dpop }`.
- The authenticator port takes the scheme: `authenticate(token, scheme)`. Every caller and every
  test double changes. `paigasus-iam-core` has `publish = false`, so this is not a semver break.
- `ValidatedClaims` gets `key_binding: Option<Jkt>`. `Jkt` is a newtype over the base64url
  thumbprint string. It is `Some` only on the `Dpop` scheme.
- A new error `AuthnError::InvalidDpopProof(ProofDefect)`. `ProofDefect` has one variant for
  each check: `Missing`, `Malformed`, `Typ`, `Alg`, `Jwk`, `Signature`, `Htm`, `Htu`, `Iat`,
  `Ath`, `Thumbprint`, `Replayed`, `StoreFull`.
- New ports: `ReplayStore` (§ 4.5). The verifier uses the clock abstraction that IAM already
  uses for token time checks. If IAM has none that the application layer can inject, add a
  `Clock` port with a system adapter.

### 4.3 The validator (`adapters/oidc/validator.rs`)

Steps 1 to 6b do not change. Step 7 depends on the scheme:

- `Bearer`: no change. A token with `cnf` or `typ: DPoP` is refused with
  `TokenDefect::SenderConstrained` (D8).
- `Dpop`: the token must have `cnf` as a JSON object with a `jkt` member that is a non-empty
  string. Other `cnf` members (for example `x5t#S256`) are allowed, but `jkt` is required. A
  token with `typ: DPoP` and no `cnf.jkt`, or with no binding at all, is refused with
  `TokenDefect::SenderConstrained`. The log line uses a new static marker, `no cnf.jkt`. On
  success, `key_binding` is `Some(jkt)`.

The `Dpop` scheme reaches the validator only when DPoP is on (§ 4.6).

### 4.4 The proof verifier (`application/dpop.rs`, new)

`DpopProofVerifier::verify(proof, request: RequestTarget, token, jkt, origins) -> Result<(), ProofDefect>`.

`RequestTarget` holds the method and the URL that the proof must name. The verifier checks, in
this order:

1. Size: the proof is at most 8192 bytes, else `Malformed`.
2. Form: one JWS in compact form (three base64url parts). The header and the payload are JSON
   objects. Else `Malformed`.
3. Header `typ` is exactly `dpop+jwt`, else `Typ`.
4. Header `alg` is `ES256` or `RS256` (D9), else `Alg`. `none` and any HMAC algorithm are
   refused here.
5. Header `jwk`:
   - It is present and is a public key. It has none of the private members `d`, `p`, `q`,
     `dp`, `dq`, `qi`, `oth`, `k`. Else `Jwk`.
   - For `ES256`: `kty` is `EC` and `crv` is `P-256`. For `RS256`: `kty` is `RSA` and the
     modulus is at least 2048 bits. Else `Jwk`.
6. Signature: verify the JWS with the key from `jwk` (`DecodingKey::from_jwk`). Else
   `Signature`. Claims are read only after this step.
7. Payload claims `jti` (a non-empty string, at most 256 bytes), `htm`, `htu` and `iat` (a
   number) are present. Else `Malformed`.
8. `htm` equals the request method (exact, case-sensitive, RFC 9110 methods are upper case).
   Else `Htm`.
9. `htu` matches the request (§ 4.7). Else `Htu`.
10. `iat`: `|now - iat| <= iat_window_secs + leeway_secs`. Else `Iat`.
11. `ath` equals base64url, with no padding, of SHA-256 over the ASCII bytes of the access
    token. Else `Ath`.
12. The RFC 7638 thumbprint of `jwk` equals `jkt`. The thumbprint uses the required members
    only, in lexicographic order, with no whitespace: `{"crv","kty","x","y"}` for EC and
    `{"e","kty","n"}` for RSA. SHA-256, then base64url with no padding. Else `Thumbprint`.
13. Replay: `ReplayStore::check_and_record(jkt, jti, iat + iat_window_secs + leeway_secs)`.
    `Replayed` and `Full` give `Replayed` and `StoreFull`.

The replay step is last, so a proof that fails another check does not use a `jti`.

A proof that has `nonce` is accepted, and IAM ignores the claim. IAM sends no nonce (§ 2).

### 4.5 The replay store (`adapters/dpop_replay.rs`, new)

- Port: `ReplayStore::check_and_record(jkt, jti, expires_at) -> Fresh | Replayed | Full`.
- Adapter: `InMemoryReplayStore`, a `Mutex<HashMap<(Jkt, String), Instant>>` plus a count for
  each `jkt`. This is the same pattern as the other in-memory stores in IAM.
- An entry that has expired is the same as no entry.
- When the store holds `replay_capacity` entries, or the key holds `per_key_quota` entries,
  the store first removes all expired entries. If it is still full, it returns `Full` (D10).
- A restart clears the store (residual R1).

### 4.6 Header parsing and the request target

- `bearer_from_headers` becomes `credentials_from_headers(headers) -> Option<Credentials>`.
  `Credentials` is `Bearer(token)` or `Dpop { token, proof: ProofHeader }`. `ProofHeader` is
  `One(String)`, `Missing` or `Invalid`. The scheme match is ASCII case-insensitive, as today.
- The proof is the value of the `DPoP` header. A missing header gives `Missing`, and the
  verifier refuses it with `ProofDefect::Missing`. Two or more `DPoP` headers, or a value that
  is not visible ASCII, give `Invalid`, and the verifier refuses it with
  `ProofDefect::Malformed`. The token is validated first in both cases (§ 4.6, last point).
- When DPoP is off, the `Dpop` credentials are refused exactly as the `None` case is today:
  `InvalidToken(Malformed)` with the same body and the same `Bearer` challenge (D11).
- An API key on the `DPoP` scheme (the token starts with `api_key_prefix`) is refused as
  `InvalidToken(Malformed)`. API keys are not bound to a key.
- On the `Bearer` scheme, a `DPoP` header is ignored. An unbound token is accepted as today. A
  bound token is refused by step 7 (D8).
- HTTP request target: `htm` is the request method. The URL is each configured origin plus the
  request path, as received, with no query (§ 4.7).
- gRPC request target: `htm` is `POST`. The URL is each configured origin plus the gRPC path
  `/<package.Service>/<Method>` (D6).
- `AuthenticateToken::resolve` and `introspect` take the credentials and an optional
  `RequestTarget` plus the origin list. The flow is: validate the token with the scheme, then
  call the verifier with `key_binding`, then provision (resolve only).

### 4.7 `htu` comparison

RFC 9449 § 4.3 point 9 and RFC 3986 § 6.2.2, § 6.2.3:

- Parse `htu` as an absolute URL. A parse failure gives `Htu`.
- Remove the query and the fragment from `htu`.
- Compare the scheme and the host without case. Remove a default port (`443` for `https`, `80`
  for `http`) on both sides.
- Compare the path exactly, as bytes. IAM does not decode percent-encoding and does not remove
  dot segments. An empty path is `/`.
- The request matches if `htu` equals one configured origin plus the request path.

For `Introspect`, the `DpopContext.url` must itself have an origin in `forwarded_origins`, and
the proof `htu` must match `DpopContext.url` by the rules above. A `url` whose origin is not
listed gives `Htu`.

### 4.8 `Introspect`

- gRPC `AuthnService.Introspect` and HTTP `POST /v1/authn/introspect` read the optional `dpop`
  context. The HTTP body field is `dpop: { proof, method, url }`.
- `dpop` absent: the `Bearer` scheme (today's behaviour, D8).
- `dpop` present, DPoP on: the `Dpop` scheme, checked against `forwarded_origins` (D7).
- `dpop` present, DPoP off: `InvalidToken(Malformed)`, as for a direct request (D11).
- `dpop` present with an empty `proof`: `ProofDefect::Missing`.
- `Introspect` stays exempt from the auth middleware. The caller is trusted to give the client's
  method and URL. The proof still needs the client's private key (residual R5).

### 4.9 Configuration (`config.rs`)

```toml
[authn.dpop]
enabled = false
origins = []
forwarded_origins = []
iat_window_secs = 60
replay_capacity = 100000
per_key_quota = 1000
```

- `DpopConfig` with `#[serde(default)]` and a `Default` implementation (the `MigrationConfig`
  pattern). Env names: `IAM_AUTHN__DPOP__ENABLED`, `IAM_AUTHN__DPOP__ORIGINS`,
  `IAM_AUTHN__DPOP__FORWARDED_ORIGINS`, and the same for the numbers.
- `validate()` refuses to boot when:
  - `enabled` is true and `origins` is empty;
  - an origin has a path other than `/`, a query, a fragment or user info;
  - an origin has a scheme other than `https`, except `http` on a loopback host;
  - a number is 0, or `iat_window_secs` is more than 300.
- `forwarded_origins` may be empty. Then every `Introspect` with a `dpop` context gets `Htu`.
- The test helpers `test_config` and `test_config_with` get the default `DpopConfig`.

### 4.10 Chart, docs and the follow-up issue

- Chart values `iam.dpop.enabled` (default `false`), `iam.dpop.origins`,
  `iam.dpop.forwardedOrigins`. `_iam-backend.tpl` projects them to env only when `enabled`
  is true. With the defaults, the rendered output does not change. Add one golden case with
  DPoP on. Add a refusal case for `enabled: true` with no origins, if the chart already
  refuses other bad IAM values that way.
- The comment at the IAM `replicas: 1` pin gets one more reason: the in-memory DPoP replay
  store (D3). It names SMA-700.
- `docs/ops/RUNBOOK-chart.md` (the SMA-690 text at about lines 230-240): DPoP-bound tokens
  work on IAM's own API when `iam.dpop.enabled` is true and the origins are correct. They do
  not work through the gateway until the follow-up issue merges. The `Bearer` scheme still
  refuses a bound token.
- `rs/crates/services/paigasus-iam/CHANGELOG.md`: one entry.
- A follow-up Linear issue (project Paigasus Polyglot, milestone IAM Gaps, priority Low, labels
  `area:gateway` or the nearest gateway area label, and `Feature`, related to SMA-700): "The
  gateway forwards the DPoP context to IAM". It must state: the gateway's own public origin
  config, `credentials_from_headers` parity in the gateway parser
  (`gateway/src/adapters/http/auth.rs:399`), sending `DpopContext`, and the `WWW-Authenticate`
  challenge on the gateway. Record its key in the PR body and in § 9.

### 4.11 Errors, challenges and logs

| Case | HTTP | gRPC |
|---|---|---|
| A token defect, DPoP off | 401, `invalid-token`, `WWW-Authenticate: Bearer error="invalid_token"` (no change) | `Unauthenticated`, `INVALID_TOKEN` (no change) |
| A token defect, DPoP on | 401, `invalid-token`, the `Bearer` challenge plus `WWW-Authenticate: DPoP error="invalid_token", algs="ES256 RS256"` | `Unauthenticated`, `INVALID_TOKEN` |
| A proof defect | 401, `invalid-dpop-proof`, `WWW-Authenticate: DPoP error="invalid_dpop_proof", algs="ES256 RS256"` | `Unauthenticated`, `INVALID_DPOP_PROOF` |
| No `Authorization` header, DPoP on | 401, a bare `Bearer` challenge plus `DPoP algs="ES256 RS256"` | no change |

- The two challenges are two separate `WWW-Authenticate` header lines.
- The `paigasus-retryable` header is the same as for `invalid-token` (not retryable).
- gRPC sends no challenge metadata, as today.
- `Introspect` returns the same codes. It is not an HTTP resource of the client, so the HTTP
  twin sends no `DPoP` challenge.
- Logs: each refusal writes one `info` line through `log_refusal` and its rate limit. The
  message is static: "refused a DPoP proof". The line has the issuer and the static defect name
  (`htu`, `replayed`, and so on). It never contains the proof, the token, the `jti`, the `jkt`
  or the URL.
- `StoreFull` also writes one `warn` line, rate-limited, because it means the limits are too
  low or IAM is under attack.

## 5. Tests

### 5.1 Unit tests

- **Verifier** (`application/dpop.rs`): one valid ES256 proof and one valid RS256 proof pass.
  Then one test for each check in § 4.4. Each test changes only one field of a valid proof.
  Extra cases: `alg: none`; `alg: HS256`; a `jwk` with `d`; an RSA key under 2048 bits; a
  P-384 key with `ES256`; `iat` at the window edge (pass) and one second over (fail); `ath` of
  a different token; a thumbprint of a different key; a valid proof sent twice; a proof with a
  `nonce` claim (pass).
- **Thumbprint:** the RFC 7638 § 3.1 example key gives the RFC's published thumbprint.
- **`htu`:** default port, host case, query and fragment removed, a path that differs only by
  percent-encoding (fail), a trailing slash (fail), an origin not in the list (fail).
- **Replay store:** fresh, replayed, expired then fresh, the key quota, the global cap, `Full`
  after expired entries are removed and the store is still full.
- **Header parser:** scheme case; two `DPoP` headers; a missing header; a non-ASCII value; the
  `Bearer` scheme with a `DPoP` header.
- **Validator:** the `Dpop` scheme with `cnf.jkt` (pass, `key_binding` set); with `cnf` but no
  `jkt`; with `typ: DPoP` only; with no binding. The `Bearer` scheme cases from SMA-690 stay
  as they are.
- **Config:** each `validate()` refusal in § 4.9, and the env names through `figment::Jail`.
- **Wire mapping:** each row of the table in § 4.11, for HTTP and gRPC.

### 5.2 Integration tests

Extend `MockIdp` so it can mint a token with `cnf.jkt` and `typ: DPoP`. Add a proof helper next
to it (the shape of `keycloak_e2e.rs:358`).

- `tests/http_authn.rs`: DPoP on, a valid proof on a protected route gives 200. The same proof
  again gives 401 `invalid-dpop-proof`. A proof for another path gives 401. DPoP off: the
  `DPoP` scheme gives a response byte-identical to today's `None` case. The `Bearer` scheme
  with a bound token gives 401 `invalid-token` with DPoP on.
- `tests/grpc_authn.rs`: the same cases on one gRPC method, with the gRPC `htu` (D6).
- `Introspect`, gRPC and HTTP: a `dpop` context checked against `forwarded_origins` passes. A
  proof whose `htu` is on an IAM origin, not a forwarded origin, fails (D7). No `dpop` context
  with a bound token fails as today.
- `tests/keycloak_e2e.rs`: with DPoP on, a real Keycloak DPoP-bound token and a new proof are
  accepted by `resolve` with the `Dpop` scheme. The existing SMA-690 assertion (the `Bearer`
  scheme refuses the token) stays.

### 5.3 Proof that the tests bite

For each of the 13 checks in § 4.4, and for D8 and D11: remove the check (make it always pass),
run the IAM tests with `--no-fail-fast`, and record which test fails. The mutation must
compile. If a mutation fails no test, add a test, then run the full battery again. Record the
results in `docs/superpowers/plans/2026-10-04-sma-700-mutation-results.md`.

### 5.4 Acceptance criteria

| Criterion | Evidence |
|---|---|
| 1. With DPoP on, a valid DPoP request to IAM's HTTP and gRPC API is accepted. | § 5.2 http, grpc, keycloak |
| 2. Each RFC 9449 § 4.3 check refuses a proof that fails it. | § 5.1 verifier; § 5.3 |
| 3. A replayed proof is refused. | § 5.1 replay store; § 5.2 http |
| 4. The `Bearer` scheme still refuses a bound token (SMA-690 D9). | § 5.1 validator; § 5.2 |
| 5. With DPoP off, every response is the same as before. | § 5.2 byte-identical case; § 5.3 D11 mutation |
| 6. `IntrospectRequest` carries the DPoP context, and IAM checks it against `forwarded_origins`. | § 4.1; § 5.2 Introspect |
| 7. A `WWW-Authenticate: DPoP` challenge is sent where § 4.11 says. | § 5.1 wire mapping |

## 6. Files

- `contracts/proto/paigasus/iam/v1/iam.proto`, `contracts/proto/paigasus/common/v1/error.proto`,
  and the generated output.
- `rs/crates/libs/paigasus-iam-core/src/authn.rs` (scheme, `Jkt`, `ProofDefect`, error, ports).
- `rs/crates/services/paigasus-iam/src/`: `adapters/auth.rs`, `adapters/http/auth_middleware.rs`,
  `adapters/http/authn.rs`, `adapters/grpc/authn.rs`, `adapters/grpc/convert.rs`,
  `adapters/oidc/validator.rs`, `application/authenticate_token.rs`, `application/dpop.rs`
  (new), `adapters/dpop_replay.rs` (new), `config.rs`, the app state wiring.
- `rs/crates/services/paigasus-iam/tests/`: `support/mod.rs`, `http_authn.rs`, `grpc_authn.rs`,
  `keycloak_e2e.rs`.
- `rs/crates/services/paigasus-gateway/`: only the changes that the new proto field forces
  (a struct literal for `IntrospectRequest` gets `dpop: None`).
- `charts/paigasus/`: `values.yaml`, `templates/_iam-backend.tpl`,
  `templates/backend-deployment.yaml` (comment), golden files, `README.md`.
- `docs/ops/RUNBOOK-chart.md`, `rs/crates/services/paigasus-iam/CHANGELOG.md`.
- New crates: none. `sha2` and `base64` are already IAM dependencies. `jsonwebtoken` verifies
  the proof.

## 7. Rollout and effect on operators

- The default is off. With the default, every IAM response is the same as before (D11). The
  chart renders the same output.
- An operator who turns DPoP on must set `iam.dpop.origins` to the public origins at which
  clients reach IAM. A wrong origin refuses DPoP requests (`Htu`). It does not accept a wrong
  request.
- A DPoP client still cannot use the gateway until the follow-up issue merges.
- More than one IAM replica with DPoP on is not safe (R2). The chart pin prevents it.

## 8. Verification before merge

- The full gate graph from the root `CLAUDE.md` (`moon ci … --base origin/main
  --include-relations`), with the bash rules for this machine.
- `contracts:generate` drift, `:breaking`, `:error-code-single-site`, `:helm-render`.
- The mutation battery (§ 5.3).
- `keycloak_e2e.rs` locally with Docker.

## 9. Residuals

| # | Residual | Effect | Owner |
|---|---|---|---|
| R1 | A restart clears the replay store. | A proof used before the restart can be used again until its `iat` window ends (at most `iat_window_secs + leeway_secs`, about two minutes with the defaults). The attacker still needs a captured proof for the same method and URL. | Accepted. |
| R2 | The in-memory store needs one IAM replica. | Two replicas would accept one proof once on each. The chart pin (SMA-559) prevents this today. A change to more replicas must add a shared store first. | The comment at the pin (§ 4.10). |
| R3 | The gateway does not forward the DPoP context. | DPoP clients cannot use the gateway. | Follow-up issue (§ 4.10). Key: to be recorded. |
| R4 | No server nonce (RFC 9449 § 8). | A client can make proofs in advance, within the `iat` window. | Accepted. |
| R5 | `Introspect` trusts its caller for the method and the URL. | A caller that can reach IAM's internal gRPC port can claim any URL with a forwarded origin. It still needs a proof signed by the client's private key for that URL. | Accepted. `Introspect` is already unauthenticated in-cluster. |

## 10. Open questions

None. Sven decided the scope, the replay store, the switch and the ADR on 2026-10-04.

## 11. Spec challenge

To be filled after the adversarial challenge (pipeline Stage 2).
