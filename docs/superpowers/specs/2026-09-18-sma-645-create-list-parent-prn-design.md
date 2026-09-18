# SMA-645 — Create/List tenancy RPCs must validate the parent PRN

**Status:** revised after the adversarial challenge; for review
**Issue:** [SMA-645](https://linear.app/smaschek/issue/SMA-645/rs-iam-createlist-tenancy-rpcs-accept-a-forged-parent-organization)
**Predecessor:** SMA-643 (the same defect class on Rename/Archive/Restore)
**Scope:** `rs/crates/services/paigasus-iam/src/adapters/grpc/tenancy.rs` and its test file

---

## 1. The defect

Four gRPC `TenancyService` RPCs take a PARENT node by PRN:

| RPC | parent field | parent type |
|---|---|---|
| `CreateTeam` | `org_prn` | organization |
| `ListTeams` | `org_prn` | organization |
| `CreateProject` | `team_prn` | team |
| `ListProjects` | `team_prn` | team |

Each calls `convert::node_uuid(&req.<parent>_prn, "<type>")`, which returns `(uuid, canonical)`, and
each **discards the canonical** (`let (org_id, _) = …`). The uuid alone then selects the parent. No
handler compares the caller's PRN with the parent's stored PRN.

Handler names, not line numbers, identify the sites: the numbers in the issue text
(`302,348,447,497`) are pre-SMA-643 positions and were already stale when this spec was written.

### 1.1 What a caller can forge

`convert::node_uuid` (`grpc/convert.rs`) validates **only two** of the six PRN fields: the service
must be `iam` and the resource type must match. It does **not** call `TenancyNodeRef::from_prn`, so
the domain's own `check(prn, type, wants_org)` (`paigasus-iam-core/src/tenancy.rs:66-69`) never runs
on this path.

That leaves two forgeable fields, and the issue names only one of them:

| field | organization parent (`prn:pgs:iam:::organization/<uuid>`) | team parent (`prn:pgs:iam::<org>:team/<uuid>`) |
|---|---|---|
| **org slot** | canonically EMPTY; any uuid is accepted today | the owning org's uuid; any other uuid, or empty, is accepted today |
| **region** | canonically EMPTY; any *syntactically valid* region is accepted today | canonically EMPTY; likewise |

"Syntactically valid" is load-bearing: `is_valid_region` requires lowercase alphanumeric segments
with no empty segment (`paigasus-kernel/src/resource_name.rs:97-99`, applied at `:131-133`), so
`EU-WEST-1` is already `invalid-prn`. `eu-west-1` is not, and is the value the tests use.

So `prn:pgs:iam:eu-west-1::organization/<real-uuid>` and
`prn:pgs:iam::<other-org-uuid>:organization/<real-uuid>` both create a team in the real
organization today, with no error. The region hole is not in the issue text; a canonical comparison
closes it at no extra cost, and §6 covers it.

### 1.2 What this is, and is not

It is **not** an authorization bypass. The uuid selects the parent, the authorize call already runs
against the parent's *stored* PRN, and the write lands in the real parent. It is a correctness
defect: the server accepts a request that names an organization it then ignores, so a client holding
a wrong org uuid writes into a different organization than the one it named and is never told.

---

## 2. Decision

**Compare, and answer `prn-mismatch`.** All four handlers load the parent, authorize, then compare
the request PRN's canonical form against the parent's stored canonical PRN, and refuse a mismatch
with `Code::InvalidArgument` / `ErrorInfo.reason = "prn-mismatch"`.

Three reasons.

1. **Consistency.** Every other node RPC that accepts a caller PRN already compares it. After this
   change all thirteen node RPCs follow one rule. §4.3 states that rule with its true scope — it is
   narrower than "every RPC in the module", and §5 names the exception.
2. **The stated justification does not hold.** The module doc says there is "no stored resource yet
   to compare against". The PARENT is stored, and the handlers already load it under
   `enforce_tenancy`.
3. **Cost is near zero.** SMA-643 built the exact helper this needs. The change removes more code
   than it adds (§4.2).

### 2.1 Rejected: document why the slot may be ignored

The alternative the issue offers is to keep the behaviour and justify it. Rejected: the only
available justification is "the uuid is sufficient to select the resource", which is equally true of
`GetTeam` — and `GetTeam` compares. Accepting it here would mean accepting it there, which would
undo SMA-643.

### 2.2 The parent is loaded unconditionally

Today the parent is loaded only inside `if self.state.enforce_tenancy`. The comparison needs the
parent's stored PRN, so the load moves out of that block; only the `authorize.check` call stays
gated. This matches `load_{org,team,project}_for_write`, which SMA-643 already wrote that way.

The alternative — comparing only when `enforce_tenancy` is on — was rejected because it couples a
PRN-integrity check to an authorization flag, and because SMA-643's tests deliberately assert the
refusal holds under both settings.

### 2.3 Behaviour changes, case by case

Enumerated in the SMA-643 B-list style, because the ordering change is the part a reviewer cannot
infer. **`enforce_tenancy = true` is the production default; `false` is test-only.**

Under `enforce_tenancy = true` the parent is already loaded and already authorized today, so the
handler gains exactly one step. Under `false` the load itself is new.

| # | request | today | after | settings |
|---|---|---|---|---|
| B1 | forged parent, everything else valid | succeeds against the real parent | `prn-mismatch` | both |
| B2 | forged parent + invalid slug or name | `invalid-slug` / `invalid-name` | `prn-mismatch` | both |
| B3 | forged parent + effectively archived parent | `parent-archived` | `prn-mismatch` | both |
| B4 | forged parent + out-of-range `limit` (Lists) | `invalid-pagination` | `prn-mismatch` | both |
| B5 | **unknown** parent uuid, Create | `not-found` | `not-found` — **no change** | both |
| B6 | **unknown** parent uuid, List | empty OK list | `not-found` | `false` only |
| B7 | unknown parent uuid + invalid slug, Create | `invalid-slug` | `not-found` | `false` only |

B1 is the intended fix. B2–B4 are precedence changes: for a request that names the wrong parent,
`prn-mismatch` now takes precedence over the field and state errors. That is the correct precedence
— the server should reject a request that misidentifies its target before judging its contents.

**B5 is the case an earlier draft of this spec got wrong**, and the correction matters: `CreateTeam`
against an unknown organization *already* answers `not-found` today, because `pg_teams::create_in`
locks the parent row and returns `RepositoryError::NotFound` when it is absent
(`pg_teams.rs:127-129`, documented at `application/teams.rs:113-114`). The unconditional load does
not introduce that answer for a Create.

**B6 is the one genuinely new answer.** The Lists have no existence check today
(`list_by_org` / `list_by_team`), so an unknown parent returns an empty 200 under
`enforce_tenancy = false`. After the change it answers `not-found`. **This is the desired
behaviour** and it matches the HTTP twin, which already chose `not-found` and has a test recording
the "bare empty 200 pre-enforcement" history (`tests/http_tenancy.rs:247-279`). §6 T5 adds the gRPC
twin of that test.

B2, B3, B4 and B7 are confined to precedence; no request that succeeds today with a *correct* parent
changes its answer, under either setting.

### 2.4 No existing caller breaks

The change makes a previously accepted request fail, so every caller in the repo was audited.

**No caller breaks.** All four RPCs have one production consumer, the IAM console, and it builds
every parent PRN through the canonical constructors in
`ts/packages/paigasus-console-core/src/prn-tenancy.ts:76-84` — `organizationPrn` emits an empty
region and an empty org slot; `teamPrn` emits an empty region and the correct org uuid. The call
sites are `ts/apps/iam-console/app/(console)/orgs/[org]/{load.ts,commands.ts}` and
`.../teams/[team]/{load.ts,commands.ts}`. The team loader additionally confirms the team with
`GetTeam` before calling `listProjects` (`teams/[team]/load.ts:40-50`). `gateway-console` calls none
of the four. `@paigasus/sdk` and `@paigasus/proto` build no PRNs. `py/` has no TenancyService client.

In Rust, `tests/grpc_tenancy.rs` is the only file that calls these four RPCs, and it always passes
the canonical `org.prn` / `team.prn` returned by a prior create.

The seven non-canonical `prn:pgs:iam:` literals in the repo are grammar negative-test vectors or
fixtures for unrelated RPCs (`IsAuthorized`, `GetOrganization`, membership `node_prn`) against fakes.
None reaches these handlers.

**The console's own reader is already stricter than the server.** `parseTenancyPrn` returns `null`
for a region on any tenancy PRN and for a populated org slot on an organization, with named test
vectors for both (`tests/unit/prn-tenancy.test.ts:112,120`). A PRN this change starts refusing is
one the TypeScript consumer already treats as invalid.

**Out-of-tree clients ship without a flag or a log-only phase**, as SMA-643 did for the same wire
surface. The shape that would break is the plausible partial-knowledge mistake of filling an
organization PRN's org slot with the organization's own uuid. That is the defect being fixed, and a
silent wrong-organization write is worse than a loud refusal.

---

## 3. Order of operations

Per handler, in this order:

1. `convert::node_uuid(parent_prn, type)` → `(parent_uuid, requested_canonical)`; a malformed PRN,
   or one of the wrong service or type, is `invalid-prn` and stops here.
2. Load the parent by uuid through the owning service (`orgs.get` / `teams.get`). An unknown uuid is
   `not-found`.
3. If `enforce_tenancy`, `authorize.check(actor, action, stored_parent_prn)`. A caller with no grant
   gets `permission-denied`.
4. Compare `stored_parent_canonical` with `requested_canonical`. On a difference: log one warning and
   return `prn-mismatch`.
5. For the two Lists, `convert::to_page(req.limit, req.offset)`.
6. Call `teams.create` / `teams.list_by_org` / `projects.create` / `projects.list_by_team`.

**Step 3 before step 4 is load-bearing.** An ungranted caller must get `permission-denied` for a
forged PRN and a correct PRN alike. If the comparison ran first, the difference between
`prn-mismatch` and `permission-denied` would tell an unauthorized caller which organization owns a
node. §6 T4 pins this.

**Step 5 after step 4 is a decision, not an accident.** `to_page` sits before the service call today;
placing it after the helper makes a forged parent with an out-of-range `limit` answer `prn-mismatch`
rather than `invalid-pagination` (row B4). This matches the HTTP twin, which puts `Page::new` after
its authorize block (`adapters/http/teams.rs:102-107`). Do not leave the placement to the
implementer.

### 3.1 The stored canonical cannot itself drift

The comparison refuses a correct PRN only if the STORED canonical is ever non-canonical. Two
invariants make that impossible, and both must be revisited by any future "move a node" feature.

**F1 — the stored PRN is written once and re-parsed strictly.** An organization row's PRN is
re-parsed from the `prn` column through `OrganizationId::from_prn` (`pg_organizations.rs:143-144`)
and a team's through `TeamId::from_prn` (`pg_teams.rs:73-74`); `check`
(`paigasus-iam-core/src/tenancy.rs:66-69`) enforces the org slot's presence on that read path. The
column is written from `canonical()`, which always emits an empty region. A corrupt row therefore
surfaces as a `Backend` error on read, never as a false `prn-mismatch`.

**F2 — a project's `team_prn` is synthesized, and agrees only while the two org ids agree.** The
wire `Project.team_prn` is `v.node.team_id.canonical()` (`grpc/convert.rs:314`), and that `TeamId`
is built as `TeamId::from_parts(model.org_id, model.team_id)` from the **project** row's `org_id`
(`pg_projects.rs:77`) — never from the team row's own `prn` column. A client that takes `team_prn`
from a `GetProject` response and posts it to `CreateProject` is comparing a synthesized value
against the team's stored PRN. They agree because `Project::new` rejects a project whose org differs
from its team's (`paigasus-iam-core/src/tenancy.rs:399-401`), so `project.org_id == team.org_id`
holds by construction. This is the only false-positive path for a legitimately obtained PRN, and it
is closed by that domain invariant rather than by anything in this change.

Uuid case is safe: the comparison is between two `Prn::canonical()` outputs, and `canonical()`
renders every uuid through `as_hyphenated()`, which lower-cases both the resource uuid and the org
slot (`resource_name.rs:163-166`). §6 T3 pins both.

### 3.2 Soundness of comparing outside the write transaction

Unchanged from SMA-643, and it is the parent's PRN that must be stable here: a node's stored PRN is
written once, at insert, and nothing moves a node to a different parent.

---

## 4. Implementation

### 4.1 Rename the three helpers

`load_org_for_write`, `load_team_for_write`, `load_project_for_write` become
`load_org_checked`, `load_team_checked`, `load_project_checked`.

`ListTeams` and `ListProjects` are reads, so `_for_write` would be false at two of the new call
sites. The rename is safe: the names appear in live code only in `tenancy.rs` (whole-tree grep; the
other hits are the SMA-643 spec and plan, which record history and are left alone).

`warn_prn_mismatch`'s message changes from "refused a tenancy write" to "refused a tenancy request"
for the same reason. Nothing asserts that string.

**The warning stays at `warn` for all six RPC kinds, including the two Lists.** A refused request
leaves no audit row and no denial row, so the log line is the only trace, and that is as true of a
refused List as of a refused Rename. The cost is accepted knowingly: a List is called far more often
than a Rename, so a buggy or hostile client can now produce one `warn` per request. That is the
intended signal — a client forging parent PRNs at volume is exactly what an operator should see —
and the line is already structured (`rpc`, `actor`, `requested_prn`, `stored_prn`) for rate-limiting
or filtering downstream. Revisit only with evidence of real log pressure.

`load_project_checked` gains no new caller — `CreateProject` and `ListProjects` have a **team**
parent. Only `load_org_checked` (from `CreateTeam`, `ListTeams`) and `load_team_checked` (from
`CreateProject`, `ListProjects`) get new callers.

### 4.2 Rewrite the four handlers

Each handler's hand-rolled `if self.state.enforce_tenancy { … }` block is replaced by one helper
call. `CreateTeam` becomes (illustrative — the real call sites map their errors explicitly, as there
is no `impl From<TenancyError> for Status`):

```rust
let (org_id, canonical) = convert::node_uuid(&req.org_prn, "organization")?;
load_org_checked(&self.state, &actor, Action::CreateTeam, org_id, &canonical, "CreateTeam").await?;
let view = self.state.teams
    .create(org_id, &req.slug, &req.name, &actor_principal)
    .await
    .map_err(convert::status_to_grpc)?;
```

| handler | helper | action | rpc label |
|---|---|---|---|
| `create_team` | `load_org_checked` | `Action::CreateTeam` | `"CreateTeam"` |
| `list_teams` | `load_org_checked` | `Action::ListTeams` | `"ListTeams"` |
| `create_project` | `load_team_checked` | `Action::CreateProject` | `"CreateProject"` |
| `list_projects` | `load_team_checked` | `Action::ListProjects` | `"ListProjects"` |

For the two Lists, the `convert::to_page` call moves to **after** the helper call, per §3 step 5.

**Do not lose the existing comments.** The four blocks being deleted carry the SMA-444 reasoning for
why the parent is resolved by uuid rather than trusted from the wire PRN (a claimed-but-nonexistent
parent would otherwise reach the entity-slice loader and fail closed as an internal error instead of
`NotFound`). That reasoning must move into the helpers' doc comments, where it now also explains the
unconditional load.

### 4.3 Documentation to correct

- **`tenancy.rs:16-18`** — the paragraph beginning "Creates and Lists **that take a parent PRN** do
  NOT compare it" is now false. Replace it with the rule below and the §2.3 B-list summary.
- **`tenancy.rs:26-38`** — the SMA-444 paragraph says these handlers resolve the parent "rather than
  trusting the wire PRN's org slot directly". Still true, now incomplete: they also refuse it. This
  is also the paragraph that records the `enforce_tenancy = false` divergence, so B6 belongs here.
- **`grpc/convert.rs`** — `node_uuid`'s doc says the canonical "is compared by every
  Get/Rename/Archive/Restore handler". Extend to Create/List, and record that `node_uuid` itself
  validates only service and type, so the org slot and the region reach the comparison rather than
  being refused earlier as `invalid-prn`.

**The rule, stated with its true scope.** Do NOT write "a tenancy RPC never acts on a PRN it has not
confirmed against storage" — §5 shows that sentence is false. Write instead:

> Every tenancy-NODE PRN this module accepts is confirmed against the stored node before it is acted
> on: in the handler for the thirteen node RPCs, and in the repository for the two membership RPCs
> that take a node PRN. The one exception is `ListMemberships`' PRINCIPAL filter — see SMA-649.

The follow-up issue is SMA-649.

**No CLAUDE.md change.** An earlier draft claimed CLAUDE.md carries an SMA-643 divergence sentence
to update. It does not: CLAUDE.md has zero occurrences of `SMA-643`, `enforce_tenancy` or
`prn-mismatch`. The sentence lives in `tenancy.rs:26-31` and is covered by the second bullet above.

### 4.4 The error-code gate

`ci/error-registry/check.py` does not list `adapters/grpc/tenancy.rs` in its `MANIFEST`, and its
`code_pattern` matches a registry code **in quotes anywhere in the file, comments included**. §4.3
asks for doc rewrites that name this reason, so: keep the literal `"prn-mismatch"` out of `src/`.
Write it in backticks in prose, or name `TenancyError::PrnMismatch` instead. A quoted literal in a
doc comment reds `repo:error-code-single-site` on a doc-only edit.

---

## 5. Out of scope

- **Node-filtered `ListMemberships` and `AttachMembership` — already protected, in a different
  layer.** An earlier draft of this spec claimed `MembershipService::list` performs no comparison.
  That was wrong: it checks only the application layer. The guard is in the **repository** —
  `pg_memberships::list_by_node` loads the node by uuid and returns `RepositoryError::PrnMismatch`
  when `model.prn != node.canonical()`, for all three node kinds (`pg_memberships.rs:373-403`), and
  `attach_in` does the same (`:215-256`). Both catch a forged org slot and a forged region. No
  change needed; the module doc must say the confirmation lives in the repository for these two,
  which is why §4.3's rule names the layer.

- **`ListMemberships` with a PRINCIPAL filter — a real gap, tracked as SMA-649.**
  `parse_principal_prn` checks only service and resource type
  (`application/memberships.rs:42-48`), and `list_by_principal` then filters on a bare uuid
  (`pg_memberships.rs:367-371`). So `prn:pgs:iam:eu-west-1:<any-org>:principal/<real-uuid>` returns
  the real principal's memberships with no error — the same defect class as this issue, in the same
  module, with no repository guard to fall back on. It is excluded here because it is not one of
  SMA-645's four handlers, its fix belongs in the application layer rather than this adapter, and
  widening the issue silently is how SMA-643 left SMA-645 behind in the first place. SMA-649 is the
  key §4.3's rule names.

- **The HTTP transport.** All four HTTP twins name the parent with a bare uuid path segment
  (`/v1/organizations/{id}/teams`, `/v1/teams/{id}/projects`, via `UuidPath<…>`), so there is no
  caller-supplied PRN and structurally nothing to forge.

- **Making `convert::node_uuid` itself validate the org slot.** It would turn a forged org slot into
  `invalid-prn` instead of `prn-mismatch`, changing the answer for the nine RPCs SMA-643 just
  settled.

- **The proto contract.** `contracts/proto/paigasus/iam/v1/iam.proto` documents no semantics for
  `org_prn` / `team_prn`, though it does document field semantics elsewhere. Stating the
  must-match rule there is defensible but **deliberately not done here**: a proto edit pulls in the
  codegen-drift step and the three generated bindings, which widens a four-handler fix into a
  cross-language change. Worth its own issue if the contract should carry the rule.

---

## 6. Tests

All tests go in `rs/crates/services/paigasus-iam/tests/grpc_tenancy.rs` and follow the SMA-643
shape: real Postgres via Docker, real mock IdP, two `AppState`s (`enforce_tenancy` on and off)
sharing one database, a real tonic client, and the `check(&mut failures, …)` accumulator so one run
reports every failing case. PRNs are forged with the existing `with_org`, `with_region` and
`upper_uuid` helpers.

`ListTeams` and `ListProjects` have **no** gRPC test coverage today, forged or otherwise. These
tests are their first.

**Forged shapes, spelled out.** "Wrong org uuid" is ambiguous and must not appear in a test name or
comment unqualified — a wrong RESOURCE uuid answers `not-found`, not `prn-mismatch`, and a test
built that way fails for the wrong reason. Use SMA-643's precise wording:

- **organization parent:** a NON-EMPTY organization slot (the stored organization PRN has an empty
  slot, `paigasus-iam-core/src/tenancy.rs:79`); and separately a non-empty region, `eu-west-1`.
- **team parent:** a WRONG organization uuid in the slot; an EMPTY organization slot; and separately
  a non-empty region, `eu-west-1`.

### T1 — a forged parent PRN never creates a node

`a_forged_parent_prn_never_creates_a_node`. For each `enforce_tenancy` setting, and for both
`CreateTeam` and `CreateProject`, over every forged shape above:

- Assert `Code::InvalidArgument` and `reason == "prn-mismatch"`.
- Assert the parent's child list is unchanged.
- Assert no `audit_log` and no `event_outbox` row was written.

**The row assertion needs a new helper, and the obvious one is vacuous.** The existing
`audit_count(&db, action, resource_prn)` filters by resource PRN. A `CreateTeam` audit row carries
the **new team's** PRN (`application/teams.rs:105`, `resource_prn: Some(view.node.id.prn()
.canonical())`), and a refused create produces no team, so there is no such PRN to filter by. The
only PRN in hand is the parent's, and a count filtered by the parent PRN is **zero before and after,
with the fix present or absent** — a green test proving nothing, and the exact vacuity class
SMA-643's positive control exists to prevent.

Add `audit_count_by_action(&db, action)` and `outbox_count_by_type(&db, event_type)`, filtering on
the action or event type **alone**. Snapshot immediately before the forged call and re-read
immediately after; the delta must be 0. A global count is safe here because
`start_migrated_postgres` gives each test its own container, so nothing else writes concurrently.

- **Positive control:** the same call with the correct parent PRN must move that same unfiltered
  count by exactly one. Without it the delta-0 assertion cannot distinguish "refused" from "the
  query sees nothing".

### T2 — a forged parent PRN never lists

`a_forged_parent_prn_never_lists`. For each `enforce_tenancy` setting, for both `ListTeams` and
`ListProjects`, over every forged shape, asserting `InvalidArgument` / `prn-mismatch`.

A List writes nothing, so there is no row assertion. The control is that the **correct** parent PRN
returns the expected rows — a seeded child must come back — so the refusal cannot pass by the RPC
being broken for every input.

### T3 — a correct-but-differently-cased parent PRN still works

`an_upper_case_uuid_in_a_correct_parent_prn_still_works`. Two cases per handler, across **all four**
handlers:

- the parent's resource uuid upper-cased (`upper_uuid`);
- for the two team-parent handlers, the ORG SLOT uuid upper-cased
  (`with_org(prn, org_uuid.to_uppercase())`) — `canonical()` folds that uuid too, so this is a
  correct PRN and must succeed.

All four handlers are needed, not one case per parent type: the canonical string is produced and
passed **per call site**, so "pass the raw request PRN instead of the canonical one" is a
per-handler defect. A version covering only the two Creates leaves the same defect in `list_teams`
and `list_projects` uncaught.

### T4 — an ungranted caller cannot tell a forged parent from a correct one

**Extend the existing `an_ungranted_caller_cannot_tell_a_forged_prn_from_a_correct_one`** rather
than writing a new test. It already drives `RenameOrganization` through the same helper, so a new
test would largely re-prove what it proves. Add the four new handlers to it: with `enforce_tenancy`
on and a caller holding no grant, each must answer `permission-denied` for the forged and the
correct parent PRN alike.

### T5 — an unknown parent answers not-found, not an empty list

`an_unknown_parent_is_not_found_for_a_list`. With `enforce_tenancy = false`, `ListTeams` and
`ListProjects` against a parent uuid that does not exist must answer `not-found`. This pins row B6 —
the one genuinely new answer — and is the gRPC twin of the HTTP test at
`tests/http_tenancy.rs:247-279`. Without it, B6 is an unasserted behaviour change.

### 6.1 Mutation checks

Each must red at least one test above. Run them, record which test caught each, and restore by
deleting the marked insert rather than by `git checkout --`, which would also revert the fix.

| # | mutation | expected to fail |
|---|---|---|
| m1 | in `create_team`, restore the pre-fix block (`if enforce_tenancy { orgs.get; authorize }`, no compare) | T1 |
| m2 | in `list_projects`, restore the pre-fix block | T2 |
| m3 | in `list_teams`, restore the pre-fix block | T2, T5 |
| m4 | in `create_project`, restore the pre-fix block | T1 |
| m5 | at the `create_team` call site, pass `&req.org_prn` instead of the canonicalized PRN | T3 |
| m6 | at the `list_teams` call site, pass `&req.org_prn` instead of the canonicalized PRN | T3 |
| m7 | in `load_org_checked`, move the comparison above the authorize call | T4 |

m1–m4 restore the pre-fix block rather than deleting the helper call outright: deleting it would
remove the load, the authorization AND the comparison at once, which proves nothing specific about
the new check.

m5 and m6 are stated at the **call site**, not in the helper: the helper receives `canonical: &str`
and never sees the request string, so "compare against the raw PRN" is not a mutation it can
express. SMA-643 measured this same mutation at its own call site.

**m7 is expected to red two tests** — the extended T4 and the pre-existing SMA-643 ungranted-caller
assertions, which share the helper. Record both; a single-test result means the T4 extension did not
take effect.

### 6.2 Local run

Run with the repo-pinned toolchain on `PATH` and Docker made mandatory, or a green means nothing:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
export PAIGASUS_REQUIRE_DOCKER=1
cd rs && cargo nextest run -p paigasus-iam --profile iam
```

Without the first export the proto-managed CLIs do not resolve. Without the second, an unreachable
Docker daemon makes every Docker-backed test return early and be counted as **passed**, so the suite
reports green having asserted nothing. Before pushing, run the full graph as CI does, per CLAUDE.md —
the per-crate task does not run the repo-level gates, and no single local bash satisfies all of them.

---

## 7. Acceptance criteria

1. For an **existing** parent whose stored canonical PRN differs from the request's canonical form,
   `CreateTeam`, `ListTeams`, `CreateProject` and `ListProjects` answer `InvalidArgument` /
   `prn-mismatch`, under both `enforce_tenancy` settings, for a forged org slot and a forged region
   alike. (A PRN naming a parent that does not exist answers `not-found` instead, per §3 step 2 —
   an earlier draft's wording conflated the two.)
2. A forged parent on a Create writes no node, and moves neither the `audit_log` nor the
   `event_outbox` count for that action — proven against a positive control that moves both by one.
3. A correct parent PRN still succeeds for all four handlers, including with an upper-case resource
   uuid and an upper-case org slot.
4. An ungranted caller gets `permission-denied` for a forged and a correct parent alike, on all four
   handlers.
5. `ListTeams` and `ListProjects` answer `not-found` for an unknown parent under
   `enforce_tenancy = false` (row B6).
6. The module doc states the rule with the scope in §4.3, names the `ListMemberships` principal-filter
   exception with its follow-up issue key, and no longer claims Creates and Lists do not compare.
7. The seven mutations in §6.1 each red at least one test, and m7 reds two.
