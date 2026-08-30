# SMA-440 — Server-side audit stamping (set created/modified who+when on write)

**Issue:** [SMA-440](https://linear.app/smaschek/issue/SMA-440/server-side-audit-stamping-set-createdmodified-whowhen-on-write)
**Follow-up from:** SMA-425 (explicitly out of scope there)
**Depends on:** SMA-439 (structured `Actor`) — **Done**, merged as PR 165
**ADR context:** ADR-0014 (PRN, tenancy), ADR-0012 (per-language thin wrappers)
**Date:** 2026-08-30

## Problem

`AuditMetadata` is populated half-way. The "when" half works. Every tenancy aggregate
carries `created_at` and `updated_at`, fed by the injected `Clock` port
(`rs/crates/libs/paigasus-iam-core/src/ports.rs:184`).

The "who" half is hard-coded absent. `convert::audit`
(`rs/crates/services/paigasus-iam/src/adapters/grpc/convert.rs:250`) writes
`creator: None, modifier: None` unconditionally, on all six of its call sites. The test
`audit_leaves_the_actor_unset` (same file, ~line 530) pins that absence.

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

**Gate 1 — the actor representation.** The issue says stamping uses "whatever actor
representation wins" in SMA-439. SMA-439 is Done. `AuditMetadata` now carries
`Actor creator = 5` and `Actor modifier = 6`. Fields 3 and 4 are permanently reserved,
names included.

**Gate 2 — a real auditable aggregate.** The issue is "gated on the first real domain
auditable aggregate". Six now exist in `contracts/proto/paigasus/iam/v1/iam.proto`:
`Organization` (audit = 6), `Team` (7), `Project` (8), `Membership` (4),
`ServiceAccount` (5), `ApiKey` (10).

One issue note is **already satisfied and needs no work**: "Clock should be an injected
port for testability". The `Clock` port exists, `SystemClock` implements it (truncating to
microseconds to match Postgres `TIMESTAMPTZ`), `FixedClock` fakes it, and every application
service that mints a timestamp is generic over `C: Clock`. No application service calls
`Utc::now()` directly.

## Scope

**In scope:** `Organization`, `Team`, `Project`, `Membership`.

**Out of scope:** `ServiceAccount` and `ApiKey` stamping. They keep an absent actor and a
follow-up issue closes them. `ApiKey` needs its own thought first — it has no generic
`updated_at`, and `convert::to_proto_api_key` synthesises `modified_at` as
`revoked_at.unwrap_or(created_at)`, so its modifier is a revoker, not a general editor.

Also out of scope: `CreateUser::execute` takes no actor parameter, which its own module doc
records as a deliberately deferred gap. That is an **audit-log** gap, not a stamping one —
`CreateUserResponse` is `{ string principal_prn = 1; }` and carries no `AuditMetadata`.
Nothing on the wire is wrong because of it. It does not belong to this issue.

**No proto change.** `creator` and `modifier` already exist. Therefore no buf breaking
change, no codegen drift, and no regenerated bindings in any language. The change is Rust
and SQL only.

## Design

### D1 — `Stamp`, a write-path value object

New type in `paigasus-iam-core`:

```rust
/// The who+when of a single write. Carried as one value so a mutation cannot advance the
/// timestamp without naming the actor — the inconsistency `convert::audit` warns about.
pub struct Stamp {
    pub at: DateTime<Utc>,
    pub by: Prn,
}
```

It **replaces** the `now: DateTime<Utc>` parameter on every mutating port method, rather
than joining it as a sibling. Two parallel parameters can drift: a future method could take
`now` and forget the actor, which reopens the hole this issue exists to close. One parameter
cannot.

`by` is a plain `Prn`, not `Option<Prn>`. D3 justifies this.

### D2 — entities keep flat fields

```rust
pub struct Organization {
    ...,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub created_by: Option<Prn>,
    pub modified_by: Option<Prn>,
}
```

The rejected alternative was one embedded `AuditStamp { created_at, updated_at, created_by,
modified_by }` on each entity, mirroring how the proto embeds `AuditMetadata`. It is the
cleaner domain shape, but it rewrites every `org.created_at` read across both crates —
**129 `.created_at`/`.updated_at` sites, measured** — to fix the half that already works.
That is unrelated refactoring. Flat fields leave all 129 untouched.

The stored fields are `Option<Prn>` even though `Stamp.by` is not. The `Option` models a
**pre-migration row**, not a missing actor at write time. See D4.

`Organization::new`, `Team::new` and `Project::new` take `&Stamp` in place of `now`, and set
`modified_by = created_by` on the first write. `AuditMetadata`'s own contract states
`modified_at` equals `created_at` on first write; this extends the same rule to the actor.

`Membership` gets `created_by` only. Its table has no `updated_at`, and `iam.proto:72`
marks it `modified_at == created_at (immutable)`.

### D3 — the actor is always available

`Stamp.by` can be non-optional because every tenancy mutation is authenticated.

Measured: `adapters/grpc/tenancy.rs:131` calls `actor_context(&request)?`
**unconditionally**, before the `enforce_tenancy` branch. The helper reads `AuthContext`
from the request extensions and returns `missing_auth_context()` rather than panicking. Its
doc records that the `AuthLayer` in `grpc::mod` always resolves one first for a non-exempt
RPC. The HTTP surface has the same shape: `AuthContext` is attached by `require_bearer`
(`adapters/http/auth_middleware.rs:69`), and all twelve HTTP and gRPC adapter modules
already define an `actor_prn(&AuthContext)` helper.

`enforce_tenancy` gates **authorization**, not authentication. The handler comment at
`tenancy.rs:139` confirms the distinction: the creating principal becomes the owner
"regardless of `enforce_tenancy`".

Bootstrap is not a counter-example. `bootstrap_admin.rs` grants platform-admin roles and
creates no tenancy aggregate. Where it does record an actor-less event it already writes
`actor_prn: None` deliberately, with a test asserting *"SMA-468 D2: no principal authorized
this — configuration did"*. That convention is untouched here.

### D4 — migration `m0011`, and why there is no backfill

Add `created_by text NULL` and `modified_by text NULL` to `organization`, `team` and
`project`. Add `created_by text NULL` to `membership` only.

`text`, holding a canonical PRN, with **no foreign key**. This follows the settled
precedent of `audit_log.actor_prn` in `m0006`, whose migration doc states the columns are
"free-form PRN text, not FK'd" because "an audit entry must survive its actor or resource
being deleted later". An organization must likewise outlive its creator's deletion.

**No backfill runs.** A pre-migration row keeps `NULL`. `NULL` becomes an absent `Actor`,
and `actor.proto`'s contract already defines an absent `Actor` as *unknown-or-system*. The
existing contract covers historical rows exactly, so inventing a synthetic backfill value
would be worse than leaving them alone: a synthetic PRN is a *valid* PRN and would read as a
real principal. The superseded test's own comment makes this point — "a synthetic 'system'
PRN here would violate `Actor`'s contract".

This is the answer to the `convert::audit` doc comment's objection. It worried that writing
the actor "on create only" leaves the field "inconsistent across later reads". Stamping
every mutation, not only create, is what removes that inconsistency.

### D5 — the idempotent no-op must not restamp

`archive` on an already-archived node is a documented no-op that leaves `updated_at`
untouched (`organizations.rs`, decision D10, with an existing test asserting it). It must
leave `modified_by` untouched too.

This is the one place where a correct-looking implementation is wrong. If `set_status`
writes the modifier before testing whether the status actually changed, a no-op silently
records a principal who changed nothing. The timestamp and the actor must move together, or
not at all.

### D6 — `convert::audit` stays the single choke point

```rust
pub fn audit(created: DateTime<Utc>, updated: DateTime<Utc>,
             creator: Option<&Prn>, modifier: Option<&Prn>) -> AuditMetadata
```

A present `Prn` maps to `Some(Actor { prn: p.canonical() })`; `None` maps to `None`.

Its six call sites split by scope. `to_proto_org`, `to_proto_team` and `to_proto_project`
pass all four real values. `to_proto_membership` passes `created_by` for **both** the creator
and the modifier, because a membership is immutable and stores no `modified_by` (D2) — it
already passes `created_at` twice today (`convert.rs:312`), so this extends the existing
pattern rather than inventing one. `to_proto_service_account` and `to_proto_api_key` pass
`None, None`.

Those two explicit `None`s are deliberate. A second builder, or a defaulted parameter, would
hide the remaining gap; a literal `None` at the call site documents it and turns the
follow-up issue into a one-line change per site.

**The test `audit_leaves_the_actor_unset` is superseded.** It asserts the absence this issue
removes, so it cannot survive unchanged. It becomes a test that a present PRN round-trips
into `Some(Actor { prn })` and that `None` yields `None`. Deleting it outright would drop
the `None` half, which the two out-of-scope call sites still rely on.

### D7 — HTTP is a second wire surface

The HTTP DTOs do not embed `AuditMetadata`. They use flat `created_at`/`updated_at` JSON
fields (`adapters/http/dto.rs:50`, `:75`, `:102`, `:158`). They gain flat
`created_by`/`modified_by` optional string fields, following that existing convention rather
than importing the proto's embedded shape.

Both surfaces must be threaded. Leaving HTTP out would make the two disagree about what IAM
knows about the same row.

## Data flow

For a rename, end to end:

1. The handler holds `AuthContext` (middleware-attached, D3) and reads
   `ctx.principal_id.prn()`.
2. It calls `orgs.rename(id, new_slug, new_name, actor)`.
3. `OrganizationService::rename` builds `Stamp { at: self.clock.now(), by: actor.clone() }`.
4. It calls `repo.rename(id, slug, name, &stamp)`.
5. The Postgres adapter sets `updated_at = stamp.at` and `modified_by = stamp.by`, in the
   same statement, under the existing in-transaction guards.
6. `convert::to_proto_org` reads the entity's four fields and calls `convert::audit`.

`create` differs at step 3 only: it sets `created_by` and `modified_by` to the same value.

## Error handling

No new error variants. The actor is resolved by middleware before the handler runs, so a
missing `AuthContext` already maps to `missing_auth_context()` on gRPC and a 401 on HTTP,
unchanged by this work. A malformed stored PRN is not an error either: `actor.proto` binds
consumers to read an unparseable `prn` as unknown, never as a failure.

## Testing

**Unit, with `FixedClock` and the in-memory fakes.** Three cases carry the real risk:

1. A first write sets `modified_by == created_by`.
2. An update advances `modified_by` and leaves `created_by` unchanged.
3. An idempotent no-op advances neither (D5). This is the test most likely to fail against a
   plausible implementation.

**`convert` unit tests.** The superseded `audit_leaves_the_actor_unset`, rewritten per D6,
covering both the present and the absent mapping.

**Postgres integration.** A row written before `m0011` reads back with an absent `Actor`,
proving D4's no-backfill decision on real data rather than on a fake.

Run the Docker-gated suites with `PAIGASUS_REQUIRE_DOCKER=1` for any filtered run, because
the Docker canary is not in the filter and the suites would otherwise skip and report a
green that tested nothing.

## Risks

**The largest risk is D5**, the idempotent no-op. It is the only case where the obvious
implementation is silently wrong, and it produces bad audit data rather than a failure.

**A second risk is partial threading.** Twelve adapter modules and two transports touch this.
A missed mutation leaves a stale `modified_by` — worse than an absent one, because it names
a principal who did not make the change. Making `Stamp` replace `now` rather than sit beside
it is the structural defence: a missed site does not compile.
