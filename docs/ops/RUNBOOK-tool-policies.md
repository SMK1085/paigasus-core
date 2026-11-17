# RUNBOOK: tool policies and the gateway service identity (SMA-775)

This runbook is for the platform operator. It explains how to give the AI Gateway an IAM identity
that can fetch the compiled tool policies. It also explains what the gateway does when IAM is down.
The tenant side is in [`docs/tool-policies/tenant-guide.md`](../tool-policies/tenant-guide.md).

The gateway has no caller when it fetches policies. A NATS event or a refresh timer starts the
fetch. So the gateway needs its own credential. It sends this credential on the two fetch calls
(`GetCompiledToolPolicies` and `GetCompiledFloorPolicies`) and on no other call. The per-request
`IsAuthorized` call keeps the credential of the caller.

## 1. Provision the gateway identity

Make all calls as a `platform_admin`. Send each HTTP call with `Authorization: Bearer <key>`. Do the
steps in this order. Each step shows the HTTP route and the gRPC method.

1. **Make the operator-owned organization.** `POST /v1/organizations` (`CreateOrganization`).
   The body is `{"slug": "paigasus-platform", "name": "Paigasus Platform"}`. No tenant uses this
   organization. Keep the `prn` of the organization from the response.
2. **Make the service account.** `POST /v1/service-accounts` (`CreateServiceAccount`).
   The body is `{"owner_prn": "<the organization PRN>", "name": "paigasus-gateway"}`. Keep the
   `prn` and the UUID of the account from the response.
3. **Issue an API key.** `POST /v1/service-accounts/{sa}/api-keys` (`IssueApiKey`), where `{sa}` is
   the UUID of the account. Set `scope_prn` to the PRN of the platform organization. Leave
   `scope_actions` and `scope_roles` empty. Check `api_keys.default_expiry_days` in the IAM
   config. If it is set, the key expires, and the gateway fetch fails after that date. Put the
   expiry date in the rotation plan. The key scope does not limit the
   fetch. The Root grant in step 4 does. Store the `token` from the response at once. IAM shows it
   one time only.
4. **Grant the reader role at Root.** `POST /v1/authz/role-grants` (`GrantRole`). The body is
   `{"principal_prn": "<the service account PRN>", "role_key": "tool_policy_reader",
   "scope_prn": "prn:pgs:iam:::root/00000000-0000-0000-0000-000000000000"}`.
   The role holds one action: `ReadCompiledToolPolicies`.
5. **Mount the key and configure the gateway.** Write the token to a file with mode `0400`. Mount
   the file into the gateway. Set `iam.service_credential_file` to the path of the file. Set
   `tool_policy.source = { kind = "iam" }`. See section 2.
6. **Check.** Use the new key.
   - `GET /v1/compiled-tool-policy-floor` must answer 200. An empty floor gives an empty set.
   - `GET /v1/organizations/{id}/tool-policy-set` must answer 403.
   If the second call answers 200, the account holds a role that it must not hold. Find the grant
   with `GET /v1/authz/role-grants` and revoke it.

Do not give this account any other role. Do not archive the platform organization. Archive of the
organization archives the account.

## 2. Gateway configuration

```toml
[iam]
service_credential_file = "/run/secrets/paigasus-gateway-iam-key"

[tool_policy]
enabled = true
on_policy_unavailable = "closed"   # "open" or "closed". Required.
source = { kind = "iam" }
```

The gateway refuses to start in these cases.

- `kind = "iam"` with no `iam.service_credential_file`.
- A `service_credential_file` that is empty or has leading or trailing white space.
- A `[[tool_policy.org]]` entry with `kind = "iam"`. IAM holds the mode and the fail mode of each
  organization, so an entry would look as if it applied. Remove the entries.

`tool_policy.refresh_secs` (default 30) and `tool_policy.max_stale_secs` (default 300) set the
refresh timer and the age limit of a loaded set. The NATS subscription (`[tool_policy.nats]`) makes
the gateway fetch a changed set at once. The event only tells the gateway to fetch again. The
gateway never takes mode values from the event.

## 3. Rotation

1. Issue a new key for the same account (step 3 above).
2. Write the new token to the credential file of every gateway replica. Record the time.
3. On every replica, make sure that the file holds the new token. Compare a hash of the file content
   with a hash of the new token, for example with `sha256sum`. Remove the white space at both ends
   first, because the gateway does the same. Do not print the token. A mounted secret
   can reach each replica at a different time.
4. On every replica, confirm a good fetch with the new token. Read the metrics of each replica
   separately. Do not add them across replicas. Use only a full refresh interval
   (`tool_policy.refresh_secs`) that starts at least one refresh interval after step 3. In that
   interval, all of these must be true:

   - `gateway_tool_policy_cache_loads_total{result="ok"}` increases.
   - `gateway_tool_policy_refresh_errors_total` does not increase.
   - `gateway_tool_policy_cache_loads_total{result="source_error"}` does not increase.
   - The log has no `tool-policy source failed` line. A bad token gives this line, and its
     field `kind` is `io`. The line does not show the IAM status.

   The gateway fetches only the sets that it has loaded. A replica that has served no request since
   it started has no set, so it makes no fetch and the counter stays the same. Send one model
   request through that replica, then look again. A fetch that started before step 3 can end in
   the interval with the old token. The wait of one refresh interval stops this, unless a fetch
   hangs for longer than that wait.
5. Revoke the old key only when step 4 is true on every replica:
   `DELETE /v1/service-accounts/{sa}/api-keys/{id}` (`RevokeApiKey`). If a replica fails step 4,
   keep the old key. Write the old token back to the file of that replica, and find the cause.

No restart is needed. The gateway reads the file again on every fetch. The file must be 4 KiB (4096
bytes) or smaller. The gateway trims white space at both ends. A file that is empty, too large or
not valid text makes the fetch fail with a short message. The message never holds the key. Never log
the key or the file content.

## 4. When IAM is down

The gateway keeps the last loaded sets and settings. With no NATS event, a loaded set stays in use
until `max_stale_secs` after its load, also when a refresh fails. An event makes the gateway load
each set again. If that load fails, the fail mode that applies to the set decides. Under `closed`,
an event makes a loaded set unusable until the next good load. Under `open`, the last good set
serves up to `max_stale_secs` after its load. After that, or when no set was ever loaded, the
gateway uses the fail mode.

- **The floor is unavailable.** The global `tool_policy.on_policy_unavailable` decides. `closed`
  blocks every call. `open` allows the call and reports it. The tenant fail mode never decides
  this case, in every tenant mode.
- **The tenant set is unavailable, and the gateway knows the settings of the organization.** The
  last known mode and `fail_mode` of that organization decide.
- **The gateway never loaded the organization.** The gateway uses mode `enforce` with the global
  `on_policy_unavailable`.

Watch the metric `gateway_tool_policy_cache_loads_total`. Its results are `ok`, `source_error`,
`compile_error` and `timeout`. The label `result="source_error"` has two causes:

- A fetch from IAM failed. The gateway logs `tool-policy source failed` with the field `kind`
  set to `not_found`, `io` or `malformed`. A wrong or revoked key looks the same as an IAM
  outage (`kind="io"`). Check the key first.
- IAM answered, but the gateway refused the tenant set because it holds a history rule
  (section 5). The gateway logs `tool-policy source refused the tenant set` with
  `kind="refused"` and the organization in the field `key`. This occurs on every load of that set
  while the organization is in mode `log_only` or `enforce`. In mode `off`, the gateway does not
  refuse the set.

Before you look at IAM or the key, read the field `kind` of the log line. If it is `refused`, IAM
is healthy. Do the steps in section 5 for that organization.

## 5. History rules

IAM refuses a save that holds a history rule (reason 62 `tool-policy-history-unsupported`) until
the gateway can enforce them (SMA-753). The gateway also refuses a stored set that holds one. The
floor and a tenant set are refused in different ways. The gateway treats a floor with a history
rule as malformed, so the floor is unavailable. Under a global `closed`, such a floor blocks
every call of every tenant.

The gateway does not treat a tenant set with a history rule as malformed. It treats the answer
as refused, and the settings of that organization from IAM in the same answer still decide. In
mode `off`, the gateway does not use the set, so the set blocks no call. In mode `log_only`, the
gateway records the set as unavailable and does not block the call. In mode `enforce`, the tenant
`fail_mode` decides. The global `on_policy_unavailable` does not decide this case. The floor
applies in every mode.

A set saved before this change can hold such a rule. Do these steps.

1. Read the floor (`GET /v1/tool-policy-floor`) and the set of each tenant
   (`GET /v1/organizations/{id}/tool-policy-set`).
2. Save each set that has a temporal block again, with no temporal block.

Until a tenant does this, that tenant cannot write its tool catalog. The write is refused with
`tool-catalog-breaks-policy`, and the message names `tool-policy-history-unsupported`.

## 6. Concurrent first enables

Two admins can enable tool policies for one organization at the same time. One request gets a
version conflict (HTTP 409, gRPC `ABORTED`, `tool-policy-version-conflict`). IAM writes nothing for
that request. The change of the other request is saved. The client reads the settings and sends its
request again if it still needs a change. The retry succeeds. This is not a fault.

## 7. Rollback

Do not roll back IAM to a release before this one after the first enable of any organization.
The first enable saves a version with `origin = starter`. An older IAM reads that origin as
`corrupt`, so `GetToolPolicySet` and every catalog write fail for each enabled organization.

## 8. Not done yet

The Helm chart does not mount the credential file. It does not set `iam.service_credential_file`.
Until a later change adds this, mount the file and set the key by your own means.
