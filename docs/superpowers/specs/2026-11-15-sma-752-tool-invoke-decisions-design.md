# SMA-752: InvokeTool decisions in the gateway, with log-only and enforce modes

Linear: SMA-752 (parent SMA-747). ADR-0027, decisions (c), (d) and (e). Builds on SMA-749 (the
interceptor and the `ToolCallDecider` port), SMA-757 (complete calls only) and SMA-769
(`/v1/responses`). Related: SMA-751 (IAM tool policy sets), SMA-492 (gateway IAM event consumer),
SMA-753 (history), SMA-754 (detectors), SMA-755 (decision log).

Status: revision 2, after the builder check and the adversarial challenge. Approved by Sven at
Gate 1 on 2026-11-15, with Q1-A and Q2-A.

## 1. Intent

The gateway can see every complete tool call since SMA-749, but it allows every call. The only
decider is `AllowAll` (`rs/crates/libs/paigasus-gateway-core/src/decider.rs`). This change puts real
Cedar decisions behind that port.

A tenant must be able to say which tools a principal may call, and with which arguments. The
platform operator must be able to forbid calls for every tenant (the floor). A tenant must be able
to test its policies without effect (log-only) before it lets them block (enforce).

The decision function lives in `paigasus-kernel`, so that Helikon can use the same decision shape
in the agent (ADR-0005, ADR-0027 (e)).

## 2. Goals, non-goals, anti-goals

### Goals

- G1. `paigasus_kernel::tool_policy::decide`: synchronous and pure. It merges the floor and the
  tenant set, and returns the verdict and the policies that decided it.
- G2. Tool names are normalised: trim, Unicode NFC, then compared case-insensitively. Two
  definitions with the same normalised name give a `name_collision` finding.
- G3. Tool arguments become `context.input`. The request and the session become
  `context.request` and `context.session`.
- G4. A `PolicySource` port, a per-tenant cache of compiled policies, invalidation by IAM change
  events on NATS, and a periodic refresh as a backstop.
- G5. Request-side actions: remove a tool definition, reset a forcing `tool_choice` to `auto`,
  and replace a tool result with a notice.
- G6. A mode per tenant (off, log-only, enforce) and an explicit fail-open or fail-closed setting.
- G7. Three new gateway error reasons in `error.proto`.
- G8. p99 overhead below 15 ms on non-streaming requests with 50 policies.
- G9. End-to-end tests for log-only against enforce, and fail-open against fail-closed.

### Non-goals

- The IAM side: the tool catalog, tool policy sets, Dogwood compile and the IAM fetch endpoint
  (SMA-751). Until then, policies come from a file-backed source (§ 6.3.2).
- History and `when temporal` slots (SMA-753). `context.session` holds a placeholder.
- Detectors other than `name_collision` and `unparsed_arguments` (SMA-754).
- The decision log proto and its NATS publish (SMA-755).
- Anthropic Messages ingress (SMA-750). The core code supports the format, and § 6.5 covers it,
  but no route exists, so no end-to-end test covers it.
- Bindings: `decide` is not exported to py, node or wasm in this change.
- An MCP gateway (SMA-756).
- Protection against a malicious client. The client writes the conversation history, so the
  request-side actions are defence in depth, not a boundary (ADR-0027, "Negative and risks").

### Anti-goals (a failure even if the code works)

- A tool call that a policy denies reaches the client in enforce mode.
- Log-only mode changes a request or a response in any byte, except gateway comments and headers.
- A definition is removed when some input could make the call allowed.
- A tenant policy loosens the floor.
- A policy outage silently allows calls when the operator chose fail-closed.
- A model can get past a `forbid` by the way it spells an argument.
- The kernel gets a dependency on a gateway type, or the bindings get Cedar.
- A policy change reaches only one of the gateway replicas until the next refresh, while the
  others work normally. The accepted window under partial failure is in § 6.3.3.

## 3. Constraints found in the repo

- C1. `paigasus-kernel` has no Cedar today (`src/cedar.rs:1-2`). Its dependencies are `uuid` and
  `thiserror`. It is `publish = true`, and `repo:version-lockstep` ties it to the binding
  versions, so any kernel change causes a kernel release.
- C2. `cedar-policy = "4"` (lock: 4.14.1) is used only by `paigasus-iam-core`.
  `rs/crates/services/paigasus-gateway/Cargo.toml:78-80` keeps iam-core out of the gateway because
  it pulls Cedar. This change brings Cedar into the gateway through the kernel, on purpose
  (ADR-0027 (e)). Update that comment.
- C3. `cargo tree -p paigasus-iam-core --target wasm32-unknown-unknown -i getrandom` prints
  nothing (measured 2026-11-15). The feature gate of D2 still keeps Cedar out of the bindings.
- C4. The binding crates wrap chosen kernel functions by hand. `repo:parity-corpus-drift` needs a
  vector only for a bound function.
- C5. The decider port: `ToolCallDecider::decide(ctx: Arc<DecisionContext>, turn: Arc<ToolTurn>,
  calls: Vec<ToolCall>) -> DecideFuture`. One decision per call, in order, under
  `decide_timeout_secs` (default 5). `DecisionContext` holds `principal`, `scope` and
  `request_id`. The relay drops the decider future on a timeout or a client disconnect
  (`adapters/http/interception.rs:149-155`).
- C6. `TurnFacts` holds `definitions`, `prior_calls`, `tool_results` (call id and name, no
  content) and `definitions_complete`, but no `tool_choice`. SMA-749 D19 says "no request
  rewrite". This change adds the first one.
- C7. `ToolCall::display_name()` is `namespace.name` or `name` (`types.rs:84`).
- C8. Tenant identity: `CallerContext.scope_prn`. For an OIDC bearer it is the org. For an API
  key it is the key's scope, which can be an org, a team or a project (`auth.rs:286-289`).
  `domain::org_of` (`domain/mod.rs:107`) maps a scope to its org UUID, and limits already use it
  (`application/limits.rs:77`). There is no per-tenant settings struct.
- C9. The gateway has no NATS code. The `gateway-consumer` identity may pull only from the
  durable `gateway-cache-invalidator`. It has no `CONSUMER.CREATE` right, and
  `ops/nats/permissions.md` makes the durable's filter binding because of that.
- C10. The chart runs 2 gateway replicas by default (`charts/paigasus/values.yaml:121`). A shared
  durable pull consumer gives each event to one replica only.
- C11. Today's `iam.policy.put` event is for IAM's own policies. Its payload is
  `{"policy_id", "kind"}`, with no tenant scope (`paigasus-iam/src/application/policies.rs:120-131`).
- C12. Gateway error reasons use the plain `ERROR_REASON_<NAME>` form in the range 300-599. The
  last one is 314. `repo:error-code-single-site` needs a `MANIFEST` row in
  `ci/error-registry/check.py` for each source file that spells a code; an `emits` row needs a
  membership test.
- C13. Chat `parse_request` already gives `ToolTurn::Unavailable` for duplicate keys
  (`turn.rs:76`), case-folded classifier keys and the keys `__proto__`, `constructor` and
  `prototype` (`json.rs:75-91`). `/v1/responses` refuses duplicate keys at ingress.
- C14. Cedar 4.14.1 `decimal` has no `<` or `>`; it has `.lessThan()` and similar methods
  (`cedar-policy-core-4.14.1/src/extensions/decimal.rs:179`). A Cedar policy that compares a
  `decimal` with `<` errors.
- C15. `PolicySet::from_str` gives positional IDs (`policy0`, `policy1`, ...). They change when a
  policy is added or removed.
- C16. OpenAI function arguments are parsed by `parse_value_strict` without `check_numbers`
  (`openai/assembly.rs:244-257`), so an integer outside `i64` and `u64` is rounded to `f64`.
- C17. `pipeline::RequestFacts` already exists (`adapters/http/pipeline.rs:43`).
- C18. The workspace has no criterion setup. `paigasus-gateway-core/tests/perf.rs` is
  `#[ignore]`, runs in release mode, and measures without gating.

## 4. Approaches

These are the choices made in the conversation on 2026-11-15.

### 4.1 Shape of the kernel API (chosen: A)

- **A. Neutral kernel types (chosen).** The kernel owns `ToolInvocation`, `Facts`, `Findings`, a
  compiled `ToolPolicies` and a `Verdict`. Gateway-core keeps its provider types and maps into
  the kernel. The kernel has no provider concepts, and Helikon can call `decide` directly.
- B. Move the gateway-core types into the kernel. Rejected: provider-specific `ToolCallKind`
  variants would enter a shared public crate, and every gateway type change would cause a kernel
  release.
- C. `decide` takes policy text and compiles it on each call. Rejected: it puts the 15 ms p99 at
  risk.

### 4.2 How much of the policy source to build (chosen: port, cache and NATS)

- **Port, cache, NATS invalidation and refresh (chosen)**, in front of a file-backed source.
  SMA-751 adds only the IAM fetch adapter.
- Port and file source only. Rejected: the cache would stay untested until SMA-751.
- Port, cache and refresh, no NATS. Rejected.

### 4.3 How events reach every replica (chosen: every replica gets every event)

Sven chose "a consumer per replica" so that every replica gets every event. The challenge
showed that the JetStream form of that choice needs wildcard grants, which reach every consumer
on the stream, including the durable `gateway-cache-invalidator` (§ 6.3.5). The intent stays.
The mechanism is the open item **Q1** for Gate 1:

- **Q1-A. Core NATS subscription on `iam.policy.>` (chosen by Sven at Gate 1).** Each replica subscribes to the
  subject directly. A JetStream publish is also delivered to core subscribers of the subject.
  The grant is one `subscribe` permission on `iam.policy.>`, and no JetStream API right. Delivery
  is at most once. The reconnect invalidation and the 30 s refresh cover a lost message.
- Q1-B. A derived stream `IAM_POLICY_EVENTS` that sources only `iam.policy.>`, with per-replica
  ephemeral consumers on it. The wildcard grants then reach policy events only. It is at least
  once, but it adds a stream to provision and more grants.
- Q1-C. The JetStream per-replica consumer on `IAM_EVENTS`, as revision 1 had it. Not proposed:
  a stolen gateway credential could pull from, or change, other consumers on the stream.

### 4.4 Definition removal (chosen: only when no input can change the deny)

- **Remove a definition only if the tool is denied for every input (chosen).**
- Never remove in this change. Rejected.
- Remove on any deny with an empty input. Rejected.

### 4.5 Tool-result replacement (chosen: the result of a tool that is denied for every input)

- **Replace the result of a tool that is denied for every input (chosen).**
- Build the mechanism only, driven by findings. Rejected.
- A separate Cedar action for results. Rejected.

### 4.6 The floor and the tenant mode (open: Q2)

The challenge showed that in revision 1 the floor applies only when the tenant is in enforce
mode, and that a tenant set that fails to compile removes the floor. So the operator cannot
forbid for every tenant, and § 1 fails. This is the open item **Q2** for Gate 1:

- **Q2-A. The floor applies to every tenant, whatever the tenant's mode (chosen by Sven at
  Gate 1).** The floor is compiled and cached on its own, so a tenant set that fails does not
  remove it. In a tenant whose mode is `off`, only the floor is evaluated. Revision 2 first had a
  global `floor_mode` knob for this. On 2026-11-15 Sven removed the knob, to agree with SMA-751:
  **the floor is always enforced** when `[tool_policy] enabled = true` (D33).
- Q2-B. The floor follows the tenant mode. The spec then says plainly that the floor only
  advises until a tenant is in enforce mode.

## 5. Decisions

"Sven" marks a decision Sven made in the conversation. "Call" marks a decision I made, which
Sven can change at Gate 1. "Challenge" marks a call that the adversarial challenge caused.

| # | Decision | By |
|---|---|---|
| D1 | Kernel API shape A (§ 4.1). | Sven |
| D2 | The kernel module `tool_policy` is behind a cargo feature `tool-policy`, off by default. Only the gateway crates enable it. Add `[package.metadata.docs.rs] features = ["tool-policy"]` to the kernel. | Call |
| D3 | Policy source scope per § 4.2. | Sven |
| D4 | Every replica gets every policy event (§ 4.3), through a core NATS subscription on `iam.policy.>` (Q1-A). | Sven |
| D33 | The floor is always enforced for every tenant, whatever the tenant's mode, when `[tool_policy] enabled = true` (Q2-A, § 4.6). There is no `floor_mode` setting. This agrees with SMA-751. | Sven |
| D34 | `on_policy_unavailable` is global and required (D9). `[[tool_policy.org]]` may override it per org. When SMA-751 lands, the per-tenant `fail_mode` from IAM replaces the config override. When the tenant's policies are unavailable, the IAM value is unavailable too, so the last known per-tenant value applies, and without one the global value applies. | Sven |
| D5 | Definition removal per § 4.4, with the rule of § 6.1.6. | Sven |
| D6 | Tool-result replacement per § 4.5. | Sven |
| D7 | Per-tenant settings come from the gateway config, keyed by **org UUID**, in the `[[tool_policy.org]] id = "<uuid>"` shape of `[[limits.org]]`. The tenant of a request is `domain::org_of(scope_prn)`, so a team or project key uses its org's settings. An unknown or duplicate UUID fails at boot. IAM takes the settings over later. | Challenge |
| D8 | A tenant that is not in the config uses `default_mode`, default `off`. The ADR-0027 (d) start state (log-only, allow-everything starter policy) belongs to IAM (SMA-751). | Call |
| D9 | `on_policy_unavailable` (`open` or `closed`) has no default. With `[tool_policy] enabled = true` and no value, the gateway does not start. | Call |
| D10 | The error reasons: `ERROR_REASON_TOOL_CALL_DENIED = 315` ("tool-call-denied"), `ERROR_REASON_TOOL_RESULT_BLOCKED = 316` ("tool-result-blocked"), `ERROR_REASON_TOOL_POLICY_UNAVAILABLE = 317` ("tool-policy-unavailable"). The issue's `GATEWAY_` prefix comes from the `ErrorDomain`. "tool-policy" makes the name specific in a global registry. | Challenge |
| D11 | A blocked call or result does not fail the request (ADR-0027 (c)). The reasons appear in the notice and the decision report. The only new HTTP error is the refusal of D17. | Call |
| D12 | `context.input` mapping per § 6.1.4: by value, not by spelling. | Challenge |
| D13 | `context.findings` is a set of strings, one per finding kind. | Call |
| D14 | `context.session` is `{ id: "", turn: 0 }` until SMA-753. The issue asks for the placeholder. A policy must not read it before SMA-753. | Call |
| D15 | The floor that ships with the file source holds one policy, with an unconstrained action: `@id("floor-name-collision") forbid(principal, action, resource) when { context.findings.contains("name_collision") };` | Challenge |
| D16 | A policy event clears the cache of all tenants, because the event has no tenant scope (C11). SMA-751 must publish its tool-policy events under `iam.policy.>`. | Call |
| D17 | In enforce mode, a request whose turn is `ToolTurn::Unavailable` is refused with HTTP 400 and the existing invalid-request reason, before `limits.admit_for` (SMA-677 D9), so it uses no quota. This covers duplicate keys, case-folded keys and every other strict-parse failure (C13). In log-only mode it is forwarded, logged and counted as "enforce would refuse". | Challenge |
| D18 | Performance per § 7.4. | Challenge |
| D19 | The request-side actions run in `pipeline.rs::proxy` through a new service, `RequestToolPolicy` (§ 6.5.0). The `ToolCallDecider` trait signature does not change. The `Decision` type gets a new variant (D28). | Call |
| D20 | Cedar names: the principal is `to_cedar_uid(principal_prn)`, so its type follows the PRN (every IAM principal PRN has the resource type "principal", so the type is `Pgs::Iam::Principal`). The resource is the org: `Pgs::Iam::Organization::"<org uuid>"`. The raw scope PRN is in `context.request.scope`. The action namespace is `Pgs::Gateway::Action`. SMA-751 must compile tenant policies against these names. | Challenge |
| D21 | A new assertion in the script of `repo:wasm-getrandom-free`: for each of the three binding crates, `cargo tree -p <crate> -e normal -i cedar-policy` prints nothing. The gate's description and `ci/` README say that it now also guards Cedar. No new `repo:` gate. | Challenge |
| D22 | The action UID does not depend on the catalog: always `Action::"<normalised name>"`. A catalog tool has the parent group `InvokeTool`. Any other tool has the parent group `Unregistered`. So a floor forbid that names a tool applies in every tenant. A tenant with an empty catalog in enforce mode removes every definition, unless it permits `Unregistered`. | Challenge |
| D23 | With `definitions_complete = false`, `name_collision` is computed from the visible definitions. A collision with a tool that the gateway cannot see is not found. | Call |
| D24 | If a NATS behaviour that the design needs is not as measured, the builder stops and returns to Sven. The builder must not grant a wider right. | Call |
| D25 | Log-only mode computes the request-side plan and reports it with `enforced: false`, but changes nothing. | Challenge |
| D26 | A call for which every applicable **permit** errored is a normal deny. An errored **forbid** is a deny, with reason `tool-call-denied`, its own log field and metric. Cedar would skip it, which fails open. A panic or a timeout in the decider stays with `[tool_interception] on_failure`. | Challenge |
| D27 | A new public kernel API: a minor release (0.3.0), with the binding versions moved by `repo:version-lockstep`. | Call |
| D28 | `Decision` gets `Observe { would: BlockCode }`. `is_allow()` is true for it. The report schema goes to `v: 2` with `side`, `enforced` and `reason` (§ 6.5.3). | Challenge |
| D29 | Every policy needs an `@id("...")` annotation, unique in its set. A policy without one, or a template, is a `CompileError`. | Challenge |
| D30 | One policy snapshot per request. `RequestToolPolicy` puts its `Arc<ToolPolicies>` (or "unavailable") into `DecisionContext`, and `CedarDecider` uses it. The decider does not read the cache again. | Challenge |
| D31 | The policy load has its own deadline, `load_timeout_ms` (default 1000), shorter than `decide_timeout_secs`. A load that passes it is "unavailable" (§ 6.4), on the request side and in the decider. | Challenge |
| D32 | The kernel's request record is named `RequestInfo`, to avoid a clash with `pipeline::RequestFacts` (C17). | Challenge |

## 6. Design

### 6.1 Kernel: `paigasus_kernel::tool_policy` (feature `tool-policy`)

New dependencies, only under the feature: `cedar-policy` (workspace), `serde_json`,
`unicode-normalization` (already in the lock). No gateway crate.

#### 6.1.1 Types

All public types are `#[non_exhaustive]`.

```rust
pub struct ToolInvocation {
    pub name: String,              // raw name as the model sent it (namespace.name if any)
    pub input: ToolInput,          // Object(serde_json::Map) or Text(String)
    pub declared: Declared,        // Yes | No | Unknown: the name is among the request definitions
}

pub struct Facts {
    pub principal: CedarUid,       // to_cedar_uid(principal PRN)
    pub org: Uuid,                 // the tenant (D7, D20)
    pub request: RequestInfo,      // scope, ingress, model
    pub session: SessionInfo,      // id, turn (placeholder, D14)
}

pub struct Findings(BTreeSet<String>);

pub struct ToolPolicies { /* compiled: floor set, tenant set, action entities, optimistic sets */ }

pub struct Verdict {
    pub decision: Effect,               // Allow or Deny
    pub determining: Vec<PolicyRef>,
    pub errored: Vec<PolicyRef>,
    pub errored_forbid: bool,           // D26
}

pub struct PolicyRef { pub set: PolicyOrigin /* Floor | Tenant */, pub id: String }
```

`RequestInfo.ingress` is `"chat_completions"`, `"anthropic_messages"` or `"responses"`.
`Declared` is `Unknown` when the definitions are not complete.

#### 6.1.2 Compile

```rust
impl ToolPolicies {
    pub fn compile_floor(floor: &str) -> Result<FloorPolicies, CompileError>;
    pub fn compile(floor: Arc<FloorPolicies>, tenant: &str, catalog: &[String])
        -> Result<Self, CompileError>;
}
```

- Each policy needs an `@id`. The merged policy ID is `floor/<id>` or `tenant/<id>` (D29).
- A floor policy with the effect `permit` is a `CompileError` (ADR-0027 (d)).
- An action literal in either set that is not in normal form (`normalise(id) != id`, for example
  `Action::"Send_Email"`) is a `CompileError`, because it would never match. The two group
  identifiers `InvokeTool` and `Unregistered` are exempt from this rule.
- The catalog is normalised. Two catalog names with the same normal form, or a catalog name equal
  to `invoketool` or `unregistered`, are a `CompileError`.
- Action entities (D22): the groups `Pgs::Gateway::Action::"InvokeTool"` and
  `Pgs::Gateway::Action::"Unregistered"`, and one action per catalog tool with the parent
  `InvokeTool`. The action of a call that is not in the catalog is made per call, with the parent
  `Unregistered`.
- Compile builds the optimistic sets of § 6.1.6.
- No schema validation in the gateway. IAM validates on save (SMA-751). A policy that reads an
  absent attribute errors and is handled per D26.

`FloorPolicies` is compiled and cached on its own. A tenant set that fails to compile does not
remove the floor (§ 6.4).

#### 6.1.3 Name normalising

`normalise(name) = nfc(lowercase(nfc(trim(name))))`. `trim` removes Unicode white space at both
ends, and `lowercase` is `str::to_lowercase`. NFC runs again after lowercasing, because
lowercasing can leave a string that is not in NFC. The raw name stays in logs.

#### 6.1.4 Argument mapping into `context.input`

The mapping is by value, so a model cannot change the Cedar type by the way it writes a number
(D12):

| JSON | Cedar |
|---|---|
| object | record, keys unchanged, values mapped recursively |
| string | string |
| true or false | boolean |
| a number whose value is an integer in the `i64` range (`5`, `5.0`, `5e0`, `-0`) | `Long` |
| other number that `decimal` can hold exactly (at most 4 fraction digits, in range) | `decimal` |
| other number | string: the original number text from the request |
| array | set, elements mapped recursively |
| null | the key is left out |

- The mapping uses the original number text, not a re-parsed `f64`. The arguments are parsed
  with `check_numbers` and `arbitrary_precision` semantics, so an integer outside `i64` is never
  rounded (C16).
- The context is built only with `RestrictedExpression::new_record`, `new_set`, `new_string`,
  `new_long`, `new_bool` and `new_decimal`. It never goes through `Context::from_json_value`, so
  the keys `__entity` and `__extn` stay plain record keys and cannot forge an entity or an
  extension value.
- A depth limit of 16. A value deeper than that becomes a string of its JSON text.
- If the arguments are not a JSON object, `input` is `{}`, `context.input_text` holds the raw text
  (for kinds `custom`, `apply_patch` and `other`), and the finding `unparsed_arguments` is added.
- Keys keep their case. The spec of the policy language (for the SMA-751 docs) must say: a
  `forbid` on arguments is a deny list and is not robust against a client that maps keys
  case-insensitively. Prefer a `permit` allow list with `has` guards.

#### 6.1.5 `decide`

```rust
pub fn decide(call: &ToolInvocation, facts: &Facts, findings: &Findings,
              policies: &ToolPolicies) -> Verdict;
```

- Principal: `to_cedar_uid` of the principal PRN, no parents. Resource: the org, no parents.
- Context: `{ input, input_text?, findings, call: { declared }, request: { scope, ingress, model },
  session: { id, turn } }`. `declared` is per call, so it is in `context.call`.
- One `is_authorized` call on the merged set. `determining` comes from the reason set. `errored`
  comes from the errors. If an errored policy is a `forbid`, the verdict is `Deny` and
  `errored_forbid` is true (D26).
- No I/O, no clock, no random numbers, no global state.
- A policy that reads a context attribute the gateway does not fill (for example a SMA-753 history
  slot in IAM's lowered Cedar) errors. Per D26 a forbid then denies and a permit is skipped, so a
  missing slot fails closed (ADR-0027 (f)). Test both cases.

#### 6.1.6 Denied for every input

```rust
pub fn decide_tool(name: &str, facts: &Facts, findings: &Findings,
                   policies: &ToolPolicies) -> ToolVerdict; // DeniedForAllInputs | MayAllow
```

The result is `DeniedForAllInputs` only if the **optimistic set** denies the call. Compile builds
it from the merged set:

- Every `permit` keeps its scope (principal, action, resource) and loses its conditions.
- Every `forbid` whose conditions read `context.input`, `context.input_text`, `context.findings`,
  `context.call` or `context.session` is dropped. The other forbids read only the principal, the
  action, the resource and `context.request`, which are fixed for one request.

`decide_tool` evaluates the optimistic set with `{ input: {}, findings: <request-level findings>,
call: { declared: "yes" }, request, session }`. In the optimistic set, an errored forbid is
skipped, not turned into a deny. This makes the set allow more, which is the safe side for
removal.

Soundness: every change allows more. So if the optimistic set denies, the real set denies for
every input, every set of findings, every `declared` value and every session. `MayAllow` only
keeps a definition, and the call is still decided in full.

To find what a condition reads, walk `Policy::to_json`. Both `context.input.x`, `context has
input` and `context.input has x` count as reading `input`. A use of `context` as a whole value
counts as reading all of it. A JSON shape that the walk does not know also counts as reading all
of it. Each of these drops the forbid, which is the safe side.

### 6.2 Gateway-core: mapping and request rewrite

Gateway-core gets a dependency on the kernel with `tool-policy`. It stays I/O-free.

- `ToolCall -> ToolInvocation`: `name = display_name()`, `input` from the arguments (§ 6.1.4),
  `declared` from the turn. SMA-757 keeps the decided arguments equal to what the client runs.
- `TurnFacts` gets `tool_choice: ToolChoice` (`Auto`, `None`, `Required`, `Named(name)`,
  `AllowedTools(names)`, `Unknown`), parsed for each format.
- `name_collision`: group the definitions by normalised name. A group with more than one entry is
  a collision. Every call whose name is in such a group gets the finding. Colliding definitions
  are not removed (the D15 floor reads `context.findings`, so `decide_tool` drops it). Every
  call to them is blocked at call time.
- `rewrite_request(format, body, plan) -> Option<Bytes>`: pure. It returns `None` when the plan
  changes nothing, and the original bytes go upstream. It re-serialises from the strict parse,
  which has no duplicate keys, with the original number text.
- `Decision::Observe { would: BlockCode }` (D28) and `BlockCode::PolicyUnavailable`.

### 6.3 Policy source, cache and invalidation (gateway service)

Hexagonal layout: the port in `domain`, the cache and the decider in `application`, the adapters
in `adapters`.

#### 6.3.1 The port

```rust
#[async_trait]
pub trait PolicySource: Send + Sync {
    async fn floor(&self) -> Result<PolicyText, SourceError>;
    async fn tenant(&self, org: Uuid) -> Result<TenantPolicies, SourceError>; // set + catalog + version
}
```

#### 6.3.2 The file source

`adapters/policy_source/file.rs`. It reads a directory set in the config:

```
<dir>/floor.cedar
<dir>/tenants/<org uuid>/policies.cedar
<dir>/tenants/<org uuid>/catalog.json   # ["read_file", "send_email", ...]
```

- A missing `floor.cedar` is "unavailable", not an empty floor. An empty file is an empty floor.
- A tenant with no directory has an empty set and an empty catalog.
- The two tenant files are read from one directory snapshot: resolve the directory's real path
  once, then read both files under it. This matches the Kubernetes ConfigMap symlink swap.
- The version is a hash of the file contents.

#### 6.3.3 The cache

`application/tool_policy/cache.rs`. One entry for the floor, and one per org.

- An entry holds `Arc<ToolPolicies>`, the version and the load time.
- A load runs in a task that the cache owns (spawned on the runtime), and the waiters share its
  result. A dropped waiter (a timeout or a client disconnect, C5) does not cancel the load.
- Every waiter waits at most `load_timeout_ms` (D31). After that, the entry is "unavailable" for
  that request. The load goes on in the background.
- Refresh: every `refresh_secs` (default 30), reload each entry that was used in the last
  `idle_evict_secs` (default 3600). Evict the others. If the version is unchanged, do not compile.
- A failed refresh keeps the last good entry, up to `max_stale_secs` (default 300) after its load.
- `invalidate_all()` (from NATS) marks every entry as invalidated. The next request reloads. If
  that reload fails, the entry is "unavailable" at once under `on_policy_unavailable = "closed"`.
  Under `"open"`, the last good entry is still used up to `max_stale_secs`. So under `closed`,
  the stale window after a policy change is the event delivery time; without an event it is
  `refresh_secs`.
- A miss whose load fails is "unavailable" at once.

#### 6.3.4 The decider

`application/tool_policy/decider.rs`: `CedarDecider` implements `ToolCallDecider`.

1. Read the snapshot from `DecisionContext` (D30). The snapshot holds the floor, the tenant
   policies (or "unavailable"), the modes and the request facts.
2. If the tenant mode is `off`, evaluate the floor alone (D33). The floor is never skipped. The
   result is `Allow` unless a floor `forbid` matches. This path needs no `permit`.
3. For each call: build the `ToolInvocation`, `Facts` and `Findings`, and call `decide`.
4. Map the verdict per § 6.4.

Evaluation is synchronous inside the future. The decider spawns nothing (SMA-749 D25).

#### 6.3.5 The NATS subscription (mechanism per Q1)

`adapters/nats/policy_events.rs`, started from `runtime.rs` when `[tool_policy.nats]` is set.
This section describes Q1-A, which Sven chose.

- Each replica opens a core NATS subscription on `iam.policy.>`. It acks nothing and makes no
  JetStream API call.
- Permissions: `gateway-consumer` gets `subscribe` on `iam.policy.>`. It gets no
  `CONSUMER.CREATE` and no wildcard `MSG.NEXT`, `INFO` or `ACK`. Edit `ops/nats/subjects.env`,
  `ops/nats/test/accounts.conf.tmpl`, `ops/nats/permissions.md` and the provisioning together.
  `repo:nats-permissions` checks them. Add negative tests: the identity cannot subscribe to
  `iam.api_key.>`, and cannot pull from or update `gateway-cache-invalidator` with any new
  right.
- The builder measures, against the pinned NATS server, that a JetStream publish on
  `iam.policy.put` reaches a core subscriber with only that grant (D24).
- On every message: `invalidate_all()` (D16). Delivery is at most once. Two invalidations are
  harmless.
- On connect and on every reconnect: `invalidate_all()`, because messages may have been missed.
- A replica that starts after an event loses nothing: its cache is empty.
- If NATS is down, the client retries with backoff and records a metric. The refresh is the
  backstop. A NATS outage never blocks a request.
- Client settings: the inbox prefix `_INBOX_GW` (`permissions.md` § 7), the creds file read by the
  same callback that IAM uses (SMA-493 D8), and an optional CA file. SMA-492 can reuse the
  section later.

### 6.4 Settings, modes and failure

```toml
[tool_policy]
enabled = true
default_mode = "off"              # off | log_only | enforce (tenant mode)
on_policy_unavailable = "closed"  # open | closed; required when enabled (D9, D34)
refresh_secs = 30
max_stale_secs = 300
idle_evict_secs = 3600
load_timeout_ms = 1000

[tool_policy.source]
kind = "file"
dir = "/etc/paigasus/tool-policies"

[tool_policy.nats]                # optional; absent = refresh only
url = "nats://..."
creds_file = "..."
ca_file = "..."                   # optional

[[tool_policy.org]]
id = "0190a100-0000-7000-8000-0000000000aa"
mode = "enforce"
on_policy_unavailable = "open"    # optional per-org override (D34)
```

`[tool_policy] enabled = true` needs `[tool_interception] enabled = true`. Otherwise the gateway
does not start.

**Which policies apply (D33).** The floor is always enforced. The tenant set follows the tenant
mode. One evaluation covers both: a deny that a floor policy decided is always enforced, and a
deny that only tenant policies decided follows the tenant mode. A tenant whose mode is `off` is
evaluated against the floor alone. If the tenant set is unavailable, the floor still applies,
and the tenant part follows the tenant's `on_policy_unavailable` (D34). If the floor itself is
unavailable, `closed` blocks every call with `PolicyUnavailable` in every tenant mode, and
`open` allows them and reports `tool-policy-unavailable`.

| Effective mode of a deny | Calls | Request side | Report |
|---|---|---|---|
| `off` | `Allow`, no evaluation | no change | as today |
| `log_only` | `Observe { would }` | plan reported, no change (D25) | `enforced: false` |
| `enforce` | `Block { code: Policy }` | § 6.5 applies | `enforced: true` |

Policies unavailable (no entry, a failed load, a load past `load_timeout_ms`, or a PRN that does
not parse):

| Mode | `on_policy_unavailable = "closed"` | `"open"` |
|---|---|---|
| `log_only` | `Observe { would: PolicyUnavailable }` | `Observe { would: PolicyUnavailable }` |
| `enforce` | `Block { code: PolicyUnavailable }` | `Allow`, reported as `tool-policy-unavailable` |

With policies unavailable, the request side changes nothing: no policy can show that a tool is
denied for every input.

`[tool_interception] on_failure` still covers hold limits, malformed records, truncation, the
decide timeout and a decider panic. The two settings are separate.

### 6.5 Request-side actions

#### 6.5.0 Where it runs (D19)

`pipeline.rs::proxy` builds the interception tuple: it calls `parse_request` and makes the
`DecisionContext`. Directly after that, before `limits.admit_for` and before `state.openai.send`,
it calls `RequestToolPolicy::apply(ctx, format, &turn, body) -> Result<RequestOutcome, Refusal>`.

- `Refusal` is the D17 400.
- `RequestOutcome` holds the body to send (original or rewritten), the request-side report
  entries, and the snapshot that goes into `DecisionContext` (D30).

#### 6.5.1 The steps

With policies available, and an effective mode that is not `off`:

1. Compute the request-level findings (`name_collision`).
2. For each definition, call `decide_tool`. A `DeniedForAllInputs` definition is planned for
   removal.
3. For each tool result, call `decide_tool` with the result's tool name. A `DeniedForAllInputs`
   result is planned for replacement. Where the result names no tool (a Chat `role:"tool"`
   message has only the call id), the name comes from the earlier assistant call with that id in
   the same request. If two earlier calls have that id, use each name and plan the replacement
   if any of them is denied. With no such call, the result is not changed.
4. `tool_choice` after the removals:
   - `Named(n)` with `n` removed: set it to `auto`.
   - `AllowedTools(names)`: remove the removed names. If none are left, set `auto`.
   - `Unknown` and at least one removal: refuse the request (D17 400). The gateway cannot show
     that the rewritten choice is valid.
5. When the last definition is removed:
   - Chat: drop `tools`, `tool_choice` and `parallel_tool_calls`; for the legacy form, drop
     `functions` and `function_call`.
   - Responses: drop `tools`, `tool_choice` and `parallel_tool_calls`. A `namespace` tool with no
     inner tools left is removed.
   - Anthropic: if the messages hold `tool_use` or `tool_result` blocks, do not remove the last
     definition. The calls are still blocked at call time. Otherwise drop `tools` and
     `tool_choice`.
6. In enforce mode, apply the plan with `rewrite_request`. In log-only mode, only report it.

The builder must record the real upstream answer for each case of steps 4 and 5 where an API key
is available (OpenAI Chat, OpenAI Responses, Anthropic), in the way SMA-757 recorded real
streams. A case that cannot be recorded is listed as not measured in the PR.

#### 6.5.2 Where the definitions and results are

Definitions:

- Chat: `tools[]` (by `function.name`), legacy `functions[]`.
- Anthropic: `tools[]` (by `name`).
- Responses: `tools[]`, by the name that `turn.rs` gives each definition, including the built-in
  client-run tools (`local_shell`, `shell`, `apply_patch`, `computer`). A hosted tool that the
  provider runs (for example `web_search`) has no call that the client runs, and is not removed.

Results (every type that `turn.rs:240-249` reads as a result):

- Chat: `role:"tool"` (`content`), legacy `role:"function"`.
- Anthropic: `tool_result` blocks (`content`, and `is_error: true` is set).
- Responses: `function_call_output`, `custom_tool_call_output`, `computer_call_output`,
  `local_shell_call_output`, `shell_call_output`, `apply_patch_call_output` (`output`), and
  `mcp_approval_response` (changed to a refusal).

A result refers to its call by `call_id` (`tool_call_id`, `tool_use_id`, `approval_request_id`).
A Responses `local_shell_call_output` can have only an `id`. That `id` is matched against both
the `call_id` and the item `id` of every earlier `local_shell_call`, and the output is replaced
when either one matches a denied call (fail-safe, CodeRabbit round 2 on PR 475).

The notice replaces the whole content (ADR-0027 (c)). Its text is fixed and built from the reason
and a cleaned tool name (SMA-749 D21).

#### 6.5.3 Reporting

Report schema `v: 2` (D28). Each entry has `side` (`request` or `response`), `enforced` and
`reason` (`tool-call-denied`, `tool-result-blocked` or `tool-policy-unavailable`). The existing
`code` field stays for the SMA-749 block codes. A request entry has no `choice` or `position`;
it has `tool` (the cleaned name) and `action` (`remove-definition`, `replace-result`,
`reset-tool-choice`).

- Order: call entries first, then request entries, so the 2 KiB header cap removes request
  entries first. If any request entry is removed, one summary entry gives their count.
- Non-stream: the `paigasus-tool-decisions` header is sent whenever an entry exists, also on a
  non-2xx upstream answer and on a body with no calls.
- Stream: the request entries go out as `: paigasus-decision` comments before the first upstream
  frame.
- Log-only reports tell the caller what its own tenant's policies would block. That is accepted:
  the caller belongs to the tenant.
- The determining policy IDs go to logs and metrics, not to the report.

### 6.6 Error reasons

Append to the gateway range of `contracts/proto/paigasus/common/v1/error.proto`, under a new
banner "Tool decisions (SMA-752)", the three values of D10. Run `buf format -w` and
`contracts:generate`.

`MANIFEST` rows in `ci/error-registry/check.py`: an `emits` row for each source file that spells
a new code (at least `paigasus-gateway-core/src/report.rs` and the gateway's
`application/tool_policy/` module), each with a membership test against
`ErrorReason::from_wire_reason`. Gateway-core gets a dev-dependency on `paigasus-proto` for those
tests.

### 6.7 Observability

- `gateway_tool_policy_decisions_total{side, mode, effect, basis}`, where `basis` is `policy`,
  `errored_forbid` or `unavailable`.
- Metrics for cache loads (result), load time, compile time, stale entries, evictions, refresh
  errors, NATS invalidations and NATS connection state.
- A `tracing` event per decision: request ID, cleaned tool name, verdict, determining and errored
  policy IDs, mode. Never arguments or result content (ADR-0027 (i)).
- New names go in `paigasus_observability::names`. Check `repo:observability-drift`.

### 6.8 Rollout and rollback

- The feature ships with `[tool_policy] enabled = false` (the default). The chart does not
  project `[tool_interception]` or `[tool_policy]` yet. An operator enables both in the gateway
  config.
- Order: enable with `default_mode = "off"` and the shipped floor (it enforces at once, D33);
  then move tenants to `log_only` and then `enforce`, one by one. To test a new floor policy
  first, put it in one tenant's set in log-only mode, then move it to the floor.
- Rollback: `[tool_policy] enabled = false`, or the mode of one tenant to `off`.

## 7. Tests

### 7.1 Kernel unit and property tests

- Normalising: trim, NFC (composed against decomposed), case, a string that lowercasing takes out
  of NFC.
- Argument mapping: every row of § 6.1.4. `5`, `5.0`, `5e0` give the same `Long`. The decimal
  edges. A big integer keeps its text. `{"__entity": {...}}` and `{"__extn": {...}}` stay records.
- Errored forbid: a floor forbid `context.input.amount > 1000` with `{"amount": 1500.5}`
  (a `decimal`, which errors on `>`) denies.
- Floor: a floor `permit`, a policy without `@id`, a template and a non-normal action literal
  are `CompileError`s. A floor forbid wins over a tenant permit. A floor forbid that names a tool
  applies to that tool in a tenant whose catalog does not list it.
- Unregistered: denied by default; allowed when the tenant permits `Unregistered`; the floor
  still applies then.
- Property (soundness of § 6.1.6): for generated policy sets, inputs, findings, `declared` values
  and sessions, `decide_tool == DeniedForAllInputs` implies `decide == Deny`. The generator covers
  `has`, `like`, `==`, `<`, `contains`, sets, records, `decimal` method calls, `unless`, action
  groups, `Unregistered`, `context.call`, `context.findings` and `context.request`. Mutation
  proof: delete one drop rule of the optimistic set, and the property must fail.

### 7.2 Gateway tests (`services/paigasus-gateway/tests/`)

Use the `tool_interception.rs` harness with fixture policy directories. Chat (non-stream and
stream) and `/v1/responses`:

- log-only against enforce, for an allowed call and a denied call. Log-only output must be
  byte-equal to mode `off` apart from gateway comments and headers.
- fail-open against fail-closed, with an unavailable source and with a source that is slower than
  `load_timeout_ms`, in both modes.
- An argument policy (`read_file` only under `/workspace/`): an allowed and a denied path.
- Definition removal, each `tool_choice` case of § 6.5.1 step 4, each last-definition case of
  step 5, and no removal when an input-reading permit exists.
- Tool result replacement for each result type, including the Chat case where the name comes
  from the earlier call.
- `name_collision` with the shipped floor.
- An `Unavailable` turn: refused in enforce mode before the limits, forwarded and counted in
  log-only mode.
- An empty catalog in enforce mode removes every definition.
- A team-scoped and a project-scoped API key use their org's mode.
- Tenant `off` and tenant `log_only`: a floor forbid still blocks (D33).
- A per-org `on_policy_unavailable` override wins over the global value (D34).
- An unavailable floor under `closed` blocks every call; under `open` it allows and reports.
- Mode `off` everywhere: the request and response bytes are unchanged.
- Config: `enabled` without `on_policy_unavailable`, an unknown key, and a duplicate org UUID
  fail at start.

### 7.3 Cache, file source and NATS

- Cache: shared load, a dropped waiter does not cancel the load, `load_timeout_ms`, last good on
  a failed refresh, `max_stale_secs`, a failed reload after an invalidation under `closed`,
  eviction, no compile on an unchanged version.
- File source: a missing floor is unavailable; one snapshot for both tenant files.
- NATS (Docker, through `paigasus-test-docker`): two subscribers both get one JetStream publish on
  `iam.policy.put`, and each invalidates. The negative permission tests of § 6.3.5.

### 7.4 Performance (D18)

The target is the policy work per request, not the HTTP round trip:

- Measure, for each request, the time of the cache lookup, `RequestToolPolicy::apply` and the
  decider (`decide` for each call). Record it in a histogram.
- Fixture: 50 policies, all in scope for the called action, with argument and name conditions;
  30 definitions; tool results in the history; one rewrite; one call with an argument of 64 KiB.
- 2000 non-stream Chat requests, 8 at a time, warm cache. Assert p99 < 15 ms.
- Measure the cold load and compile of 50 policies apart from that, and assert < 50 ms.
- An `#[ignore]` release-mode test in the gateway service. The PR runs it once and records the
  numbers in the PR body:
  `cargo nextest run --release -p paigasus-gateway --test perf --run-ignored only --no-capture`.

## 8. Left to the builder

- The exact `ToolChoice` parse rules for each format, and the cleaned-name and notice text.
- The internal layout of `ToolPolicies` and `FloorPolicies`.
- The NATS reconnect backoff values.
- The generator of the § 7.1 property test, within the coverage it lists.

## 9. Follow-ups that this change does not do

- SMA-751: the IAM `PolicySource` adapter, a tool-policy event under `iam.policy.>` with an org
  scope (so invalidation can become per tenant), the start state of a tenant, and the
  policy-language note of § 6.1.4.
- SMA-492: the other gateway caches can reuse the NATS section of § 6.3.5.
- SMA-753: fill `context.session` and the history slots.
- SMA-754: more findings.
- SMA-755: the decision log.
- Bindings for `decide`, when Helikon needs them.
