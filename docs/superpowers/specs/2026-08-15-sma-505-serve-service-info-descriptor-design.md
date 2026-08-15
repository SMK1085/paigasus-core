# SMA-505 — `paigasus-iam` + `paigasus-gateway` serve the `ServiceInfo` descriptor

Linear: [SMA-505](https://linear.app/smaschek/issue/SMA-505/iamgateway-serve-the-serviceinfo-capability-descriptor)
ADR: [ADR-0020 — Service capability discovery](https://app.notion.com/p/3bb830e8fbaa8113b9f3da910893aaa8) (accepted 2026-08-13)
Blocked by: [SMA-499](https://linear.app/smaschek/issue/SMA-499) — merged as PR 119, `ce0dc28`
Blocks: [SMA-509](https://linear.app/smaschek/issue/SMA-509) (TypeScript capability-discovery client)
Predecessor spec: `docs/superpowers/specs/2026-08-14-sma-499-service-info-capability-descriptor-design.md`

## 1. Context

### 1.1 What exists today

SMA-499 landed the contract and nothing else. `contracts/proto/paigasus/common/v1/service_info.proto`
declares `ServiceInfo { service, version, capabilities }`, the four-value `Capability` registry, and
`ServiceInfoService.GetServiceInfo`; it specifies an equivalent `GET /v1/service-info` HTTP route
normatively in prose (D3 of that spec — a `google.api.http` option was probed and rejected because it
breaks the TypeScript output). `rs/crates/libs/paigasus-proto/src/capability.rs` supplies
`Capability::as_wire_key` / `from_wire_key`.

No service serves the descriptor. The generated `ServiceInfoService` server trait is implemented by
nobody.

`paigasus-iam` runs both an axum HTTP server (`adapters/http/mod.rs`) and a tonic gRPC server
(`adapters/grpc/mod.rs`). `paigasus-gateway` is axum-only and merely *dials* IAM over gRPC
(`adapters/iam/client.rs`).

### 1.2 What this issue delivers

Both services answer with a descriptor whose capability list is derived from live configuration, on
an authenticated surface, with a version fed from the build.

## 2. Findings that constrain the design

Established by reading the worktree before designing. Three of the four contradict a plain reading of
the issue's scope notes.

### 2.1 No registered capability key maps to any existing config flag

SMA-499 § 4.2 predicted this and recorded it as a crack in its own boundary. Confirmed:

`rs/crates/services/paigasus-iam/src/config.rs` offers `authz.enforce_tenancy` (`:169`),
`audit.retention.enabled`, `authn.jit_provisioning`, `outbox.relay_enabled` and `metrics.enabled`.
None of them means "Cedar is available", "API keys are available" or "the audit log is queryable" —
they gate, respectively, whether tenancy handlers call `Authorize::check`, whether the partition
maintenance task is spawned, whether unknown identities are JIT-provisioned, whether the outbox relay
drains, and whether the Prometheus recorder is installed.

`rs/crates/services/paigasus-gateway/src/config.rs` is worse: it has `stream_idle_timeout_secs` but no
streaming toggle at all, so `gateway.chat.stream` is a compile-time constant today.

The issue's own scope note names `authz.enforce_tenancy` and "the outbox publisher backend" as
examples of config-gated capabilities. Neither corresponds to a registered key: `enforce_tenancy`
governs enforcement on the tenancy routes rather than Cedar's availability, and the outbox publisher
backend (`PublisherBackend::{Tracing, Nats}`, `config.rs:568`) has no key in the registry at all.

**Consequence.** AC 3 cannot be satisfied by reading today's configuration. Either new flags are
introduced, or new keys are appended to the registry. This spec introduces flags (D1).

### 2.2 Two gRPC services bundle must-stay RPCs with capability-scoped ones

From `contracts/proto/paigasus/iam/v1/iam.proto`:

| Service | RPCs |
| --- | --- |
| `AuditService` (`:495`) | `ListAuditEntries` — one RPC, wholly within `iam.audit` |
| `AuthorizationService` (`:352`) | `IsAuthorized` **plus** `PutPolicy`, `DeletePolicy`, `ListPolicies`, `GrantRole`, `RevokeRole`, `ListRoleGrants` |
| `ServiceAccountService` (`:451`) | `CreateServiceAccount`, `GetServiceAccount`, `ListServiceAccounts`, `ArchiveServiceAccount` **plus** `IssueApiKey`, `RevokeApiKey`, `ListApiKeys` |

`IsAuthorized` is the gateway's per-request D9 self-query; service-account lifecycle is tenancy
management, not an API-key concern. Neither service can be unmounted wholesale without collateral
damage. D3 resolves this.

### 2.3 The gateway cannot authenticate a console user today

`adapters/http/auth.rs::require_iam_auth` performs bearer extraction → `IntrospectApiKey` →
`is_authorized_self(InvokeModel, scope)`. The `Iam` port (`adapters/iam/client.rs:53-66`) exposes only
`introspect_api_key` and `is_authorized_self`, so a user's OIDC token has no path through it.

ADR-0020 D4 requires discovery to run inside an authenticated **user's** request with no provisioned
service credential. SMA-499 D3 flagged this explicitly as SMA-505's problem: "neither of its current
route groups fits `/v1/service-info` as-is."

The fix is small in practice: IAM's `AuthnService` already exposes `Introspect` (`iam.proto:286`) and
it is bearer-exempt (`adapters/grpc/authn.rs:126`), on the very `AuthnServiceClient` the gateway
adapter already holds (`client.rs:73`).

### 2.4 The TypeScript workspace has no gRPC transport

`ts/packages/paigasus-proto` generates protobuf-es message types only. No `@connectrpc/*`, no
`grpc-web`, no `@grpc/grpc-js` appears in any `package.json`. SMA-509 — the TS client this issue
blocks — would need an entire transport stack added to consume a gRPC-only descriptor. This decides
D4.

### 2.5 IAM's HTTP api-key introspection route is already unauthenticated

`adapters/http/api_keys.rs::introspect_router` (`:48`) is merged **outside** the bearer
`route_layer`, mirroring `authn.rs`'s token-introspect route. Noted because it bounds what "no new
unauthenticated surface" means here: this spec adds none, and does not touch the existing ones.

## 3. Decisions

### D1 — Capabilities are real feature toggles, not descriptor-only flags

Each capability gets a configuration flag that genuinely changes what the service serves. The
descriptor reads the same flag. One truth, two observable effects: flipping the flag makes the
feature's surface disappear **and** the key disappear.

The rejected alternative is a `[capabilities]` block read only by the descriptor. It satisfies AC 3's
letter at a fraction of the cost, and is rejected because it recreates the issue's own stated failure
in mirror image: a descriptor that says "off" while the endpoint still answers misleads the console
exactly as badly as one that says "on" for a feature the operator disabled. The note closing the
issue — "worse than no descriptor" — applies to both directions.

All flags default to `true`. An existing deployment upgrading to this build sees no behaviour change.

### D2 — "Off" unmounts the surface; internal wiring is untouched

A disabled capability's HTTP routes are not registered, so an unmatched path 404s exactly as it would
on a build predating the feature. Components are **not** torn down: `CedarAuthorizer`, the policy
snapshot and its reload task, the audit writer, the API-key repository and both introspection paths
all continue to exist and function.

A capability key answers "what can a client call here?", which is precisely the question ADR-0020's
console asks. It does not claim anything about internal resource usage.

The rejected alternative — full teardown, making `AppState.authz` an `Option` and skipping the reload
task — reaches into bootstrap-admin seeding, starter-policy reconciliation (SMA-477), the generations
counter and system-row retirement. That is a wide blast radius in code this issue otherwise never
touches, for a property no client can observe.

The other rejected alternative — keeping routes mounted and returning `501` — leaves a surface that
answers, so a client probing endpoints rather than reading the descriptor sees something alive, and
`501` reads as "broken" rather than "not offered here".

### D3 — On gRPC, a disabled RPC returns `UNIMPLEMENTED`

Per 2.2, `AuthorizationService` and `ServiceAccountService` cannot be unmounted wholesale.
Capability-scoped RPCs on those two services check the flag and return
`tonic::Status::unimplemented` before doing any work. `AuditService`, which is wholly within
`iam.audit`, is simply not added to the tonic router when disabled.

These are observably identical: a client calling an RPC on a service the server never registered also
receives `UNIMPLEMENTED`. So the rule is uniform across both mechanisms and consistent with HTTP's
404 — a disabled capability is indistinguishable from a build that never had it.

The `UNIMPLEMENTED` message carries the capability's wire key (e.g. `capability iam.apikeys is not
enabled on this service`) so an operator reading a log can act on it. This is a diagnostic, not a
contract; clients gate on the descriptor.

### D4 — Transports: HTTP on both services, plus gRPC on IAM

The gateway serves `GET /v1/service-info` only. Giving it a tonic server means a second listening
port plus Helm and ingress entries for every self-hoster — the surface SMA-499 D3 already rejected as
exceeding ADR-0020's "deliberate, small tax on service authors".

IAM serves **both** the HTTP route and `ServiceInfoService.GetServiceInfo`. The RPC is nearly free
because IAM's tonic server already exists, it honours the proto's stated intent, it serves Rust and
Python gRPC clients, and it stops the freshly declared `ServiceInfoService` from being implemented by
nobody. The HTTP route is what makes SMA-509 possible without adding a transport stack to `ts/`
(2.4), and it is the protocol IAM's console-facing clients already speak.

The cost is two code paths for one descriptor in IAM. Mitigated structurally: both call the same
builder, and § 6 requires a test asserting the two transports agree.

Per SMA-499 D3 the two bodies differ by design — HTTP returns the **bare** `ServiceInfo`, gRPC returns
`GetServiceInfoResponse { service_info }`. The wrapper is a buf-lint artefact.

### D5 — Both routes require authentication and perform no authorization

**IAM.** The route joins the existing `protected` sub-router, behind
`auth_middleware::require_bearer`. That middleware already routes a token carrying the configured
`api_key_prefix` to the API-key authenticator and everything else to OIDC, so a console user session
and a service-account key both work with no new middleware. The gRPC `GetServiceInfo` is covered
automatically: `AuthLayer` enforces a bearer on every path absent from `is_exempt`
(`adapters/grpc/authn.rs:125`), and `ServiceInfoService/GetServiceInfo` is not added to it.

**Gateway.** A new `require_authenticated` middleware mirrors that credential router: a
`pgs_sk_`-prefixed token goes to the existing `Iam::introspect_api_key`, anything else to a new
`Iam::introspect_token` wrapping IAM's OIDC `Introspect` RPC. A valid, active principal is required;
**no** `is_authorized_self` call is made.

Discovery is deliberately not gated on an action. `InvokeModel` — what `require_iam_auth` demands —
would make a caller who legitimately cannot invoke models unable to discover that streaming exists,
and would force ADR-0020's console to hold a service credential for the gateway, which D4 of that ADR
rejects outright.

No new unauthenticated surface is introduced (AC 2). Errors keep each service's existing envelope:
IAM's `AuthnApiError` 401 funnel, the gateway's OpenAI-shaped envelope (ADR-0019 — envelopes are
per-surface).

### D6 — Version comes from `CARGO_PKG_VERSION`, read in the service crate

`env!("CARGO_PKG_VERSION")` is evaluated in each **service** crate and passed into the shared builder.
Nothing is hand-written, no build script is added, the build needs no git checkout, and rebuilds are
byte-identical. release-plz already owns the number in `Cargo.toml`.

Both crates are `version = "0.0.0"` today, so that is what ships until the first release. It is valid
SemVer and harmless: the proto forbids clients from ever gating a feature decision on version, and
requires them to tolerate an unparseable value by suppressing skew reporting.

Rejected: a `build.rs` embedding `git describe`, which makes the build depend on a git checkout (a
Docker build from a source tarball yields a different or empty value) and makes output
non-deterministic, at odds with this repo's codegen-drift posture. Deferred, not rejected: appending
CI-stamped SemVer build metadata (`0.0.0+ce0dc28`) — a small, additive change once there is a CI
decision to attach it to (§ 9).

### D7 — A shared `paigasus-service-info` crate owns the wire invariants

A new library crate holds the descriptor builder, the HTTP DTO and the shared route constant. Each
service owns its own config→capability predicate, because the two read entirely different config
types and share no logic there.

The cheaper alternatives are real: a per-service module duplicates roughly twenty lines, and a helper
alongside `capability.rs` in `paigasus-proto` costs nothing at all. The crate is chosen because the
invariants it enforces are the ones most expensive to get wrong twice — dropping the `UNSPECIFIED`
sentinel, and guaranteeing `capabilities` is emitted even when empty (SMA-499 § 2.7: canonical
protojson omits it, and a console doing `info.capabilities.includes(k)` then throws `TypeError`
instead of rendering "feature off"). Enforcing those in one tested place, for the third service as
much as the first two, is worth the crate.

The cost is paid once and is known: a `moon.yml`, an entry in `ci/affected-graph/run.sh`'s
strict-equality expected set, and CODEOWNERS regeneration (§ 7).

### D8 — Two `Capability` doc comments are narrowed in `contracts/`

The registry currently documents:

- `CAPABILITY_IAM_APIKEYS` — "service-account API key issuance **and introspection** are available."
- `CAPABILITY_IAM_AUTHZ_CEDAR` — "Cedar policy evaluation is enabled: authorization **decisions**,
  policy administration and role grants are available."

Both are narrowed to management/administration only:

- `iam.apikeys` — service-account API **key management** (issuance, listing, revocation) is available.
- `iam.authz.cedar` — Cedar policy **administration** (policies and role grants) is available.

Introspection and `IsAuthorized` are service-to-service primitives that other Paigasus services call
on every request, not user-facing features the console renders. Keeping them permanently mounted
means **no flag combination can break the gateway**, and it keeps both IAM toggles carrying identical
safety properties, which is far easier to document than two toggles where one is deployment-coupled.

The doc comments additionally state, for each key, that the primitive stays available when the
capability is off — so the next reader does not infer the wider meaning from the key's name.

This is a comment-only edit: no field, no enum value, no wire change, so `:breaking` stays green. It
still regenerates all three binding trees, because the embedded `FILE_DESCRIPTOR_SET` shifts.

### D9 — Streaming-off is a request-parameter rejection, not an unmount

Streaming is a body field, not a route. `/v1/chat/completions` stays mounted; when `stream_enabled` is
false a request whose parsed DTO has `stream: true` is rejected with `400` through the existing
OpenAI error envelope, carrying `param: "stream"`. Non-streaming requests are unaffected. The check
sits at `adapters/http/chat.rs:77`, where `dto.stream` is already read, before any egress call.

`400` rather than `501`: OpenAI-compatible SDKs commonly treat 5xx as retryable and would back off and
retry a request that can never succeed, converting a deliberate configuration choice into repeated
load. `400` with a `param` is the OpenAI idiom for an unsupported request parameter.

Silently serving a non-streamed response was rejected: it returns a single JSON object where the
client is parsing for SSE frames, converting a clear configuration signal into a confusing
client-side parse failure.

## 4. The contract

### 4.1 Configuration

| Service | Flag | Default | Governs |
| --- | --- | --- | --- |
| iam | `authz.admin_enabled` | `true` | `iam.authz.cedar` |
| iam | `api_keys.management_enabled` | `true` | `iam.apikeys` |
| iam | `audit.query_enabled` | `true` | `iam.audit` |
| gateway | `stream_enabled` | `true` | `gateway.chat.stream` |

Flags live inside the existing config blocks they belong to, so environment overrides follow the
established figment shape: `IAM_AUTHZ__ADMIN_ENABLED`, `IAM_API_KEYS__MANAGEMENT_ENABLED`,
`IAM_AUDIT__QUERY_ENABLED`, `GATEWAY_STREAM_ENABLED`. The gateway's flag is top-level, matching its
sibling `stream_idle_timeout_secs`.

Each is documented in `iam.toml.example` / `gateway.toml.example` with what disabling it removes and
what it deliberately leaves running.

No cross-field validation is added to either `validate()`. Under D2 and D8 there is no invalid
combination: every flag is independent, and none can leave the service incoherent.

### 4.2 What each flag removes

| Flag off | HTTP | gRPC | Deliberately unaffected |
| --- | --- | --- | --- |
| `authz.admin_enabled` | `/v1/authz/policies`, `/v1/authz/policies/{id}`, `/v1/authz/role-grants`, `/v1/authz/role-grants/{id}` not registered | `PutPolicy`, `DeletePolicy`, `ListPolicies`, `GrantRole`, `RevokeRole`, `ListRoleGrants` → `UNIMPLEMENTED` | `POST /v1/authz/is-authorized`, gRPC `IsAuthorized`, `Authorize::check` under `enforce_tenancy`, the policy snapshot and its reload task |
| `api_keys.management_enabled` | `/v1/service-accounts/{sa}/api-keys`, `/v1/service-accounts/{sa}/api-keys/{id}` not registered | `IssueApiKey`, `RevokeApiKey`, `ListApiKeys` → `UNIMPLEMENTED` | `/v1/authn/api-keys/introspect`, gRPC `IntrospectApiKey`, `require_bearer`'s API-key credential path, service-account lifecycle routes and RPCs |
| `audit.query_enabled` | `/v1/audit` not registered | `AuditServiceServer` not added to the router | All audit **writing**: the denial audit sink, `PgAuditLog`, partition maintenance and retention |
| `stream_enabled` (gateway) | `stream: true` → `400`, `param: "stream"` | n/a | Non-streaming `/v1/chat/completions`, `stream_idle_timeout_secs` (unread when disabled, retained for when it is re-enabled) |

### 4.3 The HTTP response

```
GET /v1/service-info
Authorization: Bearer <oidc token | pgs_sk_… key>

200 OK
Content-Type: application/json

{"service":"iam","version":"0.0.0","capabilities":["iam.authz.cedar","iam.apikeys","iam.audit"]}
```

`service` is the bare slug matching the prefix of the service's own keys — `"iam"`, `"gateway"`.
Per the proto it is advisory and must never be used by a client as a cache key.

`capabilities` is always present, as `[]` when empty. This is guaranteed structurally rather than by
convention: the DTO's field is a `Vec<String>` with no `skip_serializing_if`, and serde serializes an
empty `Vec` as `[]`.

The body is the bare `ServiceInfo`. Ordering is the registry's declaration order, so it is stable
across runs and tests can assert exact JSON.

### 4.4 The gRPC response (IAM only)

`paigasus.common.v1.ServiceInfoService/GetServiceInfo` returns
`GetServiceInfoResponse { service_info }`, with `service_info` always populated. Bearer-enforced by
the existing `AuthLayer`.

## 5. Components

### 5.1 `rs/crates/libs/paigasus-service-info` (new)

Depends on `paigasus-proto` and `serde`. No other dependencies.

```rust
/// Build the descriptor. `capabilities` are the ENABLED ones; the UNSPECIFIED
/// sentinel is dropped, duplicates are removed, order follows the registry.
pub fn descriptor(service: &str, version: &str, capabilities: &[Capability]) -> ServiceInfo;

/// The JSON body of `GET /v1/service-info`. `capabilities` is a plain `Vec`, so an
/// empty list serializes as `[]` rather than being omitted (SMA-499 § 2.7).
#[derive(Serialize)]
pub struct ServiceInfoDto { … }
impl From<&ServiceInfo> for ServiceInfoDto;

/// The route both services serve, so the path literal cannot drift.
pub const ROUTE: &str = "/v1/service-info";
```

`moon.yml`: `id: paigasus-service-info-rs`, `layer: library`, `language: rust`, with
`build` and `test` declaring `deps: ['contracts:generate']` **explicitly**, exactly as
`paigasus-proto-rs`'s own `moon.yml` does. Not left to transitivity: a project-level `dependsOn`
edge does not propagate task-affected state in Moon (SMA-389), so the dependency has to be stated on
the task.

### 5.2 `paigasus-iam`

- `config.rs` — the three flags, their defaults, their doc comments.
- `service_info.rs` (new, crate root) — `pub fn capabilities(cfg: &IamConfig) -> Vec<Capability>`, a
  **pure function of config**: no `AppState`, no database, no I/O. This is what makes AC 3's central
  assertion testable without a container (§ 6.1).
- `adapters/http/service_info.rs` (new) — `router()` plus the handler.
- `adapters/http/mod.rs` — conditional merges in `app_routes`; the service-info route joins
  `protected`. `AppState` carries the **three booleans** — route construction and the gRPC guards
  read them, and the handler passes them to `service_info::capabilities`. It does not cache a
  pre-computed `Vec<Capability>`: one source of truth, derived on demand.
- `adapters/grpc/service_info.rs` (new) — the `ServiceInfoService` impl.
- `adapters/grpc/mod.rs` — add `ServiceInfoServiceServer`; add `AuditServiceServer` conditionally.
- `adapters/grpc/authz.rs`, `adapters/grpc/service_accounts.rs` — `UNIMPLEMENTED` guards on the
  capability-scoped RPCs.
- `iam.toml.example`.

### 5.3 `paigasus-gateway`

- `config.rs` — `stream_enabled`.
- `service_info.rs` (new, crate root) — `pub fn capabilities(cfg: &GatewayConfig) -> Vec<Capability>`,
  the same pure-function shape as IAM's, tested the same container-free way.
- `adapters/iam/client.rs` — `Iam::introspect_token` over `AuthnService.Introspect`, bearer-exempt
  exactly like `introspect_api_key`.
- `adapters/http/auth.rs` — `require_authenticated`, sharing `bearer()` and the introspect error
  mapping with `require_iam_auth`.
- `adapters/http/service_info.rs` (new) — `router()` plus the handler.
- `adapters/http/mod.rs` — a second `route_layer`-protected group for the service-info route.
- `adapters/http/chat.rs` — the `stream: true` rejection at the existing `dto.stream` read.
- `adapters/http/error.rs` — the `StreamingDisabled` variant rendering `400` with `param: "stream"`.
- `gateway.toml.example`.

### 5.4 `contracts/`

`service_info.proto` — the D8 doc-comment narrowing, `buf format -w`, then all three binding trees
regenerated.

## 6. Testing

AC 3 is the acceptance criterion the issue calls "the one that matters", so its tests are specified
first and in the most detail.

### 6.1 AC 3 — capability reporting follows live config

For each of the four flags, a pair of assertions in the same test, both directions:

**Flag `true`** — the feature's surface answers (not 404 / not `UNIMPLEMENTED`) **and** the key is
present in `/v1/service-info`.

**Flag `false`** — the feature's surface is gone **and** the key is absent, **and the sibling keys
are still present**.

The sibling assertion is what makes the test non-vacuous. Asserting only "the key is absent" passes
against an implementation that returns an empty list unconditionally.

**The assertions are split across two layers, deliberately.**

*Descriptor side — no container.* `service_info::capabilities(&IamConfig)` is a pure function
(§ 5.2), so "flip the flag, the key disappears, the siblings remain" is an ordinary in-crate unit
test: build an `IamConfig`, toggle a field, compare the returned `Vec<Capability>`. No `AppState`, no
Postgres, no Docker. This is the half of AC 3 that must never be skippable.

*Surface side — Docker-gated.* Proving the route is actually gone needs a real router, and
`AppState::new` takes a live `DatabaseConnection`. The precedent is
`tests/authz_enforce_toggle.rs`, which is the closest existing analogue to this issue's AC 3: it
mutates `cfg.authz.enforce_tenancy`, builds `app_with_config(db, &cfg)`, and drives the real router.
New cases follow it exactly.

That precedent carries a known hazard: `support::start_migrated_postgres()` returns `None` and the
test **returns early** when Docker is unavailable and `CI` is unset — reporting a pass having
executed nothing, which `cargo nextest`'s skip count does not reveal. Mitigations, both required:
the pure-predicate tests above are unconditional, so the descriptor half of AC 3 is proven even on a
Docker-less machine; and the surface tests are run at least once with `CI=1` before the PR, so a
missing daemon is a hard failure rather than a silent pass.

The gRPC `UNIMPLEMENTED` guards are unit-tested at the handler, container-free.

The gateway's flag is exercised in `tests/chat_proxy.rs`'s existing harness — which needs no
database — where `stream: true` against `stream_enabled = false` returns `400` with
`param: "stream"`, and the fake upstream records that it was never called.

### 6.2 AC 2 — authenticated, and no new unauthenticated surface

- No `Authorization` header → `401` on both services' `/v1/service-info`.
- A valid bearer → `200`.
- Gateway: a valid **OIDC** token (not a `pgs_sk_` key) succeeds, proving D5's credential router —
  run against a fake `Iam` whose `is_authorized_self` panics, so the test also proves discovery makes
  no authorization call.
- IAM gRPC: `GetServiceInfo` without a bearer → `Unauthenticated`, proving the path was not added to
  `is_exempt`.

### 6.3 AC 1 — the descriptor matches the SMA-499 shape

- Exact-JSON assertion on the HTTP body: field names `service`, `version`, `capabilities`.
- IAM with all three flags `false` serializes `"capabilities":[]` — the precise case SMA-499 § 2.7's
  MUST-emit-defaults rule exists for, and the one canonical protojson would drop.
- Every advertised string resolves through `Capability::from_wire_key`, so no service can invent a key
  outside the registry.

### 6.4 AC 4 — version is wired to the build

Each service asserts the served `version` equals **that service crate's** `CARGO_PKG_VERSION`. The
test fails if anyone hardcodes a literal, and — because the builder lives in a different crate — it
also fails if the library's own version is passed by mistake.

### 6.5 Transport agreement (IAM)

One test asserting the HTTP JSON and the gRPC `GetServiceInfoResponse.service_info` carry identical
`service`, `version` and `capabilities` for the same config. This is what keeps D4's two code paths
from drifting.

### 6.6 Shared-crate unit tests

In `paigasus-service-info`: the `UNSPECIFIED` sentinel is dropped; duplicates are removed; ordering is
stable; an empty capability list serializes as `[]`; the DTO's field names match the proto's canonical
JSON names.

## 7. Repo gates this change trips

- **`ci/affected-graph/run.sh`** — strict equality, default-deny. `paigasus-service-info-rs` depends on
  `paigasus-proto`, so it enters the `contracts->proto` case's affected set and **will red that case
  until it is added** to the expected CSV (currently
  `contracts,paigasus-proto-rs,paigasus-proto-py,paigasus-proto-ts,paigasus-gateway-rs,paigasus-iam-rs`).
  This is the SMA-409 guard; the failure message names it as a legitimate new edge.
- **CODEOWNERS** — Moon-generated. Regenerated by the sync, never hand-edited.
- **`contracts:fmt`** — `buf format -w` before commit, or the gate reds silently.
- **codegen drift** — `contracts:generate` has no `outputs:` and can serve stale cache, so
  `buf generate` is run directly and all three trees committed.
- **`:breaking`** — comment-only proto change; expected green.
- **`deny` / `machete`** — no new external dependencies; expected quiet. `serde` in the new crate is
  workspace-pinned and consumed immediately.

The full graph is run before pushing, per CLAUDE.md:

```
moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke \
  :parity-corpus-drift :wasm-getrandom-free :redis-connect-single-site :promtool \
  :observability-drift :nats-permissions :release-parity :release-parity-py \
  :release-parity-ts --base origin/main --include-relations
```

## 8. Acceptance-criteria mapping

| AC | Where satisfied |
| --- | --- |
| 1 — accurate descriptor over the SMA-499 shape | D4, § 4.3, § 4.4; tested § 6.3 |
| 2 — authenticated, no new unauthenticated surface | D5; tested § 6.2 |
| 3 — derived from live config, proven by flipping a flag | D1, D2, D3, D9, § 4.2; tested § 6.1 |
| 4 — version wired to the build | D6; tested § 6.4 |

## 9. Out of scope

- **The TypeScript discovery client** — SMA-509, including the three UI states, the Redis
  `svcinfo:<service>` cache, single-flight and stale-while-revalidate. ADR-0020's "degraded" state is
  derived entirely client-side and is not expressible in this payload.
- **Python and TypeScript server-side helpers.** Only Rust serves the descriptor.
- **CI-stamped build metadata** (`0.0.0+<sha>`) — D6 records the design; attaching it needs a CI
  decision this issue does not own.
- **Registering new capability keys.** The outbox publisher backend and tenancy enforcement have no
  registered key. Appending one is cheap by construction and a wrong guess in an append-only registry
  is permanent, so none is invented here.
- **A drift gate asserting every advertised key is registered.** § 6.3 asserts it per service at test
  time; a repo-level gate is the follow-up SMA-499 § 9 anticipated now that an "advertised" side
  exists.
- **Helm values for the new flags.** Nothing in this repo is containerized yet.

## 10. Risks and sequencing

- **R1 — The new crate reds `:affected-smoke` until § 7's expected set is updated.** Known, cheap,
  and caught by a repo-level gate rather than review. It must be in the same commit as the crate.
- **R2 — IAM's two transports can drift.** Structurally mitigated (one builder) and pinned by § 6.5.
- **R3 — `AppState` gains flags that route construction reads.** `app_routes` becomes conditional,
  and axum panics at *registration* time on a path conflict. `adapters/http/mod.rs`'s existing
  `protected_router_merge_has_no_path_conflicts` test must be extended to cover the conditional
  variants, or a conflict introduced by a flag combination would only surface at boot.
- **R4 — The proto edit regenerates three binding trees for two comment changes.** Expected and
  accepted; it is the cost SMA-499 D1 already priced for any registry change.
- **R5 — An operator disabling `authz.admin_enabled` still has a working gateway but cannot manage
  policies.** Intended, and the reason D8 narrowed the key's documented meaning. The
  `iam.toml.example` entry states it.
- **R6 — AC 3's surface tests can pass without running.** IAM's Docker-gated suites return early on
  a machine without a daemon, reporting a pass in under a second. § 6.1 mitigates this on both sides
  (unconditional pure-predicate tests plus a `CI=1` run before the PR), but the mitigation is a
  procedure, not a gate, so it is recorded here as a live risk rather than a solved problem.
