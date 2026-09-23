# SMA-635 — Interactive-user auth for the gateway chat surface, and the console playground

- **Issue:** [SMA-635](https://linear.app/smaschek/issue/SMA-635) (split out of SMA-512, spec decision D11)
- **Status:** Draft, revision 1
- **Date:** 2026-09-23
- **ADR:** a Notion ADR (status Proposed) records decisions D1 to D5. It is written after this spec
  is approved and is linked here. It must be Accepted before implementation starts.

## 1. Problem

`POST /v1/chat/completions` is wrapped by `require_iam_auth`
(`rs/crates/services/paigasus-gateway/src/adapters/http/mod.rs`). That middleware calls only
`iam.introspect_api_key`, requires a non-empty `scope_prn`, and authorizes `InvokeModel` against
that `scope_prn` with the D9 self-query. A console session bearer is an OIDC access token, so it
receives `401 invalid-api-key`. The chat surface has no path for an interactive user.

Two shortcuts are closed:

- A shared service-account key for the console is forbidden by ADR-0020 D4. It also removes the
  per-user attribution in the gateway's request log.
- A test against a fake gateway is not sufficient. SMA-512 revision 1 did this, and every tier was
  green while the product could not make one chat call.

## 2. Outcome

1. A console user calls the gateway chat surface with the user's own OIDC bearer. The gateway
   authorizes `InvokeModel` for that user against an organization, and it logs the user as the
   principal.
2. The gateway console has a playground page at `/gateway/orgs/<org>/playground`. It streams a chat
   completion through a Next route handler, and it has a Stop control.
3. The composer is disabled when the gateway does not advertise `gateway.chat.stream`.
4. The top test tier runs the **real** gateway binary.

## 3. Decisions

| # | Decision | Reason |
|---|---|---|
| D1 | Widen `require_iam_auth` in place: API-key leg first, then the OIDC leg (approach A). | One route, one SDK URL, one log line. The two-leg order copies `require_authenticated`, so the two middlewares cannot drift. A second middleware needs a prefix test to choose, which the code already rejects (`api_keys.key_prefix` is operator-configurable). A second route duplicates the handler. |
| D2 | A user names the org with the header `paigasus-org: <org UUID>`. | IAM has no RPC that resolves an org slug (`GetOrganization` takes a PRN; `ListOrganizations` has no filter and is `platform_admin`-only). The org PRN holds the UUID. The console URL already holds the UUID. |
| D3 | Without the header, the gateway infers the org when the user has exactly one org. With zero or several orgs, it answers 400 `org-required`. | The user's rule. 400, not 403: the request is incomplete, not forbidden. |
| D4 | The gateway authorizes `InvokeModel` against the **org** PRN only. A user with `gateway_user` only on a team or project gets 403. | One IAM call per request. The scope matches the console's org route. A narrower scope can come in a later issue. |
| D5 | On the API-key path the gateway ignores `paigasus-org`. The key's own `scope_prn` stays the scope. | A key is issued under one scope. The header must not let a key holder choose a different resource. |
| D6 | The playground's route handler sends every failure as `{ "error": <PaigasusError> }`. | One browser-safe shape that carries the correlation ID. The SDK already maps every gateway failure to a `PaigasusError`. |
| D7 | When streaming is off, the composer is disabled. There is no non-stream fallback. | The issue's scope. One render path. |
| D8 | The route handler does not check the capability itself. The gateway's `400 streaming-disabled` is the authority. | One source of truth, and one discovery call less per chat turn. The page still disables the composer. |
| D9 | The top tier is the gateway-console Playwright e2e with the real `paigasus-gateway` binary, the in-process fake IAM, and a mock OpenAI SSE server. | It runs the real middleware, route, passthrough and Stop. No Keycloak and no new Docker suite. |

## 4. Gateway (Rust)

### 4.1 `require_iam_auth`, new order

1. Read the bearer (`bearer()`, no change). No bearer gives `401 invalid-api-key`.
2. Call `introspect_api_key(token)`. If it succeeds with status `active`, continue on the
   **existing API-key path without any change**: the `scope_prn` checks, the D9 self-query with the
   key, `CallerContext` with the key. The `paigasus-org` header is not read on this path (D5).
3. If the key leg fails or the status is not `active`, record `api_key_inconclusive` with the same
   rule as `require_authenticated` (`introspect_error(err) == GatewayError::IamUnavailable`). Then
   call `introspect_token(token)`.
   - An `Rpc` error maps through `introspect_error`, then through `preserve_outage`. So an IAM
     outage on the key leg gives 503, not 401.
   - `identity-not-provisioned` is **rejected** (401), unlike `require_authenticated`. A user with
     no provisioned identity has no grants. The test
     `require_iam_auth_still_rejects_an_unprovisioned_identity` stays and keeps passing.
   - A status that is not `active` gives `401 invalid-api-key`.
4. Resolve the org (section 4.2). A failure gives 400.
5. Call `is_authorized_self(token, principal_prn, "InvokeModel", org_prn)`. The existing
   `authz_result` mapping applies unchanged: `false` gives `403 insufficient-permissions`, and the
   error rows keep their current codes. IAM accepts an OIDC bearer on `IsAuthorized`
   (`paigasus-iam/src/adapters/grpc/authn.rs`, credential router). The self-query
   (`principal == actor`) skips the exposure gate (`application/authorize.rs`, `decide_gated`).
6. Insert `CallerContext` (section 4.3) and call the handler.

The 401 code stays `invalid-api-key` for a rejected user bearer too. The code is part of the
OpenAI-compatible envelope, and a new code would break clients that match on it. The message text
may say "credential" instead of "API key".

The doc comment of `require_authenticated` says "`require_iam_auth` is unchanged". That sentence
is updated: `require_iam_auth` now accepts a user bearer too, but it still rejects an unprovisioned
identity and it still authorizes.

### 4.2 Org resolution (a pure function)

A new pure function in the gateway domain, with no I/O:

```rust
fn resolve_org(header: Option<&HeaderValue>, introspection: &IntrospectResponse)
    -> Result<Prn, OrgResolutionError>
```

- **Header present.** The value must be valid visible ASCII and must parse as a UUID
  (`Uuid::parse_str`, which also accepts the simple, braced and `urn:uuid:` forms). A failure gives
  `400 invalid-org-header`, `param: "paigasus-org"`. On success the PRN is the kernel's
  organization PRN for that UUID (`OrganizationId::from_uuid`, canonical form
  `prn:pgs:iam:::organization/<uuid>`). The header is not checked against the user's grants
  here. IAM decides that in step 5. Two or more `paigasus-org` headers give
  `400 invalid-org-header`.
- **Header absent.** Collect the distinct org UUIDs from every `memberships[].node_prn` and every
  `role_grants[].scope_prn`, with `Prn::parse` and `TenancyNodeRef::from_prn(..)` (organization:
  its own UUID; team and project: `org_uuid()`). Ignore a PRN that does not parse as a tenancy node
  (for example a Root grant). Exactly one org gives that org's PRN. Zero or several give
  `400 org-required`, `param: "paigasus-org"`, with a message that tells the caller to send the
  header.

The gateway crate adds a dependency on `paigasus-iam-core` for `TenancyNodeRef` and
`OrganizationId` if it does not have one. The plan checks that this does not pull a server-side
dependency into the gateway. If it does, the plan moves the two parsers or uses the
`paigasus-kernel` `Prn` API directly.

### 4.3 `CallerContext` and the request log

```rust
pub struct CallerContext {
    pub principal_prn: String,
    pub scope_prn: String,          // key: the key's scope; user: the resolved org PRN
    pub credential: Credential,     // ApiKey { key_id } | User
}
```

The log line in `chat.rs` records `principal`, `scope` (new), `auth` (`api_key` or `user`), and
`key_id` only for an API key. It still never records the prompt, the body or the OpenAI key.

### 4.4 Error codes

Two new `ErrorReason` values, registered at the registry's single site
(`contracts/proto/paigasus/common/v1/error.proto`, and `EXPECTED_REASONS` in
`rs/crates/libs/paigasus-proto/src/error.rs`), and so checked by `repo:error-code-single-site`:

| Code | Status | `param` | When |
|---|---|---|---|
| `invalid-org-header` | 400 | `paigasus-org` | The header is not a single valid UUID. |
| `org-required` | 400 | `paigasus-org` | No header, and the user has zero or several orgs. |

Both get a `GatewayError` variant and a row in the gateway's envelope tests. The generated
bindings (Rust, Python, TypeScript) are regenerated, and the SDK's `ErrorReason` mapping gets the
two values if it lists reasons explicitly.

### 4.5 Rust tests (`auth.rs` unit module, with the existing `FakeIam`)

The existing rows stay. New rows:

1. OIDC bearer, valid header, authz allows: the handler runs with `Credential::User` and the org
   PRN as scope.
2. OIDC bearer, no header, exactly one org (one from memberships, one from a team role grant, and
   both in the same org): inferred.
3. No header, zero orgs: `400 org-required`.
4. No header, two orgs: `400 org-required`.
5. Malformed header, and two headers: `400 invalid-org-header`.
6. Authz returns false: `403 insufficient-permissions`.
7. **Self-query proof (D9 row for users):** `is_authorized_self` receives the user's own token,
   the introspected user PRN, `InvokeModel`, and the org PRN from the header.
8. Key leg inconclusive (IAM unavailable) and OIDC leg rejects: 503, not 401.
9. API key with a `paigasus-org` header for another org: the key's `scope_prn` is used, and
   `introspect_token` is never called.
10. A team-only grant with a header for its org: the gateway asks IAM about the org PRN (the
    fake returns false, so the result is 403). This row pins D4.

The pure `resolve_org` function gets its own table tests, including a Root grant, a PRN that does
not parse, and duplicates of the same org.

## 5. SDK (`ts/packages/paigasus-sdk/src/chat.ts`)

`completions(request, { signal, org })` gets an optional `org?: string`. When `org` is set, the
client sends `paigasus-org: <org>`. The SDK does not validate the value; the gateway is the
authority. The option is per call, because one route handler serves every org. The existing tests
stay; a new test checks that the header is sent when `org` is set and absent when it is not.

## 6. Console (`ts/apps/gateway-console`)

### 6.1 Page: `/gateway/orgs/[org]/playground`

- A server component. It validates `[org]` as a UUID with the rule `orgs/[org]/load.ts` uses. If
  the value is not a UUID, the page gives not-found.
- It reads the gateway state through `discovery().getServiceState('gateway', token)` and
  `gatewayView()` (`app/_components/gateway-state.ts`). It passes `streaming: boolean` to the
  client component `Playground`.
- When `streaming` is false, the composer is disabled and a notice says that streaming is off on
  this gateway.
- The org navigation gets a "Playground" link.

### 6.2 Route handler: `POST /gateway/api/chat` (`app/api/chat/route.ts`)

The path is fixed, so `'/api/chat'` is added to the `publicPaths` list in `proxy.ts`. The auth
proxy then does not redirect it, and the handler answers its own 401. The proxy still sets the
correlation headers.

The handler does these steps in this order:

1. **Origin.** If `Origin` is absent or is not the zone's own public origin: 403.
2. **Content type.** If the media type is not `application/json`: 415. With step 1, this stops a
   cross-site form post.
3. **Session.** `optionalSession()`. No session: 401.
4. **Body.** `{ org, model, messages }`. `org` is a UUID. `model` is a non-empty string.
   `messages` is a non-empty array of `{ role: 'system' | 'user' | 'assistant', content: string }`.
   Any other shape: 400. The handler builds the gateway request itself as
   `{ model, messages, stream: true }`, and it forwards no other client field. The gateway
   enforces the size limit.
5. **Call.** `createChatClient({ baseUrl: <gateway service URL from the runtime config> },
   { bearer: session.accessToken })`, then `.completions(req, { signal: request.signal, org })`.
   The browser's abort closes the request, Next aborts `request.signal`, and the abort reaches the
   gateway and the upstream.
6. **The three `ChatResult` arms:**
   - `stream`: `200`, `content-type: text/event-stream`, `cache-control: no-store`, and the
     correlation ID header. The body goes through a pass-through `TransformStream`. Each gateway
     chunk is enqueued **unchanged and at once**, and the same chunk goes into
     `createTerminalFrameParser(200, ids)`. When the parser returns a `PaigasusError`, the handler
     enqueues one more SSE event after that chunk: `event: paigasus-error` and
     `data: <PaigasusError JSON>`. The browser then sees only one error shape (D6).
   - `json`: not expected, because the handler always sends `stream: true`. Answer 502 with a
     `PaigasusError`.
   - `error`: `{ "error": <PaigasusError> }`, with the HTTP status of the error's presentation.

Every failure in steps 1 to 4 also uses `{ "error": <PaigasusError> }`. The plan names the helper
that builds a `PaigasusError` for these local failures, so that the shape is identical to the SDK's.

### 6.3 Browser: `Playground` client component

- A model text field (required, no default: the gateway has no model list), a message list, a
  composer, a Send button and a Stop button.
- The conversation lives in React state only. Nothing is stored. Each turn sends the full history.
- Send makes a new `AbortController` and calls `fetch('/gateway/api/chat')`.
  - A non-2xx answer: read `{ error }` and show the message and the correlation ID.
  - A 2xx answer: read the body with a pure SSE parser, `lib/chat-stream.ts`. It appends
    `choices[0].delta.content`, ends at `data: [DONE]`, and shows a `paigasus-error` event as an
    error. It keeps a partial record and a partial multi-byte character across chunks, and it
    bounds the pending record like `createTerminalFrameParser` does.
- Stop calls `abort()`. The partial answer stays and is marked "stopped".
- The Send button is disabled while a turn runs, when the model field is empty, and when
  `streaming` is false.

## 7. Tests

### 7.1 Unit (vitest)

- `lib/chat-stream.ts`: a record split over chunks, a multi-byte character split over chunks,
  `[DONE]`, the `paigasus-error` event, a comment line, and the pending-record bound.
- The route handler: 403 for origin, 415, 401, 400 for body shapes, the three SDK arms (with an
  injected `fetch`), the added `paigasus-error` event, and a check that the gateway request holds
  only `model`, `messages` and `stream: true`.
- The SDK `org` header (section 5).

### 7.2 E2E (Playwright, `gateway-console-ts:test-e2e`)

The harness starts the **real** `paigasus-gateway` binary. Its IAM endpoint is the in-process fake
IAM (h2c gRPC). Its OpenAI endpoint is a new mock SSE server in the harness. The Moon task gets a
dependency on the gateway's build target. `isAuthorized` is **scripted** in every test that is
about authorization, because the fake's unscripted default is allow.

1. A streamed answer appears on the page, chunk by chunk.
2. Stop ends the stream, and the mock OpenAI server sees its connection close.
3. A scripted `isAuthorized` deny shows the 403 message on the page.
4. A gateway started with `stream_enabled = false` shows the disabled composer.
5. The fake IAM records that `isAuthorized` received the org PRN of the URL's org UUID, the user's
   principal, and `InvokeModel`.
6. A request without a session cookie to `/gateway/api/chat` gets 401 JSON, not a redirect.

Org inference (D3) is tested only in the Rust unit tests. The e2e always sends the header.

## 8. Out of scope

- A team- or project-level scope for the playground (D4).
- A model list, conversation storage, and system-prompt presets.
- A non-stream fallback (D7).
- A Docker-gated suite with real IAM and Keycloak for the gateway.
- Slug lookup in IAM (D2).

## 9. Risks

- **The fake IAM is not Cedar.** The e2e proves the wiring (the right PRN, principal and action
  reach IAM, and a deny is shown). It does not prove the Cedar decision. The IAM crate's own tests
  own that decision. `gateway_user` at org scope already exists in `roles.rs`.
- **A new gateway binary in the e2e harness** adds build time to `gateway-console-ts:test-e2e`.
  The plan measures it.
- **The `Origin` check** depends on the zone's public origin in the runtime config. The plan reads
  the value that the auth routes already use, and it does not add a second setting.
