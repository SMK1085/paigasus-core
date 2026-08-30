# SMA-440 — Server-side audit stamping (set created/modified who+when on write)

**Issue:** [SMA-440](https://linear.app/smaschek/issue/SMA-440/server-side-audit-stamping-set-createdmodified-whowhen-on-write)
**Follow-up from:** SMA-425 (explicitly out of scope there)
**Depends on:** SMA-439 (structured `Actor`) — **Done**, merged as PR 165
**ADR context:** ADR-0014 (PRN, tenancy), ADR-0012 (per-language thin wrappers)
**Date:** 2026-08-30 (revised the same day after adversarial review)

## Problem

`AuditMetadata` is populated half-way. The "when" half works. Every tenancy aggregate
carries `created_at` and `updated_at`, fed by the injected `Clock` port
(`rs/crates/libs/paigasus-iam-core/src/ports.rs:184`).

The "who" half is hard-coded absent. `convert::audit`
(`rs/crates/services/paigasus-iam/src/adapters/grpc/convert.rs:250`) writes
`creator: None, modifier: None` unconditionally, at all six of its call sites
(`convert.rs:274, :287, :301, :312, :400, :421`). The test `audit_leaves_the_actor_unset`
(same file, ~line 530) pins that absence.

That function's own doc comment names the blocker exactly:

> `creator`/`modifier` stay ABSENT — the canonical "unknown/system" (SMA-439) — until the
> actor is PERSISTED. The acting principal is already available at every mutation
> (`actor_prn(&AuthContext)`), but no tenancy aggregate stores a creator and no migration
> defines the column, so writing it on create only would leave the field inconsistent
> across later reads.

So the work is: **persist the actor, thread it to every mutation, then let `convert::audit`
emit it.**

## What changed since the issue was filed

The issue was filed in June with two stated gates. Both are now open.

**Gate 1 — the actor representation.** SMA-439 is Done. `AuditMetadata` now carries
`Actor creator = 5` and `Actor modifier = 6` (`audit.proto:25-33`). Fields 3 and 4 are
permanently reserved, names included.

**Gate 2 — a real auditable aggregate.** Six now exist in
`contracts/proto/paigasus/iam/v1/iam.proto`: `Organization` (audit = 6, line 47), `Team`
(7, :56), `Project` (8, :66), `Membership` (4, :72), `ServiceAccount` (5, :379),
`ApiKey` (10, :401).

One issue note is **already satisfied and needs no work**: "Clock should be an injected port
for testability". The `Clock` port exists, `SystemClock` implements it (truncating to
microseconds to match Postgres `TIMESTAMPTZ`), and `FixedClock` fakes it. Every application
service **in scope here** is generic over `C: Clock` and calls `self.clock.now()`. (Two
out-of-scope services — `SystemRetirementService` at `system_retirement.rs:104` and
`DeadLetterService` — hold `Arc<dyn Clock>` instead, and `DeadLetterService::replay` calls
`Utc::now()` directly at `dead_letters.rs:156` for a metric label. Neither is in scope; the
generic-`C` claim is stated for the tenancy services only.)

## Scope

**In scope:** `Organization`, `Team`, `Project`, `Membership`.

**Out of scope:** `ServiceAccount` and `ApiKey` stamping. They keep an absent actor and a
follow-up issue closes them. `ApiKey` needs its own thought first — it has no generic
`updated_at`, and `convert::to_proto_api_key` synthesises `modified_at` as
`revoked_at.unwrap_or(created_at)`, so its modifier is a revoker, not a general editor.

Also out of scope: `CreateUser::execute` takes no actor parameter, which its own module doc
records as a deliberately deferred gap. That is an **audit-log** gap, not a stamping one —
`CreateUserResponse` is `{ string principal_prn = 1; }` and carries no `AuditMetadata`.

**No proto change.** `creator` and `modifier` already exist. Therefore no buf breaking
change, no codegen drift, and no regenerated bindings in any language. The change is Rust
and SQL only — though see "Blast radius" below, because that phrase understates it.

## Design

### D1 — `Stamp`, and its two entry paths

New type in `paigasus-iam-core`:

```rust
/// The who+when of a single write.
pub struct Stamp {
    pub at: DateTime<Utc>,
    pub by: PrincipalId,
}
```

`by` is a **`PrincipalId`, not a bare `Prn`.** `PrincipalId`
(`rs/crates/libs/paigasus-iam-core/src/value.rs:53`) is the existing wrapper asserting that
a PRN names a principal, and every caller already holds one (`AuthContext.principal_id`,
`OrganizationService::create(actor: &PrincipalId)`). A bare `Prn` would let an organization
PRN be stored as `created_by` and still compile — and with no FK (D4) and lenient reads
(Error handling), nothing downstream would catch it. Using the narrower type keeps the
check the codebase already has at the exact point where a wrong value becomes permanent.

**Every mutating port method takes `&Stamp`. There are no exceptions, and the uniformity is
the point.** The ten methods are not symmetric today, so they change in two ways. Measured:
`OrganizationRepository::create` (`ports.rs:103`), `TeamRepository::create` (`:118`),
`ProjectRepository::create` (`:131`) and `MembershipRepository::attach` (`:146`) take **no
`now` parameter at all** — the timestamp currently rides inside the entity. The six
`rename`/`set_status` methods (`:108, :110, :122, :123, :134, :135`) do take `now`.

- **create / attach** — **gain** a `stamp: &Stamp` parameter they did not have.
- **rename / set_status** — `stamp: &Stamp` **replaces** `now: DateTime<Utc>`.

The four entity constructors — `Organization::new`, `Team::new`, `Project::new`,
`Membership::new` — also take `&Stamp` in place of `now`, because the entity is the read
model too (D2) and must carry `created_by` back to the caller. `create_organization` builds
its gRPC response from the returned entity directly (`grpc/tenancy.rs:142-152`), not from a
re-read, so an entity that did not carry the creator would emit an absent one on the create
response.

**The application service constructs exactly one `Stamp`** — from `self.clock.now()` plus
the actor it was handed — and passes that same binding to both the entity constructor and
the port call. Adapters never construct one. Because there is a single binding per
operation, the entity's fields and the port's `stamp` cannot disagree; the redundancy is
deliberate, and it is what lets the adapter stamp rows that are **not** represented by an
entity at all.

That last point is not hypothetical. `OrganizationRepository::create` writes three rows in
one transaction — the org, the auto-provisioned default team, and the owner `RoleGrant`
(`organizations.rs:47-65`). Only two of them are entities. Passing the stamp to the port is
what gives the adapter an authoritative time and actor for the third, and for any row a
future implementation adds.

It also resolves D2's `MembershipRecord` problem cleanly:
`PgMembershipRepository::attach` builds that record itself (`pg_memberships.rs:237-242`) and
can now read `created_by` straight off `stamp.by`, rather than reaching back into the
entity it was handed.

**Correction to the earlier draft.** That draft gave create and attach a different door —
the stamp travelled only inside the entity, and the port signatures were left alone. It then
had to concede that the anti-drift argument covered only six of the ten methods. Taking the
stamp on every mutating port method removes the concession: a new mutating method cannot be
added without deciding what stamps it, and a new adapter implementation cannot silently
forget the actor for a row it writes.

### D2 — entities keep flat fields; `MembershipRecord` gains one

```rust
pub struct Organization {
    ...,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub created_by: Option<PrincipalId>,
    pub modified_by: Option<PrincipalId>,
}
```

The rejected alternative was one embedded `AuditStamp { created_at, updated_at, created_by,
modified_by }` on each entity, mirroring how the proto embeds `AuditMetadata`. It is the
cleaner domain shape, but it rewrites every tenancy-entity timestamp read to fix the half
that already works. **Measured: 50 sites** matching
`(node|org|organization|team|project|membership|record)\.(created_at|updated_at)` across
`rs/crates`, most of them mechanical test fixtures. Flat fields leave all 50 untouched and
match the shape the entities already have.

The stored fields are `Option` even though `Stamp.by` is not. The `Option` models a
**pre-migration row**, not a missing actor at write time. See D4.

`Membership` gets `created_by` only. Its table has no `updated_at`, and `iam.proto:72`
marks it `modified_at == created_at (immutable)`.

**`MembershipRecord` must gain `created_by` too, and this is the sharpest edge in the
change.** Both wire surfaces project `MembershipRecord` (`ports.rs:64-70`), *not* the
`Membership` entity: `to_proto_membership(r: &MembershipRecord)` (`convert.rs:307`) and
`MembershipDto::from` (`adapters/http/dto.rs:161`). `MembershipRecord` has four fields and
no creator. It is built in two ways: by `PgMembershipRepository::attach`
(`pg_memberships.rs:237-242`), and from `MembershipRow` (`:49-55`), which is filled by
**nine hand-written `SELECT` statements** across five constants — `FIND_SQL` (`:70`),
`LIST_BY_PRINCIPAL_SQL` (`:86`), `LIST_BY_ORG_SQL` (`:103`), `LIST_BY_TEAM_SQL` (`:110`)
and `LIST_BY_PROJECT_SQL` (`:117`), each of which `UNION`s an org, team and project arm.

A `SELECT` that omits `m.created_by` **compiles**. It then yields a wrong or absent creator
on one listing path only — reproducing precisely the "inconsistent across later reads"
defect this issue exists to remove. All nine need the column, and a test must prove the read
paths agree (Testing, case 5).

### D3 — the actor is always available

`Stamp.by` can be non-optional because every tenancy mutation is authenticated. This was
challenged and independently verified.

`adapters/grpc/tenancy.rs:131` calls `actor_context(&request)?` **unconditionally**, before
the `enforce_tenancy` branch, and the same pattern repeats at `:170, :210, :237, :263,
:291`. `is_exempt` lists no `TenancyService` path (`adapters/grpc/authn.rs:139-141`). Every
HTTP tenancy handler takes `Extension<AuthContext>`, and the whole tenancy sub-router sits
behind `route_layer(require_bearer)` (`adapters/http/mod.rs:864-891`). Neither
`bootstrap.rs` nor `bootstrap_admin.rs` writes a tenancy aggregate. The outbox relay
republishes events and re-applies no mutation.

`enforce_tenancy` gates **authorization**, not authentication. The handler comment at
`tenancy.rs:139` confirms it: the creating principal becomes the owner "regardless of
`enforce_tenancy`". Under `enforce_tenancy: false` a real authenticated principal still
makes the change, so `modified_by` naming them is correct — the field records **who wrote**,
not who was authorized to.

An API-key-authenticated service account arrives as a `principal` PRN
(`auth_middleware.rs:51-55`), so its stamp is indistinguishable from a human's. That is
accepted: `Actor`'s contract says the PRN's resource-type segment carries the kind, and
both are genuinely principals.

Note `grpc/tenancy.rs` is **not** among the twelve modules that define an
`actor_prn(&AuthContext)` helper — it reads `actor_context(&request)?.principal_id` inline
(`:74-76`). Of those twelve, only five modules mutate a tenancy node: `grpc/tenancy.rs` and
`http/{organizations,teams,projects,memberships}.rs`.

### D4 — migration `m0011`

Add `created_by text NULL` and `modified_by text NULL` to `organization`, `team` and
`project`. Add `created_by text NULL` to `membership` only.

`text` holding a canonical PRN, with **no foreign key**, following `audit_log.actor_prn`
(`m0006`), whose doc states the columns are "free-form PRN text, not FK'd" because "an audit
entry must survive its actor or resource being deleted later". An organization must likewise
outlive its creator's deletion.

**Follow `m0010_policy_reconcile_columns.rs:32-48`'s guards**, which are the newer
convention and which the earlier draft omitted:

- `SET LOCAL lock_timeout = '5s'`. `ALTER TABLE ... ADD COLUMN` takes `ACCESS EXCLUSIVE` on
  tables every authorization decision reads through (`pg_entity_slice.rs:51, :74, :100`).
  Without the timeout the DDL queues ahead of in-flight readers and can stall IAM during a
  rolling deploy.
- `ADD COLUMN IF NOT EXISTS`.
- A real `down()` dropping both columns. Every other migration has one.
- `CHECK (created_by IS NULL OR created_by LIKE 'prn:%')`, mirroring `m0010`'s
  `ck_policy_fingerprint`. `actor.proto` makes writing a canonical PRN a **producer**
  obligation; this enforces it where the value becomes permanent. It does not conflict with
  the lenient read rule below — the CHECK guards new writes, leniency covers rows that
  predate it.

`migration_lock.rs:12-15` states its advisory lock is not a licence to drop a migration's
own guards.

**No backfill runs.** A pre-migration row keeps `NULL`. `NULL` becomes an absent `Actor`,
and `actor.proto` already defines an absent `Actor` as *unknown-or-system*. A synthetic
backfill value would be worse than nothing: a synthetic PRN is a *valid* PRN and would read
as a real principal. The superseded test's own comment makes this point — "a synthetic
'system' PRN here would violate `Actor`'s contract".

This answers the `convert::audit` doc comment's objection. It worried that writing the actor
"on create only" leaves the field "inconsistent across later reads". Stamping every
mutation, not only create, is what removes that inconsistency.

### D5 — a write that changes nothing must not restamp, on any method

**The rule is general.** A mutation that changes no stored value leaves `updated_at` and
`modified_by` untouched. Restamping a no-op records a principal who changed nothing, which
is worse than recording no one: it is a false statement about who last touched the row, and
this issue exists to make that field trustworthy.

`set_status` already implements the branch for the timestamp
(`pg_organizations.rs:225-233`, tested at `organizations.rs:154-159`). It must leave
`modified_by` untouched too — so `modified_by` is set only in the `else` arm, next to
`updated_at`, never before the status comparison.

**`rename` does not have the branch today and gains one.** Measured:
`PgOrganizationRepository::rename` (`pg_organizations.rs:203-211`) sets
`updated_at = Set(now)` unconditionally, even when the supplied slug and name equal the
stored values. `NothingToRename` (`organizations.rs:81`) rejects only the both-*absent*
case, never the both-*identical* one.

The no-op test is: **every supplied field equals its stored value.** A `new_slug` that
matches with `new_name` absent is a no-op; a matching slug with a differing name is a real
change. Both-absent stays `NothingToRename`, unchanged.

**Guard order matters and must not change.** The existing checks run first — `NotFound`,
then the archived precondition — then the no-op test, then the slug-conflict check, then the
update. Testing for a no-op *before* the archived guard would turn
`rename(archived_node, same_slug)` from `Precondition(NodeArchived)` into a silent `Ok`.
Placing it *after* the conflict check would be harmless but pointless; placing it before also
fixes a latent oddity, since renaming a node to the slug it already holds cannot be a
`SlugTaken` conflict with itself.

**Twelve sites implement these two branches**: three `set_status` and three `rename` bodies
in `pg_organizations.rs`, `pg_teams.rs` and `pg_projects.rs`, and their six twins in the
in-memory fakes (`application/fakes.rs:109-117` for `InMemoryOrgs::set_status`, and so on).
The fakes matter because the unit tier measures the fakes; a fake and its adapter can
disagree, so both need the assertion.

**`bump_entity_gen()` stays unconditional.** It is already called on the existing
`set_status` no-op path (`pg_organizations.rs:236-238`), and the `rename` no-op follows the
same shape. Making it conditional would change Cedar cache-invalidation behaviour, which is
a separate concern from audit correctness and is not part of this issue.

**This supersedes the previous draft**, which kept `rename` restamping on the grounds that a
no-op would "change tested behaviour that no one asked to change". That reason was wrong.
Measured: of the 22 `rename` call sites across `tests/` and the application unit tests, **not
one** passes a value equal to the stored one — every case either uses a genuinely new value
or exercises an error path. Adding the no-op breaks no existing test.

### D6 — `convert::audit` takes a struct, not four positional arguments

The earlier draft proposed `audit(created, updated, creator, modifier)`. That has two
`DateTime<Utc>` and two `Option<&PrincipalId>` positions; **swapping either pair compiles**,
and two of the six call sites are mandated to pass `None, None`, so "it compiles" proves
nothing anywhere. Instead:

```rust
pub struct AuditFields<'a> {
    pub created: DateTime<Utc>,
    pub modified: DateTime<Utc>,
    pub creator: Option<&'a PrincipalId>,
    pub modifier: Option<&'a PrincipalId>,
}

pub fn audit(f: AuditFields<'_>) -> AuditMetadata
```

Named fields make a swap a visible error at the call site. A present `PrincipalId` maps to
`Some(Actor { prn: p.canonical() })`; `None` maps to `None`.

The six call sites split by scope. `to_proto_org`, `to_proto_team` and `to_proto_project`
pass all four real values. `to_proto_membership` passes `created_by` for **both** creator and
modifier — a membership is immutable and stores no `modified_by` (D2), and it already passes
`created_at` twice today (`convert.rs:312`), so this extends an existing pattern.
`to_proto_service_account` (`:400`) and `to_proto_api_key` (`:421`) pass `None` for both.

Those two explicit `None`s are deliberate. A defaulted parameter or a second builder would
hide the remaining gap; a literal `None` documents it and makes the follow-up a one-line
change per site.

**The test `audit_leaves_the_actor_unset` is superseded.** It asserts the absence this issue
removes. It becomes a test that a present PRN round-trips into `Some(Actor { prn })` and that
`None` yields `None` — the `None` half still matters, because the two out-of-scope call sites
depend on it.

### D7 — HTTP is a second wire surface

The HTTP DTOs do not embed `AuditMetadata`. They use flat `created_at`/`updated_at` JSON
fields (`adapters/http/dto.rs:50, :75, :102, :158`). They gain flat `created_by`/`modified_by`
optional string fields, following that convention rather than importing the proto's embedded
shape. Leaving HTTP out would make the two surfaces disagree about the same row.

### D8 — the twelve mutations

The actor parameter is `actor: &PrincipalId`, added to every entry point that lacks one. The
service builds the `Stamp`; adapters never construct one.

Every row passes the stamp to its port method. The `create`/`attach` rows pass the **same**
`Stamp` binding to an entity constructor as well (D1), because the entity is the read model
returned to the caller.

| Service method | Line | Has actor today | Port param | Also into |
|---|---|---|---|---|
| `OrganizationService::create` | `organizations.rs:42` | **yes** | added | `Organization::new` + `Team::new` |
| `OrganizationService::rename` | `:80` | no | replaces `now` | — |
| `OrganizationService::archive` | `:91` | no | replaces `now` | — |
| `OrganizationService::restore` | `:97` | no | replaces `now` | — |
| `TeamService::create` | `teams.rs:34` | no | added | `Team::new` |
| `TeamService::rename` | `:57` | no | replaces `now` | — |
| `TeamService::archive` | `:69` | no | replaces `now` | — |
| `TeamService::restore` | `:76` | no | replaces `now` | — |
| `ProjectService::create` | `projects.rs:38` | no | added | `Project::new` |
| `ProjectService::rename` | `:67` | no | replaces `now` | — |
| `ProjectService::archive` | `:78` | no | replaces `now` | — |
| `ProjectService::restore` | `:84` | no | replaces `now` | — |
| `MembershipService::attach` | `memberships.rs:63` | no | added | `Membership::new` |

`MembershipService::detach` (`memberships.rs:86`) is **not** stamped — it deletes the row.
See Limitations.

Call sites: thirteen in `adapters/grpc/tenancy.rs` (`:142, :221, :247, :273, :303, :365,
:387, :409, :443, :544, :570, :598` and the project `rename`), plus the twins in
`adapters/http/{organizations,teams,projects,memberships}.rs`.

**The auto-provisioned default team records the org creator, not `NULL`.**
`OrganizationService::create` (`organizations.rs:42-65`) writes org, default team and owner
grant in one transaction; it passes the **same** `Stamp` to `Organization::new` (`:47`) and
`Team::new` (`:51`). A named test asserts it.

## Data flow

For a rename, end to end:

1. The handler holds `AuthContext` (D3) and passes `&ctx.principal_id`.
2. `orgs.rename(id, new_slug, new_name, actor)`.
3. `OrganizationService::rename` builds `Stamp { at: self.clock.now(), by: actor.clone() }`.
4. `repo.rename(id, slug, name, &stamp)`.
5. The Postgres adapter sets `updated_at` and `modified_by` together, under the existing
   in-transaction guards.
6. `convert::to_proto_org` builds `AuditFields` from the entity and calls `convert::audit`.

`create` differs at steps 3–4 only in that the one `Stamp` goes to **two** places: the entity
constructor, which sets `created_by` and `modified_by` to the same value, and the port call,
which now also takes it (D1). For `OrganizationRepository::create` the adapter uses the
port's stamp for the owner `RoleGrant` row, which is not an entity and would otherwise have
no stamped source.

## Error handling

No new error variants. The actor is resolved by middleware before the handler runs, so a
missing `AuthContext` already maps to `missing_auth_context()` on gRPC and 401 on HTTP,
unchanged. A malformed **stored** PRN is not a read error either: `actor.proto` binds
consumers to read an unparseable `prn` as unknown, never as a failure. New writes are
guarded by D4's `CHECK`.

## Testing

**Unit, with `FixedClock` and the in-memory fakes.** Five cases carry the real risk:

1. A first write sets `modified_by == created_by`.
2. An update advances `modified_by` and leaves `created_by` unchanged.
3. An idempotent `set_status` no-op advances **neither** `updated_at` nor `modified_by`
   (D5).
4. A `rename` supplying values **identical** to the stored ones advances neither either
   (D5). This is new behaviour, so it needs both a positive case (identical → no-op) and a
   negative one (matching slug, differing name → a real change that *does* restamp),
   otherwise an over-broad no-op that swallows genuine renames passes.
5. Renaming an **archived** node to its own current slug still returns
   `Precondition(NodeArchived)`, not a silent `Ok` — the guard-order rule in D5.
6. `OrganizationService::create` gives the default team the org creator (D8).
7. The same membership read back through `find`, `list_by_principal` and `list_by_node`
   agrees on the creator — the nine-`SELECT` hazard in D2.

Cases 3, 4 and 5 are asserted against **both** the fake and Postgres, because the fake is
what the unit tier measures and the two can disagree (D5's twelve sites).

**`convert` unit tests.** The superseded `audit_leaves_the_actor_unset`, rewritten per D6,
plus **one test per in-scope projector** (`to_proto_org`, `to_proto_team`,
`to_proto_project`, `to_proto_membership`) feeding a **distinct** creator and modifier and
asserting each lands in its own field. A per-projector test is the only thing that catches a
swapped pair; testing `audit` in isolation cannot.

**Postgres integration.** A row with `created_by IS NULL` reads back as an absent `Actor`,
proving D4's no-backfill decision on real data. Note `start_migrated_postgres` runs
`Migrator::up(&db, None)` to the tip (`tests/support/mod.rs:73-81`), so a genuinely
"pre-`m0011`" row is not directly constructible — insert a `NULL` row, or use
`start_raw_postgres` (`:144`) with a stepwise `up`.

Also add a rename-as-A-then-as-B assertion per aggregate: `modified_by` must move. This is
the control for the Risks item below.

Run the Docker-gated suites with `PAIGASUS_REQUIRE_DOCKER=1` for any filtered run — the
Docker canary is not in the filter, and the suites would otherwise skip and report a green
that tested nothing. New Docker-backed tests must use the shared policy in
`tests/support/docker.rs` or `repo:iam-docker-policy-single-site` reds.

## Blast radius

"Rust and SQL only" is true but reads as small. The change touches roughly 40 direct
`Organization::new`/`Team::new`/`Project::new`/`Membership::new` call sites in `tests/`
(`tenancy_nodes.rs`, `tenancy_orgs.rs`, `tenancy_memberships.rs`, `authz_entity_slice.rs`,
`authz_entity_gen_bumps.rs`, `authz_forged_org_slot_escalation.rs`), nine raw-SQL row
fixtures (`tests/support/mod.rs:782`, `tests/authz_role_grants.rs:52, :68, :83`,
`tests/service_accounts.rs:133`, `tests/authz_schema.rs:47`, `tests/authz_bootstrap.rs:74`,
`tests/tenancy_schema.rs:78, :92`), and ten implementations of the four repository traits —
plus two extra `MembershipRepository` fakes inside `authenticate_api_key.rs:419` and
`authenticate_token.rs:372`.

**Unaffected, stated so no reviewer re-derives it:** `NodeView<T>` (`ports.rs:58`) needs no
change; `pg_entity_slice.rs` builds Cedar entities from `effective_status` alone, so no
authorization decision changes; and with no new crate, no new dependency and no proto edit,
`repo:affected-smoke`, the codegen-drift step and `repo:error-code-single-site` are all
unaffected.

## Limitations, accepted

**No tenancy mutation writes an `audit_log` row or a domain event.** `EventType` has eight
variants (`domain_event.rs:118-128`) and none names an organization, team, project or
membership; no tenancy repository calls `AuditLog::record`. So `created_by`/`modified_by`
become IAM's **only** record of who touched a tenancy node: last-writer-wins, no history.

**`detach` erases its actor entirely.** It deletes the row (`memberships.rs:86`,
`pg_memberships.rs:251-269`), and an org detach cascades to team and project memberships
(`:128-131`). "Who removed this principal from the organization" is arguably the
highest-value tenancy audit fact, and it stays unrecoverable after this issue. Closing it
needs an `AuditEntry` on `attach`/`detach`, which is event-trail work, not stamping work —
it belongs in a follow-up issue, filed alongside the ServiceAccount/ApiKey one.

**`Introspect` will expose membership creators.** `to_proto_membership` also feeds
`to_introspect_response` (`convert.rs:376`) and `to_introspect_api_key_response` (`:458`),
both reachable through `POST /v1/authn/introspect` and the gRPC `Introspect`, which sit
outside the bearer layer (`adapters/http/mod.rs:892-896`, `adapters/grpc/authn.rs:139-141`).
A token holder therefore learns who attached them to each node. The data is self-scoped —
you see only your own memberships — so this is accepted, but it is a deliberate disclosure,
not an oversight.

## Risks

**The largest risk is a silently stale `modified_by`, and the type system does not stop
it.** In `PgOrganizationRepository::rename` (`pg_organizations.rs:203-211`) the required
edit is one line, `active.modified_by = Set(...)`, beside the existing
`active.updated_at = Set(now)`. Omitting it **compiles**, and SeaORM leaves a `NotSet` field
out of the `UPDATE`, so the previous `modified_by` survives — naming a principal who did not
make the change. **Six Postgres assignment sites** have this shape: the update arm of three
`rename` bodies and of three `set_status` bodies. (These six are the *lines that write the
column*. D5's twelve are the *no-op branches*, counted across both the adapters and the
fakes. The two counts measure different things and are both right.)

The earlier draft claimed "a missed site does not compile". **That claim was wrong** and is
withdrawn. The real controls are behavioural, not structural: the rename-as-A-then-as-B
assertion per aggregate (Testing), and the nine-`SELECT` read-agreement test.

There is now a stronger structural option than there was, and it is **recommended**. After
D5 both `rename` and `set_status` have the same shape — a no-op arm and an update arm — so a
single private helper on each adapter that takes the `&Stamp` and sets `updated_at` and
`modified_by` **together**, called only from the update arm, makes it impossible to write one
without the other. That collapses this risk and D5's into one reviewed place per aggregate.
The tests remain the requirement; the helper is how to stop needing them to catch it.

**Second risk: partial threading.** Thirteen service methods and five adapter modules across
two transports. Here the type system does help, and after D1's revision it helps everywhere:
**all ten mutating port methods change signature** — six replacing `now`, four gaining a
parameter — and all four entity constructors change too. A missed call site fails to
compile. The gap is only the assignment-line risk above.

**Third risk: an over-broad `rename` no-op.** D5 adds a value-equality branch that does not
exist today. Written too greedily — comparing only the slug, say, or treating an absent
`new_name` as "matching" when a name was in fact supplied — it would swallow real renames
and silently stop updating rows. That is a data-correctness bug, not merely an audit one.
Testing case 4's negative half is the control, and it is why that case is specified as a
pair rather than a single assertion.
