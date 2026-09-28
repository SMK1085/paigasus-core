# SMA-649 — ListMemberships must confirm a principal PRN against storage

**Status:** approved by Sven on 2026-09-27, with changes (see §0). The adversarial challenge ran and its findings are folded in; see the Challenge changelog.
**Issue:** [SMA-649](https://linear.app/smaschek/issue/SMA-649/rs-iam-listmemberships-accepts-a-forged-principal-prn-region-or-org)
**Predecessors:** SMA-643 (Rename/Archive/Restore), SMA-645 (Create/List parent PRN). Same defect class.
**Scope:** `paigasus-iam-core` ports, `paigasus-iam` membership service, Postgres adapter, in-memory fake, two module docs, tests.
**Scope extended on 2026-09-27 (Q1):** `RoleService::grant` (`application/roles.rs`) also confirms the
principal PRN against storage (§4.7), with its own tests and mutations (T6, M5, M6).
**Scope extended on 2026-09-28 (Q6):** `RoleService::list` (`ListRoleGrants` with a principal
filter) also confirms the principal PRN against storage (§4.8), with its own tests and mutation
(T7, M7, AC15–AC19).

---

## 0. Approval decisions (Sven, 2026-09-27)

These decisions override the defaults that the draft proposed.

- **Q1 — CHANGED.** No follow-up issue for the role-grant path. The fix is part of THIS issue.
  `RoleService::grant` writes a forged principal PRN into the outbox `aggregate_prn` and into the
  response. It must confirm the principal PRN against storage, with the same `not-found` /
  `prn-mismatch` semantics, tests, and compiling mutations. See §4.7, T6, M5, M6, AC11–AC14.
- **Q2 — spec default.** No warning log on a principal `prn-mismatch`.
- **Q3 — spec default.** The rename of `MembershipKindQuery`, the removal of `list_by_node` and the
  rename of `list_by_principal` go to a follow-up issue (filed in Linear, see §9 Q3).
- **Q4 — accepted.** `not-found` for an unknown principal uuid (B4).
- **Q5 — accepted.** `prn-mismatch` (not `invalid-prn`) for a forged organization slot on a principal
  PRN (B2).
- **Q6 — ADDED on 2026-09-28.** Sven answered the question "ListRoleGrants with a principal filter
  probably has the same forged-PRN defect. What do you want?" with "Fold into SMA-649". So
  `RoleService::list` (`application/roles.rs:373-391`) confirms the principal PRN against storage
  too, with the same `not-found` / `prn-mismatch` semantics, tests on the service and on the gRPC and
  HTTP transports, and compiling mutations. See §4.8, T7, M7, AC15–AC19. The Q2, Q4 and Q5 answers
  apply to it unchanged.

---

## 1. The defect

`ListMemberships` has two filters: a node PRN and a principal PRN. Both transports (gRPC
`TenancyService.ListMemberships`, HTTP `GET /v1/memberships?principal=…`) call the same
`MembershipService::list` (`application/memberships.rs`).

For the **principal** filter:

1. `parse_principal_prn` (`application/memberships.rs`) checks only that the PRN parses, that the
   service is `iam`, and that the resource type is `principal`. It ignores the region and the
   organization slot.
2. `MembershipService::list` then keeps only the uuid: `MembershipAxis::Principal(parse_principal_prn(&raw)?.uuid())`.
3. The repository filters on that bare uuid: `list_by_principal` (kind `Any`) and
   `list_of_kind(MembershipAxis::Principal(uuid), …)` (a kind is set, SMA-676). Neither reads the
   `principal` row.

A stored principal PRN has an empty region and an empty organization slot
(`Prn::build("iam", "", None, "principal", uuid)`, so `prn:pgs:iam:::principal/<uuid>`). So
`prn:pgs:iam:eu-west-1::principal/<real-uuid>` and
`prn:pgs:iam::<any-org-uuid>:principal/<real-uuid>` both return the real principal's memberships,
with no error. This is true with and without `principal_kind`.

### 1.1 Why the node filter is not affected

The node filter is guarded in the REPOSITORY. `PgMembershipRepository::node_list_sql` loads the node
by uuid, answers `RepositoryError::NotFound` when it is absent, and answers
`RepositoryError::PrnMismatch` when the stored `prn` differs from `node.canonical()`. Both
`list_by_node` and the node arm of `list_of_kind` call it. `attach_in` does the same check for the
principal and for the node. The principal listing path has no equivalent guard at any layer.

### 1.2 What this is, and what it is not

It is **not** an authorization bypass. Under `enforce_tenancy = true` (the production default) a
principal-filtered listing authorizes `Action::ListMemberships` against `root_prn()` in both
transports. Only a caller with a platform-level grant gets past that step, and that caller may list
any principal's memberships anyway.

It is a correctness defect of the same class as SMA-643 and SMA-645. The server accepts a request
that names a principal PRN that does not exist, and answers for a different PRN. It also blocks one
sentence: the `adapters/grpc/tenancy.rs` module doc states the PRN-confirmation rule with this issue
as its one named exception (SMA-645 §4.3). This fix removes the exception.

---

## 2. Decision

**Compare, and answer `prn-mismatch`.** A principal-filtered `ListMemberships` loads the principal
by uuid, answers `not-found` when it is absent, and answers `prn-mismatch` when the stored PRN
differs from the canonical form of the supplied PRN. The comparison lives in the REPOSITORY, next to
the node guard, so both filters are confirmed in the same layer and by the same shape of code.

The alternative that the issue offers ("state why a principal PRN may carry a region and an
organization slot") is rejected. A principal is global: it has no organization, and no stored
principal PRN carries a region. The only possible justification is "the uuid is sufficient", and
SMA-645 §2.1 already rejected that argument for the node RPCs.

**Why `prn-mismatch` and not `invalid-prn` for an organization slot (B2).** The domain answers
`invalid-prn` for an org slot on an ORGANIZATION PRN (`tenancy.rs` `check(.., wants_org = false)`,
through `DomainError::InvalidNodePrn`). So the same fault ("an org slot on a global PRN") gets two
codes on two global resource types. This spec keeps `prn-mismatch` for the principal, for these
reasons:

- The issue text asks for exactly this: "Confirm the principal PRN against the stored principal,
  and answer `prn-mismatch` on a difference".
- The organization precedent is itself mixed. `check` does not look at the region, so a REGION on an
  organization PRN reaches the stored compare and answers `prn-mismatch` (SMA-645). Only the org slot
  answers `invalid-prn`, because `OrganizationId::from_prn` has a type-level shape rule.
- `PrincipalId::from_prn` has no shape rule. To add one in the parser hard-codes today's mint
  convention (approach D, §2.1) into the wire contract.
- One code for both principal faults (region and org slot) is simpler for a client than a split.

This is a judgement call. Sven accepted it (Q5, §0).

### 2.1 Where the guard goes — three approaches

**A. (chosen) Guard the wire listing port.** `MembershipKindQuery::list_of_kind` becomes the one
read that both transports use for every listing. Its `kind` parameter becomes
`Option<PrincipalKind>` (`None` = any kind). `MembershipAxis::Principal` carries a `PrincipalId`,
not a bare `Uuid`, so the full supplied PRN reaches the repository. The Postgres adapter gets a
private helper `principal_list_uuid(&PrincipalId)`, the twin of `node_list_sql`. The service's
`list` always calls `self.kinds.list_of_kind(&axis, kind, …)`.

- `MembershipRepository::list_by_principal(Uuid, …)` stays **unchanged and unguarded**. Its one
  production caller is `principal_context::load_all_memberships`, the authn introspection path. That
  caller holds a `PrincipalId` that the server resolved itself (from an external identity or an API
  key record), never a PRN from the wire. A guard there adds one SELECT per introspection page on
  the authn hot path and protects nothing. Its doc comment must say that it is for trusted callers
  only, and that the wire path goes through `MembershipKindQuery`.
- `MembershipKindQuery` has two implementations (Postgres, `fakes::InMemoryMemberships`).
  `MembershipRepository` has six. This is the reason SMA-676 D8 made the kind query a separate
  port, and it keeps this change small.
- `MembershipRepository::list_by_node` keeps its guard. After this change it has no production
  caller in the service. It stays, because removing a port method is outside this issue. Note: NO
  existing test pins the node guard of either `list_by_node` or `list_of_kind`. The
  `tests/tenancy_memberships.rs` calls of `list_by_node` (lines 325, 398, 444) use real nodes only,
  and no unit, repository or transport test lists memberships with a forged or unknown node PRN
  (the only node-PRN forgeries in `tests/` are the SMA-676 kind cases in `grpc_tenancy.rs` and the
  both-filters case in `http_memberships.rs`). Under `enforce_tenancy = true`, `resolve_node`
  authorizes against the REAL node, found by uuid, so the repository compare is the only defense
  against a forged node org slot on `ListMemberships`. This change moves the node listing (kind
  unset) from `list_by_node` onto the node arm of `list_of_kind`, so T2 adds node-arm guard cases
  and mutation M4 (§6).

**B. (rejected) Guard `MembershipRepository::list_by_principal` itself.** Change it to take
`&PrincipalId`. This touches six implementations, five of them test fakes, and puts an extra query
on the authn introspection path (§2.1 A, first bullet). The authn fakes do not seed principals, so
every one of them needs new setup code for a guard that authn does not need.

**C. (rejected) Guard in the application layer.** Give `MembershipService` a
`PrincipalRepository` and call `find_principal` before the list. This adds a dependency to the five
`MembershipServiceDeps` construction sites (`application/memberships.rs:275, 296, 556`,
`adapters/http/mod.rs:437`, `tests/tenancy_events_pg.rs:306`), and it splits the principal guard (service) from the
node guard (repository). A reader then has to look in two layers to learn one rule. SMA-645 §5
said the fix "belongs in the application layer". That sentence was written before anyone chose a
design. It names no reason that A does not meet: the service still owns the parse, and the
repository owns the comparison with stored data, exactly as for the node filter.

**D. (rejected as the fix) A structural check in `parse_principal_prn`.** Refuse a non-empty region
or org slot in the parser. Every mint confirms that a stored principal PRN has both empty: the two
mints in `adapters/id.rs:27, 55` are `Prn::build("iam", "", None, "principal", ..)`, and the
inserts write that minted id's `canonical()` (for example
`adapters/persistence/pg_service_accounts.rs:172`, `prn: Set(principal.id.canonical())`). D closes B1 and B2 with no port change, no
`Option<PrincipalKind>`, no fake change, no extra query and no reroute of the node path. The same few
lines would also close the copy in `application/roles.rs`. It is rejected as the fix of THIS issue
for these reasons:

- The issue asks to "confirm the principal PRN against the stored principal". D compares with a
  convention, not with storage.
- D does not give B4 (`not-found` for an unknown principal uuid). Without B4 the principal filter
  still answers an empty OK list for a PRN that names nothing, while the node filter of the same RPC
  answers `not-found`. The module-doc rule (§4.5) then still has an exception in substance.
- D encodes the mint convention in the wire contract. If a principal ever gets a region or an
  organization, D refuses its correct PRN, and nothing reds until a caller sees it. The stored
  compare stays correct in that case.
- D's error code would be `invalid-prn` (a parse fault) or a `prn-mismatch` that is raised without a
  read. The first splits the codes (§2 above); the second uses the code for something it does not
  mean elsewhere.

D is also rejected for the `roles.rs` grant path (§4.7): Sven asked for a confirmation against
storage there too (Q1, §0).

### 2.2 Behaviour changes, case by case

Rows apply to both transports and to both values of `principal_kind` (unset and set).

| # | request | today | after | `enforce_tenancy` |
|---|---|---|---|---|
| B1 | principal PRN with a non-empty region | the real principal's rows | `prn-mismatch` | both |
| B2 | principal PRN with a non-empty organization slot | the real principal's rows | `prn-mismatch` | both |
| B3 | correct principal PRN with an upper-case uuid | the rows | the rows — **no change** | both |
| B4 | **unknown** principal uuid | empty OK list | `not-found` | both (see below) |
| B5 | forged principal PRN + out-of-range `limit` | `invalid-pagination` | `invalid-pagination` — **no change** | both |
| B6 | caller without a root grant, forged or correct PRN | `permission-denied` | `permission-denied` — **no change** | `true` |
| B7 | forged PRN + unknown `principal_kind` value | `invalid-principal-kind` | `invalid-principal-kind` — **no change** | both |
| B8 | **unknown** principal uuid + out-of-range `limit` | `invalid-pagination` | `invalid-pagination` — **no change** | both |

**B4 is the one new answer for a correct-looking request.** It matches the node filter, which
already answers `not-found` for an unknown node, and it matches `attach_in`, which answers
`not-found` for an unknown principal. Under `enforce_tenancy = true` only a root-authorized caller
reaches the repository, so the new `not-found` tells nothing to a caller who could not already list
every principal. B6 is the reason: authorization against `root_prn()` runs in the handler BEFORE the
service call, so an ungranted caller gets `permission-denied` for a forged, correct or unknown PRN
alike, and learns nothing about which principals exist.

**B5, B7 and B8 do not change.** `convert::to_page` (gRPC) and `Page::new` (HTTP) run before
`MembershipService::list`, and `kind.resolve()` runs first inside `list`. The guard runs in the
repository, after both. This differs from SMA-645 row B4, where the comparison moved ahead of the
page check. That is deliberate: moving `to_page` would change a transport's order for a problem that
is not a transport problem. The node filter has the same order for a FORGED node PRN, which reaches
the repository compare after `to_page`. The two filters differ for an UNKNOWN uuid under
`enforce_tenancy = true`: `resolve_node` answers `not-found` for an unknown node in the handler,
before `to_page`, while an unknown principal with a bad `limit` answers `invalid-pagination` first
(B8), because the principal filter authorizes at `root_prn()` and loads nothing in the handler.

**No log line.** The node guard in `pg_memberships` logs nothing on `PrnMismatch`. The handler-level
`warn_prn_mismatch` belongs to the sixteen node RPCs in `tenancy.rs`. The principal guard follows the
node guard of the same RPC and does not log. Sven confirmed this (Q2, §0). The grant guard (§4.7)
does not log either, the same as `RoleService::resolve_scope`.

### 2.3 No existing caller breaks

- **TypeScript.** No console calls `listMemberships` with the `principalPrn` arm. The two call sites
  (`ts/apps/gateway-console/app/(console)/people-model-access/load.ts:80` and
  `ts/apps/iam-console/app/(console)/orgs/members.ts:16`) both use `nodePrn`.
- **Python.** `py/` has no `TenancyService` client.
- **Rust tests.** `tests/http_memberships.rs` passes `user_prn` returned by a real create, which is
  canonical. `application/memberships.rs` unit tests pass `seed_principal(...).canonical()`, and
  `seed_principal` also writes the principal into `store.principals`. The unit test that lists after
  a detach still lists an existing principal, so it still answers an empty OK list.
- **Stored PRN drift.** The stored `principal.prn` column is written once from `canonical()` at
  insert. `canonical()` renders the uuid lower-case with `as_hyphenated()`, so an upper-case uuid in
  a correct PRN still matches (B3). No code moves a principal to another organization or region.

---

## 3. Order of operations (per request, after the change)

1. Transport: parse the filter (`missing-required-field` / `mutually-exclusive-fields`).
2. Transport, if `enforce_tenancy`: authorize `ListMemberships` at `root_prn()` for the principal
   filter (unchanged).
3. Transport: `to_page` / `Page::new` (unchanged).
4. Service: `kind.resolve()` (`invalid-principal-kind`), then `parse_principal_prn` (`invalid-prn`
   for a malformed PRN or the wrong service or resource type).
5. Repository (`list_of_kind`, principal arm): load the `principal` row by uuid. Absent →
   `NotFound`. `stored_prn != principal.canonical()` → `PrnMismatch`.
6. Repository: run `LIST_BY_PRINCIPAL_SQL` with `$4` bound to the kind or NULL.

The load in step 5 is a plain read with no lock, the same as `node_list_sql`. A principal is never
archived or deleted in M1, and its PRN never changes, so the result cannot become stale between
steps 5 and 6.

---

## 4. Implementation

### 4.1 Port changes (`rs/crates/libs/paigasus-iam-core/src/ports.rs`)

- `MembershipAxis::Principal(Uuid)` → `MembershipAxis::Principal(PrincipalId)`.
- `MembershipKindQuery::list_of_kind(&self, axis, kind: PrincipalKind, …)` →
  `kind: Option<PrincipalKind>`. Doc: this is the wire listing read. Both axes confirm the supplied
  PRN against storage (`NotFound`, `PrnMismatch`). `None` means any kind.
- Keep the trait name `MembershipKindQuery` and the method name `list_of_kind`. A rename touches
  more lines than the fix. The rename goes to a follow-up issue (Q3, §0).
- `MembershipRepository::list_by_principal` doc: "Filters on a bare uuid and does not confirm a
  PRN. For callers that hold a server-resolved `PrincipalId` only (authn introspection). A PRN from
  the wire goes through `MembershipKindQuery::list_of_kind`."

### 4.2 Postgres adapter (`adapters/persistence/pg_memberships.rs`)

- Add `async fn principal_list_uuid(&self, principal: &PrincipalId) -> Result<Uuid, RepositoryError>`
  in the `impl PgMembershipRepository` block beside `node_list_sql`. It reads
  `principal::Entity::find_by_id(principal.uuid())`, maps absent to `NotFound`, and compares
  `model.prn` with `principal.canonical()`.
- `list_of_kind`: the principal arm calls the helper, then `list_rows(LIST_BY_PRINCIPAL_SQL, uuid, kind, …)`.
  The node arm is unchanged except that `kind` is now an `Option` (pass it through; `list_rows`
  already takes `Option<PrincipalKind>`).
- `list_by_principal` and `list_by_node` are unchanged.

### 4.3 In-memory fake (`application/fakes.rs`, `InMemoryMemberships`)

- `list_of_kind`: the principal arm looks up `store.principals` by uuid (`NotFound` if absent) and
  compares the stored string with `principal.canonical()` (`PrnMismatch`), exactly as `attach_in`
  in the same fake does. `kind == None` keeps every row.
- The fake stays faithful to the port doc. Update the fake's doc comment.

### 4.4 Service (`application/memberships.rs`)

- `MembershipFilter::Principal(raw)` → `MembershipAxis::Principal(parse_principal_prn(&raw)?)`.
- `list` becomes one call: `self.kinds.list_of_kind(&axis, kind, page.limit, page.offset).await?`.
  Remove the `match` and the "`Any` keeps the pre-SMA-676 repository path" sentence from its doc.
  Say instead that both axes go through the guarded port.
- `parse_principal_prn` doc: say that it checks only the syntax, the service and the type, and that
  the region and the organization slot are confirmed by the repository.
- **`application/roles.rs` `parse_principal_prn` doc.** It says that it "Mirrors
  `application::memberships::parse_principal_prn`". Change the doc to say that this copy checks only
  the syntax, the service and the type, and that `RoleService::grant` confirms the region and the
  organization slot against storage (§4.7). Task 4 of the plan first writes that `RoleService::list`
  does NOT confirm them; §4.8 then changes the sentence to say that `list` confirms them too. The
  code changes in `roles.rs` are in §4.7 and §4.8.

### 4.5 Documentation

- **`adapters/grpc/tenancy.rs` module doc**, paragraph "The rule, and its one exception": remove
  the exception. New text (write `TenancyError::PrnMismatch`, never the quoted wire code, per §4.6):

  > **The rule.** Every tenancy-NODE PRN this module accepts is confirmed against the stored node
  > before it is acted on: in the handler for the sixteen node RPCs (…unchanged…), and in the
  > REPOSITORY for the two membership RPCs that take a node PRN. `ListMemberships` with a PRINCIPAL
  > filter is confirmed in the repository too: `MembershipKindQuery::list_of_kind` loads the
  > principal by uuid and answers [`TenancyError::PrnMismatch`] on a difference (SMA-649).

  The paragraph to replace is `adapters/grpc/tenancy.rs:30-39`; its last sentence names SMA-649.
  **No change to the SMA-444 paragraph** (`adapters/grpc/tenancy.rs:41-61`). It is about the
  AUTHORIZATION resource, and for a principal-filtered `ListMemberships` that resource stays
  `root_prn()`. Its sentence about the node-filtered `ListMemberships` stays correct.
- **`adapters/http/memberships.rs` module doc**: add one sentence that the principal filter's PRN is
  confirmed against storage, and that an unknown principal answers `not-found` (B4).
- **`pg_memberships.rs`** module doc and `MembershipKindQuery` impl doc: name the principal guard.
- **No CLAUDE.md change.** No CLAUDE.md file names this rule.
- **No proto change.** Same reason as SMA-645 §5: a proto doc edit pulls in codegen drift for three
  bindings.

### 4.6 The error-code gate

`repo:error-code-single-site` (`ci/error-registry/check.py`) matches a registry code in quotes
anywhere in a file that is not in its `MANIFEST`, comments included. Keep the literal
`"prn-mismatch"` and `"not-found"` out of every `src/` file that this change edits. In tests, route
the expected code through `ErrorReason::…::as_wire_reason()` or `TenancyError::code()`, as
`adapters/grpc/tenancy.rs`'s own unit test does, or use the existing test helpers that already
compare the reason.

### 4.7 Role-grant path: `RoleService::grant` (scope extended, Q1)

**The defect (measured by the challenger, re-checked on `origin/main` 4051df5e).**
`RoleService::grant` (`application/roles.rs:238`) parses the principal PRN with its own
`parse_principal_prn` copy (`roles.rs:61-67`, line 239 is the call). That copy checks only the
syntax, the service and the type. `grant` then:

- keeps the parsed (forged) `PrincipalId` in the new `RoleGrant` (`roles.rs:257-264`);
- writes `grant.principal.canonical()`, the FORGED PRN, into the outbox event's `aggregate_prn`
  (`roles.rs:271`);
- returns that grant to the caller (`Ok(grant)`, `roles.rs:308`).

Only the uuid reaches the `role_grant` row. This is the SMA-606 D2 hazard that
`MembershipService::attach` already prevents. Also, `existing_grant` (`roles.rs:211-215`, SMA-676 D9)
runs with the forged `PrincipalId`, so a forged PRN for an existing grant answers OK today.

**The fix: confirm against storage in the application layer.** Add a private
`RoleService::resolve_principal(&self, principal: &PrincipalId) -> Result<(), TenancyError>`, the
twin of `resolve_scope` in the same file:

- `self.principals.find_principal(principal).await?` → `None` answers `TenancyError::NotFound`.
- The found principal's `id.canonical()` (the STORED PRN; `map_principal_row` in
  `adapters/persistence/pg_repository.rs:39-45` builds the id from the stored `prn` column) differs
  from `principal.canonical()` → `TenancyError::PrnMismatch`.

`grant` calls it right after `resolve_scope` (after the authorization check, and BEFORE
`existing_grant`). So the new step (7) runs only for an actor who holds `GrantRole` at the scope,
and a forged PRN can no longer reach the idempotent OK path.

This is the application layer, not the repository layer that §2.1 chose for `ListMemberships`. The
reason: `grant` already confirms its SCOPE PRN in the application layer (`resolve_scope`), and the
write goes through the `RoleGrantStore` port in `paigasus-iam-core::authz`, which takes only a
`RoleGrant`. So the principal guard sits next to the scope guard of the same use case. It is the
same "one rule, one layer per use case" argument as §2.1.

**New dependency.** `RoleService` and `RoleServiceDeps` get a field
`principals: Arc<dyn PrincipalRepository>`. Construction sites on `origin/main` (three):

- `adapters/http/mod.rs:536` — pass a `PgPrincipalRepository` over the same `db`;
- `tests/authz_bootstrap.rs:207` — the same;
- `application/roles.rs:452` (`new_service_with_fakes`, the unit-test harness) — pass an in-memory
  fake.

Confirm the count with `git grep -n "RoleServiceDeps {"` before the change.

**In-memory fake.** `application/fakes.rs` has no `PrincipalRepository` fake. `TenancyStore` has
`principals: HashMap<Uuid, String>` (uuid → stored canonical PRN), which `InMemoryMemberships`
already uses. Add a small fake over `TenancyStore` whose `find_principal` reads that map, parses the
stored string, and returns a `Principal` (kind from `principal_kinds` when present, else `User`;
status `Active`). Its other three methods are `unimplemented!` with a message that names the fake.
The existing `roles.rs` unit tests that grant to a principal must seed `store.principals`; list them
in the plan.

**Behaviour changes on `GrantRole` (gRPC and `POST /v1/authz/role-grants`).**

| # | request | today | after |
|---|---|---|---|
| G1 | principal PRN with a non-empty region, authorized actor | grant stored; forged PRN in event and response | `prn-mismatch`; no row, no event, no audit, no bump |
| G2 | principal PRN with a non-empty organization slot, authorized actor | the same as G1 | `prn-mismatch`; nothing written |
| G3 | **unknown** principal uuid, authorized actor | `fk_role_grant_principal` fails → `Backend` → `internal` | `not-found` |
| G4 | correct principal PRN with an upper-case uuid | the grant | the grant — **no change**; the event `aggregate_prn` is the lower-case stored PRN |
| G5 | forged PRN, actor without `GrantRole` at the scope | `permission-denied` | `permission-denied` — **no change** |
| G6 | forged PRN for an EXISTING (principal, role, scope) grant | the existing grant, OK | `prn-mismatch` |

G3 does not add an existence oracle: today an authorized actor already tells an unknown principal
(an internal error) from a known one (OK). G5 holds because authorization runs first.

**Not changed.** `RoleService::revoke` reads the grant from storage by id, so its event carries the
stored principal PRN. `RoleService::list` with a principal filter is §4.8.

### 4.8 Role-grant listing: `RoleService::list` (scope extended, Q6)

**The defect (re-checked on the branch base, `roles.rs:373-391` and `pg_role_grants.rs`).**
`RoleService::list` parses the principal filter with the `roles.rs` copy of `parse_principal_prn`,
which checks only the syntax, the service and the type. Both read paths then filter on the bare
uuid:

- the principal-only path calls `RoleGrantStore::list_by_principal`, and
  `PgRoleGrantStore::list_by_principal` filters `role_grant.principal_id = p.uuid()`;
- every other path calls `RoleGrantQuery::find`, and `find_statement` binds
  `g.principal_id = p.uuid()` when a principal is set.

So `prn:pgs:iam:eu-west-1::principal/<real-uuid>` and `prn:pgs:iam::<any-org>:principal/<real-uuid>`
list the real principal's grants, with and without a scope, a role key or a kind. The rows carry the
STORED principal PRN (`model_to_grant`), so the response does not echo the forgery. It is the same
defect as §1: the server answers for a PRN that no principal has. An unknown principal uuid answers
an empty OK list today.

**The fix: the same application-layer guard as `grant`.** `list` calls
`self.resolve_principal(principal)` (§4.7) when the filter carries a principal, directly after the
authorization step (D4) and before `Page::new` and any read. It is ONE call site for both read paths:

```rust
if let Some(principal) = filter.principal() {
    self.resolve_principal(principal).await?;
}
```

Why the application layer, and not the repository layer that §2.1 chose for `ListMemberships`:
`resolve_principal` exists after §4.7, in the same service, and the read ports
(`RoleGrantStore::list_by_principal`, `RoleGrantQuery::find`) live in `paigasus-iam-core::authz`.
`RoleGrantStore::list_by_principal` also serves the policy snapshot and the authn path with
server-resolved ids, the same trusted-caller argument as §2.1 A. One guard per use case, next to the
scope and principal handling of the same service, is the §4.7 argument again.

**The self path runs the guard too.** When the principal filter equals the actor
(`is_self`, D4 (a)), the supplied PRN is byte-equal to the actor's server-resolved canonical PRN,
so it cannot be a forgery. The guard still runs, for one rule with no exception. The cost is one
primary-key read of `principal` per self listing. A caller whose own principal row is absent gets
`not-found`. The authn layer resolves an actor from an external identity or an API key record
(§2.1 A), and both point at a stored principal, so this is not expected for a real caller. The
Task 7 transport tests pin the self path (L8) against Postgres.

**Order after the change.** Parse (`invalid-prn`) → kind (`invalid-principal-kind`) → filter
(`missing-required-field`) → authorize (D4, `forbidden`) → **`resolve_principal`**
(`not-found` / `prn-mismatch`) → principal-only read, or `Page::new` (`invalid-pagination`) and
`find`.

**Behaviour changes on `ListRoleGrants` (gRPC and `GET /v1/authz/role-grants`).** Rows apply to
both transports. "Authorized" means the caller passes D4 for that request: `ListRoleGrants` at Root
for a principal-only request about another principal, or at the scope node when a scope is set.

| # | request | today | after |
|---|---|---|---|
| L1 | principal-only, forged region, authorized caller | the real principal's grants | `prn-mismatch` |
| L2 | principal-only, forged organization slot, authorized caller | the real principal's grants | `prn-mismatch` |
| L3 | principal + scope (or + role key or kind), forged region or org slot, authorized caller | the real principal's grants at that scope | `prn-mismatch` |
| L4 | **unknown** principal uuid, authorized caller, with or without a scope | empty OK list | `not-found` |
| L5 | correct PRN with an upper-case uuid, authorized caller or self | the grants | the grants — **no change** |
| L6 | caller without the D4 grant, forged, canonical or unknown PRN of another principal | `forbidden` | `forbidden` — **no change** |
| L7 | the caller's OWN uuid with a forged region or org slot, caller without the D4 grant | `forbidden` (not self: the canonical form differs) | `forbidden` — **no change** |
| L8 | the caller's own canonical PRN (self, D4 (a)) | the caller's grants | the caller's grants — **no change**; one extra read |
| L9 | forged or unknown principal + scope + out-of-range `limit`, authorized caller | `invalid-pagination` | `prn-mismatch` / `not-found` — **changed order** |
| L10 | forged principal + unknown `principal_kind`, or a missing principal and scope | `invalid-principal-kind` / `missing-required-field` | the same — **no change** |

**L9 is a deliberate difference from B5/B8.** `ListMemberships` keeps `invalid-pagination` first
because its transports validate the page before the service runs. `ListRoleGrants` validates the
page inside the service (D6), and only on the query path. Keeping the page first would need a second
call site of the guard (one per read path), or an unreachable branch. One call site keeps the M7
mutation proof simple. Both answers are refusals of an invalid request; only the reason code of a
doubly-invalid request changes.

**L4 and an existence oracle.** A scope-authorized caller (for example an `org_admin` of org X,
which holds `ListRoleGrants` and `GrantRole` at X) now tells an unknown principal uuid
(`not-found`) from a known one (an OK list) with `principal_prn` + `scope_prn = X`. This adds no new
capability: that caller already tells them apart through `GrantRole` at X (G3: `internal` before
SMA-649, `not-found` after, against OK for a known principal). Principal uuids are uuid v7 values,
so this answers "does this uuid exist", not "which uuids exist".

**No log line** (Q2), the same as `grant`.

---

## 5. Out of scope

- **`ListRoleGrants` with a principal filter** is now IN scope (§4.8, Q6).
- **The scope filter of `ListRoleGrants`.** `find_statement` binds `g.scope_node_prn` to the
  canonical form of the supplied scope, so a forged scope PRN matches no row and answers an empty
  list, and D4 authorizes against the scope's stored ancestry. That is not the defect of this
  issue (no answer for a PRN that names another resource), and Sven's Q6 names the principal filter
  only.
- **`RevokeRole`.** It takes a grant id, not a principal PRN, and its event carries the stored
  principal PRN. Not affected.
- **`GrantRole`** is now IN scope (§4.7).
- **Authn introspection** (`principal_context::load_all_memberships`). It does not take a wire PRN
  (§2.1 A).
- **Moving `to_page` after the guard** (§2.2, B5).
- **Removing `MembershipRepository::list_by_node`**, which has no service caller after this change.

---

## 6. Tests

Every forged shape is spelled out. "Wrong principal uuid" must not appear unqualified: a wrong
RESOURCE uuid answers `not-found`, not `prn-mismatch`, and a test built that way passes for the
wrong reason.

- **forged region:** `prn:pgs:iam:eu-west-1::principal/<real-uuid>` (`eu-west-1` is a valid region
  under `is_valid_region`; upper-case is already `invalid-prn`).
- **forged organization slot:** `prn:pgs:iam::<some-org-uuid>:principal/<real-uuid>`. Use a real
  organization's uuid in one case and a random uuid in another.
- **correct, upper-case uuid:** the canonical PRN with the uuid upper-cased (control, B3).
- **unknown principal:** a canonical PRN with a uuid that no principal has (B4).

### T1 — service unit tests (`application/memberships.rs`, fakes)

`list_refuses_a_forged_principal_prn`: for kind `Any` and `Only(User)`, for the two forged shapes,
`svc.list(MembershipFilter::Principal(forged), …)` answers `TenancyError::PrnMismatch`.
Control in the same test: the canonical PRN returns the seeded rows (non-empty), so the refusal
cannot pass because the list is broken for every input. `seed_principal` writes only
`store.principals`, and `InMemoryMemberships::list_of_kind` keeps a row only when
`store.principal_kinds` holds the uuid. So the test must also insert `principal_kinds[uuid] = User`,
and must assert a NON-EMPTY control list for both kind values. Without that, the `Only(User)` control
is empty and proves nothing about the kind-set path.

`list_answers_not_found_for_an_unknown_principal`: kind `Any` and `Only(User)`.

`list_accepts_an_upper_case_uuid_in_a_correct_principal_prn`.

Update the existing tests that call `list_of_kind` or build `MembershipAxis::Principal(uuid)` to the
new types.

### T2 — Postgres repository tests (`tests/tenancy_memberships.rs`)

`list_of_kind_confirms_the_principal_prn`: real Postgres. Seed a principal with one membership. For
`kind = None` and `kind = Some(User)`: forged region → `RepositoryError::PrnMismatch`; forged
organization slot → `PrnMismatch`; unknown uuid → `NotFound`; canonical → one row. This is the test
that proves the SQL helper, because the unit tests only prove the fake.

`list_of_kind_confirms_the_node_prn`: real Postgres. This pins the node guard, which no test pins
today and which now carries the kind-unset node listing (§2.1 A). Call
`list_of_kind(&MembershipAxis::Node(..), kind, ..)` for `kind = None` and `kind = Some(User)`:

- a team PRN with a forged organization slot → `RepositoryError::PrnMismatch`;
- an organization PRN with a forged region (for example `eu-west-1`) → `PrnMismatch`;
- an unknown node uuid → `NotFound`;
- the canonical PRN → the seeded rows (non-empty).

Update `list_of_kind_keeps_only_members_of_that_kind_on_both_axes` (currently
`MembershipAxis::Principal(bot.uuid())`) to pass the `PrincipalId`, and its `PrincipalKind` argument
to `Some(…)`. Correct its doc comment (lines 419-420): it says that the test "still applies the node
guard", but the test asserts no guard error. Either drop that clause or point to
`list_of_kind_confirms_the_node_prn`.

### T3 — transport tests (both transports, real Postgres)

- **gRPC** (`tests/grpc_tenancy.rs`): `a_forged_principal_prn_never_lists_memberships`. For each
  `enforce_tenancy` setting, for `principal_kind` unset and `USER`, over both forged shapes: assert
  `Code::InvalidArgument` and reason `prn-mismatch`. Control: the canonical PRN returns the seeded
  membership. Add the unknown-principal case: `Code::NotFound`.
- **HTTP** (`tests/http_memberships.rs`): the same matrix on `GET /v1/memberships?principal=…`.
  Assert the HTTP status that `TenancyError::PrnMismatch` maps to today (400) and the `code` field of
  the body. URL-encode the forged PRN; the colons are safe in a query value, but the test must not
  depend on that.
- **B6 pin** (both transports, `enforce_tenancy = true`): a caller with no root grant gets
  `permission-denied` for the forged PRN, the canonical PRN and the unknown PRN alike. Add the case
  to `tests/grpc_tenancy.rs` AND to `tests/http_memberships.rs`. HTTP authorizes at `root_prn()`
  before `list` (`adapters/http/memberships.rs:123-129`), but only a test pins that order. This pins
  that the new `not-found` and `prn-mismatch` answers are not reachable by an unauthorized caller.
- **B8 pin** (either transport): an unknown principal uuid with an out-of-range `limit` answers
  `invalid-pagination`.

### T4 — prove the guard, not only the tests

A test that is red before the fix is not proof (project memory "red-first is not proof"). After the
implementation is green:

`rs/Cargo.toml` sets `warnings = "deny"`. If a mutation deletes the comparison, the bound row
(for example `stored`) becomes unused and rustc refuses the build; `--no-fail-fast` does not help
with a compile error. So each mutation below short-circuits the condition and keeps every binding
used. Run with `cargo nextest run --no-fail-fast` (memory: "a mutation must compile to prove
anything").

- **M1:** in `principal_list_uuid`, change the compare to
  `if false && stored.prn != principal.canonical()`. T2's forged principal cases and the
  Postgres-backed T3 cases must go red.
- **M2:** in the fake's principal arm of `list_of_kind`, change the compare to
  `if false && stored != principal.canonical()` (same shape). T1's forged cases must go red.
- **M4:** in `node_list_sql` (Postgres), change the compare to `if false && stored != node.canonical()`.
  The helper is shared by `list_by_node` and the node arm of `list_of_kind`, and after this change
  the second is the production path. T2's `list_of_kind_confirms_the_node_prn` forged cases must go
  red. (The unknown-node case stays green under M4, because `ok_or(NotFound)` is not mutated.)
- **M3:** make the service call `repo.list_by_principal(principal.uuid(), …)` for kind `None` again
  (the old routing). T1 and T3 kind-unset forged cases must go red. This pins the routing, which is
  the call site the guard depends on (memory: "guard-the-guard: pin production call sites").
- **M5:** in `RoleService::resolve_principal`, change the compare to
  `if false && stored != principal.canonical()` (keep every binding used). T6's forged grant cases
  (unit and transport) must go red.
- **M6:** in `RoleService::grant`, wrap the call as `if false { self.resolve_principal(&principal).await?; }`,
  so the helper stays used and the tree compiles. T6's forged AND unknown cases must go red. This
  pins the call site (memory: "guard-the-guard: pin production call sites").
- **M5 (extended, Q6):** the same M5 edit must also red T7's forged cases (unit and transport),
  because `list` calls the same `resolve_principal`.
- **M7:** in `RoleService::list`, change `if let Some(principal) = filter.principal() {` to
  `if let Some(principal) = filter.principal().filter(|_| false) {`, so the helper call stays in
  the tree and compiles. T7's forged AND unknown cases (unit and transport) must go red, on the
  principal-only path and on the principal + scope path. This pins the `list` call site.
- Restore each mutation by an Edit revert, not `git checkout --`, so the uncommitted fix survives.

### T6 — role-grant path (§4.7)

Use the same forged shapes as the top of §6.

- **Unit (`application/roles.rs`, fakes).** `grant_refuses_a_forged_principal_prn`: for the forged
  region and the forged organization slot (a real org uuid and a random uuid), with an authorizer
  that allows, `grant` answers `TenancyError::PrnMismatch`, and the outbox, the audit log and the
  gen-bumper fakes record NOTHING. `grant_answers_not_found_for_an_unknown_principal`: the same
  assertions with `TenancyError::NotFound`. Control in each test: the canonical PRN grants, and the
  one event has `aggregate_prn == stored canonical PRN`. Add `grant_accepts_an_upper_case_uuid`
  (G4): the grant succeeds and the event `aggregate_prn` is the lower-case stored PRN.
  `grant_refuses_a_forged_principal_prn_for_an_existing_grant` (G6): seed the grant first, then a
  forged PRN answers `PrnMismatch`, not the existing grant. `grant_with_a_forged_principal_prn_is_denied_before_the_lookup`
  (G5): with an authorizer that denies, the answer is `Forbidden`.
- **gRPC (`tests/grpc_authz.rs`) and HTTP (`tests/http_authz.rs`), real Postgres.** A root-granted
  actor grants a role to a seeded principal: forged region → `InvalidArgument` / 400 with reason
  `prn-mismatch`; forged organization slot → the same; unknown uuid → `NotFound` / 404. After each
  refusal, assert that no `role_grant` row exists for the real principal at that scope. Control:
  the canonical PRN grants, and the response `principal_prn` is the stored canonical PRN. If
  `tests/authz_forged_org_slot_escalation.rs` is a better home for the gRPC case, the plan may put it
  there.
- Route every expected code through `TenancyError::code()` or `ErrorReason::…::as_wire_reason()`
  (§4.6).

### T7 — role-grant listing (§4.8)

Use the same forged shapes as the top of §6.

- **Unit (`application/roles.rs`, fakes).** The harness seeds principals 1 and 2 into
  `store.principals` (principal 3 stays unknown). With an authorizer that allows `ListRoleGrants`
  at Root: `list_refuses_a_forged_principal_prn` — the forged region, the forged org slot (a real
  org uuid and a random uuid), both slots, and an upper-case uuid with a region, each on the
  principal-only path AND on the principal + Root-scope path, answer `TenancyError::PrnMismatch`
  (L1–L3). Control in the same test: the canonical PRN lists the seeded grant on both paths.
  `list_answers_not_found_for_an_unknown_principal` (L4, both paths).
  `list_accepts_an_upper_case_uuid_in_a_correct_principal_prn` (L5).
  `list_refuses_a_forged_principal_prn_before_the_page_check` (L9: principal + scope + `limit`
  201 answers `PrnMismatch`). `list_with_a_forged_own_principal_prn_is_not_self` (L7: with an
  authorizer that allows nothing, the actor's own uuid with a region answers `Forbidden`, and the
  canonical own PRN lists).
- **gRPC (`tests/grpc_authz.rs`) and HTTP (`tests/http_authz.rs`), real Postgres.** A
  root-granted admin grants a role to a seeded member. For the principal-only path and the
  principal + Root-scope path: forged region → `InvalidArgument` / 400 with reason `prn-mismatch`;
  forged organization slot → the same; unknown uuid → `NotFound` / 404. Control: the canonical PRN
  lists the member's grant. B6-style pin (L6, L7): a second, ungranted caller gets
  `permission-denied` / 403 `forbidden` for the member's forged, canonical and unknown PRN and for
  its OWN uuid with a forged region, and its own canonical PRN lists OK (L8).
- Route every expected code through `TenancyError::code()` or `ErrorReason::…::as_wire_reason()`
  in `src/` (§4.6); files under `tests/` may quote codes.

### T5 — full graph

Run the full `moon ci` target list from CLAUDE.md with `--base origin/main --include-relations`.
The gates most at risk: `repo:error-code-single-site` (§4.6), `:fmt` (longer identifiers reflow),
`:lint` (clippy), and `paigasus-iam-rs:test` (Docker-gated suites).

---

## 7. Acceptance criteria

- **AC1.** `ListMemberships` with a principal PRN whose region is not empty answers `prn-mismatch`
  on gRPC (`InvalidArgument`) and HTTP (400), with and without `principal_kind`.
- **AC2.** The same for a principal PRN whose organization slot is not empty.
- **AC3.** A canonical principal PRN, including one with an upper-case uuid, returns the same rows as
  before.
- **AC4.** An unknown principal uuid answers `not-found` on both transports.
- **AC5.** A caller without a root grant gets `permission-denied` for any principal PRN, under
  `enforce_tenancy = true`, on gRPC and on HTTP.
- **AC6.** `MembershipRepository::list_by_principal` and the authn introspection path are
  unchanged; `principal_context` tests pass without edits.
- **AC7.** The `adapters/grpc/tenancy.rs` module doc no longer names an exception to the
  PRN-confirmation rule, and no edited `src/` file contains a quoted registry code.
- **AC8.** Mutations M1–M7 each red at least one test, and each mutated tree compiles.
- **AC10.** A forged or unknown NODE PRN on `list_of_kind` (kind `None` and `Some(User)`) answers
  `PrnMismatch` / `NotFound` in a Postgres test (`list_of_kind_confirms_the_node_prn`).
- **AC11.** `GrantRole` with a principal PRN whose region or organization slot is not empty answers
  `prn-mismatch` on gRPC (`InvalidArgument`) and HTTP (400), and writes no grant row, no outbox
  event, no audit entry and no policy-generation bump.
- **AC12.** `GrantRole` with an unknown principal uuid answers `not-found` on both transports (today
  `internal`).
- **AC13.** A successful `GrantRole` writes the STORED canonical principal PRN into the outbox
  `aggregate_prn` and into the response.
- **AC14.** An actor without `GrantRole` at the scope still gets `permission-denied` for a forged
  principal PRN.
- **AC15.** `ListRoleGrants` with a principal PRN whose region or organization slot is not empty
  answers `prn-mismatch` on gRPC (`InvalidArgument`) and HTTP (400), on the principal-only path
  and with a scope.
- **AC16.** `ListRoleGrants` with an unknown principal uuid answers `not-found` on both transports
  (today an empty OK list).
- **AC17.** A canonical principal PRN, including one with an upper-case uuid, and a self listing
  return the same grants as before.
- **AC18.** A caller without the D4 grant still gets `permission-denied` for any principal PRN of
  another principal, and for its own uuid with a forged slot.
- **AC19.** `RoleGrantStore::list_by_principal`, `RoleGrantQuery::find` and the policy snapshot
  are unchanged.
- **AC9.** The full `moon ci` graph (CLAUDE.md target list) is green.

---

## 8. Files expected to change

| file | change |
|---|---|
| `rs/crates/libs/paigasus-iam-core/src/ports.rs` | `MembershipAxis::Principal(PrincipalId)`; `list_of_kind` kind → `Option`; docs |
| `rs/crates/services/paigasus-iam/src/adapters/persistence/pg_memberships.rs` | `principal_list_uuid`; principal arm guarded; docs |
| `rs/crates/services/paigasus-iam/src/application/fakes.rs` | `InMemoryMemberships::list_of_kind` guard + `Option` kind |
| `rs/crates/services/paigasus-iam/src/application/memberships.rs` | `list` routes through `list_of_kind`; docs; T1 |
| `rs/crates/services/paigasus-iam/src/adapters/grpc/tenancy.rs` | module doc only (§4.5) |
| `rs/crates/services/paigasus-iam/src/adapters/http/memberships.rs` | module doc only (§4.5) |
| `rs/crates/services/paigasus-iam/src/application/roles.rs` | `principals` dependency, `resolve_principal`, call in `grant`, docs (§4.4, §4.7); T6 unit tests; harness seeds principals |
| `rs/crates/services/paigasus-iam/src/application/fakes.rs` | also: a `PrincipalRepository` fake over `TenancyStore` (§4.7) |
| `rs/crates/services/paigasus-iam/src/adapters/http/mod.rs` | `RoleServiceDeps.principals` wiring (§4.7) |
| `rs/crates/services/paigasus-iam/tests/authz_bootstrap.rs` | `RoleServiceDeps.principals` wiring (§4.7) |
| `rs/crates/services/paigasus-iam/tests/grpc_authz.rs` | T6 gRPC; T7 gRPC |
| `rs/crates/services/paigasus-iam/tests/http_authz.rs` | T6 HTTP; T7 HTTP |
| `rs/crates/services/paigasus-iam/src/application/roles.rs` | also (§4.8): `resolve_principal` call in `list`, `list` and `parse_principal_prn` docs, T7 unit tests, harness seeds principal 1 |
| `rs/crates/services/paigasus-iam/tests/tenancy_memberships.rs` | T2; update one existing test |
| `rs/crates/services/paigasus-iam/tests/grpc_tenancy.rs` | T3 gRPC |
| `rs/crates/services/paigasus-iam/tests/http_memberships.rs` | T3 HTTP |

Any other implementer of `MembershipKindQuery` found by `git grep "impl MembershipKindQuery for"`
must change too. On `origin/main` at the time of writing there are exactly two.

Estimated size: **medium to large** — about 60 lines of product code and docs for
`ListMemberships`, about 50 more for the grant path and its fake, about 15 more for the listing
path (§4.8), and about 280 + 200 + 250 lines of tests.

---

## 9. Open questions (all answered by Sven on 2026-09-27, see §0)

- **Q1. ANSWER: fix it in this issue, no follow-up (§4.7, T6, M5, M6, AC11–AC14).** The challenger measured the
  role-grant path. `RoleService::grant` (`application/roles.rs:239`) accepts a forged principal PRN
  (its own `parse_principal_prn` copy checks only the service and the type), stores only the uuid,
  writes the FORGED PRN into the outbox event's `aggregate_prn` (line 271), and returns it in the
  response (`Ok(grant)`, line 308). That is the SMA-606 D2 hazard that `MembershipService::attach`
  prevents. The principal-only path of `ListRoleGrants` (lines 374-387) filters on the uuid.
  **CLOSED on 2026-09-28 by Q6 (§0):** no follow-up issue. `ListRoleGrants` is fixed in this issue
  with a storage compare (§4.8), not with approach D.
- **Q6. ANSWER: fold `ListRoleGrants` into SMA-649 (Sven, 2026-09-28, §0).** See §4.8, T7, M7,
  AC15–AC19.
- **Q2. ANSWER: no warning log (the spec default).** Should a principal `prn-mismatch` log a warning, as the sixteen node RPCs do with
  `warn_prn_mismatch`? This spec says no, to match the node filter of the same RPC, which logs
  nothing. If the answer is yes, both membership filters should log, and that is a separate change.
- **Q3. ANSWER: a follow-up issue, filed as SMA-719 (project Paigasus Polyglot, milestone IAM Gaps,
  priority Low, labels area:iam + Improvement).** After this change `MembershipKindQuery::list_of_kind` serves every wire listing, not only
  kind-filtered ones, and `MembershipRepository::list_by_node` has no production caller. Two
  parallel node-listing paths then share one helper but can drift. Should the port be renamed (for
  example `MembershipListQuery::list`) and `list_by_node` removed in this change, or in a tracked
  follow-up? No issue tracks it today. If the port is renamed, consider also a name for the
  unguarded `MembershipRepository::list_by_principal(Uuid)` that carries its contract (for example
  `list_by_principal_uuid_trusted`); today only a doc comment keeps a wire caller off it.
- **Q4. ANSWER: `not-found` is accepted.** B4 changes an unknown principal from an empty OK list to `not-found`. The issue does not
  name B4 explicitly, but it asks to "confirm the principal PRN against the stored principal", and
  an absent row has no other honest answer. The only callers who reach it hold a root grant. Is
  `not-found` acceptable for any out-of-tree client or script that you know of, which lists
  memberships of principals that may not exist yet? If B4 is NOT wanted, approach D (§2.1) becomes a
  closer contender, because B4 is one of its main gaps.
- **Q5. ANSWER: `prn-mismatch` is accepted.** B2 answers `prn-mismatch` for an org slot on a principal PRN, but the domain answers
  `invalid-prn` for an org slot on an organization PRN (§2). This spec keeps `prn-mismatch` because
  the issue asks for it and one code covers both principal faults. Do you accept the split between
  the two global resource types, or do you want `invalid-prn` for the principal org slot too (a
  shape check in `parse_principal_prn`, with the stored compare kept for the region and B4)?

---

## Challenge changelog

**Verdict:** APPROVE WITH CHANGES (spec-challenger, 2026-09-27). No blocker.

**Folded:**

- MAJOR 1 (node-listing reroute has no guard test). Confirmed: no test lists memberships with a
  forged or unknown node PRN, and the doc of `list_of_kind_keeps_only_members_of_that_kind_on_both_axes`
  claims a guard it does not assert. Corrected §2.1 A, added T2 `list_of_kind_confirms_the_node_prn`,
  mutation M4, AC10, and a doc fix for the existing test.
- MAJOR 2 (B2 code versus the `invalid-prn` precedent). Confirmed in `tenancy.rs` `check`. Decided
  explicitly with option 2: keep `prn-mismatch` and write the reason in §2. Added Q5 so that Sven can
  overrule it.
- MAJOR 3 (cheaper structural fix not evaluated). Added approach D to §2.1 with the concrete reasons
  for rejection as the fix of this issue (the issue asks for a storage compare; no B4; the mint
  convention becomes a wire contract). Recorded D as a candidate for the Q1 follow-up.
- MINOR: M1/M2 would not compile under `warnings = "deny"`. Rewrote M1, M2 (and the new M4) as
  `if false && …` short-circuits.
- MINOR: the T1 kind-set control was vacuous. T1 now seeds `principal_kinds` and asserts a non-empty
  control for both kind values.
- MINOR: B6/AC5 was pinned on gRPC only. Added the HTTP case to T3 and AC5.
- MINOR: "both filters stay alike" was only partly true. Added row B8 and corrected the sentence.
- MINOR: two wrong counts. TypeScript has two `listMemberships` call sites (both `nodePrn`); there
  are five `MembershipServiceDeps` construction sites. Both corrected (§2.3, §2.1 C).
- MINOR: the two `parse_principal_prn` copies will have different contracts. Added a doc-only change
  to `application/roles.rs` (§4.4, §8).
- MINOR: "if the edit makes it incomplete" was ambiguous. §4.5 now states no change to the SMA-444
  paragraph, with the reason.
- QUESTIONS: Q1 recorded as answered with the challenger's evidence; Q3 extended with the
  `list_by_node` removal and the trusted-name idea; Q4 extended with the effect on approach D.

**Rejected:**

- MINOR "unguarded `list_by_principal(Uuid)` stays on the port": no change in this spec, because the
  challenger itself rates it acceptable now. It is recorded under Q3 for the rename.
- MAJOR 2 option 1 (`invalid-prn`): not adopted, because the issue text requires `prn-mismatch` and
  the organization precedent is itself mixed (a region on an organization PRN already answers
  `prn-mismatch`). Left open as Q5.
- MAJOR 3 "adopt D alone": not adopted, because D does not confirm against storage as the issue
  requires, and gives no B4.

The size estimate moved from "small to medium" to "medium".

## Changes after approval (2026-09-27)

**Approval decisions applied (§0).** Q1 extended the scope to `RoleService::grant`: new §4.7, T6,
M5, M6, AC11–AC14, new rows in §8, a new §5, and a larger size estimate. Q2–Q5 are marked as
answered in §9 and in the body text. Q3's follow-up is SMA-719.

**Checked against `origin/main` 4051df5e (SMA-695).** No file that this spec names changed after
SMA-676 (000e90e4). SMA-695 changed only the Helm chart and does not touch this spec. Corrections:

- §2.1 D: `pg_service_accounts.rs:172` is not a `Prn::build` mint. It writes the minted id's
  `canonical()`. The sentence now says that.
- §4.5: the SMA-444 paragraph of `adapters/grpc/tenancy.rs` is lines 41-61, not 51-61. The
  paragraph to replace ("The rule, and its one exception") is lines 30-39.
- §4.4: the `roles.rs` doc bullet no longer says "doc only" and no longer waits for a follow-up.

Re-confirmed as correct: the five `MembershipServiceDeps` sites (`memberships.rs:275, 296, 556`,
`http/mod.rs:437`, `tests/tenancy_events_pg.rs:306`); the two `MembershipKindQuery` implementers;
the two TypeScript `listMemberships` call sites (both `nodePrn`); `http/memberships.rs:123-129`;
the `list_by_node` test lines 325, 398, 444 and the doc comment at 419-420; `roles.rs:239, 271, 308`.

## Changes after approval (2026-09-28)

**Approval decision applied (§0, Q6).** Sven folded `ListRoleGrants` with a principal filter into
this issue. New §4.8 (with the L1–L10 table), T7, M7, the M5 extension, AC15–AC19, new rows in §8,
§5 no longer lists `ListRoleGrants`, and §9 Q1 is closed. AC8 now covers M1–M7.

**Decisions taken in this extension (for review):**

- The guard is `RoleService::resolve_principal` (§4.7), called once in `list`, after D4 and
  before `Page::new` (§4.8). This changes the reason code of one doubly-invalid request (L9).
- The self path runs the guard too (one extra read per self listing), for one rule with no
  exception.
- `not-found` for an unknown uuid applies also to a scope-authorized caller (L4). The existence
  oracle this gives is already present through `GrantRole` at the same scope (G3).
- The scope filter of `ListRoleGrants` stays out of scope (§5).

**Checked on the branch base (`origin/main` 4051df5e plus commits 82f55279 and 75e57087).**
`roles.rs:373-391` (`list`), `pg_role_grants.rs` `list_by_principal` (uuid filter) and
`find_statement` (`g.principal_id` bind); `org_admin` holds `GrantRole` and `ListRoleGrants`
(`paigasus-iam-core/src/authz/roles.rs`). No existing test lists role grants of a principal that
does not exist: the `principal_prn` listings in `tests/grpc_authz.rs`, `tests/http_authz.rs` and
`tests/authz_people_model_access.rs` use provisioned principals.
