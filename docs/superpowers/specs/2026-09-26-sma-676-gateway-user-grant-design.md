# SMA-676 — gateway-console: grant `gateway_user` to a person at org scope

**Status:** Approved (Gate 1, 2026-09-26), revision 2; implemented
**Date:** 2026-09-26
**Issue:** [SMA-676](https://linear.app/smaschek/issue/SMA-676)
**Depends on:** SMA-635 (the playground, merged)
**Related:** SMA-635 spec `2026-09-23-sma-635-gateway-user-auth-design.md`, decisions D4 and D10;
SMA-444 (the `ListRoleGrants` exposure rule "M3"); SMA-636 (the org settings page, §7.3 call count)

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

1. **No "grants at a scope" query.** `ListRoleGrantsRequest` has one filter, `principal_prn`
   (`contracts/proto/paigasus/iam/v1/iam.proto:382-386`).
2. **An `org_admin` cannot list another principal's grants.** `RoleService::list`
   (`rs/crates/services/paigasus-iam/src/application/roles.rs:313-319`) checks `ListRoleGrants`
   against the **Root** PRN when the actor is not the principal. An `org_admin` holds
   `ListRoleGrants` only at org scope, so IAM refuses.
3. **`RevokeRole` takes a grant id** (`iam.proto:378`). A list call is the only way to get the id
   of an existing grant. So revoke depends on the same missing query.
4. **A principal PRN does not show the principal's kind.** A PRN is `iam:principal/<uuid>` for a
   person and for a service account. IAM stores the kind (`user` / `service_account`,
   `paigasus-iam-core/src/principal.rs:10-13`) in its own table. `Membership` and `RoleGrant`
   carry no kind (`iam.proto:68-73,364-369`).
5. **An org-owned service account holds `gateway_user` at the org PRN** (`commands.ts:73`,
   `scopePrn: account.ownerPrn`). A service account can also be an org member:
   `AttachMembership` has no kind check. So both lists need a kind filter.
6. **The org creator is not an org member.** `OrganizationService::create` gives the creator an
   `org_admin` grant and no membership (`application/organizations.rs:156-164`). A "members only"
   picker never offers the person who most needs model access first.

## 2. Scope and decisions

### 2.1 Decisions

| # | Decision | Reason |
|---|---|---|
| D1 | This issue extends IAM **and** adds the console control, in one PR. | Sven's choice at brainstorming. The console needs the query to list and to revoke. |
| D2 | Extend `ListRoleGrants` with optional, AND-ed filters `scope_prn`, `role_key`, `principal_kind`. Do not add a new RPC. | The change is additive on the wire. The existing caller (`scopes.ts`) does not change. A later team or project scope uses the same query. A new RPC duplicates the list, mapping and fake code. |
| D3 | A request must set `principal_prn` or `scope_prn` (or both). If it sets neither, IAM answers `InvalidArgument` with `MissingRequiredField("principal_prn\|scope_prn")`. | Keeps "a principal is required" for old callers and adds one entry point. The field name tells a caller both ways to fix the request, in the style of `MutuallyExclusiveFields("principal\|node")`. |
| D4 | Authorization for `ListRoleGrants`: (a) `principal_prn` is set and equals the actor → no check (as today); (b) else `scope_prn` is set → `ListRoleGrants` at the scope node's resource PRN (`scope_resource_prn`, `application/roles.rs:82`); (c) else → `ListRoleGrants` at Root (as today). The check runs **before** any scope lookup. | Case (b) is the anti-escalation shape of `grant`/`revoke` and of `Authorize::decide_gated` (`application/authorize.rs:75-80`). It widens the SMA-444 "M3" rule ("self or platform admin") to "self, or an admin at that exact scope". Sven decided that this follows the existing rule and needs no ADR amendment; this row records the reason. |
| D5 | The scope filter is an **exact** match on the stored `scope_node_prn` (canonical PRN). It returns no grants at descendant teams or projects. A `scope_prn` equal to the Root sentinel matches `scope_kind = 'root'` rows, and D4 (b) then checks at Root. A team PRN with a forged org slot matches no row, and Cedar decides the check against the stored ancestry. | The playground authorizes at the org PRN only (SMA-635 D4), so a descendant grant gives no playground access. The duplicate key already uses `scope_node_prn` (`m0004_create_authz.rs:187-188`), so the predicate has no NULL gap. |
| D6 | The scope path honours `limit` and `offset` through `Page::new` (1..=200, default 50, `application/pagination.rs:11,28-29`). Order: `principal_id`, then `id`. The principal-only path does not change: it still returns every row and ignores `limit`/`offset`. | The console needs bounded, stable pages. A change to the old path would cut `scopes.ts` without a signal, because HTTP has no `limit` there (`adapters/http/dto.rs:426-428`) and `Page::new(None)` gives 50. |
| D7 | `principal_kind` is an enum. `UNSPECIFIED` means any kind. An unknown numeric value is refused with `InvalidArgument`; it never widens to "any". HTTP takes `principal_kind=user` or `service_account` (the strings of `PrincipalKind::as_str`). | A proto3 enum is open. The repo refused silent widening before (`adapters/grpc/convert.rs:210-214`, SMA-583). |
| D8 | `ListMemberships` gets the same optional `principal_kind` filter (field 5). It AND-s with either existing filter, and it uses the same refusal rule as D7. | Sven's choice. The console asks for user members only, so a member service account never shows as a person. |
| D9 | `GrantRole` becomes **idempotent**. When a grant for the same (principal, role, scope) exists, IAM returns the existing grant with OK. It writes no event, no audit row, and no policy-generation bump. | Sven's choice. It removes the generic Internal error on a double grant, needs no new error reason, and makes a two-admin race a success. It also makes the service-account "Allow model calls" retry a success. |
| D10 | HTTP parity: `GET /v1/authz/role-grants` and the membership list route accept the new parameters with the same rules. `RoleService::list` takes raw strings and owns D3, D4 and D7, so both adapters get one behaviour by construction. | The HTTP routes call the same service functions (`adapters/http/authz.rs:145-150`). |
| D11 | The console section lives on the org settings page `/gateway/orgs/[org]`, below the service-accounts section. Title: "Model access for people". | Org-scope administration, like the service-account section on the same page. No new route and no nav change. |
| D12 | **Candidates** = (user members of the org ∪ users that hold any role at the org scope) − the `gateway_user` holders. The grant control offers only candidates, in a combobox. There is no free-text PRN field. | Sven's choice. It offers the org creator (§1.1 fact 6), including the signed-in admin, and it never offers a service account or an outsider. |
| D13 | **Holders** = every user that holds `gateway_user` at the org PRN, member or not. A holder that is not a user member shows a "not a member" mark. The mark is hidden when the member list is truncated. | `DetachMembership` does not remove role grants. A removed person keeps the grant and can still use the playground, so the admin must see them to revoke. |
| D14 | Controls show when `mayI('GrantRole', orgPrn)` / `mayI('RevokeRole', orgPrn)` allow them **and** the org and its ancestors are active. `mayI` is cosmetic. IAM is the real gate. | The same rules as the service-account section (`service-accounts/load.ts:59-63,110,126`). |
| D15 | When the `iam.authz.cedar` capability is off, the loader makes no IAM call. The section shows one line: "Role administration is not enabled on this IAM." | `ListRoleGrants` itself is behind that capability (`adapters/grpc/authz.rs:85-91,228`; HTTP mounts it only in `admin_router`). A call would only produce the error state. |
| D16 | The section does not show names or emails. It shows the principal PRN, as the iam-console member list does. | IAM has no user lookup RPC. §8 records the follow-up. |

### 2.2 Out of scope

- Team or project scope for the playground (SMA-635 D4).
- A user lookup (name, email) in IAM, and a person search by email.
- A grant to a person outside the candidate set of D12.
- Paging for the principal-only `ListRoleGrants` path (D6).
- Feature changes to the iam-console. (Its test list of IAM action names changes; see §7.4.)

## 3. Backend facts this design relies on

- `org_admin` holds `GrantRole`, `RevokeRole`, `ListRoleGrants` at org scope
  (`paigasus-iam-core/src/authz/roles.rs:125-127`).
- `gateway_user` allows scope kinds Organization, Team, Project (`roles.rs:263`).
- `RoleService::grant` checks the role, the scope-kind fit, and `GrantRole` at the scope node
  (`application/roles.rs:199-208`). It has no role allow-list and no membership check.
- `RoleService::revoke` returns `NotFound` when the grant id does not exist
  (`application/roles.rs:266`, pinned by `revoke_missing_grant_is_not_found`, `:630-635`). Only
  the race between `find` and `revoke_in` is a no-op (`:293-304`).
- A duplicate grant violates `uq_role_grant_principal_role_scope` and surfaces as
  `AuthzError::Backend` today (`pg_role_grants.rs:96-101,186-190`).
- `AuthzError::Conflict` maps to `TenancyError::PolicyConflict` (`application/error.rs:364`).
  This design does not use it (D9).
- `Page::new` accepts a limit of 1..=200 (`application/pagination.rs:11,28-29`). `ListMemberships`
  validates its limit with it (`adapters/grpc/tenancy.rs:679`).
- The gRPC `ListRoleGrants` handler ignores `limit`/`offset` today (`adapters/grpc/authz.rs:232-236`).
- A missing node in an authorization check is a fail-closed `Deny`
  (`adapters/authz/cedar_authorizer.rs:228-233`), not an existence oracle.
- IAM bumps `policy_gen` after a grant or revoke commits. A replica with the memory cache can
  keep an old decision until the snapshot TTL backstop, about 31 s at defaults
  (`application/bootstrap_admin.rs:194-196`).

## 4. Architecture

### 4.1 Contract (`contracts/proto/paigasus/iam/v1/iam.proto`)

```proto
enum PrincipalKind {
  PRINCIPAL_KIND_UNSPECIFIED = 0;
  PRINCIPAL_KIND_USER = 1;
  PRINCIPAL_KIND_SERVICE_ACCOUNT = 2;
}
message ListRoleGrantsRequest {
  string principal_prn = 1;          // optional when scope_prn is set (D3)
  uint32 limit = 2;                  // honoured on the scope path only (D6)
  uint64 offset = 3;
  string scope_prn = 4;              // exact match (D5)
  string role_key = 5;
  PrincipalKind principal_kind = 6;  // UNSPECIFIED = any (D7)
}
message ListMembershipsRequest {
  oneof filter { string principal_prn = 1; string node_prn = 2; }
  uint32 limit = 3;
  uint64 offset = 4;
  PrincipalKind principal_kind = 5;  // D8
}
```

All changes add fields or a new enum, so `buf breaking` stays green under the FILE rules. `buf
format -w` runs on the file. Codegen regenerates the Rust, Python and TS bindings.

### 4.2 IAM core (`paigasus-iam-core`)

- A new value object `RoleGrantFilter { principal: Option<PrincipalId>, scope: Option<GrantScope>,
  role_key: Option<String>, principal_kind: Option<PrincipalKind> }`. Its constructor returns
  `None` when neither `principal` nor `scope` is set; `RoleService` turns that into the D3 error.
- A new **read port** `RoleGrantQuery::find(&self, f: &RoleGrantFilter, limit: u64, offset: u64)
  -> Result<Vec<RoleGrant>, AuthzError>`. `RoleGrantStore` does not change. A new method on
  `RoleGrantStore` would change nine implementations, seven of them test fakes, and the repo
  refused that cost once before (`authz/ports.rs:101-103`).
- A new variant `AuthzError::DuplicateGrant`, raised only by the grant insert (D9).

### 4.3 IAM service (`paigasus-iam`)

- `RoleServiceDeps` gets the `RoleGrantQuery` port.
- `RoleService::list(actor, ListRoleGrantsInput { principal_prn, scope_prn, role_key,
  principal_kind, page })` parses the raw strings, applies D3, D4, D6 and D7, then calls the old
  `list_by_principal` (principal-only path) or `RoleGrantQuery::find` (scope path).
- `RoleService::grant` (D9): after the authorization check and the scope resolution, it asks
  `RoleGrantQuery::find` for (principal, role, scope). If a grant exists, it returns that grant.
  Else it inserts. If the insert raises `AuthzError::DuplicateGrant` (a concurrent insert won),
  the transaction has rolled back, and `grant` reads the winner's grant and returns it. The
  pre-check runs after the authorization check, so it is not an existence oracle.
- `pg_role_grants.rs`: `map_grant_err` maps a unique violation on
  `uq_role_grant_principal_role_scope` to `AuthzError::DuplicateGrant`, by constraint name, as
  the `fk_role_grant_role` arm does. `PgRoleGrantStore` implements `RoleGrantQuery` with one
  SeaORM query: `scope_node_prn = $scope` (or `scope_kind = 'root'`), `role_key = $role`, and an
  inner join on the principals table for the kind. Order `principal_id, id`.
- Memberships: the membership list query gets the same optional kind join (D8). The plan
  chooses the port shape by the same rule as above: no new method on a port with many fakes.
- `From<AuthzError> for TenancyError` maps `DuplicateGrant` to `Internal`. It can only reach
  that arm through a path other than `grant`, which is a defect.
- Fakes: one `InMemoryRoleGrantQuery` that holds the grants and a `principal_id →
  PrincipalKind` map. The in-memory membership fake learns the same map.
- `application/bootstrap_admin.rs:239-245`: a `DuplicateGrant` from a concurrent seed is a
  success, not a "lockout" warning.
- The gRPC and HTTP handlers only move fields into `ListRoleGrantsInput` (D10).

### 4.4 Console (`ts/apps/gateway-console`)

New directory `app/(console)/people-model-access/`, the same shape as `service-accounts/`:

| File | Holds |
|---|---|
| `load.ts` | `loadPeopleModelAccess(deps, { orgPrn, orgActive })`. When the cedar capability is off, it returns `disabled` and makes no call (D15). Else it reads two lists in parallel, each in pages of 200 (below): `grantsAtOrg` = `listRoleGrants({ scopePrn: orgPrn, principalKind: USER })` and `members` = `listMemberships({ filter: nodePrn(orgPrn), principalKind: USER })`. Plus `mayI` for `GrantRole` and `RevokeRole`. It returns `holders[] { grantId, principalPrn, member: boolean \| null }`, `candidates[] { principalPrn }`, `flags { canGrant, canRevoke, grantsTruncated, membersTruncated }`, or `denied` / `error`. |
| `commands.ts` | `grantModelAccess({ principalPrn, orgPrn })` → `authz.grantRole({ principalPrn, roleKey: GATEWAY_ROLE, scopePrn: orgPrn })`. `revokeModelAccess({ principalPrn, grantId, orgPrn })` → first `listRoleGrants({ principalPrn, scopePrn: orgPrn, roleKey: GATEWAY_ROLE })`; it revokes only when `grantId` is in that answer, else it returns `invalid-input`; then `authz.revokeRole({ id: grantId })`. All calls go through `callIam`. |
| `actions.ts` | Two Server Actions: `iamClientsForAction()` → zod parse (`prnField` for the principal and the org, a UUID for the grant id) → command → `revalidatePath` of the org page. |
| `block.tsx` | `peopleModelAccessBlock({ view, path })`: the holders `Table` with a Revoke button per row, the `Combobox` of candidates and a Grant button, the empty states, the `disabled` and `denied` lines, and one truncation line per list. |

**Join.** Holders = `grantsAtOrg` rows with `roleKey = gateway_user`. Grantees = the distinct
principals of `grantsAtOrg`. Candidates = (member PRNs ∪ grantee PRNs) − holder PRNs, sorted.
A holder's `member` is `null` (no mark) when `membersTruncated` is true.

**Paging.** Each list reads pages of 200 until a page returns fewer than 200 rows, at most 5
pages. If the fifth page is full, the loader asks for one more row (`limit: 1, offset: 1000`).
If that row exists, the list is truncated, and the section shows "Showing the first 1000" for
that list. This is the zone's N+1 rule (`lib/paging.ts:3-6`) applied to a bounded read.

**Shared.** `GATEWAY_ROLE = 'gateway_user'` moves from `service-accounts/commands.ts:17` to one
shared module. `RevokeRole` joins `IAM_ACTIONS` in
`ts/packages/paigasus-console-core/src/authorize.ts:45-72`, and the comment at `:32-35` is
updated for D4.

`orgs/[org]/load.ts` and `page.tsx` call the loader and render the block below
`serviceAccountsBlock`. The org PRN and the lifecycle state come from the existing loader.

## 5. Flows

### 5.1 View the section

1. When the cedar capability is off, the section shows the D15 line. Stop.
2. The loader reads the two lists. A permission denial on either list shows the `denied` line
   ("You cannot see model access for this organization"). Another failure shows the `error`
   state. The page stays 200 in both cases.
3. Holders show in PRN order, with the "not a member" mark per D13.
4. Controls show per D14. When the org is not active, the section is read-only.

### 5.2 Grant

1. The admin picks a candidate and presses Grant.
2. The action validates the input and calls `grantRole`.
3. On success, including the idempotent case (D9), the page revalidates. The person moves from
   the candidates to the holders.
4. On `denied`, `invalid-input` or another error, the form shows the presentation's copy. It
   never shows IAM message text.

### 5.3 Revoke

1. The admin presses Revoke on a holder row. The form carries the principal PRN and the grant id.
2. The command checks that the grant id is a `gateway_user` grant of that principal at this org
   (§4.4). A crafted form cannot revoke another role through this section.
3. On success, the page revalidates.
4. On `not-found` (another admin revoked first, or the check in step 2 found no grant), the page
   revalidates and shows no error: the grant is gone, which is what the admin wanted.
5. On `denied` or another error, the form shows the presentation's copy.

## 6. Security

- IAM is the only gate. `mayI` only hides controls.
- The scope path authorizes at the scope node (D4 b) before it reads anything. Under Cedar
  `resource in ?resource`, an `org_admin` is allowed at its own org and at no other org. A
  `team_admin` is not allowed at its parent org. §7.1 proves this against the real policy set.
- A forged team PRN matches no stored row (D5), and Cedar decides against the stored ancestry.
- The kind filter runs in SQL. The console never guesses a kind from a PRN.
- Grant: the role key is a server constant, so a crafted form cannot grant a different role.
  The action does not check that the principal is a candidate. D12 is a UI choice, not a
  security boundary: IAM already lets an `org_admin` grant to any principal with a direct call.
- Revoke: the command bounds the revoke to `gateway_user` at this org (§5.3 step 2).
- The idempotent grant (D9) runs its pre-check after the authorization check, so a caller
  without `GrantRole` at the scope learns nothing about existing grants.

## 7. Testing

### 7.1 Rust

- `RoleGrantFilter`: no principal and no scope gives `None`; a filter with only `role_key` gives
  `None`.
- `RoleService::list` with the fakes: self (no check), scope path (check at the scope node),
  other principal without scope (check at Root), Root sentinel as scope, a deny on each path,
  the D3 error with its field name, an unknown kind refused (D7), `Page` bounds on the scope path.
- `RoleService::grant` idempotency: an existing grant returns it with no event, no audit row and
  no bump; the `DuplicateGrant` race path returns the winner's grant.
- **Real Cedar**: new `ListRoleGrants` rows in `starter_policy_table`
  (`paigasus-iam-core/src/authz/roles.rs:476`): `org_admin` at its own org allows; at another
  org denies; `team_admin` at its parent org denies; `org_member` at its org denies.
- Docker-gated (`tests/authz_role_grants.rs` and neighbours):
  - the kind join, the exact scope match, the Root case, and the stable order with paging;
  - a duplicate grant returns the existing grant (the asserts at `:232` and `:517` change from
    `AuthzError::Backend`);
  - a team `scope_prn` with a forged org slot is denied, next to
    `tests/authz_forged_org_slot_escalation.rs`;
  - end to end: an `org_admin` grants `gateway_user`, `IsAuthorized(InvokeModel, org)` allows,
    the admin revokes, and the check denies;
  - the `ListMemberships` kind filter.
- gRPC and HTTP: the new fields parse, the D3 error, the D7 refusal and the HTTP kind strings.

### 7.2 TS unit and integration

- Unit (`tests/unit/`): the block in each state (disabled, denied, error, holders and
  candidates, empty, read-only, each truncation line, the not-a-member mark and its hiding).
- Integration (`tests/integration/`, with `fake-iam.ts`): the loader join including the org
  creator as a candidate, the paging and the N+1 probe, both commands, the revoke bound check,
  and `not-found` as success.
- `fake-iam.ts` refuses `limit > 200` on `listMemberships` and on the scope path of
  `listRoleGrants`, as IAM does. `fake-iam.ts` and `dev-world.ts` learn the new request fields.

### 7.3 e2e

One Playwright row in `tests/e2e/`, with **one identity**: the signed-in admin is the org
creator in the world. The admin grants model access to themself, the playground answers, the
admin revokes, and the playground refuses. The world's `isAuthorized(InvokeModel)` self branch
reads the recorded grants, as its service-account branch does (`support/world.ts:155`). The
fake IAM is not Cedar (`playground-authz.spec.ts:3`), so this row proves the wiring only; the
Docker test in §7.1 proves the decision. Register the row in `tests/unit/e2e-rows.test.ts` and in
the README row list.

### 7.4 Tests and contracts that change on purpose

| Item | Change |
|---|---|
| `tests/e2e/call-count.spec.ts` R19/R20 | The org page adds calls. Derive the new formula per SMA-636 §7.3 before the measurement. |
| `tests/unit/actions-structure.test.ts:27` | Add the new `actions.ts`. |
| `ts/packages/paigasus-console-core` `action-names` test, iam-console e2e `ALL_ACTIONS` | Add `RevokeRole`. |
| `rs/.../tests/authz_role_grants.rs:232,517` | Duplicate grant: `Backend` → returns the existing grant. |
| `support/world.ts:163-165`, `service-account-commands.test.ts:143`, `actions.test.ts:217`, `service-account-section.test.tsx:159` | They model a duplicate as an error. They change to the D9 success, or they keep testing the `generic` copy with a non-duplicate failure. |
| `service-accounts/revalidation.ts:24`, `service-account-frame.tsx:51-52,76` | The comment "a `generic` allow can be a duplicate grant" is no longer true. The copy stays for real `generic` errors. |

## 8. Follow-ups

- A user lookup RPC in IAM, so the section can show names and emails.
- Paging for the principal-only `ListRoleGrants` path.

## 9. Recorded limits

- Each list stops at 1000 rows (§4.4 paging).
- A person with `gateway_user` only at a team or project does not show. They also have no
  playground access (SMA-635 D4).
- The holders list is not the full set of people who can call models. A `platform_admin`, or a
  custom Cedar policy, can give `InvokeModel` with no `gateway_user` grant at the org.
- A revoke is not immediate on every IAM replica. A replica with the memory cache can allow
  `InvokeModel` for about 31 s after the revoke (§3). The same lag applies after a grant: a person
  just granted `gateway_user` can still see "You need the gateway_user role" in the playground for
  up to about 31 s.

## 10. Rollout and version skew

- IAM and the console release together (version lockstep). Deploy IAM first.
- A new console against an old IAM: the old IAM needs `principal_prn`, so the section shows its
  error state. No data is at risk.
- An old IAM ignores the unknown filter fields. A caller that sends `principal_prn` **and** a
  filter gets an unfiltered list with no signal. No caller in this repo does that; the console
  scope path sends no `principal_prn` except in the revoke bound check, where an unfiltered
  answer still contains only that principal's grants and the id check stays correct.

## 11. Non-functional facts

- No new metric, so `repo:observability-drift` is not affected. The alert at
  `ops/observability/prometheus/rules/iam.rules.yml:208` counts non-OK statuses; after D9 a
  duplicate grant is OK, so that alert fires less.
- No new audit event. Grant and revoke already record `RoleGranted` / `RoleRevoked` with an audit
  row. A list call is not audited. The idempotent grant records nothing.
- No new Moon project, so CODEOWNERS does not change.
- Docs to update: `application/roles.rs:5-8,307-312` (the exposure rule),
  `pg_role_grants.rs:186-190` (the duplicate), `ts/apps/gateway-console/README.md:86` (D10 "out of
  band"), `app/_components/playground.tsx:18`.

## 12. Change log

- 2026-09-26: first draft after brainstorming.
- 2026-09-26, revision 2, after the adversarial challenge (verdict NEEDS REWORK):
  - Fixed the BLOCKER: `ListMemberships` rejects a limit over 200; the loader pages at 200 (§4.4).
  - Corrected §3: a revoke of a missing grant is `NotFound`; §5.3 treats it as success.
  - D9 (new): idempotent `GrantRole`, Sven's choice, instead of a conflict error.
  - D12 (changed): candidates include users with any org-scope grant, so the org creator appears.
  - D8 (new): a kind filter on `ListMemberships`, Sven's choice.
  - D6 (new): paging on the scope path only. D7 (new): refuse an unknown kind.
  - D15 (changed): no call when the cedar capability is off.
  - §4.2: a separate read port `RoleGrantQuery`, not a new method on `RoleGrantStore`.
  - §4.4 and §5.3: the revoke is bounded to `gateway_user` at this org.
  - §7: real-Cedar tests, a Docker end-to-end grant/revoke test, a one-identity e2e row, and the
    list of tests that change on purpose.
  - New §10 rollout and §11 non-functional facts; citation fixes.
- 2026-09-26, implementation: the new error reason `ERROR_REASON_INVALID_PRINCIPAL_KIND = 40`
  (`invalid-principal-kind`) implements D7's refusal. The e2e world's grant-reading `InvokeModel`
  self branch is opt-in through an `orgCreator` option. Other e2e rows keep their own world.
