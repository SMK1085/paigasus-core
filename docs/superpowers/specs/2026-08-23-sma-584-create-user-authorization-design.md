# SMA-584 — `CreateUser` authorization posture (design)

**Issue:** [SMA-584](https://linear.app/smaschek/issue/SMA-584/iam-v1users-has-no-per-action-authorization-on-either-transport)
**Predecessor:** SMA-501 (#156), whose design decision D0 deferred exactly this decision.
**Date:** 2026-08-23
**Revision:** 2 — revised after the adversarial spec challenge (§10 records what changed).

## 1. Problem

`CreateUser` — the operation that mints a **user principal** — is bearer-gated and
otherwise unauthorized on both transports:

* `POST /v1/users` (`adapters/http/users.rs`) extracts no `AuthContext`.
* `UserService.CreateUser` (`adapters/grpc/users.rs`) mirrors that exactly.
* `libs/paigasus-iam-core/src/authz/action.rs` has no `Action::CreateUser`.

So any bearer-authenticated principal — including one the gRPC `AuthLayer`
JIT-provisions under `Provisioning::Enabled` — can mint further user principals.
Every one of the 21 `TenancyService` RPCs, by contrast, authorizes in its adapter.

### 1.1 Threat model: this is account squatting, not just principal inflation

The obvious framing ("an attacker can create extra principals") understates it, and
raises a fair objection — JIT provisioning already mints a user principal for any valid
token from a `jit_provisioning = true` issuer, so what does `CreateUser` add?

The answer is **denial of identity**. JIT provisioning fails *permanently* on an email
collision and never auto-links by email (design D5):

```rust
// application/authenticate_token.rs:205
Err(RepositoryError::Conflict(ConflictKind::EmailTaken)) =>
    Err(AuthnError::ProvisioningFailed(ProvisioningDefect::EmailConflict)),
```

So today any bearer-authenticated principal can `POST /v1/users` with a *colleague's*
email address and permanently lock that person out of OIDC JIT provisioning — the
squatted `user` row has no `external_identity`, so the victim's first login fails
`ProvisioningFailed` forever, with no self-service remedy. That is the real severity,
and it is why this is worth a behavior change rather than a documentation fix.

SMA-501 mirrored the existing posture rather than silently widening it, documented the
absence at four sites, and pinned it with
`tests/grpc_users.rs::create_user_requires_a_bearer_but_no_authorization`. Two
independent reviewers (the adversarial spec-challenger, then CodeRabbit) flagged the
gap unprompted.

## 2. What the code actually says (verified, 2026-08-23)

Findings that change the risk assessment the issue text assumes:

**F1 — JIT provisioning does not go through this use case.**
`AuthenticateToken::jit_provision` (`application/authenticate_token.rs`) writes via
`ExternalIdentityRepository::provision`, which spans principal + user +
external_identity in one transaction. It never calls `CreateUser::execute`.

**F2 — `CreateUser::execute` has exactly two _production_ call sites**, both adapters:
`http/users.rs:46` and `grpc/users.rs:77`. There is a third in test code
(`tests/grpc_tenancy.rs:139`, minting a principal directly through `AppState.users`
because `TenancyService` has no `CreateUser` RPC); it bypasses both adapters and is
therefore unaffected by an adapter-level guard. Bootstrap-admin seeding
(`application/bootstrap_admin.rs`) grants a role; it mints no user.

Together, F1 + F2 mean acceptance criterion 3 ("JIT provisioning and bootstrap still
work") is a **no-regression assertion, not a design constraint**. Nothing in either
path can be broken by a check placed at the adapters.

**F3 — no chicken-and-egg.** The first administrator of a fresh deployment arrives via
OIDC JIT provisioning plus the `authz.bootstrap_admins` config, which JIT-grants
`platform_admin`@`Root`. That path never touches `POST /v1/users`. Tightening
`CreateUser` therefore cannot lock an operator out of a new deployment.

**F4 — the closest analogue authorizes against an owner node, and cannot be copied
here.** `CreateServiceAccount` — the other principal-minting operation — authorizes in
`ServiceAccountService` against the service account's **owner tenancy node**
(`application/service_accounts.rs:147`). A *user* principal has no owner node: users
attach to orgs/teams later, via memberships. There is no node to scope against, so
`root_prn()` is the only coherent resource.

## 3. Decisions

### D1 — Tighten, at `root_prn()`, gated by `enforce_tenancy`

Add `Action::CreateUser` to the Cedar catalog and check it on both transports:

```rust
if <enforce_tenancy> {
    authorize.check(&actor, Action::CreateUser, &root_prn()).await?;
}
```

This is the exact shape of `CreateOrganization`/`ListOrganizations`.

**Effective audience under the starter policy set: `platform_admin` only.** Cedar's
template scoping is `resource in ?resource`, true iff `?resource` is the resource
itself or one of its hierarchy *ancestors*. `entity Organization in [Root]`
(`authz/schema.rs:14`) makes `Root` the ancestor, so `Root in <Organization>` is false
and an `Organization`-, `Team`- or `Project`-scoped grant can never satisfy a `Root`
resource. This is independently reinforced by `RoleService::grant`, which rejects a
scope kind the role does not declare (`application/roles.rs:203`), and only
`platform_admin` declares `NodeKind::Root` (`authz/roles.rs:209`).

**Qualifier — this is a property of the starter *role* set, not of the system.** An
operator can author a **static** Cedar policy through `PutPolicy` that permits
`CreateUser` at `Root` for one named principal; `put_in` enforces only
`validate_policy` plus a reserved-id check (`adapters/persistence/pg_policies.rs:277`).
That is not a hole — it is the intended narrow escape hatch, and §8 relies on it.
Stating D1 as a closed property would mislead a reviewer or operator.

**Rejected alternatives.** A config escape hatch (`authz.open_user_creation`) was
rejected: `enforce_tenancy = false` already exists as the deployment-wide bypass, and a
static `PutPolicy` grant (above) is a strictly narrower, auditable third lever, so a
new global knob adds a way to be insecure by misconfiguration without adding
capability. Org-scoped user creation was rejected as materially larger scope — it needs
a new required `organization_prn` field on both transports (a breaking contract change)
and a decision about what a user principal "belonging to" an org means when memberships
are already that mechanism.

### D2 — The check lives in both adapters; the use case is untouched

`CreateUser::execute` keeps its `(&self, cmd: NewUser)` signature, its
`CreateUserDeps` shape, and its `DomainEvent { actor_prn: None }`.

Rationale: every enforced surface in this service authorizes in the adapter
(`http/organizations.rs`, `teams.rs`, `projects.rs`, `memberships.rs`, and all 21
`TenancyService` RPCs), and `enforce_tenancy` is an `AppState` field the adapters
already hold. Pushing the check into the use case would mean threading an `Authorize`
dependency and an `enforce_tenancy` flag into an M0 application service purely to
enforce a transport-layer policy, changing its DI struct, its four unit tests, and the
emitted event's payload.

**Accepted cost 1 — successful user creation stays unattributable.** After this change
a *denied* `CreateUser` is audited (via the denial audit sink), but a *successful* one
— now necessarily performed by a `platform_admin` — still writes no `audit_log` row and
emits an outbox event whose `actor_prn` is `None` (`application/create_user.rs:119`;
`CreateUserDeps` deliberately has no `audit` field, `:70`). "Which admin minted this
principal" is exactly the record an IAM audit wants, and this design does not add it.
Note the attribution fix is *separable* from the check-placement decision: threading
`actor: &Prn` into `execute` (without an `Authorize` dependency) would fix attribution
wherever the check lives. It is deferred to a follow-up issue rather than folded in
here, to keep this change a single reviewable security decision.

**Accepted cost 2 — the `CreateServiceAccount` inconsistency is entrenched, twice.**
`CreateUser` will authorize at the adapter while `CreateServiceAccount` authorizes in
the application service, even though D3 keeps both on their own gRPC service for the
same "distinct principal aggregate" reason. Unifying them is out of scope here and is
named in §9 as a follow-up; this spec records it rather than letting it pass silently.

The cost of two enforcement sites — a future third transport forgetting one — is paid
down by §6's per-transport tests, which are written so that tightening or loosening
either transport alone reds CI.

### D3 — `UserService` stays; its rationale is rewritten

SMA-501 justified a separate `UserService` by the very property this issue removes
("parking the one unchecked RPC among 21 authorized ones would camouflage it"). That
rationale is now false and is itself a defect.

The RPC stays put and every doc site is rewritten to the durable reason: **a user is a
principal, not a tenancy node** — a different aggregate from `TenancyService`'s
org/team/project/membership surface, exactly as `ServiceAccountService` is. `UserService`
is the intended home for future user-principal operations (`GetUser`, `ListUsers`,
`ArchiveUser`), none of which exist today. Moving the RPC to `TenancyService` was
rejected: it is a breaking proto change one commit after SMA-501 landed, requiring
regeneration of the Rust/Py/TS bindings and clearance through `repo:breaking`, to fix an
organizational nit.

### D4 — No role allow-list gains `CreateUser`

`platform_admin`'s template omits the `action in [...]` clause entirely
(`authz/roles.rs:321`), so it covers every action automatically. Adding
`Action::CreateUser` to `ORG_ADMIN_ACTIONS` (or any other `*_ACTIONS` const) would be
dead weight for the reason given in D1: those templates are scoped below `Root` and can
never match a `Root` resource. `roles.rs`'s module doc already states this exclusion
principle — "an action whose §9.4-authorized resource can never be a
descendant-or-self of a role's scope kind is excluded".

The allow-lists are allow-lists precisely so a new `Action` grants nothing until
someone deliberately adds it. This design deliberately adds it nowhere — and §6.3 pins
that with an executable case, because D1 and D4 are otherwise prose assertions backed
by nothing.

### D5 — `is_write() == true`, with the mechanical consequences taken, not special-cased

`CreateUser` is a mutation, so `Action::is_write` returns `true` and `is_restore`
returns `false`. That places it in `forbid_archived_writes_source()`'s **derived**
action list (`authz/roles.rs:307`), which changes the starter policy set's content.
Consequences, all required together:

1. `STARTER_POLICY_REVISION` 2 → 3.
2. `EXPECTED_STARTER_CONTENT_HASH` re-pinned to the new blake3 digest — the failure
   message of `starter_policy_content_is_pinned_to_the_declared_revision`
   (`authz/roles.rs:718`) prints the value to use.
3. A `the_create_user_action_is_in_the_generated_forbid_source` test, mirroring the
   existing `the_retire_action_is_in_the_generated_forbid_source` — so a hand-updated
   hash with the action missing from `Action::ALL` cannot look green.
4. A `the_create_user_action_validates_against_the_embedded_schema` test in
   `authz/schema.rs`, mirroring `the_retire_action_validates_against_the_embedded_schema`
   (`authz/schema.rs:66`) — the established twin for a newly added action.

The forbid never actually bites: `entity Root;` (`authz/schema.rs:13`) declares no
attributes, so the `resource has effective_status` guard is unsatisfiable at `Root`. It
is still taken, because the derivation is deliberately mechanical (`roles.rs` module
doc: a missed action in a `forbid` only weakens a belt-and-braces guard, so the
direction is safe — special-casing it would introduce exactly the hand-maintenance the
derivation exists to avoid).

### D6 — No new error codes, no new wire vocabulary

`Authorize::check` already returns `TenancyError::Forbidden` on `Effect::Deny`
(`application/authorize.rs:53`), which maps to HTTP **403** `forbidden`
(`http/error.rs:24`) and gRPC **`PermissionDenied`**, reason `forbidden`
(`grpc/convert.rs:117`). Both adapters already convert `TenancyError`. Nothing is added
to `contracts/proto/paigasus/common/v1/error.proto`, and
`repo:error-code-single-site`'s `MANIFEST` needs no entry — the deny originates in
`application/authorize.rs`, not in either `users.rs`.

**Implementation trap:** that gate scans `rs/crates/**/src/**/*.rs` and matches a quoted
code literal **anywhere in the file, comments included** (`ci/error-registry/check.py:75`).
The module-doc rewrites in §5.2 therefore must write the code as `` `forbidden` ``
(backticks), never `"forbidden"`, or an otherwise-correct doc reds the gate.

### D7 — Follow the per-file `actor_context` convention

`grpc/users.rs` gets its own private
`fn actor_context<T>(request: &Request<T>) -> Result<AuthContext, Status>`, matching
the identical private copy already in `grpc/tenancy.rs`, `grpc/authz.rs`,
`grpc/audit.rs`, `grpc/service_accounts.rs` and `grpc/dead_letters.rs`. Lifting the
five existing copies into a shared helper is a cross-cutting refactor of files this
issue does not otherwise touch, and is out of scope.

### D8 — `AuthzLayer` is not used, and here is why

`adapters/http/authz_middleware.rs` ships an `AuthzLayer` — a `tower` layer that
authorizes a wrapped sub-router against one **fixed** `Action` and a per-request
resource, unit-tested and deliberately unwired because the tenancy routes "don't reduce
to ONE fixed `Action`" (`:10-18`). `POST /v1/users` is exactly the one-route,
one-action, one-fixed-resource shape it was built for, so its non-use needs a stated
reason rather than silence.

It is not used because this change's whole acceptance criterion is HTTP/gRPC
**symmetry**, and there is no tonic equivalent of `AuthzLayer` scoped to a single RPC
(`grpc::authn::AuthLayer` is authn-only, service-wide). Enforcing HTTP in a layer and
gRPC in the handler would put the two transports' guards in structurally different
places — the precise class of divergence this issue exists to end. The per-handler form
also matches all ~40 other enforced call sites. `AuthzLayer` remains shipped and unwired
for the coarse gateway surface it was written for.

## 4. Rollout

This is a **breaking behavior change for callers**, and the spec must say so.

**4.1 What breaks.** Any client that today creates users with an ordinary bearer starts
receiving `403` / `PermissionDenied`. That is the intent, but it is not a no-op
deployment.

**4.2 Rolling-deploy window.** During a rolling deploy the same non-admin caller gets
`201` from old replicas and `403` from new ones behind one load balancer —
nondeterministic for the duration. D1 rejects a feature flag, so there is no
deprecation window; the mitigation is 4.3, done **before** rolling.

**4.3 Pre-deploy step for operators.** Grant the identity that legitimately creates
users its authority *before* deploying. Three levers, in order of preference:

1. **A narrow static Cedar policy via `PutPolicy`** permitting exactly
   `Pgs::Iam::Action::"CreateUser"` at `Root` for that one principal. This is the
   recommended lever: least privilege, auditable, and revocable independently.
2. **`platform_admin`@`Root` role grant** — simple, but per `authz/roles.rs:321` this
   grants *every* action everywhere, including `PutPolicy` and `GrantRole`. Use only if
   the identity is genuinely a platform administrator.
3. **`enforce_tenancy = false`** — disables tenancy authorization at all ~40 enforced
   call sites fleet-wide. Not a remediation for this endpoint; listed only so its
   blast radius is explicit.

**4.4 Rollback.** Reverting the binary restores the open posture. The
`STARTER_POLICY_REVISION` 2→3 bump is *not* rolled back by a binary revert, and does not
need to be: an older replica whose revision is lower deliberately leaves the newer row
alone (SMA-477 D11), so a mixed or rolled-back fleet converges on the newer policy set
rather than flapping. The stored row simply carries a `CreateUser` entry in
`forbid-archived-writes` that an older binary ignores — harmless, since D5 establishes
the forbid is unsatisfiable at `Root` regardless.

**4.5 Existing squatted rows.** §1.1 means a running deployment may already hold `user`
rows created by a non-admin to squat someone's email. Tightening the endpoint stops new
squatting but remediates nothing already written. Operators should audit for `user`
rows with no corresponding `external_identity` row before or shortly after deploying.
Writing that query, and any remediation tooling, is **out of scope** (§9) — but the
need is recorded here rather than discovered later.

## 5. Changes

### 5.1 `rs/crates/libs/paigasus-iam-core`

| File | Change |
|---|---|
| `src/authz/schema.rs` | Add `CreateUser` to `SCHEMA_SRC`'s hand-maintained `action` declaration; add the `the_create_user_action_validates_against_the_embedded_schema` twin test (D5.4). |
| `src/authz/action.rs` | Add the `Action::CreateUser` variant, its `ALL` entry, its `as_wire` arm (`"CreateUser"`), and its `is_write` arm (write side). Update the exhaustive `match` in `all_covers_every_variant` (rustc enforces this), the `ALL.len()` assertion `40 → 41`, **and that assertion's explanatory message** (`action.rs:302`), which enumerates the count's provenance and would otherwise become arithmetically wrong. |
| `src/authz/roles.rs` | Bump `STARTER_POLICY_REVISION` to `3`; re-pin `EXPECTED_STARTER_CONTENT_HASH`; add the forbid-source membership test (D5.3) and the two `starter_policy_table` cases (§6.3). No `*_ACTIONS` const changes (D4). |

**Ordering note.** `SCHEMA_SRC` must gain the action in the same change as
`Action::ALL`, not after: `roles.rs`'s `every_starter_policy_passes_schema_validation`
(`:669`) runs `validate_policy` over the *generated* `forbid-archived-writes` source, so
an action present in `ALL` but absent from `SCHEMA_SRC` fails that test.

That unit test is the *only* real guard here. Contrary to what one might assume, boot
does **not** fail on this mismatch for an already-seeded database:
`SystemPolicyReconciler::reconcile_system` validates (`pg_policies.rs:401`) but logs and
skips rather than refusing to boot, and `bootstrap::reconcile_policies` classifies a
failure on an *existing* row as `Survivable` — `tracing::error!` then `continue`
(`application/bootstrap.rs:135`). Only a *missing* row is fatal. So the mismatch would
fail **silently** in production.

### 5.2 `rs/crates/services/paigasus-iam` (source)

| File | Change |
|---|---|
| `src/adapters/http/users.rs` | Handler takes `Extension<AuthContext>`; add the `actor_prn` helper (verbatim from `organizations.rs`) and the `if s.enforce_tenancy { s.authorize.check(&actor_prn(&ctx), Action::CreateUser, &root_prn()).await? }` guard **before** `s.users.execute`. Rewrite the module doc (backticked `` `forbidden` `` only — D6). |
| `src/adapters/grpc/users.rs` | Add the private `actor_context` helper (D7) and the mirrored guard, `map_err(convert::status_to_grpc)`. Rewrite the module doc and the `create_user` RPC doc. |
| `src/adapters/grpc/authn.rs` | Rewrite the `is_exempt` doc comment (~line 126) asserting `CreateUser` performs no authorization check. |
| `src/adapters/grpc/mod.rs` | Rewrite **three** stale sites: the module-level doc (~lines 7-8, "`CreateUser` is deliberately unauthorized"), and the two at ~lines 79 and 102. |
| `src/adapters/http/mod.rs` | Add a `tracing::warn!` when `enforce_tenancy == false`, mirroring the `accept_invalid_tls` precedent at `:672`. See §5.5. |

**Check-before-work ordering.** The guard runs before `to_command`/`execute`, so an
unauthorized caller never reaches email validation or the unit of work — matching
`create_org`, and ensuring a denied call cannot be used as an email-existence oracle.
This closes the *response-content* oracle (403 is returned uniformly, whether or not the
email exists). It does not claim to close side channels: a denied request still
increments the transport's metrics and writes a denial audit row. Those are equally
present on every other enforced route and are not made worse here.

### 5.3 `rs/crates/services/paigasus-iam` (tests other than the new ones)

| File | Change |
|---|---|
| `tests/grpc_users.rs` | §6.1. |
| `tests/http_memberships.rs` | **Required, and previously missed** — see §6.2. |
| `tests/authz_enforce_toggle.rs` | §6.4. |
| `tests/docker_preflight.rs` | The new `tests/http_users.rs` is a Docker-backed binary, so the hand-maintained counts move: the module doc (`:5`, "61 of this crate's 65") and the assertion message (`:44`, "60 of this crate's 65"). Verify the exact totals at implementation time by counting, not by trusting this table. |
| `tests/support/docker.rs` | The count at `:255` moves for the same reason. |

### 5.4 `contracts/proto/paigasus/iam/v1/iam.proto`

Rewrite the `Users (SMA-501)` banner comment (~line 508) to state the new posture:
`CreateUser` requires `Action::CreateUser` at `Root`, and `UserService` exists as the
user-principal aggregate's home. **Comment-only** — no field, message, or service
changes, so `repo:breaking` is unaffected. Per CLAUDE.md, run `buf format -w`; a
whitespace-only proto change still shifts the embedded `FILE_DESCRIPTOR_SET`, so
regenerate bindings with `buf generate` directly (the `contracts:generate` Moon task has
no `outputs:` and can serve stale cached output).

### 5.5 Docs and operator-visible config

`enforce_tenancy = false` is leaned on by D1 and §4.3 as *the* deployment-wide bypass,
yet it is invisible: absent from `iam.toml.example`, unlogged at boot, and absent from
the `ServiceInfo` descriptor. This change widens its blast radius to include principal
minting, so two one-line additions are in scope:

* a boot-time `tracing::warn!` when it is `false` (§5.2), mirroring `accept_invalid_tls`;
* an entry in `rs/crates/services/paigasus-iam/iam.toml.example`.

Adding it to the `ServiceInfo` capability descriptor is **not** in scope — that is a
contract change and belongs to its own issue.

`docs/dev-setup.md:67` and `CLAUDE.md:140` both carry the "61 of its 65" integration-suite
count and move with §5.3.

## 6. Testing

Two properties must hold, and the first spec revision only tested one:

* **P1 — no single-transport change can pass CI.** Each transport gets a test that fails
  if authorization is added to, or removed from, that transport alone.
* **P2 — the binding to `Action::CreateUser` specifically is falsifiable.** This is the
  hole the challenge found: because `platform_admin`'s template carries no `action in
  [...]` clause and D4 grants `CreateUser` to no other role, *every* test in revision 1
  passed identically with `Action::CreateOrganization` (or any other Root-only action)
  wired into the adapters. The new `Action` variant would have been decorative and a
  copy-paste of the `create_org` guard would have shipped green.

### 6.1 gRPC — `tests/grpc_users.rs`

Rewrite `create_user_requires_a_bearer_but_no_authorization` as
`create_user_requires_platform_admin`, pinning three outcomes against one server:

| Caller | Expected |
|---|---|
| no bearer | `Code::Unauthenticated` (proves `UserService` is still absent from `is_exempt`) |
| ordinary JIT-provisioned principal, no grants | `Code::PermissionDenied` |
| `platform_admin`@`Root` | `Ok`, response `principal_prn` parses as a `Prn` |

The four pre-existing tests (`create_user_over_grpc_mints_a_principal`,
`a_duplicate_email_is_already_exists`, `a_malformed_email_is_invalid_argument`,
`an_empty_locale_becomes_unset`) switch `support::provision` →
`support::provision_platform_admin`. The malformed-email test's principal-row-count
baseline is taken after provisioning (`grpc_users.rs:120`), so the extra grant does not
disturb it.

### 6.2 HTTP — `tests/http_memberships.rs` must change, and a new `tests/http_users.rs`

**Correction to revision 1.** Revision 1 claimed `http_memberships.rs` "authenticates as
`platform_admin`, so it stays green whether or not HTTP is tightened". That is false.
Only `ac1_membership_lifecycle_over_http` (`:40`) calls `provision_platform_admin`. The
other three build the app with `support::app(db)` and an **ungranted** JIT bearer and
then assert on `POST /v1/users`:

* `list_memberships_requires_exactly_one_filter` (`:132`, via the `create_user` helper)
* `create_user_rejects_duplicate_email` (`:155`)
* `create_user_rejects_invalid_email` (`:170`)

`test_config` uses `AuthzConfig::default()` (`support/mod.rs:470`) and `enforce_tenancy`
defaults to `true` (`config.rs:841`), so under the new guard all three receive `403`.
Implementing revision 1 verbatim would have reded `paigasus-iam-rs:test` on a file the
spec listed as unchanged.

**Required change:** switch those three to `app_with_state(db)` + `provision_platform_admin`,
mirroring §6.1's `grpc_users.rs` migration.

**The new file is still justified, on its own merits** (not on revision 1's false
premise): `http_memberships.rs` covers the membership lifecycle and, after the change
above, authenticates as `platform_admin` throughout — so it still cannot express the
403 case. `/v1/users`' authorization deserves its own subject-matter file, mirroring the
gRPC side's dedicated `grpc_users.rs`.

`tests/http_users.rs`, over the real router via `tower::ServiceExt::oneshot`:

| Caller | Expected |
|---|---|
| no bearer | `401` (mirrors `http_authn.rs`'s route-table entry) |
| ordinary JIT-provisioned principal, no grants | `403`, body `error.code == "forbidden"` |
| `platform_admin`@`Root` | `201`, body `principal_prn` parses as a `Prn` |

The `403` case additionally asserts **no principal row was minted** — the check must run
before the use case, not after. That assertion needs the same setup §6.1 relies on: a
`db.clone()` taken *before* `AppState::new` consumes it, and the row-count baseline taken
*after* provisioning (the pattern at `grpc_users.rs:117,120`). Without it the assertion
is either impossible to write or vacuous.

### 6.3 P2 — pinning the action binding (new in revision 2)

Two layers, because neither alone is sufficient.

**(a) Unit — `authz/roles.rs`'s `starter_policy_table` (`:471`)** gains two cases,
making D1 and D4 executable rather than prose:

| Case | Grants | Action | Resource | Expect |
|---|---|---|---|---|
| `platform_admin` at Root allows CreateUser at Root | `platform_admin`@`Root` | `CreateUser` | `root_prn()` | `Allow` |
| `org_admin` denies CreateUser at Root | `org_admin`@`Organization` | `CreateUser` | `root_prn()` | `Deny` |

**(b) Integration — the action-identity test.** A test in which a principal is authorized
for `CreateUser` and *not* for other Root actions. Seed, via the policy store, a static
Cedar policy permitting exactly `Pgs::Iam::Action::"CreateUser"` at `Root` for principal
X (the §4.3 lever 1 shape — so this test doubles as executable documentation of the
recommended remediation). Then assert, for X:

* `POST /v1/users` → `201`, and `UserService.CreateUser` → `Ok`;
* **control:** `POST /v1/organizations` → `403`, proving the seeded policy really is
  narrow and X is not simply a platform admin by another name.

A mutation that wires any action other than `CreateUser` into either adapter fails this
test. Placed in `tests/http_users.rs` with the gRPC half in `tests/grpc_users.rs`, so each
transport's binding is pinned on its own side and P1 still holds.

### 6.4 `enforce_tenancy = false`

Extend `tests/authz_enforce_toggle.rs` with a `POST /v1/users` case: an ungranted
principal, `cfg.authz.enforce_tenancy = false`, expects `201`. Without it the toggle is
untested for the newly-gated route, and a guard that ignored `enforce_tenancy` would pass
everything else.

### 6.5 AC-3 regression coverage (JIT + bootstrap)

Existing suites already cover this and must stay green **unmodified**:
`tests/authn_identities.rs`, `tests/grpc_authn.rs`, `tests/http_authn.rs`,
`tests/authz_bootstrap.rs`, `tests/authz_bootstrap_admin.rs`. F1/F2 explain why: the
provisioning path does not run through `CreateUser::execute` at all. No new test is added
for AC-3; the assertion is that these suites are untouched and passing. Any diff required
in one of them is a signal that F1/F2 are wrong and the design needs revisiting.

### 6.6 Unit-level

`paigasus-iam-core`: the `Action` catalog tests (`wire_roundtrip_all_variants`,
`all_covers_every_variant`) extend mechanically; `roles.rs` gains the forbid-source
membership test (D5.3) and §6.3(a); `schema.rs` gains the twin validation test (D5.4).
`http/users.rs`'s existing HTTP/gRPC twin test for the `to_command` projection is
unaffected — the guard sits outside the projection.

## 7. Verification

Per CLAUDE.md, per-project tasks do not run the repo-level gates. Before pushing, run the
full CI graph:

```
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site
  :input-liveness :promtool :observability-drift :nats-permissions :release-parity
  :release-parity-py :release-parity-ts :publish-metadata --base origin/main --include-relations
```

Gates specifically implicated:

* `:breaking` — comment-only proto edit; expected pass.
* `:error-code-single-site` — no new emission site (D6); the trap is the comment-literal
  scan, handled by backticking `` `forbidden` ``.
* `:iam-docker-policy-single-site` — the new `tests/http_users.rs` must use the shared
  policy in `tests/support/docker.rs`, never a hand-rolled skip.
* `:fmt` — `buf format -w` on the proto.

No new crate and no new dependency, so `:affected-smoke`'s expected-set baselines and
`:deny`/`:machete` need no re-baselining. No metric names change, so
`:observability-drift` is unaffected.

`paigasus-iam`'s integration suites are Docker-backed; run them with a reachable daemon
(`tests/docker_preflight.rs` reds otherwise). A filtered run needs
`PAIGASUS_REQUIRE_DOCKER=1`, since the canary is outside the filter.

## 8. Out of scope

* Unifying where `CreateUser` and `CreateServiceAccount` authorize (D2, cost 2).
* Attributing successful user creation — `actor` on `CreateUser::execute`, an audit row,
  a non-`None` `actor_prn` (D2, cost 1). Follow-up.
* Moving `CreateUser` onto `TenancyService` (D3).
* `GetUser`/`ListUsers`/`ArchiveUser` (D3 names `UserService` as their future home; it
  designs none of them).
* Org-scoped user creation (D1).
* **JIT provisioning's own posture.** It is the other user-principal-minting path and is
  deliberately left alone: it is gated by a per-issuer `jit_provisioning` flag against a
  configured, trusted issuer, so it is a different control surface with a different
  operator contract. Changing it is a separate decision, not a corollary of this one.
* Remediating already-squatted `user` rows (§4.5) — the need is recorded; the query and
  tooling are not designed here.
* Adding `enforce_tenancy` to the `ServiceInfo` capability descriptor (§5.5).
* Self-service signup as a product feature. If one is ever wanted it needs its own
  unauthenticated, rate-limited, verification-bearing endpoint — not a relaxation of this
  administrative one.

## 9. Acceptance criteria mapping

| AC | Where satisfied |
|---|---|
| 1. Both transports enforce the same authorization | D1 + D2; §5.2; pinned by §6.1 + §6.2 (P1) and §6.3 (P2) |
| 2. `grpc_users.rs` pins the new posture; an equivalent HTTP test exists | §6.1 + §6.2 |
| 3. JIT provisioning and bootstrap still work | F1/F2 explain why unaffected; §6.5 pins it |

## 10. Revision history

**Revision 2 (2026-08-23)** — after the adversarial spec challenge. Folded in:

* *(blocker)* §6.2 — revision 1's premise that `http_memberships.rs` could not be
  affected was **false**; three of its tests would have reded. The file is now a listed,
  required change, and the new-file decision is re-argued on its own merits.
* *(blocker)* §6.3 — revision 1's entire test plan passed with the **wrong `Action`**
  wired. Added `starter_policy_table` cases and an action-identity integration test.
* *(major)* §1.1 — the real threat model (permanent JIT lockout via email squatting,
  `authenticate_token.rs:205`), which revision 1 never stated.
* *(major)* §4 — a rollout section: breaking-change notice, rolling-deploy window,
  three remediation levers ranked, rollback, and already-squatted rows.
* *(major)* D1 — qualified "platform_admin only" as a property of the starter *role* set;
  a static `PutPolicy` policy can permit it narrowly, and that is the intended hatch.
* *(major)* D2 — named two accepted costs explicitly: unattributable successful creation,
  and the entrenched `CreateServiceAccount` inconsistency.
* *(major)* §5.5 — `enforce_tenancy = false` gets a boot warning and a config-example
  entry, since this change widens its blast radius.
* *(minor)* F2 wording (two *production* call sites; a third in `grpc_tenancy.rs`);
  corrected test name `every_starter_policy_passes_schema_validation`; corrected the
  claim that a schema/catalog mismatch fails boot (it is *survivable* and silent on a
  seeded DB — the unit test is the only guard); added the `schema.rs` twin test; the
  `ALL.len()` explanatory message; the third stale doc site in `grpc/mod.rs`; the
  `docker_preflight`/`support/docker.rs`/`dev-setup.md`/`CLAUDE.md` suite counts; the
  `repo:error-code-single-site` comment-literal trap; §6.2's `db.clone()` setup
  requirement.
* *(question)* D8 — `AuthzLayer` named and rejected with a reason (HTTP/gRPC symmetry)
  rather than passed over in silence.
* *(question)* §8 — JIT provisioning explicitly deferred with reasoning.

Not folded in: nothing. Every finding was either accepted or converted into an explicit,
reasoned deferral in §8.
