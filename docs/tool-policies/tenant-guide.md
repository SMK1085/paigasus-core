# Tool policies: tenant guide (SMA-775)

This guide is for an organization admin. It explains how to turn on tool policies and how to
write a policy that works. The operator steps are in
[`docs/ops/RUNBOOK-tool-policies.md`](../ops/RUNBOOK-tool-policies.md).

A tool policy set decides which tool calls the AI Gateway allows for your organization. IAM
stores the set and compiles it. The gateway fetches the compiled set and enforces it.

## 1. Modes and fail modes

Each organization has two settings. Write them with
`PUT /v1/organizations/{id}/tool-policy-settings` (gRPC `PutToolPolicySettings`). The body is
`{"mode": "...", "fail_mode": "..."}`. Read them with `GET` on the same route. You need the
`ManageToolPolicySettings` action for the write and the `ReadToolPolicies` action for the read.
The `org_admin` role has both.

| `mode` | What the gateway does with your set |
|---|---|
| `off` | The gateway does not use your set. This is the default. |
| `log_only` | The gateway evaluates your set and records a block, but it does not block the call. |
| `enforce` | The gateway blocks every call that your set does not permit. |

The fail mode applies only in `enforce` mode. It tells the gateway what to do when it cannot load
your set.

| `fail_mode` | What the gateway does in `enforce` mode when it cannot load your set |
|---|---|
| `closed` | The gateway blocks the call. This is the default. |
| `open` | The gateway allows the call and reports it. |

In `log_only` mode, the gateway does not block a call when it cannot load your set. It only
records the call. In `off` mode, the gateway does not use your set, so your fail mode has no
effect.

The gateway must load your settings from IAM one time before they apply. Until that first good
load, the gateway uses mode `enforce` and the fail mode of the platform operator's global setting.
For example, this occurs when IAM is not available at a cold start of the gateway. After one good
load, the tables above apply. If a later load fails, the gateway keeps the last settings that it
loaded. The gateway removes the cache entry of an organization that sends no request for a long
time. After that, the rule for the first load applies again.

Your fail mode does not apply to the platform floor (section 2). When the gateway cannot load the
floor, the platform operator's global setting decides, in every mode. This includes `off`.

Follow these rules for the first enable.

- The first change away from `off` must go to `log_only`. This rule also applies when your
  organization saved a set while it was in `off`.
- A change to `enforce` before that first change fails with reason 61
  `tool-policy-mode-transition-invalid` (HTTP 409, gRPC `FAILED_PRECONDITION`).
- The first change to `log_only` saves the starter set as version 1, if your organization has no
  set. If it has a set, IAM keeps that set.
- A later change from `off` to `enforce` is allowed when your organization left `off` before and
  has a live set.
- A change from `off` to `log_only` is always allowed.

The starter set is this text, byte for byte (`STARTER_TOOL_POLICY_SOURCE`):

```
@id("starter-allow-all")
permit (principal, action in Pgs::Gateway::Action::"InvokeTool", resource);
```

The starter permits the tools in your catalog only. A tool that is not in the catalog is not
permitted. In `enforce` mode the gateway blocks it. In `log_only` mode the gateway only records it.
An empty set permits nothing, so in `enforce` mode it blocks every call.

If two admins make the first enable at the same time, one of them can get a version conflict
(`tool-policy-version-conflict`, HTTP 409, gRPC `ABORTED`). IAM writes nothing for that request.
The change of the other request is saved. Read the settings. If you still need a change, send
your request again. The second request succeeds.

## 2. The platform floor

The platform floor is a set of `forbid` policies that the platform operator owns. It applies to
every organization in every mode. This includes `off`. You cannot loosen it, and your mode and
fail mode do not change it. A floor block is always enforced.

## 3. Write a policy

You save the whole set with `PUT /v1/organizations/{id}/tool-policy-set`. The body is
`{"source": "...", "expected_version": N}`. Each policy needs a unique `@id`. The `@id` of a tenant
policy must not start with `floor-`. The action of a tool is `Pgs::Gateway::Action::"<name>"` in
lowercase. The group `InvokeTool` holds every catalog tool.

The call arguments are in `context.input`.

### Prefer an allow list

A `forbid` on argument values is a deny list. A deny list is not robust. Argument keys keep their
case, and a model can vary a value to avoid a pattern. Prefer a `permit` that names the values that
you accept. A call that no `permit` matches is blocked in `enforce` mode.

An allow list. The compiler accepts it:

```
@id("shell-allow")
permit (principal, action == Pgs::Gateway::Action::"shell", resource)
when { context.input has command && ["ls", "pwd"].contains(context.input.command) };
```

A second allow list, with a pattern:

```
@id("fetch-docs")
permit (principal, action == Pgs::Gateway::Action::"fetch", resource)
when { context.input has url && context.input.url like "https://docs.example.com/*" };
```

A deny list. The compiler accepts it, but use it only as an addition to an allow list:

```
@id("deny-rm")
forbid (principal, action == Pgs::Gateway::Action::"shell", resource)
when { context.input has command && context.input.command like "*rm -rf*" };
```

### A value of the wrong type makes a policy inert

The gateway maps the arguments of a call by value, not by your schema.

- An integer in the `i64` range becomes `Long`.
- A decimal number with at most four fraction digits becomes `decimal`.
- Any other number becomes a `String`.

The compiler checks your policy against the schema of your catalog. But the compiler cannot check
what the model sends at run time. If the model sends a value of another type than
your schema, the result depends on the operator:

- `<`, `<=`, `>`, `>=` and `like` cause an error. An errored `permit` is skipped, so it permits
  nothing. An errored `forbid` denies the call. The gateway records the basis `errored_forbid`.
- `==` and `contains` give `false`. They do not cause an error.
- `!=` gives `true`. It does not cause an error.

So a type mismatch can allow a call that your policy would deny:

- A `forbid` with `==` or `contains` does not match. A `permit` that matches then allows the call.
  For example, `forbid ... when { context.input.amount == 1000 }` does not stop the string
  `"1000"`.
- A `permit` with `!=` matches. For example, `permit ... when { context.input.amount != 0 }`
  allows the string `"0"`.

Write rules that fail closed:

- Prefer an allow list. A `permit` with `==`, `contains`, `like` or an ordering operator permits
  nothing when the type is wrong.
- Do not use `!=` or `!` in a `permit`.
- Do not use a `forbid` with `==` or `contains` as your only control. Add an allow list that
  permits only the values that you accept.
- In a `forbid` on a number, prefer an ordering operator. For example, use `>= 1000` instead of
  `== 1000`. A wrong type then causes an error, and the errored `forbid` denies the call.

Guard every argument read with `has`, and use a comparison that fits the type. This example
allows a payment of 100 or less:

```
@id("pay-small")
permit (principal, action == Pgs::Gateway::Action::"pay", resource)
when { context.input has amount && context.input.amount <= 100 };
```

The compiler also accepts the same policy with no `has` guard. Do not write it that way. The
compiler does not check what the model sends at run time.

A `has` guard in a `forbid` has a different effect. If the argument is absent, `has` is false, so
the `forbid` does not apply. A `permit` that matches then allows the call. For example, this
`forbid` does not stop a call with no `amount`:

```
@id("deny-big")
forbid (principal, action == Pgs::Gateway::Action::"pay", resource)
when { context.input has amount && context.input.amount >= 1000 };
```

Do one of these:

- Put the limit in the allow list. Use a `permit` with a `has` guard, as in `pay-small` above. A
  call with no `amount` then matches no `permit`, so the gateway blocks it.
- Add a second `forbid` for a required argument that is absent:

```
@id("deny-no-amount")
forbid (principal, action == Pgs::Gateway::Action::"pay", resource)
unless { context.input has amount };
```

### History rules are not yet available

A history rule is a temporal block: `formerly`, `count_within` or `sum_within`. The gateway cannot
enforce them yet (SMA-753). So IAM refuses a save that holds one, with reason 62
`tool-policy-history-unsupported`. The rejection names each policy that has the block. Remove the
block, or wait for history support.

An organization whose live set already holds a history rule (saved before this change) has a
problem. It cannot write its tool catalog until it removes the rule. The catalog write is refused
with `tool-catalog-breaks-policy`, and the message names the history reason. To fix it, save the
set again without the temporal block. Then write the catalog.

### A catalog change cannot break a live policy

A catalog change that would break your live set is refused with `tool-catalog-breaks-policy`.
Change the policy first, then change the catalog.

## 4. Detector findings

The gateway runs detectors on each tool call and on each tool result. A detector gives a finding.
A finding has a kind and a severity. A finding does not block anything by itself. A finding blocks
a call or a tool result only when one of your policies reads it.

Your policies read the findings in three context attributes:

- `context.findings` is the set of the finding kinds, for example
  `["secret_in_args", "unseen_host"]`. `context.findings.isEmpty()` is the plain test for "no
  findings".
- `context.max_severity` is the highest severity of the findings, as a number: `info` is 0, `low`
  is 1, `medium` is 2 and `high` is 3. It is -1 when there are no findings. So every comparison
  with a real level is false for a call with no findings.
- `context.call.hosts` is the set of the hosts in the call arguments, with no duplicates and in
  sorted order. It holds every host of the arguments, also a host that is in the history. It is
  empty for a call with no host. A comma does not end a URL in text. So
  `https://a.example,https://b.example` gives the host `a.example,https`, and a deny list can miss
  `b.example`.

### The finding kinds

| Kind | Detector | Severity | Meaning |
|---|---|---|---|
| `name_collision` | `kernel` | high | Two tool definitions have the same name. The platform floor blocks this already. |
| `unparsed_arguments` | `kernel` | info or medium | `info`: the call has text input (`custom` or `apply_patch`). `medium`: the arguments are not valid JSON, or they have a duplicate key. |
| `undeclared_tool` | `undeclared_tool` | medium | The model calls a tool that the client did not offer. |
| `invalid_args` | `invalid_args` | low | The arguments do not agree with the JSON Schema of the tool. |
| `unseen_host` | `unseen_host` | medium | A host in the arguments is not in the history of the request. |
| `secret_in_args` | `secret_in_args` | high | The arguments hold a credential, for example a cloud access key. |
| `injection_markers` | `injection_markers` | high or medium | A tool result holds a marker of a prompt injection. Only hidden Unicode tag text is `high`. |
| `detector_error` | the detector that failed: `built_in`, `kernel` or `webhook` | high, or the `failure_severity` of a webhook provider | A detector failed, or a scan limit stopped it. For a webhook provider, the severity is the `failure_severity` of that provider. |
| `forged_history` | `kernel` | high | The request has a tool result for a call that the gateway did not forward. This kind needs the session history. |
| `mixed_sessions` | `kernel` | medium | The calls and the tool results of the request belong to more than one session. This kind needs the session history. |
| `history_truncated` | `kernel` | low | The history store removed events of the session at a limit. This kind needs the session history. |

The three history kinds apply to each call of the request. Your policies read them only for a
call. The decision of a tool result does not use them.

`unparsed_arguments` is `info` for every `custom` and `apply_patch` call. So for these calls,
`context.findings.isEmpty()` is always false, and `context.max_severity >= 0` is always true. Do
not use these two tests to find a problem. Use `context.max_severity >= 1` or a test of a kind.

### The starter policy

This policy blocks a call or a tool result that has a `high` finding:

```
@id("detectors-block-high")
forbid (principal, action, resource)
when { context.max_severity >= 3 };
```

Add it to your set. Put your organization in `log_only` mode first. Read the decision log for one
week. Then change the mode to `enforce`.

This policy is a `forbid`, so it never removes a tool definition from a request. It blocks calls,
and it replaces tool results.

What the policy blocks:

- `secret_in_args`, `name_collision` and `forged_history`.
- `injection_markers` only for hidden Unicode tag text (rule `unicode_tags`). The other markers
  are `medium`: chat template tokens, turn markers, close tags and phrases such as "ignore the
  previous instructions". These markers occur in normal source code, tests and documentation, and
  a coding agent reads such files as tool results. So they are signals only. The meaning of a text
  is the job of a judge (SMA-857).
- `detector_error`. A built-in or kernel error is `high`, so it blocks under this policy. That is fail
  closed. A webhook failure has the `failure_severity` of its provider. Under this policy, a `high`
  failure blocks and a `low` failure does not. These are the causes of a detector error:
  - A detector fails.
  - A scan limit stops a scan.
  - The scan of the request fails.
  - A URL has an authority that is too long.
  - A call has more than 256 hosts.

  At the default limits, the gateway scans a request in full. A larger request limit or a smaller
  `max_scan_bytes` of the operator can give a scan limit on old tool results.

### When the detectors run

- The detectors that read tool results run when your mode is `log_only` or `enforce`. In mode
  `off`, the gateway does not scan the tool results, with one exception: a platform floor policy
  that reads a detector result, for example `context.max_severity`. Then the gateway scans the
  tool results also in mode `off`, and only the floor can replace a result. The shipped floor
  does not read a detector result. So with the shipped floor, the findings of a tool result are
  in the decision log only when your set is live and your mode is `log_only` or `enforce`.
- The platform operator can stop all detectors with `[tool_policy.detectors] enabled = false`.
  Then `context.max_severity` is -1 and `context.call.hosts` is `[]`, also when a finding of the
  kernel such as `name_collision` exists. `context.findings` still holds the kernel kinds. Cedar
  still evaluates your policies with these values. So the starter rule
  `context.max_severity >= 3` and a forbid on a host in `context.call.hosts` never match while
  the detectors are off. A rule such as `context.max_severity < 3` matches every call.

### How a tool result is decided

A tool result with findings is decided with the call that the result answers. The gateway
evaluates your set two times: one time with the findings and one time without them. The gateway
replaces the result when the findings make the policy deny the call. A forbid that applies also without the
findings does not replace the result. In `log_only` mode, the gateway records the replacement,
but it does not replace the result.

- A tool result whose earlier call is not in the request is decided with the tool name of the
  result, or with the empty tool name. A policy that names the tool of the result applies to
  it. When the result has no tool name, a policy that names a tool does not apply to it.
- The gateway never replaces a tool result with no call id and no tool name. The gateway records
  it in the decision log with its findings.

### What one aggregated `max_severity` cannot express

`context.max_severity` tells how bad the worst finding is. It does not tell which kind the worst
finding is. So a policy cannot say "block on `high`, but not when the only `high` finding is
`detector_error`" with `max_severity` alone. If you want to allow a call when a detector fails
(fail open), use a policy on the kinds:

```
@id("detectors-block-kinds")
forbid (principal, action, resource)
when { context.findings.containsAny(["secret_in_args", "injection_markers", "name_collision"]) };
```

This policy blocks every `injection_markers` finding, also the `medium` ones, because a kind in
`context.findings` does not carry its severity.

### Host rules

An allow list of hosts for one tool:

```
@id("fetch-hosts")
forbid (principal, action == Pgs::Gateway::Action::"fetch", resource)
unless { ["docs.example.com", "api.example.com"].containsAll(context.call.hosts) };
```

An allow list finds only the hosts that the gateway can extract:

- The gateway reads a host from a URL with the scheme `http`, `https`, `ws`, `wss` or `ftp`.
- The gateway also reads a bare host, with no scheme, from a string value under one of these keys:
  `url`, `uri`, `href`, `link`, `host`, `hostname`, `domain`, `endpoint`, `base_url` and
  `website`. A bare host under a different key is not in `context.call.hosts`.
- A host in a base64 string or in a different encoded string is not in `context.call.hosts`.

A deny list of hosts fails open when a call has more than 256 hosts. Then `context.call.hosts`
holds the first 255 hosts in sorted order and the value `<overflow>`, so a denied host can be
absent from the set. Always pair a deny list with a test for `detector_error`:

```
@id("fetch-deny-hosts")
forbid (principal, action == Pgs::Gateway::Action::"fetch", resource)
when { context.call.hosts.containsAny(["evil.example"]) || context.findings.contains("detector_error") };
```

`unseen_host` has these limits:

- The client writes the history of the request. So `unseen_host` is a hint, not a proof. A
  client can put any host into the history.
- With a Responses `previous_response_id`, the earlier turns are not in the request. Then
  `unseen_host` occurs frequently.
- A host does not count as seen when it is only in a tool result with a finding of `medium` or
  higher. An attacker can split the URL and the instruction into two tool results to get past
  this rule.

### Webhook providers

The platform operator can register webhook providers. A provider is an external service that
reads each tool call and returns findings. Ask your operator for the provider names and the kinds
that each provider returns.

A provider runs for your organization only when you opt in:

- In gRPC, set `webhooks` (a `WebhookOptIn` with `names`) in `PutToolPolicySettings`.
- In REST, set `webhooks` to a list of names.
- When you leave the field out, or send `null`, the list does not change. An empty list `[]`
  clears the list.

A name is `[a-z0-9_]{1,64}`. You can opt in to at most 16 names. The write fails with the reason
`invalid-webhook-opt-in` when a name is not valid. A name that the operator did not register, or
that is not open to your organization, gives a `detector_error` finding with severity `high`.

A provider runs only when your mode is `log_only` or `enforce`. The provider receives the
arguments of the tool call without redaction.

The finding of a provider has the kind that the provider declared. A policy reads it like any
other kind:

```cedar
@id("block-email-pii")
forbid (principal, action, resource)
when { context.findings.contains("pii_email") };
```

A provider that fails gives a `detector_error` finding. Its severity is the failure severity that
the operator set. A rule such as `context.max_severity >= 3` does not match a `low` failure.

When you opt out, the change can take up to `max_stale_secs` to reach a gateway during an IAM
outage with fail mode `open`. The gateway then uses the last known list until it expires.

## 5. Held versions

A new compiler or schema version can change how IAM compiles your set. IAM compiles your live
set again after such a change. If the output is the same, your set gets a new live version with no
action from you. If the output is different, or the compile now rejects the set, IAM keeps your
live version and stores a held version.

A held version is a proposal. The gateway never reads it. Your live set keeps working with no
change. IAM publishes no event for a held version.

1. List the held versions of your organization with
   `GET /v1/organizations/{id}/held-tool-policy-versions` (gRPC `ListHeldToolPolicyVersions`). You
   need the `ReadToolPolicies` action.
2. Read the `report` of each row. It shows the policies, conditions and schema names that differ.
   For a rejected set, it shows the rejections.
3. Accept the row with `POST /v1/organizations/{id}/held-tool-policy-versions/{version}/accept`
   (gRPC `AcceptHeldToolPolicyVersion`). Discard it with the same route and `/discard` (gRPC
   `DiscardHeldToolPolicyVersion`). You need the `ManageToolPolicies` action for both.

An accept makes the held output your next live version. Your own save of the set also replaces the
held version, because it makes a new live version. A rejected set has no output and you cannot
accept it. Change the source and save the set instead.

While a held version waits for your decision, your live set stays at the old compiler. After a
discard, IAM does not propose the same result again for the same live version and the same
compiler. In both cases, a write of your tool catalog is refused with
`tool-policy-recompile-pending`. In the first case, you accept the held version to clear this.
If the compile rejected the set, the held version has no output and an accept returns
`tool-policy-held-not-acceptable`. Correct the source and save the set to clear this. In the
second case, there is no held version, so you save the set again. In the first case, you can also
save the set again.

A save of the set or a discard removes the held version. An accept or a discard of that held
version after the save or the discard returns `tool-policy-version-not-found`. List the held
versions again.

An accept of a held version that still exists but is no longer current returns
`tool-policy-version-conflict`. This occurs when the compiler, the schema or the catalog changed
after IAM made the held version, or when a save of the set commits during the accept. List the
held versions again and use the current row, or save the set.

## 6. Error and rejection codes

A rejection is a successful response with the rejected list. An error uses the status shown.

| Reason | Form | Status | Meaning |
|---|---|---|---|
| `tool-policy-invalid` | rejection | - | The policy failed Dogwood or Cedar validation. |
| `tool-policy-shape-unsupported` | rejection | - | A block has an unsupported shape. |
| `tool-policy-lowering-failed` | rejection | - | Dogwood could not lower the policy. |
| `tool-policy-floor-permit` | rejection | - | A floor set contains a `permit`. |
| `tool-policy-id-invalid` | rejection | - | An `@id` or `@scope` is missing, duplicate, malformed or unknown. |
| `tool-policy-window-too-long` | rejection | - | A time window is longer than the limit. |
| `tool-catalog-breaks-policy` | rejection | - | A catalog write would break the live set. |
| `tool-catalog-invalid` | rejection | - | A tool name or a write breaks a limit. |
| `tool-catalog-conflict` | error | 409 `ALREADY_EXISTS` | Two tool names have the same match key. |
| `tool-policy-version-conflict` | error | 409 `ABORTED` | The expected version or catalog revision is not current. |
| `tool-policy-recompile-pending` | error | 409 `FAILED_PRECONDITION` | The live set is from an older compiler or schema. |
| `tool-policy-compiler-unavailable` | error | 503 `UNAVAILABLE` | The compiler crashed, timed out or broke the protocol. |
| `tool-policy-compiler-busy` | error | 429 `RESOURCE_EXHAUSTED` | The compiler is busy. Try again. |
| `tool-policy-held-not-acceptable` | error | 409 `FAILED_PRECONDITION` | The held version has no output because its compile failed (reason 63). |
| `tool-policy-version-not-found` | error | 404 `NOT_FOUND` | The version does not exist, or it is not live or superseded. It is also the answer for a held version that is gone. |
| `tool-policy-mode-transition-invalid` | error | 409 `FAILED_PRECONDITION` | The first enable must go to `log_only` (reason 61). |
| `tool-policy-history-unsupported` | rejection | - | The set has a history rule (reason 62). Not yet available. |
| `invalid-webhook-opt-in` | error | 400 `INVALID_ARGUMENT` | A webhook name is not `[a-z0-9_]{1,64}`, or the list has more than 16 distinct names (reason 64). The field is `webhooks`. |
