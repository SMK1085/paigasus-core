# SMA-642 — validate the new name on rename of organizations, teams and projects

Status: revised after the adversarial challenge, for review
Issue: [SMA-642](https://linear.app/smaschek/issue/SMA-642/rs-iam-validate-the-new-name-on-rename-of-organizations-teams-and)
Found during: SMA-630, `docs/superpowers/specs/2026-09-17-sma-630-tenancy-lifecycle-design.md`, fact F11

## 1. Problem

The `create` path of the IAM tenancy services validates the display name. The `rename` path does
not. So IAM stores a name on a rename that it refuses on a create.

`validate_name` (`rs/crates/libs/paigasus-iam-core/src/tenancy.rs:178-184`) trims the input, then
refuses an empty result or a result longer than `NAME_MAX_CHARS` (256 Unicode scalar values,
`tenancy.rs:11`). `Organization::new`, `Team::new` and `Project::new` call it
(`tenancy.rs:205,234,268`), so `create` stores the trimmed, bounded name.

The three `rename` methods pass `new_name` to the repository unchecked:

- `rs/crates/services/paigasus-iam/src/application/organizations.rs:302-330`
- `rs/crates/services/paigasus-iam/src/application/teams.rs:178-204`
- `rs/crates/services/paigasus-iam/src/application/projects.rs:194-220`

Each one parses the slug (`let slug = new_slug.map(Slug::parse).transpose()?;`) and then hands
`new_name` down with no equivalent line. The persistence adapters store the value verbatim
(`adapters/persistence/pg_organizations.rs:258`, `pg_teams.rs:220`, `pg_projects.rs:253`). No
database constraint exists: the three `name` columns are `text NOT NULL` with no `CHECK`
(`migration/m0002_create_tenancy.rs:79,105,143`).

Both transport surfaces reach the same three methods, so both carry the defect:

- gRPC: `adapters/grpc/tenancy.rs:225,378,535`
- HTTP: `adapters/http/{organizations,teams,projects}.rs`, the `PATCH /v1/…/{id}` handlers

## 2. Effect

- A `Rename*` call with `new_name = ""`, a whitespace-only name, or a 257-character name succeeds.
- The `invalid-name` reason cannot come back from a rename.
- The iam-console form is the only guard on a renamed name today (SMA-630 fact F11).

## 3. Facts measured for this change

- **F1.** The console never sends an unchanged name. `renameChange`
  (`ts/apps/iam-console/lib/form.ts:72-79`) compares the trimmed form field against the trimmed
  stored value and omits `newName` when the two agree. So a slug-only rename of a node with an
  overlong stored name sends no name at all. Validation on the IAM side does not break that flow.
  This holds for all three node kinds: the three rename commands route through the one
  `renameForm()`/`renameChange()` pair (`app/(console)/orgs/[org]/commands.ts:23,27`,
  `.../teams/[team]/commands.ts:19,23`,
  `.../teams/[team]/projects/[project]/commands.ts:11,15`). There is no fourth, hand-rolled copy.
- **F2.** The console trims the name before it sends it (`ts/apps/iam-console/lib/form.ts:74`, and
  `name: z.string().trim()` at `:55`). IAM's own trim is therefore rarely visible to the console,
  but it is visible to every other client. The two trims are not identical: `nameField` uses
  JavaScript `String.prototype.trim()`, `validate_name` uses Rust `str::trim()` (Unicode
  `White_Space`). U+0085 (NEL) is Rust whitespace but not JavaScript whitespace, so a name of only
  U+0085 passes the console form and IAM answers `invalid-name`.
- **F3.** The no-op comparison in the repositories is byte-exact on the value the application
  passes, and it runs in Rust after the row is fetched, not in SQL:
  `let name_same = new_name.is_none_or(|n| model.name == n);` at `pg_organizations.rs:246`,
  `pg_teams.rs:208` and `pg_projects.rs:241`, mirrored in the three test fakes at
  `application/fakes.rs:125,286,421`.
- **F4.** The wire path for the reason already exists end to end.
  `DomainError::InvalidName` (`paigasus-iam-core/src/value.rs:17`) converts to
  `TenancyError::InvalidName` (`application/error.rs:334`), which answers the wire reason
  `"invalid-name"` (`application/error.rs:179`) and classifies as `ErrorClass::Validation`
  (`application/error.rs:215`). `status_to_grpc` maps that class to `Code::InvalidArgument`
  (`adapters/grpc/convert.rs:111-131`); HTTP maps it to 400. The reason is declared as
  `ERROR_REASON_INVALID_NAME = 7` (`contracts/proto/paigasus/common/v1/error.proto:78`).
  **This path is verified by reading only.** No test asserts `invalid-name` on either transport
  today. § 6.3 closes that.
- **F5.** The error-code registry gate scans for literal quoted code strings and registers
  `application/error.rs` as the emission site (`ci/error-registry/check.py:88`). It does not
  register the three application service files. A call to `validate_name` writes no code literal,
  so the gate needs no new entry — **provided no new test asserts the literal `"invalid-name"`**
  in one of those three files. § 6 states how the tests avoid that.
- **F6.** `rename_in` raises `NodeArchived` inside the transaction (`pg_organizations.rs:238`,
  `pg_teams.rs:200`, `pg_projects.rs:233`, and `fakes.rs:106-108`). The application parses the slug
  before it opens the transaction. So `invalid-slug` already outranks `node-archived` today.
- **F7.** Under the default configuration, both transports read and authorize the node **before**
  they call `rename`. `authz.enforce_tenancy` defaults to `true` (`config.rs:847`). HTTP:
  `if s.enforce_tenancy { let view = s.orgs.get(id).await?; s.authorize.check(…).await?; }`
  (`adapters/http/organizations.rs:84-87`). gRPC: the same shape at
  `adapters/grpc/tenancy.rs:214-221`, `:371-374`, `:524-531`. So `not-found` and `forbidden`
  precede every refusal the application service can raise.
- **F8.** No read path re-validates a stored name. `model_to_org`, `model_to_team` and
  `model_to_project` assign `name: model.name` verbatim (`pg_organizations.rs:151`,
  `pg_teams.rs:81`, `pg_projects.rs:85`), never through `Organization::new`/`Team::new`/
  `Project::new`. A stored name that breaks the rule stays readable and stays returned.
- **F9.** The three `rename` methods are byte-identical in the path this change touches
  (`organizations.rs:302-311`, `teams.rs:178-187`, `projects.rs:194-203`), apart from the
  repository field (`self.repo` against `self.projects`).

## 4. Decisions

### D1 — Validate in the three application `rename` methods

Add one line to each method, directly below the existing slug line:

```rust
let slug = new_slug.map(Slug::parse).transpose()?;
let name = new_name.map(validate_name).transpose()?;
```

Then pass `name.as_deref()` to `rename_in` in place of `new_name`.

The name line mirrors the slug line exactly. This keeps the two fields symmetric in a method a
reader holds in one screen. A shared helper for one line would add a hop and would not remove the
three call sites, because each method must still call it.

Each file also needs `validate_name` added to its `use paigasus_iam_core::{…}` list
(`organizations.rs:22-25`, `teams.rs:18-20`, `projects.rs:18-21`). Those are wrapped lists that
rustfmt reflows, so the diff is larger than three lines. `validate_name` is already re-exported
(`paigasus-iam-core/src/lib.rs:37`), so no export changes.

### D2 — The rename stores the trimmed name

`validate_name` returns the trimmed string, and `create` stores that return value. `rename` now
stores it too. Storing the untrimmed input while validating the trimmed form would keep the exact
defect this change closes: IAM would still hold a name that `create` refuses.

Two consequences follow from F3, in opposite directions. Both are intended.

1. **A whitespace-only difference stops being a change.** Stored `"Acme"`, input `"  Acme  "`: the
   application passes `"Acme"`, the comparison matches, `Mutated::changed` is `false`, and the
   method emits no event, writes no audit entry, and leaves `updated_at` and `modified_by` alone.
   The two names are the same name.
2. **A stored untrimmed name is normalized by the next rename that supplies a name.** Stored
   `"  Acme  "`, input `"  Acme  "`: today the comparison matches and the call is a no-op; after
   this change the application passes `"Acme"`, the comparison does **not** match, and the call
   writes. It trims the stored name, advances `updated_at` and `modified_by`, emits
   `OrganizationRenamed` and records an audit entry (`organizations.rs:312-326`). A call that was
   idempotent becomes a real mutation, once, for that row.

A stored untrimmed name is only reachable through the unvalidated rename this issue closes, so the
affected set is bounded by what the defect already wrote.

**Rejected alternative: compare trimmed against trimmed in the no-op check.** That would avoid
consequence 2 by making the repositories compare `model.name.trim()` against the input. It is
rejected for three reasons. It changes six sites (three Postgres adapters, three fakes) instead of
three, for a case the change is meant to correct rather than preserve. It would leave a stored
untrimmed name unreachable by any rename, permanently — the row could never be normalized. And the
normalization is not silent: it emits the event and the audit entry that record exactly what
changed, which is the repository's own convention for a real write.

### D3 — Order of refusals

**At the application service**, in order: `nothing-to-rename`, then `invalid-slug`, then
`invalid-name`, then everything the repository raises (`node-archived`, `slug-conflict`,
`not-found`). `nothing-to-rename` is first because the
`new_slug.is_none() && new_name.is_none()` check stays at the top of the method. The slug and name
lines both run before `uow.begin()`, so both outrank every repository refusal. This extends the
precedence F6 records for the slug; it does not change it.

**On both transports, in the default configuration, that is not the order a client observes.** By
F7, `get(id)` and `authorize.check` run first, so the observable order is:

`not-found` → `forbidden` → `nothing-to-rename` → `invalid-slug` → `invalid-name` → repository
refusals

Only `enforce_tenancy = false` produces the application-layer order end to end. The default
(`true`, `config.rs:847`) is the documented contract; the transport order above is what a client
may rely on.

**`rename` never answers `parent-archived`.** Measured during implementation, and the reason is
worth stating because `create` differs. `rename_in` guards on the node's EFFECTIVE status — it
folds the ancestors in and then raises the single `NodeArchived`
(`pg_teams.rs:199-201`: `if NodeStatus::effective(team_status, &[org_status]) == …`, and
`pg_projects.rs:232-234` over both ancestors). `ParentArchived` is raised only by the `create`
paths (`pg_teams.rs:131`, `projects.rs:136`). So an active team under an archived organization
answers `node-archived` on a rename and `parent-archived` on a create of a child. A doc comment or
a test that expects `parent-archived` from a rename is wrong.

A request that carries both an invalid slug and an invalid name answers `invalid-slug`, because the
slug line comes first. `OrganizationService::create` and `TeamService::create` behave the same way
(`organizations.rs:144-148`, `teams.rs:124-127`). `ProjectService::create` does **not**: it returns
`not-found` or `parent-archived` at `projects.rs:134-137`, before `Slug::parse` at `:139`.

### D4 — Names IAM already stores are not migrated

The issue leaves this open. The decision: no migration, no backfill, no report. By F8 nothing
re-validates a stored name on read, so such a row stays readable and stays returned.

Precisely what happens to such a row:

- **A rename that supplies no name leaves it untouched.** `None` is never validated. F1 shows this
  is the exact shape the console sends for a slug-only rename.
- **A rename that supplies a name that breaks the rule is refused.** The caller must supply a valid
  name.
- **A rename that supplies a name differing from the stored one only by surrounding whitespace
  normalizes it,** per D2 consequence 2. This is the one case where "stays as it is" does not hold,
  and it is intended.

### D5 — No change to the proto, the adapters or the error registry

F4 and F5 show the whole path is in place. This change touches no `.proto` file, no gRPC handler,
no HTTP handler and no `ci/` gate configuration.

`TenancyError::field()` returns `None` for `InvalidName` (`application/error.rs:273`), so a client
gets no `ErrorInfo.metadata["field"]` and cannot tell which field a two-field request failed on.
That is the contract today for every `Invalid*` variant that carries a `String`. This change does
not widen it. A client that needs the distinction can send one field at a time.

### D6 — No database constraint

Decided with Sven on 2026-09-17. The application layer is the only guard this change adds.

A `CHECK` constraint would need a decision on the rows that already break the rule — either
`NOT VALID`, or a backfill that rewrites data D4 says to keep. Postgres also expresses the rule
differently from Rust: `char_length()` counts code points, but the trim half needs `btrim()`. The
issue's "Expected" section names the application services only.

### D7 — Correct the iam-console comments that this change falsifies

Three doc comments in `ts/apps/iam-console/lib/form.ts` state the invariant this change removes:

- `:22-23` — "IAM's rename path does not validate a name (spec F11), so for a rename this schema is
  the only guard."
- `:37-38` — "IAM stores a renamed name without a check (spec F11)."
- `:48-49` — "IAM can store a name longer than 256 code points (spec F11)."

`ts/apps/iam-console/tests/unit/form.test.ts:44` carries the same premise.

The **code** stays exactly as it is. `renameForm`'s conditional bound (`form.ts:54-61`) is still
needed — but for a different reason: the legacy rows D4 keeps, not "IAM does not validate". A
reader who checks IAM, finds `validate_name`, and applies `nameField` unconditionally would break
every slug-only rename of a legacy node. Nothing gates the comment, so the comment must say why.

This makes the change touch `ts/`. The cost is real: per CLAUDE.md, an `iam-console` edit also runs
`gateway-console-ts:test-e2e`, which needs Docker. The alternative is to leave three comments that
contradict the code and file the correction separately.

## 5. Scope

In scope:

- `rs/crates/services/paigasus-iam/src/application/organizations.rs` — `rename`, plus unit tests
- `rs/crates/services/paigasus-iam/src/application/teams.rs` — `rename`, plus unit tests
- `rs/crates/services/paigasus-iam/src/application/projects.rs` — `rename`, plus unit tests
- `rs/crates/services/paigasus-iam/tests/http_tenancy.rs` — two integration cases (§ 6.3)
- `ts/apps/iam-console/lib/form.ts` and `tests/unit/form.test.ts` — comments only (D7)

Out of scope:

- The `display_name` of a user (`application/create_user.rs:34`,
  `application/authenticate_token.rs:181`). `User` is not a tenancy node and `validate_name` is not
  applied there today. If that is a defect, it is a separate one.
- The service-account name. `ServiceAccount::new` already calls `validate_name`
  (`paigasus-iam-core/src/service_account.rs:25`), and `application/service_accounts.rs` exposes
  only `create`/`get`/`list`/`archive` — no rename. `ApiKeyService::issue` (`api_keys.rs:204-212`)
  takes no name at all.
- Any behaviour change in the iam-console. F1 shows the console already sends what IAM will accept.
  `error-copy.ts:30` maps `INVALID_NAME` to "Enter a name.", which reads wrong for a length
  refusal; the console bounds a changed name at 256 code points itself, so a length refusal from
  IAM needs a non-console client. Left as it is.
- A database constraint (D6).
- gRPC integration coverage for rename. `tests/grpc_tenancy.rs` never calls `RenameOrganization`,
  `RenameTeam` or `RenameProject` — a pre-existing gap this change does not create and does not
  close. § 6.3 pins the wire behaviour over HTTP instead.

## 6. Testing

### 6.1 Unit tests on all three node kinds

In the existing `#[cfg(test)]` module of each application file, using the existing fakes
(`application/fakes.rs`). The issue names these three. Each one proves the `validate_name` call is
present in that file.

| Case | Input | Expected |
|---|---|---|
| Empty name | `Some("")` | `TenancyError::InvalidName` |
| Whitespace-only name | `Some("   ")` | `TenancyError::InvalidName` |
| 257 characters | `Some(&"x".repeat(257))` | `TenancyError::InvalidName` |

`TenancyError::InvalidName` carries a `String` (`application/error.rs:40`), so these assert with
`matches!(err, TenancyError::InvalidName(_))`, never `assert_eq!` against a bare variant, which
does not compile. **No test in these three files may assert the literal `"invalid-name"`** — none
of them is in `ci/error-registry/check.py`'s `MANIFEST`, so a literal there reds
`repo:error-code-single-site` (F5).

### 6.2 Unit tests on the semantics, on one node kind each

These prove the shared semantics of the pipeline, not the presence of the call. F9 shows the three
methods are byte-identical in this path, and 6.1 already proves each one calls `validate_name`, so
each case below runs on one node kind rather than three. All of them run on the organization except
the ancestor-archived case, which needs a node that has an ancestor and so runs on a team.

| Case | Input | Expected |
|---|---|---|
| The upper bound is inclusive | `Some(&"x".repeat(256))` | Accepted. A wrong comparison operator fails here and nowhere else. |
| Unicode scalars, not bytes | `Some(&"ü".repeat(256))` | Accepted. The count is scalar values (`tenancy.rs:394` asserts the same on `validate_name` itself). |
| The stored name is trimmed (D2) | `Some("  Acme Corp.  ")` | The stored name is `"Acme Corp."`. |
| A whitespace-only difference is a no-op (D2.1) | Stored `"Acme"`, input `Some("  Acme  ")` | `changed == false`: `updated_at` and `modified_by` unchanged, no event, no audit entry. |
| A legacy untrimmed name is normalized (D2.2) | Stored `"  Acme  "`, input `Some("  Acme  ")` | `changed == true`: the stored name becomes `"Acme"`, `updated_at` and `modified_by` advance, one event and one audit entry. |
| A bad name outranks an archived node (D3) | An archived org, `Some("")` | `TenancyError::InvalidName`, not `NodeArchived`. |
| A bad name outranks an ancestor-archived node (D3) | A team under an archived org, `Some("")` | `TenancyError::InvalidName`, not `ParentArchived`. The org's own guard is own-status only (`pg_organizations.rs:238`); teams and projects fold ancestors (`pg_teams.rs:200`, `pg_projects.rs:233`), so this case needs a team. |
| A legacy overlong name survives a slug-only rename (D4) | Stored name of 300 characters, `new_slug = Some("x")`, `new_name = None` | The rename succeeds and the stored name is unchanged. |
| Create and rename accept the same set (Risk 3) | One table of names, each run through `Organization::new` and through `rename` | The two agree on every row. A file that loses its `validate_name` line fails 6.1; this fails if the two rules ever diverge. |

### 6.3 Integration tests over HTTP

In `rs/crates/services/paigasus-iam/tests/http_tenancy.rs`, beside the existing
`nothing-to-rename` assertion (`:57-60`). This suite is Docker-backed. These two are the only
execution-level proof that F4's wire path works and that D3's transport order is real.

| Case | Request | Expected |
|---|---|---|
| A bad name answers `invalid-name` on the wire | `PATCH /v1/organizations/{id}` with `{"name": ""}` | HTTP 400, code `invalid-name`. |
| `not-found` precedes `invalid-name` (D3, F7) | `PATCH /v1/organizations/{unknown-id}` with `{"name": ""}` | HTTP 404, code `not-found` — not `invalid-name`. |

`adapters/http/error.rs` is on the registry MANIFEST as `asserts` (`check.py:109`), and the test
file is not a `src/` file, so asserting the literal code string here is correct and safe.

### 6.4 Regression

The existing rename unit tests must stay green unchanged. They use names such as `"Acme"` and
`"Acme Corp."`, which the rule accepts. In particular `rename_to_identical_values_is_a_no_op`
(`organizations.rs:540`), `rename_with_a_matching_slug_but_a_new_name_still_changes`
(`organizations.rs:559`) and `a_no_op_rename_emits_nothing_but_a_real_one_emits`
(`organizations.rs:832`) must not need an edit. An edit to one of them is a signal that the change
altered behaviour beyond this spec.

### 6.5 Commands

Unit tests, no Docker needed:

```
cd rs && cargo nextest run -p paigasus-iam --lib
```

The integration cases in 6.3 need Docker. Run them named, with the require-Docker flag, so a
missing daemon reds instead of skipping quietly:

```
cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test http_tenancy
```

Then the full repo gate command from `CLAUDE.md` before the push.

## 7. Risks

- **A client that today sends an untrimmed name now sees the trimmed name stored.** Intended (D2).
  The console is nearly unaffected (F2). No other client exists in this repository.
- **One previously idempotent call becomes a real write, once per affected row** (D2 consequence 2).
  It emits an event and an audit entry, so it is observable, not silent. 6.2 pins it.
- **A client that today renames to an empty name now receives `invalid-name`.** This is the point of
  the change. The reason is already declared and already handled by `@paigasus/sdk` (SMA-630 fact
  F12), so no consumer needs new code to read it.
- **Three copies of one line can drift.** Nothing gates this. The copies sit one line below the slug
  line they mirror, 6.1 fails in the file that loses its copy, and 6.2's last row fails if `create`
  and `rename` ever stop agreeing.
- **gRPC rename stays without integration coverage.** Pre-existing (§ 5). The application-layer
  unit tests and the HTTP cases in 6.3 cover the logic and the wire mapping; only the gRPC handler
  wiring is unproven, and it is a pass-through (`adapters/grpc/tenancy.rs:225,378,535`).
