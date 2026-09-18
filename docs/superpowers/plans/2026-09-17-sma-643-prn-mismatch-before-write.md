# SMA-643 — check `prn-mismatch` before a tenancy write commits: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** The nine gRPC tenancy write handlers answer `prn-mismatch` BEFORE they write, so a
forged PRN leaves no row, no event and no audit entry.

**Architecture:** One helper per node kind in `src/adapters/grpc/tenancy.rs` loads the node,
authorizes against the stored PRN, compares the stored canonical PRN with the request PRN, and
logs a warning before it refuses. The nine handlers call the helper and then call the service.
The old comparison after the service call is removed.

**Tech Stack:** Rust (edition 2024, rust-version 1.95), tonic, SeaORM, `cargo nextest`,
Docker-backed integration tests, `tracing`.

**Spec:** `docs/superpowers/specs/2026-09-17-sma-643-prn-mismatch-before-write-design.md`

## Global Constraints

- Every source file opens with `// SPDX-License-Identifier: Apache-2.0`.
- Branch: `feature/sma-643-prn-mismatch-before-write` (already checked out, a git worktree).
- Conventional commits with a workspace scope: `fix(rs): …`, `test(rs): …`, `docs(rs): …`.
- Commit messages end with
  `Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>`.
- Never bypass the git hooks. The worktree already has its pnpm dependencies installed.
- Shell prefix for every command in this plan:
  `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- All tests in this plan need Docker. Run them with `env -u CI` (a stray `CI` value counts as
  "CI present") and with `PAIGASUS_REQUIRE_DOCKER=1`, because a FILTERED nextest run does not
  include the `docker_preflight` canary.
- Reason strings such as `"prn-mismatch"` may appear only in `tests/`. A literal under `src/`
  reds `repo:error-code-single-site`.
- Action names come from `Action::…​.as_wire()`, event types from `EventType::…​.as_wire()`.
  No string literals for either.
- `cargo fmt` uses this workspace's wide `max_width`. Always run `cargo fmt` before a commit;
  a longer identifier can reflow a line.

---

### Task 1: Test harness — two states, counters, PRN forging, and the upper-case guard

**Files:**
- Modify: `rs/crates/services/paigasus-iam/tests/grpc_tenancy.rs` (add to the existing file;
  the module doc is at `:1-11`, the harness at `:31-57`)

**Interfaces:**
- Consumes: `support::{start_migrated_postgres, start_mock_idp, test_config, provision,
  provision_platform_admin, seed_platform_admin, grpc_bearer}` (`tests/support/mod.rs`),
  `spawn_tenancy_server`, `connect`, `authed` (already in this file).
- Produces, for Tasks 2–5:
  - `async fn two_states(db: &DatabaseConnection, idp: &MockIdp) -> (AppState, AppState)` —
    `(enforced, unenforced)`.
  - `async fn audit_count(db: &DatabaseConnection, action: &str, resource_prn: &str) -> u64`
  - `async fn outbox_count(db: &DatabaseConnection, event_type: &str, aggregate_prn: &str) -> u64`
  - `fn with_org(prn: &str, org: &str) -> String`
  - `fn with_region(prn: &str, region: &str) -> String`
  - `fn upper_uuid(prn: &str) -> String`
  - `fn check(failures: &mut Vec<String>, label: &str, ok: bool, detail: String)`
  - `async fn create_org(client, token, slug, name) -> ProtoOrganization`
  - `async fn create_team(client, token, org_prn, slug) -> ProtoTeam`
  - `async fn create_project(client, token, team_prn, slug) -> ProtoProject`

- [ ] **Step 1: Add the imports and the harness helpers**

Add to the `use` block of `tests/grpc_tenancy.rs`:

```rust
use paigasus_iam::adapters::persistence::entities::{audit_log, event_outbox};
use paigasus_iam_core::{Action, EventType};
use paigasus_proto::paigasus::iam::v1::{
    ArchiveOrganizationRequest, ArchiveProjectRequest, ArchiveTeamRequest, CreateProjectRequest, Organization as ProtoOrganization, Project as ProtoProject,
    RenameOrganizationRequest, RenameProjectRequest, RenameTeamRequest, RestoreOrganizationRequest, RestoreProjectRequest, RestoreTeamRequest, Team as ProtoTeam,
};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter};
```

Keep the existing imports. `GetOrganizationRequest`, `GetTeamRequest`, `CreateOrganizationRequest`,
`CreateTeamRequest` and `AttachMembershipRequest` are already imported.

Add these helpers after `authed` (`tests/grpc_tenancy.rs:57`):

```rust
/// Builds the two `AppState`s this file's forged-PRN tests need on ONE database: `enforced`
/// keeps the default `enforce_tenancy = true`, `unenforced` turns it off.
///
/// Two states on one database work (`tests/authz_acceptance.rs:453-454` does the same):
/// `AppState::new` runs no migration and takes no advisory lock, and `reconcile_starter`
/// converges to the code. They do NOT share generation counters, though — `test_config` uses
/// the memory authz cache, so each state builds its own `Generations::memory()`
/// (`http/mod.rs:339-340`). Therefore every node in a case is created, changed and read
/// through the SAME state, and the `platform_admin` grant is seeded through the enforced one.
async fn two_states(db: &DatabaseConnection, idp: &support::MockIdp) -> (AppState, AppState) {
    let enforced = AppState::new(db.clone(), &support::test_config(idp)).await.unwrap();
    let mut cfg = support::test_config(idp);
    cfg.authz.enforce_tenancy = false;
    let unenforced = AppState::new(db.clone(), &cfg).await.unwrap();
    (enforced, unenforced)
}

/// Counts the `audit_log` rows for one action against one resource PRN. The queries below use
/// the SeaORM entities directly, exactly as `tests/mutation_audit_e2e.rs:101-116` does.
async fn audit_count(db: &DatabaseConnection, action: &str, resource_prn: &str) -> u64 {
    audit_log::Entity::find()
        .filter(audit_log::Column::Action.eq(action))
        .filter(audit_log::Column::ResourcePrn.eq(resource_prn))
        .count(db)
        .await
        .expect("count audit_log")
}

/// Counts the `event_outbox` rows for one event type against one aggregate PRN.
async fn outbox_count(db: &DatabaseConnection, event_type: &str, aggregate_prn: &str) -> u64 {
    event_outbox::Entity::find()
        .filter(event_outbox::Column::EventType.eq(event_type))
        .filter(event_outbox::Column::AggregatePrn.eq(aggregate_prn))
        .count(db)
        .await
        .expect("count event_outbox")
}

/// Splits a canonical PRN into its six fields: `prn`, `pgs`, service, region, org, `type/uuid`.
fn prn_fields(prn: &str) -> Vec<&str> {
    let fields: Vec<&str> = prn.splitn(6, ':').collect();
    assert_eq!(fields.len(), 6, "a canonical prn has six fields: {prn}");
    fields
}

/// Replaces the organization slot. `""` removes it.
fn with_org(prn: &str, org: &str) -> String {
    let f = prn_fields(prn);
    format!("prn:pgs:{}:{}:{}:{}", f[2], f[3], org, f[5])
}

/// Replaces the region slot.
fn with_region(prn: &str, region: &str) -> String {
    let f = prn_fields(prn);
    format!("prn:pgs:{}:{}:{}:{}", f[2], region, f[4], f[5])
}

/// Upper-cases ONLY the resource uuid. A correct PRN written this way must still succeed:
/// `Prn::canonical()` lower-cases it, so a fix that compares the raw request string breaks.
fn upper_uuid(prn: &str) -> String {
    let f = prn_fields(prn);
    let (kind, uuid) = f[5].split_once('/').expect("the last prn field is type/uuid");
    format!("prn:pgs:{}:{}:{}:{}/{}", f[2], f[3], f[4], kind, uuid.to_uppercase())
}

/// Records one assertion. Every case collects its failures instead of panicking, so ONE test
/// run shows every failing case — the unfixed run must show all of them, not only the first
/// (spec § 5.1).
fn check(failures: &mut Vec<String>, label: &str, ok: bool, detail: String) {
    if !ok {
        failures.push(format!("{label}: {detail}"));
    }
}

async fn create_org(client: &mut TenancyServiceClient<Channel>, token: &str, slug: &str, name: &str) -> ProtoOrganization {
    client
        .create_organization(authed(
            CreateOrganizationRequest {
                slug: slug.to_string(),
                name: name.to_string(),
            },
            token,
        ))
        .await
        .unwrap_or_else(|e| panic!("create org {slug}: {e}"))
        .into_inner()
        .organization
        .expect("organization")
}

async fn create_team(client: &mut TenancyServiceClient<Channel>, token: &str, org_prn: &str, slug: &str) -> ProtoTeam {
    client
        .create_team(authed(
            CreateTeamRequest {
                org_prn: org_prn.to_string(),
                slug: slug.to_string(),
                name: slug.to_string(),
            },
            token,
        ))
        .await
        .unwrap_or_else(|e| panic!("create team {slug}: {e}"))
        .into_inner()
        .team
        .expect("team")
}

async fn create_project(client: &mut TenancyServiceClient<Channel>, token: &str, team_prn: &str, slug: &str) -> ProtoProject {
    client
        .create_project(authed(
            CreateProjectRequest {
                team_prn: team_prn.to_string(),
                slug: slug.to_string(),
                name: slug.to_string(),
            },
            token,
        ))
        .await
        .unwrap_or_else(|e| panic!("create project {slug}: {e}"))
        .into_inner()
        .project
        .expect("project")
}

/// Reads `ErrorInfo.reason` off a `tonic::Status`. Every IAM status carries one (SMA-504).
fn reason(err: &tonic::Status) -> String {
    let details = tonic_types::StatusExt::get_error_details(err);
    details.error_info().expect("every IAM status carries ErrorInfo").reason.clone()
}
```

`support::MockIdp` must be a public type in `tests/support/mod.rs`. If it is not exported under
that path, use the path that `tests/grpc_users.rs` uses for it and keep the signature.

- [ ] **Step 2: Write the upper-case guard test (T2)**

Append to `tests/grpc_tenancy.rs`:

```rust
/// T2 (spec § 5.2): a CORRECT prn whose uuid is upper-case must still succeed, for all three
/// node kinds. `Prn::canonical()` lower-cases the uuid, so a comparison against the raw request
/// string would refuse this request. This test passes before the SMA-643 fix as well — it
/// guards the fix's shape, and mutation m4 must break it.
#[tokio::test]
async fn an_upper_case_uuid_in_a_correct_prn_still_renames() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let state = AppState::new(db.clone(), &support::test_config(&idp)).await.unwrap();
    let token = idp.bearer("upper-uuid", Some("upper-uuid@example.com"), "paigasus", 3600);
    support::provision_platform_admin(&state, &token).await;
    let (addr, server) = spawn_tenancy_server(state).await;
    let mut client = connect(addr).await;

    let org = create_org(&mut client, &token, "upper-org", "Upper Org").await;
    let team = create_team(&mut client, &token, &org.prn, "upper-team").await;
    let project = create_project(&mut client, &token, &team.prn, "upper-project").await;

    let renamed = client
        .rename_organization(authed(
            RenameOrganizationRequest {
                prn: upper_uuid(&org.prn),
                new_slug: Some("upper-org-2".to_string()),
                new_name: None,
            },
            &token,
        ))
        .await
        .expect("an upper-case uuid must still address the organization")
        .into_inner()
        .organization
        .expect("organization");
    assert_eq!(renamed.prn, org.prn, "the answer must carry the stored, canonical prn");
    assert_eq!(renamed.slug, "upper-org-2");

    client
        .rename_team(authed(
            RenameTeamRequest {
                prn: upper_uuid(&team.prn),
                new_slug: Some("upper-team-2".to_string()),
                new_name: None,
            },
            &token,
        ))
        .await
        .expect("an upper-case uuid must still address the team");

    client
        .rename_project(authed(
            RenameProjectRequest {
                prn: upper_uuid(&project.prn),
                new_slug: Some("upper-project-2".to_string()),
                new_name: None,
            },
            &token,
        ))
        .await
        .expect("an upper-case uuid must still address the project");

    server.abort();
}
```

- [ ] **Step 3: Run the new test**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs
env -u CI PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test grpc_tenancy \
  -E 'test(an_upper_case_uuid_in_a_correct_prn_still_renames)'
```

Expected: PASS. It passes against the unchanged code; Task 8's mutation m4 is what makes it
bite. If it FAILS, stop: either `Prn::parse` refuses an upper-case uuid (then record that and
delete T2), or a helper is wrong.

- [ ] **Step 4: Check that the file still builds clean**

```bash
cd rs && cargo fmt && cargo clippy -p paigasus-iam --tests -- -D warnings
```

Expected: no warning. Unused helpers are used by Tasks 2–5; if clippy reports one as dead code,
leave it and finish Task 2 before the commit, or add nothing — do NOT add `#[allow(dead_code)]`
to a helper that Task 2 uses.

- [ ] **Step 5: Commit**

```bash
git add rs/crates/services/paigasus-iam/tests/grpc_tenancy.rs
git commit -m "test(rs): harness for the forged-prn tenancy tests (SMA-643)" \
  -m "Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: T1 for organizations (6 cases)

**Files:**
- Modify: `rs/crates/services/paigasus-iam/tests/grpc_tenancy.rs`

**Interfaces:**
- Consumes: every helper from Task 1.
- Produces: `async fn org_forged_cases(...)` is local to this test; nothing later depends on it.

- [ ] **Step 1: Write the failing test**

Append to `tests/grpc_tenancy.rs`:

```rust
/// T1 for organizations (spec § 5.2): each of `RenameOrganization`, `ArchiveOrganization` and
/// `RestoreOrganization`, against both `enforce_tenancy` settings, on a FRESH organization per
/// case. A forged prn must answer `prn-mismatch`, must leave the node untouched, and must write
/// neither an `audit_log` row nor an `event_outbox` row. The positive control that follows each
/// case proves the two queries can see a row at all.
///
/// The stored organization prn has an EMPTY organization slot
/// (`paigasus-iam-core/src/tenancy.rs:79`), so the forged shapes here are a NON-EMPTY slot and a
/// non-empty region. `convert::node_uuid` checks only the service and the resource type, so both
/// reach the comparison.
#[tokio::test]
async fn a_forged_prn_never_writes_an_organization() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let (enforced, unenforced) = two_states(&db, &idp).await;
    let token = idp.bearer("forged-org", Some("forged-org@example.com"), "paigasus", 3600);
    // Seeded through the ENFORCED state: `seed_platform_admin` bumps only the state it gets
    // (`tests/support/mod.rs:639-643`), and the two states have separate generation counters.
    support::provision_platform_admin(&enforced, &token).await;
    let (on_addr, on_server) = spawn_tenancy_server(enforced).await;
    let (off_addr, off_server) = spawn_tenancy_server(unenforced).await;
    let mut on = connect(on_addr).await;
    let mut off = connect(off_addr).await;

    let mut failures: Vec<String> = Vec::new();
    let forged_slot = Uuid::from_u128(0x0f01).as_hyphenated().to_string();

    for (setting, client) in [("enforce=on", &mut on), ("enforce=off", &mut off)] {
        // ---- rename ----
        let org = create_org(client, &token, &format!("forged-rn-{setting}"), "Rename Me").await;
        let action = Action::RenameOrganization.as_wire();
        let event = EventType::OrganizationRenamed.as_wire();
        let before = client
            .get_organization(authed(GetOrganizationRequest { prn: org.prn.clone() }, &token))
            .await
            .unwrap()
            .into_inner()
            .organization
            .expect("organization");
        let audits = audit_count(&db, action, &org.prn).await;
        let events = outbox_count(&db, event, &org.prn).await;
        let request = |prn: String| RenameOrganizationRequest {
            prn,
            new_slug: Some(format!("forged-rn-{setting}-renamed")),
            new_name: Some("Renamed".to_string()),
        };
        let label = format!("{setting} RenameOrganization");
        let err = client.rename_organization(authed(request(with_org(&org.prn, &forged_slot)), &token)).await.unwrap_err();
        check(&mut failures, &label, err.code() == Code::InvalidArgument, format!("code was {:?}", err.code()));
        check(&mut failures, &label, reason(&err) == "prn-mismatch", format!("reason was {}", reason(&err)));
        let after = client
            .get_organization(authed(GetOrganizationRequest { prn: org.prn.clone() }, &token))
            .await
            .unwrap()
            .into_inner()
            .organization
            .expect("organization");
        check(&mut failures, &label, after == before, format!("the organization changed: {before:?} -> {after:?}"));
        check(&mut failures, &label, audit_count(&db, action, &org.prn).await == audits, "an audit_log row was written".to_string());
        check(&mut failures, &label, outbox_count(&db, event, &org.prn).await == events, "an event_outbox row was written".to_string());
        // positive control: the same call with the correct prn writes exactly one of each.
        client.rename_organization(authed(request(org.prn.clone()), &token)).await.expect("the correct prn must succeed");
        check(
            &mut failures,
            &label,
            audit_count(&db, action, &org.prn).await == audits + 1 && outbox_count(&db, event, &org.prn).await == events + 1,
            "the positive control wrote no row — the queries cannot see anything".to_string(),
        );

        // ---- archive ----
        let org = create_org(client, &token, &format!("forged-ar-{setting}"), "Archive Me").await;
        let action = Action::ArchiveOrganization.as_wire();
        let event = EventType::OrganizationArchived.as_wire();
        let before = client
            .get_organization(authed(GetOrganizationRequest { prn: org.prn.clone() }, &token))
            .await
            .unwrap()
            .into_inner()
            .organization
            .expect("organization");
        let audits = audit_count(&db, action, &org.prn).await;
        let events = outbox_count(&db, event, &org.prn).await;
        let label = format!("{setting} ArchiveOrganization");
        // a non-empty REGION this time (the second forged shape for this node kind).
        let err = client
            .archive_organization(authed(
                ArchiveOrganizationRequest {
                    prn: with_region(&org.prn, "eu-west-1"),
                },
                &token,
            ))
            .await
            .unwrap_err();
        check(&mut failures, &label, err.code() == Code::InvalidArgument, format!("code was {:?}", err.code()));
        check(&mut failures, &label, reason(&err) == "prn-mismatch", format!("reason was {}", reason(&err)));
        let after = client
            .get_organization(authed(GetOrganizationRequest { prn: org.prn.clone() }, &token))
            .await
            .unwrap()
            .into_inner()
            .organization
            .expect("organization");
        check(&mut failures, &label, after == before, format!("the organization changed: {before:?} -> {after:?}"));
        check(&mut failures, &label, audit_count(&db, action, &org.prn).await == audits, "an audit_log row was written".to_string());
        check(&mut failures, &label, outbox_count(&db, event, &org.prn).await == events, "an event_outbox row was written".to_string());
        client.archive_organization(authed(ArchiveOrganizationRequest { prn: org.prn.clone() }, &token)).await.expect("the correct prn must succeed");
        check(
            &mut failures,
            &label,
            audit_count(&db, action, &org.prn).await == audits + 1 && outbox_count(&db, event, &org.prn).await == events + 1,
            "the positive control wrote no row — the queries cannot see anything".to_string(),
        );

        // ---- restore (on a fresh org that this case archives first) ----
        let org = create_org(client, &token, &format!("forged-rs-{setting}"), "Restore Me").await;
        client.archive_organization(authed(ArchiveOrganizationRequest { prn: org.prn.clone() }, &token)).await.expect("setup archive");
        let action = Action::RestoreOrganization.as_wire();
        let event = EventType::OrganizationRestored.as_wire();
        let before = client
            .get_organization(authed(GetOrganizationRequest { prn: org.prn.clone() }, &token))
            .await
            .unwrap()
            .into_inner()
            .organization
            .expect("organization");
        let audits = audit_count(&db, action, &org.prn).await;
        let events = outbox_count(&db, event, &org.prn).await;
        let label = format!("{setting} RestoreOrganization");
        let err = client
            .restore_organization(authed(
                RestoreOrganizationRequest {
                    prn: with_org(&org.prn, &forged_slot),
                },
                &token,
            ))
            .await
            .unwrap_err();
        check(&mut failures, &label, err.code() == Code::InvalidArgument, format!("code was {:?}", err.code()));
        check(&mut failures, &label, reason(&err) == "prn-mismatch", format!("reason was {}", reason(&err)));
        let after = client
            .get_organization(authed(GetOrganizationRequest { prn: org.prn.clone() }, &token))
            .await
            .unwrap()
            .into_inner()
            .organization
            .expect("organization");
        check(&mut failures, &label, after == before, format!("the organization changed: {before:?} -> {after:?}"));
        check(&mut failures, &label, audit_count(&db, action, &org.prn).await == audits, "an audit_log row was written".to_string());
        check(&mut failures, &label, outbox_count(&db, event, &org.prn).await == events, "an event_outbox row was written".to_string());
        client.restore_organization(authed(RestoreOrganizationRequest { prn: org.prn.clone() }, &token)).await.expect("the correct prn must succeed");
        check(
            &mut failures,
            &label,
            audit_count(&db, action, &org.prn).await == audits + 1 && outbox_count(&db, event, &org.prn).await == events + 1,
            "the positive control wrote no row — the queries cannot see anything".to_string(),
        );
    }

    on_server.abort();
    off_server.abort();
    assert!(failures.is_empty(), "forged-prn organization cases failed:\n{}", failures.join("\n"));
}
```

- [ ] **Step 2: Run the test and record that every case fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs
env -u CI PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test grpc_tenancy \
  -E 'test(a_forged_prn_never_writes_an_organization)' 2>&1 | tail -40
```

Expected: FAIL. The printed list must hold all SIX labels (`enforce=on` and `enforce=off`, times
rename, archive and restore), each with "the organization changed" or a row-count failure. Copy
the list into the commit message body.

If a case reports `slug-conflict` or `nothing-to-rename` instead, a slug is not unique in the
test — fix the slug, do not weaken the assertion.

- [ ] **Step 3: Commit the failing test**

```bash
cd .. && cargo fmt --manifest-path rs/Cargo.toml
git add rs/crates/services/paigasus-iam/tests/grpc_tenancy.rs
git commit -m "test(rs): a forged prn must not write an organization (SMA-643)" \
  -m "Fails against the current handlers: all six cases commit the write before the check." \
  -m "Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: T1 for teams (6 cases)

**Files:**
- Modify: `rs/crates/services/paigasus-iam/tests/grpc_tenancy.rs`

**Interfaces:**
- Consumes: every helper from Task 1.
- Produces: nothing that a later task uses.

- [ ] **Step 1: Write the failing test**

Append to `tests/grpc_tenancy.rs`. The shape matches Task 2 with three differences: each case
creates its own parent organization first; the forged shapes are a WRONG organization UUID and an
EMPTY organization slot; the requests and the counters are the team ones.

```rust
/// T1 for teams (spec § 5.2). Forged shapes for this node kind: an organization slot holding a
/// uuid that no organization has, and an EMPTY organization slot.
#[tokio::test]
async fn a_forged_prn_never_writes_a_team() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let (enforced, unenforced) = two_states(&db, &idp).await;
    let token = idp.bearer("forged-team", Some("forged-team@example.com"), "paigasus", 3600);
    support::provision_platform_admin(&enforced, &token).await;
    let (on_addr, on_server) = spawn_tenancy_server(enforced).await;
    let (off_addr, off_server) = spawn_tenancy_server(unenforced).await;
    let mut on = connect(on_addr).await;
    let mut off = connect(off_addr).await;

    let mut failures: Vec<String> = Vec::new();
    let absent_org = Uuid::from_u128(0x0f02).as_hyphenated().to_string();

    for (setting, client) in [("enforce=on", &mut on), ("enforce=off", &mut off)] {
        let read = |client: &mut TenancyServiceClient<Channel>, prn: String, token: String| async move {
            client.get_team(authed(GetTeamRequest { prn }, &token)).await.unwrap().into_inner().team.expect("team")
        };

        // ---- rename (forged shape: a wrong organization uuid) ----
        let org = create_org(client, &token, &format!("t-rn-{setting}"), "Team Parent").await;
        let team = create_team(client, &token, &org.prn, &format!("t-rn-{setting}")).await;
        let action = Action::RenameTeam.as_wire();
        let event = EventType::TeamRenamed.as_wire();
        let before = read(client, team.prn.clone(), token.clone()).await;
        let audits = audit_count(&db, action, &team.prn).await;
        let events = outbox_count(&db, event, &team.prn).await;
        let request = |prn: String| RenameTeamRequest {
            prn,
            new_slug: Some(format!("t-rn-{setting}-renamed")),
            new_name: Some("Renamed".to_string()),
        };
        let label = format!("{setting} RenameTeam");
        let err = client.rename_team(authed(request(with_org(&team.prn, &absent_org)), &token)).await.unwrap_err();
        check(&mut failures, &label, err.code() == Code::InvalidArgument, format!("code was {:?}", err.code()));
        check(&mut failures, &label, reason(&err) == "prn-mismatch", format!("reason was {}", reason(&err)));
        let after = read(client, team.prn.clone(), token.clone()).await;
        check(&mut failures, &label, after == before, format!("the team changed: {before:?} -> {after:?}"));
        check(&mut failures, &label, audit_count(&db, action, &team.prn).await == audits, "an audit_log row was written".to_string());
        check(&mut failures, &label, outbox_count(&db, event, &team.prn).await == events, "an event_outbox row was written".to_string());
        client.rename_team(authed(request(team.prn.clone()), &token)).await.expect("the correct prn must succeed");
        check(
            &mut failures,
            &label,
            audit_count(&db, action, &team.prn).await == audits + 1 && outbox_count(&db, event, &team.prn).await == events + 1,
            "the positive control wrote no row — the queries cannot see anything".to_string(),
        );

        // ---- archive (forged shape: an EMPTY organization slot) ----
        let org = create_org(client, &token, &format!("t-ar-{setting}"), "Team Parent").await;
        let team = create_team(client, &token, &org.prn, &format!("t-ar-{setting}")).await;
        let action = Action::ArchiveTeam.as_wire();
        let event = EventType::TeamArchived.as_wire();
        let before = read(client, team.prn.clone(), token.clone()).await;
        let audits = audit_count(&db, action, &team.prn).await;
        let events = outbox_count(&db, event, &team.prn).await;
        let label = format!("{setting} ArchiveTeam");
        let err = client
            .archive_team(authed(
                ArchiveTeamRequest {
                    prn: with_org(&team.prn, ""),
                },
                &token,
            ))
            .await
            .unwrap_err();
        check(&mut failures, &label, err.code() == Code::InvalidArgument, format!("code was {:?}", err.code()));
        check(&mut failures, &label, reason(&err) == "prn-mismatch", format!("reason was {}", reason(&err)));
        let after = read(client, team.prn.clone(), token.clone()).await;
        check(&mut failures, &label, after == before, format!("the team changed: {before:?} -> {after:?}"));
        check(&mut failures, &label, audit_count(&db, action, &team.prn).await == audits, "an audit_log row was written".to_string());
        check(&mut failures, &label, outbox_count(&db, event, &team.prn).await == events, "an event_outbox row was written".to_string());
        client.archive_team(authed(ArchiveTeamRequest { prn: team.prn.clone() }, &token)).await.expect("the correct prn must succeed");
        check(
            &mut failures,
            &label,
            audit_count(&db, action, &team.prn).await == audits + 1 && outbox_count(&db, event, &team.prn).await == events + 1,
            "the positive control wrote no row — the queries cannot see anything".to_string(),
        );

        // ---- restore (forged shape: a wrong organization uuid) ----
        let org = create_org(client, &token, &format!("t-rs-{setting}"), "Team Parent").await;
        let team = create_team(client, &token, &org.prn, &format!("t-rs-{setting}")).await;
        client.archive_team(authed(ArchiveTeamRequest { prn: team.prn.clone() }, &token)).await.expect("setup archive");
        let action = Action::RestoreTeam.as_wire();
        let event = EventType::TeamRestored.as_wire();
        let before = read(client, team.prn.clone(), token.clone()).await;
        let audits = audit_count(&db, action, &team.prn).await;
        let events = outbox_count(&db, event, &team.prn).await;
        let label = format!("{setting} RestoreTeam");
        let err = client
            .restore_team(authed(
                RestoreTeamRequest {
                    prn: with_org(&team.prn, &absent_org),
                },
                &token,
            ))
            .await
            .unwrap_err();
        check(&mut failures, &label, err.code() == Code::InvalidArgument, format!("code was {:?}", err.code()));
        check(&mut failures, &label, reason(&err) == "prn-mismatch", format!("reason was {}", reason(&err)));
        let after = read(client, team.prn.clone(), token.clone()).await;
        check(&mut failures, &label, after == before, format!("the team changed: {before:?} -> {after:?}"));
        check(&mut failures, &label, audit_count(&db, action, &team.prn).await == audits, "an audit_log row was written".to_string());
        check(&mut failures, &label, outbox_count(&db, event, &team.prn).await == events, "an event_outbox row was written".to_string());
        client.restore_team(authed(RestoreTeamRequest { prn: team.prn.clone() }, &token)).await.expect("the correct prn must succeed");
        check(
            &mut failures,
            &label,
            audit_count(&db, action, &team.prn).await == audits + 1 && outbox_count(&db, event, &team.prn).await == events + 1,
            "the positive control wrote no row — the queries cannot see anything".to_string(),
        );
    }

    on_server.abort();
    off_server.abort();
    assert!(failures.is_empty(), "forged-prn team cases failed:\n{}", failures.join("\n"));
}
```

If the `read` closure does not borrow-check (it takes `&mut` on a captured client), replace each
call with the inline `client.get_team(authed(GetTeamRequest { prn: team.prn.clone() }, &token))`
chain that Task 2 uses for organizations. Do NOT change what is asserted.

- [ ] **Step 2: Run the test and record that every case fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs
env -u CI PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test grpc_tenancy \
  -E 'test(a_forged_prn_never_writes_a_team)' 2>&1 | tail -40
```

Expected: FAIL with all six team labels listed.

Note: a team whose organization slot is EMPTY is refused by `TeamId::from_prn`
(`paigasus-iam-core/src/tenancy.rs:377`), but `convert::node_uuid` does not build a `TeamId` — it
only checks the service and the resource type (`convert.rs:164-170`). If this case answers
`invalid-prn` instead of `prn-mismatch`, `node_uuid` refuses the shape before the comparison.
Record that, and change that one case to the wrong-organization-uuid shape.

- [ ] **Step 3: Commit the failing test**

```bash
cd .. && cargo fmt --manifest-path rs/Cargo.toml
git add rs/crates/services/paigasus-iam/tests/grpc_tenancy.rs
git commit -m "test(rs): a forged prn must not write a team (SMA-643)" \
  -m "Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: T1 for projects (6 cases)

**Files:**
- Modify: `rs/crates/services/paigasus-iam/tests/grpc_tenancy.rs`

**Interfaces:**
- Consumes: every helper from Task 1.
- Produces: nothing that a later task uses.

- [ ] **Step 1: Write the failing test**

Append to `tests/grpc_tenancy.rs`. Same shape as Task 3, with a project under a team under an
organization, `GetProjectRequest`/`RenameProjectRequest`/`ArchiveProjectRequest`/
`RestoreProjectRequest`, the `Action::…Project` actions and the `EventType::Project…` event
types. Forged shapes: a wrong organization uuid (rename, restore) and a non-empty region
(archive).

```rust
/// T1 for projects (spec § 5.2). Forged shapes for this node kind: a wrong organization uuid,
/// and a non-empty region.
#[tokio::test]
async fn a_forged_prn_never_writes_a_project() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let (enforced, unenforced) = two_states(&db, &idp).await;
    let token = idp.bearer("forged-project", Some("forged-project@example.com"), "paigasus", 3600);
    support::provision_platform_admin(&enforced, &token).await;
    let (on_addr, on_server) = spawn_tenancy_server(enforced).await;
    let (off_addr, off_server) = spawn_tenancy_server(unenforced).await;
    let mut on = connect(on_addr).await;
    let mut off = connect(off_addr).await;

    let mut failures: Vec<String> = Vec::new();
    let absent_org = Uuid::from_u128(0x0f03).as_hyphenated().to_string();

    for (setting, client) in [("enforce=on", &mut on), ("enforce=off", &mut off)] {
        for (verb, forged) in [("rename", "org"), ("archive", "region"), ("restore", "org")] {
            let org = create_org(client, &token, &format!("p-{verb}-{setting}"), "Project Parent").await;
            let team = create_team(client, &token, &org.prn, &format!("p-{verb}-{setting}")).await;
            let project = create_project(client, &token, &team.prn, &format!("p-{verb}-{setting}")).await;
            if verb == "restore" {
                client
                    .archive_project(authed(ArchiveProjectRequest { prn: project.prn.clone() }, &token))
                    .await
                    .expect("setup archive");
            }
            let (action, event) = match verb {
                "rename" => (Action::RenameProject.as_wire(), EventType::ProjectRenamed.as_wire()),
                "archive" => (Action::ArchiveProject.as_wire(), EventType::ProjectArchived.as_wire()),
                _ => (Action::RestoreProject.as_wire(), EventType::ProjectRestored.as_wire()),
            };
            let forged_prn = if forged == "org" {
                with_org(&project.prn, &absent_org)
            } else {
                with_region(&project.prn, "eu-west-1")
            };
            let label = format!("{setting} {verb}Project");
            let before = client
                .get_project(authed(GetProjectRequest { prn: project.prn.clone() }, &token))
                .await
                .unwrap()
                .into_inner()
                .project
                .expect("project");
            let audits = audit_count(&db, action, &project.prn).await;
            let events = outbox_count(&db, event, &project.prn).await;

            let err = match verb {
                "rename" => client
                    .rename_project(authed(
                        RenameProjectRequest {
                            prn: forged_prn,
                            new_slug: Some(format!("p-{verb}-{setting}-renamed")),
                            new_name: Some("Renamed".to_string()),
                        },
                        &token,
                    ))
                    .await
                    .unwrap_err(),
                "archive" => client.archive_project(authed(ArchiveProjectRequest { prn: forged_prn }, &token)).await.unwrap_err(),
                _ => client.restore_project(authed(RestoreProjectRequest { prn: forged_prn }, &token)).await.unwrap_err(),
            };
            check(&mut failures, &label, err.code() == Code::InvalidArgument, format!("code was {:?}", err.code()));
            check(&mut failures, &label, reason(&err) == "prn-mismatch", format!("reason was {}", reason(&err)));
            let after = client
                .get_project(authed(GetProjectRequest { prn: project.prn.clone() }, &token))
                .await
                .unwrap()
                .into_inner()
                .project
                .expect("project");
            check(&mut failures, &label, after == before, format!("the project changed: {before:?} -> {after:?}"));
            check(&mut failures, &label, audit_count(&db, action, &project.prn).await == audits, "an audit_log row was written".to_string());
            check(&mut failures, &label, outbox_count(&db, event, &project.prn).await == events, "an event_outbox row was written".to_string());

            // positive control with the correct prn.
            match verb {
                "rename" => {
                    client
                        .rename_project(authed(
                            RenameProjectRequest {
                                prn: project.prn.clone(),
                                new_slug: Some(format!("p-{verb}-{setting}-renamed")),
                                new_name: Some("Renamed".to_string()),
                            },
                            &token,
                        ))
                        .await
                        .expect("the correct prn must succeed");
                }
                "archive" => {
                    client.archive_project(authed(ArchiveProjectRequest { prn: project.prn.clone() }, &token)).await.expect("the correct prn must succeed");
                }
                _ => {
                    client.restore_project(authed(RestoreProjectRequest { prn: project.prn.clone() }, &token)).await.expect("the correct prn must succeed");
                }
            }
            check(
                &mut failures,
                &label,
                audit_count(&db, action, &project.prn).await == audits + 1 && outbox_count(&db, event, &project.prn).await == events + 1,
                "the positive control wrote no row — the queries cannot see anything".to_string(),
            );
        }
    }

    on_server.abort();
    off_server.abort();
    assert!(failures.is_empty(), "forged-prn project cases failed:\n{}", failures.join("\n"));
}
```

Add `GetProjectRequest` to the import list if Task 1 did not.

- [ ] **Step 2: Run the test and record that every case fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs
env -u CI PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test grpc_tenancy \
  -E 'test(a_forged_prn_never_writes_a_project)' 2>&1 | tail -40
```

Expected: FAIL with all six project labels listed.

- [ ] **Step 3: Commit the failing test**

```bash
cd .. && cargo fmt --manifest-path rs/Cargo.toml
git add rs/crates/services/paigasus-iam/tests/grpc_tenancy.rs
git commit -m "test(rs): a forged prn must not write a project (SMA-643)" \
  -m "Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: T3 — authorization comes before the comparison

**Files:**
- Modify: `rs/crates/services/paigasus-iam/tests/grpc_tenancy.rs`

**Interfaces:**
- Consumes: every helper from Task 1.
- Produces: nothing that a later task uses.

- [ ] **Step 1: Write the test**

The property: a caller with no grant gets `permission-denied` for BOTH a forged and a correct
PRN. If the comparison came first, the two answers would differ and the caller could find the
owning organization by trying UUIDs.

Append to `tests/grpc_tenancy.rs`:

```rust
/// T3 (spec § 5.2, decision D1): with `enforce_tenancy` on, a principal with NO grant must get
/// `permission-denied` for a forged prn and for the correct prn alike. A different answer for
/// the two would tell the caller which organization owns the node. This test passes before the
/// SMA-643 fix as well; mutation m3 (compare before authorize) must break it.
#[tokio::test]
async fn an_ungranted_caller_cannot_tell_a_forged_prn_from_a_correct_one() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let state = AppState::new(db.clone(), &support::test_config(&idp)).await.unwrap();
    let admin = idp.bearer("t3-admin", Some("t3-admin@example.com"), "paigasus", 3600);
    support::provision_platform_admin(&state, &admin).await;
    // A second principal, JIT-provisioned but never granted anything.
    let stranger = idp.bearer("t3-stranger", Some("t3-stranger@example.com"), "paigasus", 3600);
    support::provision(&state, &stranger).await;
    let (addr, server) = spawn_tenancy_server(state).await;
    let mut client = connect(addr).await;

    let org = create_org(&mut client, &admin, "t3-org", "T3 Org").await;
    let team = create_team(&mut client, &admin, &org.prn, "t3-team").await;
    let project = create_project(&mut client, &admin, &team.prn, "t3-project").await;
    let archived_org = create_org(&mut client, &admin, "t3-org-archived", "T3 Archived").await;
    client.archive_organization(authed(ArchiveOrganizationRequest { prn: archived_org.prn.clone() }, &admin)).await.unwrap();

    let mut failures: Vec<String> = Vec::new();
    let absent_org = Uuid::from_u128(0x0f04).as_hyphenated().to_string();
    let forged_org = with_org(&org.prn, &absent_org);
    let forged_archived = with_org(&archived_org.prn, &absent_org);
    let forged_team = with_org(&team.prn, &absent_org);
    let forged_project = with_org(&project.prn, &absent_org);

    // Each row: a label, the correct prn, the forged prn. Both must answer permission-denied.
    let mut expect_denied = |failures: &mut Vec<String>, label: &str, err: tonic::Status| {
        check(failures, label, err.code() == Code::PermissionDenied, format!("code was {:?}", err.code()));
        check(failures, label, reason(&err) == "forbidden", format!("reason was {}", reason(&err)));
    };

    for (label, prn) in [
        ("RenameOrganization correct", org.prn.clone()),
        ("RenameOrganization forged", forged_org.clone()),
    ] {
        let err = client
            .rename_organization(authed(
                RenameOrganizationRequest {
                    prn,
                    new_slug: Some("t3-stolen".to_string()),
                    new_name: None,
                },
                &stranger,
            ))
            .await
            .unwrap_err();
        expect_denied(&mut failures, label, err);
    }
    for (label, prn) in [("ArchiveOrganization correct", org.prn.clone()), ("ArchiveOrganization forged", forged_org.clone())] {
        let err = client.archive_organization(authed(ArchiveOrganizationRequest { prn }, &stranger)).await.unwrap_err();
        expect_denied(&mut failures, label, err);
    }
    for (label, prn) in [
        ("RestoreOrganization correct", archived_org.prn.clone()),
        ("RestoreOrganization forged", forged_archived.clone()),
    ] {
        let err = client.restore_organization(authed(RestoreOrganizationRequest { prn }, &stranger)).await.unwrap_err();
        expect_denied(&mut failures, label, err);
    }
    for (label, prn) in [("RenameTeam correct", team.prn.clone()), ("RenameTeam forged", forged_team.clone())] {
        let err = client
            .rename_team(authed(
                RenameTeamRequest {
                    prn,
                    new_slug: Some("t3-stolen".to_string()),
                    new_name: None,
                },
                &stranger,
            ))
            .await
            .unwrap_err();
        expect_denied(&mut failures, label, err);
    }
    for (label, prn) in [("ArchiveTeam correct", team.prn.clone()), ("ArchiveTeam forged", forged_team.clone())] {
        let err = client.archive_team(authed(ArchiveTeamRequest { prn }, &stranger)).await.unwrap_err();
        expect_denied(&mut failures, label, err);
    }
    for (label, prn) in [("RestoreTeam correct", team.prn.clone()), ("RestoreTeam forged", forged_team.clone())] {
        let err = client.restore_team(authed(RestoreTeamRequest { prn }, &stranger)).await.unwrap_err();
        expect_denied(&mut failures, label, err);
    }
    for (label, prn) in [("RenameProject correct", project.prn.clone()), ("RenameProject forged", forged_project.clone())] {
        let err = client
            .rename_project(authed(
                RenameProjectRequest {
                    prn,
                    new_slug: Some("t3-stolen".to_string()),
                    new_name: None,
                },
                &stranger,
            ))
            .await
            .unwrap_err();
        expect_denied(&mut failures, label, err);
    }
    for (label, prn) in [("ArchiveProject correct", project.prn.clone()), ("ArchiveProject forged", forged_project.clone())] {
        let err = client.archive_project(authed(ArchiveProjectRequest { prn }, &stranger)).await.unwrap_err();
        expect_denied(&mut failures, label, err);
    }
    for (label, prn) in [("RestoreProject correct", project.prn.clone()), ("RestoreProject forged", forged_project.clone())] {
        let err = client.restore_project(authed(RestoreProjectRequest { prn }, &stranger)).await.unwrap_err();
        expect_denied(&mut failures, label, err);
    }

    // Nothing was written by any of the refused calls.
    check(
        &mut failures,
        "no write",
        audit_count(&db, Action::RenameTeam.as_wire(), &team.prn).await == 0 && outbox_count(&db, EventType::TeamRenamed.as_wire(), &team.prn).await == 0,
        "a denied call wrote a row".to_string(),
    );

    server.abort();
    assert!(failures.is_empty(), "authorize-before-compare cases failed:\n{}", failures.join("\n"));
}
```

If the denied reason is not `"forbidden"`, read the real value from
`src/application/error.rs`'s `as_wire_reason` mapping and use that. Do not weaken the check to
"any reason".

- [ ] **Step 2: Run the test**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs
env -u CI PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test grpc_tenancy \
  -E 'test(an_ungranted_caller_cannot_tell_a_forged_prn_from_a_correct_one)' 2>&1 | tail -30
```

Expected: PASS against the unchanged handlers (they authorize first already). If it FAILS, the
current order is not what the spec says — stop and report before you change any handler.

- [ ] **Step 3: Commit**

```bash
cd .. && cargo fmt --manifest-path rs/Cargo.toml
git add rs/crates/services/paigasus-iam/tests/grpc_tenancy.rs
git commit -m "test(rs): an ungranted caller gets the same answer for a forged prn (SMA-643)" \
  -m "Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 6: The fix — three helpers, nine handlers, no post-write comparison

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/grpc/tenancy.rs`
  - add the helpers after `resolve_node` (`:116-122`)
  - `rename_organization` (`:205-236`), `archive_organization` (`:238-263`),
    `restore_organization` (`:265-290`)
  - `rename_team` (`:355-385`), `archive_team` (`:387-410`), `restore_team` (`:412-435`)
  - `rename_project` (`:516-546`), `archive_project` (`:548-571`), `restore_project` (`:573-596`)
  - (Line numbers are from the pre-fix file. Find each handler by name.)

**Interfaces:**
- Consumes: `AppState.{orgs,teams,projects,authorize,enforce_tenancy}`, `Action`,
  `TenancyError::PrnMismatch`, `convert::{node_uuid, status_to_grpc}`.
- Produces:
  - `async fn load_org_for_write(state: &AppState, actor: &Prn, action: Action, id: Uuid,
    canonical: &str, rpc: &str) -> Result<(), Status>`
  - `async fn load_team_for_write(...) -> Result<(), Status>`
  - `async fn load_project_for_write(...) -> Result<(), Status>`

- [ ] **Step 1: Add the three helpers**

Add after `resolve_node` in `src/adapters/grpc/tenancy.rs`. Import the node types the signatures
need: `use paigasus_iam_core::{Organization, Project, Team};` (extend the existing
`paigasus_iam_core` import line; `NodeView` is already imported).

```rust
/// Loads the stored organization, authorizes against its OWN prn, and refuses a request prn
/// that does not match the stored canonical one — all BEFORE the caller writes (SMA-643).
///
/// Order matters twice over. The load comes first because the request prn's organization slot
/// is caller input; authorizing against the stored prn is what keeps a forged slot from
/// choosing the resource. The comparison comes AFTER the authorization, so a caller with no
/// grant gets `permission-denied` for a forged prn and for the correct prn alike — otherwise
/// the difference between the two answers would tell the caller which organization owns the
/// node.
///
/// The comparison is sound outside the write transaction because a node's stored prn never
/// changes: the `prn` column is written once, at insert, and no repository method, service or
/// migration moves a node to a different parent. A future "move" feature breaks that invariant
/// and must revisit this helper.
async fn load_org_for_write(state: &AppState, actor: &Prn, action: Action, id: Uuid, canonical: &str, rpc: &str) -> Result<(), Status> {
    let view = state.orgs.get(id).await.map_err(convert::status_to_grpc)?;
    if state.enforce_tenancy {
        state.authorize.check(actor, action, view.node.id.prn()).await.map_err(convert::status_to_grpc)?;
    }
    let stored = view.node.id.canonical();
    if stored != canonical {
        warn_prn_mismatch(actor, canonical, &stored, rpc);
        return Err(convert::status_to_grpc(TenancyError::PrnMismatch));
    }
    Ok(())
}

/// The team twin of [`load_org_for_write`] — same order, same reasons.
async fn load_team_for_write(state: &AppState, actor: &Prn, action: Action, id: Uuid, canonical: &str, rpc: &str) -> Result<(), Status> {
    let view = state.teams.get(id).await.map_err(convert::status_to_grpc)?;
    if state.enforce_tenancy {
        state.authorize.check(actor, action, view.node.id.prn()).await.map_err(convert::status_to_grpc)?;
    }
    let stored = view.node.id.canonical();
    if stored != canonical {
        warn_prn_mismatch(actor, canonical, &stored, rpc);
        return Err(convert::status_to_grpc(TenancyError::PrnMismatch));
    }
    Ok(())
}

/// The project twin of [`load_org_for_write`] — same order, same reasons.
async fn load_project_for_write(state: &AppState, actor: &Prn, action: Action, id: Uuid, canonical: &str, rpc: &str) -> Result<(), Status> {
    let view = state.projects.get(id).await.map_err(convert::status_to_grpc)?;
    if state.enforce_tenancy {
        state.authorize.check(actor, action, view.node.id.prn()).await.map_err(convert::status_to_grpc)?;
    }
    let stored = view.node.id.canonical();
    if stored != canonical {
        warn_prn_mismatch(actor, canonical, &stored, rpc);
        return Err(convert::status_to_grpc(TenancyError::PrnMismatch));
    }
    Ok(())
}

/// One warning line per refused write (SMA-643 D4). After the check moved before the write, a
/// refused attempt leaves NO audit row and no denial row, and the attempt is a tampering
/// signal, so the log line is the only trace.
fn warn_prn_mismatch(actor: &Prn, requested: &str, stored: &str, rpc: &str) {
    tracing::warn!(rpc = %rpc, actor = %actor.canonical(), requested_prn = %requested, stored_prn = %stored, "refused a tenancy write: the request prn does not match the stored node");
}
```

- [ ] **Step 2: Rewire the nine handlers**

In each handler, replace the `if self.state.enforce_tenancy { … }` block with the helper call,
and DELETE the `if view.node.id.canonical() != canonical { … }` block that follows the service
call. `rename_organization` becomes:

```rust
    async fn rename_organization(&self, request: Request<RenameOrganizationRequest>) -> Result<Response<RenameOrganizationResponse>, Status> {
        let started = Instant::now();
        let result: Result<Response<RenameOrganizationResponse>, Status> = async {
            let actor_principal = actor_context(&request)?.principal_id;
            let actor = actor_principal.prn().clone();
            let req = request.into_inner();
            let (id, canonical) = convert::node_uuid(&req.prn, "organization")?;
            load_org_for_write(&self.state, &actor, Action::RenameOrganization, id, &canonical, "RenameOrganization").await?;
            let view = self
                .state
                .orgs
                .rename(id, req.new_slug.as_deref(), req.new_name.as_deref(), &actor_principal)
                .await
                .map_err(convert::status_to_grpc)?;
            Ok(Response::new(RenameOrganizationResponse {
                organization: Some(convert::to_proto_org(&view)),
            }))
        }
        .await;
        record_grpc("Tenancy", "RenameOrganization", started, &result);
        result
    }
```

Apply the same three edits to the other eight handlers, each with its own helper, `Action` and
RPC name:

| Handler | Helper | Action | RPC name string |
|---|---|---|---|
| `archive_organization` | `load_org_for_write` | `Action::ArchiveOrganization` | `"ArchiveOrganization"` |
| `restore_organization` | `load_org_for_write` | `Action::RestoreOrganization` | `"RestoreOrganization"` |
| `rename_team` | `load_team_for_write` | `Action::RenameTeam` | `"RenameTeam"` |
| `archive_team` | `load_team_for_write` | `Action::ArchiveTeam` | `"ArchiveTeam"` |
| `restore_team` | `load_team_for_write` | `Action::RestoreTeam` | `"RestoreTeam"` |
| `rename_project` | `load_project_for_write` | `Action::RenameProject` | `"RenameProject"` |
| `archive_project` | `load_project_for_write` | `Action::ArchiveProject` | `"ArchiveProject"` |
| `restore_project` | `load_project_for_write` | `Action::RestoreProject` | `"RestoreProject"` |

The six Archive/Restore handlers read the PRN from `request.get_ref().prn` and never call
`request.into_inner()`; keep that as it is. Their helper call goes exactly where the
`if self.state.enforce_tenancy { … }` block was.

The helpers return `Result<(), Status>`. No handler needs the loaded node: the service call
returns the post-write view that the response carries. Call the helper as a statement, with
`?`. Do NOT bind the result; clippy is set to `-D warnings` in this workspace and an unused
binding is a warning.

- [ ] **Step 3: Check that the handlers compile and the whole crate is clean**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo fmt && cargo clippy -p paigasus-iam --all-targets -- -D warnings
```

Expected: no warning. If clippy reports `canonical` as unused in a handler, that handler still
has its old post-write comparison — remove it.

- [ ] **Step 4: Run the five tests**

```bash
env -u CI PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test grpc_tenancy 2>&1 | tail -30
```

Expected: every test in the binary PASSES, the two pre-existing ones included
(`organization_lifecycle_over_grpc`, `team_membership_flow_over_grpc`).

- [ ] **Step 5: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/adapters/grpc/tenancy.rs
git commit -m "fix(rs): check prn-mismatch before a tenancy write commits (SMA-643)" \
  -m "The nine Rename/Archive/Restore gRPC handlers now load the node, authorize against its stored prn and compare the canonical prn BEFORE the service call. A forged organization slot no longer commits a write, an audit row, an outbox event or a generation bump. One warn line records the refused attempt." \
  -m "Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 7: Correct every text that describes the old order (D3)

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/grpc/tenancy.rs` (module doc `:6-12`,
  `:17-19`, `:26-29`)
- Modify: `rs/crates/services/paigasus-iam/src/adapters/grpc/convert.rs` (`:159-163`)
- Modify: `ts/apps/iam-console/app/(console)/orgs/node-ref.ts` (`:4-5`)
- Modify: `ts/apps/iam-console/app/(console)/orgs/[org]/teams/[team]/load.ts` (`:43-44`)

**Interfaces:** none. This task changes comments only.

- [ ] **Step 1: Rewrite the `tenancy.rs` module doc**

Replace the second paragraph (the one that begins "Every Get/Rename/Archive/Restore re-checks")
with:

```rust
//! Every Get/Rename/Archive/Restore compares the *stored* canonical PRN (`view.node.id
//! .canonical()`) with the request's parsed one — the forged-org-slot defense (brief rule 8,
//! mirroring the HTTP layer's semantics). A Get compares after its read. A Rename/Archive/
//! Restore compares BEFORE its write, in `load_{org,team,project}_for_write` (SMA-643): the
//! comparison used to run after the service call, so a forged organization slot committed the
//! write, the audit row and the outbox event and still answered `prn-mismatch`. The comparison
//! is sound outside the write transaction because a node's stored PRN never changes (the `prn`
//! column is written once, at insert, and nothing moves a node to a different parent).
//!
//! Creates and Lists do NOT compare their parent PRN at all: they take the parent's uuid and
//! discard the rest, so a forged parent organization slot is accepted without an error (the
//! write still goes to the real parent). That is SMA-645, not a property of this design.
```

- [ ] **Step 2: Correct the two other passages in the same file**

In the SMA-444 paragraph, replace "mirrors `adapters::http::{organizations,teams,projects,
memberships}`'s fetch-then-authorize-then-act posture exactly … so the two transports can never
diverge" with a sentence that names the limit:

```rust
//! mirrors `adapters::http::{organizations,teams,projects,memberships}`'s
//! fetch-then-authorize-then-act posture (the same action to resource map, spec §9.4). Under
//! the default `enforce_tenancy = true` the two transports answer alike. They differ only in
//! the test-only `enforce_tenancy = false` setting, where gRPC still loads the node (SMA-643)
//! and HTTP does not, so gRPC answers `not-found` for an unknown uuid where HTTP answers
//! `nothing-to-rename` or `invalid-slug` first.
```

In the last paragraph, replace "The existing forged-org-slot defense (this module's own
stored-canonical recheck, and `MembershipService::attach`'s own `PrnMismatch` detection) still
fires on the actual mutating call" with:

```rust
//! The existing forged-org-slot defense (this module's own stored-canonical check, and
//! `MembershipService::attach`'s own `PrnMismatch` detection) fires BEFORE the actual mutating
//! call;
```

- [ ] **Step 3: Correct `convert.rs`**

Open `src/adapters/grpc/convert.rs:159-163` and replace the phrase that says the canonical PRN is
compared "after the call" with "before the write for Rename/Archive/Restore, and after the read
for Get". Keep the rest of that doc comment.

- [ ] **Step 4: Correct the two TypeScript comments**

Both cite `tenancy.rs` line numbers that this change moves. Replace each line-number citation
with the handler or helper name, for example `tenancy.rs`'s `load_team_for_write`. Change the
comment text only — no code.

- [ ] **Step 5: Verify**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo fmt --check && cargo clippy -p paigasus-iam --all-targets -- -D warnings
cd ../ts && pnpm exec prettier --check "apps/iam-console/app/**/*.ts"
```

Expected: both clean. `ts:fmt` is its own CI gate, so the prettier check matters.

- [ ] **Step 6: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/adapters/grpc/tenancy.rs \
        rs/crates/services/paigasus-iam/src/adapters/grpc/convert.rs \
        "ts/apps/iam-console/app/(console)/orgs/node-ref.ts" \
        "ts/apps/iam-console/app/(console)/orgs/[org]/teams/[team]/load.ts"
git commit -m "docs(rs): describe the new prn-mismatch order (SMA-643)" \
  -m "Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 8: Mutation battery — prove the four tests bite

**Files:** none committed. Every edit in this task is undone before the task ends.

**Interfaces:** none.

Undo each mutation by editing the code back by hand. Do NOT use `git checkout --` — it would
also revert any uncommitted work.

- [ ] **Step 1: m1 — move the comparison inside the enforce block**

In all three helpers, move the `if stored != canonical { … }` block inside the
`if state.enforce_tenancy { … }` block. Run:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs
env -u CI PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test grpc_tenancy 2>&1 | tail -40
```

Expected: the three T1 tests FAIL, and every failure label starts with `enforce=off`. Record the
labels. Undo the edit.

- [ ] **Step 2: m2 — restore the old order in one handler**

In `rename_team` only, move the `load_team_for_write` call to AFTER the `teams.rename` call. Run
the same command.

Expected: `a_forged_prn_never_writes_a_team` FAILS with the two `RenameTeam` labels.
`an_ungranted_caller_cannot_tell_a_forged_prn_from_a_correct_one` fails too, because the helper
holds the authorization as well as the comparison: moving the call after `teams.rename` moves
the authorization after the write, so a denied caller's `RenameTeam` also writes. This was
MEASURED; the first version of this step expected the T1 failures alone. Record it. Undo the
edit.

- [ ] **Step 3: m3 — compare before authorize**

In `load_team_for_write` only, move the `if stored != canonical { … }` block ABOVE the
`if state.enforce_tenancy { … }` block. Run the same command.

Expected: `an_ungranted_caller_cannot_tell_a_forged_prn_from_a_correct_one` FAILS on the
`RenameTeam forged`, `ArchiveTeam forged` and `RestoreTeam forged` labels, which now answer
`prn-mismatch` instead of `permission-denied`. Record it. Undo the edit.

- [ ] **Step 4: m4 — compare the raw request string**

In `load_org_for_write` only, compare against the raw request PRN instead of the canonical one:
change the helper's parameter use so that it compares `stored != request_raw`. The simplest form
is to pass `&req.prn` at one call site in `rename_organization`. Run the same command.

Expected: `an_upper_case_uuid_in_a_correct_prn_still_renames` FAILS. Record it. Undo the edit.

- [ ] **Step 5: Confirm the code is back to the committed state**

```bash
git status --short
git diff --stat
```

Expected: no change to any tracked file. If a file still differs, finish undoing it by hand.

- [ ] **Step 6: Record the battery in the spec**

Append a short "Mutation battery result" subsection to
`docs/superpowers/specs/2026-09-17-sma-643-prn-mismatch-before-write-design.md` § 5.3 with the
four recorded outcomes, one line each, stating what failed and what did not. Then:

```bash
git add docs/superpowers/specs/2026-09-17-sma-643-prn-mismatch-before-write-design.md
git commit -m "docs(rs): record the SMA-643 mutation battery result" \
  -m "Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 9: Full verification before the pull request

**Files:** none.

**Interfaces:** none.

- [ ] **Step 1: The whole IAM test suite**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs
env -u CI cargo nextest run -p paigasus-iam --no-tests=pass 2>&1 | tail -20
```

Expected: PASS. Docker must run. A test that fails on every attempt is a real failure; one that
passes on a retry is reported FLAKY and is acceptable (`rs/.config/nextest.toml` holds the retry
budget).

- [ ] **Step 2: Format, lint and build the workspace**

```bash
cd rs && cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo build --workspace
```

Expected: all clean.

- [ ] **Step 3: The repository gates that this change can red**

```bash
cd .. && moon run repo:error-code-single-site repo:affected-smoke ts:fmt
```

Expected: all pass. Notes from CLAUDE.md, needed to read the result:
- `repo:affected-smoke` needs system `/bin/bash` 3.2 on this machine. A hang is the known
  here-string deadlock, not a gate failure.
- Do not run `repo:actionlint` locally: no local bash finishes it.
- A sub-3-second `affected-smoke` failure is the known abort. Capture the output BEFORE a
  re-run.

- [ ] **Step 4: Confirm the diff matches the plan**

```bash
git diff --stat origin/main...HEAD
```

Expected files: `src/adapters/grpc/tenancy.rs`, `src/adapters/grpc/convert.rs`,
`tests/grpc_tenancy.rs`, the two `ts/apps/iam-console` comment files, the spec and this plan.
Nothing else. No debug print, no commented-out code.

---

## Self-review

- **Spec coverage.** D1 → Task 6 Steps 1–2. D2 → Task 6 Step 2 (the deletion). D3 → Task 7.
  D4 → Task 6 Step 1 (`warn_prn_mismatch`). § 5.1 harness → Task 1. § 5.2 T1 → Tasks 2–4.
  T2 → Task 1 Step 2. T3 → Task 5. § 5.3 → Task 8. B1–B4 are asserted by T1's node comparison,
  its counters and its positive control. § 6 is out of scope by construction; SMA-645 and
  SMA-646 are named in the Task 7 doc text.
- **Placeholders.** None. Every step carries its command or its code.
- **Type consistency.** The helper names, their parameter order (`state, actor, action, id,
  canonical, rpc`) and their return types are the same in Task 6's interface block, its code and
  the mutation steps in Task 8. The test helper names in Task 1's "Produces" list match their
  uses in Tasks 2–5.
