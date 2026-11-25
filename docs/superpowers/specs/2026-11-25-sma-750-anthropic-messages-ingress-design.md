# SMA-750: Anthropic Messages ingress (`/v1/messages`)

Linear: SMA-750 (parent SMA-747, ADR-0027). Related: SMA-749 (the tool-call model and the
interceptor), SMA-752 (request-side policy), SMA-753 (history), SMA-755 (decision log), SMA-758
(the Anthropic oracle generator, which lands in this change), SMA-769 (the Responses ingress and
the generic pipeline), SMA-791 (prompt caching), SMA-855 (detectors).

This spec extends the SMA-749 spec (`2026-11-09-sma-749-tool-call-assembler-design.md`) and the
SMA-769 spec (`2026-11-14-sma-769-responses-ingress-design.md`). Every rule of those specs applies
to the new route unless this spec says otherwise. "749 § 4.3" points to them.

## 1. Intent

The gateway serves `/v1/chat/completions` and `/v1/responses`, both with an OpenAI upstream. A
client that speaks the Anthropic Messages API cannot use the gateway.

Two clients need it now:

- **Helikon** (the agent host). It uses an Anthropic SDK. SMA-791 needs Anthropic prompt caching
  (`cache_control`), which the OpenAI formats do not carry.
- **Claude Code.** An operator points it at the gateway with `ANTHROPIC_BASE_URL`.

ADR-0027 also requires tool-call interception on Anthropic Messages. The Anthropic stream and body
code exists in `paigasus-gateway-core`, but no route calls it (749 D26).

This change adds `POST /v1/messages` with the same authentication, rate limits, budget charge and
tool-call interception as the two OpenAI routes. It also adds `POST /v1/messages/count_tokens` and
`GET /v1/models` as plain pass-through routes, because Claude Code uses them.

**Success:** Claude Code and an Anthropic SDK work through the gateway without a change on the
client, and with no feature loss. Prompt caching, extended thinking, beta features and Claude
Code's own error recovery all work as they do against `api.anthropic.com`.

## 2. Sources read (2026-11-25)

- The code at `origin/main` `65f5a304`. The gateway is `rs/crates/services/paigasus-gateway`
  (below: GW). The core is `rs/crates/libs/paigasus-gateway-core` (below: CORE).
- Anthropic's gateway compatibility spec for Claude Code:
  `https://code.claude.com/docs/en/llm-gateway-protocol`, with `llm-gateway`,
  `llm-gateway-connect`, `network-config` and `errors` on the same site. Anthropic publishes no
  gateway source code. Its "Claude apps gateway" is closed source and is part of the `claude`
  binary. Only its documented behaviour was read.
- LiteLLM's `/v1/messages` route: docs and source (`litellm/proxy/anthropic_endpoints/`,
  `litellm/proxy/guardrails/guardrail_hooks/tool_permission.py`) and its incident posts.

### 2.1 What the gateway does today

- `pipeline::proxy` (GW `adapters/http/pipeline.rs`) is the shared flow. One `Endpoint` descriptor
  per route gives the parse, the two usage readers, the terminal frame and the interception limits.
- The upstream is the concrete `OpenAiClient` (GW `adapters/openai/client.rs`). It has no port
  trait. It sends `Authorization: Bearer <operator key>`. It never forwards a caller header.
  `UpstreamRoute` is a closed enum, so no request value can choose the upstream URL.
- The auth middleware reads only `Authorization` (`credentials()`, GW `adapters/http/auth.rs:627`).
- Every gateway error renders the OpenAI envelope (GW `adapters/http/error.rs`). This includes the
  auth middleware and the `EnvelopeBytes` extractor rejections (413, 400).
- `UsageScanner` and `ResponsesUsageScanner` read OpenAI usage only (`StreamUsage` enum,
  GW `adapters/http/usage.rs:82`).
- CORE has `Format::AnthropicMessages`: `AnthropicStream` (stream), `anthropic/body.rs`
  (non-stream), `parse_request` (request), and the policy rewrite (`policy/rewrite.rs:359`). The
  gateway's `interception::intercepted_stream` and `intercept_body` already take a `Format`.
- While a tool call is held, the gateway emits the SSE comment `: keep-alive` every
  `keepalive_secs` (GW `adapters/http/interception.rs:339`).
- `[upstream.openai]` is required. There is no `[upstream.anthropic]`.

### 2.2 What Claude Code requires of a gateway (Anthropic's spec)

Hard requirements:

- R1. `POST /v1/messages`. Claude Code sends `/v1/messages?beta=true`. Match the path, not the URL.
- R2. Forward `anthropic-version`, `anthropic-beta` and every other `anthropic-*` request header
  unchanged. Do not allowlist beta values. The set changes with every Claude Code release.
- R3. Forward every body field unchanged, `cache_control` included. A beta header and its body
  field travel together. If only one of them arrives, the answer is a 400.
- R4. Do not buffer the stream. Keep `ping` events. Send the full event set through
  `message_delta` and `message_stop`. Claude Code has a 300 s byte-level idle timer and a 300 s
  event-level idle timer. An SSE comment resets only the byte-level timer.
- R5. Forward upstream error bodies unchanged. Claude Code recovers from some 400s by reading the
  error text (for example `prompt is too long` starts auto-compact). A changed body breaks that.
- R6. `retry-after` is integer seconds. A value over 60 makes Claude Code stop and show the error.
  Forward `x-should-retry` from the upstream.

Optional:

- O1. `POST /v1/messages/count_tokens`. Without it, Claude Code uses a character estimate.
- O2. `GET /v1/models?limit=1000`. Claude Code calls it only with
  `CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY=1`. Timeout 3 s. A redirect is a failure. Claude
  Code keeps only ids that contain `claude` or `anthropic`. Without it, `/model` uses a cached or
  built-in list.

Authentication: `ANTHROPIC_API_KEY` sends `x-api-key`. `ANTHROPIC_AUTH_TOKEN` sends
`Authorization: Bearer`. `apiKeyHelper` sends the same value in both headers.

The spec also says not to rewrite `system`, `tools` or earlier `messages`. A rewrite invalidates
the prompt cache, and it can make Anthropic reject a thinking signature with a 400 ("bound to a
different conversation").

### 2.3 What LiteLLM teaches

- Its Claude Code failures came from allowlists of beta headers and body fields, from changed
  error bodies, and from `count_tokens` calls that went to a fixed host with the wrong key. This
  design avoids all three.
- Its tool guardrail buffers the whole stream before it releases any byte. Our interceptor holds
  only a tool call's frames (749). Our design is the one to keep.
- It had a bug where a stray `Authorization` header hid a valid `x-api-key`.
- It charges a cut-off stream from the `message_start` and `message_delta` events that arrived.

## 3. Threat model and invariant

749 § 2 applies unchanged: T1 hostile model output, T2 a non-conforming upstream, T3 the caller is
not an adversary of its own agent. Invariant I1 applies to the Anthropic format as 749 § 4 states
it, for every client model in the oracle (CORE `tests/support/oracle.rs`, `ANTHROPIC_RUNNERS`).

New for this route:

- **Egress hygiene, Anthropic form (I2).** The upstream request carries the operator's key and
  only these caller values: the body bytes (or the policy-rewritten bytes, D9), the query string,
  and the `anthropic-*` request headers. No other caller header reaches the upstream. The caller's
  `x-api-key` and `Authorization` never reach it.
- **The upstream URL is not caller-controlled (I3).** The path comes from a closed enum. The query
  string is appended to that path and cannot change the host or the path. The client never follows
  a redirect (D16).

Known gaps that this change does not close (stated so that ADR-0027 coverage is not overstated):

- **Server tools and the MCP connector are not enforceable.** `server_tool_use`, `mcp_tool_use`
  and their results run at Anthropic. The gateway only reports them as `unenforceable` (CORE
  `anthropic/stream.rs:216-217`, `anthropic/body.rs:31-32`). The rewrite never removes a server
  tool definition, and `mcp_servers` passes (R3). A Cedar deny of a server tool has no effect on
  this route. A control for them is a later sweep.
- **Caller-chosen features can raise the operator's cost** with no budget effect: premium betas
  (for example long context), `service_tier`, per-use server tools. The budget counts tokens only
  (D10). The OpenAI routes have the same property for `service_tier`. Cost control is M4. This
  change logs the `anthropic-beta` values of each request (§ 9), so an operator can see them.
- **Shared prompt cache.** All tenants use one operator key. `cache_read_input_tokens` can show
  that another tenant sent the same prefix. The OpenAI routes have the same property. Accepted.
- **Fail-closed on unknown wire shapes (I1 kept).** An unknown event type is dropped, an unknown
  delta on a text block is a deviation, and an unknown block type is held and decided as a tool
  call (`Kind::Other`) (CORE `anthropic/stream.rs:172`, `:213-220`, `:312-313`). This is the 749
  I1 rule. It can conflict with "no feature loss" when Anthropic adds a new type. This change keeps
  I1, records and tests every event and block type that Claude Code produces today (§ 10.1), and
  adds a runbook alert for `gateway_tool_interception_records_dropped_total{reason="malformed"}` on
  this route. It applies only when interception is on.

## 4. Decisions

| # | Decision |
|---|---|
| D1 | **Direct Anthropic upstream.** No translation to the OpenAI upstream. A translation loses `cache_control`, thinking signatures and beta features. No canonical internal model in this change; the M1 decision stays open. |
| D2 | **A separate `AnthropicClient` adapter** in GW `adapters/anthropic/`, beside `adapters/openai/`. No `ProviderAdapter` trait (that is M1 routing). The reqwest builder, the CA-bundle load and the error chain move into a small shared helper that both clients use. The OpenAI behaviour does not change. |
| D3 | **Config.** A new optional `[upstream.anthropic]` table: `base_url` (default `https://api.anthropic.com`), `api_key` (from `GATEWAY_UPSTREAM__ANTHROPIC__API_KEY` only, never serialized, an empty value is a boot failure), `extra_ca_bundle_path` (ADDS to the trust store, as the OpenAI knob; this is the sixth CA knob, and `rs/CLAUDE.md` changes "five" to "six"), `default_version` (default `2023-06-01`; a value that is not a valid header value is a boot failure), `non_stream_timeout_secs` (default 600; a new name, so that it does not repeat the top-level `first_byte_timeout_secs`). A new `[messages]` table: `enabled` (default `false`; the three routes are a 404 when off), `max_request_bytes` (default 33 554 432, Anthropic's own request limit), and the D15 limits. `enabled = true` without `[upstream.anthropic]` is a boot failure. `[upstream.openai]` stays required. The detector default `max_scan_bytes` is "the largest request cap of any route" (GW `config.rs:801-812`), so it becomes the maximum over all enabled routes, and its test covers `[messages]`. `/readyz` docs name both upstreams. |
| D4 | **Routes.** `POST /v1/messages` (the pipeline), `POST /v1/messages/count_tokens` and `GET /v1/models` (pass-through, D11). All three are behind `require_iam_auth` and the `[messages] max_request_bytes` body limit. |
| D5 | **Request headers.** Forward every caller header whose name starts with `anthropic-` (case-insensitive), with all its values, in order. If the caller sends no `anthropic-version`, send `default_version`. Never forward `x-api-key`, `authorization`, `dpop`, `paigasus-*`, `x-claude-code-*` or any other header. Forward the raw query string unchanged (`?beta=true`). |
| D6 | **Client authentication.** On the three routes, `x-api-key: <value>` is read as `Authorization: Bearer <value>`. If both headers are present, `x-api-key` wins, and `Authorization` is ignored. An empty `x-api-key`, or two `x-api-key` headers, is `MissingBearer` (401); the message on these routes names both headers. The switch is a flag on `AuthState` for the Anthropic route group; `credentials()` takes it as an argument. DPoP does not apply to `x-api-key`: a DPoP-bound token sent in `x-api-key` is refused by IAM on the Bearer scheme (IAM `adapters/oidc/validator.rs:526-536`), and a test proves the 401. The runbook says that a stale `ANTHROPIC_API_KEY` wins over `ANTHROPIC_AUTH_TOKEN`. The two OpenAI routes do not change. |
| D7 | **Gateway errors in the Anthropic envelope.** On the three routes, every error that the gateway itself makes (auth middleware, body extractor, pipeline, limits, policy) renders as `{"type":"error","error":{"type":<anthropic type>,"message":<text>,"code":<registry kebab code>},"request_id":<paigasus request id>}`. The mapping is in § 6. Upstream error bodies are never re-rendered (R5). Mechanism: § 6.2. |
| D8 | **Response headers.** Forward the upstream `request-id`, `retry-after` and `x-should-retry` on every answer (2xx and non-2xx, stream and body). Do not forward `anthropic-ratelimit-*`: they describe the operator's shared quota, not the caller's quota. The Claude apps gateway removes them for the same reason. No other upstream header is forwarded. |
| D9 | **The request-side policy rewrite stays on.** In enforce mode, SMA-752 removes denied tool definitions and replaces a blocked `tool_result` with a notice (CORE `policy/rewrite.rs`). This happens only when a policy denies a tool. The cost is one prompt-cache miss, and possibly a 400 from Anthropic for a thinking signature. That 400 passes to the client unchanged (R5), and Claude Code then retries without thinking. Sven's choice (option a). |
| D10 | **Budget charge.** The charge is `input_tokens + cache_creation_input_tokens + cache_read_input_tokens + output_tokens`. This is the same unit as OpenAI `total_tokens`, which includes cached tokens. Cost weighting is M4. § 7 gives the readers. |
| D11 | **`count_tokens` and `models` are plain pass-through.** Same auth, same `Limits::admit_for` (request rate and budget check), zero token charge, no interception, no policy, no detectors. Same upstream base URL and key as `/v1/messages`. They do not use `pipeline::proxy` and have no `Endpoint` descriptor: one small handler does auth (middleware), `admit_for` with the guard marked no-charge, the send with the D5 `Forward`, and the D8 response headers. The body is not parsed (`count_tokens` forwards it as it is). Accepted consequence: `count_tokens` counts tool definitions that the policy would remove, and sends `tool_result` content that the policy would replace. Each call uses one unit of the request rate; § 10.4 measures how often Claude Code calls it. | `GET /v1/models` shows every model of the operator's Anthropic account, because the gateway has no per-key model allowlist yet (that is the routing work). |
| D12 | **Keep-alive on an Anthropic stream is a `ping` event.** While a tool call is held on a `Format::AnthropicMessages` stream, the keep-alive frame is `event: ping\ndata: {"type":"ping"}\n\n`, not `: keep-alive`. A comment does not reset Claude Code's event-level timer (R4). The OpenAI formats keep the comment. |
| D13 | **The terminal frame** for a mid-stream upstream failure is `\n\nevent: error\ndata: {"type":"error","error":{"type":"api_error","message":"upstream stream error","code":"upstream-error"}}\n\n`. The leading blank line closes a partial record, as on the other routes. |
| D14 | **SMA-758 lands in this change**, because its own note says "before or with SMA-750" and this route makes the Anthropic interceptor reachable from outside. |
| D15 | **Streaming** obeys the shared `stream_enabled` capability, as the Responses route does (769 D9). **Interception limits are per route**, as 769 D15: `[messages] max_record_bytes` (default 8 MiB), `max_held_bytes` (default 32 MiB) and `max_hold_secs` (default 600). A server-tool result (for example a base64 PDF from `web_fetch`) arrives whole in one `content_block_start`, and a large Claude Code `Write` call streams for minutes. Task 1 measures both, and the defaults change if the recordings say so. The `Endpoint.limits` function applies them. |
| D16 | **No redirects.** The shared reqwest helper sets `redirect::Policy::none()` for both clients. reqwest removes only `Authorization` on a cross-host redirect and keeps `x-api-key` (reqwest 0.13.5 `redirect.rs:239-250`), so a followed redirect would send the operator key to another host. An upstream 3xx becomes `UpstreamUnavailable` (502). `Location` is never forwarded. |
| D17 | **Upstream `error` event mid-stream.** On `Format::AnthropicMessages`, an upstream `error` record that arrives while a hold is open is emitted as the terminal frame, after the closing frames, in place of D13. Today `Interceptor::abort` replaces it (CORE `interceptor.rs:287-290`, `:163-174`), and that breaks R5 (Claude Code treats `overloaded_error` differently). D13 is used only for a transport failure. This is a CORE change. |
| D18 | **Strict request parse: Anthropic data keys.** `parse_strict` relaxes the envelope key rules only below `input`, `input_schema` and `parameters` (CORE `json.rs:117`). For Anthropic requests, the subtrees that hold user data are also data keys: the structured-output schema (`output_format` and `output_config.format`, whichever the recordings show), `tools[].input_examples`, and any other user-data subtree found in Task 1. This follows 769's `RESPONSES_DATA_KEYS` (`json.rs:162`). Without it, an enforced policy refuses valid requests with a 400. |
| D19 | **Duplicate keys**, as 769 built it (GW `responses.rs:92-102`): a duplicate top-level key is refused in both modes (`BadRequestBody`). With interception or the policy on, a duplicate key at any depth is refused. Otherwise the gateway could read `stream:false` while Anthropic reads `stream:true`. |
| D20 | **`GET /v1/models` only for Anthropic callers.** The route serves the Anthropic list only when the request has an `anthropic-version` header. Claude Code and the Anthropic SDKs always send it. Without it the answer is a 404 in the OpenAI envelope, as today, so OpenAI SDK callers (`models.list()`) see no change. |
| D21 | **Recordings are scrubbed.** The repository is public. A recorded fixture keeps only `content-type`, `request-id` (replaced by a fixed value), `retry-after` and `x-should-retry`. A test fails when a fixture holds any other header or an `sk-ant-` string. |

## 5. Upstream client (D2, D3, D5, D8)

`AnthropicClient` has the shape of `OpenAiClient`:

```rust
pub enum AnthropicRoute { Messages, CountTokens, Models }   // closed set (I3)

pub struct Forward { pub query: Option<String>, pub headers: Vec<(HeaderName, HeaderValue)> } // D5 result

impl AnthropicClient {
    pub async fn send(&self, route: AnthropicRoute, method: Method, body: Bytes, stream: bool, fwd: &Forward)
        -> Result<UpstreamResponse, UpstreamError>;
}
```

- Headers it sets: `x-api-key: <operator key>`, `content-type: application/json` (not on `GET`),
  `accept` (SSE for a stream, JSON otherwise), then the `fwd.headers`. A forwarded header never
  replaces `x-api-key`, `content-type` or `accept`, because D5 forwards only `anthropic-*` names.
- `UpstreamResponse` is today's `ChatResponse` (`Full` or `Stream`) plus the D8 response headers.
  The builder may rename `ChatResponse` and `OpenAiError` to provider-neutral names, or wrap them.
  The pipeline must not branch on the provider beyond the `send` call.
- Timeouts: `connect_timeout_secs` and `stream_idle_timeout_secs` are the shared top-level keys.
  The non-stream deadline is `[upstream.anthropic] non_stream_timeout_secs` (600 s). An Anthropic
  non-stream answer with extended thinking can take minutes; the OpenAI 30 s default would cut it.
  reqwest's `read_timeout` (set from `stream_idle_timeout_secs`, default 300) also runs until the
  response head arrives, so it would fire first. The Anthropic client therefore builds its own
  `reqwest::Client` with `read_timeout = max(stream_idle_timeout_secs, non_stream_timeout_secs)`.
  A test holds the mock's response head for longer than `stream_idle_timeout_secs`.
- Redirects: none (D16). The shared helper takes the config key name for its CA-bundle error and
  log texts, so the Anthropic errors name `upstream.anthropic.extra_ca_bundle_path` and the OpenAI
  texts stay byte-identical (the test at GW `adapters/openai/client.rs:496-512`).
- If an `Upstream::Anthropic` descriptor runs while the client is not configured, the answer is
  `Internal` (500). Boot validation (D3) makes this unreachable.
- Errors map as `OpenAiError` does: a timeout is `UpstreamTimeout` (504), any other failure is
  `UpstreamUnavailable` (502). A non-2xx upstream answer is a `Full` response, forwarded unchanged.
  A `529 overloaded_error` is forwarded unchanged.
- `Debug` never prints the key (the same test as `debug_never_leaks_the_api_key`).

The `Endpoint` descriptor gets one new field that names its upstream, for example
`upstream: Upstream::OpenAi(UpstreamRoute::ChatCompletions)` or
`Upstream::Anthropic(AnthropicRoute::Messages)`. `pipeline::proxy` gets the `Forward` value from
the handler (empty for the OpenAI routes) and passes it to the Anthropic client only.

## 6. Errors (D7, D13)

### 6.1 Mapping

| Gateway status | Anthropic `error.type` |
|---|---|
| 400 | `invalid_request_error` |
| 401 | `authentication_error` |
| 403 | `permission_error` |
| 404 | `not_found_error` |
| 413 | `request_too_large` |
| 429, `RateLimited` | `rate_limit_error` |
| 429, `BudgetExhausted` | `billing_error` |
| 500, 502, 503 | `api_error` |
| 504 | `timeout_error` |

- `message` is the same text as the OpenAI envelope. `code` is the same registry kebab code. No
  new registry code is needed. If the builder finds one is needed, it goes into
  `contracts/proto/paigasus/common/v1/error.proto` first.
- The status code and the headers (`paigasus-retryable`, `x-should-retry`, `WWW-Authenticate`) do
  not change. `RateLimited` keeps `retry-after` (integer seconds, at least 1).
- `BudgetExhausted` on the Anthropic routes also sends `retry-after` with the integer seconds
  until `resets_at_unix`, at least 61. A value over 60 makes Claude Code stop at once and show the
  error (R6). The OpenAI routes stay as they are (no `retry-after` on a budget refusal). The
  seconds are computed in `Limits` with its injected clock (as `charge_guard.rs:166` uses
  `limits.clock()`) and carried in the refusal, so the renderer stays pure.
- `ToolInterceptionMalformedResponse` (502) on the Anthropic routes also sends
  `x-should-retry: false`. Claude Code retries a 5xx, and each retry is charged again, while the
  same upstream answer is likely to fail the same way.
- `request_id` is the `paigasus-request-id` of the request.
- Exceptions. No `GatewayError` maps to 404 or 405. An axum 404 (route off) or 405 (for example
  `GET /v1/messages`) on these paths gets the Anthropic envelope from a route-group fallback, with
  `not_found_error` and `invalid_request_error` and no registry code.

### 6.2 Mechanism (builder's choice, with one constraint)

The auth middleware and the `EnvelopeBytes` extractor render errors before the handler runs, so
the envelope cannot be chosen in the handler. A suggested mechanism: `GatewayError::into_response`
also puts the `GatewayError` value into the response extensions. A `map_response` layer on the
three Anthropic routes re-renders any response that carries that extension. An upstream answer
never carries it, so it passes unchanged. The constraints:

- An upstream error body must never be re-rendered (R5), and a test must prove it.
- The re-render replaces only the body and its `content-length`. It keeps the status and every
  header. Headers are added after `into_response` today: `WWW-Authenticate` in the auth
  middleware (GW `auth.rs:123-130`) and `paigasus-tool-decisions` in the pipeline
  (GW `pipeline.rs:133-135`, `:158-160`). A test proves both survive.

The `repo:http-extractor-envelope` and `repo:error-code-single-site` gates apply. A new file that
spells a registry code needs a `ci/error-registry/check.py` MANIFEST row and a membership test.

## 7. Usage and the charge (D10)

### 7.1 Non-stream

Read `usage` from a COPY of a 2xx body. The total is the sum of the four D10 fields. A missing
field counts as 0. If `usage` is missing or is not an object, the charge is the completion
estimate: `ceil(len/4)` of the `text` of `text` blocks and the compact JSON of `tool_use.input`.

### 7.2 Stream

A new `StreamUsage::Anthropic(AnthropicUsageScanner)` reads a COPY of the forwarded SSE, as the two
OpenAI scanners do (record splitting as `UsageScanner`, which handles CR, LF and CRLF):

- The scanner dispatches on the top-level `data.type` of each record only, never on a substring.
- `message_start`: read `message.usage`: the three input fields and `output_tokens`.
- `message_delta`: read `usage`. Each field present there replaces the earlier value. Anthropic
  reports cumulative values in `message_delta`. A `null` field counts as absent.
- Before any `message_delta`, `reported_total()` is
  `input sum + max(output_tokens, complete data records since message_start)`. After a
  `message_delta`, it is the exact D10 sum. So a stream cut after `message_start` charges the
  exact input and an estimated output, and `ChargeGuard` needs no change (it uses the reported
  total once it is `Some`, `charge_guard.rs:127-133`).
- If `message_start` never arrived, `reported_total()` is `None`, and the charge is the request
  estimate plus the completion estimate, as today.

### 7.3 Request estimate

The estimate reads `system` (a string or an array of blocks) and `messages[].content` (a string or
an array of blocks). Text counts `ceil(bytes/4)`. Each non-text block (`image`, `document`, and so
on) counts 85, as today. `tool_use.input`, `tool_result.content` and the `tools[]` definitions
count as text (Claude Code sends about 15 000 to 20 000 tokens of tool definitions). The estimate
is at least `min(ceil(body_len/4), 32 768)`, so that a caller who disconnects before
`message_start` does not pay too little. The fallback for a body that does not parse is the same
`min(ceil(body_len/4), 32 768)`, as today.

## 8. The stream and the body

- `/v1/messages` uses `pipeline::proxy` with `Format::AnthropicMessages`. The interceptor, the
  request-side policy, history, detectors and the decision log run as they do for the other
  formats. CORE changes are D12 (in `Interceptor::tick`, which emits the keep-alive from CORE
  `sse.rs:33` and `interceptor.rs:207-208`), D17, D18 and the SMA-758 generator work.
- The parse (`parse_messages`) reads `model` (string, required), `stream` (bool, default false)
  and `messages` (array, required) from a COPY, with the D19 duplicate rule. The failure split is
  SMA-588: a syntax failure is `BadRequestBody`, a type failure is `InvalidRequestSchema`.
- A non-2xx stream request is forwarded as its body, with no charge, as today.
- `content-type` is `text/event-stream` for a 2xx stream. A non-2xx answer keeps the upstream
  `content-type` (today the pipeline always writes `application/json`, GW `pipeline.rs:220`,
  `:275`; the OpenAI routes do not change).
- A `max_tokens` cut inside a `tool_use` block gives `incomplete_arguments` (CORE
  `anthropic/stream.rs:325-326`), and the client gets a `(malformed)` notice. This is accepted:
  the call was not complete, so no client could run it.
- `anthropic-version`: the gateway forwards the caller's value. The stream shape is the same for
  every version that Anthropic accepts today (`2023-06-01` is the only one). If Anthropic adds a
  version that changes the stream, the interceptor reports deviations (fail-closed, § 3).

## 9. Observability

- `endpoint` label and log field: `messages`, `messages_count_tokens`, `models`. The HTTP metrics
  layer covers the routes by its `route` label. No new metric family is needed.
- The per-request log line stays as it is (no body, no key). On `/v1/messages` it also carries the
  `anthropic-beta` values of the request (§ 3, cost visibility).
- The `endpoint` label is `messages`, and the decision-log ingress label stays
  `anthropic_messages` (CORE `policy/mapping.rs:19`). The runbook documents the mapping.
- Dashboards that filter `endpoint="chat_completions"` do not show the new routes. The builder
  checks `ops/observability/grafana/dashboards/**` and adds the new values where a panel lists
  endpoints.

## 10. Testing

### 10.1 Recordings (the first task)

Record real Anthropic answers with an operator key, the way SMA-757 and SMA-769 recorded real
streams. Sven supplies the key (decision of 2026-11-25) in a file outside the repository's `.env*`
files, because the agent may not read those. The key is never printed into a log, a fixture or the
PR. The recordings are the first task, and the mock shapes and the fixtures are built from them.
The SMA-758 task (§ 10.3) does not depend on them and may run in parallel. Record at least:

- a text stream, a stream with parallel `tool_use` blocks, a stream with `thinking` blocks;
- a `tool_use` with more than 40 KB of input (for D15's hold time);
- a server-tool stream (`web_search` or `web_fetch`) (for D15's record size);
- a `max_tokens` cut inside a `tool_use` block;
- a non-stream answer with `tool_use`, a 400 (`prompt is too long`), a 429 with its headers, and a
  mid-stream `error` event if one can be provoked;
- a `count_tokens` answer and a `GET /v1/models` answer;
- the `anthropic-beta` values and the event, block and delta types of one real Claude Code session
  through a local gateway (§ 3, fail-closed), and the request bodies' user-data subtrees (D18).

Record the response headers, scrubbed by D21. Also measure D9's risk once: send a follow-up turn
after a blocked `tool_use` that came after a `thinking` block, once with all calls blocked and
once with one blocked and one allowed, and record whether Anthropic answers 200 or 400. A case
that cannot be recorded is listed as not measured in the PR.

### 10.2 Service tests (GW `tests/`)

- Extend `MockOpenAi` (or add a `MockAnthropic`) in `tests/support/mod.rs` with the three
  Anthropic paths. It records the request headers, the path and the query.
- `messages_proxy.rs`, modelled on `chat_proxy.rs`: 401 with no credential, `x-api-key` accepted,
  `Authorization: Bearer` accepted, both present with `x-api-key` winning, verbatim body
  pass-through, streaming order, the D13 terminal frame, `stream_enabled` off, the route is a 404
  with `[messages] enabled = false`, client abort cancels the upstream.
- Egress (I2): the upstream sees the operator's `x-api-key`, every `anthropic-*` header with all
  its values, the default `anthropic-version` when the caller sent none, the query string, and no
  other caller header. The caller's `x-api-key`, `Authorization` and `paigasus-org` never arrive.
- Errors: every gateway error on the three routes is in the Anthropic envelope (auth 401,
  extractor 413, schema 400, rate 429, budget 429 with `retry-after` over 60). A non-2xx upstream
  body arrives byte-identical (R5). The D8 headers pass, and `anthropic-ratelimit-*` does not.
- Limits: the D10 charge from a non-stream body, from a full stream, and from a stream cut after
  `message_start`. `count_tokens` and `models` charge 0.
- Upstream hardening: a mock that answers 307 to a second mock host, and the second host gets no
  request (D16). A mock that holds the response head for longer than `stream_idle_timeout_secs`
  on a non-stream request, and the answer arrives (§ 5). A duplicate top-level `stream` key is
  refused (D19). `GET /v1/models` without `anthropic-version` is a 404 in the OpenAI envelope
  (D20). The re-render keeps `WWW-Authenticate` and `paigasus-tool-decisions` (§ 6.2). An axum 405
  on `GET /v1/messages` is in the Anthropic envelope. A fixture with an extra header or an
  `sk-ant-` string fails the scrub test (D21).
- Interception: a blocked `tool_use` through `/v1/messages`, stream and body. An upstream
  `error` event during a hold reaches the client unchanged (D17), and a transport failure gives
  the D13 frame. Each event, block and delta type from the Task 1 Claude Code recording passes
  with interception on (§ 3). A request with a user-data key such as `Name` under each D18
  subtree passes an enforced policy. The D12 `ping`
  keep-alive while a call is held. The existing "no Anthropic route" notes in
  `tests/tool_policy.rs:7`, `tests/tool_history.rs:6`, `interception.rs:1026` and `lib.rs` become
  real tests or are removed.

### 10.3 CORE (SMA-758)

SMA-758 is its own task and its own commit, before the route tasks. It does not depend on the
route, and the route does not depend on it. The text of SMA-758, copied from Linear on 2026-11-25:

> The proptest generator for Anthropic Messages streams in `paigasus-gateway-core` covers fewer
> shapes than the OpenAI one. Gaps to close:
>
> - More quirks: split `partial_json` at odd boundaries, `ping` frames inside a held block, and
>   `message_delta` arriving before the last `content_block_stop`.
> - Parallel `tool_use` blocks mixed with `text` and `thinking` blocks.
> - Multi-quirk scenarios in observe mode (`on_failure = "allow"`). Today they are only checked for
>   the output bound (SMA-749 ruling D9(a)).
>
> Done when:
>
> - The Anthropic P1 and P2 properties run over the wider generator at the same 10 000 cases.
> - The mutation check (a pass-through "interceptor") still fails P1 on the Anthropic side.
>
> Generator and properties: `rs/crates/libs/paigasus-gateway-core/tests/properties.rs` and
> `tests/support/` (`streams.rs` `AnthropicScenario`, `AQuirk`, `render_anthropic`).

If the wider generator finds a real interceptor defect, a fix in CORE of about one task is part of
this change. A larger fix stops the task, and the coordinator asks Sven before it continues.

### 10.4 Claude Code conformance (manual, in the PR)

Run a real Claude Code against a local gateway with `ANTHROPIC_BASE_URL`, once with
`ANTHROPIC_API_KEY` and once with `ANTHROPIC_AUTH_TOKEN`. Check: a session with tool calls,
`/context` (count_tokens), `/model` with `CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY=1`, and a
turn that uses prompt caching (`cache_read_input_tokens` above 0 on the second turn). With a
policy that blocks one tool, check that the session continues after the block, and that Claude
Code recovers if Anthropic answers the D9 400. Count the `count_tokens` calls per session. Record
the result in the PR. This is not a CI test.

## 11. Repo gates this change touches

- `repo:error-code-single-site`: a MANIFEST row for each new file that spells a code.
- `repo:http-extractor-envelope`: the new handlers use `EnvelopeBytes` (or a new extractor with an
  ALLOW row for its definition file).
- `repo:observability-drift`: only if a dashboard changes.
- `repo:affected-smoke`: no new crate is planned. A new crate needs a `members` line and the
  `ci/affected-graph/run.sh` set.
- Docs: `GW/gateway.toml.example` gets the two new tables. `GW/src/lib.rs` drops the "only chat is
  wired" note. `docs/ops/` runbooks get the Claude Code setup: `ANTHROPIC_BASE_URL`, the key
  (`ANTHROPIC_API_KEY` wins over `ANTHROPIC_AUTH_TOKEN`), `ANTHROPIC_CUSTOM_HEADERS` for
  `paigasus-org` (an OIDC user in several organizations), the `apiKeyHelper` TTL for OIDC tokens
  that expire, the note that `CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS` is not needed with a direct
  Anthropic upstream, the § 3 gaps, and the malformed-record alert.

## 12. Acceptance criteria

1. With `[messages] enabled = true`, `POST /v1/messages` proxies to Anthropic, stream and
   non-stream, with the same auth, limits and budget as the OpenAI routes.
2. I2 and I3 hold, proved by the egress tests, the redirect test included.
3. Every gateway error on the three routes is in the Anthropic envelope. Every upstream error body
   arrives unchanged.
4. The budget charge follows D10 and § 7, including a cut-off stream.
5. Tool-call interception works through `/v1/messages`, stream and body, with the D12 keep-alive.
6. `count_tokens` and `models` pass through with zero charge.
7. SMA-758's "Done when" holds.
8. The § 10.1 recordings exist, or the PR lists each case that is not measured.
9. The § 10.4 Claude Code check is recorded in the PR.

## 13. Non-goals and left to the builder

Non-goals:

- Translation between formats. Multi-provider routing, failover, a `ProviderAdapter` trait, a
  canonical model (M1).
- A per-key model allowlist, and filtering `GET /v1/models`.
- Cost-weighted budgets (M4). Prompt-cache-aware pricing.
- Bedrock or Vertex upstreams. A claude.ai OAuth login passed through the gateway.
- Our own `anthropic-ratelimit-*` headers for the caller's quota. (A later sweep.)
- Use of `x-claude-code-session-id` for SMA-753 session linking. (A later sweep.)
- Claude Code telemetry stubs (`/api/event_logging/batch`, `HEAD /api/hello`). They are a 404.
- Making `[upstream.openai]` optional for an Anthropic-only deployment.

- A control for server tools and `mcp_servers`, and an operator denylist for betas (§ 3 gaps; a
  later sweep).

Left to the builder:

- The names of the shared client helper, `UpstreamResponse`, `UpstreamError` and `Forward`.
- The error-envelope mechanism of § 6.2, within its constraints.
- The test file layout.

## 14. Spec challenge (2026-11-25) — changelog

The Opus spec challenger gave "approve with changes": 1 blocker, 11 major, 16 minor, 7 questions.

Folded in:

- BLOCKER, redirects leak `x-api-key` (verified in reqwest 0.13.5 `redirect.rs:239-250`): D16, I3.
- MAJOR, the 300 s `read_timeout` cuts the 600 s non-stream deadline: § 5 (own client).
- MAJOR, shared interception limits are too small for Anthropic payloads: D15 per-route limits.
- MAJOR, an upstream `error` event during a hold is replaced (verified at CORE
  `interceptor.rs:287-290`): D17.
- MAJOR, unknown types and I1: kept fail-closed, stated in § 3, recorded and tested, with an alert.
- MAJOR, the strict parse refuses user-data keys: D18.
- MAJOR, the 769 duplicate-key rule is missing: D19.
- MAJOR, server tools and MCP are not enforceable: stated as a gap in § 3, no control now.
- MAJOR, caller-chosen features raise cost: stated in § 3; the beta values are logged (§ 9).
- MAJOR, the re-render loses headers and needs a clock: § 6.1, § 6.2.
- MAJOR, `GET /v1/models` collides with OpenAI SDKs: D20.
- MAJOR, recordings can publish account data: D21.
- MINOR: the simpler cut-off charge (§ 7.2), the D6 cases, 404/405 (§ 6.1), `timeout_error` and
  `x-should-retry: false` on the 502 (§ 6.1), `count_tokens` and the policy (D11), the detector
  cap (D3), config validation and the key rename (D3), the helper's OpenAI strings (§ 5), the
  estimate floor and `tools[]` (§ 7.3), the label mapping (§ 9), the keep-alive citation (§ 8),
  the SMA-758 bound (§ 10.3), more recordings (§ 10.1), the D9 check in § 10.4, the runbook gaps
  (§ 11), the upstream `content-type` (§ 8), the unconfigured-client fallback (§ 5).
- QUESTIONS answered in the spec: the cache side channel (§ 3, accepted), the `anthropic-version`
  shape (§ 8), the `max_tokens` cut (§ 8), the `count_tokens` rate (D11, § 10.4).

Not acted on:

- An operator `deny_betas` list and a server-tool or MCP control: real, but new policy surface.
  The gaps are stated in § 3 and go to a later sweep.
- "Which `anthropic-*` header changes billing or workspace": no header is known to do this on an
  API-key upstream (`anthropic-workspace-id` applies only to Claude Platform on AWS). The § 10.1
  Claude Code recording lists the headers that arrive.
- "`billing_error` with 429 versus 402": kept at 429, as the Claude apps gateway does, so that the
  status matches the OpenAI routes. The `retry-after` over 60 makes Claude Code stop at once.
- "Which duplicate key Anthropic reads": D19 refuses duplicates, so the answer does not matter.
