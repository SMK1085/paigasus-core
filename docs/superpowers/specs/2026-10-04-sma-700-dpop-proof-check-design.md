# SMA-700: The gateway accepts a DPoP-bound token, and IAM checks the proof (RFC 9449)

- Linear: SMA-700 (decision D9 of SMA-690,
  `2026-09-26-sma-690-refuse-sender-constrained-token-design.md`)
- Status: approved at GATE 1 (2026-10-04). The two spec challenges are in § 11.
- Path: architectural (a new authentication scheme, a proto contract change, new ports, a
  change in two services)
- ADR: ADR-0026 (Notion, Accepted 2026-10-04)
- History: a draft of 2026-09-27 was lost before it was committed. Draft 1 of this spec
  (commit `1016c0b3`) put the check on IAM's own API. Challenge 1 showed that the chart gives
  no route to IAM's own API. Sven changed the scope to the gateway path on 2026-10-04 (D1, D14).
  Draft 2 (commit `55adc5d4`) had that scope. Draft 3 adds the findings of challenge 2.

## 1. Problem

SMA-690 made IAM refuse a sender-constrained access token: a token with a `cnf` claim, or with
a payload `typ` of `DPoP` in any letter case (`validator.rs`, step 7,
`sender_constraint_marker`). IAM cannot check the key binding, so a client that uses DPoP
(RFC 9449) cannot use Paigasus.

Keycloak 26.4 binds an access token when the client sends a `DPoP` header to the token
endpoint. The realm needs no setting for this (SMA-690 measurement M2). The bound token has
`typ: DPoP` and `cnf.jkt`. `cnf.jkt` is the RFC 7638 thumbprint of the proof key.

Measured facts about the request path (2026-10-04):

- The chart routes external traffic only to the consoles
  (`charts/paigasus/templates/ingress.yaml:41-45`, `httproute.yaml`). The IAM backend Service
  is `ClusterIP` (`backend-service.yaml:16`). No external client can reach IAM's own API with
  the shipped chart.
- The chart does not deploy the Rust gateway (`_helpers.tpl:100-101`). The operator deploys it
  and gives its URL (`values.yaml:111`). An API client reaches Paigasus through the gateway.
- The gateway has two protected HTTP routes (`paigasus-gateway/src/adapters/http/mod.rs:87-120`):
  `POST /v1/chat/completions` (`require_iam_auth`) and `GET /v1/service-info`
  (`require_authenticated`). It has no gRPC server.
- `require_iam_auth` (`adapters/http/auth.rs:72-146`) does this: parse `Bearer`
  (`bearer`, `auth.rs:402`); try `IntrospectApiKey`; then `Introspect`; resolve the org; then
  `IsAuthorized` with action `InvokeModel`.
- `require_authenticated` accepts an `identity-not-provisioned` answer from `Introspect`
  (`auth.rs:302-305`).
- The gateway authenticates the `IsAuthorized` call with the client token as
  `authorization: Bearer <token>` metadata (`adapters/iam/client.rs:159-168`). `IsAuthorized` is
  not exempt from IAM's `AuthEnforce` (`paigasus-iam/src/adapters/grpc/authn.rs:181-232`), which
  calls `resolve(.., Provisioning::Enabled)` and the bootstrap seeder. So a bound token passes
  `Introspect` and then fails at `IsAuthorized` (SMA-690 D8).
- `AuthenticateToken::resolve` validates the token, then looks up the identity, then
  provisions (`application/authenticate_token.rs:188-202`). `introspect` uses `resolve` with
  `Provisioning::Disabled` (`:267-270`).
- The gateway sets no `WWW-Authenticate` header. It maps every `Unauthenticated` from IAM to
  401 `invalid-api-key` (`auth.rs:412-450`, `error.rs:172`). It has no introspect cache
  (`auth.rs:23`). Its middleware state is only `Arc<dyn Iam>` (`http/mod.rs:91`, `:103`).
- An IAM `Unavailable` becomes a gateway 503 and the metric label `unavailable`, which fires the
  critical alert `GatewayIamDependencyUnavailable`
  (`ops/observability/prometheus/rules/gateway.rules.yml:10-14`).
- `IntrospectRequest` has only `string token = 1` (`contracts/proto/paigasus/iam/v1/iam.proto:242`).
- IAM has a `Clock` port with `SystemClock` and `FixedClock`
  (`paigasus-iam-core/src/ports.rs:338`, `application/fakes.rs`).
- The chart pins IAM to one replica with `maxSurge: 0` (`backend-deployment.yaml:22-35`,
  SMA-559). A config that IAM refuses at boot therefore leaves no IAM pod.

## 2. Goal and non-goals

Goal: an operator can turn on DPoP in IAM and in the gateway. Then a client that holds a
DPoP-bound token and its private key can call the gateway's protected routes with
`Authorization: DPoP <token>` and a valid `DPoP` proof. A bound token without a valid proof is
refused.

Precondition: the identity of the token is provisioned in IAM. This is the same rule as for a
`Bearer` token on the chat route today. A DPoP-only client cannot provision itself, because
IAM's own API refuses the `DPoP` scheme. The operator provisions it (ADR-0024 linking), or the
user logs in to a console once with the same subject.

Non-goals:

- The `DPoP` scheme on IAM's own HTTP and gRPC API. IAM keeps refusing it there, except for the
  one `IsAuthorized` follow-up in § 4.8.
- Server-provided nonces (RFC 9449 § 8) and the `DPoP-Nonce` header.
- mTLS-bound tokens (RFC 8705). They stay refused (SMA-690 D4), also on the `DPoP` scheme.
- A shared (Redis) replay store.
- Browser DPoP clients. Neither service has a CORS layer.
- A record in the audit log or in `AuthContext` that a request was DPoP-bound.
- A DPoP capability in the gateway's service-info descriptor.
- A `Bearer` challenge on the gateway. The gateway sends none today.
- A new metric.

## 3. Decisions

| # | Decision | Reason |
|---|---|---|
| D1 | Scope: IAM checks a DPoP context forwarded by the gateway in `Introspect`, and the gateway sends it. IAM's own API keeps refusing the `DPoP` scheme. | Sven chose this on 2026-10-04, after challenge 1 showed that no client can reach IAM's own API (§ 1). |
| D2 | The stateless proof checks are a core port, `DpopProofChecker`. Its adapter is in `adapters/oidc/dpop.rs`. An application service, `DpopProofVerifier`, adds the request checks, the time check and the replay check. | Sven approved approach B. JOSE code belongs in an adapter (`paigasus-iam-core/src/authn.rs:3-4`). |
| D3 | The replay store is a port, `ReplayStore`, with one in-memory adapter. One instance is shared (one `Arc` in `AppState`). The port is synchronous. | Sven chose this on 2026-10-04. It is correct because the chart pins IAM to one replica. A Redis adapter would need an async port; that change comes with the adapter. |
| D4 | DPoP is off by default, in IAM and in the gateway. Each has its own switch. | Sven chose a global switch. Existing deployments do not change. |
| D5 | IAM checks `htu` against configured base URLs (`forwarded_base_urls`). The gateway sends the method and the path that it received. Neither service builds a URL from `Host`, `Forwarded` or `X-Forwarded-*`. | A client or a proxy controls those headers. The gateway does not know its public origin (§ 1). |
| D6 | A base URL can have a path prefix. | A proxy in front of the gateway can remove a prefix. The client then signs `https://h/prefix/v1/...` and the gateway sees `/v1/...`. |
| D7 | On the `Bearer` scheme, both SMA-690 markers stay refused, also when DPoP is on. | SMA-690 D9. A bound token is never a bearer token. |
| D8 | Allowed proof algorithms: ES256 and RS256. | The same list as `ALLOWED_ALGORITHMS` for access tokens. |
| D9 | Every proof defect maps to `invalid_dpop_proof`, also `ath` and the thumbprint. | RFC 9449 § 4.3 lists `ath` and the key binding as checks of the proof. One code is simpler for a client. |
| D10 | Replay-store limits: a quota hit (per key or per subject) gives a 429 with `Retry-After` at the gateway. A full global capacity gives a retryable 503. | A quota hit is a rate limit of one client. It must not look like an IAM outage and fire the critical alert (§ 1). A full global capacity is a real capacity problem, so the alert is correct for it. Fail open would allow replay. |
| D11 | When DPoP is off, the responses of both services do not change. | Existing deployments must see no change (§ 7). |
| D12 | No new metric. Each refusal writes one rate-limited `info` line. A full store writes a `warn` line with the store occupancy. | The `observability-drift` gate stays unchanged. The occupancy lets an operator size the store. |
| D13 | ADR-0026 records the protocol decision. | Sven asked for it on 2026-10-04. |
| D14 | `IsAuthorized` accepts a one-time follow-up ticket. `Introspect` records the accepted proof. The gateway then sends the same token and proof on `IsAuthorized`. IAM accepts this once, only for `IsAuthorized`, only as a self-query, and only before the ticket deadline. | Sven chose this on 2026-10-04. The client sends one proof for one request, and the gateway makes two IAM calls for it (§ 1). |
| D15 | On the `DPoP` scheme, the gateway skips the API-key leg. | API keys are not bound to a key. The proof is then sent to IAM on the token leg only. |
| D16 | The proof `typ` is `dpop+jwt` or `application/dpop+jwt`, ASCII case-insensitive. | RFC 7515 § 4.1.9. The validator already accepts both forms for `logout+jwt` (`validator.rs:53`). |
| D17 | The `jwk` members `alg`, `use` and `key_ops` are ignored. | RFC 9449 does not require them. The header `alg` and the key type are checked. |
| D18 | On the `DPoP` scheme, the proof check runs before any identity lookup, provisioning, seeding or API-key branch. | Challenge 2: a check after the lookup lets a stolen bound token pass `service-info` with no proof, because the gateway accepts `identity-not-provisioned` there (§ 1). |
| D19 | The new error reason is in the shared range: `ERROR_REASON_INVALID_DPOP_PROOF = 909`. | IAM and the gateway both emit it. The numbering rule (`error.proto:27-34`, SMA-498 D3) puts such a code in 900-999. The registry is append-only, so a wrong number is permanent. |
| D20 | The proof `iat` window is its own setting. It does not add `authn.leeway_secs`. | Challenge 2: the token-expiry skew must not change the replay window and the store memory. |

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

The comment on `rpc IsAuthorized` documents the follow-up (§ 4.8): the metadata
`authorization: DPoP <token>` plus `dpop: <proof>`.

`contracts/proto/paigasus/common/v1/error.proto`:

```proto
// In the shared range (900-999). Emitted by IAM and by the gateway.
// "invalid-dpop-proof" — the DPoP proof is missing, malformed, or does not match the
// request or the token.
ERROR_REASON_INVALID_DPOP_PROOF = 909;
```

and in the IAM authn range:

```proto
// "dpop-quota-exceeded" — the caller sent more DPoP proofs than its quota allows inside the
// proof window. Retry after the RetryInfo delay. The gateway answers with its own
// "rate-limited".
ERROR_REASON_DPOP_QUOTA_EXCEEDED = 46;
```

Run `contracts:generate` and commit the generated Rust, Python and TypeScript output. The
`:breaking` gate must stay green: all changes only add.

Register both codes as `ci/error-registry/README.md` requires (`ci/error-registry/check.py`,
`MANIFEST`). The new enum values also change:

- `ts/packages/paigasus-sdk/src/errors/presentation.ts:20` (a total `Record`, so
  `:typecheck` fails without the entries);
- the count 67 → 69 in `ts/packages/paigasus-sdk/tests/presentation.test.ts:13`,
  `ts/packages/paigasus-proto/src/error.test.ts:60` and
  `rs/crates/libs/paigasus-proto/src/error.rs` (with its `EXPECTED_REASONS` list).

### 4.2 `paigasus-iam-core`

In `authn.rs`:

- `TokenScheme { Bearer, Dpop }`.
- `Jkt`, a newtype over the base64url thumbprint string.
- `ValidatedClaims` gets `key_binding: Option<Jkt>`. It is `Some` only on the `Dpop` scheme.
- A new `TokenDefect::NotKeyBound`: the `Dpop` scheme with a token that has no `cnf.jkt`, or
  that has an mTLS binding (`x5t#S256`).
- A new `AuthnError::InvalidDpopProof(ProofDefect)`. `ProofDefect` is `Copy + Eq + Hash`, with
  one variant for each check: `Missing`, `Malformed`, `Typ`, `Alg`, `Jwk`, `Signature`, `Htm`,
  `Htu`, `Iat`, `Ath`, `Thumbprint`, `Replayed`, `FollowUp`.
- A new `AuthnError::DpopQuotaExceeded { retry_after_secs: u32 }`.
- A global-capacity hit uses the existing `AuthnError::Unavailable`.

In `ports.rs`:

- The authenticator port takes the scheme: `authenticate(token, scheme)`. Every caller and
  every test double changes. `paigasus-iam-core` has `publish = false`.
- `DpopProofChecker`:
  - `check(proof, token, jkt) -> Result<ProofClaims, ProofDefect>` (checks 1-9 of § 4.4).
  - `follow_up_claims(proof) -> Result<FollowUpClaims, ProofDefect>`: reads `jti` and `ath`
    from the payload with no signature check (§ 4.8).
  - `ProofClaims` holds `jti`, `iat`, `htm` and `htu` as a string. The `url` parse is in the
    application service, so `paigasus-iam-core` needs no `url` dependency.
- `ReplayStore` (§ 4.5).

### 4.3 The validator (`adapters/oidc/validator.rs`)

Steps 1 to 6b do not change. Step 7 depends on the scheme:

- `Bearer`: no change. A token with `cnf` or `typ: DPoP` is refused with
  `TokenDefect::SenderConstrained` (D7).
- `Dpop`: the token must have `cnf` as a JSON object with a `jkt` member that is a non-empty
  string, and no `x5t#S256` member. Else the token is refused with `TokenDefect::NotKeyBound`.
  Its log line has its own static message: "refused a DPoP request: the token is not bound to a
  key, or is also bound to a certificate". On success, `key_binding` is `Some(jkt)`.

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
6. Signature: build the key from the validated members (`DecodingKey::from_ec_components` or
   `from_rsa_components`) and call `jsonwebtoken::crypto::verify(signature, signing_input,
   &key, alg)` on the raw parts. Else `Signature`. This avoids a second header parse and the
   `Validation` flags.
7. Claims: `jti` is a string of 1 to 256 bytes; `htm` and `htu` are strings; `iat` is a JSON
   integer in the `i64` range; `ath` is a string. A missing or wrong-type claim gives
   `Malformed`. A fractional `iat` gives `Malformed`.
8. `ath` equals base64url, with no padding, of SHA-256 over the ASCII bytes of the access
   token. Else `Ath`.
9. The RFC 7638 thumbprint equals the `jkt` of the token. It is computed by hand from the
   members that check 5 validated (`{"crv","kty","x","y"}` for EC, `{"e","kty","n"}` for RSA,
   in that order, no whitespace; the values are strict base64url, so they need no escapes).
   SHA-256, then base64url with no padding. Else `Thumbprint`.

`follow_up_claims` runs checks 1, 2 and the `jti` and `ath` parts of check 7 only. It reuses
the check-8 code for `ath` in the verifier.

**Application, `application/dpop.rs` (`DpopProofVerifier`), checks 10-13:**

10. `htm` equals the forwarded method, exactly. Else `Htm`.
11. `htu` matches (§ 4.6). Else `Htu`.
12. `iat`: with `now` from `Clock`, `|now − iat| ≤ iat_window_secs`, with checked arithmetic.
    An overflow gives `Iat`. Else `Iat`.
13. Replay: `ReplayStore::record(...)` (§ 4.5). This is last, so a proof that fails another
    check does not use a `jti`.

A `nonce` claim is accepted and ignored.

The verifier owns the base-URL list. An adapter cannot pass another list. The verifier gets
the issuer, and it owns its own `LogRateLimiter<ProofDefect>`.

### 4.5 The replay store (`adapters/dpop_replay.rs`)

Port:

```rust
pub trait ReplayStore: Send + Sync {
    fn record(&self, entry: NewProof, now: i64) -> RecordOutcome;
    // Fresh | Replayed | QuotaExceeded { retry_after_secs } | CapacityFull
    fn redeem_follow_up(&self, key: ProofKey, proof_digest: [u8; 32], now: i64) -> RedeemOutcome;
    // Redeemed | Refused
}
```

- `ProofKey` is the first 16 bytes of `blake3(jkt || 0x00 || jti)`. `blake3` is already an IAM
  dependency.
- `NewProof` holds the key, a 16-byte hash of the subject (`issuer || 0x00 || sub`), a 16-byte
  hash of the `jkt`, `expires_at`, `follow_up_deadline` and the follow-up digest (`blake3` of
  the whole proof string, as received, with no trim).
- `expires_at = iat + iat_window_secs`. `follow_up_deadline = max(expires_at, now + 30)`. The
  entry stays until the later of the two. So the follow-up always has at least 30 s, also for a
  client whose clock is behind.
- An entry is live while `now <= expires_at`, and its ticket while `now <= follow_up_deadline`.
  Time comes only from `Clock`, through the `now` argument.
- An expiry index (`BTreeMap<i64, Vec<ProofKey>>`) holds the keys by removal second. Each call
  removes only the expired heads. No call scans the whole map.
- Quotas: `replay_capacity` (global), `per_key_quota` (each `jkt` hash), `per_subject_quota`
  (each subject hash). The counts go down when an entry is removed. A count at zero is removed.
- `record` returns `QuotaExceeded` when a key or subject quota is reached after the expired
  heads are removed. `retry_after_secs` is the time until the oldest entry of that key or
  subject is removed. It returns `CapacityFull` when the global capacity is reached.
- `redeem_follow_up` returns `Redeemed` only if the ticket is live, not used, and the digest
  matches. It then marks the ticket as used. A digest mismatch does not use the ticket.
- One `Mutex` protects the store. A restart clears it (R1).
- Memory budget (an estimate; the plan measures it): about 150 to 250 bytes for each entry with
  the map, the index and the count maps, so about 30 to 50 MB at the default capacity of
  200 000. The chart sets no IAM memory limit.

### 4.6 `htu` comparison

- Each `forwarded_base_urls` entry is parsed at boot into scheme, host, port and a path
  prefix. The prefix has no trailing `/`; an empty prefix means the root.
- The forwarded `path` is checked first (§ 4.8). Then the expected URL is
  `scheme://host[:port]` + prefix + `path`, parsed with the `url` crate. After the parse, the
  scheme, host and port must equal those of the entry. Else `Htu`.
- The proof `htu` and the expected URL are compared after both pass the same `url` parser:
  the scheme, the host and the port with a default port removed, and `Url::path()`. The query
  and the fragment of `htu` are ignored. `Url::path()` keeps percent-encoding, so a path that
  differs only by percent-encoding does not match. The parser removes dot segments on both
  sides. This is a deliberate deviation from the full RFC 3986 § 6.2.2 normalisation.
- The proof matches if it matches one entry.

### 4.7 `AuthenticateToken`

A new method `resolve_dpop(token, input, provisioning)`. `input` is `Introspect(DpopContext)` or
`FollowUp(proof)`. The order is fixed (D18):

1. `authenticate(token, TokenScheme::Dpop)` → `ValidatedClaims` with `key_binding`.
2. `Introspect`: `DpopProofVerifier::verify(...)`. `FollowUp`: the ticket redeem (§ 4.8).
3. Only then: the identity lookup, provisioning and anything after it, as in `resolve`.

No identity lookup, provisioning, seeding or API-key branch runs before step 2. `introspect`
calls `resolve_dpop` with `Provisioning::Disabled` when a `dpop` context is present.

### 4.8 IAM: `Introspect` and the `IsAuthorized` follow-up

`Introspect` (gRPC; the HTTP twin `POST /v1/authn/introspect` does not change):

- `dpop` absent: the `Bearer` scheme, as today (D7).
- `dpop` present and DPoP off: `InvalidToken(Malformed)` (D11).
- `dpop` present and DPoP on:
  - a `proof`, `method` or `path` over its size in § 4.1 gives
    `InvalidDpopProof(Malformed)`; an empty `proof` gives `InvalidDpopProof(Missing)`;
  - a `path` that does not start with `/`, or that contains `?`, `#`, `\` or a control
    character, gives `InvalidDpopProof(Malformed)`;
  - then `resolve_dpop` (§ 4.7).

`IsAuthorized` follow-up, in `AuthEnforce` (`adapters/grpc/authn.rs`):

- The scheme parser accepts `DPoP` only when DPoP is on and the path equals
  `format!("/{}/IsAuthorized", AuthorizationService SERVICE_NAME)`. tonic generates only the
  service name, so this string is built once and pinned by a unit test. On every other path,
  and when DPoP is off, the `DPoP` scheme is refused as `InvalidToken(Malformed)`, as today.
  IAM's HTTP routes keep refusing it too (`bearer_from_headers` is shared,
  `adapters/auth.rs:33`).
- The request must have exactly one `dpop` metadata entry. Else
  `InvalidDpopProof(Missing)` or `InvalidDpopProof(Malformed)`.
- `AuthEnforce` calls `resolve_dpop(token, FollowUp(proof), Provisioning::Disabled)`. The
  identity exists, because `Introspect` required it on the chat path. The bootstrap seeder does
  not run before the redeem. The API-key branch does not run for the `DPoP` scheme.
- The redeem: `follow_up_claims(proof)` gives `jti` and `ath`. The verifier checks that `ath`
  matches the token. It calls `redeem_follow_up` with the key from the token `jkt` and `jti`, and
  the digest of the proof bytes as received. A refusal gives `InvalidDpopProof(FollowUp)`. The
  digest proves that the bytes are the same bytes that `Introspect` verified.
- On success, `AuthEnforce` inserts a `DpopFollowUp` request extension. `is_authorized`
  refuses the request with `InvalidDpopProof(FollowUp)` when the extension is present and
  `principal_prn` is not the caller (`grpc/authz.rs:99-110`). The gateway always self-queries,
  so this costs nothing.
- The IAM gRPC server sets `http2_max_header_list_size` to `max_token_bytes + 8192 + 4096`,
  so the follow-up metadata fits (the hyper default is 16 KiB).

Errors on gRPC:

| Error | Code | Reason | Message |
|---|---|---|---|
| `InvalidDpopProof(_)` | `Unauthenticated` | `INVALID_DPOP_PROOF` | "invalid DPoP proof" |
| `InvalidToken(NotKeyBound)` | `Unauthenticated` | `INVALID_TOKEN` | "invalid bearer token" (no change) |
| `DpopQuotaExceeded` | `ResourceExhausted` | `DPOP_QUOTA_EXCEEDED`, plus `google.rpc.RetryInfo` | "too many DPoP proofs" (retryable) |
| `Unavailable` (global capacity) | `Unavailable` | `AUTHN_UNAVAILABLE` | as today (retryable) |

The new `AuthnError` variants also go into the HTTP funnel
(`adapters/http/authn.rs:40-55`), the registry test there (`:191-203`), `all_authn_errors` and
`authn_retryable` in `adapters/retryable.rs`, and the table test in `convert.rs` (`:886-915`).
On HTTP they map to 401 `invalid-dpop-proof` and 429 `dpop-quota-exceeded`. No HTTP route
produces them today, but the exhaustive matches need an arm.

Logs: each refusal writes one `info` line through the verifier's `LogRateLimiter<ProofDefect>`.
The message is static: "refused a DPoP proof". The line has the issuer and the static defect
name. It never contains the proof, the token, the `jti`, the `jkt`, the path or the URL.
`QuotaExceeded` writes one rate-limited `info` line. `CapacityFull` writes one rate-limited
`warn` line with the entry count.

### 4.9 IAM configuration (`config.rs`)

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
  (the figment inline `[...]` form with quoted entries, as `IAM_AUTHN__ISSUERS` uses), and the
  same pattern for the numbers.
- `validate()` refuses to boot when:
  - `enabled` is true and `forwarded_base_urls` is empty;
  - an entry has a query, a fragment or user info;
  - an entry has a scheme other than `https`, except `http` on a loopback host (`localhost`,
    `127.0.0.0/8` or `::1`);
  - a number is 0;
  - `iat_window_secs` is more than 300;
  - `per_key_quota` or `per_subject_quota` is more than `replay_capacity`.
- An entry lives up to `2 × iat_window_secs` (a proof with `iat = now + window` expires at
  `now + 2 × window`). With the defaults, a key can make about `1000 / 120 ≈ 8` proofs a
  second without a quota hit, and a subject about 16. The runbook states this.
- `test_config`, `test_config_with`, and the `IamConfig`/`AuthnConfig` literals in
  `src/service_info.rs:169-187` and `tests/zitadel_e2e.rs:728-745` get the default
  `DpopConfig`. `iam.toml.example` gets the section.
- A config test parses the exact env form that the chart renders.

### 4.10 The gateway

**Config** (`config.rs`): `dpop: GatewayDpopConfig { enabled: bool }`, default false, env
`GATEWAY_DPOP__ENABLED`. The test constructor in `service_info.rs` and `gateway.toml.example`
change.

**State**: the middleware state becomes `AuthState { iam: Arc<dyn Iam>, dpop_enabled: bool }`
(`http/mod.rs:91`, `:103`, `main.rs:93`, and the test builders `build_app` and
`build_discovery_app`, `auth.rs:640-642`, `:693-697`).

**Parser** (`auth.rs:402`): `bearer` becomes `credentials(headers) -> Option<Credentials>`.
`Credentials` is `Bearer(token)` or `Dpop { token, proof: ProofHeader }`. `ProofHeader` is
`One(String)`, `Missing` or `Invalid` (two or more `DPoP` headers, or a value that is not
visible ASCII). When gateway DPoP is off, the `DPoP` scheme gives `None`, as today (D11). On the
`Bearer` scheme, a `DPoP` header is ignored.

**`Iam` trait** (`adapters/iam/client.rs:52-75`):

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
- `require_authenticated` keeps its rule for `identity-not-provisioned`. It is safe, because IAM
  answers `identity-not-provisioned` only after the proof check passed (D18).
- `Bearer` credentials: no change.

**Errors** (`error.rs`):

- `introspect_error` and `authz_error` read the IAM reason for `Unauthenticated` and
  `ResourceExhausted`. The reason read must not write the existing "PermissionDenied with no
  ErrorInfo" warning (`auth.rs:344`) for these codes.
  - `Unauthenticated` + `INVALID_DPOP_PROOF`: a new `GatewayError::InvalidDpopProof`, 401, type
    `invalid_request_error`, code `invalid-dpop-proof`, message "Invalid DPoP proof.".
  - `ResourceExhausted` + `DPOP_QUOTA_EXCEEDED`: the existing `RateLimited` (429,
    `rate-limited`) with `Retry-After` from `RetryInfo`. `iam_result` labels it `denied`, not
    `unavailable`.
  - Every other `Unauthenticated` gives `InvalidCredential`, as today. `Unavailable` gives
    `IamUnavailable` (503, retryable), as today.
- The challenge: `IntoResponse` does not change. The middleware appends the header after
  `err.into_response()`, through one function
  `dpop_challenge(dpop_enabled, scheme_used, &GatewayError) -> Option<HeaderValue>`. When
  gateway DPoP is on, every 401 gets one `WWW-Authenticate` line:
  - the client used `DPoP` and the error is `InvalidDpopProof`:
    `DPoP error="invalid_dpop_proof", algs="ES256 RS256"`;
  - the client used `DPoP` and the error is another 401: `DPoP error="invalid_token", algs="ES256 RS256"`;
  - the client used `Bearer` or sent no credential: `DPoP algs="ES256 RS256"` (no error
    attribute: RFC 6750 § 3 puts the error on the scheme the client used).
- When gateway DPoP is off, no `WWW-Authenticate` header is sent, as today (D11).

### 4.11 Chart and docs

- IAM values `zones.iam.backend.dpop.enabled` (default `false`) and
  `zones.iam.backend.dpop.forwardedBaseUrls`. `_iam-backend.tpl` projects them to env only
  when `enabled` is true, with `%q` quoting and a character rule, as
  `paigasus.iamIdTokenMarkerClaims` does (`_iam-backend.tpl:116-125`, `:153-155`).
- `paigasus.validateIamBackend` copies IAM's boot rules for the list (SMA-703 D5,
  `_iam-backend.tpl:127-136`), because a refused boot leaves no IAM pod (§ 1): `enabled` with an
  empty list; an entry that is not `https` or loopback `http`; an entry with a query, fragment
  or user info. Each rule gets a row in `tests/refusals.sh`.
- Add `IAM_AUTHN__DPOP__ENABLED` and `IAM_AUTHN__DPOP__FORWARDED_BASE_URLS` to
  `paigasus.iamReservedEnv` (`_iam-backend.tpl:16-18`). `tests/env.sh` row B6
  (`env.sh:572-573`, `:628-640`) turns DPoP on so the reserved list still equals the rendered
  names. Add a restart-scope row for the new values.
- With the defaults, the rendered output does not change. Add one golden case with DPoP on.
- The comment at the IAM `replicas: 1` pin gets one more reason: the in-memory replay store
  (D3, SMA-700). It is a `{{/* */}}` template comment, so the rendered output does not change.
- The gateway is not deployed by the chart, so the chart has no gateway DPoP value. The
  operator sets `GATEWAY_DPOP__ENABLED`.
- `docs/ops/RUNBOOK-chart.md` (the SMA-690 text at about lines 230-247; its "do not send DPoP"
  rule becomes conditional):
  - How to turn DPoP on: IAM first, then the gateway. `forwardedBaseUrls` = the public URLs at
    which clients reach the gateway, with any prefix that a proxy removes.
  - A client must make a new proof for each attempt, in a per-attempt hook. An SDK retry that
    sends the same headers again is refused as a replay.
  - The quota: about 8 proofs a second for each key and 16 for each subject with the defaults.
    A 429 means the client is over its quota.
  - The precondition in § 2: the identity must be provisioned.
  - The `Bearer` scheme still refuses a bound token. IAM's own API does not accept DPoP.
  - A 503 with a full replay store: read the `warn` line, then raise `replay_capacity`.
- `CHANGELOG.md` of `paigasus-iam` and `paigasus-gateway`: one entry each.
- ADR-0026 is updated to this scope.

## 5. Tests

### 5.1 Unit tests (each asserts the exact `ProofDefect`, `AuthnError` or `GatewayError`)

- **Proof checker:** a valid ES256 proof and a valid RS256 proof pass. One test for each of
  checks 1-9, which changes only that field. Extra cases: `alg: none`; `alg: HS256`; a `jwk`
  with `d`; an RSA `n` of 2047 and 4097 bits (synthetic values: check 5 refuses them before any
  signature, so no key generation is needed); `e` even; a P-384 key with `ES256`; `typ`
  `application/DPoP+JWT` (pass); a fractional `iat`; `ath` of another token; the thumbprint of
  another key; a `nonce` claim (pass).
- **Thumbprint:** the RFC 7638 § 3.1 example key gives the published thumbprint.
- **`follow_up_claims`:** reads `jti` and `ath`; refuses a proof that fails check 1 or 2.
- **Verifier:** checks 10-13 with `FixedClock`; `iat` at the edge (pass) and one second over
  (fail); `iat` near `i64::MAX` (`Iat`, no panic); `htm` `post` against `POST` (fail).
- **`htu`:** default port; host case; query and fragment ignored; a path prefix base URL; a
  path that differs by percent-encoding (fail); a trailing slash (fail); a base URL not in the
  list (fail).
- **Forwarded path:** `.evil.example/x`, `@evil.example/x`, `:8443/x`, `x` (no leading `/`), a
  path with `?`, `#`, `\` or a control character: each gives `Malformed`.
- **Replay store:** fresh; replayed; live at `now == expires_at` and gone after the later
  deadline; the follow-up has 30 s when `now == expires_at` at record; each quota with its
  `retry_after_secs`; the capacity; expired heads removed before a limit is reported; counts go
  to zero and are removed; follow-up redeemed once, refused the second time, refused with
  another digest (and still redeemable with the right one), refused after the deadline.
- **`AuthenticateToken::resolve_dpop`:** an unprovisioned identity with a bad proof gives
  `InvalidDpopProof`, not `IdentityNotProvisioned`; the identity port is not called before the
  proof check (a fake that records calls).
- **Validator:** the `Dpop` scheme with `cnf.jkt` (pass); with `cnf` but no `jkt`; with `jkt`
  and `x5t#S256`; with `typ: DPoP` only; with no binding (`NotKeyBound`). The SMA-690 `Bearer`
  cases stay.
- **Config:** each `validate()` refusal in § 4.9; the env names through `figment::Jail`; the
  chart's env form.
- **IAM mappings:** each row of the gRPC table in § 4.8; the HTTP funnel arms; the
  `IsAuthorized` path constant.
- **`is_authorized` with the `DpopFollowUp` extension:** a self-query passes; another
  `principal_prn` is refused.
- **Gateway parser:** scheme case; two `DPoP` headers; a missing header; a non-ASCII value; the
  `Bearer` scheme with a `DPoP` header; DPoP off.
- **Gateway `dpop_challenge`:** each case in § 4.10, and `None` when off.
- **Gateway error mapping:** `INVALID_DPOP_PROOF` → 401; `DPOP_QUOTA_EXCEEDED` → 429 with
  `Retry-After` and the metric label `denied`; no "PermissionDenied with no ErrorInfo" warning.
- **RS256 fixtures:** the validator tests do not use the `rsa` crate (`validator.rs:477-480`).
  The plan commits fixed RSA test keys as fixtures, or adds `rsa` as an IAM dev-dependency and
  updates the rationale in `rs/deny.toml:10-19`. The plan chooses one and records it.

### 5.2 Integration tests

Extend `MockIdp` (`paigasus-iam/tests/support/mod.rs:165`) so it can mint a token with
`cnf.jkt` and `typ: DPoP`, and two different tokens with the same `jkt`. Add a proof helper next
to it (the shape of `keycloak_e2e.rs:358`).

- **IAM, `tests/grpc_authn.rs` and `tests/grpc_whoami.rs`** (their `IntrospectRequest`
  literals get `dpop: None`). New cases, over real gRPC:
  - the gateway sequence: `Introspect` with a valid `dpop` context, then `IsAuthorized` with the
    follow-up metadata. Both pass.
  - the same `Introspect` again: `INVALID_DPOP_PROOF` (replay).
  - the follow-up twice: the second gives `INVALID_DPOP_PROOF`.
  - the follow-up with another token bound to the same key: `INVALID_DPOP_PROOF` (`ath`).
  - the follow-up for another principal: `INVALID_DPOP_PROOF`.
  - the follow-up on another RPC (for example `WhoAmI`): `INVALID_TOKEN`.
  - the follow-up with no earlier `Introspect`: `INVALID_DPOP_PROOF`.
  - an unprovisioned identity with a bad proof: `INVALID_DPOP_PROOF`, not
    `IDENTITY_NOT_PROVISIONED`.
  - DPoP off: a `dpop` context gives `INVALID_TOKEN`, as for a malformed token.
  - no `dpop` context with a bound token: `INVALID_TOKEN`, as today.
  - a proof whose `htu` names a base URL that is not configured: `INVALID_DPOP_PROOF`.
  - the follow-up with a token near `max_token_bytes` and a proof near 8192 bytes passes the
    transport.
- **IAM, `tests/http_authn.rs`:** with DPoP on, `Authorization: DPoP` on an HTTP route is still
  refused as today.
- **IAM, `tests/keycloak_e2e.rs`:** with DPoP on, a real Keycloak DPoP-bound token and a new
  proof pass `Introspect` with a `dpop` context. The SMA-690 `Bearer` assertion stays.
- **Gateway, `auth.rs` tests with `FakeIam`:** a DPoP request sends the context to
  `introspect_token` with the right method and path, skips `introspect_api_key`, and sends
  `CallerCredential::Dpop` to `is_authorized_self`. The same for `require_authenticated`
  (service-info), without `is_authorized_self`. An IAM `INVALID_DPOP_PROOF` gives 401
  `invalid-dpop-proof` with the challenge. Gateway DPoP off: the response is the same as today,
  except the per-request headers.
- **Gateway, `tests/chat_proxy.rs`:** one DPoP request end to end with a fake IAM that records
  both calls.
- **Chart:** `refusals.sh` rows, `env.sh` B6 and the restart-scope row, the golden case.

### 5.3 Proof that the tests bite

Each mutation below must compile. Run `cargo nextest run --no-fail-fast` on the IAM `--lib`,
`grpc_authn`, `grpc_whoami` and `http_authn` binaries and the gateway `--lib` and `chat_proxy`
binaries, with `PAIGASUS_REQUIRE_DOCKER=1`. `keycloak_e2e` runs once after the battery, not
for each mutation. Record which test fails for each mutation.

- Checks 1-13: each one is replaced by its pass value. For check 3, the mutation accepts a
  header with no `typ`. For check 6, the mutation skips `crypto::verify`. For check 7, the
  mutation gives a default `jti`.
- Wiring: the verifier call is removed from `Introspect`; `Introspect` uses the `Bearer` scheme
  for a `dpop` context; the verifier runs after the identity lookup; `redeem_follow_up` always
  gives `Redeemed`; the follow-up skips the `ath` check; the follow-up path check accepts any
  RPC; the self-query check on the extension is removed; the forwarded path check is removed;
  the gateway sends `dpop: None`; the gateway calls the API-key leg first on `DPoP`; the
  gateway puts the error attribute on the challenge of a `Bearer` request.
- D7: the `Bearer` scheme accepts a bound token. D11: IAM accepts a `dpop` context when off.
- If a mutation fails no test, add a test and run the full battery again.

Record the results in `docs/superpowers/plans/2026-10-04-sma-700-mutation-results.md`.

### 5.4 Acceptance criteria

| Criterion | Evidence |
|---|---|
| 1. With DPoP on in both services and a provisioned identity, a valid DPoP request to the gateway is accepted and authorized. | § 5.2 IAM gateway sequence; gateway `chat_proxy`; keycloak |
| 2. Each RFC 9449 § 4.3 check refuses a proof that fails it. | § 5.1 proof checker and verifier; § 5.3 |
| 3. A replayed proof is refused. The follow-up works once only, for a self-query only. | § 5.1 replay store; § 5.2 |
| 4. No identity lookup or provisioning runs before the proof check. | § 5.1 `resolve_dpop`; § 5.2 unprovisioned case; § 5.3 |
| 5. The `Bearer` scheme still refuses a bound token (SMA-690 D9). | § 5.1 validator; § 5.2; § 5.3 D7 |
| 6. With DPoP off, the responses of both services and the default chart output are the same as before. | § 5.2 DPoP-off cases; chart golden files; § 5.3 D11 |
| 7. The gateway sends a `WWW-Authenticate: DPoP` challenge as § 4.10 says, and a quota hit is a 429, not a 503. | § 5.1 gateway; § 5.2 gateway |

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
  `src/adapters/retryable.rs`, `src/application/{dpop.rs (new),authenticate_token.rs}`,
  `src/adapters/grpc/{authn.rs,authz.rs,convert.rs}`, `src/adapters/http/authn.rs`,
  `src/config.rs`, `src/service_info.rs`, `src/main.rs` (the HTTP/2 header limit), the
  `AppState` wiring, `iam.toml.example`, `tests/support/mod.rs`,
  `tests/{grpc_authn.rs,grpc_whoami.rs,http_authn.rs,keycloak_e2e.rs,zitadel_e2e.rs}`,
  `CHANGELOG.md`.
- Gateway: `src/config.rs`, `src/service_info.rs`, `src/main.rs`,
  `src/adapters/http/{auth.rs,error.rs,mod.rs}`, `src/adapters/iam/client.rs`,
  `gateway.toml.example`, `tests/{chat_proxy.rs,service_info.rs,metrics.rs}`,
  `tests/support/limits.rs`, `CHANGELOG.md`.
- Chart: `charts/paigasus/values.yaml`, `templates/_iam-backend.tpl`,
  `templates/backend-deployment.yaml` (template comment), golden files,
  `tests/{refusals.sh,env.sh}`, `README.md`.
- Docs: `docs/ops/RUNBOOK-chart.md`.
- Possibly `rs/deny.toml` and an `rsa` dev-dependency (§ 5.1).

## 7. Rollout and effect on operators

- The defaults are off. With the defaults, every IAM and gateway response is the same as
  before (D11), and the chart renders the same output.
- Order: turn DPoP on in IAM first (`zones.iam.backend.dpop.enabled`,
  `zones.iam.backend.dpop.forwardedBaseUrls`), then in the gateway (`GATEWAY_DPOP__ENABLED`).
  The other order also fails closed: IAM refuses the `dpop` context while it is off, and an old
  IAM ignores the new field and refuses a bound token as `SenderConstrained`.
- Supported topology: TLS ends in front of the gateway. Each public URL at which a client
  reaches the gateway is in `forwardedBaseUrls`, with any prefix that a proxy removes. A wrong
  entry refuses DPoP requests (`Htu`); it does not accept a wrong request. The chart refuses a
  bad entry at render time, so a typo cannot stop IAM.
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
| R1 | A restart clears the replay store. | A proof used before the restart can be used again until it expires: up to `2 × iat_window_secs` (120 s with the defaults) after the restart. The attacker needs a captured proof for the same method and URL. | Accepted. |
| R2 | The in-memory store needs one IAM replica. | Two replicas would accept one proof once on each. Also, `Introspect` and the follow-up could reach different pods, and the follow-up would then be refused (401). The chart pin (SMA-559) prevents both today. | The comment at the pin (§ 4.11). |
| R3 | No server nonce (RFC 9449 § 8). | A client can make proofs in advance, within the `iat` window. | Accepted. |
| R4 | `Introspect` and the follow-up trust the caller for the method and the path. | A caller that can reach IAM's gRPC port can claim any valid path under a configured base URL (the path check in § 4.8 keeps the host and port fixed). It still needs a proof signed by the client's private key for that URL. `Introspect` is already unauthenticated in-cluster. | Accepted. |
| R5 | The follow-up skips the signature check, and a ticket can stay unredeemed. | The follow-up relies on the digest of bytes that `Introspect` verified, the `ath` check and the self-query rule. A captured proof and token give one self-query answer for the token owner, before the deadline. A `service-info` call or a 400 `org-required` leaves an unused ticket, which expires with its entry. | Accepted. |
| R6 | IAM's own API does not accept DPoP. | A client cannot use DPoP against IAM directly, and a DPoP-only client cannot provision itself (§ 2). No shipped route exists for that (§ 1). | Not tracked. A new issue if a route is added. |
| R7 | Many identities can still fill the global capacity. | DPoP requests get 503 until entries expire (up to 120 s with the defaults). | The `warn` line; the operator raises `replay_capacity`. |
| R8 | The memory budget in § 4.5 is an estimate. | The default capacity can use more memory than stated. | The plan measures it, and corrects the default and the runbook. |

## 10. Open questions

None. Sven decided the scope, the replay store, the switches, the authz step and the ADR on
2026-10-04. Challenge 2's questions are answered in § 11.

## 11. Spec challenges

### Challenge 1 (2026-10-04, on draft 1): NEEDS REWORK

| Finding | Severity | Action |
|---|---|---|
| The replay store can be filled by one user, its clock is not the `iat` clock, the expiry scan is O(n), the count map leaks, and `StoreFull` maps to a non-retryable 401. | BLOCKER | Folded in: § 4.5, D10. |
| The chart exposes no IAM origin. | MAJOR | Scope changed by Sven (D1). § 7 states the supported topology. |
| The verifier in `application/` imports JOSE code. | MAJOR | Folded in: D2, § 4.4. |
| The jsonwebtoken configuration and the parse step are not pinned. | MAJOR | Folded in: § 4.4. |
| The challenges have no place in the code, and the error attribute is on the wrong scheme. | MAJOR | Folded in: § 4.10. IAM's HTTP challenges are out of scope with the new scope. |
| The mutation battery leaves out the wiring and D7. | MAJOR | Folded in: § 5.3. |
| § 6 leaves out files that red CI, and cites a wrong gate path. | MAJOR | Folded in: § 4.1, § 4.2, § 6. |
| MINOR findings (clock, `NotKeyBound`, `htu` model, `typ`, RSA bounds, claim types, log issuer, origin list ownership, chart details, config bounds, test gaps, client retries, store sharing, a cited line). | MINOR | Folded in. The HTTP introspect body cap did not apply: the HTTP twin does not change. |
| Questions (first client, `invalid_token` for `Thumbprint`, both lists, `dpop` once, audit, `jwk` members, CORS). | QUESTION | Answered: D1, D9, one list, D15 and D14, § 2, D17, § 2. |

### Challenge 2 (2026-10-04, on draft 2): APPROVE WITH CHANGES

| Finding | Severity | Action |
|---|---|---|
| The order of the proof check and the identity lookup is not stated; a late check lets a stolen token pass `service-info`. | BLOCKER | Folded in: D18, § 4.7, § 4.8, § 5.1, § 5.2, § 5.3, AC 4. |
| `invalid-dpop-proof` has an IAM-only number, but the gateway emits it. | MAJOR | Folded in: D19 (909, shared range). |
| A quota hit looks like an IAM outage and fires the critical alert. | MAJOR | Folded in: D10, a new IAM reason `DPOP_QUOTA_EXCEEDED = 46` with `RetryInfo`, the gateway's existing 429 `rate-limited`, the label `denied`. `ReplayStoreFull` is removed; the global capacity uses `Unavailable`. |
| `IntoResponse` cannot build the challenge. | MAJOR | Folded in: `AuthState`, the challenge appended in the middleware (§ 4.10). |
| The forwarded `path` can change the host of the expected URL. | MAJOR | Folded in: the path check (§ 4.8), the host check after the parse (§ 4.6), tests and a mutation. |
| The chart copies only one IAM boot refusal; B6; the YAML comment renders. | MAJOR | Folded in: § 4.11. |
| The follow-up window can be zero. | MINOR | Folded in: `follow_up_deadline` (§ 4.5). |
| Unredeemed tickets. | MINOR | Named in R5. A flag in the contract was not added: an unused ticket expires and grants nothing. |
| The follow-up is not bound to the question. | MINOR | Folded in: the `DpopFollowUp` extension and the self-query rule (§ 4.8). |
| A mixed binding (`jkt` plus `x5t#S256`) is accepted. | MINOR | Folded in: § 4.3. |
| R1 and R7 understate the lifetime; the window is tied to the token leeway. | MINOR | Folded in: D20, § 4.9, R1, R7. |
| Memory per entry. | MINOR | Folded in: hashes in § 4.5, the budget, R8. |
| The HTTP/2 header limit on the follow-up. | MINOR | Folded in: § 4.8, a test in § 5.2. |
| § 6 misses files. | MINOR | Folded in: § 4.8, § 4.9, § 4.10, § 6. |
| The "generated service constants" claim. | MINOR | Folded in: § 4.8. |
| The follow-up parsing has no named home. | MINOR | Folded in: `follow_up_claims` (§ 4.2, § 4.4). |
| Simplification for checks 6 and 9. | MINOR | Folded in: `crypto::verify` and a hand-built thumbprint (§ 4.4). |
| § 5.3 battery gaps. | MINOR | Folded in: § 5.1, § 5.2, § 5.3. |
| Rollout order and R2. | MINOR | Folded in: § 7, R2. |
| Gateway logs and runbook. | MINOR | Folded in: § 4.10, § 4.11. |
| Q: which `Malformed` for the sizes? | QUESTION | `InvalidDpopProof(Malformed)` for the proof, method and path (§ 4.8). The token size stays an `InvalidToken` check. |
| Q: what does "two lines with `append`" test? | QUESTION | Removed. The gateway sends one line. The builder still uses `append`. |
| Q: how is a DPoP-only client provisioned? | QUESTION | The precondition in § 2 and AC 1. |
| Q: does service-info advertise DPoP? | QUESTION | Non-goal (§ 2). |
| Q: what memory limit do operators give IAM? | QUESTION | The chart sets none. § 4.5 states the budget; R8. |
