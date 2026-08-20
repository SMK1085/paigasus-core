# SMA-504 — `google.rpc.ErrorInfo`, correlation ids and retryable metadata

**Issue:** [SMA-504](https://linear.app/smaschek/issue/SMA-504) · **ADR:** ADR-0019 (decisions E1–E6 + Amendment A1)
**Depends on:** SMA-498 (merged, PR 120) · **Unblocks:** SMA-507 (drift gate), SMA-508 (TS SDK)
**Date:** 2026-08-17 · **Revision:** 2 (adversarial review folded in)

## 1. Problem

The error code is machine-readable on exactly one of four surfaces, and only by accident on the rest.

- **IAM tenancy gRPC** stuffs the code in-band: `Status::new(code, format!("{}: {}", e.code(), e))`. A client recovers the code only by splitting on `": "`, which breaks the moment a message legitimately contains that sequence.
- **IAM authn gRPC** (`authn_status`) carries no machine code at all — five static message strings and nothing else.
- **Six further gRPC sites** construct a bare `Status` directly and carry no code of any kind (§5.1.3).
- **IAM HTTP authn** and the **gateway** emit snake_case codes that do not match the canonical kebab registry SMA-498 landed in `contracts/proto/paigasus/common/v1/error.proto`.
- **No surface carries a correlation id**, which for a self-hosted product means supporting a deployment you cannot access is guesswork.

There is a live consequence, not just a theoretical one. The gateway's `require_authenticated` (SMA-505, ADR-0020 D4) accepts a bare `Code::PermissionDenied` from IAM's `Introspect` as "validated but not yet provisioned". `authn_status` collapses **three** `AuthnError` variants onto that one code (`IdentityNotProvisioned`, `ProvisioningFailed`, `PrincipalInactive`). It is not exploitable today — both other variants are unreachable through `Introspect` — but the accept is blanket, and its doc comment carries an explicit "if either becomes reachable, this arm must narrow" tripwire that only this issue can discharge.

## 2. Goals and non-goals

**Goals**

1. A gRPC client reads `(domain, reason)` from `grpc-status-details-bin` without parsing the message string, for every `Status` this codebase constructs.
2. Every code emitted by every emission site resolves via `ErrorReason::from_wire_reason` — the precondition for SMA-507's emitted-side gate.
3. Every response on both services' **API surfaces** carries a request id and a correlation id; every error response additionally carries a retryability signal.
4. The gateway's `IamError::Rpc` branching is updated in the same change, and the SMA-505 tripwire is discharged.
5. No envelope changes shape. Internal errors leak nothing new.

**Non-goals**

- Generating a `google.rpc.ErrorInfo` descriptor for TypeScript (§9).
- Turning on the two-way drift gate — that is SMA-507, which this issue unblocks.
- Moving the canonical error types into `paigasus-kernel` (ADR-0019 defers this explicitly).
- Wiring OpenTelemetry, `traceparent`, or distributed tracing generally.
- Covering `Status`es produced by tonic itself rather than by our code (§4.1, accepted gap).

## 3. Decisions

### D1 — Two ids, per the Notion HTTP-header guideline

The [HTTP Headers](https://app.notion.com/p/2aa830e8fbaa8004bfb6c7c73a8f224a) guideline defines both, with different jobs:

| Header | Request | Response | Role |
| -- | -- | -- | -- |
| `paigasus-request-id` | Forbidden | Mandatory | Identifies **this individual call**, so repeated or retried calls stay separable. Server-minted; a client-sent value is ignored and overwritten. |
| `paigasus-correlation-id` | Optional | Mandatory | Tracks **one logical invocation** end-to-end across services. Adopted from the caller when well-formed, else minted. |

Both are implemented. The guideline's other three headers (`paigasus-idempotency-key`, `paigasus-locale`, `paigasus-timing`) are out of scope.

> **Reviewer note.** That page currently sits under *Paigasus → Legacy / Archive → Development Guidelines → Interservice Communication → REST*, and the live Development Guidelines page has no HTTP-headers section. The names are treated as binding. They become a wire contract on two services and then an SDK contract in SMA-508, so promoting the page out of the archive — and adding `paigasus-retryable` to it — is a documentation follow-up that should not lag far behind this change.

### D2 — Ids surface in headers, never in a JSON body

Both JSON error bodies keep their **exact shape and key set** — IAM's `error` object stays `{code, message}`, the gateway's stays `{message, type, param, code}`. No key is added, removed or renamed, and no id ever appears in a body. That is what AC 3 and ADR-0019's "all four envelopes stay exactly as they are" require, and it is what keeps the gateway's OpenAI-compatible envelope safe for SDKs that branch on `error.type`.

The bodies are **not** byte-identical, and saying so would contradict this document's own rename inventory (§5.2): the `code` *values* change on 18 sites — snake_case to kebab, plus `GatewayError::Internal`'s `null` becoming `"internal"` — and every assertion pinning one of them changes with it. What does not change is the envelope: shape, keys, and `type`'s OpenAI semantics. A console reads the ids off the headers.

On gRPC the ids ride in `ErrorInfo.metadata` as `correlation_id` and `request_id`, which is where ADR-0019 decision 6 puts them. On a gRPC error they therefore appear twice — once as response headers set by the layer, once in the metadata map. A test pins that the two agree; clients may read either.

### D3 — Ambient request-scoped context, not threaded parameters

`status_to_grpc` is a free function and `ApiError`/`AuthnApiError`/`GatewayError` are `IntoResponse` impls; none has access to the request, and they are called from roughly fifty places. Threading a context parameter through all of them would delete the blanket `impl<E: Into<TenancyError>> From<E> for ApiError` that lets ~40 IAM handlers use `?`, producing a large mechanical diff inside a change that is already 18 renames wide.

Instead a `tokio::task_local!` holds the ids for the duration of the request-head future. The compile-time guarantee this gives up — "every error site has an id" — is recovered by one integration test per surface (§7). Its real limits are stated in §4.3, not hand-waved.

### D4 — Retryability is a three-state signal, defaulted by the layer and overridden by the renderers

ADR-0019 decision 7 puts retryability in `ErrorInfo.metadata`, which exists only on gRPC — yet the gateway's OpenAI-SDK callers are precisely the audience that hard-codes "503 means retry". A `paigasus-retryable` response header carries the same datum on HTTP without touching either envelope.

The value is `true`, `false`, or **`unknown`**, not a boolean. The third state is not hedging; it is the honest encoding of a real limitation. `TenancyError::Internal` absorbs `RepositoryError::Backend`, `AuthzError::Backend` and `AuthzError::Evaluation`, so a Postgres pool blip and a genuine logic bug arrive as the same variant with the source erased at conversion. Labelling that `false` would be *worse* than the status-class guess it replaces — it would tell a client to give up on precisely the failure most likely to succeed on retry — and labelling it `true` invites retry storms against a service that is already failing.

| Error | value |
| -- | -- |
| `AuthnError::Unavailable` | `true` |
| `GatewayError::{IamUnavailable, UpstreamUnavailable, UpstreamTimeout}` | `true` |
| every `internal` (`TenancyError::Internal`, `AuthnError::Backend`, `GatewayError::{Internal, MissingScope}`) | `unknown` |
| everything else — all 4xx-class, all validation, all conflicts | `false` |

**Two writers, in priority order.** `CorrelationLayer` sets a status-class default on every error response it sees (`408/429/502/503/504 → true`, other 5xx → `unknown`, everything else → `false`); a renderer that knows better overrides it. Without the layer default the header would be absent on error responses no renderer owns — axum's 404/405 fallbacks, `DefaultBodyLimit`'s 413, `TimeoutLayer`'s 408 — and a client following the contract would have to treat absence as meaningful. The layer default is explicitly an inference of last resort: it exists so "present on every error response the layer covers" is true, not because status-class inference is good.

Splitting `TenancyError::Internal` into transient and permanent variants is the durable fix and would let `unknown` collapse to `true`/`false`. It is a behaviour change to IAM's error taxonomy and is deliberately **not** in this issue's scope; §9 records it as a follow-up.

### D5 — `missing-scope` keeps its own reason

`GatewayError::MissingScope` continues to emit `missing-scope` rather than folding into `internal`. Its response is already indistinguishable from `internal` to a caller (same 500, same `"Internal error."` message, same `api_error` type), so the code is purely a diagnostic for our own logs — and it names the exact plumbing bug it detects (`introspect` returned an empty `scope_prn`). Folding it would lose that signal for no client-visible gain.

### D6 — Inbound correlation ids are adopted only if they parse as a UUID

An adopted value reaches log lines, two response headers, outbound gRPC metadata and (per D9) persisted audit rows. Accepting an arbitrary caller-controlled string there is a log-injection and header-splitting surface. A value that does not parse as a UUID is silently replaced with a freshly minted one and is **never echoed anywhere**.

A rejection emits a `tracing::debug!` naming the reason but **never the value** — without it, an operator whose reverse proxy mangles the header gets zero diagnostics. Only the first `paigasus-correlation-id` header is read (`HeaderMap::get` semantics); duplicates are ignored, which is the same posture as discarding a malformed one.

`paigasus-request-id` is never read from the request at all.

### D7 — UUIDv7 is minted through `paigasus-kernel`, not `uuid`'s `v7` feature

`paigasus_kernel::mint_uuid7(ms, entropy)` is the repo's single UUIDv7 implementation; `KernelIdGenerator::mint` already calls it with `SystemTime::now()` and `rand::random::<[u8;10]>()`.

The reason is **not** that the `repo:wasm-getrandom-free` gate would otherwise catch it — that gate runs `cargo tree -p paigasus-wasm --target wasm32-unknown-unknown` and `paigasus-observability` is not in that tree. The reason is workspace-wide feature unification: enabling `uuid/v7` anywhere in `rs/` enables `uuid/rng`, and therefore `getrandom`, for *every* `uuid` dependent in the same resolution — including `paigasus-kernel`, whose entire wasm story depends on staying feature-free. That is a trap best avoided rather than tested into. `mint_uuid7` takes explicit entropy and needs no `uuid` feature at all.

Cost, stated up front: this makes `paigasus-observability` a new `paigasus-kernel` dependent, which reds `repo:affected-smoke`'s strict-equality `kernel->bindings` expected set until `ci/affected-graph/run.sh:203` is updated (SMA-409), and requires a Moon `dependsOn` entry plus `^:build` on **`build` and `test`** (`lint` already has it from `.moon/tasks/rust.yml`); `cargo_moon_parity.py:95-102` checks all three independently. `paigasus-kernel` depends only on `uuid` and `thiserror`, so there is no cycle.

### D8 — Both services derive domain and reason from the registry, never from literals

`require_authenticated` compares against `ErrorReason::IdentityNotProvisioned.as_wire_reason()`, not the literal `"identity-not-provisioned"`. This makes the gateway the *consumed* half of SMA-507's two-way gate rather than a second hand-maintained copy of the vocabulary.

The domain strings get the same treatment, and they live in **`paigasus-proto`**, not in either service:

```rust
// paigasus-proto/src/error.rs — next to as_wire_domain, which they are derived from.
pub static IAM_DOMAIN: LazyLock<String> = ...;      // "iam.paigasus.io"
pub static GATEWAY_DOMAIN: LazyLock<String> = ...;  // "gateway.paigasus.io"
```

The gateway must compare `ErrorInfo.domain` against IAM's domain, and it cannot see a `LazyLock` private to the IAM crate. Leaving that unstated invites a hardcoded `"iam.paigasus.io"` in the gateway — a second copy of exactly the vocabulary D8 exists to protect. `ErrorReason::as_wire_reason()` allocates a `String` per call, so reasons compared on a hot path (the gateway's discovery-rejection path) are likewise hoisted into a `LazyLock`.

### D9 — The header correlation id and IAM's audit correlation id are the same id

IAM already has a `correlation_id`. `IdGenerator::new_correlation_id()` mints one per **mutation**, and it is persisted on both the `audit_log` row and the `event_outbox` row of that transaction (`application/{policies,api_keys,roles,system_retirement,bootstrap_admin}.rs`), surfaced on the dead-letter HTTP body, and carried into published CloudEvents. Leaving the new header id unlinked would put two different things called `correlation_id` in one service and defeat §1's own motivation: an operator handed a `paigasus-correlation-id` by a customer could not find the audit row it produced.

So `KernelIdGenerator::new_correlation_id()` **adopts `current_ids().correlation_id` when there is one**, and mints fresh otherwise (boot-time paths such as `bootstrap_admin` have no request). Existing "the outbox event and the audit entry share one correlation id" assertions are unaffected — they compare two rows within one transaction, which still hold.

The trade-off, stated rather than hidden: because a caller may supply the correlation id (D6), a caller can make many of their own audit rows share one id. That is what a correlation id is *for*, and it degrades nothing — each row keeps its own primary key and `occurred_at`, and the server-minted `request_id` remains unforgeable and appears in every log line. A caller cannot affect anyone else's rows.

### D10 — The layer covers the API surface, mirroring `http_metrics_layer` exactly

`CorrelationLayer` attaches at precisely the composition point `paigasus_observability::http_metrics_layer` already uses: `app_routes` in IAM (`adapters/http/mod.rs:849`) and `router` in the gateway (`adapters/http/mod.rs:103`). Consequences, all of them deliberate and all of them precedented by that layer's own documented placement:

- The `oneshot` test harness exercises it, because `app_routes` is inside `router()`. Attaching in `serve_http` instead would leave every existing integration test blind to it — the bearer layer's own doc comment records that exact reasoning.
- **IAM.** `/healthz`, `/readyz` and `/metrics` are **outside** it and carry no ids and no retryable header. They are operational endpoints, not a client API; a 15-second Prometheus scrape minting two UUIDv7s per tick would be pure waste, and `readyz_router` is deliberately outside every layer today.
- §2 goal 3 and D1's "Mandatory" are therefore scoped to the API surface for IAM. This is a narrowing of the guideline and is called out as such.

**Amendment (final whole-branch review, finding #4).** The gateway does the opposite: its `/healthz` and `/readyz` sit **inside** `CorrelationLayer` and carry both ids. This is not an inconsistency between the two services worth reconciling — it is a second, independently-justified decision, and both should be recorded rather than only one:

- The gateway's router is flatter than IAM's — `router()` builds one merged tree in a single function (`adapters/http/mod.rs`), with no analogue of IAM's separate `health_router()`/`readyz_router()`/`app_routes()` split that `CorrelationLayer` could attach below. Carving the two health routes out into their own pre-`CorrelationLayer` merge, purely to withhold two headers, is not worth diverging the gateway's simpler composition from IAM's.
- The cost is genuinely nil where IAM's avoidance reasoning does not transfer: the gateway's `/readyz` already does live work every poll (a real IAM introspect RPC, `adapters/http/mod.rs::readyz`), so the marginal cost of two more UUIDv7 mints on top of that RPC is immaterial — unlike IAM's `/readyz`, which is otherwise a bare `SELECT 1`.
- So: both services made a real decision, in opposite directions, each defensible on its own composition and its own operational endpoint's cost. `correlation_headers.rs`'s `the_operational_endpoints_carry_no_ids` test pins IAM's narrowing; `adapters/http/mod.rs`'s `healthz_and_readyz_carry_ids_too` test (gateway) pins the opposite. Neither is a bug.

## 4. Architecture

### 4.1 New module: `paigasus-observability::correlation`

One tower `Layer`/`Service`, generic over the body type, wraps **all three** server surfaces: IAM's axum API router, IAM's tonic server, and the gateway's axum router. gRPC metadata *is* HTTP/2 headers, so the same header names are read on both protocols. Verified: a single generic `Layer` satisfies both `axum::Router::layer` and tonic's `L::Service: Service<Request<Body>, Response = Response<ResBody>>` bound — no second implementation is needed.

```rust
/// `request_id` identifies THIS individual call — server-minted, never read from the client —
/// so repeated or retried calls stay separable. `correlation_id` tracks ONE logical invocation
/// end-to-end across services, adopted from the caller when it parses as a UUID.
#[derive(Debug, Clone, Copy)]
pub struct RequestIds {
    pub request_id: Uuid,
    pub correlation_id: Uuid,
}

tokio::task_local! { static IDS: RequestIds; }

/// The ids for the in-flight request head, or `None` outside that scope — see §4.3.
pub fn current_ids() -> Option<RequestIds>;

pub const REQUEST_ID_HEADER: &str = "paigasus-request-id";
pub const CORRELATION_ID_HEADER: &str = "paigasus-correlation-id";
pub const RETRYABLE_HEADER: &str = "paigasus-retryable";

pub struct CorrelationLayer;
```

Per request the layer mints a request id; adopts or mints a correlation id (D6); runs the inner service inside `IDS.scope(ids, ...)`; and on the response sets both id headers unconditionally plus `paigasus-retryable` **only if the inner service did not already set it** (D4).

**Placement, and an accepted gap.** On axum the layer is outermost over the API router (D10). On tonic it is outermost *among our own layers only*: `Server::builder().layer(...)` feeds a `ServiceBuilder` that tonic then wraps in `RecoverErrorLayer`, `LoadShedLayer`, `ConcurrencyLimitLayer` and `GrpcTimeout`. Since `adapters/grpc/mod.rs:66` sets `.timeout(...)`, a gRPC **timeout** `Status` is produced outside our layer and carries no ids, no `ErrorInfo` and no retryable metadata. That gap is accepted and recorded here rather than discovered later; closing it would mean reimplementing tonic's timeout.

The test that pins placement therefore has two parts: headers present on an **auth-rejected** response (proves the layer sits outside `AuthLayer`, which is the ordering we control), and an explicit assertion of what a `Server::timeout`-produced `Status` actually carries (documents the gap so it cannot silently widen). An auth-rejection test alone would pass regardless of the tonic-internal ordering and would prove less than it appears to.

### 4.2 Header ownership

| Header | Set by | Present on |
| -- | -- | -- |
| `paigasus-request-id` | `CorrelationLayer` | every API-surface response **the layer covers** |
| `paigasus-correlation-id` | `CorrelationLayer` | every API-surface response **the layer covers** |
| `paigasus-retryable` | `CorrelationLayer` default, overridden by the renderers | every API-surface **error** response the layer covers |

"The layer covers" is doing real work in that column, and it excludes exactly two things, both documented rather than incidental: the operational endpoints, which sit outside the layer on IAM by D10; and any `Status` tonic produces from its own outer stack, notably a `Server::timeout` expiry (§4.1), which carries no ids and no retryability because it is generated outside every layer we control.

Five renderers override the layer's `retryable` default — `iam::http::error::ApiError`, `iam::http::authn::AuthnApiError`, `iam::http::authn::envelope_rejection`, `iam::http::system_retirement::conflict`, and `gateway::http::error::GatewayError`. The two in `authn` are separate paths that happen to share a module: one renders the `AuthnError` funnel, the other the extractor's rejections.

The gateway's terminal SSE frame is the one unavoidable exception: by the time it is emitted the `200 OK` head is already sent and no header can change. It carries the canonical `code` only — see §9 for why the frame's JSON is not extended instead.

### 4.3 When `current_ids()` is `None`

`None` is returned outside the request-**head** future. That is three situations, not one:

1. Unit tests that construct a response or a `Status` directly.
2. Background tasks (the outbox relay, the partition maintainer).
3. **Response-body streaming.** A `task_local!` scope set in `Service::call` ends when the future resolves to `Response<Body>`; hyper polls the body *afterwards*. The gateway's SSE path returns `Body::from_stream(...)` (`adapters/http/chat.rs:108`), so everything inside the stream adapter — including any log line or metric a later change adds there — runs with no ids. The same will apply to any future server-streaming gRPC method.

Situation 3 is the non-obvious one and gets an explicit comment at the stream adapter. Nothing in the repo `tokio::spawn`s on a request path today (checked: the only spawns are `servers.spawn` in `main.rs`/`runtime.rs`), so task-local propagation across a spawn is not currently a concern — but a spawn would drop the scope too.

In all three cases the `correlation_id` / `request_id` metadata keys are **omitted entirely** rather than filled with a nil UUID that would read as a real id, and the direct-render HTTP path emits no id headers. This is also what keeps the existing unit tests compiling unchanged.

### 4.4 Propagation

`IamClient` attaches the current `paigasus-correlation-id` to outbound IAM gRPC metadata, so a single id spans both services' logs and — via D9 — IAM's audit rows. It does **not** forward its request id: IAM mints its own for its own hop, which is what makes the two ids distinguishable in a stitched trace.

## 5. Surface-by-surface changes

### 5.1 IAM gRPC

#### 5.1.1 `status_to_grpc` — `adapters/grpc/convert.rs:31`

```rust
pub fn status_to_grpc(e: TenancyError) -> Status {
    let code = /* unchanged ErrorClass mapping */;
    Status::with_error_details(
        code,
        e.to_string(),                       // no more "{code}: {display}" prefix
        &ErrorDetails::with_error_info(e.code(), &*IAM_DOMAIN, metadata(retryable)),
    )
}
```

`reason` is `e.code()` verbatim — it already *is* the canonical wire string, so there is no runtime `from_wire_reason` unwrap; the registry is the validation (§7), not the transform.

`TenancyError::Internal`'s `Display` is already static and interpolation-free (ADR-0019 D7), so the generic-message requirement holds without a special case — but a test pins it rather than trusting the invariant.

#### 5.1.2 `authn_status` — `convert.rs:53`

The same treatment with its five canonical codes. Every static message stays byte-identical; only the machine code and metadata are new.

#### 5.1.3 The six bare `Status` sites — a gap this spec's first revision inherited

SMA-498's sweep counted six *HTTP and SSE* emission sites and concluded "the gRPC surfaces add no codes of their own". That is true of *codes*, and false of `Status`es. These six construct a `Status` directly and would still carry no `ErrorInfo` after §5.1.1–5.1.2:

| Site | Today |
| -- | -- |
| `grpc/tenancy.rs:79`, `grpc/authz.rs:64`, `grpc/service_accounts.rs:69`, `grpc/audit.rs:51` | `Status::unauthenticated("missing authentication context")` |
| `grpc/authz.rs:91` | `Status::unimplemented("capability iam.authz.cedar is not enabled on this service")` |
| `grpc/service_accounts.rs:104` | `Status::unimplemented("capability iam.apikeys is not enabled on this service")` |

Leaving them bare would falsify goal 1 for exactly the responses an SDK most needs to branch on — the capability gates SMA-505 introduced. All six route through a shared `iam_status(code, reason, message)` helper alongside `status_to_grpc`, and the registry gains two values:

- `ERROR_REASON_MISSING_AUTH_CONTEXT = 903` (**shared** range; 902 was the previous maximum). An internal invariant violation, exactly like `missing-scope`: it means `AuthLayer` did not attach a context. Shared rather than IAM-only for the same reason as the next entry — any service with an enforcement layer emits this condition. `retryable = false` (the cause is known exactly and cannot resolve on retry, so `unknown` would be dishonest here — see D4).
- `ERROR_REASON_CAPABILITY_DISABLED = 904` (**shared** range), with the capability name in `ErrorInfo.metadata["capability"]`. Shared rather than IAM-only because capability gating is not IAM-specific, and metadata-carried rather than one reason per capability because the registry is append-only and capabilities are not.

This does **not** subsume the gateway's `streaming-disabled`, which is a different thing: a request-*parameter* refusal (400, `param: "stream"`), not a route-level capability gate (501/`UNIMPLEMENTED`).

### 5.2 The rename inventory — 18 operations, plus 3 registry appends

Every rename is currently asserted by a test, so each is a real wire change.

| # | Site | Today | Canonical |
| -- | -- | -- | -- |
| 1 | `iam` `http/authn.rs:40` | `invalid_token` | `invalid-token` |
| 2 | `iam` `http/authn.rs:41` | `identity_not_provisioned` | `identity-not-provisioned` |
| 3 | `iam` `http/authn.rs:42` | `provisioning_failed` | `provisioning-failed` |
| 4 | `iam` `http/authn.rs:43` | `principal_inactive` | `principal-inactive` |
| 5 | `iam` `http/authn.rs:44` | `unavailable` | `authn-unavailable` — rename, not recasing |
| 6 | `iam` `http/authn.rs:79` | `request_too_large` | `request-too-large` |
| 7 | `iam` `http/authn.rs:81` | `invalid_request` | `invalid-request-body` — merged with the gateway's |
| 8–15 | `gateway` `http/error.rs:92-139` | `missing_authorization`, `invalid_api_key`, `insufficient_permissions`, `missing_scope`, `iam_unavailable`, `invalid_request_body`, `upstream_unavailable`, `upstream_timeout` | the kebab spelling of each |
| 16 | `gateway` `http/error.rs:117` | `null` (`GatewayError::Internal`) | `internal` — null → string |
| 17 | `gateway` `http/chat.rs:52` | `upstream_error` (terminal SSE frame) | `upstream-error` |
| **18** | `gateway` `http/error.rs:136` | `streaming_disabled` | `streaming-disabled` — **and a new registry entry** |

Rename 18 is a discovery, not in the issue's table. `GatewayError::StreamingDisabled` was added by SMA-505 *after* SMA-498 seeded the registry, and nothing gates the emitted side yet — which is precisely what SMA-507 exists to stop recurring.

**Registry appends:** `ERROR_REASON_STREAMING_DISABLED = 308`, `ERROR_REASON_MISSING_AUTH_CONTEXT = 903`, `ERROR_REASON_CAPABILITY_DISABLED = 904`. The registry grows 43 → 46, so `paigasus-proto/src/error.rs`'s `EXPECTED_REASONS` list and its `assert_eq!(actual.len(), 43)` anchor both change. `error.proto`'s file comment (lines 36–40) currently says the metadata carries "`retryable` and `correlation_id`"; it gains `request_id` and `capability`. Nothing gates that prose, so it must be updated in the same commit.

`system_retirement.rs`'s `grants-survive` and `decision-change-unacknowledged` are already canonical. Its malformed-body rejections route through `authn.rs`'s shared `envelope_rejection`, so renames 6 and 7 cover them.

**RFC 6750 carve-out.** `authn.rs:25`'s `BEARER_CHALLENGE` (`Bearer error="invalid_token"`) is standardised by RFC 6750 §3.1 and is **not ours to rename**. Only the JSON body's `code` becomes `invalid-token`. A test asserts the two diverge (AC 7).

### 5.3 Gateway consumption — `adapters/http/auth.rs`

The blanket `PermissionDenied` accept in `require_authenticated` narrows:

```rust
fn is_identity_not_provisioned(status: &Status) -> bool {
    if status.code() != Code::PermissionDenied {
        return false;
    }
    // `get_error_details` returns an owned `ErrorDetails`; bind it before borrowing.
    let details = status.get_error_details();
    details.error_info().is_some_and(|info| {
        info.domain == *paigasus_proto::error::IAM_DOMAIN
            && Some(info.reason.as_str()) == IDENTITY_NOT_PROVISIONED.as_deref()
    })
}
```

`IDENTITY_NOT_PROVISIONED` is a `LazyLock<Option<String>>` over `ErrorReason::IdentityNotProvisioned.as_wire_reason()` — hoisted because `as_wire_reason` allocates and this runs on every rejected discovery request.

**What a detail-less `PermissionDenied` actually does.** It is no longer accepted, but "fails closed → 401" is only true when the API-key leg reached a verdict. It falls through to the `Err(err)` arm (`auth.rs:231`) → `introspect_error` → `InvalidCredential` → `preserve_outage` (`:251`), which widens to `IamUnavailable` (503) when the API-key leg was inconclusive. Both outcomes are correct and both get a test row.

**Rollout ordering is a real constraint, not an assumption.** A new gateway pod talking to an old IAM pod sees detail-less `PermissionDenied` and rejects every unprovisioned console user's `/v1/service-info` for the duration of the skew window. **IAM must roll before the gateway.** This goes in the release notes. To make skew observable, a detail-less `PermissionDenied` emits a `tracing::warn!` naming the condition. A metric was considered and rejected: a new counter drags in `paigasus-observability::names`, a `describe_*!` site and the `:observability-drift` gate — three unlinked places to keep in sync — for a signal that matters only during an upgrade window.

The SMA-505 tripwire paragraph in `require_authenticated`'s doc comment is deleted and replaced with a short note on what the narrow accepts and why.

`introspect_error` and `authz_error` branch on `status.code()` only and never read the message — verified against `auth.rs:303-331` — so removing the in-band prefix does not affect them.

## 6. Dependencies and gates

- **`tonic-types = "0.14"`** — new *production* workspace dep. `paigasus-iam` emits (`ErrorDetails`, `StatusExt::with_error_details`); `paigasus-gateway` consumes (`StatusExt::get_error_details`). It is MIT, already on `rs/deny.toml:26`'s allow-list, so no `exceptions` entry is expected — confirm by running `:deny`, do not assume. Both crates consume it in this commit, so no `cargo-machete` allowlist is needed.
- **`paigasus-observability`** gains `tower`, `http`, `uuid`, `rand`, `paigasus-kernel` and `tokio = { workspace = true, features = ["rt"] }` as real dependencies (`task_local!`/`LocalKey` live behind tokio's `rt` feature, and `rs/Cargo.toml:24` sets no feature baseline). `tower` and `tokio` are currently dev-only there.
- **`ci/affected-graph/run.sh:203`** — add `paigasus-observability` to the `kernel->bindings` expected set (D7).
- **Moon edges** — `paigasus-observability-rs` gets a `dependsOn` on `paigasus-kernel-rs` plus `^:build` on its `build` and `test` tasks; the crate's `moon.yml` currently declares no `tasks:` block at all (cf. `paigasus-gateway/moon.yml:18-22`). `lint` already inherits `^:build` from `.moon/tasks/rust.yml`. `cargo_moon_parity.py:95-102` checks all three.
- **`grpc::router`'s signature changes** — `adapters/grpc/mod.rs:62` returns the concrete `TonicRouter<Stack<AuthLayer, Identity>>` and its doc comment reasons about that type. Adding a layer makes it `Stack<AuthLayer, Stack<CorrelationLayer, Identity>>`; the seven integration-test call sites are unaffected, but the signature and the comment are not.
- **`contracts/`** — `error.proto` gains three enum values and a file-comment update. Run `buf format -w` before committing or `contracts:fmt` reds `moon ci` silently, and regenerate with `buf generate` directly rather than `contracts:generate` (no `outputs:`, so it can serve stale cache). The regenerated Rust carries a hex delta in prost's embedded `FILE_DESCRIPTOR_SET`, which encodes comment text. `contracts/buf.yaml:29-33` already carries the reserve-tolerant enum rules, and appending values is not a `buf breaking` violation.
- Full pre-push graph per CLAUDE.md. `moon` is proto-managed, so the run needs `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` first (shims ahead of `bin`, or a global pin wins): `moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free :redis-connect-single-site :promtool :observability-drift :nats-permissions :release-parity :release-parity-py :release-parity-ts :publish-metadata --base origin/main --include-relations`.

## 7. Testing

**Registry coverage (AC 6).** One test per emission site, enum-driven wherever the site is enum-driven so a future variant cannot silently escape. `TenancyError` already has such a test (`convert.rs:390`); `GatewayError` and `AuthnError` gain `#[cfg_attr(test, derive(strum::EnumIter))]` and the matching test (`strum` becomes a gateway dev-dependency). The terminal SSE frame is a `const` literal, so its test parses the frame's JSON and resolves the `code` field rather than string-comparing the same literal. The six bare `Status` sites (§5.1.3) are covered by a test over the `iam_status` helper's call sites.

**Structured detail (AC 1).** Build a real `Status` through `status_to_grpc`, `authn_status` and `iam_status`, read it back with `StatusExt::get_error_details()`, and assert `domain`, `reason` and each metadata key. Separately assert `status.message()` does **not** start with `"{code}: "`.

**One test must cross a real wire.** Same-process `with_error_details` → `get_error_details` proves the predicate, not the transport — and the trailers-only path (`AuthLayer::reject` → `Status::into_http`, `grpc/authn.rs:194`) is a *different* serialization path from a handler-returned `Status`. Add one server-backed test asserting the details survive the trailers-only path.

**Existing assertions that pin the removed prefix** — all must convert to an `ErrorInfo.reason` check, not simply be deleted: `tests/grpc_tenancy.rs:111,196,232`, `tests/grpc_authz.rs:172`, `tests/grpc_audit.rs:107`, and the unit test at `src/adapters/grpc/convert.rs:342`. The five integration ones live in Docker-gated suites that `return` early and report PASS in ~1s without a Docker daemon, so **verification requires `CI=1 cargo nextest run -p paigasus-iam`**, which makes a missing daemon a hard failure. A conversion that quietly dropped the assertion would otherwise go green twice over.

**Integration tests asserting the renamed codes** — `paigasus-iam/tests/http_authn.rs`, `paigasus-iam/tests/api_key_auth.rs`, `paigasus-gateway/tests/chat_proxy.rs:300,319` — are real work and belong in the plan, not just the unit-level renames.

**Gateway narrowing (AC 2), and its harness.** `FakeIam`'s three `*Outcome::Rpc(Code)` variants (`auth.rs:353,365,374`) build detail-less statuses via `Status::new(*code, "")` (`:455`), so they extend to `Rpc(Code, Option<ErrorDetails>)`. Rows: `PermissionDenied` + `identity-not-provisioned` → proceeds (this keeps `require_authenticated_accepts_a_validated_but_unprovisioned_identity` at `:749` green — the test whose own doc warns it is "the one most likely to be 'fixed' back into a 401 by a later reader", so inverting it silently is the worst available outcome); `PermissionDenied` + `principal-inactive` → 401; `PermissionDenied` with no details, API-key leg conclusive → 401; `PermissionDenied` with no details, API-key leg inconclusive → 503.

**Envelope shape (AC 3).** Assert the key set of the **`error` object** — `message`/`type`/`param`/`code` for the gateway, `code`/`message` for IAM — not of the whole body. `system_retirement::conflict` deliberately emits sibling keys next to `error` (`:145`, pinned by `:290`), and the gateway's 413 path returns axum's plain text rather than the envelope at all; both are explicitly out of this assertion's scope.

**No new leakage (AC 4).** Existing "internal errors never leak details" tests stay, extended to assert the gRPC `Internal` message is the static generic one and that no metadata value contains backend text.

**RFC 6750 (AC 7).** One test asserting `WWW-Authenticate` still carries `invalid_token` while the same response's body carries `invalid-token`.

**The layer.** Id headers on a successful response; id headers on an **auth-rejected** response (proves outside `AuthLayer`); the documented tonic-timeout gap asserted explicitly (§4.1); a well-formed inbound `paigasus-correlation-id` adopted verbatim; a malformed one replaced and never echoed anywhere in the response; a client-sent `paigasus-request-id` overwritten; the operational endpoints pinned **per service**, because D10 records deliberately different answers — on IAM, `/healthz` and `/readyz` carry **no** id headers (they are merged above `app_routes`, outside the layer), while on the gateway both carry ids (its router is one flat function and separating them was judged not worth diverging the two services' composition). Each is asserted on its own service, so neither reads as a bug to a future reader of the other. Then: `paigasus-retryable` matching D4's table enum-exhaustively, including the layer's status-class default on a 404 and a 413 that no renderer owns; and on a gRPC error, the header ids equal to the `ErrorInfo.metadata` ids.

**Propagation and linkage.** `IamClient` attaches the current correlation id to outbound metadata (recording fake). `KernelIdGenerator::new_correlation_id` returns the ambient correlation id inside a scope and a fresh one outside it (D9).

## 8. Risks

| Risk | Mitigation |
| -- | -- |
| The layer lands inside auth, so rejections carry no ids | Test asserts headers on an auth-rejected response |
| tonic's own outer layers produce id-less `Status`es | Accepted and documented (§4.1); asserted so it cannot widen silently |
| `current_ids()` is `None` during body streaming | §4.3 states it explicitly; comment at the stream adapter; no error path panics |
| Adopting a caller-supplied correlation id enables log injection | D6: UUID-parse or discard, never echo; `debug!` names the reason, never the value |
| D9 lets a caller group their own audit rows under one id | Accepted (D9); rows keep their own PK and `occurred_at`; `request_id` stays unforgeable |
| Renaming 18 codes across two services misses one | AC 6's enum-driven per-site tests are exhaustive by construction |
| A prefix assertion is deleted rather than converted, and the Docker-gated suite hides it | §7 lists all six sites; verification requires `CI=1` |
| Rolling upgrade: new gateway against old IAM 401s console users | IAM rolls first (release note); `warn!` makes skew visible |
| `error.proto` edit ships stale generated output | `buf generate` directly + the codegen-drift gate |
| New `paigasus-kernel` edge reds `:affected-smoke` | Budgeted in §6: expected-set update plus `dependsOn` and `^:build` on `build` and `test` |

## 9. Out of scope, with reasons

**TypeScript `google.rpc.ErrorInfo` descriptor.** ADR-0019 A1.4 flags the TS path as unverified. Verified here: `ts/` has **no `@connectrpc` dependency and no client code** — the only generated artifact in this area is `error_pb.ts`, the registry enums, which build fine. Nothing in TypeScript consumes a gRPC error until SMA-508 introduces the SDK. Generating a googleapis module now would be speculative work against no consumer.

**Splitting `TenancyError::Internal` into transient and permanent variants.** The durable fix that would let D4's `unknown` collapse to `true`/`false`. It changes IAM's error taxonomy and its `From<RepositoryError>` boundary — a separate issue.

**Putting `retryable` inside the terminal SSE frame's JSON.** Tempting, since the frame is the one error that cannot carry a header and a mid-stream upstream failure is exactly the retryable case. Rejected: the frame is the OpenAI-compatible envelope, and adding a non-OpenAI key to it is precisely the deviation AC 3 forbids. The information is not lost — the frame's `code` is `upstream-error`, which is by construction the transient case, so retryability is derivable from the reason for this one error.

**The two-way drift gate.** SMA-507. This issue delivers its precondition (AC 6) and nothing more. One hand-off note: SMA-507 must decide whether its emitted-side scan covers JSON `code` sites, `ErrorInfo.reason` sites, or both — §5.1.3 exists because a JSON-only sweep missed the gRPC side once already.

**Promoting the HTTP-headers page out of Notion's archive** and documenting `paigasus-retryable` there (D1).
