# SMA-586 — Split the `invalid-prn` catch-all into a per-kind error taxonomy

**Issue:** [SMA-586](https://linear.app/smaschek/issue/SMA-586/iam-invalid-prn-is-a-catch-all-sentinel-for-four-unrelated-validation)
**Date:** 2026-08-23
**Status:** Revised after adversarial challenge; pending approval

## Problem

`TenancyError::InvalidPrn(String)` is the sentinel for any validation failure that has no
dedicated error code. Two facts compound:

- `code()` returns `"invalid-prn"` (`application/error.rs:130`), so the wire `reason` says
  "prn" regardless of what actually failed.
- `Display` is the static `"invalid resource prn"` (`application/error.rs:41-42`), so the
  free-text detail each call site passes is **discarded** before it reaches the client.
  `grpc/convert.rs`'s `parse_opt_ts_detail_is_swallowed_by_invalid_prns_static_display` test
  pins this, so it is established behaviour rather than a local bug.

A caller who sends a malformed timestamp, a malformed uuid, an unknown enum value, or a bad
argument combination gets the identical, actively misleading response:

```
reason:  "invalid-prn"
message: "invalid resource prn"
```

This became a public-contract problem when SMA-504 put `reason` on the wire as the
machine-readable identity clients branch on. SMA-508 (`@paigasus/sdk`) is the forcing
function: its AC2 requires error mapping to branch on `(domain, reason)` **only, never on
message text**, so against today's vocabulary the SDK gets one branch for four unrelated
failures and cannot fall back to the message, because the message is static and carries
nothing.

The registry itself already follows one-reason-per-validation-kind (`invalid-action`,
`invalid-email`, `invalid-name`, `invalid-slug`, `invalid-scope`, `invalid-pagination`,
`invalid-api-key`, `invalid-request-body`). `invalid-prn` doing catch-all duty contradicts
that convention. It survived SMA-498 only because that ticket's AC2 was to freeze the
existing vocabulary *unchanged*, not to correct it.

## Acceptance criteria → design

| AC | Where it is satisfied |
|---|---|
| **AC-1** — malformed timestamp, malformed uuid, unknown enum value and bad argument combination each yield a **distinct** reason, **on both HTTP and gRPC** | The vocabulary (§ *The vocabulary*) plus the two parity repairs in D5 — without those, `invalid-uuid` has no HTTP emitter and AC-1 fails on that kind |
| **AC-2** — `invalid-prn` is emitted only for genuine PRN parse/shape failures | D3; the residual `InvalidPrn` sites are enumerated in § *Sites that keep `InvalidPrn`* |
| **AC-3** — a test asserts HTTP and gRPC agree on the reason for the same logical failure | D4 and § *Testing*, including the divergence table that makes each accepted asymmetry an assertion rather than an omission |
| **AC-4** — every new reason resolves via `ErrorReason::from_wire_reason`, and `repo:error-code-single-site` is green | Free: the membership test iterates `strum::EnumIter`. § *Registry mechanics* and § *Verification* |
| **AC-5** — `buf breaking` is clean (additive only) | § *Compatibility* |

## Scope

18 production sentinel sites across **six** kinds (the ticket's "four kinds" framing predates
D1's split), on both transports, plus the parity repairs D5 adds. Genuine PRN uses are out of
scope and keep `InvalidPrn`.

## Design decisions

### D1 — Six reasons, not four

The ticket proposed four candidates (`invalid-timestamp`, `invalid-uuid`, `invalid-outcome`,
`invalid-argument`). This design splits two of them, for six total.

**`invalid-argument` is rejected.** It echoes gRPC's `INVALID_ARGUMENT` status code and is
broad enough to re-accrete exactly the catch-all this work removes. The five argument sites
divide along a line clients act on differently — "you omitted something" versus "you sent too
much" — becoming `missing-required-field` and `mutually-exclusive-fields`.

**Cursors are split from ids.** The eight uuid sites are four opaque, server-issued
pagination cursors and four caller-supplied resource ids. A console handles a rejected cursor
by resetting pagination; it handles a rejected id by asking the user to fix their input. The
registry is append-only, so `invalid-cursor` could be added later — but a client that had
already branched on `invalid-uuid` for cursors would break when it was. Splitting now is free;
splitting later is not.

**A single `invalid-field` plus `metadata["field"]` was considered and rejected.** It would
satisfy SMA-508 AC2 only weakly: the SDK's mapping table would again have one branch for every
validation failure, and would have to read metadata to distinguish them — which is branching
on something other than `reason`, the thing AC2 exists to prevent. The metadata channel is
still used, but as *additional* detail on top of a distinct reason (D2), not as a substitute
for one.

### D2 — Variants carry a `&'static str` field name, surfaced in `Display` and in gRPC metadata

The ticket requires a deliberate decision on whether the swallowed detail now reaches the
wire, and states that if it stays discarded the argument should be dropped rather than left
as a trap. Three options were considered:

1. **No payload, static `Display`.** Strictest reading of SMA-504 D7. Rejected: a client
   sending two bad timestamp bounds cannot tell which one failed.
2. **`String` payload, full detail on the wire.** Rejected: today's details interpolate
   caller-supplied values (`unknown audit outcome: {raw}`, `invalid RFC3339 timestamp: {s}`),
   so this reflects untrusted input into the error body and leaves every future call site free
   to interpolate whatever it likes — a policy no type enforces.
3. **`&'static str` field name (chosen).** Each variant carries a server-authored field name
   and `Display` interpolates only that.

Option 3 makes "never reflect caller input" a **structural** property rather than a discipline
the next call site has to remember: caller data is unrepresentable in a `&'static str`. It
preserves SMA-504 D7's posture — a field name the client itself sent is not backend detail.

**Where the field name lands.** `Display` alone is not enough: SMA-508 AC2 forbids branching
on message text, so a field name reachable only through the message is reachable only by
humans. On gRPC the field name is therefore *also* emitted as
`ErrorInfo.metadata["field"]`, using the mechanism that already exists and is already used for
`capability` (`grpc/convert.rs:96-104`) and that `error.proto:36-42` documents as an open map.

On **HTTP** it stays in the message only. The error object's key set is positively pinned to
exactly `{code, message}` by `http/error.rs::the_error_object_key_set_is_unchanged`, and
adding a third key is a wire-shape change to the HTTP contract that no acceptance criterion
requires. That asymmetry is deliberate and is recorded here rather than hidden; extending the
HTTP envelope is filed as a follow-up rather than smuggled into this change.

*(Note: an earlier draft justified option 3 partly by "server-side context is lost from logs
too". That was wrong — `status_to_grpc` and `ApiError::into_response` log only on
`ErrorClass::Internal`, so these payloads are logged nowhere today. The claim is withdrawn;
the structural argument above stands on its own.)*

### D3 — `InvalidPrn` is untouched

It keeps its `String` payload and its static `Display`. Its remaining callers pass either the
kernel's stable error-kind token or a canonical PRN, and changing its `Display` is explicitly
out of scope per the ticket. After migration it is emitted only for genuine PRN parse and
shape failures, which is AC-2.

### D4 — Parity is asserted as a divergence table, not just a same-variant table

Both transports derive the reason from the *same* function — `http/error.rs:35` calls
`self.0.code()` and `grpc/convert.rs:125` calls `e.code()`. So for a given `TenancyError`
variant the reason is identical **by construction**, and a test that feeds both transports the
same failure and compares reasons proves almost nothing: parity can only break where the two
transports *construct different variants*.

The guard is therefore built around the cases that can actually drift. It drives
`to_filter` / request-conversion entry points rather than the raw `parse_*` helpers (so it also
proves the helper is still wired in), and it carries one row per **divergent** site asserting
each transport's actual reason explicitly. An accepted asymmetry becomes a failing assertion
the moment it changes, rather than a gap nothing covers.

Expected reasons are pinned as `ErrorReason` values compared via `as_wire_reason()`, never as
kebab literals — which routes the assertion through the registry (so an unregistered rename
fails) and keeps `grpc/convert.rs`'s documented "every code leaves this file through an
`ErrorReason` static" posture honest.

### D5 — Two parity repairs are required, one divergence is accepted

Migrating naively would *create* cross-transport divergence where uniformity (wrong, but
uniform) exists today. AC-1 requires each kind to yield its reason "on both HTTP and gRPC", so
two repairs are in scope rather than optional:

1. **`invalid-uuid` has no HTTP emitter.** HTTP's uuid inputs are path params typed
   `Path<Uuid>`, so a malformed uuid is rejected by axum's extractor before any of our code
   runs — producing axum's default plain-text rejection, not even the
   `{"error":{code,message}}` envelope. A custom `FromRequestParts` path extractor emitting
   `InvalidUuid` closes both the AC-1 gap and a pre-existing envelope inconsistency.

   **It is applied at all 26 `Path<Uuid>` / `Path<(Uuid, Uuid)>` sites**, not only the four
   that twin a gRPC `invalid-uuid` site (`memberships.rs:82`, `authz.rs:150`,
   `api_keys.rs:113`, `dead_letters.rs:126,132`). Migrating only those four would satisfy AC-1
   while leaving 22 routes — every organization, team, project, service-account and api-key
   path — answering a malformed uuid outside the error contract, which is a *new* inconsistency
   inside HTTP itself of exactly the kind this ticket exists to remove. The change is wide but
   mechanical: one extractor type, a signature swap per handler, no logic change. It models
   `authn.rs`'s existing `EnvelopeJson` extractor, which already does this for `Json<T>`
   rejections.
2. **`missing-required-field`'s gRPC twins fall through to the PRN parser.** `grpc/authz.rs:236`,
   `grpc/service_accounts.rs:142` and `:177` pass an empty `principal_prn` / `owner_prn` /
   `scope_prn` straight into `parse_node_prn`, yielding `invalid-prn` where HTTP now yields
   `missing-required-field`. Each gains an explicit empty-string check first. This also matches
   proto3's convention that an empty string *is* the unset sentinel.

**Accepted divergence: `expires_at`.** gRPC `IssueApiKey.expires_at` is a
`prost_types::Timestamp` and yields `invalid-timestamp`; HTTP's `IssueApiKeyBody.expires_at`
is `Option<DateTime<Utc>>` (`http/dto.rs:464`) and fails inside serde. Making the HTTP side
`invalid-timestamp` would require a custom deserializer for no contract gain, so the
divergence stands.

**Correction (found during Task 8, 2026-08-24).** An earlier draft of this section claimed the
HTTP side yields `invalid-request-body`. **That is false**, and the divergence table must not
assert it. `http/api_keys.rs::issue` deserializes its body with plain `axum::Json`, not
`authn::EnvelopeJson`, so a malformed body is rejected by axum with a plain-text 422 that
carries no `error.code` at all — it never reaches the IAM error envelope or the registry.

This is not unique to that handler. **Seven** JSON-body routes take plain `axum::Json`:
`api_keys::issue`, `authz::{is_authorized, put_policy, create_role_grant}`,
`dead_letters::replay_matching` and `service_accounts::create`. Only `api_keys::introspect` and
`system_retirement::retire` use `EnvelopeJson`. So a malformed JSON body answers outside the
error contract on seven routes — the same class of hole as D5.1's `Path<Uuid>` finding, and at
comparable scale.

**It is deliberately NOT fixed here** (see Out of scope). Closing it changes the status code and
body shape of seven public endpoints, which no acceptance criterion of this ticket asks for and
which deserves its own review — the same reasoning that defers the HTTP `field` key. AC-1's
"malformed timestamp" is already satisfied on both transports by the query-param surfaces
(`from`/`to`, `parked_from`/`parked_to`), which do yield `invalid-timestamp`. The divergence
table therefore asserts only the gRPC half and says so.

### D6 — The gRPC `oneof` site is `missing-required-field`, not a conflict

`grpc/tenancy.rs:611-615` matches a proto3 `oneof` (`list_memberships_request::Filter`). A
`oneof` **cannot** carry two values on the wire, so its `None` arm means *neither field set* —
which is `missing-required-field`. Today's message there ("provide exactly one of
principal_prn|node_prn") is already misleading for that reason.

`http/memberships.rs:93-97` uses `match (q.principal, q.node)` with a `_ =>` catch-all that
folds `(None, None)` and `(Some, Some)` into one arm — a catch-all in miniature, of exactly
the kind this ticket exists to remove. It is split:

```rust
(None, None)         => MissingRequiredField("principal|node"),
(Some(_), Some(_))   => MutuallyExclusiveFields("principal|node"),
```

Consequently **`mutually-exclusive-fields` has exactly one emitter and no gRPC counterpart**,
because the gRPC wire format makes the failure impossible. That asymmetry is structural rather
than accidental, and is stated in the proto comment so a future reader does not "fix" it.

### D7 — Present-but-empty is normalised to absent

`?principal_prn=` yields `Some("")`, which today falls through to a PRN parse and returns
`invalid-prn`. `MembershipQuery` (`http/dto.rs:181-186`) has no `opt_non_empty` filter, unlike
every other query struct in the module. The three `missing-required-field` sites and
`MembershipQuery` gain that normalisation, so present-but-empty and absent yield the same
reason. This is not incidental tidying: without it, D5's repair #2 would make the gRPC side
(where empty *is* the unset sentinel) disagree with HTTP again.

## The vocabulary

Appended to `contracts/proto/paigasus/common/v1/error.proto` at numbers 33–38 — the next free
values in the IAM 1–299 range (current IAM max is 32, `error.proto:138`) — under a new
`// ---- IAM: request validation (1-299)` sub-banner. Sub-banners carry no numbering meaning
per the file's own rule, so this does not disturb existing values.

| Proto value | Wire reason | Emitters |
|---|---|---|
| `ERROR_REASON_INVALID_TIMESTAMP = 33` | `invalid-timestamp` | 3 helpers, both transports |
| `ERROR_REASON_INVALID_UUID = 34` | `invalid-uuid` | 4 gRPC + HTTP path extractor (D5.1) |
| `ERROR_REASON_INVALID_CURSOR = 35` | `invalid-cursor` | 4, both transports |
| `ERROR_REASON_INVALID_AUDIT_OUTCOME = 36` | `invalid-audit-outcome` | 2, both transports |
| `ERROR_REASON_MISSING_REQUIRED_FIELD = 37` | `missing-required-field` | 3 HTTP + 1 gRPC oneof (D6) + 3 gRPC twins (D5.2) |
| `ERROR_REASON_MUTUALLY_EXCLUSIVE_FIELDS = 38` | `mutually-exclusive-fields` | 1, HTTP only (D6) |

Two names deviate from the ticket's candidates, deliberately:

- **`mutually-exclusive-fields`, not `conflicting-fields`.** Every other `*conflict*` reason in
  the registry (`slug-conflict`, `email-conflict`, `policy-conflict`,
  `service-account-name-conflict`, `duplicate-membership`) is `ErrorClass::Conflict` → HTTP 409
  / gRPC `ALREADY_EXISTS` (`application/error.rs:168`). A "conflict"-named reason that is a 400
  would be the only one, and an SDK author pattern-matching on the word would mis-bucket it.
- **`invalid-audit-outcome`, not `invalid-outcome`.** `outcome` is already an overloaded token
  here — a Prometheus label on `iam_authz_generation_rewinds_total`, and the unrelated
  `RetireOutcome` domain type. The longer name matches the specificity of `invalid-bulk-replay`
  and `invalid-pagination`.

## The enum

Six variants added to `TenancyError`, all classified `ErrorClass::Validation` — which is what
the sites already produce today via `InvalidPrn`, so HTTP stays 400 and gRPC stays
`InvalidArgument`. No status-code change is intended or acceptable.

```rust
#[error("invalid timestamp for {0}")]
InvalidTimestamp(&'static str),
#[error("{0} must be a uuid")]
InvalidUuid(&'static str),
#[error("{0} is not a valid pagination cursor")]
InvalidCursor(&'static str),
#[error("{0} is not a known audit outcome")]
InvalidAuditOutcome(&'static str),
#[error("{0} is required")]
MissingRequiredField(&'static str),
#[error("provide exactly one of {0}")]
MutuallyExclusiveFields(&'static str),
```

`strum::EnumIter` (derived under `cfg(test)`) requires each field type to implement `Default`;
`&'static str` does, so the derive keeps working and the AC-4 membership test picks the new
variants up automatically.

A `field(&self) -> Option<&'static str>` accessor is added alongside `code()` and `class()`,
returning the payload for the six new variants and `None` otherwise. `status_to_grpc` uses it
to populate `ErrorInfo.metadata["field"]` (D2) without matching on variants at the transport
layer.

## Site migration

Field-name literals are the wire field names on each transport, except the four `invalid-uuid`
path params — see the note below that table.

### `invalid-timestamp`

| Site | Field literal |
|---|---|
| `grpc/convert.rs::parse_opt_ts` | parameter, already threaded |
| ← `grpc/audit.rs:95,96` | `"from"`, `"to"` |
| ← `grpc/dead_letters.rs:91,92,107,108` | `"parked_from"`, `"parked_to"` |
| ← `grpc/service_accounts.rs:184` | `"expires_at"` |
| `http/audit.rs::parse_ts` ← `:101,102` | `"from"`, `"to"` |
| `http/dead_letters.rs::parse_ts` ← `:86,87,103,104` | `"parked_from"`, `"parked_to"` |

`parse_opt_ts`'s `field: &str` narrows to `&'static str`; every existing caller already passes
a literal. Both HTTP `parse_ts` helpers gain a `field: &'static str` parameter — today they
take none, so they cannot name which bound failed.

### `invalid-uuid`

`grpc/tenancy.rs:592` (`"membership_id"`), `grpc/authz.rs:217` (`"role_grant_id"`),
`grpc/service_accounts.rs:213` (`"api_key_id"`), `grpc/dead_letters.rs:114`
(`"dead_letter_id"`), plus the HTTP path extractor from D5.1 at all 26 path-param sites, each
carrying a literal naming what the segment is (`"organization_id"`, `"team_id"`,
`"project_id"`, `"service_account_id"`, `"api_key_id"`, `"membership_id"`, `"role_grant_id"`,
`"dead_letter_id"`).

These four use a *descriptive* name rather than the bare wire field name, which is `id` on all
four RPCs. The existing payloads already say "membership id must be a uuid" / "role grant id
must be a uuid" / "api key id must be a uuid"; collapsing them all to `"id"` would make the new
message strictly less informative than the string it replaces, on a change whose purpose is to
make messages worth reading.

### `invalid-cursor`

`grpc/audit.rs:74`, `grpc/dead_letters.rs:79`, `http/audit.rs:61`,
`http/dead_letters.rs:76` — all `"cursor"`.

### `invalid-audit-outcome`

`grpc/audit.rs:64`, `http/audit.rs:52` — both `"outcome"`.

### `missing-required-field`

- HTTP: `http/service_accounts.rs:71` (`"owner_prn"`), `http/api_keys.rs:85` (`"scope_prn"`),
  `http/authz.rs:145` (`"principal_prn"`), plus `http/memberships.rs`'s new `(None, None)` arm
  (`"principal|node"`, D6).
- gRPC: `grpc/tenancy.rs:614` (`"principal_prn|node_prn"`, D6), plus the three new
  empty-string checks at `grpc/authz.rs:236`, `grpc/service_accounts.rs:142` and `:177`
  (D5.2).

### `mutually-exclusive-fields`

`http/memberships.rs`'s new `(Some, Some)` arm (`"principal|node"`). No gRPC emitter, by D6.

### Sites that keep `InvalidPrn`

`application/roles.rs:59,74`, `application/memberships.rs:24,35`,
`grpc/convert.rs:160,162` (`node_uuid`), `grpc/authz.rs:68`, `grpc/tenancy.rs:81,595`,
`grpc/service_accounts.rs:71,82,84`, `http/authz.rs:72`, `http/api_keys.rs:62`,
`http/memberships.rs:52,85`, `http/service_accounts.rs:51`. All are genuine `Prn::parse`
failures or wrong-service/wrong-type PRNs. This list is AC-2's evidence.

## Registry mechanics

Three coupled edits; a partial change reds CI rather than passing silently.

1. `contracts/proto/paigasus/common/v1/error.proto` — six values, each with the comment
   repeating its wire literal verbatim, per the file's stated convention. The
   `mutually-exclusive-fields` comment records that it has no gRPC emitter and why (D6).
2. `rs/crates/libs/paigasus-proto/src/error.rs` — six strings appended to
   `EXPECTED_REASONS`. `check.py` treats this and its own proto parse as **independent
   transcriptions** and cross-checks them, so both must move.
3. Same file — `assert_eq!(actual.len(), 46, "the registry should hold 46 reasons")` becomes
   **52**. This is the single easiest step to miss.

Then `buf format -w` followed by `buf generate`, both run from `contracts/`. A comment-and-constant change shifts the
embedded `FILE_DESCRIPTOR_SET`, so all three generated trees (Rust, Python, TypeScript) move
and must be committed, or the codegen-drift gate reds.

The gateway needs no change: its only cross-service reason match is
`paigasus-gateway/src/adapters/http/auth.rs:228`, which matches `identity-not-provisioned`.

## Module visibility

The parity guard lives in `grpc/convert.rs` and must reach both transports' helpers.
`grpc::audit`, `grpc::dead_letters` and `grpc::convert` are already `pub mod`
(`adapters/grpc/mod.rs:14,18`), so only their `fn`s widen to `pub(crate)`. On the HTTP side
`mod audit;`, `mod dead_letters;` and `mod memberships;` (`adapters/http/mod.rs:19,24,26`) are
**private modules**, so a `pub(crate) fn` inside them is still unreachable — those three
modules widen to `pub(crate) mod` as well. (The existing drift guard works only because
`pub mod dto;` at `:25` is already public.)

## Testing

### AC-3 — the transport parity guard

A table test in `grpc/convert.rs`'s test module, alongside the existing
`dead_letter_entry_projects_identically_for_http_and_grpc` drift guard, in two parts.

**Part 1 — agreement.** Drives both transports' request-conversion entry points with
equivalent malformed input and asserts a single expected `ErrorReason`:

| Logical failure | HTTP driver | gRPC driver | Expected |
|---|---|---|---|
| malformed audit timestamp | `http::audit::to_filter` | `grpc::audit::to_filter` | `InvalidTimestamp` |
| malformed audit cursor | `http::audit::to_filter` | `grpc::audit::to_filter` | `InvalidCursor` |
| unknown audit outcome | `http::audit::to_filter` | `grpc::audit::to_filter` | `InvalidAuditOutcome` |
| malformed dead-letter timestamp | `http::dead_letters::to_filter` | `grpc::dead_letters::to_filter` | `InvalidTimestamp` |
| malformed dead-letter cursor | `http::dead_letters::to_filter` | `grpc::dead_letters::to_filter` | `InvalidCursor` |
| missing required field | `http::memberships` `(None, None)` | `grpc::tenancy` oneof `None` | `MissingRequiredField` |

Driving `to_filter` rather than the raw `parse_*` helpers is deliberate: it proves the helper
is still *wired in*, which is the failure mode SMA-583 actually hit.

**Part 2 — recorded divergence.** One row per site where the transports intentionally differ,
asserting *both* sides' reasons explicitly so a change breaks the test rather than slipping
through:

| Case | HTTP | gRPC | Why |
|---|---|---|---|
| `IssueApiKey.expires_at` malformed | `invalid-request-body` | `invalid-timestamp` | D5, accepted |
| `(Some, Some)` membership filter | `mutually-exclusive-fields` | *unreachable* | D6, structural |

### Tests that must change, not merely be added

**Unit tests in `src/`:**

- `grpc/convert.rs::parse_opt_ts_detail_is_swallowed_by_invalid_prns_static_display` pins the
  exact behaviour being removed. It is **replaced** by its inverse, asserting the field name
  now surfaces in `Display`.
- `grpc/audit.rs:210,220,252,265`, `grpc/dead_letters.rs:250,258,259,310,318`,
  `http/audit.rs:198,208,218`, `http/dead_letters.rs:220,227` each carry
  `matches!(err, TenancyError::InvalidPrn(_))` assertions that must be retargeted to the new
  variants.
- `application/error.rs::error_classes_are_correct` gains the six new variants.
- `http/error.rs::the_error_object_key_set_is_unchanged` is **unchanged** — D2 deliberately
  keeps HTTP's envelope at `{code, message}`.

**Integration tests under `tests/`** — these assert on runtime JSON, so they are a
compile-time pass and a test-time red, and `check.py`'s `SCAN_GLOB = "**/src/**/*.rs"` does not
scan them. They are also behind `support::start_migrated_postgres()`, so a Docker-less run
reports them quietly while CI reds:

- `tests/http_memberships.rs:150` — `invalid-prn` → `missing-required-field` (the
  neither-set case).
- `tests/http_memberships.rs:156` — `invalid-prn` → `mutually-exclusive-fields` (the both-set
  case). Its comment at `:144` and `:152` must change too.
- `tests/http_audit.rs:109` — `invalid-prn` → `invalid-cursor`.
- `tests/http_authz.rs:222` is **not** affected: its `"principal_prn": "not-a-prn"` reaches the
  genuine PRN parser at `http/authz.rs:72`, not the required-field check at `:145`. Stated here
  so it is not "fixed" by mistake.

### Comments that must change

SMA-583's D2 set the precedent that stale prose in this area gets fixed alongside the code, not
left. Every migrated site carries a doc comment asserting "there is no dedicated error code for
X", or naming `InvalidPrn`/`invalid-prn` outright:

`grpc/convert.rs:216-217`, `grpc/audit.rs:56-59` and `:67-69`, `grpc/dead_letters.rs:73-74`,
`grpc/tenancy.rs:584-587`, `grpc/authz.rs:214-216`, `grpc/service_accounts.rs:178-183`,
`http/audit.rs:47-48,57,66-67`, `http/dead_letters.rs:62-63`, `http/memberships.rs:6-7`,
`http/service_accounts.rs:49`, and `http/dto.rs:179,364,414,457`.

### Tests needing no change

`grpc/convert.rs::every_tenancy_code_is_declared_in_the_canonical_registry` (AC-4) iterates
via `strum::EnumIter`, so it covers the new variants automatically. That is by design — there
is no second list to leave un-extended.

## Compatibility

`buf breaking` is clean and stays clean: six added enum values are additive, and nothing is
removed or renumbered (AC-5).

**But `buf breaking` is not the relevant guarantee.** This change alters the wire `reason` for
18 already-mounted failure modes, and `error.proto:21-25` says explicitly that `buf breaking`
cannot see the kebab strings. The registry's append-only rule covers *removal* of values, not
*reassignment* of which reason a given failure emits. For a consumer branching on `reason`,
this is a breaking change.

It is taken deliberately and now because there are no external consumers: the only
cross-service reason match in the tree is `paigasus-gateway/.../auth.rs:228`
(`identity-not-provisioned`, untouched), and `@paigasus/sdk` (SMA-508) has not shipped. Doing
this before SMA-508 costs one migration; doing it after costs that migration plus a rewrite of
the SDK's mapping table and its tests.

Two follow-ups are noted rather than folded in:

- Extending the HTTP error envelope with an optional `field` key, so HTTP reaches parity with
  gRPC's `metadata["field"]` (D2).
- Updating ADR-0019's vocabulary section in Notion with the six new reasons. The repo-side test
  `paigasus-proto/src/error.rs::the_adr_examples_are_spelled_as_documented` pins only the
  ADR's *example* spellings, which are unchanged, so nothing reds — this is a docs task.

## Verification

Per CLAUDE.md, per-project Moon tasks do **not** run the repo-level gates, so the full graph
is run as CI does:

```
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site
  :input-liveness :promtool :observability-drift :nats-permissions :release-parity
  :release-parity-py :release-parity-ts :publish-metadata :version-lockstep
  --base origin/main --include-relations
```

Gate-specific expectations:

- **`:breaking`** — clean, additive only (AC-5).
- **`:error-code-single-site`** — green with no MANIFEST change expected: the six codes appear
  as literals only in `application/error.rs`, already an `emits` row, and the parity table uses
  `ErrorReason` values rather than strings — which keeps `grpc/convert.rs`'s existing `asserts`
  row honest. If the gate demands a row anyway, add it with a stated reason rather than working
  around it.
- **`:affected-smoke`** — no new crate and no new in-tree dep, so no expected-set re-baselining.

**Codegen drift is a separate step**, not part of that command: it is a standalone `ci.yml`
step (`.github/workflows/ci.yml:249-262`) and is deliberately absent from the `T=(…)` array. It
is verified by hand, mirroring ci.yml's shape:

```
(cd contracts && buf format -w && buf generate)
git add --intent-to-add . && git diff --exit-code
```

buf runs from `contracts/`: there is no root-level `buf.gen.yaml`, and that is the cwd Moon's
`contracts:generate` task uses.

Note `contracts:generate` has no `outputs:` declared (`contracts/moon.yml:7-8`), so a Moon run
can serve stale cached output — run `buf generate` directly.

Integration tests need Docker (`PAIGASUS_REQUIRE_DOCKER=1` for any filtered run, since the
`docker_preflight` canary is not in a filter).

## Out of scope

- Whether `Display` should carry interpolated detail *in general* — decided per-variant here
  (D2) for the six new variants only, and explicitly not changed for `InvalidPrn`.
- The gateway's own error vocabulary.
- Any change to HTTP or gRPC status codes. All six are `Validation`, exactly as today.
- Extending the HTTP error envelope with a `field` key (follow-up, § *Compatibility*).
- Switching the seven plain-`axum::Json` routes to `EnvelopeJson` so a malformed body answers
  inside the error contract (follow-up — see the correction under D5). Discovered while writing
  the AC-3 guard; real, pre-existing, and out of scope for a taxonomy change.
