# SMA-606 — Tenancy audit-log entries and domain events on every mutation

**Linear:** [SMA-606](https://linear.app/smaschek/issue/SMA-606/tenancy-audit-log-entries-and-domain-events-on-every-mutation-incl)
**Depends on:** SMA-440 Part 1, merged as `c3b9ee9` (PR #195).
**Measurements:** `2026-09-01-sma-606-measurements.md` — every fact this document asserts about
the current tree was measured there, at that commit. It also records three claims in SMA-440's
Part 2 that measurement contradicted; those corrections are folded in below and marked.

## Problem

Every tenancy mutation must write an `AuditEntry` and raise a `DomainEvent`. Today none of them
do: `EventType` has eight variants, none naming a tenancy aggregate, and no tenancy repository
calls `AuditLog::record`.

That leaves SMA-440's stamped `created_by`/`modified_by` columns as IAM's only record of who
touched a node — last-writer-wins, with no history — and a `detach` erases its actor completely,
because it deletes the row those columns live on.

SMA-440's Part 2 (decisions P2-D1 … P2-D5, in
`2026-08-30-sma-440-server-side-audit-stamping-design.md`) designed this work and is the input
here. This document supersedes it where the two disagree, and says so at each point.

## Scope

**In scope.** All thirteen row-preserving tenancy mutations plus `detach`: organization, team and
project `create`/`rename`/`archive`/`restore`, and membership `attach`/`detach`. The transactional
refactor that makes an atomic outbox write possible. Fourteen new `EventType` variants. Two
corrections to existing tests that this change makes load-bearing.

**Out of scope.** Adding a tenancy subject to `CONSUMER_FILTER_SUBJECTS` so the gateway's decision
cache reacts to, say, an org archive — a separate downstream decision, as P2-D4 already states.
`ServiceAccount` and `ApiKey` stamping (SMA-440's own out-of-scope list). Any change to
authorization, which stays in the transport adapters.

**No contract change, no gate change.** Measured: `EventType` has no proto twin;
`DeadLetterEntry.event_type` and `AuditEntry.action` are plain `string`s; the NATS payload is
CloudEvents JSON (`adapters/events/cloud_event.rs`), never protobuf. `ops/nats/subjects.env` grants
the wildcard `iam.>`, so no NATS config moves. `ci/error-registry/check.py`'s `MANIFEST` lists no
tenancy service file and no new error code is introduced. `repo:observability-drift` keys on metric
families, not event types.

## Design

### D1 — `_in` twins, and what they return

All four tenancy repositories open and commit their own transaction internally and never expose
it, so an outbox row cannot be made atomic with the mutation it describes. The house pattern for
fixing this already exists four times over — `PrincipalRepository::create_user_in`,
`ServiceAccountRepository::create_in`, `ApiKeyRepository::issue_in`/`revoke_in` — and the tenancy
ports have no twins.

Add `create_in` / `rename_in` / `set_status_in` to the three node ports and `attach_in` /
`detach_in` to `MembershipRepository`, each taking `tx: &dyn Transaction` as its first parameter
after `&self`. Note `&dyn`, not `&mut dyn`: `Transaction` is `Send + Sync` precisely so
`#[async_trait]`'s `Send` bound holds, and every existing `_in` call site uses the `&*tx` deref
idiom over a `Box<dyn Transaction>`.

**The existing ten methods stay**, re-expressed as thin "open a one-shot UoW, delegate, commit"
wrappers over their twins — the shape `ApiKeyRepository::revoke` already documents. This is not
dead API surface kept for symmetry: roughly forty test fixtures across `tenancy_orgs.rs`,
`tenancy_nodes.rs`, `tenancy_memberships.rs` and the `authz_*` suites call these methods directly
to build state, and `tests/api_key_auth.rs:137,:220` is the existing precedent for a non-`_in`
wrapper surviving exactly because fixtures use it.

`create_in` returns `()`, as `create` does today. The team and project services keep their existing
post-`create` refetch (`teams.rs:41`, `projects.rs:51`) — it runs after `commit()`, which is
exactly where it effectively runs today, since the repository committed before returning. A
`create` is never a no-op, so it needs no `Mutated`.

**`rename_in` and `set_status_in` return `Mutated<NodeView<T>>`**, a new type beside `NodeView` in
`paigasus-iam-core`'s `ports.rs`:

```rust
/// A mutation's result plus whether it actually changed anything. `changed == false` is a
/// no-op — SMA-440 D5's "a write that changes nothing stamps nothing", extended here to
/// "and emits nothing".
pub struct Mutated<T> {
    pub value: T,
    pub changed: bool,
}
```

The service needs a no-op signal to satisfy P2-D3's "a no-op emits nothing", and the tenancy
methods — unlike `revoke_in`, which returns only a `bool` — must also return the `NodeView`. A
named struct rather than a `(NodeView<T>, bool)` tuple: `.changed` states intent at the ten
implementation sites and both call sites, where `.1` would not.

`attach_in` returns `MembershipRecord`, as `attach` does today. `detach_in` returns
`Vec<MembershipRecord>` — see D6.

**Ten implementations change**, measured: four Postgres adapters, four fakes in
`application/fakes.rs`, and the two extra `InMemoryMemberships` in `authenticate_api_key.rs` and
`authenticate_token.rs`. That is 26 new method bodies. The fakes' wrappers pass the existing
`CountingTransaction`.

### D2 — the service shape

The reference is `ApiKeyService::issue` (`application/api_keys.rs:204-283`). Per mutation:

1. Mint **one** `correlation_id`, shared by every event and entry the call produces.
2. Build all events and entries **before** any I/O.
3. `let tx = self.uow.begin().await?;`
4. The `_in` call, then `outbox.enqueue(&*tx, &event)`, then `audit.record(&*tx, &entry)`.
5. `tx.commit().await?;`
6. The post-commit generation bump (D7).

**One deliberate departure from that reference: authorization is not copied.** `ApiKeyService`
owns an `authorize` field and checks before mutating. The tenancy services own none — authorization
happens in the transport adapters, gated by `AppState.enforce_tenancy`, fetch-first and against the
stored PRN (`adapters/http/teams.rs:52-57` is the canonical shape). That stays exactly where it is.
P2-D3's recipe opens with "authorize"; here that step is already done by the caller.

The four services move from positional `new(repo, ids, clock)` to `*ServiceDeps` params structs,
the house pattern (`ApiKeyServiceDeps`, `RoleServiceDeps`, `CreateUserDeps`), gaining
`uow: Arc<dyn UnitOfWork>`, `outbox: Arc<dyn Outbox>`, `audit: Arc<dyn AuditLog>`, and — for the
three node services — `gen_bumper: Arc<dyn EntityGenBumper>`. `OrganizationService` additionally
takes `policy_gen_bumper: Arc<dyn PolicyGenBumper>`, because its `create` writes the owner grant.
`MembershipService` takes neither bumper (D7).

**A composition-root reordering is required.** The four services are constructed at
`adapters/http/mod.rs:355-363`; the shared `audit_log` they now depend on is created at `:394`.
The services move below it. The `pub type OrgSvc` / `TeamSvc` / `ProjectSvc` / `MembershipSvc`
aliases at `:94-97` change with the generic parameters.

**A no-op emits nothing.** Where `Mutated.changed` is false, the `enqueue` and `record` calls are
skipped — `ApiKeyService::revoke`'s `did_revoke` branch, exactly. The commit still happens, and so
does the bump (D7).

### D3 — fourteen new `EventType` variants

`iam.organization.{created,renamed,archived,restored}`, the `iam.team.*` and `iam.project.*`
quartets, and `iam.membership.{attached,detached}`. These map 1:1 onto the fourteen tenancy
`Action` variants that `authz/action.rs:13` already declares — `CreateOrganization` through
`DetachMembership` — so the vocabulary is not invented here, only mirrored.

`set_status` produces `archived` or `restored` according to the target `NodeStatus`, not one
`status_changed` variant: a consumer filtering for archival should not have to parse a payload.

`EventType::ALL` goes `[EventType; 8]` → `[EventType; 22]`, and `as_wire`/`parse` gain their arms.

### D4 — `OrganizationService::create` emits three events

`OrganizationRepository::create` writes three rows in one transaction: the organization, the
auto-provisioned default team (ADR-0014), and the creating principal's `org_admin` owner grant.

It emits **three** events on one `correlation_id`: `iam.organization.created`, `iam.team.created`,
and the **existing** `iam.role.granted`.

This applies P2-D5's own principle — one event per row, so per-node provenance stays queryable —
symmetrically to create. The alternative, one org event describing the other two rows in its
payload, would leave the default team with no creation event while an explicitly created team has
one, and would hide the owner grant from the same query that finds every other role grant. Reusing
`RoleGranted` rather than minting a tenancy-specific variant keeps one event type per real-world
fact; `RoleService::grant` becomes its second emitter, which is why the payload shape must match
that emitter's (`{"grant_id", "role_key", "scope"}`).

### D5 — audit entries

One `AuditEntry` per `DomainEvent`, sharing its `correlation_id` and `occurred_at`.

- `action` — the string literal matching the `Action` variant (`"RenameTeam"`, `"AttachMembership"`).
  There is no `Action::as_str`; `roles.rs:236`, `policies.rs:137` and `api_keys.rs:264` all write
  the literal, and this follows them.
- `actor_prn` — `Some(stamp.by.canonical())`, i.e. SMA-440's `Stamp.by`. For `detach`, which has
  no `Stamp`, the actor parameter D6 adds.
- `resource_prn` — the node's PRN. For a membership, the node it attaches to.
- `outcome` — `AuditOutcome::Committed`. These entries are written only on a path that commits.
- `determining_policies` — **empty**. Authorization ran in the transport adapter, outside this
  service, and denials are already recorded independently by `adapters/authz/denial_audit.rs`.
  Populating it here would mean threading the Cedar decision through the service for no consumer.
- `detail` — the same shape as the event's payload.

### D6 — detach

**`MembershipService::detach` gains an actor.** It is `detach(&self, id: Uuid)` today
(`memberships.rs:85-87`) — SMA-440 left it unstamped deliberately, because it deletes the row and
there is nothing to stamp. An `AuditEntry.actor_prn` needs one regardless, so the signature gains
`actor: &PrincipalId`, updating `adapters/grpc/tenancy.rs:653` and
`adapters/http/memberships.rs:96`.

**`detach_in` returns what it deleted.** An org detach cascades to the principal's team and project
memberships in that org (`DETACH_CASCADE_SQL`, `pg_memberships.rs:134-141`). Each deleted row gets
its own entry and its own event, all on one `correlation_id`, so "when did this principal lose
access to project X" is answerable by filtering on that project's PRN — the fact the trail exists
to expose, and one a single org-detach event with the cascade buried in a payload would hide.

The service cannot build those records itself: it holds only a `Uuid`, and the `membership` table
stores no PRN columns at all, only FK uuids. The PRNs exist solely inside the repository, which
resolves them by join.

**Mechanism: SELECT-then-DELETE inside the transaction.** `detach_in` selects the rows it is about
to delete — the target row, plus everything the cascade will remove — then deletes, then returns
the `Vec<MembershipRecord>`.

Concretely: one new `SELECT` constant beside the existing five, built from
`LIST_BY_PRINCIPAL_SQL`'s PRN-joining projection (`pg_memberships.rs:96-109`) so it populates
`MembershipRow` through the same mapping every read path uses, with `DETACH_CASCADE_SQL`'s
`WHERE` clause verbatim plus the target row's own id. Reusing that projection is the point — it is
what keeps the sixth statement from being a sixth chance to omit a column.

The rejected alternative is a `DELETE … RETURNING` CTE. Inside one transaction with the target row
already locked (`lock_exclusive()`, `pg_memberships.rs:266`) there is no TOCTOU gap for
SELECT-then-DELETE to open, so `RETURNING` buys no correctness. It costs something, though: a
`DELETE` cannot join to the `organization`/`team`/`project`/`principal` tables the PRNs come from,
so the CTE would have to rebuild that projection in new hand-written SQL. Both options add a sixth
statement; only one of them adds a sixth *projection*, and a projection that omits a column
compiles and goes wrong on one path only — SMA-440's D2 documents that exact failure across the
existing five.

Note today's `detach` discards even the cascade's row count: the `txn.execute(stmt)` result is
dropped at `pg_memberships.rs:275`.

### D7 — the generation bump moves, and needs a port that does not exist

**Correction to P2-D2.** It says `bump_entity_gen`/`bump_policy_gen` "must move" to the service,
post-commit. Measured: there is no way for a service to call them. `bump_entity_gen` is a *private
inherent* method on each Pg repository (`pg_organizations.rs:46`, `pg_teams.rs:37`,
`pg_projects.rs:37`) delegating to a `Generations` field, and ADR-0005 keeps `Generations` out of
the application layer. Only `PolicyGenBumper` exists as a port (`ports.rs:367-370`), with adapter
`GenerationsPolicyGenBumper`.

So this adds `EntityGenBumper` to `ports.rs` and `GenerationsEntityGenBumper` to
`adapters/authz/generation.rs`, mirroring the policy pair line-for-line, including its
swallow-and-log posture: a failed cache-invalidation bump must never fail an already-committed
write.

Why it must move at all: this is a Cedar cache-invalidation path. Left inside a repository that no
longer commits, it would invalidate against a transaction that may still roll back.

**The bump still fires on a no-op.** Today both the adapter and the fake no-op paths commit and
bump (`pg_organizations.rs:225`, and the `set_status` idempotency arms). Making it conditional
would change cache-invalidation behaviour, which is a separate concern from audit correctness —
SMA-440's D5 says so explicitly and this preserves it. The visible consequence is that a no-op
rename invalidates the Cedar cache while emitting no event; that is current behaviour plus
nothing.

The repositories keep their private bumps, which now serve only the non-`_in` wrapper path that
test fixtures use. There is no double bump: a service calls `_in` and bumps through the port; a
fixture calls the wrapper and gets the repository's own bump.

**`MembershipService` gets no bumper.** `PgMembershipRepository` has no `gens` field and bumps
nothing today — and that is correct, not an oversight: `pg_entity_slice.rs` never reads
memberships, so a membership change invalidates nothing. Stated here so this work does not grow a
bump that never existed.

### D8 — two tripwire corrections

**Correction to P2-D4.** It names three tripwires that "fail the build until they agree". Measured:
only two do.

`EventType::ALL`'s fixed-size array and `all_lists_every_event_type`'s wildcard-free match are
genuine — a new variant does not compile until both move. But
`type_matches_the_wire_string_for_every_variant` (`adapters/events/cloud_event.rs:158-176`)
**hard-codes an eight-element array literal** rather than iterating `EventType::ALL`. A new variant
compiles cleanly and is silently uncovered.

It changes to iterate `EventType::ALL`, becoming the tripwire the design already believed it was.

**A fourth hand-listed site P2-D4 does not mention at all.**
`no_payload_shape_carries_a_secret_or_pii_key` (`cloud_event.rs:181-195`) hand-lists four payload
shapes and asserts none renders a banned key (`hash`, `secret`, `plaintext`, `email`, `pepper`,
`token`, `password`). The new tenancy payload shapes are not covered until added. This is a
secret-safety control, so it gets the new shapes explicitly rather than being converted to
something derived.

`tests/nats_permissions.rs:324` genuinely iterates `EventType::ALL`, so the NATS grant assertion
covers the new variants automatically. P2-D4 is right about that one.

### D9 — payload shapes

| Event | Payload / detail |
|---|---|
| `iam.{organization,team,project}.created` | `{"node_prn", "slug", "name", "status"}` |
| `iam.{organization,team,project}.renamed` | `{"node_prn", "slug", "name"}` — post-change values |
| `iam.{organization,team,project}.{archived,restored}` | `{"node_prn", "status"}` |
| `iam.membership.attached` | `{"membership_id", "principal_prn", "node_prn"}` |
| `iam.membership.detached` | `{"membership_id", "principal_prn", "node_prn"}` |
| `iam.role.granted` (from org create) | `{"grant_id", "role_key", "scope"}` — matches `RoleService`'s existing shape |

`aggregate_prn` is the node's PRN for node events and, for membership events, the node the
membership attaches to — so a consumer filtering one node's history sees its attachments.

None of these carries a secret or PII; slug and name are already public on both wire surfaces.

## Data flow

For a team rename, end to end:

1. The HTTP handler authorizes (`enforce_tenancy` path) and calls
   `teams.rename(id, slug, name, &ctx.principal_id)`.
2. `TeamService::rename` builds `Stamp { at: clock.now(), by: actor.clone() }`, mints one
   `correlation_id`, and builds the `DomainEvent` and `AuditEntry`.
3. `let tx = self.uow.begin()`.
4. `repo.rename_in(&*tx, id, slug, name, &stamp)` → `Mutated { value, changed }`.
5. If `changed`: `outbox.enqueue(&*tx, &event)`, `audit.record(&*tx, &entry)`.
6. `tx.commit()`.
7. `self.gen_bumper.bump().await` — post-commit, awaited, unconditional.
8. Return `Mutated.value`.

For an org create, step 2 builds three events and three entries on one `correlation_id`, step 5
enqueues and records all three, and step 7 bumps both entity and policy generations.

For an org detach, step 4 returns N records and step 2's work moves after it — the only mutation
whose events cannot be built before the `_in` call, because the repository is what discovers how
many rows there are. The `correlation_id` is still minted first and shared by all N.

## Error handling

No new error variants. An `enqueue` or `record` failure is a `RepositoryError` that propagates and
aborts the transaction, so the mutation rolls back with its event — which is the point of the
outbox. A failed post-commit bump is swallowed and logged, per D7.

## Testing

**Unit, against the fakes, with `FixedClock` and the existing `FakeUnitOfWork` / `FakeOutbox` /
`FakeAuditLog`:**

1. Each of the twelve single-event mutations — the thirteen row-preserving ones minus
   `OrganizationService::create`, which case 4 covers — emits exactly one event and one entry
   sharing one `correlation_id`, with the expected wire string and action.
2. A `rename` supplying values identical to the stored ones emits **nothing** — and the negative
   half, a matching slug with a differing name, emits normally. (SMA-440 D5's case 4, extended
   from "does not restamp" to "does not emit".)
3. An idempotent `set_status` no-op emits nothing.
4. `OrganizationService::create` emits three events on one `correlation_id`, and the `team.created`
   one names the default team.
5. An org `detach` that cascades emits one event and one entry per deleted row, all on one
   `correlation_id`, and the project row's event carries that project's PRN.
6. A mutation that fails mid-transaction leaves no event and no entry —
   `FailingRevokeApiKeys` (`api_keys.rs:465-493`) is the shape to copy.
7. The post-commit bump fires after a successful commit and **not** before it; and it still fires
   on a no-op. `FakeUnitOfWork::commits()` plus a counting bumper is the mechanism.

**Postgres integration**, Docker-gated through `tests/support/docker.rs`:

8. Commit makes the tenancy row, its outbox row and its audit row visible together; a rollback
   leaves none of the three. `tests/outbox_uow_pg.rs` is the template.
9. An org detach's cascade writes one audit row per deleted membership, with the right PRNs —
   the case that cannot be proven against a fake, since the fakes do not implement the cascade join.

**End to end:** extend `tests/mutation_audit_e2e.rs`, which already drives HTTP → row → relay, with
a tenancy mutation.

Naming follows the crate's style: `snake_case`, full-sentence, behaviour-asserting, with a doc
comment citing the decision id.

Run filtered suites with `PAIGASUS_REQUIRE_DOCKER=1` — the Docker canary is not in the filter, so
the suites would otherwise skip and report a green that tested nothing.

## Blast radius

Ten repository implementations, four services, four service constructors and their type aliases,
one composition root, two transport adapters (for `detach`'s new parameter), one new port, one new
adapter, one new value type, and the four `EventType` sites. The forty-odd existing fixture call
sites are untouched, which is what D1's retained wrappers buy.

No proto change, no codegen, no migration, no NATS config, no `repo:*` gate registration.

## Limitations, accepted

**The gateway's decision cache does not react to tenancy events.** Adding a tenancy subject to
`CONSUMER_FILTER_SUBJECTS` is a separate decision, already declared out of scope by P2-D4. Until
then these events are written, relayed and published, but no in-tree consumer filters on them —
the audit trail is the consumer.

**A no-op rename still invalidates the Cedar cache.** D7 preserves current behaviour rather than
making the bump conditional. It costs a cache refill on a write that changed nothing.

**`determining_policies` is empty on every tenancy entry.** D5's reasoning. An operator
reconstructing *why* a mutation was permitted must join to the denial-audit trail or the policy
snapshot; the entry records that it was permitted and by whom, not under which policy.

## Risks

**The largest risk is an event emitted for a mutation that rolled back, or suppressed for one that
committed.** Both reduce to getting the `changed` flag and the commit boundary right across ten
implementations. The control is Testing case 6 (mid-transaction failure leaves nothing) paired
with case 2's negative half (a real change does emit) — either alone passes a wrong
implementation.

**Second: the fakes and the adapters can disagree**, and the unit tier measures only the fakes.
SMA-440 hit this exact shape — its D5 required the same assertion against both, because the fake's
no-op branch sits in a different position relative to the conflict guard than the adapter's. Cases
2, 3 and 5 therefore run against Postgres as well as the fakes.

**Third: `detach_in`'s SELECT and its DELETE can drift.** The cascade `DELETE` and the new
`SELECT` that predicts it are two statements that must describe the same row set. If they diverge,
the trail silently under- or over-reports. They are written adjacently and share the cascade's
`WHERE` clause; case 9 is the control, and it must assert the audit row count equals the deleted
row count rather than merely that some rows appeared.

**Fourth, and cheap to miss: `type_matches_the_wire_string_for_every_variant` is not currently a
tripwire.** Until D8 lands, adding fourteen variants leaves fourteen wire strings unasserted. Do
D8's one-line change to that test *before* adding the variants, so the new arms are covered as
they are written rather than after.
