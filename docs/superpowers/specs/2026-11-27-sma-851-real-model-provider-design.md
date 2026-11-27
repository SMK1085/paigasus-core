# SMA-851: a real model provider behind the `ModelRegistry` port

- Linear: SMA-851. Blocks SMA-791. Related: SMA-790 (worker run loop), SMA-750 (gateway
  `/v1/messages`, merged in PR 535).
- Status: draft for Gate 1. Revised after the builder check and the spec challenge (§13).
- Repositories: `paigasus-helikon` (part H) and `paigasus-core` (parts U, P, C, F, T).

## 1. Intent

The agent host must call a real model. Today the worker runs only with the `scripted` test model
behind the `ModelRegistry` port (SMA-790 §6.2). The production registry is empty, so the boot fails
when an `[[agents]]` entry names any model.

The real model is Anthropic Claude, called through the Paigasus AI Gateway on its Anthropic
Messages route (`POST /v1/messages`, SMA-750). The agent host never calls `api.anthropic.com`
itself. The gateway holds the Anthropic key, applies IAM and tool-call policy for the scope of the
agent host's service-account key, and meters the call against that scope. The Anthropic route
keeps `cache_control` available for SMA-791 (prompt caching).

## 2. Goals

- G1. An `[[agents]]` entry names a configured real model, and the worker runs that agent end to
  end through the gateway.
- G2. `repo:deny` stays green. No `aws-lc-rs`, `aws-lc-sys`, `openssl-sys` or `native-tls` enters
  `rs/Cargo.lock`.
- G3. The agent host uses the Helikon Anthropic provider. It does not copy the Anthropic wire
  format (ADR-0005: behaviour lives once).
- G4. The gateway credential comes from the agent host config, never from an agent definition.
- G5. A test runs the real binary against a fake gateway that replays a recorded Anthropic stream.

## 3. Non-goals

- N1. Prompt caching settings. SMA-791 owns them. This change sends no `cache_control`
  (`CacheStrategy::None`, the provider default).
- N2. Other providers (OpenAI, Bedrock, Gemini, LiteLLM) and the gateway's OpenAI routes.
- N3. Per-agent or per-principal gateway credentials, token exchange and delegation (SMA-793,
  SMA-794).
- N4. An agent-definition registry (SMA-852). Models stay in the config file.
- N5. Extended thinking, `effort`, `top_k`, beta headers and native tool types. The builder calls
  `.native_tool_types(false)`, because the provider default is `true`.
- N6. Key rotation without a restart. The runbook gives the order: issue the new key, restart the
  hosts, revoke the old key.
- N7. A deployment of the agent host in kind, Helm or compose. The agent host is not in `charts/`
  or `ops/` today.

## 4. Anti-goals

- A1. A waiver or a `skip` for a banned TLS crate in `rs/deny.toml`. The ban exists because two
  rustls providers make rustls panic at the first TLS use (SMA-786 §3).
- A2. A second rustls crypto provider in either workspace. paigasus-core is ring-only. Helikon is
  aws-lc-only, and its root `Cargo.toml` requires that `cargo tree -i ring --all-features -e normal
  --target all` prints nothing.
- A3. A change of the default features of the Helikon Anthropic provider. A behaviour change is
  allowed only in a release that is marked as breaking (§7.1).
- A4. The gateway key in a log line, a `Debug` output, a serialized config, a header `Debug` or an
  error text.
- A5. A fault of the shared gateway link (a bad key, a missing grant, the route off, the gateway
  down or stalled, an exhausted budget) that moves events to `dead`. See §9 D1.

## 5. Constraints

- paigasus-core pins `paigasus-helikon = "=0.6.12"` (core 0.6.0). Helikon `main` is at facade
  0.9.7, core 0.8.3 and `paigasus-helikon-providers-anthropic` 0.3.6. A 0.1.31 backport on the
  core-0.6 line is possible in principle. The upgrade is chosen because it brings `cache_strategy`
  for SMA-791 and the provider fixes since 0.6 (SMA-834, SMA-840, SMA-854).
- The changelogs of core 0.7 and 0.8 list two breaking changes: jiff 0.2 → 1 (SMA-831) and
  `ToolDef` `#[non_exhaustive]` (SMA-830). The host never builds a `ToolDef`. Core 0.8.1 refuses a
  turn with two blank-id parallel tool calls (SMA-621). That is a behaviour change, not an API one.
- `paigasus-helikon-providers-anthropic` sets `reqwest = { features = ["json", "stream", "rustls"] }`.
  In reqwest 0.13.5 the `rustls` feature enables `__rustls-aws-lc-rs`. `rustls-no-provider` enables
  only the platform verifier and `__rustls`. Cargo features only add.
- The builder's `build_config()` calls `self.http_client.unwrap_or_default()`. Without an installed
  provider and without the aws-lc feature, reqwest panics in `Client::new()`.
- `build_headers` calls `HeaderValue::from_str(key).expect(..)` on every `invoke`, and it does not
  mark the auth header as sensitive. `AuthSource` and `AuthHeader` derive `Debug`.
- The paigasus-core workspace sets `reqwest = { default-features = false, features =
  ["rustls-no-provider", "json", "form"] }`. Because features unify, the provider gets
  `rustls-no-provider` from that line.
- `rs/deny.toml` sets `[graph] all-features = true`. That flag applies to the workspace members.
- paigasus-core pins `jiff = "0.2"` in three crates: `paigasus-agent-host`,
  `paigasus-agent-host-core` and `paigasus-cloudevents`. `croner` 4.0.3 uses jiff 0.2. `croner`
  5.0.1 uses jiff 1.0.1 with its `jiff` feature, and `default-features = false` now means `no_std`.
- Each claim adds 1 to `attempts` (`claim.rs` `MARK_SQL`). `DEAD_SQL` moves an event with
  `attempts >= max_attempts` to `dead`. Only `release_after_shutdown` gives an attempt back
  (`REFUND_SQL`). With the defaults, a transient fault reaches `dead` in about 7.5 minutes.
- The gateway (SMA-750) reads `x-api-key` as a bearer (D6). It forwards only `anthropic-*` headers
  (D5). It answers errors in the Anthropic envelope (§6.1): 400 `invalid_request_error`, 401
  `authentication_error`, 403 `permission_error`, 404 `not_found_error`, 413 `request_too_large`,
  429 `rate_limit_error` or `billing_error` (with `retry-after`), 5xx `api_error`, 504
  `timeout_error`. The route is off by default (`[messages] enabled = false`, a 404).
- For an API key, the gateway takes the scope from the key and ignores `paigasus-org`.
- The provider maps 401 and 403 to `Refused { reason }`, and a `refusal` stop reason to
  `Refused { reason: "model refused" }`. It maps 429 `billing_error`, 400, 404 and 413 to
  `ModelError::Other` and drops `retry-after`. The host classifies every `Refused` as
  deterministic (SMA-790 §6.7), and `Other` as transient.
- `ModelError` is `#[non_exhaustive]`.
- An id not in the provider's `KNOWN_MODELS` table gets `max_output_default: 4096` and no caching,
  reasoning or parallel-tool capability.
- Helikon CI builds and tests the provider only with `--all-features`.
- reqwest reads `HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY` and `NO_PROXY`.

## 6. Approaches

- **A. Fix the TLS feature in Helikon, then upgrade Helikon in paigasus-core (chosen by Sven on
  2026-11-27).** One provider implementation, with `cache_strategy` and the provider fixes. Cost:
  two repositories, and the Helikon release must come first.
- **B. An Anthropic Messages `Model` adapter in the agent host.** It copies the SSE parsing, the
  tool-use translation, the thinking signatures and `cache_control`, and it breaks ADR-0005.
- **C. Waive `aws-lc-rs` in `deny.toml`.** It breaks A1, and `aws-lc-sys` needs a C build.

Route: the gateway's `/v1/messages` (chosen by Sven on 2026-11-27).

## 7. Design

### 7.1 Part H — Helikon

In `paigasus-helikon`, crate `paigasus-helikon-providers-anthropic`. One PR, marked breaking (`!`).

**TLS features.**

```toml
[features]
default            = ["rustls-aws-lc"]
rustls-aws-lc      = ["reqwest/rustls"]               # today's behaviour
rustls-no-provider = ["reqwest/rustls-no-provider"]   # the caller installs a provider

[dependencies]
reqwest = { workspace = true, features = ["json", "stream"] }
```

- No `compile_error!`. When `build()` must make its own client and no TLS feature can give one
  (no `rustls-aws-lc`, and `rustls::crypto::CryptoProvider::get_default()` is `None`), `build()`
  returns a new `BuildError::NoCryptoProvider`. It never panics.
- The docs say: under `rustls-no-provider`, install a provider before `build()`, or pass
  `http_client`.
- Helikon CI: add `cargo build` and `cargo test -p paigasus-helikon-providers-anthropic
  --no-default-features -F rustls-no-provider` to `build-no-default-features`. The tests install a
  provider from a dev-dependency, so the A2 `-e normal` rule holds. Add
  `cargo tree -p paigasus-helikon-providers-anthropic --no-default-features -F rustls-no-provider
  -e normal --target all -i aws-lc-rs`, and the same for `-i ring`. Both must print nothing.

**Credential safety.**

- A hand-written `Debug` for `AuthSource` and `AuthHeader` writes `<redacted>`.
- `set_sensitive(true)` on the `x-api-key` and `authorization` header values.
- The key is checked once in `build()`: `HeaderValue::from_str` must accept it, or `build()`
  returns `BuildError::InvalidAuthCredential`. The `expect` in `build_headers` goes away.

**Errors.** Subject to §9 D1 and D2. The recommended form:

- Core gets `ModelError::Rejected { status: u16, error_type: String, message: String }` for a
  non-retryable HTTP 4xx. It is an addition to a `#[non_exhaustive]` enum, so core gets a
  non-breaking release.
- The Anthropic provider maps 400, 401, 403, 404 and 413 to `Rejected`. 429 `billing_error`
  becomes `RateLimited` and keeps `retry-after`. The `refusal` stop reason stays `Refused`.
- The other providers do not change in this PR.

**Models.** Add the Claude ids that Paigasus uses (at least `claude-sonnet-5-5` and
`claude-opus-5-5`) to `KNOWN_MODELS`, with their output limits and capabilities.

**Release.** The provider becomes `0.4.0` (breaking: new error mapping). Core gets a patch with
`Rejected`. release-plz publishes. Sven merges the release PR. paigasus-core waits for the
crates.io release. It uses no `[patch]` and no path or git dependency.

**Tracking.** A new Linear issue in the Paigasus Helikon project that blocks SMA-851.

### 7.2 Part U — Upgrade Helikon in paigasus-core

- `paigasus-helikon` `=0.6.12` → the facade version that matches the new core. Keep
  `default-features = false, features = ["runtime-tokio"]` and an exact pin.
- `paigasus-helikon-sessions-testkit` `=0.1.2` → the version for the same core.
- `jiff` 0.2 → 1 in the workspace. Keep `tzdb-bundle-always`.
- `croner` 4.0.3 → 5.0.1 with `default-features = false, features = ["std", "jiff"]`. Check the
  croner 5.0.0 breaking changes, in particular the DST gap search. The proof is `cron.rs`,
  `dst_tests.rs` and `scheduler_pg.rs`. One jiff version must remain, because the dependency
  allowlist test in `paigasus-agent-host-core/src/lib.rs` requires it.
- `cargo tree -d` shows one `paigasus-helikon-core` and one `jiff`.
- **Formats.** The proof that timestamp formats do not change is the existing golden strings:
  `format_fire_at` (`timer/mod.rs`), `format_time` (`paigasus-cloudevents/src/build.rs`), the ids in
  `timer_event.rs` and `timer_rows.rs`. Add one golden session row with a fractional-second `ts`, as
  core 0.6 wrote it, and read it with the new core.
- **Rollback.** The builder measures whether core 0.6 reads a row that the new core writes, and
  records the result in the PR. If it cannot, the PR says that a rollback of part U needs a
  database restore.
- Part U is its own commit, and every existing test passes on it alone.

### 7.3 Part P — The gateway model adapter

New module `adapters/helikon/gateway_model.rs` in the library crate `paigasus-agent-host`.

- Dependency: `paigasus-helikon-providers-anthropic = { version = "=0.4.<n>", default-features =
  false }`. The workspace `reqwest` line supplies `rustls-no-provider`.
- `pub fn register(models: &mut ModelRegistry, gateway: &GatewayConfig, entries: &[ModelConfig])
  -> anyhow::Result<()>`. `main.rs`'s `model_registry(&Config)` calls it. The `test-model`
  registration stays.
- One shared reqwest `Client` for every gateway model:
  - `let _ = rustls::crypto::ring::default_provider().install_default();` in the client
    constructor, as `memory_s3/client.rs` does.
  - `redirect::Policy::none()`. reqwest keeps `x-api-key` on a cross-host redirect (SMA-750 D16).
  - `.no_proxy()`. The proxy env variables must not route the key through another host.
  - Roots: when `gateway.ca_file` is set, it REPLACES the platform roots (`tls_certs_only`), as
    `nats.ca_file` and `iam.tls.ca_cert_path` do. When it is not set, the platform roots apply.
    Its doc says REPLACE. `rs/CLAUDE.md`'s CA-knob count goes from six to seven.
  - `connect_timeout` = `gateway.connect_timeout_ms`.
  - `read_timeout` = `gateway.read_timeout_ms`. It applies to each read, including the wait for
    the response head. A stalled stream then ends as `Transport`, not as the deterministic
    `RunError::Timeout`.
- Each `[[models]]` entry: `AnthropicModel::messages(<model>).base_url(<base_url>).api_key(<key>)
  .http_client(<client>).native_tool_types(false)`, then `.build()?`. `api_key` sends `x-api-key`
  (SMA-750 D6). Then wrap it in `RetryingModel` (Helikon runtime-tokio) with 2 retries that obey
  `retry_after_ms`, so a short 529 does not repeat the whole run. Then wrap it in `SharedModel`
  and register it under `name`.
- `max_output_tokens` of the entry goes to the request. The builder finds where: the agent's model
  settings in `build_agent`, or the provider's capabilities via `with_capabilities`. The E2E test
  checks `max_tokens` in the recorded request body.

### 7.4 Part C — Config

```toml
[gateway]
base_url = "https://gateway.paigasus.internal"   # a path prefix is allowed
api_key_file = "/run/secrets/agent-host-gateway-key"
ca_file = "/etc/paigasus/gateway-ca.pem"          # optional, REPLACES the platform roots
connect_timeout_ms = 5000                          # default 5000, range 100 to 60000
read_timeout_ms = 60000                            # default 60000, range 1000 to 600000

[[models]]
name = "claude"                 # what [[agents]].model names
provider = "anthropic"          # the only value in v1
model = "claude-sonnet-5-5"     # the Anthropic model id in the request body
max_output_tokens = 16000       # required, range 1 to 128000
```

`Config::validate()` (no I/O):

- `[gateway]` is required when `[[models]]` is not empty. `[gateway]` with no `[[models]]` is an
  error (`GatewayUnused`), as `MemoryS3Unused` is.
- `base_url` parses as a URL. Scheme `https`, or `http` only when `is_loopback_host` accepts the
  host. No user name, no password, no query, no fragment. A path prefix is allowed.
- `api_key_file` and `ca_file`: not blank and no surrounding whitespace, as `MemoryS3CaPathBlank`
  does.
- `read_timeout_ms` must be less than `run.timeout_secs × 1000`. It must also be more than the
  gateway's longest silent gap during a stream (the SMA-750 D12 `ping` interval). The builder
  reads that interval and sets the default from it.
- `[[models]].name`: unique, not empty, not `scripted` (reserved for `test-model`).
- `provider`: a closed enum. `model`: not empty. The host does not check `model` against a list.
- `ModelRegistry::check` still fails the boot when an `[[agents]].model` has no registered model.
- Subject to §9 D3: every agent that names a gateway model has the same org.

`register()` (I/O, once, before the database step, also when `worker.enabled = false`):

- Read `api_key_file`. Remove surrounding whitespace. Refuse an empty key, a key that is not
  UTF-8, a key with a BOM or inner whitespace, and a key that `HeaderValue::from_str` refuses. The
  error names the path, never the content.
- Read and parse `ca_file` as at least one PEM certificate.
- The key lives in a type whose `Debug` and `Serialize` write `<redacted>`.

### 7.5 Part F — Fault classification

The gateway is a dependency that every inbox shares. A fault of that link is not the fault of one
event (A5). The recommended rule is in §9 D1. With D1 (a):

| Error | Class |
|---|---|
| `Rejected` 401, 403, 404 | host fault |
| `RateLimited` (rate limit or budget) after the in-run retries | host fault, delay at least `retry_after` |
| `Unavailable`, `Transport` (connect failure, read timeout) after the in-run retries | host fault |
| `Rejected` 400, 413 | deterministic |
| `Refused` (model `refusal`) | deterministic |
| `ContextLengthExceeded` | deterministic and close, as today |
| `Other` | transient, as today |

- **Host fault.** A new `RunEnd::HostFault { error, retry_after }`. The worker backs off the events
  (`not_before`, `last_error`) and gives the attempt back in the same transaction, as `REFUND_SQL`
  does. The run row gets `status = 'failed'`, `error_class = 'host_fault'`. No memory commit.
- **Pool pause.** After `worker.host_fault_pause_after` host faults in a row (default 3) across the
  pool, every worker task waits `worker.host_fault_pause_secs` (default 30, doubling to a cap of
  600) before the next claim. One completed run resets the count. This stops a series of calls to
  the gateway and IAM during an outage.
- **Log and metric.** Each host fault writes one `error` line with the status and the error type,
  never the key. A 401 or 403 line says "the gateway rejected the agent host credential". A new
  label on the run-outcome counter counts host faults, and a gauge shows the pause state.
- **Boot probe.** At boot, `GET <base_url>/v1/models` with `anthropic-version` (no tokens, SMA-750
  D11, D20). A 401, 403 or 404, or a connect failure, writes one `warn` line. The boot does not
  fail, because the gateway can be down for a short time during a deploy.

### 7.6 Part T — Tests

Fixtures come from `rs/crates/services/paigasus-gateway/tests/fixtures/anthropic/*.sse` (SMA-750
D21), so that one provenance rule applies.

- **End to end, real binary.** A spawned-binary test (as `tests/support/mod.rs` does) with the
  Postgres testcontainer, a `[gateway]`, one `[[models]]` entry and one `[[agents]]` entry. A fake
  gateway (axum on `127.0.0.1:0`, plain HTTP) serves `POST /v1/messages` and `GET /v1/models`. The
  test ingests one event and asserts the committed run row, the output text and the usage. It
  fails when the `register()` call in `main.rs` is deleted.
- **Request shape.** The fake records each request: `x-api-key` equals the key file content,
  `anthropic-version` is present, no `authorization`, body `model` and `max_tokens` equal the
  config.
- **Tool round.** The fake answers two requests in order: a `tool_use` for `memory`, then text. The
  second request carries a `tool_result` with the `tool_use` id.
- **Classes.** One test per row of the §7.5 table, with the class and the `attempts` value after the
  run: 401, 403, 404, 400, 413, a 429 `billing_error` with `retry-after`, a 529 that the in-run
  retry absorbs, a stream that stalls after one frame (read timeout), and a `refusal` stream.
- **Gateway tool block.** The gateway's block notice (text and `end_turn`) completes the run, with
  the notice as the output.
- **Pool pause.** Three host faults in a row pause the pool. One completed run resets it.
- **No redirect.** The fake answers 307 to a second listener. The second listener gets no request,
  and the run is a host fault.
- **Key safety.** At TRACE level (`paigasus_logging::test_support`), the 401 test's logs do not
  contain the key. The `Debug` of the built model, of the builder and of every boot error does not
  contain the key. Config `Debug` and `Serialize` do not contain it.
- **Config.** Unit tests for every rule in §7.4.
- **Dependency graph.** `repo:deny` passes, and `rs/Cargo.lock` has no entry for a banned crate.

### 7.7 Docs

- `README.md` and `agent-host.toml.example` of the agent host: the new sections, the rotation order
  (N6), the REPLACE rule of `ca_file`, the proxy rule.
- The gateway prerequisites: `[messages] enabled = true`, `[upstream.anthropic]`, streaming on, and
  a service-account key with `gateway_user` on the target scope.
- `describe_counter!` texts in `main.rs`, and `paigasus_observability::names` for the new label and
  gauge.

## 8. Decided by Sven

- 2026-11-27: the model calls go through the AI Gateway.
- 2026-11-27: the route is the gateway's Anthropic Messages route.
- 2026-11-27: approach A (fix the TLS feature in Helikon, upgrade Helikon in paigasus-core).
- 2026-11-27 (Gate 1): spec approved. D1 (a), D2 (a), D3 (a). A new Linear issue in the Paigasus
  Helikon project tracks part H. The same pipeline run delivers both part H (Helikon) and the
  paigasus-core parts.

## 9. Decisions for Gate 1 (decided: D1 a, D2 a, D3 a)

- **D1. A gateway fault and the attempt rule.** Under SMA-790, every failed attempt counts, and
  `dead` follows after about 7.5 minutes. A bad key, a missing grant, the route off or a gateway
  outage then moves every pending event of every tenant to `dead`.
  - (a) Recommended. The host-fault class of §7.5. It gives the attempt back, as a shutdown does,
    and pauses the pool. This changes the SMA-790 attempt rule, and the Notion ADR-0028 entry gets
    an amendment. Risk: a fault that the host misclassifies as a host fault retries without end.
    The metric and the `error` log make it visible.
  - (b) Keep the SMA-790 rule. A5 becomes an accepted risk with the 7.5-minute window, and a
    runbook to re-queue `dead` rows.
- **D2. How the host sees the HTTP status.** Needed for (a).
  - (a) Recommended. `ModelError::Rejected { status, error_type, message }` in Helikon core, as in
    §7.1.
  - (b) Helikon exposes the gateway's `x-should-retry` and `paigasus-retryable` headers. This is
    more general, but the gateway sends them only on some errors.
- **D3. The org of the agents.** The gateway authorizes, meters and applies tool policy for the
  key's scope, not for the agent's org.
  - (a) Recommended. A boot rule: every agent that names a gateway model has the same org, and the
    README says that the key must have that org's scope (or a project in it).
  - (b) No rule. Cross-org hosting is an accepted risk under N3.

## 10. Calls made by the coordinator (check these)

- The credential is an IAM service-account API key in a file, sent as `x-api-key`, read once at
  boot.
- `ca_file` REPLACES the platform roots.
- A path prefix in `base_url` is allowed.
- No total request timeout. Connect and read timeouts only.
- `RetryingModel` with 2 retries inside a run.
- `max_output_tokens` is required in `[[models]]`.
- A warn-only boot probe.
- `scripted` is a reserved model name.
- paigasus-core depends on the provider crate directly, with `default-features = false`.
- No development against an unreleased Helikon: paigasus-core waits for the crates.io release.
- 400 and 413 are deterministic: the request cannot pass, so five more calls cost money for
  nothing.

## 11. Left to the builder

- Where `max_output_tokens` enters the request (§7.3).
- The exact metric, label and gauge names.
- The module layout inside `adapters/` and the names of the new config and error types.
- The default of `read_timeout_ms`, from the gateway's `ping` interval.
- The Helikon test that proves `BuildError::NoCryptoProvider`.

## 12. Follow-ups

- Map 401 and 403 to `Rejected` in the other Helikon providers.
- Per-agent credentials (SMA-793, SMA-794).

## 13. Review changelog (2026-11-27)

Builder check: six questions. Spec challenge: verdict NEEDS REWORK.

Folded in:

- BLOCKER (both reviews): a gateway fault still reached `dead` through the attempt rule. Now
  §7.5 and §9 D1.
- MAJOR: the `compile_error!` and the error mapping broke A3. Now a `BuildError`, a breaking
  provider `0.4.0`, and A3 reworded.
- MAJOR: no CI job built the new feature, and `build()` panics without a provider. Now CI jobs and
  `BuildError::NoCryptoProvider`.
- MAJOR: the key leaked through Helikon `Debug` and a non-sensitive header. Now §7.1 and the
  TRACE-log test.
- MAJOR: one invalid key byte panicked every call. Now a boot check and
  `BuildError::InvalidAuthCredential`.
- MAJOR: no read timeout, so a stall became deterministic. Now `read_timeout_ms`.
- MAJOR: `ca_file` had the opposite meaning of `nats.ca_file`. Now REPLACE.
- MAJOR: an unknown model id had a 4096-token limit. Now `max_output_tokens` and `KNOWN_MODELS`.
- MAJOR: one key for agents of many orgs. Now §9 D3, and §1 corrected.
- MAJOR: the E2E test could pass with the wiring deleted. Now `register()` in the library and a
  spawned-binary test.
- MAJOR: a short 529 repeated the whole run. Now `RetryingModel`.
- MINOR: croner 5.0.1 uses jiff 1, the fallback is deleted. Golden strings as the format proof.
  The key read moved out of `validate()`. `GatewayUnused`. URL rules. `.no_proxy()`.
  `native_tool_types(false)`. Existing gateway fixtures. More class tests. Docs. Rollback. Text
  errors (§6.7, the 0.1.31 number).
- Builder Q3 (develop before the release): wait for the crates.io release. Builder Q4: the
  changelog facts are in §5. Builder Q5: the provider install is in the client constructor.
  Builder Q6: a path prefix is allowed.

Not folded in:

- The `cargo tree -i` checks in paigasus-core: replaced by `repo:deny` and a lock-file check,
  because cargo fails on a package that is not in the lock.
- A kind or compose route to the gateway: out of scope (N7).
