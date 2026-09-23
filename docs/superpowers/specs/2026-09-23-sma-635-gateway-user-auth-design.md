# SMA-635 — Interactive-user auth for the gateway chat surface, and the console playground

- **Issue:** [SMA-635](https://linear.app/smaschek/issue/SMA-635) (split out of SMA-512, spec decision D11)
- **Status:** Draft, revision 2 (after the adversarial challenge; changelog in section 11)
- **Date:** 2026-09-23
- **ADR:** [ADR-0023 — Interactive-user authentication on the gateway chat surface](https://app.notion.com/p/3e4830e8fbaa81819807f9f91b469e60)
  (status Proposed) records decisions D1 to D5 and D10. It must be Accepted before
  implementation starts.
- **Follow-ups:** [SMA-676](https://linear.app/smaschek/issue/SMA-676) (grant control, D10),
  [SMA-677](https://linear.app/smaschek/issue/SMA-677) (rate limit and spend budget).

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
   completion through a Next route handler, chunk by chunk, and it has a Stop control.
3. The composer is disabled when the gateway does not advertise `gateway.chat.stream`.
4. The top test tier runs the **current** gateway binary, built from the commit under test.

## 3. Decisions

| # | Decision | Reason |
|---|---|---|
| D1 | Widen `require_iam_auth` in place: API-key leg first, then the OIDC leg (approach A). | One route, one SDK URL, one log line. The two-leg order copies `require_authenticated`, so the two middlewares cannot drift. A second middleware needs a prefix test to choose, which the code already rejects (`api_keys.key_prefix` is operator-configurable). A second route duplicates the handler. |
| D2 | A user names the org with the header `paigasus-org: <org UUID>`. | IAM has no RPC that resolves an org slug (`GetOrganization` takes a PRN; `ListOrganizations` has no filter and is `platform_admin`-only). The org PRN holds the UUID. The console URL already holds the UUID. |
| D3 | Without the header, the gateway infers the org when the user has exactly one org. With zero or several orgs, it answers 400 `org-required`. | The user's rule. 400, not 403: the request is incomplete, not forbidden. |
| D4 | The gateway authorizes `InvokeModel` against the **org** PRN only. A user with `gateway_user` only on a team or project gets 403. | One IAM call per request. The scope matches the console's org route. A narrower scope can come in a later issue. |
| D5 | On the API-key path the gateway ignores `paigasus-org`. The key's own `scope_prn` stays the scope. The gateway logs a warning when it ignores the header. | A key is issued under one scope. The header must not let a key holder choose a different resource. |
| D6 | The playground's route handler sends every failure as `{ "error": <PaigasusError> }`, and every mid-stream failure as one `event: paigasus-error` SSE event. | One browser-safe shape that carries the correlation ID. The SDK already maps every gateway failure to a `PaigasusError`. |
| D7 | When streaming is off, the composer is disabled. There is no non-stream fallback. | The issue's scope. One render path. |
| D8 | The route handler does not check the capability itself. The gateway's `400 streaming-disabled` is the authority. | One source of truth, and one discovery call less per chat turn. The gateway checks `stream` after authorization and before egress (`chat.rs:104-106`). The page still disables the composer. |
| D9 | The top tier is a new `playground` Playwright project in gateway-console, with the real `paigasus-gateway` binary, its own in-process fake IAM, and a mock OpenAI SSE server. | It runs the real middleware, route, passthrough and Stop. No Keycloak and no new Docker suite. A separate project keeps the existing rows (R19, R20, `capabilities.spec.ts`) unchanged. |
| D10 | A person gets `gateway_user` at org scope **out of band**: an `org_admin` calls IAM `GrantRole` (`org_admin` holds `GrantRole`, `roles.rs:125`). This issue adds no grant control in the console. A follow-up issue owns that control. | `org_admin` does not hold `InvokeModel` (`roles.rs:105-135`), and the console grants `gateway_user` only to service accounts (`service-accounts/commands.ts:16-17`). Without a stated grant path, an org creator gets 403 on the first Send. |

## 4. Gateway (Rust)

### 4.1 `require_iam_auth`, new order

1. Read the bearer (`bearer()`, no change). No bearer gives `401 invalid-api-key`.
2. Call `introspect_api_key(token)`. If it succeeds with status `active`, continue on the
   **existing API-key path without any change**: the `scope_prn` checks, the D9 self-query with the
   key, `CallerContext` with the key. The `paigasus-org` header is not read for the scope on this
   path (D5). If the header is present, the gateway logs one `warn` line with `key_id` and
   "paigasus-org ignored for an API key". It never logs the header value.
3. If the key leg fails or the status is not `active`, record `api_key_inconclusive` with the same
   rule as `require_authenticated` (`introspect_error(err) == GatewayError::IamUnavailable`). Then
   call `introspect_token(token)`.
   - An `Rpc` error maps through `introspect_error`, then through `preserve_outage`. So an IAM
     outage on the key leg gives 503, not 401.
   - `identity-not-provisioned` on the token leg is **rejected** (401), unlike
     `require_authenticated`. A user with no provisioned identity has no grants. This cannot affect
     a console user: the console layout calls `WhoAmI` on every render, which provisions the
     identity (`layout.tsx:32` → `runtime.ts:99,111-117`).
   - A status that is not `active` gives `401 invalid-api-key`.
4. Resolve the org (section 4.2). A failure gives 400.
5. Call `is_authorized_self(token, principal_prn, "InvokeModel", org_prn)`. IAM accepts an OIDC
   bearer on `IsAuthorized` (`paigasus-iam/src/adapters/grpc/authn.rs:211-221`). The self-query
   (`principal == actor`) skips the exposure gate (`application/authorize.rs:75-80`). An org UUID
   that does not exist gives Deny, not an error, so the answer does not show whether the org
   exists (`cedar_authorizer.rs:231-236`).
   - `Ok(false)` gives `403 insufficient-permissions`.
   - A `PermissionDenied` with the IAM reason `principal-inactive` or `provisioning-failed` gives
     `401 invalid-api-key`. `AuthEnforce` can send these reasons for an OIDC bearer
     (`authn.rs:217-241`). Only the reason `forbidden` keeps the existing 500 "possible broken
     self-query" mapping (`auth.rs:106-117,336-347`). The branch reads `ErrorInfo` with the same
     domain check as `is_identity_not_provisioned`.
   - The other error rows keep their current codes.
6. Insert `CallerContext` (section 4.3) and call the handler.

The 401 code stays `invalid-api-key` for a rejected user bearer too. The code is part of the
OpenAI-compatible envelope, and a new code would break clients that match on it. The message text
says "credential" instead of "API key".

**Cost per user turn.** A user turn makes three IAM RPCs: `IntrospectApiKey` (rejected),
`Introspect` (loads every membership and every grant, `authenticate_token.rs:163-191`), and
`IsAuthorized`. An API-key turn makes two, as today. The argument that `require_authenticated`
uses ("one extra RPC on a low-frequency, client-cached call", `auth.rs:131-137`) does not hold for
chat. This spec accepts the cost for the playground's traffic. A cache is out of scope (section 9).

### 4.2 Org resolution (a pure domain function)

The domain function takes no transport types:

```rust
pub enum OrgHeader<'a> { Absent, One(&'a str), Many }

pub fn resolve_org(header: OrgHeader<'_>, node_prns: &[&str]) -> Result<Prn, OrgResolutionError>
```

The adapter builds `OrgHeader` from `headers.get_all("paigasus-org")`: no value gives `Absent`,
one value gives `One` (a value that is not visible ASCII gives `invalid-org-header` at once), and
two or more give `Many`. The adapter passes every `memberships[].node_prn` and every
`role_grants[].scope_prn` as `node_prns`.

- **`One(value)`.** The value must be a UUID in the kernel's 36-character hyphenated form (the rule
  in `paigasus-kernel` `resource_name.rs:101-107`). Any other form gives `400 invalid-org-header`,
  `param: "paigasus-org"`. On success the PRN is built with the kernel `Prn` builder:
  service `iam`, empty region, empty org slot, type `organization`, id the UUID. The canonical
  form is `prn:pgs:iam:::organization/<uuid>`. The header is not checked against the user's grants
  here. IAM decides that in step 5.
- **`Many`.** `400 invalid-org-header`.
- **`Absent`.** Parse each PRN with the kernel `Prn` API. For type `organization`, the org is the
  resource id. For type `team` or `project`, the org is the `org()` slot. Ignore a PRN that does
  not parse, a PRN of another service, and a PRN of another type (for example a Root grant).
  Exactly one distinct org gives that org's PRN. Zero or several give `400 org-required`,
  `param: "paigasus-org"`. The message tells the caller to send the header.

**Dependencies.** The gateway uses the `paigasus-kernel` `Prn` API (`service()`, `org()`,
`resource_type()`, `resource_id()`, `build()`, `resource_name.rs:109-187`). It does **not** depend
on `paigasus-iam-core`: that crate pulls `cedar-policy` and other crates into the gateway
(`paigasus-iam-core/Cargo.toml:10-30`), and the gateway's own doc keeps iam-core out
(`auth.rs:43-45`). The change:

- adds `paigasus-kernel` to the gateway's `[dependencies]`, and the matching `dependsOn` edge and
  `fileGroups.upstreams` entries in the gateway's `moon.yml`;
- removes the gateway's `ALLOW_NO_CARGO_BACKING` entry in `ci/affected-graph/cargo_moon_parity.py`
  (`:75-81`), because a real Cargo edge now backs it;
- moves `uuid` from `[dev-dependencies]` to `[dependencies]` (`Cargo.toml:92`).

**Consequence of D3, documented in the gateway README and the ADR.** A client that sends no header
works while its user has one org. When the user joins a second org, the same client starts to get
`400 org-required`. Clients that know the org must send the header.

### 4.3 `CallerContext` and the request log

```rust
pub struct CallerContext {
    pub principal_prn: String,
    pub scope_prn: String,          // key: the key's scope; OIDC: the resolved org PRN
    pub credential: Credential,     // ApiKey { key_id } | Oidc
}
```

`Oidc`, not `User`, because an OIDC bearer can belong to a machine client. IAM uses the same name
(`authn.rs:230`).

The log line `chat completion proxied` in `chat.rs` records `principal`, `scope` (new), `auth`
(`api_key` or `oidc`), and `key_id` only for an API key. It still never records the prompt, the
body or the OpenAI key.

### 4.4 Metrics

`record_iam_call` labels, per leg:

| Leg | Operation label | Result label |
|---|---|---|
| Key leg, active key | `introspect_api_key` | `ok` (no change) |
| Key leg, rejected bearer | `introspect_api_key` | `denied` (same as `require_authenticated`, `auth.rs:172`) |
| Key leg, IAM outage | `introspect_api_key` | the existing outage label |
| Token leg | `introspect_token` | `ok`, `denied`, or the outage label |
| Self-query | `is_authorized` | no change |

`require_iam_auth` today records `ok` for a key with a status that is not `active`
(`auth.rs:64-67`). That row changes to `denied`, to match `require_authenticated`. The Grafana
panel (`ops/observability/grafana/dashboards/gateway.json:74`) keeps working, because the label
names do not change. `repo:observability-drift` is run to confirm. A new metrics test covers the
OIDC path.

### 4.5 Error codes

Two new `ErrorReason` values in the gateway block of
`contracts/proto/paigasus/common/v1/error.proto`, with the next numbers **309** and **310**:

| Code | Number | Status | `param` | When |
|---|---|---|---|---|
| `invalid-org-header` | 309 | 400 | `paigasus-org` | The header is not a single UUID in the 36-character form. |
| `org-required` | 310 | 400 | `paigasus-org` | No header, and the user has zero or several orgs. |

- Registration is checked by `the_registry_contains_exactly_the_expected_reasons`
  (`paigasus-proto/src/error.rs:221-230`), so both codes go into `EXPECTED_REASONS` and its count
  anchor. `repo:error-code-single-site` checks the files that emit codes, so the new `GatewayError`
  variants emit them only in `adapters/http/error.rs`.
- The generated bindings (Rust, Python, TypeScript) are regenerated with `contracts:generate`.
- The SDK table `PRESENTATION` is a total `Record` over `ErrorReason`
  (`sdk/src/errors/presentation.ts:20-79`). `tsc` fails until both codes get an entry. Both are
  `invalid-request`-type presentations (the plan uses the presentation of the existing gateway
  400 codes).
- `param` names a header here. OpenAI uses `param` for a body field. The envelope doc in
  `error.rs` records this deviation, and the sentence "only `StreamingDisabled` sets `param`"
  (`error.rs:32-33`) is corrected.

### 4.6 The terminal SSE frame

`TERMINAL_SSE_ERROR` (`chat.rs:63`) gets `\n\n` in front of `data:`. Without it, an upstream failure
inside a record joins the frame to the partial record, and no parser can read the frame. An empty
record dispatches nothing in SSE, so a client that reads a clean record boundary sees no change.
The existing test at `chat.rs:236-248` stays green. A new test puts the failure inside a record and
asserts that the frame is a separate record.

### 4.7 Doc comments that become false

Update: `Iam::introspect_token` ("The chat path never calls this", `iam/client.rs:67-73`); the
`auth.rs` module doc (`:3-25`, service accounts only); the `require_authenticated` sentence
"`require_iam_auth` is unchanged" (`:145-146`); the `record_iam_call` doc (`:274-279`); the
`param` sentence in `error.rs` (`:32-33`); `ts/apps/gateway-console/README.md:85`; and the
`unreachable!` in `tests/chat_proxy.rs:102-103`.

### 4.8 Rust tests

**Existing rows.** The unit `FakeIam::introspect_token` panics when a test sets no outcome
(`auth.rs:483-488`). Every existing key-leg failure row (`auth.rs:657-690`) now calls the token
leg. Each such row gets the token outcome that real IAM sends for a bearer that is not a JWT:
`Unauthenticated` with reason `invalid-token` (`paigasus-iam` `convert.rs:141`). The expected
result of each row does not change.

**The unprovisioned-identity guard is rewritten.** The current row puts `identity-not-provisioned`
on the key leg, but real IAM sends it on the token leg (`convert.rs:142`). New form: key leg
`Unauthenticated`, token leg `PermissionDenied` with `identity-not-provisioned` details: 401. A
second form: key leg inconclusive (IAM unavailable), same token leg: 503.

**The integration fakes** in `tests/chat_proxy.rs`, `tests/metrics.rs` and `tests/service_info.rs`
get a real `introspect_token` answer in place of `unreachable!`.

**New rows** (unit module, `FakeIam`):

1. OIDC bearer, valid header, authz allows: the handler runs with `Credential::Oidc` and the org
   PRN as scope.
2. OIDC bearer, no header, one org reached twice (one membership on the org, one `gateway_user`
   grant on a team of the same org): inferred.
3. No header, zero orgs: `400 org-required`.
4. No header, two orgs: `400 org-required`.
5. A header that is not a UUID, a simple-form UUID, and two headers: `400 invalid-org-header`.
6. Authz returns false: `403 insufficient-permissions`.
7. **Self-query proof (D9 row for OIDC):** `is_authorized_self` receives the user's own token, the
   introspected user PRN, `InvokeModel`, and the org PRN built from the header.
8. Key leg inconclusive and OIDC leg rejects: 503, not 401.
9. API key with a `paigasus-org` header for another org: the key's `scope_prn` is used,
   `introspect_token` is never called, and one warning is logged.
10. A team-only grant with a header for its org: the gateway asks IAM about the **org** PRN (the
    fake returns false, so the result is 403). This row pins D4.
11. Authz `PermissionDenied` with `principal-inactive`, and with `provisioning-failed`: 401. With
    `forbidden`: 500 (no change).

`resolve_org` gets table tests: every `OrgHeader` arm, a Root grant, a PRN of another service, a
PRN that does not parse, and duplicates of the same org.

**IAM-side row.** The table test in `paigasus-iam-core` `roles.rs` gets the row "`org_admin` does not
hold `InvokeModel`". It records the consequence of D4 and D10 as a decision, so that a change to the
role becomes a visible test change.

## 5. SDK (`ts/packages/paigasus-sdk/src/chat.ts`)

`completions(request, { signal, org, correlationId })` gets two optional per-call fields:

- `org?: string`: sent as `paigasus-org`. The SDK does not check the UUID form; the gateway is the
  authority. It does refuse a value with a character that is not valid in a header value (for
  example CR or LF) with a `TypeError`, the same way it refuses a request that is not
  serializable. Without this check, `fetch` throws and the SDK reports a false `network` failure.
- `correlationId?: string`: sent as `paigasus-correlation-id`. The gateway adopts an inbound id
  (`paigasus-observability` `correlation.rs:104-115`), so the console log and the gateway log
  share one id.

New tests: each header is sent when its field is set and absent when it is not, and a CR or LF in
`org` throws.

## 6. Console (`ts/apps/gateway-console`)

### 6.1 Page: `/gateway/orgs/[org]/playground`

- A server component. It uses the same loader as the org page (`orgs/[org]/load.ts:79-87`): a value
  that is not a UUID gives not-found, and a `GetOrganization` denial or absence gives the same
  not-found or forbidden view as the org page.
- It reads the gateway state through `discovery().getServiceState('gateway', token)` and
  `gatewayView()` (`app/_components/gateway-state.ts`). It passes the view to the client
  component `Playground`.
- The composer notice depends on the view:
  - `available` without `gateway.chat.stream`: "Streaming is off on this gateway." Composer
    disabled.
  - `degraded` or `absent`: "The gateway is not available." Composer disabled.
  - `available` with the capability: composer enabled.
- The org page `orgs/[org]` gets a "Playground" link. The zone-level `PrimaryNav` (`lib/nav.ts`)
  does not change, because it has no org context.

### 6.2 Route handler: `POST /gateway/api/chat`

The logic lives in a factory, `lib/chat-route.ts`, that takes its dependencies (session reader,
chat-client factory, config, correlation id reader). `app/api/chat/route.ts` only connects the real
dependencies. Unit tests call the factory.

The path is fixed, so `'/api/chat'` is added to the `publicPaths` list in `proxy.ts`. The auth
proxy then does not redirect it, and the handler answers its own 401. The proxy still sets the
correlation headers. `publicPaths` is an exact-match set (`middleware.ts:107,116`), so this opens
no other path.

The handler does these steps in this order. Every local failure uses `{ "error": <PaigasusError> }`
with the request's correlation id (`requestCorrelationId()`), built with the console-core error
helpers (`console-core/src/errors.ts:69-93`):

1. **Origin.** If `Origin` is absent or is not the zone's own public origin (the value the auth
   routes already use, no new setting): 403, `neverReachedIam()` with presentation `forbidden`.
2. **Content type.** If the media type is not `application/json`: 415, reason
   `unsupported-content-type`. With step 1 and the `SameSite=Lax` session cookie
   (`paigasus-auth/src/http/cookies.ts:41-47`), this gives three separate CSRF controls.
3. **Session.** `optionalSession()`. No session: 401, `sessionExpired()`. `getSession` refreshes
   under its single-flight lock and answers `null` on any failure (`get-session.ts:47-85`).
4. **Body.** The handler reads at most the gateway's `max_request_bytes` (from the runtime config).
   More gives 413. The body is `{ org, model, messages }`: `org` is a UUID, `model` is a non-empty
   string, and `messages` is a non-empty array of
   `{ role: 'system' | 'user' | 'assistant', content: string }`. Any other shape gives 400, reason
   `invalid-request-schema`. The handler builds the gateway request itself as
   `{ model, messages, stream: true }`, and it forwards no other client field.
5. **Call.** `createChatClient({ baseUrl: <gateway service URL>, headerTimeoutMs: 35_000 },
   { bearer: session.accessToken })`, then
   `.completions(req, { signal: request.signal, org, correlationId })`. The 35 s header timeout is
   longer than the gateway's 30 s first-byte wait (`config.rs:187`) plus its three IAM RPCs. The
   SDK default of 10 s is shorter, so it would time out before the gateway does.
6. **The three `ChatResult` arms:**
   - `stream`: status 200, with these headers: `content-type: text/event-stream`,
     `cache-control: no-cache, no-transform`, `x-accel-buffering: no`, and the correlation id.
     `no-transform` stops Next's default response compression, which would otherwise hold the
     bytes until a flush. The response body is `result.body.pipeThrough(transform)`. So a cancel
     from the browser reaches the SDK stream too, as a second path next to `request.signal`. The
     transform:
     - enqueues each gateway chunk **unchanged and at once**;
     - pushes the same chunk into `createTerminalFrameParser(200, ids)`. When the parser returns a
       `PaigasusError`, it enqueues one more event after that chunk:
       `\n\nevent: paigasus-error\ndata: <PaigasusError JSON>\n\n`. The leading blank line closes
       any partial record;
     - on an error of the source stream (a transport failure between the gateway and Next), it
       enqueues a `paigasus-error` event built from `mapError({ kind: 'transport', cause:
       'network' })` with the correlation id, and then closes the stream.
   - `json`: not expected, because the handler always sends `stream: true`. Answer 502 with a
     `PaigasusError`.
   - `error`: `{ "error": <PaigasusError> }`. The HTTP status is `transport.status` for an HTTP
     error, 504 for a timeout, and 502 for a network failure.

**Upstream text.** The gateway forwards an upstream non-2xx body unchanged (`chat.rs:113-119,
139-141`), and `mapHttp` copies its `message` (`sdk/src/errors/map-error.ts:187-205`). An OpenAI 401
text contains a masked piece of the gateway's OpenAI key. So when the `PaigasusError` has a null
`reason` (a body that is not a Paigasus envelope), the handler replaces `message` with a generic
text and keeps the correlation id. This applies to the `error` arm and to the injected
`paigasus-error` event.

### 6.3 Browser: `Playground` client component and `lib/chat-stream.ts`

- A model text field (required, no default: the gateway has no model list), a message list, a
  composer, a Send button and a Stop button.
- The conversation lives in React state only. Nothing is stored. Each turn sends the full history.
  A stopped turn's partial answer stays in the history. A failed turn (an error with no content) is
  not sent in the next turn.
- Send makes a new `AbortController` and calls `fetch('/gateway/api/chat')`.
  - A non-2xx answer: read `{ error }` and show the message and the correlation id in the
    playground's own error area. For `insufficient-permissions`, the text names the missing role:
    "You need the gateway_user role on this organization. Ask an organization admin to grant it."
  - A 2xx answer: read the body with the pure parser `lib/chat-stream.ts`.
- `lib/chat-stream.ts`:
  - appends `choices[0].delta.content` from each `data:` record;
  - ignores a `data:` record that has no `choices` (this includes the gateway's raw
    `{"error": …}` frame, because the handler injects the `paigasus-error` event for it);
  - ends the turn at `data: [DONE]`;
  - shows an `event: paigasus-error` record as an error;
  - treats an end of stream with no `[DONE]` and no `paigasus-error` as an error;
  - ignores comment lines;
  - keeps a partial record and a partial multi-byte character across chunks, and bounds the
    pending record the same way `createTerminalFrameParser` does.
- Stop calls `abort()`. The partial answer stays and is marked "stopped".
- The component aborts a running turn when it unmounts (a client-side navigation away), so the
  upstream stops too.
- The Send button is disabled while a turn runs, when the model field is empty, and when the
  composer is disabled (section 6.1).

## 7. Tests

### 7.1 Unit (vitest)

- `lib/chat-stream.ts`: a record split over chunks, a multi-byte character split over chunks,
  `[DONE]`, the `paigasus-error` event, a raw gateway error record (ignored), an end with no
  `[DONE]`, a comment line, and the pending-record bound.
- `lib/chat-route.ts`: 403 for origin, 415, 401, 413, 400 for body shapes, the three SDK arms (with
  an injected client), the injected `paigasus-error` event (terminal frame and source-stream
  error), the upstream-text replacement, the response headers, and a check that the gateway
  request holds only `model`, `messages` and `stream: true`.
- The SDK fields (section 5).
- Existing tests to update: `tests/unit/proxy.test.ts:52` (the public-path list gets
  `/api/chat`); `token-leak.spec.ts` also scans the new route's JSON and SSE responses.

### 7.2 E2E: the `playground` Playwright project

**Topology.** A third Playwright project, `playground`, in `gateway-console`, single-zone and with
no Docker. Its own worker fixture starts:

- its own in-process fake IAM;
- the **real** `paigasus-gateway` binary as `PAIGASUS_SERVICES.gateway`. It is configured only
  through `GATEWAY_*` environment variables (`config.rs:244-248`), started from a working directory
  that has no `gateway.toml`, and started with every inherited `GATEWAY_*` variable removed. This
  keeps the e2e read-only scan green;
- a mock OpenAI SSE server that the test controls step by step.

The existing `single-zone` and `two-zone` projects do not change. So R19, R20 and
`capabilities.spec.ts` keep their exact call sets.

**The fake IAM in this project** answers `authn.introspectApiKey` with `Unauthenticated`, reason
`invalid-token`, as real IAM does (`convert.rs:141`). The fake's default for this method becomes
that answer, and the gap is added to the fake's divergence list (`fake-iam.ts:25-42`). Without
this, the fake answers `Unimplemented`, the gateway maps it to an outage, and every e2e request
takes the 503 branch. `authz.isAuthorized` is scripted in every row.

**Current binary (the blocker).** The Rust `build` task declares no `outputs`
(`.moon/tasks/rust.yml:34-46`), and CI restores both `rs/target` (keyed only by lockfile and
toolchain) and `.moon/cache` (`ci.yml:81-120`). A Moon cache hit on `build` can therefore leave an
old binary. The change:

- adds a task `paigasus-gateway-rs:e2e-bin` that runs
  `cargo build --locked --bin paigasus-gateway` with `options.cache: false`, and with
  `/rs/.cargo/config.toml` in its inputs;
- makes `gateway-console-ts:test-e2e` depend on `paigasus-gateway-rs:e2e-bin`;
- adds to `gateway-console-ts:test-e2e`'s `inputs` the gateway's `src/**/*`, its `Cargo.toml`,
  `/rs/Cargo.lock`, and every path in the gateway's `fileGroups.upstreams`. A `deps` edge alone does
  not make a task affected on Moon 2.5.3, so without these inputs an edit to `auth.rs` does not
  select the e2e tier;
- makes row 1 assert the gateway child process's own `chat completion proxied` log line with
  `auth=oidc`. The row fails if anything other than the real binary serves the call.

**Rows** (registered as R22 and later in `tests/unit/e2e-rows.test.ts`):

1. **Stream, chunk by chunk.** The mock sends chunk 1 and waits. The test waits until the page shows
   chunk 1, then releases chunk 2. The test asserts the gateway log line (above). A buffering
   regression fails this row.
2. **Stop.** The mock streams without end. After Stop, the test asserts that the mock sees its
   connection close within a set time.
3. **Deny.** `isAuthorized` denies only when `action === 'InvokeModel'`. The test asserts the
   `insufficient-permissions` text in the playground's own error area.
4. **Self-query wiring.** The fake IAM records the `isAuthorized` call: the resource is the org PRN of
   the URL's UUID, the action is `InvokeModel`, the principal equals `iam.principalPrnFor(token)`,
   and the bearer is the session token.
5. **No session.** A request to `/gateway/api/chat` with a valid `Origin` and `content-type` and
   no session cookie gets exactly 401 JSON, not a redirect.
6. **Mid-record failure.** The mock breaks the connection inside a record. The page shows the
   `paigasus-error` text with a correlation id.

**Streaming-off row.** Row "composer disabled when the capability is absent" goes to the existing
`single-zone` project, with `gatewayDescriptor: { capabilities: [] }`. The composer rule reads only
discovery, and the Rust tests already cover `400 streaming-disabled`.

Org inference (D3) is tested only in the Rust unit tests. The e2e always sends the header.

### 7.3 Measurements the plan does first

These are open facts. The plan's first task measures them, and a failure changes the design before
other code is written:

1. A tonic client against the connect-node h2c fake IAM, including `ErrorInfo` in
   `grpc-status-details-bin`. No Rust client talks to the fake today.
2. Next 16.3.4's standalone server: does it abort `request.signal` when the client disconnects?
3. Next's response compression with `cache-control: no-transform` on a route-handler stream.
4. The added build time of `paigasus-gateway-rs:e2e-bin` in the e2e job.

## 8. Out of scope

- A console control that grants `gateway_user` to a person (D10). A follow-up issue owns it.
- A team- or project-level scope for the playground (D4).
- A model list, conversation storage, and system-prompt presets.
- A non-stream fallback (D7).
- A Docker-gated suite with real IAM and Keycloak for the gateway.
- Slug lookup in IAM (D2).
- A cache for the per-turn IAM RPCs (section 4.1).
- A rate limit or spend budget on the gateway (section 9).

## 9. Risks

- **The fake IAM is not Cedar.** The e2e proves the wiring (the right PRN, principal, action and
  bearer reach IAM, and a deny is shown). It does not prove the Cedar decision. The IAM crate's own
  tests own that decision.
- **Spend.** The gateway has no rate limit and no spend budget (`gateway.toml.example:4-6`). The
  playground opens model spend to every person who holds `gateway_user`. The grant is out of band
  (D10), so an org admin controls who can spend. A limit is not in this issue.
- **Build time.** `paigasus-gateway-rs:e2e-bin` runs uncached. Section 7.3 item 4 measures it.

## 10. Consequences to document

- D3: a header-less client starts to get `400 org-required` when its user joins a second org
  (gateway README and ADR).
- D5: an SDK user who sends `org` with an API key gets no error. The gateway logs a warning
  (section 4.1).
- D10: a person needs an out-of-band `GrantRole` before the playground works (console README and
  the playground's 403 text).

## 11. Changelog

**Revision 2** folds in the adversarial challenge (verdict: approve with changes).

Folded in:

- Blocker: the e2e could run a stale gateway binary, and a gateway edit did not select the e2e.
  Added `paigasus-gateway-rs:e2e-bin`, the `inputs`, and the log-line assertion (7.2).
- Blocker: no ordinary user holds `InvokeModel`. Added D10, the 403 text, the `roles.rs` row, and a
  follow-up issue (3, 4.8, 6.3, 8).
- The existing `auth.rs` rows and the unprovisioned guard need real token-leg outcomes (4.8).
- The fake IAM's `introspectApiKey` default sent every e2e request to the outage branch (7.2).
- The e2e topology was not specified and broke R19 and R20. Added the `playground` project (D9, 7.2).
- Five e2e rows could pass for the wrong reason. Each row is now gated or narrowed (7.2).
- Next's compression could buffer the stream. Added `no-transform` and `x-accel-buffering` (6.2).
- Mid-stream failures did not reach the browser as a `PaigasusError`. Added the `\n\n` prefix on
  the terminal frame (4.6), the source-error event and the parser rules (6.2, 6.3).
- The `paigasus-iam-core` dependency. Replaced with the kernel `Prn` API (4.2).
- Correlation ids did not join. Added the SDK `correlationId` field (5, 6.2).
- Minor: the `resolve_org` signature, the 36-character UUID rule, `authz_error` for OIDC reasons,
  the doc comments, the metric labels, `Credential::Oidc`, the registry facts and numbers, the
  `param` deviation, the status mapping, the body limit, the header timeout, the page loader and
  link, the degraded notice, the unmount abort, the history rule, the local failure codes, the
  upstream text, the existing test updates, the D3 and D5 consequences, and the per-turn cost.
- Questions: the tonic/h2c, `request.signal` and compression facts are measured first (7.3). The
  SDK refuses CR and LF in `org` (5). Spend is recorded as a risk and is out of scope (8, 9).

Not folded in: none. Every finding was justified. The spend question needs a human decision; this
revision puts it out of scope.
