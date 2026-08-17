# SMA-504 — `google.rpc.ErrorInfo`, correlation ids and retryable metadata

**Issue:** [SMA-504](https://linear.app/smaschek/issue/SMA-504) · **ADR:** ADR-0019 (decisions E1–E6 + Amendment A1)
**Depends on:** SMA-498 (merged, PR 120) · **Unblocks:** SMA-507 (drift gate), SMA-508 (TS SDK)
**Date:** 2026-08-17

## 1. Problem

The error code is machine-readable on exactly one of four surfaces, and only by accident on the rest.

- **IAM tenancy gRPC** stuffs the code in-band: `Status::new(code, format!("{}: {}", e.code(), e))`. A client recovers the code only by splitting on `": "`, which breaks the moment a message legitimately contains that sequence.
- **IAM authn gRPC** (`authn_status`) carries no machine code at all — five static message strings and nothing else.
- **IAM HTTP authn** and the **gateway** emit snake_case codes that do not match the canonical kebab registry SMA-498 landed in `contracts/proto/paigasus/common/v1/error.proto`.
- **No surface carries a correlation id**, which for a self-hosted product means supporting a deployment you cannot access is guesswork.

There is a live consequence, not just a theoretical one. The gateway's `require_authenticated` (SMA-505, ADR-0020 D4) accepts a bare `Code::PermissionDenied` from IAM's `Introspect` as "validated but not yet provisioned". `authn_status` collapses **three** `AuthnError` variants onto that one code (`IdentityNotProvisioned`, `ProvisioningFailed`, `PrincipalInactive`). It is not exploitable today — both other variants are unreachable through `Introspect` — but the accept is blanket, and its doc comment carries an explicit "if either becomes reachable, this arm must narrow" tripwire that only this issue can discharge.

## 2. Goals and non-goals

**Goals**

1. A gRPC client reads `(domain, reason)` from `grpc-status-details-bin` without parsing the message string.
2. Every code emitted by all six emission sites resolves via `ErrorReason::from_wire_reason` — the precondition for SMA-507's emitted-side gate.
3. Every response on both services carries a request id and a correlation id; every *error* response additionally carries a retryability flag.
4. The gateway's `IamError::Rpc` branching is updated in the same change, and the SMA-505 tripwire is discharged.
5. No envelope changes shape. Internal errors leak nothing new.

**Non-goals**

- Generating a `google.rpc.ErrorInfo` descriptor for TypeScript (§9).
- Turning on the two-way drift gate — that is SMA-507, which this issue unblocks.
- Moving the canonical error types into `paigasus-kernel` (ADR-0019 defers this explicitly).
- Wiring OpenTelemetry, `traceparent`, or distributed tracing generally.

## 3. Decisions

### D1 — Two ids, per the Notion HTTP-header guideline

The [HTTP Headers](https://app.notion.com/p/2aa830e8fbaa8004bfb6c7c73a8f224a) guideline defines both, with different jobs:

| Header | Request | Response | Role |
| -- | -- | -- | -- |
| `paigasus-request-id` | Forbidden | Mandatory | Identifies **this individual call**, so repeated or retried calls stay separable. Server-minted; a client-sent value is ignored and overwritten. |
| `paigasus-correlation-id` | Optional | Mandatory | Tracks **one logical invocation** end-to-end across services. Adopted from the caller when well-formed, else minted. |

Both are implemented. The guideline's other three headers (`paigasus-idempotency-key`, `paigasus-locale`, `paigasus-timing`) are out of scope.

> **Note for the reviewer.** That page currently sits under *Paigasus → Legacy / Archive → Development Guidelines → Interservice Communication → REST*, and the live Development Guidelines page has no HTTP-headers section. The names are treated as binding here. Promoting the page out of the archive (and adding `paigasus-retryable`, D4) is a documentation follow-up, not part of this change.

### D2 — Ids surface in headers, never in a JSON body

Both JSON error bodies stay **byte-identical** to today. This satisfies AC 3 and ADR-0019's "all four envelopes stay exactly as they are" literally, keeps the gateway's OpenAI-compatible envelope untouched (SDKs branch on `error.type`), and means no existing body assertion in either service changes for this reason. A console reads the header.

On gRPC the ids ride in `ErrorInfo.metadata` as `correlation_id` and `request_id`, which is where ADR-0019 decision 6 puts them.

### D3 — Ambient request-scoped context, not threaded parameters

`status_to_grpc` is a free function and `ApiError`/`AuthnApiError`/`GatewayError` are `IntoResponse` impls; none has access to the request, and they are called from roughly fifty places. Threading a context parameter through all of them would delete the blanket `impl<E: Into<TenancyError>> From<E> for ApiError` that lets ~40 IAM handlers use `?`, producing a large mechanical diff inside a change that is already 18 renames wide.

Instead a `tokio::task_local!` holds the ids for the duration of the request future. The compile-time guarantee this gives up — "every error site has an id" — is recovered by one integration test per surface (§7).

### D4 — `retryable` is a header on HTTP and metadata on gRPC

ADR-0019 decision 7 puts retryability in `ErrorInfo.metadata`, which exists only on gRPC — yet the gateway's OpenAI-SDK callers are precisely the audience that hard-codes "503 means retry". A `paigasus-retryable: true|false` response header carries the same datum on HTTP without touching either envelope.

The rule is deliberately narrow: **`retryable = true` only for transient dependency failures.**

| Error | retryable |
| -- | -- |
| `AuthnError::Unavailable` | `true` |
| `GatewayError::IamUnavailable` | `true` |
| `GatewayError::UpstreamUnavailable` | `true` |
| `GatewayError::UpstreamTimeout` | `true` |
| everything else, **including every `internal`** | `false` |

A false negative costs a client one give-up. A false positive causes a retry storm against a service that is already failing. `TenancyError` has no transient variant, so all 26 of its codes are `false`.

### D5 — `missing-scope` keeps its own reason

`GatewayError::MissingScope` continues to emit `missing-scope` rather than folding into `internal`. Its response is already indistinguishable from `internal` to a caller (same 500, same `"Internal error."` message, same `api_error` type), so the code is purely a diagnostic for our own logs — and it names the exact plumbing bug it detects (`introspect` returned an empty `scope_prn`). Folding it would lose that signal for no client-visible gain.

### D6 — Inbound correlation ids are adopted only if they parse as a UUID

An adopted value reaches log lines, two response headers and outbound gRPC metadata. Accepting an arbitrary caller-controlled string there is a log-injection and header-splitting surface. A value that does not parse as a UUID is silently replaced with a freshly minted one and is **never echoed anywhere**. `paigasus-request-id` is never read from the request at all.

### D7 — UUIDv7 is minted through `paigasus-kernel`, not `uuid`'s `v7` feature

`paigasus_kernel::mint_uuid7(ms, entropy)` is the repo's single UUIDv7 implementation; `KernelIdGenerator::mint` already calls it with `SystemTime::now()` and `rand::random::<[u8;10]>()`. Reusing it keeps one implementation and preserves the kernel's entropy-free posture (the host supplies both clock and randomness), which is what the `repo:wasm-getrandom-free` gate protects.

Cost, stated up front: this makes `paigasus-observability` a new `paigasus-kernel` dependent, which reds `repo:affected-smoke`'s strict-equality `kernel->bindings` expected set until `ci/affected-graph/run.sh` is updated (SMA-409), and requires **both** a Moon `dependsOn` entry and a task-level `^:build` edge — `cargo_moon_parity.py` asserts both independently (SMA-524).

### D8 — The gateway derives the reason it matches from the registry

`require_authenticated` compares against `ErrorReason::IdentityNotProvisioned.as_wire_reason()`, not the literal `"identity-not-provisioned"`. This makes the gateway the *consumed* half of SMA-507's two-way gate rather than a second hand-maintained copy of the vocabulary.

## 4. Architecture

### 4.1 New module: `paigasus-observability::correlation`

One tower `Layer`/`Service`, generic over the body type, wraps **all three** server surfaces: IAM's axum router, IAM's tonic server, and the gateway's axum router. gRPC metadata *is* HTTP/2 headers, so the same header names are read on both protocols and no second implementation is needed.

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

/// The ids for the in-flight request, or `None` outside a layer scope (a unit test rendering
/// an error directly, or a background task).
pub fn current_ids() -> Option<RequestIds>;

pub const REQUEST_ID_HEADER: &str = "paigasus-request-id";
pub const CORRELATION_ID_HEADER: &str = "paigasus-correlation-id";
pub const RETRYABLE_HEADER: &str = "paigasus-retryable";

pub struct CorrelationLayer;
```

Per request the layer: mints a request id; adopts or mints a correlation id (D6); runs the inner service inside `IDS.scope(ids, ...)`; and on the way out sets `paigasus-request-id` and `paigasus-correlation-id` on the response, overwriting whatever is there.

**Placement is load-bearing.** The layer must be *outermost* — before IAM's gRPC `AuthLayer` and before both services' auth middleware — so a rejected request still carries ids. Layer-ordering semantics differ between `axum::Router::layer` and `tonic::transport::Server::builder().layer`, so this is pinned by a test that asserts the headers are present on an **auth-rejected** response, not by reading the builder docs.

### 4.2 Header ownership

| Header | Set by | Present on |
| -- | -- | -- |
| `paigasus-request-id` | `CorrelationLayer` | every response |
| `paigasus-correlation-id` | `CorrelationLayer` | every response |
| `paigasus-retryable` | the error renderers | error responses only |

`retryable` cannot live in the layer: only the error type knows its own retryability, and by the time the layer sees the response the error is an opaque body. The four renderers that set it are `iam::http::error::ApiError`, `iam::http::authn::{AuthnApiError, envelope_rejection}`, `iam::http::system_retirement::conflict`, and `gateway::http::error::GatewayError`.

The header is **always present on an error response**, carrying the literal `false` where the error is not retryable. A client must never have to read absence as `false` — that is the "clients hard-code 503 means retry" inference this decision exists to remove. On gRPC the `retryable` metadata key is likewise always present.

The gateway's terminal SSE frame is the one exception, and unavoidably so: by the time it is emitted the `200 OK` head has already been sent and no header can change. It carries the canonical `code` only.

### 4.3 Absent context

When `current_ids()` returns `None`, the `correlation_id` / `request_id` metadata keys are **omitted entirely** rather than filled with a nil UUID that would read as a real id, and the HTTP renderers emit no id headers. In production the layer always runs, so this path is reached only by unit tests that construct a response directly — which is also what keeps the existing unit tests compiling unchanged.

### 4.4 Propagation

`IamClient` attaches the current `paigasus-correlation-id` to outbound IAM gRPC metadata, so a single id spans both services' logs. It does **not** forward its request id: IAM mints its own for its own hop, which is what makes the two ids distinguishable in a stitched trace.

## 5. Surface-by-surface changes

### 5.1 IAM gRPC — `adapters/grpc/convert.rs`

```rust
pub fn status_to_grpc(e: TenancyError) -> Status {
    let code = /* unchanged ErrorClass mapping */;
    Status::with_error_details(
        code,
        e.to_string(),                       // no more "{code}: {display}" prefix
        &ErrorDetails::with_error_info(e.code(), &*IAM_DOMAIN, metadata()),
    )
}
```

`reason` is `e.code()` verbatim — it already *is* the canonical wire string, so there is no runtime `from_wire_reason` unwrap; the registry is the validation (§7), not the transform. `IAM_DOMAIN` is a `LazyLock<String>` over `ErrorDomain::Iam.as_wire_domain()`, which is `Some` for every non-sentinel value.

`TenancyError::Internal`'s `Display` is already static and interpolation-free (ADR-0019 D7), so the generic-message requirement holds without a special case — but a test pins it rather than trusting the invariant.

`authn_status` gains the same details with its five canonical codes. Every static message stays byte-identical; only the machine code is new.

### 5.2 The rename inventory — 18 operations, not 17

Every one is currently asserted by a test, so each is a real wire change.

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

Rename 18 is a discovery, not in the issue's table. `GatewayError::StreamingDisabled` was added by SMA-505 *after* SMA-498 seeded the registry from a sweep, and nothing gates the emitted side yet — that is exactly what SMA-507 exists to prevent recurring. AC 6 forces it in: `ERROR_REASON_STREAMING_DISABLED = 308` is appended to the gateway's 300–599 range (307 is the current maximum), and Rust, Python and TypeScript bindings are regenerated.

`system_retirement.rs`'s `grants-survive` and `decision-change-unacknowledged` are already canonical. Its malformed-body rejections route through `authn.rs`'s shared `envelope_rejection`, so renames 6 and 7 cover them.

**RFC 6750 carve-out.** `authn.rs:25`'s `BEARER_CHALLENGE` (`Bearer error="invalid_token"`) is standardised by RFC 6750 §3.1 and is **not ours to rename**. Only the JSON body's `code` becomes `invalid-token`. A test asserts the two diverge (AC 7).

### 5.3 Gateway consumption — `adapters/http/auth.rs`

The blanket `PermissionDenied` accept in `require_authenticated` narrows:

```rust
fn is_identity_not_provisioned(status: &Status) -> bool {
    status.code() == Code::PermissionDenied
        && status.get_error_details().error_info().is_some_and(|info| {
            info.domain == *IAM_DOMAIN
                && Some(info.reason) == ErrorReason::IdentityNotProvisioned.as_wire_reason()
        })
}
```

Absent `ErrorInfo` **fails closed** (401): after this change IAM always emits it, and both services ship from one repo and deploy together, so a new gateway against an old IAM is not a supported configuration — ADR-0019 already accepts the wire-visible break on the grounds that there are no external consumers yet.

The SMA-505 tripwire paragraph in `require_authenticated`'s doc comment is deleted and replaced with a short note on what the narrow accepts and why.

`introspect_error` and `authz_error` branch on `status.code()` only and never read the message — verified against the source — so removing the in-band prefix does not affect them. They gain no changes beyond the shared `retryable` header.

## 6. Dependencies and gates

- **`tonic-types = "0.14"`** — new *production* workspace dep. `paigasus-iam` emits (`ErrorDetails`, `StatusExt::with_error_details`); `paigasus-gateway` consumes (`StatusExt::get_error_details`). It is MIT, already on `rs/deny.toml`'s allow-list, so no `exceptions` entry is expected — to be confirmed by running `:deny`, not assumed. Both crates consume it in this same commit, so no `cargo-machete` allowlist is needed.
- **`paigasus-observability`** gains `tower`, `http`, `uuid`, `rand`, `paigasus-kernel` and `tokio` (for `task_local!`) as real dependencies; `tower` and `tokio` are currently dev-only there.
- **`ci/affected-graph/run.sh`** — add `paigasus-observability` to the `kernel->bindings` expected set (D7).
- **Moon edges** — `paigasus-observability-rs` gets a `dependsOn` on `paigasus-kernel-rs` *and* a task-level `^:build`; `cargo_moon_parity.py` checks each independently.
- **`contracts/`** — `error.proto` gains one enum value. Run `buf format -w` before committing or `contracts:fmt` reds `moon ci` silently, and regenerate with `buf generate` directly rather than `contracts:generate` (which has no `outputs:` and can serve stale cache). The regenerated Rust carries a hex delta in prost's embedded `FILE_DESCRIPTOR_SET`, since that encodes comment text. Appending an enum value is not a `buf breaking` violation.
- Full pre-push graph per CLAUDE.md: `moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free :redis-connect-single-site :promtool :observability-drift :nats-permissions :release-parity :release-parity-py :release-parity-ts :publish-metadata --base origin/main --include-relations`.

## 7. Testing

**AC 6 — every emitted code resolves.** One test per emission site, enum-driven wherever the site is enum-driven so a future variant cannot silently escape the registry. `TenancyError` already has such a test via `strum::EnumIter`; `GatewayError` and `AuthnError` gain `#[cfg_attr(test, derive(strum::EnumIter))]` and the matching test. The terminal SSE frame is a `const` string literal, so its test parses the frame's JSON and resolves the `code` field — not a string comparison against the same literal.

**AC 1 — structured detail.** Build a real `Status` through `status_to_grpc` and `authn_status`, read it back with `StatusExt::get_error_details()`, and assert `domain`, `reason`, and each metadata key. Separately assert `status.message()` does **not** start with `"{code}: "` — the wire change itself.

**AC 2 — gateway narrowing.** Three cases through the existing `FakeIam` decision-table harness: `PermissionDenied` + `identity-not-provisioned` details → request proceeds; `PermissionDenied` + `principal-inactive` details → 401; `PermissionDenied` with **no** details → 401 (fail-closed).

**AC 3 — envelope shape.** Assert the gateway's error body has exactly the keys `message`/`type`/`param`/`code` and IAM's exactly `code`/`message` — a positive key-set assertion, so a stray added field fails rather than passing unnoticed.

**AC 4 — no new leakage.** The existing "internal errors never leak details" tests stay, extended to assert the gRPC `Internal` message is the static generic one and that no metadata value contains backend text.

**AC 7 — RFC 6750.** One test asserting `WWW-Authenticate` still carries `invalid_token` while the same response's body carries `invalid-token`.

**The layer.** Headers present on a successful response; headers present on an **auth-rejected** response (this is what proves outermost placement, on both the axum and tonic sides); a well-formed inbound `paigasus-correlation-id` is adopted verbatim; a malformed one is replaced and never echoed; a client-sent `paigasus-request-id` is overwritten; `paigasus-retryable` is `true` for exactly the four errors in D4's table and `false` for the rest, asserted enum-exhaustively.

**Propagation.** `IamClient` attaches the current correlation id to outbound metadata, asserted against a recording fake.

## 8. Risks

| Risk | Mitigation |
| -- | -- |
| The layer lands *inside* auth, so rejections carry no ids | Test asserts headers on an auth-rejected response, per protocol |
| A task-local read from a spawned task sees `None` | `current_ids()` returns `Option`; §4.3 defines the behaviour; no error path panics |
| Adopting a caller-supplied correlation id enables log injection | D6: UUID-parse or discard, never echo |
| Renaming 18 codes across two services misses one | AC 6's enum-driven per-site tests are exhaustive by construction, not by list |
| `error.proto` edit ships stale generated output | `buf generate` directly (not the cached Moon task) + the codegen-drift gate |
| New `paigasus-kernel` edge reds `:affected-smoke` | Budgeted explicitly in §6; both Moon edges plus the expected-set update |

## 9. Out of scope, with reasons

**TypeScript `google.rpc.ErrorInfo` descriptor.** ADR-0019 A1.4 flags the TS path as unverified. Verified here: `ts/` has **no `@connectrpc` dependency and no client code** — the only generated artifact touching this area is `error_pb.ts`, the registry enums, which build fine today. Nothing in TypeScript consumes a gRPC error until SMA-508 introduces the SDK. Generating a googleapis module now would be speculative work against no consumer, and SMA-508 can answer the question against a real one.

**The two-way drift gate.** SMA-507. This issue delivers its precondition (AC 6) and nothing more.

**Promoting the HTTP-headers page out of Notion's archive** and documenting `paigasus-retryable` there. A documentation follow-up.
