# SMA-676 — gateway-console: grant `gateway_user` to a person at org scope

**Status:** Draft (Gate 1 pending)
**Date:** 2026-09-26
**Issue:** [SMA-676](https://linear.app/smaschek/issue/SMA-676)
**Depends on:** SMA-635 (the playground, merged)
**Related:** SMA-635 spec `2026-09-23-sma-635-gateway-user-auth-design.md`, decisions D4 and D10

## 1. Problem

The SMA-635 playground authorizes `InvokeModel` against the **org** PRN (SMA-635 D4). Only
`gateway_user` and `platform_admin` hold `InvokeModel`
(`rs/crates/libs/paigasus-iam-core/src/authz/roles.rs:202-204`). `org_admin` does not hold it
(`roles.rs:105-135`). The console grants `gateway_user` only to a service account
(`ts/apps/gateway-console/app/(console)/service-accounts/commands.ts:17,73,87`). So today a person
gets `gateway_user` only through an out-of-band IAM `GrantRole` call (SMA-635 D10).

SMA-676 adds a console control. With it, an `org_admin` grants and revokes `gateway_user` for a
person at org scope, and sees which people in the org hold it.

### 1.1 Why the console alone cannot do this

Three facts in IAM block a console-only change:

1. **No "holders of a role at a scope" query.** `ListRoleGrantsRequest` has one filter,
   `principal_prn` (`contracts/proto/paigasus/iam/v1/iam.proto:382-386`).
2. **An `org_admin` cannot list another principal's grants.** `RoleService::list`
   (`rs/crates/services/paigasus-iam/src/application/roles.rs:313-319`) checks `ListRoleGrants`
   against the **Root** PRN when the actor is not the principal. An `org_admin` holds
   `ListRoleGrants` only at org scope, so IAM refuses.
3. **`RevokeRole` takes a grant id** (`iam.proto:378`). A list call is the only way to get the id
   of an existing grant. So revoke depends on the same missing query.

Two more facts shape the design:

4. **A principal PRN does not show the principal's kind.** A PRN is `iam:principal/<uuid>` for a
   person and for a service account. IAM stores the kind (`user` / `service_account`,
   `paigasus-iam-core/src/principal.rs:10-13`) in its own table.
5. **An org-owned service account holds `gateway_user` at the org PRN.** The console grants it at
   `scopePrn: account.ownerPrn` (`commands.ts:73`). So "grants of `gateway_user` at org X" mixes
   people and service accounts. The new query must filter by kind.

## 2. Scope and decisions

### 2.1 Decisions

| # | Decision | Reason |
|---|---|---|
| D1 | This issue extends IAM **and** adds the console control, in one PR. | Sven's choice at brainstorming. The console needs the query to list and to revoke. |
| D2 | Extend `ListRoleGrants` with optional, AND-ed filters `scope_prn`, `role_key`, `principal_kind`. Do not add a new RPC. | The change is additive on the wire. Existing callers (`scopes.ts`, `modelCallState`) do not change. A later team or project scope uses the same query. A new RPC would duplicate the list, mapping and fake code. |
| D3 | A request must set `principal_prn` or `scope_prn` (or both). If it sets neither, IAM answers `InvalidArgument` (`MissingRequiredField("principal_prn")`, as today). | Keeps the old "principal required" contract for old callers, and adds one new entry point. |
| D4 | Authorization for `ListRoleGrants`: (a) `principal_prn` equals the actor → no check (as today); (b) else `scope_prn` is set → `ListRoleGrants` at the scope node's resource PRN (`scope_resource_prn`, `roles.rs:82`); (c) else → `ListRoleGrants` at Root (as today). | Case (b) is the same anti-escalation shape as `GrantRole`/`RevokeRole`, which authorize at the scope node. An `org_admin` holds `ListRoleGrants` at org scope (`roles.rs:127-129`). |
| D5 | A scope-filtered list returns only grants **at** that exact scope, not grants at descendant teams or projects. | The playground authorizes at the org PRN only (SMA-635 D4). A descendant grant does not give playground access, so it does not belong in this list. |
| D6 | A second grant of the same (principal, role, scope) returns `AlreadyExists`. The adapter maps the `uq_role_grant_principal_role_scope` unique violation to `AuthzError::Conflict`, and the gRPC and HTTP layers map that to `AlreadyExists` / 409. | Today the violation is a generic `Backend` error (`pg_role_grants.rs:96-101` maps only the role FK). The SDK maps `AlreadyExists` to the `conflict` presentation (`ts/packages/paigasus-sdk/src/errors/transport-status.ts:37`), so the console can say "already holds the role". |
| D7 | HTTP parity: `GET /v1/authz/role-grants` accepts the same three new query parameters, with the same rules (D3, D4, D5). | The HTTP route calls the same `RoleService::list` (`adapters/http/authz.rs:145-150`). One service function, two adapters, one behavior. |
| D8 | The console section lives on the org settings page `/gateway/orgs/[org]`, below the service-accounts section. Title: "Model access for people". | It is org-scope administration, like the service-account section on the same page. No new route and no nav change. |
| D9 | The grant control offers **only org members that do not hold the role**, from a combobox. There is no free-text PRN field. | Sven's choice at brainstorming. An admin cannot grant model spend to a principal outside the org by a typo or a pasted PRN. |
| D10 | The holders table lists every **person** (`principal_kind = user`) that holds `gateway_user` at the org PRN, members or not. A holder that is not an org member shows a "not a member" mark. | `DetachMembership` does not remove role grants. A removed person keeps `gateway_user` and can still use the playground. The admin must see such a person to revoke the grant. |
| D11 | Controls show when `mayI('GrantRole', orgPrn)` / `mayI('RevokeRole', orgPrn)` allow them **and** the `iam.authz.cedar` capability is present. `mayI` is cosmetic. IAM is the real gate. | The same rule as the service-account "Allow model calls" button (`service-accounts/load.ts:110,126`; `actions.ts:27-31`). |
| D12 | Each list reads at most 500 rows. When a list reaches 500, the section shows "List truncated at 500". | `ListRoleGrants` does not page on the server (`adapters/grpc/authz.rs:232-236`), and orgs are small at this stage. The cap stops an unbounded response. §9 records the limit. |
| D13 | The section does not show names or emails. It shows the principal PRN, as the iam-console member list does. | IAM has no user lookup RPC (no `GetUser`/`ListUsers`). Adding one is outside this issue (§8). |

### 2.2 Out of scope

- Team or project scope for the playground (SMA-635 D4).
- A user lookup (name, email) in IAM, and a person search by email.
- A grant to a person who is not an org member.
- Server-side paging for `ListRoleGrants` (§8).
- Changes to the iam-console.

## 3. Backend facts this design relies on

- `org_admin` holds `GrantRole`, `RevokeRole`, `ListRoleGrants` at org scope (`roles.rs:127-129`).
- `gateway_user` allows scope kinds Organization, Team, Project (`roles.rs:263`).
- `RoleService::grant` checks role existence, the scope-kind fit, and `GrantRole` at the scope node
  (`application/roles.rs:199-208`). It has no role allow-list and no membership check. So an
  `org_admin` can grant `gateway_user` at org scope today.
- `RoleService::revoke` of a missing grant is a no-op that returns `Ok(())`
  (`application/roles.rs:260-264`). The console treats a revoke race as success.
- `ListMemberships(node_prn = org)` returns the org's members as `principal_prn`s
  (`iam.proto:219-229`). Service accounts do not get an org membership at creation, but an admin
  can attach one by PRN.
- The IAM gRPC `ListRoleGrants` handler ignores `limit`/`offset` (`adapters/grpc/authz.rs:232-236`).

## 4. Architecture

### 4.1 Contract (`contracts/proto/paigasus/iam/v1/iam.proto`)

```proto
enum PrincipalKind {
  PRINCIPAL_KIND_UNSPECIFIED = 0;
  PRINCIPAL_KIND_USER = 1;
  PRINCIPAL_KIND_SERVICE_ACCOUNT = 2;
}
message ListRoleGrantsRequest {
  string principal_prn = 1;   // now optional when scope_prn is set (D3)
  uint32 limit = 2;
  uint64 offset = 3;
  string scope_prn = 4;       // exact scope match (D5)
  string role_key = 5;
  PrincipalKind principal_kind = 6; // UNSPECIFIED = any kind
}
```

`buf breaking` must stay green: all changes add fields or add an enum. `buf format -w` runs on the
file. Codegen regenerates the Rust, Python and TS bindings.

### 4.2 IAM core (`paigasus-iam-core`)

- A new value object `RoleGrantFilter { principal: Option<PrincipalId>, scope: Option<GrantScope>,
  role_key: Option<String>, principal_kind: Option<PrincipalKind> }`. A constructor rejects the
  empty filter (D3).
- `RoleGrantStore` gets `list_by_filter(&self, f: &RoleGrantFilter, limit: u32) ->
  Result<Vec<RoleGrant>, AuthzError>`. `list_by_principal` stays for existing callers.

### 4.3 IAM service (`paigasus-iam`)

- `RoleService::list` takes the filter and applies the D4 authorization rule, then calls
  `list_by_filter` with the cap (D12).
- `pg_role_grants.rs`: `list_by_filter` builds one SeaORM query. The scope filter uses
  `scope_columns` (`pg_role_grants.rs:115-122`). The kind filter joins the principals table on
  `principal_id`. Order by `principal_id`, then `id`, so the result is stable.
- `pg_role_grants.rs`: `map_grant_err` maps the `uq_role_grant_principal_role_scope` unique
  violation to `AuthzError::Conflict` (D6), by the constraint name, in the same way as the
  existing `fk_role_grant_role` arm.
- The in-memory fake (`application/fakes.rs`, `InMemoryRoleGrants`) implements the same filter.
- gRPC `list_role_grants` and HTTP `list_role_grants` parse the new fields and build the filter.
  An unknown `role_key` in the filter is not an error: the list is empty.
- `AuthzError::Conflict` maps to gRPC `AlreadyExists` and HTTP 409, if the existing conversion
  does not do so already. The plan checks the current mapping.

### 4.4 Console (`ts/apps/gateway-console`)

New directory `app/(console)/people-model-access/`, the same shape as `service-accounts/`:

| File | Holds |
|---|---|
| `load.ts` | `loadPeopleModelAccess(deps, orgPrn)`: two IAM calls in parallel, `authz.listRoleGrants({ scopePrn: orgPrn, roleKey: 'gateway_user', principalKind: USER, limit: 500 })` and `tenancy.listMemberships({ filter: nodePrn(orgPrn), limit: 500 })`, plus `mayI` for `GrantRole` and `RevokeRole`. It returns a view: `holders[] { grantId, principalPrn, member: boolean }`, `candidates[] { principalPrn }` (members minus holders), `flags { cedar, canGrant, canRevoke, truncated }`, or a `denied` / `error` state. |
| `commands.ts` | Pure commands: `grantModelAccess({ principalPrn, orgPrn })` → `authz.grantRole({ principalPrn, roleKey: 'gateway_user', scopePrn: orgPrn })`; `revokeModelAccess({ grantId })` → `authz.revokeRole({ id: grantId })`. Both go through `callIam`. |
| `actions.ts` | Two Server Actions: `iamClientsForAction()` → zod parse (`prnField` for the principal and the org, a UUID for the grant id) → command → `revalidatePath` of the org page on success. |
| `block.tsx` | `peopleModelAccessBlock({ view, path })`: the holders `Table` with a Revoke button per row, the `Combobox` of candidates and a Grant button, the empty states, and the "List truncated at 500" line. |

`orgs/[org]/load.ts` and `page.tsx` call the loader and render the block below
`serviceAccountsBlock`. The org PRN comes from the loader's existing `orgPrn`.

The shared constant `GATEWAY_ROLE = 'gateway_user'` moves from `service-accounts/commands.ts:17`
to a small shared module, so both sections name the role from one place.

## 5. Flows

### 5.1 View the section

1. The org page loads. The loader calls `listRoleGrants` and `listMemberships` in parallel.
2. If either call is denied, the section shows the `denied` state. If either call fails, the
   section shows the `error` state. The page stays 200 in both cases.
3. Holders show in PRN order. A holder whose PRN is not in the member list shows "not a member".
4. Candidates are the members whose PRN is not a holder PRN.
5. If `flags.cedar` is false, the section shows the list only, with no controls.

### 5.2 Grant

1. The admin picks a candidate and presses Grant.
2. The action validates the input and calls `grantRole`.
3. On success, the page revalidates. The person moves from the candidates to the holders.
4. On `conflict` (D6: another admin granted the role first), the form shows "This person already
   has model access" and the page revalidates.
5. On `denied`, `invalid-input` or another error, the form shows the presentation's message. The
   form never shows the IAM message text.

### 5.3 Revoke

1. The admin presses Revoke on a holder row.
2. The action calls `revokeRole(grantId)`. A missing grant is a no-op success in IAM (§3).
3. On success, the page revalidates. A member moves back to the candidates. A non-member leaves
   the list.

## 6. Security

- IAM is the only gate. `mayI` only hides controls.
- The new list path authorizes at the scope node (D4 b). A caller cannot read grants of another
  org: the scope PRN names the org, and `ListRoleGrants` at that org needs a grant in that org.
- A forged `scope_prn` for a nonexistent node gives a deny (`ResourceNotFound` fails closed as a
  `Deny`, `model.rs:245-252`), not an existence oracle.
- The kind filter runs in SQL. A service account never appears in the holders table, and the
  console never has to guess a kind from a PRN.
- The grant action accepts only a principal PRN and the org PRN from the form. The role key is a
  server constant, so a crafted form cannot grant a different role. The action does not re-check
  that the principal is a member: D9 is a UI choice, not a security boundary. IAM already lets an
  `org_admin` grant to any principal with a direct API call.

## 7. Testing

### 7.1 Rust

- `RoleGrantFilter`: the empty filter is rejected.
- `RoleService::list` with the fakes: self (no check), scope path (check at the scope node),
  other principal without scope (check at Root), and a deny on each path.
- Filter semantics with `InMemoryRoleGrants`: scope exact match (a team grant is absent from an
  org-scope list), role filter, kind filter, AND of all filters.
- `pg_role_grants` (Docker-gated, `tests/authz_role_grants.rs`): the kind join, the scope match,
  and the duplicate grant → `AuthzError::Conflict`.
- gRPC and HTTP: the new fields parse, an empty request is `InvalidArgument`, a duplicate grant
  is `AlreadyExists` / 409.

### 7.2 TS

- Unit (`tests/unit/`): the block in each state (holders and candidates, empty, denied, error,
  no cedar, truncated, not-a-member mark).
- Integration (`tests/integration/`, with `fake-iam.ts`): the loader join, the two commands, the
  `conflict` path. `fake-iam.ts` and `dev-world.ts` learn the new request fields.
- The existing `scopes.ts` and service-account tests stay green without edits.

### 7.3 e2e

One Playwright test in `tests/e2e/` (Docker-gated): an `org_admin` grants model access to a member,
the member opens the playground and gets an answer, the admin revokes the access, and the
playground refuses the member. It extends the `playground-authz.spec.ts` fixtures.

## 8. Follow-ups

- A user lookup RPC in IAM, so the section can show names and emails.
- Server-side paging for `ListRoleGrants` (limit and offset are ignored today).

## 9. Recorded limits

- Each list stops at 500 rows (D12).
- A person with `gateway_user` only at a team or project does not show. They also do not have
  playground access (SMA-635 D4), so the list is correct for this page.

## 10. Change log

- 2026-09-26: first draft after brainstorming.
