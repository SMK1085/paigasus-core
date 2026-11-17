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

| `fail_mode` | What the gateway does when it cannot load your set |
|---|---|
| `closed` | The gateway blocks the call. This is the default. |
| `open` | The gateway allows the call and reports it. |

Follow these rules for the first enable.

- The first change away from `off` must go to `log_only`, if your organization has no set.
- A change to `enforce` in that state fails with reason 61 `tool-policy-mode-transition-invalid`
  (HTTP 409, gRPC `FAILED_PRECONDITION`).
- The first change to `log_only` saves the starter set as version 1.
- A later change from `off` to `enforce` is allowed when your organization has a live set.

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

If the model sends a value of another type than your schema, a comparison is false or it errors.
An errored `permit` is skipped, so it permits nothing. An errored `forbid` denies the call. A type
mismatch never allows a call that your policy would deny. But it can make a `permit` inert.

So guard every argument read with `has`, and use a comparison that fits the type. This example
allows a payment of 100 or less:

```
@id("pay-small")
permit (principal, action == Pgs::Gateway::Action::"pay", resource)
when { context.input has amount && context.input.amount <= 100 };
```

The compiler also accepts the same policy with no `has` guard. Do not write it that way. The
compiler does not check what the model sends at run time.

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

## 4. Error and rejection codes

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
| `tool-policy-version-not-found` | error | 404 `NOT_FOUND` | The version does not exist, or it is not live or superseded. |
| `tool-policy-mode-transition-invalid` | error | 409 `FAILED_PRECONDITION` | The first enable must go to `log_only` (reason 61). |
| `tool-policy-history-unsupported` | rejection | - | The set has a history rule (reason 62). Not yet available. |
