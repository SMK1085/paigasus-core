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

[tool_interception]
enabled = true   # Tool policies need tool interception.

[tool_policy]
enabled = true
on_policy_unavailable = "closed"   # "open" or "closed". Required.
source = { kind = "iam" }
```

The gateway refuses to start in these cases.

- `tool_policy.enabled = true` with `tool_interception.enabled = false` or with no
  `[tool_interception]` table.
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

The re-compile job (section 9) adds two more origins, `recompile` and `accept`. It also adds two
statuses, `held` and `discarded`. An older IAM reads a live version with the origin `recompile` or
`accept` as `corrupt`. The effect is the same as for `starter`.

The `down` step of the migration `m0015_tool_policy_held_versions` does two deletions.

1. It deletes every held or discarded row with a version above the live version of its set.
   An older IAM numbers a save `live_version + 1`. A row above the live version holds that number,
   and every save of the set would fail.
2. It deletes every row that has no compiled output. These are the held rows of a failed compile.

So a rollback loses each pending proposal. The job makes them again after the next upgrade.

### Rollback of a compiler change

This case is a rollback between two releases that both have the re-compile job, from compiler
generation G+1 back to G. The same applies to a schema revision. The `down` step of the migration
does not run.

- The job reads the largest generation and schema revision over every stored row of every status:
  live, superseded, held and discarded. The rows that G+1 wrote stay. So every run on a G replica,
  at boot and on request, returns `older_replica` and writes nothing. This continues until a
  release with generation G+1 or higher ships again.
- A set with a live version at G+1 is stale for a G replica. Each catalog write of that
  organization is refused with `tool-policy-recompile-pending`. An admin saves the set to clear
  it: a save compiles again at G and makes a new live version.
- An accept of a held row from G+1 that has an output returns `tool-policy-version-conflict`,
  because the generation of the row is not the running generation. Discard the row, or wait for the next upgrade.

## 8. Not done yet

The Helm chart does not mount the credential file. It does not set `iam.service_credential_file`.
Until a later change adds this, mount the file and set the key by your own means.

## 9. Compiler or schema changes

A new compiler generation or a new tool schema revision makes the live sets stale. A stale set is
a live version that an older compiler or schema made. The re-compile job compiles each stale set
again. IAM changes a live set by itself only when the new output is the same.

### When the job runs

- Each IAM replica starts the job in the background at boot, after the migration. The job does not
  delay readiness. An error does not stop IAM.
- An operator can start the job with `POST /v1/tool-policy-recompile` (gRPC
  `RunToolPolicyRecompile`). The caller needs the action `ManageToolPolicyRecompile` at Root. The
  `platform_admin` role has it.
- One run at a time: the job takes a database advisory lock. A second run gets the status
  "already running" and compiles nothing.
- If a replica finds a stored row with a newer generation or schema revision, it is an older
  replica. It writes nothing and logs a warning. So during a rolling deploy the older replicas do
  not run the job.
- A run on request can last longer than the 30 s request timeout of IAM when many sets are stale.
  The caller then gets a timeout. The job continues in the background and finishes. Read the
  results with `GET /v1/held-tool-policy-versions` and in the IAM log. At a shutdown of the
  replica, the job stops between two sets, and the replica waits for it to release the lock
  before it exits. The replica waits a maximum of 30 s after the shutdown signal, below the 45 s
  `terminationGracePeriodSeconds`. This limit covers all of the shutdown: the servers, the boot
  run and the runs on request. At the limit, the replica logs one warning that names what is
  still open, and exits. This is safe: the transaction of
  the open set rolls back, and Postgres releases the advisory lock when the connection closes.
- After a rolling deploy that changes the compiler generation or the schema revision, do one more
  step. When every replica runs the new release, start the job one more time with
  `POST /v1/tool-policy-recompile`. The reason follows. The new replicas run the job at boot, but
  the old replicas continue to serve until the rollout ends. A save or a catalog write on an old
  replica after that boot run writes a live version at the old generation. No later run picks up
  that version until a replica starts again or an operator starts the job. Until then, each
  catalog write of that organization is refused with `tool-policy-recompile-pending`.

### The run response

`POST /v1/tool-policy-recompile` and gRPC `RunToolPolicyRecompile` wait for the run and return
two fields.

- `status` is the result of the run. The gRPC values are in the enum `RecompileRunStatus`. The
  HTTP JSON uses the short name.

  | gRPC `status` | HTTP `status` | Meaning |
  |---|---|---|
  | `RECOMPILE_RUN_STATUS_RAN` | `ran` | The job ran. `results` has one row for each stale set. |
  | `RECOMPILE_RUN_STATUS_ALREADY_RUNNING` | `already_running` | Another run has the lock. This run compiled nothing. `results` is empty. |
  | `RECOMPILE_RUN_STATUS_OLDER_REPLICA` | `older_replica` | A stored row is newer than this replica. This run wrote nothing. `results` is empty. |

  IAM never sends `RECOMPILE_RUN_STATUS_UNSPECIFIED`.
- `results` has one row for each stale set, in the order of the job: the floor first, then the
  tenants by organization UUID. Each row has these fields.

  | Field | Meaning |
  |---|---|
  | `kind` | `TOOL_POLICY_SET_KIND_FLOOR` or `TOOL_POLICY_SET_KIND_TENANT`. The HTTP JSON uses `floor` or `tenant`. |
  | `org_id` | The organization UUID. It is empty for the floor. |
  | `base_version` | The live version that the job compiled again. |
  | `outcome` | The result of the set, from the enum `RecompileOutcome`, for example `RECOMPILE_OUTCOME_HELD_DIFFERENT`. The HTTP JSON uses the same name. |
  | `version` | The new live version (`IDENTICAL`) or the new held version (`HELD_DIFFERENT`, `HELD_FAILED`). It is 0 for the other outcomes. |

  `RECOMPILE_OUTCOME_ALREADY_PROPOSED` means that a held or discarded row already exists for this
  live version and these compiler values. The job wrote nothing, and the metric does not count it.
  The other outcomes are the `result` values of the metric in the next section.

### The results

The job counts one result for each set in the metric `iam_tool_policy_recompile_results_total`.
The label is `result`.

| `result` | Meaning | Action |
|---|---|---|
| `identical` | The new compile gives the same output. The job made a new live version with `origin = recompile`. | None. |
| `held_different` | The output is different. The job made a held version. The old version stays live. | An admin accepts or discards it. |
| `held_failed` | The compile rejected the set. The job made a held version with no output. | An admin fixes and saves the set, or discards the row. |
| `conflict` | The set changed after the job read it, or the organization is gone. The job wrote nothing. | None. The next run reads the set again. |
| `error` | The compiler was busy or unavailable, or a read or write of this set failed. For example, a compiled value does not decode. The job wrote nothing and logged a warning that names the set. | Make the compiler healthy. Then run the job again. |

A non-zero `held_different` or `held_failed` needs an admin. An `error` result needs a second run
after the compiler is healthy. The job continues with the next set after an `error`.

### Held versions

A held version is a proposal. The gateway never reads it. The live set keeps working.

1. List the held versions. `GET /v1/held-tool-policy-versions` lists every set, the floor included.
   It needs `ManageToolPolicyRecompile` at Root. A tenant admin lists its own with
   `GET /v1/organizations/{id}/held-tool-policy-versions` (gRPC `ListHeldToolPolicyVersions`).
2. Read the `report` of the row. It has the `result` and the difference of the policies,
   conditions and schema names. A failed row has the `rejections` and no `compiled` output.
3. Accept or discard the row. Use
   `POST /v1/organizations/{id}/held-tool-policy-versions/{version}/accept` or `.../discard`. For
   the floor use `POST /v1/tool-policy-floor/held-versions/{version}/accept` or `.../discard`.
   The gRPC methods are `AcceptHeldToolPolicyVersion` and `DiscardHeldToolPolicyVersion`.
   A floor row needs `ManageToolPolicyFloor`. A tenant row needs `ManageToolPolicies`.

An accept makes the stored output the next live version with `origin = accept`. It runs no
compile. These errors are possible:

- `tool-policy-version-conflict` (409): the held row is stale. The live version, the compiler
  generation, the schema revision or the catalog revision is not the same as when the job made the row.
- `tool-policy-held-not-acceptable` (reason 63, 409): the row is a failed compile and has no output.
  Fix the source, save the set, or discard the row.
- `tool-policy-version-not-found`: another request already discarded or replaced the row.

IAM publishes no event for a held version. A save or an accept of any live version of a set
discards its held row.

After a discard, the set stays at the old generation. A catalog write of that organization is
refused with `tool-policy-recompile-pending` until the set is saved again. This error also appears
while the set stays stale. The job does not make the same proposal again for
the same live version and the same compiler values. A new compiler generation, a new schema
revision or a new live version allows a new proposal.

A set that holds a history rule (section 5) is held as failed. Remove the temporal block and save
the set.

### Rolling deploy and mixed replicas

While replicas of the release before this one and of this release run together, an older replica
returns `Internal` (HTTP 500) on every write to a set that has a held row, or a discarded row with
a number above the live version. It also reads a live row with the origin `recompile` or `accept`
as `corrupt`. Both stop when the older replicas are gone. If a newer replica writes a set while
the job of an older replica runs, that job skips the set and logs it.

A rolling deploy between two releases of this feature, from compiler generation G to G+1, has two
more effects.

- During the rollout, an old replica answers `tool-policy-recompile-pending` on a catalog write
  for a set that the new job already moved to G+1. The old replica reads G+1 as stale. This stops
  when the rollout ends.
- A save or a catalog write on an old replica writes a live version at G. The boot runs of the new
  replicas do not see it if they ran before the write. When every replica runs the new release,
  start the job one more time with `POST /v1/tool-policy-recompile` (see "When the job runs").

The first rollout of this release must finish before any compiler or schema change ships. No
change of the Dogwood version and no change of the compiler output may merge without a bump of
`COMPILER_GENERATION`. The golden test enforces this.
