# SMA-446 (#2) — AI Gateway M0 walking skeleton + IAM-backed auth

**Status:** Design (brainstormed) · **Date:** 2026-07-13 · **Linear:** SMA-446 (Part of; the
"gateway consumes IAM" AC) · **Crates:** `paigasus-gateway` (service) + IAM changes in
`paigasus-iam` / `paigasus-iam-core` / `contracts` / `paigasus-proto`.

---

## 1. Context

The [AI Gateway PRD](https://app.notion.com/p/385830e8fbaa809dbe29ff5deb10cc17) describes a full
Rust LLM gateway (dual OpenAI+Anthropic ingress, canonical model, multi-provider routing, virtual
keys/auth/rate-limit, cost/budgets, caching/observability) delivered across its own milestones
**M0–M5**. Its §12 open question — *"reuse IAM, or keep gateway keys self-contained?"* — is the one
**SMA-446 resolves: the gateway consumes IAM** (IAM-issued keys + `is_authorized`), no
self-contained key store.

The `paigasus-gateway` crate is currently a `fn main(){}` stub. You cannot integrate IAM into a
gateway that doesn't exist, so this sub-project is the **start of the gateway product**: the PRD's
**M0 walking skeleton, with IAM as the auth/authz backbone from day one**. IAM (merged M1–M5) now
exposes exactly what's needed: `AuthnService.IntrospectApiKey`, `AuthorizationService.IsAuthorized`.

**Scope decided with Sven:** M0 + IAM auth; REST-only Rust structs (canonical model + `contracts/`
deferred to gateway M1/M2); authz = a **new project-scoped `InvokeModel` Cedar action**.

## 2. Goals / Non-goals

### Goals (this slice)
- **G1.** A real `paigasus-gateway` axum service exposing `POST /v1/chat/completions`
  (OpenAI-wire-compatible, **SSE streaming + non-streaming**) that proxies to the **OpenAI**
  upstream (`reqwest`), plus a health endpoint and structured JSON request logs.
- **G2.** Every request is **authenticated via IAM**: the caller's bearer token is an IAM-issued
  API key; the gateway calls `IntrospectApiKey` → rejects invalid/inactive/expired (`401`), else
  resolves the service-account principal + the key's scope.
- **G3.** Every request is **authorized via IAM**: `IsAuthorized(SA_prn, "InvokeModel",
  key.scope_prn)` → deny ⇒ `403` (IAM records the denial in its audit log, Slice A). The gateway
  holds the **real OpenAI credential** and swaps it in for the upstream call; the caller never
  sees it.
- **G4 (IAM-side):** add a **project-scoped `InvokeModel`** Cedar action to IAM, and surface the
  key's **`scope_prn`** on `IntrospectApiKeyResponse` so the gateway can authorize against the
  tenant.

### Non-goals (deferred to gateway M1–M5 / follow-ups)
- Multi-provider routing/fallback/load-balancing; Anthropic ingress; the canonical model +
  `contracts/`/gRPC surface; rate limiting; cost tracking/budgets; caching; guardrail impls
  (M0 may scaffold the no-op hook only if cheap, else defer); admin UI; other provider endpoints
  (`/v1/embeddings`, `/v1/models`, legacy `/v1/completions`) — **M0 is `/v1/chat/completions`
  only**.
- Enforcing the key's `scope_actions` model-allowlist (stored-but-unenforced in IAM v1) — later
  milestone.

## 3. Architecture

Hexagonal, in the existing `paigasus-gateway` service crate (a `paigasus-gateway-core` lib is
extracted when M1's routing/canonical-model needs pure testable logic — YAGNI for M0):

```
rs/crates/services/paigasus-gateway/
  src/
    main.rs                      # compose: config, IAM clients, upstream client, axum server, shutdown
    config.rs                    # [gateway] config + validate (+ gateway.toml.example)
    domain/                      # CallerContext { principal_prn, scope_prn, key_id }
    application/
      proxy_chat.rs              # the ChatCompletions use-case (authz already done by middleware)
    adapters/
      http/
        mod.rs                   # axum router: /healthz, /v1/chat/completions; auth middleware layer
        auth.rs                  # tower middleware: bearer → IAM authn+authz → CallerContext extension
        chat.rs                  # POST /v1/chat/completions handler (stream + non-stream)
        error.rs                 # OpenAI-shaped error envelope + status mapping
        dto.rs                   # OpenAI request/response structs (serde) — enough for model/stream/logging
      iam/
        client.rs                # tonic clients: AuthnServiceClient + AuthorizationServiceClient
      openai/
        client.rs                # reqwest client to the OpenAI upstream (non-stream + stream passthrough)
```

**Request flow:** `POST /v1/chat/completions` → **auth middleware** (bearer → `IntrospectApiKey`
→ active? → `IsAuthorized(SA, InvokeModel, scope_prn)`) → chat handler parses the OpenAI request
(for `model`/`stream` + logging) → **OpenAI egress** (`reqwest`, real OpenAI key) → non-stream:
return JSON; stream: pipe the upstream SSE body to the axum response with no buffering → structured
log (model, latency, status, caller — never prompt/PII).

**Tech:** `axum` + `tower` (matches IAM's stack), `reqwest` (egress, streaming), `tonic`
(IAM gRPC clients via `paigasus-proto`), `tokio`, `paigasus-logging`/`tracing`. New deps:
`reqwest` (with `stream`/`rustls-tls`), `tonic`/`prost` (already in the tree via `paigasus-proto`),
`axum`/`tower`/`tower-http`, `serde`/`serde_json`, `futures`/`tokio-stream`, `secrecy` for the
provider key. New workspace deps → `deny.toml`/`machete` review.

## 4. IAM-backed auth/authz (the SMA-446 core)

### 4.1 Transport — gRPC client to IAM
The gateway is a **tonic client** of IAM's `AuthnService`/`AuthorizationService` (generated in
`paigasus-proto`). Config: `iam.grpc_addr`. Per request = 2 IAM calls (`IntrospectApiKey` +
`IsAuthorized`); both are Redis-cached inside IAM (api-key validation cache + decision cache), so
the steady-state cost is a cache hit. (A combined introspect-and-authorize RPC is a possible future
optimization; out of scope for M0.)

### 4.2 Authn
`Authorization: Bearer <iam-key>` → `IntrospectApiKey{token}` →
- transport/`Unauthenticated`/not-found/inactive/expired ⇒ `401` (OpenAI-shaped error envelope);
- else read `principal_prn`, `status`, `key_id`, `expires_at`, **`scope_prn`** (new, §4.4). Reject
  if `status != active`. Build `CallerContext`.

### 4.3 Authz
`IsAuthorized{ principal_prn: SA, action: "InvokeModel", resource_prn: scope_prn }` →
- `allowed == false` ⇒ `403` (IAM already audited the denial with its determining policy);
- transport error ⇒ **fail-closed `403`** (a gateway that can't reach IAM must not grant access —
  distinct from IAM's own internal fail-open decision-cache posture, which is IAM's concern);
- `allowed == true` ⇒ proceed. `CallerContext` is attached as a request extension for the handler
  + logs.

### 4.4 IAM changes (bundled in this slice)
1. **`Action::InvokeModel`** — add to `paigasus-iam-core` `authz/action.rs` (enum + `ALL` +
   `as_wire()=="InvokeModel"` + `is_write` classification + the exhaustiveness test/`len`), and to
   the embedded Cedar `SCHEMA_SRC` (`authz/schema.rs`) in the shared action block (resource applies
   to `Organization/Team/Project` — the tenant scopes a key can have). **Archived-project safety:**
   classify `InvokeModel` so an **archived** project denies it — either mark it `is_write`
   (the `forbid-archived-writes` policy then blocks it on archived nodes) or add a dedicated
   forbid rule. *(Decision to confirm in the plan: `is_write=true` is the simplest reuse; a model
   call arguably isn't a tenancy "write", so a dedicated `forbid InvokeModel on archived` rule may
   read cleaner — see §9 D5.)*
2. **Grant it in the starter roles** — add `InvokeModel` to the `org_member`/`team_member`/
   `project_member` (and the corresponding admin) action allowlists in `authz/roles.rs`, so a
   service account holding any membership role at/above its key's scope may invoke. (A dedicated
   `gateway_user` role is a possible later refinement.)
3. **`scope_prn` on `IntrospectApiKeyResponse`** — add `string scope_prn = 7;` to `iam.proto`,
   `buf format -w` + regenerate rs/py/ts bindings, and populate it in IAM's `IntrospectApiKey`
   handler from the key's stored `scope_prn` (the `ApiKey.scope_prn` already persisted in M4).

## 5. OpenAI passthrough + streaming

- The chat handler deserializes the body into an OpenAI-shaped `ChatCompletionRequest` (fields
  needed: `model`, `stream`, `messages`, plus `#[serde(flatten)] extra` to preserve unknown
  provider fields losslessly — the passthrough escape hatch), for `model`/`stream`/logging, then
  re-serializes to the upstream. (M0 could forward raw bytes, but a typed parse buys `model`/
  `stream` + validation cheaply.)
- **Non-streaming** (`stream != true`): `reqwest` POST to `<upstream.base_url>/v1/chat/completions`
  with `Authorization: Bearer <real-openai-key>`; return the upstream status + JSON body verbatim.
- **Streaming** (`stream == true`): request the upstream as a byte stream; return an axum
  `Body::from_stream` piping the upstream SSE chunks **unbuffered** (first-token passthrough, PRD
  NFR). Map upstream/transport errors to an OpenAI-shaped error (and, mid-stream, a terminal SSE
  error event where practical).
- Upstream errors (4xx/5xx from OpenAI) are surfaced with their status; a gateway-side failure
  (timeout, connect) → `502`/`504` OpenAI-shaped.

## 6. Config & secrets

`[gateway]` (in a new `config.rs` + `gateway.toml.example`), validated at boot:
- `http_addr`, `request_timeout_secs`.
- `iam.grpc_addr` (+ TLS later).
- `upstream.openai.base_url` (default `https://api.openai.com`), `upstream.openai.api_key`
  (a **secret** — `secrecy::SecretString`, from env; never logged; constant-time not needed as it's
  outbound, but never serialized into logs/errors).
- `log_level`.

## 7. Testing

- **Unit:** config validation; the OpenAI DTO round-trips + `extra` passthrough; the error-envelope
  mapping (transport→401/403/502); the auth-middleware decision table over **fake IAM clients**
  (invalid key→401; inactive→401; authz deny→403; IAM transport error→fail-closed 403; allow→pass +
  `CallerContext` populated).
- **Integration (the gateway's own tests):** boot the router with a **mock IAM gRPC server**
  (an in-process tonic server returning canned Introspect/IsAuthorized) + a **mock OpenAI upstream**
  (an in-process axum server) → drive `POST /v1/chat/completions` (a) unauthorized bearer → 401,
  (b) authz-denied SA → 403, (c) allowed → 200 with the mock upstream's body, (d) `stream:true` →
  the SSE chunks arrive in order, unbuffered. (Follows the IAM `tests/support` mock-server idiom.)
- **IAM-side:** `InvokeModel` in the action catalog + schema-validates + a starter-policy table case
  (a `project_member` SA is allowed `InvokeModel` on its project; denied on another; denied on an
  archived project); `IntrospectApiKey` returns the persisted `scope_prn` (a real-PG test).
- A true end-to-end against the real OpenAI API is **manual/out-of-band** (needs a live key); CI
  uses the mock upstream.

## 8. CI / gate considerations

- **New workspace deps** (`reqwest`, `secrecy`, `tokio-stream`, `futures`, `axum`/`tower-http` if
  not already present) → likely `rs/deny.toml` `[licenses] exceptions` / a dev advisory ignore, and
  a temporary `machete` `ignored` for any dep consumed a commit later than introduced. Run the full
  gate list.
- **Proto change** (`scope_prn`) → `buf format -w` + regen rs/py/ts bindings; additive → `:breaking`
  clean (verify). The embedded `FILE_DESCRIPTOR_SET` shifts → regen.
- **Affected-graph:** `paigasus-gateway` already `dependsOn` `paigasus-proto` + `paigasus-kernel`;
  it does **not** dependOn `paigasus-iam` (it talks over gRPC), so the `kernel->bindings`
  strict-equality set is unaffected. Confirm `:affected-smoke`.
- `wasm-getrandom-free`: the gateway is a native service (not wasm-bound) — unaffected; keep any
  shared/kernel crates untouched.
- Windows-reserved-name check: no `CON`/`PRN`/`AUX`/etc. source basenames.

## 9. Decision log

| # | Decision | Rationale |
|---|---|---|
| D1 | Sub-project #2 = gateway **M0 walking skeleton + IAM auth** (not the full gateway) | Can't integrate IAM into a nonexistent gateway; M0 is the buildable first slice + the SMA-446 "consumes IAM" proof |
| D2 | **REST-only Rust structs**; no canonical model / `contracts/` / gRPC ingress yet | PRD v1 non-goal; the canonical model is only needed at M1/M2 (multi-provider/dual-ingress) |
| D3 | Authn+authz via **IAM over gRPC** (`IntrospectApiKey` + `IsAuthorized`); no self-contained keys | Resolves the PRD IAM-reuse open question; IAM caches both calls |
| D4 | **`InvokeModel` Cedar action, project-scoped**; authorize against the key's `scope_prn` | Tenant-isolated model access; sets up per-project governance (Sven's choice) |
| D5 | Surface **`scope_prn` on `IntrospectApiKeyResponse`** (proto add) | The gateway needs the tenant resource to authorize against; the key already stores it |
| D6 | Gateway **fail-closed** when it can't reach IAM for authz | A gateway that can't authorize must not grant model access |
| D7 | M0 in the existing `paigasus-gateway` service crate; extract `paigasus-gateway-core` at M1 | YAGNI — M0's pure logic is thin |

## 10. Risks / open items for the plan

- **`InvokeModel` archived-project classification** (§4.4/D5) — `is_write=true` reuse vs. a
  dedicated forbid rule; pick one in the plan (a starter-policy test pins the behavior).
- **Which roles grant `InvokeModel`** — member+admin roles at org/team/project, or a dedicated
  `gateway_user` role. Plan picks the minimal set; a bootstrap SA must be able to invoke in the
  integration test.
- **Streaming error semantics** — mapping a mid-stream upstream failure to a client-visible SSE
  error without breaking the passthrough; the plan specifies the terminal-event behavior.
- **Secret handling** — the OpenAI key via `secrecy`; ensure it never reaches logs/error bodies
  (a test asserts it).
- **Proto/deny/machete churn** — the `scope_prn` regen + new gateway deps need the full gate run
  and likely deny/machete waivers.

## 11. Follow-ups (gateway M1–M5 + governance)

- M1 multi-provider routing/fallback + the `ProviderAdapter` trait + extract `paigasus-gateway-core`
  + the canonical-model ADR + `contracts/`.
- M2 Anthropic ingress; M3 rate limiting (per-key RPM/TPM) + enforcing the key `scope_actions`
  model-allowlist; M4 cost/budgets; M5 caching + gateway dashboards/RUNBOOK.
- IAM: a dedicated `gateway_user` role; a combined introspect-and-authorize RPC to halve per-request
  IAM round-trips.
