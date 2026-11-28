# SMA-856: tool-call detectors, part 2 (the signed webhook detector)

Linear: SMA-856, part 2 of 3 of the parent SMA-754 "tool-call detectors". ADR-0027, decision (h):
"A signed webhook lets external providers add findings. It cannot change content." Builds on
SMA-855 (the `Detector` port, the `Finding` type, the pipeline; spec
`2026-11-22-sma-855-tool-call-detectors-design.md`, "SMA-855 spec" below) and on the IAM tool
policy settings (spec `2026-11-15-iam-tool-policy-sets-design.md` § 9.2). Later part: SMA-857
(the judge model).

Status: revision 2, after the builder check ("nothing important left") and the first challenge
(NEEDS REWORK, 2 blockers). The revision 2 changelog lists what changed.

## Revision 3 (plan) amendments

The plan (`docs/superpowers/plans/2026-11-28-sma-856-webhook-detector.md`) found these points
against the code. They change the spec as follows.

- P1. § 7.1: a 25-digit number passes the strict parse. The kernel keeps a big integer as text
  (`paigasus-kernel/src/tool_policy/input.rs:76-83`). The test asserts that such an input is
  embedded byte-equal. A duplicate key and invalid JSON still go as `args_text`.
- P2. § 7.5: no test can observe that a client trusts a CA, because reqwest has no trust-store
  accessor and the gateway has no TLS listener (C18). The tests cover the build, the boot
  failures that name the provider and the path, and the control-build attribution.
- P3. D30: the new error reason also needs the TS SDK `presentation.ts` entry and the count
  changes in `presentation.test.ts` and `paigasus-proto/src/error.rs` (93 → 94). Its proto
  number is 64.
- P4. D7 and D25: the policy cache does not know the providers. A shared `WebhookRegistry` is
  injected into the cache, so that the cache load can write the warn.
- P5. § 6.8: the failure counter is primed for each provider and each of the 9 adapter reasons.
  The "8 × 15 + 3" count in § 6.8 is replaced by this rule.
- P6. D8: when the tenant part is unavailable for one hold (SMA-753 P7), that hold sends no
  webhook request.
- P7. D17: `detector_error` is in `FINDING_KINDS`, so the strict webhook check must not drop
  the adapter's own failure finding. The late-stage check keeps a `detector_error` from a
  webhook when its `version` is an adapter reason, its severity is that provider's
  `failure_severity()` (or `high` for D7 and D25), and it has no evidence. Any other
  `detector_error` from a webhook is still a `check` failure.

## Revision 2 changelog

Each line gives the finding of the first challenge and the change.

- R1 (blocker). At the pipeline cut, tokio drops the whole adapter future, so every call of a
  provider failed, not only the unfinished calls. → `DetectBatch` gets the cut as `deadline`.
  The adapter stops each call at `deadline - 10 ms` and returns per-call results itself. The
  pipeline cut stays as a backstop (D24, § 6.5).
- R2 (blocker). Any tenant could opt in to any provider, and so send its arguments to another
  tenant's vendor. → Each provider has a required `orgs` list, or `orgs = "*"` for a shared
  provider. An opt-in from another org sends nothing and fails closed (D25, § 6.2).
- R3. `buffered(8)` blocks the queue behind a slow early call. → `buffer_unordered`, with results
  put back by index (D1).
- R4. `args` was not defined for JSON that does not parse, and re-serialisation changes content.
  → Strict-parsed JSON is embedded verbatim as raw JSON. Anything else goes as `args_text` (§ 6.3).
- R5. The opt-in path was left to the builder, and a missed step fails open. → The spec fixes the
  path: `PolicySnapshot.webhooks`, set with the D8 rule, copied in both pipeline rebuilds (§ 6.7).
- R6. A bad name in the IAM answer made the whole tenant set unavailable. → It is an unknown
  provider (D7), and the tenant set stays (§ 6.6).
- R7. "Absent keeps the list" could lose a concurrent write. → The repository takes
  `Option<Vec<String>>` and leaves the column out of the update when it is `None` (§ 6.6).
- R8. No audit record or event showed the opt-in. → The audit detail and the event payload get
  the old and the new list (§ 6.6).
- R9. No secret rotation. → `previous_secret_env`. The gateway accepts a response that either
  secret signs (D20, § 6.3).
- R10. The HMAC key bytes were not defined. → The exact bytes of the env var. Leading or trailing
  whitespace is a boot failure (D20).
- R11. Nothing limited the requests in flight to one provider across holds. → `max_in_flight` per
  provider (default 64), reason `overloaded`. HTTP/1.1 stays; the pool is set on purpose (D26).
- R12. The main e2e test used `in`, which is not set membership in Cedar, so it passed without the
  finding. → `.contains("pii_email")`, plus a negative control (§ 7.3).
- R13. Four failure reasons, the runtime wiring and the IAM opt-in path had no test. → Added (§ 7).
- R14. A config difference between pods, or a removed provider, blocks whole tenants. → A
  rollout order in the runbook, and a `retired = true` entry (D27).
- R15 (minors). The trait gets `kinds()`; the D37 path uses `failure_severity`; D7 findings get
  `Stage::Webhook` and the reason `unknown_provider`; the label table becomes an injected
  `FindingLabels` value; the failure counter gets a `provider` label and the warn is rate-limited;
  webhook wait time leaves the overhead histogram; a per-provider `max_request_bytes`, and one body
  per call; URL hygiene; `Content-Encoding: identity`; the provider name is in the signed strings;
  the reserved kinds are "every kind in `FINDING_KINDS`"; `secret_env` must not start with
  `GATEWAY_`; IAM stores the list as JSON text, as roles do; no `optional` on the proto message
  field; a new IAM error reason; fixed test values and a barrier for parallelism; a provider
  timeout below the pipeline timeout; the opt-out delay during an IAM outage is documented.
- R16 (questions). The challenger's questions are answered in § 9.

## 1. Intent

The gateway now has five built-in detectors (SMA-855). Some tenants already pay for an external
security service: a secret scanner, a data-loss service or an agent-firewall product. They want
its verdict next to the built-in findings, in the same Cedar policy.

This change adds a webhook detector. An operator registers an external finding provider in the
gateway config, and lists the orgs that may use it. A tenant opts in to that provider in IAM.
Then, for each tool call of the model, the gateway sends one signed `POST` to the provider. The
provider answers with findings. The gateway checks the signature of the answer and adds the
findings to the call. Cedar alone decides. A provider cannot change a call, an argument or a
result.

## 2. Goals, non-goals, anti-goals

### Goals

- G1. A webhook adapter of the `Detector` port, in `adapters/detectors/webhook.rs`. It sends one
  HMAC-signed `POST` per tool call and checks an HMAC signature on the response.
- G2. Operator config: a list `[[tool_policy.detectors.webhook]]` of providers, each with a URL,
  the orgs that may use it, a secret (and a previous secret), a timeout, a failure severity, a
  closed list of kinds, limits and an optional `extra_ca_bundle_path` that **adds** to the trust
  store.
- G3. Tenant opt-in in IAM: a list of provider names in the tool policy settings, which the
  gateway reads from `GetCompiledToolPolicies`. With the file policy source, the list comes from
  `[[tool_policy.org]] webhooks`.
- G4. Every failure of a provider gives a `detector_error` finding with the provider's
  `failure_severity`. No failure lets a call go without a finding (SMA-855 A3).
- G5. The webhooks run after the built-ins, in parallel, inside the existing pipeline deadline.
  One slow call does not discard the answers of the other calls.
- G6. The SMA-855 comment inputs: a name check, a version check, a panic guard around `detect()`,
  and the metric label `webhook`.
- G7. End-to-end tests for timeouts and the failure severity, and for every failure reason, with
  a bad signature, a slow provider and a malformed response among them.

### Non-goals

- N1. The judge model (SMA-857).
- N2. Webhooks on the request side. The pipeline gives webhooks call subjects only (SMA-855
  § 6.6.2), so a provider never sees a tool result or its text.
- N3. Provider registration in IAM. Providers live in the gateway config only (D6).
- N4. A console UI for the opt-in, and a list of provider names for tenants (§ 9, Q2).
- N5. Evidence in the answer of a provider. A provider gives kind, severity and version only.
- N6. A retry. A failed request is not sent again.
- N7. A cache of answers.
- N8. A redaction of arguments before they go to a provider (D12).
- N9. Webhooks in the agent host (SMA-853).
- N10. An egress proxy, mTLS client certificates and HTTP/2 for providers (§ 9, Q3).

### Anti-goals (a failure even if the code works)

- A1. A provider failure, of any kind, gives a call with no finding from that provider (SMA-855
  A3). The severity of the failure finding may be low. The finding must be there.
- A2. A provider changes content: a call, an argument, a tool definition or a result.
- A3. A provider finding blocks a call when no policy reads it (SMA-855 A1).
- A4. A provider's secret, a request body, a response body, an argument, a nonce, a signature or a
  URL path reaches the decision log, the proto, a metric label, a `tracing` event, a panic
  message of our code, a `Debug` output or the readiness dump. The provider name, the URL host
  and a fixed failure reason may appear.
- A5. A tenant that did not opt in to a provider, or that is not in the provider's `orgs`, has a
  call sent to that provider.
- A6. A response that does not carry a valid signature for this request and this provider adds a
  finding (other than the failure finding).
- A7. A provider adds a finding under a reserved kind, a built-in detector name or another
  provider's name.
- A8. Our code calls `tokio::spawn` for the webhooks (SMA-855 A8, SMA-749 D25).
- A9. An opt-in that names a provider the gateway does not know, or may not use, is silently
  ignored.
- A10. One slow or failed call of a provider removes the valid answers of other calls.

## 3. Constraints found in the repo

- C1. The pipeline (`application/detectors/pipeline.rs:68-265`) holds
  `webhooks: Vec<Arc<dyn Detector>>` and runs them with `join_all` inside
  `tokio::time::timeout_at(min(run_start + timeout, deadline))`. A webhook slot is filled only
  when `detect` returns the whole vector (l.181-182). At the cut, the future is dropped, and
  l.217 gives all `m` calls `failed("webhook", name, Timeout, m)`, which is `detector_error`
  with severity `high` (l.260-265).
- C2. `guarded("webhook", d.detect(b))` (pipeline.rs:181) calls `detect()` outside
  `catch_unwind`. A hand-written impl that panics before it returns its future is not caught.
- C3. D37 (`check`, pipeline.rs:236-243) keeps a finding when its kind matches `[a-z0-9_]{1,64}`
  and its detector is a known name. A webhook may today return a finding under any known name,
  `kernel` included. It does not check `Finding.version`. `check` adds `detector_error(name)`,
  which is always `high` (`paigasus-kernel/src/detect/types.rs:254-256`). The tests
  `same_kind_from_two_stages_keeps_both` (pipeline.rs:619-626),
  `a_bad_kind_or_an_unknown_detector_name_is_replaced` and `decider.rs:770` assert today's
  permissive rule.
- C4. `DetectBatch` (`domain/detectors.rs:80-94`) is `#[non_exhaustive]` and has `side`, `org`,
  `request_id`, `subjects`, `seen_hosts` and `earlier`. It has no principal, no session, no
  opt-in list and no deadline. The pipeline rebuilds it with `..DetectBatch::new(..)` at
  pipeline.rs:162-171 and 196-205. `DetectBatch::new` has callers in `decider.rs:280`,
  `request.rs:473`, `decision_log.rs:380`, `built_in.rs:106,120`, `tests/perf.rs:693`,
  `tests/tool_policy.rs:2191`.
- C5. `DecisionContext` (`paigasus-gateway-core/src/decider.rs:28-42`) has `principal: String`
  and `session: Option<Arc<SessionContext>>`. `SessionContext.session_id` is a `Uuid`.
- C6. `Finding` (`paigasus-kernel/src/detect/types.rs:202`) has `detector`, `kind`, `severity`,
  `version` and `evidence`. `Evidence` is not serde-serialisable. Its `rule` is `&'static str`,
  its `location` is `Argument { pointer } | TextInput | Result { index }`, and `host` is
  `Option<String>`. Pointers are secret-cut (SMA-855 A5).
- C7. `ToolInput::Text` is free text for the tool kinds `custom`, `apply_patch` and `other`
  (`types.rs:261-266`). A `ToolInput::Json` whose text fails the kernel's strict parse (bad JSON,
  a duplicate key, too much depth) is "unparsed" (`detect/input.rs:121-157`). The `raw_value`
  serde_json feature is on through gateway-core and the kernel.
- C8. `build_client` (`adapters/upstream.rs:279`) loads an `extra_ca_bundle_path` with
  `Certificate::from_pem_bundle`, adds each certificate with `add_root_certificate`, fails at
  boot on an unreadable, malformed or empty bundle, and attributes a failure with a control
  build. It is bound to the `Provider` enum. `base_builder` installs the ring provider and turns
  redirects off. reqwest 0.13 uses `rustls-platform-verifier`: the OS store plus the added roots.
  The workspace builds reqwest with `default-features = false` (`rs/Cargo.toml:144`): no
  `http2`, no `gzip`, no `system-proxy`.
- C9. `rs/CLAUDE.md:101-110`: a new CA-bundle knob must say in its doc whether it adds or
  replaces. The `extra_` prefix means it adds.
- C10. Secrets are `secrecy::SecretString`, from env vars (`GATEWAY_<SECTION>__<KEY>`) through the
  figment env layer (`config.rs:807`), with `#[serde(skip_serializing)]`. An entry of a `[[...]]`
  TOML array cannot take a per-entry env override. A variable named `GATEWAY_TOOL_POLICY__…`
  reaches the figment layer, and `deny_unknown_fields` then refuses it. The repo has an overlap
  pattern for a key rotation: `tool_history.previous_digest_key` (`config.rs:402-406`).
- C11. The gateway has `hmac 0.13`, `sha2 0.11`, `base64`, `rand`, `secrecy`, `futures` and
  `reqwest` with `stream`. It has no `hex` and no `subtle` as direct dependencies.
  `Mac::verify_slice` is constant time.
- C12. Metrics: `DETECTOR_LABELS` (`decider.rs:135`) has `webhook`. `FINDING_TRIPLES` is a const
  table with `("detector_error","webhook","high")` only. `finding_labels` (decider.rs:186-199) is
  a free function over the const table, and `prime_tool_policy_metrics` takes no input. A webhook
  finding that is not in the table counts as `other/other/<sev>`. `FINDING_KINDS`
  (decider.rs:106-132) also holds the history kinds `mixed_sessions`, `forged_history` and
  `history_truncated`. `gateway_tool_policy_overhead_seconds` includes the pipeline time
  (decider.rs:364-366, 443-444), and the runbook sets its p99 below 15 ms
  (`docs/ops/RUNBOOK-tool-policies.md:596`).
- C13. IAM settings: table `tool_policy_settings` (m0014), domain type
  `paigasus-iam-core/src/tool_policy/model.rs:163-230`, write RPC `PutToolPolicySettings` and
  REST `PUT /v1/organizations/{id}/tool-policy-settings`, both guarded by
  `ManageToolPolicySettings`. `put_settings` reads the row outside the transaction
  (`application/tool_policies/mod.rs:384`) and upserts a full row (l.426-434). `first_enabled_at`
  uses `COALESCE` in SQL (`pg_tool_policies.rs:611`). The audit detail holds `mode`,
  `fail_mode` and `starter_version` (mod.rs:444-448). The event payload holds `org_id`, `mode`
  and `fail_mode` (`events.rs:42`). IAM stores string lists as JSON text
  (`entities/role.rs:4-6`). The last migration is m0015. `TenancyError` has no generic
  invalid-argument reason (`application/error.rs:52-102`).
- C14. Proto `ToolPolicySettings` (`iam.proto:1211`) has fields 1-4.
  `PutToolPolicySettingsRequest` (`iam.proto:1230`) has fields 1-3. A message field already has
  presence in proto3.
- C15. The gateway IAM source builds a version token `t{set.version}:{mode}:{fail}`
  (`adapters/policy_source/iam.rs:149`). An unchanged token keeps the cached data
  (`cache.rs:582-605`). A `SourceError::Malformed` marks the entry stale (`cache.rs:563-575`).
  Under fail mode `open`, the cache serves the last good entry for up to `max_stale_secs`
  (default 300, `cache.rs:481-497`).
- C16. The file policy source returns `settings: None`. Then the gateway takes `mode` and
  `on_policy_unavailable` from `[[tool_policy.org]]` (`config.rs:1154`, `config.rs:1312`).
- C17. The pipeline runs in mode `off` too when the floor is available (`decider.rs:388`, `:447`;
  `gateway-core/src/policy/calls.rs:464`). So the webhook mode rule (D8) cannot rely on "the
  pipeline did not run".
- C18. The gateway has no TLS test listener on purpose (SMA-558 D6). The SMA-558 tests prove the
  CA bundle with `rcgen` and a control build.
- C19. End-to-end detector tests live in `tests/tool_policy.rs`, `mod detectors` (l.1479 on).
  The stack builder builds `DetectorPipeline::new` by hand (l.1657) over `FilePolicySource`.
  `runtime::build_tool_policy` passes `Vec::new()` for the webhooks (`runtime.rs:414-419`). Mock
  upstreams are axum servers on `127.0.0.1` (`tests/support/mod.rs:97`). The default keep-alive
  is 10 s and `keepalive_secs` is at least 1 (`gateway-core/src/limits.rs:39`).
- C20. The pipeline deadline is `min(hold_start + decide_timeout - margin, run_start +
  [tool_policy.detectors] timeout_ms)` (D35). `timeout_ms` defaults to 500.
- C21. Cedar reads findings with `context.findings.contains("…")`
  (`gateway-core/src/policy/mod.rs:27`). `in` is the entity-hierarchy operator. An errored
  forbid denies (`kernel/src/tool_policy/decide.rs:209`).
- C22. `is_loopback_host` (`config.rs:782-796`) accepts `127.0.0.0/8`, `::1` and `localhost`.

## 4. Approaches

### 4.1 The wire shape (chosen: one signed request per call, signed response)

- **A (chosen by Sven, 2026-11-28).** One `POST` per tool call, with the body of the issue. The
  provider signs its response over the request's nonce and the response body. Trade-off: a hold
  with N calls and M providers sends N × M requests. Each is small, and they run in parallel.
- B. One `POST` per hold with a `calls` array. Fewer requests. But the body is not the one of the
  issue, and one bad call fails every call of the hold for that provider.
- C. Sign the request only, and trust TLS for the response. Simpler for providers. But with an
  extra CA bundle, TLS does not prove that a response comes from this provider for this request,
  and a "bad signature" has nothing to test.

### 4.2 The tenant opt-in (chosen: IAM settings)

- A. A list in the gateway config `[[tool_policy.org]]`. Small, but the operator controls it,
  not the tenant.
- **B (chosen by Sven, 2026-11-28).** A list in the IAM tool policy settings. The tenant controls
  it with the existing settings API. It needs a proto field, a migration and the IAM write path.
  The file source keeps the config list of A as its fallback (D9). The operator still limits
  each provider to its orgs (D25).

### 4.3 The failure setting (chosen: a failure severity per provider)

- A. `on_failure = "closed" | "open"`. `open` gives no finding, which breaks SMA-855 A3.
- **B (chosen by Sven, 2026-11-28).** `failure_severity`. Every failure gives `detector_error`
  with that severity. The tenant policy decides whether that severity blocks.
- C. No setting: every failure is `high`.

## 5. Decisions

"Sven" marks a decision that Sven made on 2026-11-28. "Coordinator" marks a proposal that Sven
can change at Gate 1.

| # | Decision | By |
|---|---|---|
| D1 | One signed `POST` per tool call (§ 4.1 A). Requests to all providers and for all calls of a batch run at the same time. Per batch and provider, at most 8 requests are open, driven with `buffer_unordered(8)` over `(index, future)` pairs. Results go back by index. | Sven (shape); Coordinator (cap) |
| D2 | The response must carry a valid HMAC signature over the provider name, the request nonce and the exact response body (§ 6.3). A response without it is a failure. | Sven |
| D3 | The tenant opts in through IAM: a new list `webhooks` in `ToolPolicySettings` (§ 6.6). | Sven |
| D4 | Every failure gives one `detector_error` finding per affected call, with the provider's `failure_severity` (default `high`), the provider's name as detector, and the failure reason as `version` (§ 6.4). | Sven (severity); Coordinator (reason in `version`) |
| D5 | `subject` in the body is `DecisionContext.principal` (a PRN). `session` is `SessionContext.session_id`, or `null`. | Coordinator |
| D6 | Providers are registered in the gateway config only. IAM accepts any opt-in name that matches `[a-z0-9_]{1,64}`, at most 16 distinct names. IAM cannot check that the gateway knows the name. | Coordinator |
| D7 | An opt-in name that the gateway does not know, or that fails the name rule, gives `detector_error` (`high`, detector = that name if it is valid, else `webhook`) on every call of that tenant, with `Stage::Webhook` and reason `unknown_provider`. The tenant set stays available. The policy cache load writes one `warn` per org and name when it stores a new entry. | Coordinator |
| D8 | Webhooks run only for a tenant in mode `log_only` or `enforce` whose tenant part is evaluated (`TenantPart::Evaluated`). In mode `off`, with the floor only, or when the tenant set is unavailable, the opt-in list of the snapshot is empty and no request goes out. | Coordinator |
| D9 | With the file policy source, the opt-in list comes from `[[tool_policy.org]] webhooks` (default empty), as `mode` does today (C16). An org with no entry has an empty list. | Coordinator |
| D10 | The adapter sets `Finding.detector` to the provider name. A `detector` field in the answer is a malformed answer. | Coordinator |
| D11 | A provider declares `kinds`, a closed list of 1 to 16 kinds. An answer with a kind outside the list is malformed. A declared kind must not be in `FINDING_KINDS` (the reserved kinds). A later release that reserves a new kind lists it in the changelog. A config that then declares it fails at boot. | Coordinator |
| D12 | The provider gets the call's arguments as the model sent them, without redaction, also when `secret_in_args` found a secret. An operator registers only providers that may see tool arguments. The runbook says so. | Coordinator |
| D13 | The request `findings` holds the built-in findings of that call, with their evidence: rule, location, pointer and host. Pointers and hosts are already secret-cut (C6). | Coordinator |
| D14 | `Finding.version` from a provider must match `[a-z0-9._-]{0,32}`. The pipeline checks it for every non-built-in stage, as D37 checks the kind. | Coordinator |
| D15 | The panic guard wraps the call: `guarded(.., async move { d.detect(b).await })`, for the webhook and the judge call sites. | Coordinator |
| D16 | The `Detector` trait gets `fn failure_severity(&self) -> Severity` (default `High`) and `fn kinds(&self) -> Option<&BTreeSet<String>>` (default `None`). The pipeline uses `failure_severity` for every pipeline-level failure: the cut, a panic, an `Err`, a wrong shape and the D37 `check` replacement. | Coordinator |
| D17 | Metrics: an injected `FindingLabels` value, built at boot from the config, replaces the const-only lookup. It adds `(kind, "webhook", sev)` for each declared kind and each severity, and `("detector_error", "webhook", sev)` for each severity, response side only, primed at zero. A new counter `gateway_tool_policy_webhook_failures_total{provider, reason}`: `provider` is the configured name (at most 8, operator-controlled, not tenant content), `reason` is the closed set of § 6.4. The finding counter gets no provider label. | Coordinator |
| D18 | Limits: at most 8 providers; a request body of at most `max_request_bytes` (default 1 MiB, at most `[tool_policy.detectors] max_scan_bytes`); a response body of at most 64 KiB; at most 16 findings per answer. | Coordinator |
| D19 | `url` must be `https`, with no userinfo and no query. `http` is accepted only for a host that `is_loopback_host` accepts (C22). Redirects are off. | Coordinator |
| D20 | `secret_env` names the env var with the secret, and `previous_secret_env` optionally names the one with the previous secret. The key is the exact bytes of the variable, with no decode. Each must hold at least 32 bytes and have no leading or trailing whitespace. A name must match `[A-Z][A-Z0-9_]{0,127}` and must not start with `GATEWAY_`. A missing, short or padded secret is a boot failure. The gateway signs requests with the current secret. It accepts a response that the current or the previous secret signs. | Coordinator |
| D21 | A provider's `timeout_ms` (default 300) must be at least 1 and at most `[tool_policy.detectors] timeout_ms - 50`. So a built-in stage and the judge (SMA-857) keep headroom. | Coordinator |
| D22 | The IAM write keeps the current list when the request leaves the field out: gRPC `WebhookOptIn webhooks = 4` (a message field has presence), REST `webhooks` absent or `null`. An empty list clears it. | Coordinator |
| D23 | The gateway sorts and de-duplicates the list from IAM, and the version token gets that list, so a change of the list alone reaches a cached tenant (C15). | Coordinator |
| D24 | `DetectBatch` carries the cut as `deadline`. The adapter bounds each call by `min(now + timeout, deadline - 10 ms)` and returns before the cut, with reason `deadline` for each call that it stopped. The pipeline cut stays as a backstop with `failure_severity`. Only the backstop path makes the pipeline count `deadline`. | Coordinator |
| D25 | Each provider has a required `orgs`: a list of org UUIDs, or the string `"*"` for a shared provider. An opt-in from an org outside the list sends no request and gives `detector_error` (`high`, that provider, reason `not_permitted`), and one `warn` per org and name per cache load. | Coordinator |
| D26 | A per-provider `max_in_flight` (default 64, 1 to 1024) caps the open requests across all holds of one gateway process. The adapter acquires a permit inside the call bound (D24). It spawns no task. When no permit is free within the bound, the call gets reason `overloaded`. The client stays on HTTP/1.1. `pool_max_idle_per_host = max_in_flight`, `pool_idle_timeout = 90 s`. | Coordinator |
| D27 | `retired = true` on a provider entry sends no request. Every call of an opted-in tenant gets `detector_error` with the provider's `failure_severity` and reason `retired`. An operator sets it, waits for tenants to opt out, then removes the entry. | Coordinator |
| D28 | The webhook wait time is not part of `gateway_tool_policy_overhead_seconds`. The sample subtracts the time of the late stages. `gateway_tool_policy_detector_seconds{stage="webhook"}` measures it. | Coordinator |
| D29 | IAM stores the opt-in list as JSON text in a new column, as `role.rs` does. Writing it needs no new sea-orm feature. | Coordinator |
| D30 | A bad opt-in list is a new IAM error reason `invalid-webhook-opt-in`, with the field `webhooks`. | Coordinator |
| D31 | The same action `ManageToolPolicySettings` guards the opt-in (§ 9, Q1). | Coordinator |

## 6. Design

### 6.1 Data flow

```
response side, one hold:
  CedarDecider ── DetectBatch{calls, principal, session, opt_in, deadline} ──> DetectorPipeline::run
     built-ins (sync, catch_unwind)
     ├── resolve opt_in: registered & permitted & not retired -> run; else a failure finding
     ├── for each provider that runs, at the same time (join_all):
     │      WebhookDetector::detect
     │        body per call built once, shared by every provider
     │        for each call (buffer_unordered(8), each bounded by D24):
     │          permit (max_in_flight) -> sign(name, t, nonce, body) -> POST
     │          200 + signature -> verify -> parse -> check kinds, version, count
     │        per call: findings, or [detector_error(name, failure_severity, reason)]
     └── judge (SMA-857, not here)
  pipeline backstop cut (timeout_at), D37 check, merge -> Cedar decide -> notice or allow
```

### 6.2 Config

```toml
[[tool_policy.detectors.webhook]]
name = "acme_dlp"                       # [a-z0-9_]{1,64}, unique, not reserved
url = "https://dlp.acme.example/v1/findings"   # https, no userinfo, no query
orgs = ["6f1c0000-0000-4000-8000-000000000001"]  # org UUIDs, or "*" for a shared provider
secret_env = "ACME_DLP_WEBHOOK_SECRET"          # not GATEWAY_*; the exact bytes are the key
# previous_secret_env = "ACME_DLP_WEBHOOK_SECRET_OLD"   # optional, for a rotation
timeout_ms = 300                        # default 300; 1 ..= [tool_policy.detectors] timeout_ms - 50
failure_severity = "high"               # info | low | medium | high; default high
kinds = ["pii_email", "pii_iban"]       # 1 to 16, [a-z0-9_]{1,64}, unique, not in FINDING_KINDS
max_request_bytes = 1048576             # default 1 MiB; at most [tool_policy.detectors] max_scan_bytes
max_in_flight = 64                      # default 64; 1 ..= 1024
retired = false                         # default false (D27)
extra_ca_bundle_path = "/etc/paigasus/acme-ca.pem"   # optional; ADDS to the trust store
```

- The table is a field `webhook: Vec<WebhookConfig>` of `DetectorsConfig`, with
  `#[serde(default)]`. `WebhookConfig` has `deny_unknown_fields`.
- **Reserved names** (a provider name must not be one): every name in `BUILT_IN_NAMES`
  (`kernel`, `built_in`, the five built-ins), `judge`, `webhook` and `other`.
- **Reserved kinds** (a declared kind must not be one): every kind in `FINDING_KINDS`.
- `extra_ca_bundle_path` doc text: "This ADDS to the trust store, it does not replace it. The
  client trusts the operating-system store and every certificate here. Roots only. Read once at
  boot. An unreadable, malformed or empty bundle is a hard boot failure." Same rules as
  `upstream.openai.extra_ca_bundle_path` (C8, C9).
- **Secrets and validation.** `validate` stays pure: it checks the env var names only. A separate
  step `load_webhook_secrets(&config, &dyn EnvReader)` reads the values at boot and checks D20.
  Tests inject a map, so they need no `set_var`. Both steps name the provider and the field in
  each error.
- Validation at boot, each a config error that names the provider and the field: name, URL
  (D19), `orgs` (each a valid UUID by `domain::parse_org_uuid`, or `"*"`, not empty), secret
  names (D20), timeout (D21), kinds (D11), limits (D18, D26), at most 8 providers, duplicate
  names, and two providers with the same `url`.
- With `[tool_policy.detectors] enabled = false`, the webhook list is still validated, but no
  pipeline runs (SMA-855 § 6.10).
- `[[tool_policy.org]]` gets `webhooks: Vec<String>` with `#[serde(default)]` (D9). It is used
  only when the policy source is not IAM. A name in it that is not registered, or whose `orgs`
  does not hold that org, is a config error at boot.
- The readiness dump: `WebhookConfig` serialises the name, the URL host, `orgs` count, the
  limits and `retired`. It never serialises a secret value or the URL path.
- `gateway.toml.example` gets a commented block. The config test that checks the example block
  (config.rs:2562) gets a webhook case.

### 6.3 The wire protocol (version 1)

**Request.** `POST <url>`, with these headers:

- `Content-Type: application/json`
- `Accept-Encoding: identity`
- `Paigasus-Webhook-Version: 1`
- `Paigasus-Nonce: <32 lower-case hex characters>`: 16 bytes from `OsRng`, new for each request.
- `Paigasus-Signature: t=<unix seconds>,v1=<64 lower-case hex characters>`, where
  `v1 = HMAC-SHA256(secret, "paigasus.request." || name || "." || t || "." || nonce || "." || body)`.
  `name` is the provider name, and `body` is the exact bytes sent. Marked sensitive in reqwest.

The body (the field order is fixed; a provider must not rely on it):

```json
{
  "version": 1,
  "request_id": "req_...",
  "subject": "prn:paigasus:iam::…",
  "org": "6f1c…-uuid",
  "session": "9a2e…-uuid",
  "tool": { "name": "send_email", "declared": "yes" },
  "args": { "to": "x@y.example", "body": "…" },
  "args_text": null,
  "findings": [
    { "detector": "secret_in_args", "kind": "secret_in_args", "severity": "high",
      "version": "v1",
      "evidence": [ { "rule": "github_pat", "location": "argument", "pointer": "/body" } ] }
  ]
}
```

- `request_id` and `session` are `null` when absent.
- `tool.declared` is `yes`, `no` or `unknown` (`Declared`).
- **Arguments.** When the input is `ToolInput::Json` and its text passes the kernel's strict
  parse, `args` is that text embedded verbatim as raw JSON (`serde_json::value::RawValue`), and
  `args_text` is `null`. So the provider sees the same bytes that the kernel parsed. In every
  other case, `args` is `null` and `args_text` is the raw argument text. That covers a
  `ToolInput::Text` input (the tool kinds `custom`, `apply_patch` and `other`) and a JSON input
  that fails the strict parse (bad JSON, a duplicate key, too much depth). Exactly one of the two
  is not `null`.
- `findings` is the built-in findings of this call (D13). An evidence entry has `rule`,
  `location` (`argument` or `text_input`), and `pointer` and `host` when present.
- The body of a call is built once and shared by every provider. Only the signature differs.

**Response.** The provider must answer `200` with the header
`Paigasus-Signature: v1=<64 lower-case hex>[,v1=<64 lower-case hex>]`, where
`v1 = HMAC-SHA256(secret, "paigasus.response." || name || "." || nonce || "." || body)`. One or
two `v1` values are allowed. The gateway accepts the response when any value verifies with the
current or the previous secret.

```json
{ "findings": [ { "kind": "pii_email", "severity": "medium", "version": "2026.11" } ] }
```

- `findings` is required. An empty list is a valid answer: no findings.
- Each finding has `kind`, `severity` and, optionally, `version` (absent means `""`). Any other
  field, a missing field, an unknown severity, a kind outside `kinds`, a version that fails D14
  or more than 16 findings make the answer malformed. Unknown top-level fields make the answer
  malformed too.

**The order of checks on a response:**

1. The status is 200. Anything else, a 3xx included, is `status`.
2. `Content-Encoding` is absent or `identity`. Anything else is `malformed`.
3. The body is read as a stream and cut at 64 KiB. Over the cap is `response_too_large`.
4. There is exactly one `Paigasus-Signature` header, and it is well-formed. A second header or a
   bad form is `signature`.
5. Some `v1` value verifies in constant time (`Mac::verify_slice`) with the current or the
   previous secret. Else `signature`.
6. `Content-Type` is `application/json`, parameters allowed. Else `malformed`.
7. The JSON parse and the field checks. Else `malformed`.

The body is not parsed before the signature is verified.

**Rotation order** (the protocol doc): (1) the provider accepts requests signed with the old or
the new secret; (2) the operator sets `secret_env` to the new and `previous_secret_env` to the
old secret, and rolls the gateway; (3) the provider signs responses with the new secret; (4) the
operator removes `previous_secret_env`.

The provider is responsible for a replay window on `t`. The gateway sends no request twice (N6).
The protocol doc suggests a window of 5 minutes.

### 6.4 Failures

Each failure gives the affected call one `detector_error` finding with detector = provider name,
severity = `failure_severity` and `version` = the reason (D4). Other calls keep their own answers
(A10).

| Failure | Reason | Where | Scope |
|---|---|---|---|
| The request body is over `max_request_bytes` | `request_too_large` | adapter | the call |
| No `max_in_flight` permit within the call bound | `overloaded` | adapter | the call |
| Connect, transport or TLS error | `transport` | adapter | the call |
| The provider's `timeout_ms` passes | `timeout` | adapter | the call |
| `deadline - 10 ms` passes first (D24) | `deadline` | adapter | the call |
| A status other than 200, a redirect | `status` | adapter | the call |
| The response body is over 64 KiB | `response_too_large` | adapter | the call |
| The signature header is missing, doubled, malformed or does not verify | `signature` | adapter | the call |
| An encoding, content-type, JSON or field check fails | `malformed` | adapter | the call |
| The pipeline backstop cut passes | `deadline` | pipeline | every call of that provider |
| A panic, an `Err`, a wrong shape | `panic`, `error`, `shape` | pipeline | every call of that provider |
| The D37 `check` drops a finding | `check` | pipeline | the call |
| An opt-in name that is unknown or invalid (D7) | `unknown_provider` | pipeline | every call |
| An org outside `orgs` (D25) | `not_permitted` | pipeline | every call |
| A retired provider (D27) | `retired` | pipeline | every call |

- D7 and D25 findings are `high`. The others use the provider's `failure_severity` (D16).
- Every failure finding gets `Stage::Webhook` in `out.stages`, so the counters label it
  `webhook` (C12).
- The adapter writes a `warn` event with `detector` (the provider name), `reason` and
  `request_id`, rate-limited to one per provider and reason per 10 s, with a suppressed count.
  It never writes a body, a header value, a URL path or query, an argument or the `Display` of a
  `reqwest::Error` (it contains the URL) (A4).
- The counter `gateway_tool_policy_webhook_failures_total{provider, reason}` counts each failed
  call once. For D7, `provider` is `unknown`.
- A failure in `log_only` mode: the floor does not read `max_severity` (SMA-855 N2), so a
  failure finding cannot block a `log_only` tenant.

### 6.5 The adapter

`adapters/detectors/webhook.rs`:

```rust
pub struct WebhookDetector {
    name: String,
    url: reqwest::Url,
    secret: SecretString,
    previous_secret: Option<SecretString>,
    timeout: Duration,
    failure_severity: Severity,
    kinds: BTreeSet<String>,
    max_request_bytes: usize,
    in_flight: Arc<tokio::sync::Semaphore>,   // max_in_flight
    client: reqwest::Client,                  // one per provider: its own CA bundle and pool
    clock: Arc<dyn Clock>,                    // for `t`; injected for the worked-example test
    nonce: Arc<dyn NonceSource>,              // OsRng in production; fixed in the test
}

#[async_trait]
impl Detector for WebhookDetector {
    fn name(&self) -> &str { &self.name }
    fn failure_severity(&self) -> Severity { self.failure_severity }
    fn kinds(&self) -> Option<&BTreeSet<String>> { Some(&self.kinds) }
    async fn detect(&self, batch: Arc<DetectBatch>) -> Result<Vec<Vec<Finding>>, DetectorError>;
}
```

- `detect` builds one future per call subject, each with its index, and drives them with
  `futures::stream::iter(..).buffer_unordered(8)` (D1). It puts each result back by index. It
  spawns no task (A8).
- Each call future runs inside `tokio::time::timeout_at(min(now + timeout, batch.deadline -
  10 ms), ..)`. It distinguishes `timeout` from `deadline` by which bound was smaller (D24).
- `detect` never returns `Err`. Every failure is a finding per call (§ 6.4). It returns one vector
  per subject in subject order.
- The call bodies: the pipeline builds them once per batch (`Arc<[Bytes]>`, one per call) and
  puts them into the batch as `webhook_bodies`, so the providers share them. A body over
  `max_request_bytes` of one provider fails only for that provider.
- The client builder: `build_client` is split, so that a new function
  `build_extra_ca_client(label: &str, extra_ca_bundle_path: Option<&str>, connect_timeout)`
  shares the bundle loader and the attribution with the upstream clients. The upstream callers
  do not change. The error names the provider and the path. The webhook client adds
  `pool_max_idle_per_host` and `pool_idle_timeout` (D26). No request-level read timeout is
  needed: the call bound covers it.
- Hex encoding and decoding are hand-written (no new dependency).
- `WebhookDetector` has a hand-written `Debug` that prints the name and the URL host only.

### 6.6 The tenant opt-in

**Proto** (`contracts/proto/paigasus/iam/v1/iam.proto`):

```proto
message ToolPolicySettings {
  ...
  // SMA-856: the webhook providers that this tenant opts in to, sorted, unique. The gateway
  // decides which of them may run for this org (the provider's `orgs`).
  repeated string webhooks = 5;
}

message WebhookOptIn {
  repeated string names = 1;
}

message PutToolPolicySettingsRequest {
  ...
  // SMA-856: absent keeps the current list; present replaces it; an empty list clears it.
  WebhookOptIn webhooks = 4;
}
```

`buf format -w`, then `moon run contracts:generate`. `:breaking` passes: both are additions.

**IAM:**

- Migration m0016: `ALTER TABLE tool_policy_settings ADD COLUMN webhooks TEXT NOT NULL DEFAULT
  '[]'`, JSON text (D29). The down migration drops the column. The runbook records the rollback
  hazard: a down migration or an older IAM gives every tenant an empty list. Then no provider
  runs and no finding shows it.
- `ToolPolicySettings` (iam-core `model.rs`) gets `webhooks: Vec<String>`. The absent-row
  default is empty.
- `put_settings` gets `webhooks: Option<Vec<String>>`. It validates each name against
  `[a-z0-9_]{1,64}` and rejects more than 16 distinct names, with `invalid-webhook-opt-in`
  (D30). It stores the list sorted and without duplicates. The mode transition rules do not
  change.
- The repository's upsert takes `webhooks: Option<Vec<String>>`. With `None`, the column is not
  in the insert's update columns, so a concurrent write of the list is not undone (R7). A new row
  with `None` gets the default `[]`.
- The audit detail gets `webhooks_before` and `webhooks_after` (only when the request carries the
  field). The event payload of `iam.policy.tool_settings.changed` gets `webhooks`.
- `ManageToolPolicySettings` guards the write (D31). The error code goes through
  `:error-code-single-site`.
- `settings_to_pb`, the REST DTO and `GetToolPolicySettings` carry the list. In REST, an absent
  key and `null` both keep the list.
- Rollout note: an older IAM ignores the unknown REST key and returns 200 without storing it.

**Gateway:**

- `TenantSettings` gets `webhooks: Arc<[String]>`. It loses `Copy`. It keeps `Clone` and `Eq`.
- The IAM source maps `settings.webhooks`, sorts and de-duplicates it (D23). A name that fails
  the IAM rule stays in the list. It becomes an `unknown_provider` finding later (D7, R6). It is
  never `malformed`.
- The version token becomes `t{set.version}:{mode}:{fail}:{webhooks joined with ","}`.

### 6.7 The pipeline changes

**The opt-in path** (fixed, R5):

1. `PolicySnapshot` gets `webhooks: Arc<[String]>`. The request side sets it when it builds the
   snapshot. It holds the tenant's list only under the D8 rule (mode `log_only` or `enforce`, and
   `TenantPart::Evaluated`). Else it is empty.
2. `call_batch` (decider.rs:265) reads it from `ctx.policy` and gives it to the batch.
3. A new response-side constructor `DetectBatch::for_calls(org, request_id, subjects, seen_hosts,
   caller: Caller)` takes `Caller { principal: Arc<str>, session: Option<Uuid>, opt_in:
   Arc<[String]> }`. `DetectBatch::new` stays for the request side and the tests, with an empty
   `Caller`.
4. Both pipeline rebuilds (pipeline.rs:162-171, 196-205) copy `caller`, `deadline` and
   `webhook_bodies` from the source batch. A unit test fails when a rebuild drops one of them.

**`DetectBatch` new fields:** `caller: Caller`, `deadline: Instant` (set by `run` to the cut) and
`webhook_bodies: Option<Arc<[Bytes]>>` (set by `run` before the webhook stage).

**`late_stages`:**

- It resolves each name of `batch.caller.opt_in`, in sorted order:
  - registered, `orgs` holds `batch.org`, not retired → the provider runs;
  - registered, not permitted → `not_permitted` findings;
  - registered, retired → `retired` findings;
  - not registered or not valid → `unknown_provider` findings.
- A registered webhook that is not in the opt-in list does not run (A5).
- D15: `guarded("webhook", async move { d.detect(b).await })`. The judge call site gets the same
  change.
- D16: `failed(stage, name, err, m)` takes `d.failure_severity()`. The `check` replacement takes it
  too.

**The D37 `check`, for the webhook stage:** the finding's detector must equal the webhook's own
name; the kind must be in `d.kinds()` when it is `Some`; the version must pass D14. The adapter
already checks all three, so this is the second line. The tests of C3 that assert the permissive
rule are rewritten to assert the strict rule.

### 6.8 Metrics and the decision log

- `FindingLabels` (D17): a value built at boot from the built-in const table plus the webhook
  config. `CedarDecider`, `RequestToolPolicy` and `prime_tool_policy_metrics` take it by
  injection. A kind that two providers declare is one label triple. Worst case: 8 × 16 × 4 + 4 =
  516 finding series on the response side.
- `gateway_tool_policy_webhook_failures_total{provider, reason}`: primed at zero for every
  configured provider and every adapter reason. At most 8 × 15 + 3 series. New name in
  `paigasus_observability::names` and `names::ALL`, a registry test, a `describe_counter!` line,
  the row in `docs/ops/RUNBOOK-observability.md`, and `repo:observability-drift`.
- D28: the overhead sample subtracts the late-stage time. The runbook line 596 states that the
  15 ms limit excludes webhook wait time.
- The decision log needs no schema change: a webhook finding is a `FindingRecord` with detector =
  the provider name, its kind, severity and version. A failure finding carries the reason in
  `version` (D4). No evidence.

### 6.9 Docs

- `docs/ops/RUNBOOK-tool-policies.md`, a section "Webhook providers":
  - register a provider, with `orgs`, the secret env, the CA bundle (it adds), `failure_severity`;
  - the arguments and the caller PRN go to the provider unredacted (D5, D12);
  - the reserved names and kinds;
  - the failure reasons and the counter;
  - Cedar sees only the kinds and `max_severity`, not the provider name, so a `low` failure
    finding does not trigger a "block on high" policy;
  - more than 8 calls in a hold queue per provider, and a slow provider can reach `deadline`;
  - cold connections: HTTP/1.1 with a pool; the first requests after an idle period pay a TLS
    handshake;
  - the rollout order: add a provider to every pod before a tenant opts in; to remove one, set
    `retired = true`, tell the tenants to opt out, then remove the entry (D27);
  - the secret rotation order (§ 6.3);
  - the IAM rollback hazard (§ 6.6).
- A new `docs/tool-policies/webhook-protocol.md`: the wire protocol of § 6.3 for provider authors,
  with the key-bytes rule and a worked example (fixed secret, name, nonce, `t` and bodies, and the
  expected hex for the request and the response), which a unit test checks byte for byte.
- `docs/tool-policies/tenant-guide.md`: the opt-in; a policy example with
  `context.findings.contains("pii_email")`; that an opt-out can take up to `max_stale_secs` during
  an IAM outage under fail mode `open` (C15).
- `paigasus-gateway/CHANGELOG.md` and the IAM changelog, with the rollout note of § 6.6.

## 7. Tests

Timing rule for all tests: the detectors `timeout_ms` is 2000 and `keepalive_secs` is 1 where a
test needs a keep-alive. Parallelism is proved with a barrier in the mock, not with wall time.

### 7.1 Adapter unit tests (`adapters/detectors/webhook.rs`)

- The request and the response signatures match the worked example of the protocol doc byte for
  byte (fixed clock and nonce).
- The response checks, each with its reason: one changed body byte, a signature over another
  nonce, a signature over another provider name, a missing header, two headers, an upper-case
  hex, a short hex → `signature`; a response signed with the previous secret → accepted; a
  `Content-Encoding: gzip` → `malformed`; `application/json; charset=utf-8` → accepted.
- The response parse: unknown field, missing `findings`, unknown severity, undeclared kind, bad
  version, 17 findings, a `detector` field → `malformed`.
- The arguments: a duplicate key, a 25-digit number and invalid JSON. The first two are sent as
  `args_text`, because they fail the strict parse; a strict-parsed input with a 19-digit number
  is sent verbatim (byte-equal).
- `buffer_unordered`: 10 calls, call 0 slow, calls 1-9 fast. Calls 1-9 finish before call 0
  (a barrier proves the order).
- `overloaded`: `max_in_flight = 1` and two calls against a mock that holds the first.
- A `Debug` of `WebhookDetector` and of a request holds no secret, no body, no URL path and no
  argument (canary strings).

### 7.2 Pipeline tests (`pipeline.rs`)

- D15: a detector whose `detect` panics before it returns a future gives `detector_error` with its
  `failure_severity`, and the other webhook still adds its findings.
- D16: at the backstop cut, a slow detector with `failure_severity = low` gives a `low`
  `detector_error`.
- D24: an adapter-like detector that honours `batch.deadline` returns per-call results: 2 calls
  answered, 1 stopped with `deadline`.
- D7, D25, D27: an unknown name, a name outside `orgs` and a retired provider each give their
  reason, severity and `Stage::Webhook`.
- A7: a webhook finding under another detector name, a built-in kind or an undeclared kind is
  dropped and gives `detector_error` with `failure_severity`.
- A5: a registered webhook not in `opt_in` gets no call (a `RecordingDetector`).
- R5: each pipeline rebuild keeps `caller`, `deadline` and `webhook_bodies`.

### 7.3 End-to-end tests (`tests/tool_policy.rs`, `mod detectors`, with a mock provider)

The mock provider is an axum server on `127.0.0.1` that verifies the request signature, records
each request and answers with a scripted response that it signs. The tenant policy is the
starter policy (blocks on `high`) plus `forbid (...) when { context.findings.contains("pii_email") };`.

- **Good provider:** it answers `pii_email` (`medium`). The call is denied. The decision log lists
  the finding with detector `acme_dlp`. The mock saw a valid request signature, the principal, the
  session and the built-in findings. **Negative control:** the same policy, the provider answers
  `{"findings": []}`, and the call passes.
- **Bad signature:** the provider signs with the wrong secret. The call gets `detector_error` from
  `acme_dlp` with `version = "signature"`. With `failure_severity = high`, the call is blocked.
- **Slow provider and the failure severity:** it sleeps past `timeout_ms`. With
  `failure_severity = high` the call is blocked. With `failure_severity = low` the call passes,
  and the decision log holds the `low` `detector_error` with `version = "timeout"`. With
  `keepalive_secs = 1` and a provider that answers in 1.5 s, the hold sends a keep-alive.
- **Deadline per call:** a hold with 3 calls; the mock answers calls 1 and 2 at once and holds
  call 3 past the pipeline cut. Calls 1 and 2 keep their findings, call 3 gets `deadline`.
- **Malformed and other reasons, each with the finding and the counter label:** invalid JSON, an
  undeclared kind (`malformed`); a status 500 and a 302 (`status`); a 70 KiB response
  (`response_too_large`); a request over `max_request_bytes` (`request_too_large`, and the mock
  records no request); a refused connection to a closed port (`transport`).
- **Two providers, one slow:** the fast one adds its finding, the slow one gives its failure
  finding. A barrier in the mock proves that both requests were open at the same time.
- **Opt-in:** a tenant without the opt-in sends no request. A tenant in mode `off` sends no
  request (D8). A tenant whose set is unavailable sends no request (D8). An opt-in of an unknown
  name gives `high` `detector_error` with `unknown_provider`. An org outside `orgs` sends no
  request and gives `not_permitted` (R2). A retired provider sends no request and gives
  `retired`.
- **Leaks:** a canary in the arguments, the secret and the response body appear in no captured
  `tracing` event, no decision log record, no metric label and no readiness dump.
- **Metrics:** the failure counter is primed at zero for every configured provider and reason
  before the first request.

### 7.4 Runtime and IAM-source tests

- `runtime::build_tool_policy` with two providers gives a pipeline with two webhooks in config
  order (it no longer passes `Vec::new()`).
- `tests/tool_policy_iam_source.rs`: the fake IAM returns `webhooks = ["acme_dlp"]`, and the mock
  provider records a request. The fake IAM then returns an empty list with a new token, and the
  requests stop.
- `adapters/policy_source/iam.rs`: a list change alone changes the version token; an unsorted or
  doubled list gives the same token as the sorted one; a bad name in a response keeps the tenant
  set and is not `malformed`.

### 7.5 CA bundle tests

Follow SMA-558 (C18): a boot with an unreadable, malformed or empty bundle fails and names the
provider. A client built with an `rcgen` CA bundle trusts that CA, and the control build without
it does not.

### 7.6 Config tests

Each validation rule of § 6.2 has a failing case that names the provider and the field. The
secret loader uses an injected env map: a missing, short, padded and `GATEWAY_`-prefixed secret.

### 7.7 IAM tests

- `tool_policies_settings_pg.rs`: the migration; a round trip of the list; the absent row gives an
  empty list; a concurrent write with the list absent does not undo a write of the list (like the
  `first_enabled_at` test).
- `grpc_tool_policies.rs` and `http_tool_policies.rs`: absent keeps, `null` keeps (REST), present
  replaces, empty clears, sorted and unique on read, 17 names and a bad name are
  `invalid-webhook-opt-in`.
- The audit detail and the event payload hold the lists.

### 7.8 Repo gates

`moon ci` with the full target list of the root `CLAUDE.md`, in particular `:breaking`,
`:observability-drift`, `:error-code-single-site`, `:deny` and `:machete` (no new dependency is
expected).

## 8. Left to the builder

- The internal layout of `FindingLabels` and of the rate limiter for the `warn` events.
- The trait names `Clock` and `NonceSource`, or the reuse of an existing clock port.
- The exact place where the request side sets `PolicySnapshot.webhooks`, within the rule of § 6.7.

## 9. Answers to the challenge questions

- Q1. The action: `ManageToolPolicySettings` guards the opt-in (D31). The opt-in sends data only to
  providers that the operator registered for that org (D25). **Sven can choose a new action
  at Gate 1.**
- Q2. How a tenant learns the names: the operator tells the tenant. No API lists the providers,
  so no tenant sees another tenant's vendor (N4).
- Q3. An egress proxy and mTLS: not in this change (N10). A provider that needs one waits for a
  follow-up.
- Q4. The failure reason in the decision log: yes, in the `version` of the failure finding (D4).
- Q5. A floor rule on `max_severity` in `log_only`: the floor reads only `name_collision` (SMA-855
  N2), so a provider failure cannot block a `log_only` tenant. A later floor change must keep
  this in mind.
- Q6. The principal: a PRN. It goes to the provider (D5), and the runbook says so.
- Q7. The file source with no `[[tool_policy.org]]` entry: an empty list (D9).

## 10. Follow-ups that this change does not do

- Provider registration in IAM, so that IAM can check an opt-in name (N3).
- A console UI for the opt-in (N4).
- Webhooks in the agent host (SMA-853, N9).
- An argument redaction per provider (N8).
- An egress proxy, mTLS and HTTP/2 for providers (N10).
