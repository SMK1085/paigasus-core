# SMA-636 — Organization and project settings in the gateway zone

**Status:** draft, revision 1 (before the adversarial challenge)
**Date:** 2026-09-18
**Issue:** [SMA-636](https://linear.app/smaschek/issue/SMA-636/ts-organization-and-project-settings-screens-in-the-gateway-zone)
**Depends on:** SMA-512 (`gateway-console`, decisions D7 and D8), SMA-511 (`iam-console`),
SMA-630 (tenancy lifecycle in the console) — all Done
**Related:** SMA-446 (gateway M0 IAM auth, the `gateway_user` role, decision D10)

---

## 1. Problem

SMA-512 put an organization switcher in the gateway zone (D7) and a scope route at
`/gateway/orgs/<org>` (D8). No settings screens hang off that route. A scope selection changes
the URL and the breadcrumbs and nothing else (SMA-512 § 13).

This issue delivers the settings screens that the route shape exists for.

### 1.1 What "settings" means here

The issue does not say which settings the screens show. The backend was measured on 2026-09-18:

1. **The gateway holds no per-organization or per-project settings.** It is stateless, has one
   global `GatewayConfig`, one OpenAI upstream, no database and no admin RPC
   (`rs/crates/services/paigasus-gateway/src/config.rs:18-35`). Budgets, rate limits and model
   allowlists are gateway M3/M4 work with no design (`docs/ops/RUNBOOK-observability.md:1454-1462`,
   SMA-446 D6).
2. **IAM holds only `slug`, `name` and `status`** for an organization, a team or a project. The
   mutation surface is Rename, Archive and Restore. The IAM zone already has these screens.
3. **IAM has `ServiceAccountService`** (`CreateServiceAccount`, `ListServiceAccounts`,
   `ArchiveServiceAccount`, `IssueApiKey`, `RevokeApiKey`, `ListApiKeys`). An API key is what a
   client sends to the gateway. **No console screen uses these RPCs today.**

Sven chose item 3 on 2026-09-18: **the gateway settings of an organization or a project are its
service accounts and their API keys for model calls.**

### 1.2 What a working key needs

The gateway flow is `rs/crates/services/paigasus-gateway/src/adapters/http/auth.rs:53-124`:
`IntrospectApiKey` gives the service account's principal PRN and the key's `scope_prn`. Then the
gateway asks IAM whether that principal may `InvokeModel` on `scope_prn`.

So a key can call models only if three things exist:

1. A service account, owned by an organization, a team or a project (`CreateServiceAccount`).
2. A `gateway_user` role grant for that service account, at the key's scope or an ancestor of
   it (`GrantRole`). `gateway_user` holds only `InvokeModel`
   (`rs/crates/libs/paigasus-iam-core/src/authz/roles.rs:202-204`). No admin role holds
   `InvokeModel`.
3. An API key whose `scope_prn` is inside that grant (`IssueApiKey`).

Nothing in the repository documents this setup for an operator. These screens are the first
place that does it.

---

## 2. Scope and decisions

### 2.1 Decisions

| # | Decision | Reason |
|---|---|---|
| D1 | The settings are **service accounts and API keys** (§ 1.1). | The only gateway-relevant per-node data that the backend supports today. Sven, 2026-09-18. |
| D2 | **One flow:** creating a service account also grants it `gateway_user` at its owner node. | A key issued in the gateway zone must work at once. Sven, 2026-09-18. |
| D3 | The create action makes **two RPCs** (`CreateServiceAccount`, then `GrantRole`), not one atomic RPC. A failed grant leaves the account without the role, and the screen offers a repair control (§ 5.2). | IAM has no atomic RPC for both. A new one would make this a cross-stack issue. Sven chose this approach ("A"), 2026-09-18. |
| D4 | Two screens: the **organization** and the **project**. Teams show only as a grouping label. | The issue names organizations and projects. Sven, 2026-09-18. |
| D5 | The project route is **flat**: `/gateway/orgs/<org>/projects/<project>`, with no team segment. | `GetProject` needs only the project PRN. The team is visible on the page, not in the URL. Sven, 2026-09-18. |
| D6 | **A key's scope is always the service account's owner node.** The form has no scope field. | IAM does not check that a key's scope is inside the owner node (§ 1.2). A free scope field could make keys that always get 403. |
| D7 | Expiry choices: **server default, 30, 90 or 365 days.** No custom date. | Enough for a first screen. IAM enforces no maximum, so the console does not invent one. |
| D8 | The "can call models" state comes from **`IsAuthorized(sa, InvokeModel, owner)`**, not from the grant list. | `ListRoleGrants` for another principal needs `ListRoleGrants` at Root (§ 3.2). `IsAuthorized` about another principal needs `ListRoleGrants` only at the resource, which every `*_admin` role holds. It also includes the archived-node `forbid`, so it is the effective state. |
| D9 | **No "Stop model calls" control.** To stop an account, the user archives it or revokes its keys. | `RevokeRole` needs a grant id, and only a platform admin can list another principal's grants (§ 3.2). |
| D10 | Rename and archive of the organization or project **stay in the IAM zone.** The page links there. | The IAM zone already has these controls (SMA-630). A second copy would duplicate the Manage section and its form components (`manage-section.tsx`, `manage-controls.tsx`, `rename-form.tsx`, `lifecycle-button.tsx`). |
| D11 | The shared form helpers that both zones now use **move to existing packages**, not to a new one (§ 4.4). | Removes duplication instead of adding to the ~250 lines SMA-512 § 13 records. Sven, 2026-09-18. |
| D12 | The `myScopes()` per-render cost is **counted, not cached**, in this issue (§ 7.3). | The issue names a cache as the next step only "if it proves too slow". A count is the first measurement. Sven, 2026-09-18. |

### 2.2 Out of scope

1. Team-level service accounts and a team route.
2. A key scope narrower than the owner node, and a custom expiry date.
3. Revoking `gateway_user` ("Stop model calls") — blocked by D9.
4. Restoring an archived service account. IAM has no RPC for it.
5. Rename, archive and restore of the organization or the project in the gateway zone (D10).
6. A per-session cache for `myScopes()` (D12).
7. Any backend change. This issue changes only `ts/`.
8. An operator runbook for the service account setup outside the console.

---

## 3. Backend facts this design relies on

All measured by reading the source on 2026-09-18.

### 3.1 Service accounts and keys

| RPC | Cedar check | Notes |
|---|---|---|
| `CreateServiceAccount(owner_prn, name)` | `CreateServiceAccount` at the owner node | Owner is an organization, a team or a project. Name is trimmed, 1–256 characters. A duplicate name per owner gives `service-account-name-conflict` (`AlreadyExists`). |
| `ListServiceAccounts(owner_prn, limit, offset)` | `ListServiceAccounts` at the owner node | **Direct children only**, not the subtree. Oldest first. Default limit 50, maximum 200. |
| `ArchiveServiceAccount(prn)` | `ArchiveServiceAccount` at the owner node | Disables the principal. Its keys stay `Active` in the database but fail authentication at once (the cache is evicted). No restore RPC exists. |
| `IssueApiKey(sa_prn, scope_prn, expires_at, …)` | `IssueApiKey` at the owner node, **and** `GrantRole` at the scope of **every** grant the account holds (D15 anti-escalation, `application/api_keys.rs:215-220`) | The plaintext token is in the response once. `scope_actions` and `scope_roles` are stored but not enforced in v1. No scope-containment check. No maximum expiry. |
| `RevokeApiKey(id)` | `RevokeApiKey` at the owner node | Idempotent for a key that is already revoked. |
| `ListApiKeys(sa_prn, limit, offset)` | `ListApiKeys` at the owner node | Includes revoked and expired keys. Oldest first. Never carries the secret. |

`org_admin`, `team_admin` and `project_admin` hold all seven service-account actions and
`GrantRole`, `RevokeRole` and `ListRoleGrants` within their node. `*_member` roles hold none of
them.

### 3.2 Role grants

- `GrantRole(principal_prn, role_key, scope_prn)`: the caller needs `GrantRole` at the scope. A
  service-account principal is a valid target.
- **A duplicate grant is an internal error, not a conflict.** The table has
  `UNIQUE (principal_id, role_key, scope_node_prn)`. `map_grant_err`
  (`adapters/persistence/pg_role_grants.rs:96-101`) maps only the role foreign key, so the
  unique violation becomes `AuthzError::Backend` and then `TenancyError::Internal`
  (`application/error.rs:365`).
- **`ListRoleGrants` for another principal needs `ListRoleGrants` at Root**
  (`application/roles.rs:313-318`), so only `platform_admin` can list a service account's grants.
- **`IsAuthorized` for another principal needs `ListRoleGrants` at the queried resource**
  (`application/authorize.rs:64-70`).
- `InvokeModel` is a write action, so the `forbid-archived-writes` policy denies it on an
  archived node, whatever the grants (`authz/roles.rs:311-315`).

### 3.3 The TypeScript side today

- `@paigasus/sdk` exports `ServiceAccountService`, but `IamClients`
  (`ts/packages/paigasus-console-core/src/iam-clients.ts`) has no `serviceAccounts` client.
- `IAM_ACTIONS` (`src/authorize.ts`) has none of the service-account actions, `GrantRole`,
  `ListRoleGrants` or `InvokeModel`.
- The fake IAM (`testing/fake-iam.ts`) routes `tenancy`, `authn`, `authz`, `audit` and
  `serviceInfo`. It does not route `ServiceAccountService`. Its unscripted `authz.isAuthorized`
  answers `allowed: true`. It records every call (`calls`, `callsTo`).
- `ActionState` is `{ ok: true } | { ok: false; error } | null`. It carries no value.

---

## 4. Architecture

### 4.1 Routes

All under `ts/apps/gateway-console/app/(console)/orgs/[org]/`.

| Route | Content |
|---|---|
| `/gateway/orgs/<org>` | Organization header: name, slug, status badge, and a "Manage in IAM" link. The gateway state line, in compact form. **Service accounts** section for accounts that the organization owns. **Projects** list, grouped by team, each row a link to the project page. |
| `/gateway/orgs/<org>/projects/<project>` | Project header: name, slug, team, status badge, "Manage in IAM" link. Breadcrumbs `Overview › <org> › <project>`. **Service accounts** section for accounts that the project owns. |

Keys show inline, in an expandable row per service account. There is no service-account route.

The `[project]` segment is a UUID, like `[org]`. A non-UUID gives `notFound()`.

**Org/project pairing.** The project page compares the project's `org_prn` with the URL's
organization. A mismatch gives `notFound()`. This stops a URL that pairs one organization with
another organization's project.

The "Manage in IAM" link uses `ZoneLink` from `@paigasus/app-shell`. It points to the IAM zone's
page for the same node: `/iam/orgs/<org>` and
`/iam/orgs/<org>/teams/<team>/projects/<project>`.

### 4.2 The page loader

Each page has a `load.ts`, in the iam-console pattern (`app/(console)/orgs/[org]/load.ts`):

1. `GetOrganization` (or `GetProject`) first. `forbidden` → the whole page is 403 through
   `PageError`. `invalid-input` or `not-found` → 404.
2. Then in parallel:
   - `ListServiceAccounts(owner, limit 50)`.
   - `mayI()` for `CreateServiceAccount`, `IssueApiKey`, `RevokeApiKey`,
     `ArchiveServiceAccount` and `GrantRole` at the owner node.
   - Organization page only: `ListTeams(org, limit 200)`, then `ListProjects(team, limit 200)`
     for each team. A full page gets the same "more exist" note as below.
3. For each listed service account, in parallel: `ListApiKeys(sa, limit 200)` and
   `IsAuthorized(sa, InvokeModel, owner)`.

The loader returns a view model. It does not return raw proto messages to a client component.

**Section-level denial.** If `ListServiceAccounts` returns `forbidden` (for example for an
`org_member`), only the Service accounts section shows "You cannot view service accounts here."
The page stays 200. Any other failure of that call shows the section's error state with the
correlation reference.

**The "can call models" state is not cosmetic.** `mayI()` fails open. The model-call state must
not: a failed or `forbidden` `IsAuthorized` shows "Unknown", never "Yes".

**Page limits.** The section shows at most 50 accounts. If IAM returns exactly 50, the section
shows "More service accounts exist. The console shows the first 50." Keys: at most 200 per
account, with the same kind of note.

### 4.3 Where the code lives

| Home | What |
|---|---|
| `@paigasus/console-core` | `serviceAccounts` client in `IamClients` / `createIamClients`. The new action names in `IAM_ACTIONS`. The pure form helpers from iam-console's `lib/form.ts` (§ 4.4). The fake IAM: routing for `ServiceAccountService`, and scriptable `IsAuthorized` answers for another principal. |
| `@paigasus/ui` | The two button class constants from iam-console's `app/_components/button-class.ts`. |
| `ts/apps/gateway-console` | The two pages, a `load.ts`, `commands.ts` and `actions.ts` per level, and the client components (§ 4.5). |
| `ts/apps/iam-console` | Imports the moved helpers from their new homes. No behaviour change. |

### 4.4 The extraction (D11)

Moves, with their existing tests:

- From `ts/apps/iam-console/lib/form.ts` to `@paigasus/console-core`: `formFields`,
  `invalidFormInput`, `toActionResult`, the `ActionResult` type, and `nameField`
  (with `NAME_MAX_CODE_POINTS`). The rename and lifecycle helpers (`renameForm`,
  `renameChange`, `slugField`, `currentField`, `refreshesAfterLifecycleAction`) stay in
  iam-console, because only iam-console uses them.
- From `ts/apps/iam-console/app/_components/form-action.ts` to `@paigasus/console-core`: the
  `FormAction` type.
- From `ts/apps/iam-console/app/_components/button-class.ts` to `@paigasus/ui`:
  `PRIMARY_BUTTON_CLASS`, `SECONDARY_BUTTON_CLASS`.

`FormError` does **not** move. It depends on each app's `error-copy.ts` and
`error-reference.tsx`, which are app-local. Moving it would drag those along. gateway-console
gets its own copy of `FormError`, and § 9 records the duplication.

`@paigasus/console-core` is server-only. A client component imports the `FormAction` and
`ActionResult` types with `import type`, which is erased at build time.

### 4.5 Client components (gateway-console)

- `ServiceAccountSection` — the section frame, the result region, the create form, and the list.
- `CreateServiceAccountForm` — one `name` field.
- `ServiceAccountRow` — name, created date, "Can call models: Yes / No / Unknown", the
  "Allow model calls" control, the archive control, and the expandable keys list.
- `IssueKeyForm` — the expiry choice.
- `TokenPanel` — shows a new token once (§ 5.3).
- `RevokeKeyButton`, `ArchiveServiceAccountButton` — two-step confirm, in the pattern of
  iam-console's `ArchiveButton`.

---

## 5. Flows

Every mutation follows the iam-console pattern: `iamClientsForAction()` first, then a `zod`
shape check, then a pure command in `commands.ts`, then `revalidatePath`. IAM is the only
authority. `mayI()` only hides controls.

### 5.1 Create a service account

1. `CreateServiceAccount(owner_prn, name)`.
2. On success, `GrantRole(sa_prn, "gateway_user", owner_prn)`.
3. Results:
   - Both succeed: "Service account created. It can call models."
   - Step 1 fails: the error shows on the form. `service-account-name-conflict` shows "A service
     account with this name already exists here."
   - Step 2 fails: "Service account created, but it cannot call models yet." and the error. The
     action logs `gateway.sa.grant_failed` with the correlation id. The row shows "Allow model
     calls".

The create control shows only if `mayI(CreateServiceAccount)` is true. `mayI(GrantRole)` false
does not hide it; the result then is the step-2 message.

### 5.2 Allow model calls

`GrantRole(sa_prn, "gateway_user", owner_prn)`. The control shows only when all are true:

- the model-call state is "No" (not "Unknown");
- the owner node's lifecycle is `active`;
- `mayI(GrantRole)` is true.

These conditions make a duplicate grant rare. A duplicate grant still gives an internal error
(§ 3.2), for example when two users click at the same time. The page then revalidates and the
row shows "Yes".

### 5.3 Issue an API key

`IssueApiKey(sa_prn, scope_prn = owner_prn, expires_at)`. `expires_at` is empty for "server
default", else now plus 30, 90 or 365 days. `scope_actions` and `scope_roles` stay empty.

The action returns its own state type, `IssueKeyState`:
`{ ok: true; token: string; prefix: string } | { ok: false; error } | null`. The shared
`ActionState` does not change.

Token rules:

- The token is in the action result only. It is never logged, never in a URL or a cookie, and
  never in the page data.
- `TokenPanel` shows the token, a copy button, and "You cannot see this token again." It sits in
  the section's result region, **outside** the row that revalidation re-renders. A revalidation
  that changes the tree can unmount the control that holds an action result (React 19), so the
  panel must not live in the key list.
- The panel closes on "Done", on the next submission in the section, or on navigation.

A `forbidden` result here can come from the D15 anti-escalation check. The error text says:
"You need permission to grant every role this service account holds."

### 5.4 Revoke an API key

Two-step confirm, then `RevokeApiKey(id)`.

### 5.5 Archive a service account

Two-step confirm. The confirm text says: "All keys of this account stop working at once. You
cannot undo this in the console." Then `ArchiveServiceAccount(prn)`. An archived account stays in
the list with an "Archived" badge and no controls.

### 5.6 The keys list

From `ListApiKeys`, newest first in the console. Columns: prefix, status, created, expires, last
used. Status is `Revoked` if IAM says revoked, else `Expired` if `expires_at` is in the past,
else `Active`. Revoked and expired keys stay visible.

### 5.7 An archived owner node

If the organization or the project has `effective_status` archived, the Service accounts
section is read-only and shows the SMA-630 archived note. IAM would refuse every write anyway.

### 5.8 Errors

`callIam` results map to inline form errors through gateway-console's `FormError`. After a
`forbidden` or `conflict` result the page revalidates, because these often mean the page is
stale. A `relogin` result shows the sign-in link.

---

## 6. Security

1. The token (§ 5.3) is the only secret. It never leaves the action result.
2. The console never sends a scope other than the owner node (D6).
3. Every mutation re-checks in IAM. `mayI()` failing open can show a control, never permit an
   action.
4. The org/project pairing check (§ 4.1) stops a mixed URL from showing one organization's
   project under another organization's name.
5. The console asks `IsAuthorized` about another principal. IAM refuses that unless the caller
   administers roles at the resource, so it leaks nothing a caller could not already see.

---

## 7. Testing

### 7.1 Unit (vitest)

- `@paigasus/console-core`: the moved helpers keep their tests. `IAM_ACTIONS` contains the new
  names. The fake IAM routes `ServiceAccountService` and records its calls.
- `gateway-console` commands, against the fake IAM: create with both steps passing; create with
  a failed grant; create with a failed create; issue with each expiry choice, and
  `scope_prn` always the owner; revoke; archive; allow model calls.
- The loader: the section-level 403; the org/project mismatch → 404; an archived owner →
  read-only; the key status mapping; the page-limit notes; `IsAuthorized` failure → "Unknown".
- The token and the logger: the issue action runs with a spy logger, and the test asserts that
  no log line contains the token.

### 7.2 End-to-end (Playwright, the existing Docker-backed gateway-console tier)

1. Create an account → the row shows "Can call models: Yes" → issue a key → the token shows once
   → reload → the token is gone and the key is `Active` → revoke → `Revoked` → archive the
   account → "Archived".
2. A user who may view but not manage sees no mutation controls.
3. A user who may not view service accounts sees the section-level denial, and the page is 200.

### 7.3 The `myScopes()` count (D12)

An e2e row renders `/gateway/orgs/<org>` once for a fixed fixture and reads the fake IAM's
`callsTo()`. It asserts the exact count per RPC. A lost `cache()` memoization or a new
per-render call reds it.

The spec records the formula after the measurement: one `Introspect`, one `ListRoleGrants`, N
tenancy reads for the scopes, plus the page's own calls (§ 4.2).

**Limit.** This counts calls against a fake. It does not measure latency against a real IAM. The
per-session cache stays a follow-up that needs a latency number first.

---

## 8. Follow-ups

1. IAM: a scoped `ListRoleGrants`, so a node admin can list and revoke a service account's
   grants. This unblocks "Stop model calls" (D9).
2. IAM: map the `uq_role_grant_principal_role_scope` violation to `already-exists`, not to
   `internal` (§ 3.2).
3. A latency measurement of `myScopes()` against a real IAM, then the per-session cache if it is
   too slow.
4. An operator runbook for the service account, `gateway_user` and key setup.

---

## 9. Recorded limits

- `FormError` is duplicated between the two zones (§ 4.4). Nothing gates a divergence.
- The two create RPCs are not atomic (D3). A failed grant leaves an account that cannot call
  models until someone uses the repair control.
- An account that got `gateway_user` outside the console at another scope shows "Yes" if that
  grant covers its owner node. The console cannot show where the grant comes from (D8).
- The org page makes one `ListProjects` call per team. An organization with many teams makes
  many calls per render. This is not measured.
