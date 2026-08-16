# SMA-505 — `paigasus-iam` + `paigasus-gateway` serve the `ServiceInfo` descriptor

Linear: [SMA-505](https://linear.app/smaschek/issue/SMA-505/iamgateway-serve-the-serviceinfo-capability-descriptor)
ADR: [ADR-0020 — Service capability discovery](https://app.notion.com/p/3bb830e8fbaa8113b9f3da910893aaa8) (accepted 2026-08-13;
amended 2026-08-15 by this issue — A1–A5 record D1/D2/D3/D5/D8 below, plus the inert-skew and
404-vs-401 consequences)
Blocked by: [SMA-499](https://linear.app/smaschek/issue/SMA-499) — merged as PR 119, `ce0dc28`
Blocks: [SMA-509](https://linear.app/smaschek/issue/SMA-509) (TypeScript capability-discovery client)
Predecessor spec: `docs/superpowers/specs/2026-08-14-sma-499-service-info-capability-descriptor-design.md`

Revised after an adversarial challenge; § 11 records what the challenge changed.

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

Established by reading the worktree before designing, then re-verified under challenge. Each is
cited so a reviewer can check it rather than trust it.

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
introduced, or new keys are appended to the registry. This spec introduces flags (D1), having
considered and rejected the append-a-key route — see D1's rejected alternatives.

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

IAM's `AuthnService` exposes `Introspect` (`iam.proto:286`), bearer-exempt
(`adapters/grpc/authn.rs:126`), on the very `AuthnServiceClient` the gateway adapter already holds
(`client.rs:73`). But see 2.6 — it is not a drop-in.

### 2.4 The TypeScript workspace has no gRPC transport

`ts/packages/paigasus-proto` generates protobuf-es message types only. No `@connectrpc/*`, no
`grpc-web`, no `@grpc/grpc-js` appears in any `package.json`. SMA-509 — the TS client this issue
blocks — would need an entire transport stack added to consume a gRPC-only descriptor. This decides
D4.

### 2.5 IAM's HTTP api-key introspection route is already unauthenticated

`adapters/http/api_keys.rs::introspect_router` (`:48`) is merged **outside** the bearer
`route_layer`, mirroring `authn.rs`'s token-introspect route. Noted because it bounds what "no new
unauthenticated surface" means here: this spec adds none, and does not touch the existing ones.

### 2.6 `Introspect` refuses an unprovisioned identity, and is expensive

`AuthenticateToken::introspect` resolves with `Provisioning::Disabled`
(`application/authenticate_token.rs:147`). An unknown `(issuer, subject)` therefore returns
`AuthnError::IdentityNotProvisioned` (`:105`), which `convert::authn_status` maps to
`Code::PermissionDenied` (`adapters/grpc/convert.rs:56`), which the gateway's existing
`introspect_error` maps to `GatewayError::InvalidCredential` → **401** (`adapters/http/auth.rs:174`).

Meanwhile IAM's own `require_bearer` resolves with `Provisioning::Enabled` and JIT-provisions
(`adapters/http/auth_middleware.rs:54`).

So a freshly-logged-in console user with a perfectly valid IdP token is authenticated by IAM and
rejected by the gateway, purely on call ordering. Whether gateway discovery works would depend on
whether the console happened to hit IAM first — which is exactly the lazy-discovery flow ADR-0020 D4
describes. D5 must handle this explicitly.

`introspect` additionally pages through **every** membership row for the principal
(`authenticate_token.rs:149-159`), none of which discovery needs.

### 2.7 IAM's API-key prefix is an operator knob, not a constant

`api_keys.key_prefix` (`config.rs:222`) defaults to `pgs_sk_` but is validated as a real setting
(`config.rs:1013-1017` rejects empty and `bearer`-colliding values), which is why
`AppState.api_key_prefix` exists. `GatewayConfig` has no equivalent field and needs none today, since
`require_iam_auth` calls `introspect_api_key` unconditionally. Any gateway-side design that
*branches* on the prefix would silently break for an operator who changed it. D5 avoids branching.

### 2.8 Every crate in the workspace is `version = "0.0.0"`, and release-plz is dormant

All eleven crates under `rs/crates/` carry `version = "0.0.0"`. `rs/release-plz.toml:1-7` states:
"NO release-plz workflow is wired yet: activation (0.0.0 -> 0.1.0, live release PRs/tags) is deferred
to E-activate."

Two consequences, both load-bearing:

1. Nothing bumps the version, so every deployment reports `"0.0.0"` until E-activate. ADR-0020's
   N-1-minor skew reporting is therefore inert on arrival.
2. **Any test of the form `assert_eq!(served_version, env!("CARGO_PKG_VERSION"))` is vacuous** — it
   passes against a hardcoded `"0.0.0"` literal and against the shared library's own version, because
   all three strings are identical. D6 and § 6.4 are written around this.

### 2.9 The gateway's `AppState` and `Iam` trait have many construction sites

Unlike IAM's `AppState` (private fields, built only via `AppState::new`, so a config addition is
absorbed), the gateway's `AppState` is a public literal-constructed struct
(`adapters/http/mod.rs:39-46`) built at `src/main.rs:49`, `adapters/http/mod.rs:206-212`,
`tests/metrics.rs:85` and `:135`, and `tests/chat_proxy.rs:125`. The `Iam` trait has six impls:
`UnusedIam`/`ProbeIam` (`http/mod.rs:151`, `:179`), `FakeIam` (`http/auth.rs:260`),
`UnusedIam`/`AllowedIam` (`tests/metrics.rs:43`, `:57`), `FakeIam` (`tests/chat_proxy.rs:88`).

`GatewayConfig` also carries a hand-written `Defaults` mirror struct (`config.rs:127-137`); a new
field missing from it fails figment extraction at **runtime**, not compile time.

## 3. Decisions

### D1 — Capabilities are real feature toggles, not descriptor-only flags

Each capability gets a configuration flag that genuinely changes what the service serves. The
descriptor reads the same flag. One truth, two observable effects: flipping the flag makes the
feature's surface disappear **and** the key disappear.

**Rejected: a `[capabilities]` block read only by the descriptor.** It satisfies AC 3's letter at a
fraction of the cost, and recreates the issue's own stated failure in mirror image: a descriptor that
says "off" while the endpoint still answers misleads the console exactly as badly as one that says
"on" for a feature the operator disabled. The note closing the issue — "worse than no descriptor" —
applies in both directions.

**Rejected: append one key mapping to an operator choice that already varies** — e.g. the outbox
publisher backend (`PublisherBackend::{Tracing, Nats}`, `config.rs:568`) — and prove AC 3 against
that, with zero new flags and zero conditional routing. This is the cheapest design that satisfies
AC 3 and it was considered seriously. Rejected because it satisfies AC 3 while leaving all **four**
registered keys as compile-time constants, which is precisely the state the issue calls "worse than
no descriptor". A registry whose every existing key is a constant, plus one honest key bolted on to
pass a test, is a worse outcome than four honest keys. The append-a-key route also permanently
commits a new key to an append-only registry, which is the one decision here that cannot be undone.

**Operator use cases**, stated because a flag without one is a liability:

| Flag | Who turns it off |
| --- | --- |
| `authz.admin_enabled` | A deployment whose policies are managed exclusively as code (GitOps-applied at boot), closing the runtime mutation surface so a compromised admin token cannot rewrite policy. |
| `api_keys.management_enabled` | A deployment where service accounts are provisioned out-of-band and long-lived; closes runtime key issuance without disturbing existing keys. |
| `audit.query_enabled` | A deployment shipping audit to an external SIEM, where the in-product reader is redundant and its query load on the partitioned table is unwanted. |
| `stream_enabled` (gateway) | An upstream or compliance posture that requires complete, inspectable responses — a mid-stream SSE body cannot be scanned before the first byte reaches the client. |

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

**Accepted consequence: unmounting creates an unauthenticated 404-vs-401 oracle.** `route_layer` runs
only on matched routes (`adapters/http/mod.rs:816-818`), so an anonymous `GET /v1/audit` returns 401
when `audit.query_enabled = true` and 404 when false. The capability list is auth-gated by AC 2, yet
the same bit is readable without any credential. This is accepted rather than mitigated: the proto
declares capability gating "COSMETIC" and explicitly not a security boundary
(`service_info.proto:76-78`), the server remains authoritative, and a deployment's enabled feature
set is not a secret. Recorded so AC 2's "no new unauthenticated surface" is read precisely — no new
*route* is unauthenticated; one bit of configuration became inferable.

### D3 — On gRPC, a disabled RPC returns `UNIMPLEMENTED`

Per 2.2, `AuthorizationService` and `ServiceAccountService` cannot be unmounted wholesale.
Capability-scoped RPCs on those two services check the flag and return
`tonic::Status::unimplemented` before doing any work. `AuditService`, which is wholly within
`iam.audit`, is simply not added to the tonic router when disabled.

These are observably identical: a client calling an RPC on a service the server never registered also
receives `UNIMPLEMENTED`. So the rule is uniform across both mechanisms and consistent with HTTP's
404 — a disabled capability is indistinguishable from a build that never had it.

`Router::add_service` returns `Self`, so conditionally adding `AuditServiceServer` does not disturb
`grpc::router`'s concrete `TonicRouter<Stack<AuthLayer, Identity>>` return type (`grpc/mod.rs:56`).
An unauthenticated call to an unregistered service still passes through `AuthLayer` first, so the
`Unauthenticated`-before-`Unimplemented` ordering stays uniform with every other RPC.

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
builder, and § 6.5 requires a test asserting the two transports agree.

`/v1/service-info` introduces no axum path conflict — it is a distinct literal segment, and does not
collide with `/v1/service-accounts` or its children.

Per SMA-499 D3 the two bodies differ by design — HTTP returns the **bare** `ServiceInfo`, gRPC returns
`GetServiceInfoResponse { service_info }`. The wrapper is a buf-lint artefact.

### D5 — Both routes require authentication and perform no authorization

**IAM.** The route joins the existing `protected` sub-router, behind
`auth_middleware::require_bearer`. That middleware already routes a token carrying the configured
`api_key_prefix` to the API-key authenticator and everything else to OIDC, so a console user session
and a service-account key both work with no new middleware. The gRPC `GetServiceInfo` is covered
automatically: `AuthLayer` enforces a bearer on every path absent from `is_exempt`
(`adapters/grpc/authn.rs:125`), and `ServiceInfoService/GetServiceInfo` is not added to it.

*Side effect, stated deliberately:* inheriting `require_bearer` means a `GET /v1/service-info` also
inherits `Provisioning::Enabled` JIT provisioning and `bootstrap_seeder.ensure_platform_admin`
(`auth_middleware.rs:54`, `:66-68`). A discovery `GET` can therefore create a principal, user and
external-identity row, and grant `platform_admin` to a configured bootstrap identity. This is
identical to every other protected IAM route and is not changed here — but it is a write on a read
endpoint, so it is recorded rather than left for a reader to discover.

**Gateway.** A new `require_authenticated` middleware **tries both introspections in order** and
accepts the first success: `Iam::introspect_api_key` (cheaper — no membership enumeration), and on
failure `Iam::introspect_token`, a new port method wrapping IAM's OIDC `Introspect`. A valid, active
principal from either is sufficient; **no** `is_authorized_self` call is made.

*Why try-both rather than branch on the token prefix.* Per 2.7 the `pgs_sk_` prefix is an operator
knob configured in IAM, and the gateway has no visibility of it. A gateway that branched on a
hardcoded prefix would silently route every service-account key to the OIDC path — and rejecting it —
for any operator who changed `api_keys.key_prefix`, with no boot error and no natural test coverage.
Adding a mirrored `iam.api_key_prefix` to `GatewayConfig` would fix that at the cost of a
must-match-or-break coupling between two services' configs. Trying both costs one extra RPC on the
OIDC path of a low-frequency, client-cached call, and cannot drift.

*An authenticated-but-unprovisioned identity is accepted for discovery.* Per 2.6, `Introspect`
returns `PermissionDenied` for a validated token whose `(issuer, subject)` has no local principal.
On the discovery path **only**, that outcome is treated as authenticated: the token's signature,
issuer and expiry were verified by IAM before the lookup, and the descriptor is byte-identical for
every caller — it exposes no per-principal data whatsoever. Rejecting here would make gateway
discovery depend on whether the console happened to call IAM first, breaking the exact lazy
in-user-request flow ADR-0020 D4 specifies. `require_iam_auth` on the chat path is **unchanged**;
this relaxation is scoped to `require_authenticated` and tested explicitly (§ 6.2).

*Discovery becomes unavailable during an IAM outage.* `stream_enabled` is a purely local fact, yet
gating the route on an IAM round-trip means `introspect_error`'s `Connect`/`Unavailable`/
`DeadlineExceeded`/`Internal` → `IamUnavailable` → 503 mapping (`adapters/http/auth.rs:170-178`)
applies to discovery too. Accepted: AC 2 forbids an unauthenticated surface, the gateway has no
independent way to authenticate anyone, and ADR-0020's client-side "degraded" state is precisely the
right rendering for it. The alternative — serving a cached or unauthenticated descriptor during an
outage — trades AC 2 for cosmetics. Pinned by a test (§ 6.2) so the behaviour is deliberate rather
than emergent.

*Observability.* `require_authenticated` records the shared `gateway_iam_calls_total` metric under a
**new** `operation = "introspect_token"` label value, alongside the existing `introspect`. That
metric drives the `rate(gateway_iam_calls_total{result="unavailable"}[5m]) > 0` alert
(`ops/observability/prometheus/rules/gateway.rules.yml:11`) and a Grafana panel grouped
`by (operation, result)` (`ops/observability/grafana/dashboards/gateway.json:74`). Recording nothing
would make IAM outages on the discovery path invisible to that alert; reusing `introspect` would
conflate two different RPCs. Because `:observability-drift` only checks that names resolve, the
`describe_gateway_metrics` text at `paigasus-gateway/src/main.rs:151-154` (currently
"introspect/authorize") is updated in the **same commit**, and § 6.2 asserts a `Connect`-failing IAM
increments `result="unavailable"`.

Discovery is deliberately not gated on an action. `InvokeModel` — what `require_iam_auth` demands —
would make a caller who legitimately cannot invoke models unable to discover that streaming exists,
and would force ADR-0020's console to hold a service credential for the gateway, which D4 of that ADR
rejects outright.

Errors keep each service's existing envelope: IAM's `AuthnApiError` 401 funnel, the gateway's
OpenAI-shaped envelope (ADR-0019 — envelopes are per-surface).

### D6 — Version comes from `CARGO_PKG_VERSION`, read in the service crate

`env!("CARGO_PKG_VERSION")` is evaluated in each **service** crate and passed into the shared builder.
Nothing is hand-written, no build script is added, the build needs no git checkout, and rebuilds are
byte-identical. This satisfies AC 4: the value is wired to the build rather than maintained by hand.

**What it does not do, stated plainly.** Per 2.8, release-plz is dormant and every crate is `0.0.0`,
so this reports `"0.0.0"` on every deployment until E-activate wires the release workflow. ADR-0020's
N-1-minor skew reporting is therefore **inert until then** — not degraded, inert. That is acceptable
because the proto forbids clients from ever gating a feature decision on version and requires them to
tolerate an unparseable value by suppressing skew reporting, so an inert-but-valid version breaks
nothing. It is recorded as R7 rather than left implied, because a reader could otherwise assume
skew reporting works the day this ships.

The same fact makes the obvious AC 4 test vacuous (2.8). § 6.4 is designed around that.

Rejected: a `build.rs` embedding `git describe`, which makes the build depend on a git checkout (a
Docker build from a source tarball yields a different or empty value) and makes output
non-deterministic, at odds with this repo's codegen-drift posture.

Deferred: appending CI-stamped SemVer build metadata (`0.0.0+ce0dc28`). This would make the version
genuinely vary and would make § 6.4's test non-vacuous — but it needs a CI decision this issue does
not own, and it does not restore skew reporting (build metadata is ignored in SemVer precedence).
Recorded in § 9.

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

It also carries AC 4's only non-vacuous assertion: because `descriptor()` takes `version` as a
parameter, the shared crate can prove a distinct sentinel string flows through to
`ServiceInfo.version` verbatim (§ 6.4), which no service-level test can do while every crate is
`0.0.0`.

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

Each narrowed doc comment additionally states that the primitive stays available when the capability
is off, so a reader does not infer the wider meaning from the key's name.

**Why re-scoping a shipped key is safe here, and why it would not be later.** SMA-499's file comment
makes the doc comment the human-readable contract and names review discipline as its only guard
(`service_info.proto:59-63`), so "comment-only, `:breaking` stays green" is a statement about tooling
and not a safety argument. The safety argument is that the contract has **no consumers**: it landed
one commit ago (`ce0dc28`), no service advertises any key until this issue, SMA-509 is unbuilt, and
nothing outside the repo has seen it. This is the last moment at which narrowing is free. Recorded
explicitly so the precedent is not read as "doc comments are soft" — after this issue ships, a key's
documented meaning is as fixed as its string.

Rejected: appending `iam.authz.admin` and `iam.apikeys.management` as new keys and leaving the
originals unadvertised. Cleaner in the abstract, but it permanently commits two keys to an
append-only registry and leaves two others defined-but-never-advertised, which is a worse artefact
than two narrowed comments.

`docs/superpowers/specs/2026-08-14-sma-499-service-info-capability-descriptor-design.md` § 4.2's
table is updated in the same commit, since it restates the old wording and would otherwise be stale.

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

*This is not a one-variant change.* `GatewayError::parts()` returns a 4-tuple and `into_response`
hardcodes `param: None` (`adapters/http/error.rs:88-121`, `:130`), and the module doc asserts
"`param` is always `null` for gateway-originated errors" (`:30-32`). D9 requires threading a `param`
through the shared constructor and rewriting that doc statement. Budgeted in § 5.3.

## 4. The contract

### 4.1 Configuration

| Service | Flag | Default | Governs |
| --- | --- | --- | --- |
| iam | `authz.admin_enabled` | `true` | `iam.authz.cedar` |
| iam | `api_keys.management_enabled` | `true` | `iam.apikeys` |
| iam | `audit.query_enabled` | `true` | `iam.audit` |
| gateway | `stream_enabled` | `true` | `gateway.chat.stream` |

Flags live inside the existing config blocks they belong to, so environment overrides follow the
established figment shape (`Env::prefixed(..).split("__")`, `iam/config.rs:891`,
`gateway/config.rs:220`): `IAM_AUTHZ__ADMIN_ENABLED`, `IAM_API_KEYS__MANAGEMENT_ENABLED`,
`IAM_AUDIT__QUERY_ENABLED`, `GATEWAY_STREAM_ENABLED`. The gateway's flag is top-level, matching its
sibling `stream_idle_timeout_secs`, and must also be added to that crate's hand-written `Defaults`
mirror struct (2.9) or figment extraction fails at runtime.

Each is documented in `iam.toml.example` / `gateway.toml.example` with what disabling it removes and
what it deliberately leaves running. **No CI gate reads `*.toml.example`** — nothing in `moon.yml`,
`.moon/tasks.yml` or `ci/` references them — so this is review-enforced only, stated here so nobody
assumes otherwise.

No cross-field *validation* is added: under D2 and D8 every flag is independent and no combination
leaves a service incoherent. One boot-time **warning** is added: with `audit.query_enabled = false`
the service still writes audit entries nobody can read in-product, which is legitimate when shipping
to an external SIEM (D1) and a misconfiguration otherwise. This mirrors the existing startup warns
for disabled retention and unbounded stream age.

### 4.2 What each flag removes

| Flag off | HTTP | gRPC | Deliberately unaffected |
| --- | --- | --- | --- |
| `authz.admin_enabled` | `/v1/authz/policies`, `/v1/authz/policies/{id}`, `/v1/authz/role-grants`, `/v1/authz/role-grants/{id}`, **and `/v1/authz/system-policies/{id}/retire`** not registered | `PutPolicy`, `DeletePolicy`, `ListPolicies`, `GrantRole`, `RevokeRole`, `ListRoleGrants` → `UNIMPLEMENTED` | `POST /v1/authz/is-authorized`, gRPC `IsAuthorized`, `Authorize::check` under `enforce_tenancy`, the policy snapshot and its reload task |
| `api_keys.management_enabled` | `/v1/service-accounts/{sa}/api-keys`, `/v1/service-accounts/{sa}/api-keys/{id}` not registered | `IssueApiKey`, `RevokeApiKey`, `ListApiKeys` → `UNIMPLEMENTED` | `/v1/authn/api-keys/introspect`, gRPC `IntrospectApiKey`, `require_bearer`'s API-key credential path, service-account lifecycle routes and RPCs |
| `audit.query_enabled` | `/v1/audit` not registered | `AuditServiceServer` not added to the router | All audit **writing**: the denial audit sink, `PgAuditLog`, partition maintenance and retention |
| `stream_enabled` (gateway) | `stream: true` → `400`, `param: "stream"` | n/a | Non-streaming `/v1/chat/completions` |

**`/v1/authz/system-policies/{id}/retire` is included in the authz row deliberately.**
`adapters/http/system_retirement.rs:66` registers a privileged route that bypasses
`PolicyStore::delete_in`'s `SystemImmutable` guard — it is policy administration, and the most
privileged kind. Leaving it mounted while advertising "policy administration is unavailable" would
make the descriptor inaccurate in the most dangerous direction, so it unmounts with the rest.

`stream_idle_timeout_secs` is **not** listed as unaffected-and-unread: it is passed to
`OpenAiClient::new` unconditionally (`gateway/src/main.rs:46`) and `GatewayConfig::validate` still
rejects `0` regardless of `stream_enabled` (`config.rs:239-243`). It is simply never reached on a
request path when streaming is off. No change is made to either.

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

The body is the bare `ServiceInfo`.

**Ordering is an implementation detail, not a contract.** The proto states the list is unordered and
that clients must build a set from it (`service_info.proto:74-75`). The builder nonetheless emits a
deterministic order — `sort_by_key(|c| *c as i32)` over the enum discriminant, then dedup — purely so
output is stable across runs. Tests therefore assert **set equality**, never sequence equality, so
they do not encode a property the contract disclaims. (prost emits no variant iterator, and
`capability.rs`'s `ALL` array is private and `#[cfg(test)]`, so the discriminant is the only available
ordering source.)

No `Cache-Control` or `ETag` is emitted. Deliberately none: ADR-0020 places caching in the client,
under `svcinfo:<service>` with its own TTL and single-flight, and a second server-side caching policy
would only give the two places to disagree.

### 4.4 The gRPC response (IAM only)

`paigasus.common.v1.ServiceInfoService/GetServiceInfo` returns
`GetServiceInfoResponse { service_info }`, with `service_info` always populated. Bearer-enforced by
the existing `AuthLayer`.

## 5. Components

### 5.1 `rs/crates/libs/paigasus-service-info` (new)

Depends on `paigasus-proto` and `serde`. No other dependencies.

```rust
/// Build the descriptor. `capabilities` are the ENABLED ones; the UNSPECIFIED
/// sentinel is dropped, duplicates are removed, order is by enum discriminant.
pub fn descriptor(service: &str, version: &str, capabilities: &[Capability]) -> ServiceInfo;

/// The JSON body of `GET /v1/service-info`. `capabilities` is a plain `Vec`, so an
/// empty list serializes as `[]` rather than being omitted (SMA-499 § 2.7).
#[derive(Serialize)]
pub struct ServiceInfoDto { … }
impl From<&ServiceInfo> for ServiceInfoDto;

/// The route both services serve, so the path literal cannot drift.
pub const ROUTE: &str = "/v1/service-info";
```

`version` is a parameter, not read from this crate's own `CARGO_PKG_VERSION` — that is what lets
§ 6.4 prove flow-through with a sentinel.

`moon.yml`: `id: paigasus-service-info-rs`, `layer: library`, `language: rust`, with
`build` and `test` declaring `deps: ['contracts:generate']` **explicitly**, exactly as
`paigasus-proto-rs`'s own `moon.yml` does. Not left to transitivity: a project-level `dependsOn`
edge does not propagate task-affected state in Moon (SMA-389), so the dependency has to be stated on
the task.

### 5.2 `paigasus-iam`

The capability predicate operates on a small value type, **not** on `IamConfig`:

```rust
/// The three capability toggles, projected out of IamConfig once at wiring time.
#[derive(Clone, Copy)]
pub struct Capabilities { authz_admin: bool, apikeys_management: bool, audit_query: bool }

impl Capabilities {
    pub fn from_config(cfg: &IamConfig) -> Self;
    /// Pure, container-free — the unit under test for AC 3's descriptor half.
    pub fn enabled(&self) -> Vec<Capability>;
}
```

`AppState` holds one `Capabilities`. Route construction, the gRPC guards and the descriptor handler
all read that same value, so there is one source of truth and nothing is cached separately.

Storing `&IamConfig` on `AppState` instead was rejected outright: `IamConfig` transitively carries
`RawPepper` (`config.rs:272`) and every `RedactedUrl` (`config.rs:61`), so it would clone the API-key
pepper into every HTTP and gRPC worker's request state.

Files:

- `config.rs` — the three flags, their defaults, their doc comments, the boot-time audit warn.
- `service_info.rs` (new, crate root) — `Capabilities` and the descriptor assembly.
- `adapters/http/service_info.rs` (new) — `router()` plus the handler.
- `adapters/http/mod.rs` — `AppState.capabilities`; conditional merges in `app_routes`; the
  service-info route joins `protected`; extend `protected_router_merge_has_no_path_conflicts`.
- `adapters/grpc/service_info.rs` (new) — the `ServiceInfoService` impl.
- `adapters/grpc/mod.rs` — add `ServiceInfoServiceServer`; add `AuditServiceServer` conditionally.
- `adapters/grpc/authz.rs`, `adapters/grpc/service_accounts.rs` — `UNIMPLEMENTED` guards.
- `iam.toml.example`.

IAM's own construction surface absorbs the change: `AppState` has private fields and is built only
via `AppState::new`, and `tests/support/mod.rs:342-344`'s `test_config_with` uses
`AuthzConfig::default()` / `AuditConfig::default()`, so existing tests inherit the `true` defaults
without edits.

### 5.3 `paigasus-gateway`

- `config.rs` — `stream_enabled`, **and the matching field in the hand-written `Defaults` mirror
  struct** (`config.rs:127-137`); omitting the latter fails figment extraction at runtime, not
  compile time.
- `service_info.rs` (new, crate root) — the same `Capabilities` shape over `GatewayConfig`, tested
  the same container-free way.
- `adapters/iam/client.rs` — `Iam::introspect_token` over `AuthnService.Introspect`, bearer-exempt
  exactly like `introspect_api_key`. **Adding this trait method touches six impls**:
  `UnusedIam`/`ProbeIam` (`http/mod.rs:151`, `:179`), `FakeIam` (`http/auth.rs:260`),
  `UnusedIam`/`AllowedIam` (`tests/metrics.rs:43`, `:57`), `FakeIam` (`tests/chat_proxy.rs:88`).
- `adapters/http/auth.rs` — `require_authenticated` (try-both introspects, unprovisioned accepted,
  `operation = "introspect_token"` metric), sharing `bearer()` with `require_iam_auth`.
- `adapters/http/service_info.rs` (new) — `router()` plus the handler.
- `adapters/http/mod.rs` — `AppState.stream_enabled`; a second `route_layer`-protected group for the
  service-info route. **`AppState` is a public literal-constructed struct**, so the new field touches
  five construction sites: `src/main.rs:49`, `adapters/http/mod.rs:206-212`, `tests/metrics.rs:85`
  and `:135`, `tests/chat_proxy.rs:125`.
- `adapters/http/chat.rs` — the `stream: true` rejection at the existing `dto.stream` read.
- `adapters/http/error.rs` — the `StreamingDisabled` variant, plus threading `param` through
  `parts()`/`into_response` and correcting the module doc's "always `null`" claim (D9).
- `src/main.rs:151-154` — `describe_gateway_metrics`'s `operation` text, in the same commit (D5).
- `gateway.toml.example`.

### 5.4 `contracts/`

`service_info.proto` — the D8 doc-comment narrowing, `buf format -w`, then all three binding trees
regenerated. Plus the SMA-499 spec's § 4.2 table, per D8.

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

**Combinations, not just single flags.** R3's real risk surface is interaction, so the predicate
tests cover the full 2³ = 8 IAM combinations (cheap — it is a pure function), and the router tests
cover at minimum one two-off/one-on case and the all-off case, since those are where conditional
`app_routes` merging can produce a path conflict or an unexpected 404.

**The assertions are split across two layers, deliberately.**

*Descriptor side — no container.* `Capabilities::enabled()` is a pure function of three booleans
(§ 5.2), so "flip the flag, the key disappears, the siblings remain" is an ordinary in-crate unit
test. No `AppState`, no Postgres, no Docker. This is the half of AC 3 that must never be skippable.

*Surface side — Docker-gated.* Proving the route is actually gone needs a real router, and
`AppState::new` takes a live `DatabaseConnection`. The precedent is
`tests/authz_enforce_toggle.rs`, the closest existing analogue to this issue's AC 3: it mutates
`cfg.authz.enforce_tenancy`, builds `app_with_config(db, &cfg)`, and drives the real router. New
cases follow it exactly.

That precedent carries a known hazard: `support::start_migrated_postgres()` returns `None` and the
test **returns early** when Docker is unavailable and `CI` is unset (`authz_enforce_toggle.rs:24-26`)
— reporting a pass having executed nothing, which `cargo nextest`'s skip count does not reveal.
Mitigations, both required: the pure-predicate tests above are unconditional, so the descriptor half
of AC 3 is proven even on a Docker-less machine; and the surface tests are run at least once with
`CI=1` before the PR, so a missing daemon is a hard failure rather than a silent pass.

The gRPC `UNIMPLEMENTED` guards are unit-tested at the handler, container-free.

The gateway's flag is exercised in `tests/chat_proxy.rs`'s existing harness — which needs no
database — where `stream: true` against `stream_enabled = false` returns `400` with
`param: "stream"`, and the fake upstream records that it was never called.

**Existing tests that assume a route is mounted keep passing**, because every flag defaults to `true`
and both services' test configs are built from `Default` (§ 5.2). Any test that must exercise a
disabled route sets the flag explicitly.

### 6.2 AC 2 — authenticated, and no new unauthenticated surface

- No `Authorization` header → `401` on both services' `/v1/service-info`.
- A valid bearer → `200`.
- Gateway, valid **OIDC** token → `200`, proving D5's try-both introspection — run against a fake
  `Iam` whose `is_authorized_self` panics, so the test also proves discovery makes no authorization
  call.
- Gateway, valid OIDC token for an **unprovisioned** identity (`introspect_token` returning
  `PermissionDenied`) → `200`. This is D5's deliberate relaxation and the one most likely to be
  "fixed" back into a 401 by a later reader, so it is asserted directly.
- Gateway, the same unprovisioned-identity credential against `/v1/chat/completions` → still `401`,
  proving the relaxation is scoped to discovery and did not leak onto the chat path.
- Gateway, IAM unreachable (`IamError::Connect`) → `503`, **and** `gateway_iam_calls_total` records
  `operation="introspect_token", result="unavailable"` (D5's observability decision). Asserted by
  reading a parsed metric value, never by a `contains()` on a `# TYPE` line.
- IAM gRPC `GetServiceInfo` without a bearer → `Unauthenticated`, proving the path was not added to
  `is_exempt`.

### 6.3 AC 1 — the descriptor matches the SMA-499 shape

- Exact-JSON assertion on the HTTP body's **field names and shape** (`service`, `version`,
  `capabilities`), with the capability list compared as a **set** (§ 4.3).
- IAM with all three flags `false` serializes `"capabilities":[]` — the precise case SMA-499 § 2.7's
  MUST-emit-defaults rule exists for, and the one canonical protojson would drop.
- Every advertised string resolves through `Capability::from_wire_key`, so no service can invent a key
  outside the registry.

### 6.4 AC 4 — version is wired to the build

Designed around 2.8: every crate is `0.0.0`, so `assert_eq!(served, env!("CARGO_PKG_VERSION"))`
proves nothing — it passes against a hardcoded literal and against the library's own version alike.
Three assertions replace it:

1. **Flow-through, in the shared crate.** `descriptor("svc", "9.9.9-test-sentinel", &[])` yields a
   `ServiceInfo` whose `version` is exactly `"9.9.9-test-sentinel"`. This is the only assertion here
   that can actually fail today, and it pins the one thing the library controls: it neither rewrites
   nor substitutes the caller's version.
2. **Wiring, in each service.** The service exposes `const VERSION: &str = env!("CARGO_PKG_VERSION");`
   in its `service_info` module and the handler is required to use it. The test asserts the served
   value equals `env!("CARGO_PKG_VERSION")` **expanded in the test itself**, not read back from that
   const — so substituting a literal for the const makes the served value diverge from the crate's
   real `Cargo.toml` version and fails, where comparing against the const would pass trivially.
3. **Non-empty and SemVer-shaped**, so an empty or malformed value fails loudly.

The spec states plainly what is *not* proven: while every crate is `0.0.0`, no test can distinguish
the service's version from the library's or from a hardcoded `"0.0.0"`. That becomes provable for
free once release-plz activates or CI stamps build metadata (D6, § 9), and until then it is a stated
gap rather than a false assurance.

### 6.5 Transport agreement (IAM)

One test asserting the HTTP JSON and the gRPC `GetServiceInfoResponse.service_info` carry identical
`service`, `version` and capability **set** for the same config. This is what keeps D4's two code
paths from drifting.

### 6.6 Shared-crate unit tests

In `paigasus-service-info`: the `UNSPECIFIED` sentinel is dropped; duplicates are removed; ordering by
discriminant is deterministic; an empty capability list serializes as `[]`; the DTO's field names
match the proto's canonical JSON names.

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
- **`:observability-drift`** — only checks that metric names resolve, so it will **not** catch the new
  `operation` label value going undocumented. D5 handles that by updating `describe_gateway_metrics`
  in the same commit; nothing automated enforces it.
- **`deny` / `machete`** — no new external dependencies; expected quiet. `serde` in the new crate is
  workspace-pinned and consumed immediately.
- **Not gated at all:** `iam.toml.example` / `gateway.toml.example`. Review-enforced only (§ 4.1).

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
| 2 — authenticated, no new unauthenticated surface | D5, and D2's accepted 404/401 oracle; tested § 6.2 |
| 3 — derived from live config, proven by flipping a flag | D1, D2, D3, D9, § 4.2; tested § 6.1 |
| 4 — version wired to the build | D6; tested § 6.4, with its limits stated there and in R7 |

## 9. Out of scope

- **The TypeScript discovery client** — SMA-509, including the three UI states, the Redis
  `svcinfo:<service>` cache, single-flight and stale-while-revalidate. ADR-0020's "degraded" state is
  derived entirely client-side and is not expressible in this payload.
- **Python and TypeScript server-side helpers.** Only Rust serves the descriptor.
- **CI-stamped build metadata** (`0.0.0+<sha>`) — D6 records the design; attaching it needs a CI
  decision this issue does not own. It would also make § 6.4's second assertion non-vacuous.
- **Registering new capability keys.** The outbox publisher backend and tenancy enforcement have no
  registered key; D1 records why appending one was rejected as this issue's AC-3 vehicle.
- **A drift gate asserting every advertised key is registered.** § 6.3 asserts it per service at test
  time; a repo-level gate is the follow-up SMA-499 § 9 anticipated now that an "advertised" side
  exists.
- **Helm values for the new flags.** Nothing in this repo is containerized yet.

## 10. Risks and sequencing

- **R1 — The new crate reds `:affected-smoke` until § 7's expected set is updated.** Known, cheap,
  and caught by a repo-level gate rather than review. It must be in the same commit as the crate.
- **R2 — IAM's two transports can drift.** Structurally mitigated (one builder) and pinned by § 6.5.
- **R3 — Conditional `app_routes` can panic at registration.** axum panics inside `.route`/`.merge`
  on a path conflict, so a bad flag combination would surface at boot rather than in review.
  `adapters/http/mod.rs`'s existing `protected_router_merge_has_no_path_conflicts` test is extended
  to cover the conditional variants, and § 6.1 requires multi-flag combination cases.
- **R4 — The proto edit regenerates three binding trees for two comment changes.** Expected and
  accepted; it is the cost SMA-499 D1 already priced for any registry change.
- **R5 — An operator disabling `authz.admin_enabled` still has a working gateway but cannot manage
  policies, and loses system-policy retirement.** Intended, and the reason D8 narrowed the key's
  documented meaning. The `iam.toml.example` entry states both.
- **R6 — AC 3's surface tests can pass without running.** IAM's Docker-gated suites return early on
  a machine without a daemon, reporting a pass in under a second. § 6.1 mitigates this on both sides
  (unconditional pure-predicate tests plus a `CI=1` run before the PR), but the mitigation is a
  procedure, not a gate, so it is recorded here as a live risk rather than a solved problem.
- **R7 — `version` is permanently `"0.0.0"` until E-activate, so ADR-0020's skew reporting is inert
  and § 6.4's service-level assertions cannot fail.** Not fixable within this issue (2.8, D6). Stated
  in the spec, in § 6.4, and worth a line in SMA-509's contract so the console does not build a skew
  banner against a constant.
- **R8 — D5's unprovisioned-identity relaxation is a security-adjacent decision that reads like a
  bug.** A later reader may "fix" it back to a 401 and silently break lazy discovery. Mitigated by
  the paired tests in § 6.2 (discovery 200, chat 401) and by naming the rationale in the middleware's
  doc comment.

## 11. What the adversarial challenge changed

Recorded so the next reader can see which parts were revised under pressure and why.

**Blockers fixed.** AC 4's test design was vacuous — every crate is `0.0.0`, so the obvious assertion
passes against a hardcoded literal (2.8, § 6.4 rewritten). D6's premise that "release-plz already owns
the number" was false; it is dormant (2.8, R7). § 5.1/§ 5.2 contradicted each other on the capability
predicate's input, and one reading would have cloned the API-key pepper into request state (§ 5.2's
`Capabilities` type). D5's gateway middleware would have 401'd exactly the unprovisioned console user
ADR-0020 D4 describes (2.6, D5's relaxation).

**Majors fixed.** The `pgs_sk_` prefix is an operator knob, so D5 tries both introspections rather
than branching. `/v1/authz/system-policies/{id}/retire` is policy administration and now unmounts with
the authz row. D8 gained an actual safety argument — no consumers yet — rather than resting on
"`:breaking` stays green". § 5.3 now lists the gateway's five `AppState` sites, six `Iam` impls and
the `Defaults` mirror-struct trap. D5 gained the `gateway_iam_calls_total` observability decision.
The IAM-outage 503 coupling is now stated and tested.

**Minors fixed.** The `stream_idle_timeout_secs` claim was wrong. D2 now states the 404-vs-401 oracle
and accepts it. D9 budgets the wider `error.rs` `param`-threading change. § 4.3 specifies the ordering
mechanism and switches tests to set equality, since the contract disclaims ordering. D5 states IAM's
discovery-`GET`-provisions side effect and `Introspect`'s membership-enumeration cost. § 4.1 and § 7
state that `*.toml.example` has no gate. § 4.3 states that no cache headers are emitted, deliberately.
A boot-time warn is added for audit-writes-but-unreadable. § 6.1 gained multi-flag combination cases.

**Challenged and kept.** The core D1 decision — four real feature toggles rather than appending one
key mapping to the outbox publisher backend, which would have satisfied AC 3 with no new flags and no
conditional routing. Kept because the cheap route leaves all four registered keys as compile-time
constants, the exact state the issue calls "worse than no descriptor"; D1 now records the rejected
alternative and each flag's operator use case, which the challenge correctly noted were missing.
