# SMA-646 — CreateServiceAccount and IssueApiKey must not echo a forged node PRN

**Status:** APPROVED by Sven on 2026-09-28, with the decisions in §0. Challenged once (APPROVE WITH
CHANGES, folded — see the changelog). Re-checked against `origin/main` at 906de46c (SMA-649 merged).
**Issue:** [SMA-646](https://linear.app/smaschek/issue/SMA-646/rs-iam-createserviceaccount-echoes-a-forged-owner-slot-in-its-response)
**Related:** SMA-643 (Rename/Archive/Restore), SMA-645 (tenancy Create/List parent PRN), SMA-649
(principal-PRN confirmation for ListMemberships, GrantRole and ListRoleGrants, merged as 906de46c),
SMA-606 D2 (never echo the caller's own input), SMA-444 (`RoleService::resolve_scope`)
**Scope:** `rs/crates/services/paigasus-iam/src/application/service_accounts.rs`,
`application/api_keys.rs`, one new shared helper module, and the wiring. Both transports (HTTP and
gRPC) use these application services, so one change fixes both.

---

## 0. Approval decisions (2026-09-28)

Sven gave these decisions through the coordinator. They override the defaults of the draft.

| # | decision | effect on this spec |
|---|---|---|
| A1 | SMA-649 is merged on `main` (906de46c). Rebase the design on its real code and reuse its helpers and patterns where they fit. | §2.1 and §3 use the SMA-649 lookup shape (`find(uuid)` → `ok_or(TenancyError::NotFound)` → compare `canonical()`). Line numbers are re-taken on 906de46c (changelog, round 2). |
| A2 | List stays in scope (CreateServiceAccount and ListServiceAccounts). | §8 Q1 answered: yes. |
| A3 | An out-of-tree client that sends a forged owner slot now gets `prn-mismatch`: accepted. | §8 Q2 answered: refuse. |
| A4 | `not-found` for an ungranted caller (the load-first existence oracle) is accepted, as on the tenancy surfaces. | §8 Q4 answered: accepted. |
| A5 | CHANGED: fold `IssueApiKey`'s forged scope echo into this PR. Do not file a separate issue. Confirm its scope PRN against storage with the same order and semantics, with service and gRPC/HTTP transport tests and compiling mutations. | §8 Q3 answered: fold in. New §1.4, §2.5, §3.2, K-rows, U9–U13, I3–I4, m7–m9, AC 11–16. |
| A6 | Mutation runs are authorized by Sven. | §4.3 runs as written. |

The fold-in (A5) makes the helper of §2.2 have two callers in two modules. So the helper moves to one
shared crate module (§2.1 approach A, revised). `RoleService` stays unchanged (§2.1 D).

---

## 1. The defect

### 1.1 What happens today

`ServiceAccountService::create` (`application/service_accounts.rs:147-175`) takes the owner as a
`TenancyNodeRef` that the transport parsed from the caller's `owner_prn`
(`adapters/grpc/service_accounts.rs:112`, `adapters/http/service_accounts.rs:71`). It builds the
`ServiceAccount` with that owner, writes it, and returns it. It never compares the owner with a
stored node.

The row stores only the owner's bare uuid (`pg_service_accounts.rs::owner_columns`, line 56). A
later `Get` rebuilds the owner from the stored team or project row (`owner_from_columns`, lines
80-106). So the two reads disagree:

| call | `owner_prn` in the answer |
|---|---|
| `CreateServiceAccount` with a forged slot | the forged PRN, as the caller sent it |
| `GetServiceAccount` for the same account | the stored PRN |

`ListServiceAccounts` has the same echo, and the issue does not name it.
`PgServiceAccountRepository::list_by_owner` (starts at line 199) filters on the owner uuid only, and
"reuses the filtered-on `owner`" for each row (the comment at lines 216-218). A forged owner PRN on
a List therefore returns the real owner's accounts, each one labelled with the forged PRN.

### 1.2 What a caller can forge

`TenancyNodeRef::from_prn` (`paigasus-iam-core/src/tenancy.rs:147`, through `check` at line 66)
checks the service, the resource type and that the org slot is present or absent as the type
requires. It does not check the org slot VALUE, and it does not check the region. So:

| node type | forgeable fields |
|---|---|
| organization (`prn:pgs:iam:::organization/<uuid>`) | region only (an org slot is already `invalid-prn`) |
| team, project (`prn:pgs:iam::<org>:team/<uuid>`) | the org slot and the region |

A syntactically valid region such as `eu-west-1` passes `Prn::parse`; `EU-WEST-1` does not (SMA-645
§1.1 measured this).

The uuid CASE is not forgeable. `Prn` stores `org` and `resource_id` as `Uuid`, and `Prn::parse`
normalises them (`paigasus-kernel/src/resource_name.rs:12-21`). Two `Prn` values that differ only
in uuid case are `==` and have the same `canonical()`.

### 1.3 What this is, and is not (service accounts)

It is **not** an authorization bypass. `Authorize::check` receives the forged PRN today, but the
Cedar entity uid is only the type and the uuid (`paigasus-kernel/src/cedar.rs:18-23`), and
`PgEntitySliceLoader` takes the team's parent organization from the stored row, not from the
caller's PRN (`adapters/persistence/pg_entity_slice.rs`, the SMA-444 comment near line 63). The
decision for a forged PRN is the same as the decision for the correct PRN. The row itself is
written with the correct owner uuid.

It is a correctness defect with one visible effect today. The gateway console's
`createServiceAccount` command (`ts/apps/gateway-console/app/(console)/service-accounts/commands.ts:66-71`)
takes `account.ownerPrn` FROM THE CREATE RESPONSE and sends it as the `scopePrn` of a
`GrantRole`. `RoleService::grant` compares that scope with the stored node (`resolve_scope`,
`roles.rs:194-214`) and answers `prn-mismatch`. So a forged `ownerPrn` on that form creates the
account and then fails the grant: the console shows a `partial` result for an account that exists
without its role.

A second effect: `CedarAuthorizer` writes `req.resource.canonical()` into the decision audit
(`cedar_authorizer.rs:241-245`). Today that is the forged PRN. §2.2 removes this for this service.

### 1.4 The same echo on `IssueApiKey` (folded in, A5)

`ApiKeyService::issue` (`application/api_keys.rs:204-282`) takes the key's `scope` as a
`TenancyNodeRef` that the transport parsed from the caller's `scope_prn`
(`adapters/grpc/service_accounts.rs:179-180`, `adapters/http/api_keys.rs:88-89`). Today it:

- finds the service account (`NotFound` if absent);
- authorizes `IssueApiKey` at the SA's OWNER node, then `GrantRole` at every grant scope of the SA
  (D15);
- builds the `ApiKey` with the caller's `scope`, writes it, and writes an `iam.api_key.issued`
  outbox event whose payload carries `"scope": key.scope.canonical()` (line 260);
- returns the key with the caller's `scope`.

The row stores only the scope's bare uuid (`pg_api_keys.rs::scope_columns`, line 141), and a
later read rebuilds it from the stored node (`scope_from_columns`, line 162). So the forged scope
PRN reaches the Issue answer and the event stream, and a later `ListApiKeys` shows the stored PRN.
The audit entry does NOT carry the scope: its `detail` is only `key_id` and `prefix`, and its
`resource_prn` is the stored owner of the SA (lines 268-277). The draft (§7) said that the forged
scope reaches the audit log. That was wrong; this spec corrects it.

Two further facts, measured by reading:

- **The scope is never authorized today.** `issue` authorizes the SA's owner and the SA's grant
  scopes, but not the key's `scope` node. A caller who may issue keys for an SA can label a key
  with any existing node, in any organization. The FK on `scope_*_id`
  (`m0005_create_service_accounts_and_api_keys.rs:152-168`) only proves that the node exists.
- **An unknown scope uuid answers `not-found` today**, but late: the insert fails on the FK, and
  `persistence::map_err` maps `ForeignKeyConstraintViolation` to `RepositoryError::NotFound`
  (`adapters/persistence/mod.rs`, near line 62).

The gateway console issues keys with `scopePrn: account.value.ownerPrn`
(`commands.ts:101`), and that `ownerPrn` comes from an IAM read. So the console always sends a
stored PRN.

---

## 2. Decision

**Load the node by uuid, authorize against the STORED PRN, then compare the stored canonical with
the caller's canonical and refuse a mismatch with `prn-mismatch`.** Do this for the owner of
`ServiceAccountService::create` and `list`, and for the scope of `ApiKeyService::issue`, with one
shared helper. This is the order of the tenancy surfaces after SMA-643 and SMA-645
(`adapters/grpc/tenancy.rs:177-188`, `load_org_checked` and its twins): load → authorize against
the stored PRN → compare → warn on a mismatch.

The issue allows two outcomes: answer `prn-mismatch`, or return the stored PRN. This design refuses
a forged PRN, and nothing is written. That is the rule of every other tenancy surface after SMA-643
and SMA-645, of `RoleService::grant`, and of the SMA-649 principal guards. Sven accepted the effect
on out-of-tree clients (A3).

The write and the answer use the node that the load READ. After the compare, that node is `==` to
the caller's node (§1.2), so this has no effect that a test can see today. It is a code-review
item (AC 6), not a tested claim.

### 2.1 Approaches considered

**A. Application layer, one shared helper (recommended, revised for A5).** Add a crate module
`application/tenancy_nodes.rs` with a `TenancyNodes` value that holds `orgs`, `teams` and `projects`
(`Arc<dyn …Repository>`), and one method that loads the node by uuid through the matching
repository's `find`, authorizes against the stored PRN, compares `canonical()` strings, logs one
warning on a mismatch, and returns the stored `TenancyNodeRef`. `ServiceAccountServiceDeps` and
`ApiKeyServiceDeps` each get one `nodes: TenancyNodes` field.

- The lookup is the SMA-649 shape of `RoleService::resolve_scope` (`roles.rs:194-214`):
  `find(uuid).await?.ok_or(TenancyError::NotFound)?`, then `view.node.id.canonical()`. It uses the
  same three repositories.
- One helper covers Create, List and Issue. With two calling modules, a private copy per module
  would duplicate the order rule, and a later fix could reach one copy only.
- The unit tests run against the existing in-memory fakes (`InMemoryOrgs`, `InMemoryTeams`,
  `InMemoryProjects` over a `TenancyStore`, `application/fakes.rs:38, 60, 219, 341`), with no
  Docker.
- No port changes (`ServiceAccountRepository`, `ApiKeyRepository`).
- One `nodes` field, not three, keeps the helper under clippy's `too_many_arguments` limit and
  keeps the three handles together at every constructor.

**B. Persistence layer, in the write transaction.** Make `create_in` (and `issue_in`) load the node
row and compare its `prn` column, like `PgMembershipRepository::attach_in`
(`pg_memberships.rs:219`). Rejected: it changes the port signatures in `paigasus-iam-core`, it needs
the same logic in each Pg adapter and each in-memory fake, and it needs a separate change for
`list_by_owner`. The in-transaction lock that B gives is not needed: a node's stored PRN is written
once and nothing moves a node to another parent (SMA-645 §3.1 F1).

**C. Only return the stored PRN (re-read after commit).** Rejected: the create still succeeds for a
request that names the wrong organization, which SMA-645 §2.1 rejected for the tenancy RPCs for the
same reason.

**D. Move `RoleService` onto the shared helper too.** Rejected for this change.
`RoleService::resolve_scope` uses the other order (authorize against the caller's PRN, then
compare), takes a `GrantScope`, and logs nothing (SMA-649 kept that, its Q2). Moving it changes
`RoleService` behaviour: its decision-audit PRN and its unknown-scope answer. That is a separate
change. §7 records it as a follow-up.

### 2.2 The helper, and the order of operations

`TenancyNodes::resolve_and_authorize(&self, authorize: &Authorize, actor: &Prn, action: Action,
claimed: &TenancyNodeRef, operation: &'static str) -> Result<TenancyNodeRef, TenancyError>`:

1. `find(claimed uuid)` on the matching repository → `TenancyError::NotFound` if absent.
2. `authorize.check(actor, action, <stored node>.prn())` — against the STORED PRN, not the
   caller's.
3. Compare `claimed.canonical()` with the stored canonical. On a difference: one
   `tracing::warn!` (§3.1), then `Err(TenancyError::PrnMismatch)`.
4. Return the stored `TenancyNodeRef`.

`create(actor, owner, name)`:

1. `let owner = self.nodes.resolve_and_authorize(&self.authorize, actor, Action::CreateServiceAccount, &owner, "CreateServiceAccount").await?;`
   This replaces today's `authorize.check(…, owner_resource_prn(&owner))`.
2. `ServiceAccount::new(id, owner, name, now)` — name validation, unchanged.
3. The unit of work, unchanged.
4. Return the record built from the resolved `owner`.

`list(actor, owner, page)`:

1. `let owner = self.nodes.resolve_and_authorize(&self.authorize, actor, Action::ListServiceAccounts, owner, "ListServiceAccounts").await?;`
2. `repo.list_by_owner(&owner, …)` with the RESOLVED owner.

`issue(actor, sa_id, scope, …)` — §2.5.

**Authorize before compare is load-bearing** (SMA-645 §3): if the comparison ran first, the
difference between `prn-mismatch` and `forbidden` would tell an ungranted caller which organization
owns a node.

**Load before authorize is a decision, with one cost (A4, accepted).** The gain: the authz decision
audit, the trace and the decision-cache key carry the stored PRN, not the forged one (§1.3), and an
unknown node answers `not-found` like the tenancy surfaces (SMA-645 B5). The cost: an ungranted
caller learns whether a node uuid exists (`not-found` against `forbidden`). The tenancy surfaces
already accept this cost (`adapters/grpc/tenancy.rs:170-176`, "The load is UNCONDITIONAL"). Sven
accepted the same cost here.

**Resolve before `ServiceAccount::new` is a decision.** A forged owner together with an invalid
name answers `prn-mismatch`, not `invalid-name`. That is the SMA-645 B2 precedence: reject a request
that names the wrong target before judging its contents.

**Pagination stays first: an accepted divergence from SMA-645 B4.** The `Page` for List is built
in the transport before the service call (HTTP `adapters/http/service_accounts.rs:80`, gRPC
`adapters/grpc/service_accounts.rs:145`). So a forged owner with an out-of-range `limit` still
answers `invalid-pagination`. SMA-645 B4 chose `prn-mismatch` over `invalid-pagination` for the
tenancy Lists. This spec does not, on purpose: moving `Page::new` into the service touches both
transports for a precedence case that has no user. The implementer must not change this placement.

### 2.3 Behaviour changes, case by case (service accounts)

`enforce_tenancy` does not apply here: this service always authorizes (module doc of
`adapters/grpc/service_accounts.rs`, lines 15-20).

| # | request | today | after |
|---|---|---|---|
| B1 | Create, forged org slot or region, granted caller, all else valid | OK, answer echoes the forged PRN | `prn-mismatch`, no row, no outbox event, one warning line |
| B2 | List, forged owner PRN, granted caller | OK, real accounts labelled with the forged PRN | `prn-mismatch`, one warning line |
| B3 | Create, forged owner + invalid name, granted caller | `invalid-name` | `prn-mismatch` |
| B4 | Create or List, correct PRN with upper-case uuids | OK | OK — no change (`Prn::parse` normalises uuid case, §1.2) |
| B5 | Create or List, unknown owner uuid, any caller | `forbidden` | `not-found` |
| B6 | ungranted caller, existing owner, forged or correct PRN | `forbidden` | `forbidden` — no change |
| B7 | List, forged owner + out-of-range `limit` | `invalid-pagination` | `invalid-pagination` — no change (§2.2) |

**B5, by reading.** Today step 1 is `authorize.check`. For an unknown node the entity-slice loader
returns `AuthzError::ResourceNotFound` (`pg_entity_slice.rs:192-198`), and
`CedarAuthorizer::is_authorized` catches it and returns a fail-closed `Deny`
(`cedar_authorizer.rs:228-236`). So today's answer is `forbidden`. After the change, the `find`
runs first and answers `not-found`. This matches SMA-645 B5 for the tenancy surfaces. I1 pins it.

### 2.4 The stored node and the read answers agree

AC 3 says that the Create answer equals a following Get answer. Two sources give the owner, and
they must agree:

- The helper takes the node from its `prn` column (`pg_teams.rs:73-74`, `pg_projects.rs:75-76`,
  `pg_organizations.rs:143-144`).
- `Get` builds the owner from `(org_id, uuid)` with an empty region
  (`pg_service_accounts.rs:80-106`, `owner_from_columns`). `ListApiKeys` builds the key scope the
  same way (`pg_api_keys.rs:162`, `scope_from_columns`).

They agree while `prn == from_parts(org_id, id).canonical()` holds for every node row. SMA-645
§3.1 F1 states this invariant: the `prn` column is written once from `canonical()`, which always
emits an empty region, and no code moves a node to another parent. The doc comment of
`load_org_checked` states the same invariant (`adapters/grpc/tenancy.rs:165-168`). A future "move a
node" feature breaks it and must revisit this helper. The helper's doc comment cites F1.

### 2.5 `IssueApiKey` (A5)

`issue(actor, sa_id, scope, expires_at, scope_actions, scope_roles)`, new order:

1. `service_accounts.find(sa_id)` → `NotFound` if absent. Unchanged.
2. `authorize.check(actor, IssueApiKey, owner_resource_prn(&sa.account.owner))`. Unchanged. The
   owner comes from the stored SA row, so it has no forgeable slot.
3. D15: `GrantRole` at every grant scope of the SA. Unchanged.
4. **New:** `let scope = self.nodes.resolve_and_authorize(&self.authorize, actor, Action::IssueApiKey, &scope, "IssueApiKey").await?;`
5. Mint the id and the secret, build the `ApiKey` with the RESOLVED `scope`, and run the unit of
   work. Unchanged apart from the binding.

**Step 4 authorizes `IssueApiKey` at the stored scope node. This is new behaviour, and it is a
decision of this spec (§8 Q5).** "The same order and semantics" (A5) is load → authorize against
the stored PRN → compare. Today there is no authorize step for the scope, so the helper adds one.
The reason is the same as "authorize before compare is load-bearing" (§2.2): without it, any
caller who may issue a key for SOME service account could send a team or project uuid of ANY
organization with a guessed org slot, and read `prn-mismatch` against OK as an answer to "which
organization owns this node". Step 4 closes that oracle, and it also stops a caller from labelling
a key with a node outside their authority (§1.4, K4).

The console and every in-repo test that issues a key send the SA's owner as the scope
(`commands.ts:101`; `tests/api_keys_grpc.rs:101, 122, 190, 218, 301`). A caller who holds
`IssueApiKey` at the owner therefore passes step 4 for the same node. Cedar evaluates
`resource in ?resource`, so a caller granted at an organization also passes for a team or project
inside that organization.

**Step 4 runs after D15, before anything is minted.** A forged scope from a caller who fails step
2 or 3 answers `forbidden`, as today. A forged scope from a caller who passes both answers
`prn-mismatch`, and no id is minted and no secret generated.

| # | request | today | after |
|---|---|---|---|
| K1 | Issue, forged scope org slot or region, caller granted at owner and at the stored scope | OK; the answer and the `iam.api_key.issued` payload carry the forged PRN | `prn-mismatch`, no `api_key` row, no outbox event, no audit row, one warning line |
| K2 | Issue, correct scope PRN with upper-case uuids | OK | OK — no change |
| K3 | Issue, unknown scope uuid, caller granted at owner | `not-found` (late, from the FK) | `not-found` (early, from the `find`) — no visible change |
| K4 | Issue, existing scope node where the caller has no `IssueApiKey`, caller granted at owner | OK — the key is labelled with that node | `forbidden`, for a forged and a correct scope PRN alike |
| K5 | Issue, caller not granted at the SA owner | `forbidden` | `forbidden` — no change |
| K6 | Issue, unknown service account | `not-found` | `not-found` — no change |

### 2.6 No existing caller breaks

- **gateway-console** is the only non-generated client (`ts/apps/gateway-console/app/(console)/service-accounts/`).
  It sends the `ownerPrn` of the page it shows, which comes from IAM reads, and it issues keys with
  that same `ownerPrn` as the scope (`commands.ts:101`). A correct PRN is unchanged (B4, K2). A
  forged owner now fails at Create instead of at the follow-up `GrantRole`, which removes the
  `partial` state of §1.3.
- **iam-console** does not call these RPCs. **py/** has only generated stubs.
- **Out-of-tree clients** (SDK users, automation): Sven accepted that a forged slot now answers
  `prn-mismatch` (A3). A client that scopes a key to a node outside the caller's `IssueApiKey`
  authority now answers `forbidden` (K4, §8 Q5).
- **Rust tests** that create a service account or issue a key through a service must own a real
  node whose stored `prn` is canonical. `support::seed_org_ref` writes `prn = id.canonical()`
  (`tests/support/mod.rs:796`), so the HTTP and gRPC suites keep working. The implementer checks
  `tests/authz_acceptance.rs:228` and `tests/api_key_auth.rs` (the `scope_prn` bodies at lines 387,
  499, 567): each issue there must name a scope where the issuing actor holds `IssueApiKey`
  (K4). The UNIT tests in `application/service_accounts.rs` use `owner_org(n)` for an organization
  that no fake store holds, and the unit tests in `application/api_keys.rs` use `test_scope()`
  (line 364) the same way. After this change those tests get `not-found`. They must seed the node
  in the `TenancyStore` first, and `FakeAuthorizer::allow` must name the STORED canonical (for
  `issue`: `IssueApiKey` at the stored scope too). This is expected churn, not a regression.

---

## 3. Implementation

### 3.1 `application/tenancy_nodes.rs` (new) and `application/service_accounts.rs`

- New module `application/tenancy_nodes.rs` (SPDX header; register it in `application/mod.rs`):
  - `#[derive(Clone)] pub struct TenancyNodes { pub orgs: Arc<dyn OrganizationRepository>, pub teams: Arc<dyn TeamRepository>, pub projects: Arc<dyn ProjectRepository> }`.
    Only `find` is used. No narrower read-only port exists, and `RoleService` takes the same three.
  - `pub(crate) async fn resolve_and_authorize(&self, authorize: &Authorize, actor: &Prn, action: Action, claimed: &TenancyNodeRef, operation: &'static str) -> Result<TenancyNodeRef, TenancyError>`,
    step by step as §2.2. Per arm: `find(uuid).await?` → `ok_or(TenancyError::NotFound)?` →
    `authorize.check` against `view.node.id.prn()` → compare `claimed.canonical()` with
    `view.node.id.canonical()` → on a difference, warn and `Err(TenancyError::PrnMismatch)` → else
    return `TenancyNodeRef::<Arm>(view.node.id)`.
  - On a mismatch, emit exactly one `tracing::warn!` with the same fields as `warn_prn_mismatch`
    (`adapters/grpc/tenancy.rs:223-225`): `rpc = operation`, `actor = actor.canonical()`,
    `requested_prn`, `stored_prn`. The reason: a refused request writes no row, no outbox event and
    no denial row, so the log line is the only trace of a probe with a forged PRN (SMA-643 D4).
    SMA-649 chose NO log line for its principal guards (its Q2), to match the node guard of the
    same RPC. Here the guarded value IS a tenancy node, so the node-RPC rule (`warn_prn_mismatch`)
    applies. The module doc states this difference.
- `application/service_accounts.rs`:
  - Add `nodes: TenancyNodes` to `ServiceAccountServiceDeps` and to the struct.
  - `create` and `list` call the helper as §2.2 says.
  - `owner_resource_prn` keeps callers: `get` and `archive` still authorize against the stored
    owner of the SA row. Keep it. (The draft said that it loses its last caller. That was wrong on
    both 83d446fc and 906de46c: `get` at line 182 and `archive` at line 207 call it.)
  - Update the module doc and the `create`/`list` doc comments: they state the rule, the order,
    the F1 invariant (§2.4), and cite SMA-646. The module doc line "Mirrors `RoleService`'s DI +
    authorize pattern (`application/roles.rs: 78-204`)" names a stale range; correct or drop it.
- Do not write any registry reason code as a quoted string literal in `tenancy_nodes.rs`,
  `service_accounts.rs` or `api_keys.rs`, their `#[cfg(test)]` modules included (SMA-645 §4.4:
  `repo:error-code-single-site` matches a quoted registry code anywhere in a file, comments
  included). Write `TenancyError::PrnMismatch` or the code in backticks. The unit tests assert on
  `TenancyError::PrnMismatch`, not on a string.

### 3.2 `application/api_keys.rs` (A5)

- Add `nodes: TenancyNodes` to `ApiKeyServiceDeps` and to the struct.
- In `issue`, add step 4 of §2.5 after the D15 loop and before `self.ids.new_api_key_id()`. Shadow
  `scope` with the resolved value, so `ApiKey { scope, … }` and the event payload use it.
- Update the module doc and the `issue` doc comment: the new step 4, its order, the new
  `IssueApiKey`-at-scope authorization (K4), and SMA-646.
- `node_resource_prn`, `owner_resource_prn` and `grant_scope_resource_prn` keep their callers.

### 3.3 `adapters/http/mod.rs` (the composition root)

`role_orgs`, `role_teams` and `role_projects` (`mod.rs:524-526`) are MOVED into `RoleServiceDeps`
at `mod.rs:542-544`, before the `ServiceAccountServiceDeps` literal (line 702) and the
`ApiKeyServiceDeps` literal (line 722).

- Rename the three handles to `tenancy_orgs`, `tenancy_teams`, `tenancy_projects`.
- Pass `.clone()` at `mod.rs:542-544`. Build one `TenancyNodes { orgs, teams, projects }` from the
  three handles, and pass a clone to `ServiceAccountServiceDeps` and to `ApiKeyServiceDeps`.
- Update the comment at `mod.rs:519-523`: the handles now serve `RoleService`,
  `ServiceAccountService` and `ApiKeyService`.

### 3.4 Other constructors

Every `ServiceAccountServiceDeps { … }` and `ApiKeyServiceDeps { … }` literal gets the `nodes`
field: `application/service_accounts.rs:276` and `:494`, `application/api_keys.rs:410` and `:793`
(unit tests), and `adapters/http/mod.rs:702` and `:722`. On 906de46c no constructor exists in
`tests/`; the implementer re-greps both names across the crate.

### 3.5 Not changed

- `ServiceAccountRepository`, `ApiKeyRepository` and their implementations.
- `ServiceAccountService::get` and `archive`, `ApiKeyService::revoke` and `list`: they take the
  SA's own principal PRN or a key id, and authorize against the stored owner of the SA row. A
  principal PRN has no forgeable parent slot (`adapters/grpc/service_accounts.rs:76-88`).
- The transports, including the `Page` placement (§2.2). The gRPC module doc (lines 1-29) needs no
  edit; if it claims anything about owner or scope validation, the implementer corrects it.
- `RoleService` (§2.1 D).

---

## 4. Tests

### 4.1 Unit tests (in-memory fakes)

Seed one organization, one team and one project in a `TenancyStore`. For each forged case use the
real node's uuid.

**`FakeAuthorizer` is not Cedar.** It keys on the full canonical PRN (`application/fakes.rs:855-869`),
and Cedar ignores the org slot and the region. After the change, the helper authorizes against the
STORED PRN, so each granted test calls `allow` with the STORED canonical only. It must NOT allow the
forged canonical: then a mutation that authorizes against the caller's PRN (m3) answers `Forbidden`
and reds U1. `InMemoryServiceAccounts::list_by_owner` filters on full `TenancyNodeRef` equality
(`fakes.rs:1078`), but Postgres filters on the uuid only. So only I1 and I2 reproduce the B2
relabelling.

**Log capture exists.** `crate::log_capture::capture_logs` (`src/log_capture.rs`, used by
`adapters/oidc/validator.rs:367` and `application/authenticate_token.rs:378`) captures `tracing`
output in a unit test. U8 and U13 use it. No new dependency.

`application/service_accounts.rs`:

- **U1 `create_refuses_a_forged_owner`** — cases: team with a wrong org slot, project with a wrong
  org slot, organization with region `eu-west-1`, team with region `eu-west-1`. Each answers
  `TenancyError::PrnMismatch`, the fake repo holds no account, and `FakeOutbox` holds no event.
- **U2 `create_returns_the_stored_owner`** — the correct PRN succeeds, and
  `record.account.owner.canonical()` equals the stored canonical.
- **U3 `list_refuses_a_forged_owner`** — the team and project forged cases from U1.
- **U4 `list_returns_accounts_for_the_correct_owner`** — the correct PRN returns the seeded
  accounts, and each owner canonical equals the stored one.
- **U5 `an_ungranted_caller_cannot_tell_a_forged_owner_from_a_correct_one`** — default-deny
  `FakeAuthorizer`; Create AND List answer `Forbidden` for the forged and the correct PRN alike.
  This proves the ORDER only. The "alike" property for real decisions rests on Cedar; I1 proves it.
- **U6 `a_forged_owner_outranks_an_invalid_name`** — forged team PRN plus a blank name answers
  `PrnMismatch`. Control: the correct PRN plus the same name answers the name error, so U6 cannot
  pass vacuously.
- **U7 `an_unknown_owner_is_not_found`** — Create and List with an unseeded uuid answer
  `NotFound`, for a default-deny authorizer too. Production reaches this arm (B5).
- **U8 `a_forged_owner_logs_one_warning`** — for one forged Create and one forged List, exactly one
  warn line with `requested_prn` and `stored_prn`, through `capture_logs`.
- Update the existing tests to seed their owner (§2.6).

`application/api_keys.rs` (A5). Each test seeds the SA with a stored owner, and the granted tests
`allow` `IssueApiKey` at the stored owner and at the STORED scope only:

- **U9 `issue_refuses_a_forged_scope`** — the four forged shapes of U1, applied to the scope. Each
  answers `TenancyError::PrnMismatch`; `InMemoryApiKeys` holds no key, `FakeOutbox` holds no event,
  and the fake audit log holds no entry.
- **U10 `issue_returns_the_stored_scope`** — the correct scope succeeds, and
  `new_key.key.scope.canonical()` and the event payload `scope` equal the stored canonical.
- **U11 `a_caller_without_issue_at_the_scope_cannot_tell_a_forged_scope_from_a_correct_one`** —
  the authorizer allows `IssueApiKey` at the SA owner, but not at the scope node (a different
  seeded team). Issue answers `Forbidden` for the forged and the correct scope PRN alike, and holds
  no key. This pins K4 and the order of the helper for this caller.
- **U12 `issue_with_an_unknown_scope_is_not_found`** — an unseeded scope uuid answers `NotFound`,
  and no key id is minted (the `SeqIds` counter or the key store shows nothing).
- **U13 `a_forged_scope_logs_one_warning`** — one forged Issue writes exactly one warn line with
  `rpc` `IssueApiKey`, `requested_prn` and `stored_prn`.
- Update the existing tests to seed their scope and allow `IssueApiKey` at it (§2.6).

### 4.2 Integration tests (Docker Postgres)

- **I1 gRPC, `tests/api_keys_grpc.rs`** — `a_forged_owner_prn_never_creates_a_service_account`:
  - A real team (the fixture note below).
  - `CreateServiceAccount` with a wrong org slot answers `InvalidArgument` with `ErrorInfo.reason`
    `prn-mismatch`.
  - Then `ListServiceAccounts` with the CORRECT PRN returns none of that name.
  - No `principal` row with that name exists.
  - The outbox count filtered by `event_type = iam.principal.created` and the payload `name` does
    not move. Copy the filter style of `outbox_count` in `tests/grpc_tenancy.rs:95`; do not use a
    total `event_outbox` count.
  - `ListServiceAccounts` with the forged PRN answers `prn-mismatch`.
  - An unknown team uuid answers `NOT_FOUND` for Create (B5).
  - Ungranted caller: a second identity with no grant gets `PermissionDenied` for the forged and
    the correct PRN alike, on Create and List. Copy the fixture of
    `an_ungranted_caller_cannot_tell_a_forged_prn_from_a_correct_one` (`tests/grpc_tenancy.rs:1178`).
  - Positive control in the same test: the correct PRN with the resource uuid and the org slot
    UPPER-CASED creates the account; the filtered outbox count moves by one; and the `owner_prn` in
    the Create answer equals the `owner_prn` in a following `GetServiceAccount` and in the List
    entry (the issue's "the two reads agree", AC 3).
- **I2 HTTP, `tests/http_service_accounts.rs`** — the same forged create over
  `POST /v1/service-accounts` answers 400 with the `prn-mismatch` envelope code, and a forged
  `GET /v1/service-accounts?owner_prn=…` answers the same. Before the fix, that GET answered 200
  with the forged PRN on each row (B2); I2 is the only test that reproduces that relabelling.
- **I3 gRPC, `tests/api_keys_grpc.rs`** — `a_forged_scope_prn_never_issues_an_api_key` (A5):
  - An SA owned by a real team, and an actor granted at that team's organization.
  - `IssueApiKey` with the team as scope and a wrong org slot answers `InvalidArgument` with
    `ErrorInfo.reason` `prn-mismatch`. Then no `api_key` row exists for the SA, the outbox count
    filtered by `event_type = iam.api_key.issued` and the SA's `aggregate_prn` does not move, and
    no `audit_log` row with `action = IssueApiKey` for the SA exists.
  - The same with region `eu-west-1` answers the same.
  - An unknown team uuid as scope answers `NOT_FOUND` (K3).
  - K4 against Cedar: a second SA owned by a team in a SECOND organization, and an actor granted
    only at the first organization's SA owner. Issuing a key for the first SA with a scope node in
    the second organization answers `PermissionDenied` for the forged and the correct scope PRN
    alike.
  - Positive control in the same test: the correct scope PRN with upper-case uuids issues the key;
    the filtered outbox count moves by one; the `scope_prn` of the Issue answer equals the
    `scope_prn` of the `ListApiKeys` entry for that key.
- **I4 HTTP, `tests/http_service_accounts.rs`** — the same forged scope over
  `POST /v1/service-accounts/{id}/api-keys` answers 400 with the `prn-mismatch` envelope code, and
  a following `GET /v1/service-accounts/{id}/api-keys` lists no key.

The integration fixtures need a TEAM node (an org-only fixture can forge only the region). Seed
the team through `PgTeamRepository::create` or a raw insert with `prn = TeamId::canonical()`,
following `tests/authz_entity_slice.rs:56-66`. API-key RPCs need API-key management enabled
(`require_apikey_management`, `adapters/grpc/service_accounts.rs`); copy the setup of the
existing tests in `tests/api_keys_grpc.rs`.

### 4.3 Proof that the tests bite

Mutation runs are authorized (A6). Restore each mutation by reverting the exact edit, not by
`git checkout --` (that also reverts the fix), and confirm with `git diff`. Never commit a
mutation. Each mutation must COMPILE: run with `--no-fail-fast`, and read a rustc error as "the
mutation proved nothing". Under `warnings = "deny"`, write a moved or discarded result as
`let _ = …;` so that an unused binding does not stop the build.

| # | mutation | expected to fail |
|---|---|---|
| m1 | in `create`, replace the helper call with today's `authorize.check` on the caller's owner | U1, U7, I1, I2 |
| m2 | in `list`, replace the helper call with today's `authorize.check` on the caller's owner | U3, U7, I1, I2 |
| m3 | in the helper, authorize against `claimed`'s PRN, not the stored PRN | U1, U9 (the fake allows the stored canonical only) |
| m4 | in the helper, move the compare above `authorize.check` | U5 (Create arm and List arm), U11 |
| m5 | in `create`, move the helper call below `ServiceAccount::new` | U6 |
| m6 | in the helper, delete the `tracing::warn!` | U8, U13 |
| m7 | in `issue`, delete the helper call (today's code) | U9, U11, U12, I3, I4 |
| m8 | in `issue`, move the helper call below `tx.commit()` (as `let _ = …;`) | U9 (a key and an event exist) |
| m9 | in the helper, delete the `authorize.check` step | U5, U11 |

The helper is shared, so m3, m4, m6 and m9 cover `create`, `list` and `issue` together. U5 has a
Create arm and a List arm, and U11 covers the Issue caller, so that a later split of the helper
keeps every caller covered.

**Equivalent mutants, recorded on purpose.** Three further mutations cannot red any test, and they
are NOT in AC 10:

- return `claimed.clone()` instead of the stored id, and keep the compare;
- in `list`, pass the caller's `owner` to `list_by_owner`, not the resolved one;
- in `issue`, build the `ApiKey` with the caller's `scope`, not the resolved one.

After the compare passes, the claimed and the stored `TenancyNodeRef` are `==` and have the same
canonical: `Prn` stores the uuids as `Uuid` and normalises case at parse
(`paigasus-kernel/src/resource_name.rs:12-21`). Using the stored value stays a code-review item
(AC 6).

Record which test caught each mutation. If the permission system refuses a mutation run, restore
the file, and record the exact manual mutation steps under "Mutation proof pending" in the PR body.

### 4.4 Local run

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
export PAIGASUS_REQUIRE_DOCKER=1
cd rs && cargo nextest run -p paigasus-iam --profile iam
```

Without `PAIGASUS_REQUIRE_DOCKER=1` an unreachable Docker daemon makes the Docker tests return
early and count as passed. Before the push, run the full gate graph as CLAUDE.md says;
`repo:error-code-single-site` is the gate this change can red (§3.1). On the development Mac, a
bash-5 gate can hang in the 512-byte small-pipe state (SMA-612); run such a gate in a Linux
container.

---

## 5. Acceptance criteria

1. `CreateServiceAccount` (gRPC) and `POST /v1/service-accounts` (HTTP) answer `prn-mismatch` to a
   granted caller for an existing owner whose stored canonical PRN differs from the request's
   canonical form, for a forged org slot and a forged region alike. No `service_account` row, no
   `principal` row and no `iam.principal.created` outbox event is written.
2. `ListServiceAccounts` and `GET /v1/service-accounts` answer `prn-mismatch` for the same inputs.
3. For a correct owner PRN, including upper-case uuids, Create succeeds and the `owner_prn` of the
   Create answer equals the `owner_prn` of a following Get and of the List entry.
4. An ungranted caller gets `forbidden` for a forged and a correct owner PRN of an existing node
   alike, on Create and List (I1, against Cedar).
5. A forged owner outranks an invalid name on Create. `invalid-pagination` still outranks a forged
   owner on List (B7).
6. Code review: the helper authorizes against the STORED PRN (tested, m3), and the records that
   `create`, `list` and `issue` return carry the node that the helper read (not testable, §4.3
   equivalent mutants).
7. An unknown owner uuid answers `not-found` on Create and List (B5).
8. Each refused mismatch emits exactly one `tracing::warn!` with the operation (`rpc`), the actor,
   `requested_prn` and `stored_prn`.
9. The module docs, the helper doc and the `create`/`list`/`issue` docs state the rule, the order
   and the F1 invariant. `tenancy_nodes.rs`, `service_accounts.rs` and `api_keys.rs`, their
   `#[cfg(test)]` modules included, spell no quoted registry reason code.
10. Every mutation in the §4.3 table reds at least one test.
11. `IssueApiKey` (gRPC) and `POST /v1/service-accounts/{id}/api-keys` (HTTP) answer `prn-mismatch`
    to a caller granted at the SA owner and at the stored scope, for a forged scope org slot and a
    forged scope region alike. No `api_key` row, no `iam.api_key.issued` outbox event and no
    `IssueApiKey` audit row is written (K1).
12. For a correct scope PRN, including upper-case uuids, Issue succeeds, and the `scope_prn` of the
    Issue answer and of the event payload equals the stored canonical and the `scope_prn` of the
    `ListApiKeys` entry (K2).
13. An unknown scope uuid answers `not-found` on Issue (K3).
14. A caller granted at the SA owner but without `IssueApiKey` at the stored scope node gets
    `forbidden` for a forged and a correct scope PRN alike (K4, I3 against Cedar).
15. The scope check runs after the D15 checks and before an id is minted: a caller who fails the
    owner or D15 check still gets `forbidden` for a forged scope (K5).
16. The existing `IssueApiKey` tests that scope a key to the SA's owner still pass (§2.6).

---

## 6. Files

| file | change |
|---|---|
| `rs/crates/services/paigasus-iam/src/application/tenancy_nodes.rs` | new: `TenancyNodes`, the shared helper, its warning, docs, unit tests of the helper if useful |
| `rs/crates/services/paigasus-iam/src/application/mod.rs` | register the new module |
| `rs/crates/services/paigasus-iam/src/application/service_accounts.rs` | `nodes` dep, `create`, `list`, docs, unit tests U1–U8 |
| `rs/crates/services/paigasus-iam/src/application/api_keys.rs` | `nodes` dep, `issue` step 4, docs, unit tests U9–U13 |
| `rs/crates/services/paigasus-iam/src/adapters/http/mod.rs` | rename and share the three repository handles, build one `TenancyNodes` |
| `rs/crates/services/paigasus-iam/tests/api_keys_grpc.rs` | I1, I3 |
| `rs/crates/services/paigasus-iam/tests/http_service_accounts.rs` | I2, I4 |
| `rs/crates/services/paigasus-iam/tests/support/mod.rs` | a team-seeding helper, only if none exists |
| `rs/crates/services/paigasus-iam/tests/authz_acceptance.rs`, `tests/api_key_auth.rs` | only if an existing issue names a scope outside the actor's `IssueApiKey` authority (§2.6) |

Estimated size: small to medium. No proto, TS, migration or port change.

---

## 7. Out of scope, and follow-ups

- **One shared tenancy-node resolver for `RoleService` too.** After this change the same lookup
  exists in `adapters/grpc/tenancy.rs` (`load_*_checked`), `RoleService::resolve_scope`, and the new
  `TenancyNodes` helper. Moving `RoleService::grant` and the tenancy handlers onto the helper
  removes the copies. It also moves `RoleService` to the stored-PRN order, which changes its
  decision-audit PRN and its unknown-scope answer. Recommend a separate issue.
- **The authz decision audit records the caller's resource PRN elsewhere.** This change removes the
  forged PRN from the decision audit for `CreateServiceAccount`, `ListServiceAccounts` and the new
  scope check of `IssueApiKey` only (§1.3). `RoleService::grant` and every other caller that
  authorizes a caller-supplied PRN still record it. Not changed here.
- (Removed: the draft's `IssueApiKey` follow-up. A5 folds it into this change, §1.4 and §2.5.)

---

## 8. Open questions

1. **List in scope?** ANSWERED (A2): yes. §2.2 List, U3, U4, the List arms of U5, U7 and I1/I2,
   m2, and acceptance criterion 2 stay.
2. **Refuse, or only return the stored PRN?** ANSWERED (A3): refuse. Sven accepted that an
   out-of-tree client that sends a forged owner slot now gets `prn-mismatch`.
3. **`IssueApiKey` now, or a separate issue?** ANSWERED (A5): fold it into this PR, with the same
   order and semantics, service and gRPC/HTTP transport tests, and compiling mutations. See §1.4,
   §2.5, §3.2, U9–U13, I3–I4, m7–m9 and AC 11–16. No separate issue is filed.
4. **The existence oracle of the load-first order.** ANSWERED (A4): accepted, as on the tenancy
   surfaces. It now applies to the scope of `IssueApiKey` too, for a caller who passes the owner
   and D15 checks.
5. **NEW (from A5), decided by this spec, for Sven to confirm at plan review: authorize
   `IssueApiKey` at the stored scope node.** "The same order and semantics" needs an authorize
   step between the load and the compare, and `issue` has none for the scope today. Without it, a
   caller who may issue keys for any SA gets an org-slot oracle for every team and project uuid in
   every organization. The cost: a caller granted at the SA owner but not at the scope node now
   gets `forbidden` (K4), where today the key is issued with a label outside that caller's
   authority. No in-repo caller does this (§2.6). The alternative is a compare with no authorize
   step, which keeps K4 as OK and keeps the oracle; this spec rejects it.

---

## Challenge changelog

**Round 1 verdict: APPROVE WITH CHANGES.** Each finding was checked against the code on `main`
(83d446fc).

**Folded:**

- BLOCKER, equivalent mutants m3/m6: confirmed (`resource_name.rs:12-21, 134, 143`). Removed them
  from the mutation table and from the ACs, recorded them as equivalent mutants in §4.3, made AC 6
  a code-review item, and removed the upper-case rationale from U2/U4. Upper-case inputs now appear
  only in I1's positive control, for AC 3.
- MAJOR, B5 is `forbidden`, not `internal`: confirmed (`cedar_authorizer.rs:228-236`). With the new
  order (next item) B5 becomes `not-found`. The wrong §7 item is deleted.
- MAJOR, no warning log: confirmed (`tenancy.rs:221-226`). Added the `tracing::warn!` to §3.1,
  U8, m6 and AC 8.
- MAJOR, order of operations: chose option (a), the SMA-643 order (load → authorize against the
  stored PRN → compare). The parity claim is now true, the forged PRN leaves the decision audit
  for this service, and the cost (an existence oracle) is recorded in §2.2 and §8 Q4. The
  authorize-against-stored step gives a new, non-equivalent mutation (m3).
- MINOR, the F1 invariant: added §2.4 and a doc-comment requirement.
- MINOR, wiring: confirmed that the handles are moved at `mod.rs:542-544`. §3.3 now renames them
  and clones at that site.
- MINOR, `FakeAuthorizer` is not Cedar: §4.1 now says that tests allow the stored canonical only,
  that U5 proves the order only, and that the "alike" property comes from an ungranted-caller case
  in I1. Noted that only I1/I2 reproduce B2.
- MINOR, m4-list: the helper is shared, so one mutation covers both; U5 has a Create and a List arm.
- MINOR, weak I1 assertions: filter the outbox by `event_type` and payload name, check the
  `principal` row, and send an upper-case uuid in the positive control.
- MINOR, archived-owner item in §7: confirmed wrong (`action.rs:210`, `roles.rs:311-315`). Deleted.
- MINOR, §2 point 2 overstates the event stream: confirmed (`service_accounts.rs:162`, no owner in
  the payload). Rewritten; only the response carries the owner.
- MINOR, AC 7 wording: rewritten as AC 9, scoped to the changed files and their test modules;
  unit tests assert on `TenancyError::PrnMismatch`.
- MINOR, pagination precedence: recorded as an accepted divergence from SMA-645 B4 (§2.2, B7,
  AC 5), with an explicit instruction not to move `Page::new`.

**Rejected:**

- MINOR, one shared `TenancyNodeResolver` used by `RoleService` now: rejected for this change,
  because it changes `RoleService`'s order and answers too. Recorded as approach D and a §7
  follow-up. (Round 2 shares the helper between the two NEW callers only.)
- MINOR, repositories that are too wide: rejected; no narrower read-only port exists, and
  `RoleService` injects the same three traits. Adding a port is out of proportion for a Low issue.
- QUESTION, `resolve_owner` returning `()`: moot. With the stored-PRN order the helper's return
  value feeds the authorize call, so it returns the stored `TenancyNodeRef`.

**Round 2 (setup, 2026-09-28): approval decisions and a re-check against `origin/main` at
906de46c (SMA-649 merged).**

Scope and decisions:

- Added §0 with Sven's decisions A1–A6. Marked §8 Q1–Q4 as answered.
- A5 folds `IssueApiKey` in: new §1.4, §2.5 (K1–K6), §3.2, U9–U13, I3–I4, m7–m9, AC 11–16. The
  title now names both RPCs.
- Because the helper now has two calling modules, it moves from a private method of
  `ServiceAccountService` to a shared `TenancyNodes` value in `application/tenancy_nodes.rs`
  (§2.1 A, §3.1). Both Deps structs get one `nodes` field instead of three repository fields.
- New decision §2.5 / §8 Q5: the scope check authorizes `IssueApiKey` at the stored scope, which
  is new behaviour (K4). Flagged for Sven at plan review.
- A1: the lookup reuses the SMA-649 shape of `RoleService::resolve_scope`. The warning line stays,
  and §3.1 records why it differs from SMA-649's no-log principal guards.

Stale facts corrected:

- §1.4 / old §7: the forged API-key scope does NOT reach the audit log. The audit `detail` is only
  `key_id` and `prefix`, and `resource_prn` is the stored SA owner (`api_keys.rs:268-277`). Only
  the Issue answer and the `iam.api_key.issued` payload carry it. The payload line is 260, not 258.
- Old §3.1: `owner_resource_prn` does NOT lose its last caller. `get` (line 182) and `archive`
  (line 207) still call it. The instruction to delete it is removed; deleting it would break the
  build.
- Old §4.1 U8 condition: a log-capture pattern exists (`src/log_capture.rs`, `capture_logs`). U8
  is no longer conditional, and m6 is a full row of the table.
- Warning field name: `warn_prn_mismatch` uses `rpc`, not an "operation" field
  (`adapters/grpc/tenancy.rs:223-225`).
- The gateway console also issues keys with the owner PRN as scope (`commands.ts:101`); added to
  §1.4 and §2.6.
- An unknown API-key scope already answers `not-found` today, through the FK and
  `persistence::map_err` (K3).

Line numbers re-taken on 906de46c:

| reference | draft | now |
|---|---|---|
| `adapters/grpc/tenancy.rs` load-checked order | 178-189 | 177-188 |
| `adapters/grpc/tenancy.rs` "UNCONDITIONAL" | 170-177 | 170-176 |
| `adapters/grpc/tenancy.rs` `warn_prn_mismatch` | 221-226 | 223-225 |
| `roles.rs` `resolve_scope` | 187-207 | 194-214 |
| `pg_memberships.rs` `attach_in` | 215-265 | starts at 219 |
| `fakes.rs` `FakeAuthorizer` | 809-817 | 855-869 |
| `fakes.rs` `InMemoryServiceAccounts::list_by_owner` | 1033 | 1078 |
| `adapters/http/mod.rs` `ServiceAccountServiceDeps` literal | "around 699" | 702 |
| `pg_entity_slice.rs` `ResourceNotFound` | 197-199 | 192-198 (helper doc and body) |
| `cedar_authorizer.rs` decision audit | 241-249 | 241-245 |
| `pg_service_accounts.rs` `list_by_owner` | 199-235 | starts at 199 |
| `commands.ts` create then grant | 66-75 | 66-71 |
| `api_keys.rs` event `"scope"` | near 258 | 260 |

Unchanged and confirmed: `service_accounts.rs:147` (`create`), `:189` (`list`), `:276` and `:494`
(test constructors); gRPC `service_accounts.rs:112` and `:145`; HTTP `service_accounts.rs:71` and
`:80`; `tenancy.rs:66` and `:147` in `paigasus-iam-core`; `cedar.rs:18-23`; `pg_teams.rs:73-74`;
`tests/support/mod.rs:796`; `tests/grpc_tenancy.rs:95` and `:1178`;
`tests/authz_entity_slice.rs:56-66`.
