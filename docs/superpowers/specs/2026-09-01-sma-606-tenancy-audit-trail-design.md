# SMA-606 — Tenancy audit-log entries and domain events on every mutation

**Linear:** [SMA-606](https://linear.app/smaschek/issue/SMA-606/tenancy-audit-log-entries-and-domain-events-on-every-mutation-incl)
**Depends on:** SMA-440 Part 1, merged as `c3b9ee9` (PR #195).
**Measurements:** `2026-09-01-sma-606-measurements.md` — the facts this document rests on, and
three claims in SMA-440's Part 2 that measurement contradicted.

**Revision 2**, after an adversarial review. Three of revision 1's own claims were wrong and are
corrected here: `Action::as_wire()` exists (so D5 no longer hand-writes literals),
`EventType::RoleGranted` already has two emitters with a `"source"` convention (D4), and events
cannot be built before the `_in` call for most mutations (D2). Each is marked **[R2]** below.

## Problem

Every tenancy mutation must write an `AuditEntry` and raise a `DomainEvent`. Today none do:
`EventType` has eight variants, none naming a tenancy aggregate, and no tenancy repository calls
`AuditLog::record`.

That leaves SMA-440's stamped `created_by`/`modified_by` columns as IAM's only record of who
touched a node — last-writer-wins, no history — and a `detach` erases its actor completely, because
it deletes the row those columns live on.

SMA-440's Part 2 (P2-D1 … P2-D5) designed this work and is the input. This document supersedes it
where the two disagree, and says so at each point.

## Scope

**In scope.** All thirteen row-preserving tenancy mutations plus `detach`: organization, team and
project `create`/`rename`/`archive`/`restore`, and membership `attach`/`detach`. The transactional
refactor that makes an atomic outbox write possible. Fourteen new `EventType` variants. Two
corrections to existing tests this change makes load-bearing.

**Out of scope.** Widening `CONSUMER_FILTER_SUBJECTS` to the thirteen *new* tenancy subjects, so
the gateway decision cache reacts to (say) an org archive — P2-D4 already fenced this off. Note
this is **not** the same as saying no consumer is affected; see D4 and Limitations, because org
create emits an already-filtered subject. `ServiceAccount` and `ApiKey` stamping. Any change to
authorization, which stays in the transport adapters.

**No contract change, no gate change.** `EventType` has no proto twin; `DeadLetterEntry.event_type`
and `AuditEntry.action` are plain `string`s; the NATS payload is CloudEvents JSON, never protobuf.
`ops/nats/subjects.env` grants the wildcard `iam.>`, so no publisher permission moves.
`ci/error-registry/check.py`'s `MANIFEST` lists no tenancy service file and no new error code is
introduced. `repo:observability-drift` keys on metric families, not event types.

**Open for the reviewer.** CLAUDE.md requires a Notion ADR for significant choices, and
`cloud_event.rs:10-11` calls `EventType`'s wire strings "the public wire contract" that "external
consumers depend on". Growing the publishable surface from 8 subjects to 22 plausibly needs an ADR
or an amendment, plus a note in `ops/nats/permissions.md`. Flagged, not decided here.

## Design

### D1 — `_in` twins, and what they return

All four tenancy repositories open and commit their own transaction internally and never expose it,
so an outbox row cannot be made atomic with the mutation it describes. The house pattern exists
four times over — `PrincipalRepository::create_user_in`, `ServiceAccountRepository::create_in`,
`ApiKeyRepository::issue_in`/`revoke_in` — and the tenancy ports have no twins.

Add `create_in` / `rename_in` / `set_status_in` to the three node ports and `attach_in` /
`detach_in` to `MembershipRepository`, each taking `tx: &dyn Transaction` first after `&self`.
Note `&dyn`, not `&mut dyn`: `Transaction` is `Send + Sync` precisely so `#[async_trait]`'s `Send`
bound holds, and every existing `_in` call site uses `&*tx` over a `Box<dyn Transaction>`.

`create_in` returns `()`. The team and project services keep their post-`create` refetch
(`teams.rs:41`, `projects.rs:51`), which runs after `commit()` — effectively where it runs today,
since the repository committed before returning. A `create` is never a no-op.

**`rename_in` and `set_status_in` return `Mutated<NodeView<T>>`**, a new type beside `NodeView` in
`paigasus-iam-core`'s `ports.rs`:

```rust
/// A mutation's result plus whether it actually changed anything. `changed == false` is a
/// no-op — SMA-440 D5's "a write that changes nothing stamps nothing", extended to
/// "and emits nothing".
#[must_use]
pub struct Mutated<T> {
    pub value: T,
    pub changed: bool,
}
```

A named struct rather than `(NodeView<T>, bool)`: `.changed` states intent at the ten
implementation sites, where `.1` would not. `#[must_use]` because dropping the result silently
discards the no-op signal. The fields stay public — ten implementations construct it by hand, and
an accessor would not stop a wrong `changed` any more than a public field does. The control is
Testing case 2's pair, not the type.

`attach_in` returns `MembershipRecord`, as `attach` does. `detach_in` returns
`Vec<MembershipRecord>` — D6.

**The existing ten methods stay**, re-expressed as wrappers over their twins — the shape
`ApiKeyRepository::revoke` documents, and `PgApiKeyRepository::issue`/`revoke`
(`pg_api_keys.rs:253-262,:281-291`) implement. Roughly forty test fixtures across `tenancy_orgs.rs`,
`tenancy_nodes.rs`, `tenancy_memberships.rs` and the `authz_*` suites call these directly, and
`tests/authz_entity_gen_bumps.rs` asserts strict bump counts through them.

**Each wrapper must be literally `begin` → delegate → `commit` → bump, with no duplicated logic.**
Otherwise the wrapper and its twin are two bodies that must write the same rows, exercised by
disjoint tests — fixtures hit only wrappers, services only twins — so a divergence is invisible on
both paths. There is one body; the wrapper only owns the transaction.

**Ten implementations change:** four Postgres adapters, four fakes in `application/fakes.rs`, and
the two extra `InMemoryMemberships` in `authenticate_api_key.rs` and `authenticate_token.rs`. That
is 26 new method bodies. The fakes' wrappers pass the existing `CountingTransaction`
(`fakes.rs:943`).

Adding a trait method without a default body is a hard compile error until all ten land, so D1 is
one atomic commit and cannot be split.

### D2 — the service shape [R2]

The reference is `ApiKeyService::issue` (`application/api_keys.rs:204-283`). Per mutation:

1. Mint **one** `correlation_id` and one `occurred_at`, shared by every event and entry the call
   produces.
2. `let tx = self.uow.begin().await?;`
3. The `_in` call.
4. Build the events and entries.
5. `outbox.enqueue(&*tx, &event)`, then `audit.record(&*tx, &entry)`, per event.
6. `tx.commit().await?;`
7. The post-commit generation bump (D7).

**[R2] Steps 3 and 4 are in this order deliberately, and revision 1 had them reversed.**
`ApiKeyService::issue` builds its event before `begin()`, and copying that literally is impossible
here. `TeamService::{rename,archive,restore}` (`teams.rs:57,:69,:76`) and the project equivalents
(`projects.rs:67,:78,:84`) receive `id: Uuid` and nothing else, but `TeamId` and `ProjectId` expose
only `from_prn` and `from_parts(org, id)` — **there is no `from_uuid`**
(`libs/paigasus-iam-core/src/tenancy.rs:94-140`; only `OrganizationId` has one, at `:78`). The
service cannot build a team or project PRN from a bare uuid. The PRN exists only in the `NodeView`
the `_in` call returns. Independently, D9 requires *post-change* `slug`/`name` on a `renamed`
event, and a rename supplying one field leaves the other unknown until the repository answers.

So: the three `create` methods build their events before `begin()`, because the service constructs
the entities and therefore holds every PRN. **Every other mutation builds after the `_in` call**,
from `Mutated.value`, the returned `MembershipRecord`, or the returned `Vec`. Only the
`correlation_id` and `occurred_at` are minted up front. Revision 1's "build all values before any
I/O" is withdrawn.

**Security corollary, and it is the reason this is not merely stylistic.** `attach`'s event must
take its `node_prn` from the returned `MembershipRecord` — the *stored* PRN — never from the
caller's `node_prn` string. `PgMembershipRepository::attach` byte-matches the supplied PRN against
the stored one and answers `PrnMismatch` (`pg_memberships.rs:168,:178,:192`), which is the
forged-org-slot defence `tests/authz_forged_org_slot_escalation.rs` covers. Echoing the caller's
input into the event stream would route a forged PRN straight past it.

**One deliberate departure from the reference: authorization is not copied.** `ApiKeyService` owns
an `authorize` field. The tenancy services own none — authorization runs in the transport adapters,
gated by `AppState.enforce_tenancy`, fetch-first and against the stored PRN
(`adapters/http/teams.rs:52-57` is the canonical shape). That stays where it is. P2-D3's recipe
opens with "authorize"; here the caller has already done it.

The four services move from positional `new(repo, ids, clock)` to `*ServiceDeps` params structs —
the house pattern (`ApiKeyServiceDeps`, `RoleServiceDeps`, `CreateUserDeps`) — gaining
`uow: Arc<dyn UnitOfWork>`, `outbox: Arc<dyn Outbox>`, `audit: Arc<dyn AuditLog>` and, for the three
node services, `gen_bumper: Arc<dyn EntityGenBumper>`. `OrganizationService` additionally takes
`policy_gen_bumper: Arc<dyn PolicyGenBumper>` for the owner grant. `MembershipService` takes
neither bumper (D7).

**A composition-root reordering is required.** The four services are built at
`adapters/http/mod.rs:355-363`; the shared `audit_log` they now need is created at `:394`. They move
below it. The `OrgSvc`/`TeamSvc`/`ProjectSvc`/`MembershipSvc` aliases at `:94-97` change with the
generic parameters.

**A no-op emits nothing.** Where `Mutated.changed` is false, steps 4–5 are skipped —
`ApiKeyService::revoke`'s `did_revoke` branch. The commit still happens, and so does the bump (D7).

**Consequence: guard locks are now held across the outbox and audit writes.** `PgTeamRepository::
create` holds `FOR SHARE` on the org row (`pg_teams.rs:115`); `rename`/`set_status` hold
`FOR UPDATE` on the node (`pg_teams.rs:169,:214`, `pg_organizations.rs:211,:248`); `attach` holds
`FOR SHARE` on the node, its ancestors and the org-membership row (`pg_memberships.rs:165-218`).
Those locks now also span N outbox inserts and N audit inserts. For twelve of the fourteen
mutations N is 1 and the added hold is two inserts. For org create N is 3. For a cascading org
detach **N is unbounded** — nothing caps a principal's team and project memberships in one org —
so that one transaction holds `FOR UPDATE` on the target membership across 2N inserts. This is
accepted rather than mitigated: the rows are the audit trail the issue exists to produce, and
batching them would break the atomicity that is the whole point. It is recorded in Limitations so
a future contention report starts here rather than re-deriving it.

### D3 — fourteen new `EventType` variants

`iam.organization.{created,renamed,archived,restored}`, the `iam.team.*` and `iam.project.*`
quartets, and `iam.membership.{attached,detached}`. These mirror 1:1 the fourteen tenancy `Action`
variants `authz/action.rs:13` already declares, so the vocabulary is not invented here.

`set_status` produces `archived` or `restored` by target `NodeStatus`, not one `status_changed`
variant: a consumer filtering for archival should not have to parse a payload.

`EventType::ALL` goes `[EventType; 8]` → `[EventType; 22]`, and `as_wire`/`parse` gain their arms.

### D4 — `OrganizationService::create` emits three events [R2]

`OrganizationRepository::create` writes three rows in one transaction: the organization, the
auto-provisioned default team (ADR-0014), and the creating principal's `org_admin` owner grant. It
emits three events on one `correlation_id`: `iam.organization.created`, `iam.team.created`, and the
**existing** `iam.role.granted`.

This applies P2-D5's principle — one event per row, so per-node provenance stays queryable —
symmetrically to create. One org event describing the other two rows in its payload would leave the
default team with no creation event while an explicitly created team has one, and would hide the
owner grant from the query that finds every other role grant.

**[R2] Revision 1 called this the "second emitter" of `RoleGranted` and said to match
`RoleService::grant`'s payload. Both were wrong.** There are already two —
`RoleService::grant` (`roles.rs:224`) and `BootstrapAdminSeeder::seed_grant`
(`bootstrap_admin.rs:150`) — so org create is the **third**. And the relevant precedent is the
non-`RoleService` one, which deliberately *diverges* to stay distinguishable: it sets
`"source": "bootstrap_admins"` in the payload (`bootstrap_admin.rs:158-163`).

Matching `RoleService`'s shape exactly would make an org-create grant indistinguishable from a
user-requested one — even though it never ran `RoleService::grant`'s anti-escalation check
(`roles.rs:207`). So org create follows the established convention instead:

- payload `{"grant_id", "role_key", "scope", "source": "organization_create"}`
- `aggregate_prn: grant.principal.canonical()` — the **principal**, matching both existing
  emitters, and explicitly *not* the node (D9's general rule does not apply to this one event)
- `actor_prn: Some(actor.canonical())` — unlike bootstrap's `None`, a real principal did this

`iam.team.created` for the default team carries `"source": "organization_create"` for the same
reason, so a consumer can tell it from an explicit `TeamService::create`.

### D5 — audit entries [R2]

One `AuditEntry` per `DomainEvent`, sharing its `correlation_id` and `occurred_at`.

- **`action` — `Action::…​.as_wire().to_string()`, not a hand-written literal. [R2]** Revision 1's
  measurement C5 claimed no such method exists; it does, at
  `libs/paigasus-iam-core/src/authz/action.rs:114-158`, returning exactly `"RenameTeam"`,
  `"AttachMembership"`, `"CreateOrganization"`. `cedar_uid` (`:225`) calls it, and
  `pg_api_keys.rs:103` already uses it for this class of purpose. `AuditEntry.action` is a free
  `String` and `AuditFilter.action` is how operators query, so a typo in a literal makes rows
  permanently unfindable with nothing to red it. `as_wire`'s match is wildcard-free, so it also
  makes a future `Action` rename a compile error. The existing literals in `roles.rs:236` and
  `policies.rs:137` are not followed here; they are the older habit.
- `actor_prn` — `Some(stamp.by.canonical())`, i.e. SMA-440's `Stamp.by`. For `detach`, the actor
  D6 adds.
- `resource_prn` — the node's PRN; for a membership, the node it attaches to. For org create's
  role entry, `root_prn()` is **not** used (that is bootstrap's case, where no tenancy scope
  existed); the new org's PRN is.
- `outcome` — `AuditOutcome::Committed`. These are written only on a committing path.
- `determining_policies` — **empty**. Authorization ran in the transport adapter, outside this
  service, and denials are recorded independently by `adapters/authz/denial_audit.rs`. Populating
  it would mean threading the Cedar decision through the service for no consumer.
- `detail` — the event's payload shape, **plus a provenance key on every derived entry**.

**[R2] The provenance key is not cosmetic.** D5's one-entry-per-event rule makes org create write
an entry with `action: "GrantRole"`, `outcome: Committed`, `determining_policies: []` for an actor
authorized for `CreateOrganization` and never checked for `GrantRole`. Each cascaded detach entry
likewise reads `"DetachMembership"` though authorization ran once, at the org node
(`adapters/http/memberships.rs:91-95`). Left unmarked those are false statements about what was
authorized — the same class SMA-440's D5 refused for `modified_by`. So:

- org create's team and role entries carry `"source": "organization_create"` in `detail`
- each cascaded detach entry carries `"cascade_of": <the org membership's id>` in `detail`

A directly authorized action carries neither, so their absence is the signal.

### D6 — detach [R2]

**`MembershipService::detach` gains an actor.** It is `detach(&self, id: Uuid)`
(`memberships.rs:85-87`) — SMA-440 left it unstamped because it deletes the row and there is nothing
to stamp. An `AuditEntry.actor_prn` needs one anyway, so it gains `actor: &PrincipalId`.
Unconditionally: SMA-440's D3 established that every tenancy mutation is authenticated regardless
of `enforce_tenancy`, which gates authorization only. Call sites: `adapters/grpc/tenancy.rs:653`
and `adapters/http/memberships.rs:96`, plus its own unit tests at `memberships.rs:234,:238,:243`.
Note `grpc/tenancy.rs:638` binds `actor` as a `Prn`, so that site needs a second binding for the
`&PrincipalId`.

**`detach_in` returns what it deleted.** An org detach cascades to the principal's team and project
memberships in that org (`DETACH_CASCADE_SQL`, `pg_memberships.rs:138-141`). Each deleted row gets
its own entry and event on one `correlation_id`, so "when did this principal lose access to project
X" is answerable by filtering on that project's PRN — the fact the trail exists to expose, and one
a single org-detach event would hide. The service cannot build these itself: it holds a `Uuid`, and
`membership` stores no PRN columns, only FK uuids. The PRNs exist only inside the repository.

**Mechanism: lock, project, delete — three statements, in that order, in the transaction.**

1. **Lock.** `SELECT id FROM "membership" WHERE id = $3 OR (principal_id = $1 AND (team_id IN
   (SELECT id FROM "team" WHERE org_id = $2) OR project_id IN (SELECT id FROM "project" WHERE
   org_id = $2))) FOR UPDATE`. Single table, no `UNION`, so `FOR UPDATE` is legal.
2. **Project.** The PRN-joining `SELECT` that populates `MembershipRow`, over the same predicate,
   built from `LIST_BY_PRINCIPAL_SQL`'s projection (`pg_memberships.rs:96-109`) so it uses the
   mapping every read path uses.
3. **Delete.** `DETACH_CASCADE_SQL` then the target row, unchanged from today.

**[R2] Step 1 is new, and revision 1's "the target row is already locked" argument was
insufficient.** `detach` takes `lock_exclusive()` on the **target row only**
(`pg_memberships.rs:266`). A concurrent transaction detaching one of the *cascade* rows — its own
target, a different row — is not blocked by that. Under READ COMMITTED the unlocked projection
would see the row, the later `DELETE` would re-evaluate after the peer commits and remove nothing,
and the trail would over-report a detach this call never performed. Step 1 closes it. Testing case
9 runs single-threaded and would not have caught this.

**[R2] The `DELETE … RETURNING` alternative was rejected on a false premise.** Revision 1 claimed a
`DELETE` cannot join to the tables the PRNs come from. A CTE's *outer* `SELECT` joins freely, and
that form reuses one projection rather than adding a second. It is a legitimate option. It is still
not taken, for a narrower reason: `RETURNING` returns rows as the `DELETE` sees them, so it would
have to be reconciled against the same concurrency question step 1 answers, and the three-statement
form keeps the locking explicit and the projection shared with the read paths. Recorded so the
rejection is honest.

**[R2] The cascade `WHERE` clause is not reusable "verbatim", and the bindings need stating.**
`$2` is `model.org_id`, an `Option<Uuid>`: for a team or project detach it is `NULL`, `org_id =
NULL` is never true, and only the `id = $3` term matches — correct, but by arithmetic rather than
by design, so it is commented at both sites. In step 2's three-arm union the `OR` disjunct is
neutralised per arm only by the inner joins (a project row has `team_id IS NULL`, so the team arm's
`JOIN "team" t ON t.id = m.team_id` drops it) — also correct, and it silently breaks if any arm
ever becomes a `LEFT JOIN`. Both accidents get a comment naming the coupling.

**[R2] The fake diverges structurally, and it is a third statement in the drift set.**
`InMemoryMemberships::detach` (`fakes.rs:464-475`) cascades on `parent_org_uuid(&m.node)` — the org
slot embedded in the caller's PRN — while Postgres resolves the stored `org_id` by subquery. Risk 3
counts three statements, not two.

Note today's `detach` discards even the cascade's row count: the `txn.execute` result is dropped at
`pg_memberships.rs:274`.

### D7 — the generation bump moves, and needs a port that does not exist

**Correction to P2-D2.** It says the bumps "must move" to the service. There is no way for a
service to call them: `bump_entity_gen` is a *private inherent* method on each Pg repository
(`pg_organizations.rs:46`, `pg_teams.rs:37`, `pg_projects.rs:37`) delegating to a `Generations`
field, and ADR-0005 keeps `Generations` out of the application layer. Only `PolicyGenBumper` exists
as a port (`ports.rs:367-370`), with adapter `GenerationsPolicyGenBumper`.

So this adds `EntityGenBumper` to `ports.rs` and `GenerationsEntityGenBumper` to
`adapters/authz/generation.rs`, mirroring the policy pair line-for-line including its
swallow-and-log posture: a failed cache-invalidation bump must never fail an already-committed
write. `GenerationsEntityGenBumper` must be `pub` from the lib root or `dead_code` reds it in the
window before it is wired.

Why it must move: this is a Cedar cache-invalidation path. Left inside a repository that no longer
commits, it would invalidate against a transaction that may still roll back.
`bootstrap_admin.rs:194-197` records the identical hazard for `grant_in`.

**The bump still fires on a no-op**, as today (`pg_organizations.rs:225`, and the `set_status`
idempotency arms). Making it conditional would change cache-invalidation behaviour, a separate
concern from audit correctness — SMA-440's D5 says so and this preserves it. The visible
consequence is that a no-op rename invalidates the Cedar cache while emitting no event: current
behaviour plus nothing.

The repositories keep their private bumps, which now serve only the wrapper path. No path bumps
twice: a service calls `_in` and bumps through the port; a fixture calls the wrapper and gets the
repository's bump.

**`MembershipService` gets no bumper.** `PgMembershipRepository` has no `gens` field and bumps
nothing — correctly, not by oversight: `pg_entity_slice.rs` never reads memberships, so a
membership change invalidates nothing. Stated so this work does not grow a bump that never existed.

### D8 — two tripwire corrections

**Correction to P2-D4.** It names three tripwires that "fail the build until they agree". Only two
do. `EventType::ALL`'s fixed-size array and `all_lists_every_event_type`'s wildcard-free match are
genuine. But `type_matches_the_wire_string_for_every_variant` (`cloud_event.rs:158-174`)
**hard-codes an eight-element array literal** rather than iterating `EventType::ALL`, so a new
variant compiles and goes uncovered. It changes to iterate `ALL`.

**A fourth hand-listed site P2-D4 does not mention.** `no_payload_shape_carries_a_secret_or_pii_key`
(`cloud_event.rs:178-195`) hand-lists four payload shapes and asserts none renders a banned key.
The new tenancy shapes are uncovered until added, so they are added explicitly rather than derived.
Note it is a substring scan over hard-coded *sample* values and cannot see runtime content.

`tests/nats_permissions.rs:324` genuinely iterates `EventType::ALL`, so the NATS grant assertion
covers the new variants automatically. P2-D4 is right about that one.

**Do D8 before adding the variants.** Otherwise fourteen wire strings land unasserted. The ordering
is feasible: D8 is test-only, and a `pub` enum variant referenced by `ALL` is never dead code.

### D9 — payload shapes

`schema_version: 1` and `actor_prn: Some(actor.canonical())` on every new event.

| Event | Payload / detail |
|---|---|
| `iam.{organization,team,project}.created` | `{"node_prn", "slug", "name", "status", "effective_status"}` |
| `iam.{organization,team,project}.renamed` | `{"node_prn", "slug", "name"}` — post-change |
| `iam.{organization,team,project}.{archived,restored}` | `{"node_prn", "status", "effective_status"}` |
| `iam.membership.attached` | `{"membership_id", "principal_prn", "node_prn"}` |
| `iam.membership.detached` | `{"membership_id", "principal_prn", "node_prn"}` |
| `iam.role.granted` (org create) | `{"grant_id", "role_key", "scope", "source"}` — D4 |

**[R2] Both statuses, not one.** `NodeView<T>` carries own and effective status
(`ports.rs:57-61`), and `TeamService::restore`'s own doc (`teams.rs:74-76`) warns a restored team
"may still be effectively archived if its org remains archived". A single `"status"` field would
let `iam.team.restored` describe a node still archived to every authorization decision.

`aggregate_prn` is the node's PRN for node events, and for membership events the node the membership
attaches to — so a consumer filtering one node's history sees its attachments. The one exception is
org create's `iam.role.granted`, which uses the principal's PRN to match both existing emitters
(D4).

### D10 — deploy ordering [R2]

`OutboxRelay::row_to_domain_event` (`adapters/events/relay.rs:96`) returns `Err` for an
unrecognized `event_type` wire string, and its own doc records that the relay "treats this exactly
like a failed publish (counts against `attempts`, eventually parks)".

So during a rolling deploy or a rollback, a replica running the **old** binary that drains a **new**
replica's outbox rows will burn `max_attempts` on every tenancy event and dead-letter it. Nothing
in the design otherwise prevents this, and it is a routine canary or revert, not an exotic failure.

**Constraint: every replica must carry the new `EventType` set before any replica writes one.** In
practice that means the deploy is ordinary — a single-version rollout — but a *rollback* after
tenancy events have been written will park them. Parked rows are recoverable through the existing
dead-letter replay path (SMA-469), which is why this is a documented constraint rather than a
feature flag. Stated so an operator rolling back knows to expect dead letters and where to replay
them from.

## Data flow

For a team rename:

1. The HTTP handler authorizes (`enforce_tenancy` path) and calls
   `teams.rename(id, slug, name, &ctx.principal_id)`.
2. `TeamService::rename` builds `Stamp { at: clock.now(), by: actor.clone() }` and mints one
   `correlation_id`.
3. `let tx = self.uow.begin()`.
4. `repo.rename_in(&*tx, id, slug, name, &stamp)` → `Mutated { value, changed }`.
5. If `changed`: build the event and entry **from `value`** (D2), then `outbox.enqueue`,
   `audit.record`.
6. `tx.commit()`.
7. `self.gen_bumper.bump().await` — post-commit, awaited, unconditional.
8. Return `value`.

Org create differs: it builds all three events before step 3, because it constructs the entities and
holds every PRN; step 5 enqueues and records three; step 7 bumps entity **and** policy generations.
`ProjectService::create`'s existing pre-read of its parent team (`projects.rs:39`) stays **outside**
`begin()` — the repository re-guards under `FOR SHARE` regardless, so moving it in would buy nothing
and lengthen the transaction.

Org detach differs: step 4 returns N records and step 5 builds N events and N entries from them.

## Error handling

No new error variants. An `enqueue` or `record` failure is a `RepositoryError` that propagates and
aborts the transaction, so the mutation rolls back with its event — the point of the outbox. A
failed post-commit bump is swallowed and logged (D7).

`map_err`'s constraint-name-to-`ConflictKind` mapping is unaffected: it maps the error of whichever
statement failed, and the guard statements' relative order inside the transaction does not change.

## Testing

**Unit, against the fakes, with `FixedClock`, `FakeUnitOfWork`, `FakeOutbox`, `FakeAuditLog`:**

1. Each of the twelve single-event mutations — the thirteen row-preserving ones minus
   `OrganizationService::create`, which case 4 covers — emits exactly one event and one entry
   sharing one `correlation_id`, with the expected wire string and action.
2. A `rename` supplying values identical to the stored ones emits **nothing**; and the negative
   half, a matching slug with a differing name, emits normally. Either alone passes a wrong
   implementation.
3. An idempotent `set_status` no-op emits nothing.
4. `OrganizationService::create` emits three events on one `correlation_id`; the `team.created` and
   `role.granted` ones carry `"source": "organization_create"`; the role event's `aggregate_prn` is
   the principal's PRN.
5. An org `detach` that cascades emits one event and one entry per deleted row on one
   `correlation_id`, each cascaded entry carrying `"cascade_of"`, and the project row's event
   carrying that project's PRN. **The fake does implement the cascade** (`fakes.rs:469-473`), so
   fan-out and correlation are provable here.
6. A mutation failing mid-transaction leaves no event and no entry. `FailingRevokeApiKeys`
   (`api_keys.rs:465-493`) is the shape.
7. The post-commit bump fires after a successful commit and not before, and still fires on a no-op.
   `FakeUnitOfWork::commits()` plus a counting bumper.
8. Every emitted `action` equals `Action::…​.as_wire()` for the corresponding variant — the control
   for D5's fourteen call sites.
9. `attach`'s event carries the **stored** `node_prn` from the returned record, not the caller's
   input — the D2 security corollary. Feed a `MembershipRecord` whose PRN differs from the input.

**Postgres integration**, Docker-gated through `tests/support/docker.rs`:

10. Commit makes the tenancy row, its outbox row and its audit row visible together; a rollback
    leaves none of the three. `tests/outbox_uow_pg.rs` is the template.
11. An org detach's cascade writes one audit row per deleted membership with the right PRNs, and
    the audit row count **equals** the deleted row count. This is what the fake structurally cannot
    prove: not the cascade, which it implements, but that step 2's projection and step 3's `DELETE`
    agree on the row set.
12. A concurrent detach of a cascade row does not make this call over-report — the control for D6
    step 1's lock. Two transactions, the peer committing between the lock and the delete.

**End to end:** extend `tests/mutation_audit_e2e.rs`, which already drives HTTP → row → relay, with
a tenancy mutation.

Naming follows the crate's style: `snake_case`, full-sentence, behaviour-asserting, with a doc
comment citing the decision id. Run filtered suites with `PAIGASUS_REQUIRE_DOCKER=1` — the Docker
canary is not in the filter, so they would otherwise skip and report a green that tested nothing.

## Blast radius

Ten repository implementations, four services and their constructors and type aliases, one
composition root, two transport adapters plus `MembershipService::detach`'s own unit tests (for the
new actor parameter), one new port, one new adapter, one new value type, and the four `EventType`
sites.

The forty-odd existing *repository* fixture call sites are untouched — that is what D1's retained
wrappers buy — but `detach`'s new parameter does break its callers, which the count above includes.

No proto change, no codegen, no migration, no NATS publisher permission change, no `repo:*` gate
registration.

## Limitations, accepted

**[R2] Org create does reach an in-tree consumer.** `ops/nats/subjects.env:64-71` lists
`iam.role.granted` in `CONSUMER_FILTER_SUBJECTS`, which `provision.sh:139-142` turns into the
`gateway-cache-invalidator` durable's filter. So D4's third event adds a message to that durable on
every organization create. Revision 1's claim that no in-tree consumer filters on any of this was
false for the reused variant. The extra invalidation is **wanted** — a new `org_admin` grant genuinely
changes authorization decisions, which is exactly why that subject is filtered — but it is a
throughput change SMA-492 did not size for, and the `"source"` field is what would let a future
consumer exclude it.

**The thirteen new tenancy subjects reach no consumer.** They are written, relayed and published,
but nothing filters on them; the audit trail is the consumer. Widening
`CONSUMER_FILTER_SUBJECTS` stays out of scope.

**A cascading org detach holds its target row's `FOR UPDATE` across an unbounded number of
inserts** (D2). Accepted: batching would break the atomicity that is the point.

**`IAM_OUTBOX_NOTIFYING_ENQUEUES_TOTAL` over-counts on multi-event transactions.** `PgOutbox::
enqueue` increments it per event and runs one `pg_notify` per event (`pg_outbox.rs:91`). The
payload is empty, so Postgres collapses them into a single delivered notification per transaction —
queue pressure is therefore not the concern — but the counter still moves N times for one nudge.
`IamOutboxNotificationsAbsent` (`ops/observability/prometheus/rules/iam.rules.yml:43-56`) gates on
that term against `iam_outbox_relay_drained_total`, so org create and cascading detach skew the
ratio. The alert fires on notifications being *absent*, and over-counting moves it away from
firing, so this is a sensitivity loss rather than a false alarm.

**A no-op rename still invalidates the Cedar cache** (D7).

**`determining_policies` is empty on every tenancy entry** (D5). An operator reconstructing *why* a
mutation was permitted must join to the denial-audit trail or the policy snapshot.

**`name` is operator-controlled free text and now crosses the outbox.** `slug` is constrained by
`Slug::parse`; `name` is not. It lands in the CloudEvents `data` broadcast on `iam.>` and in
`audit_log.detail`. The exposure is bounded — the audit read path is Root-only
(`application/audit.rs:33-39`) and only `iam-provisioner` may subscribe
(`ops/nats/subjects.env:23-30,:51-55`) — but this is a stated bound, not a claim that the field
cannot contain PII, which nothing enforces.

**Steady-state row rate rises.** `audit_log` is LIST×RANGE partitioned
(`migration/m0008_partition_audit_log.rs`) and `event_outbox` has a configurable sweep
(`pg_outbox_maintainer.rs:44-57`); neither breaks, and the existing sweep defaults still bound
growth. One audit row per tenancy mutation is nonetheless a material change to the rate.

## Risks

**The largest risk is an event emitted for a mutation that rolled back, or suppressed for one that
committed.** Both reduce to getting `changed` and the commit boundary right across ten
implementations. The control is Testing case 6 paired with case 2's negative half — either alone
passes a wrong implementation.

**Second: the fakes and the adapters can disagree**, and the unit tier measures only the fakes.
SMA-440 hit this exact shape. Cases 2, 3 and 5 therefore also run against Postgres.

**Third: three statements must describe the same row set** — D6's projection `SELECT`, its `DELETE`,
and the fake's `retain`, which cascades on a different key entirely (the caller's PRN, not the
stored `org_id`). If any two diverge the trail silently under- or over-reports. Case 11 is the
row-count control and case 12 the concurrency one; the fake's divergence is covered only by case 5
agreeing with case 11.

**Fourth: the events are built from repository output, not from service inputs** (D2). That is what
makes `attach` safe against a forged PRN, and it is also the thing an implementer copying
`ApiKeyService::issue` literally will get backwards. Case 9 is the control.

**Fifth: `type_matches_the_wire_string_for_every_variant` is not currently a tripwire.** Until D8
lands, fourteen wire strings are unasserted. D8 is therefore first.
