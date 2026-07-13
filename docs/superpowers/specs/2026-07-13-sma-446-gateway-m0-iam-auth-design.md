# SMA-446 (#2) — AI Gateway M0 walking skeleton + IAM-backed auth

**Status:** Design (brainstormed + adversarially challenged, rev 2) · **Date:** 2026-07-13 ·
**Linear:** SMA-446 (Part of; the "gateway consumes IAM" AC) · **Crates:** `paigasus-gateway`
(service) + IAM changes in `paigasus-iam` / `paigasus-iam-core` / `contracts` / `paigasus-proto`.

> Rev 2 folds in the Stage-2 challenge (self-query authz BLOCKER, `scope_prn` cache plumbing,
> spend guardrails, 503-on-IAM-outage, dedicated `gateway_user` role, archived-deny, gateway↔IAM
> TLS, streaming/egress hardening). See §12 changelog.

---

## 1. Context

The [AI Gateway PRD](https://app.notion.com/p/385830e8fbaa809dbe29ff5deb10cc17) describes a full
Rust LLM gateway delivered across its own milestones **M0–M5**. Its §12 open question — *"reuse IAM
vs self-contained keys?"* — is the one **SMA-446 resolves: the gateway consumes IAM**. The
`paigasus-gateway` crate is a `fn main(){}` stub; you can't integrate IAM into a nonexistent
gateway, so this sub-project builds the PRD's **M0 walking skeleton with IAM as the auth backbone
from day one**. Merged IAM (M1–M5) exposes `AuthnService.IntrospectApiKey` and
`AuthorizationService.IsAuthorized`.

**Scope (decided with Sven):** M0 + IAM auth; REST-only Rust structs (canonical model + `contracts/`
deferred to gateway M1/M2); authz = a new **`InvokeModel`** Cedar action authorized against the
key's scope node.

## 2. Goals / Non-goals

### Goals
- **G1.** A real `paigasus-gateway` axum service: `POST /v1/chat/completions` (OpenAI-wire, SSE
  **stream** + non-stream) proxied to the **OpenAI** upstream (`reqwest`); `/healthz` (liveness) +
  `/readyz` (IAM+upstream reachability); structured JSON logs (never prompt/PII).
- **G2. Authn via IAM:** the caller's bearer is an IAM-issued API key; `IntrospectApiKey` →
  invalid/inactive/expired ⇒ `401`; else resolve the SA principal + the key's **scope**.
- **G3. Authz via IAM (self-query):** `IsAuthorized("InvokeModel", scope)` performed as a
  **self-query** (§4.3) → deny ⇒ `403` (IAM audits it); IAM unreachable ⇒ `503`.
- **G4. IAM-side:** add an **`InvokeModel`** Cedar action (archived-deny) + a **`gateway_user`**
  starter role, and surface the key's **`scope_prn`** on the introspect response (with cache
  plumbing).
- **G5. Guardrails on a real-credential proxy:** request-body size cap; split
  connect/first-byte/overall-stream timeouts; the gateway holds the OpenAI credential and never
  leaks it; the caller's inbound `Authorization` never reaches OpenAI.

### Non-goals (gateway M1–M5 / follow-ups)
- Multi-provider routing/fallback; Anthropic ingress; the canonical model + `contracts/`/gRPC
  ingress; **rate limiting** + **cost/budgets** (M3/M4 — see the M0 guardrail constraint, §5);
  caching; guardrail impls; admin UI; endpoints other than `/v1/chat/completions` (see `/v1/models`
  risk, §10).
- Enforcing the key's `scope_actions` model-allowlist (unenforced in IAM v1).

## 3. Architecture

Hexagonal, in the existing `paigasus-gateway` service crate (extract a `paigasus-gateway-core` lib
at M1 — YAGNI for M0):

```
rs/crates/services/paigasus-gateway/src/
  main.rs                # compose config, IAM gRPC clients, upstream client, axum, graceful shutdown
  config.rs              # [gateway] config + validate (+ gateway.toml.example)
  domain.rs              # CallerContext { principal_prn, scope_prn, key_id }
  adapters/http/
    mod.rs               # router: /healthz, /readyz, /v1/chat/completions; body-limit + auth layers
    auth.rs              # tower middleware: bearer → IAM authn + self-query authz → CallerContext ext
    chat.rs              # POST /v1/chat/completions (stream + non-stream), egress header hygiene
    error.rs             # OpenAI error envelope {error:{message,type,param,code}} + status mapping
    dto.rs               # OpenAI request struct (model, stream, messages, #[serde(flatten)] extra)
  adapters/iam/client.rs # tonic AuthnServiceClient + AuthorizationServiceClient (+ per-call bearer)
  adapters/openai/client.rs # reqwest upstream (non-stream + unbuffered stream + cancel-on-drop)
```

**Request flow:** body-limit layer → **auth middleware** (§4) → chat handler (parse OpenAI request
for `model`/`stream`/log) → **strip caller headers**, inject the real OpenAI key → OpenAI egress →
non-stream: return status+JSON; stream: pipe upstream SSE unbuffered, cancel upstream on client
drop → structured log.

**Tech:** `axum`+`tower`+`tower-http` (body limit), `reqwest` (egress, `stream`, `rustls-tls`),
`tonic`/`prost` (IAM clients via `paigasus-proto`; **TLS**, §6), `tokio`/`tokio-stream`/`futures`,
`secrecy` (OpenAI key), `paigasus-logging`. New workspace deps → deny/machete review (§8).

## 4. IAM-backed auth/authz (the SMA-446 core)

### 4.1 Transport
The gateway is a tonic client of IAM's `AuthnService`/`AuthorizationService` (generated in
`paigasus-proto`). Config `iam.grpc_addr` **must be TLS/mTLS** unless gateway+IAM are provably
loopback/co-located (§6, D8) — the calls carry raw API keys. Two IAM calls per request; both are
Redis-cached inside IAM. **The gateway's availability is coupled to IAM's** (every request needs
IAM); mitigation = IAM HA / co-location; a combined introspect-and-authorize RPC is a strong
follow-up (§11) that also halves round-trips.

### 4.2 Authn — `IntrospectApiKey`
`IntrospectApiKey` is **exempt from bearer enforcement** (the key is in the request body). Call
`IntrospectApiKey{ token: <caller bearer> }`:
- transport/unreachable ⇒ **`503`** (retryable — NOT 401);
- `Unauthenticated`/not-found/inactive/expired/`status != active` ⇒ **`401`**;
- else read `principal_prn` (SA), `status`, `key_id`, `expires_at`, **`scope_prn`** (§4.4). A
  missing/empty `scope_prn` ⇒ `500` with a distinct diagnostic (a plumbing bug, not a client
  error), never a silent deny.

### 4.3 Authz — `IsAuthorized` as a SELF-QUERY (the BLOCKER fix)
`AuthorizationService.IsAuthorized` is **bearer-enforced** and `decide_gated` (application/authorize.rs)
rejects a *cross-principal* query unless the caller holds `ListRoleGrants` at the resource. So the
gateway must query as the **same principal it's asking about**: present the **caller's own IAM key**
as the `authorization` gRPC metadata AND set `principal_prn = the caller's SA` (from §4.2). A
**self-query** (`req.principal == actor`) passes `decide_gated`'s exposure gate and evaluates the
policy directly. No gateway service credential exists or is needed.

`IsAuthorized{ principal_prn: SA, action: "InvokeModel", resource_prn: scope_prn }` (bearer = caller
key):
- `allowed == false` ⇒ **`403`** (IAM already audited the denial with its determining policy,
  attributed to the caller SA — Slice A);
- transport/unreachable/non-deny error ⇒ **`503`** (retryable); a genuine `PermissionDenied` from a
  *non-self* query would be a bug (we always self-query) — treat as `500`;
- `allowed == true` ⇒ attach `CallerContext` and proceed.

*(An integration test asserts the self-query path: a cross-principal query is NEVER issued.)*

### 4.4 IAM changes (bundled)
1. **`Action::InvokeModel`** — add to `authz/action.rs` (enum + `ALL` (append after `ListAuditLog`)
   + `as_wire()=="InvokeModel"` + **`is_write`: classify TRUE** so `forbid-archived-writes`
   (derived from `ALL.filter(is_write && !is_restore)`) denies it on an **archived** project +
   the exhaustiveness match + bump `ALL.len()` 35→36 & its comment), and to the shared action
   block in `SCHEMA_SRC` (`authz/schema.rs`) — it inherits the block's `resource: [Root,
   Organization, Team, Project]` (harmless: keys never scope to Root). *(is_write=true is the
   minimal reuse that achieves archived-deny; a model call isn't literally a tenancy write, but
   this avoids a bespoke forbid rule — D5.)*
2. **New `gateway_user` starter role** (`authz/roles.rs`) grantable at `Organization`/`Team`/
   `Project`, whose allowlist is `[InvokeModel]` (and nothing else). **Do NOT** add `InvokeModel`
   to the `*_member`/`*_admin` allowlists — those stay their documented read/manage sets. Add the
   role to `system_roles()` + a starter-policy test (a `gateway_user` SA is allowed `InvokeModel`
   on its scope, denied on another scope, denied on an archived project).
3. **`scope_prn` on the introspect response — with cache plumbing** (bigger than a handler tweak):
   - proto: `string scope_prn = 7;` on `IntrospectApiKeyResponse` (additive; `buf format -w` +
     regen rs/py/ts + FILE_DESCRIPTOR_SET).
   - Thread the key's scope through the authenticated-principal path: add the scope to
     `Credential::ApiKey` (or `PrincipalContext`) in `paigasus-iam-core`, AND to the Redis-cached
     `CachedValidation` (`adapters/api_keys/cache.rs`) so a **cache hit** returns `scope_prn`
     WITHOUT a DB read (the p99 story). Populate from the stored `ApiKey.scope` in
     `resolve_uncached`.
   - map it in BOTH the gRPC `to_introspect_api_key_response` (`grpc/convert.rs`) and the HTTP
     `IntrospectApiKeyResponseDto` (`http/dto.rs`) for parity.
   - Tests: a **cache-hit** introspect returns the right `scope_prn` with no DB read; the HTTP +
     gRPC responses agree.

## 5. OpenAI passthrough, streaming & M0 guardrails

- **Parse** the body into an OpenAI `ChatCompletionRequest` (`model`, `stream`, `messages`,
  `#[serde(flatten)] extra` for lossless passthrough) for `model`/`stream`/logging, then
  re-serialize upstream.
- **Egress hygiene:** build a fresh upstream request; set `Authorization: Bearer <real-openai-key>`;
  **strip the caller's inbound `Authorization`** and any caller `OpenAI-*`/cookie headers (explicit
  egress allowlist). A test asserts the caller's bearer never reaches OpenAI and the real key never
  reaches logs/error bodies.
- **Non-stream:** `reqwest` POST → return upstream status + JSON verbatim.
- **Stream (`stream==true`):** stream the upstream bytes into an axum `Body::from_stream`,
  **unbuffered**. **Cancel-on-drop:** if the client disconnects, dropping the response cancels the
  upstream `reqwest` request (no leaked connection / wasted tokens). **Mid-stream upstream failure:**
  the HTTP status is already `200` + `data:` chunks sent, so emit a terminal `data: {"error":{…}}\n\n`
  event and close (never attempt a status change). Tests cover happy-stream, mid-stream-error, and
  client-abort.
- **Error envelope:** exact OpenAI shape `{"error":{"message","type","param","code"}}` for all
  gateway-originated errors (401/403/500/502/503/504), so SDK error handling works.
- **M0 guardrails (real-credential proxy):** `tower_http`/`DefaultBodyLimit` **max request body**;
  **split timeouts** — connect + first-byte (short) vs overall-stream (long, so a legit long stream
  isn't killed by one `request_timeout`); optional `max_tokens` clamp. **M0 deployment constraint
  (D6):** rate limiting + cost budgets are M3/M4, so M0 is for **internal / non-production** use or
  behind a **hard OpenAI account-level spend cap** — stated in the RUNBOOK/config docs; a single
  over-granted key otherwise means unbounded spend.

## 6. Config & secrets

`[gateway]` (new `config.rs` + `gateway.toml.example`), validated at boot:
- `http_addr`; `connect_timeout_secs`, `first_byte_timeout_secs`, `stream_idle_timeout_secs`;
  `max_request_bytes`.
- `iam.grpc_addr` + **`iam.tls`** (CA/cert/key or a "loopback-insecure" explicit opt-out — D8).
- `upstream.openai.base_url` (default `https://api.openai.com`), `upstream.openai.api_key`
  (`secrecy::SecretString` from env; never logged/serialized).
- `log_level`.

## 7. Testing

- **Unit:** config validation; OpenAI DTO round-trip + `extra` passthrough; error-envelope + status
  mapping (IAM-unreachable→503, bad-key→401, deny→403, missing-scope→500, upstream-5xx→502); auth
  middleware over **fake IAM clients** (invalid→401; inactive→401; deny→403; **self-query issued,
  never cross-principal**; IAM transport→503; allow→pass); egress strips caller Authorization.
- **Integration (gateway):** boot the router with a **mock IAM gRPC server** (canned
  Introspect/IsAuthorized, asserting the IsAuthorized bearer == caller key and principal == SA) +
  a **mock OpenAI upstream** (in-process axum) → `POST /v1/chat/completions`: (a) bad bearer→401,
  (b) denied→403, (c) allowed→200 body, (d) `stream:true`→ordered unbuffered SSE, (e) mid-stream
  upstream error→terminal SSE error event, (f) client abort→upstream cancelled, (g) oversized body→
  413, (h) IAM down→503.
- **IAM-side (real PG):** `InvokeModel` in the catalog + schema-validates; starter-policy table
  (`gateway_user` allowed on its scope, denied elsewhere, **denied on archived**); `IntrospectApiKey`
  returns the persisted `scope_prn` on a **cache hit** (no DB read) with HTTP/gRPC parity.
- Real-OpenAI end-to-end is manual/out-of-band (live key); CI uses the mock upstream.

## 8. CI / gates
- **New workspace deps** (`reqwest`, `secrecy`, `tokio-stream`, `futures`, `tower-http`) → likely
  `rs/deny.toml` `[licenses] exceptions` / dev advisory ignores + temporary `machete` `ignored` for
  a dep consumed a commit later than introduced. Run the full gate list.
- **Proto** (`scope_prn=7`, additive over current max 6) → `buf format -w` + regen rs/py/ts +
  FILE_DESCRIPTOR_SET; `:breaking` should be additive-clean (verify).
- **Affected-graph:** `paigasus-gateway` `dependsOn` `paigasus-proto` + `paigasus-kernel`, NOT
  `paigasus-iam` (gRPC over the wire) → `kernel->bindings` strict set unaffected; confirm
  `:affected-smoke`. `wasm-getrandom-free` unaffected (native service; keep kernel/shared crates
  untouched). No Windows-reserved source basenames.

## 9. Decision log

| # | Decision | Rationale |
|---|---|---|
| D1 | #2 = gateway **M0 skeleton + IAM auth** | Can't integrate IAM into a nonexistent gateway; M0 is the buildable "consumes IAM" proof |
| D2 | **REST-only Rust structs**; canonical model/`contracts/` deferred | PRD v1 non-goal; canonical model needed only at M1/M2 |
| D3 | Authn+authz via **IAM over gRPC**; no self-contained keys | Resolves the PRD IAM-reuse question |
| D4 | **`InvokeModel`** action, authorized against the key's **scope node** | Tenant-isolated model access (Sven) |
| D5 | `InvokeModel` **`is_write=true`** ⇒ archived-project denies it | Minimal reuse of `forbid-archived-writes`; closes the archived-spend hole |
| **D9** | **Self-query authz:** gateway presents the caller's key as the `IsAuthorized` bearer + queries the caller's own SA | `decide_gated` blocks cross-principal queries; a self-query needs no gateway credential (challenge BLOCKER) |
| **D10** | **`gateway_user`** dedicated role (not diluting `*_member`) | Member roles are documented read-only; a spend-capable action doesn't belong there |
| **D11** | `scope_prn` threaded through `Credential`/`CachedValidation` (cache-hit, no DB read) | The hot-path cache doesn't carry scope today; a per-request DB read would defeat it |
| **D12** | IAM-unreachable ⇒ **`503`** (retryable), distinct from `401`/`403` | OpenAI SDKs treat 401/403 as fatal; 503 is retryable; keeps deny-by-default |
| **D6** | M0 is **internal/non-prod or hard-capped**; body-size + split timeouts now; rate-limit/budget = M3/M4 | A real-credential proxy with no throttle is an unbounded-spend hazard |
| **D8** | **TLS/mTLS** on `iam.grpc_addr` (unless loopback) — not deferred | The link carries raw API keys |

## 10. Risks / open items for the plan
- **`scope_prn` cache plumbing** (D11) is the biggest IAM change — get the `CachedValidation`
  serialization + eviction right, and prove cache-hit returns scope without a DB read.
- **`gateway_user` grantability** — confirm the anti-escalation/`GrantScope` rules let a
  `gateway_user` grant be issued at org/team/project (it's a normal role grant); the integration
  test seeds one.
- **`/v1/models` probe** — some OpenAI-compatible SDKs `GET /v1/models` on init and will error on a
  404. Confirm the target M0 SDK/client, or add a minimal `/v1/models` passthrough (small; decide in
  the plan).
- **Streaming cancel-on-drop** correctness — verify axum drops the body future on client
  disconnect and that propagates to the `reqwest` stream (a test with a client that aborts).
- **Deny/machete/proto churn** — new deps + the regen need the full gate run + waivers.

## 11. Follow-ups (gateway M1–M5 + IAM)
- Gateway M1 multi-provider + `ProviderAdapter` + extract `paigasus-gateway-core` + canonical-model
  ADR + `contracts/`; M2 Anthropic ingress; M3 rate-limit + enforce the key `scope_actions`
  allowlist; M4 cost/budgets; M5 caching + gateway dashboards/RUNBOOK.
- IAM: a **combined introspect-and-authorize RPC** (halves per-request round-trips AND avoids the
  two-call self-query dance); TLS everywhere.

## 12. Changelog — Stage-2 challenge fold-in (rev 2)
All challenge findings folded (none rejected): **self-query authz** (BLOCKER → D9/§4.3);
**`scope_prn` cache plumbing** properly scoped (MAJOR → D11/§4.4.3); **M0 spend guardrails +
deployment constraint** (MAJOR → D6/§5); **IAM-outage → 503** (MAJOR → D12/§4); **dedicated
`gateway_user` role** instead of diluting members (MAJOR → D10/§4.4.2); **archived-deny resolved to
`is_write=true`** in-spec (MAJOR → D5/§4.4.1); **gateway↔IAM TLS** not deferred (MAJOR → D8/§6);
plus MINORs — **streaming mid-error/client-abort/cancel-on-drop** (§5), **egress header stripping**
(§5), **exact OpenAI error envelope** (§5), **`/readyz` checks IAM+upstream** (§3/G1),
**`/v1/models` probe** flagged (§10), **empty-`scope_prn` distinct diagnostic** (§4.2), **action
`len` 35→36 + exhaustiveness sites** enumerated (§4.4.1).
