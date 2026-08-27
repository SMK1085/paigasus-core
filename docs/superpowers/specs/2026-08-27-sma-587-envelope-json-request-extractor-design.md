# SMA-587 — `EnvelopeJson` at every IAM request extractor

**Issue:** [SMA-587](https://linear.app/smaschek/issue/SMA-587) — *iam: fourteen HTTP routes answer a malformed JSON body outside the error envelope*
**Date:** 2026-08-27
**Status:** Drafted

## Problem

`adapters::http::authn::EnvelopeJson` exists so a body axum refuses is rejected inside IAM's
stable `{"error":{code,message}}` envelope with a registered `reason`. **Only two request
extractors use it**: `api_keys::introspect` and `system_retirement::retire`.

The other **fourteen** take plain `axum::Json<T>`, so axum's own rejection escapes — a
plain-text body carrying no `error.code` at all. The route answers outside its own error
contract:

| File:line | Handler | Body type |
| -- | -- | -- |
| `api_keys.rs:82` | `issue` | `IssueApiKeyBody` |
| `authz.rs:85` | `is_authorized` | `IsAuthorizedBody` |
| `authz.rs:107` | `put_policy` | `PutPolicyBody` |
| `authz.rs:137` | `create_role_grant` | `GrantRoleBody` |
| `dead_letters.rs:140` | `replay_matching` | `BulkReplayBody` |
| `memberships.rs:68` | `create_membership` | `CreateMembershipBody` |
| `organizations.rs:52` | `create_org` | `CreateNodeBody` |
| `organizations.rs:80` | `rename_org` | `RenameBody` |
| `organizations.rs:114` | `create_team` | `CreateNodeBody` |
| `projects.rs:41` | `rename_project` | `RenameBody` |
| `service_accounts.rs:63` | `create` | `CreateServiceAccountBody` |
| `teams.rs:48` | `rename_team` | `RenameBody` |
| `teams.rs:78` | `create_project` | `CreateNodeBody` |
| `users.rs:59` | `create_user` | `CreateUserBody` |

SMA-504 put `reason` on the wire as the machine-readable identity clients branch on, and
SMA-508's `@paigasus/sdk` (AC2) must branch on `(domain, reason)` only, never message text. On
these fourteen there is no `reason` to branch on — the SDK gets a plain-text body it cannot
parse into its error type at all. This is the same class of hole as SMA-586's `Path<Uuid>`
finding, and larger.

### A correction to the ticket's framing

The ticket says "status code and body shape both move". **Only the body moves.**
`authn::envelope_rejection` renders with `rejection.status()`, so 400/415/422/413 are preserved
exactly as axum produces them today. That materially narrows the compatibility surface (see
§ *Compatibility*) and is why no route's status changes anywhere in this work.

## Acceptance criteria

| AC | Satisfied by |
| -- | -- |
| **AC-1** — all fourteen routes answer a refused body inside `{"error":{code,message}}` with a registered reason | D3 (the swap) + D5 (real-router coverage) |
| **AC-2** — the reason distinguishes the failure kinds rather than re-installing a catch-all | D1 (four reasons, two of them new) |
| **AC-3** — the extractor is not owned by a handler module | D2 (`adapters::http::json`) |
| **AC-4** — a fifteenth bare `Json<T>` request extractor cannot land silently | D4 (`repo:envelope-json-single-site`) |
| **AC-5** — every reason resolves via `ErrorReason::from_wire_reason`; `repo:error-code-single-site` green; `buf breaking` clean | D1 + § *Verification* |

## Scope

The fourteen request extractors above, the extractor's home and its rejection taxonomy, two
additive registry values, and one new CI gate. Request **body limits** and the **gateway** are
out of scope (§ *Out of scope*).

## Design decisions

### D1 — Four reasons where there were two

`envelope_rejection` today folds `JsonSyntaxError`, `MissingJsonContentType` and `JsonDataError`
into a single `invalid-request-body`, branching only on `status() == PAYLOAD_TOO_LARGE` to peel
off `request-too-large`. That is tolerable on two low-traffic routes. Rolling it onto most of
IAM's write surface would install exactly the catch-all sentinel SMA-586 exists to remove — one
code standing for three unrelated failures, on fourteen public endpoints, right before the SDK
starts branching on it.

`RejectionKind` therefore grows to four arms:

| axum rejection | status | reason | registry |
| -- | -- | -- | -- |
| `JsonSyntaxError` | 400 | `invalid-request-body` | 901, existing — **narrowed** to mean *syntax* |
| `MissingJsonContentType` | 415 | `unsupported-media-type` | **905, new** |
| `JsonDataError` | 422 | `invalid-request-schema` | **906, new** |
| length-limited body | 413 | `request-too-large` | 902, existing |

Both new values are additive entries in `contracts/proto/paigasus/common/v1/error.proto`'s
900-block (the framework/transport family), plus the hand-transcribed `EXPECTED_REASONS` mirror
in `rs/crates/libs/paigasus-proto/src/error.rs`. The two transcriptions are independent by
design — `ci/error-registry/check.py` cross-checks them, and they can only agree if both are
right.

#### D1.1 — The classification rule is hybrid, and deliberately so

The obvious refactor — replace the status sniff with a clean `match` on `JsonRejection`'s
variants — **is wrong**, for two reasons found in the axum 0.8.9 / axum-core 0.5.6 source:

1. **`JsonRejection::BytesRejection` is not always 413.** It wraps `FailedToBufferBody`, itself a
   composite of `LengthLimitError` (413) *and* `UnknownBodyError` (400). Mapping that variant
   straight to `request-too-large` would render a `request-too-large` code on a 400 response —
   a body that contradicts its own status line. The existing status-based branch is *correct* on
   this point and must survive the refactor.
2. **`JsonRejection` is `#[non_exhaustive]`** (the `__composite_rejection!` macro stamps it), so a
   fallback arm is mandatory. Leaving it to fall into `invalid-request-body` unconditionally
   would mean a future axum variant carrying a 413 or 415 gets a code contradicting its status,
   silently, on an axum bump.

The rule is therefore: **match the variant where the variant determines the status; dispatch on
`rejection.status()` everywhere else.**

```
JsonSyntaxError          -> Invalid              (400, exactly one status)
MissingJsonContentType   -> UnsupportedMediaType (415, exactly one status)
JsonDataError            -> InvalidSchema        (422, exactly one status)
_ (BytesRejection, and any future variant)
    -> by status: 413 -> TooLarge
                  415 -> UnsupportedMediaType
                  422 -> InvalidSchema
                  _   -> Invalid
```

The fallback arm is not a rubber stamp: it is the *only* thing standing between an axum bump and
a status/code contradiction, and it carries a test per branch.

#### D1.2 — Two consequences stated rather than discovered

- **This also narrows the two routes already on `EnvelopeJson`.** `api_keys::introspect` and
  `system_retirement::retire` today answer 415 and 422 with `invalid-request-body`; after this
  they answer `unsupported-media-type` and `invalid-request-schema`. That is intended — one
  extractor, one taxonomy — but it means the wire change is sixteen routes, not fourteen. Any
  existing assertion on those two routes' codes must be updated, not deleted.
- **error.proto's 901 comment stops being true.** It reads "covers IAM's `invalid_request`
  extractor rejection and the gateway's `invalid_request_body`, merged". IAM's half is now
  narrower than the gateway's, which keeps emitting 901 from its own `Bytes` funnel
  (`gateway/src/adapters/http/error.rs:130`) for any deserialization failure. The comment is
  reworded to say so explicitly rather than left to imply a symmetry that no longer holds.

### D2 — `EnvelopeJson` moves to `adapters::http::json`

`EnvelopeJson` lives in `adapters::http::authn`, is `pub(crate)`, and its doc comment names its
two users individually — a shape that made sense when there were two. After D3 it is the house
request extractor for every write route in the service, and a reader looking for it has no
reason to open a module named for authentication.

It moves to a new `adapters::http::json`, a sibling of `path.rs`. This is precedent, not
invention: SMA-586 put `UuidPath` in its own neutral `path.rs` rather than leaving a
cross-cutting extractor inside a handler module. The result is symmetric — one extractor module
per input kind, neither owned by a handler:

```
adapters/http/
  authn.rs   AuthnApiError + BEARER_CHALLENGE + the introspect route
  path.rs    UuidPath / UuidPathPair          (SMA-586)
  json.rs    EnvelopeJson / RejectionKind      (SMA-587)
```

Moving with it: `RejectionKind`, `envelope_rejection`, both the `FromRequest` and
`OptionalFromRequest` impls, and the extractor's unit tests. `authn.rs`'s
`every_authn_http_code_is_in_the_registry` test **splits** — its `RejectionKind` half follows the
enum to `json.rs`, its `AuthnError` half stays. Splitting rather than moving matters: that test
is `authn.rs`'s entry on `ci/error-registry/check.py`'s `MANIFEST`, and the manifest requires a
membership test in *each* file that spells a registry code. `json.rs` becomes a new manifest
entry with the `RejectionKind` half as its required test; `authn.rs` keeps its own.

### D3 — The swap

Fourteen mechanical signature edits at the sites tabulated under § *Problem*:

```rust
-async fn create_org(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, Json(b): Json<CreateNodeBody>)
+async fn create_org(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, EnvelopeJson(b): EnvelopeJson<CreateNodeBody>)
```

No handler body changes. Return-position `Json<Dto>` is untouched everywhere — it is a response
type and has nothing to do with this. Extractor ordering already holds: `Json` is `FromRequest`
and is last in every one of the fourteen signatures, and `EnvelopeJson` inherits that position.

### D4 — `repo:envelope-json-single-site`

Nothing stops a fifteenth handler being written with bare `Json<T>` tomorrow, and this is the
*second* time this class of hole has been found (SMA-586's `Path<Uuid>` was the first). The house
answer is a gate: `repo:redis-connect-single-site`, `repo:error-code-single-site` and
`repo:iam-docker-policy-single-site` are all this same shape.

`ci/envelope-json/` fails when a function signature under
`rs/crates/services/*/src/adapters/http/**/*.rs` binds `Json<…>` in **request** position.

- **Scoped services-wide, not iam-only.** The gateway is clean today — it takes `Bytes` and maps
  failures in its own funnel — so the gate is green there on day one and stops it growing the
  same hole. An iam-only glob would leave the gateway unguarded for no saving.
- **Request vs response position** is the whole difficulty. A binding pattern (`Json(x): Json<T>`
  in an argument list) is banned; `-> Result<Json<Dto>, ApiError>` and `Ok(Json(...))` are not.
  The discriminator is the argument-list context, not the token `Json`.
- **`ALLOW` table** with a stated reason per entry, matching the other gates' escape-hatch
  convention. Empty at merge.
- **Self-tests and a negative control**, per `ci/release-parity`'s precedent: the gate must be
  shown to red on a planted violation, under an explicit `set -euo pipefail`, or it proves
  nothing. Guard-the-guard applies — the control's own *call site* is pinned, not just its
  verdict function, since a fixture table exercises the verdict and never its invocation.
- **Plumbing** (all mandatory, none optional): the task joins `ci.yml`'s `T=(…)` array **and**
  CLAUDE.md's marker-delimited command — `ci/affected-graph/ci_targets.py` asserts the two agree,
  and `moon ci` exits 0 on a target resolving to nothing, so a typo is otherwise a silent no-op
  on every PR. It needs a `SELF_TASK_EXPECTED_GLOBS` entry listing its literal `inputs`, and its
  `moon.yml` lines pinned in `SELF_SCHEDULED_GATES`. Every declared input must match a tracked
  file or `repo:input-liveness` reds.

### D5 — Coverage on the real router

The gate proves no bare `Json` remains. It does **not** prove a route answers in the envelope —
that needs the route to be reachable, authorized, and wired to the extractor. SMA-586 learned
this the expensive way: its extractor's unit tests built a synthetic
`Router::new().route("/x/{id}", …)`, which proved the extractor but not the handler-to-marker
wiring, and that is exactly how a mis-named `{sa}` segment survived the entire suite until the
second fix round.

So coverage is table-driven against the **real merged `router(...)`**, in the six existing
Docker-backed suites, following `http_tenancy.rs:279`'s established shape:

| Suite | Routes covered |
| -- | -- |
| `http_tenancy.rs` | create org, rename org, create team, rename team, create project, rename project |
| `http_users.rs` | `create_user` |
| `http_memberships.rs` | `create_membership` |
| `http_authz.rs` | `is_authorized`, `put_policy`, `create_role_grant` |
| `http_dead_letters.rs` | `replay_matching` |
| `http_service_accounts.rs` | `create`, api-key `issue` |

Each of the fourteen asserts the envelope for **syntax (400 `invalid-request-body`)** and
**schema (422 `invalid-request-schema`)**. **Content-type (415 `unsupported-media-type`)** is
asserted once per suite rather than fourteen times: it is refused before any handler-specific
code runs, so per-route repetition would add rows without adding a distinct assertion — stated
here so the asymmetry reads as a decision rather than an omission.

Each table ends with a **well-formed body on the same route** reaching the handler, so every row
above is an assertion about the body's shape and not about the route being broken.

`json.rs` carries unit tests per `RejectionKind` arm, including one per fallback branch (D1.1) —
the fallback is the arm with no compiler pressure behind it.

These suites are Docker-backed and inherit the existing policy in `tests/support/docker.rs`: they
skip when the daemon is unreachable, with `tests/docker_preflight.rs` as the canary that turns a
Docker-less run into one loud red rather than silent passes.

## Out of scope

- **Request body limits.** The fourteen inherit axum's 2 MB default; only the authn router sets an
  explicit `DefaultBodyLimit`. After the swap a >2 MB body answers 413 `request-too-large` in the
  envelope, which is the improvement. *Choosing* per-route limits is a sizing and DoS-posture
  question with no acceptance criterion here, and lowering one is the single change in this area
  that would genuinely move a status code (413 where 200 used to be). Deferred deliberately.
- **The gateway's `invalid-request-body`.** It parses `Bytes` itself and has no bare-`Json`
  request extractor, so it needs no swap. Whether its funnel should also split 415/422 is a
  separate question about a separate code path; D1.2 records that the two services' 901 meanings
  now differ.
- **An HTTP `field` key in the error envelope.** Deferred by SMA-586 for its own reasons; nothing
  here changes that.

## Compatibility

- **Statuses are unchanged on every route** (§ *A correction to the ticket's framing*).
- **Bodies change on sixteen routes** — the fourteen swapped, plus the two already on
  `EnvelopeJson` whose 415/422 codes narrow (D1.2). Each moves from plain text (or a broad code)
  to a strictly more specific registered reason. No client can regress by gaining an `error.code`
  where it previously had unparseable text; a client branching on `invalid-request-body` for a
  415/422 on the two existing routes would, which is why D1.2 names them.
- **`buf breaking` is clean.** Both registry values are additive `ErrorReason` entries with fresh
  field numbers (905, 906); nothing is renamed or renumbered.
- **Regenerated bindings** (Rust/Py/TS) ride along under the codegen-drift gate. `buf format -w`
  before commit or `contracts:fmt` reds `moon ci`.

## Verification

- `cargo nextest run -p paigasus-iam` with a reachable Docker daemon — the new rows are in
  Docker-backed suites and pass vacuously without one.
- `repo:error-code-single-site` green with `json.rs` added to the `MANIFEST` carrying its
  membership test.
- `repo:envelope-json-single-site` green, its negative control shown to red on a planted
  fifteenth site.
- `repo:affected-smoke`, `repo:input-liveness`, `repo:actionlint` green — the new gate touches
  all three (`T=(…)` parity, live inputs, `ci.yml` shape).
- The full CI graph as CI runs it, per CLAUDE.md — a new `repo:*` gate and a `contracts/` change
  both reach beyond per-project tasks.
