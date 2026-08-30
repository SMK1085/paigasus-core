# SMA-440 Part 1 — Server-side audit stamping Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stamp *who* created and last modified every tenancy aggregate, so `AuditMetadata.creator`/`.modifier` stop being hard-coded absent on the wire.

**Architecture:** A new `Stamp { at, by }` value object in `paigasus-iam-core` replaces the `now: DateTime<Utc>` parameter on the six `rename`/`set_status` port methods and is **added** to the four `create`/`attach` methods. The four entity constructors take it too, because the entity is also the read model. Migration `m0011` adds nullable PRN text columns; `NULL` reads as the absent `Actor` that `actor.proto` already defines as unknown-or-system, so there is no backfill. A write that changes nothing stamps nothing.

**Tech Stack:** Rust (edition 2024, rust-version 1.95), SeaORM + Postgres, tonic/prost, axum, `cargo nextest`, Moon 2.5.3.

**Spec:** `docs/superpowers/specs/2026-08-30-sma-440-server-side-audit-stamping-design.md` (Part 1 only — Part 2 is SMA-606 and is **not** in this plan)

## Global Constraints

- Every source file opens with `// SPDX-License-Identifier: Apache-2.0`. Files here already exist except the migration — do not add or remove headers elsewhere.
- **Working directory:** the worktree `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-440`. Do **not** `cd` to the main checkout.
- **PATH:** prefix every shell command with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` — the Bash tool's PATH lacks moon/nextest/buf.
- **`[workspace.lints.rust] warnings = "deny"`** — dead code is a hard compile error. This dictates task order; do not reorder Tasks 1–3.
- **Docker:** for any *filtered* test run (`-E 'test(...)'`, `--test <name>`) prefix `PAIGASUS_REQUIRE_DOCKER=1`, because the Docker canary is not in the filter and the suites would otherwise skip silently and report a green that tested nothing.
- Conventional commits with a workspace scope: `feat(rs):`, `fix(rs):`, `test(rs):`. Subject starts **lowercase**, ≤100 chars. Never write a bare `#NNN` in a commit body (commitlint reads it as a footer and fails `footer-leading-blank`) — write "PR NNN".
- Do **not** use `--no-verify`. The worktree is provisioned; commitlint works.
- `PrincipalId` is imported from `paigasus_iam_core::PrincipalId` in the service crate and is in scope inside `paigasus-iam-core` itself via `crate::value::PrincipalId`.
- **Id constructors:** this plan writes `OrganizationId::new(Uuid::from_u128(n))` in test fixtures as shorthand. Before using it, read the surrounding file and copy whichever constructor its existing tests use (`OrganizationId::new`, `::from_uuid`, or `::from_prn`) — the same applies to `TeamId` and `ProjectId`. Do not introduce a second idiom.

## A note on task shape

The service crate is one compilation unit, so a type change that touches its ports cannot be split into independently-compiling halves. **Task 3 is therefore deliberately large and mechanical**: its job is to restore a green build after Task 2 changes the core's signatures. Tasks 4 onward add behaviour on top of a compiling base and are normal size. Do not try to subdivide Task 3 — you will land in a non-compiling intermediate state with no way to run tests.

## File structure

| File | Responsibility | Task |
|---|---|---|
| `rs/crates/libs/paigasus-iam-core/src/value.rs` | `Stamp` lives beside `PrincipalId` | 1 |
| `rs/crates/libs/paigasus-iam-core/src/lib.rs` | re-export `Stamp` | 1 |
| `rs/crates/libs/paigasus-iam-core/src/tenancy.rs` | entity fields + constructors | 2, 5 |
| `rs/crates/libs/paigasus-iam-core/src/ports.rs` | port signatures, `MembershipRecord` | 2, 5 |
| `.../persistence/migration/m0011_audit_stamp_columns.rs` | the four tables' columns | 3 |
| `.../persistence/entities/{organization,team,project,membership}.rs` | SeaORM models | 3, 5 |
| `.../persistence/pg_{organizations,teams,projects}.rs` | mapping + writes + no-op | 3, 4 |
| `.../persistence/pg_memberships.rs` | `MembershipRecord` + 9 `SELECT`s | 5 |
| `.../application/{organizations,teams,projects,memberships}.rs` | build the `Stamp` | 3, 5 |
| `.../application/fakes.rs` | in-memory twins | 3, 4, 5 |
| `.../adapters/{grpc/tenancy.rs,http/*.rs}` | pass the actor | 3, 5 |
| `.../adapters/grpc/convert.rs` | `AuditFields` + `audit()` | 6 |
| `.../adapters/http/dto.rs` | flat JSON fields | 7 |

---

### Task 1: The `Stamp` value object

**Files:**
- Modify: `rs/crates/libs/paigasus-iam-core/src/value.rs` (append at end, before `#[cfg(test)] mod tests`)
- Modify: `rs/crates/libs/paigasus-iam-core/src/lib.rs` (the `pub use` list)
- Test: same file's `mod tests`

**Interfaces:**
- Produces: `paigasus_iam_core::Stamp` with public fields `at: DateTime<Utc>` and `by: PrincipalId`, plus `Stamp::new(at, by) -> Stamp`. Every later task consumes this.

`Stamp` is `pub` in a library crate, so it is not dead code even before Task 2 uses it.

- [ ] **Step 1: Write the failing test**

In `rs/crates/libs/paigasus-iam-core/src/value.rs`, inside the existing `#[cfg(test)] mod tests`, add:

```rust
    #[test]
    fn stamp_carries_both_halves_of_a_write() {
        let at = chrono::Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        let by = PrincipalId::from_prn(
            paigasus_kernel::Prn::build("iam", "", None, "principal", Uuid::from_u128(1)).unwrap(),
        );
        let stamp = Stamp::new(at, by.clone());
        assert_eq!(stamp.at, at);
        assert_eq!(stamp.by, by);
    }
```

If `chrono::TimeZone` and `Uuid` are not already imported in that test module, add `use chrono::TimeZone;` and `use uuid::Uuid;` to it.

- [ ] **Step 2: Run the test and verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo test -p paigasus-iam-core stamp_carries_both_halves
```

Expected: FAIL — `cannot find type Stamp in this scope`.

- [ ] **Step 3: Write the implementation**

Append to `value.rs`, immediately after the `impl PrincipalId { ... }` block:

```rust
/// The who+when of a single write (SMA-440).
///
/// Carried as one value so a mutation cannot advance a timestamp without naming the actor.
/// Every mutating repository port takes one, and the application service is the only place
/// that constructs one — from its `Clock` port plus the actor the transport handed it.
///
/// `by` is a [`PrincipalId`] rather than a bare `Prn` because that is the type asserting the
/// PRN names a principal, and every caller already holds one. The *stored* columns are
/// `Option<PrincipalId>` instead, which models a row written before the columns existed —
/// not a missing actor at write time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stamp {
    pub at: DateTime<Utc>,
    pub by: PrincipalId,
}

impl Stamp {
    #[must_use]
    pub fn new(at: DateTime<Utc>, by: PrincipalId) -> Self {
        Stamp { at, by }
    }
}
```

If `value.rs` does not already import them, add `use chrono::{DateTime, Utc};` at the top.

- [ ] **Step 4: Re-export it**

In `rs/crates/libs/paigasus-iam-core/src/lib.rs`, find the `pub use crate::value::{...}` line and add `Stamp` to the braces, keeping the list alphabetical.

- [ ] **Step 5: Run the test and verify it passes**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo test -p paigasus-iam-core stamp_carries_both_halves
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add rs/crates/libs/paigasus-iam-core/src/value.rs rs/crates/libs/paigasus-iam-core/src/lib.rs
git commit -m "feat(rs): add the Stamp write-path value object (SMA-440)"
```

---

### Task 2: Entity fields, constructors and ports for org/team/project

**Files:**
- Modify: `rs/crates/libs/paigasus-iam-core/src/tenancy.rs:186-252` (the three structs and their `new`)
- Modify: `rs/crates/libs/paigasus-iam-core/src/ports.rs:98-138` (the three repository traits)
- Test: `rs/crates/libs/paigasus-iam-core/src/tenancy.rs`'s `mod tests`

**Interfaces:**
- Consumes: `Stamp` from Task 1.
- Produces:
  - `Organization { id, slug, name, status, created_at, updated_at, created_by: Option<PrincipalId>, modified_by: Option<PrincipalId> }`, and the same two fields on `Team` and `Project`.
  - `Organization::new(id: OrganizationId, slug: Slug, name: &str, stamp: &Stamp) -> Result<Self, DomainError>`
  - `Team::new(id: TeamId, slug: Slug, name: &str, stamp: &Stamp) -> Result<Self, DomainError>`
  - `Project::new(id: ProjectId, team_id: TeamId, slug: Slug, name: &str, stamp: &Stamp) -> Result<Self, DomainError>`
  - Ports: `create(&self, org: &Organization, default_team: &Team, owner_grant: &RoleGrant, stamp: &Stamp)`; `rename(&self, id, new_slug, new_name, stamp: &Stamp)`; `set_status(&self, id, status, stamp: &Stamp)` — and the `Team`/`Project` equivalents.

This task leaves the **service** crate non-compiling. That is expected; Task 3 repairs it. Verify with `-p paigasus-iam-core` only.

- [ ] **Step 1: Write the failing test**

In `tenancy.rs`'s `mod tests`, add:

```rust
    /// Named `stamp_at_secs` rather than `test_stamp` deliberately: Task 3 adds a `pub
    /// test_stamp(at: DateTime<Utc>, actor: u128)` to the service crate's `fakes.rs` with a
    /// different first parameter, and two helpers sharing a name across crates invites a
    /// wrong-argument mistake that still compiles.
    fn stamp_at_secs(secs: i64, actor: u128) -> Stamp {
        Stamp::new(
            Utc.timestamp_opt(secs, 0).unwrap(),
            PrincipalId::from_prn(Prn::build("iam", "", None, "principal", Uuid::from_u128(actor)).unwrap()),
        )
    }

    /// SMA-440: the first write sets `modified_by` equal to `created_by`, mirroring the rule
    /// `AuditMetadata` already states for `modified_at` vs `created_at`.
    #[test]
    fn a_new_org_records_its_creator_as_its_first_modifier() {
        let stamp = stamp_at_secs(1_700_000_000, 1);
        let org = Organization::new(OrganizationId::new(Uuid::from_u128(10)), Slug::parse("acme").unwrap(), "Acme", &stamp).unwrap();
        assert_eq!(org.created_by.as_ref(), Some(&stamp.by));
        assert_eq!(org.modified_by.as_ref(), Some(&stamp.by));
        assert_eq!(org.created_at, stamp.at);
        assert_eq!(org.updated_at, stamp.at);
    }
```

Match the existing tests' construction of `OrganizationId` — read the file's other tests and copy whichever constructor they use (`OrganizationId::new(uuid)` or `from_prn`). Ensure `Stamp`, `PrincipalId`, `Prn`, `Utc` and `TimeZone` are imported in the test module.

- [ ] **Step 2: Run it and verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo test -p paigasus-iam-core a_new_org_records_its_creator
```

Expected: FAIL — `no field created_by on type Organization`.

- [ ] **Step 3: Add the fields and change the constructors**

In `tenancy.rs`, for `Organization`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Organization {
    pub id: OrganizationId,
    pub slug: Slug,
    pub name: String,
    pub status: NodeStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Who created the row. `None` only for a row written before SMA-440's `m0011` — the
    /// absent `Actor` that `actor.proto` defines as unknown-or-system.
    pub created_by: Option<PrincipalId>,
    /// Who last modified the row. Equals `created_by` on the first write.
    pub modified_by: Option<PrincipalId>,
}
impl Organization {
    pub fn new(id: OrganizationId, slug: Slug, name: &str, stamp: &Stamp) -> Result<Self, DomainError> {
        Ok(Self {
            id,
            slug,
            name: validate_name(name)?,
            status: NodeStatus::Active,
            created_at: stamp.at,
            updated_at: stamp.at,
            created_by: Some(stamp.by.clone()),
            modified_by: Some(stamp.by.clone()),
        })
    }
}
```

Apply the identical two fields and the identical `stamp` substitution to `Team::new` and `Project::new`. `Project::new` keeps its `team_id` parameter and its existing org-mismatch guard unchanged — only the trailing `now: DateTime<Utc>` becomes `stamp: &Stamp`.

Add `use crate::value::Stamp;` to `tenancy.rs`'s imports if `Stamp` is not already reachable there.

- [ ] **Step 4: Change the three port traits**

In `ports.rs`, replace the `now: DateTime<Utc>` parameter with `stamp: &Stamp` on `rename` and `set_status` for all three traits, and **add** `stamp: &Stamp` as the last parameter of all three `create` methods. For `OrganizationRepository`:

```rust
    async fn create(&self, org: &Organization, default_team: &Team, owner_grant: &RoleGrant, stamp: &Stamp) -> Result<(), RepositoryError>;
    async fn rename(&self, id: Uuid, new_slug: Option<&Slug>, new_name: Option<&str>, stamp: &Stamp) -> Result<NodeView<Organization>, RepositoryError>;
    async fn set_status(&self, id: Uuid, status: NodeStatus, stamp: &Stamp) -> Result<NodeView<Organization>, RepositoryError>;
```

Keep every existing doc comment. Add this sentence to each `create`'s doc comment:

```
    /// `stamp` also stamps rows this method writes that are not entities — the owner grant.
```

Import `Stamp` in `ports.rs` if needed.

- [ ] **Step 5: Fix the core crate's own tests and verify**

Update every `Organization::new`/`Team::new`/`Project::new` call inside `paigasus-iam-core` to pass a `&Stamp`. Then:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo test -p paigasus-iam-core
```

Expected: PASS, all tests. (`cargo build --workspace` will still fail — that is Task 3.)

- [ ] **Step 6: Commit**

```bash
git add rs/crates/libs/paigasus-iam-core/src
git commit -m "feat(rs): carry the actor on tenancy entities and their ports (SMA-440)"
```

---

### Task 3: Restore the service crate — migration, adapters, services

**This task is large and mechanical by necessity** (see "A note on task shape"). It ends when `cargo nextest run -p paigasus-iam` is green with no behaviour change beyond stamping.

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/adapters/persistence/migration/m0011_audit_stamp_columns.rs`
- Modify: `.../migration/mod.rs`
- Modify: `.../entities/{organization,team,project,membership}.rs`
- Modify: `.../pg_organizations.rs`, `.../pg_teams.rs`, `.../pg_projects.rs`
- Modify: `.../application/{organizations,teams,projects}.rs`, `.../application/fakes.rs`
- Modify: `.../adapters/grpc/tenancy.rs`, `.../adapters/http/{organizations,teams,projects}.rs`
- Modify: the test fixtures listed in Step 8

**Interfaces:**
- Consumes: `Stamp`, the entity fields, and the port signatures from Task 2.
- Produces: `OrganizationService::rename(&self, id: Uuid, new_slug: Option<&str>, new_name: Option<&str>, actor: &PrincipalId)`, `::archive(&self, id: Uuid, actor: &PrincipalId)`, `::restore(&self, id: Uuid, actor: &PrincipalId)`; `TeamService::create(&self, org: Uuid, slug: &str, name: &str, actor: &PrincipalId)` and its rename/archive/restore twins; the `ProjectService` equivalents. `OrganizationService::create` keeps its existing `actor: &PrincipalId` first parameter.

- [ ] **Step 1: Write the migration**

Create `m0011_audit_stamp_columns.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! m0011 — the tenancy tables gain audit-stamp actor columns (SMA-440).
//!
//! `created_by`/`modified_by` hold a canonical PRN as free-form text with **no foreign key**,
//! following `audit_log.actor_prn` (m0006): an organization must survive the deletion of the
//! principal that created it.
//!
//! `membership` gets `created_by` only — it has no `updated_at` and `iam.proto` marks it
//! immutable.
//!
//! **No backfill.** A pre-migration row keeps NULL, and NULL is the absent `Actor` that
//! `actor.proto` already defines as unknown-or-system. A synthetic "system" PRN would be a
//! *valid* PRN and would read as a real principal, which is worse than nothing.
//!
//! Every statement is idempotent and `SET LOCAL lock_timeout` mirrors m0008/m0009/m0010:
//! SeaORM's migrator does not serialize concurrent `up()` across replicas, and `ADD COLUMN`
//! takes `ACCESS EXCLUSIVE` on tables every authorization decision reads through.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        conn.execute_unprepared("SET LOCAL lock_timeout = '5s';").await?;
        for table in ["organization", "team", "project"] {
            conn.execute_unprepared(&format!(
                r#"ALTER TABLE "{table}"
                     ADD COLUMN IF NOT EXISTS created_by TEXT NULL,
                     ADD COLUMN IF NOT EXISTS modified_by TEXT NULL;"#
            ))
            .await?;
            conn.execute_unprepared(&format!(r#"ALTER TABLE "{table}" DROP CONSTRAINT IF EXISTS ck_{table}_audit_actor_prn;"#)).await?;
            conn.execute_unprepared(&format!(
                r#"ALTER TABLE "{table}" ADD CONSTRAINT ck_{table}_audit_actor_prn
                     CHECK ((created_by IS NULL OR created_by LIKE 'prn:%')
                        AND (modified_by IS NULL OR modified_by LIKE 'prn:%'));"#
            ))
            .await?;
        }
        conn.execute_unprepared(r#"ALTER TABLE "membership" ADD COLUMN IF NOT EXISTS created_by TEXT NULL;"#).await?;
        conn.execute_unprepared(r#"ALTER TABLE "membership" DROP CONSTRAINT IF EXISTS ck_membership_audit_actor_prn;"#).await?;
        conn.execute_unprepared(
            r#"ALTER TABLE "membership" ADD CONSTRAINT ck_membership_audit_actor_prn
                 CHECK (created_by IS NULL OR created_by LIKE 'prn:%');"#,
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        conn.execute_unprepared("SET LOCAL lock_timeout = '5s';").await?;
        for table in ["organization", "team", "project"] {
            conn.execute_unprepared(&format!(r#"ALTER TABLE "{table}" DROP CONSTRAINT IF EXISTS ck_{table}_audit_actor_prn;"#)).await?;
            conn.execute_unprepared(&format!(r#"ALTER TABLE "{table}" DROP COLUMN IF EXISTS modified_by, DROP COLUMN IF EXISTS created_by;"#))
                .await?;
        }
        conn.execute_unprepared(r#"ALTER TABLE "membership" DROP CONSTRAINT IF EXISTS ck_membership_audit_actor_prn;"#).await?;
        conn.execute_unprepared(r#"ALTER TABLE "membership" DROP COLUMN IF EXISTS created_by;"#).await?;
        Ok(())
    }
}
```

Register it in `migration/mod.rs`: add `mod m0011_audit_stamp_columns;` after the m0010 line, and `Box::new(m0011_audit_stamp_columns::Migration),` as the last vec entry.

- [ ] **Step 2: Add the SeaORM columns**

In `entities/organization.rs`, `team.rs` and `project.rs`, add to `Model` after `updated_at`:

```rust
    pub created_by: Option<String>,
    pub modified_by: Option<String>,
```

In `entities/membership.rs`, add only `pub created_by: Option<String>,` (used in Task 5, but the column must match the migration now or every query fails).

- [ ] **Step 3: Map the columns in `pg_organizations.rs`**

Add this helper near the other free functions:

```rust
/// Parses a stored actor PRN. A malformed value reads as `None` rather than erroring:
/// `actor.proto` binds consumers to treat an unparseable actor as unknown, never as a
/// failure. New writes are guarded by m0011's CHECK.
fn model_to_actor(raw: Option<String>) -> Option<PrincipalId> {
    raw.and_then(|s| Prn::parse(&s).ok()).map(PrincipalId::from_prn)
}
```

Extend `org_to_model` (after `updated_at`):

```rust
        created_by: Set(org.created_by.as_ref().map(PrincipalId::canonical)),
        modified_by: Set(org.modified_by.as_ref().map(PrincipalId::canonical)),
```

Extend `team_to_model` the same way. Extend `model_to_org`'s returned struct:

```rust
        created_by: model_to_actor(model.created_by),
        modified_by: model_to_actor(model.modified_by),
```

Repeat the equivalent edits in `pg_teams.rs` and `pg_projects.rs`.

- [ ] **Step 4: Update the pg port implementations**

Change the three `create` signatures to take the trailing `stamp: &Stamp`. In `PgOrganizationRepository::create`, bind it so it is not unused, and use it for the owner-grant row's timestamp:

```rust
    async fn create(&self, org: &Organization, default_team: &Team, owner_grant: &RoleGrant, stamp: &Stamp) -> Result<(), RepositoryError> {
        debug_assert_eq!(org.created_at, stamp.at, "the service must pass the same Stamp it built the entity from");
```

For `rename` and `set_status`, replace `now` with `stamp.at` and add the modifier assignment **next to** the existing `updated_at` line — in `rename`:

```rust
        active.updated_at = Set(stamp.at);
        active.modified_by = Set(Some(stamp.by.canonical()));
```

and in `set_status`'s `else` arm only:

```rust
            active.status = Set(status.as_str().to_owned());
            active.updated_at = Set(stamp.at);
            active.modified_by = Set(Some(stamp.by.canonical()));
```

Leave the `set_status` no-op arm untouched — it must not restamp. Repeat for teams and projects.

- [ ] **Step 5: Update the fakes**

In `fakes.rs`, change the six port-impl signatures the same way. In `InMemoryOrgs::rename`, after the existing `org.updated_at = now;` line:

```rust
        org.updated_at = stamp.at;
        org.modified_by = Some(stamp.by.clone());
```

In `InMemoryOrgs::set_status`, inside the existing `if org.status != status` block only:

```rust
        if org.status != status {
            org.status = status;
            org.updated_at = stamp.at;
            org.modified_by = Some(stamp.by.clone());
        }
```

Repeat for the team and project fakes. Also add a `FixedClock`-friendly helper at the top of `fakes.rs` for tests to use:

```rust
/// Builds a deterministic `Stamp` for tests: `at` from the clock, `by` from a u128 seed.
#[must_use]
pub fn test_stamp(at: DateTime<Utc>, actor: u128) -> Stamp {
    Stamp::new(at, PrincipalId::from_prn(Prn::build("iam", "", None, "principal", Uuid::from_u128(actor)).unwrap()))
}
```

- [ ] **Step 6: Thread the actor through the application services**

In `application/organizations.rs`, `create` already has `actor`. Build the stamp once and pass it to both constructors and the port:

```rust
    pub async fn create(&self, actor: &PrincipalId, slug: &str, name: &str) -> Result<CreateOrgOutput, TenancyError> {
        let slug = Slug::parse(slug)?;
        let stamp = Stamp::new(self.clock.now(), actor.clone());

        let org_id = self.ids.new_organization_id();
        let organization = Organization::new(org_id, slug, name, &stamp)?;

        let team_id = self.ids.new_team_id(organization.id.uuid());
        let default_slug = Slug::parse("default").expect("\"default\" is a valid slug");
        // The auto-provisioned default team records the ORG's creator (spec D8) — the same
        // Stamp, so the two rows cannot disagree.
        let default_team = Team::new(team_id, default_slug, "Default", &stamp)?;

        let grant_id = self.ids.new_membership_id();
        let owner_grant = RoleGrant {
            id: grant_id,
            principal: actor.clone(),
            role_key: "org_admin".to_string(),
            scope: GrantScope::Node(TenancyNodeRef::Organization(organization.id.clone())),
            linked_policy_id: format!("grant:{grant_id}"),
            created_at: stamp.at,
        };

        self.repo.create(&organization, &default_team, &owner_grant, &stamp).await?;
        Ok(CreateOrgOutput { organization, default_team })
    }
```

Add `actor: &PrincipalId` as the **last** parameter of `rename`, `archive` and `restore`, replacing their `let now = self.clock.now();` with `let stamp = Stamp::new(self.clock.now(), actor.clone());` and passing `&stamp`:

```rust
    pub async fn rename(&self, id: Uuid, new_slug: Option<&str>, new_name: Option<&str>, actor: &PrincipalId) -> Result<NodeView<Organization>, TenancyError> {
        if new_slug.is_none() && new_name.is_none() {
            return Err(TenancyError::NothingToRename);
        }
        let slug = new_slug.map(Slug::parse).transpose()?;
        let stamp = Stamp::new(self.clock.now(), actor.clone());
        Ok(self.repo.rename(id, slug.as_ref(), new_name, &stamp).await?)
    }

    pub async fn archive(&self, id: Uuid, actor: &PrincipalId) -> Result<NodeView<Organization>, TenancyError> {
        let stamp = Stamp::new(self.clock.now(), actor.clone());
        Ok(self.repo.set_status(id, NodeStatus::Archived, &stamp).await?)
    }

    pub async fn restore(&self, id: Uuid, actor: &PrincipalId) -> Result<NodeView<Organization>, TenancyError> {
        let stamp = Stamp::new(self.clock.now(), actor.clone());
        Ok(self.repo.set_status(id, NodeStatus::Active, &stamp).await?)
    }
```

`TeamService::create` and `ProjectService::create` gain `actor: &PrincipalId` as their last parameter and build a stamp the same way. Apply the same rename/archive/restore shape to both.

- [ ] **Step 7: Pass the actor from the adapters**

In `adapters/grpc/tenancy.rs`, every tenancy RPC already resolves `actor_context(&request)?`. Where a handler currently discards it, bind it and pass `&actor_principal`. For example at `:221`:

```rust
            let actor_principal = actor_context(&request)?.principal_id;
            // ... existing authorize block ...
            let view = self.state.orgs.rename(id, req.new_slug.as_deref(), req.new_name.as_deref(), &actor_principal).await.map_err(convert::status_to_grpc)?;
```

Do the same for `archive`, `restore`, `teams.create/rename/archive/restore` and `projects.create/rename/archive/restore`.

In `adapters/http/{organizations,teams,projects}.rs`, each handler already takes `Extension<AuthContext>` and defines `actor_prn(&ctx)`. Pass `&ctx.principal_id` to the service call.

- [ ] **Step 8: Update the test fixtures**

Every direct `Organization::new`/`Team::new`/`Project::new` call and every service-method call in these files needs the new argument:

`tests/tenancy_nodes.rs`, `tests/tenancy_orgs.rs`, `tests/authz_entity_slice.rs`, `tests/authz_entity_gen_bumps.rs`, `tests/authz_forged_org_slot_escalation.rs`, plus the `mod tests` blocks in `application/{organizations,teams,projects}.rs`.

Use `fakes::test_stamp(clock.now(), 1)` for entity constructors and `&actor(1)` for service calls, matching each file's existing helper style.

The nine raw-SQL row fixtures (`tests/support/mod.rs:782`, `tests/authz_role_grants.rs:52,:68,:83`, `tests/service_accounts.rs:133`, `tests/authz_schema.rs:47`, `tests/authz_bootstrap.rs:74`, `tests/tenancy_schema.rs:78,:92`) insert rows **without** the new columns. They need **no change** — the columns are nullable, and a NULL actor is exactly the legacy-row case Task 8 asserts.

- [ ] **Step 9: Verify the whole crate is green**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo build --workspace && cargo clippy --workspace -- -D warnings && cargo nextest run -p paigasus-iam
```

Expected: build clean, clippy clean, all tests PASS.

- [ ] **Step 10: Commit**

```bash
git add rs/crates
git commit -m "feat(rs): thread the write stamp through the tenancy write path (SMA-440)"
```

---

### Task 4: A write that changes nothing must not restamp

Implements spec D5. `set_status` already has the branch; `rename` does not and gains one.

**Files:**
- Modify: `.../pg_organizations.rs`, `.../pg_teams.rs`, `.../pg_projects.rs` (the three `rename` bodies)
- Modify: `.../application/fakes.rs` (the three fake `rename` bodies)
- Test: `.../application/organizations.rs`'s `mod tests`, and `tests/tenancy_orgs.rs`

**Interfaces:**
- Consumes: everything from Task 3. No signature changes.

- [ ] **Step 1: Write the failing tests**

In `application/organizations.rs`'s `mod tests`, add:

```rust
    /// SMA-440 D5: a rename supplying the values the row already holds changes nothing, so it
    /// must advance neither `updated_at` nor `modified_by`.
    #[tokio::test]
    async fn rename_to_identical_values_is_a_no_op() {
        let clock = FixedClock::default();
        let t0 = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        clock.set(t0);
        let svc = OrganizationService::new(InMemoryOrgs::default(), SeqIds::default(), clock.clone());

        let created = svc.create(&actor(1), "acme", "Acme").await.unwrap();
        let id = created.organization.id.uuid();

        clock.set(t0 + Duration::seconds(10));
        let same = svc.rename(id, Some("acme"), Some("Acme"), &actor(2)).await.unwrap();
        assert_eq!(same.node.updated_at, t0, "a no-op rename must not advance updated_at");
        assert_eq!(same.node.modified_by.as_ref(), Some(&actor(1)), "a no-op rename must not restamp the modifier");
    }

    /// The negative half, and the one that catches an over-broad no-op: a matching slug with a
    /// DIFFERENT name is a real change and must restamp. Without this, a rename that compares
    /// only the slug would pass the test above while silently dropping every rename.
    #[tokio::test]
    async fn rename_with_a_matching_slug_but_a_new_name_still_changes() {
        let clock = FixedClock::default();
        let t0 = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        clock.set(t0);
        let svc = OrganizationService::new(InMemoryOrgs::default(), SeqIds::default(), clock.clone());

        let created = svc.create(&actor(1), "acme", "Acme").await.unwrap();
        let id = created.organization.id.uuid();

        let t1 = t0 + Duration::seconds(10);
        clock.set(t1);
        let renamed = svc.rename(id, Some("acme"), Some("Acme Corp."), &actor(2)).await.unwrap();
        assert_eq!(renamed.node.name, "Acme Corp.");
        assert_eq!(renamed.node.updated_at, t1);
        assert_eq!(renamed.node.modified_by.as_ref(), Some(&actor(2)));
        // Spec Testing case 2: an update moves the MODIFIER and leaves the CREATOR alone. An
        // implementation that stamps both on every write passes every other assertion here.
        assert_eq!(renamed.node.created_by.as_ref(), Some(&actor(1)), "an update must not rewrite created_by");
        assert_eq!(renamed.node.created_at, t0, "an update must not rewrite created_at");
    }

    /// Guard order: the archived precondition runs BEFORE the no-op test, so renaming an
    /// archived node to its own slug is still an error and not a silent Ok.
    #[tokio::test]
    async fn a_no_op_rename_on_an_archived_node_is_still_rejected() {
        let svc = new_service();
        let created = svc.create(&actor(1), "acme", "Acme").await.unwrap();
        let id = created.organization.id.uuid();
        svc.archive(id, &actor(1)).await.unwrap();
        assert_eq!(svc.rename(id, Some("acme"), None, &actor(2)).await.unwrap_err(), TenancyError::NodeArchived);
    }

    /// The `set_status` half of D5: an idempotent archive advances neither field.
    #[tokio::test]
    async fn an_idempotent_archive_does_not_restamp_the_modifier() {
        let clock = FixedClock::default();
        let t0 = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        clock.set(t0);
        let svc = OrganizationService::new(InMemoryOrgs::default(), SeqIds::default(), clock.clone());

        let created = svc.create(&actor(1), "acme", "Acme").await.unwrap();
        let id = created.organization.id.uuid();

        clock.set(t0 + Duration::seconds(10));
        svc.archive(id, &actor(2)).await.unwrap();

        clock.set(t0 + Duration::seconds(20));
        let again = svc.archive(id, &actor(3)).await.unwrap();
        assert_eq!(again.node.updated_at, t0 + Duration::seconds(10));
        assert_eq!(again.node.modified_by.as_ref(), Some(&actor(2)), "a no-op archive must not restamp");
    }
```

`actor(n)` already exists in that module and returns a `PrincipalId`.

**Then repeat all four tests in `application/teams.rs` and `application/projects.rs`**, against `TeamService` and `ProjectService`. The spec's Risks section asks for the rename-as-A-then-as-B assertion *per aggregate*, and the three fakes are separate implementations that can drift apart — a bug in the project fake is invisible to the org tests. Adjust each file's fixture construction to match its existing `new_service` helper; the assertions are otherwise identical.

- [ ] **Step 2: Run them and verify the two rename no-op tests fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo test -p paigasus-iam --lib application::organizations
```

Expected: `rename_to_identical_values_is_a_no_op` and `a_no_op_rename_on_an_archived_node_is_still_rejected` FAIL; the other two PASS (Task 3 already got `set_status` right).

- [ ] **Step 3: Add the no-op branch to the fake**

In `fakes.rs`'s `InMemoryOrgs::rename`, after the existing archived and slug-conflict guards and before the mutation:

```rust
        let org = orgs.get_mut(&id).expect("existence checked above");

        // SMA-440 D5: every SUPPLIED field already equal to the stored one means this write
        // changes nothing, so it stamps nothing. Placed AFTER the archived and conflict
        // guards so a no-op on an archived node is still an error.
        let slug_same = new_slug.is_none_or(|s| &org.slug == s);
        let name_same = new_name.is_none_or(|n| org.name == n);
        if slug_same && name_same {
            return Ok(org_view(org));
        }

        if let Some(slug) = new_slug {
            org.slug = slug.clone();
        }
        if let Some(name) = new_name {
            org.name = name.to_owned();
        }
        org.updated_at = stamp.at;
        org.modified_by = Some(stamp.by.clone());
        Ok(org_view(org))
```

Apply the identical branch to the team and project fakes.

- [ ] **Step 4: Add the same branch to the three Postgres adapters**

In `PgOrganizationRepository::rename`, after the archived guard and before `let mut active = model.into_active_model();`:

```rust
        // SMA-440 D5: a write that changes nothing stamps nothing. Placed after the archived
        // guard (so a no-op on an archived node still errors) and before the slug-conflict
        // check (a node cannot conflict with the slug it already holds).
        let slug_same = new_slug.is_none_or(|s| model.slug == s.as_str());
        let name_same = new_name.is_none_or(|n| model.name == n);
        if slug_same && name_same {
            txn.commit().await.map_err(map_err)?;
            self.bump_entity_gen().await;
            return Ok(org_view(model_to_org(model)?));
        }
```

`bump_entity_gen` stays unconditional here, matching the existing `set_status` no-op path — making it conditional would change Cedar cache-invalidation behaviour and is out of scope.

Apply the identical branch to `pg_teams.rs` and `pg_projects.rs`.

- [ ] **Step 5: Verify**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo test -p paigasus-iam --lib application::organizations
```

Expected: all four PASS.

- [ ] **Step 6: Add the Postgres twin of the no-op assertions**

In `tests/tenancy_orgs.rs`, inside `rename_and_lifecycle_contracts` after the existing legitimate-rename block, add:

```rust
    // SMA-440 D5: a rename to the values already stored changes nothing, so it advances
    // neither updated_at nor modified_by. The fake and the adapter can disagree, so this
    // asserts the Postgres half of the same rule the unit tests cover.
    let before = repo.find(id).await.unwrap().expect("org exists");
    let noop = repo
        .rename(id, Some(&Slug::parse("acme-renamed").unwrap()), Some("Acme Renamed"), &test_stamp(clock.now(), 99))
        .await
        .unwrap();
    assert_eq!(noop.node.updated_at, before.node.updated_at, "no-op rename must not advance updated_at");
    assert_eq!(noop.node.modified_by, before.node.modified_by, "no-op rename must not restamp the modifier");
```

Import `test_stamp` from the fakes module, or build the `Stamp` inline in this file's existing style.

- [ ] **Step 7: Run the Postgres suite and verify**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test tenancy_orgs
```

Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add rs/crates
git commit -m "fix(rs): never restamp a tenancy write that changes nothing (SMA-440)"
```

---

### Task 5: Membership — entity, `MembershipRecord` and the nine `SELECT`s

Implements spec D2's membership half. This is the task with the compile-silent hazard: a `SELECT` that omits the column builds fine and then disagrees with the other read paths.

**Files:**
- Modify: `rs/crates/libs/paigasus-iam-core/src/tenancy.rs` (`Membership`)
- Modify: `rs/crates/libs/paigasus-iam-core/src/ports.rs:64-70` (`MembershipRecord`) and `:146` (`attach`)
- Modify: `.../persistence/pg_memberships.rs` (`MembershipRow`, nine `SELECT`s, `attach`)
- Modify: `.../application/memberships.rs`, `.../application/fakes.rs`
- Modify: `.../adapters/grpc/tenancy.rs:598`, `.../adapters/http/memberships.rs`
- Test: `tests/tenancy_memberships.rs`

**Interfaces:**
- Produces: `Membership { id, principal_id, node, created_at, created_by: Option<PrincipalId> }`; `Membership::new(id, principal_id, node, stamp: &Stamp)`; `MembershipRecord { id, principal_prn, node_prn, created_at, created_by: Option<PrincipalId> }`; `MembershipRepository::attach(&self, membership: &Membership, stamp: &Stamp)`; `MembershipService::attach(&self, principal_prn: &str, node_prn: &str, actor: &PrincipalId)`.

- [ ] **Step 1: Write the failing test**

In `tests/tenancy_memberships.rs`, add:

```rust
/// SMA-440 D2: `MembershipRecord` is what BOTH wire surfaces project, and it is filled by
/// nine hand-written SELECTs across five constants. A SELECT that omits `m.created_by`
/// compiles and then disagrees with the others, which is exactly the "inconsistent across
/// later reads" defect this issue exists to remove. This asserts all three read paths agree.
#[tokio::test]
async fn every_membership_read_path_agrees_on_the_creator() {
    let (_pg, db) = start_migrated_postgres().await;
    let repo = PgMembershipRepository::new(db.clone());
    let (principal, org) = seed_principal_and_org(&db).await;

    let stamp = test_stamp(Utc.timestamp_opt(1_700_000_000, 0).unwrap(), 7);
    let membership = Membership::new(Uuid::now_v7(), principal.clone(), TenancyNodeRef::Organization(org.clone()), &stamp);
    let attached = repo.attach(&membership, &stamp).await.unwrap();

    let found = repo.find(attached.id).await.unwrap().expect("membership exists");
    let by_principal = repo.list_by_principal(principal.uuid(), 50, 0).await.unwrap();
    let by_node = repo.list_by_node(&TenancyNodeRef::Organization(org), 50, 0).await.unwrap();

    let expected = Some(stamp.by.clone());
    assert_eq!(attached.created_by, expected, "attach must return the creator it wrote");
    assert_eq!(found.created_by, expected, "FIND_SQL must select created_by");
    assert_eq!(by_principal[0].created_by, expected, "LIST_BY_PRINCIPAL_SQL must select created_by");
    assert_eq!(by_node[0].created_by, expected, "LIST_BY_ORG_SQL must select created_by");
}
```

Reuse whatever seeding helper the file already has instead of `seed_principal_and_org` if one exists — read the file first and match its style.

- [ ] **Step 2: Run it and verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test tenancy_memberships
```

Expected: FAIL — `no field created_by on type MembershipRecord`.

- [ ] **Step 3: Add the domain and port fields**

In `tenancy.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Membership {
    pub id: Uuid,
    pub principal_id: PrincipalId,
    pub node: TenancyNodeRef,
    pub created_at: DateTime<Utc>,
    /// Memberships are immutable, so there is no `modified_by` — `iam.proto` marks
    /// `modified_at == created_at`, and the wire projection reuses this for both.
    pub created_by: Option<PrincipalId>,
}
impl Membership {
    #[must_use]
    pub fn new(id: Uuid, principal_id: PrincipalId, node: TenancyNodeRef, stamp: &Stamp) -> Self {
        Self { id, principal_id, node, created_at: stamp.at, created_by: Some(stamp.by.clone()) }
    }
}
```

In `ports.rs`, add `pub created_by: Option<PrincipalId>,` to `MembershipRecord` and add `stamp: &Stamp` to `attach`:

```rust
    async fn attach(&self, membership: &Membership, stamp: &Stamp) -> Result<MembershipRecord, RepositoryError>;
```

- [ ] **Step 4: Add the column to all nine `SELECT`s**

In `pg_memberships.rs`, add `pub created_by: Option<String>,` to `MembershipRow`, then add `m.created_by` to the trailing column list of **every one of the nine** `SELECT` statements — three arms each in `FIND_SQL` and `LIST_BY_PRINCIPAL_SQL`, and one each in `LIST_BY_ORG_SQL`, `LIST_BY_TEAM_SQL` and `LIST_BY_PROJECT_SQL`.

Verify the count before moving on:

```bash
grep -c "m.created_by" rs/crates/services/paigasus-iam/src/adapters/persistence/pg_memberships.rs
```

Expected: **9**. Any other number means a `UNION` arm was missed — that is the whole hazard of this task.

Map it wherever `MembershipRow` becomes a `MembershipRecord`:

```rust
        created_by: row.created_by.and_then(|s| Prn::parse(&s).ok()).map(PrincipalId::from_prn),
```

In `attach`, set the insert column from the stamp and return it on the record:

```rust
        created_by: Set(Some(stamp.by.canonical())),
```

- [ ] **Step 5: Thread it through the service and adapters**

`MembershipService::attach` gains `actor: &PrincipalId`:

```rust
    pub async fn attach(&self, principal_prn: &str, node_prn: &str, actor: &PrincipalId) -> Result<MembershipRecord, TenancyError> {
        let principal = parse_principal_prn(principal_prn)?;
        let node = parse_node_prn(node_prn)?;
        let stamp = Stamp::new(self.clock.now(), actor.clone());
        let membership = Membership::new(self.ids.new_membership_id(), principal, node, &stamp);
        Ok(self.repo.attach(&membership, &stamp).await?)
    }
```

Match the existing body — read it first and change only the stamp-related lines. Update the fake, `grpc/tenancy.rs:598` and `http/memberships.rs` to pass the actor. `detach` is unchanged (it deletes the row; its trail is SMA-606).

- [ ] **Step 6: Verify**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test tenancy_memberships
```

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add rs/crates
git commit -m "feat(rs): record the creator on memberships across every read path (SMA-440)"
```

---

### Task 6: Emit the actor on the gRPC wire

Implements spec D6. This is where the field finally stops being absent.

**Files:**
- Modify: `.../adapters/grpc/convert.rs:245-257` (the `audit` builder) and its six call sites at `:274, :287, :301, :312, :400, :421`
- Test: the same file's `mod tests`, replacing `audit_leaves_the_actor_unset`

**Interfaces:**
- Produces: `convert::AuditFields<'a>` and `convert::audit(f: AuditFields<'_>) -> AuditMetadata`.

- [ ] **Step 1: Write the failing tests**

Replace `audit_leaves_the_actor_unset` entirely with:

```rust
    /// SMA-440 supersedes `audit_leaves_the_actor_unset`, which pinned the absence this issue
    /// removes. The `None` half still matters: `to_proto_service_account` and
    /// `to_proto_api_key` are out of scope and pass `None` deliberately.
    #[test]
    fn audit_maps_a_present_actor_and_preserves_absence() {
        let t = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let creator = principal(1);
        let modifier = principal(2);

        let meta = audit(AuditFields { created: t, modified: t, creator: Some(&creator), modifier: Some(&modifier) });
        assert_eq!(meta.creator.as_ref().map(|a| a.prn.as_str()), Some(creator.canonical().as_str()));
        assert_eq!(meta.modifier.as_ref().map(|a| a.prn.as_str()), Some(modifier.canonical().as_str()));
        assert_eq!(meta.created_at, Some(ts(t)));
        assert_eq!(meta.modified_at, Some(ts(t)));

        let unknown = audit(AuditFields { created: t, modified: t, creator: None, modifier: None });
        assert!(unknown.creator.is_none(), "an absent actor stays absent — the out-of-scope call sites rely on it");
        assert!(unknown.modifier.is_none());
    }

    /// The swap catcher. Testing `audit` alone cannot see a projector that passes its creator
    /// into the modifier slot, so each in-scope projector gets a DISTINCT pair.
    #[test]
    fn each_projector_puts_the_creator_and_modifier_in_their_own_fields() {
        let t = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let creator = principal(1);
        let modifier = principal(2);

        let org = Organization {
            id: OrganizationId::new(Uuid::from_u128(10)),
            slug: Slug::parse("acme").unwrap(),
            name: "Acme".to_string(),
            status: NodeStatus::Active,
            created_at: t,
            updated_at: t,
            created_by: Some(creator.clone()),
            modified_by: Some(modifier.clone()),
        };
        let wire = to_proto_org(&NodeView { node: org, effective_status: NodeStatus::Active });
        let a = wire.audit.expect("audit is always present");
        assert_eq!(a.creator.unwrap().prn, creator.canonical());
        assert_eq!(a.modifier.unwrap().prn, modifier.canonical());
    }
```

Add a `principal(n: u128) -> PrincipalId` helper to that test module if one is not already there, mirroring the `actor` helpers used elsewhere. Write the `to_proto_team` and `to_proto_project` twins the same way; for `to_proto_membership`, assert that **both** `creator` and `modifier` equal the record's single `created_by`.

- [ ] **Step 2: Run and verify failure**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo test -p paigasus-iam --lib adapters::grpc::convert
```

Expected: FAIL — `cannot find struct AuditFields`.

- [ ] **Step 3: Replace the builder**

```rust
/// The four inputs `audit` needs, named rather than positional: two `DateTime<Utc>` and two
/// `Option<&PrincipalId>` in a row would let a swapped pair compile silently, and two of the
/// six call sites pass `None` deliberately, so "it compiles" proves nothing there.
pub struct AuditFields<'a> {
    pub created: DateTime<Utc>,
    pub modified: DateTime<Utc>,
    pub creator: Option<&'a PrincipalId>,
    pub modifier: Option<&'a PrincipalId>,
}

/// Builds `AuditMetadata`. A present `PrincipalId` becomes `Some(Actor { prn })`; `None` stays
/// absent — the canonical "unknown/system" of `actor.proto`, which is what a row written
/// before SMA-440's `m0011` reads back as.
pub fn audit(f: AuditFields<'_>) -> AuditMetadata {
    AuditMetadata {
        created_at: Some(ts(f.created)),
        modified_at: Some(ts(f.modified)),
        creator: f.creator.map(|p| Actor { prn: p.canonical() }),
        modifier: f.modifier.map(|p| Actor { prn: p.canonical() }),
    }
}
```

Add `Actor` to the existing `use paigasus_proto::paigasus::common::v1::{...}` import at `convert.rs:25`.

- [ ] **Step 4: Update the six call sites**

In `to_proto_org`, `to_proto_team` and `to_proto_project`:

```rust
        audit: Some(audit(AuditFields {
            created: v.node.created_at,
            modified: v.node.updated_at,
            creator: v.node.created_by.as_ref(),
            modifier: v.node.modified_by.as_ref(),
        })),
```

In `to_proto_membership` — a membership is immutable and stores no `modified_by`, so its single `created_by` fills both, extending the existing pattern of passing `created_at` twice:

```rust
        audit: Some(audit(AuditFields {
            created: r.created_at,
            modified: r.created_at,
            creator: r.created_by.as_ref(),
            modifier: r.created_by.as_ref(),
        })),
```

In `to_proto_service_account` (`:400`) and `to_proto_api_key` (`:421`) — out of scope, so the `None`s are literal and deliberate:

```rust
        // ServiceAccount/ApiKey stamping is out of SMA-440's scope; these literal `None`s
        // document the remaining gap rather than hiding it behind a default.
        audit: Some(audit(AuditFields { created: sa.created_at, modified: sa.updated_at, creator: None, modifier: None })),
```

- [ ] **Step 5: Verify**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo test -p paigasus-iam --lib adapters::grpc::convert && cargo clippy --workspace -- -D warnings
```

Expected: PASS, clippy clean.

- [ ] **Step 6: Commit**

```bash
git add rs/crates
git commit -m "feat(rs): emit the audit actor on the gRPC tenancy wire (SMA-440)"
```

---

### Task 7: Emit the actor on the HTTP wire

Implements spec D7. The HTTP DTOs use flat JSON fields, not an embedded `AuditMetadata`, so they follow that convention.

**Files:**
- Modify: `.../adapters/http/dto.rs:44-70` (`OrgDto`), and the `TeamDto`, `ProjectDto` and `MembershipDto` blocks below it
- Test: the same file's `mod tests` (add one if absent)

- [ ] **Step 1: Write the failing test**

```rust
    /// SMA-440 D7: the HTTP surface flattens rather than embedding `AuditMetadata`, so it
    /// carries `created_by`/`modified_by` as optional PRN strings. Leaving HTTP out would make
    /// the two surfaces disagree about the same row.
    #[test]
    fn org_dto_carries_both_actors_as_prn_strings() {
        let t = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        let creator = principal(1);
        let modifier = principal(2);
        let dto = OrgDto::from(NodeView {
            node: Organization {
                id: OrganizationId::new(Uuid::from_u128(10)),
                slug: Slug::parse("acme").unwrap(),
                name: "Acme".to_string(),
                status: NodeStatus::Active,
                created_at: t,
                updated_at: t,
                created_by: Some(creator.clone()),
                modified_by: Some(modifier.clone()),
            },
            effective_status: NodeStatus::Active,
        });
        assert_eq!(dto.created_by, Some(creator.canonical()));
        assert_eq!(dto.modified_by, Some(modifier.canonical()));
    }
```

- [ ] **Step 2: Run and verify failure**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo test -p paigasus-iam --lib adapters::http::dto
```

Expected: FAIL — `no field created_by on OrgDto`.

- [ ] **Step 3: Add the fields**

To `OrgDto`, `TeamDto` and `ProjectDto`:

```rust
    pub created_by: Option<String>,
    pub modified_by: Option<String>,
```

and in each `From` impl, after `updated_at`:

```rust
            created_by: view.node.created_by.as_ref().map(PrincipalId::canonical),
            modified_by: view.node.modified_by.as_ref().map(PrincipalId::canonical),
```

To `MembershipDto` add `pub created_by: Option<String>,` only, mapped from `record.created_by`.

- [ ] **Step 4: Verify**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo test -p paigasus-iam --lib adapters::http::dto
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add rs/crates
git commit -m "feat(rs): expose the audit actor on the HTTP tenancy DTOs (SMA-440)"
```

---

### Task 8: Prove the no-backfill decision on real data

Implements spec D4's read-side claim: a row with `created_by IS NULL` reads back as an absent `Actor`, which is what every pre-`m0011` row is.

**Files:**
- Test: `rs/crates/services/paigasus-iam/tests/tenancy_orgs.rs`

Note `start_migrated_postgres` runs `Migrator::up(&db, None)` to the tip, so a genuinely pre-`m0011` row is not constructible. Insert a NULL row directly instead — the assertion is about NULL, not about migration history.

- [ ] **Step 1: Write the test**

```rust
/// SMA-440 D4: no backfill runs, so a row predating `m0011` keeps NULL actor columns. NULL is
/// the absent `Actor` that `actor.proto` already defines as unknown-or-system, which is why
/// inventing a synthetic "system" PRN would have been worse than leaving these alone.
#[tokio::test]
async fn a_row_with_null_actor_columns_reads_back_as_unknown() {
    let (_pg, db) = start_migrated_postgres().await;
    let repo = PgOrganizationRepository::new(db.clone(), Generations::new(db.clone()));

    let id = Uuid::now_v7();
    let prn = OrganizationId::new(id).canonical();
    db.execute_unprepared(&format!(
        r#"INSERT INTO "organization" (id, prn, slug, name, status, created_at, updated_at)
           VALUES ('{id}', '{prn}', 'legacy', 'Legacy', 'active', now(), now());"#
    ))
    .await
    .unwrap();

    let view = repo.find(id).await.unwrap().expect("legacy org exists");
    assert_eq!(view.node.created_by, None);
    assert_eq!(view.node.modified_by, None);

    let wire = convert::to_proto_org(&view);
    let meta = wire.audit.expect("audit is always present");
    assert!(meta.creator.is_none(), "a NULL column must project as an ABSENT Actor, never Actor{{prn:\"\"}}");
    assert!(meta.modifier.is_none());
}
```

Match the file's existing fixture style for building `PgOrganizationRepository` and `Generations` — read the top of the file first.

- [ ] **Step 2: Run and verify it passes**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test tenancy_orgs
```

Expected: PASS. This test should pass on the first run — it asserts a property Tasks 3 and 6 already built. If it fails, the `model_to_actor` mapping or the projector is wrong.

- [ ] **Step 3: Commit**

```bash
git add rs/crates
git commit -m "test(rs): pin that a null stored actor reads back as unknown (SMA-440)"
```

---

### Task 9: Full-graph verification

Per-project Moon tasks do not run the repo-level gates. Run the graph the way CI does before opening the PR.

- [ ] **Step 1: Format and lint**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo fmt --all && cargo clippy --workspace -- -D warnings
```

Expected: no diff from fmt, clippy clean. If `cargo fmt` reflows a signature, commit that separately — renaming to a longer identifier legitimately reflows past `max_width`.

- [ ] **Step 2: Full IAM suite with Docker**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --profile iam
```

Expected: all PASS. An unreachable Docker daemon shows as exactly one red — the `docker_preflight` canary — not 64 silent passes.

- [ ] **Step 3: The CI target graph**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :http-extractor-envelope :input-liveness :promtool :observability-drift \
  :nats-permissions :release-parity :release-parity-py :release-parity-ts \
  :publish-metadata :version-lockstep :workflow-credentials --base origin/main --include-relations
```

Expected: all green. Notes on what should **not** trip, so a red here is a real finding rather than an expected chore:

- `:breaking` — no `.proto` file changed, so buf has nothing to compare.
- The codegen-drift step — no proto change means no regenerated bindings.
- `:affected-smoke` — no new crate and no new in-tree dependency.
- `:error-code-single-site` — no new error variant.
- `:deny`/`:osv`/`:machete` — no new dependency.

If `moon ci` reports an unattributed failure, read `.moon/cache/ciReport.json` to find which target went red.

- [ ] **Step 4: Commit any formatting fallout**

```bash
git add -A
git commit -m "style(rs): apply rustfmt after the audit-stamp signature changes (SMA-440)"
```

Skip this commit if there is no diff.

---

## Verification checklist

Before opening the PR, confirm each spec decision has a live assertion:

- **D1** — all ten mutating port methods take `&Stamp` (Tasks 2, 5).
- **D2** — entities carry flat actor fields; `MembershipRecord` carries one; all nine `SELECT`s agree (Tasks 2, 5; `grep -c "m.created_by"` returns 9).
- **D3** — no code change; every tenancy mutation already had an `AuthContext`.
- **D4** — `m0011` has `lock_timeout`, `IF NOT EXISTS`, a `CHECK` and a real `down()`; a NULL row reads as unknown (Tasks 3, 8).
- **D5** — no-op never restamps, for both `rename` and `set_status`, on both the fake and Postgres, with the over-broad-no-op negative case (Task 4).
- **D6** — `audit` takes a named struct; each in-scope projector has a distinct-pair test (Task 6).
- **D7** — HTTP DTOs carry flat actor fields (Task 7).
- **D8** — the default team records the org creator (Task 3, Step 6; assert it in `application/organizations.rs`'s existing `create_provisions_default_team` test).
