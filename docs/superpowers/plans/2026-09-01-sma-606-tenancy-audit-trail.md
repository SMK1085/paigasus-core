# SMA-606 — Tenancy audit trail Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make every tenancy mutation write an `AuditEntry` and raise a `DomainEvent`, atomically with the row it describes — `detach` included.

**Architecture:** The four tenancy repositories gain `_in` twins taking `tx: &dyn Transaction`, with the existing methods kept as one-shot-UoW wrappers over them. The services gain `uow`/`outbox`/`audit` and drive mutation + event + entry on one transaction, then bump the Cedar generation post-commit through a new `EntityGenBumper` port. Fourteen new `EventType` variants. A no-op emits nothing, signalled by a new `Mutated<T>`.

**Tech Stack:** Rust (edition 2024, rust-version 1.95), SeaORM + Postgres, tonic/prost, axum, `cargo nextest`, Moon 2.5.3.

**Spec:** `docs/superpowers/specs/2026-09-01-sma-606-tenancy-audit-trail-design.md` (revision 2). Supporting measurements: `docs/superpowers/specs/2026-09-01-sma-606-measurements.md`.

## Global Constraints

- Every source file opens with `// SPDX-License-Identifier: Apache-2.0`. Every file here already exists — do not add or remove headers.
- **Working directory:** the main checkout `/Users/sven/dev/paigasus/paigasus-core`, on branch `feature/sma-606-tenancy-audit-log-entries-and-domain-events-on-every`. This is **not** a worktree.
- **PATH:** prefix every shell command with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`. This applies to `git commit` too — without it lefthook's commitlint runs under the system node and dies with `SyntaxError: Invalid regular expression flags`, which looks like a bad commit message but is not.
- **`[workspace.lints.rust] warnings = "deny"`** — dead code is a hard compile error.
- **The service crate is one compilation unit.** A trait signature change cannot be split into independently-compiling halves. Tasks 6, 7 and 8 each update the composition root in the same task for exactly this reason; do not defer that to a later task.
- **Docker:** for any *filtered* test run (`-E 'test(...)'`, `--test <name>`) prefix `PAIGASUS_REQUIRE_DOCKER=1` — the Docker canary is not in the filter, so the suites would otherwise skip and report a green that tested nothing.
- Conventional commits with a workspace scope: `feat(rs):`, `test(rs):`, `refactor(rs):`. Subject lowercase, ≤100 chars. **No line in the body may begin with `word:`** — commitlint reads it as a footer token and fails `footer-leading-blank`. Never write a bare `#NNN`; write "PR NNN".
- Do **not** use `--no-verify`.
- `paigasus-iam-core` is imported in the service crate as `paigasus_iam_core::…`; inside the core crate itself use `crate::…`.

## A note on task shape

Tasks 4 and 5 add trait methods. Adding a method without a default body is a hard compile error until every implementation lands, so each of those tasks must land all of its implementations at once — six impls for Task 4, four for Task 5. They are deliberately large and mechanical. Do not try to subdivide them; you will land in a non-compiling intermediate state with no way to run tests.

Tasks 6–8 are the reverse shape: each is small, but each changes a service constructor, so each must also update `adapters/http/mod.rs` to keep the crate compiling.

## File structure

**`rs/crates/libs/paigasus-iam-core/src/`**
- `ports.rs` — add `Mutated<T>`, `EntityGenBumper`; add ten `_in` methods across the four tenancy traits.
- `lib.rs` — re-export `Mutated` and `EntityGenBumper`.
- `domain_event.rs` — 14 new `EventType` variants; `ALL` 8 → 22; `as_wire`/`parse` arms; the `all_lists_every_event_type` match and length assertion.

**`rs/crates/services/paigasus-iam/src/`**
- `adapters/events/cloud_event.rs` — the two hand-listed tests (Task 1, then Task 3).
- `adapters/persistence/pg_{organizations,teams,projects}.rs` — split each mutating method into wrapper + `_in`.
- `adapters/persistence/pg_memberships.rs` — same, plus the new lock and projection SQL for `detach_in`.
- `adapters/authz/generation.rs` — `GenerationsEntityGenBumper`.
- `application/{organizations,teams,projects,memberships}.rs` — `*ServiceDeps`, event/entry construction.
- `application/fakes.rs` — the six node-fake `_in` splits, the membership fake's two, and a counting `EntityGenBumper`.
- `application/{authenticate_api_key,authenticate_token}.rs` — the two extra `InMemoryMemberships` impls.
- `adapters/http/mod.rs` — composition root.
- `adapters/http/memberships.rs`, `adapters/grpc/tenancy.rs` — `detach`'s new actor argument.

**`rs/crates/services/paigasus-iam/tests/`**
- `tenancy_events_pg.rs` — new; the Postgres integration tier (Task 10).
- `mutation_audit_e2e.rs` — extended with a tenancy mutation (Task 10).

---

### Task 1: Make the wire-string test a real tripwire

The spec's D8 and Risk 5: `type_matches_the_wire_string_for_every_variant` hard-codes an eight-element array, so it is not the compile-time tripwire P2-D4 claimed. Fix it **before** adding variants, or fourteen wire strings land unasserted.

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/events/cloud_event.rs:158-174`

**Interfaces:**
- Consumes: nothing.
- Produces: nothing consumed by later tasks. Task 3 relies on this test now failing when a variant is added without a wire string.

- [ ] **Step 1: Replace the hard-coded array with `EventType::ALL`**

Replace the whole `type_matches_the_wire_string_for_every_variant` body:

```rust
    /// SMA-606 D8: iterates `EventType::ALL` rather than a hand-listed array. The previous
    /// form hard-coded eight variants, so a new one compiled cleanly and went uncovered —
    /// P2-D4 called this a compile-time tripwire and it was not one. `ALL` is kept exhaustive
    /// by `all_lists_every_event_type`'s wildcard-free match, so this now transitively fails
    /// to compile for a variant with no wire string.
    #[test]
    fn type_matches_the_wire_string_for_every_variant() {
        for et in EventType::ALL {
            let mut ev = sample(None, None, "prn:x");
            ev.event_type = et;
            assert_eq!(render(&ev)["type"], et.as_wire(), "rendered `type` must equal the wire string for {et:?}");
        }
    }
```

- [ ] **Step 2: Run it and confirm it still passes on today's eight variants**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --lib -E 'test(type_matches_the_wire_string_for_every_variant)'
```

Expected: PASS, 1 test.

- [ ] **Step 3: Prove it is now a tripwire**

Temporarily add a ninth variant to `EventType` in `libs/paigasus-iam-core/src/domain_event.rs` — add `Probe,` to the enum only, touching neither `ALL` nor `as_wire`. Run:

```bash
cd rs && cargo build -p paigasus-iam-core 2>&1 | tail -20
```

Expected: FAIL — `all_lists_every_event_type`'s match is non-exhaustive and `as_wire`/`parse` are non-exhaustive. Revert the `Probe` variant.

- [ ] **Step 4: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
git add rs/crates/services/paigasus-iam/src/adapters/events/cloud_event.rs
git commit -m "test(rs): make the event wire-string test iterate EventType::ALL (SMA-606)"
```

---

### Task 2: `Mutated<T>` and the `EntityGenBumper` port

**Files:**
- Modify: `rs/crates/libs/paigasus-iam-core/src/ports.rs`
- Modify: `rs/crates/libs/paigasus-iam-core/src/lib.rs:30-34`

**Interfaces:**
- Produces: `paigasus_iam_core::Mutated<T>` with public fields `value: T` and `changed: bool`; `paigasus_iam_core::EntityGenBumper` with `async fn bump(&self)`. Tasks 4, 6, 7, 8, 9 all consume these.

- [ ] **Step 1: Add `Mutated<T>` beside `NodeView`**

In `ports.rs`, immediately after the `NodeView<T>` definition (currently ending at line 61):

```rust
/// A mutation's result plus whether it actually changed anything (SMA-606 D1).
///
/// `changed == false` is a no-op: SMA-440 D5 established that a write changing no stored
/// value must not restamp, and SMA-606 extends that to "and must not emit" — the service
/// skips the outbox and audit writes, exactly as `ApiKeyService::revoke` gates on
/// `revoke_in`'s bool. The post-commit generation bump still runs, because cache
/// invalidation is a separate concern from audit correctness (SMA-440 D5, preserved).
///
/// A named struct rather than a `(T, bool)` tuple: `.changed` states intent at the ten
/// implementations that construct it and the two services that read it, where `.1` would
/// not. The fields are public because ten implementations set them by hand — an accessor
/// would not make a wrong `changed` any less wrong. The control is behavioural, not
/// structural (spec Testing case 2).
#[must_use]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mutated<T> {
    pub value: T,
    pub changed: bool,
}
```

- [ ] **Step 2: Add `EntityGenBumper` beside `PolicyGenBumper`**

In `ports.rs`, immediately after the `PolicyGenBumper` trait (currently ending at line 370):

```rust
/// Post-commit `entity_gen` bump port (SMA-606 D7), the entity-cache twin of
/// [`PolicyGenBumper`].
///
/// It exists because there was no way for a service to bump `entity_gen` at all:
/// `bump_entity_gen` is a *private inherent* method on each Pg tenancy repository, and
/// ADR-0005 keeps `crate::adapters::authz::Generations` out of the application layer. Once
/// the service owns the commit (D1), the bump has to move with it — left inside a repository
/// that no longer commits, it would invalidate the Cedar caches against a transaction that
/// may still roll back.
///
/// Implementations swallow and log their own errors and return nothing, for the same reason
/// [`PolicyGenBumper`] does: the mutation has already committed by the time `bump` runs, so a
/// failed invalidation must never surface as a use-case error — the caches self-heal on their
/// next TTL expiry instead.
#[async_trait]
pub trait EntityGenBumper: Send + Sync {
    async fn bump(&self);
}
```

- [ ] **Step 3: Re-export both from the crate root**

In `lib.rs`, edit the `pub use ports::{…}` block to add `EntityGenBumper` and `Mutated` in alphabetical position:

```rust
pub use ports::{
    ApiKeyRepository, AuditLog, Authenticator, Clock, ConflictKind, EntityGenBumper, EventPublisher, ExternalIdentityRepository, IdGenerator, KeyEntropy, MembershipRecord, MembershipRepository,
    Mutated, NodeView, OrganizationRepository, Outbox, PolicyGenBumper, PreconditionKind, PrincipalRepository, ProjectRepository, PublishError, RepositoryError, Savepoint, SecretHasher,
    ServiceAccountRepository, TeamRepository, Transaction, UnitOfWork,
};
```

- [ ] **Step 4: Build**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo build -p paigasus-iam-core
```

Expected: success. Neither item is dead code — both are `pub` from the crate root.

- [ ] **Step 5: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
git add rs/crates/libs/paigasus-iam-core/src/ports.rs rs/crates/libs/paigasus-iam-core/src/lib.rs
git commit -m "feat(rs): add Mutated<T> and the EntityGenBumper port (SMA-606)"
```

---

### Task 3: The fourteen tenancy `EventType` variants

**Files:**
- Modify: `rs/crates/libs/paigasus-iam-core/src/domain_event.rs` — enum (13-23), `ALL` (25-41), `as_wire` (43-54), `parse` (56-68), `all_lists_every_event_type` (110-129)
- Modify: `rs/crates/services/paigasus-iam/src/adapters/events/cloud_event.rs:178-195` — `no_payload_shape_carries_a_secret_or_pii_key`

**Interfaces:**
- Consumes: Task 1's `ALL`-iterating wire-string test.
- Produces: `EventType::{OrganizationCreated, OrganizationRenamed, OrganizationArchived, OrganizationRestored, TeamCreated, TeamRenamed, TeamArchived, TeamRestored, ProjectCreated, ProjectRenamed, ProjectArchived, ProjectRestored, MembershipAttached, MembershipDetached}`. Tasks 6, 7, 8 consume these.

- [ ] **Step 1: Add the variants to the enum**

Append inside `pub enum EventType` after `PolicyDeleted`:

```rust
    OrganizationCreated,
    OrganizationRenamed,
    OrganizationArchived,
    OrganizationRestored,
    TeamCreated,
    TeamRenamed,
    TeamArchived,
    TeamRestored,
    ProjectCreated,
    ProjectRenamed,
    ProjectArchived,
    ProjectRestored,
    MembershipAttached,
    MembershipDetached,
```

- [ ] **Step 2: Run the build to see every tripwire fire at once**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo build -p paigasus-iam-core 2>&1 | tail -30
```

Expected: FAIL — non-exhaustive matches in `as_wire` and `parse`, plus a length mismatch on `ALL`. This is the tripwire working; fix each in the next steps.

- [ ] **Step 3: Extend `ALL` and its declared length**

Change `pub const ALL: [EventType; 8]` to `pub const ALL: [EventType; 22]` and append the fourteen variants in declaration order after `Self::PolicyDeleted`:

```rust
        Self::OrganizationCreated,
        Self::OrganizationRenamed,
        Self::OrganizationArchived,
        Self::OrganizationRestored,
        Self::TeamCreated,
        Self::TeamRenamed,
        Self::TeamArchived,
        Self::TeamRestored,
        Self::ProjectCreated,
        Self::ProjectRenamed,
        Self::ProjectArchived,
        Self::ProjectRestored,
        Self::MembershipAttached,
        Self::MembershipDetached,
```

- [ ] **Step 4: Extend `as_wire`**

Append these arms:

```rust
            Self::OrganizationCreated => "iam.organization.created",
            Self::OrganizationRenamed => "iam.organization.renamed",
            Self::OrganizationArchived => "iam.organization.archived",
            Self::OrganizationRestored => "iam.organization.restored",
            Self::TeamCreated => "iam.team.created",
            Self::TeamRenamed => "iam.team.renamed",
            Self::TeamArchived => "iam.team.archived",
            Self::TeamRestored => "iam.team.restored",
            Self::ProjectCreated => "iam.project.created",
            Self::ProjectRenamed => "iam.project.renamed",
            Self::ProjectArchived => "iam.project.archived",
            Self::ProjectRestored => "iam.project.restored",
            Self::MembershipAttached => "iam.membership.attached",
            Self::MembershipDetached => "iam.membership.detached",
```

- [ ] **Step 5: Extend `parse`**

Append these arms before the `_ => None` arm:

```rust
            "iam.organization.created" => Some(Self::OrganizationCreated),
            "iam.organization.renamed" => Some(Self::OrganizationRenamed),
            "iam.organization.archived" => Some(Self::OrganizationArchived),
            "iam.organization.restored" => Some(Self::OrganizationRestored),
            "iam.team.created" => Some(Self::TeamCreated),
            "iam.team.renamed" => Some(Self::TeamRenamed),
            "iam.team.archived" => Some(Self::TeamArchived),
            "iam.team.restored" => Some(Self::TeamRestored),
            "iam.project.created" => Some(Self::ProjectCreated),
            "iam.project.renamed" => Some(Self::ProjectRenamed),
            "iam.project.archived" => Some(Self::ProjectArchived),
            "iam.project.restored" => Some(Self::ProjectRestored),
            "iam.membership.attached" => Some(Self::MembershipAttached),
            "iam.membership.detached" => Some(Self::MembershipDetached),
```

- [ ] **Step 6: Extend the exhaustive match and the length assertion**

In `all_lists_every_event_type`, add the fourteen variants to the `|`-chain and change the final assertion to `assert_eq!(EventType::ALL.len(), 22);`:

```rust
            match et {
                EventType::PrincipalCreated
                | EventType::PrincipalArchived
                | EventType::RoleGranted
                | EventType::RoleRevoked
                | EventType::ApiKeyIssued
                | EventType::ApiKeyRevoked
                | EventType::PolicyPut
                | EventType::PolicyDeleted
                | EventType::OrganizationCreated
                | EventType::OrganizationRenamed
                | EventType::OrganizationArchived
                | EventType::OrganizationRestored
                | EventType::TeamCreated
                | EventType::TeamRenamed
                | EventType::TeamArchived
                | EventType::TeamRestored
                | EventType::ProjectCreated
                | EventType::ProjectRenamed
                | EventType::ProjectArchived
                | EventType::ProjectRestored
                | EventType::MembershipAttached
                | EventType::MembershipDetached => {}
            }
        }
        assert_eq!(EventType::ALL.len(), 22);
```

- [ ] **Step 7: Add the tenancy payload shapes to the secret/PII scan**

The spec's D8: this test hand-lists its inputs, so new shapes are uncovered until added. In `cloud_event.rs`'s `no_payload_shape_carries_a_secret_or_pii_key`, append to the `payloads` array:

```rust
            // SMA-606 D9: the tenancy shapes. Hand-listed because this test scans sample
            // values by substring — it cannot see runtime content, so it proves the SHAPE
            // carries no banned key, not that an operator's `name` is free of PII (see the
            // spec's Limitations and the ADR-0016 amendment).
            serde_json::json!({"node_prn": "prn:pgs:iam:::org/o", "slug": "acme", "name": "Acme", "status": "active", "effective_status": "active"}),
            serde_json::json!({"node_prn": "prn:pgs:iam:::org/o", "slug": "acme", "name": "Acme"}),
            serde_json::json!({"node_prn": "prn:pgs:iam:::org/o", "status": "archived", "effective_status": "archived"}),
            serde_json::json!({"membership_id": "m", "principal_prn": "prn:pgs:iam:::principal/p", "node_prn": "prn:pgs:iam:::org/o"}),
            serde_json::json!({"grant_id": "g", "role_key": "org_admin", "scope": "prn:pgs:iam:::org/o", "source": "organization_create"}),
```

- [ ] **Step 8: Run the full core + event tests**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam-core && cargo nextest run -p paigasus-iam --lib -E 'test(cloud_event)'
```

Expected: PASS. `wire_strings_are_namespaced_and_distinct` proves all 22 start with `iam.` and are unique; `event_type_roundtrips_through_wire_strings` proves `parse ∘ as_wire == identity`.

- [ ] **Step 9: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
git add rs/crates/libs/paigasus-iam-core/src/domain_event.rs rs/crates/services/paigasus-iam/src/adapters/events/cloud_event.rs
git commit -m "feat(rs): add the fourteen tenancy event types (SMA-606)"
```

---

### Task 4: `_in` twins on the three node ports

Ten new method signatures' worth of work, six implementations. Adding a trait method with no default body does not compile until every impl lands, so this task is atomic.

**Files:**
- Modify: `rs/crates/libs/paigasus-iam-core/src/ports.rs:97-138`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/persistence/pg_organizations.rs`, `pg_teams.rs`, `pg_projects.rs`
- Modify: `rs/crates/services/paigasus-iam/src/application/fakes.rs` — `InMemoryOrgs`, `InMemoryTeams`, `InMemoryProjects`

**Interfaces:**
- Consumes: `Mutated<T>`, `Transaction` (Task 2).
- Produces, on `OrganizationRepository`:
  - `async fn create_in(&self, tx: &dyn Transaction, org: &Organization, default_team: &Team, owner_grant: &RoleGrant, stamp: &Stamp) -> Result<(), RepositoryError>`
  - `async fn rename_in(&self, tx: &dyn Transaction, id: Uuid, new_slug: Option<&Slug>, new_name: Option<&str>, stamp: &Stamp) -> Result<Mutated<NodeView<Organization>>, RepositoryError>`
  - `async fn set_status_in(&self, tx: &dyn Transaction, id: Uuid, status: NodeStatus, stamp: &Stamp) -> Result<Mutated<NodeView<Organization>>, RepositoryError>`
  - and the `TeamRepository` / `ProjectRepository` equivalents, with `create_in(&self, tx, team: &Team, stamp: &Stamp)` / `create_in(&self, tx, project: &Project, stamp: &Stamp)` and `NodeView<Team>` / `NodeView<Project>`.

- [ ] **Step 1: Add the `_in` signatures to the three traits**

In `ports.rs`, inside `OrganizationRepository`, after the existing `create`:

```rust
    /// Txn-scoped twin of [`OrganizationRepository::create`] (SMA-606 D1): writes the org, its
    /// default team and the owner grant on the caller's own `tx`, so the service can enqueue
    /// the outbox rows and audit entries in the same transaction. Does **not** bump any
    /// generation — that moves to the service, post-commit, through
    /// [`EntityGenBumper`]/[`PolicyGenBumper`] (D7).
    async fn create_in(&self, tx: &dyn Transaction, org: &Organization, default_team: &Team, owner_grant: &RoleGrant, stamp: &Stamp) -> Result<(), RepositoryError>;
```

after the existing `rename`:

```rust
    /// Txn-scoped twin of [`OrganizationRepository::rename`]. `Mutated::changed` is `false`
    /// when every supplied field already equalled the stored one — the SMA-440 D5 no-op — and
    /// the caller then writes no event and no audit entry (SMA-606 D2).
    async fn rename_in(&self, tx: &dyn Transaction, id: Uuid, new_slug: Option<&Slug>, new_name: Option<&str>, stamp: &Stamp) -> Result<Mutated<NodeView<Organization>>, RepositoryError>;
```

and after the existing `set_status`:

```rust
    /// Txn-scoped twin of [`OrganizationRepository::set_status`]. `Mutated::changed` is
    /// `false` for the idempotent case (already at `status`).
    async fn set_status_in(&self, tx: &dyn Transaction, id: Uuid, status: NodeStatus, stamp: &Stamp) -> Result<Mutated<NodeView<Organization>>, RepositoryError>;
```

Add the same three to `TeamRepository` (with `create_in(&self, tx: &dyn Transaction, team: &Team, stamp: &Stamp)` and `NodeView<Team>`) and to `ProjectRepository` (with `project: &Project` and `NodeView<Project>`), copying the doc comments and substituting the type names.

- [ ] **Step 2: Split `PgOrganizationRepository::create` into wrapper + `_in`**

Read the current `create` body (`pg_organizations.rs:172-185`) first. Move everything between `self.db.begin()` and `txn.commit()` into `create_in`, operating on `recover_txn(tx)?` instead of the local `txn`. The wrapper becomes exactly the `pg_api_keys.rs:253-262` shape — and note the wrapper **keeps** both bumps, because fixtures call it:

```rust
    async fn create(&self, org: &Organization, default_team: &Team, owner_grant: &RoleGrant, stamp: &Stamp) -> Result<(), RepositoryError> {
        // SMA-606 D1: a thin one-shot-`UnitOfWork` wrapper over `create_in`, mirroring
        // `PgApiKeyRepository::issue`. There is exactly ONE body — this method owns the
        // transaction and the post-commit bumps, nothing else. Do not inline any of
        // `create_in`'s logic here: the wrapper is exercised only by test fixtures and
        // `create_in` only by the service, so a divergence would be invisible on both paths.
        let txn = self.db.begin().await.map_err(map_err)?;
        let tx: Box<dyn Transaction> = Box::new(SeaOrmTransaction { txn });
        self.create_in(&*tx, org, default_team, owner_grant, stamp).await?;
        tx.commit().await?;
        self.bump_entity_gen().await;
        self.bump_policy_gen().await;
        Ok(())
    }

    async fn create_in(&self, tx: &dyn Transaction, org: &Organization, default_team: &Team, owner_grant: &RoleGrant, stamp: &Stamp) -> Result<(), RepositoryError> {
        let txn = recover_txn(tx)?;
        // <the existing body, verbatim, with `&txn` replaced by `txn` and the
        //  `txn.commit()` / bump lines removed — they now belong to the wrapper above>
        Ok(())
    }
```

Import `SeaOrmTransaction` and `recover_txn` from `super::uow` if not already in scope.

- [ ] **Step 3: Split `PgOrganizationRepository::rename` into wrapper + `_in`**

The current body is at `pg_organizations.rs:208-241`. The no-op arm is what sets `changed: false`:

```rust
    async fn rename(&self, id: Uuid, new_slug: Option<&Slug>, new_name: Option<&str>, stamp: &Stamp) -> Result<NodeView<Organization>, RepositoryError> {
        let txn = self.db.begin().await.map_err(map_err)?;
        let tx: Box<dyn Transaction> = Box::new(SeaOrmTransaction { txn });
        let out = self.rename_in(&*tx, id, new_slug, new_name, stamp).await?;
        tx.commit().await?;
        self.bump_entity_gen().await;
        Ok(out.value)
    }

    async fn rename_in(&self, tx: &dyn Transaction, id: Uuid, new_slug: Option<&Slug>, new_name: Option<&str>, stamp: &Stamp) -> Result<Mutated<NodeView<Organization>>, RepositoryError> {
        let txn = recover_txn(tx)?;

        let Some(model) = organization::Entity::find_by_id(id).lock_exclusive().one(txn).await.map_err(map_err)? else {
            return Err(RepositoryError::NotFound);
        };
        if model.status == NodeStatus::Archived.as_str() {
            return Err(RepositoryError::Precondition(PreconditionKind::NodeArchived));
        }

        // SMA-440 D5: a write that changes nothing stamps nothing. Placed after the archived
        // guard (so a no-op on an archived node still errors) and before the slug-conflict
        // check (a node cannot conflict with the slug it already holds). SMA-606 D2 extends
        // it: `changed: false` also means the caller emits no event and no audit entry.
        let slug_same = new_slug.is_none_or(|s| model.slug == s.as_str());
        let name_same = new_name.is_none_or(|n| model.name == n);
        if slug_same && name_same {
            return Ok(Mutated { value: org_view(model_to_org(model)?), changed: false });
        }

        let mut active = model.into_active_model();
        if let Some(slug) = new_slug {
            active.slug = Set(slug.as_str().to_string());
        }
        if let Some(name) = new_name {
            active.name = Set(name.to_owned());
        }
        active.updated_at = Set(stamp.at);
        active.modified_by = Set(Some(stamp.by.canonical()));
        let updated = active.update(txn).await.map_err(map_err)?;

        Ok(Mutated { value: org_view(model_to_org(updated)?), changed: true })
    }
```

**Note the guard order is unchanged** — `NotFound`, then archived, then no-op, then the update. SMA-440's D5 warns this must not move: testing for a no-op before the archived guard would turn `rename(archived_node, same_slug)` from `Precondition(NodeArchived)` into a silent `Ok`.

- [ ] **Step 4: Split `PgOrganizationRepository::set_status` into wrapper + `_in`**

The current body is at `pg_organizations.rs:245-265`. Its existing `if model.status == status.as_str() { model } else { …update… }` shape becomes the `changed` flag:

```rust
    async fn set_status(&self, id: Uuid, status: NodeStatus, stamp: &Stamp) -> Result<NodeView<Organization>, RepositoryError> {
        let txn = self.db.begin().await.map_err(map_err)?;
        let tx: Box<dyn Transaction> = Box::new(SeaOrmTransaction { txn });
        let out = self.set_status_in(&*tx, id, status, stamp).await?;
        tx.commit().await?;
        self.bump_entity_gen().await;
        Ok(out.value)
    }

    async fn set_status_in(&self, tx: &dyn Transaction, id: Uuid, status: NodeStatus, stamp: &Stamp) -> Result<Mutated<NodeView<Organization>>, RepositoryError> {
        let txn = recover_txn(tx)?;

        let Some(model) = organization::Entity::find_by_id(id).lock_exclusive().one(txn).await.map_err(map_err)? else {
            return Err(RepositoryError::NotFound);
        };

        if model.status == status.as_str() {
            return Ok(Mutated { value: org_view(model_to_org(model)?), changed: false });
        }

        let mut active = model.into_active_model();
        active.status = Set(status.as_str().to_string());
        active.updated_at = Set(stamp.at);
        active.modified_by = Set(Some(stamp.by.canonical()));
        let updated = active.update(txn).await.map_err(map_err)?;

        Ok(Mutated { value: org_view(model_to_org(updated)?), changed: true })
    }
```

- [ ] **Step 5: Do the same three splits in `pg_teams.rs` and `pg_projects.rs`**

Read each existing body first and change only the transaction ownership and the return type — the guards, locks and SQL are unchanged. `pg_teams.rs`: `create` at `:111-124`, `rename` at `:167-207`, `set_status` at `:212-237`. `pg_projects.rs`: `create` at `:115-140`, `rename` at `:193-240`, `set_status` at `:245-274`. Both wrappers call only `self.bump_entity_gen()` — neither has a `bump_policy_gen` (only `PgOrganizationRepository` does, for the owner grant).

- [ ] **Step 6: Split the three node fakes the same way**

In `fakes.rs`, `InMemoryOrgs` (`:66-140`), `InMemoryTeams` (`:163-240`), `InMemoryProjects` (`:268-348`). The fakes hold no database, so the wrapper passes a `CountingTransaction` (`fakes.rs:943`). For `InMemoryOrgs::rename`:

```rust
    async fn rename(&self, id: Uuid, new_slug: Option<&Slug>, new_name: Option<&str>, stamp: &Stamp) -> Result<NodeView<Organization>, RepositoryError> {
        let tx: Box<dyn Transaction> = Box::new(CountingTransaction::detached());
        let out = self.rename_in(&*tx, id, new_slug, new_name, stamp).await?;
        Ok(out.value)
    }
```

Add a `detached()` constructor to `CountingTransaction` returning one whose commit counter is a throwaway `Arc::new(AtomicUsize::new(0))`, so the wrapper path needs no shared counter. Keep each fake's existing no-op comment verbatim on the `_in` body and return `Mutated { value, changed: false }` where it currently returns early.

- [ ] **Step 7: Build and run the whole existing suite**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo build -p paigasus-iam && cargo nextest run -p paigasus-iam --lib
```

Expected: PASS with no test changes. This task is a pure refactor — every existing caller uses the wrappers, whose behaviour is unchanged.

- [ ] **Step 8: Run the Postgres tenancy suites, which exercise the wrappers**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test tenancy_orgs --test tenancy_nodes --test authz_entity_gen_bumps
```

Expected: PASS. `authz_entity_gen_bumps` asserts strict bump counts through the wrappers and is the control that Step 2–5's wrappers still bump exactly once.

- [ ] **Step 9: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
git add rs/crates/libs/paigasus-iam-core/src/ports.rs rs/crates/services/paigasus-iam/src/adapters/persistence/ rs/crates/services/paigasus-iam/src/application/fakes.rs
git commit -m "refactor(rs): add _in twins to the three tenancy node ports (SMA-606)"
```

---

### Task 5: `_in` twins on `MembershipRepository`, and what `detach_in` returns

**Files:**
- Modify: `rs/crates/libs/paigasus-iam-core/src/ports.rs:140-156`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/persistence/pg_memberships.rs`
- Modify: `rs/crates/services/paigasus-iam/src/application/fakes.rs` — `InMemoryMemberships`
- Modify: `rs/crates/services/paigasus-iam/src/application/authenticate_api_key.rs:419`, `authenticate_token.rs:372` — the two extra `InMemoryMemberships` impls

**Interfaces:**
- Produces:
  - `async fn attach_in(&self, tx: &dyn Transaction, membership: &Membership, stamp: &Stamp) -> Result<MembershipRecord, RepositoryError>`
  - `async fn detach_in(&self, tx: &dyn Transaction, id: Uuid) -> Result<Vec<MembershipRecord>, RepositoryError>`

- [ ] **Step 1: Add the two signatures**

```rust
    /// Txn-scoped twin of [`MembershipRepository::attach`] (SMA-606 D1). The returned record
    /// carries the **stored** PRNs, which is what the caller must put in the event — never
    /// the caller's own input (D2's security corollary): this method byte-matches the
    /// supplied PRN against the stored one and answers `PrnMismatch`, and echoing the input
    /// would route a forged org slot straight past that check into the event stream.
    async fn attach_in(&self, tx: &dyn Transaction, membership: &Membership, stamp: &Stamp) -> Result<MembershipRecord, RepositoryError>;

    /// Txn-scoped twin of [`MembershipRepository::detach`], returning **every record it
    /// deleted** — the target row plus, for an org membership, each row the cascade removed
    /// (SMA-606 D6). The service holds only a `Uuid` and the `membership` table stores no PRN
    /// columns, so these records are the only place the cascaded PRNs exist; each becomes one
    /// audit entry and one event, all sharing the call's single correlation id.
    async fn detach_in(&self, tx: &dyn Transaction, id: Uuid) -> Result<Vec<MembershipRecord>, RepositoryError>;
```

- [ ] **Step 2: Add the lock and projection SQL to `pg_memberships.rs`**

Beside the existing five `SELECT` constants:

```rust
/// SMA-606 D6 step 1: locks every row `detach_in` is about to delete, before the projection
/// reads them. `lock_exclusive()` on the target row alone is NOT enough — a concurrent
/// transaction detaching one of the CASCADE rows (its own target, a different row) is not
/// blocked by it, so under READ COMMITTED the projection would see a row that the later
/// DELETE then re-evaluates away, and the trail would report a detach this call never
/// performed. Single table, no UNION, so `FOR UPDATE` is legal here where it is not on
/// `DETACH_PROJECT_SQL` below.
///
/// `$2` is `model.org_id`, an `Option<Uuid>`: for a team or project detach it binds NULL,
/// `org_id = NULL` is never true, and only the `m.id = $3` term matches. That is correct, but
/// it is correct by arithmetic rather than by construction — do not "simplify" the OR away.
const DETACH_LOCK_SQL: &str = r#"SELECT m.id FROM "membership" m
 WHERE m.id = $3
    OR (m.principal_id = $1
        AND (m.team_id    IN (SELECT id FROM "team"    WHERE org_id = $2)
          OR m.project_id IN (SELECT id FROM "project" WHERE org_id = $2)))
 FOR UPDATE"#;

/// SMA-606 D6 step 2: the same row set, projected through `LIST_BY_PRINCIPAL_SQL`'s PRN joins
/// so it fills `MembershipRow` by the mapping every read path already uses. Reusing that
/// projection is the point — a sixth hand-written projection is the hazard SMA-440 D2
/// documents across the existing five, where a statement omitting a column compiles and goes
/// wrong on one path only.
///
/// The `OR` disjunct is neutralised per arm only by the INNER joins: a project row has
/// `team_id IS NULL`, so the team arm's `JOIN "team" t ON t.id = m.team_id` drops it. Correct,
/// but it breaks silently if any arm is ever changed to a LEFT JOIN.
const DETACH_PROJECT_SQL: &str = r#"
SELECT m.id, pr.prn AS principal_prn, o.prn AS node_prn, m.created_at, m.created_by
  FROM "membership" m JOIN "principal" pr ON pr.id = m.principal_id
  JOIN "organization" o ON o.id = m.org_id
 WHERE m.id = $3
    OR (m.principal_id = $1
        AND (m.team_id    IN (SELECT id FROM "team"    WHERE org_id = $2)
          OR m.project_id IN (SELECT id FROM "project" WHERE org_id = $2)))
UNION ALL
SELECT m.id, pr.prn, t.prn, m.created_at, m.created_by FROM "membership" m
  JOIN "principal" pr ON pr.id = m.principal_id JOIN "team" t ON t.id = m.team_id
 WHERE m.id = $3
    OR (m.principal_id = $1
        AND (m.team_id    IN (SELECT id FROM "team"    WHERE org_id = $2)
          OR m.project_id IN (SELECT id FROM "project" WHERE org_id = $2)))
UNION ALL
SELECT m.id, pr.prn, pj.prn, m.created_at, m.created_by FROM "membership" m
  JOIN "principal" pr ON pr.id = m.principal_id JOIN "project" pj ON pj.id = m.project_id
 WHERE m.id = $3
    OR (m.principal_id = $1
        AND (m.team_id    IN (SELECT id FROM "team"    WHERE org_id = $2)
          OR m.project_id IN (SELECT id FROM "project" WHERE org_id = $2)))
ORDER BY created_at, id"#;
```

The column list is `LIST_BY_PRINCIPAL_SQL`'s, verbatim, so `impl From<MembershipRow> for MembershipRecord` (`pg_memberships.rs:66`) maps it unchanged. No `LIMIT`/`OFFSET`: this returns the whole deleted set, not a page. The org arm can only ever match through `m.id = $3` — the cascade never removes an org membership — which is the belt-and-braces half of `DETACH_CASCADE_SQL`'s own "NULL columns never satisfy `IN`" comment.

- [ ] **Step 3: Split `attach` and `detach` into wrapper + `_in`**

`attach`'s existing five-step guard chain (`:145-255`) moves wholesale into `attach_in`, unchanged, on `recover_txn(tx)?`. `detach_in` becomes lock, project, delete:

```rust
    async fn detach_in(&self, tx: &dyn Transaction, id: Uuid) -> Result<Vec<MembershipRecord>, RepositoryError> {
        let txn = recover_txn(tx)?;

        let Some(model) = membership::Entity::find_by_id(id).lock_exclusive().one(txn).await.map_err(map_err)? else {
            return Err(RepositoryError::NotFound);
        };

        let principal_id = model.principal_id;
        let org_id = model.org_id;

        // D6 step 1 — lock the whole set, cascade included, before reading it.
        txn.execute(Statement::from_sql_and_values(DbBackend::Postgres, DETACH_LOCK_SQL, [principal_id.into(), org_id.into(), id.into()]))
            .await
            .map_err(map_err)?;

        // D6 step 2 — project the locked set into records the service can build entries from.
        let rows = MembershipRow::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres, DETACH_PROJECT_SQL, [principal_id.into(), org_id.into(), id.into()]))
            .all(txn)
            .await
            .map_err(map_err)?;
        let deleted: Vec<MembershipRecord> = rows.into_iter().map(MembershipRecord::from).collect();

        // D6 step 3 — the delete, unchanged from before SMA-606.
        if let Some(org_id) = org_id {
            let stmt = Statement::from_sql_and_values(DbBackend::Postgres, DETACH_CASCADE_SQL, [principal_id.into(), org_id.into()]);
            txn.execute(stmt).await.map_err(map_err)?;
        }
        membership::Entity::delete_by_id(id).exec(txn).await.map_err(map_err)?;

        Ok(deleted)
    }
```

The wrapper:

```rust
    async fn detach(&self, id: Uuid) -> Result<(), RepositoryError> {
        let txn = self.db.begin().await.map_err(map_err)?;
        let tx: Box<dyn Transaction> = Box::new(SeaOrmTransaction { txn });
        self.detach_in(&*tx, id).await?;
        tx.commit().await?;
        Ok(())
    }
```

No bump in either: `PgMembershipRepository` has no `gens` field and never bumped, correctly — `pg_entity_slice.rs` never reads memberships (spec D7).

- [ ] **Step 4: Split the three `InMemoryMemberships` impls**

`fakes.rs:406-475`, plus `authenticate_api_key.rs:419` and `authenticate_token.rs:372`. The `fakes.rs` `detach_in` must return the records it removed, so its `retain` becomes a partition:

```rust
    async fn detach_in(&self, _tx: &dyn Transaction, id: Uuid) -> Result<Vec<MembershipRecord>, RepositoryError> {
        let mut memberships = self.0.memberships.lock().unwrap();
        let membership = memberships.get(&id).cloned().ok_or(RepositoryError::NotFound)?;
        memberships.remove(&id);
        let mut deleted = vec![to_record(&membership)];

        // NOTE (SMA-606 D6): this fake cascades on `parent_org_uuid(&m.node)` — the org slot
        // embedded in the caller's PRN — while Postgres resolves the STORED `org_id` by
        // subquery. The two can disagree. That makes this a third statement in the drift set
        // the spec's Risk 3 tracks; the agreement control is Postgres test case 11.
        if let TenancyNodeRef::Organization(org_id) = &membership.node {
            let org_uuid = org_id.uuid();
            let principal_uuid = membership.principal_id.uuid();
            memberships.retain(|_, m| {
                let cascaded = m.principal_id.uuid() == principal_uuid && parent_org_uuid(&m.node) == Some(org_uuid);
                if cascaded {
                    deleted.push(to_record(m));
                }
                !cascaded
            });
        }
        Ok(deleted)
    }
```

The two auth-test fakes only need the methods to exist; give their `attach_in`/`detach_in` the same delegation their `attach`/`detach` already have (read each file first — if the existing bodies are `unimplemented!()`, keep that).

- [ ] **Step 5: Build and run the membership suites**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo build -p paigasus-iam && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --lib --test tenancy_memberships --test http_memberships
```

Expected: PASS with no test changes — `detach_cascades_but_leaves_other_principals_untouched` is the control that the new lock and projection did not change what gets deleted.

- [ ] **Step 6: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
git add rs/crates/libs/paigasus-iam-core/src/ports.rs rs/crates/services/paigasus-iam/src/adapters/persistence/pg_memberships.rs rs/crates/services/paigasus-iam/src/application/
git commit -m "refactor(rs): add attach_in and a record-returning detach_in (SMA-606)"
```

---

### Task 6: `OrganizationService` emits three events, and the `EntityGenBumper` adapter

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/application/organizations.rs`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/authz/generation.rs`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/mod.rs:94-97,:355-363,:394`
- Modify: `rs/crates/services/paigasus-iam/src/application/fakes.rs` — add a counting `EntityGenBumper`

**Interfaces:**
- Consumes: `create_in`/`rename_in`/`set_status_in` (Task 4), `EntityGenBumper` (Task 2), the four org/team `EventType` variants (Task 3).
- Produces: `OrganizationServiceDeps<R, I, C>` with fields `repo`, `uow`, `outbox`, `audit`, `gen_bumper`, `policy_gen_bumper`, `ids`, `clock`; `OrganizationService::new(deps)`.

- [ ] **Step 1: Add `GenerationsEntityGenBumper`**

In `generation.rs`, beside `GenerationsPolicyGenBumper` (`:450-471`):

```rust
/// [`EntityGenBumper`] over the same shared `Generations` handle every other tenancy
/// invalidation uses (SMA-606 D7). Swallow-and-log, exactly like
/// [`GenerationsPolicyGenBumper`]: the mutation has already committed.
#[derive(Clone)]
pub struct GenerationsEntityGenBumper {
    gens: Generations,
}

impl GenerationsEntityGenBumper {
    #[must_use]
    pub fn new(gens: Generations) -> Self {
        GenerationsEntityGenBumper { gens }
    }
}

#[async_trait]
impl EntityGenBumper for GenerationsEntityGenBumper {
    async fn bump(&self) {
        if let Err(err) = self.gens.bump_entity_gen().await {
            tracing::warn!(error = %err, "GenerationsEntityGenBumper: entity_gen bump failed after a committed write — authz caches may serve stale data until TTL");
        }
    }
}
```

- [ ] **Step 2: Add a counting fake bumper**

In `fakes.rs`, beside `FakeUnitOfWork`:

```rust
/// Counts `bump()` calls so a test can assert the post-commit bump ran, and ran after the
/// commit (SMA-606 Testing case 7).
#[derive(Clone, Default)]
pub struct CountingGenBumper(pub Arc<AtomicUsize>);

impl CountingGenBumper {
    pub fn bumps(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl EntityGenBumper for CountingGenBumper {
    async fn bump(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}
```

- [ ] **Step 3: Write the failing tests**

Add to `organizations.rs`'s `mod tests`:

```rust
    /// SMA-606 D4: create writes three rows, so it emits three events on ONE correlation id.
    /// The team and role events carry `"source": "organization_create"` so a consumer can tell
    /// the auto-provisioned team from an explicit one, and this grant from a user-requested
    /// one that actually passed `RoleService::grant`'s anti-escalation check.
    #[tokio::test]
    async fn create_emits_three_events_on_one_correlation_id() {
        let (svc, outbox, audit, _bumper) = service_with_fakes();
        let actor = PrincipalId::from_prn(principal_prn(1)).unwrap();

        svc.create(&actor, "acme", "Acme").await.unwrap();

        let events = outbox.0.lock().unwrap().clone();
        assert_eq!(events.len(), 3, "org create writes three rows, so it emits three events");
        let corr = events[0].correlation_id.expect("every tenancy event carries a correlation id");
        assert!(events.iter().all(|e| e.correlation_id == Some(corr)), "all three share one correlation id");

        let types: Vec<EventType> = events.iter().map(|e| e.event_type).collect();
        assert_eq!(types, vec![EventType::OrganizationCreated, EventType::TeamCreated, EventType::RoleGranted]);

        let team = &events[1];
        assert_eq!(team.payload["source"], "organization_create");
        let grant = &events[2];
        assert_eq!(grant.payload["source"], "organization_create");
        assert_eq!(grant.aggregate_prn, actor.canonical(), "the role event's aggregate is the principal, matching RoleService and BootstrapAdminSeeder");

        let entries = audit.0.lock().unwrap().clone();
        assert_eq!(entries.len(), 3);
        assert!(entries.iter().all(|e| e.correlation_id == Some(corr)));
        assert_eq!(entries[0].action, Action::CreateOrganization.as_wire());
    }

    /// SMA-440 D5 + SMA-606 D2: a rename whose every supplied field already equals the stored
    /// one changes nothing, so it emits nothing. The negative half is the control — without
    /// it an over-broad no-op that swallows real renames passes.
    #[tokio::test]
    async fn a_no_op_rename_emits_nothing_but_a_real_one_emits() {
        let (svc, outbox, audit, _bumper) = service_with_fakes();
        let actor = PrincipalId::from_prn(principal_prn(1)).unwrap();
        let out = svc.create(&actor, "acme", "Acme").await.unwrap();
        let id = out.organization.id.uuid();
        outbox.0.lock().unwrap().clear();
        audit.0.lock().unwrap().clear();

        svc.rename(id, Some("acme"), Some("Acme"), &actor).await.unwrap();
        assert!(outbox.0.lock().unwrap().is_empty(), "a no-op rename emits no event");
        assert!(audit.0.lock().unwrap().is_empty(), "a no-op rename writes no audit entry");

        svc.rename(id, Some("acme"), Some("Acme Inc"), &actor).await.unwrap();
        let events = outbox.0.lock().unwrap().clone();
        assert_eq!(events.len(), 1, "a matching slug with a differing name is a real rename");
        assert_eq!(events[0].event_type, EventType::OrganizationRenamed);
        assert_eq!(events[0].payload["name"], "Acme Inc", "the payload carries the POST-change name");
    }

    /// SMA-606 D7: the bump is post-commit, awaited, and unconditional — it still runs for a
    /// no-op, preserving SMA-440 D5's deliberate choice to leave cache invalidation alone.
    #[tokio::test]
    async fn the_gen_bump_runs_after_commit_and_even_for_a_no_op() {
        let (svc, _outbox, _audit, bumper) = service_with_fakes();
        let actor = PrincipalId::from_prn(principal_prn(1)).unwrap();
        let out = svc.create(&actor, "acme", "Acme").await.unwrap();
        let before = bumper.bumps();

        svc.rename(out.organization.id.uuid(), Some("acme"), Some("Acme"), &actor).await.unwrap();

        assert_eq!(bumper.bumps(), before + 1, "a no-op still bumps entity_gen");
    }
```

Write a `service_with_fakes()` helper in the same `mod tests` returning `(OrganizationService<…>, FakeOutbox, FakeAuditLog, CountingGenBumper)`, modelled on `api_keys.rs`'s `ServiceWithFakes` (`:393-434`).

- [ ] **Step 4: Run them and confirm they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --lib -E 'test(organizations)'
```

Expected: FAIL to compile — `OrganizationService::new` does not take deps and the service has no `outbox`.

- [ ] **Step 5: Convert the service to a deps struct and emit**

```rust
pub struct OrganizationServiceDeps<R, I, C> {
    pub repo: R,
    pub uow: Arc<dyn UnitOfWork>,
    pub outbox: Arc<dyn Outbox>,
    pub audit: Arc<dyn AuditLog>,
    pub gen_bumper: Arc<dyn EntityGenBumper>,
    pub policy_gen_bumper: Arc<dyn PolicyGenBumper>,
    pub ids: I,
    pub clock: C,
}
```

`create` keeps its current value-building (it holds every PRN, so per D2 it is the one shape that builds events *before* `begin()`), then:

```rust
        let corr = self.ids.new_correlation_id();
        let events = vec![
            DomainEvent {
                id: self.ids.new_event_id(),
                event_type: EventType::OrganizationCreated,
                schema_version: 1,
                aggregate_prn: organization.id.prn().canonical(),
                actor_prn: Some(actor.canonical()),
                occurred_at: stamp.at,
                payload: serde_json::json!({
                    "node_prn": organization.id.prn().canonical(),
                    "slug": organization.slug.as_str(),
                    "name": organization.name,
                    "status": organization.status.as_str(),
                    "effective_status": organization.status.as_str(),
                }),
                correlation_id: Some(corr),
            },
            // …the TeamCreated event, same shape, with "source": "organization_create"…
            // …the RoleGranted event, aggregate_prn = actor.canonical(), payload
            //    {"grant_id","role_key","scope","source":"organization_create"}…
        ];
        let entries = vec![ /* one per event; see D5 for action/resource_prn/detail */ ];

        let tx = self.uow.begin().await?;
        self.repo.create_in(&*tx, &organization, &default_team, &owner_grant, &stamp).await?;
        for ev in &events {
            self.outbox.enqueue(&*tx, ev).await?;
        }
        for entry in &entries {
            self.audit.record(&*tx, entry).await?;
        }
        tx.commit().await?;

        // POST-COMMIT (D7): only reachable once the commit above succeeded. Both, because
        // `create` writes the owner grant — a policy change — as well as three entity rows.
        self.gen_bumper.bump().await;
        self.policy_gen_bumper.bump().await;

        Ok(CreateOrgOutput { organization, default_team })
```

`rename`/`archive`/`restore` follow D2's order — `_in` first, then build from `out.value`:

```rust
    pub async fn rename(&self, id: Uuid, new_slug: Option<&str>, new_name: Option<&str>, actor: &PrincipalId) -> Result<NodeView<Organization>, TenancyError> {
        if new_slug.is_none() && new_name.is_none() {
            return Err(TenancyError::NothingToRename);
        }
        let slug = new_slug.map(Slug::parse).transpose()?;
        let stamp = Stamp::new(self.clock.now(), actor.clone());
        let corr = self.ids.new_correlation_id();

        let tx = self.uow.begin().await?;
        let out = self.repo.rename_in(&*tx, id, slug.as_ref(), new_name, &stamp).await?;
        if out.changed {
            // D2: built AFTER the call. `TeamId`/`ProjectId` have no `from_uuid`, so the
            // sibling services cannot build a PRN from their `id` argument at all; org can,
            // but follows the same order so the four services read alike — and because the
            // payload must carry the POST-change slug and name, which only `out.value` has.
            let ev = self.org_event(EventType::OrganizationRenamed, &out.value, &stamp, corr);
            let entry = self.org_entry(Action::RenameOrganization, &out.value, &stamp, corr, serde_json::json!({}));
            self.outbox.enqueue(&*tx, &ev).await?;
            self.audit.record(&*tx, &entry).await?;
        }
        tx.commit().await?;
        self.gen_bumper.bump().await;
        Ok(out.value)
    }
```

Write private `org_event` and `org_entry` helpers on the service (the `system_retirement.rs:152` / `dead_letters.rs:93` precedent) so the four methods share one construction site. `archive`/`restore` call `set_status_in` and pick `OrganizationArchived`/`OrganizationRestored`.

- [ ] **Step 6: Rewire the composition root**

Move the four service constructions from `http/mod.rs:355-363` to **below** `audit_log` (`:394`). Add above them:

```rust
        // SMA-606 D2/D7: the tenancy services now drive mutation + outbox + audit on one
        // transaction and bump post-commit, so they need `audit_log` and must be constructed
        // after it. `tenancy_gen_bumper` is over the SAME `gens` handle every other authz
        // invalidation uses — the services never import `Generations` (ADR-0005).
        let tenancy_uow: Arc<dyn UnitOfWork> = Arc::new(SeaOrmUnitOfWork::new(db.clone()));
        let tenancy_outbox: Arc<dyn Outbox> = Arc::new(PgOutbox::new(cfg.outbox.wake_on_commit));
        let tenancy_gen_bumper: Arc<dyn EntityGenBumper> = Arc::new(GenerationsEntityGenBumper::new(gens.clone()));
```

and construct `orgs` with the deps struct, reusing `role_gen_bumper`'s pattern for the policy bumper. Leave `teams`/`projects`/`memberships` on their positional constructors for now — Tasks 7 and 8 convert them.

- [ ] **Step 7: Run the tests**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --lib -E 'test(organizations)'
```

Expected: PASS, including the three new tests.

- [ ] **Step 8: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
git add rs/crates/services/paigasus-iam/src/
git commit -m "feat(rs): emit events and audit entries from OrganizationService (SMA-606)"
```

---

### Task 7: `TeamService` and `ProjectService`

Same shape as Task 6, minus the three-event create and the policy bumper.

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/application/teams.rs`, `projects.rs`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/mod.rs`

**Interfaces:**
- Produces: `TeamServiceDeps<R, I, C>` and `ProjectServiceDeps<PR, TR, I, C>`, each with `uow`, `outbox`, `audit`, `gen_bumper` added to the existing fields.

- [ ] **Step 1: Write the failing tests**

Add to `teams.rs`'s `mod tests` (and the project twin in `projects.rs`, substituting the types and `EventType::Project*`):

```rust
    /// SMA-606 D1/D2: one event and one entry per mutation, sharing one correlation id, with
    /// the action taken from `Action::as_wire()` rather than a hand-typed literal.
    #[tokio::test]
    async fn each_team_mutation_emits_one_event_and_one_entry() {
        let (svc, outbox, audit, _bumper, org) = service_with_fakes().await;
        let actor = PrincipalId::from_prn(principal_prn(1)).unwrap();

        let view = svc.create(org, "eng", "Engineering", &actor).await.unwrap();
        outbox.0.lock().unwrap().clear();
        audit.0.lock().unwrap().clear();

        svc.archive(view.node.id.uuid(), &actor).await.unwrap();

        let events = outbox.0.lock().unwrap().clone();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, EventType::TeamArchived);
        assert_eq!(events[0].payload["status"], "archived");
        assert_eq!(events[0].payload["effective_status"], "archived", "D9: both statuses, since a node's own status and its effective one can differ");

        let entries = audit.0.lock().unwrap().clone();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].action, Action::ArchiveTeam.as_wire());
        assert_eq!(entries[0].correlation_id, events[0].correlation_id);
    }

    /// SMA-606 D5: every emitted action string comes from `Action::as_wire()`. A hand-typed
    /// literal would be a free `String` nothing checks, and `AuditFilter.action` is how
    /// operators query — a typo makes rows permanently unfindable.
    #[tokio::test]
    async fn the_emitted_actions_match_the_action_vocabulary() {
        let (svc, _outbox, audit, _bumper, org) = service_with_fakes().await;
        let actor = PrincipalId::from_prn(principal_prn(1)).unwrap();
        let view = svc.create(org, "eng", "Engineering", &actor).await.unwrap();
        let id = view.node.id.uuid();
        svc.rename(id, Some("eng2"), None, &actor).await.unwrap();
        svc.archive(id, &actor).await.unwrap();
        svc.restore(id, &actor).await.unwrap();

        let actions: Vec<String> = audit.0.lock().unwrap().iter().map(|e| e.action.clone()).collect();
        assert_eq!(actions, vec![
            Action::CreateTeam.as_wire(),
            Action::RenameTeam.as_wire(),
            Action::ArchiveTeam.as_wire(),
            Action::RestoreTeam.as_wire(),
        ]);
    }
```

- [ ] **Step 2: Run them and confirm they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --lib -E 'test(teams) or test(projects)'
```

Expected: FAIL to compile.

- [ ] **Step 3: Convert both services**

Apply Task 6 Step 5's shape. `create` builds its event before `begin()` (the service constructs the entity and holds the PRN); `rename`/`archive`/`restore` build after the `_in` call from `out.value`. Both `create` methods keep their existing post-`commit` refetch (`teams.rs:41`, `projects.rs:51`) — it runs after the commit, which is effectively where it runs today. `ProjectService::create`'s pre-read of its parent team (`projects.rs:39`) stays **outside** `begin()`: the repository re-guards under `FOR SHARE` regardless, so moving it in would only lengthen the transaction.

- [ ] **Step 4: Rewire both in the composition root**

- [ ] **Step 5: Run the tests**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --lib
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
git add rs/crates/services/paigasus-iam/src/
git commit -m "feat(rs): emit events and audit entries from TeamService and ProjectService (SMA-606)"
```

---

### Task 8: `MembershipService`, and `detach`'s new actor

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/application/memberships.rs` (incl. its tests at `:234,:238,:243`)
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/memberships.rs:96`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/grpc/tenancy.rs:638,:653`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/mod.rs`

**Interfaces:**
- Produces: `MembershipServiceDeps<M, I, C>`; `MembershipService::detach(&self, id: Uuid, actor: &PrincipalId) -> Result<(), TenancyError>`.

- [ ] **Step 1: Write the failing tests**

```rust
    /// SMA-606 D2 security corollary: the event carries the STORED node PRN from the returned
    /// record, never the caller's input. `attach_in` byte-matches the supplied PRN against the
    /// stored one and answers PrnMismatch; echoing the input would route a forged org slot
    /// past that check into the event stream.
    #[tokio::test]
    async fn attach_emits_the_stored_node_prn_not_the_callers_input() {
        let (svc, outbox, _audit) = service_with_fakes();
        let actor = PrincipalId::from_prn(principal_prn(1)).unwrap();

        let record = svc.attach(&principal_prn(2).to_string(), &org_prn(1).to_string(), &actor).await.unwrap();

        let events = outbox.0.lock().unwrap().clone();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, EventType::MembershipAttached);
        assert_eq!(events[0].payload["node_prn"], record.node_prn, "the event's node_prn is the record's, which the repository resolved");
    }

    /// SMA-606 D6/P2-D5: a cascading org detach emits one event and one entry PER DELETED ROW,
    /// all on one correlation id, so "when did this principal lose access to project X" is
    /// answerable by filtering on that project's PRN. Each cascaded entry is marked
    /// `cascade_of` — authorization ran once, at the org node, and an unmarked entry would
    /// read as a separately authorized DetachMembership (D5).
    #[tokio::test]
    async fn a_cascading_detach_emits_one_event_per_deleted_row() {
        let (svc, outbox, audit) = service_with_fakes();
        let actor = PrincipalId::from_prn(principal_prn(1)).unwrap();
        let org = svc.attach(&principal_prn(2).to_string(), &org_prn(1).to_string(), &actor).await.unwrap();
        svc.attach(&principal_prn(2).to_string(), &team_prn(1, 1).to_string(), &actor).await.unwrap();
        svc.attach(&principal_prn(2).to_string(), &project_prn(1, 1).to_string(), &actor).await.unwrap();
        outbox.0.lock().unwrap().clear();
        audit.0.lock().unwrap().clear();

        svc.detach(org.id, &actor).await.unwrap();

        let events = outbox.0.lock().unwrap().clone();
        assert_eq!(events.len(), 3, "the org row plus the two rows its cascade removed");
        assert!(events.iter().all(|e| e.event_type == EventType::MembershipDetached));
        let corr = events[0].correlation_id.unwrap();
        assert!(events.iter().all(|e| e.correlation_id == Some(corr)), "one operation, one correlation id");
        assert!(events.iter().any(|e| e.payload["node_prn"] == project_prn(1, 1).to_string()), "the project row has its own event, filterable by its PRN");

        let entries = audit.0.lock().unwrap().clone();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries.iter().filter(|e| e.detail.get("cascade_of").is_some()).count(), 2, "the two cascaded rows are marked; the directly authorized one is not");
    }
```

- [ ] **Step 2: Run them and confirm they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --lib -E 'test(memberships)'
```

Expected: FAIL to compile.

- [ ] **Step 3: Convert the service**

`attach` calls `attach_in`, then builds its event and entry from the returned `MembershipRecord`. `detach` gains the actor and fans out:

```rust
    pub async fn detach(&self, id: Uuid, actor: &PrincipalId) -> Result<(), TenancyError> {
        let now = self.clock.now();
        let corr = self.ids.new_correlation_id();

        let tx = self.uow.begin().await?;
        let deleted = self.repo.detach_in(&*tx, id).await?;
        for record in &deleted {
            // D5: the directly requested row carries no provenance key; each cascaded row
            // carries `cascade_of`, because authorization ran once at the org node and an
            // unmarked entry would misstate what was authorized.
            let detail = if record.id == id {
                serde_json::json!({"membership_id": record.id, "principal_prn": record.principal_prn, "node_prn": record.node_prn})
            } else {
                serde_json::json!({"membership_id": record.id, "principal_prn": record.principal_prn, "node_prn": record.node_prn, "cascade_of": id})
            };
            let ev = DomainEvent {
                id: self.ids.new_event_id(),
                event_type: EventType::MembershipDetached,
                schema_version: 1,
                aggregate_prn: record.node_prn.clone(),
                actor_prn: Some(actor.canonical()),
                occurred_at: now,
                payload: detail.clone(),
                correlation_id: Some(corr),
            };
            let entry = AuditEntry {
                id: self.ids.new_audit_id(),
                occurred_at: now,
                actor_prn: Some(actor.canonical()),
                action: Action::DetachMembership.as_wire().to_string(),
                resource_prn: Some(record.node_prn.clone()),
                outcome: AuditOutcome::Committed,
                determining_policies: vec![],
                detail,
                correlation_id: Some(corr),
            };
            self.outbox.enqueue(&*tx, &ev).await?;
            self.audit.record(&*tx, &entry).await?;
        }
        tx.commit().await?;
        Ok(())
    }
```

No generation bump: `MembershipService` has no bumper, because memberships never fed the entity slice (D7).

- [ ] **Step 4: Update the two transport call sites and the service's own tests**

`http/memberships.rs:96` becomes `s.memberships.detach(id, &ctx.principal_id).await?;`. In `grpc/tenancy.rs`, `:638` binds `actor` as a `Prn` — add a second binding for the `&PrincipalId` and pass it at `:653`. Update `memberships.rs:234,:238,:243`.

- [ ] **Step 5: Rewire in the composition root**

- [ ] **Step 6: Run everything**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --lib && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test tenancy_memberships --test http_memberships --test grpc_tenancy
```

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
git add rs/crates/services/paigasus-iam/src/
git commit -m "feat(rs): emit a detach trail per deleted row and stamp its actor (SMA-606)"
```

---

### Task 9: Atomicity and the mid-transaction failure

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/application/organizations.rs` (tests)
- Modify: `rs/crates/services/paigasus-iam/src/application/fakes.rs`

**Interfaces:**
- Consumes: everything from Tasks 6–8.

- [ ] **Step 1: Add a failing-repository fake**

Modelled on `FailingRevokeApiKeys` (`api_keys.rs:465-493`):

```rust
/// Wraps an `OrganizationRepository` and fails `rename_in` after the caller has begun its
/// transaction, so a test can prove the outbox and audit writes roll back with the mutation
/// (SMA-606 Testing case 6).
pub struct FailingRenameOrgs(pub InMemoryOrgs);
```

Delegate every method to `self.0`, and make `rename_in` return `Err(RepositoryError::Backend(...))`.

- [ ] **Step 2: Write the test**

```rust
    /// SMA-606 Risk 1: an event must never outlive a mutation that rolled back. Paired with
    /// `a_no_op_rename_emits_nothing_but_a_real_one_emits` — either test alone passes an
    /// implementation that emits nothing at all, or one that always emits.
    #[tokio::test]
    async fn a_failure_mid_transaction_leaves_no_event_and_no_entry() {
        let (svc, outbox, audit, _bumper) = service_with_failing_rename();
        let actor = PrincipalId::from_prn(principal_prn(1)).unwrap();

        let err = svc.rename(Uuid::from_u128(1), Some("acme2"), None, &actor).await.unwrap_err();

        assert!(matches!(err, TenancyError::Backend(_)));
        assert!(outbox.0.lock().unwrap().is_empty(), "the event must not survive a failed mutation");
        assert!(audit.0.lock().unwrap().is_empty(), "nor the audit entry");
    }
```

- [ ] **Step 3: Run it**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --lib -E 'test(a_failure_mid_transaction)'
```

Expected: PASS — the `?` on `rename_in` returns before either write.

- [ ] **Step 4: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
git add rs/crates/services/paigasus-iam/src/
git commit -m "test(rs): prove a failed tenancy mutation emits nothing (SMA-606)"
```

---

### Task 10: The Postgres tier

The fakes cannot prove the two things that matter most: that step 2's projection and step 3's `DELETE` agree on a row set, and that a concurrent detach cannot make this call over-report.

**Files:**
- Create: `rs/crates/services/paigasus-iam/tests/tenancy_events_pg.rs`
- Modify: `rs/crates/services/paigasus-iam/tests/mutation_audit_e2e.rs`

- [ ] **Step 1: Write the atomicity test**

Model the harness on `tests/outbox_uow_pg.rs`. Assert that after a committed org create the `organization`, `event_outbox` and `audit_log` rows are all visible with one shared `correlation_id`, and that a rolled-back transaction leaves none of the three.

- [ ] **Step 2: Write the cascade row-count test**

```rust
/// SMA-606 D6, Testing case 11. THE control for Risk 3: the projection SELECT and the cascade
/// DELETE are two statements that must describe the same row set, and only Postgres can prove
/// they do — the fake implements the cascade on a different key entirely (the caller's PRN,
/// not the stored org_id), so it can prove fan-out but never agreement.
#[tokio::test]
async fn a_cascading_detach_writes_exactly_one_audit_row_per_deleted_membership() {
    // attach the principal to an org, two teams and two projects in that org, plus one
    // membership in a DIFFERENT org that must survive; detach the org membership; then
    // assert deleted_count == audit_row_count == 5 and the surviving row is untouched.
}
```

- [ ] **Step 3: Write the concurrency test**

```rust
/// SMA-606 D6 step 1, Testing case 12. Without the FOR UPDATE lock the projection sees a
/// cascade row that a concurrent detach then removes first, and this call reports a detach it
/// never performed. Two transactions: this one locks and projects, the peer commits its own
/// detach of a cascade row, then this one deletes and commits.
#[tokio::test]
async fn a_concurrent_detach_of_a_cascade_row_does_not_make_this_call_over_report() {
    // …
}
```

To prove the test is real, temporarily drop `FOR UPDATE` from `DETACH_LOCK_SQL` and confirm it fails; restore it.

- [ ] **Step 4: Extend the end-to-end test**

Add a tenancy mutation to `mutation_audit_e2e.rs`'s existing HTTP → row → relay chain, asserting the CloudEvent's `type` is `iam.organization.created`.

- [ ] **Step 5: Run the Postgres tier**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test tenancy_events_pg --test mutation_audit_e2e
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
git add rs/crates/services/paigasus-iam/tests/
git commit -m "test(rs): prove the tenancy trail commits atomically and the cascade agrees (SMA-606)"
```

---

### Task 11: Full-graph verification

Per-project Moon tasks do **not** run the repo-level gates. Run the graph the way CI does before pushing.

- [ ] **Step 1: Rust checks**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo fmt --check && cargo clippy --workspace --locked -- -D warnings
```

- [ ] **Step 2: The whole IAM suite with Docker**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run --locked -p paigasus-iam --profile iam
```

Expected: PASS. `nats_permissions.rs` iterates `EventType::ALL`, so it now proves the publisher grant covers all 22 subjects.

- [ ] **Step 3: The full affected graph**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :http-extractor-envelope :input-liveness :promtool :observability-drift \
  :nats-permissions :release-parity :release-parity-py :release-parity-ts \
  :publish-metadata :version-lockstep :workflow-credentials --base origin/main --include-relations
```

Expected: PASS. No new crate, no new dependency, no proto edit, so `affected-smoke`'s expected sets and the codegen-drift step are unaffected. If `release-parity*` aborts at rc=2 with a `proto` NDJSON message, that is the documented agent-session artifact — `unset AI_AGENT CLAUDECODE CLAUDE_CODE_ENTRYPOINT` and re-run those three.

- [ ] **Step 4: Confirm no stray debug code**

```bash
git diff origin/main --stat && git diff origin/main | grep -n 'dbg!\|println!\|TODO\|FIXME' || echo "clean"
```

---

## Verification checklist

- [ ] All fourteen variants have a wire string, a `parse` arm, an `ALL` entry, and are covered by `type_matches_the_wire_string_for_every_variant` (now iterating `ALL`).
- [ ] Every one of the thirteen row-preserving mutations plus `detach` emits an event and an entry sharing one correlation id.
- [ ] A no-op `rename` and a no-op `set_status` emit nothing, and the negative half of each proves a real change still does.
- [ ] Org create emits three events; the team and role events carry `"source": "organization_create"`; the role event's `aggregate_prn` is the principal.
- [ ] A cascading detach emits one event and one entry per deleted row, cascaded rows marked `cascade_of`.
- [ ] `attach`'s event carries the stored `node_prn`, not the caller's input.
- [ ] Every `action` string comes from `Action::as_wire()`.
- [ ] The post-commit bump runs after commit, and still runs for a no-op.
- [ ] `MembershipService` has no generation bumper.
- [ ] Each repository wrapper is `begin` → delegate → `commit` → bump, with no duplicated logic.
- [ ] **The spec's D10 deploy constraint is in the PR body.** It implements no code, so nothing above enforces it: `OutboxRelay::row_to_domain_event` (`adapters/events/relay.rs:96`) returns `Err` for an unrecognized wire string and the relay parks the row after `max_attempts`. Every replica must therefore carry the new `EventType` set before any replica writes one, and a **rollback** after tenancy events have been written will park them — recoverable through the SMA-469 dead-letter replay path. The PR body must say so, because the operator rolling back is the only reader who can act on it.
- [ ] `cargo clippy --workspace -- -D warnings` and `cargo fmt --check` pass.
- [ ] The full `moon ci` target list passes.
