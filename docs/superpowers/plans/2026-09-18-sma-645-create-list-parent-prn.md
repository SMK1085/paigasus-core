# SMA-645 Create/List parent-PRN check — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `CreateTeam`, `ListTeams`, `CreateProject` and `ListProjects` refuse a parent PRN whose canonical form differs from the parent's stored canonical PRN, answering `InvalidArgument` / `prn-mismatch`.

**Architecture:** SMA-643 already built the helper this needs. Rename `load_{org,team,project}_for_write` to `load_{org,team,project}_checked` (two of the four new callers are reads), then replace each handler's hand-rolled `if enforce_tenancy { get; authorize }` block with one helper call. The helper loads the parent unconditionally, authorizes only under `enforce_tenancy`, then compares. No application- or domain-layer change.

**Tech Stack:** Rust (edition 2024, rust-version 1.95), tonic gRPC, SeaORM, `cargo nextest` with the `iam` profile, Docker-backed integration tests.

**Spec:** `docs/superpowers/specs/2026-09-18-sma-645-create-list-parent-prn-design.md`

## Global Constraints

- Every source file opens with `// SPDX-License-Identifier: Apache-2.0`. Both files here already have it; do not add a second.
- **Never write the literal `"prn-mismatch"` in quotes anywhere under `rs/crates/services/paigasus-iam/src/`.** `ci/error-registry/check.py`'s `code_pattern` matches a registry code in quotes anywhere in a file, comments included, and `adapters/grpc/tenancy.rs` is not in its `MANIFEST`. In `src/` prose use backticks or name `TenancyError::PrnMismatch`. Test files under `tests/` are not scanned and may use the literal.
- The comparison order is fixed: **load → authorize (if `enforce_tenancy`) → compare**. Never compare before authorizing.
- For the two Lists, `convert::to_page` runs **after** the helper call, not before.
- **Every shell block below assumes these two exports.** They are stated once here rather than repeated, but they are not optional:

  ```bash
  export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"   # the Bash tool's PATH lacks the proto-managed CLIs
  export PAIGASUS_REQUIRE_DOCKER=1                          # turn a Docker-less skip into a panic
  ```

  The first is required by CLAUDE.md so `moon`/`uv`/`nextest` resolve to the repo-pinned versions. The second matters more than it looks: `support::start_migrated_postgres()` returns `None` when the daemon is unreachable and every test then returns early, **counted as passed**. A whole run can report green having asserted nothing — which would make the entire mutation battery in Task 8 meaningless, since a mutation that reds nothing is indistinguishable from a suite that never ran. Never read a green without it.
- Do not hand-edit `.github/CODEOWNERS` (Moon-generated).
- Commit messages are conventional with a workspace scope, e.g. `fix(rs): …`, and end with the `Co-Authored-By` trailer used by the other commits on this branch.
- A `#NNN` or `token: value` line in a commit BODY fails `footer-leading-blank`. Keep the body prose-only.

---

### Task 1: Rename the three helpers

Mechanical rename with no behaviour change. It lands first so the four handler tasks each touch one call site rather than renaming under themselves.

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/grpc/tenancy.rs` (helpers at `:147`, `:161`, `:175`, `:191`; call sites throughout the `impl TenancyService` block)

**Interfaces:**
- Consumes: nothing.
- Produces: `load_org_checked`, `load_team_checked`, `load_project_checked` — each
  `async fn(state: &AppState, actor: &Prn, action: Action, id: Uuid, canonical: &str, rpc: &str) -> Result<(), Status>`. Signatures are unchanged from the `_for_write` versions; only the names change.

- [ ] **Step 1: Confirm the suite is green before touching anything**

```bash
cd rs && cargo nextest run -p paigasus-iam --profile iam --no-tests=pass -E 'binary(grpc_tenancy)'
```

Expected: PASS. If Docker is unreachable every test skips — that is not a green. Re-run with `PAIGASUS_REQUIRE_DOCKER=1` to confirm the suite actually executed.

- [ ] **Step 2: Rename the three functions and their nine call sites**

```bash
cd rs/crates/services/paigasus-iam/src/adapters/grpc
sed -i '' 's/load_org_for_write/load_org_checked/g; s/load_team_for_write/load_team_checked/g; s/load_project_for_write/load_project_checked/g' tenancy.rs
grep -c "_for_write" tenancy.rs
```

Expected: the final `grep -c` prints `0`.

- [ ] **Step 3: Reword the warning, which now also fires on reads**

In `warn_prn_mismatch` (`tenancy.rs:191`), change the message string only:

```rust
tracing::warn!(rpc = %rpc, actor = %actor.canonical(), requested_prn = %requested, stored_prn = %stored, "refused a tenancy request: the request prn does not match the stored node");
```

Leave the structured fields and the level alone. The spec (§4.1) decided `warn` stays for the Lists.

- [ ] **Step 4: Update the two doc comments that name the old identifiers**

`load_team_checked`'s doc reads "The team twin of [`load_org_for_write`]" and `load_project_checked`'s reads the same. The `sed` in Step 2 already rewrote both, because the names appear inside the doc links. Verify:

```bash
grep -n "load_org_checked\|load_team_checked\|load_project_checked" rs/crates/services/paigasus-iam/src/adapters/grpc/tenancy.rs | head -20
```

Expected: three definitions, two intra-doc links, nine call sites.

- [ ] **Step 5: Build and re-run the suite**

```bash
cd rs && cargo clippy -p paigasus-iam --all-targets -- -D warnings && cargo fmt --check
cargo nextest run -p paigasus-iam --profile iam --no-tests=pass -E 'binary(grpc_tenancy)'
```

Expected: clippy clean, fmt clean, tests PASS. A rename cannot change behaviour; a failure here means the sed hit something it should not have.

- [ ] **Step 6: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/adapters/grpc/tenancy.rs
git commit -m "refactor(rs): rename the tenancy prn helpers for read callers (SMA-645)

Two of the four handlers about to call these helpers are reads, so the
_for_write suffix would be false at those call sites. The warning text
loses the word write for the same reason.

No behaviour change.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: The organization-parent handlers — `CreateTeam` and `ListTeams`

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/grpc/tenancy.rs` (`create_team` at `:337`, `list_teams` at `:384`)
- Test: `rs/crates/services/paigasus-iam/tests/grpc_tenancy.rs`

**Interfaces:**
- Consumes: `load_org_checked` from Task 1.
- Produces: two test functions, `a_forged_org_parent_never_creates_a_team` and `a_forged_org_parent_never_lists_teams`; and two test helpers, `audit_total(&DatabaseConnection) -> u64` and `outbox_total(&DatabaseConnection) -> u64`, which Task 3 reuses.

- [ ] **Step 1: Add the two unfiltered count helpers**

The existing `audit_count`/`outbox_count` filter by resource PRN. A refused create produces no node, so there is no resource PRN to filter by, and filtering by the PARENT's PRN returns zero with the fix present or absent — a vacuous assertion (spec §6 T1). Count unfiltered instead, exactly as `an_ungranted_caller_cannot_tell_a_forged_prn_from_a_correct_one` already does.

Add below `outbox_count` in `tests/grpc_tenancy.rs`:

```rust
/// Total `audit_log` rows, unfiltered. A refused create writes no node, so there is no resource
/// PRN to filter on — and filtering by the PARENT's PRN is vacuous, because a CreateTeam audit
/// row carries the NEW team's PRN (`application/teams.rs`, `team_entry`). Each test owns its
/// own Postgres container, so nothing else writes concurrently and a total is safe. Mirrors
/// `an_ungranted_caller_cannot_tell_a_forged_prn_from_a_correct_one`'s snapshot.
async fn audit_total(db: &DatabaseConnection) -> u64 {
    audit_log::Entity::find().count(db).await.expect("count audit_log total")
}

/// The `event_outbox` twin of [`audit_total`].
async fn outbox_total(db: &DatabaseConnection) -> u64 {
    event_outbox::Entity::find().count(db).await.expect("count event_outbox total")
}
```

- [ ] **Step 2: Write the failing test for `CreateTeam`**

Append to `tests/grpc_tenancy.rs`:

```rust
/// SMA-645 T1, organization parent: `CreateTeam` must refuse a forged parent PRN with
/// `prn-mismatch`, under both `enforce_tenancy` settings, and must create nothing.
///
/// The stored organization PRN has an EMPTY organization slot and an EMPTY region
/// (`paigasus-iam-core/src/tenancy.rs`, `OrganizationId::from_uuid`), so the forged shapes are a
/// NON-EMPTY organization slot and a non-empty region. Both reach the comparison rather than
/// being refused earlier as `invalid-prn`: `convert::node_uuid` checks only the service and the
/// resource type, and never builds an `OrganizationId`.
///
/// A wrong RESOURCE uuid is deliberately NOT tested here — that answers `not-found`, before the
/// comparison runs (spec §3 step 2).
#[tokio::test]
async fn a_forged_org_parent_never_creates_a_team() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let (enforced, unenforced) = two_states(&db, &idp).await;
    let token = idp.bearer("forged-ct", Some("forged-ct@example.com"), "paigasus", 3600);
    support::provision_platform_admin(&enforced, &token).await;
    let (on_addr, on_server) = spawn_tenancy_server(enforced).await;
    let (off_addr, off_server) = spawn_tenancy_server(unenforced).await;
    let mut on = connect(on_addr).await;
    let mut off = connect(off_addr).await;

    let mut failures: Vec<String> = Vec::new();
    let absent_org = Uuid::from_u128(0x0f05).as_hyphenated().to_string();

    for (setting, tag, client) in [("enforce=on", "on", &mut on), ("enforce=off", "off", &mut off)] {
        let org = create_org(client, &token, &format!("ct-{tag}"), "Create Team Parent").await;

        for (shape, forged) in [
            ("non-empty org slot", with_org(&org.prn, &absent_org)),
            ("non-empty region", with_region(&org.prn, "eu-west-1")),
        ] {
            let label = format!("{setting} CreateTeam {shape}");
            let audits = audit_total(&db).await;
            let events = outbox_total(&db).await;
            let err = client
                .create_team(authed(
                    CreateTeamRequest {
                        org_prn: forged,
                        slug: format!("ct-{tag}-forged"),
                        name: "Forged".to_string(),
                    },
                    &token,
                ))
                .await
                .unwrap_err();
            check(&mut failures, &label, err.code() == Code::InvalidArgument, format!("code was {:?}", err.code()));
            check(&mut failures, &label, reason(&err) == "prn-mismatch", format!("reason was {}", reason(&err)));
            check(&mut failures, &label, audit_total(&db).await == audits, "an audit_log row was written".to_string());
            check(&mut failures, &label, outbox_total(&db).await == events, "an event_outbox row was written".to_string());
            // The team must not exist. Listing through the CORRECT parent PRN is the only way to
            // ask, and it doubles as proof that ListTeams works for this org.
            let teams = client
                .list_teams(authed(
                    ListTeamsRequest {
                        org_prn: org.prn.clone(),
                        limit: 100,
                        offset: 0,
                    },
                    &token,
                ))
                .await
                .expect("the correct parent prn must list")
                .into_inner()
                .teams;
            check(&mut failures, &label, teams.is_empty(), format!("the forged create made a team: {teams:?}"));
        }

        // Positive control: the same call with the CORRECT parent PRN must succeed AND move both
        // totals by exactly one. Without it the two "unchanged" assertions above cannot tell a
        // refusal from a query that sees nothing.
        let label = format!("{setting} CreateTeam control");
        let audits = audit_total(&db).await;
        let events = outbox_total(&db).await;
        create_team(client, &token, &org.prn, &format!("ct-{tag}-control")).await;
        check(
            &mut failures,
            &label,
            audit_total(&db).await == audits + 1 && outbox_total(&db).await == events + 1,
            "the positive control wrote no row — the queries cannot see anything".to_string(),
        );
    }

    on_server.abort();
    off_server.abort();
    assert!(failures.is_empty(), "forged org-parent CreateTeam cases failed:\n{}", failures.join("\n"));
}
```

Add `ListTeamsRequest` to the `paigasus_proto::paigasus::iam::v1::{…}` import list at the top of the file.

- [ ] **Step 3: Run it to verify it fails**

```bash
cd rs && cargo nextest run -p paigasus-iam --profile iam -E 'test(a_forged_org_parent_never_creates_a_team)'
```

Expected: FAIL, with the accumulated failures naming `code was Ok`-shaped problems — specifically the `unwrap_err()` panicking because the forged call SUCCEEDED. That panic is the correct pre-fix signal.

- [ ] **Step 4: Write the failing test for `ListTeams`**

Append to `tests/grpc_tenancy.rs`:

```rust
/// SMA-645 T2, organization parent: `ListTeams` must refuse a forged parent PRN with
/// `prn-mismatch`, under both `enforce_tenancy` settings. A list writes nothing, so the control
/// is that the CORRECT parent PRN returns the seeded team — otherwise the refusal could pass by
/// the RPC being broken for every input.
#[tokio::test]
async fn a_forged_org_parent_never_lists_teams() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let (enforced, unenforced) = two_states(&db, &idp).await;
    let token = idp.bearer("forged-lt", Some("forged-lt@example.com"), "paigasus", 3600);
    support::provision_platform_admin(&enforced, &token).await;
    let (on_addr, on_server) = spawn_tenancy_server(enforced).await;
    let (off_addr, off_server) = spawn_tenancy_server(unenforced).await;
    let mut on = connect(on_addr).await;
    let mut off = connect(off_addr).await;

    let mut failures: Vec<String> = Vec::new();
    let absent_org = Uuid::from_u128(0x0f06).as_hyphenated().to_string();

    for (setting, tag, client) in [("enforce=on", "on", &mut on), ("enforce=off", "off", &mut off)] {
        let org = create_org(client, &token, &format!("lt-{tag}"), "List Teams Parent").await;
        let seeded = create_team(client, &token, &org.prn, &format!("lt-{tag}-seed")).await;

        for (shape, forged) in [
            ("non-empty org slot", with_org(&org.prn, &absent_org)),
            ("non-empty region", with_region(&org.prn, "eu-west-1")),
        ] {
            let label = format!("{setting} ListTeams {shape}");
            let err = client
                .list_teams(authed(
                    ListTeamsRequest {
                        org_prn: forged,
                        limit: 100,
                        offset: 0,
                    },
                    &token,
                ))
                .await
                .unwrap_err();
            check(&mut failures, &label, err.code() == Code::InvalidArgument, format!("code was {:?}", err.code()));
            check(&mut failures, &label, reason(&err) == "prn-mismatch", format!("reason was {}", reason(&err)));
        }

        // Control: the correct parent PRN returns the seeded team.
        let label = format!("{setting} ListTeams control");
        let teams = client
            .list_teams(authed(
                ListTeamsRequest {
                    org_prn: org.prn.clone(),
                    limit: 100,
                    offset: 0,
                },
                &token,
            ))
            .await
            .expect("the correct parent prn must list")
            .into_inner()
            .teams;
        check(
            &mut failures,
            &label,
            teams.iter().any(|t| t.prn == seeded.prn),
            format!("the control did not return the seeded team: {teams:?}"),
        );
    }

    on_server.abort();
    off_server.abort();
    assert!(failures.is_empty(), "forged org-parent ListTeams cases failed:\n{}", failures.join("\n"));
}
```

- [ ] **Step 5: Run it to verify it fails**

```bash
cd rs && cargo nextest run -p paigasus-iam --profile iam -E 'test(a_forged_org_parent_never_lists_teams)'
```

Expected: FAIL — `unwrap_err()` panics because the forged list SUCCEEDED.

- [ ] **Step 6: Implement `create_team`**

Replace the body of `create_team` (`tenancy.rs:337`) between the `node_uuid` call and the `teams.create` call. The whole `if self.state.enforce_tenancy { … }` block and its comment go away; the comment's reasoning moves to the helper doc in Task 7.

```rust
    async fn create_team(&self, request: Request<CreateTeamRequest>) -> Result<Response<CreateTeamResponse>, Status> {
        let started = Instant::now();
        let result: Result<Response<CreateTeamResponse>, Status> = async {
            let actor_principal = actor_context(&request)?.principal_id;
            let actor = actor_principal.prn().clone();
            let req = request.into_inner();
            let (org_id, canonical) = convert::node_uuid(&req.org_prn, "organization")?;
            load_org_checked(&self.state, &actor, Action::CreateTeam, org_id, &canonical, "CreateTeam").await?;
            let view = self.state.teams.create(org_id, &req.slug, &req.name, &actor_principal).await.map_err(convert::status_to_grpc)?;
            Ok(Response::new(CreateTeamResponse {
                team: Some(convert::to_proto_team(&view)),
            }))
        }
        .await;
        record_grpc("Tenancy", "CreateTeam", started, &result);
        result
    }
```

- [ ] **Step 7: Implement `list_teams`**

Note `convert::to_page` moves to AFTER the helper call (spec §3 step 5), so a forged parent with an out-of-range `limit` answers `prn-mismatch` rather than `invalid-pagination`.

```rust
    async fn list_teams(&self, request: Request<ListTeamsRequest>) -> Result<Response<ListTeamsResponse>, Status> {
        let started = Instant::now();
        let result: Result<Response<ListTeamsResponse>, Status> = async {
            let actor = actor_context(&request)?.principal_id.prn().clone();
            let req = request.into_inner();
            let (org_id, canonical) = convert::node_uuid(&req.org_prn, "organization")?;
            load_org_checked(&self.state, &actor, Action::ListTeams, org_id, &canonical, "ListTeams").await?;
            let page = convert::to_page(req.limit, req.offset).map_err(convert::status_to_grpc)?;
            let views = self.state.teams.list_by_org(org_id, page).await.map_err(convert::status_to_grpc)?;
            Ok(Response::new(ListTeamsResponse {
                teams: views.iter().map(convert::to_proto_team).collect(),
            }))
        }
        .await;
        record_grpc("Tenancy", "ListTeams", started, &result);
        result
    }
```

- [ ] **Step 8: Run both tests to verify they pass**

```bash
cd rs && cargo nextest run -p paigasus-iam --profile iam -E 'test(a_forged_org_parent)'
```

Expected: 2 tests PASS.

- [ ] **Step 9: Run the whole file, to catch a regression in the existing tests**

```bash
cd rs && cargo nextest run -p paigasus-iam --profile iam -E 'binary(grpc_tenancy)'
cargo clippy -p paigasus-iam --all-targets -- -D warnings && cargo fmt --check
```

Expected: all PASS, clippy and fmt clean.

- [ ] **Step 10: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/adapters/grpc/tenancy.rs rs/crates/services/paigasus-iam/tests/grpc_tenancy.rs
git commit -m "fix(rs): refuse a forged organization parent on CreateTeam and ListTeams (SMA-645)

Both handlers took the parent uuid from the wire PRN and discarded the
canonical form, so a non-empty organization slot or a non-empty region
was accepted and the write went to the real parent with no error.

Both now load the parent unconditionally, authorize under
enforce_tenancy as before, then compare the stored canonical PRN.

ListTeams had no gRPC test coverage at all. Its to_page call moves
after the check, so a forged parent with a bad limit answers
prn-mismatch.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: The team-parent handlers — `CreateProject` and `ListProjects`

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/grpc/tenancy.rs` (`create_project` at `:464`, `list_projects` at `:515`)
- Test: `rs/crates/services/paigasus-iam/tests/grpc_tenancy.rs`

**Interfaces:**
- Consumes: `load_team_checked` from Task 1; `audit_total` / `outbox_total` from Task 2.
- Produces: `a_forged_team_parent_never_creates_a_project`, `a_forged_team_parent_never_lists_projects`.

- [ ] **Step 1: Write the failing test for `CreateProject`**

A team's stored PRN carries the parent organization's uuid in the organization slot, so the forged shapes here differ from Task 2's: a WRONG organization uuid, an EMPTY organization slot, and a non-empty region.

```rust
/// SMA-645 T1, team parent: `CreateProject` must refuse a forged parent PRN with
/// `prn-mismatch`, under both `enforce_tenancy` settings, and must create nothing.
///
/// A team's stored PRN carries the parent organization's uuid in the organization slot
/// (`paigasus-iam-core/src/tenancy.rs`, `TeamId::from_parts`), so the forged shapes are a WRONG
/// organization uuid, an EMPTY organization slot, and a non-empty region. All three reach the
/// comparison: `convert::node_uuid` checks only the service and the resource type, and never
/// builds a `TeamId`.
#[tokio::test]
async fn a_forged_team_parent_never_creates_a_project() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let (enforced, unenforced) = two_states(&db, &idp).await;
    let token = idp.bearer("forged-cp", Some("forged-cp@example.com"), "paigasus", 3600);
    support::provision_platform_admin(&enforced, &token).await;
    let (on_addr, on_server) = spawn_tenancy_server(enforced).await;
    let (off_addr, off_server) = spawn_tenancy_server(unenforced).await;
    let mut on = connect(on_addr).await;
    let mut off = connect(off_addr).await;

    let mut failures: Vec<String> = Vec::new();
    let absent_org = Uuid::from_u128(0x0f07).as_hyphenated().to_string();

    for (setting, tag, client) in [("enforce=on", "on", &mut on), ("enforce=off", "off", &mut off)] {
        let org = create_org(client, &token, &format!("cp-{tag}"), "Create Project Parent").await;
        let team = create_team(client, &token, &org.prn, &format!("cp-{tag}")).await;

        for (shape, forged) in [
            ("wrong org uuid", with_org(&team.prn, &absent_org)),
            ("empty org slot", with_org(&team.prn, "")),
            ("non-empty region", with_region(&team.prn, "eu-west-1")),
        ] {
            let label = format!("{setting} CreateProject {shape}");
            let audits = audit_total(&db).await;
            let events = outbox_total(&db).await;
            let err = client
                .create_project(authed(
                    CreateProjectRequest {
                        team_prn: forged,
                        slug: format!("cp-{tag}-forged"),
                        name: "Forged".to_string(),
                    },
                    &token,
                ))
                .await
                .unwrap_err();
            check(&mut failures, &label, err.code() == Code::InvalidArgument, format!("code was {:?}", err.code()));
            check(&mut failures, &label, reason(&err) == "prn-mismatch", format!("reason was {}", reason(&err)));
            check(&mut failures, &label, audit_total(&db).await == audits, "an audit_log row was written".to_string());
            check(&mut failures, &label, outbox_total(&db).await == events, "an event_outbox row was written".to_string());
            let projects = client
                .list_projects(authed(
                    ListProjectsRequest {
                        team_prn: team.prn.clone(),
                        limit: 100,
                        offset: 0,
                    },
                    &token,
                ))
                .await
                .expect("the correct parent prn must list")
                .into_inner()
                .projects;
            check(&mut failures, &label, projects.is_empty(), format!("the forged create made a project: {projects:?}"));
        }

        // Positive control: see `a_forged_org_parent_never_creates_a_team` for why this is here.
        let label = format!("{setting} CreateProject control");
        let audits = audit_total(&db).await;
        let events = outbox_total(&db).await;
        create_project(client, &token, &team.prn, &format!("cp-{tag}-control")).await;
        check(
            &mut failures,
            &label,
            audit_total(&db).await == audits + 1 && outbox_total(&db).await == events + 1,
            "the positive control wrote no row — the queries cannot see anything".to_string(),
        );
    }

    on_server.abort();
    off_server.abort();
    assert!(failures.is_empty(), "forged team-parent CreateProject cases failed:\n{}", failures.join("\n"));
}
```

Add `ListProjectsRequest` to the proto import list.

- [ ] **Step 2: Run it to verify it fails**

```bash
cd rs && cargo nextest run -p paigasus-iam --profile iam -E 'test(a_forged_team_parent_never_creates_a_project)'
```

Expected: FAIL — `unwrap_err()` panics because the forged create SUCCEEDED.

- [ ] **Step 3: Write the failing test for `ListProjects`**

```rust
/// SMA-645 T2, team parent: `ListProjects` must refuse a forged parent PRN with `prn-mismatch`,
/// under both `enforce_tenancy` settings. The control is that the correct parent PRN returns the
/// seeded project.
#[tokio::test]
async fn a_forged_team_parent_never_lists_projects() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let (enforced, unenforced) = two_states(&db, &idp).await;
    let token = idp.bearer("forged-lp", Some("forged-lp@example.com"), "paigasus", 3600);
    support::provision_platform_admin(&enforced, &token).await;
    let (on_addr, on_server) = spawn_tenancy_server(enforced).await;
    let (off_addr, off_server) = spawn_tenancy_server(unenforced).await;
    let mut on = connect(on_addr).await;
    let mut off = connect(off_addr).await;

    let mut failures: Vec<String> = Vec::new();
    let absent_org = Uuid::from_u128(0x0f08).as_hyphenated().to_string();

    for (setting, tag, client) in [("enforce=on", "on", &mut on), ("enforce=off", "off", &mut off)] {
        let org = create_org(client, &token, &format!("lp-{tag}"), "List Projects Parent").await;
        let team = create_team(client, &token, &org.prn, &format!("lp-{tag}")).await;
        let seeded = create_project(client, &token, &team.prn, &format!("lp-{tag}-seed")).await;

        for (shape, forged) in [
            ("wrong org uuid", with_org(&team.prn, &absent_org)),
            ("empty org slot", with_org(&team.prn, "")),
            ("non-empty region", with_region(&team.prn, "eu-west-1")),
        ] {
            let label = format!("{setting} ListProjects {shape}");
            let err = client
                .list_projects(authed(
                    ListProjectsRequest {
                        team_prn: forged,
                        limit: 100,
                        offset: 0,
                    },
                    &token,
                ))
                .await
                .unwrap_err();
            check(&mut failures, &label, err.code() == Code::InvalidArgument, format!("code was {:?}", err.code()));
            check(&mut failures, &label, reason(&err) == "prn-mismatch", format!("reason was {}", reason(&err)));
        }

        let label = format!("{setting} ListProjects control");
        let projects = client
            .list_projects(authed(
                ListProjectsRequest {
                    team_prn: team.prn.clone(),
                    limit: 100,
                    offset: 0,
                },
                &token,
            ))
            .await
            .expect("the correct parent prn must list")
            .into_inner()
            .projects;
        check(
            &mut failures,
            &label,
            projects.iter().any(|p| p.prn == seeded.prn),
            format!("the control did not return the seeded project: {projects:?}"),
        );
    }

    on_server.abort();
    off_server.abort();
    assert!(failures.is_empty(), "forged team-parent ListProjects cases failed:\n{}", failures.join("\n"));
}
```

- [ ] **Step 4: Run it to verify it fails**

```bash
cd rs && cargo nextest run -p paigasus-iam --profile iam -E 'test(a_forged_team_parent_never_lists_projects)'
```

Expected: FAIL — `unwrap_err()` panics because the forged list SUCCEEDED.

- [ ] **Step 5: Implement `create_project`**

```rust
    async fn create_project(&self, request: Request<CreateProjectRequest>) -> Result<Response<CreateProjectResponse>, Status> {
        let started = Instant::now();
        let result: Result<Response<CreateProjectResponse>, Status> = async {
            let actor_principal = actor_context(&request)?.principal_id;
            let actor = actor_principal.prn().clone();
            let req = request.into_inner();
            let (team_id, canonical) = convert::node_uuid(&req.team_prn, "team")?;
            load_team_checked(&self.state, &actor, Action::CreateProject, team_id, &canonical, "CreateProject").await?;
            let view = self.state.projects.create(team_id, &req.slug, &req.name, &actor_principal).await.map_err(convert::status_to_grpc)?;
            Ok(Response::new(CreateProjectResponse {
                project: Some(convert::to_proto_project(&view)),
            }))
        }
        .await;
        record_grpc("Tenancy", "CreateProject", started, &result);
        result
    }
```

- [ ] **Step 6: Implement `list_projects`**

```rust
    async fn list_projects(&self, request: Request<ListProjectsRequest>) -> Result<Response<ListProjectsResponse>, Status> {
        let started = Instant::now();
        let result: Result<Response<ListProjectsResponse>, Status> = async {
            let actor = actor_context(&request)?.principal_id.prn().clone();
            let req = request.into_inner();
            let (team_id, canonical) = convert::node_uuid(&req.team_prn, "team")?;
            load_team_checked(&self.state, &actor, Action::ListProjects, team_id, &canonical, "ListProjects").await?;
            let page = convert::to_page(req.limit, req.offset).map_err(convert::status_to_grpc)?;
            let views = self.state.projects.list_by_team(team_id, page).await.map_err(convert::status_to_grpc)?;
            Ok(Response::new(ListProjectsResponse {
                projects: views.iter().map(convert::to_proto_project).collect(),
            }))
        }
        .await;
        record_grpc("Tenancy", "ListProjects", started, &result);
        result
    }
```

- [ ] **Step 7: Run both tests, then the whole file**

```bash
cd rs && cargo nextest run -p paigasus-iam --profile iam -E 'test(a_forged_team_parent)'
cargo nextest run -p paigasus-iam --profile iam -E 'binary(grpc_tenancy)'
cargo clippy -p paigasus-iam --all-targets -- -D warnings && cargo fmt --check
```

Expected: all PASS, clippy and fmt clean.

- [ ] **Step 8: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/adapters/grpc/tenancy.rs rs/crates/services/paigasus-iam/tests/grpc_tenancy.rs
git commit -m "fix(rs): refuse a forged team parent on CreateProject and ListProjects (SMA-645)

A team PRN carries its organization uuid in the organization slot, so
the forged shapes here are a wrong organization uuid, an empty slot,
and a non-empty region. All three were accepted.

ListProjects had no gRPC test coverage at all.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: The correct-but-differently-cased control (T3)

Guards against an over-strict fix that compares the raw request string instead of the canonical one. This is a **per-handler** defect, because the canonical is produced and passed at each call site — so all four handlers need a case.

**Files:**
- Test: `rs/crates/services/paigasus-iam/tests/grpc_tenancy.rs`

**Interfaces:**
- Consumes: the four handlers from Tasks 2 and 3; the existing `upper_uuid` and `with_org` helpers.
- Produces: `an_upper_case_uuid_in_a_correct_parent_prn_still_works`.

- [ ] **Step 1: Write the test**

```rust
/// SMA-645 T3: a CORRECT parent PRN written with upper-case uuids must still succeed, on all
/// four handlers. `Prn::canonical()` renders every uuid through `as_hyphenated()`, which
/// lower-cases the resource uuid AND the organization slot — so a fix that compares the raw
/// request string instead of the canonical one breaks these cases.
///
/// All four handlers are covered, not one per parent type: the canonical string is produced and
/// passed at each call site, so passing the raw PRN is a per-handler defect.
#[tokio::test]
async fn an_upper_case_uuid_in_a_correct_parent_prn_still_works() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let state = AppState::new(db.clone(), &support::test_config(&idp)).await.unwrap();
    let token = idp.bearer("upper-parent", Some("upper-parent@example.com"), "paigasus", 3600);
    support::provision_platform_admin(&state, &token).await;
    let (addr, server) = spawn_tenancy_server(state).await;
    let mut client = connect(addr).await;

    let mut failures: Vec<String> = Vec::new();
    let org = create_org(&mut client, &token, "upper-parent", "Upper Parent").await;
    let team = create_team(&mut client, &token, &org.prn, "upper-parent").await;
    let org_uuid = org.prn.rsplit('/').next().expect("org uuid").to_string();

    // CreateTeam: the organization PRN's resource uuid upper-cased.
    let label = "CreateTeam upper resource uuid";
    let r = client
        .create_team(authed(
            CreateTeamRequest {
                org_prn: upper_uuid(&org.prn),
                slug: "upper-ct".to_string(),
                name: "Upper CT".to_string(),
            },
            &token,
        ))
        .await;
    check(&mut failures, label, r.is_ok(), format!("refused a correct prn: {:?}", r.err()));

    // ListTeams: the same shape.
    let label = "ListTeams upper resource uuid";
    let r = client
        .list_teams(authed(
            ListTeamsRequest {
                org_prn: upper_uuid(&org.prn),
                limit: 100,
                offset: 0,
            },
            &token,
        ))
        .await;
    check(&mut failures, label, r.is_ok(), format!("refused a correct prn: {:?}", r.err()));

    // CreateProject: the team PRN's resource uuid upper-cased.
    let label = "CreateProject upper resource uuid";
    let r = client
        .create_project(authed(
            CreateProjectRequest {
                team_prn: upper_uuid(&team.prn),
                slug: "upper-cp".to_string(),
                name: "Upper CP".to_string(),
            },
            &token,
        ))
        .await;
    check(&mut failures, label, r.is_ok(), format!("refused a correct prn: {:?}", r.err()));

    // CreateProject: the team PRN's ORG SLOT uuid upper-cased. `canonical()` folds that uuid
    // too, so this is also a correct PRN — a case `upper_uuid` alone does not reach.
    let label = "CreateProject upper org slot";
    let r = client
        .create_project(authed(
            CreateProjectRequest {
                team_prn: with_org(&team.prn, &org_uuid.to_uppercase()),
                slug: "upper-cp-slot".to_string(),
                name: "Upper CP Slot".to_string(),
            },
            &token,
        ))
        .await;
    check(&mut failures, label, r.is_ok(), format!("refused a correct prn: {:?}", r.err()));

    // ListProjects: both shapes.
    let label = "ListProjects upper resource uuid";
    let r = client
        .list_projects(authed(
            ListProjectsRequest {
                team_prn: upper_uuid(&team.prn),
                limit: 100,
                offset: 0,
            },
            &token,
        ))
        .await;
    check(&mut failures, label, r.is_ok(), format!("refused a correct prn: {:?}", r.err()));

    let label = "ListProjects upper org slot";
    let r = client
        .list_projects(authed(
            ListProjectsRequest {
                team_prn: with_org(&team.prn, &org_uuid.to_uppercase()),
                limit: 100,
                offset: 0,
            },
            &token,
        ))
        .await;
    check(&mut failures, label, r.is_ok(), format!("refused a correct prn: {:?}", r.err()));

    server.abort();
    assert!(failures.is_empty(), "upper-case parent prn cases failed:\n{}", failures.join("\n"));
}
```

- [ ] **Step 2: Run it**

```bash
cd rs && cargo nextest run -p paigasus-iam --profile iam -E 'test(an_upper_case_uuid_in_a_correct_parent_prn_still_works)'
```

Expected: PASS. This test guards the implementation from Tasks 2 and 3 rather than driving new code, so it passes immediately. Task 8's m5 and m6 prove it bites.

- [ ] **Step 3: Commit**

```bash
git add rs/crates/services/paigasus-iam/tests/grpc_tenancy.rs
git commit -m "test(rs): pin that an upper-case parent prn still succeeds (SMA-645)

Covers all four handlers, because the canonical string is produced and
passed at each call site, so comparing the raw request string is a
per-handler defect rather than a helper one.

Includes an upper-case organization SLOT case, which canonical() folds
the same way and which the upper_uuid helper alone does not reach.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: Extend the ungranted-caller test (T4)

Pins the authorize-before-compare order for the four new handlers. The spec chose to extend the existing SMA-643 test rather than write a new one, because that test already drives the same helpers.

**Files:**
- Modify: `rs/crates/services/paigasus-iam/tests/grpc_tenancy.rs` (`an_ungranted_caller_cannot_tell_a_forged_prn_from_a_correct_one`, from `:1111`)

**Interfaces:**
- Consumes: the four handlers; the test's existing `expect_denied` closure, `forged_org` and `forged_team` bindings.
- Produces: nothing new.

- [ ] **Step 1: Add the four handlers to the existing test**

Insert these four loops after the last existing `for (label, prn) in […]` loop and BEFORE the `server.abort()` line. `forged_org` and `forged_team` are already in scope; both use `absent_org`.

```rust
    // SMA-645: the four parent-PRN handlers. Each must answer permission-denied for the forged
    // and the correct parent alike — otherwise the difference between prn-mismatch and
    // permission-denied tells an ungranted caller which organization owns the node.
    for (label, prn) in [("CreateTeam correct", org.prn.clone()), ("CreateTeam forged", forged_org.clone())] {
        let err = client
            .create_team(authed(
                CreateTeamRequest {
                    org_prn: prn,
                    slug: "t3-stolen-team".to_string(),
                    name: "Stolen".to_string(),
                },
                &stranger,
            ))
            .await
            .unwrap_err();
        expect_denied(&mut failures, label, err);
    }
    for (label, prn) in [("ListTeams correct", org.prn.clone()), ("ListTeams forged", forged_org.clone())] {
        let err = client
            .list_teams(authed(
                ListTeamsRequest {
                    org_prn: prn,
                    limit: 100,
                    offset: 0,
                },
                &stranger,
            ))
            .await
            .unwrap_err();
        expect_denied(&mut failures, label, err);
    }
    for (label, prn) in [("CreateProject correct", team.prn.clone()), ("CreateProject forged", forged_team.clone())] {
        let err = client
            .create_project(authed(
                CreateProjectRequest {
                    team_prn: prn,
                    slug: "t3-stolen-project".to_string(),
                    name: "Stolen".to_string(),
                },
                &stranger,
            ))
            .await
            .unwrap_err();
        expect_denied(&mut failures, label, err);
    }
    for (label, prn) in [("ListProjects correct", team.prn.clone()), ("ListProjects forged", forged_team.clone())] {
        let err = client
            .list_projects(authed(
                ListProjectsRequest {
                    team_prn: prn,
                    limit: 100,
                    offset: 0,
                },
                &stranger,
            ))
            .await
            .unwrap_err();
        expect_denied(&mut failures, label, err);
    }
```

- [ ] **Step 2: Update the test's own comment about its call count**

The comment above the `audit_total_before` snapshot says "Eighteen calls below span nine handlers and three node kinds". Eight more calls across four more handlers now follow. Change it to "Twenty-six calls below span thirteen handlers and three node kinds". The unfiltered-total assertion already covers them, which is exactly the property that comment defends.

- [ ] **Step 3: Run it**

```bash
cd rs && cargo nextest run -p paigasus-iam --profile iam -E 'test(an_ungranted_caller_cannot_tell_a_forged_prn_from_a_correct_one)'
```

Expected: PASS. Every new call is refused at the authorization gate, before the comparison, so the answer is `permission-denied` for both PRNs.

- [ ] **Step 4: Commit**

```bash
git add rs/crates/services/paigasus-iam/tests/grpc_tenancy.rs
git commit -m "test(rs): extend the ungranted-caller test to the four parent handlers (SMA-645)

Pins authorize-before-compare for CreateTeam, ListTeams, CreateProject
and ListProjects. Without that order, the difference between
prn-mismatch and permission-denied tells an ungranted caller which
organization owns a node.

Extends the existing test rather than adding a new one: it already
drives the same helpers.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 6: An unknown parent answers not-found for the two Lists (T5)

Pins row B6, the one genuinely new answer: before this change the Lists had no existence check, so an unknown parent returned an empty 200 under `enforce_tenancy = false`.

**Files:**
- Test: `rs/crates/services/paigasus-iam/tests/grpc_tenancy.rs`

**Interfaces:**
- Consumes: `list_teams`, `list_projects` from Tasks 2 and 3.
- Produces: `an_unknown_parent_is_not_found_for_a_list`.

- [ ] **Step 1: Write the test**

```rust
/// SMA-645 row B6: `ListTeams` and `ListProjects` answer `not-found` for a parent that does not
/// exist. This is the one genuinely NEW answer in the change — before it, neither list had an
/// existence check, so an unknown parent returned an empty OK list under
/// `enforce_tenancy = false`. (Under `enforce_tenancy = true` the parent was already loaded for
/// the authorize call, so that setting already answered `not-found`; this test covers both so
/// the two settings are pinned to agree.)
///
/// The gRPC twin of the HTTP behaviour in `tests/http_tenancy.rs`, which records the same
/// "bare empty 200 pre-enforcement" history.
#[tokio::test]
async fn an_unknown_parent_is_not_found_for_a_list() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let (enforced, unenforced) = two_states(&db, &idp).await;
    let token = idp.bearer("absent-parent", Some("absent-parent@example.com"), "paigasus", 3600);
    support::provision_platform_admin(&enforced, &token).await;
    let (on_addr, on_server) = spawn_tenancy_server(enforced).await;
    let (off_addr, off_server) = spawn_tenancy_server(unenforced).await;
    let mut on = connect(on_addr).await;
    let mut off = connect(off_addr).await;

    let mut failures: Vec<String> = Vec::new();
    // Well-formed PRNs naming nodes that were never created.
    let absent_org_uuid = Uuid::from_u128(0x0f09).as_hyphenated().to_string();
    let absent_team_uuid = Uuid::from_u128(0x0f0a).as_hyphenated().to_string();
    let absent_org_prn = format!("prn:pgs:iam:::organization/{absent_org_uuid}");
    let absent_team_prn = format!("prn:pgs:iam::{absent_org_uuid}:team/{absent_team_uuid}");

    for (setting, client) in [("enforce=on", &mut on), ("enforce=off", &mut off)] {
        let label = format!("{setting} ListTeams");
        let err = client
            .list_teams(authed(
                ListTeamsRequest {
                    org_prn: absent_org_prn.clone(),
                    limit: 100,
                    offset: 0,
                },
                &token,
            ))
            .await
            .unwrap_err();
        check(&mut failures, &label, err.code() == Code::NotFound, format!("code was {:?}", err.code()));
        check(&mut failures, &label, reason(&err) == "not-found", format!("reason was {}", reason(&err)));

        let label = format!("{setting} ListProjects");
        let err = client
            .list_projects(authed(
                ListProjectsRequest {
                    team_prn: absent_team_prn.clone(),
                    limit: 100,
                    offset: 0,
                },
                &token,
            ))
            .await
            .unwrap_err();
        check(&mut failures, &label, err.code() == Code::NotFound, format!("code was {:?}", err.code()));
        check(&mut failures, &label, reason(&err) == "not-found", format!("reason was {}", reason(&err)));
    }

    on_server.abort();
    off_server.abort();
    assert!(failures.is_empty(), "unknown-parent list cases failed:\n{}", failures.join("\n"));
}
```

- [ ] **Step 2: Run it**

```bash
cd rs && cargo nextest run -p paigasus-iam --profile iam -E 'test(an_unknown_parent_is_not_found_for_a_list)'
```

Expected: PASS.

If the `enforce=on` half fails with `permission-denied` rather than `not-found`, the load is running after the authorize call rather than before it — check the helper's order against spec §3. If the `enforce=off` half fails with `Ok`, the helper is not being called from that handler at all.

- [ ] **Step 3: Commit**

```bash
git add rs/crates/services/paigasus-iam/tests/grpc_tenancy.rs
git commit -m "test(rs): pin not-found for an unknown list parent (SMA-645)

Row B6 in the spec: the one genuinely new answer. Neither list had an
existence check, so an unknown parent returned an empty OK list when
enforce_tenancy was off.

The gRPC twin of the HTTP behaviour, which already answers not-found.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 7: Correct the documentation

The module doc currently states the OPPOSITE of what the code now does, and names SMA-645 as the reason.

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/grpc/tenancy.rs` (module doc `:10-38`; the three helper doc comments at `:147`, `:161`, `:175`)
- Modify: `rs/crates/services/paigasus-iam/src/adapters/grpc/convert.rs` (`node_uuid`'s doc)

**Interfaces:**
- Consumes: everything from Tasks 1–6.
- Produces: nothing.

- [ ] **Step 1: Replace the false paragraph in the module doc**

Delete the paragraph at `tenancy.rs:16-18` beginning "Creates and Lists **that take a parent PRN** do NOT compare it" and put this in its place. Note the rule's scope is deliberately narrow — see Step 2.

```rust
//! Creates and Lists that take a PARENT PRN compare it too (SMA-645): `CreateTeam`/`ListTeams`
//! against the stored organization, `CreateProject`/`ListProjects` against the stored team, in
//! `load_{org,team}_checked`. Before that they took the parent's uuid and discarded the rest, so
//! a forged parent organization slot — or a forged region, which `convert::node_uuid` does not
//! check either — was accepted and the write went to the real parent with no error.
//!
//! Two consequences worth knowing. The parent load is UNCONDITIONAL for these four, so with the
//! test-only `enforce_tenancy = false` a List against an unknown parent now answers `not-found`
//! where it used to return an empty OK list. And for a request naming the wrong parent, the
//! mismatch now outranks the field and state errors: a forged parent with an invalid slug, an
//! archived parent or an out-of-range `limit` answers the mismatch, not `invalid-slug`,
//! `parent-archived` or `invalid-pagination`.
```

- [ ] **Step 2: State the confirmation rule with its true scope**

Append this to the module doc, after the paragraph from Step 1. Do NOT write an unconditional rule: `ListMemberships`' principal filter is a live counterexample, so the sentence would be false.

```rust
//! **The rule, and its one exception.** Every tenancy-NODE PRN this module accepts is confirmed
//! against the stored node before it is acted on: in the handler for the sixteen node RPCs — the
//! thirteen that route through `load_{org,team,project}_checked`, plus the three Gets, which
//! compare inline after their read — and
//! in the REPOSITORY for the two membership RPCs that take a node PRN (`pg_memberships`'s
//! `list_by_node` and `attach_in` both compare the stored `prn` column and answer
//! `TenancyError::PrnMismatch`). The exception is `ListMemberships` with a PRINCIPAL filter:
//! `parse_principal_prn` checks only the service and the resource type, and `list_by_principal`
//! then filters on a bare uuid, so a forged region or organization slot on a principal PRN is
//! accepted. That is SMA-649, not a property of this design.
```

- [ ] **Step 3: Move the deleted SMA-444 reasoning into the helper docs**

Tasks 2 and 3 deleted four `if enforce_tenancy { … }` blocks whose comments carried the SMA-444 reasoning for resolving the parent by uuid. Add it to `load_org_checked`'s doc (`:147`), after the existing paragraphs:

```rust
/// The load is UNCONDITIONAL, and that predates the comparison needing it: resolving the parent
/// through `orgs.get` — rather than trusting the wire PRN's organization slot, or building an
/// `OrganizationId::from_uuid` PRN without confirming existence — is what keeps a
/// claimed-but-nonexistent parent from reaching the entity-slice loader with a dangling id and
/// failing closed as an internal error instead of the expected `NotFound` (SMA-444).
```

Add the same paragraph to `load_team_checked` (`:161`), with `teams.get` and `TeamId` in place of `orgs.get` and `OrganizationId`.

- [ ] **Step 4: Update `node_uuid`'s doc in `convert.rs`**

Its doc says the canonical "is compared by every Get/Rename/Archive/Restore handler". Replace that sentence with:

```rust
/// The returned canonical is compared against the service's stored canonical PRN by every
/// Get/Rename/Archive/Restore handler, and — since SMA-645 — by the four Create/List handlers
/// that take a PARENT PRN, against the stored PARENT.
///
/// Region and organization slot are validated in TWO stages, and this function is only the
/// first. `Prn::parse` here rejects a syntactically invalid region (`EU-WEST-1` fails
/// `is_valid_region`), so that answers `invalid-prn`. Beyond the service and the resource type
/// this function checks nothing else: it never builds an `OrganizationId`/`TeamId`/`ProjectId`,
/// so the domain's own org-slot rule does not run, and a syntactically VALID region
/// (`eu-west-1`) and any organization slot both pass through into the returned canonical. The
/// second stage is the caller's stored-PRN comparison, which is what rejects a well-formed but
/// non-matching region or slot — and why a forged slot answers `TenancyError::PrnMismatch`
/// rather than `invalid-prn`.
```

- [ ] **Step 5: Verify no quoted error-code literal reached `src/`**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-645
grep -rn '"prn-mismatch"' rs/crates/services/paigasus-iam/src/ || echo "CLEAN"
```

Expected: `CLEAN`. A quoted literal here reds `repo:error-code-single-site`, including in a doc comment.

- [ ] **Step 6: Verify the doc no longer contradicts the code**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-645
grep -n "do NOT compare\|not a property of this design" rs/crates/services/paigasus-iam/src/adapters/grpc/tenancy.rs
```

Expected: no "do NOT compare" line; exactly one "not a property of this design", now naming SMA-649 rather than SMA-645.

- [ ] **Step 7: Build and run the full crate suite**

```bash
cd rs && cargo clippy -p paigasus-iam --all-targets -- -D warnings && cargo fmt --check
cargo nextest run -p paigasus-iam --profile iam --no-tests=pass
```

Expected: clippy clean (it checks intra-doc links), fmt clean, all tests PASS.

- [ ] **Step 8: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/adapters/grpc/tenancy.rs rs/crates/services/paigasus-iam/src/adapters/grpc/convert.rs
git commit -m "docs(rs): correct the tenancy module doc for the parent-prn check (SMA-645)

The module doc stated the opposite of what the code now does and named
SMA-645 as the reason.

States the confirmation rule with its true scope rather than an
unconditional one: the handler confirms for the sixteen node RPCs, the
repository confirms for the two membership RPCs that take a node PRN,
and ListMemberships with a principal filter is a real exception tracked
as SMA-649.

Also records the two visible consequences: an unknown list parent now
answers not-found when enforce_tenancy is off, and a mismatch outranks
the field and state errors.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 8: Prove the tests bite, then run the full graph

A test that passes with the fix removed proves nothing. Seven mutations, each expected to red at least one named test.

**Files:**
- Temporarily modify, then restore: `rs/crates/services/paigasus-iam/src/adapters/grpc/tenancy.rs`

**Interfaces:**
- Consumes: everything from Tasks 1–7.
- Produces: a recorded result table, pasted into the PR description.

- [ ] **Step 1: Read the restore rule before mutating anything**

Restore each mutation by **deleting the lines you inserted**, never with `git checkout --` or `git stash`. A checkout reverts the whole file including the fix under test, so the next mutation then measures an unfixed tree and every result after it is meaningless.

Mark every inserted line with a trailing `// MUTATION` comment so the deletion is unambiguous, and after each restore confirm the tree is clean:

```bash
git diff --stat rs/crates/services/paigasus-iam/src/adapters/grpc/tenancy.rs
```

Expected after each restore: empty output.

- [ ] **Step 2: Run m1 — `create_team` loses the check**

In `create_team`, replace the `load_org_checked(...)` line with the pre-fix block:

```rust
            if self.state.enforce_tenancy { // MUTATION
                let org_view = self.state.orgs.get(org_id).await.map_err(convert::status_to_grpc)?; // MUTATION
                self.state.authorize.check(&actor, Action::CreateTeam, org_view.node.id.prn()).await.map_err(convert::status_to_grpc)?; // MUTATION
            } // MUTATION
```

Restoring the pre-fix block rather than deleting the helper call outright matters: a bare deletion removes the load, the authorization AND the comparison at once, which proves nothing specific about the new check.

```bash
cd rs && cargo nextest run -p paigasus-iam --profile iam -E 'binary(grpc_tenancy)'
```

Expected: `a_forged_org_parent_never_creates_a_team` FAILS. Record which tests failed, then restore.

- [ ] **Step 3: Run m2 — `list_projects` loses the check**

Same shape in `list_projects`, using `teams.get(team_id)` and `Action::ListProjects`, with `to_page` moved back above the block.

Expected: `a_forged_team_parent_never_lists_projects` FAILS. Restore.

- [ ] **Step 4: Run m3 — `list_teams` loses the check**

Same shape in `list_teams`, using `orgs.get(org_id)` and `Action::ListTeams`.

Expected: `a_forged_org_parent_never_lists_teams` FAILS, and `an_unknown_parent_is_not_found_for_a_list` FAILS on its `enforce=off` half. Restore.

- [ ] **Step 5: Run m4 — `create_project` loses the check**

Same shape in `create_project`, using `teams.get(team_id)` and `Action::CreateProject`.

Expected: `a_forged_team_parent_never_creates_a_project` FAILS. Restore.

- [ ] **Step 6: Run m5 — `create_team` passes the raw PRN**

At the `create_team` call site, pass the raw request string instead of the canonicalized one. This mutation belongs at the CALL SITE, not in the helper: the helper receives `canonical: &str` and never sees the request string, so it cannot express this defect.

```rust
            load_org_checked(&self.state, &actor, Action::CreateTeam, org_id, &req.org_prn, "CreateTeam").await?; // MUTATION
```

Expected: `an_upper_case_uuid_in_a_correct_parent_prn_still_works` FAILS on its `CreateTeam upper resource uuid` case. Restore.

- [ ] **Step 7: Run m6 — `list_teams` passes the raw PRN**

```rust
            load_org_checked(&self.state, &actor, Action::ListTeams, org_id, &req.org_prn, "ListTeams").await?; // MUTATION
```

Expected: `an_upper_case_uuid_in_a_correct_parent_prn_still_works` FAILS on its `ListTeams upper resource uuid` case. Restore.

- [ ] **Step 8: Run m7 — compare before authorize**

In `load_org_checked`, move the `if stored != canonical { … }` block ABOVE the `if state.enforce_tenancy { … }` authorize block.

Expected: ONE test function FAILS — `an_ungranted_caller_cannot_tell_a_forged_prn_from_a_correct_one` — but its failure must name **both** the pre-existing SMA-643 cases and the four added in Task 5, since they share the helper and live in the same function. Check the failing labels, not the test count: `RenameOrganization forged` and `ArchiveOrganization forged` alongside `CreateTeam forged` and `ListTeams forged`. If only the SMA-643 labels appear, Task 5's extension did not take effect. Only organization-parent handlers appear at all, because this mutation touches `load_org_checked` alone. Restore.

- [ ] **Step 9: Confirm the tree is clean and the suite is green**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
export PAIGASUS_REQUIRE_DOCKER=1
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-645
git status --short
grep -c "MUTATION" rs/crates/services/paigasus-iam/src/adapters/grpc/tenancy.rs || echo "0 mutations left"
cd rs && cargo nextest run -p paigasus-iam --profile iam --no-tests=pass
```

Expected: `git status --short` shows no modified source file, zero `MUTATION` markers, all tests PASS.

- [ ] **Step 10: Run the full graph as CI does**

Per-project tasks do not run the repo-level gates. Run the whole graph. Note the local-bash split recorded in CLAUDE.md: `repo:affected-smoke` needs system `/bin/bash` 3.2, while `repo:ruff-ci`, `repo:next-public-free` and `repo:publish-metadata` need bash 4+, so a single invocation cannot satisfy every gate — re-run the mismatched ones directly with `/opt/homebrew/bin/bash ci/<gate>/run.sh` and read those results instead of the `moon ci` verdict for them.

Use a bash-3.2-only shim directory for the `moon ci` invocation rather than prepending `/bin` to `PATH`, which would also downgrade `python3` to 3.9 and break `cargo_moon_parity.py` on `tomllib`.

`repo:publish-metadata` was added to that bash-4+ set during this task (SMA-645): `ci/publish-metadata/run.sh:662` uses `declare -A`, so under bash 3.2 it dies immediately with `declare: -A: invalid option` on stderr and an EMPTY `stdout.log`. That shape reads as an infrastructure abort rather than a failed assertion — do not diagnose it as a real break. `repo:actionlint` has no working local bash at all, so it has no local verdict; CI is the only signal for it.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
export PROTO_REPORTER=text
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-645
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :http-extractor-envelope :input-liveness :promtool :observability-drift \
  :nats-permissions :release-parity :release-parity-py :release-parity-ts \
  :publish-metadata :version-lockstep :workflow-credentials :pyo3-stub-drift :ruff-ci \
  :next-public-free :test-e2e \
  --base origin/main \
  --include-relations
```

Expected: green. `repo:error-code-single-site` is the gate most likely to red here, from a quoted `"prn-mismatch"` in a `src/` doc comment — Task 7 Step 5 should already have caught that.

If a task fails without attribution, follow the diagnosis procedure in CLAUDE.md: copy `.moon/cache/ciReport.json` and the task's state directory out of the repo BEFORE re-running anything, because a passing re-run overwrites every artifact holding the evidence.

<!-- moon-diagnosis:ok -->
<!-- This file references ciReport.json only to point at CLAUDE.md's procedure; it does not
     restate or supersede it. `ci/actionlint/run.sh` check 12 requires this marker on any file
     naming ciReport.json. -->

Note the earlier `PROTO_REPORTER=text` export is not decorative: `proto` emits NDJSON on stdout
inside an agent session, which breaks any captured `$(proto …)` call in a gate script.

- [ ] **Step 11: Record the mutation results**

This plan has eight tasks; opening the PR is the pipeline's own Stage 6, not a task here. Write the table to a scratchpad file as you go, then paste it into the PR description at that stage. Format:

| # | mutation | tests that failed |
|---|---|---|
| m1 | `create_team` pre-fix block | … |
| … | … | … |

A mutation that reds NOTHING is a finding, not a formality: it means the corresponding test does not constrain the behaviour it claims to. Stop and report it rather than proceeding.

- [ ] **Step 12: Commit (only if anything changed)**

The mutation battery should leave the tree byte-identical. If Step 9 showed a clean tree there is nothing to commit, and that is the expected outcome. If a mutation exposed a missing assertion, fix the test, re-run the whole battery from Step 2 (a later fix can render an earlier mutation inert), and commit the test change.

---

## Self-Review

**Spec coverage.** §2 decision → Tasks 2, 3. §2.2 unconditional load → Tasks 2, 3 (helper already loads unconditionally from SMA-643). §2.3 row B6 → Task 6. §3 order → Task 5 (T4) and Task 8 m7. §3 step 5 `to_page` → Task 2 Step 7, Task 3 Step 6. §4.1 rename and warn text → Task 1. §4.2 handlers → Tasks 2, 3. §4.3 docs and the scoped rule → Task 7. §4.4 error-code gate → Global Constraints and Task 7 Step 5. §5 SMA-649 exception → Task 7 Step 2. §6 T1 → Tasks 2, 3; T2 → Tasks 2, 3; T3 → Task 4; T4 → Task 5; T5 → Task 6. §6.1 mutations → Task 8. §6.2 full graph → Task 8 Step 10. AC1–AC7 all map to a task.

**Placeholder scan.** No TBD, TODO, "similar to Task N", or "add appropriate error handling". Every code step carries the actual code. Task 3 repeats Task 2's structure in full rather than referring back to it.

**Type consistency.** `load_org_checked` / `load_team_checked` / `load_project_checked` keep SMA-643's signature and are spelled identically in Tasks 1, 2, 3, 7 and 8. `audit_total` / `outbox_total` are defined in Task 2 Step 1 and used in Tasks 2 and 3. `ListTeamsRequest` is imported in Task 2, `ListProjectsRequest` in Task 3; both are used again in Tasks 4, 5 and 6. `check`, `reason`, `authed`, `with_org`, `with_region`, `upper_uuid`, `two_states`, `create_org`, `create_team`, `create_project` all already exist in the test file and keep their current signatures.

**One known ordering dependency.** Task 4's test passes as soon as Tasks 2 and 3 land, so it does not follow the write-a-failing-test cycle. That is correct for a control: it guards against an over-strict fix, and Task 8's m5/m6 are what prove it constrains anything. The same holds for Task 5.
