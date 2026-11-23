# SMA-855: tool-call detectors, part 1 (findings with severity, the detector port, five built-ins)

Linear: SMA-855, part 1 of 3 of the parent SMA-754 "tool-call detectors". ADR-0027, decisions (h)
and (i), with (c), (d), (e) and (f). Builds on SMA-752 (the Cedar decision), SMA-755 (the decision
log) and SMA-776 (blocked structured results). Later parts: SMA-856 (the webhook detector) and
SMA-857 (the judge model). Related: SMA-853 (the agent host screens inbox events with these
detectors), SMA-751 (the IAM tool policy sets and their Cedar schema).

Status: revision 3, after the builder check, the first challenge and the second challenge
(APPROVE WITH CHANGES). Sven answered QA and QB on 2026-11-22.

## Revision 4 (build) amendments

These items changed during the build. Each line gives the change, the reason and the task.

- A1. § 6.3.2: raw-mode argument text also decodes `\uXXXX` (surrogate pairs; a lone surrogate gives U+FFFD) and `\\`, not only `\/`. Reason: an escaped host failed open on a deny list. Task 3.
- A2. § 6.3: a URL candidate with an authority longer than 1024 bytes marks the host set as overflowed, as the 256-host cap does. The hosts-cap `detector_error` reports it. Reason: bounded, linear parsing that fails closed. Task 3.
- A3. § 6.3.4: `unicode_tags` counts every tag character (U+E0020 to U+E007E) outside an exempt emoji tag sequence in the whole text. A total of 4 or more is a match. The tag characters are joined and every rule scans the joined text. Reason: a payload split into runs of 3 or fewer evaded every rule. Task 6.
- A4. § 6.3.4: only the RGI subdivision flags `gbeng`, `gbsct` and `gbwls` are exempt. Every other flag-form sequence counts. Reason: a payload hid in fake flags. Task 6.
- A5. § 6.2.1: `Evidence::argument` and `Evidence::host` cut every pointer before the first token that matches the secret set. The walk computes the cut once per key. Reason: no evidence copy holds a secret, and SMA-856 sends evidence to webhooks. Task 7.
- A6. § 6.11.1 and § 6.11.2: proto `CedarDetail` gets `bool detection_disabled = 9`. The spec said there was no proto field. Reason: a detection-off decision otherwise replays with a derived `max_severity`, not the -1 the kernel used. Task 11.
- A7. § 6.3.5: an OpenAPI `nullable: true` skip is added. An `enum` of non-integer numbers, or of integers spelled with an exponent or `.0`, gives no signal. Reason: anti-goal A7 holds without the serde_json `float_roundtrip` feature. Task 5.
- A8. § 6.8.2: F(v) in the differential predicate holds only FORBID policies: `v.determining_forbids` and `v.errored_forbids`. The kernel has a new field `Verdict.determining_forbids`. Reason: the kernel keeps satisfied permits in `determining` when an errored forbid denies, which gave a false replacement. Task 15.
- A9. § 6.8.2: an optimistic-only replacement takes the detail of the call that the optimistic path denied. The spec said the detail of the first earlier call. Reason: the record keeps the enforcing floor policy. Task 15.
- A10. § 6.3: a wrong scans count gives every call a `detector_error` (detector `built_in`) and no scan hosts. Reason: A3 fails closed. Task 13.
- A11. § 6.13.3: an IAM rollback does not make held rows "different". The code returns `older_replica` and writes nothing. The runbook follows the code. Task 17.
- A12. § 6.10: Anthropic `search_result` blocks and text or content `document` blocks are scanned. `tool_reference` is a part with no text. Base64, URL and file documents, Responses `input_file`, object content and unknown part types are "not scanned" and give a `detector_error`. Reason: fail closed without false replacements on normal RAG traffic. Task 22.
- A13. § 6.8.2: one request decides each distinct differential pair once (a cache keyed by digests of the call and of the finding set), and at most `PAIR_BUDGET` = 64 pairs. Past the budget, each target with findings that is not yet decided gets one `detector_error` (detector `built_in`, high), is not evaluated, and is replaced (enforced in `enforce` mode or with a floor-only set; an orphan only gets the screen record). Reason: the client controlled about 2 × N × N Cedar decisions. Task 15 (PR 534 review).
- A14. § 6.8.2: the record cites the first enforced differing pair, else the first differing pair (Sven, 2026-11-23).

## Revision 3 changelog

- S1. § 6.8.0, D27: the request-side gate is mode-based. Mode `log_only` or `enforce` with the
  tenant evaluated always screens. Floor-only screens only when the floor reads a screen-dependent
  kind. One constant holds the two kind lists. The `FloorPolicies` flag is removed. With no screen,
  `unseen_host` does not run. (challenge 2)
- S2. § 6.8.2, D12: one replacement predicate,
  `A.decision == Deny && (B.decision == Allow || F(A) ⊄ F(B))`. `Verdict` gets `errored_forbids`.
  The examples are added, the short-circuit case of step 5 is fixed, and a result in two targets
  adds its findings to both. (challenge 2)
- S3. § 6.9, D35: the pipeline gets an absolute deadline from the hold start. The margin is
  provisional and the builder measures it. A two-hold stream test is added. The overhead metric
  includes the pipeline time. (challenge 2)
- S4. § 6.13.2, D28: the additive rule is a separate `outputs_equivalent_for_recompile`, used only
  by the re-compile job, and defined on the schema grammar. `outputs_identical` stays exact.
  (challenge 2; Sven's intent kept)
- S5. § 6.13.3: the IAM rollback and rolling-deploy effects are stated, with a pre-rollback query
  and a post-rollout re-compile trigger. A forward-compatible `is_current` is a follow-up.
  (challenge 2)
- S6. § 6.11.2: the decision-log read path accepts `FINDING_SEVERITY_UNSPECIFIED`, derives
  `max_severity` on decode, and an older export counts `SCREEN_RESULT` as unmappable. A
  round-trip test with a pre-SMA-855 event is added. (challenge 2)
- S7. § 6.2.4, § 7.4: `catch_unwind` cannot stop the panic hook. No global hook is installed. The
  stderr assertion is removed, and the residual is recorded. (challenge 2)
- S8. § 6.2.3: `ToolInvocation` carries an optional precomputed `HostSet`, so `decide` does not
  scan again. A no-panic property test for `call_hosts` is added. (challenge 2)
- S9. § 6.3.3: URL forms are matched by `(?i)(https?|wss?|ftp):[/\\]*`, and `Url::parse`
  normalises them. The guide states the limits of an allow list. (challenge 2)
- S10. § 6.3.3, D36: the hosts-cap sentinel is `<overflow>`, which no URL parser accepts as a host.
  The guide pairs a deny list with `detector_error`. (challenge 2)
- S11. § 6.3.5, § 6.4, D20: only `unicode_tags` stays `high`, and it excludes valid emoji tag
  sequences. The other structural rules are `medium`. (challenge 2)
- S12. § 6.10: the default `max_scan_bytes` is 16 MiB, the largest request cap. Results are scanned
  first, newest first, then messages. (challenge 2)
- S13. § 6.8.1: the screen uses the same walker as `parse_request`, with a text sink. A key
  mismatch or a screen failure gives `detector_error` on every result. (challenge 2)
- S14. Several sections: the kernel-added findings reach the log; `built_in` is a known name; the
  seen hosts travel in `PolicySnapshot`; the plan's record lists are defined; D14 is defined for
  `log_only`; the record-literal release note; the perf fixture has findings; the
  `#[non_exhaustive]` reason is fixed; the webhook stage gets call subjects only. (challenge 2)
- S15. § 10, D38 to D41: the four questions of the challenge are answered. (challenge 2)

## Revision 2 changelog

- R1. § 6.8.2, D12: the result decision is differential. (challenge)
- R2. § 6.11.2, D31: request-side log records for results with findings. (challenge)
- R3. § 6.8.0, D27: the request-side scan is gated. The O(history) cost is stated. (challenge)
- R4. § 6.6.2, § 6.9, D30, D35: a hard deadline and a margin. A8 is "no `tokio::spawn` in our
  code". (builder check)
- R5. § 6.6.2, D22: the built-in stage runs under `catch_unwind`. (challenge)
- R6. § 6.2.4, D37: hand-written `Debug`, validated kinds and detector names. (challenge)
- R7. § 6.3.5: `injection_markers` evidence is the rule id and the result index. (challenge)
- R8. § 6.3.2: `prefixItems`, instance types, a work budget. (challenge)
- R9. § 6.3.3, D36: host extraction rules and the hosts cap. (challenge)
- R10. § 6.10: the scan budget and its order. C20 is corrected. (builder check)
- R11. § 6.10, § 6.13.3: `enabled`, and the deploy and rollback order. (challenge)
- R12. § 6.6.1: `DetectBatch` fields belong to SMA-857. `stage()` is removed. (builder check)
- R13. § 6.8.1, § 6.9: no seen set, and the hosts of the result decision. (builder check)
- R14. Several sections: `Findings::insert` removed, the parser moved, constructors, `Arc`
  schemas, provider-run results a non-goal, fixtures, the duplicate-key scan. (builder check)
- R15. § 6.12: a correct fail-open form. (challenge)
- R16. § 5: questions became decisions. (builder check)
- R17. D28: QA resolved by Sven, option (1). (challenge BLOCKER 2, Sven)
- R18. D22: QB resolved by Sven, `high`. (Sven)

## 1. Intent

The gateway decides every tool call with Cedar (SMA-752), and it logs each decision (SMA-755).
Today the only findings are `name_collision` and `unparsed_arguments`. Both are kinds with no
severity. A tenant cannot write a policy about a leaked secret, a call to an unknown host, or a
prompt injection in a tool result. The gateway does not look for them.

This change adds the detectors. A detector reads a tool call or a tool result and gives
findings. A finding has a kind, a severity and the name of its detector. Cedar reads the findings
in `context`. ADR-0027 (h) says: "Detectors produce findings. Only Cedar decides." So a finding
never blocks by itself. It blocks only when a policy reads it and denies.

The first part gives the types, the port, the pipeline and five built-in detectors. The built-ins
are pure functions in `paigasus-kernel`. So SMA-853 can use the same functions in the agent host
(ADR-0005). The two later parts (a webhook and a judge model) plug into the same pipeline.

## 2. Goals, non-goals, anti-goals

### Goals

- G1. Kernel types `Severity` and `Finding`, and a `Findings` set that keeps the highest severity.
  They are behind a new optional kernel feature `detectors`.
- G2. Five built-in detectors in the kernel: `undeclared_tool`, `invalid_args`, `unseen_host`,
  `secret_in_args` and `injection_markers`. They are pure and synchronous, with no I/O.
- G3. Two new Cedar context attributes: `context.max_severity` (a `Long`) and
  `context.call.hosts` (a set of strings). `context.findings` stays a set of kinds.
- G4. An async `Detector` port and a `DetectorPipeline` in the gateway service. The running order
  is: built-ins, then webhooks in parallel, then the judge. This part has only the built-ins.
- G5. Tool results get a real Cedar decision with their own findings. Under a deny, the existing
  notice replaces the result.
- G6. The decision log, the proto and the metrics carry the detector, the kind and the severity.
  They never carry evidence.
- G7. A documented and tested starter policy in the tenant guide, which blocks on high severity.
- G8. The SMA-752 overhead budget holds: p99 below 15 ms on non-streaming requests with 50
  policies, with the detectors on.
- G9. The IAM Cedar schema knows the two new attributes, so that IAM accepts a policy that reads
  them. A re-compile after a schema change that only adds attributes does not hold the live sets
  (D28).

### Non-goals

- N1. The webhook detector (SMA-856) and the judge model (SMA-857). This part leaves a clear place
  for them in the port and the pipeline (§ 6.6).
- N2. A change to `floor.cedar`. The floor stays one policy on `name_collision`.
- N3. A finding that blocks without a policy. `BlockCode::Detector` stays unused.
- N4. An entropy heuristic for secrets. `secret_in_args` uses known token prefixes only.
- N5. The agent host `EventScreen` adapter (SMA-853). This part gives the kernel functions that
  it will call.
- N6. Detection in images, audio or other non-text content.
- N7. Per-tenant detector settings. The built-ins have no tenant settings. A tenant controls the
  effect only with policies.
- N8. Bindings for the detectors (py, node, wasm).
- N9. Results of provider-run tools: Anthropic `web_fetch`, `web_search` and MCP tool results,
  and the output of a Responses `mcp_call`. A follow-up (§ 10) covers them.
- N10. A cache of earlier scan results (for example a digest per tool result). A follow-up covers
  it.
- N11. A forward-compatible `is_current` in IAM (§ 6.13.3). A follow-up covers it.
- N12. A global panic hook (§ 6.2.4).

### Anti-goals (a failure even if the code works)

- A1. A finding blocks a call or a result when no policy reads it.
- A2. Evidence, a tool-result text, a secret, an argument, a host or any part of them reaches the
  decision log, the proto, a metric label, a `tracing` event, a panic message of our code, a
  `Debug` output or the decision report. § 6.2.4 records the one residual (a panic message of a
  dependency on stderr).
- A3. A detector error, a timeout or a panic lets a call go without a finding.
- A4. A missing attribute makes a tenant `forbid` on `context.max_severity` deny a call that has
  no findings.
- A5. The kernel gets a dependency on a gateway type, or a binding crate gets the `detectors`
  feature or Cedar.
- A6. A definition is removed when some input could make the call allowed (SMA-752 § 6.1.6). The
  new attributes must not break that soundness.
- A7. `invalid_args` gives a finding for a value that the full JSON Schema accepts.
- A8. Our code calls `tokio::spawn` (or another task spawn) for the detectors (SMA-749 D25).
  Tasks inside hyper or other libraries are outside this rule.
- A9. A finding that the policies cannot see changes a result decision. For example a
  request-level `name_collision` replaces a result only because the result has another finding.

## 3. Constraints found in the repo

- C1. `decide(call, facts, findings, policies) -> Verdict` is at
  `rs/crates/libs/paigasus-kernel/src/tool_policy/decide.rs:133`. `context()` at `decide.rs:71`
  builds `{ input, input_text?, findings, call: { declared }, request, session }`. There is no
  `context.call.hosts` and no `context.max_severity`.
- C2. `Findings(BTreeSet<String>)` is at `tool_policy/types.rs:143`. The constants
  `NAME_COLLISION` and `UNPARSED_ARGUMENTS` are at `types.rs:12` and `types.rs:14`. `ToolInput` is
  `Json(String)` or `Text(String)`, not a parsed map (`types.rs:17-24`). `Declared` is
  `Yes`, `No` or `Unknown` (`types.rs:29-37`).
- C3. The kernel adds `unparsed_arguments` itself in `context()` (`decide.rs:76-79`), for every
  `ToolInput::Text` and for JSON that does not parse strictly. The strict parse refuses duplicate
  keys at any depth (`tool_policy/input.rs:131-147`). `input.rs` imports `cedar_policy`
  (`input.rs:7`), so its parser and `classify_number` are not usable without Cedar today.
- C4. The optimistic set of `decide_tool` drops every `forbid` that reads `context.input`,
  `input_text`, `findings`, `call` or `session` (`tool_policy/optimistic.rs:47-53`). An attribute
  that the walk does not know counts as a read of all parts (`optimistic.rs:63-76`).
- C5. The kernel feature `tool-policy` is optional (`paigasus-kernel/Cargo.toml`). Its users are
  `paigasus-gateway-core`, `paigasus-gateway` and `paigasus-iam-core`. `paigasus-agent-host-core`
  and the three binding crates use the kernel without it. `repo:wasm-getrandom-free` asserts that
  no binding crate pulls `cedar-policy` (`moon.yml:438-465`).
- C6. `regex` 1.13.2, `aho-corasick` 1.1.5 and `url` 2.5.8 are in `rs/Cargo.lock`. `url = "2"` is a
  workspace dependency (`rs/Cargo.toml:286`). `regex` is not a workspace dependency.
- C7. `name_collisions` and `call_findings` are at `paigasus-gateway-core/src/policy/mapping.rs:23`
  and `:76`. `invocation` is at `mapping.rs:41`.
- C8. `plan_request` is at `policy/plan.rs:213`. Its `ResultJudge` (`plan.rs:146`) decides a tool
  result only through the optimistic `decide_tool`, by tool name (`plan.rs:136`), and caches one
  outcome per target key (`plan.rs:151`). `floor_only` is at `plan.rs:220`. Result content is
  never read today.
- C9. `TurnFacts` keeps no message text and no result content (`turn.rs:1-3`, `turn.rs:105-114`).
  `ToolDefinition.schema` is `Option<Value>` (`turn.rs:72`). `PriorCall` keeps the name and the
  arguments of an earlier call, but not its kind (`turn.rs:90-101`).
- C10. `decide_calls` is at `policy/calls.rs:98`, and `outcome` at `calls.rs:62`.
- C11. `BlockCode::Detector` exists and is not used to block (`types.rs:141`;
  `report.rs:64` maps it to no reason).
- C12. `CedarDecider::decide` returns `std::future::ready(..)` and spawns nothing
  (`paigasus-gateway/src/application/tool_policy/decider.rs:113-115`, SMA-749 D25).
  `FINDING_KINDS` is closed at three values (`decider.rs:42`). `decide_now` starts its overhead
  timer at `decider.rs:76` and adds it to the snapshot at `decider.rs:98-100`.
- C13. The relay holds one pending decider future. The body path awaits it under
  `decide_timeout` (`adapters/http/interception.rs:131`). The stream path sends keep-alive
  comments while a hold waits (`interception.rs:307`). Stream holds queue, and the relay starts the
  next decision only when none is in flight (`interception.rs:213-221`). The hold's clock starts
  when it completes (`Track::Deciding { since }`, `paigasus-gateway-core/src/interceptor.rs:322-329`).
- C14. `RequestToolPolicy::apply_inner` (`application/tool_policy/request.rs:247`) takes
  `&DecisionContext`, loads the policies and calls `plan_request` (`request.rs:277`) before the
  upstream call. In mode `off` and with the tenant part unavailable, the snapshot holds the
  floor-only set `floor.floor_only` (`request.rs:157-164`). `record_entries` writes one record per
  plan entry (`request.rs:241-245`).
- C15. `build_tool_policy` is at `runtime.rs:272`. `ToolPolicyConfig` (`config.rs:656`) and
  `ToolInterceptionConfig` (`config.rs:245`) use `deny_unknown_fields`.
  `tool_interception.decide_timeout_secs` defaults to 5 s. `load_timeout_ms` must be less than it
  (`config.rs:907-913`).
- C16. The proto `Finding` has `string kind = 1;` and keeps field 2 free for the severity
  (`contracts/proto/paigasus/gateway/v1/tool_decision.proto:68-71`). The Redis adapter maps it at
  `paigasus-gateway/src/adapters/decision_log/proto.rs:74`. The read path `cedar_from`
  (`proto.rs:190-225`) keeps only the kind (`proto.rs:220`). The `request_action` decode
  refuses an unknown enum value with `BadEnum` (`proto.rs:269-276`).
- C17. The decision record keeps `findings: BTreeSet<String>`
  (`paigasus-gateway-core/src/decision_log/record.rs:126`). The Dogwood trace encoder writes
  `findings: [<kinds>]` and `call: { declared }` (`decision_log/trace.rs:119-127`). It skips
  request-side records (`SkipReason::RequestSide`, `trace.rs:16`).
- C18. **IAM validates tenant policies against a Cedar schema.** The schema lists the context
  attributes with `required: true` (`paigasus-iam-core/src/tool_policy/schema.rs:37-53`), and
  `CallInfo` has only `declared` (`schema.rs:85`). `TOOL_SCHEMA_REVISION` is 1 (`schema.rs:17`).
- C19. **A schema revision bump holds every live set.** `outputs_identical`
  (`paigasus-iam-core/src/tool_policy/compare.rs:14-16`) compares `cedar_policies`, the canonical
  `cedar_schema_json` and `conditions`. The re-compile job selects every live version whose
  generation or revision differs (`recompile.rs:243`; the query is
  `pg_tool_policies.rs:452-468`), and stores a held "different" row when the outputs differ
  (`recompile.rs:339-347`). Until a set is current (`tool_policies/mod.rs:471-473`), a catalog
  write fails with `ToolPolicyRecompilePending` (`mod.rs:663-664`). The catalog coupling also calls
  `outputs_identical` (`mod.rs:679`), and the diff report uses it (`tool_policy/report.rs:96`). A
  replica whose revision is lower than the stored one stops the job with `OlderReplica`
  (`recompile.rs:19`, `recompile.rs:238`). An operator can start the job with
  `POST /v1/tool-policy-recompile` (`adapters/http/tool_policies.rs:63`).
- C20. Size caps bound every scan. Chat request bodies: `max_request_bytes`, default 1 MiB
  (`config.rs:32`, `config.rs:508`). Responses request bodies: `responses.max_request_bytes`,
  default 16 MiB (`config.rs:281`). Held response bytes: `tool_interception.max_held_bytes`,
  default 4 MiB (`paigasus-gateway-core/src/limits.rs:36`), and `responses.max_held_bytes`,
  default 32 MiB (`config.rs:283`).
- C21. The error reasons 315-318 exist (`contracts/proto/paigasus/common/v1/error.proto:327-333`).
- C22. The agent host `EventScreen` port that SMA-853 names does not exist in the code yet.
- C23. In a Cedar `Deny`, the reason set can be empty (no permit matched) or hold satisfied
  forbids. `decide` copies it to `determining` (`decide.rs:144`). An errored forbid also gives
  `Deny` (`decide.rs:145-153`). So "deny" does not imply "a forbid decided".
- C24. `DecisionContext` derives `Debug` (`paigasus-gateway-core/src/decider.rs:22-23`).
- C25. The gateway sets no panic hook. The default hook writes the panic payload to stderr.
  `catch_unwind` catches the unwind, but the hook runs first. A `str` range index that panics
  prints part of the string in its message.
- C26. `build_request` (`paigasus-gateway-core/src/decision_log/build.rs:173`) builds a
  request-side record from a `RequestEntry` only, and leaves `cedar` unset.
  `DecisionRecorder::record_request` is at `paigasus-gateway/src/application/decision_log.rs:139`.
  `RequestEntry` holds `tool`, `action`, `enforced` and `reason` (`report.rs:149-155`).
- C27. `adapters/decision_log/supervised.rs:34` runs a future under
  `AssertUnwindSafe(..).catch_unwind()`.

## 4. Approaches

### 4.1 Where the detectors live (chosen: A)

- **A. Types and built-ins in the kernel, the port and the pipeline in the gateway service
  (chosen by Sven).** The kernel gets `Severity`, `Finding` and the five built-ins behind a new
  feature `detectors`. They are pure, sync and without I/O. The gateway service owns the async
  `Detector` port, the pipeline and an adapter that wraps the built-ins. SMA-853 calls the kernel
  functions directly. Trade-off: the kernel gets `regex` and `url` under the feature, and a kernel
  change causes a kernel release (SMA-752 C1).
- B. Everything in `paigasus-gateway-core`, the port in the gateway. Rejected: the agent host would
  need gateway-core and its provider types to screen an event, or it would have its own copy. That
  is the duplication that ADR-0005 forbids.
- C. A new library crate `paigasus-detectors`. Rejected: `Findings` in the kernel must know the
  severity anyway, so the kernel changes in all cases. Two crates for one concept add a release
  and a lockstep question without a gain.

### 4.2 How `invalid_args` validates (chosen: the subset validator)

- **A small in-house validator for a JSON Schema subset (chosen by Sven).** It knows `type`,
  `required`, `properties`, `enum`, `additionalProperties: false` and `items`. It ignores every
  other keyword. It has no dependency, it has a bounded cost per call, and it cannot fetch a remote
  `$ref`. Trade-off: it finds fewer violations than a full validator.
- The `jsonschema` crate. Rejected: a large dependency tree in a published kernel, draft
  differences, remote `$ref` resolution to switch off, and a cost per call that is hard to bound.

Why a subset is safe (A7): JSON Schema keywords are conjunctive. A value is valid only if every
keyword accepts it. So a check of some keywords can miss a violation, but it cannot invent one.
§ 6.3.2 lists the exceptions (`$ref`, `patternProperties`, `prefixItems`, schema-valued
`additionalProperties`), and the validator skips them.

### 4.3 How a tool result is decided (chosen: differential)

- **Differential (chosen).** Decide the result twice: with its findings (A) and without them (B).
  Replace it only when the findings make the verdict worse. So only a policy that reads the
  findings can replace a result (A1, A9).
- "Deny decided by a forbid" (revision 1). Rejected: an input forbid without a `has` guard errors
  with input `{}`, and the request-level `name_collision` makes the floor deny.
- Deny with the findings. Rejected: an argument `permit` does not match input `{}`, so the default
  deny replaced every result with any finding.

### 4.4 When the request side screens (chosen: by mode)

- **By mode (chosen, revision 3).** A tenant in `log_only` or `enforce` always gets a screen. A
  floor-only snapshot gets one only when the floor needs it. The rule is simple, and a tenant
  policy that reads findings in an unusual form cannot be missed.
- A walk over every tenant policy for a `contains` literal (revision 2). Rejected: the walk must
  be correct for every Cedar form that reads `findings`. A miss is fail open, and the saving is
  small, because a tenant that writes policies wants detection.

## 5. Decisions

"Sven" marks a decision that Sven made on 2026-11-22. "Coordinator" marks a proposal of the
coordinator, which Sven can change at Gate 1. "Spec" marks a further call that this spec makes,
which Sven can also change.

| # | Decision | By |
|---|---|---|
| D1 | Scope is SMA-855 only: the `Finding` type with severity, the `Detector` port, the async pipeline and five built-ins. Webhook (SMA-856) and judge (SMA-857) are non-goals, but the port and the pipeline have a place for them, in the order built-ins, webhooks in parallel, judge (§ 6.6). | Sven |
| D2 | Findings are signals only. `floor.cedar` does not change. No finding blocks unless a tenant policy reads it. The tenant guide ships a tested starter policy that blocks on high severity (§ 6.12). Tenants copy it into IAM. The guide says to run it in `log_only` first. | Sven |
| D3 | Evidence stays in memory only. It goes to later detectors (webhook, judge) and nowhere else. The decision log, the proto and the metrics get only the detector, the kind and the severity (and the version, D19). Proto `Finding` field 2 is the severity. The detector name is a new field. | Sven |
| D4 | Four severities: `info`, `low`, `medium`, `high`. `context.findings` stays a `Set<String>` of kinds. `context.max_severity` is a `Long`: `info` = 0, `low` = 1, `medium` = 2, `high` = 3. | Sven |
| D5 | `context.max_severity` is **always present**. With no findings, its value is **-1** (§ 6.2.3). | Coordinator |
| D6 | Approach 4.1 A. The kernel feature `detectors` holds the types and the built-ins. The gateway owns the async port, the pipeline and the built-in adapter. | Sven |
| D7 | Approach 4.2: the subset validator. An unknown keyword gives no finding. No `jsonschema` crate. | Sven |
| D8 | The kernel feature `tool-policy` enables `detectors`. The kernel fills `context.call.hosts` in `decide`: from the precomputed `HostSet` on `ToolInvocation` when it is there, else from the call input. A caller cannot forget it. Cost: `paigasus-iam-core` compiles `regex` and `url` too. | Coordinator |
| D9 | `ToolInput`, `Declared`, the strict JSON parser (with its duplicate-key visitor) and `classify_number` move to a Cedar-free kernel module under `detectors`. `tool_policy` re-exports `ToolInput` and `Declared` at the same path. | Coordinator |
| D10 | Only `injection_markers` reads tool-result content. It reads it in memory, on the request side. The content is never stored, logged or put into evidence. | Coordinator |
| D11 | A tool result with at least one finding gets a real `decide` with its findings (§ 6.8.2). A result with no finding keeps the optimistic path of SMA-752 unchanged. Under a deny, the existing notice replaces the result. | Coordinator |
| D12 | The result decision is **differential** (§ 6.8.2). Per earlier call: A with the target's findings, B with none, both with `Declared::Unknown`. `replaced = A.decision == Deny && (B.decision == Allow \|\| F(A) ⊄ F(B))`, where `F(v)` is the determining policies of `v` and the errored forbids of `v`. One decision per `ResultTarget`, on the union of the findings of its results. | Coordinator |
| D13 | `unseen_host` extracts hosts from URL-like string values in the arguments, and compares them with the hosts of the sources of § 6.3.3. The request side collects only a set of seen hosts, not the text. `context.call.hosts` holds every host of the call. Client history is a hint, not proof (ADR-0027). | Coordinator |
| D14 | A host does not count as seen when it is only in a tool result that has a finding of severity `medium` or higher, or in a result that the plan replaces with `enforced = true`. In `log_only` mode, an unenforced replacement does not remove the hosts, but the `medium` rule still applies. A split across two results gets past this rule (§ 6.3.3). | Coordinator |
| D15 | `unseen_host` also takes a bare host (with no scheme) from a string value whose key is one of a fixed list (`url`, `uri`, `href`, `link`, `host`, `hostname`, `domain`, `endpoint`, `base_url`, `website`). A bare value under any other key is not a host. A split of command strings into tokens was considered and rejected: tokens like `main.rs` give false findings. | Coordinator |
| D16 | `secret_in_args` matches known token prefixes only (§ 6.3.4). No entropy heuristic. Severity `high`. | Coordinator |
| D17 | `undeclared_tool` comes from `Declared::No` only. `Declared::Unknown` gives no finding. | Coordinator |
| D18 | The severity of each built-in is fixed in the kernel (§ 6.4). | Coordinator |
| D19 | `injection_markers` has its pattern set in the kernel, with a version string (`v1`). The version goes into the decision log next to the finding. `secret_in_args` has a versioned pattern set too. Each built-in reports a version. | Coordinator |
| D20 | Each pattern of the `injection_markers` set has a fixed severity. Only `unicode_tags` is `high`, and it excludes valid emoji tag sequences. Every other rule is `medium`, because chat-template tokens and close tags occur in the normal source code of LLM projects. The finding takes the highest severity of its matches. | Coordinator |
| D21 | The two kernel findings get fixed severities: `name_collision` is `high`. `unparsed_arguments` is `info` for a `ToolInput::Text` and `medium` for JSON that does not parse strictly. | Spec |
| D22 | A detector error, a detector timeout, a detector panic, a partial scan (`scan_limit`), a screen failure and the hosts cap give a `detector_error` finding of severity **`high`**: fail closed under the starter policy (A3). A panic in the built-in stage is caught (§ 6.6.2). A panic outside the pipeline stays with `[tool_interception] on_failure`. | Sven (severity); Coordinator (panic) |
| D23 | Config: a new table `[tool_policy.detectors]` with `enabled` (default true), `timeout_ms` (default 500) and `max_scan_bytes` (default 16777216), with `deny_unknown_fields` (§ 6.10). | Coordinator |
| D24 | Metrics: `FINDING_KINDS` grows to the new closed set. `gateway_tool_policy_findings_total` gets the labels `detector` and `severity` (§ 6.11.3). One new histogram for the pipeline time. | Coordinator |
| D25 | Performance: the built-ins add at most 2 ms at p99 to the SMA-752 perf fixture, and the total stays below 15 ms (§ 7.6). | Coordinator |
| D26 | No new error code. A deny that a policy makes on a finding uses the existing reasons `tool-call-denied` (315) and `tool-result-blocked` (316). | Spec |
| D27 | Response side: call detection runs whenever the policy set is available and detectors are enabled, in every tenant mode. `unseen_host` runs only when a screen ran. Request side: the screen runs when the set is available, detectors are enabled, and either the tenant part counts (mode `log_only` or `enforce`, tenant evaluated) or the floor-only set reads a screen-dependent kind (§ 6.8.0). | Coordinator |
| D28 | The IAM Cedar schema gets `max_severity` (`Long`, required) in the context and `hosts` (`Set<String>`, required) in `CallInfo`. `TOOL_SCHEMA_REVISION` goes to 2. The re-compile job treats a new schema that only **adds** attributes to `Record` types as equivalent (§ 6.13.2), through a new function `outputs_equivalent_for_recompile`. `outputs_identical` stays exact for the catalog coupling and the diff report. So every live set stays current, and no runbook step is needed. In scope for this PR. | Sven (intent); Coordinator (realisation) |
| D29 | The Dogwood trace writes `max_severity` and `call: { declared, hosts: [] }`. The export CLI says that a policy that reads `context.call.hosts` does not replay correctly. | Spec |
| D30 | No `tokio::spawn` in our code (A8). The pipeline runs inside the decider future, which the relay polls. The deadline is `tokio::time::timeout_at` around the webhook and judge stages. | Coordinator |
| D31 | Each tool result with findings gets a request-side decision record, also when it is not replaced (§ 6.11.2). The report header gets no findings field. | Coordinator |
| D32 | A result with a finding and no earlier call in the request is decided by the same differential pair, with the result's tool name (or the empty name) and input `{}`. | Coordinator |
| D33 | The rule id of a match does not go into the decision log. | Coordinator |
| D34 | Hosts in the descriptions of tool definitions do not count as seen. | Coordinator |
| D35 | The pipeline gets an **absolute** deadline: `hold_start + decide_timeout - margin`, and at most `run_start + timeout_ms`. The margin is provisional at 250 ms. The builder sets it from a measured p99 (§ 6.9) and records the number. `timeout_ms + margin <= decide_timeout_secs * 1000` is checked at start. | Coordinator |
| D36 | Past 256 hosts, `context.call.hosts` holds the first 255 sorted hosts and the sentinel `<overflow>`, and the kernel adds `detector_error`. The sentinel holds forbidden host code points, so no parsed URL can give it, and a host allow list fails closed. A host deny list fails open past the cap (§ 6.12). | Coordinator |
| D37 | The pipeline accepts a kind only when it matches `[a-z0-9_]{1,64}`, and a finding only from a known detector name (`kernel`, `built_in`, the five built-ins, and later each configured webhook name and `judge`). Anything else becomes `detector_error` with the name of the detector that returned it. | Coordinator |
| D38 | Mode `off` with the shipped floor still runs the call built-ins on every response call. Accepted: they are cheap and bounded. | Coordinator |
| D39 | The boot re-compile of the revision bump writes a new live version for every tenant set and the floor. So every gateway reloads every tenant at about the same time. Accepted: it happens once per revision bump. | Coordinator |
| D40 | SMA-754 scope (Sven): the judge never sees tool results, including `role: "tool"` messages. | Sven |
| D41 | A refused request writes no screen records (§ 6.11.2). | Coordinator |

## 6. Design

### 6.1 Kernel features and modules

`rs/crates/libs/paigasus-kernel/Cargo.toml`:

```toml
regex = { version = "1.13", optional = true }   # already in the lock (C6)
url = { workspace = true, optional = true }      # already in the lock (C6)

[features]
detectors = ["dep:serde", "dep:serde_json", "dep:unicode-normalization", "dep:regex", "dep:url"]
tool-policy = ["detectors", "dep:cedar-policy", "dep:serde", "dep:serde_json", "dep:unicode-normalization"]

[package.metadata.docs.rs]
features = ["tool-policy"]
```

- The new module is `paigasus_kernel::detect`, behind `#[cfg(feature = "detectors")]` in
  `src/lib.rs`. It has no Cedar type.
- D9: `ToolInput`, `Declared`, the strict parser (the duplicate-key visitor of `input.rs:131-167`,
  the nesting check of `input.rs:226-252`) and `classify_number` move to a Cedar-free module under
  `detect`. `tool_policy::input` keeps only the Cedar mapping and imports them.
- The kind registry (§ 6.8.0) is one constant in `detect`, next to the kind constants.
- `paigasus-agent-host-core` enables `detectors` only (SMA-853). It gets no Cedar.
- `repo:wasm-getrandom-free` gets one more assertion: no binding crate enables the kernel feature
  `detectors`. The builder chooses the form, for example a `cargo tree -e features -i
  paigasus-kernel` per binding crate and a grep for `detectors`. Update the task description and
  the `ci/` README of the gate.
- A new public kernel API. release-plz cuts the kernel release, and `repo:version-lockstep` moves
  the binding versions (SMA-752 D27).

### 6.2 Kernel types

#### 6.2.1 `Severity`, `Finding` and `Evidence`

All public types are `#[non_exhaustive]`.

```rust
// paigasus_kernel::detect
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum Severity { Info, Low, Medium, High }

impl Severity {
    pub fn as_long(self) -> i64;         // Info = 0, Low = 1, Medium = 2, High = 3
    pub fn as_str(self) -> &'static str; // "info", "low", "medium", "high"
}

/// The value of `context.max_severity` when a call has no findings (D5).
pub const NO_SEVERITY: i64 = -1;

#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Finding {
    pub detector: String,         // "kernel", "built_in", "undeclared_tool", ..., later a webhook name or "judge"
    pub kind: String,             // the value in `context.findings`
    pub severity: Severity,
    pub version: String,          // the detector or pattern-set version, for example "v1"
    pub evidence: Vec<Evidence>,  // in memory only (D3), at most 16 entries
}

impl Finding {
    pub fn new(detector: impl Into<String>, kind: impl Into<String>, severity: Severity,
               version: impl Into<String>) -> Self;
    pub fn with_evidence(self, e: Evidence) -> Self; // drops entries past 16
}

#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Evidence {
    pub rule: &'static str,      // a pattern id or a schema keyword, never content
    pub location: Location,
    pub host: Option<String>,    // set only by `unseen_host`
}

impl Evidence {
    pub fn argument(rule: &'static str, pointer: String) -> Self;
    pub fn host(rule: &'static str, pointer: Option<String>, host: String) -> Self;
    pub fn text_input(rule: &'static str) -> Self;
    pub fn result(rule: &'static str, index: usize) -> Self;
}

#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Location {
    Argument { pointer: String },  // a JSON pointer into the call arguments, at most 256 bytes
    TextInput,                     // somewhere in a `ToolInput::Text`
    Result { index: usize },       // the index of a tool result in `TurnFacts.tool_results`
}
```

- No evidence holds an excerpt of content. `secret_in_args` evidence holds no copy of the secret,
  so no evidence copy anywhere holds a secret. `injection_markers` evidence is the rule id and the
  result index (§ 6.3.5).
- A JSON pointer holds argument keys, and a key can be content. So the pointer is evidence, and it
  stays in memory too.

#### 6.2.2 `Findings` and `Verdict`

`Findings` (`tool_policy/types.rs:143`) keeps its kind set and gets the highest severity:

```rust
impl Findings {
    pub fn add(&mut self, kind: &str, severity: Severity);
    pub fn max_severity(&self) -> Option<Severity>;
}
```

- `Findings::insert` is **removed**. The workspace builds with `-D warnings`, so a deprecated
  method would fail every call site. Every caller moves to `add`: the kernel tests
  (`tests/tool_policy_decide.rs:137` and `:225`, `tests/tool_policy_props.rs:115`), the kernel unit
  tests (`optimistic.rs:264`, `types.rs:190-192`), and gateway-core (`mapping.rs:79`,
  `plan.rs:134`).
- `Verdict` (`decide.rs:28`) gets three fields:
  - `max_severity: Option<Severity>`, after the kernel adds its own findings;
  - `added: Vec<Finding>`: the findings that the kernel adds itself (`unparsed_arguments` with its
    D21 severity, and the hosts-cap `detector_error`, D36). The gateway copies them into the
    `FindingRecord` list of the log;
  - `errored_forbids: Vec<PolicyRef>`: the subset of `errored` whose effect is `forbid`, sorted.
    D12 needs it.

#### 6.2.3 The Cedar context

`context()` (`decide.rs:71`) builds:

```
{
  input: { ... },                     // unchanged (SMA-752 § 6.1.4)
  input_text: "...",                  // unchanged, only for text input
  findings: ["secret_in_args", "unseen_host"],
  max_severity: 3,                    // new: a Long, -1 with no findings
  call: { declared: "yes", hosts: ["api.example.com", "evil.example"] },  // hosts is new
  request: { scope: "...", ingress: "chat_completions", model: "..." },
  session: { id: "", turn: 0 }
}
```

- `hosts` holds every host of the call, seen or not, sorted and unique (§ 6.3.3), with the cap of
  D36. It is empty for a call with no host.
- `ToolInvocation` gets `hosts: Option<HostSet>` and a constructor `with_hosts(HostSet)`. The
  gateway fills it from `CallDetection.hosts` (§ 6.3). The kernel calls `call_hosts(&input)` only
  when it is `None`. So `decide` does not scan the input again, and the A and B verdicts of one
  result share one set (§ 6.8.2). One extraction function serves both, so the two cannot differ.
- `Facts` gets a flag `detection` (default true). With `false` (§ 6.10, `enabled = false`), the
  kernel writes `max_severity: -1` and `hosts: []`, so that a policy that reads them does not
  error. The kinds set stays as today.
- The optimistic context of `decide_tool` (`call = None`) gets `max_severity: -1` and `hosts: []`,
  also when `findings` holds `name_collision`. That is sound: no forbid that reads
  `max_severity`, `findings` or `call` is in the optimistic set (§ 6.7), and every permit there
  has no conditions. So no kept policy reads the value.
- Release note: `context.call` now has two attributes. A policy that compares the whole record
  with a literal, for example `context.call == {"declared": "yes"}`, becomes false for every
  call. Compare `context.call.declared` instead.

**Why -1 with no findings (D5).**

- The attribute is absent with no findings. Rejected: a tenant forbid
  `when { context.max_severity >= 3 }` then errors on every clean call, and an errored forbid
  denies (SMA-752 D26). That is anti-goal A4.
- The value is 0 with no findings. Rejected: 0 is also `info`. A tenant who writes
  `context.max_severity >= 0` to mean "any finding" would then block every call.
- **The value is -1 with no findings (chosen).** Every comparison with a real level is false for a
  clean call. Cedar `Long` holds -1. The levels stay 0 to 3.

#### 6.2.4 `Debug`, panics and other leak paths (A2)

- These types implement `Debug` by hand and print counts only (for example
  `Subject::Call { index: 2, .. }` or `HostSet { len: 3 }`): `Finding`, `Evidence`, `Location`,
  `HostSet`, `CallSubject`, `CallDetection`, `Subject`, `DetectBatch`, `DetectorError`,
  `RequestScreen`, and the `SeenHosts` wrapper. `Finding` prints its detector, kind, severity and
  version, which are not content.
- `DetectorError` holds no free text. It is an enum with fixed variants (`Failed`, `WrongShape`,
  `Timeout`, `Panicked`).
- No `#[tracing::instrument]` on a function that takes one of these types, unless it has
  `skip_all`.
- **Panics.** Our own code never puts content into a panic message (a review rule). The built-ins
  index strings only with `str::get` or after a checked `is_char_boundary`, and use no `expect` or
  `unwrap` whose message holds input. The gateway does not install a global panic hook (N12): it
  would change the panic output of the whole process.
- **Residual.** `catch_unwind` cannot stop the default panic hook (C25). A panic inside a
  dependency (`regex`, `url`, `idna`, `unicode-normalization`) can print content to stderr. The
  decision log, the metrics and the `tracing` events still hold no content.
- D37: the pipeline checks each finding: the kind matches `[a-z0-9_]{1,64}`, and the detector is
  a known name. A finding that fails the check is replaced by one `detector_error` finding of the
  detector that returned it.

### 6.3 The five built-ins

All built-ins are in `paigasus_kernel::detect`. They are pure and synchronous. They read no clock
and no global state. Each one is linear in the size of its input.

```rust
pub struct CallSubject<'a> {
    pub name: &'a str,                   // raw name, as the model sent it
    pub input: &'a ToolInput,
    pub declared: Declared,
    pub schema: Option<&'a serde_json::Value>, // the matching definition's JSON Schema
    pub seen_hosts: Option<&'a HostSet>,       // None: no screen ran, so `unseen_host` does not run
}

pub struct CallDetection { pub findings: Vec<Finding>, pub hosts: HostSet }

pub fn detect_call(subject: &CallSubject<'_>) -> CallDetection; // the four call built-ins, one parse
pub fn undeclared_tool(subject: &CallSubject<'_>) -> Option<Finding>;
pub fn invalid_args(subject: &CallSubject<'_>) -> Option<Finding>;
pub fn unseen_host(seen: &HostSet, hosts: &HostSet) -> Option<Finding>;
pub fn secret_in_args(subject: &CallSubject<'_>) -> Option<Finding>;
pub fn injection_markers(text: &str, result_index: usize) -> Option<Finding>;
pub fn call_hosts(input: &ToolInput) -> HostSet;
pub fn hosts_in_text(text: &str, into: &mut HostSet);

pub const INJECTION_MARKERS_VERSION: &str = "v1";
pub const SECRET_PATTERNS_VERSION: &str = "v1";
```

`HostSet` is a sorted set of normalised host strings. One finding per detector per subject: a
detector with many matches gives one finding with up to 16 evidence entries.

**Common input handling.**

- `ToolInput::Json` that parses strictly: one walk over the string values and the object keys,
  after JSON unescaping. So `\u0041KIA...` is the same as `AKIA...`.
- `ToolInput::Json` that fails **only** the duplicate-key rule: the kernel adds
  `unparsed_arguments` (C3). The detectors then walk all values with a lenient visitor that keeps
  every duplicate member. So a `\u` escape under a duplicate key cannot hide a secret or a host.
  `invalid_args` gives no finding.
- `ToolInput::Json` that is not valid JSON, or deeper than 128 levels: `secret_in_args` and
  `unseen_host` scan the raw text. The scan first replaces `\/` with `/`. `invalid_args` gives no
  finding.
- `ToolInput::Text` (kinds `custom`, `apply_patch`, `other`): `secret_in_args` and `unseen_host`
  scan the text. `invalid_args` gives no finding.
- Depth: the walk visits every value up to the 128 levels of the parse. The 16-level limit of the
  Cedar mapping (SMA-752 § 6.1.4) does not apply.
- Size: bounded by the caps of C20 and by `max_scan_bytes` (§ 6.10).
- `regex` patterns use `(?-u:\b)` for word boundaries. Every token form is ASCII.

#### 6.3.1 `undeclared_tool`

- Input: `declared`.
- `Declared::No` gives one finding, kind `undeclared_tool`. `Yes` and `Unknown` give none (D17).
- `Declared::Unknown` is the value for a Responses request whose definitions are not complete, for
  an MCP approval request, and for an unreadable turn (`mapping.rs:51-67`). The `Unregistered`
  action group (SMA-752 D22) is a separate concept: it is about the tenant catalog.
- Evidence: none.

#### 6.3.2 `invalid_args`

- Input: the parsed arguments and the JSON Schema of the matching definition.
- `ToolDefinition.schema` becomes `Option<Arc<Value>>` (`turn.rs:72`), so the decider can share it
  with a `Subject` without a copy.
- The definition is the one whose `definition_call_name` is the normalised call name
  (`mapping.rs:35`). No definition, two definitions with that name, or a schema that is not a JSON
  object: no finding.
- The keywords, with the instance types that each one applies to:

| Keyword | Applies to | Check |
|---|---|---|
| `type` | every instance | a string or an array of strings. `integer` accepts a number whose value is an integer, by `classify_number` (`5`, `5.0`, `5e0`). `number`, `string`, `boolean`, `object`, `array` and `null` as in JSON Schema. An unknown type name: the keyword is skipped. |
| `enum` | every instance | an array. The value must be equal to one element. Numbers compare by value, not by text. Objects compare by members, with no order. |
| `required` | objects only | an array of strings. Each one must be a key of the object. |
| `properties` | objects only | an object of schemas. Each present key is checked against its schema. |
| `additionalProperties` | objects only | only the value `false` is checked: every key must be in `properties`. A schema object as the value: skipped. |
| `items` | arrays only | a schema object: each element is checked. An array (the tuple form): skipped. |

- An object keyword on a non-object instance, and `items` on a non-array instance, does nothing.
- Skips (the exceptions to "conjunctive", § 4.2):
  - A schema node with `$ref`, `$dynamicRef` or `$recursiveRef` is not checked at all.
  - `additionalProperties: false` is not checked when the same node has `patternProperties`.
  - `items` is not checked when the same node has `prefixItems`.
  - A boolean schema `true` accepts everything. A boolean schema `false` gives a finding.
- Every other keyword is ignored (D7). The checks of the other keywords at the same node go on.
- Work budget: at most 100 000 steps per call. One step is one schema node visit, one `enum`
  element comparison (one step per compared member of a large value), or one `required` entry.
  Past the budget, the check stops and gives **no** finding (A7).
- A `null` member counts as present for `required` and is type `null`, as in JSON Schema.
- Kind `invalid_args`. Evidence: the JSON pointer of each violation (at most 16) and the keyword
  as the `rule`.
- The schema comes from the request, which the client writes. The finding says that the model's
  call does not match the client's own declaration.

#### 6.3.3 `unseen_host`

Host extraction (one function, used for the call, for `context.call.hosts` and for the seen set):

1. In every string value (and in text input), find each match of `(?i)(https?|wss?|ftp):[/\\]*`.
   Any other scheme gives no host. The candidate runs from the match to the next white space,
   `"`, `'`, `<`, `>` or backtick.
2. Trim the end: remove trailing characters of the set `.,;:!?)]}'*`, and remove a trailing `)`,
   `]` or `}` that has no open partner in the candidate. Repeat until nothing changes.
3. Parse the candidate with `url::Url::parse`. The WHATWG rules of `url` normalise the special
   schemes: `https:evil.example/x`, `https:/evil.example` and `https:\\evil.example` all give the
   host `evil.example`. Keep the host when `host_str()` is not empty.
4. D15: for a string value whose key is in the fixed list, and which has no scheme and no white
   space, parse `"http://" + value` after step 2. Keep the host when it holds a dot or is an IP
   address.
5. Normalise: lowercase, IDNA to ASCII (as `url` does), strip one trailing dot. An IPv6 address
   keeps its brackets. A port and user information are not part of the host.

The seen sources, per format (D13, D34). Each source counts only its text parts:

- Chat: `messages[]` with `role` `system`, `developer` or `user`, where `content` is a string or an
  array of `{ "type": "text" }` parts.
- Anthropic: the top-level `system` (a string or text blocks), and `messages[]` with `role: "user"`
  where `content` is a string or holds `text` blocks.
- Responses: `instructions`; `input` when it is a string; and items of `input[]` with `role`
  `system`, `developer` or `user`, where `content` is a string or holds `input_text` parts.
- Every format: the text of earlier tool results (§ 6.8.1), except under D14.
- Assistant text, earlier call arguments and tool definition descriptions do not count.

The finding:

- `unseen_host` runs only when a screen ran (`CallSubject.seen_hosts` is `Some`). With no screen
  there is no seen set, so there is no hint. `context.call.hosts` is still filled.
- Kind `unseen_host` when the call has at least one host that is not in the seen set.
- Evidence: one entry per unseen host (at most 16), with the pointer and the host.
- Hosts cap (D36): past 256 hosts, the kernel and the detector use the first 255 sorted hosts
  plus the sentinel `<overflow>`, and the kernel adds a `detector_error` finding with detector
  `kernel`. `<` and `>` are forbidden host code points in the WHATWG URL standard, so no parsed
  URL gives this value. A unit test asserts that `Url::parse("http://<overflow>/")` fails.
- Caveats:
  - The client writes the history, so a malicious client can make any host "seen" (ADR-0027).
  - With a Responses `previous_response_id`, the earlier turns are not in the request. The seen set
    is then nearly empty, so `unseen_host` fires often. It is `medium`, so the starter policy does
    not block on it.
  - D14 can be bypassed: an injection in one result and the URL in another result without a
    finding make the host "seen".
  - A URL in a base64 or other encoded string is not found. A bare host under a key outside the
    D15 list is not found.

#### 6.3.4 `secret_in_args`

- Input: the decoded string values and keys, or the raw text.
- One `RegexSet` of known token forms, set version `v1` (D19). The builder confirms each form
  against the vendor's documentation and writes the source link in a comment. `\b` below means
  `(?-u:\b)`.

| Rule id | Form (sketch) |
|---|---|
| `aws_access_key_id` | `\b(AKIA\|ASIA\|ABIA\|ACCA)[0-9A-Z]{16}\b` |
| `github_token` | `\bgh[pousr]_[A-Za-z0-9]{36,255}\b` |
| `github_fine_grained_pat` | `\bgithub_pat_[A-Za-z0-9_]{22,255}\b` |
| `slack_token` | `\bxox[abposr]-[A-Za-z0-9-]{10,}` |
| `slack_webhook` | `https://hooks\.slack\.com/services/T[A-Z0-9]+/B[A-Z0-9]+/[A-Za-z0-9]+` |
| `anthropic_api_key` | `\bsk-ant-(api\|admin)[0-9]{2}-[A-Za-z0-9_-]{80,}` |
| `openai_api_key` | `\bsk-(proj\|svcacct\|admin)-[A-Za-z0-9_-]{20,}`, and the legacy `\bsk-[A-Za-z0-9]{20}T3BlbkFJ[A-Za-z0-9]{20}\b` |
| `google_api_key` | `\bAIza[0-9A-Za-z_-]{35}\b` |
| `stripe_secret_key` | `\b(sk\|rk)_live_[0-9A-Za-z]{24,}\b` |
| `pem_private_key` | `-----BEGIN ((RSA\|EC\|DSA\|OPENSSH\|ENCRYPTED\|PGP) )?PRIVATE KEY( BLOCK)?-----` |
| `jwt` | `\beyJ[A-Za-z0-9_-]{10,}\.eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}` |

- Kind `secret_in_args`, severity `high` (D16).
- Evidence: the rule id and the location (the pointer, or `TextInput`). No copy of the secret.
- Caveats: no entropy heuristic (N4). A split, encoded or reversed secret is not found. A JWT can
  be a legitimate value for a tool. The tenant decides with a policy.

#### 6.3.5 `injection_markers`

- Input: one tool-result text and its result index. A result with content parts gives the
  concatenation of its text parts, with a newline between them. Image, audio and file parts are
  not read (N6).
- Normalisation before the match, on a copy in memory:
  1. Find Unicode tag characters (U+E0020 to U+E007E). A valid emoji tag sequence (UTS #51: U+1F3F4,
     then tag characters, then U+E007F, for example the flag of England) is not a match and is not
     decoded. Every other run of 4 or more tag characters is a match (rule `unicode_tags`), and its
     ASCII form is appended to the scan.
  2. NFKC.
  3. Remove default-ignorable code points (zero-width characters U+200B to U+200D, U+2060,
     U+FEFF, and the bidirectional controls U+202A to U+202E and U+2066 to U+2069).
  4. Lowercase, and collapse each run of white space to one space. The `turn_marker` rule runs
     before this step.
- The pattern set `v1` is one `RegexSet`. A `RegexSet` gives the ids of the matched rules, which
  is all that the evidence needs.

| Rule id | Severity | Form (sketch, after normalisation) |
|---|---|---|
| `unicode_tags` | high | 4 or more tag characters in a row, outside a valid emoji tag sequence |
| `chat_template_token` | medium | `<\|im_start\|>`, `<\|im_end\|>`, `<\|endoftext\|>`, `<\|system\|>`, `[inst]`, `<<sys>>` |
| `turn_marker` | medium | `\n\nhuman:`, `\n\nassistant:` |
| `result_close_tag` | medium | `</tool_result>`, `</function_results>`, `</tool_output>` |
| `ignore_instructions` | medium | `(ignore\|disregard\|forget) (all \|any )?(the )?(previous\|prior\|above\|earlier\|your) (instructions\|prompts\|messages\|rules)` |
| `role_override` | medium | `you are now (in )?(developer\|dan\|jailbreak\|god) mode`, `new (system )?instructions:` |
| `prompt_leak` | medium | `(reveal\|print\|show) (your\|the) (system prompt\|instructions)` |
| `concealment` | medium | `do not (tell\|inform\|mention (this )?to) the user` |

- Why only `unicode_tags` is `high` (D20): the other markers occur in normal source code, tests
  and documentation of LLM projects, which coding agents read as tool results. Tag smuggling has
  no normal use outside emoji tag sequences. Meaning is the judge's job (SMA-857).
- Kind `injection_markers`. The severity is the highest of the matched rules. Version `v1`.
- Evidence: one entry per matched rule, with the rule id and `Location::Result { index }`.
- Why no excerpt and no byte range: no consumer reads tool-result evidence. The judge never sees
  tool results (D40), and the webhook stage gets call subjects only (§ 6.6.2).
- A change of the pattern set is a new version (`v2`).

### 6.4 Severities (D18, D20, D21, D22)

| Kind | Detector | Severity | Reason |
|---|---|---|---|
| `name_collision` | `kernel` | high | Two definitions for one name is a confusion attack. The floor blocks it already. |
| `unparsed_arguments` (text input) | `kernel` | info | Every `custom` and `apply_patch` call has it. It is normal. |
| `unparsed_arguments` (bad JSON) | `kernel` | medium | A duplicate key can make the client and the gateway read different values. It is also a common model error. |
| `undeclared_tool` | `undeclared_tool` | medium | The model calls a tool that the client did not offer. That is a typical sign of an injection, but also of a model error. |
| `invalid_args` | `invalid_args` | low | Models often send small schema errors. The client validates too. |
| `unseen_host` | `unseen_host` | medium | A new host is the typical exfiltration path. Models also know real hosts. |
| `secret_in_args` | `secret_in_args` | high | A credential in a call is a leak whatever the cause. |
| `injection_markers` | `injection_markers` | high (`unicode_tags`) or medium | § 6.3.5. |
| `detector_error` | the failed detector, `built_in`, or `kernel` (hosts cap) | high | An unknown result is not safe. Decided by Sven (QB). |

### 6.5 The kernel decision with the new attributes

- `decide` (`decide.rs:133`) does not change its signature. It reads the severity from `Findings`
  and the hosts from `ToolInvocation` (D8).
- `decide_tool` does not change its signature.
- Request-level findings (`name_collision`) keep their place in `Findings`, now with a severity.

### 6.6 The detector port and the pipeline (gateway service)

Hexagonal layout: the port in `domain/detectors.rs`, the pipeline in
`application/detectors/pipeline.rs`, the built-in adapter in `adapters/detectors/built_in.rs`.

#### 6.6.1 The port

```rust
/// One thing to screen. It borrows nothing: the pipeline runs inside a 'static decider future.
#[non_exhaustive]
pub enum Subject {
    /// A tool call of the model (response side).
    Call { index: usize, invocation: ToolInvocation, schema: Option<Arc<Value>> },
    /// A tool result in the request (request side). `text` lives only for the request side.
    Result { index: usize, tool: Option<String>, text: Arc<str> },
}

#[non_exhaustive]
pub struct DetectBatch {
    pub side: Side,                         // Request | Response
    pub org: Option<Uuid>,
    pub request_id: Option<String>,
    pub subjects: Vec<Subject>,
    pub seen_hosts: Option<Arc<HostSet>>,
    /// The findings of the earlier stages, per subject, with evidence. Empty for the built-ins.
    pub earlier: Vec<Vec<Finding>>,
}

#[async_trait]
pub trait Detector: Send + Sync {
    /// The name in `Finding.detector`. It must be a known name (D37).
    fn name(&self) -> &str;
    /// One vector of findings per subject, in subject order.
    async fn detect(&self, batch: Arc<DetectBatch>) -> Result<Vec<Vec<Finding>>, DetectorError>;
}
```

- The stage of a detector is the field of the pipeline that holds it (§ 6.6.2).
- `DetectorError` (§ 6.2.4) has fixed variants and no text.
- The trait uses `#[async_trait]`, like `PolicySource` and `LimitStore` (`domain/limits.rs:517`).
- `#[non_exhaustive]` on `DetectBatch` stops other crates from building it with a struct literal.
  SMA-857 adds its fields in the same crate, so that change does not depend on the attribute.

#### 6.6.2 The pipeline

```rust
pub struct DetectorPipeline {
    built_in: Arc<dyn Detector>,
    webhooks: Vec<Arc<dyn Detector>>,  // empty in SMA-855
    judge: Option<Arc<dyn Detector>>,  // None in SMA-855
    timeout: Duration,                  // [tool_policy.detectors] timeout_ms, an upper bound per run
}

impl DetectorPipeline {
    /// One vector of findings per subject. Never fails: an error is a finding (D22).
    pub async fn run(&self, batch: DetectBatch, deadline: Instant) -> Vec<Vec<Finding>>;
}
```

The order:

1. **Built-ins.** The adapter calls `detect_call` for each `Call` subject and `injection_markers`
   for each `Result` subject. The stage runs as
   `AssertUnwindSafe(stage).catch_unwind().await`, as `adapters/decision_log/supervised.rs:34`
   does (C27). This holds on both sides. A panic gives one `detector_error` finding with detector
   `built_in` per subject. The pipeline writes one `tracing` event that names the stage, never the
   payload. The stage is synchronous, so the deadline cannot stop it. § 6.10 and § 7.6 bound its
   time.
2. **Webhooks (SMA-856).** All webhooks run at the same time (`futures::future::join_all`). The
   pipeline gives them a batch with the `Call` subjects only, and no result text.
3. **Judge (SMA-857).** It runs after the webhooks, with the findings of steps 1 and 2 in
   `earlier`. It gets no result subject (D40).

- Steps 2 and 3 run inside one `tokio::time::timeout_at(min(deadline, run_start + timeout), ..)`.
  It is a hard deadline, and it spawns no task (D30). At the deadline, the future of each
  unfinished detector is dropped, and each subject gets one `detector_error` finding with that
  detector's name.
- SMA-856 and SMA-857 define the budgets inside the stages.
- A detector that returns an error gives one `detector_error` finding per subject, with its name.
- The pipeline checks each finding per D37.
- The pipeline adds the findings of all stages per subject. Two findings of the same kind keep the
  higher severity in `Findings`, and both stay in the decision log entry.

#### 6.6.3 How SMA-856 and SMA-857 fit

- SMA-856 adds webhook adapters to `webhooks` and their config. Each webhook must declare a closed
  list of the kinds that it gives (§ 10).
- SMA-857 adds the judge adapter and the fields that it needs to `DetectBatch` (the message text,
  the caller and the session). SMA-857 also owns the flow of message text from the request side to
  the response side. The trait does not change. The data flow does.

#### 6.6.4 Where the pipeline runs

- Response side: in `CedarDecider`. The future becomes
  `async move { let found = pipeline.run(batch, deadline).await; decide_now(.., &found) }`. It is
  still one future that the relay polls (C13). With only the built-ins, it is ready at the first
  poll. The deadline comes from the hold (§ 6.9).
- Request side: in `RequestToolPolicy::apply_inner` (`request.rs:247`), after the policy load and
  before `plan_request` (§ 6.8).
- `build_tool_policy` (`runtime.rs:272`) builds one `DetectorPipeline` and gives an `Arc` of it to
  both the decider and the request side. With `[tool_policy.detectors] enabled = false`, it builds
  none.

### 6.7 The optimistic set (SMA-752 § 6.1.6)

- `ContextAttr` (`optimistic.rs:33`) gets `MaxSeverity`. `Reads::attr` maps `"max_severity"` to
  it. `ALL_READS` and `DROP_READS` get it, on one line of its own (for the mutation proof).
- `call.hosts` is under `call`, so `ContextAttr::Call` already covers it.
- Soundness: a forbid that reads `max_severity` or `call.hosts` is dropped from the optimistic
  set. A dropped forbid makes the set allow more, which is the safe side.
- Definitions get no detector findings. A definition is removed only through the optimistic path,
  as today.

### 6.8 Request side: tool results

#### 6.8.0 When the request side screens (D27)

The kind registry, one constant in `paigasus_kernel::detect`:

```rust
/// Kinds that the response side can give with no request screen.
pub const SCREEN_INDEPENDENT_KINDS: &[&str] =
    &["name_collision", "unparsed_arguments", "undeclared_tool", "invalid_args", "secret_in_args"];
// Every other kind is screen-dependent: `unseen_host`, `injection_markers`, `detector_error`,
// and every kind of SMA-856 and SMA-857.
```

The request side screens when all of these are true:

- the policy set is available, and `[tool_policy.detectors] enabled` is true; and
- the tenant part counts (mode `log_only` or `enforce`, and the tenant part is evaluated), **or**
  the floor-only set reads a screen-dependent kind.

The floor check:

- The snapshot is floor-only when the mode is `off` or the tenant part is unavailable. It then holds
  `floor.floor_only` (`request.rs:157-164`, C14).
- `ToolPolicies` gets a flag `reads_screen_dependent`, computed at compile time from the
  `policy_reads` walk (`optimistic.rs`). The request side reads the flag of `floor.floor_only` only.
  There is no separate flag on `FloorPolicies`.
- A policy is screen-dependent when it reads `max_severity`, reads `call`, or reads `findings` in
  any form other than `context.findings.contains(<string literal>)` whose decoded value is in
  `SCREEN_INDEPENDENT_KINDS`. The walk compares the decoded value from `Policy::to_json`, not the
  source text, so an escaped literal cannot hide a dependent kind. A read of `context` as a whole
  counts as dependent.
- The shipped floor (`contains("name_collision")`) is not screen-dependent. So mode `off` with the
  shipped floor screens nothing.

When no screen runs:

- No `screen_request`, no request-side pipeline, and `seen_hosts` is `None`.
- The response side runs the call built-ins, but not `unseen_host` (§ 6.3.3). `context.call.hosts`
  is still filled. D38 accepts the cost of the call built-ins in mode `off`.

Cost: with a screen, each request scans its history again. A chat sends every earlier result in
every turn, so the cost per turn is O(history), and the total over a session grows with the square
of its length. N10 lists a cache as a follow-up.

#### 6.8.1 The request screen (gateway-core, no I/O)

```rust
// paigasus-gateway-core::screen
#[non_exhaustive]
pub struct RequestScreen {
    /// One text per entry of `TurnFacts.tool_results`, in the same order. `None` when the result
    /// has no text (a screenshot, an approval).
    pub result_texts: Vec<Option<Arc<str>>>,
    /// The hosts of the seen message sources (§ 6.3.3).
    pub message_hosts: HostSet,
}

pub fn screen_request(format: Format, body: &[u8], turn: &TurnFacts, max_scan_bytes: usize)
    -> Result<RequestScreen, ScreenError>;
```

- **One walker.** `parse_request` and `screen_request` use the same format walker of `turn.rs`.
  The walker takes an optional text sink. `parse_request` passes none, so it does not change in
  behaviour or cost. `screen_request` runs the walker again with a sink, only when § 6.8.0 says so.
  So an interception-only deployment does not pay for the screen.
- `screen_request` checks that the keys of the results it saw (call id, name, item id, shape) equal
  `turn.tool_results`, in order.
- If `screen_request` fails, or the keys differ, while `parse_request` gave `ToolTurn::Parsed`,
  every result gets one `detector_error` finding with detector `built_in`. The message hosts are
  then empty.
- `RequestScreen` has a hand-written `Debug` (§ 6.2.4).
- The result texts are dropped when `apply_inner` returns. Only the seen-host set goes on.
- **Where the seen hosts go.** `apply_inner` takes `&DecisionContext` (C14), so it cannot write to
  the context. The seen-host set goes into the new snapshot: `PolicySnapshot` gets
  `seen_hosts: Option<SeenHosts>`, where `SeenHosts` wraps `Arc<HostSet>` with a `Debug` that
  prints the count. The snapshot already goes into `DecisionContext.policy` through
  `RequestOutcome` (SMA-752 D30), so the decider reads it from there.

#### 6.8.2 The result decision (D11, D12, D32)

`plan_request` gets the per-result findings and a clock function, like `decide_calls`
(`calls.rs:98`), for `decide_duration`:

```rust
pub fn plan_request(snapshot: &PolicySnapshot, format: Format, turn: &TurnFacts,
                    result_findings: &[Vec<Finding>], now: &dyn Fn() -> Instant) -> RequestPlan;
```

Targets and findings:

- A result belongs to each `ResultTarget` that it names. A result with both a call id and an item
  id (a Responses `local_shell_call_output`) belongs to the `CallId` target and the
  `LocalShellId` target. Its findings go into the union of both.
- For each target, F is the union of the findings of all its results. The `ResultJudge` cache
  (`plan.rs:151`) keeps one outcome per target, so a target is decided once.
- A target with an empty union keeps the SMA-752 path, unchanged.

For a target with findings:

1. Find the earlier calls of the target, as `plan.rs:265-307` does today.
2. For each earlier call c, build a `ToolInvocation` from its `PriorCall` (name, arguments and the
   new `kind` field), with `Declared::Unknown`. The gateway computes its hosts once with
   `call_hosts` and attaches them (`with_hosts`), so A and B share one set.
3. Verdict A: `decide` with F only. Verdict B: `decide` with no findings. The request-level
   findings (`name_collision`) are in neither (A9).
4. Let `F(v) = v.determining ∪ v.errored_forbids`. The target is replaced by the findings when,
   for any c:

   `A.decision == Deny && (B.decision == Allow || F(A) ⊄ F(B))`

   In words: the findings turn an allow into a deny, or they bring a forbid into the deny that was
   not there without them.
5. With no earlier call in the request (D32), use one pair with the result's tool name (or the
   empty name), input `{}` and `Declared::Unknown`. An input forbid with no reference to findings
   errors in both A and B, so it is in both F sets, and it does not replace the result.

Examples (each one is a test, § 7.3):

- Starter policy, a `high` finding: A is denied by the forbid, B is allowed (or denied with an
  empty F). Replaced.
- `permit (..) when { context.max_severity < 3 }`, a `high` finding: A has no permit, so it is
  `Deny`; B is `Allow`. Replaced.
- A permit on findings, `permit (..) when { context.findings.contains("injection_markers") }`,
  and an injection finding: A is `Allow`. Not replaced.
- `forbid (..) when { context.findings.isEmpty() || context.input.x == 1 }` with input `{}`: in B
  the left side is true, so the forbid is satisfied and is in F(B). In A the left side is false,
  the right side errors, and the forbid is an errored forbid in F(A). F(A) ⊆ F(B), and B is
  `Deny`. Not replaced: the forbid denies without the findings too.
- `forbid (..) when { context.findings.contains("injection_markers") && context.input.path like
  "/etc/*" }` with input `{}`: in B the left side is false, so Cedar does not evaluate the right
  side and the forbid does not error. In A the left side is true, the right side errors, and the
  forbid is in F(A) only. Replaced. That is correct: the policy reads the finding.
- The shipped floor, colliding definition names and a `medium` result finding: `name_collision` is
  in neither A nor B. Not replaced (A9).

Enforced and observed:

- The policies that make the difference are F(A) \ F(B), or all of F(A) when B is `Allow`. If any
  of them is a floor policy, the item is enforced in every mode (SMA-752 D33).
- Else, with a floor-only snapshot, the difference does not count, and nothing is planned.
- Else, the item is enforced in `enforce` mode, and not enforced in `log_only` mode. A not
  enforced item gives an `Observe` record and changes no byte (SMA-752 D25).
- When the optimistic path (unchanged) also plans the same target, the plan holds one item. Its
  `enforced` flag is the OR of the two. The report holds one entry. Its tool name is the name of
  the first earlier call whose pair differed, else the name of the optimistic hit (as today).

The decision-log detail of the target (§ 6.11.2):

- Replaced by the findings: A's `CedarDetail` of the first earlier call c whose pair differed.
- Replaced by the optimistic path only: A's `CedarDetail` of the first earlier call, so that the
  findings are in the log. When the optimistic path enforced through a floor policy, that floor
  forbid does not read input, findings, call or session, so it also denies in A and is in A's
  determining set. So the record shows the enforcing floor policy.
- Not replaced: A's `CedarDetail` of the first earlier call, in a screen record.

Other rules:

- `PriorCall` gets a `kind: ToolCallKind` field, so that the invocation gets `Json` or `Text` input
  as `mapping.rs:43-53` does. The builder fills it in each format parser.
- With the differential rule, A and B use the result's findings only. The call's own hosts still
  fill `context.call.hosts`.
- A `computer_call_output` has no text, so it gets no finding from `injection_markers`.

#### 6.8.3 The order in `apply_inner`

1. Load the policies (unchanged).
2. If § 6.8.0 says to screen: `screen_request`, then the pipeline on the `Result` subjects, with
   the deadline `now + timeout_ms`.
3. `plan_request` with the result findings.
4. Build the seen-host set: the message hosts, plus the hosts of each result that has no finding
   of `medium` or higher and that the plan does not replace with `enforced = true` (D14).
5. Rewrite or refuse (unchanged). Put the seen-host set into the snapshot.
6. Write the records (§ 6.11.2).

Time bound of the request side: `load_timeout_ms`, then the built-in run, then at most
`timeout_ms` for the webhook and judge stages. Past `load_timeout_ms` the set is unavailable, as
today, and no detector runs. Past `timeout_ms` the unfinished detectors give `detector_error`, and
the request goes on. There is no other timer on the request side.

### 6.9 Response side: tool calls

- Non-streaming (`intercept_body`, `interception.rs:96`) and streaming (the relay) both call the
  decider, so both run the pipeline the same way.
- `CedarDecider` builds one `Call` subject per call: the invocation from `invocation`
  (`mapping.rs:41`), the schema of the matching definition, and the seen hosts of the snapshot
  (`None` when no screen ran).
- `decide_calls` (`calls.rs:98`) gets the per-call findings and the hosts of each `CallDetection`,
  and adds them before `decide`.
- In a stream, the hold waits for the pipeline. The relay sends keep-alive comments while it waits
  (C13).
- **The overhead metric.** `decide_now` starts its timer at `decider.rs:76`. The pipeline run moves
  inside the timed region, so `gateway_tool_policy_overhead_seconds` includes it.

**The absolute deadline (D35).**

- Stream holds queue: the relay starts a decision only when none is in flight
  (`interception.rs:213-221`), but each hold's `decide_timeout` counts from the hold's completion
  (`interceptor.rs:322-329`). A timeout that starts with the pipeline run would let a second hold
  pass its `decide_timeout` and go to `on_failure`.
- So `ToolCallDecider` gets a new method with a default:
  `fn decide_by(&self, ctx, turn, calls, hold_start: Instant) -> DecideFuture` that calls `decide`
  by default. The relay and `intercept_body` call `decide_by` with the hold start. Existing
  deciders do not change.
- `CedarDecider::decide_by` computes `deadline = min(hold_start + decide_timeout - margin,
  now + timeout_ms)`, and passes it to `pipeline.run`.
- The margin covers what the deadline does not cover: the built-in run, the Cedar `decide` of each
  call, the kernel host pass, and the scheduler delay. The provisional value is 250 ms. The builder
  measures the p99 of that work at the default budget (a 4 MiB hold, 50 policies, the release
  build), sets the margin to at least twice the measured p99, and records the measurement in the PR
  body.
- At start: `timeout_ms + margin <= decide_timeout_secs * 1000`. With the default
  `decide_timeout_secs = 5`, the default `timeout_ms = 500` passes.
- Only a decider that stalls outside the pipeline goes to `on_failure`. The existing SMA-752 tests
  cover `on_failure`.

### 6.10 Config

```toml
[tool_policy.detectors]
enabled = true             # default true
timeout_ms = 500           # an upper bound for the webhook and judge stages of one run; default 500
max_scan_bytes = 16777216  # the text that one run scans; default 16 MiB
```

- The table is optional. With no table, the defaults apply. `deny_unknown_fields`, like every
  `[tool_policy]` table (C15).
- `enabled = false`: no pipeline is built, and no screen runs. `Facts.detection` is false, so the
  context holds `max_severity: -1` and `hosts: []` (§ 6.2.3). A policy that reads them then never
  fires on detector output: that is fail open. The switch is for an incident. The gateway logs one
  line at boot when it is off.
- `timeout_ms`: at least 1. With `[tool_policy] enabled = true`, D35 applies.
- `max_scan_bytes`: at least 65536, with no upper bound. The default, 16 MiB, is the largest
  request cap of the shipped routes (`responses.max_request_bytes`, C20). So a request at the
  default caps is scanned in full.
- Order on the request side: the result texts first, newest result first, then the message text
  of the seen sources. Reason: a skipped result fails toward a block (`detector_error`), and a
  short seen set fails only toward `medium` (`unseen_host`). A result that does not fit in full
  gets a `detector_error` finding with detector `built_in` and rule `scan_limit`. A message source
  that does not fit is not scanned for hosts.
- On the response side, the budget covers the arguments of one hold. A call whose arguments do not
  fit in full gets the same `detector_error` finding. `responses.max_held_bytes` (32 MiB) is larger
  than the default, so a hold over 16 MiB can get it.
- The chart does not project `[tool_policy]` yet (SMA-752 § 6.8), so there is no Helm change.

### 6.11 Decision log, proto and metrics

#### 6.11.1 Proto

`contracts/proto/paigasus/gateway/v1/tool_decision.proto`:

```proto
message Finding {
  string kind = 1;
  FindingSeverity severity = 2;   // SMA-855: the field that SMA-755 kept free
  string detector = 3;            // "kernel", "built_in", "undeclared_tool", ..., later a webhook name or "judge"
  string detector_version = 4;    // for example "v1"; empty when the detector has no version
  // No evidence field, now or later (SMA-855 D3).
}

enum FindingSeverity {
  FINDING_SEVERITY_UNSPECIFIED = 0;
  FINDING_SEVERITY_INFO = 1;
  FINDING_SEVERITY_LOW = 2;
  FINDING_SEVERITY_MEDIUM = 3;
  FINDING_SEVERITY_HIGH = 4;
}
```

- The proto values are the Cedar levels plus 1, because proto3 needs 0 for `UNSPECIFIED`. One
  mapping function in the adapter, with a test for each value.
- Append `REQUEST_ACTION_SCREEN_RESULT = 5` to `RequestAction`.
- `buf format -w`, then `moon run contracts:generate`. `:breaking` passes. The Python and TS
  bindings regenerate.

#### 6.11.2 Decision records

Write path:

- `CedarDetail.findings` (`record.rs:126`) becomes `Vec<FindingRecord>` with `kind`,
  `severity: Option<Severity>`, `detector` and `version`, sorted by kind and then detector. No
  evidence. The list holds the findings that the gateway gave and `Verdict.added`.
- `CedarDetail` gets `max_severity: Option<Severity>`.
- **The plan's record lists.** `RequestPlan.entries` stays the list of report entries.
  `RequestPlan` gets two lists:
  - `entry_details: Vec<Option<CedarDetail>>`, one per entry, in the same order. `record_entries`
    (`request.rs:241-245`) writes each entry with its detail, so each entry still gives exactly one
    record, with no double record.
  - `screen_records: Vec<ScreenRecord>`, one per target with findings that is not replaced. A
    `ScreenRecord` holds the tool name and the `CedarDetail`. It is not a report entry.
- `DecisionRecorder::record_request` (`application/decision_log.rs:139`) gets
  `cedar: Option<&CedarDetail>`. `build_request` (`decision_log/build.rs:173`) sets `rec.cedar`
  from it. A new `record_screen` writes a screen record with `request_action = SCREEN_RESULT`,
  `applied = Allow`, `block_code` unset and `prevention = NOT_APPLICABLE`. `RecordedAction` gets
  `ScreenResult`.
- A replaced result keeps its `REPLACE_RESULT` record, now with the detail. Its effect is `Block`
  or `Observe`, as today.
- D41: a refused request writes its entry records and its refusal record, as today, but no screen
  records. An `Allow` record inside a refused request would mislead.
- The report header does not change. `RequestEntry` (`report.rs:149-155`) gets no findings field.
- The Redis adapter (`adapters/decision_log/proto.rs:74`) maps each `FindingRecord` to a
  `pb::Finding`.
- The `tracing` event of each decision gets the kinds and severities, never evidence.

Read path (`proto.rs:190-225`, the export CLI):

- `FINDING_SEVERITY_UNSPECIFIED` decodes to `severity: None` ("unknown"). It is accepted, so an
  event written before SMA-855 still exports.
- `max_severity` is derived on decode from the findings. There is no proto field for it. With no
  findings: -1. With findings of known severity: the highest. With a finding of unknown severity:
  the fixed severity of its kind from § 6.4 (`name_collision` 3, `unparsed_arguments` 0, an
  unknown kind 3). The export summary counts these events as "severity inferred".
- `SCREEN_RESULT` decodes to `RecordedAction::ScreenResult` (`proto.rs:269-276`).
- An export CLI older than SMA-855 refuses `SCREEN_RESULT` with `BadEnum` and counts the event as
  unmappable. After a gateway rollback, such an older CLI then counts every screen record as
  unmappable. The release notes say so.

Trace (`decision_log/trace.rs:119-127`):

- The encoder writes `max_severity: <n>` and `call: { declared: "...", hosts: [] }` (D29). For an
  old event, `<n>` is the derived value above. The export summary lists the policies that read
  `context.call.hosts` as "not reproducible", as it does for `context.input`. The builder verifies
  that Dogwood accepts `hosts: []` against a `Set<String>` schema type. Request-side records stay
  skipped by the encoder (C17).

#### 6.11.3 Metrics

`paigasus-gateway/src/application/tool_policy/decider.rs:42`:

- `FINDING_KINDS` becomes `name_collision`, `unparsed_arguments`, `undeclared_tool`,
  `invalid_args`, `unseen_host`, `secret_in_args`, `injection_markers`, `detector_error`, `other`.
- A closed set `DETECTOR_LABELS`: `kernel`, `built_in`, `undeclared_tool`, `invalid_args`,
  `unseen_host`, `secret_in_args`, `injection_markers`, `webhook`, `judge`, `other`. A webhook's own
  name never becomes a label.
- `gateway_tool_policy_findings_total{side, kind, detector, severity}`. One table of the valid
  `(kind, detector, severity)` triples is the single source for the label mapping and for the
  prime at zero. Each triple is primed for both sides. A triple that is not in the table counts
  as `other`.
- New: `gateway_tool_policy_detector_seconds{side, stage}`, a histogram of one pipeline stage,
  `stage` in `built_in`, `webhook`, `judge`.
- Each new name goes into `paigasus_observability::names` and `names::ALL`, with a registry test
  and a `describe_*!` line. Update the row in `docs/ops/RUNBOOK-observability.md` (line 150) and
  run `repo:observability-drift`.
- No label holds a host, a rule id, a pointer or any content (A2).

### 6.12 The starter policy (D2)

A new section in `docs/tool-policies/tenant-guide.md`, "Detector findings", after § 3:

```cedar
@id("detectors-block-high")
forbid (principal, action, resource)
when { context.max_severity >= 3 };
```

The section also says:

- What each kind means, with its detector and severity (the table of § 6.4).
- `context.max_severity` is -1 when there are no findings. `context.findings.isEmpty()` is the
  plain test for "no findings".
- `unparsed_arguments` has severity `info` for every `custom` and `apply_patch` call. So for these
  calls `context.findings.isEmpty()` is always false, and `context.max_severity >= 0` is always
  true. Use `>= 1` or a kind test.
- Put the policy into `log_only` mode first. Read the decision log for a week. Then move to
  `enforce`.
- The detectors that read tool results run only when your mode is `log_only` or `enforce`.
- This policy is a `forbid`, so it never removes a tool definition (§ 6.7). It blocks calls and
  replaces tool results.
- What it blocks: `secret_in_args`, `name_collision`, `injection_markers` only for hidden Unicode
  tag text, and `detector_error`. The other injection markers are `medium`. They are signals, and
  the meaning of a text is the judge's job (SMA-857).
- `detector_error` is `high`. So a detector error, a scan limit, a screen failure or more than 256
  hosts blocks under this policy. That is fail closed (D22). At the default caps, a request is
  scanned in full. A larger cap or a smaller `max_scan_bytes` can give `scan_limit` on old results.
- **What one aggregated `max_severity` cannot express.** It says how bad the worst finding is, not
  which kind it is. So a policy cannot say "block on high, but not when the only high finding is
  `detector_error`" with `max_severity` alone. A tenant that wants fail open on detector errors
  writes a kind-based policy instead:

```cedar
@id("detectors-block-kinds")
forbid (principal, action, resource)
when { context.findings.containsAny(["secret_in_args", "injection_markers", "name_collision"]) };
```

  This form blocks every `injection_markers` finding, the `medium` ones too, because a kind does
  not carry its severity in `context`.
- A host allow list for one tool:

```cedar
@id("fetch-hosts")
forbid (principal, action == Pgs::Gateway::Action::"fetch", resource)
unless { ["docs.example.com", "api.example.com"].containsAll(context.call.hosts) };
```

- An allow list finds only the hosts of § 6.3.3. A bare host under a key outside the D15 list, or a
  host in an encoded string, is not in `context.call.hosts`.
- A host deny list fails open past the hosts cap, because the sentinel replaces the hosts after the
  first 255. Pair it with `detector_error`:

```cedar
@id("fetch-deny-hosts")
forbid (principal, action == Pgs::Gateway::Action::"fetch", resource)
when { context.call.hosts.containsAny(["evil.example"]) || context.findings.contains("detector_error") };
```

- The client writes the history. `unseen_host` is a hint, not proof. With a Responses
  `previous_response_id`, `unseen_host` fires often (§ 6.3.3). A tool result whose earlier call is
  not in the request is decided with the empty tool name (D32), so a policy that names a tool does
  not apply to it.

The builder validates each policy text of the section against the revision-2 schema in a unit
test in `paigasus-iam-core`, and runs the starter policy in the gateway end-to-end tests (§ 7.4).
The tests read the policy texts from one shared fixture file.

### 6.13 IAM schema, the re-compile rule and rollout (D28)

#### 6.13.1 The schema

- `paigasus-iam-core/src/tool_policy/schema.rs`: add `"max_severity": { "type": "Long",
  "required": true }` to the context in `applies_to` (`schema.rs:37-53`), and
  `"hosts": { "type": "Set", "element": { "type": "String" }, "required": true }` to `CallInfo`
  (`schema.rs:85`). Set `TOOL_SCHEMA_REVISION` to 2.

#### 6.13.2 The additive rule for the re-compile job (Sven, QA option 1)

A new function in `paigasus-iam-core/src/tool_policy/compare.rs`:

```rust
/// True when `new` gives the same enforcement as `old` for every policy that `old` accepted:
/// the same policy text, the same conditions, and a schema that only adds Record attributes.
pub fn outputs_equivalent_for_recompile(old: &CompiledOutput, new: &CompiledOutput) -> bool;
```

It is true when `old.cedar_policies == new.cedar_policies`, `old.conditions == new.conditions`,
and `new.cedar_schema_json` is an **additive extension** of `old.cedar_schema_json`, on the Cedar
schema grammar:

- A **type object** is a JSON object with a `"type"` key, at a place where the Cedar JSON schema
  expects a type: a value of `commonTypes`, the `context` of an `appliesTo`, an attribute value
  inside an `attributes` map, and the `element` of a `Set`.
- At a type object with `"type": "Record"`: the keys other than `attributes` are exactly equal. The
  new `attributes` map holds every key of the old map. For each shared key, the two attribute
  values are exactly equal, or both are `Record` type objects and the new one is an additive
  extension of the old one. The new map may have more keys, with any `required` value.
- Everywhere else: exactly equal, as `canonical_json` compares them (`compare.rs:18-23`; array
  order counts).

The rule follows the grammar, not a key name. An input property named `attributes` is a key
inside an `attributes` map, so it is an attribute like any other.

Where it runs: only in the re-compile job, at `recompile.rs:339`, in place of
`outputs_identical`. `outputs_identical` stays exact for the catalog coupling (`mod.rs:679`) and
for the diff report (`report.rs:96`). So a catalog write that adds an input property still gives
a "different" output, and still writes a `catalog_change` version, as today.

Effect: after the revision bump, the re-compile job finds every live set at revision 1
(`recompile.rs:243`). Each one compiles to the same policy text and conditions, and its schema
only adds the two attributes. So the job writes an identical live version at revision 2
(`write_identical`, `recompile.rs:374`), and no held row. `is_current` (`mod.rs:471-473`) is then
true, and catalog writes do not fail with `ToolPolicyRecompilePending` (`mod.rs:663-664`). In the
short time before the job reaches a set, a catalog write on that set still fails with
recompile-pending, as on every revision bump today. D39: the job writes a new version for every
tenant and the floor, so every gateway reloads every tenant at about the same time.

This is in scope for this PR.

#### 6.13.3 Rollout and rollback

- Deploy order: the gateway first, then IAM.
- Failure mode if IAM accepts a revision-2 policy before the gateway fills the attributes: a
  `forbid` that reads `max_severity` or `call.hosts` errors, and an errored forbid denies
  (SMA-752 D26). A `permit` that reads them errors and is skipped, so the call gets the default
  deny in `enforce` mode. Both fail closed.
- **Rolling IAM deploy.** An old replica (revision 1) sees the sets at revision 2 as not current
  (`mod.rs:471-473`), so it refuses catalog writes with recompile-pending. Its boot job stops with
  `OlderReplica` when the stored revision is higher (`recompile.rs:238`). If it checks before the
  new boot job writes revision 2, it can still write revision-1 versions after that job
  (`recompile.rs:19`). After the rollout, start the job once more with
  `POST /v1/tool-policy-recompile` (`adapters/http/tool_policies.rs:63`), so that every set is at
  revision 2.
- **IAM rollback.** After `write_identical` stamps every set at revision 2, an IAM at revision 1
  sees every set as not current. Every catalog write then fails with recompile-pending. Its boot
  job compiles each set at revision 1 and compares with the exact rule, so every set gets a held
  "different" row until an operator accepts it or the tenant saves it again. Only roll IAM back
  when that is acceptable.
- **Pre-rollback check.** Before an IAM or a gateway rollback, find the live sets that read the new
  attributes:

```sql
SELECT kind, org_id, version
FROM tool_policy_set_version
WHERE status = 'live'
  AND (source LIKE '%max_severity%' OR source LIKE '%hosts%');
```

  Each row is a set that fails closed on a gateway without the attributes, and that IAM at
  revision 1 refuses to compile. The tenant must change it first. The query is broad (`hosts` can
  match other text), so a row is a set to read, not a proof.
- Rollback order: IAM first, then the gateway.
- A forward-compatible `is_current` (one that accepts a higher revision) is a follow-up (N11).
- The release notes give the orders, the query and the re-compile trigger.

### 6.14 Error reasons (D26)

No new value in `contracts/proto/paigasus/common/v1/error.proto`. A deny on a finding is a normal
policy deny: `tool-call-denied` (315) for a call and `tool-result-blocked` (316) for a result. A
detector error is a finding. `BlockCode::Detector` stays unused (N3). The builder adds a comment
there that points to D2.

## 7. Tests

### 7.1 Kernel unit tests, per detector

- `undeclared_tool`: `Yes`, `No`, `Unknown`.
- `invalid_args`: each keyword of § 6.3.2, accepted and refused, on its instance type and on
  another instance type (no effect). `integer` with `5`, `5.0`, `5e0` and `5.5`. `enum` with `1`
  against `1.0`. Each skip: `$ref` with siblings, `additionalProperties: false` with
  `patternProperties`, `items` with `prefixItems`, a schema object for `additionalProperties`, a
  tuple `items`, an unknown `type` name, an unknown keyword. The work budget: a large `enum` under
  `items` on a long array stops with no finding. `null` against `required` and `type`.
- `unseen_host`: a URL in a nested value, in an array, in text input, in raw invalid JSON with
  `\/`. The forms `https:evil.example/x`, `https:/evil.example`, `https:\\evil.example` and
  `HTTPS://EVIL.EXAMPLE`. Each trailing character of § 6.3.3 step 2, and a balanced `(...)` inside
  a URL. Each scheme of the allow list, and `mailto:` and `file://` (no host). IDNA
  (`bücher.example` against `xn--bcher-kva.example`), a trailing dot, a port, user information,
  IPv4 and IPv6. A bare host under `url` and under `path`. Each seen source of each format,
  including Anthropic string `user` content and the Responses string `input`. D14 in `enforce` and
  `log_only`. No seen set (no finding). 257 hosts: the cap, the sentinel, and
  `Url::parse("http://<overflow>/")` fails.
- `secret_in_args`: one positive and one near-miss per rule. A secret behind a JSON `\u` escape,
  in a key, under a duplicate key behind a `\u` escape (the lenient visitor), and deeper than 16
  levels. The evidence holds no secret byte.
- `injection_markers`: one positive and one near-miss per rule. Zero-width characters inside a
  phrase, full-width letters (NFKC), a tag-character run, mixed case. Near-misses: the flag of
  England emoji tag sequence (no `unicode_tags` match), and a Rust or Python source file that holds
  `<|im_start|>` in a string (a `medium` finding only). The highest severity wins.
- `Debug` of each type of § 6.2.4 holds no content byte of a canary input.
- `context()`: `max_severity` is -1 with no findings and the highest level otherwise. `hosts` is
  sorted and unique. A precomputed `HostSet` is used as given. A forbid `context.max_severity >= 0`
  does not deny a clean call (A4). `Facts.detection = false` gives -1 and `[]`. `Verdict.added`
  holds `unparsed_arguments` and the hosts-cap `detector_error`. `Verdict.errored_forbids` holds
  only errored forbids.

### 7.2 Kernel property tests

- **No false `invalid_args` (A7).** Generate a JSON value. Derive a schema that the value
  satisfies. Add random unknown keywords, random skipped keywords, `prefixItems` next to `items`,
  `patternProperties` next to `additionalProperties: false`, and large enums under `items`. The
  detector must give no finding. Mutation proof: remove the `$ref` skip or the `prefixItems` skip,
  and a generated case must fail.
- **Optimistic soundness (A6).** Extend the SMA-752 § 7.1 generator with `context.max_severity`
  and `context.call.hosts`. Property: `decide_tool == DeniedForAllInputs` implies `decide == Deny`.
  Mutation proof: delete the `MaxSeverity` row of `DROP_READS`, and the property must fail.
- **`call_hosts` never panics.** Arbitrary strings and arbitrary JSON text, including invalid UTF-8
  boundaries in escapes, long runs of `:` and `/`, and brackets.
- **Determinism.** The same input gives the same findings in the same order.

### 7.3 Gateway-core tests

- `screen_request`: the result keys equal `parse_request`'s `tool_results` for every fixture of
  `turn.rs` and every result shape of SMA-776. A forced key mismatch gives `detector_error` with
  detector `built_in` on every result. The scan order and the budget of § 6.10.
- The floor check of § 6.8.0:
  - `context.findings.contains("unseen_host")`: screens;
  - `context.findings.contains("secret_in_args")`: does not screen;
  - `context == {}`: screens;
  - `context.findings.contains(context.request.model)` (not a literal): screens;
  - an escaped literal of a dependent kind (`"unseen\u{5f}host"`): screens;
  - the shipped floor: does not screen.
- The mode gate: `log_only` and `enforce` with the tenant evaluated screen; `off` and an unavailable
  tenant part follow the floor check.
- `plan_request`, the differential rule, one test per example of § 6.8.2: the starter policy; the
  `max_severity < 3` permit; the permit on findings; the `isEmpty() || input.x` forbid; the
  `contains(..) && input.path` forbid (replaced); the shipped floor with colliding names and a
  `medium` finding (not replaced); an unguarded input forbid with no earlier call (not replaced).
- `plan_request`, the accounting: a floor-only snapshot ignores a tenant difference; a floor
  difference is enforced in every mode; `log_only` gives an unenforced item; two results of one
  target decided once on the union; a `local_shell_call_output` with both ids in both unions; the
  optimistic path and the differential path on one target give one item, enforced is the OR, and
  the record shows the floor policy when the floor enforces.
- The records of § 6.11.2: `REPLACE_RESULT` with the detail, `SCREEN_RESULT` with `Allow`, one
  record per entry, and no screen record on a refused request.
- The decision record and the trace line carry kind, severity, detector and version, and no
  evidence.

### 7.4 Gateway end-to-end tests (`rs/crates/services/paigasus-gateway/tests/`)

Use the `tool_interception.rs` and `tool_policy.rs` harnesses with fixture policy directories.
Chat (non-stream and stream) and `/v1/responses`:

- **Signals only (A1, D2).** A tenant in `enforce` mode, and also in `log_only` mode, with the
  shipped floor and an allow-all tenant set. A call with a secret and a result with an injection go
  through unchanged. The screen runs (the mode counts). The decision log holds the findings in a
  response-side record and a `SCREEN_RESULT` record. The report header holds no findings.
- **Starter policy.** The same requests with the starter policy and a `high` finding: enforce
  blocks the call and replaces the result with the notice; log-only reports `Observe` and changes no
  byte (SMA-752 A2).
- **Timeouts.** A test-only slow detector in `application/tool_policy/testing.rs`, added as a
  webhook-stage detector:
  - slower than `timeout_ms`: a `detector_error` finding. With the starter policy the call is
    blocked in enforce mode. Without it the call is allowed. In a stream, keep-alive comments go
    out while it waits.
  - a stream with two holds and a slow detector that takes most of `decide_timeout` per hold: the
    second hold gets `detector_error` from the absolute deadline, not `on_failure`.
  - A decider that stalls past `decide_timeout` is not tested here. The SMA-752 tests cover
    `on_failure`.
- **Errors and panics.** An erroring detector gives a `detector_error` finding with its name. A
  test-only panicking built-in stage, with a canary in its panic payload, gives `detector_error`
  with detector `built_in`. No decision-log event, no metric line and no `tracing` event holds the
  canary. The test does not check stderr (§ 6.2.4).
- **Scan limit.** A request past `max_scan_bytes` gives `detector_error` on the unscanned results,
  newest results scanned first.
- **Request-side gate.** Mode `off` with the shipped floor: no screen runs (a test hook counts the
  calls), and no `unseen_host` finding appears on the response side. A fixture floor that reads
  `max_severity` (not the shipped floor) makes the screen run and blocks in mode `off`.
- **`enabled = false`.** No pipeline, no finding of a detector, and a policy on `max_severity` does
  not fire.
- **Config.** `timeout_ms + margin` above `decide_timeout_secs * 1000` fails at start. An unknown
  key in `[tool_policy.detectors]` fails at start. `max_scan_bytes` below 65536 fails at start. The
  defaults pass.
- **No evidence leak (A2).** One canary secret, one canary host and one canary injection phrase
  per test. After the run, search every captured log line, every report header, every metric line,
  every decision-log event (the Redis stream bytes in `decision_log_e2e.rs`), and the `{:?}` output
  of each type of § 6.2.4 for the canary bytes. None may hold them.
- **Metrics.** Every valid triple is primed at zero. A finding increments its triple. The overhead
  histogram includes the pipeline time.

### 7.5 IAM and decision-log tests

- The revision-2 schema parses. The policies of § 6.12 and a policy that reads `context.call.hosts`
  validate. `tool_schema_is_pinned_to_the_declared_revision` passes with revision 2.
- `outputs_equivalent_for_recompile` unit tests:
  - an added context attribute: equivalent;
  - an added `CallInfo` attribute: equivalent;
  - an added attribute with `required: false`: equivalent;
  - a removed attribute: different;
  - a changed attribute type: different;
  - changed policy text: different;
  - a changed input property named `attributes`: different; an added one: equivalent;
  - a changed `required`, a new entity type, a changed array order: different.
- `outputs_identical` stays exact: a test that an added attribute is "different" for it.
- An IAM test: a catalog write that adds an input property still writes a `catalog_change`
  version.
- An IAM integration test (with the Postgres harness of `tests/tool_policies_held_pg.rs`): live sets
  at revision 1, then a revision bump with only additive schema changes. The re-compile job gives
  `Identical` for every set, no held row exists, every live set is current, and a catalog write
  succeeds.
- A decision-log round trip with a pre-SMA-855 event (findings with kind only): it decodes, its
  severities are unknown, its derived `max_severity` follows § 6.11.2, and its trace line holds
  that value.
- Fixtures to update: the golden files under `rs/tools/paigasus-policy-compiler/tests/golden/`
  (including `tenant_schema.json`) and `EXPECTED_GOLDEN_HASH` in
  `rs/tools/paigasus-policy-compiler/tests/golden.rs:24`;
  `rs/tools/paigasus-policy-compiler/protocol/fixtures/base_schema.json`;
  `rs/tools/paigasus-policy-compiler/tests/fixtures/decision-trace.cedarschema`; the expected trace
  lines in `rs/tools/paigasus-policy-compiler/tests/decision_trace.rs` and in
  `rs/crates/libs/paigasus-gateway-core/tests/decision_trace.rs`.

### 7.6 Performance (D25)

- Extend `rs/crates/services/paigasus-gateway/tests/perf.rs` with the detectors on. The fixture of
  SMA-752 § 7.4 (50 policies, 30 definitions, results in the history, one 64 KiB argument) gets:
  one URL and one near-miss token in each argument, 32 KiB of result text in the history, history
  results that have findings (so the A/B pair runs), and the starter policy. Assert p99 < 15 ms.
  Record the numbers with and without the detectors in the PR body.
- A second fixture: 4 MiB of history (results and messages), with non-ASCII text (CJK, emoji,
  combining marks). It records p50 and p99 and does not assert the 15 ms bound. It shows the
  O(history) cost of § 6.8.0.
- A margin measurement for D35: the p99 of the built-ins, the host pass and `decide` on a 4 MiB
  hold with 50 policies.
- A kernel microbenchmark (an `#[ignore]` release-mode test): `detect_call` on 64 KiB of arguments
  and `injection_markers` on 1 MiB and 4 MiB of text. It is a measurement, not a gate.
- Command: `cargo nextest run --release -p paigasus-gateway --test perf --run-ignored only
  --no-capture`.

### 7.7 Repo gates

- `repo:wasm-getrandom-free` with the new `detectors` assertion (§ 6.1). Its mutation proof: add
  the feature to one binding crate in a scratch branch, and the gate must fail.
- `repo:machete`, `repo:deny` and `repo:osv` for the new kernel dependencies.
- `repo:observability-drift` for the metric names.
- `contracts:fmt`, `:breaking` and the codegen drift for the proto.

## 8. Left to the builder

- The exact regular expressions of § 6.3.4 and § 6.3.5, within the rule ids and severities that
  this spec gives, each with a source link.
- The internal layout of `HostSet`, `Evidence`, the triple table and the walker's text sink.
- The module name for the moved parser (D9).
- Whether `detect_call` and the Cedar mapping share one JSON parse, if the perf test needs it.
- The exact `PriorCall.kind` fill per format.
- The form of the `detectors` assertion in `repo:wasm-getrandom-free`.
- Whether `regex` becomes a workspace dependency.
- The check that Dogwood accepts `hosts: []` against `Set<String>` (§ 6.11.2).
- The measured margin of D35, recorded in the PR body.

## 9. Open questions for Gate 1

None. Sven answered QA (D28) and QB (D22) on 2026-11-22.

## 10. Follow-ups that this change does not do

- SMA-856: the webhook detector adapter and its config, in the webhook stage. Each webhook must
  declare a closed list of the kinds that it gives, so that the metric table and D37 stay closed.
- SMA-857: the judge adapter, its `DetectBatch` fields and its config, in the judge stage. The
  judge never sees tool results, including `role: "tool"` messages (D40, Sven's SMA-754 scope).
- SMA-853: the agent host `EventScreen` adapter, which calls `injection_markers` and
  `hosts_in_text` from the kernel (the port does not exist yet, C22).
- A cache of scan results per tool result (for example a digest), so that a turn does not scan its
  whole history again (N10).
- Detection on provider-run tool results (N9).
- A forward-compatible `is_current` in IAM, so that a rollback does not block catalog writes (N11).
- SMA-751 follow-up: the console shows the new kinds and severities.
- Bindings for the detectors, when Helikon needs them.
