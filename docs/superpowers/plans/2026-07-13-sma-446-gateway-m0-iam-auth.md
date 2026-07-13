# SMA-446 (#2) — AI Gateway M0 + IAM auth — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use `- [ ]`.

**Goal:** Build the AI Gateway M0 walking skeleton — a `paigasus-gateway` axum service that proxies OpenAI `POST /v1/chat/completions` (+SSE) to OpenAI, authenticating every caller via IAM `IntrospectApiKey` and authorizing via a self-query `IsAuthorized("InvokeModel", key.scope)`. Adds the IAM-side pieces it needs.

**Architecture:** Hexagonal `paigasus-gateway` service (axum ingress, reqwest egress, tonic IAM clients, tower auth middleware). IAM gains an `InvokeModel` Cedar action + a `gateway_user` role + `scope_prn` on the introspect response (threaded through the credential + Redis cache). Spec: `docs/superpowers/specs/2026-07-13-sma-446-gateway-m0-iam-auth-design.md` (D1–D12).

**Tech:** Rust 2024/1.95, axum+tower+tower-http, reqwest (rustls, stream), tonic/prost via `paigasus-proto`, tokio/tokio-stream/futures, secrecy, serde. Base: merged Slice A+B (`4adc639`).

## Global Constraints
- SPDX + blank line + `//!` doc on every new file. Edition/rust-version workspace-inherited.
- **Self-query authz (D9):** the gateway calls `IsAuthorized` with the caller's IAM key as the gRPC `authorization` metadata AND `principal_prn == the caller's own SA` — NEVER a cross-principal query (`decide_gated` would 403 it).
- **Status codes (D12):** IAM unreachable → **503**; bad/inactive/expired key → **401**; authz deny → **403**; missing `scope_prn` → **500**; upstream 5xx/timeout → **502/504**. All errors use the OpenAI envelope `{"error":{message,type,param,code}}`.
- **Secrets:** OpenAI key via `secrecy::SecretString`, never logged/serialized; strip the caller's inbound `Authorization` before egress; a test asserts neither leaks.
- **`InvokeModel` is_write=true** (archived projects deny it). `gateway_user` role gets ONLY `[InvokeModel]`; do NOT touch `*_member`/`*_admin` allowlists.
- **TLS (D8):** `iam.grpc_addr` supports TLS/mTLS (loopback-insecure opt-out only).
- Editing `iam.proto` → `buf format -w` + regen rs/py/ts bindings. Before pushing: `moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke :parity-corpus-drift :wasm-getrandom-free --base origin/main --include-relations` (prefix `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`). New deps may need `rs/deny.toml` waivers + a temporary `machete` `ignored`.
- Commits: conventional, scope required, subject lowercase, ≤100, `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`. NEVER `--no-verify`. Docker available (real-PG IAM tests).

## Sequencing
IAM-side first (G1–G3), because the gateway consumes `InvokeModel` + `scope_prn`; then the gateway (G4–G10). G1/G2 are independent of G3.

---

## Task G1 — IAM: `InvokeModel` Cedar action + `gateway_user` role (`paigasus-iam-core`)
**Files:** modify `src/authz/action.rs`, `src/authz/schema.rs`, `src/authz/roles.rs`.
**Steps (TDD):** update the `all_covers_every_variant` count 35→36 + add `Action::InvokeModel` to the enum/`ALL`/`as_wire`("InvokeModel")/`is_write` **true** arm/the exhaustiveness match; add `InvokeModel` to `SCHEMA_SRC`'s shared action block (`authz/schema.rs`, mirror how prior actions are listed). In `roles.rs`: add a `GATEWAY_USER_ACTIONS: &[Action] = &[Action::InvokeModel]`, a `gateway_user` entry in `system_roles()` (scope_kinds `[Organization, Team, Project]`, `template_source` uses the allowlist), and its template. Add starter-policy table cases: a `gateway_user` SA is **allowed** `InvokeModel` on its scope, **denied** on a different scope, **denied** on an archived project (the `forbid-archived-writes` policy fires because `is_write=true`). Keep `every_starter_policy_passes_schema_validation` green.
Verify `cargo test -p paigasus-iam-core authz::` + clippy + fmt. Commit `feat(rs): add InvokeModel action + gateway_user role to iam (SMA-446)`.

## Task G2 — IAM: `scope_prn` on the introspect response, through the cache (`paigasus-iam` + core + proto)
**Files:** `contracts/proto/paigasus/iam/v1/iam.proto`; regen `paigasus-proto`; `paigasus-iam-core` credential type; `src/adapters/api_keys/cache.rs` (`CachedValidation`); `src/application/authenticate_api_key.rs` (`introspect`/`resolve_uncached`/`resolve_cached`); `src/adapters/grpc/convert.rs`; `src/adapters/http/dto.rs`.
**Steps:**
1. proto: add `string scope_prn = 7;` to `IntrospectApiKeyResponse`; `buf format -w`; `moon run contracts:generate`; stage all regen.
2. core: add the key scope (a PRN string) to `Credential::ApiKey` (or the `PrincipalContext` the introspect response is built from) — read `authenticate_api_key.rs` + `grpc/convert.rs::to_introspect_api_key_response` to see the exact type threaded.
3. cache: add `scope_prn: String` to `CachedValidation` (serde — it's Redis-serialized); populate it in `resolve_uncached` from the stored `ApiKey.scope`; return it on the `resolve_cached` (cache-hit) path.
4. map `scope_prn` in BOTH `grpc/convert.rs` and `http/dto.rs::IntrospectApiKeyResponseDto`.
**Tests:** a **cache-hit** introspect returns the correct `scope_prn` with NO DB read (extend `tests/api_key_cache_redis.rs`/`tests/api_key_auth.rs`); HTTP + gRPC responses agree (extend `tests/grpc_authn.rs`/`http_authn.rs`). Verify `CI=1 cargo test -p paigasus-iam api_key` + `--test grpc_authn` + build/clippy/fmt. Commit `feat(rs): surface api-key scope_prn on introspect via the cache (SMA-446)`.

## Task G3 — Gateway: crate scaffold, config, health/readyz skeleton
**Files:** `rs/crates/services/paigasus-gateway/Cargo.toml` (deps), `moon.yml`, `src/main.rs`, `src/config.rs` (+ `gateway.toml.example`), `src/domain.rs`.
**Steps:** add deps (`axum`, `tower`, `tower-http` [limit], `reqwest` [rustls-tls, stream, json], `tonic`, `prost`, `paigasus-proto`, `paigasus-logging`, `tokio`[full], `tokio-stream`, `futures`, `secrecy`, `serde`/`serde_json`, `anyhow`, `thiserror`, `tracing`) — mirror `paigasus-iam`'s Cargo.toml style; add any `rs/deny.toml` waivers + a temporary `machete` `ignored` for deps consumed a later commit. `config.rs`: `GatewayConfig { http_addr, connect_timeout_secs, first_byte_timeout_secs, stream_idle_timeout_secs, max_request_bytes, iam: { grpc_addr, tls }, upstream: { openai: { base_url, api_key: SecretString } }, log_level }` + `load()` (figment/env, mirror `iam/config.rs`) + `validate()` (non-zero timeouts/limits). `domain.rs`: `CallerContext { principal_prn, scope_prn, key_id }`. `main.rs`: load config, init logging, build the axum router with `/healthz` (200) + `/readyz` (G8), serve with graceful shutdown (mirror `iam/main.rs`). A `gateway.toml.example`.
**Test:** config validation unit tests; a `/healthz` 200 test (axum oneshot). Verify build/clippy/fmt + `cargo test -p paigasus-gateway`. Commit `feat(rs): scaffold paigasus-gateway service + config (SMA-446)`.

## Task G4 — Gateway: IAM gRPC clients (authn + authz, per-call bearer, TLS)
**Files:** `src/adapters/iam/client.rs` (+ `mod.rs`).
**Steps:** an `IamClient` holding tonic `AuthnServiceClient` + `AuthorizationServiceClient` (from `paigasus_proto`), constructed from `iam.grpc_addr` (+ TLS via `tonic::transport::ClientTlsConfig` when configured). Methods: `introspect_api_key(token) -> Result<IntrospectApiKeyResponse, IamError>` (NO bearer — the RPC is exempt), and `is_authorized_self(caller_key, principal_prn, action, resource_prn) -> Result<bool, IamError>` that sets the caller's key as `authorization` metadata on the request (self-query, D9). `IamError` distinguishes transport/unreachable (→503) from a clean response. Read `paigasus-proto`'s generated client names + `iam/adapters/grpc` for the metadata pattern.
**Test:** unit test the metadata-setting + error classification with a fake channel where practical; the real behavior is covered by G5/G9's mock-IAM integration. Verify build/clippy/fmt. Commit `feat(rs): add gateway IAM gRPC clients (introspect + self-query authz) (SMA-446)`.

## Task G5 — Gateway: auth middleware (bearer → authn → self-query authz → CallerContext)
**Files:** `src/adapters/http/auth.rs`, `src/adapters/http/error.rs`.
**Steps:** a tower/axum middleware: extract `Authorization: Bearer <key>` (missing → 401); `iam.introspect_api_key(key)` — transport→503, invalid/inactive/expired/`status!=active`→401, missing `scope_prn`→500, else get `principal_prn`+`scope_prn`; `iam.is_authorized_self(key, principal_prn, "InvokeModel", scope_prn)` — transport→503, `false`→403, `true`→insert `CallerContext` as a request extension. `error.rs`: the OpenAI error envelope + an `into_response` mapping each case to its status + shape.
**Tests (fake IAM client):** the full decision table (missing bearer→401, invalid→401, inactive→401, deny→403, IAM-transport→503, missing-scope→500, allow→pass + context); **assert the authz call is a self-query** (principal == the introspected SA, bearer == the caller key), never cross-principal. Verify. Commit `feat(rs): add gateway IAM auth middleware (self-query authz) (SMA-446)`.

## Task G6 — Gateway: OpenAI request DTO + egress client (non-stream + streaming, cancel-on-drop, header hygiene)
**Files:** `src/adapters/http/dto.rs`, `src/adapters/openai/client.rs` (+ `mod.rs`).
**Steps:** `dto.rs`: `ChatCompletionRequest { model: String, #[serde(default)] stream: bool, messages: Vec<Value>, #[serde(flatten)] extra: Map<String,Value> }` (round-trips losslessly). `openai/client.rs`: an `OpenAiClient` (reqwest, base_url + `SecretString` key, split timeouts). `chat_completion(req_bytes, stream: bool)`: builds a fresh upstream POST to `<base>/v1/chat/completions` with `Authorization: Bearer <real key>` and a curated header set — **never** forwarding the caller's `Authorization`/cookies. Non-stream → return upstream status + body bytes. Stream → return an `impl Stream<Item=Result<Bytes,_>>` piping the upstream SSE **unbuffered**; ensure dropping the stream cancels the upstream request (reqwest cancels on drop). 
**Tests:** DTO round-trip + `extra` passthrough; against an in-process **mock OpenAI** axum server: non-stream returns the body; stream yields ordered chunks; a test asserting the caller's Authorization is absent upstream and the real key is present; the real key never appears in a debug/log render. Verify. Commit `feat(rs): add gateway OpenAI dto + streaming egress client (SMA-446)`.

## Task G7 — Gateway: `POST /v1/chat/completions` handler + body limit + wiring
**Files:** `src/adapters/http/chat.rs`, `src/adapters/http/mod.rs`, `src/main.rs`.
**Steps:** the handler reads `CallerContext` (from G5), parses the body into `ChatCompletionRequest` (for `model`/`stream`/log), calls `OpenAiClient::chat_completion`; non-stream → `Response` with upstream status+JSON; stream → `Body::from_stream(...)` with `text/event-stream`; **mid-stream upstream failure** → emit a terminal `data: {"error":{…}}\n\n` then close (status already 200). Register the route + a `tower_http` body-size limit (`max_request_bytes`) + the G5 auth layer in `mod.rs`; structured log per request (model, stream, status, latency, principal — never prompt). Wire the real `IamClient`/`OpenAiClient` into `AppState` in `main.rs`.
**Tests (mock IAM + mock OpenAI, integration `tests/chat_proxy.rs`):** allowed→200 body; denied→403; bad bearer→401; `stream:true`→ordered unbuffered SSE; mid-stream error→terminal SSE error; oversized body→413; IAM down→503. Verify. Commit `feat(rs): add gateway chat-completions proxy handler + streaming (SMA-446)`.

## Task G8 — Gateway: `/readyz` (IAM + upstream reachability) + client-abort cancel test
**Files:** `src/adapters/http/mod.rs` (readyz), `tests/chat_proxy.rs` (abort test).
**Steps:** `/readyz` checks IAM gRPC reachability (a cheap call / channel-ready) and upstream base-url reachability (a HEAD/`/` probe or a config-presence check) → 200 ready / 503 not-ready. Add a **client-abort** integration test: start a streaming response, drop the client mid-stream, assert the upstream request is cancelled (the mock upstream observes the disconnect). Verify. Commit `feat(rs): add gateway readiness probe + stream cancel-on-abort (SMA-446)`.

## Task G9 — Full gate run + finalize
**Steps:** run the full `moon ci …` gate list (Global Constraints); resolve any `:deny`/`:machete` (add waivers for the new deps) / `:breaking` (the additive `scope_prn` should be clean) / `:affected-smoke` findings; `CI=1 cargo test -p paigasus-gateway -p paigasus-iam -p paigasus-iam-core`. Report each gate. Commit any gate fixes `chore(rs): satisfy ci gates for the gateway m0 slice (SMA-446)`.

## Self-Review
- Spec coverage: IAM action+role (G1), scope_prn cache (G2), scaffold/config (G3), IAM clients (G4), self-query auth mw (G5), OpenAI dto+egress (G6), proxy handler+streaming (G7), readyz+abort (G8), gates (G9). Guardrails: body-limit (G7), split timeouts (G3/G6), secret hygiene (G6). Deferred (M1+): multi-provider, canonical model, rate-limit/cost/cache, /v1/models.
- Risk: G2 (scope_prn cache plumbing) is the subtle IAM change — the cache-hit-no-DB-read test guards it. G5 self-query correctness is the security crux — its test asserts no cross-principal query.
- Type consistency: `CallerContext`/`IamClient`/`OpenAiClient`/`ChatCompletionRequest`/`scope_prn` names align G3→G9.
