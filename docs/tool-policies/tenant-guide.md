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

## 4. Held versions

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
`tool-policy-recompile-pending` until you accept the held version or save the set again.

An accept of a held version that is no longer current returns `tool-policy-version-conflict`. This
occurs when the live version, the compiler, the schema or the catalog changed after IAM made the
held version. List the held versions again and use the current row, or save the set.

## 5. Error and rejection codes

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
