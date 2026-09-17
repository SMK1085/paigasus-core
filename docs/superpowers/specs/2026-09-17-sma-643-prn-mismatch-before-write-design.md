# SMA-643 — check `prn-mismatch` before a tenancy write commits

- Linear: [SMA-643](https://linear.app/smaschek/issue/SMA-643)
- Found during: SMA-630 (`docs/superpowers/specs/2026-09-17-sma-630-tenancy-lifecycle-design.md` § 4.2)
- Scope: `rs/crates/services/paigasus-iam`. No proto change and no HTTP change. The only TS
  change is to comments that cite `tenancy.rs` line numbers (D3).

## 1. Problem

The nine gRPC handlers `RenameOrganization`, `ArchiveOrganization`, `RestoreOrganization`,
`RenameTeam`, `ArchiveTeam`, `RestoreTeam`, `RenameProject`, `ArchiveProject` and
`RestoreProject` in `src/adapters/grpc/tenancy.rs` do these steps in this order:

1. `convert::node_uuid(&req.prn, …)` takes the node UUID and the canonical request PRN.
2. If `enforce_tenancy` is on, the handler loads the node and authorizes against the STORED PRN.
3. The handler calls the service. The service commits the write, the outbox event and the audit
   entry in one transaction, then calls `gen_bumper.bump()`.
4. The handler compares the stored canonical PRN with the request PRN and answers `prn-mismatch`.

A request with a real node UUID and a wrong organization slot therefore causes a committed write,
an audit row, an outbox event and an entity-generation bump. The caller gets an error. The
iam-console does not refresh the page on that error, so the page shows old data.

This is not an authorization bypass. Step 2 authorizes against the stored node, not against the
forged slot.

## 2. Facts that the design depends on

All five facts were checked against the code by the spec challenge.

- **F1.** `OrganizationService::get`, `TeamService::get` and `ProjectService::get` are
  `repo.find(id)…ok_or(NotFound)` with no status filter (`application/teams.rs:160-162`,
  `organizations.rs:284-286`, `projects.rs:176-178`). A load before the write therefore keeps
  the `NotFound` answer and does not block `Restore*`.
- **F2.** The `prn` column is written only at insert (`pg_teams.rs:57`). `rename_in` and
  `set_status_in` change only slug, name, status, `updated_at` and `modified_by`. No code in
  `src/` moves a team or a project to a different parent, and no migration changes `prn` or
  `org_id`. The stored canonical PRN of a node therefore cannot change after creation, so a
  comparison outside the write transaction is sound. **Nothing enforces F2.** D3 writes it down
  as an invariant, so that a future "move" feature must come back to this check.
- **F3.** The HTTP layer does not have this defect. Its nine matching routes take a
  `UuidPath<…>`, not a PRN (`http/organizations.rs:42-44`, `http/teams.rs:31-33`,
  `http/projects.rs:24-26`).
- **F4.** The default is `enforce_tenancy = true` (`config.rs:847`, used by
  `tests/support/mod.rs:486`). `tests/grpc_users.rs:283-297` shows a gRPC test with it off.
- **F5.** The authorization check audits only a DENIED decision
  (`adapters/authz/denial_audit.rs:188-191`). In the test harness even a denial never reaches
  `audit_log`, because only `main.rs` starts the drain and `grpc::router` starts no background
  work (`grpc/mod.rs:130-142`).

## 3. Decision

**D1 — Handler pre-load (approved).** Each of the nine handlers does these steps in this order:

1. `node_uuid` (unchanged).
2. Load the node with the owning service's `get(id)`. Do this always, not only when
   `enforce_tenancy` is on.
3. If `enforce_tenancy` is on, authorize against `existing.node.id.prn()` (unchanged action).
4. If `existing.node.id.canonical() != canonical`, answer `TenancyError::PrnMismatch`. No
   service call happens.
5. Call the service write and return its view.

The order "authorize, then compare" is the same as in the `Get*` handlers. It is a security
property: a denied caller with a forged PRN gets `permission-denied`, not `prn-mismatch`. If the
compare came first, a denied caller could try organization UUIDs one at a time and find the
owner from the change in the answer. Test T3 holds this order.

Steps 2–4 go into ONE helper per node kind (`load_org_for_write`, `load_team_for_write`,
`load_project_for_write`, placed next to `resolve_node`). Each helper takes the state, the actor,
the action, the UUID and the canonical request PRN. It returns `Result<(), Status>`: no handler
needs the loaded node, because the service call returns the post-write view that the response
carries. The nine handlers call the helper and do not copy the block.

**D2 — Remove the post-write comparison.** Per F2 it can never fire. If it did fire, it would
bring back the defect. The pre-write comparison is the only comparison.

**D3 — Correct the text that describes the old order.**

- `grpc/tenancy.rs` module doc, lines 7-12: Rename/Archive/Restore compare BEFORE the write;
  Get compares after the read. Write F2 as the invariant that makes this sound.
- `grpc/tenancy.rs` lines 10-12: the reason given for Creates ("there is no stored resource yet")
  is weak, because the parent is stored. Replace it with the true statement: Creates and Lists
  do not compare the parent PRN (see § 6).
- `grpc/tenancy.rs` lines 17-19 ("the two transports can never diverge"): restrict this claim to
  `enforce_tenancy = true`. With it off, gRPC now loads the node and HTTP does not (B2c).
- `grpc/tenancy.rs` lines 26-29 ("this module's own stored-canonical recheck … still fires on the
  actual mutating call"): change to "before the actual mutating call".
- `grpc/convert.rs` lines 161-163 ("compared … after the call"): change to match.
- `ts/apps/iam-console/app/(console)/orgs/node-ref.ts:4-5` and
  `ts/apps/iam-console/app/(console)/orgs/[org]/teams/[team]/load.ts:43-44` cite `tenancy.rs`
  line numbers. Replace each line number with the handler name, so the comment does not go
  stale. This is a comment-only change.
- The SMA-630 spec is a historical record. Do not change it.

**D4 — Log the refused attempt.** The helper writes ONE `tracing::warn!` line before it returns
`PrnMismatch`. After the fix such an attempt leaves no audit row and no denial row, and it is a
tampering signal. The line carries structured fields: the actor PRN, the request PRN, the stored
canonical PRN and the RPC name. It uses the crate's existing `tracing` macros; it adds no metric
and no new dependency. The `Get*` handlers keep their present behavior and get no log line.

**Rejected alternatives.**

- Keep the post-write comparison as defense in depth: rejected by D2.
- Compare inside the write transaction in the service, as `MembershipService::attach` does:
  rejected. It changes nine service signatures and their HTTP callers, and it closes a race that
  F2 excludes.

## 4. Visible behavior changes

- **B1.** A forged PRN now causes no write, no audit row, no outbox row and no generation bump.
  The answer stays `InvalidArgument` with `ErrorInfo.reason = "prn-mismatch"`.
- **B2. Error order.** All the requests below fail in both versions. The reason changes, and in
  some cases the gRPC status code changes too.
  - **B2a.** A forged PRN on `Rename*` together with no new field, an invalid slug, or a slug
    that another node has: now `prn-mismatch` (`InvalidArgument`). Before: `nothing-to-rename`,
    `invalid-slug`, or `slug-conflict` (`AlreadyExists`).
  - **B2b.** With `enforce_tenancy` off, a forged PRN on an archived node: now `prn-mismatch`.
    Before: `node-archived` (`pg_teams.rs:199-201`).
  - **B2c.** With `enforce_tenancy` off, a correct-shape PRN with an UNKNOWN UUID on `Rename*`,
    together with no new field or an invalid slug: now `not-found`. Before:
    `nothing-to-rename` or `invalid-slug`, because the service checks those before it loads
    (`application/teams.rs:179-182`). HTTP keeps the old order here, because it loads only when
    enforce is on (`http/teams.rs:50-58`). This difference exists only in the test-only
    setting; D3 records it.

  No client depends on this order. The iam-console builds the PRN from the URL, confirms it with
  `Get*`, renders it into the form and reads it back from the posted form data
  (`load.ts:40-47`, `page.tsx:95`, `actions.ts:27`). Only a tampered post can mismatch. Such a
  post now gets the generic invalid-input text, not the text at
  `app/_components/error-copy.ts:29,38`. The e2e fake IAM does not read reasons
  (`tests/e2e/support/world.ts:164-172`).
- **B3.** With `enforce_tenancy` off, each of the nine calls now makes one more read. This is
  acceptable: the code calls that setting "test-only configuration, never use in production"
  (`http/mod.rs:782-786`).
- **B4.** Unchanged: an unknown UUID answers `NotFound`. A denied caller answers
  `permission-denied`. A correct PRN behaves as before, including a correct PRN written with an
  upper-case UUID (`canonical()` lower-cases it; T2 holds this).

## 5. Tests

Location: `tests/grpc_tenancy.rs` (existing Docker-backed binary; no new test binary). Reason
literals such as `"prn-mismatch"` stay in `tests/`. A literal in a `#[cfg(test)]` block under
`src/` reds `repo:error-code-single-site` (`ci/error-registry/check.py`).

### 5.1 Harness

- One migrated Postgres per test function. If Docker is not reachable, follow the crate's
  existing `support::start_migrated_postgres` policy.
- Two `AppState`s on that database: **E** (`enforce_tenancy = true`) and **O**
  (`enforce_tenancy = false`), each with its own gRPC server. Two states on one database work:
  `tests/authz_acceptance.rs:453-454` does this, `reconcile_starter` is safe to run twice, and
  `AppState::new` takes no migration lock.
- The two states do NOT share generation counters: `test_config` uses the memory authz cache,
  so each state has its own `Generations::memory()` (`http/mod.rs:339-340`). Therefore:
  - seed the `platform_admin` grant through **E** (`seed_platform_admin` bumps only the state
    it gets);
  - create and set up each node through the state that its case uses. A node for an E case is
    created and archived through E. A node for an O case is created and archived through O.
    E never makes a decision about a node that O changed.
- **One fresh node per case.** Each case creates its own node, and each rename uses a NEW slug
  that is unique in the test (organization slugs are unique across all organizations,
  `m0002_create_tenancy.rs:91`). This keeps the setup rows of one case out of the assertions of
  another case, and it keeps the unfixed run from failing on `slug-conflict` instead of on the
  defect.
- A team case creates its own organization first; a project case creates an organization and a
  team first. The parent rows are not the node under test, so they do not affect the queries.
- **Collect, then assert.** Each case pushes its failures into a `Vec<String>`. The test asserts
  once at the end that the vector is empty and prints every entry. One failed case therefore does
  not hide the others, and the unfixed run shows which cases fail.
- Queries use the SeaORM entities and columns (`audit_log`, `event_outbox`), as
  `tests/mutation_audit_e2e.rs:101-116` does. Action names come from `Action::….as_wire()` and
  event types from `EventType::….as_wire()`, not from string literals.

### 5.2 Cases

**T1 — forged PRN changes nothing (18 cases: 9 handlers × {E, O}).** For each case:

1. Create a fresh node. For a restore case, archive it with the correct PRN. For a rename or an
   archive case, leave it active. (A no-op call writes no audit row even with the defect,
   because `Mutated::changed == false`, so the case would prove nothing.)
2. Read the node with the correct PRN. Keep slug, name, status and `updated_at`.
3. Count the `audit_log` rows with the handler's action and `resource_prn` = the node's
   canonical PRN, and the `event_outbox` rows with the handler's event type and
   `aggregate_prn` = the same PRN. Keep both counts.
4. Send the handler with a forged PRN. For a rename, set a new slug and a new name.
5. Assert: status `InvalidArgument`, `ErrorInfo.reason == "prn-mismatch"`, the node reads back
   unchanged, and both counts are unchanged.
6. **Positive control.** Send the same handler with the correct PRN and the same inputs. Assert
   success, and assert that each count grew by exactly one. This proves that the queries can
   see a row, and it covers B4.

Forged PRN shapes, spread across the cases so that each shape occurs at least once per node kind:

- team or project: an organization slot with a UUID that no organization has;
- team or project: an EMPTY organization slot;
- organization: a NON-EMPTY organization slot (the stored organization PRN has an empty slot,
  `paigasus-iam-core/src/tenancy.rs:79`);
- any kind: a non-empty region slot.

`convert::node_uuid` checks only the service and the resource type (`convert.rs:164-170`), so
all these shapes reach the comparison.

B1 also claims "no generation bump". The test cannot read `Generations` directly
(`http/mod.rs:94-96,169-172`). The claim follows from "no service call", and T1 shows no service
call because the inputs would change real values.

**T2 — upper-case UUID still succeeds (3 cases, E).** For each node kind, send `Rename*` with
the correct PRN but with the UUID in upper case. Assert success. This catches a fix that
compares the raw request string instead of `canonical()`.

**T3 — authorize before compare (9 handlers, E).** Provision a second principal with no grant.
For each handler, on a fresh node, send the request once with a forged PRN and once with the
correct PRN. Assert that both answers are `PermissionDenied` with reason `forbidden`, and that
the audit and outbox counts did not change.

### 5.3 Proof that the tests bite

Write the tests first. Run them against the unchanged handlers. Record that every T1 case fails
(the collected vector shows all 18), and that T2 and T3 pass.

Then apply D1–D3 and record that all tests pass. Then apply each mutation below to the fixed
code, run the tests, record the result, and remove the mutation:

- **m1.** Move the compare inside the `if enforce_tenancy` block. Expected: only the O cases of
  T1 fail.
- **m2.** In one handler, call the service before the helper's compare (the old order). Expected:
  only that handler's T1 cases fail.
- **m3.** In the helper, compare before authorize. Expected: T3 fails for the forged requests.
- **m4.** Compare the raw request string instead of `canonical()`. Expected: T2 fails.

Remove each mutation by editing the code back, not with `git checkout`, so that the uncommitted
fix is kept.

#### Mutation battery result

Task 8 ran the battery on the fixed code (SMA-643). Each mutation was undone by hand after its
run; `git status --short` and `git diff --stat` showed no tracked-file change before the next
mutation started.

- **m1** (compare inside `enforce_tenancy`, all three helpers). All three T1 tests failed:
  `a_forged_prn_never_writes_an_organization`, `a_forged_prn_never_writes_a_team`,
  `a_forged_prn_never_writes_a_project`. Each failure was on the `enforce=off` case. Matches
  the expectation.
- **m2** (`rename_team`: call `teams.rename` before `load_team_for_write`). Two tests failed:
  `a_forged_prn_never_writes_a_team`, on exactly the two `RenameTeam` labels
  (`enforce=on RenameTeam`, `enforce=off RenameTeam`), as expected; and
  `an_ungranted_caller_cannot_tell_a_forged_prn_from_a_correct_one`, which the brief did NOT
  expect to fail. The denied call still wrote an `audit_log` row and an `event_outbox` row.
  This is a real mismatch with the brief, not an adjusted expectation: moving the load call
  after the write also moves the authorize check after the write for `RenameTeam`, so an
  ungranted caller's forged-and-correct requests both write before they are refused.
- **m3** (`load_team_for_write`: compare before authorize). One test failed:
  `an_ungranted_caller_cannot_tell_a_forged_prn_from_a_correct_one`, on exactly
  `RenameTeam forged`, `ArchiveTeam forged`, `RestoreTeam forged`, each answering
  `InvalidArgument`/`prn-mismatch` instead of `PermissionDenied`/`forbidden`. Matches the
  expectation.
- **m4** (`rename_organization`: pass `&req.prn`, the raw request string, instead of the
  canonicalized PRN, to `load_org_for_write`). One test failed:
  `an_upper_case_uuid_in_a_correct_prn_still_renames`, which got `InvalidArgument`/
  `prn-mismatch` for a request PRN differing from the stored one only by UUID case. Matches
  the expectation.

## 6. Out of scope

- The generation bump for a no-op write (`Mutated::changed == false`). That is SMA-606 D7 and
  is intended.
- The `Get*` handlers. They write nothing, so the order after the read is correct.
- Memberships. `pg_memberships.rs:209-257` already compares inside the transaction, before the
  insert.
- An in-transaction check (see the rejected alternatives).
- `CreateTeam`, `ListTeams`, `CreateProject` and `ListProjects` ignore the canonical PRN of the
  parent (`grpc/tenancy.rs:302,348,447,497`). They accept a forged parent organization slot and
  do not answer with an error. The write goes to the real parent, so this is not a bypass. It is
  a separate issue (SMA-645).
- `CreateServiceAccount` echoes the caller's owner PRN, including a forged slot, in its create
  response (`application/service_accounts.rs:147-174`). A later `Get` shows the real
  organization. Same class of defect, separate issue (SMA-646).
- A metric for `prn-mismatch` on a write RPC. D4 adds a warn log only.
