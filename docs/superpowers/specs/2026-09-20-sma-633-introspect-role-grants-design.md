# SMA-633 — populate `role_grants` in Introspect

Date: 2026-09-20
Linear: [SMA-633](https://linear.app/smaschek/issue/SMA-633/iam-populate-role-grants-in-introspect)
Status: approved design, implementation blocked on SMA-632

## 1. Problem

`AuthenticateToken::introspect` returns `role_grants: Vec::new()` unconditionally
(`rs/crates/services/paigasus-iam/src/application/authenticate_token.rs:161-165`).
`AuthenticateApiKey::introspect` has the same gap
(`application/authenticate_api_key.rs:261-281`).

The wire contract already carries the field. `IntrospectResponse.role_grants` is
proto field 8 and `IntrospectApiKeyResponse.role_grants` is proto field 6, both
`repeated RoleGrantRef`. Both mappers already forward whatever the application
layer gives them (`adapters/http/dto.rs:244-277`, `adapters/grpc/convert.rs:399-419`).
Only the application layer is empty.

The consequence reaches the consoles. `@paigasus/console-core`'s
`createIntrospectPrincipalResolver` never reads `me.roleGrants`; it hardcodes
`roleGrants: []` and `grantsAvailable: false`
(`ts/packages/paigasus-console-core/src/principal-resolver.ts:79-87`).
`can()` in `@paigasus/auth` then fails open (`ts/packages/paigasus-auth/src/client.ts:66-69`).
SMA-511 therefore used `IsAuthorized` self-queries for its affordances (spec D5, § 4.6).

## 2. Goal

Report the principal's real role grants from both introspection RPCs, and let the
consoles consume them.

## 3. Decisions

### D1 — scope: Rust and the console resolver

This issue changes the Rust application layer and the TypeScript resolver.
The resolver reads `me.roleGrants` and sets `grantsAvailable: true`, so `can()`
enforces for real.

Blast radius today is zero. `can()` has one call site,
`ts/packages/paigasus-app-shell/src/nav/primary-nav.tsx:75`, and no console sets
`requires` on a nav entry. A grep for `requires:` under `ts/apps` returns nothing.
So the change turns the mechanism on before any screen depends on it.

### D2 — the port

`AuthenticateToken` gains a `RoleGrantStore` port. The trait exists at
`rs/crates/libs/paigasus-iam-core/src/authz/ports.rs:54-77` and already offers
`list_by_principal(&PrincipalId) -> Result<Vec<RoleGrant>, AuthzError>`.
The Postgres adapter exists (`adapters/persistence/pg_role_grants.rs:229-232`).
The in-memory fake exists (`application/fakes.rs:743-784`).
The composition root already holds the store as an `Arc<dyn RoleGrantStore>`
before it builds the use case (`adapters/http/mod.rs:371`, use case built at `:772-780`).

The new port follows each struct's existing style. `AuthenticateToken` holds its
other repositories as generic type parameters, so the grant store becomes a
seventh generic parameter. `AuthenticateApiKey` mixes generics and one
`Arc<dyn …>`; the implementer follows whichever of the two that struct already
uses for a repository, not for the cache.

`RoleService::list` (`application/roles.rs:307-319`) is the existing precedent for
a live read of a principal's grants. This issue mirrors it, minus its
`Action::ListRoleGrants` check: that check exists because `RoleService::list` can
ask about a **different** principal. Introspection only ever reports the bearer's
own grants, and the token is the authorization to see them.

### D3 — projection

`introspect` maps each `RoleGrant` to the wire-friendly
`RoleGrantRef { scope_prn: grant.scope.canonical_prn(), role_key: grant.role_key }`
(`authz/model.rs:143-162`). `PrincipalContext.role_grants` is
`Vec<RoleGrantRef>`, not `Vec<RoleGrant>` (`authn.rs:135-140`), so the id, the
linked policy id and the creation time do not reach the wire. That is the existing
contract and this issue does not widen it.

### D4 — `resolve` is untouched

Only `introspect` reads grants. `resolve` runs on every authenticated request
through the HTTP and gRPC middlewares; `introspect` is an explicit RPC. Adding the
query to `resolve` would put one more database round trip on the hot path for data
that path never uses.

### D5 — deterministic order

`introspect` sorts the grants by `(scope_prn, role_key)` before it returns them.
`PgRoleGrantStore::list_by_principal` issues a `SELECT … WHERE principal_id = ?`
with no `ORDER BY`, so Postgres does not promise a row order. A stable order keeps
the assertions honest and keeps the response bytes stable for a caller that
compares them.

### D6 — a store failure fails the call

If the grant store returns an error, `introspect` returns a backend error. It must
not fall back to an empty list.

This matters because of D1. Once `grantsAvailable` is true, an empty list means
"this principal holds no grants" and `can()` denies on it. A silent degrade would
turn a transient database failure into a wrong denial that looks like a policy
decision. The mapping matches the membership loop's existing `map_err(backend)`.

### D7 — no cap and no pagination

The grant list stays unbounded. Memberships page at `MEMBERSHIP_PAGE_SIZE = 200`;
grants do not page, because `RoleGrantStore::list_by_principal` takes no limit or
offset.

A cap would silently truncate. A truncated grant list makes `can()` return false
for authority the principal really holds, which is the same wrong-denial failure
as D6 but harder to see. `RoleService::list` already returns the same unbounded set
to the same consoles, so this adds no exposure that the system does not already have.
If a principal ever holds enough grants for this to hurt, the fix is pagination in
the port and in the wire contract, not truncation in the use case.

### D8 — the API key reports the full grant set

`IntrospectApiKey` returns every grant the service account holds. It does not filter
them by `ApiKey.scope`.

SMA-632 § 9.5 asks for the opposite: "for an API-key caller the grants must respect
the key's `scope_prn`". Three measured facts say a filter would misreport authority.

1. `ApiKey.scope_actions` and `ApiKey.scope_roles` are stored and never read.
   `application/authenticate_api_key.rs:32-34` states it: they are
   "v1-UNENFORCED metadata".
2. `application/api_keys.rs:7-22` (D15) states the security model: an API key is a
   bearer credential for its service account's "ENTIRE current grant set". The
   narrowing happens at mint time, where the actor must already hold `GrantRole` at
   every scope the service account is granted at. It is not a per-request narrowing.
3. The one enforcement site in the repository is the gateway's hardcoded
   `InvokeModel` self-query
   (`rs/crates/services/paigasus-gateway/src/adapters/http/auth.rs:90-122`). Cedar
   answers it from the principal's full grant set plus the tenancy ancestor chain.

So a scope filter would report **less** authority than the key can exercise. A
console reading it would show "you cannot do this" for a call that would succeed.
There is also no Rust helper that tests whether one PRN contains another;
`TenancyNodeRef` offers `from_prn`, `canonical`, `resource_uuid` and `kind`, and the
containment logic lives inside the compiled Cedar policies, reached through
`EntitySliceLoader`. A filter would need that helper written first.

If the product later wants a key to wield less than its service account, the change
belongs in authorization, not in introspection. Introspection must describe what the
credential can do today.

### D9 — SMA-632 lands first

SMA-632 (the WhoAmI RPC) is specified and planned but not implemented. Its
`WhoAmIResponse` carries its own `repeated RoleGrantRef role_grants = 7`, which its
D8 leaves empty and assigns to this issue.

This issue waits for SMA-632 to merge, then populates all three messages in one
pass. If SMA-632 builds its `application/principal_context.rs::context_for` helper
on `PrincipalContext`, WhoAmI inherits the grants with no extra code, because
`PrincipalContext.role_grants` is the single field every introspection path reads.
If it does not, this issue adds the same call there.

The session that owns SMA-632 was told to report the merge.

## 4. Change list

Rust, in `rs/crates/services/paigasus-iam/`:

- `src/application/authenticate_token.rs` — new port, populate and sort in `introspect`.
- `src/application/authenticate_api_key.rs` — the same.
- `src/adapters/http/mod.rs` — pass the existing `Arc<dyn RoleGrantStore>` to both use cases.
- Any other composition root that builds either use case, found by the compiler.
- After SMA-632: `src/application/principal_context.rs`, only if its helper does not
  already carry the field.

No change to `src/adapters/http/dto.rs` or `src/adapters/grpc/convert.rs`. Both
already forward `ctx.role_grants`. No change to `contracts/`. Every field exists.

TypeScript:

- `ts/packages/paigasus-console-core/src/principal-resolver.ts` — read `me.roleGrants`,
  set `grantsAvailable: true`, drop the stale comment at lines 77-78.
- `ts/packages/paigasus-console-core/testing/fake-iam.ts` — stop forcing an empty list.

## 5. Tests

New Rust unit tests, in both use cases, on the existing `InMemoryRoleGrants` fake:

- A principal with grants at `GrantScope::Root` and at `GrantScope::Node` gets both,
  each with the canonical PRN of its scope.
- The returned order is `(scope_prn, role_key)`, proven with grants inserted in the
  opposite order.
- A grant store error makes `introspect` return a backend error. The test asserts the
  error, not an empty list (D6).
- A principal with no grants gets an empty list, and the call succeeds.

Existing assertions that flip:

- `src/application/authenticate_token.rs:732` — `ctx.role_grants.is_empty()`.
- `tests/http_authn.rs:71` — the test already seeds `platform_admin` at Root through
  `support::seed_platform_admin`, so it asserts that grant arrives.
- `tests/grpc_authn.rs:100` — the same, through `support::provision_platform_admin`.
  Its comment "role grants empty until a later M3 task populates them" goes away.

Other integration tests that use `seed_platform_admin`, `seed_org_admin` or
`provision_platform_admin` (`tests/support/mod.rs:648-690`) and then read an
introspection response need the same review.

TypeScript:

- A resolver test proves a grant from the IAM answer reaches `SessionView.grants`
  and that `grantsAvailable` is true.
- `ts/apps/iam-console/tests/integration/doubles/fake-iam.test.ts:115`
  ("keeps role_grants empty even when a script returns some") inverts: the double
  must now pass a scripted grant through.

Every new assertion gets a mutation check. Delete the line that populates the field,
confirm the test goes red, restore the line by deleting the marked edit. A test that
stays green with the feature removed proves nothing.

## 6. Risks

- **`can()` stops failing open.** D1 accepts this. Nothing sets `requires` today, so
  no screen changes on merge. The next author who sets `requires` gets real
  enforcement, which is the point.
- **Stale test assumptions.** Several integration tests seed grants and then read an
  introspection body. § 5 names the ones found; the implementer greps for the rest.
- **Rebase over SMA-632.** Both branches edit `authenticate_token.rs`. D9 orders them,
  so this branch rebases onto the merged SMA-632, never the other way.

## 7. Out of scope

- Replacing SMA-511's `IsAuthorized` self-queries with grant checks
  (`ts/packages/paigasus-console-core/src/authorize.ts`). The self-queries answer a
  different question: Cedar's decision, not the raw grant list. Changing them is its
  own issue.
- Filtering an API key's grants by its scope (D8).
- Any Cedar, policy or `EntitySlice` change.
- Pagination in `RoleGrantStore` (D7).
