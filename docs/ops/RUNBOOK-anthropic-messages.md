# RUNBOOK: Anthropic Messages ingress (SMA-750)

This runbook is for the platform operator. It explains how to turn on `POST /v1/messages` in the
AI Gateway. It also explains how to connect Claude Code, which gaps remain, and what to do when
the malformed-record alert fires.

The gateway has three Anthropic routes: `POST /v1/messages`, `POST /v1/messages/count_tokens`
and `GET /v1/models`. All three forward to the Anthropic API with the key of the operator.
The gateway does not translate the request to the OpenAI format.

## 1. Configuration

The routes are off by default. With `[messages] enabled = false`, the three routes answer 404.

### 1.1 Table `[upstream.anthropic]`

| Setting | Default | Meaning |
|---|---|---|
| `base_url` | `https://api.anthropic.com` | The Anthropic API origin. |
| `api_key` | none | The key of the operator. Set it only with `GATEWAY_UPSTREAM__ANTHROPIC__API_KEY`. An empty value stops the boot. |
| `extra_ca_bundle_path` | none | Extra root certificates. They are ADDED to the trust store. They do not replace it. |
| `default_version` | `2023-06-01` | The `anthropic-version` value. The gateway sends it when the caller sends none. |
| `non_stream_timeout_secs` | `600` | The time limit of one call that is not a stream. |

### 1.2 Table `[messages]`

| Setting | Default | Meaning |
|---|---|---|
| `enabled` | `false` | Turns on the three routes. It needs `[upstream.anthropic]`. Without it, the boot fails. |
| `max_request_bytes` | `33554432` | The largest request body. A larger body gets a 413. |
| `max_record_bytes` | `8388608` | The largest single SSE record. |
| `max_held_bytes` | `33554432` | The largest total of held bytes of one stream. |
| `max_hold_secs` | `600` | The longest hold of one tool call. The maximum is `86400`. |

`[upstream.openai]` stays required. `/readyz` reports both upstreams.

### 1.3 Roll back

Set `[messages] enabled = false` (or `GATEWAY_MESSAGES__ENABLED=false`) and restart the gateway.
The three routes then answer 404 in the Anthropic error format. The OpenAI routes do not change.

## 2. Connect Claude Code

Set these variables for each developer.

- `ANTHROPIC_BASE_URL`: the gateway origin, for example `https://gateway.example.com`.
- `ANTHROPIC_API_KEY`: a Paigasus API key. Claude Code sends it as `x-api-key`. The gateway reads
  `x-api-key` as a Bearer token.
- `ANTHROPIC_AUTH_TOKEN`: an OIDC token. Claude Code sends it as `Authorization: Bearer`.

If a request has both headers, `x-api-key` wins and the gateway ignores `Authorization`. So an old
`ANTHROPIC_API_KEY` in the shell wins over `ANTHROPIC_AUTH_TOKEN`. If a developer sees a 401 with
a good OIDC token, tell the developer to unset `ANTHROPIC_API_KEY`.

An empty `x-api-key`, or two `x-api-key` headers, gets a 401. A DPoP-bound token in `x-api-key`
also gets a 401, because DPoP does not apply to `x-api-key`.

Other settings:

- `ANTHROPIC_CUSTOM_HEADERS`: set `paigasus-org: <org>` here when an OIDC user is a member of
  more than one organization.
- `apiKeyHelper`: an OIDC token expires. Set the `apiKeyHelper` TTL of Claude Code to less than
  the token lifetime, so that Claude Code asks for a new token before the old one expires.
- `CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS`: you do not need it. The upstream is Anthropic, so
  all beta headers are valid.
- `CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY=1`: set it to fill the `/model` list from
  `GET /v1/models`.

`GET /v1/models` lists every model of the operator account. It does not filter by tenant.

## 3. Known gaps

These gaps stay open after SMA-750. Tool-call policy (ADR-0027) does not cover them.

- **Server tools and the MCP connector are not enforceable.** `server_tool_use`, `mcp_tool_use`
  and their results run at Anthropic. The gateway only reports them as `unenforceable`. It does
  not remove a server tool definition, and `mcp_servers` passes. A Cedar deny of a server tool
  has no effect on this route.
- **Caller-chosen features can raise the cost of the operator.** Examples: premium betas such as
  long context, `service_tier`, and server tools that bill per use. The token budget counts only
  tokens. The gateway logs the `anthropic-beta` values of each request, so you can see them.
- **The prompt cache is shared.** All tenants use one operator key. `cache_read_input_tokens` can
  show that another tenant sent the same prefix. The OpenAI routes have the same property.
- **Unknown wire shapes fail closed.** With tool interception on, the gateway drops an unknown
  event type. An unknown delta on a text block is a deviation. An unknown block type is held and
  decided as a tool call. If Anthropic adds a new type, a feature can stop working until the
  gateway supports it. Section 5 describes the alert for this case.

## 4. Metric and log labels

- The `endpoint` label of `gateway_upstream_requests_total` has these values for the Anthropic
  routes: `messages`, `messages_count_tokens` and `models`.
- The decision log uses the ingress label `anthropic_messages` for `POST /v1/messages`. The
  `endpoint` label is `messages`. These are two names for one route.
- The per-request log line of `/v1/messages` carries the `anthropic-beta` values of the request.

## 5. Malformed records alert

The alert `GatewayToolInterceptionMalformedRecords` fires when
`gateway_tool_interception_records_dropped_total{reason="malformed"}` rises in 15 minutes.

The metric has no `endpoint` label. So the alert covers every route that uses tool interception,
not only `/v1/messages`.

Do these steps when the alert fires:

1. Find the route. Look at the gateway log lines of the time of the alert. Check the `endpoint`
   field. If the route is `messages`, continue with step 2.
2. Check the `anthropic-beta` values and the Claude Code version in the log. A new Claude Code
   version or a new beta can send a block or event type that the gateway does not know.
3. Check if the user sees a lost tool call or a stopped stream. A dropped record is not
   forwarded to the caller.
4. If the cause is a new Anthropic type, report it to the gateway team. The fix is a code change.
   As a short-term action, you can set `[tool_interception] enabled = false`. This removes the
   policy check for all tool calls, so get approval first.
5. If the cause is an upstream defect, keep the alert open until Anthropic fixes it.

A conforming upstream keeps this counter at 0. The series `reason="oversized"` does not fire
this alert. Raise `max_record_bytes` only after you find the cause of an oversized record.
