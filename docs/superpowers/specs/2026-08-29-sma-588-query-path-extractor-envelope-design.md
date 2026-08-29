# SMA-588 — every IAM request extractor answers in the envelope, and the gateway reconverges

**Issue:** [SMA-588](https://linear.app/smaschek/issue/SMA-588) — *iam: twelve Query/Path request
extractors answer outside the error envelope*
**Date:** 2026-08-29
**Status:** Drafted

## Problem

SMA-586 closed IAM's `Path<Uuid>` escape. SMA-587 closed its `Json<T>` escape and left
`repo:http-extractor-envelope` behind — a banned-extractor table with per-row on/off flags, built
so the remaining instances are a flag flip rather than a second gate. **Twelve request extractors
in `paigasus-iam` still escape**, and this ticket is the third and last instance of that class in
that service.

**Ten `Query<…>` bindings.** `?limit=abc` answers with axum's plain-text
`FailedToDeserializeQueryString` 400, carrying no `error.code` at all:

| File:line | Handler |
| -- | -- |
| `api_keys.rs:100` | `list` |
| `audit.rs:117` | `list` |
| `authz.rs:125` | `list_policies` |
| `authz.rs:144` | `list_role_grants` |
| `dead_letters.rs:115` | `list` |
| `memberships.rs:119` | `list_memberships` |
| `organizations.rs:63` | `list_orgs` |
| `organizations.rs:126` | `list_teams` |
| `service_accounts.rs:75` | `list` |
| `teams.rs:94` | `list_projects` |

**Two `Path<String>`**, which SMA-586's `UuidPath` does not cover because the segment is not a
uuid. Both segments are policy ids:

| File:line | Handler | Route |
| -- | -- | -- |
| `authz.rs:132` | `delete_policy` | `DELETE /v1/authz/policies/{policy_id}` |
| `system_retirement.rs:97` | `retire` | `POST /v1/authz/system-policies/{id}/retire` |

Why it matters is unchanged from SMA-587: SMA-508's `@paigasus/sdk` (AC2) branches on
`(domain, reason)` and never on message text. A plain-text body has no `reason`, so the SDK cannot
parse it into its error type at all.

### What is actually reachable — measured, not assumed

The spec's shape depends on this, so it is stated up front rather than rediscovered by the
implementer.

- **`Query`: only the numeric fields can fail.** Every query DTO field is an `Option<…>`, and
  every one except `limit`/`offset` is an `Option<String>`, which cannot fail to deserialize.
  `PageQuery`/`MembershipQuery`/`ServiceAccountQuery` carry `Option<i64>`; `AuditQuery` and
  `DeadLetterQuery` carry `Option<u64>`. `RoleGrantQuery` carries no numeric field at all, so its
  route is reachable by this rejection only if a future field adds one. The missing-required-field
  cases (`RoleGrantQuery::principal_prn`, `ServiceAccountQuery::owner_prn`) are already funnelled
  through `TenancyError::MissingRequiredField` **by design** — their DTO comments say so — and are
  untouched here.
- **`Path<String>`: only a non-UTF-8 percent-decoded segment is client-side.** `Path<String>`
  cannot fail to *parse*. Everything else in `PathRejection` — `MissingPathParams`, and
  `WrongNumberOfParameters` / `UnsupportedType` inside `FailedToDeserializePathParams` — is the
  500 server-bug family that `path.rs:87-92` already hands back to axum.

So each new extractor closes exactly one client-facing hole. That is a narrow surface, and the
spec says so rather than implying a broader one.

### A fourth instance, in the gateway, not in the ticket

`paigasus-gateway`'s `chat_completions` (`chat.rs:74`) takes `body: Bytes` under a
`DefaultBodyLimit` layer (`mod.rs:88-90`). An oversized body therefore answers with **axum's own
plain-text 413, outside the OpenAI envelope** — the case IAM's `request-too-large` (902) exists
for. This is the same class as the twelve above, in the other service.

`ci/http-extractor/README.md`'s **L3 asserts the opposite** and is factually wrong: it says a body
taken as `Bytes` produces "no rejection is produced for this gate to care about". Under
`DefaultBodyLimit` it produces exactly one. Retiring L3 is in scope (D6).

Folding this in was an explicit scope decision, not drift (§ *Decision record*).

## Acceptance criteria

| AC | Satisfied by |
| -- | -- |
| **AC-1** — all ten `Query` and both `Path<String>` routes answer a refused query string / path segment inside `{"error":{code,message}}` with a registered reason | D2 + D3 + D6 |
| **AC-2** — the reasons distinguish the failure kinds rather than re-installing a catch-all | D1 |
| **AC-3** — a thirteenth bare `Query`/`Path`/`Bytes` request extractor cannot land silently | D5 |
| **AC-4** — `invalid-request-body` (901) means the same thing on both services | D4 |
| **AC-5** — the gateway's oversized body answers inside the OpenAI envelope | D4.2 |
| **AC-6** — every reason resolves via `ErrorReason::from_wire_reason`; `repo:error-code-single-site` green; `buf breaking` clean | § *Registry mechanics* + § *Verification* |
| **AC-7** — every consequence is recorded where a reader will meet it: stale prose retired, new transport divergences declared, a superseded gate fixture reassigned | D6 + § *Cross-transport divergence* |

## Scope

The twelve IAM request extractors, one new IAM extractor module and one new extractor in an
existing module, two additive registry values, the gateway's 901/906 split, the gateway's `Bytes`
413, three gate rows plus one new one, and the prose and assertions those invalidate.

## Decision record

Four questions the ticket posed, and one this design found. All five were decided by the human
partner before drafting; they are recorded here so a reviewer can see what was chosen over what.

| # | Question | Chosen | Over |
| -- | -- | -- | -- |
| 1 | The query reason | a new `invalid-query-parameter` (907) | reusing 906, or IAM's `invalid-pagination` |
| 2 | The `Path<String>` shape | `StringPath<F>` sibling + a new `invalid-path-segment` (908) | one merged reason for both locations, or handing the case back to axum |
| 3 | The gateway's 901 | adopt the 901/906 split, **not** 905 | full symmetry incl. 905, or declaring the asymmetry final |
| 4 | The gateway's `Bytes` 413 | fold into this ticket | a follow-up ticket, or leaving it |
| 5 | The gateway's 906 status | **422**, for full symmetry with IAM | keeping today's 400 |

## Design decisions

### D1 — Two reasons, each denoting one condition

| wire | number | meaning | status |
| -- | -- | -- | -- |
| `invalid-query-parameter` | **907, new** | a query parameter could not be deserialized into its target type | 400 |
| `invalid-path-segment` | **908, new** | a path segment could not be decoded as text | 400 |

Both land in the **900 range** ("shared — any domain may emit", `error.proto:32`), for the reason
SMA-587 D1.3 put 905 and 906 there: they are transport-level facts about an HTTP request, not IAM
domain failures, and another domain could adopt the same extractors tomorrow.

**Why not reuse 906.** `invalid-request-schema`'s own comment says "the **body** was syntactically
valid JSON but did not match the target type". A query string is not a body. Reusing it would make
906 mean two things on fourteen-plus endpoints — the catch-all re-accretion SMA-586 D1 and SMA-587
D1 each rejected.

**Why not one merged reason** (`invalid-request-parameter`, covering both a query parameter and a
path segment). It would fail SMA-587 D1.3's own test: a name is acceptable when it denotes exactly
one condition, and is rejected when it is broad enough to re-accrete a catch-all. Query and path
are two distinct request locations, and an SDK that cannot tell them apart cannot tell the caller
where to look.

**Why not `invalid-pagination` for the query half.** It exists, and `limit`/`offset` are the only
fields reachable today. But it is an IAM domain code for a *validated range* — `Page::new` emits it
after a successful deserialization — the query DTOs carry non-pagination fields, and it would
mislabel the first future non-numeric typed field. The two failures are genuinely different: 907 is
"this is not a number", `invalid-pagination` is "this number is out of range".

#### D1.1 — Both messages are static; only `StringPath` names its field

`EnvelopeQuery` **cannot** name the failing parameter. axum renders
`FailedToDeserializeQueryString` as `format!("Failed to deserialize query string: {err}")`, and
serde_urlencoded's error for a bad integer is `invalid digit found in string` — it does not carry
the key. Echoing the rejection text would also break `json.rs`'s standing rule that nothing ever
echoes caller input. So the message is static, and the parameter name is not recoverable. Stated
here so a reviewer does not read its absence as an oversight.

`StringPath<F>` **does** name its field, through `path.rs`'s `PathField` marker, exactly as
`UuidPath` does. Both routes' segments are policy ids, so one new marker suffices:
`PolicyId => "policy_id"`.

### D2 — Both new extractors render through `ApiError(TenancyError::…)`

This is the axis on which `json.rs` and `path.rs` deliberately differ (SMA-587 D2.1), so choosing a
side is a reviewable decision rather than a detail.

`json.rs` builds its envelope **by hand** from kebab literals, and pays for it with a row on
`ci/error-registry/check.py`'s MANIFEST — which blinds that gate to any future production literal
in that file. It must, because `EnvelopeJson` also serves `api_keys::introspect`, whose every other
failure is an `AuthnApiError`, a funnel deliberately separate from `ApiError`/`TenancyError`. An
extractor emitting a `TenancyError` there would make a route's error type depend on *where in the
request* it failed.

**That constraint does not reach here.** All ten `Query` routes and both `Path<String>` routes
return `Result<_, ApiError>`. So both new extractors take `path.rs`'s side:

- neither module lands on the MANIFEST,
- `path.rs` keeps its deliberate literal-free property, stated in its own comment
  (`path.rs:179-181`) as the reason `UuidPath` renders through the funnel,
- both inherit `retryable` classification for free rather than hardcoding `Retryable::No`.

The cost is two new `TenancyError` variants carrying 900-range codes. **Precedent, not invention:**
`TenancyError::Internal → "internal"` is already 900. The compiler enforces the rest — `code()`,
`class()` (both `Validation`) and the `retryable` `None` arm are exhaustive matches under
`[workspace.lints.rust] warnings = "deny"`, so a missed arm is a compile error, not a silent gap.

```rust
TenancyError::InvalidQueryParameter          => "invalid-query-parameter"
TenancyError::InvalidPathSegment(&'static str) => "invalid-path-segment"
```

`InvalidPathSegment` carries the field name so its message can name it; `InvalidQueryParameter` is
a unit variant because there is no name to carry (D1.1).

*Rejected alternative:* mirror `json.rs` and hand-build both envelopes. It would keep
`TenancyError` free of transport concerns — a real cost of the chosen option, since gRPC also maps
that enum — but it would put a code literal into `path.rs`, blinding the error-registry gate for
the file that most carefully avoided that, and it would hardcode `Retryable::No` in two more
places.

### D3 — `EnvelopeQuery<T>` in a new `query.rs`; `StringPath<F>` beside `UuidPath`

```
adapters/http/
  authn.rs   AuthnApiError + BEARER_CHALLENGE + the introspect route
  json.rs    EnvelopeJson / RejectionKind / classify            (SMA-587)
  path.rs    UuidPath / UuidPathPair / StringPath               (SMA-586, SMA-588)
  query.rs   EnvelopeQuery                                      (SMA-588)
```

`EnvelopeQuery` gets its own module, one per input kind, per SMA-587 D2's rule. `StringPath` goes
**into `path.rs`** rather than a module of its own: it is the same input kind as `UuidPath`, shares
its `PathField` marker mechanism and its `is_client_error` hand-back rule, and splitting them would
put two extractors for one input kind in two files with a shared private helper between them.

**All three extractors keep one rule for server bugs.** A rejection axum classes 5xx means the
route's pattern stopped matching its handler's arity — a *server* bug — and keeps axum's own
status and body rather than being relabelled as the caller's mistake. `path.rs:87-92` chose this,
`json.rs`'s `classify` returns `None` for it, and both new extractors do the same. Three extractors
answering server bugs identically is the property; it is stated because it is the one an
implementer is most likely to drop.

`QueryRejection` is `#[non_exhaustive]` with a single variant today
(`FailedToDeserializeQueryString`), so the fallback arm is mandatory and gates on
`is_client_error()` exactly as `path.rs` does.

### D4 — The gateway reconverges

#### D4.1 — The 901/906 split

`chat.rs:81-83` collapses every deserialization failure into one arm:

```rust
let dto: ChatCompletionRequest = match serde_json::from_slice(&body) {
    Ok(dto) => dto,
    Err(_) => return GatewayError::BadRequestBody.into_response(),
};
```

`serde_json::Error::classify()` gives the split with no new parsing: `Category::Data` → **906**
`invalid-request-schema`; `Category::Syntax` / `Eof` / `Io` → **901** `invalid-request-body`. After
this, 901 means *malformed or unreadable* on both services, which is what AC-4 asks for.

**905 is deliberately not adopted.** The gateway takes `Bytes` and never reads `Content-Type`.
Adopting `unsupported-content-type` would mean *adding* a content-type check and rejecting requests
that succeed today. So 905 stays IAM-only — and the reason is now structural rather than
incidental, which its proto comment must say (D6).

**The 906 status moves to 422** (decision 5). This is a real, deliberate compatibility break on a
live endpoint and § *Compatibility* records it as one: it contradicts SMA-587's stated property
that no route's status changes, which was scoped to that ticket's work. Full symmetry with IAM's
`(906, 422)` pairing was chosen over preserving today's 400.

#### D4.2 — `EnvelopeBytes`, and the 413

The oversized body never reaches the handler, so no change inside `chat_completions` can catch it.
`EnvelopeBytes` wraps the `Bytes` extractor and maps `BytesRejection` by **status**, the same
hybrid rule `json.rs` D1.1 established — `FailedToBufferBody` composes `LengthLimitError` (413) and
`UnknownBodyError` (400), so a variant-only match would render a 413 code on a 400 response:

| status | `GatewayError` | reason |
| -- | -- | -- |
| 413 | `RequestTooLarge` — **new variant**, `Retryable::No` | `request-too-large` (902, existing) |
| other 4xx | `BadRequestBody` | `invalid-request-body` (901, existing) |
| non-client-error | hand axum's own response back | — |

No new registry value: 902 already exists and already means this. `GatewayError::parts()` is
already on the error-registry MANIFEST with the guard
`every_gateway_code_is_declared_in_the_canonical_registry`, so the new variant is covered without a
new row.

**The egress property is preserved.** `EnvelopeBytes` wraps extraction only; the handler still
forwards the original raw bytes verbatim and still never hands the OpenAI client any caller header.
Stated because `chat.rs`'s module docs call that property load-bearing and a reviewer must be able
to confirm the change does not touch it.

### D5 — The gate: three rows on, one row added

| row | on | replacement | match |
| -- | -- | -- | -- |
| `Json` | yes (SMA-587) | `EnvelopeJson` | bare identifier, unchanged |
| `Query` | **on** | `EnvelopeQuery` | bare identifier |
| `Path` | **on** | `UuidPath` / `StringPath` | **requires a following `<`** |
| `Bytes` | **new row, on** | `EnvelopeBytes` | bare identifier |

**The `Query` row needs no special handling.** `_banned_pattern`'s identifier boundaries already
exclude `PageQuery`, `AuditQuery`, `MembershipQuery`, `RoleGrantQuery`, `ServiceAccountQuery` and
`DeadLetterQuery` — in each the `Query` is preceded by an identifier character — while
`Query<PageQuery>` matches on the outer name. `EnvelopeQuery` is excluded the same way
`EnvelopeJson` is.

**The `Path` row needs the tighter match its own comment warns about.** `check.py`'s note says a
bare `Path` also matches `std::path::Path` (`p: &Path`). **Measured: there is no `std::path`,
`PathBuf` or `&Path` anywhere in either service's `adapters/http` tree today**, so this is
prophylactic rather than a live collision — and it is cheap. The `BANNED` tuple grows a per-row
flag requiring a following `<`; `Json`, `Query` and `Bytes` keep bare-identifier matching, which
fails closed. Fixtures pin `p: &Path` and `p: PathBuf` as legal and `Path<String>` as a violation,
so the flag cannot rot into a no-op.

**The `Bytes` row inverts an existing fixture.** `FIXTURES` currently carries
*"legal — a body taken as Bytes (the gateway's shape); see README Limitations L3"*. That fixture
**flips to a planted violation** and its L3 cross-reference goes with it. A reviewer meeting a
deleted "legal" fixture must be able to see it was reassigned rather than dropped.

**ALLOW rows.** `query.rs` and the `EnvelopeBytes` definition site each get a defensive row,
mirroring `json.rs`'s: the definition site must be able to name the type it wraps, and a gate that
reds on the file whose job is to wrap the banned type would just get deleted. `path.rs` needs one
too under the `Path` row — `StringPath` delegates to `Path::<String>::from_request_parts`, which is
a call in a function *body* and therefore outside every parameter span today, but the row is
defensive for the same reason `json.rs`'s is. Each row states its reason, and `check.py` already
asserts every ALLOW path still exists.

**The liveness check needs a second look.** `LIVENESS_IDENT = "EnvelopeJson"` is a positive control
proving the parser still reaches real handler signatures. It stays correct after this change, since
`EnvelopeJson` is still in fourteen request positions. No change needed — recorded so the
implementer does not "helpfully" generalise it.

### D6 — Prose and assertions this invalidates

1. **`error.proto`'s 901 comment** says "Reconverging the two is SMA-588, not an accident". After
   D4.1 the two *are* reconverged; the comment states the shared meaning instead.
2. **`error.proto`'s 905 comment** says 905 is HTTP-only because tonic negotiates
   `application/grpc`. Still true, but now incomplete: it must also say the gateway does not emit
   it because it takes `Bytes` and never reads `Content-Type` — otherwise the next reader will
   file the same reconvergence ticket again.
3. **`check.py`'s "RESERVED, NOT FORGOTTEN" block** and its `NOTE for whoever flips Path on`
   describe work this ticket does. Both are rewritten.
4. **`ci/http-extractor/README.md`'s L3** is factually wrong (§ *Problem*) and is retired, not
   softened. A residual list that claims a hole is not a hole is worse than an absent entry.
5. **`chat.rs`'s module doc and `chat_completions`'s doc** both say the `DefaultBodyLimit` layer
   "fails oversized bodies with a 413 before this handler is reached", describing the escape as
   correct behaviour. Rewritten for `EnvelopeBytes`.
6. **`dto.rs`'s query-DTO comments** — `RoleGrantQuery`, `ServiceAccountQuery`, `AuditQuery` and
   `DeadLetterQuery` each justify keeping a field as a raw `Option<String>` by contrasting it with
   "axum's default plain-text query rejection". That rejection no longer exists on these routes.
   The reasoning still holds — a handler-validated field gives a *better* reason than 907 — but the
   contrast must be restated against `EnvelopeQuery` rather than against axum.

## Registry mechanics

Adding a reason touches **three** coupled sites. SMA-586 called the third "the single easiest step
to miss" and SMA-587's design missed it on its first draft:

1. `contracts/proto/paigasus/common/v1/error.proto` — two additive values, 907 and 908, each
   commented with its wire string, meaning and transport availability.
2. `rs/crates/libs/paigasus-proto/src/error.rs` — the hand-transcribed `EXPECTED_REASONS` list.
   Independent by design: `ci/error-registry/check.py` cross-checks the two transcriptions, which
   can only agree if both are right.
3. `rs/crates/libs/paigasus-proto/src/error.rs:228` —
   `assert_eq!(actual.len(), 55, "the registry should hold 55 reasons")` becomes **57**. Note the
   SMA-587 spec's "52 → 54" is already stale; the live value is **55**. Missing this fails
   `the_registry_contains_exactly_the_expected_reasons` in `paigasus-proto`, *not* in the crate
   under change, so it reads as an unrelated breakage.

## Cross-transport divergence

`the_recorded_transport_divergences_still_hold` (`grpc/convert.rs`) records four divergences today.
This design adds two, both HTTP-only **structurally**:

- **907 `invalid-query-parameter`** — gRPC has no query string. A gRPC client cannot present one.
- **908 `invalid-path-segment`** — gRPC has no URL path segments carrying request data; a method
  name is not deserialized into a handler parameter.

Both are recorded in that test's comment and in their proto comments, following divergences 3 and
4's precedent. Whether either is *asserted* there follows the existing rule: rows whose HTTP half is
already pinned in the extractor's own unit tests are recorded as comments, not re-asserted.

## Compatibility

**No IAM route's status changes.** Every new extractor preserves the rejection's own status, and
all twelve escapes are 400 today and 400 after.

**Two IAM wire changes.** The twelve routes' refusal bodies change from axum's plain text to the
`{"error":{code,message}}` envelope, and gain the `paigasus-retryable` header they do not carry
today — the gap `authn.rs:330-341` pins as a contract and these routes silently violate.

**One deliberate gateway status change.** `POST /v1/chat/completions` answers **422 instead of 400**
for a body that is valid JSON but does not match `ChatCompletionRequest`. A syntactically malformed
body still answers 400. This is decision 5 and it is a break: a client branching on 400 for any bad
body will see 422 for the schema half. It is recorded here rather than buried in D4.1 because it is
the only outward-facing behaviour change in this ticket that a caller can observe without reading
`error.code`.

**One gateway body-shape change.** An oversized body answers inside the OpenAI envelope with
`request-too-large` instead of axum's plain text. The status stays 413.

## Testing

Following SMA-587 D5's rule, which SMA-586 learned expensively: a synthetic
`Router::new().route(…)` proves the extractor, **not** the handler wiring, and that is exactly how
a mis-named `{sa}` segment survived SMA-586's entire suite. Route coverage runs against the **real
merged `router(...)`**.

| Level | Coverage |
| -- | -- |
| `query.rs` unit | the rejection maps to 907 in the envelope; a well-formed query still extracts; a non-client-error rejection keeps axum's own response |
| `path.rs` unit | `StringPath` maps a non-UTF-8 segment to 908 naming `policy_id`; a well-formed segment extracts; a router-arity bug keeps its own 500 — mirroring `UuidPath`'s three existing tests |
| IAM integration | each of the ten `Query` routes asserts `?limit=abc` → 400 `invalid-query-parameter`, and each table ends with a **well-formed query on the same route reaching the handler**, so every row asserts the query's shape and not a broken route. Both `Path<String>` routes assert a non-UTF-8 segment → 400 `invalid-path-segment` |
| gateway | the 901/906 split (a syntax error → 400/901, a type mismatch → 422/906) and an oversized body → 413/`request-too-large` inside the OpenAI envelope |
| gate | `--self-test` fixtures for the three newly-enabled rows and the new one, the `Path` row's `<` requirement (`&Path` and `PathBuf` legal, `Path<String>` a violation), and the inverted `Bytes` fixture |

**Prerequisites the implementer must not rediscover:**

- **Capability flags — three of them, covering half the rows.** A disabled capability does not
  register its routes at all (SMA-505), so they 404 and a row written without the right config
  passes for the wrong reason. Measured against `mod.rs:876-886`:

  | capability | routes it gates |
  | -- | -- |
  | `caps.authz_admin` | `list_policies`, `list_role_grants`, **`delete_policy`**, and **`retire`** — `system_retirement::router()` is merged inside the same branch |
  | `caps.apikeys_management` | api-key `list` |
  | `caps.audit_query` | `audit::list` |

  Note both `Path<String>` rows sit behind `authz_admin`, so the 908 coverage needs it enabled or
  it asserts nothing.
- **`RoleGrantQuery` has no numeric field.** `?limit=abc` on `list_role_grants` is simply an
  *ignored unknown parameter*, not a rejection — the route answers
  `missing-required-field` for the absent `principal_prn` instead. Its row must send a
  well-formed `principal_prn` **and** a malformed numeric parameter, and that parameter does not
  exist. **This route is not reachable by a 907 at all today** and its row asserts the
  missing-required-field behaviour is unchanged instead. Stated because a row written the obvious
  way would pass for the wrong reason.
- **Docker.** The IAM suites inherit `tests/support/docker.rs`'s policy — they skip when the daemon
  is unreachable, with `tests/docker_preflight.rs` as the canary turning a Docker-less run into one
  loud red rather than silent passes.

## Verification

- `moon ci` over the full marker-delimited target list in `CLAUDE.md`, not per-project tasks — this
  change touches `repo:http-extractor-envelope`, `repo:error-code-single-site` and
  `repo:breaking`.
- `repo:error-code-single-site` green: the two new codes resolve via `ErrorReason::from_wire_reason`
  and every emission site is on the MANIFEST (no new rows needed — D2).
- `buf breaking` clean: both proto changes are additive enum values plus comment edits.
- `repo:http-extractor-envelope` green with all four rows on, and its `--self-test` proving each
  newly-enabled row can red.

## Out of scope

- **The `Query`/`Path` rows in other services.** `paigasus-gateway`'s `adapters/http` tree contains
  no `Query` or `Path` extractor at all — measured — so the two rows are green there on day one and
  the gate stops it growing the same hole.
- **`repo:http-extractor-envelope`'s remaining residuals.** README L1 (an aliased import,
  `use axum::Json as J`) and L2/L4 are unchanged and stay recorded. Only L3 is retired, because it
  is wrong rather than merely a limit.
- **A `String` body.** L3 covered `Bytes` *and* `String`. `String` is not used as a request body
  anywhere in either tree today, so no extractor is written for it. The residual entry narrows to
  `String` rather than disappearing.
- **Handler-validated query fields.** `RoleGrantQuery::principal_prn`,
  `ServiceAccountQuery::owner_prn`, `AuditQuery`'s timestamps and cursors, and `Page::new`'s range
  checks keep funnelling through their existing, more specific `TenancyError`s. 907 is for a
  failure that happens *before* a handler runs; replacing a specific reason with a general one
  would be a regression.
