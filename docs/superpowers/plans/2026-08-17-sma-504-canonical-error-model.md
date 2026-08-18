# SMA-504 Canonical Error Model Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make every error code machine-readable on every surface — `google.rpc.ErrorInfo` in gRPC trailers, canonical kebab codes in both HTTP envelopes, plus request/correlation ids and a retryability signal.

**Architecture:** One shared tower layer in `paigasus-observability` mints a request id and adopts-or-mints a correlation id per request, holds both in a `tokio::task_local!` for the request-head future, and stamps response headers. IAM's gRPC surfaces attach `ErrorInfo` via `tonic-types`, reading the ids from that task-local. The gateway consumes IAM's `ErrorInfo` to narrow one over-broad `PermissionDenied` accept, and renames its own emitted codes to the canonical registry spellings.

**Tech Stack:** Rust 2024 / 1.95, tonic 0.14 + tonic-types 0.14, axum, tower, prost, buf, Moon 2.3.2.

**Spec:** `docs/superpowers/specs/2026-08-17-sma-504-canonical-error-model-design.md` (revision 2, approved).

## Global Constraints

- Every source file opens with `// SPDX-License-Identifier: Apache-2.0`.
- Rust crates are **edition 2024 + rust-version 1.95**. Workspace lints are `warnings = deny`, so dead code is a hard compile error — every item you add must be `pub` or used in the same task.
- Conventional commits with a workspace scope. Subject must **start lowercase** and be **≤100 chars**. Never write a bare `#NNN` in a commit body (breaks `footer-leading-blank`); write "owner/repo PR NNN".
- Prefix any command that invokes a **repo-pinned tool** — `moon`, `buf`, `uv`, `cargo`/`nextest` — with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`. Those are proto-managed and off the default PATH, and shims must come **first** or a global pin wins. Plain `git`, `grep`, `jq`, `docker` and `cd` need no prefix.
- Work in the worktree `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-504` on branch `feature/sma-504-errorinfo-correlation-retryable`. Never `cd` to the main checkout.
- **Docker-gated IAM suites silently skip without Docker**, reporting PASS in ~1s having run nothing. Any task that touches `paigasus-iam`'s integration tests MUST verify with `CI=1 cargo nextest run -p paigasus-iam`, which makes a missing daemon a hard failure.
- `cargo nextest` exits non-zero on a crate with no tests — use `--no-tests=pass` where that applies.
- Header names are fixed and case-insensitive on the wire: `paigasus-request-id`, `paigasus-correlation-id`, `paigasus-retryable`.
- Retryable wire values are exactly `"true"`, `"false"`, `"unknown"` — never `"1"`/`"0"`/absent-means-false.
- The domain strings are exactly `iam.paigasus.io` and `gateway.paigasus.io`.

## File Structure

**Created**

| File | Responsibility |
| -- | -- |
| `rs/crates/libs/paigasus-observability/src/correlation.rs` | `RequestIds`, `Retryable`, the task-local, `CorrelationLayer`, header name constants. The only place that mints or adopts an id. |
| `rs/crates/services/paigasus-iam/src/adapters/retryable.rs` | The two IAM error→`Retryable` mappings, in one place so the HTTP and gRPC surfaces cannot drift. |

**Modified**

| File | Change |
| -- | -- |
| `contracts/proto/paigasus/common/v1/error.proto` | 3 appended reasons + metadata-key prose |
| `rs/crates/libs/paigasus-proto/src/error.rs` | `IAM_DOMAIN`/`GATEWAY_DOMAIN` statics; registry test 43→46 |
| `rs/crates/libs/paigasus-observability/src/lib.rs`, `Cargo.toml`, `moon.yml` | export the module; new deps; Moon edges |
| `ci/affected-graph/run.sh` | `kernel->bindings` expected set |
| `rs/crates/services/paigasus-iam/src/adapters/grpc/convert.rs` | `ErrorInfo` on `status_to_grpc`/`authn_status`; new `iam_status` helper |
| `.../grpc/{tenancy,authz,service_accounts,audit}.rs` | 6 bare `Status` sites → `iam_status` |
| `.../grpc/mod.rs` | layer + return type |
| `.../http/{error,authn,system_retirement,mod}.rs` | renames, retryable header, layer wiring |
| `.../adapters/id.rs` | `new_correlation_id` adopts the ambient id |
| `rs/crates/services/paigasus-gateway/src/adapters/http/{error,chat,auth,mod}.rs` | renames, retryable, narrowing, layer wiring |
| `rs/crates/services/paigasus-gateway/src/adapters/iam/client.rs` | correlation-id propagation |

---

### Task 1: Registry appends and domain constants

**Files:**
- Modify: `contracts/proto/paigasus/common/v1/error.proto`
- Modify: `rs/crates/libs/paigasus-proto/src/error.rs`
- Regenerated (committed): `rs/crates/libs/paigasus-proto/src/generated/paigasus/common/v1/paigasus.common.v1.rs`, `py/packages/paigasus-proto/**`, `ts/packages/paigasus-proto/src/generated/paigasus/common/v1/error_pb.ts`

**Interfaces:**
- Consumes: nothing.
- Produces: `paigasus_proto::error::IAM_DOMAIN: LazyLock<String>`, `paigasus_proto::error::GATEWAY_DOMAIN: LazyLock<String>`; enum values `ErrorReason::{StreamingDisabled, MissingAuthContext, CapabilityDisabled}` with wire strings `"streaming-disabled"`, `"missing-auth-context"`, `"capability-disabled"`.

- [ ] **Step 1: Write the failing tests**

In `rs/crates/libs/paigasus-proto/src/error.rs`, add the three new codes to `EXPECTED_REASONS` — `"streaming-disabled"` at the end of the `// Gateway` block, and `"missing-auth-context"` / `"capability-disabled"` at the end of the `// Shared` block — then change the count anchor:

```rust
        assert_eq!(actual.len(), 46, "the registry should hold 46 reasons");
```

Append a new test to the same `mod tests`:

```rust
    /// The domain strings live HERE, next to `as_wire_domain` they are derived from, because the
    /// gateway must compare `ErrorInfo.domain` against IAM's domain and cannot see a constant
    /// private to the IAM crate. A hardcoded copy in the gateway is exactly the second
    /// hand-maintained vocabulary ADR-0019's registry exists to prevent.
    #[test]
    fn the_domain_constants_match_the_registry() {
        assert_eq!(&*crate::error::IAM_DOMAIN, "iam.paigasus.io");
        assert_eq!(&*crate::error::GATEWAY_DOMAIN, "gateway.paigasus.io");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-proto
```

Expected: FAIL — `the_registry_contains_exactly_the_expected_reasons` reports the three codes as "declared in the test but not in the registry", and `the_domain_constants_match_the_registry` fails to compile (`IAM_DOMAIN` not found).

- [ ] **Step 3: Append the three registry values**

In `contracts/proto/paigasus/common/v1/error.proto`, after `ERROR_REASON_UPSTREAM_ERROR = 307;`:

```proto
  // "streaming-disabled" — streamed completions are refused by configuration.
  // A request-PARAMETER refusal (400, param "stream"), not a route-level
  // capability gate — see ERROR_REASON_CAPABILITY_DISABLED for that.
  ERROR_REASON_STREAMING_DISABLED = 308;
```

After `ERROR_REASON_REQUEST_TOO_LARGE = 902;` (still inside the shared 900-999 block):

```proto
  // "missing-auth-context" — the enforcement layer admitted a request without
  // attaching an authenticated context. An internal invariant violation,
  // surfaced as a distinct diagnostic rather than a bare unauthenticated.
  ERROR_REASON_MISSING_AUTH_CONTEXT = 903;
  // "capability-disabled" — the RPC belongs to a capability this deployment
  // has switched off. The capability name rides in
  // ErrorInfo.metadata["capability"], so a new capability needs no new reason.
  ERROR_REASON_CAPABILITY_DISABLED = 904;
```

> Note: the spec sketched `missing-auth-context` in the IAM range at 33. Both are emitted only by IAM today, but neither is IAM-*specific* — any service with an enforcement layer or capability gates emits the same conditions — so both land in the shared range. Shared numbering also means SMA-507's arithmetic domain check does not have to special-case a gateway that later emits them.

Update the file comment (currently lines 36–40) so the prose matches what is emitted:

```proto
// ErrorInfo.metadata carries these standard keys, populated by SMA-504:
// `retryable` (exactly "true" | "false" | "unknown"), `correlation_id` and
// `request_id` (present only when the error was produced inside a request
// scope), and `capability` (only on ERROR_REASON_CAPABILITY_DISABLED). They
// are prose here, not enumerated: metadata is an open map, not a closed
// vocabulary consumers branch on exhaustively. The append-only guarantee
// above covers reasons and domains ONLY, not metadata keys.
```

- [ ] **Step 4: Format and regenerate**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd contracts && buf format -w && buf generate
```

`buf format -w` is not optional — an unformatted `.proto` reds `contracts:fmt` under `moon ci` with no useful message. Use `buf generate` **directly**: the `contracts:generate` Moon task declares no `outputs:` and can serve stale cached output.

- [ ] **Step 5: Add the domain constants**

At the top of `rs/crates/libs/paigasus-proto/src/error.rs`, after the existing `use`:

```rust
use std::sync::LazyLock;

/// The canonical wire `domain` for IAM-produced errors, `"iam.paigasus.io"`.
///
/// Derived from the registry rather than written as a literal, and living here rather than in
/// either service, because BOTH sides need it: IAM to emit, the gateway to match on. A literal
/// in the gateway would be a second hand-maintained copy of the vocabulary (ADR-0019 D8).
/// `as_wire_domain` returns `None` only for the zero sentinel, which `Iam` is not.
pub static IAM_DOMAIN: LazyLock<String> =
    LazyLock::new(|| ErrorDomain::Iam.as_wire_domain().expect("ErrorDomain::Iam is not the Unspecified sentinel"));

/// The canonical wire `domain` for gateway-produced errors, `"gateway.paigasus.io"`. See
/// [`IAM_DOMAIN`].
pub static GATEWAY_DOMAIN: LazyLock<String> =
    LazyLock::new(|| ErrorDomain::Gateway.as_wire_domain().expect("ErrorDomain::Gateway is not the Unspecified sentinel"));
```

- [ ] **Step 6: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-proto
```

Expected: PASS, including `the_registry_contains_exactly_the_expected_reasons` at 46.

- [ ] **Step 7: Verify the generated trees actually changed**

```bash
git status --short
```

Expected: modifications under `rs/crates/libs/paigasus-proto/src/generated/`, `py/`, and `ts/packages/paigasus-proto/src/generated/`. The Rust file carries a hex delta in the embedded `FILE_DESCRIPTOR_SET` because that encodes comment text — if only the `.proto` shows as modified, `buf generate` did not run.

- [ ] **Step 8: Commit**

```bash
git add contracts rs/crates/libs/paigasus-proto py ts
git commit -m "feat(contracts): register streaming-disabled, missing-auth-context, capability-disabled (SMA-504)"
```

---

### Task 2: The correlation module

**Files:**
- Create: `rs/crates/libs/paigasus-observability/src/correlation.rs`
- Modify: `rs/crates/libs/paigasus-observability/src/lib.rs`, `rs/crates/libs/paigasus-observability/Cargo.toml`, `rs/crates/libs/paigasus-observability/moon.yml`
- Modify: `ci/affected-graph/run.sh`

**Interfaces:**
- Consumes: `paigasus_kernel::mint_uuid7(ms: u64, entropy: [u8; 10]) -> Uuid`.
- Produces:
  - `paigasus_observability::correlation::{RequestIds, Retryable, CorrelationLayer, current_ids}`
  - `RequestIds { pub request_id: Uuid, pub correlation_id: Uuid }` — `Debug + Clone + Copy`
  - `Retryable::{Yes, No, Unknown}` with `pub fn as_wire(self) -> &'static str` → `"true"|"false"|"unknown"`, and `pub fn from_status(status: StatusCode) -> Retryable`
  - `pub fn current_ids() -> Option<RequestIds>`
  - `REQUEST_ID_HEADER`, `CORRELATION_ID_HEADER`, `RETRYABLE_HEADER: &str`
  - re-exported from the crate root as `paigasus_observability::{CorrelationLayer, RequestIds, Retryable, current_ids}`

- [ ] **Step 1: Add the dependencies**

In `rs/crates/libs/paigasus-observability/Cargo.toml`, add to `[dependencies]`:

```toml
# SMA-504: the shared correlation layer. `tower` for the Layer/Service impl (was dev-only),
# `http` for the header types the layer manipulates on both the axum and tonic sides,
# `tokio` with `rt` for `task_local!` (LocalKey lives behind that feature), and
# `paigasus-kernel` + `rand` to mint UUIDv7 the one way this repo mints it.
tower.workspace = true
http.workspace = true
tokio = { workspace = true, features = ["rt"] }
paigasus-kernel = { workspace = true }
rand.workspace = true
uuid = { workspace = true }
```

Remove the now-duplicated `tower` and `tokio` lines from `[dev-dependencies]` (keep `tokio`'s `macros` feature by moving it into the `[dependencies]` entry: `features = ["rt", "macros"]` — the existing tests use `#[tokio::test]`).

If `http` is not a workspace dependency, add `http = "1"` to `rs/Cargo.toml`'s `[workspace.dependencies]` next to `tower`.

- [ ] **Step 2: Write the failing tests**

Create `rs/crates/libs/paigasus-observability/src/correlation.rs` containing **only** the test module for now, so the test names exist before the implementation:

```rust
// SPDX-License-Identifier: Apache-2.0

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, Response, StatusCode};
    use tower::{ServiceExt, service_fn};
    use uuid::Uuid;

    /// Runs one request through the layer and hands back the response.
    async fn through_layer(req: Request<Body>) -> Response<Body> {
        let inner = service_fn(|_req: Request<Body>| async {
            // The handler observes the ids the layer entered the scope with.
            let ids = current_ids().expect("the layer must have entered a scope");
            let mut resp = Response::new(Body::empty());
            resp.headers_mut().insert("x-observed-request-id", ids.request_id.to_string().parse().unwrap());
            resp.headers_mut().insert("x-observed-correlation-id", ids.correlation_id.to_string().parse().unwrap());
            Ok::<_, std::convert::Infallible>(resp)
        });
        tower::Layer::layer(&CorrelationLayer, inner).oneshot(req).await.unwrap()
    }

    fn get(headers: &[(&str, &str)]) -> Request<Body> {
        let mut b = Request::builder().uri("/x");
        for (k, v) in headers {
            b = b.header(*k, *v);
        }
        b.body(Body::empty()).unwrap()
    }

    #[tokio::test]
    async fn mints_both_ids_and_sets_both_headers() {
        let resp = through_layer(get(&[])).await;
        let req_id = resp.headers()[REQUEST_ID_HEADER].to_str().unwrap();
        let corr_id = resp.headers()[CORRELATION_ID_HEADER].to_str().unwrap();
        assert!(Uuid::parse_str(req_id).is_ok(), "request id must be a UUID: {req_id}");
        assert!(Uuid::parse_str(corr_id).is_ok(), "correlation id must be a UUID: {corr_id}");
        assert_ne!(req_id, corr_id, "the two ids are independent");
        // The handler saw exactly what the response advertises.
        assert_eq!(resp.headers()["x-observed-request-id"], req_id);
        assert_eq!(resp.headers()["x-observed-correlation-id"], corr_id);
    }

    #[tokio::test]
    async fn adopts_a_well_formed_inbound_correlation_id() {
        let supplied = "0198f2c1-1111-7000-8000-000000000042";
        let resp = through_layer(get(&[(CORRELATION_ID_HEADER, supplied)])).await;
        assert_eq!(resp.headers()[CORRELATION_ID_HEADER], supplied);
        assert_eq!(resp.headers()["x-observed-correlation-id"], supplied);
    }

    /// D6: an adopted value reaches logs, two response headers, outbound gRPC metadata and
    /// persisted audit rows, so anything that is not a UUID is replaced and NEVER echoed.
    #[tokio::test]
    async fn replaces_a_malformed_inbound_correlation_id_and_never_echoes_it() {
        let hostile = "not-a-uuid-<script>";
        let resp = through_layer(get(&[(CORRELATION_ID_HEADER, hostile)])).await;
        let corr_id = resp.headers()[CORRELATION_ID_HEADER].to_str().unwrap();
        assert!(Uuid::parse_str(corr_id).is_ok(), "a malformed id must be replaced, got {corr_id}");
        let rendered = format!("{:?}", resp.headers());
        assert!(!rendered.contains("script"), "the supplied value must never be echoed anywhere");
    }

    /// The guideline marks `paigasus-request-id` Forbidden on requests: server-set, ignored
    /// when sent by the client.
    #[tokio::test]
    async fn overwrites_a_client_supplied_request_id() {
        let supplied = "0198f2c1-2222-7000-8000-000000000042";
        let resp = through_layer(get(&[(REQUEST_ID_HEADER, supplied)])).await;
        assert_ne!(resp.headers()[REQUEST_ID_HEADER], supplied, "a client-sent request id must be overwritten");
    }

    #[test]
    fn current_ids_is_none_outside_a_scope() {
        assert!(current_ids().is_none(), "no scope means no ids — never a nil-UUID stand-in");
    }

    #[test]
    fn retryable_wire_values_are_the_three_documented_strings() {
        assert_eq!(Retryable::Yes.as_wire(), "true");
        assert_eq!(Retryable::No.as_wire(), "false");
        assert_eq!(Retryable::Unknown.as_wire(), "unknown");
    }

    /// The layer's status-class default (D4). It exists so the header is present on error
    /// responses no renderer owns — axum's 404/405, DefaultBodyLimit's 413, TimeoutLayer's 408.
    #[test]
    fn status_class_default_marks_transient_statuses_retryable() {
        for s in [StatusCode::REQUEST_TIMEOUT, StatusCode::TOO_MANY_REQUESTS, StatusCode::BAD_GATEWAY, StatusCode::SERVICE_UNAVAILABLE, StatusCode::GATEWAY_TIMEOUT] {
            assert_eq!(Retryable::from_status(s), Retryable::Yes, "{s} should default retryable");
        }
        assert_eq!(Retryable::from_status(StatusCode::INTERNAL_SERVER_ERROR), Retryable::Unknown);
        assert_eq!(Retryable::from_status(StatusCode::NOT_FOUND), Retryable::No);
        assert_eq!(Retryable::from_status(StatusCode::OK), Retryable::No);
    }

    #[tokio::test]
    async fn the_layer_defaults_retryable_only_when_the_inner_service_did_not_set_it() {
        // Inner sets its own value: the layer must not clobber it.
        let inner = service_fn(|_req: Request<Body>| async {
            let mut resp = Response::new(Body::empty());
            *resp.status_mut() = StatusCode::SERVICE_UNAVAILABLE;
            resp.headers_mut().insert(RETRYABLE_HEADER, "unknown".parse().unwrap());
            Ok::<_, std::convert::Infallible>(resp)
        });
        let resp = tower::Layer::layer(&CorrelationLayer, inner).oneshot(get(&[])).await.unwrap();
        assert_eq!(resp.headers()[RETRYABLE_HEADER], "unknown", "a renderer's value wins over the layer default");

        // Inner sets nothing: the layer fills in the status-class default.
        let bare = service_fn(|_req: Request<Body>| async {
            let mut resp = Response::new(Body::empty());
            *resp.status_mut() = StatusCode::SERVICE_UNAVAILABLE;
            Ok::<_, std::convert::Infallible>(resp)
        });
        let resp = tower::Layer::layer(&CorrelationLayer, bare).oneshot(get(&[])).await.unwrap();
        assert_eq!(resp.headers()[RETRYABLE_HEADER], "true", "an unowned 503 still carries a signal");
    }

    #[tokio::test]
    async fn a_success_carries_ids_but_no_retryable_header() {
        let resp = through_layer(get(&[])).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(resp.headers().get(RETRYABLE_HEADER).is_none(), "retryable is an error-response concern");
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-observability
```

Expected: FAIL to compile — `CorrelationLayer`, `current_ids`, `Retryable`, and the three header constants do not exist.

- [ ] **Step 4: Implement the module**

Prepend to `rs/crates/libs/paigasus-observability/src/correlation.rs`, above the test module:

```rust
//! Request-scoped correlation: the two ids every Paigasus response carries, and the tower layer
//! that mints, adopts and stamps them.
//!
//! One `Layer`, generic over the body type, wraps IAM's axum API router, IAM's tonic server AND
//! the gateway's axum router — gRPC metadata *is* HTTP/2 headers, so both protocols read the same
//! header names and no second implementation is needed.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::{SystemTime, UNIX_EPOCH};

use http::{HeaderValue, Request, Response, StatusCode};
use tower::{Layer, Service};
use uuid::Uuid;

/// Server-minted, never read from the request (the guideline marks it Forbidden inbound).
pub const REQUEST_ID_HEADER: &str = "paigasus-request-id";
/// Adopted from the caller when it parses as a UUID, else minted.
pub const CORRELATION_ID_HEADER: &str = "paigasus-correlation-id";
/// `"true"` | `"false"` | `"unknown"` — set by an error renderer, or defaulted by the layer.
pub const RETRYABLE_HEADER: &str = "paigasus-retryable";

/// The two request-scoped ids.
///
/// `request_id` identifies THIS individual call — server-minted, never read from the client — so
/// repeated or retried calls stay separable. `correlation_id` tracks ONE logical invocation
/// end-to-end across services, adopted from the caller when it parses as a UUID (D6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestIds {
    pub request_id: Uuid,
    pub correlation_id: Uuid,
}

/// Whether a client should retry. Three states, not two, and deliberately so: an `internal`
/// error erases whether its source was a transient backend blip or a logic bug, and claiming
/// `false` there would be WORSE than the status-class guess this header replaces (ADR-0019 D7,
/// spec D4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Retryable {
    Yes,
    No,
    Unknown,
}

impl Retryable {
    /// The exact wire spelling. Never `"1"`/`"0"` — clients match these three literals.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Yes => "true",
            Self::No => "false",
            Self::Unknown => "unknown",
        }
    }

    /// The layer's fallback for an error response no renderer owns (axum's 404/405,
    /// `DefaultBodyLimit`'s 413, `TimeoutLayer`'s 408). This IS status-class inference — the very
    /// thing the header exists to remove — and it is used only where the alternative is an absent
    /// header a client would have to interpret.
    pub fn from_status(status: StatusCode) -> Self {
        match status {
            StatusCode::REQUEST_TIMEOUT | StatusCode::TOO_MANY_REQUESTS | StatusCode::BAD_GATEWAY | StatusCode::SERVICE_UNAVAILABLE | StatusCode::GATEWAY_TIMEOUT => Self::Yes,
            s if s.is_server_error() => Self::Unknown,
            _ => Self::No,
        }
    }
}

tokio::task_local! {
    static IDS: RequestIds;
}

/// The ids for the in-flight request HEAD, or `None` outside that scope.
///
/// `None` in three situations, not one: unit tests that render directly; background tasks; and
/// **response-body streaming** — a `task_local!` scope set in `Service::call` ends when the future
/// resolves to `Response<Body>`, and hyper polls the body afterwards. Callers omit the id fields
/// entirely rather than substituting a nil UUID that would read as a real id.
pub fn current_ids() -> Option<RequestIds> {
    IDS.try_with(|ids| *ids).ok()
}

/// Mints a UUIDv7 the one way this repo mints one: `paigasus-kernel` does the layout, the host
/// supplies clock and entropy. NOT `uuid`'s `v7` feature — enabling it anywhere in `rs/` enables
/// `uuid/rng` (and `getrandom`) for every `uuid` dependent under feature unification, including
/// `paigasus-kernel`, whose wasm story depends on staying feature-free.
fn mint() -> Uuid {
    let ms = SystemTime::now().duration_since(UNIX_EPOCH).expect("system clock before 1970").as_millis() as u64;
    paigasus_kernel::mint_uuid7(ms, rand::random::<[u8; 10]>())
}

/// Reads an inbound correlation id, accepting it ONLY if it parses as a UUID (D6).
fn adopt_or_mint<B>(req: &Request<B>) -> Uuid {
    // `HeaderMap::get` takes the FIRST value; duplicates are ignored, same posture as discarding
    // a malformed one.
    let Some(raw) = req.headers().get(CORRELATION_ID_HEADER) else {
        return mint();
    };
    match raw.to_str().ok().and_then(|s| Uuid::parse_str(s).ok()) {
        Some(id) => id,
        None => {
            // Names the rejection, NEVER the value: an adopted value would reach logs, response
            // headers, gRPC metadata and audit rows, which is the whole reason it is rejected.
            tracing::debug!("discarded a malformed inbound {CORRELATION_ID_HEADER}; minted a fresh one");
            mint()
        }
    }
}

/// The layer. See the module docs for why one implementation covers axum and tonic both.
#[derive(Debug, Clone, Copy, Default)]
pub struct CorrelationLayer;

impl<S> Layer<S> for CorrelationLayer {
    type Service = CorrelationService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        CorrelationService { inner }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CorrelationService<S> {
    inner: S,
}

impl<S, ReqBody, ResBody> Service<Request<ReqBody>> for CorrelationService<S>
where
    S: Service<Request<ReqBody>, Response = Response<ResBody>> + Clone + Send + 'static,
    S::Future: Send + 'static,
    ReqBody: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<ReqBody>) -> Self::Future {
        let ids = RequestIds {
            request_id: mint(),
            correlation_id: adopt_or_mint(&req),
        };
        // `Clone` + `replace`: the `self.inner` that was `poll_ready`ed is the one that must be
        // called, so swap a fresh clone in rather than calling the clone (the standard tower
        // readiness dance).
        let clone = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);
        Box::pin(async move {
            let mut resp = IDS.scope(ids, inner.call(req)).await?;
            let status = resp.status();
            let headers = resp.headers_mut();
            // Unconditional: a downstream value for either id is not authoritative.
            headers.insert(REQUEST_ID_HEADER, header_value(ids.request_id));
            headers.insert(CORRELATION_ID_HEADER, header_value(ids.correlation_id));
            // Retryable: fill in a default ONLY where a renderer supplied none, and only on an
            // error — `Retryable::as_wire` returns `&'static str`, which is what `from_static`
            // wants, so this allocates nothing.
            let is_error = status.is_client_error() || status.is_server_error();
            if is_error && !headers.contains_key(RETRYABLE_HEADER) {
                headers.insert(RETRYABLE_HEADER, HeaderValue::from_static(Retryable::from_status(status).as_wire()));
            }
            Ok(resp)
        })
    }
}

/// A `Uuid`'s hyphenated form is always a valid header value, so this never fails.
fn header_value(id: Uuid) -> HeaderValue {
    HeaderValue::from_str(&id.to_string()).expect("a hyphenated UUID is a valid header value")
}

/// Enters an id scope directly. Production code enters it via [`CorrelationLayer`]; this exists
/// for tests, which must be able to assert what a renderer emits INSIDE a request scope without
/// standing up a server. Tasks 7 and 8 both depend on it.
pub async fn scope_for_test<F: Future>(ids: RequestIds, f: F) -> F::Output {
    IDS.scope(ids, f).await
}
```

- [ ] **Step 5: Export from the crate root**

In `rs/crates/libs/paigasus-observability/src/lib.rs`, add to the module list and re-exports:

```rust
pub mod correlation;
```

```rust
pub use correlation::{CorrelationLayer, RequestIds, Retryable, current_ids};
```

- [ ] **Step 6: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-observability
```

Expected: PASS — nine tests in `correlation::tests` plus the two pre-existing ones.

- [ ] **Step 7: Declare the new Moon edge**

`paigasus-observability` now depends on `paigasus-kernel`. Moon 2.3.2 resolves `path =` Cargo deps automatically but **not** `workspace = true` inheritance, which is the form this repo uses — so the edge must be hand-declared, and the project edge alone is not enough: `dependsOn` is what `--affected` follows, and a task-level `^:build` is what actually schedules the upstream build.

Replace `rs/crates/libs/paigasus-observability/moon.yml` with:

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

id: 'paigasus-observability-rs'
layer: 'library'
language: 'rust'

dependsOn:
  # SMA-504: the correlation layer mints UUIDv7 via paigasus_kernel::mint_uuid7.
  - 'paigasus-kernel-rs'

tasks:
  build:
    deps: ['^:build']
  test:
    deps: ['^:build']
```

`lint` already inherits `^:build` from `.moon/tasks/rust.yml`; `cargo_moon_parity.py` checks `build`, `test` and `lint` independently.

- [ ] **Step 8: Update the affected-graph expected set**

In `ci/affected-graph/run.sh`, the `kernel->bindings` case uses **strict equality**, so a new kernel dependent reds it until listed. Append `paigasus-observability-rs` to that case's expected list and extend the comment above it:

```bash
  # + paigasus-observability-rs, whose correlation layer mints UUIDv7 via the kernel (SMA-504).
```

- [ ] **Step 9: Verify the graph gates pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:affected-smoke
```

Expected: PASS, including the `kernel->bindings` case and `cargo_moon_parity.py`'s generic per-crate assertions.

- [ ] **Step 10: Commit**

```bash
git add rs/crates/libs/paigasus-observability rs/Cargo.toml rs/Cargo.lock ci/affected-graph/run.sh
git commit -m "feat(rs): add the shared request-id and correlation-id tower layer (SMA-504)"
```

---

### Task 3: Wire the layer into all three server surfaces

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/mod.rs:849` (`app_routes`)
- Modify: `rs/crates/services/paigasus-iam/src/adapters/grpc/mod.rs:62-67` (`router`)
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/http/mod.rs:103` (`router`)
- Test: `rs/crates/services/paigasus-iam/tests/correlation_headers.rs` (create), `rs/crates/services/paigasus-gateway/src/adapters/http/mod.rs` tests

**Interfaces:**
- Consumes: `paigasus_observability::CorrelationLayer` (Task 2).
- Produces: every API-surface response on both services carries `paigasus-request-id` and `paigasus-correlation-id`. `grpc::router`'s return type becomes `TonicRouter<Stack<AuthLayer, Stack<CorrelationLayer, Identity>>>`.

- [ ] **Step 1: Write the failing tests**

Create `rs/crates/services/paigasus-iam/tests/correlation_headers.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! SMA-504 D10: the correlation layer attaches exactly where `http_metrics_layer` attaches —
//! around `app_routes` — so the `oneshot` harness exercises it and the operational endpoints
//! stay outside it.

mod support;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

const REQUEST_ID: &str = "paigasus-request-id";
const CORRELATION_ID: &str = "paigasus-correlation-id";

#[tokio::test]
async fn an_auth_rejected_request_still_carries_both_ids() {
    let Some(ctx) = support::start_migrated_postgres().await else {
        return;
    };
    let app = paigasus_iam::adapters::http::router(ctx.state.clone());
    // No Authorization header: the bearer layer rejects with 401 BEFORE any handler runs. If the
    // correlation layer were inside that middleware rather than outside it, this response would
    // carry no ids — which is the whole point of the assertion.
    let resp = app
        .oneshot(Request::builder().uri("/v1/organizations").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert!(resp.headers().contains_key(REQUEST_ID), "a rejected request must still be attributable");
    assert!(resp.headers().contains_key(CORRELATION_ID));
}

/// D10: `/healthz` and `/readyz` are operational endpoints, deliberately outside every layer
/// (`readyz_router` is merged at the top level). Pinned so the narrowing is a decision rather
/// than an accident someone later "fixes".
#[tokio::test]
async fn the_operational_endpoints_carry_no_ids() {
    let Some(ctx) = support::start_migrated_postgres().await else {
        return;
    };
    let app = paigasus_iam::adapters::http::router(ctx.state.clone());
    let resp = app.oneshot(Request::builder().uri("/healthz").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(!resp.headers().contains_key(REQUEST_ID), "/healthz is outside the API surface (D10)");
}
```

Add to the gateway's `adapters/http/mod.rs` test module:

```rust
    /// SMA-504 D10: the correlation layer attaches where `http_metrics_layer` attaches, so every
    /// API response carries both ids — including a 401 from the auth middleware.
    #[tokio::test]
    async fn every_api_response_carries_both_ids() {
        let app = router(test_state());
        // No credential: `require_iam_auth` rejects at the middleware, before any handler.
        let resp = app
            .oneshot(axum::http::Request::builder().method("POST").uri("/v1/chat/completions").body(axum::body::Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::UNAUTHORIZED);
        assert!(resp.headers().contains_key("paigasus-request-id"));
        assert!(resp.headers().contains_key("paigasus-correlation-id"));
        assert_eq!(resp.headers()["paigasus-retryable"], "false", "a 401 is not retryable");
    }
```

If `test_state()` does not already exist in that module, build the `AppState` the same way the neighbouring tests in that file do.

- [ ] **Step 2: Run the tests to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && CI=1 cargo nextest run -p paigasus-iam -E 'test(correlation_headers)' && cargo nextest run -p paigasus-gateway -E 'test(every_api_response_carries_both_ids)'
```

Expected: FAIL — the header assertions fail because no layer is attached. `CI=1` is mandatory for the IAM run: without it, a missing Docker daemon makes `start_migrated_postgres` return `None` and the test `return`s early, reporting a **PASS having run nothing**.

- [ ] **Step 3: Attach the layer to IAM's HTTP API surface**

In `rs/crates/services/paigasus-iam/src/adapters/http/mod.rs`, in `app_routes`, add the layer immediately outside the metrics layer:

```rust
    Router::new()
        .merge(protected)
        .merge(authn_api)
        .merge(api_key_introspect_api)
        .layer(paigasus_observability::http_metrics_layer("iam"))
        // SMA-504 D10: outside the metrics layer and therefore outside the bearer middleware, so
        // a 401 is still attributable. Attached HERE rather than in `serve_http` for the same
        // reason the bearer layer is: `router()` is what the `oneshot` test harness builds, and a
        // layer added in `serve_http` would be invisible to every existing integration test.
        // `/healthz`, `/readyz` and `/metrics` are merged ABOVE this router and stay outside.
        .layer(paigasus_observability::CorrelationLayer)
```

- [ ] **Step 4: Attach the layer to the gateway's router**

In `rs/crates/services/paigasus-gateway/src/adapters/http/mod.rs`, in `router`:

```rust
        .layer(paigasus_observability::http_metrics_layer("gateway"))
        // SMA-504 D10: mirrors IAM's placement exactly — outermost, so an auth rejection is
        // attributable. Note this router DOES include /healthz and /readyz (unlike IAM's, which
        // merges them above `app_routes`), so they carry ids here; harmless and not worth
        // diverging the two services' composition for.
        .layer(paigasus_observability::CorrelationLayer)
        .with_state(state)
```

- [ ] **Step 5: Attach the layer to IAM's tonic server**

In `rs/crates/services/paigasus-iam/src/adapters/grpc/mod.rs`, add the import and the layer, and update the return type:

```rust
pub async fn router(state: AppState, timeout: std::time::Duration) -> TonicRouter<Stack<AuthLayer, Stack<CorrelationLayer, Identity>>> {
    let (_reporter, health) = health_service().await;
    let audit_enabled = state.capabilities.audit_query;
    let mut router = Server::builder()
        .timeout(timeout)
        // SMA-504: applied BEFORE `AuthLayer`, so it is outermost among OUR layers and a bearer
        // rejection still carries ids. It is NOT outermost overall: tonic wraps the whole user
        // stack in RecoverError/LoadShed/ConcurrencyLimit/GrpcTimeout, so a `Server::timeout`
        // Status is produced outside this layer and carries no ids and no ErrorInfo. Accepted
        // gap — closing it would mean reimplementing tonic's timeout.
        .layer(CorrelationLayer)
        .layer(AuthLayer::new(state.clone()))
```

Update the doc comment above `router` (currently line 55-61) to mention the correlation layer alongside `AuthLayer`, and import `paigasus_observability::CorrelationLayer`.

- [ ] **Step 6: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && CI=1 cargo nextest run -p paigasus-iam -E 'test(correlation_headers)' && cargo nextest run -p paigasus-gateway
```

Expected: PASS.

- [ ] **Step 7: Verify nothing else regressed**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && CI=1 cargo nextest run -p paigasus-iam -p paigasus-gateway
```

Expected: PASS. The seven gRPC integration tests call `grpc::router` and are unaffected by its type change (they never name the type).

- [ ] **Step 8: Commit**

```bash
git add rs/crates/services/paigasus-iam rs/crates/services/paigasus-gateway
git commit -m "feat(rs): stamp request and correlation ids on every api response (SMA-504)"
```

---

### Task 4: `ErrorInfo` on every IAM gRPC status

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/adapters/retryable.rs`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/mod.rs` (declare the module)
- Modify: `rs/crates/services/paigasus-iam/src/adapters/grpc/convert.rs:31-68`
- Modify: `.../grpc/tenancy.rs:79`, `.../grpc/authz.rs:64,91`, `.../grpc/service_accounts.rs:69,104`, `.../grpc/audit.rs:51`
- Modify: `rs/crates/services/paigasus-iam/Cargo.toml`, `rs/Cargo.toml`
- Test: `.../grpc/convert.rs` tests, `rs/crates/services/paigasus-iam/tests/{grpc_tenancy,grpc_authz,grpc_audit}.rs`

**Interfaces:**
- Consumes: `paigasus_proto::error::IAM_DOMAIN` (Task 1); `paigasus_observability::{Retryable, current_ids}` (Task 2).
- Produces:
  - `crate::adapters::retryable::{tenancy_retryable, authn_retryable}`
  - `crate::adapters::grpc::convert::iam_status(code: Code, reason: &str, message: &'static str, extra: &[(&str, &str)]) -> Status`
  - Every `Status` IAM constructs carries `ErrorInfo { domain, reason, metadata }`.

- [ ] **Step 1: Add `tonic-types`**

`rs/Cargo.toml` `[workspace.dependencies]`, next to `tonic-prost`:

```toml
# tonic-types — google.rpc.* richer-error details (ErrorDetails/StatusExt). Production dep on
# BOTH services: iam emits ErrorInfo, the gateway reads it (SMA-504, ADR-0019 decision 4).
tonic-types = "0.14"
```

`rs/crates/services/paigasus-iam/Cargo.toml` `[dependencies]`:

```toml
tonic-types = { workspace = true }
```

- [ ] **Step 2: Write the failing tests**

Create `rs/crates/services/paigasus-iam/src/adapters/retryable.rs` with only its test module:

```rust
// SPDX-License-Identifier: Apache-2.0

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::error::{ErrorClass, TenancyError};
    use paigasus_iam_core::AuthnError;
    use paigasus_observability::Retryable;
    use strum::IntoEnumIterator;

    /// D4: `TenancyError` has NO transient variant — every one of its 26 codes is a client-actionable
    /// failure except `Internal`, which erases whether its source was a Postgres blip or a bug.
    #[test]
    fn every_tenancy_error_maps_to_no_except_internal() {
        for err in TenancyError::iter() {
            let want = if matches!(err.class(), ErrorClass::Internal) { Retryable::Unknown } else { Retryable::No };
            assert_eq!(tenancy_retryable(err.class()), want, "{err:?}");
        }
    }

    /// `AuthnError` lives in `paigasus-iam-core`, so a `cfg(test)` `EnumIter` derive there would
    /// NOT be visible when THIS crate's tests compile. The exhaustive `match` below is the
    /// dependency-free equivalent: adding a variant upstream fails this file to compile.
    fn all_authn_errors() -> Vec<AuthnError> {
        use paigasus_iam_core::{ProvisioningDefect, TokenDefect};
        let all = vec![
            AuthnError::InvalidToken(TokenDefect::Malformed),
            AuthnError::IdentityNotProvisioned,
            AuthnError::ProvisioningFailed(ProvisioningDefect::MissingEmail),
            AuthnError::PrincipalInactive,
            AuthnError::Unavailable,
            AuthnError::Backend("x".into()),
        ];
        // Exhaustiveness guard: no wildcard arm, so a new variant is a compile error here.
        for e in &all {
            match e {
                AuthnError::InvalidToken(_)
                | AuthnError::IdentityNotProvisioned
                | AuthnError::ProvisioningFailed(_)
                | AuthnError::PrincipalInactive
                | AuthnError::Unavailable
                | AuthnError::Backend(_) => {}
            }
        }
        all
    }

    #[test]
    fn only_the_unavailable_authn_error_is_retryable() {
        for err in all_authn_errors() {
            let want = match &err {
                AuthnError::Unavailable => Retryable::Yes,
                AuthnError::Backend(_) => Retryable::Unknown,
                _ => Retryable::No,
            };
            assert_eq!(authn_retryable(&err), want, "{err:?}");
        }
    }
}
```

Replace the two existing tests in `.../grpc/convert.rs`'s test module that pin the in-band prefix, and add the round-trip:

```rust
    #[test]
    fn forbidden_maps_to_permission_denied_with_structured_detail() {
        use tonic_types::StatusExt;

        let status = status_to_grpc(TenancyError::Forbidden);
        assert_eq!(status.code(), Code::PermissionDenied);
        // The wire change itself: the message is PURELY human-readable now. `Forbidden`'s Display
        // is static (SMA-444 task-16 brief), so no denying-policy detail reaches the wire either.
        assert_eq!(status.message(), "access denied");
        assert!(!status.message().starts_with("forbidden:"), "the in-band code prefix is gone (ADR-0019 decision 4)");

        let details = status.get_error_details();
        let info = details.error_info().expect("every IAM status carries ErrorInfo");
        assert_eq!(info.domain, *paigasus_proto::error::IAM_DOMAIN);
        assert_eq!(info.reason, "forbidden");
        assert_eq!(info.metadata.get("retryable").map(String::as_str), Some("false"));
    }

    /// §4.3: outside a request scope the id keys are OMITTED, never filled with a nil UUID that
    /// would read as a real id in a support ticket.
    #[test]
    fn the_id_metadata_keys_are_absent_outside_a_request_scope() {
        use tonic_types::StatusExt;

        let status = status_to_grpc(TenancyError::NotFound);
        let details = status.get_error_details();
        let info = details.error_info().expect("ErrorInfo");
        assert!(!info.metadata.contains_key("correlation_id"));
        assert!(!info.metadata.contains_key("request_id"));
    }

    /// AC 4: an internal error's gRPC message is the static generic one, and nothing in the
    /// metadata carries backend text.
    #[test]
    fn internal_carries_a_generic_message_and_an_unknown_retryable() {
        use tonic_types::StatusExt;

        let status = status_to_grpc(TenancyError::Internal);
        assert_eq!(status.code(), Code::Internal);
        assert_eq!(status.message(), "internal server error");
        let details = status.get_error_details();
        let info = details.error_info().expect("ErrorInfo");
        assert_eq!(info.reason, "internal");
        assert_eq!(info.metadata.get("retryable").map(String::as_str), Some("unknown"));
    }

    /// AC 6 for the authn funnel: five codes, all registry-resolvable, messages unchanged.
    #[test]
    fn every_authn_status_carries_a_registered_reason_and_its_original_message() {
        use paigasus_iam_core::{ProvisioningDefect, TokenDefect};
        use paigasus_proto::paigasus::common::v1::ErrorReason;
        use tonic_types::StatusExt;

        let cases = [
            (AuthnError::InvalidToken(TokenDefect::Malformed), Code::Unauthenticated, "invalid-token", "invalid bearer token"),
            (AuthnError::IdentityNotProvisioned, Code::PermissionDenied, "identity-not-provisioned", "identity not provisioned"),
            (AuthnError::ProvisioningFailed(ProvisioningDefect::MissingEmail), Code::PermissionDenied, "provisioning-failed", "provisioning failed"),
            (AuthnError::PrincipalInactive, Code::PermissionDenied, "principal-inactive", "principal inactive"),
            (AuthnError::Unavailable, Code::Unavailable, "authn-unavailable", "authentication backend unavailable"),
            (AuthnError::Backend("secret db detail".into()), Code::Internal, "internal", "internal error"),
        ];
        for (err, code, reason, message) in cases {
            let status = authn_status(&err);
            assert_eq!(status.code(), code, "{reason}");
            assert_eq!(status.message(), message, "authn messages are static and unchanged (D12)");
            assert!(ErrorReason::from_wire_reason(reason).is_some(), "{reason} must be in the registry");
            let details = status.get_error_details();
            let info = details.error_info().expect("ErrorInfo");
            assert_eq!(info.reason, reason);
            assert!(!format!("{:?}", info.metadata).contains("secret db detail"), "metadata must never carry backend text");
        }
    }

    /// AC 6 for the six sites that build a bare `Status` — the gap SMA-498's HTTP-only sweep
    /// missed. Both capability gates are here because they are exactly what an SDK branches on.
    #[test]
    fn the_bare_status_sites_carry_registered_reasons() {
        use paigasus_proto::paigasus::common::v1::ErrorReason;
        use tonic_types::StatusExt;

        let missing = missing_auth_context();
        assert_eq!(missing.code(), Code::Unauthenticated);
        let details = missing.get_error_details();
        assert_eq!(details.error_info().expect("ErrorInfo").reason, "missing-auth-context");
        assert!(ErrorReason::from_wire_reason("missing-auth-context").is_some());

        let disabled = capability_disabled("iam.apikeys");
        assert_eq!(disabled.code(), Code::Unimplemented);
        let details = disabled.get_error_details();
        let info = details.error_info().expect("ErrorInfo");
        assert_eq!(info.reason, "capability-disabled");
        assert_eq!(info.metadata.get("capability").map(String::as_str), Some("iam.apikeys"));
        assert!(ErrorReason::from_wire_reason("capability-disabled").is_some());
    }
```

- [ ] **Step 3: Run the tests to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam -E 'kind(lib)'
```

Expected: FAIL to compile — `tenancy_retryable`, `authn_retryable`, `missing_auth_context`, `capability_disabled` do not exist, and `get_error_details` is not in scope.

- [ ] **Step 4: Implement the retryable mappings**

Prepend to `rs/crates/services/paigasus-iam/src/adapters/retryable.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! IAM's error → [`Retryable`] mappings, in ONE place so the HTTP and gRPC surfaces cannot
//! disagree about whether the same error is worth retrying (spec D4).

use paigasus_iam_core::AuthnError;
use paigasus_observability::Retryable;

use crate::application::error::ErrorClass;

/// `TenancyError` has no transient variant: everything but `Internal` is a client-actionable
/// failure, and `Internal` absorbs `RepositoryError::Backend` alongside genuine bugs with the
/// source erased at conversion — so it is honestly `Unknown`, not a confident `No`.
pub(crate) fn tenancy_retryable(class: ErrorClass) -> Retryable {
    match class {
        ErrorClass::Internal => Retryable::Unknown,
        ErrorClass::Validation | ErrorClass::NotFound | ErrorClass::Conflict | ErrorClass::Precondition | ErrorClass::Forbidden => Retryable::No,
    }
}

/// `Unavailable` is the one authn error that names a transient dependency failure. `Backend`
/// is `Unknown` for the same reason `TenancyError::Internal` is.
pub(crate) fn authn_retryable(err: &AuthnError) -> Retryable {
    match err {
        AuthnError::Unavailable => Retryable::Yes,
        AuthnError::Backend(_) => Retryable::Unknown,
        AuthnError::InvalidToken(_) | AuthnError::IdentityNotProvisioned | AuthnError::ProvisioningFailed(_) | AuthnError::PrincipalInactive => Retryable::No,
    }
}
```

Declare it in `rs/crates/services/paigasus-iam/src/adapters/mod.rs`:

```rust
pub(crate) mod retryable;
```

- [ ] **Step 5: Implement the gRPC detail attachment**

In `.../grpc/convert.rs`, add imports and replace `status_to_grpc` / `authn_status`, then add the shared helpers:

```rust
use std::collections::HashMap;
use std::sync::LazyLock;

use paigasus_observability::{Retryable, current_ids};
use paigasus_proto::error::IAM_DOMAIN;
use paigasus_proto::paigasus::common::v1::ErrorReason;
use tonic_types::{ErrorDetails, StatusExt};

use crate::adapters::retryable::{authn_retryable, tenancy_retryable};

/// Derived from the registry, not written as a literal, and hoisted because `as_wire_reason`
/// allocates on every call (ADR-0019 D8).
static MISSING_AUTH_CONTEXT: LazyLock<String> =
    LazyLock::new(|| ErrorReason::MissingAuthContext.as_wire_reason().expect("a declared reason is never the sentinel"));
static CAPABILITY_DISABLED: LazyLock<String> =
    LazyLock::new(|| ErrorReason::CapabilityDisabled.as_wire_reason().expect("a declared reason is never the sentinel"));

/// The `ErrorInfo.metadata` every IAM gRPC error carries.
///
/// The id keys are OMITTED when there is no request scope (§4.3 — unit tests, background tasks
/// and response-body streaming) rather than filled with a nil UUID that would read as a real id.
fn error_metadata(retryable: Retryable, extra: &[(&str, &str)]) -> HashMap<String, String> {
    let mut metadata = HashMap::new();
    metadata.insert("retryable".to_owned(), retryable.as_wire().to_owned());
    if let Some(ids) = current_ids() {
        metadata.insert("correlation_id".to_owned(), ids.correlation_id.to_string());
        metadata.insert("request_id".to_owned(), ids.request_id.to_string());
    }
    for (k, v) in extra {
        metadata.insert((*k).to_owned(), (*v).to_owned());
    }
    metadata
}

/// Builds a `Status` carrying `google.rpc.ErrorInfo` in the `grpc-status-details-bin` trailer.
/// The single construction point for every IAM gRPC error, so no site can forget the details.
pub fn iam_status(code: Code, reason: &str, message: impl Into<String>, retryable: Retryable, extra: &[(&str, &str)]) -> Status {
    let details = ErrorDetails::with_error_info(reason, &*IAM_DOMAIN, error_metadata(retryable, extra));
    Status::with_error_details(code, message, &details)
}

/// The enforcement layer admitted a request without attaching an authenticated context — an
/// internal invariant violation, surfaced as a distinct diagnostic rather than a bare
/// unauthenticated with no machine code at all.
pub fn missing_auth_context() -> Status {
    iam_status(Code::Unauthenticated, &MISSING_AUTH_CONTEXT, "missing authentication context", Retryable::Unknown, &[])
}

/// An RPC belonging to a capability this deployment has switched off. The capability NAME rides
/// in metadata rather than in the reason, so a new capability needs no new registry value.
pub fn capability_disabled(capability: &str) -> Status {
    iam_status(
        Code::Unimplemented,
        &CAPABILITY_DISABLED,
        format!("capability {capability} is not enabled on this service"),
        Retryable::No,
        &[("capability", capability)],
    )
}
```

Rewrite the two existing functions (replace their bodies, and rewrite their doc comments — the current ones describe the removed in-band prefix and would become lies):

```rust
/// Maps a `TenancyError` to a `tonic::Status`: the gRPC code follows `ErrorClass`, the message is
/// purely human-readable, and the machine-readable `(domain, reason)` rides in `ErrorInfo` in the
/// `grpc-status-details-bin` trailer (ADR-0019 decision 4). The old `"{code}: {display}"` prefix
/// is GONE — clients read `ErrorInfo.reason`, never the message. `Internal`'s `Display` never
/// carries interpolated data (D7), so this never leaks backend detail either.
pub fn status_to_grpc(e: TenancyError) -> Status {
    let code = match e.class() {
        ErrorClass::Validation => Code::InvalidArgument,
        ErrorClass::NotFound => Code::NotFound,
        ErrorClass::Conflict => Code::AlreadyExists,
        ErrorClass::Precondition => Code::FailedPrecondition,
        ErrorClass::Forbidden => Code::PermissionDenied,
        ErrorClass::Internal => {
            tracing::error!(error = %e, code = e.code(), "internal error handling gRPC request");
            Code::Internal
        }
    };
    // `e.code()` IS the canonical wire string — the registry is the validation (see the
    // `every_tenancy_code_is_declared_in_the_canonical_registry` test), not the transform.
    iam_status(code, e.code(), e.to_string(), tenancy_retryable(e.class()), &[])
}
```

```rust
/// Maps an `AuthnError` to a `tonic::Status` for the gRPC authn surface (spec §6.3, D12).
/// Deliberately SEPARATE from the tenancy `status_to_grpc`: authn needs `Unauthenticated`,
/// `PermissionDenied`, `Unavailable` and `Internal`, none of which `ErrorClass` expresses. Every
/// message is STATIC per code and unchanged by SMA-504 — no token, claim or upstream error text
/// ever reaches the wire. What IS new is the machine-readable reason: the gateway previously had
/// to accept a bare `PermissionDenied`, which collapsed three variants (ADR-0020 D4's tripwire).
pub fn authn_status(err: &AuthnError) -> Status {
    let (code, reason, message) = match err {
        AuthnError::InvalidToken(_) => (Code::Unauthenticated, "invalid-token", "invalid bearer token"),
        AuthnError::IdentityNotProvisioned => (Code::PermissionDenied, "identity-not-provisioned", "identity not provisioned"),
        AuthnError::ProvisioningFailed(_) => (Code::PermissionDenied, "provisioning-failed", "provisioning failed"),
        AuthnError::PrincipalInactive => (Code::PermissionDenied, "principal-inactive", "principal inactive"),
        AuthnError::Unavailable => (Code::Unavailable, "authn-unavailable", "authentication backend unavailable"),
        AuthnError::Backend(_) => {
            tracing::error!(error = ?err, "internal error handling a gRPC authn request");
            (Code::Internal, "internal", "internal error")
        }
    };
    iam_status(code, reason, message, authn_retryable(err), &[])
}
```

- [ ] **Step 6: Route the six bare `Status` sites through the helpers**

Replace at each site, keeping the surrounding code untouched:

- `.../grpc/tenancy.rs:79`, `.../grpc/authz.rs:64`, `.../grpc/service_accounts.rs:69`, `.../grpc/audit.rs:51`: `Status::unauthenticated("missing authentication context")` → `convert::missing_auth_context()`
- `.../grpc/authz.rs:91`: `Status::unimplemented("capability iam.authz.cedar is not enabled on this service")` → `convert::capability_disabled("iam.authz.cedar")`
- `.../grpc/service_accounts.rs:104`: `Status::unimplemented("capability iam.apikeys is not enabled on this service")` → `convert::capability_disabled("iam.apikeys")`

The messages are byte-identical, so no message assertion anywhere changes.

- [ ] **Step 7: Convert the six assertions that pin the removed prefix**

These are the load-bearing edits of this task. **Convert, never delete** — a deleted assertion loses the coverage the wire change most needs, and five of the six live in Docker-gated suites that report PASS in ~1s without a daemon.

`rs/crates/services/paigasus-iam/tests/grpc_tenancy.rs:111` — and identically at `:196` (`missing-org-membership`) and `:232` (`prn-mismatch`):

```rust
    // SMA-504: the code is no longer in the message. Read it from ErrorInfo instead — asserting
    // the reason, not a prefix, is what the wire change actually moved.
    let details = tonic_types::StatusExt::get_error_details(&err);
    let info = details.error_info().expect("every IAM status carries ErrorInfo");
    assert_eq!(info.reason, "slug-conflict", "unexpected reason: {info:?}");
    assert_eq!(info.domain, *paigasus_proto::error::IAM_DOMAIN);
```

`tests/grpc_authz.rs:172` and `tests/grpc_audit.rs:107` take the same shape with reason `"forbidden"`.

Add `tonic-types` to `rs/crates/services/paigasus-iam/Cargo.toml`'s `[dev-dependencies]` if the integration tests cannot see the production dep (they can — integration tests link the crate's normal deps only through the crate itself, so `tonic-types` must be listed in `[dev-dependencies]` too for the tests to `use` it directly).

- [ ] **Step 8: Add the server-backed trailers test**

`AuthLayer::reject` renders its `Status` via `Status::into_http` (`grpc/authn.rs:194`), which is a **different serialization path** from a handler-returned `Status`. Same-process `with_error_details`/`get_error_details` proves the predicate, not the transport. Append to `rs/crates/services/paigasus-iam/tests/grpc_authn.rs`:

```rust
/// SMA-504: the trailers-only rejection path must carry ErrorInfo too. `Status::into_http`
/// serializes differently from a handler-returned Status, so the unit tests in `convert.rs`
/// cannot prove this — only a real client/server round trip can.
#[tokio::test]
async fn a_bearer_rejection_carries_error_info_over_the_wire() {
    let Some(ctx) = support::start_migrated_postgres().await else {
        return;
    };
    let mut client = support::tenancy_client(&ctx).await;
    // No `authorization` metadata: AuthLayer rejects before the handler, via `Status::into_http`.
    let err = client
        .list_organizations(tonic::Request::new(Default::default()))
        .await
        .expect_err("a bearer-less tenancy call must be rejected");
    let details = tonic_types::StatusExt::get_error_details(&err);
    let info = details.error_info().expect("the trailers-only path must carry ErrorInfo");
    assert_eq!(info.domain, *paigasus_proto::error::IAM_DOMAIN);
    assert!(
        paigasus_proto::paigasus::common::v1::ErrorReason::from_wire_reason(&info.reason).is_some(),
        "{} is not in the registry",
        info.reason
    );
}
```

Match the surrounding file's helper names (`support::tenancy_client` or whatever it uses) rather than assuming these exist.

- [ ] **Step 9: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && CI=1 cargo nextest run -p paigasus-iam
```

Expected: PASS. `CI=1` is mandatory — without a Docker daemon the converted integration assertions never execute and the run is meaningless. A genuine Docker-backed pass takes just over a second per suite; a sub-second suite is the tell that nothing ran.

- [ ] **Step 10: Commit**

```bash
git add rs/Cargo.toml rs/Cargo.lock rs/crates/services/paigasus-iam
git commit -m "feat(rs): emit google.rpc.ErrorInfo on every iam grpc status (SMA-504)"
```

---

### Task 5: IAM HTTP canonical codes and the retryable header

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/error.rs:18-37`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/authn.rs:37-84`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/system_retirement.rs:145`
- Test: the same files' test modules, plus `rs/crates/services/paigasus-iam/tests/http_authn.rs`, `.../tests/api_key_auth.rs`

**Interfaces:**
- Consumes: `crate::adapters::retryable::{tenancy_retryable, authn_retryable}` (Task 4); `paigasus_observability::Retryable` (Task 2).
- Produces: IAM HTTP error bodies carry canonical kebab codes; every IAM HTTP error response carries `paigasus-retryable`.

- [ ] **Step 1: Write the failing tests**

In `.../http/authn.rs`'s test module, update the three existing assertions and add two:

```rust
    #[tokio::test]
    async fn invalid_token_is_401_with_bearer_challenge() {
        for defect in [TokenDefect::Malformed, TokenDefect::Expired, TokenDefect::Oversized, TokenDefect::BadSignature] {
            let (status, challenge, body) = rendered(AuthnError::InvalidToken(defect)).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
            // AC 7: RFC 6750 §3.1 standardises `invalid_token` in the CHALLENGE. It is not ours
            // to rename — only the JSON body's code becomes canonical.
            assert_eq!(challenge.as_deref(), Some("Bearer error=\"invalid_token\""));
            assert_eq!(body["error"]["code"], "invalid-token");
            assert_eq!(body["error"]["message"], "invalid bearer token");
        }
    }
```

Change `forbidden_family_is_403_with_stable_codes_and_no_challenge`'s case table to `"identity-not-provisioned"`, `"provisioning-failed"`, `"principal-inactive"`; change `unavailable_is_503`'s expected code to `"authn-unavailable"`; change `optional_envelope_json_maps_a_malformed_body_to_the_stable_envelope`'s expected code to `"invalid-request-body"`.

Add:

```rust
    /// AC 6: every code this funnel and its extractor can emit is in the canonical registry.
    /// Driven off the same `all_authn_errors()` exhaustiveness pattern the retryable test uses,
    /// so a new `AuthnError` variant fails to compile rather than escaping the registry.
    #[tokio::test]
    async fn every_authn_http_code_is_in_the_registry() {
        use paigasus_proto::paigasus::common::v1::ErrorReason;

        let mut codes = vec!["request-too-large".to_owned(), "invalid-request-body".to_owned()];
        for err in crate::adapters::retryable::tests_support::all_authn_errors() {
            let (_, _, body) = rendered(err).await;
            codes.push(body["error"]["code"].as_str().expect("a code").to_owned());
        }
        for code in codes {
            assert!(ErrorReason::from_wire_reason(&code).is_some(), "{code} is not declared in common/v1/error.proto");
        }
    }

    /// D4: the header is present on EVERY error response, carrying the literal `false` where the
    /// error is not retryable — a client must never have to read absence as `false`.
    #[tokio::test]
    async fn every_authn_error_carries_a_retryable_header() {
        let cases = [
            (AuthnError::InvalidToken(TokenDefect::Malformed), "false"),
            (AuthnError::Unavailable, "true"),
            (AuthnError::Backend("x".into()), "unknown"),
        ];
        for (err, want) in cases {
            let response = AuthnApiError(err).into_response();
            assert_eq!(response.headers()["paigasus-retryable"], want);
        }
    }
```

`all_authn_errors()` must be reachable from both test modules. Move it out of `retryable.rs`'s `mod tests` into a `#[cfg(test)] pub(crate) mod tests_support` in `retryable.rs`, keeping the exhaustive-`match` guard with it.

In `.../http/error.rs`'s test module, add:

```rust
    /// AC 3: the `error` object's key set is EXACTLY code+message — a positive assertion, so a
    /// stray added field fails rather than passing unnoticed. Scoped to the `error` object, not
    /// the whole body: `system_retirement::conflict` deliberately emits sibling keys beside it.
    #[tokio::test]
    async fn the_error_object_key_set_is_unchanged() {
        let body = body_json(ApiError(TenancyError::SlugConflict).into_response()).await;
        let keys: std::collections::BTreeSet<&str> = body["error"].as_object().expect("an object").keys().map(String::as_str).collect();
        assert_eq!(keys, ["code", "message"].into_iter().collect::<std::collections::BTreeSet<_>>());
    }

    #[tokio::test]
    async fn tenancy_errors_carry_a_retryable_header() {
        let resp = ApiError(TenancyError::SlugConflict).into_response();
        assert_eq!(resp.headers()["paigasus-retryable"], "false");
        let resp = ApiError(TenancyError::Internal).into_response();
        assert_eq!(resp.headers()["paigasus-retryable"], "unknown", "an internal error erases whether its source was transient");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam -E 'kind(lib)'
```

Expected: FAIL — the code assertions report the old snake_case values, and the header assertions panic on a missing key.

- [ ] **Step 3: Rename the authn funnel codes and set the header**

In `.../http/authn.rs`, `IntoResponse for AuthnApiError`:

```rust
        let (status, code, message) = match &self.0 {
            AuthnError::InvalidToken(_) => (StatusCode::UNAUTHORIZED, "invalid-token", "invalid bearer token"),
            AuthnError::IdentityNotProvisioned => (StatusCode::FORBIDDEN, "identity-not-provisioned", "identity not provisioned"),
            AuthnError::ProvisioningFailed(_) => (StatusCode::FORBIDDEN, "provisioning-failed", "provisioning failed"),
            AuthnError::PrincipalInactive => (StatusCode::FORBIDDEN, "principal-inactive", "principal inactive"),
            // `authn-unavailable`, NOT a bare `unavailable`: a rename, not a recasing, so it does
            // not read as a generic service-down code alongside the gateway's `iam-unavailable`
            // and `upstream-unavailable`, which name different failures (ADR-0019 A1.3).
            AuthnError::Unavailable => (StatusCode::SERVICE_UNAVAILABLE, "authn-unavailable", "authentication backend unavailable"),
            AuthnError::Backend(_) => {
                tracing::error!(error = ?self.0, "internal error handling an authn request");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal error")
            }
        };

        let mut response = (status, Json(json!({ "error": { "code": code, "message": message } }))).into_response();
        response.headers_mut().insert(
            paigasus_observability::correlation::RETRYABLE_HEADER,
            HeaderValue::from_static(crate::adapters::retryable::authn_retryable(&self.0).as_wire()),
        );
        if matches!(self.0, AuthnError::InvalidToken(_)) {
            // RFC 6750 §3.1 standardises this value. NOT ours to rename — only the body's code is.
            response.headers_mut().insert(header::WWW_AUTHENTICATE, HeaderValue::from_static(BEARER_CHALLENGE));
        }
        response
```

In `envelope_rejection`:

```rust
    let (code, message) = if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
        ("request-too-large", "request body too large")
    } else {
        // `invalid-request-body`, merged with the gateway's identical case: one code for one
        // condition across both services (ADR-0019 A1.3).
        ("invalid-request-body", "invalid request body")
    };
    let mut response = (rejection.status(), Json(json!({ "error": { "code": code, "message": message } }))).into_response();
    response.headers_mut().insert(paigasus_observability::correlation::RETRYABLE_HEADER, HeaderValue::from_static("false"));
    response
```

- [ ] **Step 4: Set the header on the tenancy and retirement renderers**

In `.../http/error.rs`, `IntoResponse for ApiError`, replace the final expression:

```rust
        let mut response = (status, Json(json!({ "error": { "code": self.0.code(), "message": message } }))).into_response();
        response.headers_mut().insert(
            paigasus_observability::correlation::RETRYABLE_HEADER,
            axum::http::HeaderValue::from_static(crate::adapters::retryable::tenancy_retryable(self.0.class()).as_wire()),
        );
        response
```

In `.../http/system_retirement.rs`'s `conflict`, do the same — its codes (`grants-survive`, `decision-change-unacknowledged`) are already canonical, so only the header is new. Both are 409 conflicts, i.e. `Retryable::No`.

- [ ] **Step 5: Run the unit tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam -E 'kind(lib)'
```

Expected: PASS.

- [ ] **Step 6: Update the integration tests that assert the renamed codes**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && grep -rn 'invalid_token\|identity_not_provisioned\|provisioning_failed\|principal_inactive\|"unavailable"\|request_too_large\|invalid_request' crates/services/paigasus-iam/tests/
```

Update each hit to the canonical spelling — expect hits in `tests/http_authn.rs` and `tests/api_key_auth.rs`. Leave any `WWW-Authenticate` assertion alone: RFC 6750's `invalid_token` there is unchanged.

- [ ] **Step 7: Run the full IAM suite with Docker enforced**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && CI=1 cargo nextest run -p paigasus-iam
```

Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add rs/crates/services/paigasus-iam
git commit -m "feat(rs): move iam http error codes onto the canonical registry (SMA-504)"
```

---

### Task 6: Gateway canonical codes and the retryable header

**Files:**
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/http/error.rs:88-157`
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/http/chat.rs:52`
- Modify: `rs/crates/services/paigasus-gateway/Cargo.toml`
- Test: the same files' test modules, plus `rs/crates/services/paigasus-gateway/tests/chat_proxy.rs`

**Interfaces:**
- Consumes: `paigasus_observability::Retryable` (Task 2); the registry from Task 1.
- Produces: `GatewayError::retryable(self) -> Retryable`; canonical kebab codes on every gateway error and on the terminal SSE frame.

- [ ] **Step 1: Add strum as a dev-dependency**

`rs/crates/services/paigasus-gateway/Cargo.toml`:

```toml
[dev-dependencies]
# cfg(test)-only, exactly as paigasus-iam does it, so strum_macros never enters the shipped
# binary. Drives the AC 6 exhaustive registry test over GatewayError.
strum = { version = "0.26", features = ["derive"] }
```

- [ ] **Step 2: Write the failing tests**

In `.../http/error.rs`'s test module:

```rust
    /// AC 6: every code `GatewayError` can emit is declared in the canonical registry.
    /// Enumerated via `strum::EnumIter` off the type itself, so a variant added later is
    /// included automatically — there is no second list that can be left un-extended.
    #[test]
    fn every_gateway_code_is_declared_in_the_canonical_registry() {
        use paigasus_proto::paigasus::common::v1::ErrorReason;
        use strum::IntoEnumIterator;

        for err in GatewayError::iter() {
            let (_, _, code, _, _) = err.parts();
            let code = code.expect("SMA-504: the Internal case no longer emits a null code");
            assert!(ErrorReason::from_wire_reason(code).is_some(), "GatewayError::{err:?} emits {code:?}, absent from common/v1/error.proto");
        }
    }

    /// D4's table, asserted exhaustively so a new variant must state its retryability.
    #[test]
    fn retryability_matches_the_documented_table() {
        use paigasus_observability::Retryable;
        use strum::IntoEnumIterator;

        for err in GatewayError::iter() {
            let want = match err {
                GatewayError::IamUnavailable | GatewayError::UpstreamUnavailable | GatewayError::UpstreamTimeout => Retryable::Yes,
                GatewayError::Internal | GatewayError::MissingScope => Retryable::Unknown,
                _ => Retryable::No,
            };
            assert_eq!(err.retryable(), want, "{err:?}");
        }
    }

    #[tokio::test]
    async fn every_error_response_carries_a_retryable_header() {
        assert_eq!(GatewayError::IamUnavailable.into_response().headers()["paigasus-retryable"], "true");
        assert_eq!(GatewayError::InvalidCredential.into_response().headers()["paigasus-retryable"], "false");
        assert_eq!(GatewayError::Internal.into_response().headers()["paigasus-retryable"], "unknown");
    }

    /// AC 3: the OpenAI envelope's key set is EXACTLY message/type/param/code. SDKs branch on
    /// `type`, so this shape is a binding external contract — the ids ride in headers precisely
    /// so this assertion keeps holding.
    #[tokio::test]
    async fn the_openai_error_object_key_set_is_unchanged() {
        let body = body_json(GatewayError::InvalidCredential.into_response()).await;
        let keys: std::collections::BTreeSet<&str> = body["error"].as_object().expect("an object").keys().map(String::as_str).collect();
        assert_eq!(keys, ["code", "message", "param", "type"].into_iter().collect::<std::collections::BTreeSet<_>>());
    }
```

Replace `internal_case_serializes_a_null_code` (its premise is inverted by rename 16):

```rust
    /// SMA-504 rename 16: `Internal` emitted a NULL code, so a client could not distinguish it
    /// from any other `api_error`. It now emits the registry's `internal`.
    #[tokio::test]
    async fn internal_case_serializes_the_canonical_internal_code() {
        let body = body_json(GatewayError::Internal.into_response()).await;
        assert_eq!(body["error"]["type"], "api_error");
        assert_eq!(body["error"]["code"], "internal");
    }
```

Update `body_is_the_openai_envelope_shape`'s expected code to `"invalid-api-key"`.

In `.../http/chat.rs`'s test module (create one if absent):

```rust
    /// AC 6 for the terminal SSE frame. Parses the frame's JSON and resolves the `code` field
    /// rather than string-comparing the same literal the constant is built from — a comparison
    /// against the literal would pass even if the code were never registered.
    #[test]
    fn the_terminal_sse_frame_carries_a_registered_code() {
        use paigasus_proto::paigasus::common::v1::ErrorReason;

        let payload = TERMINAL_SSE_ERROR.strip_prefix("data: ").expect("an SSE data frame").trim_end();
        let parsed: serde_json::Value = serde_json::from_str(payload).expect("the frame must be valid JSON");
        let code = parsed["error"]["code"].as_str().expect("a code");
        assert!(ErrorReason::from_wire_reason(code).is_some(), "{code} is not declared in common/v1/error.proto");
        assert_eq!(parsed["error"]["type"], "api_error", "the frame keeps the OpenAI envelope shape");
    }
```

- [ ] **Step 3: Run the tests to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-gateway -E 'kind(lib)'
```

Expected: FAIL — `GatewayError::iter` and `retryable` do not exist; the code assertions see snake_case.

- [ ] **Step 4: Rename the codes and add `retryable`**

In `.../http/error.rs`, add the derive:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(test, derive(strum::EnumIter))]
pub enum GatewayError {
```

Change every `Some("snake_case")` in `parts()` to its kebab spelling, and `GatewayError::Internal`'s `None` to `Some("internal")`:

| variant | new code |
| -- | -- |
| `MissingBearer` | `Some("missing-authorization")` |
| `InvalidCredential` | `Some("invalid-api-key")` |
| `AuthzDenied` | `Some("insufficient-permissions")` |
| `MissingScope` | `Some("missing-scope")` |
| `IamUnavailable` | `Some("iam-unavailable")` |
| `Internal` | `Some("internal")` |
| `BadRequestBody` | `Some("invalid-request-body")` |
| `UpstreamUnavailable` | `Some("upstream-unavailable")` |
| `UpstreamTimeout` | `Some("upstream-timeout")` |
| `StreamingDisabled` | `Some("streaming-disabled")` |

`parts()` is private; make it `pub(crate)` so the registry test can call it, or have the test read the code out of the rendered body instead. Prefer the latter — it asserts what a client actually receives.

Add the method:

```rust
    /// Whether a client should retry (spec D4). `true` ONLY for transient dependency failures.
    /// The two internal cases are `Unknown` rather than `false`: the gateway cannot tell a
    /// transient fault from a bug, and a confident `false` there would be worse than the
    /// status-class guess this replaces.
    pub fn retryable(self) -> Retryable {
        match self {
            Self::IamUnavailable | Self::UpstreamUnavailable | Self::UpstreamTimeout => Retryable::Yes,
            Self::Internal | Self::MissingScope => Retryable::Unknown,
            Self::MissingBearer | Self::InvalidCredential | Self::AuthzDenied | Self::BadRequestBody | Self::StreamingDisabled => Retryable::No,
        }
    }
```

And set the header in `into_response`:

```rust
        let mut response = (status, Json(envelope)).into_response();
        response.headers_mut().insert(
            paigasus_observability::correlation::RETRYABLE_HEADER,
            axum::http::HeaderValue::from_static(self.retryable().as_wire()),
        );
        response
```

Update the doc comment on `ErrorBody::code` — it currently says "or `null`", which stops being true.

- [ ] **Step 5: Rename the terminal SSE frame's code**

In `.../http/chat.rs:52`:

```rust
const TERMINAL_SSE_ERROR: &str = "data: {\"error\":{\"message\":\"upstream stream error\",\"type\":\"api_error\",\"param\":null,\"code\":\"upstream-error\"}}\n\n";
```

Extend the constant's doc comment: the frame carries no `paigasus-retryable` header because the `200 OK` head is already committed when it is emitted, and adding a `retryable` key to the JSON would break the OpenAI envelope shape AC 3 pins — retryability is derivable here anyway, since `upstream-error` is by construction the transient case.

- [ ] **Step 6: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-gateway
```

Expected: PASS.

- [ ] **Step 7: Update the integration tests**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && grep -rn 'upstream_error\|invalid_api_key\|missing_authorization\|insufficient_permissions\|invalid_request_body\|upstream_unavailable\|upstream_timeout\|streaming_disabled\|missing_scope\|iam_unavailable' crates/services/paigasus-gateway/tests/
```

Update each hit (expect `tests/chat_proxy.rs:300` and `:319`) to the canonical spelling, then re-run `cargo nextest run -p paigasus-gateway`.

- [ ] **Step 8: Commit**

```bash
git add rs/crates/services/paigasus-gateway
git commit -m "feat(rs): move gateway error codes onto the canonical registry (SMA-504)"
```

---

### Task 7: Gateway consumption — narrow the PermissionDenied accept

**Files:**
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/http/auth.rs:147-240` (doc comment + `require_authenticated`), `:351-380` and `:425-459` (the `FakeIam` harness)
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/iam/client.rs`
- Modify: `rs/crates/services/paigasus-gateway/Cargo.toml`

**Interfaces:**
- Consumes: `paigasus_proto::error::IAM_DOMAIN`, `ErrorReason::IdentityNotProvisioned` (Task 1); IAM's `ErrorInfo` emission (Task 4); `paigasus_observability::current_ids` (Task 2).
- Produces: `require_authenticated` accepts `PermissionDenied` **only** with IAM-domain `identity-not-provisioned` details; `IamClient` propagates `paigasus-correlation-id` on every outbound call.

- [ ] **Step 1: Add `tonic-types` to the gateway**

```toml
tonic-types = { workspace = true }
```

- [ ] **Step 2: Write the failing tests**

Extend the harness so a fake can carry details. Change the three outcome enums' `Rpc` variants:

```rust
        /// An IAM gRPC error Status with this code, optionally carrying richer-error details.
        /// SMA-504: `require_authenticated` now branches on `ErrorInfo`, so a test that wants the
        /// unprovisioned-identity path must supply the reason IAM really sends.
        Rpc(Code, Option<tonic_types::ErrorDetails>),
```

and the three construction sites in `impl Iam for FakeIam`:

```rust
                IntrospectOutcome::Rpc(code, details) => Err(IamError::Rpc(match details {
                    Some(d) => tonic::Status::with_error_details(*code, "", d),
                    None => tonic::Status::new(*code, ""),
                })),
```

Add a helper and the four decision rows:

```rust
    /// The details IAM attaches to a validated-but-unprovisioned identity.
    fn identity_not_provisioned_details() -> tonic_types::ErrorDetails {
        tonic_types::ErrorDetails::with_error_info(
            paigasus_proto::paigasus::common::v1::ErrorReason::IdentityNotProvisioned.as_wire_reason().expect("a declared reason"),
            &*paigasus_proto::error::IAM_DOMAIN,
            std::collections::HashMap::new(),
        )
    }

    fn reason_details(reason: paigasus_proto::paigasus::common::v1::ErrorReason) -> tonic_types::ErrorDetails {
        tonic_types::ErrorDetails::with_error_info(
            reason.as_wire_reason().expect("a declared reason"),
            &*paigasus_proto::error::IAM_DOMAIN,
            std::collections::HashMap::new(),
        )
    }

    /// SMA-504 AC 2: the narrow accepts ONLY `identity-not-provisioned`. `principal-inactive` and
    /// `provisioning-failed` share `PermissionDenied` and were previously accepted along with it
    /// — the blanket accept ADR-0020 D4's tripwire comment warned about.
    #[tokio::test]
    async fn require_authenticated_rejects_other_permission_denied_reasons() {
        use paigasus_proto::paigasus::common::v1::ErrorReason;

        for reason in [ErrorReason::PrincipalInactive, ErrorReason::ProvisioningFailed] {
            let fake = FakeIam::new(IntrospectOutcome::Rpc(Code::Unauthenticated, None), AuthzOutcome::Unreachable)
                .with_token_introspect(TokenIntrospectOutcome::Rpc(Code::PermissionDenied, Some(reason_details(reason))));
            assert_eq!(
                discovery_status_of(fake, req_with_auth("Bearer token")).await,
                StatusCode::UNAUTHORIZED,
                "{reason:?} must not ride in on the identity-not-provisioned relaxation"
            );
        }
    }

    /// A detail-less `PermissionDenied` fails closed. Two outcomes, both correct: a 401 when the
    /// API-key leg reached a verdict, and a 503 when it did not — `preserve_outage` still widens
    /// an inconclusive leg, so "fails closed" is not unconditionally a 401.
    #[tokio::test]
    async fn a_detail_less_permission_denied_fails_closed() {
        let conclusive = FakeIam::new(IntrospectOutcome::Rpc(Code::Unauthenticated, None), AuthzOutcome::Unreachable)
            .with_token_introspect(TokenIntrospectOutcome::Rpc(Code::PermissionDenied, None));
        assert_eq!(discovery_status_of(conclusive, req_with_auth("Bearer token")).await, StatusCode::UNAUTHORIZED);

        let inconclusive = FakeIam::new(IntrospectOutcome::Connect, AuthzOutcome::Unreachable)
            .with_token_introspect(TokenIntrospectOutcome::Rpc(Code::PermissionDenied, None));
        assert_eq!(
            discovery_status_of(inconclusive, req_with_auth("Bearer token")).await,
            StatusCode::SERVICE_UNAVAILABLE,
            "an outage on the API-key leg still wins over a rejection on the OIDC leg"
        );
    }
```

Update `require_authenticated_accepts_a_validated_but_unprovisioned_identity` (`:749`) to pass `Some(identity_not_provisioned_details())`. Its own doc comment calls it "the one most likely to be 'fixed' back into a 401 by a later reader" — it must stay green and keep accepting, or the relaxation ADR-0020 D4 requires has been silently deleted.

Add to `.../adapters/iam/client.rs`'s test module:

```rust
    /// §4.4: the correlation id crosses the gateway→IAM hop, so ONE id stitches both services'
    /// logs and (via D9) IAM's audit rows. The request id is deliberately NOT forwarded — IAM
    /// mints its own for its own hop, which is what keeps the two distinguishable.
    #[tokio::test]
    async fn outbound_requests_carry_the_ambient_correlation_id() {
        let ids = paigasus_observability::RequestIds {
            request_id: uuid::Uuid::from_u128(1),
            correlation_id: uuid::Uuid::from_u128(2),
        };
        let req = paigasus_observability::correlation::scope_for_test(ids, async { introspect_request("tok") }).await;
        assert_eq!(req.metadata().get("paigasus-correlation-id").unwrap().to_str().unwrap(), ids.correlation_id.to_string());
        assert!(req.metadata().get("paigasus-request-id").is_none(), "the request id is per-hop and is not forwarded");
    }
```

`paigasus_observability::correlation::scope_for_test` already exists — Task 2 Step 4 added it. Do not re-add it.

- [ ] **Step 3: Run the tests to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-gateway
```

Expected: FAIL — `scope_for_test` missing, `Rpc` arity changed, and the two new decision rows return 200 because the accept is still blanket.

- [ ] **Step 4: Narrow the accept**

In `.../http/auth.rs`, add the predicate:

```rust
/// The reason IAM sends for a VALIDATED token whose `(issuer, subject)` has no local principal.
/// Hoisted into a `LazyLock` because `as_wire_reason` allocates and this runs on every rejected
/// discovery request.
static IDENTITY_NOT_PROVISIONED: LazyLock<Option<String>> =
    LazyLock::new(|| paigasus_proto::paigasus::common::v1::ErrorReason::IdentityNotProvisioned.as_wire_reason());

/// Is this IAM `Status` specifically "validated, but not yet provisioned"?
///
/// SMA-504 discharges ADR-0020 D4's tripwire: `PermissionDenied` alone used to be accepted,
/// which silently also accepted `provisioning-failed` and `principal-inactive`. Both were
/// unreachable through `Introspect` at the time, but the accept was blanket rather than
/// deliberate. It is now the reason that decides, read from `ErrorInfo` — never the message
/// string, which carries no test pinning its text.
///
/// Fails CLOSED: a `Status` with no `ErrorInfo` is not accepted. Post-SMA-504 IAM always emits
/// it, so the only way to see one is version skew — **IAM must roll before the gateway**.
fn is_identity_not_provisioned(status: &Status) -> bool {
    if status.code() != Code::PermissionDenied {
        return false;
    }
    // `get_error_details` returns an OWNED `ErrorDetails`; bind it before borrowing out of it.
    let details = status.get_error_details();
    let Some(info) = details.error_info() else {
        // Version skew during a rolling upgrade: an old IAM sends no details, and every
        // unprovisioned console user gets a 401 until it rolls. Logged so the window is visible.
        tracing::warn!("IAM returned a PermissionDenied with no ErrorInfo — rolling-upgrade skew? (SMA-504)");
        return false;
    };
    info.domain == *paigasus_proto::error::IAM_DOMAIN && Some(info.reason.as_str()) == IDENTITY_NOT_PROVISIONED.as_deref()
}
```

Replace the match arm at `:227`:

```rust
        Err(IamError::Rpc(ref status)) if is_identity_not_provisioned(status) => {
            record_iam_call("introspect_token", "denied", started);
            next.run(req).await
        }
```

Delete the "Why accepting the WHOLE `PermissionDenied` code is safe today (and when it stops being so)" section from `require_authenticated`'s doc comment — the whole reachability argument and its tripwire — and replace it with:

```rust
/// ### Which `PermissionDenied` is accepted
/// Exactly one: IAM's `identity-not-provisioned`, read from `ErrorInfo` (SMA-504). The other two
/// reasons that share `PermissionDenied` — `provisioning-failed` and `principal-inactive` — are
/// rejected, as is a `Status` carrying no details at all. This replaces the blanket
/// code-only accept, which was correct only by reachability accident.
```

- [ ] **Step 5: Propagate the correlation id**

In `.../adapters/iam/client.rs`, add a helper and call it from all three request builders:

```rust
/// Attach the ambient correlation id to an outbound IAM call, so one id spans both services'
/// logs and IAM's audit rows. A `None` scope (background work) attaches nothing rather than a
/// nil UUID — IAM then mints its own.
fn with_correlation<T>(mut req: Request<T>) -> Request<T> {
    if let Some(ids) = paigasus_observability::current_ids()
        && let Ok(value) = ids.correlation_id.to_string().parse()
    {
        req.metadata_mut().insert(paigasus_observability::correlation::CORRELATION_ID_HEADER, value);
    }
    req
}
```

Wrap the three constructions: `introspect_request`'s return, `self_authorize_request`'s return, and the inline `Request::new(IntrospectRequest { .. })` in `introspect_token`.

- [ ] **Step 6: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-gateway
```

Expected: PASS, including `require_authenticated_accepts_a_validated_but_unprovisioned_identity`.

- [ ] **Step 7: Commit**

```bash
git add rs/crates/services/paigasus-gateway rs/crates/libs/paigasus-observability
git commit -m "fix(rs): accept only identity-not-provisioned on the discovery path (SMA-504)"
```

---

### Task 8: Link the audit correlation id to the request correlation id

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/id.rs`

**Interfaces:**
- Consumes: `paigasus_observability::current_ids` (Task 2).
- Produces: `KernelIdGenerator::new_correlation_id` returns the ambient correlation id inside a request scope.

- [ ] **Step 1: Write the failing test**

Append to `rs/crates/services/paigasus-iam/src/adapters/id.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use paigasus_iam_core::IdGenerator;
    use paigasus_observability::RequestIds;

    /// D9: IAM already had a `correlation_id`, minted per mutation and persisted on the
    /// `audit_log` and `event_outbox` rows. Left unlinked, an operator handed a
    /// `paigasus-correlation-id` from a customer could not find the audit row it produced —
    /// which is the entire support story ADR-0019 calls the highest-value item here.
    #[tokio::test]
    async fn new_correlation_id_adopts_the_ambient_request_correlation_id() {
        let ids = RequestIds {
            request_id: Uuid::from_u128(1),
            correlation_id: Uuid::from_u128(2),
        };
        let got = paigasus_observability::correlation::scope_for_test(ids, async { KernelIdGenerator.new_correlation_id() }).await;
        assert_eq!(got, ids.correlation_id);
    }

    /// Outside a request — boot-time convergence, the outbox relay — there is nothing to adopt,
    /// so it mints. Two calls must differ, or the fallback silently became a constant.
    #[test]
    fn new_correlation_id_mints_outside_a_request_scope() {
        let a = KernelIdGenerator.new_correlation_id();
        let b = KernelIdGenerator.new_correlation_id();
        assert_ne!(a, b);
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam -E 'test(new_correlation_id)'
```

Expected: FAIL — `new_correlation_id_adopts_the_ambient_request_correlation_id` gets a freshly minted id, not `ids.correlation_id`.

- [ ] **Step 3: Adopt the ambient id**

Replace `new_correlation_id`'s body in the `impl IdGenerator for KernelIdGenerator` block:

```rust
    /// Adopts the in-flight request's correlation id when there is one (SMA-504 D9), so the id a
    /// customer reads off `paigasus-correlation-id` is the SAME id on the `audit_log` and
    /// `event_outbox` rows the request produced. Mints outside a request scope (boot-time
    /// convergence, the outbox relay).
    ///
    /// A caller may supply that id (spec D6 requires only that it parse as a UUID), so a caller
    /// can group their OWN audit rows under one id. That is what a correlation id is for; each
    /// row keeps its own primary key and `occurred_at`, and no caller can affect anyone else's
    /// rows.
    fn new_correlation_id(&self) -> Uuid {
        paigasus_observability::current_ids().map_or_else(|| self.mint(), |ids| ids.correlation_id)
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && CI=1 cargo nextest run -p paigasus-iam
```

Expected: PASS. The existing "the outbox event and the audit entry must share one correlation id" assertions compare two rows within one transaction and are unaffected.

- [ ] **Step 5: Commit**

```bash
git add rs/crates/services/paigasus-iam
git commit -m "feat(rs): stamp audit and outbox rows with the request correlation id (SMA-504)"
```

---

### Task 9: Full gate run and the rollout note

**Files:**
- Modify: `docs/superpowers/specs/2026-08-17-sma-504-canonical-error-model-design.md` (only if a gate reveals a spec inaccuracy)

- [ ] **Step 1: Format and lint the Rust workspace**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings
```

Expected: clean. If `cargo fmt --check` fails, run `cargo fmt` and re-run.

- [ ] **Step 2: Confirm the codegen trees are not stale**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd contracts && buf format --diff --exit-code && buf generate
cd .. && git status --short
```

Expected: `buf format` reports no diff, and `git status` is clean after `buf generate` — a dirty tree here means the committed bindings do not match the `.proto`.

- [ ] **Step 3: Run the full CI graph**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :promtool :observability-drift :nats-permissions \
  :release-parity :release-parity-py :release-parity-ts :publish-metadata \
  --base origin/main --include-relations
```

Per-project Moon tasks do NOT run these repo-level gates, so this is the first point at which `:deny` (the new `tonic-types` licence), `:machete` (both services must actually consume it), `:affected-smoke` (the new kernel edge) and `:breaking` (the three appended enum values) are exercised together.

- [ ] **Step 4: Diagnose any failure Moon attributes to nothing**

Moon reports a bare "1 failed" without naming the task. To find it:

```bash
jq '.actions[] | select(.status == "failed") | {label, status}' .moon/cache/ciReport.json
```

- [ ] **Step 5: Re-verify the Docker-backed suites explicitly**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && CI=1 cargo nextest run -p paigasus-iam
```

`moon ci` does not set `CI=1` for these suites, so this is a separate, mandatory step: without it a missing daemon makes `start_migrated_postgres` return `None` and every container-backed assertion in Tasks 4 and 5 reports PASS having run nothing. A whole-suite time well under a second is the tell.

- [ ] **Step 6: Record the rollout requirement on the Linear issue**

Add a comment to SMA-504 stating that **IAM must be rolled before the gateway**: during skew a new gateway sees IAM's detail-less `PermissionDenied` and rejects unprovisioned console users on `/v1/service-info` until IAM catches up. A `tracing::warn!` fires on each such response so the window is visible.

- [ ] **Step 7: Commit any gate fixes**

```bash
git add -A
git commit -m "chore(rs): satisfy the repo gates for the canonical error model (SMA-504)"
```

Skip this step if the tree is clean.

---

## Self-Review

**Spec coverage.** D1 → Tasks 2, 3. D2 → Tasks 5, 6 (key-set assertions) and Task 4 (metadata). D3 → Task 2. D4 → Tasks 2 (layer default, `Retryable`), 4 (IAM mappings), 6 (gateway mapping). D5 → Task 6 (`missing-scope` retained in the rename table). D6 → Task 2. D7 → Task 2, Steps 1/7/8. D8 → Task 1 (domain statics), Tasks 4 and 7 (registry-derived comparisons). D9 → Task 8. D10 → Task 3. §5.1.3 → Task 4, Steps 5–6. §5.2's 18 renames → Tasks 5 (1–7) and 6 (8–18). §5.3 → Task 7. §6 → Tasks 1, 2, 4, 9. §7's test list → distributed across Tasks 2–8, with the six prefix conversions in Task 4 Step 7 and the wire-crossing test in Task 4 Step 8.

**Known gaps, deliberate — superseded.** This plan originally deferred the spec's §4.1 tonic-timeout assertion, on the grounds that driving a real `Server::timeout` needs a deliberately slow handler for a gap that is accepted rather than fixed. The final whole-branch review rejected that: §8's risk table lists "asserted so it cannot widen silently" as the mitigation, and an unasserted mitigation is not one. The test shipped as `a_server_side_timeout_status_carries_no_error_info_or_ids` in `tests/grpc_authn.rs`, driving the existing `router(state, timeout)` parameter at 1ms against a DB-backed, JIT-provisioning RPC so the deadline is the deterministic outcome rather than a race. No gap remains.

**Numbering deviation from the spec.** The spec put `missing-auth-context` at 33 in the IAM range; Task 1 puts it at 903 in the shared range alongside `capability-disabled` at 904, with the reasoning inline. Neither condition is IAM-specific.

**Type consistency.** `Retryable::as_wire` returns `&'static str` at every use, which is what `HeaderValue::from_static` requires. `current_ids() -> Option<RequestIds>` is consumed identically in Tasks 4, 7 and 8. `iam_status`'s signature gained a `retryable: Retryable` parameter relative to the spec's sketch, and every call site in Task 4 passes it. `scope_for_test` is introduced in Task 2 Step 4, where the module is created, and consumed by Tasks 7 and 8 — no task adds it twice.
