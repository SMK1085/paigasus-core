# SMA-606 — pre-spec measurements

Gathered 2026-09-01 against `c3b9ee9` (SMA-440 Part 1, PR #195), on branch
`feature/sma-606-tenancy-audit-log-entries-and-domain-events-on-every`.

**Status: research only.** The pipeline halted before Stage 1 because the `superpowers`
plugin is not installed. This file exists so the re-run does not re-derive any of it. It is
**not** a spec and states no plan — it records what was measured and which of Part 2's
written claims those measurements contradict.

Part 2's design decisions (P2-D1 … P2-D5) live in
`2026-08-30-sma-440-server-side-audit-stamping-design.md`, section "Part 2". Everything below
either confirms, corrects, or extends them.

---

## C1 — "Three compile-time tripwires" is wrong: the third is not a tripwire

P2-D4 says three things "**fail the build** until they agree". Measured: only two do.

| Claimed tripwire | File | Actually a tripwire? |
|---|---|---|
| `EventType::ALL` fixed-size array | `paigasus-iam-core/src/domain_event.rs:32` | **Yes** — `[EventType; 8]`, length must change |
| `all_lists_every_event_type` | `domain_event.rs:110-129` | **Yes** — wildcard-free match, plus `assert_eq!(EventType::ALL.len(), 8)` at `:128` |
| `type_matches_the_wire_string_for_every_variant` | `adapters/events/cloud_event.rs:158-176` | **No** |

The third **hard-codes an 8-element array literal** rather than iterating `EventType::ALL`:

```rust
for et in [
    EventType::PrincipalCreated,
    …
    EventType::PolicyDeleted,
] {
```

A new variant compiles cleanly and is silently uncovered. The fix is to make it iterate
`EventType::ALL`, which turns it into the tripwire the design already believed it was.

**A fourth hand-listed site the design does not mention at all:**
`no_payload_shape_carries_a_secret_or_pii_key` (`cloud_event.rs:181-195`) hand-lists four
payload shapes and asserts none renders a banned key (`hash`, `secret`, `plaintext`, `email`,
`pepper`, `token`, `password`). New tenancy payload shapes are **not** covered until they are
added to that array. This is a secret-safety control, so it matters more than its omission
from the design suggests.

Confirmed unaffected: `tests/nats_permissions.rs:324-338`
(`the_publisher_grant_covers_ensure_and_every_event_subject`) genuinely iterates
`EventType::ALL` at `:330`, so a new variant is covered automatically — the design is right
about this one.

## C2 — moving `bump_entity_gen` to the service needs a port that does not exist

P2-D2 says `bump_entity_gen`/`bump_policy_gen` "must move" to the service. Measured: the
service layer has no way to reach them.

- `PolicyGenBumper` **exists** as a port (`ports.rs:367-370`), with adapter
  `GenerationsPolicyGenBumper` (`adapters/authz/generation.rs:450-471`), already wired for
  `RoleService` (`role_gen_bumper`, `http/mod.rs:478`) and `PolicyService`.
- **There is no `EntityGenBumper` port.** `bump_entity_gen` is a *private inherent async fn*
  on each Pg repo — `pg_organizations.rs:46`, `pg_teams.rs:37`, `pg_projects.rs:37` —
  delegating to a `Generations` field. ADR-0005 keeps `Generations` out of the application
  layer, so a service cannot call it directly.

So P2-D2 implies new infrastructure: an `EntityGenBumper` port plus a
`GenerationsEntityGenBumper` adapter, mirroring the policy pair exactly.

`bump_policy_gen` exists on `PgOrganizationRepository` **only** (`:56`) — teams and projects
have no policy bump, because only org `create` writes a grant.

## C3 — `PgMembershipRepository` bumps nothing, and that is correct

It has no `gens` field at all:

```rust
pub struct PgMembershipRepository { db: DatabaseConnection }   // pg_memberships.rs:22-25
```

Neither `attach` nor `detach` bumps any generation. **This is consistent, not a bug:**
`pg_entity_slice.rs` (199 lines) contains no reference to `membership` — memberships never
feed the Cedar entity slice, so there is nothing to invalidate.

Stated here so P2-D2's "move the bump" work does not grow a membership bump that never
existed and is not wanted.

## C4 — four things Part 2 leaves unsettled

**a. No-op detection has no return channel.** P2-D3 says a no-op must emit nothing, citing
`ApiKeyRepository::revoke_in`'s `bool`. But the tenancy `rename`/`set_status` return
`NodeView<T>`, not a bool, so the service cannot currently tell a no-op from a real change.
The no-op branches themselves exist and were added by SMA-440 D5 — adapters at
`pg_organizations.rs:218-227`, `pg_teams.rs:184-193`, `pg_projects.rs:217-226`; fakes at
`fakes.rs:106-116`, `:206-216`, `:314-325`. Both the adapter and the fake no-op paths
**commit and bump** today; that must be preserved.

**b. `detach` discards what it deleted.** P2-D5 requires `detach_in` to return the
`MembershipRecord`s removed. Today `detach` (`pg_memberships.rs:263-281`) runs
`DETACH_CASCADE_SQL` (`:134-141`) via `txn.execute(stmt)` and **drops the `ExecResult`**, so
not even the row count survives, let alone the PRNs. The `membership` table stores no PRN
columns (only FK uuids), so the records must be recovered in-transaction — SELECT-then-delete
or `DELETE … RETURNING` — before the rows vanish.

**c. `MembershipService::detach` has no actor parameter.** It is
`detach(&self, id: Uuid) -> Result<(), TenancyError>` (`memberships.rs:85-87`) — SMA-440
deliberately left it unstamped. An `AuditEntry.actor_prn` needs one, so the signature and
its call sites (`adapters/grpc/tenancy.rs`, `adapters/http/memberships.rs`) change.

**d. The four tenancy services take positional constructors and are wired too early.**
All four are `new(repo, ids, clock)` — `OrganizationService` (`organizations.rs:35`),
`TeamService` (`teams.rs:27`), `ProjectService` (`projects.rs:29`, which takes *two* repos),
`MembershipService` (`memberships.rs:55`). Adding `uow`/`outbox`/`audit`/`gen_bumper` means
introducing `*ServiceDeps` params structs, which is the house pattern
(`ApiKeyServiceDeps`, `RoleServiceDeps`, `CreateUserDeps`).

Worse, ordering blocks it: the four are constructed at **`adapters/http/mod.rs:355-363`**,
while the shared `audit_log` is created at **`:394`**. They must move below it, or
`audit_log` must move up.

Also note the tenancy services, unlike `ApiKeyService`, hold **no `authorize` field** —
authorization happens in the transport adapters, gated by `AppState.enforce_tenancy`. So
P2-D3's "authorize; mint correlation_id; …" recipe does not transfer verbatim; the authorize
step stays where it is.

## C5 — the 14 new variants map 1:1 onto existing `Action` variants

`Action` (`paigasus-iam-core/src/authz/action.rs:13`) already declares exactly the fourteen
tenancy actions: `CreateOrganization`, `RenameOrganization`, `ArchiveOrganization`,
`RestoreOrganization`, and the `…Team` / `…Project` quartets, plus `AttachMembership` and
`DetachMembership`.

`AuditEntry.action` convention is a **string literal matching the variant name** —
`"GrantRole"` (`roles.rs:236`), `"RevokeRole"` (`:285`), `"PutPolicy"` (`policies.rs:137`),
`"IssueApiKey"` (`api_keys.rs:264`). There is no `Action::as_str`; the only rendering method
is `cedar_uid` (`action.rs:225`).

`EventType::ALL` therefore goes `[EventType; 8]` → `[EventType; 22]`.

## C6 — confirmed as written in the design

- **No contract change.** `EventType` has no proto twin; the NATS payload is CloudEvents
  JSON via `adapters/events/cloud_event.rs`.
- **No NATS config change.** `ops/nats/subjects.env` grants `PUBLISHER_PUB=("iam.>" …)`, a
  wildcard.
- **No `repo:*` gate change.** `ci/error-registry/check.py`'s `MANIFEST` lists no tenancy
  service file, and no new error code is introduced. `repo:observability-drift` keys on metric
  families (`ops/observability/**`, `paigasus-observability/**`), not event types.
- **`Transaction` is passed as `&dyn Transaction`** — a shared reference, not `&mut` — with
  the `&*tx` deref idiom at call sites. `dyn Transaction: Sync` is required for
  `#[async_trait]`'s `Send` bound.
- The reference pattern is `ApiKeyService::issue` (`api_keys.rs:204-283`) and `revoke`
  (`:306-355`), the latter being the `did_revoke`-gated emit.
- Reusable fakes already exist: `FakeUnitOfWork` (`fakes.rs:979-993`, with a `commits()`
  counter), `FakeOutbox` (`:998-1007`), `FakeAuditLog` (`:1016-1035`), `CountingTransaction`
  (`:940`).
- `tests/mutation_audit_e2e.rs:61` is the existing HTTP → row → relay end-to-end template.

## Decision taken at intake

`OrganizationService::create` emits **three** events sharing one `correlation_id`:
`iam.organization.created`, `iam.team.created` (the auto-provisioned default team) and the
existing `iam.role.granted` (the creating principal's `org_admin` owner grant). Rationale:
P2-D5's own principle — one event per row, so per-node provenance stays queryable — applied
symmetrically to the three rows `create` writes. Confirmed by Sven, 2026-09-01.

## Environment notes

- `gh` is **not authenticated** in the agent shell (`gh auth status` → not logged in). Stage 6
  needs `gh auth login` or `GH_TOKEN`.
- Docker **is** reachable, so the IAM Docker-backed suites run rather than skip. Use
  `PAIGASUS_REQUIRE_DOCKER=1` on any filtered run.
- `cargo build --workspace --locked` is green at this commit.
