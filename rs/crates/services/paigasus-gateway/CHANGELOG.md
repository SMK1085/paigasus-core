# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

A maintainer writes this file by hand. release-plz does not process this crate, because its Cargo
manifest sets `publish = false` (SMA-658, spec § 3.1).

## [Unreleased]

### Added

- Three routes for the Anthropic Messages API: `POST /v1/messages`, `POST /v1/messages/count_tokens`
  and `GET /v1/models`. They forward to Anthropic with the key of the operator. All three
  routes use the same authentication and limit admission as the OpenAI routes. Only
  `POST /v1/messages` applies the tool-call policy and the budget charge. `count_tokens` and
  `models` are pass-through routes. They charge zero tokens. Two new tables
  turn them on: `[upstream.anthropic]` and `[messages]`. With `[messages] enabled = false`, the
  default, the routes answer 404. See `docs/ops/RUNBOOK-anthropic-messages.md` (SMA-750).
- The signed webhook detector. An operator registers up to 8 providers in
  `[[tool_policy.detectors.webhook]]`. The gateway sends each tool call to the providers that the
  tenant opted in to, with an HMAC signature, and checks the signature of the answer. A failure of
  a provider gives a `detector_error` finding for that call. The new counter
  `gateway_tool_policy_webhook_failures_total{provider, reason}` counts the failures. A tenant
  opts in with the new `webhooks` setting of IAM. See `docs/ops/RUNBOOK-tool-policies.md` section
  11 and `docs/tool-policies/webhook-protocol.md` (SMA-856).
- Rollout note: add a provider to every gateway pod before a tenant opts in. Roll out the new IAM
  before the new gateway. An older IAM ignores the `webhooks` setting and gives every tenant an
  empty list. Then no provider runs, and no finding shows it (SMA-856).

### Changed

- The OpenAI upstream client no longer follows redirects. An upstream 3xx answer now gives a 502
  `UpstreamUnavailable` (SMA-750).
- `gateway_tool_policy_overhead_seconds` no longer includes the wait time of the late stages (the
  webhook providers and the judge). `gateway_tool_policy_detector_seconds{stage="webhook"}`
  measures the webhook time (SMA-856).

## [0.3.0] - 2026-11-20

### Added

- A new setting, `[dpop] enabled` (`GATEWAY_DPOP__ENABLED`), turns on the DPoP scheme on the
  protected routes. It is off by default. With it on, a client sends `Authorization: DPoP <token>`
  and one `DPoP` proof header. The gateway skips the API-key leg. It sends the proof, the method
  and the path to IAM, and sends the same token and proof on the self-query. Turn DPoP on in IAM
  first (SMA-700).
- With DPoP on, every 401 carries one `WWW-Authenticate: DPoP` line. A bad proof is 401
  `invalid-dpop-proof`. The DPoP quota of IAM is 429 `rate-limited` with `Retry-After`. The
  metric `gateway_iam_calls_total` gets the result label `denied`, not `unavailable`. With DPoP
  off, no response changes (SMA-700).

- A new table, `[limits]`, sets a rate limit and a token budget for `POST /v1/chat/completions`. It
  is off unless the table exists. The settings are `principal_requests_per_minute`,
  `org_requests_per_minute`, `tokens_per_period` and `budget_period` (`daily`, `weekly` or
  `monthly`). The `[[limits.org]]` entries set a limit or `exempt = true` for one organization.
  `backend` is `memory` (default) or `redis`. With `redis`, set `GATEWAY_LIMITS__REDIS_URL`. If
  Redis is unavailable after boot, the gateway admits requests with no limit and logs it. A
  refusal is 429 `rate-limited` (with `Retry-After`) or 429 `budget-exhausted` (SMA-677).
- A new table, `[tool_interception]`, holds each tool call of a chat completion until its
  arguments are complete. The gateway asks the decider and replaces a blocked call with an
  assistant notice. It is off by default. A decision is an SSE comment `: paigasus-decision` or,
  for a non-stream answer, the header `paigasus-tool-decisions`. With `on_failure = "block"`, a
  non-stream answer that the gateway cannot check is 502 `tool-interception-malformed-response`
  (SMA-749).
- A new route, `POST /v1/responses`, serves the OpenAI Responses API with the same tool call
  interception. It is off by default (`[responses] enabled = false`), and the route answers 404
  then. With interception on, a request for server-side state is 400
  `responses-stateful-not-supported` (SMA-769).
- A new table, `[tool_policy]`, makes Cedar `InvokeTool` decisions. It is off by default and needs
  `[tool_interception]`. The mode of an organization is `off`, `log_only` or `enforce`.
  `on_policy_unavailable` is `open` or `closed`, and it has no default. The platform floor is
  always enforced. The policies come from a directory (`source.kind = "file"`) or from IAM
  (`source.kind = "iam"`). With `iam`, the gateway needs `iam.service_credential_file`, reads the
  mode and the fail mode of each organization from IAM, and refuses `[[tool_policy.org]]` entries.
  An optional `[tool_policy.nats]` table receives policy invalidation events (SMA-752, SMA-775).
- A blocked `computer_call_output` in `enforce` mode refuses the request with 400
  `tool-result-unreplaceable` (SMA-776).
- A new table, `[decision_log]`, records each tool policy decision in a Redis stream. A relay
  publishes the records to the NATS JetStream stream `GATEWAY_DECISIONS`. It is off by default. It
  needs `[tool_policy]`, `redis_url`, `digest_key`, `digest_key_id` and `[decision_log.nats]`. A
  Redis or NATS outage at boot does not stop the gateway. The release also adds a trace export
  command, metrics and alerts for the log (SMA-755).

### Changed

- The HTTP client of the gateway uses `reqwest` 0.13 with the operating-system trust store. The
  image no longer has a compiled-in Mozilla root list. On Linux, an empty system store with no
  `extra_ca_bundle_path` is a client build error (SMA-809).
- The gateway refuses to boot when `tool_policy.nats.ca_file` is set and the URL is plain
  `nats://`. `async-nats` 0.53 makes a root certificate imply TLS (SMA-809).

### Fixed

- The gateway charges the last SSE record of a stream that ends without a blank line. Before this
  fix, the token budget missed that usage (SMA-730).
- A blocked `shell_call_output` now becomes one schema-valid chunk, with the notice in `stderr`
  and exit code 1. The upstream accepts it (SMA-776).

## [0.2.0] - 2026-10-01

### Added

- The gateway chat surface accepts an OIDC access token from an interactive user, not only an
  API key. IAM authorizes `InvokeModel` against the user's own token and an organization scope.
  The `org_admin` role does not carry `InvokeModel`: a person needs a `gateway_user` grant to
  call the chat surface (SMA-635).
- A new request header, `paigasus-org`, names the organization for an OIDC caller. The header is
  optional when the caller reaches exactly one organization through its memberships and role
  grants: the gateway then infers that organization. The gateway ignores the header for an API
  key: an API key's scope comes from the key itself. The gateway logs one warning for each
  request that carries the header with an API key (SMA-635).
- The gateway returns two new error responses. It returns `invalid-org-header` (400) when
  `paigasus-org` is not exactly one organization UUID in the 36-character form. It returns
  `org-required` (400) when the caller sends no header and reaches zero or more than one
  organization. Both name the header `paigasus-org` in the response `param` field (SMA-635).

### Changed

- The `invalid-api-key` error message reads "Invalid credential." instead of "Invalid API key."
  The error code stays `invalid-api-key`. The message changed because this error now also covers
  a rejected user bearer, not only a rejected API key (SMA-635).

## [0.1.0] - 2026-09-20

### Added

- The first released container image: `ghcr.io/smk1085/paigasus-gateway` and
  `docker.io/smaschek/paigasus-gateway` (SMA-658).
