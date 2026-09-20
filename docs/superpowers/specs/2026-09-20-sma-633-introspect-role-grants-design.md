# SMA-633 — populate `role_grants` in Introspect

Date: 2026-09-20
Linear: [SMA-633](https://linear.app/smaschek/issue/SMA-633/iam-populate-role-grants-in-introspect)
Status: approved design, implementation blocked on SMA-632

Revision 2. The adversarial challenge of 2026-09-20 disproved the first revision's
D4 and inverted its D8. § 9 records what changed and why.

## 1. Problem

`AuthenticateToken::introspect` returns `role_grants: Vec::new()` unconditionally
(`rs/crates/services/paigasus-iam/src/application/authenticate_token.rs:161-165`).

The wire contract already carries the field. `IntrospectResponse.role_grants` is
proto field 8, `repeated RoleGrantRef`. Both mappers already forward whatever the
application layer gives them (`adapters/http/dto.rs:244-277`,
`adapters/grpc/convert.rs:399-419`). Only the application layer is empty.

The consequence reaches the consoles. `@paigasus/console-core`'s
`createIntrospectPrincipalResolver` hardcodes `roleGrants: []` and
`grantsAvailable: false` (`ts/packages/paigasus-console-core/src/principal-resolver.ts:79-87`),
so `can()` in `@paigasus/auth` fails open (`ts/packages/paigasus-auth/src/client.ts:66-69`).
SMA-511 therefore used `IsAuthorized` self-queries for its affordances (spec D5, § 4.6).

## 2. Goal

Make the OIDC `Introspect` RPC report the principal's real role grants, and
advertise that the build does so. The consoles keep their current behaviour; a
follow-up issue consumes the data.

## 3. Decisions

### D1 — scope: the OIDC path in Rust only

This issue changes `AuthenticateToken::introspect`, adds a capability key, and
stops. It does not change the TypeScript resolver, and `grantsAvailable` stays
`false` in every console.

Three facts drive this.

1. SMA-632 rewrites the file the TypeScript work would edit. Its plan (lines 53-57)
   modifies `src/principal-resolver.ts` ("Two calls become one") and `src/principal.ts`
   ("`introspectWithProvisioning` becomes `whoAmI`"). After SMA-632 the resolver does
   not call `Introspect` at all. TypeScript written now would target a deleted call path.
2. `can()` would enforce against a login-time snapshot. `runtime.resolver.resolve`
   runs once, at `ts/packages/paigasus-auth/src/http/routes.ts:293`; the result freezes
   into `SessionRecord.principal` (`src/core/session.ts:35`); the refresh path rewrites
   tokens and never `principal` (`src/core/single-flight.ts:194`); and
   `PAIGASUS_SESSION_ABSOLUTE_TTL_SECONDS` defaults to 86400 (`src/config.ts:55`). So a
   revoked grant would keep satisfying `can()` for up to 24 hours. This repository
   already rejected that model in writing, for memberships:
   `ts/packages/paigasus-console-core/src/principal.ts:3-5` — "a degraded login must not
   stay degraded for the session, and a membership change must appear on the next render."
3. Nothing consumes `can()` today. It has one call site
   (`ts/packages/paigasus-app-shell/src/nav/primary-nav.tsx:75`), both nav builders
   construct entries as object literals with no `requires` and no spread
   (`ts/apps/iam-console/lib/nav.ts:21-41`, `ts/apps/gateway-console/lib/nav.ts:22-29`),
   and `can` is imported nowhere else. So deferring costs no behaviour.

The follow-up therefore decides the freshness contract with a real `requires:` use
case in hand, after the SMA-632 rebase. § 7 lists what it owns.

### D2 — the API-key path stays empty

`AuthenticateApiKey::introspect` keeps returning an empty list. Its doc comment
changes from "no current caller populates it" to a statement of this decision.

`IntrospectApiKey` is not an occasional RPC. It runs on every gateway request:
`rs/crates/services/paigasus-gateway/src/adapters/http/auth.rs` calls
`iam.introspect_api_key(&key)` in `require_iam_auth`, the middleware in front of every
model invocation, and again in `require_authenticated`, the discovery middleware.
Nothing in this repository reads `role_grants` from that response; the gateway reads
`status` and `scope_prn`. Populating it would add one
`SELECT … FROM role_grant WHERE principal_id = ?` per gateway request to serve no reader.

The credential is also already narrowed at request time on that path, which is the
correction to revision 1's D8 — see § 9.

If a caller ever needs an API key's grants, the issue that introduces that caller
decides the caching and availability questions with a concrete requirement. It must
read D6 first: the gateway maps IAM's `Internal` to `IamUnavailable` (503), so a
grant-store read placed on that path turns a `role_grant` outage into a gateway outage.

**The rule is about the call site, not the credential.** This is easy to misread, so it
is stated explicitly. `WhoAmI` accepts an API-key bearer: `require_bearer` resolves an
API-key token through `api_key_auth.resolve`
(`adapters/http/auth_middleware.rs:51-55`), and the handler then calls
`state.authn.context_for` (`adapters/http/authn.rs:119`, and `adapters/grpc/authn.rs:105`).
So a service account calling `WhoAmI` **does** get its grants, while the same service
account calling `IntrospectApiKey` gets an empty list. The two endpoints disagree on
purpose. `IntrospectApiKey` is the gateway's per-request authentication call with no
reader of the field; `WhoAmI` is an explicit, console-facing request whose whole purpose
is to describe the caller. The cost argument applies to the first and not to the second.

This discloses nothing new. Both answers describe the same principal's own grants, and
D15 already establishes that the key wields the service account's entire grant set.

Found by the Task 2 review, which was right that the spec had not anticipated it.

### D3 — the port

`AuthenticateToken` gains a `grants: Arc<dyn RoleGrantStore>` field. It is a trait
object, not a further generic parameter, for the reason `RoleService` already
records at `application/roles.rs:91`: it is the same shared handle the composition
root composes into `PolicySnapshot`, so the wiring clones one `Arc` instead of
standing up a second store. A generic parameter would also not compile against that
`Arc`, because no `impl RoleGrantStore for Arc<dyn RoleGrantStore>` exists. The
`AuthnSvc` type alias therefore keeps its six generic parameters and does not change.
The trait exists
(`rs/crates/libs/paigasus-iam-core/src/authz/ports.rs:54-77`) and offers
`list_by_principal(&PrincipalId) -> Result<Vec<RoleGrant>, AuthzError>`. The Postgres
adapter exists (`adapters/persistence/pg_role_grants.rs:229-232`), the in-memory fake
exists (`application/fakes.rs:743-784`), and the composition root already holds the
store as an `Arc<dyn RoleGrantStore>` before it builds the use case
(`adapters/http/mod.rs:371`, use case built at `:772-780`). Production wiring is one
added argument; no new `Generations` handle is needed, because that store is already
constructed there.

`RoleService::list` (`application/roles.rs:307-319`) is the precedent for a live read
of a principal's grants. This mirrors it, minus its `Action::ListRoleGrants` check:
that check exists because `RoleService::list` can ask about a **different** principal.
Introspection reports only the bearer's own grants, and the token is the authorization
to see them.

### D4 — the projection

`introspect` maps each `RoleGrant` to
`RoleGrantRef { scope_prn: grant.scope.canonical_prn(), role_key: grant.role_key }`
(`authz/model.rs:142-150` and `:155-159`). `PrincipalContext.role_grants` is
`Vec<RoleGrantRef>` (`authn.rs:135-140`), so the grant id, the linked policy id and the
creation time stay off the wire. This issue does not widen that contract.

This does widen what a stolen OIDC token discloses: identity and memberships today,
plus the principal's grant scopes and role keys after the change. That is accepted.
The grant set is already readable by the same principal through `ListRoleGrants`, so
the token discloses nothing its holder could not already fetch.

### D5 — `resolve` is untouched

Only `introspect` reads grants. `resolve` runs on every authenticated request through
IAM's own middlewares (`adapters/http/auth_middleware.rs:52-54`,
`adapters/grpc/authn.rs:176-178`), and it is served from the API-key validation cache
on the key path. Adding a grant query there would put a database round trip on the
hottest path in the service for data that path never reads.

Revision 1 made a stronger claim — that `introspect` is not a per-request path at all —
and that claim was false. See D6 and § 9.

### D6 — a store failure fails the call, and what that costs

If the grant store errors, `introspect` returns `AuthnError::Backend`, mapped with the
same `map_err(backend)` the membership loop already uses. It must not fall back to an
empty list, because the follow-up will read an empty list as "this principal holds no
grants".

The honest blast radius: the OIDC `Introspect` is also reached per request, through the
gateway's `require_authenticated` discovery middleware, which calls `introspect_token`
after the API-key leg fails. `AuthnError::Backend` maps to gRPC `Internal`
(`adapters/grpc/convert.rs:146-151`), which the gateway maps to `IamUnavailable`, a 503.

This adds no new failure class. `introspect` already pages memberships from the same
database on that same path, with the same `map_err(backend)`, so a database outage
already fails that route today. The change adds one more table to a read that is
already database-dependent. It does not touch `require_iam_auth`, the model-invocation
path, because D2 leaves the API-key response empty.

### D7 — deterministic order

`introspect` sorts by `(scope_prn, role_key)` before returning.
`PgRoleGrantStore::list_by_principal` issues a `SELECT … WHERE principal_id = ?` with no
`ORDER BY`, so Postgres promises no row order.

That pair is a total order in the database: migration
`m0004_create_authz.rs:187-188` creates
`uq_role_grant_principal_role_scope UNIQUE (principal_id, role_key, scope_node_prn)`,
so one principal cannot hold two grants with the same scope and role key. The in-memory
fake does not enforce that constraint — `InMemoryRoleGrants` is a
`HashMap<Uuid, RoleGrant>` keyed by grant id (`application/fakes.rs:747`) — so the
determinism test must not insert a duplicate the database would reject.

### D8 — no cap and no pagination

The grant list stays unbounded. Memberships page at `MEMBERSHIP_PAGE_SIZE = 200`;
grants do not, because `RoleGrantStore::list_by_principal` takes no limit or offset.

A cap would silently truncate, and a truncated grant list makes a future `can()` return
false for authority the principal really holds. `RoleService::list` already returns the
same unbounded set to the same consoles, and `ListRoleGrants`'s wire `limit`/`offset`
are deliberately not enforced (`adapters/grpc/authz.rs:233-238`), so this adds no
exposure the system does not already have.

"Unbounded" has one ceiling worth naming: nothing overrides tonic's default 4 MiB decode
limit, so a large enough grant set fails at the client with a decode error rather than
truncating. Memberships already carry that property. If a principal ever holds enough
grants for either ceiling to matter, the fix is pagination in the port and in the
contract, not truncation in the use case.

### D9 — advertise a capability

`Capabilities` gains a fourth key, `iam.authn.grants`, unconditional like
`iam.deadletters` (`src/service_info.rs:28-58`). It means "this build populates
`role_grants` in `Introspect`".

The capability belongs in this issue even though nothing reads it yet. A capability
asserts a property of the build, and this is the build that gains the property.
Advertising it from a later build would misstate which images populate grants. It also
makes the follow-up purely TypeScript: the resolver gates `grantsAvailable` on the key,
exactly as `console-core/src/scopes.ts:104` already gates its grant walk on
`cedarCapabilityOf`. Without it, a console image deployed ahead of an IAM image would
read an old IAM's empty list as "no grants" and deny.

It is unconditional because it has no configuration switch to project. A `Capabilities`
field that is always `true` would be a false degree of freedom — the reasoning
`service_info.rs` already records for `iam.deadletters`.

This is the one contract change in the issue: a new `Capability` enum value in
`contracts/proto/paigasus/common/v1/`. It is additive, so it is not a breaking change.

### D10 — SMA-632 lands first

SMA-632 (the WhoAmI RPC) is specified and planned but not implemented. Its
`WhoAmIResponse` carries its own `repeated RoleGrantRef role_grants = 7`, which its D8
leaves empty and assigns to this issue.

**Resolved on 2026-09-20.** SMA-632 merged as `314ec074` and this branch is rebased onto it.
The prediction held: `AuthenticateToken::context_for` (`authenticate_token.rs:149-156`) is the
single place a `PrincipalContext` is built for the OIDC path, and `introspect` (`:160-163`)
delegates to it. So one edit fills `IntrospectResponse` and `WhoAmIResponse` together. Buf
lint's `RPC_REQUEST_RESPONSE_UNIQUE` forbade reusing the existing message, so `WhoAmI` carries
its own — two messages, one source field, no WhoAmI-specific work.

`principal_context.rs` holds only `load_all_memberships`, not `context_for`.

**One divergence to know about.** SMA-632's merged spec (§ 3.2 and § 9.5) withdrew its
`scope_prn` filtering claim and now states that SMA-633 reports the full grant set for **both**
credential kinds. This spec's D2 says otherwise: the API-key path stays empty. The two were
decided on different evidence. SMA-632's author did not have the measurement that
`IntrospectApiKey` runs on the gateway's per-request path with no reader of the field; D2 does.
Where they disagree, D2 governs this issue, and § 9 records why.

## 4. Change list

Rust, in `rs/crates/services/paigasus-iam/`:

- `src/application/authenticate_token.rs` — the new port, the populate-and-sort in
  `introspect` (or in `context_for` after the rebase), and the roughly twelve unit-test
  constructor sites (`:471,492,514,534,553,572,594,613,646,681,720,737`). The site at
  `:737-745` uses `PanicIfCalled*` fakes and needs a matching choice for the grant store.
- `src/adapters/http/mod.rs` — pass the existing `role_grant_store` (built at `:371`)
  to `AuthenticateToken::new` at `:772-780`. The `AuthnSvc` alias at `:158` does not
  change (D3).
- `src/service_info.rs` — the `iam.authn.grants` key (D9).

The capability key follows the six sites SMA-629 used for `iam.deadletters`:

- `contracts/proto/paigasus/common/v1/service_info.proto` — `CAPABILITY_IAM_AUTHN_GRANTS = 6`.
- `rs/crates/libs/paigasus-proto/src/capability.rs` — the `ALL` array grows to six, the
  literal-spelling test gains a row, and `adding_a_capability_forces_updating_these_tests`
  moves from `try_from(6)` to `try_from(7)`. That guard is designed to fail here.
- `ts/packages/paigasus-discovery/src/types.ts:68` — the `CapabilityKey` union.
- `py/packages/paigasus-proto/tests/test_service_info_smoke.py` — the registry-name assertion.
- `rs/crates/services/paigasus-iam/tests/http_service_info.rs` and
  `tests/grpc_service_info.rs` — the full-capability-set assertions.
- Regenerated bindings for Rust, Python and TypeScript.

The wire key needs no table: `as_wire_key` derives it by stripping `CAPABILITY_`,
lowercasing and turning `_` into `.`, so `CAPABILITY_IAM_AUTHN_GRANTS` yields
`iam.authn.grants` mechanically.

Doc comments that become false and must change in the same commit:

- `rs/crates/libs/paigasus-iam-core/src/authn.rs:133-134` — "`role_grants` is always
  empty until a later M3 task populates it". It sits on the type every introspection
  path returns, so it is the most load-bearing of the five.
- `src/adapters/http/dto.rs:242` and `src/adapters/grpc/convert.rs:398` — the same claim
  on the two mappers.
- `src/application/authenticate_api_key.rs:258-260` — restate as D2's decision, not as a
  missing implementation.
- `ts/packages/paigasus-console-core/testing/fake-iam.ts:15` — "Introspect always returns
  an empty `role_grants`". A comment correction only; the double's behaviour stays, because
  D1 defers the TypeScript work.

No change to `src/adapters/http/dto.rs` or `src/adapters/grpc/convert.rs` beyond their doc
comments: both already forward `ctx.role_grants`. No change to
`src/application/authenticate_api_key.rs` beyond its doc comment (D2). No change to
`tests/api_key_auth.rs`, which builds only the API-key use case.

## 5. Tests

New unit tests in `authenticate_token.rs`, on the existing `InMemoryRoleGrants` fake:

- A principal with grants at `GrantScope::Root` and at `GrantScope::Node` gets both, each
  carrying the canonical PRN of its scope.
- The order is `(scope_prn, role_key)`, proven with grants inserted in the opposite order
  and without a duplicate the unique constraint would reject (D7).
- A grant-store error makes `introspect` return a backend error. The test asserts the
  error, not an empty list (D6).
- A principal with no grants gets an empty list and a successful call.

Assertions that flip:

- `src/application/authenticate_token.rs:732` — `ctx.role_grants.is_empty()`.
- `tests/http_authn.rs:71` and its comment at `:63` — the test already seeds
  `platform_admin` at Root through `support::seed_platform_admin`, so it asserts that grant
  arrives.
- `tests/grpc_authn.rs:100` — the same through `support::provision_platform_admin`; its
  comment "role grants empty until a later M3 task populates them" goes away.

Assertions that stay, and gain a reason:

- `tests/http_service_accounts.rs:177` and `tests/api_keys_grpc.rs:143` both assert an empty
  `role_grants` on an API-key introspection. Under D2 they are now deliberate pins, not stale
  ones. Each gets a comment naming D2, so the next reader does not "fix" them.

`service_info.rs` gets the unit test its module doc describes: the new key appears in
`enabled()` alongside its siblings.

The reliable sweep for anything missed is
`rg 'role_grants|roleGrants' rs/crates/services/paigasus-iam/tests ts`. The
`seed_platform_admin` heuristic is not sufficient — it finds two of these files for the
wrong reason.

Every new assertion gets a mutation check: delete the line that populates the field, confirm
the test goes red, restore the line by deleting the marked edit. A test that stays green with
the feature removed proves nothing.

No CI gate pins the current behaviour. A grep of `ci/` for `role_grant` returns nothing, so
the tests above are the whole safety net.

## 6. Observability

No new metric. `record_grpc` already covers the added latency, and a grant-store failure
arrives as `AuthnError::Backend`, indistinguishable from the membership failure that can
already happen on the same call. Introducing a distinguishable signal is a change to IAM's
error taxonomy and does not belong in this issue.

## 7. Out of scope

The follow-up issue for the console owns all of this, and should be filed when SMA-632 merges:

- Reading grants in the resolver and gating `grantsAvailable` on `iam.authn.grants` (D9).
- The freshness contract: whether `can()` may read the login snapshot at all, given
  `principal.ts:3-5` already rejected that model for memberships (D1).
- Whether `myScopes()` (`console-core/src/scopes.ts:39-48`) can drop its walk of up to ten
  pages of `ListRoleGrants` per render, now that the same data arrives with the session. Note
  that `Introspect` is not gated on `authz.admin_enabled` while `ListRoleGrants` is
  (`adapters/grpc/authz.rs:228`), so the new path also works where the old one degrades to
  memberships only.
- `console-core/src/principal.ts:50`, which discards `roleGrants` today.

Also out of scope:

- Populating `role_grants` for API keys (D2).
- Replacing SMA-511's `IsAuthorized` self-queries (`console-core/src/authorize.ts`). They
  answer a different question: Cedar's decision, not the raw grant list.
- Any Cedar, policy or `EntitySlice` change.
- Pagination in `RoleGrantStore` (D8).

## 8. Risks

- **Rebase over SMA-632.** It rewrites `introspect` into `context_for` and deletes the
  membership paging. D10 orders the two; every line number here is pre-rebase.
- **Stale pins.** Two API-key tests keep asserting empty for a new reason. If a later issue
  populates the API-key path, those two tests are where it starts.
- **A one-way capability.** Once `iam.authn.grants` is advertised, removing it is a
  client-visible regression. It is unconditional by D9, so there is no switch to turn it off.

## 9. What the adversarial challenge changed

The challenger returned NEEDS REWORK with four blockers. Three were accepted outright and
one was corrected rather than accepted.

- **Revision 1's D4 was false.** It claimed `introspect` is an explicit RPC off the hot
  path. The gateway calls `introspect_api_key` on every model invocation and both introspect
  RPCs on every discovery request (`paigasus-gateway/src/adapters/http/auth.rs`). Verified
  directly. D2, D5 and D6 are rewritten around the real call graph.
- **Revision 1's D8 argued backwards.** It said a scope filter would report *less* authority
  than an API key can exercise, citing the gateway's self-query as proof. That call passes the
  key's own `scope_prn` as the resource (`auth.rs:98`), so the key *is* narrowed at request
  time on the one path that enforces anything, and the full set would *over*-report. The
  decision to leave the API-key path empty (D2) now rests on "no reader, per-request cost",
  which is true, instead of on a directionality claim that was not.
- **The deploy-skew gap was real.** Revision 1 flipped `grantsAvailable` with no capability
  gate. D9 adds the key and D1 defers the flip.
- **The TypeScript plan targeted a deleted call path.** SMA-632 rewrites the resolver. D1
  defers that work to a follow-up written after the rebase.
- **The test list was incomplete.** Two more Rust tests and two more TypeScript tests assert
  on `role_grants`; § 5 now names them and gives the sweep that finds them.
- **Five doc comments become false.** § 4 lists them.

Rejected: nothing. Two findings were narrowed rather than dropped. The "24-hour stale
snapshot" finding became a constraint on the follow-up (D1) rather than a change here,
because D1 removes the TypeScript work entirely. The "second composition root needs a
`Generations` handle" finding dissolved with D2: `tests/api_key_auth.rs` builds only the
API-key use case, which this issue no longer changes.
