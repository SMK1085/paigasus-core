# SMA-587 — `EnvelopeJson` at every IAM request extractor

**Issue:** [SMA-587](https://linear.app/smaschek/issue/SMA-587) — *iam: fourteen HTTP routes answer a malformed JSON body outside the error envelope*
**Date:** 2026-08-27 (revised the same day; adversarial review folded in — see § *Review corrections*)
**Status:** Drafted

## Problem

`adapters::http::authn::EnvelopeJson` exists so a body axum refuses is rejected inside IAM's
stable `{"error":{code,message}}` envelope with a registered `reason`. **Only three request
extractors use it**: `authn::introspect` (the extractor's own original home route),
`api_keys::introspect`, and `system_retirement::retire` (corrected from "two" — see
§ *Review corrections*).

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

`adapters/grpc/convert.rs:1171-1186` already records this hole from the inside: SMA-586's
divergence test carries a twelve-line comment explaining that it cannot assert `issue`'s HTTP
half because that route "is axum's plain `Json`, not `http::authn::EnvelopeJson`, so a malformed
body never reaches the IAM error envelope or the registry at all… Flagged for a follow-up ticket
rather than fixed here." **This ticket is that follow-up**, so retiring that comment is in scope
(D6).

### A correction to the ticket's framing

The ticket says "status code and body shape both move". **Only the body moves.**
`authn::envelope_rejection` renders with `rejection.status()` (`authn.rs:119`), so 400/415/422/413
are preserved exactly as axum produces them today. That materially narrows the compatibility
surface (§ *Compatibility*) and is why no route's status changes anywhere in this work.

## Acceptance criteria

| AC | Satisfied by |
| -- | -- |
| **AC-1** — all fourteen routes answer a refused body inside `{"error":{code,message}}` with a registered reason | D3 + D5 |
| **AC-2** — the reason distinguishes the failure kinds rather than re-installing a catch-all | D1 |
| **AC-3** — the extractor is not owned by a handler module | D2 |
| **AC-4** — a fifteenth bare `Json<T>` request extractor cannot land silently | D4 |
| **AC-5** — every reason resolves via `ErrorReason::from_wire_reason`; `repo:error-code-single-site` green; `buf breaking` clean | D1 + § *Registry mechanics* + § *Verification* |
| **AC-6** — every consequence of the change is recorded where a reader will meet it: stale prose retired, transport divergence declared, superseded assertions reassigned | D6 + § *Cross-transport divergence* + § *Tests that must change* |

## Scope

The fourteen request extractors above, the extractor's home and its rejection taxonomy, two
additive registry values, one new CI gate, and the prose/assertions those invalidate. Request
**body limits**, the **gateway's own funnel**, and the **twelve non-JSON extractor escapes**
are out of scope and named there.

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
| `JsonSyntaxError` | 400 | `invalid-request-body` | 901, existing — **narrowed** to mean *malformed or unreadable* |
| `MissingJsonContentType` | 415 | `unsupported-content-type` | **905, new** |
| `JsonDataError` | 422 | `invalid-request-schema` | **906, new** |
| length-limited body | 413 | `request-too-large` | 902, existing |

#### D1.1 — The classification rule is hybrid, and the fallback is the hard part

The obvious refactor — replace the status sniff with a clean `match` on `JsonRejection`'s
variants — **is wrong**, for two reasons verified against axum 0.8.9 / axum-core 0.5.6:

1. **`JsonRejection::BytesRejection` is not always 413.** It wraps `FailedToBufferBody`, itself a
   composite of `LengthLimitError` (413) *and* `UnknownBodyError` (400)
   (`axum-core-0.5.6/src/extract/rejection.rs:8-15,40-55`). Mapping that variant straight to
   `request-too-large` would render a `request-too-large` code on a 400 response — a body
   contradicting its own status line. The existing status-based branch is *correct* here and must
   survive the refactor.
2. **`JsonRejection` is `#[non_exhaustive]`** (`axum-core-0.5.6/src/macros.rs:163-164`), so a
   fallback arm is mandatory.

The rule: **match the variant where the variant determines the status; dispatch on
`rejection.status()` everywhere else — and only for client errors.**

```rust
// envelope_rejection
match &rejection {
    JsonRejection::JsonSyntaxError(_)        => envelope(RejectionKind::Invalid, &rejection),
    JsonRejection::MissingJsonContentType(_) => envelope(RejectionKind::UnsupportedContentType, &rejection),
    JsonRejection::JsonDataError(_)          => envelope(RejectionKind::InvalidSchema, &rejection),
    // BytesRejection, and any variant a future axum adds.
    _ => match classify(rejection.status()) {
        Some(kind) => envelope(kind, &rejection),
        // Not a client error => not the caller's mistake. Hand axum's own response back
        // untouched rather than relabel a server bug as a bad request.
        None => rejection.into_response(),
    },
}

/// The status-only half, extracted so it is reachable from a unit test (D1.1a).
fn classify(status: StatusCode) -> Option<RejectionKind> {
    match status {
        StatusCode::PAYLOAD_TOO_LARGE     => Some(RejectionKind::TooLarge),
        StatusCode::UNSUPPORTED_MEDIA_TYPE=> Some(RejectionKind::UnsupportedContentType),
        StatusCode::UNPROCESSABLE_ENTITY  => Some(RejectionKind::InvalidSchema),
        s if s.is_client_error()          => Some(RejectionKind::Invalid),
        _                                 => None,
    }
}
```

**The `None` arm is the correction the first draft got wrong.** A blanket `_ => Invalid` would
render `{"error":{"code":"invalid-request-body"}}` on a **500** — precisely the status/code
contradiction this section exists to prevent, in the one arm with no compiler pressure behind it.
Handing the rejection back mirrors the sibling extractor exactly: `path.rs:87-92` gates on
`is_client_error` and returns axum's own response for the `MissingPathParams` /
`WrongNumberOfParameters` family, because (`path.rs:11-17`) those mean "the route's pattern
stopped matching its handler's arity, which is a SERVER bug. Answering that with `400
invalid-uuid` would report our mistake as the caller's."

*Rejected alternative:* map 5xx to the registered `internal` code (900), keeping every response
inside the envelope. It loses on consistency — `path.rs` already chose hand-back for the
identical question nine days ago, and two extractors answering server bugs differently is worse
than one plain-text 500 on a hypothetical future axum variant.

#### D1.1a — `classify` is a free function so the fallback is testable

The fallback arms are **unreachable from a unit test** by construction:
`__define_rejection!` gives each rejection a `pub(crate)` field and a `pub(crate) fn from_err`
(`axum-core-0.5.6/src/macros.rs:98-101`), and `JsonRejection` / `BytesRejection` /
`FailedToBufferBody` are all `#[non_exhaustive]` — no `BytesRejection` can be constructed outside
axum. A spec promising "one test per fallback branch" against the `match` above would be
promising coverage the shape cannot deliver, and the implementation reviewer would accept tests
that exercise only the three variant arms.

Splitting the status-only half into a pure `classify(StatusCode) -> Option<RejectionKind>` makes
the promise keepable: it is unit-tested directly over 400, 413, 415, 422 and 500 — including the
`None` arm. `envelope_rejection` is then variant-match-then-`classify`, and the 413 path is
additionally exercised end-to-end through a real `Router` + `DefaultBodyLimit` round-trip, which
is the only way to reach a genuine `LengthLimitError`.

#### D1.2 — Two consequences stated rather than discovered

- **This also narrows the three routes already on `EnvelopeJson`.** `authn::introspect`,
  `api_keys::introspect`, and `system_retirement::retire` today answer 415 and 422 with
  `invalid-request-body`; after this they answer `unsupported-content-type` and
  `invalid-request-schema`. Intended — one extractor, one taxonomy — but the wire change is
  **seventeen** routes, not fourteen (corrected from "sixteen" — see § *Review corrections*), and
  it supersedes a live assertion (§ *Tests that must change*).
- **error.proto's 901 comment stops being true.** It reads "covers IAM's `invalid_request`
  extractor rejection and the gateway's `invalid_request_body`, merged". IAM's half is now
  narrower than the gateway's, which keeps emitting 901 from its own `Bytes` funnel
  (`gateway/src/adapters/http/error.rs:130`) for any deserialization failure. The comment is
  reworded to state the asymmetry rather than imply a symmetry that no longer holds. Whether the
  gateway should adopt the same split is a follow-up (§ *Out of scope*).

#### D1.3 — Naming and range, considered

`unsupported-content-type`, **not** `unsupported-media-type`. The latter is verbatim HTTP 415's
status phrase and would be the only reason in the registry named after a status line; the former
names the thing that actually failed — the `Content-Type` header. This does not run afoul of
SMA-586 D1's rejection of `invalid-argument`: that name lost for being *broad enough to
re-accrete a catch-all*, which neither new reason is — each denotes exactly one condition.

Both land in the **900 range** ("shared — any domain may emit", `error.proto:32`) rather than
IAM's 1-299. They are transport-level facts about an HTTP request, not IAM domain failures, and
the gateway could adopt the same extractor tomorrow. That the gateway does not emit them *today*
does not make the range wrong — 901 and 902 sit there for the same reason and 902 likewise has
one emitter.

`invalid-request-body` is **not** renamed to something more distinguishable from
`invalid-request-schema` (e.g. `malformed-request-body`), despite the two sitting adjacent in the
SDK's mapping table: 901 is already on the wire from the gateway and renaming it is a breaking
change for no functional gain.

### Registry mechanics

Adding a reason touches **three** coupled sites, not two. SMA-586's spec called the third "the
single easiest step to miss" and this design initially missed it:

1. `contracts/proto/paigasus/common/v1/error.proto` — two additive `ErrorReason` values, 905 and
   906, each with a comment carrying the wire string, its meaning, and its transport availability
   (§ *Cross-transport divergence*).
2. `rs/crates/libs/paigasus-proto/src/error.rs` — the hand-transcribed `EXPECTED_REASONS` list.
   Independent by design: `ci/error-registry/check.py` cross-checks the two transcriptions, which
   can only agree if both are right.
3. `rs/crates/libs/paigasus-proto/src/error.rs:224` — `assert_eq!(actual.len(), 52, "the registry
   should hold 52 reasons")` becomes **54**. Missing this fails
   `the_registry_contains_exactly_the_expected_reasons` in `paigasus-proto`, *not* in the crate
   under change, so it reads as an unrelated breakage.

### D2 — `EnvelopeJson` moves to `adapters::http::json`

`EnvelopeJson` lives in `adapters::http::authn`, is `pub(crate)`, and its doc comment names its
two users individually — a shape that made sense when there were two. After D3 it is the house
request extractor for every write route, and a reader looking for it has no reason to open a
module named for authentication.

It moves to a new `pub(crate) mod json`, a sibling of `path.rs` (which is also `pub(crate)`,
`mod.rs:34`). Precedent, not invention: SMA-586 put `UuidPath` in its own neutral module rather
than leaving a cross-cutting extractor inside a handler module.

```
adapters/http/
  authn.rs   AuthnApiError + BEARER_CHALLENGE + the introspect route
  path.rs    UuidPath / UuidPathPair          (SMA-586)
  json.rs    EnvelopeJson / RejectionKind / classify   (SMA-587)
```

Moving with it: `RejectionKind`, `envelope_rejection`, the new `classify`, both the `FromRequest`
and `OptionalFromRequest` impls, and the extractor's unit tests. Existing importers to repoint:
`api_keys.rs:132` and `system_retirement.rs:96,97,195,199,215,233,257`. These are compile errors,
so the risk is low — but D3's "fourteen mechanical edits" undercounts the diff and this states
the rest of it.

**The membership test splits — for cohesion, not because the gate demands it.** The first draft
claimed `check.py`'s MANIFEST "requires a membership test in each file that spells a registry
code". That is **false**: `guard_exists` searches tree-wide on purpose and its docstring says
"Do not 'fix' it into a per-row search" (`check.py:151-155`); the `application/error.rs` row
already names a guard defined in `grpc/convert.rs`. So one guard could serve both rows. The test
splits anyway because the `RejectionKind` half asserts things that now live in `json.rs`, and a
guard is easier to keep honest next to what it guards.

What the gate *does* require, and what this therefore entails:
- a new `MANIFEST` row for `json.rs`, role `emits`, naming its guard —
  `every_request_extractor_code_is_in_the_registry` — with a stated `why`. `emits` rows are
  exactly the rows that name a guard (`check.py:355-357`).
- `authn.rs`'s existing row keeps its guard but its `why` string, currently `"the authn funnel and
  envelope_rejection"` (`check.py:92-93`), goes stale and must be narrowed to the funnel alone.

#### D2.1 — `json.rs` and `path.rs` are deliberately *not* symmetric

D2 presents them as siblings; on one axis they are not, and that axis is the reviewable decision.
`path.rs:77-79` builds its response as `ApiError(TenancyError::InvalidUuid(field)).into_response()`
— through the tenancy funnel, with the code coming from `TenancyError::code()` — and its comment
(`path.rs:179-181`) says the absence of a literal is deliberate: a literal in a `src/` file "would
put this production module on `ci/error-registry/check.py`'s MANIFEST, blinding that gate to a
future *production* code literal in this file."

`json.rs` does the opposite: a hand-built `json!` envelope, four kebab literals in
`RejectionKind::parts()`, a hardcoded `Retryable::No`, and a MANIFEST row with exactly that
blinding effect.

**The constraint that forces it:** `EnvelopeJson` also serves `api_keys::introspect`, whose
handler returns `AuthnApiError` — a funnel deliberately separate from `ApiError`/`TenancyError`
(`authn.rs:5-9`). An extractor emitting `TenancyError` on a route whose every other failure is an
`AuthnError` would make the route's error type depend on *where in the request* it failed.

*Rejected alternative:* add `TenancyError::UnsupportedContentType` / `::InvalidRequestSchema` and
render through `ApiError`. It would keep one funnel, keep `json.rs` off the MANIFEST, and inherit
`retryable` classification for free — but it loses on the constraint above, and it would put
HTTP-transport concerns into the tenancy domain error, which gRPC also maps.

The blinding is mitigated by the guard, which enumerates `RejectionKind` via `strum::EnumIter`
rather than restating literals — the SMA-507 E3 lesson — so a fifth kind must state its parts or
fail to compile.

### D3 — The swap

Fourteen mechanical signature edits at the sites tabulated under § *Problem*:

```rust
-async fn create_org(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, Json(b): Json<CreateNodeBody>)
+async fn create_org(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, EnvelopeJson(b): EnvelopeJson<CreateNodeBody>)
```

No handler body changes. Return-position `Json<Dto>` is untouched everywhere — it is a response
type and has nothing to do with this. Extractor ordering already holds: `Json` is `FromRequest`
and is verified to be the **last** parameter in all fourteen signatures, so `EnvelopeJson`
inherits a valid position with no reordering.

**An unclaimed win, claimed:** `authn.rs:330-341` pins that the `paigasus-retryable` header is
present on *every* error response, "carrying the literal `false` where the error is not
retryable — a client must never have to read absence as `false`." The fourteen routes violate
that today, since axum's own rejection carries no such header. `envelope_rejection` inserts it
(`authn.rs:120-123`), so the swap closes that gap too. Stated so a reviewer knows it was intended
rather than incidental.

### D4 — `repo:http-extractor-envelope`

Nothing stops a fifteenth handler being written with bare `Json<T>` tomorrow, and this is the
*second* time this class of hole has been found (SMA-586's `Path<Uuid>` was the first) — with
twelve more instances already sitting in the same files (§ *Out of scope*). The house answer is a
gate: `repo:redis-connect-single-site`, `repo:error-code-single-site` and
`repo:iam-docker-policy-single-site` are all this shape.

**Named for the class, not for `Json`.** The gate carries a **banned-extractor table**, one row
per extractor type with an explicit on/off flag, so closing the `Query`/`Path<String>` instances
later flips a flag rather than designing a second gate. Only the `Json` row is switched on here;
the other two are reserved with no replacement chosen — that choice belongs to the follow-up, not
to this spec:

| extractor | banned in request position | required replacement |
| -- | -- | -- |
| `Json<T>` | yes (this ticket) | `EnvelopeJson<T>` |
| `Query<T>` | no — row reserved, off by default | decided by the follow-up |
| `Path<T>` (non-uuid) | no — row reserved, off by default | decided by the follow-up |

Scope: `rs/crates/services/*/src/adapters/http/**/*.rs`. Services-wide, not iam-only — the
gateway is clean today (it takes `Bytes` and maps failures in its own funnel), so the gate is
green there on day one and stops it growing the same hole.

#### D4.1 — The scan algorithm, stated because the discriminator is the whole difficulty

A naive line grep for `Json<` fails in four directions found in the tree. The gate extracts each
`fn` **signature** by paren-balancing from `fn <name>(`, cuts at the top-level `->`, and scans
only the parameter span:

- **Same line carries both.** `organizations.rs:80` is
  `… Json(b): Json<RenameBody>) -> Result<Json<OrgDto>, ApiError> {` — one banned binding and one
  legal return type on one line. Cutting at the top-level `->` separates them; a line-oriented
  scan cannot.
- **Multi-line signatures.** `api_keys.rs:82` and `organizations.rs:114` put each parameter on its
  own line. Paren-balancing handles both forms uniformly.
- **The non-destructuring form.** The house style already uses it for extractors —
  `system_retirement.rs:96` `body: Option<EnvelopeJson<RetireBody>>`, `organizations.rs:71`
  `path: UuidPath<OrganizationId>`. So `body: Json<CreateNodeBody>` must be banned too; the rule
  is *the type appears in the parameter span*, not *a `Json(x):` binding pattern appears*.
- **`where` clauses are a third context**, neither parameter list nor return type. `json.rs`
  itself will contain `where Json<T>: FromRequest<S, Rejection = JsonRejection>`,
  `where Json<T>: OptionalFromRequest<…>`, `match Json::<T>::from_request(…)` and
  `Ok(Json(value)) => Ok(EnvelopeJson(value))` — the extractor's own definition site. Cutting at
  the top-level `->` already excludes the `where` clause of a normal `fn`; the impl-block bodies
  are excluded by the ALLOW row below.

**The ALLOW table is therefore not empty at merge** — contrary to the first draft. It carries at
minimum `adapters/http/json.rs`, reason: *the extractor's own definition site; it wraps
`axum::Json` by construction*. Each row states a reason, per the other gates' convention.

**Residuals go in a README Limitations section**, per `ci/actionlint/README.md`'s precedent:
an aliased import (`use axum::Json as J`), a re-export renamed on the way through, and bodies
taken as `Bytes`/`String` all escape the scan. Naming them is the point — a gate that fails open
is worse than no gate (`check.py:73-77`).

*(This design predicted a fourth residual — a fully-qualified `axum::Json<T>` — that turned out
not to be one. The implemented scan matches on an identifier boundary, so `axum::Json<T>` and any
other `…::Json<T>` are caught; only a rename defeats it. Corrected against the shipped gate rather
than left as a claim the README contradicts; see `ci/http-extractor/README.md`'s L2.)*

#### D4.2 — Plumbing: what is actually required

The first draft called `SELF_TASK_EXPECTED_GLOBS` and `SELF_SCHEDULED_GATES` entries "all
mandatory". **They are not.** `SELF_TASK_EXPECTED_GLOBS` holds only `input-liveness` and
`version-lockstep` (`ci_targets.py:184-204`); `SELF_SCHEDULED_GATES` those plus the three
`release-parity*` (`:265-326`). **None of the three existing single-site gates appears in
either.**

This gate follows the single-site precedent and is **not** script-pinned. Consequently it does
**not** need a `repo:affected-smoke` `inputs` entry for `ci/http-extractor/**/*` — that entry is
what makes a *script pin* reachable, and there is no script pin. What it does need:

- a task on the root `repo` project, `toolchain: 'system'`, shaped like
  `repo:error-code-single-site`'s `moon.yml:626-655` — which runs `--self-test` and has **no**
  `--negative-control`. (The first draft cited `ci/release-parity` as the precedent; that is the
  wrong family — those three are script-pinned and carry controls for a different reason.)
  A planted-violation self-test case covers the "does it red?" question inside `--self-test`.
- an entry in `ci.yml`'s `T=(…)` array **and** in CLAUDE.md's marker-delimited command. This pair
  *is* mandatory: `ci_targets.py` asserts the two agree, and `moon ci` exits **0** on a target
  resolving to nothing, so a typo is otherwise a silent no-op on every PR.
- `inputs` that all match tracked files, or `repo:input-liveness` reds.

### D5 — Coverage on the real router

The gate proves no bare `Json` remains. It does **not** prove a route answers in the envelope —
that needs the route reachable, authorized, and wired. SMA-586 learned this expensively: its
extractor's unit tests built a synthetic `Router::new().route("/x/{id}", …)`, which proved the
extractor but not the handler wiring, and that is exactly how a mis-named `{sa}` segment survived
the entire suite until the second fix round.

Coverage is table-driven against the **real merged `router(...)`**, in the six existing
Docker-backed suites, following `tests/http_tenancy.rs:279-306`'s shape:

| Suite | Routes covered |
| -- | -- |
| `http_tenancy.rs` | create org, rename org, create team, rename team, create project, rename project |
| `http_users.rs` | `create_user` |
| `http_memberships.rs` | `create_membership` |
| `http_authz.rs` | `is_authorized`, `put_policy`, `create_role_grant` |
| `http_dead_letters.rs` | `replay_matching` |
| `http_service_accounts.rs` | `create`, api-key `issue` |

Each of the fourteen asserts **syntax (400 `invalid-request-body`)** and **schema (422
`invalid-request-schema`)**. Each table ends with a **well-formed body on the same route**
reaching the handler, so every row is an assertion about the body's shape and not about the route
being broken.

**415 is asserted twice, not six times or fourteen.** It is refused before any handler-specific
code runs, so it is an extractor-level fact: once as a `json.rs` unit test, and once on a single
real route to prove the extractor is reachable there at all. A route left on bare `Json` is
already caught by its own 400/422 rows, so per-suite repetition would add rows without adding a
distinct assertion. (The first draft said "once per suite" with no reason; this is the reason,
and it lowers the count.)

Three prerequisites the implementer must not rediscover:

- **Two rows sit behind capability flags.** `put_policy` and `create_role_grant` are in
  `authz::admin_router()`, mounted only when `caps.authz_admin` (`mod.rs:871-873`); api-key
  `issue` is in `api_keys::router()`, mounted only when `caps.apikeys_management`
  (`mod.rs:875-877`). A disabled capability 404s the route, so those rows need a config enabling
  both.
- **`RenameBody` makes 422 non-obvious.** All its fields are `Option<String>` with no
  `deny_unknown_fields` (`dto.rs:37-41`), so `{}` deserializes fine and reaches the handler as
  `nothing-to-rename`. The three rename routes need a *type mismatch* (`{"slug": 1}`) to reach
  `JsonDataError`.
- **Docker.** These suites inherit `tests/support/docker.rs`'s policy — they skip when the daemon
  is unreachable, with `tests/docker_preflight.rs` as the canary turning a Docker-less run into
  one loud red rather than silent passes.

`json.rs` unit tests cover each `RejectionKind` arm, `classify` over 400/413/415/422/500
including its `None` arm (D1.1a), and the 413 path end-to-end through a `DefaultBodyLimit`
router.

### D6 — Prose this change invalidates

SMA-586 and SMA-583 both set the precedent that stale prose is fixed alongside the code that
staled it. This change invalidates three sites:

1. **`grpc/convert.rs:1171-1186`** — the twelve-line comment inside
   `the_recorded_transport_divergences_still_hold` saying `issue`'s malformed body "never reaches
   the IAM error envelope or the registry at all… Flagged for a follow-up ticket rather than fixed
   here." After D3 it is false three ways: the extractor changes, the module path changes, and the
   HTTP half becomes assertable as 422 `invalid-request-schema`. The comment is rewritten and the
   HTTP half of that divergence row is **asserted**, closing the gap SMA-586 left open.
2. **`authn.rs`'s doc comments** on `EnvelopeJson` and `RejectionKind` (`:82-87`, `:296-311`),
   which name the two users individually and describe the two-code taxonomy. They move with the
   code and are rewritten for four kinds and seventeen routes.
3. **`error.proto`'s 901 comment** (D1.2).

## Cross-transport divergence

SMA-586's AC-1 required each kind to yield its reason "on both HTTP and gRPC", and where that was
structurally impossible it recorded the asymmetry in the proto comment (`error.proto:165-171`) and
pinned it as an assertion (`convert.rs:1193-1198`, `MutuallyExclusiveFields`). This design adds two
reasons that are **HTTP-only, structurally**, and says so rather than leaving it to be discovered:

- **`unsupported-content-type`** — tonic negotiates `application/grpc` at the transport layer;
  a gRPC client cannot present a wrong content type to a handler.
- **`invalid-request-schema`** — proto3 decoding has no "syntactically valid but schema-invalid"
  state; unknown fields are skipped by design, so the failure mode does not exist.

Both get an "HTTP-only, structurally: …" clause in their 905/906 proto comments, worded as
reason 38's is, and both are added as rows to `the_recorded_transport_divergences_still_hold` so
the asymmetry is an assertion rather than an omission.

## Tests that must change, not merely be added

- **`tests/http_authn.rs:162-173`** — `introspect_wrong_content_type_is_enveloped` asserts 415 →
  `invalid-request-body`. It **will red**, and its expectation becomes
  `unsupported-content-type`. Reassigned, not deleted: it is the assertion that pins the 415 path.
- **`tests/http_authn.rs`'s 400 case** (immediately above, ending `:159`) is unaffected —
  `invalid-request-body` still means malformed syntax.
- **`system_retirement.rs:241,262`** are both 400 cases and unaffected. Named here per SMA-586's
  practice of stating which test must *not* change, so a reviewer can tell a deliberate
  reassignment from collateral damage.
- **`paigasus-proto/src/error.rs:224`** — the count anchor (§ *Registry mechanics*).
- **`grpc/convert.rs`'s divergence test** — gains rows and loses a comment (D6).

## Out of scope

- **Twelve further extractor escapes in the same files.** Ten `Query<…>` bindings (`audit.rs`,
  `authz.rs` ×2, `dead_letters.rs`, `memberships.rs`, `organizations.rs` ×2, `service_accounts.rs`,
  `teams.rs`, `api_keys.rs`) — `?limit=abc` yields axum's plain-text
  `FailedToDeserializeQueryString` 400 today — plus two `Path<String>` (`authz.rs`,
  `system_retirement.rs:96`) which `UuidPath` did not cover because the segment is not a uuid.
  Same class of hole, different extractor. **A follow-up Linear issue is filed: SMA-588**, and
  D4's banned-extractor table is shaped so closing them is a table row rather than a second gate.
- **Request body limits.** The fourteen inherit axum's 2 MB default. After the swap a >2 MB body
  answers 413 `request-too-large` in the envelope, which is the improvement. *Choosing* per-route
  limits is a sizing and DoS-posture question with no AC here, and lowering one is the single
  change in this area that would genuinely move a status code (413 where 200 used to be).
- **The gateway's `invalid-request-body`.** It parses `Bytes` itself and has no bare-`Json`
  request extractor, so it needs no swap. Whether its funnel should also split 415/422 — and thus
  whether 901's two meanings reconverge — is a separate question about a separate code path,
  recorded in the follow-up issue above.
- **An HTTP `field` key in the error envelope.** Deferred by SMA-586; nothing here changes that.

## Compatibility

- **No status changes on any route.** `envelope_rejection` renders `rejection.status()`.
- **Bodies change on seventeen routes** (corrected from "sixteen" — see § *Review corrections*).
  Fourteen move from plain text to a registered reason — strictly additive for a client, which
  previously had nothing parseable to branch on.
- **Three routes are a breaking change for a `reason`-branching client**, and this is stated
  plainly rather than buried: `authn::introspect`, `api_keys::introspect`, and
  `system_retirement::retire` reassign their 415 and 422 responses from `invalid-request-body` to
  the two new codes. SMA-586's
  Compatibility section classified exactly this operation as "For a consumer branching on
  `reason`, this is a breaking change" (586 spec:470-474), and the same classification applies
  here. The mitigation is timing: no released SDK branches on these yet, which is the whole
  argument for landing this before SMA-508.
- **`buf breaking` is clean.** Both values are additive `ErrorReason` entries with fresh field
  numbers; nothing is renamed or renumbered.

## Verification

- `cargo nextest run -p paigasus-iam` with a reachable Docker daemon — the new rows are in
  Docker-backed suites and pass vacuously without one.
- `cargo nextest run -p paigasus-proto` — the count anchor.
- **Codegen is a by-hand step, not part of the `moon ci` line.** Per SMA-586's Verification
  section: `(cd contracts && buf format -w && buf generate)`, then commit the regenerated
  Rust/Py/TS bindings. `contracts:fmt` reds `moon ci` if `buf format -w` is skipped, and
  `contracts:generate` has no `outputs:` so it can serve stale cached output — run `buf generate`
  directly.
- `repo:error-code-single-site` green with `json.rs` on the `MANIFEST` and `authn.rs`'s `why`
  narrowed.
- `repo:http-extractor-envelope` green; its `--self-test` shown to red on a planted fifteenth
  site.
- **Whether the new gate re-baselines `ci/affected-graph/run.sh`'s expected sets is an open
  question to settle by running, not by reasoning.** No `repo:*single-site` task appears in those
  sets today, which suggests no — but a task keyed on `rs/crates/services/*/src/adapters/http/**`
  has not been shown to leave every smoke case's action set unchanged.
- The full CI graph as CI runs it, per CLAUDE.md's marker-delimited command — a new `repo:*` gate
  and a `contracts/` change both reach well beyond per-project tasks.

## Review corrections

Folded in from the adversarial review of the first draft, in the order they mattered:

- The registry **count anchor** (52 → 54) was missing — a hard test failure, in a different crate.
- D1.1's fallback mapped **5xx to a 4xx code**, reintroducing the exact contradiction the section
  claimed to prevent. Corrected to `path.rs`'s hand-back rule.
- D5 promised **one test per fallback branch**, which axum's `pub(crate)` constructors and
  `#[non_exhaustive]` enums make impossible. `classify` was extracted to make the promise keepable.
- D4's **ALLOW table "empty at merge"** was false (`json.rs`'s own `where` clauses), and no scan
  algorithm was given. Both fixed; residuals named.
- D2's justification for splitting the membership test **misstated `check.py`'s rules**. Restated
  as cohesion; the guard is now named and `authn.rs`'s stale `why` is flagged.
- D4's **plumbing list was inaccurate** — the pins it called mandatory hold no single-site gate,
  and it cited the wrong precedent family.
- The **stale `convert.rs` comment**, the **gRPC divergence**, the **twelve `Query`/`Path`
  escapes**, and the **`http_authn.rs:162` reassignment** were all unmentioned. Now D6,
  § *Cross-transport divergence*, § *Out of scope*, and § *Tests that must change*.
- D2.1 (the `json.rs`/`path.rs` funnel asymmetry) and D1.3 (naming and range) were invisible
  decisions; both are now stated with their rejected alternatives.
- **The extractor-count was wrong** (Task 6 cleanup, found by the controller): only two routes —
  `api_keys::introspect` and `system_retirement::retire` — were named as already on
  `EnvelopeJson`. `authn::introspect`, the extractor's own original home route, is a third. The
  wire change therefore touches **seventeen** routes, not sixteen. Corrected everywhere that
  count appeared (Problem, D1.2, § *Tests that must change*, § *Compatibility*).

**Rejected from the review:** nothing outright. The 415-granularity point was accepted but
resolved in the opposite direction to the suggestion (down to two assertions rather than
justifying six), and the proposal to rename `invalid-request-body` was declined for the reason in
D1.3.
