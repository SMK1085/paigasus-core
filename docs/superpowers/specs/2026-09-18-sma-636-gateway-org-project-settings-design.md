# SMA-636 — Organization and project settings in the gateway zone

**Status:** draft, revision 2 (after the adversarial challenge)
**Date:** 2026-09-18
**Issue:** [SMA-636](https://linear.app/smaschek/issue/SMA-636/ts-organization-and-project-settings-screens-in-the-gateway-zone)
**Depends on:** SMA-512 (`gateway-console`, decisions D7 and D8), SMA-511 (`iam-console`),
SMA-630 (tenancy lifecycle in the console) — all Done
**Related:** SMA-446 (gateway M0 IAM auth, the `gateway_user` role, decision D10), SMA-505 (IAM
capability toggles)

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
3. **IAM has `ServiceAccountService`** (`CreateServiceAccount`, `GetServiceAccount`,
   `ListServiceAccounts`, `ArchiveServiceAccount`, `IssueApiKey`, `RevokeApiKey`, `ListApiKeys`).
   An API key is what a client sends to the gateway. **No console screen uses these RPCs today.**

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
| D2 | **One flow:** creating a service account also grants it `gateway_user` at its owner node, when IAM offers role administration (D13). | A key issued in the gateway zone must work at once. Sven, 2026-09-18. |
| D3 | The create action makes **two RPCs** (`CreateServiceAccount`, then `GrantRole`), not one atomic RPC. A failed grant leaves the account without the role, and the screen offers a repair control (§ 5.3). | IAM has no atomic RPC for both. A new one would make this a cross-stack issue. Sven chose this approach ("A"), 2026-09-18. |
| D4 | Two screens: the **organization** and the **project**. Teams show only as a grouping label. | The issue names organizations and projects. Sven, 2026-09-18. |
| D5 | The project route is **flat**: `/gateway/orgs/<org>/projects/<project>`, with no team segment. | `GetProject` needs only the project PRN. The team is visible on the page, not in the URL. Sven, 2026-09-18. |
| D6 | **A key's scope and a grant's scope are always the service account's owner node, read from IAM on the server.** No form carries a scope. | IAM does not check that a key's scope is inside the owner node (§ 3.1). A Server Action's inputs are under client control (§ 5.1). |
| D7 | Expiry choices: **IAM default, 30, 90 or 365 days.** The form sends a choice, and the server computes the date. No custom date. | Enough for a first screen. IAM enforces no maximum, so the console does not invent one. |
| D8 | The "can call models" state of an **active** account comes from **`IsAuthorized(sa, InvokeModel, owner)`**, through a function that fails closed. An account that is not active shows "No (account archived)" with no call. | `ListRoleGrants` for another principal needs Root (§ 3.2). `IsAuthorized` about another principal needs `ListRoleGrants` only at the resource, which every `*_admin` role holds. No Cedar policy reads the principal's status, so IAM answers `allowed` for an archived account (§ 3.2). |
| D9 | **No "Stop model calls" control.** To stop an account, the user archives it or revokes its keys. | `RevokeRole` needs a grant id, and only a platform admin can list another principal's grants (§ 3.2). |
| D10 | Rename and archive of the organization or project **stay in the IAM zone.** The page links there when the zone map has an IAM zone. | The IAM zone already has these controls (SMA-630). A second copy would duplicate the Manage section and its form components (`manage-section.tsx`, `manage-controls.tsx`, `rename-form.tsx`, `lifecycle-button.tsx`). |
| D11 | The shared **pure** form helpers move to `@paigasus/console-core` and the button classes to `@paigasus/ui` (§ 4.5). React components stay per app and are recorded as duplicated. | Removes duplication where it is cheap. The React pieces depend on app-local error copy. Sven, 2026-09-18. |
| D12 | The `myScopes()` per-render cost is **counted per request, not cached**, in this issue (§ 7.3). | The issue names a cache as the next step only "if it proves too slow". A count is the first measurement. Sven, 2026-09-18. |
| D13 | The screens read two IAM capabilities from discovery: **`iam.apikeys`** gates every key control and every `ListApiKeys` call; **`iam.authz.cedar`** gates the grant step and "Allow model calls". The rule is the rule of `cedarCapabilityOf`: the capability counts only when IAM is `available` and lists it. A degraded IAM counts as "absent". | IAM answers `UNIMPLEMENTED` for these RPCs when the capability is off (§ 3.3). Calling them would fail every time and log an error each time. |
| D14 | **Keys and the model-call state load only for one selected account** (`?sa=<uuid>`), not for every listed account. | Bounds the per-render fan-out (§ 4.3). A per-row load would make up to 100 extra calls per render. |
| D15 | Lists use the iam-console pager (`lib/paging.ts` pattern: an offset search parameter, a request of limit+1 to detect more). The console keeps IAM's order (oldest first). | IAM sorts oldest first. Reversing one page does not show the newest item. The selected-account panel (D14) shows a new account at once, whatever page it is on. |
| D16 | The overview page lists the user's **team and project scopes** from `myScopes()`, as links to the project pages. | A `project_admin` cannot read the organization, so the org page is a 403 for them. Without this list they reach a project page only by typing its URL. The data is already loaded for the switcher, so it costs no call. |

### 2.2 Out of scope

1. Team-level service accounts and a team route. An account that a team owns is not visible in
   this zone (§ 9).
2. A key scope narrower than the owner node, and a custom expiry date.
3. Revoking `gateway_user` ("Stop model calls") — blocked by D9.
4. Restoring an archived service account. IAM has no RPC for it.
5. Rename, archive and restore of the organization or the project in the gateway zone (D10).
6. A per-session cache for `myScopes()` (D12).
7. Any backend change. This issue changes only `ts/`.
8. An operator runbook for the service account setup outside the console.

---

## 3. Backend facts this design relies on

All measured by reading the source on 2026-09-18, and checked again by the adversarial review.

### 3.1 Service accounts and keys

| RPC | Cedar check | Notes |
|---|---|---|
| `CreateServiceAccount(owner_prn, name)` | `CreateServiceAccount` at the owner node | Owner is an organization, a team or a project. Name is trimmed, 1–256 characters. A duplicate name per owner gives `service-account-name-conflict` (`AlreadyExists`). |
| `GetServiceAccount(prn)` | `GetServiceAccount` at the owner node | Returns `owner_prn` and `status` (the principal status string). |
| `ListServiceAccounts(owner_prn, limit, offset)` | `ListServiceAccounts` at the owner node | **Direct children only**, not the subtree. Oldest first. Default limit 50, maximum 200. Archived accounts stay in the list. |
| `ArchiveServiceAccount(prn)` | `ArchiveServiceAccount` at the owner node | Disables the principal and evicts its keys from IAM's API-key cache. Its key rows stay `ACTIVE`. Authentication refuses them because it re-checks the principal status. No restore RPC exists. |
| `IssueApiKey(sa_prn, scope_prn, expires_at, …)` | `IssueApiKey` at the owner node, **and** `GrantRole` at the scope of **every** grant the account holds (D15 anti-escalation, `application/api_keys.rs:215-220`) | The plaintext token is in the response once. `scope_actions` and `scope_roles` are stored but not enforced in v1. No scope-containment check. No maximum expiry. When `default_expiry_days` is not set, an empty `expires_at` gives a key that never expires. |
| `RevokeApiKey(id)` | `RevokeApiKey` at the owner node | Idempotent for a key that is already revoked. |
| `ListApiKeys(sa_prn, limit, offset)` | `ListApiKeys` at the owner node | Includes revoked and expired keys. Oldest first. Never carries the secret. |

`org_admin`, `team_admin` and `project_admin` hold all seven service-account actions and
`GrantRole`, `RevokeRole` and `ListRoleGrants` within their node. `*_member` roles hold none of
them.

For the creator, D15 is not an obstacle: the only grant a console-made account holds is
`gateway_user` at the owner node, where every `*_admin` role holds `GrantRole`.

### 3.2 Role grants and `IsAuthorized`

- `GrantRole(principal_prn, role_key, scope_prn)`: the caller needs `GrantRole` at the scope. A
  service-account principal is a valid target. `GrantRole` is a write action, so it is refused on
  an archived scope.
- **A duplicate grant is an internal error, not a conflict.** The table has
  `UNIQUE (principal_id, role_key, scope_node_prn)`. `map_grant_err`
  (`adapters/persistence/pg_role_grants.rs:96-101`) maps only the role foreign key, so the
  unique violation becomes `AuthzError::Backend` and then `TenancyError::Internal`
  (`application/error.rs:365`). It logs at error level and counts in IAM's gRPC error rate.
- **`ListRoleGrants` for another principal needs `ListRoleGrants` at Root**
  (`application/roles.rs:313-318`), so only `platform_admin` can list a service account's grants.
- **`IsAuthorized` for another principal needs `ListRoleGrants` at the queried resource**
  (`application/authorize.rs:64-80`). This works for an `org_admin` at a project, because Cedar's
  `resource in ?resource` finds the organization grant. `ListRoleGrants` is a read action, so it
  also works on an archived node. Each such call runs two Cedar decisions.
- `InvokeModel` is a write action, so the `forbid-archived-writes` policy denies it on an
  archived node, whatever the grants (`authz/roles.rs:311-315`).
- **No Cedar policy reads the principal's status** (`application/api_keys.rs:24-38`). So
  `IsAuthorized` answers `allowed` for an archived account that still holds `gateway_user`.

### 3.3 IAM capability gates (SMA-505)

- `IssueApiKey`, `RevokeApiKey` and `ListApiKeys` answer `UNIMPLEMENTED` when
  `api_keys.management_enabled` is false
  (`adapters/grpc/service_accounts.rs:90-103`, capability `iam.apikeys`). The four
  service-account lifecycle RPCs are not gated.
- `GrantRole` (and all role and policy administration) answers `UNIMPLEMENTED` when
  `authz.admin_enabled` is false (`adapters/grpc/authz.rs:81-91`, capability `iam.authz.cedar`).
  `IsAuthorized` is not gated.

### 3.4 The TypeScript side today

- `@paigasus/sdk` exports `ServiceAccountService`, but `IamClients`
  (`ts/packages/paigasus-console-core/src/iam-clients.ts`) has no `serviceAccounts` client.
- `IAM_ACTIONS` (`src/authorize.ts`) has none of the service-account actions or `GrantRole`.
  `mayI()` fails open, asks about the current user only, and memoizes by action and resource.
- The fake IAM (`testing/fake-iam.ts`) routes `tenancy`, `authn`, `authz`, `audit` and
  `serviceInfo` (`SERVICES`, lines 63-69). It does not route `ServiceAccountService`. Its
  unscripted `authz.isAuthorized` answers `allowed: true`. A handler receives the whole request,
  `principalPrn` included. The fake records every call with its correlation id.
- `ActionState` is `{ ok: true } | { ok: false; error } | null`. It carries no value.
- `ZoneLink` returns `null` when the zone map has no entry for its zone
  (`ts/packages/paigasus-app-shell/src/zone/zone-link.tsx:54`), and has no `prefetch` prop.
- After hydration, the Next router prefetches every visible same-zone link. Each prefetch is a
  render (`ts/apps/gateway-console/tests/e2e/two-zone-runtime.spec.ts:10-24`).

---

## 4. Architecture

### 4.1 Routes

All under `ts/apps/gateway-console/app/(console)/`.

| Route | Content |
|---|---|
| `/gateway/overview` (changed) | The existing zone overview, plus a **Your projects** list: the team and project entries of `myScopes()`, each project a link to its project page (D16). |
| `/gateway/orgs/<org>` | Organization header: name, slug, status. A "Manage in IAM" link (§ 4.4). The gateway state line, in compact form. **Service accounts** section for accounts that the organization owns. **Projects** list, grouped by team. |
| `/gateway/orgs/<org>/projects/<project>` | Project header: name, slug, team name, status. Breadcrumbs `Overview › <org> › <project>`. A "Manage in IAM" link. **Service accounts** section for accounts that the project owns. |

The `[org]` and `[project]` segments are UUIDs. A non-UUID gives `notFound()`.

Search parameters on both pages: `sa=<uuid>` selects one account (D14). `saOffset` and
`keyOffset` page the two lists (D15). An invalid value is ignored, in the `parseOffset` pattern.

Same-zone links to project pages and pager links render with `prefetch={false}`, so a long list
does not start one render per visible link.

**A mixed URL.** The project PRN is built from the URL's organization and project. IAM checks
authorization first and the PRN second (`adapters/grpc/tenancy.rs:499-505`). So a URL that pairs
one organization with another organization's project gives 403 or 404, depending on what the user
may read. It never shows the project. The console does no pairing check of its own.

### 4.2 The page loaders

Each page has a `load.ts`, in the iam-console pattern. All IAM calls go through `callIam`.

**Organization page:**

1. `GetOrganization`. `forbidden` → the whole page is 403 through `PageError`. `invalid-input` or
   `not-found` → 404.
2. Then in parallel:
   - the service-accounts section (below);
   - `ListTeams(org, limit 51)`, then `ListProjects(team, limit 51)` for each listed team, with at
     most **8 calls in flight**. At most 50 teams, and at most 50 projects per team. A full page
     shows "More exist. See the IAM zone." A failure shows the Projects section's error state with
     the correlation reference. The page stays 200.

**Project page:**

1. `GetProject`. Same failure mapping as `GetOrganization`.
2. Then in parallel: `GetTeam(project.team_prn)` and `GetOrganization(project.org_prn)` for the
   header and the breadcrumbs, and the service-accounts section. If `GetTeam` or
   `GetOrganization` fails, the header shows the UUID instead of the name. The page stays 200.

**The service-accounts section (both pages):**

1. `ListServiceAccounts(owner, limit 51, offset saOffset)`.
2. `mayI()` for `CreateServiceAccount`, `IssueApiKey`, `RevokeApiKey`, `ArchiveServiceAccount` and
   `GrantRole` at the owner node.
3. The IAM discovery state, for `iam.apikeys` and `iam.authz.cedar` (D13). Discovery is already
   memoized per request.
4. Only if `sa` is set: `GetServiceAccount(sa)`. Its `owner_prn` must equal this page's owner,
   else the panel shows "This service account belongs to another scope." Then, for an active
   account: `modelCallState` (§ 4.3), and `ListApiKeys(sa, limit 51, offset keyOffset)` if
   `iam.apikeys` is present.

`ListServiceAccounts` `forbidden` → only this section shows "You cannot view service accounts
here." The page stays 200. Any other failure shows the section's error state with the correlation
reference.

The loader returns a view model. It does not return raw proto messages to a client component.

### 4.3 The model-call state

`modelCallState(authz, saPrn, ownerPrn, status)` in gateway-console returns `yes`, `no`,
`archived` or `unknown`:

- `status` is not `active` → `archived` (the string `disabled` or any other value). No call.
- Else `IsAuthorized(saPrn, InvokeModel, ownerPrn)`. `allowed` → `yes`. Denied → `no`. Any
  failure (`forbidden` included) → `unknown`.

It is not `mayI()`. `mayI()` fails open and asks about the current user.

### 4.4 The "Manage in IAM" link

The href comes from the zone map (`zones['iam']`), in the pattern of
`ts/apps/gateway-console/lib/nav.ts:26-29`. Targets: `<iam>/orgs/<org>` and
`<iam>/orgs/<org>/teams/<team>/projects/<project>`. With no IAM zone in the map, the link is
absent. That is the case in the single-zone e2e tier.

### 4.5 Where the code lives

| Home | What |
|---|---|
| `@paigasus/console-core` | `serviceAccounts` client in `IamClients` / `createIamClients`. `CreateServiceAccount`, `IssueApiKey`, `RevokeApiKey`, `ArchiveServiceAccount` and `GrantRole` in `IAM_ACTIONS`. **Not** `InvokeModel` and not `ListRoleGrants` (§ 4.3). The pure form helpers from iam-console's `lib/form.ts`: `formFields`, `invalidFormInput`, `toActionResult`, `ActionResult`, `nameField` with `NAME_MAX_CODE_POINTS`, and `prnField`. The `FormAction` type from iam-console's `form-action.ts`. `zod` as a new dependency. `gateway.sa.grant_failed` in the closed `AppEventName` union (`src/logger.ts:14`). The fake IAM: `ServiceAccountService` in `SERVICES`. |
| `@paigasus/ui` | `PRIMARY_BUTTON_CLASS`, `SECONDARY_BUTTON_CLASS`. |
| `ts/apps/gateway-console` | The two pages and the overview change. A `load.ts`, `commands.ts` and `actions.ts` per page. `modelCallState`. The client components (§ 4.6). Copies of `FormError`, `node-status.ts`, `section-error.tsx` and the two-step confirm button, recorded in § 9. |
| `ts/apps/iam-console` | Imports the moved helpers from their new homes. The rename and lifecycle helpers (`renameForm`, `renameChange`, `slugField`, `currentField`, `refreshesAfterLifecycleAction`) stay. No product behaviour change. Its tests change (§ 7.4). |

`@paigasus/console-core` is server-only. A client component imports `FormAction` and
`ActionResult` with `import type`, which is erased at build time.

### 4.6 Client components (gateway-console)

- `ServiceAccountSection` — the section frame, **one result region for every action in the
  section**, keyed by the owner PRN (the ManageControls pattern, SMA-630), the create form, the
  list and the pager.
- `CreateServiceAccountForm` — one `name` field.
- `ServiceAccountRow` — name, created date, status badge, a "Select" link (`?sa=<uuid>`). Keyed by
  the account PRN.
- `ServiceAccountPanel` — the selected account: "Can call models", "Allow model calls", "Archive",
  the issue-key form, the keys list and its pager.
- `IssueKeyForm` — the expiry choice.
- `TokenPanel` — shows a new token (§ 5.4).
- `RevokeKeyButton`, `ArchiveServiceAccountButton` — two-step confirm.

A row or panel control that succeeds often unmounts itself (a revoked key loses its button). So
every result, success or error, goes to the section's result region, and a control never holds
the only copy of its result.

---

## 5. Flows

### 5.1 Common rules for every action

- `iamClientsForAction()` first, then a `zod` shape check, then a pure command in `commands.ts`.
  IAM is the only authority. `mayI()` only hides controls.
- **Accepted inputs.** Each action accepts only the fields named below. Anything else in the form
  is ignored. The owner PRN for a grant and for a key scope always comes from
  `GetServiceAccount`, on the server (D6).
- Error copy goes through gateway-console's `FormError`. A `relogin` result shows the sign-in
  link.

| Action | Inputs |
|---|---|
| Create service account | `ownerPrn`, `name` |
| Allow model calls | `saPrn` |
| Issue API key | `saPrn`, `expiry` (`default` \| `30` \| `90` \| `365`) |
| Revoke API key | `saPrn`, `keyId` |
| Archive service account | `saPrn` |

`ownerPrn` in create is under client control. That is safe: IAM checks `CreateServiceAccount` at
that node, and the grant then uses the `owner_prn` of the created account, from IAM's response.

### 5.2 Create a service account

1. `CreateServiceAccount(ownerPrn, name)`.
2. If step 1 succeeds and `iam.authz.cedar` is present: `GrantRole(sa.prn, "gateway_user",
   sa.owner_prn)`.

The action returns `CreateState`:

```ts
type CreateState =
  | { kind: 'created'; saPrn: string; granted: boolean } // granted=false: iam.authz.cedar absent
  | { kind: 'partial'; saPrn: string; error: PaigasusError } // step 1 ok, step 2 failed
  | { kind: 'failed'; error: PaigasusError } // step 1 failed
  | null;
```

| Result | Message | Revalidate |
|---|---|---|
| `created`, granted | "Service account created. It can call models." and a link that selects it. | yes |
| `created`, not granted | "Service account created. This IAM does not offer role administration, so it cannot call models from here." | yes |
| `partial` | "Service account created, but it cannot call models yet." and the error. Logs `gateway.sa.grant_failed`. | yes |
| `failed`, `conflict` (`service-account-name-conflict`) | "A service account with this name already exists here." | yes |
| `failed`, `forbidden` | the error | yes |
| `failed`, `degraded` or `generic` | the error | **yes** — step 1 may have committed before the response was lost |
| `failed`, `invalid-input` | the error | no |

The create control shows only if `mayI(CreateServiceAccount)` is true. `mayI(GrantRole)` false
does not hide it; the result then is `partial`.

### 5.3 Allow model calls

`GetServiceAccount(saPrn)`, then `GrantRole(saPrn, "gateway_user", owner_prn)`. The control shows
only when all are true: the model-call state is `no`; the owner node's lifecycle is `active`;
`iam.authz.cedar` is present; `mayI(GrantRole)` is true.

The page revalidates after **every** result. A `generic` result can be a duplicate grant
(§ 3.2), for example when two users click at the same time. Its message is: "Model calls may
already be allowed. The page was reloaded."

### 5.4 Issue an API key

`GetServiceAccount(saPrn)`, then `IssueApiKey(saPrn, scope_prn = owner_prn, expires_at)`.
`expires_at` is empty for `default`, else the command's injected clock plus 30, 90 or 365 days.
`scope_actions` and `scope_roles` stay empty. The option label for `default` is "IAM default (the
key may not expire)".

The action returns `IssueKeyState`:
`{ ok: true; token: string; prefix: string } | { ok: false; error } | null`.

A `forbidden` result says: "You need permission to issue keys here and to grant every role this
account holds." (Both a plain denial and the D15 check give `forbidden`.)

**Token rules.** § 6.1 depends on every one of them.

1. The token is in the action's response only. The server never logs it, never puts it in a
   URL, a cookie or the page data, and never revalidates it into a render.
2. The client calls the issue action **directly** in a transition, with `null` as the previous
   state. It does not use `useActionState` for this action, because `useActionState` sends the
   previous state back to the server in the next request body.
3. The client moves the token at once into a `useState` in `TokenPanel`, which lives in the
   section's result region, outside every row and panel that revalidation re-renders.
4. No generation rule may discard an issue result. A token of a key that IAM minted is always
   shown, even when another action started later.
5. While an issue is pending, every other submit in the section is disabled.
6. The panel shows the token, a copy button, and "You cannot see this token again." It closes on
   "Done" and on `pagehide`. It does **not** close on another submission, so key rotation (issue,
   copy, revoke the old key) works.
7. The issue form renders its submit control only after hydration, in the pattern of
   `ArchiveButton`'s first state. With JavaScript off there is no issue control, so no
   document-level POST can put the token in an HTML response, and a reload cannot mint a second
   key.

### 5.5 Revoke an API key

Two-step confirm, then `RevokeApiKey(keyId)`. IAM authorizes the revoke at the owner node of the
key's own account, so `saPrn` is used only to keep the panel selected. Revalidates after every
result.

### 5.6 Archive a service account

Two-step confirm. The confirm text says: "All keys of this account stop working. You cannot undo
this in the console." Then `ArchiveServiceAccount(saPrn)`. Revalidates after every result. An
archived account stays in the list with an "Archived" badge and no controls.

### 5.7 The keys list

From `ListApiKeys`, in IAM's order, with the pager. Columns: prefix, status, created, expires,
last used. Status:

- the account is not active → "Inactive (account archived)";
- else IAM says revoked → "Revoked";
- else `expires_at` is in the past → "Expired";
- else "Active".

A key whose `scope_prn` is not the owner PRN (made outside the console) shows the note "Scope:
<prn>. The gateway checks model calls against this scope."

### 5.8 An owner node that is not active

The section uses `lifecycleView` from a copy of iam-console's `node-status.ts`.

- `archived`: the section is read-only and says "This <organization|project> is archived. IAM
  refuses changes to its service accounts and keys."
- `archived-parent`: the SMA-630 parent-archived note, read-only.
- `unknown`: read-only, with no note.

---

## 6. Security

1. The token (§ 5.4) is the only secret. It exists in one response body and in one client
   `useState`. § 7.2 row 4 checks this.
2. The console never sends a scope that it did not read from IAM (D6, § 5.1).
3. Every mutation re-checks in IAM. `mayI()` failing open can show a control, never permit an
   action. `modelCallState` fails closed.
4. A mixed organization/project URL never shows the project (§ 4.1).
5. The console asks `IsAuthorized` about another principal. IAM refuses that unless the caller
   administers roles at the resource, so it leaks nothing a caller could not already see.

---

## 7. Testing

### 7.1 Unit and integration (vitest)

- `@paigasus/console-core`: the moved helpers keep their tests. `IAM_ACTIONS` contains the five
  new names and not `InvokeModel`. The fake IAM routes `ServiceAccountService` and records its
  calls.
- `gateway-console` commands, against the fake IAM:
  - create: both steps pass (assert the exact `grantRole` arguments); the grant fails →
    `partial`; the create fails; `iam.authz.cedar` absent → no `grantRole` call.
  - issue: each expiry choice with a fixed clock; the `scope_prn` is the owner from
    `GetServiceAccount`; an extra `scopePrn` field in the form does not change the request.
  - revoke, archive, allow model calls.
- `modelCallState`: `active` + allowed → `yes`; denied → `no`; `disabled` → `archived` with no
  call; an unknown status → `archived` with no call; a failed call → `unknown`.
- The loaders: the section-level 403; the Projects error state; an archived owner → read-only;
  `unknown` lifecycle → read-only; the key status mapping; the limit+1 "more exist" rule (exactly
  50 gives no note); `iam.apikeys` absent → no `ListApiKeys` call; `?sa=` of another owner → the
  "another scope" note.
- The token and the logs: the issue action runs while the test spies on `process.stdout.write`
  and `process.stderr.write` (the logger is module-level). No written line contains the token.
- jsdom tests: every row and panel action's result survives the refresh that unmounts its
  control; an issue result is shown although a later action started; the token panel stays open
  after a revoke.

### 7.2 End-to-end (Playwright)

The e2e world becomes **stateful**. It records accounts, keys and grants from the create, issue,
revoke, archive and grant calls. `IsAuthorized` about an account PRN answers from the recorded
grants. `IsAuthorized` about the user answers from an allow list, as in iam-console's world. Both
worlds get handlers for `listTeams`, `listProjects`, `getTeam` and the service-account RPCs, and
`iam.apikeys` in the IAM descriptor.

**Single-zone tier (no Docker):**

1. Create an account → "Can call models: Yes" (the world answers from a recorded grant) → issue a
   key → the token shows once → reload → the token is gone and the key is "Active" → revoke →
   "Revoked" → archive → "Archived", keys "Inactive (account archived)".
2. The world fails `grantRole` → "Can call models: No" and the "Allow model calls" control → use
   it → "Yes".
3. A user who may view but not manage sees no mutation controls. A user who may not view service
   accounts sees the section-level denial, and the page is 200.
4. **Token exposure:** after an issue, the plaintext appears in exactly one response body (the
   action response), and in no later HTML, RSC or prefetch response of the test.
5. A `project_admin` reaches the project page from the overview's Your projects list, and the org
   page is 403 for them.
6. `iam.apikeys` absent from the descriptor: no key controls, and no `ListApiKeys` call.

**Two-zone tier (Docker):** the "Manage in IAM" link is present and resolves to the IAM zone.

New rows get R-numbers in `tests/unit/e2e-rows.test.ts`.

### 7.3 The per-request call count (D12)

The row fetches `/gateway/orgs/<org>` with a plain `fetch` (no router, so no prefetch), reads the
fake's call log, and keeps only the calls with that request's correlation id. It asserts the exact
count per RPC.

The expected count is derived here, from § 4.2, **before** the measurement. A difference is a
finding to explain, not a number to copy into the test. For a fixture with S scopes (S ≤ 50),
T teams and no `sa` parameter:

| RPC | Count | From |
|---|---|---|
| `authn.introspect` | 1 | the session principal, memoized per request |
| `authz.listRoleGrants` | 1 | `myScopes()`, with `iam.authz.cedar` present |
| tenancy `Get*` for scopes | S | `myScopes()` labels |
| `tenancy.getOrganization` | 1 | the page (not shared with `myScopes()`) |
| `authz.isAuthorized` | 5 | `mayI()`, five distinct actions |
| `serviceAccounts.listServiceAccounts` | 1 | the section |
| `tenancy.listTeams` | 1 | the Projects list |
| `tenancy.listProjects` | T | the Projects list |

A second row repeats the count with `?sa=<uuid>` and expects, in addition: 1
`getServiceAccount`, 1 `isAuthorized` (the model-call state), 1 `listApiKeys`.

**Limit.** This counts calls against a fake. It does not measure latency against a real IAM. The
per-session cache stays a follow-up that needs a latency number first.

### 7.4 Re-baselines of existing tests

- R11, `two-zone-session.spec.ts:73-75` and `tests/integration/scope-page.test.ts` expect the
  `zone-overview` component at `/gateway/orgs/<org>`. They change to the new page.
- R5's comment says "this zone has no Server Action". R5 extends to scan an action response, as
  iam-console's copy does. gateway-console gets a copy of iam-console's `actions-structure.test.ts`.
- `ts/apps/iam-console/tests/unit/world-actions.test.ts`'s `ALL_ACTIONS` gets the five new
  `IAM_ACTIONS` names.
- iam-console's tests of the moved helpers move with them.

---

## 8. Follow-ups

1. IAM: a scoped `ListRoleGrants`, so a node admin can list and revoke a service account's
   grants. This unblocks "Stop model calls" (D9).
2. IAM: map the `uq_role_grant_principal_role_scope` violation to `already-exists`, not to
   `internal` (§ 3.2). A console user can cause it by a double click, and today it logs at error
   level and counts in IAM's gRPC error rate.
3. A latency measurement of `myScopes()` against a real IAM, then the per-session cache if it is
   too slow.
4. An operator runbook for the service account, `gateway_user` and key setup.
5. Team-owned service accounts in the gateway zone, so an organization admin can see every live
   key of the organization.

---

## 9. Recorded limits

- gateway-console copies `FormError`, `node-status.ts`, `section-error.tsx` and the two-step
  confirm button from iam-console (D11). Nothing gates a divergence.
- The two create RPCs are not atomic (D3). A failed grant leaves an account that cannot call
  models until someone uses the repair control.
- An account that got `gateway_user` outside the console, at an ancestor scope, shows "Yes". The
  console cannot show where the grant comes from (D8).
- An account that a team owns is not visible in this zone, so an organization admin has no
  complete list of live keys here.
- After an archive, IAM evicts the keys from its API-key cache. How fast every IAM replica stops
  accepting them depends on IAM's cache configuration, which this design does not check.
- The org page makes one `ListProjects` call per team (at most 50, 8 in flight). This is counted
  (§ 7.3), not timed.
- The overview's Your projects list shows at most the 50 scopes that `myScopes()` shows
  (`SCOPE_CAP`).
- Plan deviation 8 (a successful issue revalidates) opens a window: if the revalidated render fails
  at page level (`GetOrganization` or `GetProject`), the page is replaced and the shown token is
  lost. A section-level error or denial keeps the token, because the token panel lives in a frame
  that every section view kind renders.

---

## 10. Change log

**Revision 2** (after the adversarial challenge, verdict "needs rework"):

- The token rules (§ 5.4) now cover the four holes the review found: `useActionState` sending the
  token back, a generation rule discarding a token, a close on "the next submission", and the
  no-JavaScript document POST.
- Added the two IAM capability gates (§ 3.3, D13).
- An archived account no longer shows "Yes" and "Active" keys (D8, § 4.3, § 5.7).
- Keys and the model-call state load only for a selected account (D14). Lists use a pager with
  limit+1 (D15). Projects load with at most 8 calls in flight. Links do not prefetch.
- Actions take their scope from IAM, never from the form (D6, § 5.1).
- A typed `CreateState`, and one revalidation rule per result (§ 5.2).
- One result region per section for every action (§ 4.6).
- The call count is per request, by correlation id, with the formula written before the
  measurement (§ 7.3).
- A stateful e2e world, so the main row cannot pass without a grant (§ 7.2).
- The overview lists team and project scopes, so a `project_admin` can reach the project page
  (D16).
- Smaller fixes: the "Manage in IAM" href from the zone map; `InvokeModel` not in `IAM_ACTIONS`;
  the D15 error copy; the expiry enum and its label; the project header's data sources; the
  archived-owner copy; the duplication list; the `zod` dependency and the event name; the
  mixed-URL behaviour; keys with another scope; the existing tests to re-baseline.
