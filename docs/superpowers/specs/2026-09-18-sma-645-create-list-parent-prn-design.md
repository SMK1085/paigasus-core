# SMA-645 — Create/List tenancy RPCs must validate the parent PRN

**Status:** draft for review
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
each **discards the canonical** (`tenancy.rs:302,348,447,497` — `let (org_id, _) = …`). The uuid alone
then selects the parent. No handler compares the caller's PRN with the parent's stored PRN.

### 1.1 What a caller can forge

`convert::node_uuid` (`grpc/convert.rs:156-171`) validates **only two** of the six PRN fields: the
service must be `iam` and the resource type must match. It does **not** call
`TenancyNodeRef::from_prn`, so the domain's own `check(prn, type, wants_org)`
(`paigasus-iam-core/src/tenancy.rs:66-69`) never runs on this path.

That leaves two forgeable fields, and the issue names only one of them:

| field | organization parent (`prn:pgs:iam:::organization/<uuid>`) | team parent (`prn:pgs:iam::<org>:team/<uuid>`) |
|---|---|---|
| **org slot** | canonically EMPTY; any uuid is accepted today | the owning org's uuid; any other uuid, or empty, is accepted today |
| **region** | canonically EMPTY; any value is accepted today | canonically EMPTY; any value is accepted today |

So `prn:pgs:iam:eu-west-1::organization/<real-uuid>` and
`prn:pgs:iam::<other-org-uuid>:organization/<real-uuid>` both create a team in the real
organization today, with no error. The region hole is not in the issue text; a canonical
comparison closes it at no extra cost, and the tests below cover it.

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

1. **Consistency.** Every other RPC in this module that accepts a caller PRN already compares it.
   After this change the rule is unconditional and needs no exception list: *a tenancy RPC never
   acts on a PRN it has not confirmed against storage.* An exception list is what let this hole
   survive SMA-643.
2. **The stated justification does not hold.** The module doc says there is "no stored resource yet
   to compare against". The PARENT is stored, and three of the four handlers already load it.
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

**Consequence, stated plainly.** With the test-only `enforce_tenancy = false`, `CreateTeam` against
an unknown organization uuid starts answering `not-found` where today it reaches
`TeamService::create`. That is the same gRPC-vs-HTTP divergence CLAUDE.md already records for
SMA-643 ("gRPC still loads the node and HTTP does not"), extended from nine RPCs to thirteen. It
extends an accepted, documented difference; it does not create a new class of one. With the default
`enforce_tenancy = true` the two transports still answer alike, because HTTP loads the parent too.

The alternative — comparing only when `enforce_tenancy` is on — was rejected because it couples a
PRN-integrity check to an authorization flag, and because SMA-643's tests deliberately assert the
refusal holds under both settings.

### 2.3 No existing caller breaks

The change makes a previously accepted request fail, so every caller in the repo was audited.

**No caller breaks.** All four RPCs have exactly one production consumer, the IAM console, and it
builds every parent PRN through the canonical constructors in
`ts/packages/paigasus-console-core/src/prn-tenancy.ts:76-84` — `organizationPrn` emits an empty
region and an empty org slot; `teamPrn` emits an empty region and the correct org uuid. The four
call sites are `ts/apps/iam-console/app/(console)/orgs/[org]/{load.ts,commands.ts}` and
`.../teams/[team]/{load.ts,commands.ts}`. The team loader additionally confirms the URL's org
matches the team's stored org (`teams/[team]/load.ts:46`) before calling `listProjects`.
`gateway-console` calls none of the four. `@paigasus/sdk` and `@paigasus/proto` build no PRNs. `py/`
has no TenancyService client.

In Rust, `tests/grpc_tenancy.rs` is the only file that calls these four RPCs, and it always passes
the canonical `org.prn` / `team.prn` returned by a prior create. Its `with_org` / `with_region`
forging helpers are used today only against Rename/Archive/Restore.

The seven non-canonical `prn:pgs:iam:` literals in the repo are either grammar negative-test vectors
or fixtures for unrelated RPCs (`IsAuthorized`, `GetOrganization`, membership `node_prn`) against
fakes. None reaches these handlers.

**The console's own reader is already stricter than the server.** `parseTenancyPrn` returns `null`
for a region on any tenancy PRN and for a populated org slot on an organization, with named test
vectors for both (`tests/unit/prn-tenancy.test.ts:112,120`). So a PRN this change starts refusing is
one the TypeScript consumer already treats as invalid. The server is catching up to the client, not
diverging from it.

---

## 3. Order of operations

Per handler, in this order:

1. `convert::node_uuid(parent_prn, type)` → `(parent_uuid, requested_canonical)`; a bad PRN is
   `invalid-prn` and stops here.
2. Load the parent by uuid through the owning service (`orgs.get` / `teams.get`). An unknown uuid is
   `not-found`.
3. If `enforce_tenancy`, `authorize.check(actor, action, stored_parent_prn)`. A caller with no grant
   gets `permission-denied`.
4. Compare `stored_parent_canonical` with `requested_canonical`. On a difference: log one warning and
   return `prn-mismatch`.
5. Only now call `teams.create` / `teams.list_by_org` / `projects.create` / `projects.list_by_team`.

**Step 3 before step 4 is load-bearing**, and the reason is SMA-643's: an ungranted caller must get
`permission-denied` for a forged PRN and a correct PRN alike. If the comparison ran first, the
difference between `prn-mismatch` and `permission-denied` would tell an unauthorized caller which
organization owns a node. §6 test T4 pins this.

**Step 4 before step 5 matters only for the two Creates**, where step 5 writes. For the two Lists
step 5 is a read, so ordering is a consistency choice, not a safety one — the same order is used so
all four handlers read alike.

### 3.1 The stored canonical cannot itself drift

The comparison refuses a correct PRN only if the STORED canonical is ever non-canonical. It cannot
be. An organization row's PRN is re-parsed from the `prn` column through `OrganizationId::from_prn`
(`pg_organizations.rs:143-144`), and a team's through `TeamId::from_prn` (`pg_teams.rs:73-74`) —
and `check` (`paigasus-iam-core/src/tenancy.rs:66-69`) enforces the org slot's presence on that
read path. The column itself is written from `canonical()`, which always emits an empty region.
A corrupt row therefore surfaces as a `Backend` error on read, never as a false `prn-mismatch`.

Uuid case is likewise safe: the comparison is between two `Prn::canonical()` outputs, and
`canonical()` lower-cases the uuid (`resource_name.rs:163-167`). §6 T3 pins this.

### 3.2 Soundness of comparing outside the write transaction

Unchanged from SMA-643, and it is the parent's PRN that must be stable here: a node's stored PRN is
written once, at insert, and nothing moves a node to a different parent. A future "move" feature
breaks this and must revisit both SMA-643's helpers and this change.

---

## 4. Implementation

### 4.1 Rename the three helpers

`load_org_for_write`, `load_team_for_write`, `load_project_for_write` become
`load_org_checked`, `load_team_checked`, `load_project_checked`.

`ListTeams` and `ListProjects` are reads, so `_for_write` would be false at two of the new call
sites. The rename is mechanical: the names appear in live code only in this one file (the SMA-643
spec and plan under `docs/superpowers/` mention them as a historical record and are left alone).

`warn_prn_mismatch`'s message changes from "refused a tenancy write" to "refused a tenancy request"
for the same reason. Nothing asserts that string.

The warning stays, and stays justified: a refused request leaves no audit row and no denial row, and
the attempt is a tampering signal, so the log line is the only trace. That is true of a refused List
as well.

`load_project_checked` gains no new caller — `CreateProject` and `ListProjects` have a **team**
parent. Only `load_org_checked` (from `CreateTeam`, `ListTeams`) and `load_team_checked` (from
`CreateProject`, `ListProjects`) get new callers.

### 4.2 Rewrite the four handlers

Each handler's hand-rolled `if self.state.enforce_tenancy { … }` block is replaced by one helper
call. `CreateTeam` becomes:

```rust
let (org_id, canonical) = convert::node_uuid(&req.org_prn, "organization")?;
load_org_checked(&self.state, &actor, Action::CreateTeam, org_id, &canonical, "CreateTeam").await?;
let view = self.state.teams.create(org_id, &req.slug, &req.name, &actor_principal).await?;
```

| handler | helper | action | rpc label |
|---|---|---|---|
| `create_team` | `load_org_checked` | `Action::CreateTeam` | `"CreateTeam"` |
| `list_teams` | `load_org_checked` | `Action::ListTeams` | `"ListTeams"` |
| `create_project` | `load_team_checked` | `Action::CreateProject` | `"CreateProject"` |
| `list_projects` | `load_team_checked` | `Action::ListProjects` | `"ListProjects"` |

**Do not lose the existing comments.** The four blocks being deleted carry the SMA-444 reasoning for
why the parent is resolved by uuid rather than trusted from the wire PRN (a claimed-but-nonexistent
parent would otherwise reach the entity-slice loader and fail closed as an internal error instead of
`NotFound`). That reasoning must move into the helpers' doc comments, where it now also explains the
unconditional load.

### 4.3 Documentation to correct

- **`tenancy.rs:16-18`** — the paragraph beginning "Creates and Lists **that take a parent PRN** do
  NOT compare it" is now false. Replace it with the new rule, and state the `enforce_tenancy = false`
  consequence from §2.2.
- **`tenancy.rs:28-38`** — the SMA-444 paragraph says these handlers resolve the parent "rather than
  trusting the wire PRN's org slot directly". Still true, now incomplete: they also refuse it.
- **`grpc/convert.rs:160-163`** — `node_uuid`'s doc says the canonical "is compared by every
  Get/Rename/Archive/Restore handler". Extend to Create/List, and record that `node_uuid` itself
  validates only service and type, so the org slot and region reach the comparison rather than
  being refused earlier as `invalid-prn`.
- **`CLAUDE.md`** — the SMA-643 divergence sentence names `CreateTeam`/`ListTeams` as fetching the
  parent org. Update it to say all four now also compare, and that the `enforce_tenancy = false`
  `not-found` divergence covers them.

---

## 5. Out of scope

- **`AttachMembership` and node-filtered `ListMemberships`.** Their node PRN goes through
  `parse_node_prn` → `TenancyNodeRef::from_prn`, which validates that the org slot is *present* for a
  team/project and *absent* for an organization — but not its VALUE, and not the region. For
  `AttachMembership` the raw wire PRN reaches `MembershipService::attach`, which has its own
  `PrnMismatch` detection. Whether node-filtered `ListMemberships` has equivalent protection was not
  established here. These are not among SMA-645's four handlers; they deserve their own issue rather
  than a silent scope widening.
- **The HTTP transport.** All four HTTP twins name the parent with a bare uuid path segment
  (`/v1/organizations/{id}/teams`, `/v1/teams/{id}/projects`, via `UuidPath<…>`), so there is no
  caller-supplied PRN and structurally nothing to forge. No HTTP change is needed or possible.
- **Making `convert::node_uuid` itself validate the org slot.** Tempting, but wrong here: it would
  turn a forged org slot into `invalid-prn` instead of `prn-mismatch`, changing the answer for the
  nine RPCs SMA-643 just settled.

---

## 6. Tests

All tests go in `rs/crates/services/paigasus-iam/tests/grpc_tenancy.rs` and follow the SMA-643
shape: real Postgres via Docker, real mock IdP, two `AppState`s (`enforce_tenancy` on and off)
sharing one database, a real tonic client, and the `check(&mut failures, …)` accumulator so one run
reports every failing case. PRNs are forged with the existing `with_org`, `with_region` and
`upper_uuid` helpers.

`ListTeams` and `ListProjects` have **no** gRPC test coverage today, forged or otherwise. These
tests are their first.

### T1 — a forged parent PRN never creates a node

`a_forged_parent_prn_never_creates_a_node`. For each `enforce_tenancy` setting, and for both
`CreateTeam` and `CreateProject`:

- Forged shapes: for the organization parent, a wrong org uuid **and** a non-empty region; for the
  team parent, a wrong org uuid **and** an empty org slot.
- Assert `Code::InvalidArgument` and `reason == "prn-mismatch"`.
- Assert nothing was created: the parent's child list is unchanged, and no `audit_log` or
  `event_outbox` row was written for the create action.
- **Positive control:** the same call with the correct parent PRN succeeds and *does* add one
  `audit_log` and one `event_outbox` row — otherwise the "no row" assertions prove nothing because
  the queries see nothing. This control targets a fresh parent for the same reason SMA-643's does.

### T2 — a forged parent PRN never lists

`a_forged_parent_prn_never_lists`. For each `enforce_tenancy` setting, and for both `ListTeams` and
`ListProjects`, the same forged shapes, asserting `InvalidArgument` / `prn-mismatch`.

A List writes nothing, so there is no row assertion. The control is instead that the **correct**
parent PRN returns the expected rows — a seeded child must come back — so the refusal cannot pass by
the RPC being broken for every input.

### T3 — a correct-but-differently-cased parent PRN still works

`an_upper_case_uuid_in_a_correct_parent_prn_still_creates`. `Prn::canonical()` lower-cases uuids, so
a correct PRN written with an upper-case uuid must still succeed. This fails a fix that compares the
raw request string instead of the canonical one — the mutation SMA-643 measured. One case per
parent type is enough.

### T4 — an ungranted caller cannot tell a forged parent from a correct one

`an_ungranted_caller_cannot_tell_a_forged_parent_prn_from_a_correct_one`. With `enforce_tenancy` on
and a caller holding no grant, `CreateTeam` must answer `permission-denied` for both the forged and
the correct parent PRN. This pins §3's authorize-before-compare order; it fails if the comparison is
moved first.

### 6.1 Mutation checks

Each must red at least one test above. Run them, record which test caught each, and restore by
deleting the marked insert rather than by `git checkout --`, which would also revert the fix.

| # | mutation | expected to fail |
|---|---|---|
| m1 | drop the `load_org_checked` call from `create_team` | T1 |
| m2 | drop the `load_team_checked` call from `list_projects` | T2 |
| m3 | in `load_org_checked`, compare against the raw request PRN instead of the canonical one | T3 |
| m4 | in `load_org_checked`, move the comparison above the authorize call | T4 |

### 6.2 Local run

`cargo nextest run -p paigasus-iam --profile iam` needs a reachable Docker daemon; without one these
suites skip and prove nothing. Before pushing, run the full graph as CI does, per CLAUDE.md — the
per-crate task does not run the repo-level gates.

---

## 7. Acceptance criteria

1. `CreateTeam`, `ListTeams`, `CreateProject` and `ListProjects` answer `InvalidArgument` /
   `prn-mismatch` for a parent PRN whose canonical form differs from the parent's stored canonical
   PRN, under both `enforce_tenancy` settings.
2. A forged parent on a Create writes no node, no `audit_log` row and no `event_outbox` row.
3. A correct parent PRN still succeeds for all four, including with an upper-case uuid.
4. An ungranted caller gets `permission-denied` for a forged and a correct parent alike.
5. The module doc states the decision and no longer claims Creates and Lists do not compare.
6. The four mutations in §6.1 each red at least one test.
