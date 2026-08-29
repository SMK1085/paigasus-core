# SMA-588 — every IAM request extractor answers in the envelope, and the gateway reconverges

**Issue:** [SMA-588](https://linear.app/smaschek/issue/SMA-588) — *iam: twelve Query/Path request
extractors answer outside the error envelope*
**Date:** 2026-08-29 (revised the same day; adversarial review folded in — see § *Review corrections*)
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

### What is actually reachable — measured

The spec's shape depends on this, so it is stated up front rather than rediscovered by the
implementer. **The measurements below were run against a real `Router` on this tree**, not
reasoned from the general contract; the first draft reasoned, and got it wrong (§ *Review
corrections*, C1).

**`Query` has two reachable client-side failure classes, not one.**

| probe | status | body |
| -- | -- | -- |
| `?limit=abc` — a value that will not parse | 400 | Failed to deserialize query string: limit: invalid digit found in string |
| `?limit=1&limit=2` — a repeated key, numeric field | 400 | Failed to deserialize query string: duplicate field `limit` |
| `?principal_prn=a&principal_prn=b` — a repeated key, **`Option<String>` field** | 400 | Failed to deserialize query string: duplicate field `principal_prn` |
| `?limit=abc&principal_prn=a` — an **unknown** key on a DTO that lacks it | 200 | unknown keys are ignored |
| `?limit=1&offset=2` | 200 | — |

The second class is the one the first draft missed. `axum::extract::Query` deserializes through a
derived struct visitor, which raises `duplicate field` for a repeated key **regardless of the
field's type** — axum's own doc says so directly (`axum-0.8.9/src/extract/query.rs:45-46`: *"For
handling multiple values for the same query parameter … use `axum_extra::extract::Query`
instead"*). So **all ten routes are reachable**, including `list_role_grants`, whose
`RoleGrantQuery` carries no numeric field at all.

A value-parse failure is confined to the numeric fields: `PageQuery`, `MembershipQuery` and
`ServiceAccountQuery` carry `Option<i64>`; `AuditQuery` and `DeadLetterQuery` carry `Option<u64>`;
every other field is an `Option<String>`, which cannot fail to parse. A **repeated key** reaches
every field on every route.

The missing-required-field cases (`RoleGrantQuery::principal_prn`, `ServiceAccountQuery::owner_prn`)
are already funnelled through `TenancyError::MissingRequiredField` **by design** — their DTO
comments say so — and are untouched here.

**`Path<String>`: only a non-UTF-8 percent-decoded segment is client-side.** `Path<String>` cannot
fail to *parse*. Everything else in `PathRejection` — `MissingPathParams`, and
`WrongNumberOfParameters` / `UnsupportedType` inside `FailedToDeserializePathParams` — is the 500
server-bug family that `path.rs:87-92` already hands back to axum. The one reachable case is
genuinely reachable: `PercentDecodedStr::new` returning `None` sets
`UrlParams::InvalidUtf8InPathParam`, `Path::from_request_parts` turns it into
`FailedToDeserializePathParams`, and that kind's `status()` is `BAD_REQUEST` — so
`probe(Router::new().route("/x/{id}", get(ok)), "/x/%FF")` reaches it and `path.rs`'s existing
`is_client_error` admits it.

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
| **AC-4** — `invalid-request-body` (901) means the same thing on both services | D4.1 |
| **AC-5** — the gateway's oversized body answers inside the OpenAI envelope | D4.2 |
| **AC-6** — every reason resolves via `ErrorReason::from_wire_reason`; `repo:error-code-single-site` green; `buf breaking` clean | § *Registry mechanics* + § *Verification* |
| **AC-7** — every consequence is recorded where a reader will meet it: stale prose retired, new transport divergences declared, superseded gate assertions reassigned | D5 + D6 + § *Cross-transport divergence* |

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
| 5 | The gateway's 906 status | **400**, today's status, kept | 422, for full symmetry with IAM |

**Decision 5 was re-taken at spec approval.** It was first decided as 422, for symmetry with IAM's
`(906, 422)` pairing, before two facts this revision measured: the OpenAI client SDKs branch on
status as well as `error.type`, mapping 400 → `BadRequestError` and 422 →
`UnprocessableEntityError`, and OpenAI's own API answers 400 here; and `ChatCompletionRequest`'s
`#[serde(flatten)]` field means a scalar, `null` or array body classifies as `Data`, so 422 would
have captured a wider set than "a schema mismatch" suggests. Both argue for 400, and 400 was
chosen.

**Consequence, stated so it is not read as an oversight:** `invalid-request-schema` (906) now sits
on **422 in IAM and 400 in the gateway**. That is not the contradiction SMA-587 D1.1 forbade — that
was a `request-too-large` code on a 400, where the code names a condition the status denies. Both
statuses here are client errors and neither denies a schema mismatch. The reconvergence this ticket
was asked for is of **901's meaning**, which D4.1 delivers; status parity was never part of it, and
buying it would have cost OpenAI wire compatibility, which is the gateway's purpose.

## Design decisions

### D1 — Two reasons, each denoting one condition

| wire | number | meaning | status |
| -- | -- | -- | -- |
| `invalid-query-parameter` | **907, new** | a query parameter could not be deserialized into its target type, or was supplied more than once | 400 |
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
fields a *value-parse* failure can reach. But it is an IAM domain code for a *validated range* —
`Page::new` emits it after a successful deserialization — and it could not name the repeated-key
class at all, which reaches every field on every route. The two failures are genuinely different:
907 is "this is not a number, or you sent it twice", `invalid-pagination` is "this number is out of
range".

#### D1.1 — Both messages are static, and the reason is a type invariant

The parameter name **is** recoverable. axum wraps the deserializer in
`serde_path_to_error::deserialize` (`axum-0.8.9/src/extract/query.rs:92`), so the rejection body
reads `Failed to deserialize query string: limit: invalid digit found in string` — measured. The
first draft claimed the opposite and reasoned from it (§ *Review corrections*, C2).

The message is static anyway, and the real reason is stronger than the false one: **`TenancyError`'s
field payload is deliberately `&'static str`**, and `application/error.rs:50-56` states why —

> The payload type is load-bearing: a `&'static str` cannot hold caller-supplied input, so "never
> reflect untrusted input into an error body" is enforced by the type rather than remembered by
> each call site. … `Box::leak`/`String::leak` would defeat exactly that invariant — they mint a
> `&'static str` from runtime input, and leak memory per request while doing it — so they must
> never be used to reach these constructors.

A runtime-derived parameter name cannot reach the constructor without breaking that invariant. So
`InvalidQueryParameter` is a **unit variant** and its message names no field. This is a constraint
of the chosen funnel (D2), recorded so a future reader does not "fix" it by leaking a name.

`StringPath<F>` **does** name its field, through `path.rs`'s `PathField` marker — a compile-time
`&'static str`, which is exactly what the invariant permits. Both routes' segments are policy ids,
so one new marker suffices: `PolicyId => "policy_id"`.

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
return `Result<_, ApiError>` — verified individually. So both new extractors take `path.rs`'s side:

- neither module lands on the MANIFEST,
- `path.rs` keeps its deliberate literal-free property, stated in its own comment
  (`path.rs:179-181`) as the reason `UuidPath` renders through the funnel,
- both inherit `retryable` classification for free rather than hardcoding `Retryable::No`.

```rust
TenancyError::InvalidQueryParameter            => "invalid-query-parameter"
TenancyError::InvalidPathSegment(&'static str) => "invalid-path-segment"
```

The cost is two new `TenancyError` variants carrying 900-range codes. **Precedent, not invention:**
`TenancyError::Internal → "internal"` is already 900.

#### D2.1 — The three exhaustive matches, and the one that is not

The first draft named the wrong set (§ *Review corrections*, C3). Corrected against the tree,
adding a variant forces a choice in **three** wildcard-free matches in `application/error.rs`:

| match | choice |
| -- | -- |
| `code()` | the two wire strings above |
| `class()` | `ErrorClass::Validation` for both |
| **`field()`** | `InvalidPathSegment(f) => Some(f)`; `InvalidQueryParameter => None` |

`field()` was missed entirely and is the one with a design consequence. Its doc says a
field-carrying variant "must choose here rather than silently losing its `metadata["field"]` to a
`_ => None` arm", and `status_to_grpc` (`grpc/convert.rs:129-130`) puts that name into
`ErrorInfo.metadata["field"]`. So `InvalidPathSegment` returning `Some("policy_id")` means the
metadata key is populated on a gRPC error for a reason § *Cross-transport divergence* declares
gRPC-impossible. That is consistent — the metadata is correct *if* the error is ever constructed —
and it is exactly why that section now carries an enforcement test rather than a bare claim.

`tenancy_retryable` is **not** a fourth site: it matches on `ErrorClass`, not on `TenancyError`
(`adapters/retryable.rs:14-19`), so it needs no edit and provides no compile enforcement. The first
draft claimed it as both.

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

**Both implement `FromRequestParts`, not `FromRequest`.** This is load-bearing, not incidental:
`system_retirement::retire` (`system_retirement.rs:97`) takes `Path(id): Path<String>` **followed
by** `body: Option<EnvelopeJson<RetireBody>>`, and only one `FromRequest` extractor is permitted
and it must come last. A `FromRequest` `StringPath` would not compile there. SMA-587 D3 made the
mirror-image statement for `EnvelopeJson`; this spec states it because the constraint binds the
other way round.

`StringPath<F>` exposes its value as a **public named field** (`pub value: String`), matching
`UuidPath { pub id }` and `UuidPathPair { pub first, pub second }`. Both call sites want a `&str`
(`s.policies.delete(&actor, &policy_id)`, `s.retirement.retire(&actor_prn(&ctx), &id, ack)`), which
`&path.value` supplies.

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

`serde_json::Error::classify()` supplies the split. **Measured against the real
`ChatCompletionRequest`**, because its `#[serde(flatten)] extra` field
(`gateway/src/adapters/http/dto.rs:32-33`) forces buffering through serde's private `Content` type
and could have changed the classification path:

| body | `classify()` | maps to |
| -- | -- | -- |
| `{"model":"m","messages":[}` | `Syntax` | 901 |
| `{"model":"m",` and `` (empty) | `Eof` | 901 |
| `{"messages":[]}` (missing `model`) | `Data` | 906 |
| `{"model":42,"messages":[]}` | `Data` | 906 |
| `"hello"` · `42` · `true` · `null` · `[]` | `Data` | 906 |

`Category::Io` cannot arise from a `&[u8]`, but is mapped to 901 with the other unreadable cases
for exhaustiveness. After this, 901 means *malformed or unreadable* on both services, which is what
AC-4 asks for.

**Every row keeps status 400** (decision 5). Only the `code` moves, and only for the `Data` rows.

The last row is a consequence the first draft did not state and is the reason decision 5 was
re-taken: a scalar, `null` or array body is a `Data` error, so it answers 906 alongside genuine
schema mismatches. On 400 that is a code change to an already-refused request; on 422 it would have
been a status change too. § *Compatibility* records it.

**905 is deliberately not adopted.** The gateway takes `Bytes` and never reads `Content-Type`.
Adopting `unsupported-content-type` would mean *adding* a content-type check and rejecting requests
that succeed today. So 905 stays IAM-only — and the reason is now structural rather than
incidental, which its proto comment must say (D6).

#### D4.2 — Two new `GatewayError` variants, and the 413

`GatewayError::parts()` (`gateway/src/adapters/http/error.rs:101-150`) binds a full tuple per
variant, and `BadRequestBody` is fixed at `(400, "invalid-request-body")`. Emitting a different
status or code therefore needs a **new variant**, not a new argument. The first draft declared only
one of the two (§ *Review corrections*, C4):

| variant | status | `type` | `code` | `param` | message | `retryable()` |
| -- | -- | -- | -- | -- | -- | -- |
| `InvalidRequestSchema` **new** | 400 | `invalid_request_error` | `invalid-request-schema` (906) | `None` | "The request body does not match the expected schema." | `No` |
| `RequestTooLarge` **new** | 413 | `invalid_request_error` | `request-too-large` (902) | `None` | "The request body is too large." | `No` |

`InvalidRequestSchema` shares `BadRequestBody`'s status and differs only in `code` and `message`. It
is still a **new variant** rather than an argument to the existing one, because `parts()` binds the
whole tuple per variant — there is no way to vary the code without one.

`param` is `None` on both. OpenAI's envelope uses `param` to name an offending *request parameter*,
and the gateway parses with a plain `serde_json::from_slice` — no `serde_path_to_error` — so no
field path is available. Adding one to populate `param` is a genuine improvement and is out of
scope; `StreamingDisabled` remains the only variant that sets it.

Neither variant needs a registry value: 906 and 902 both already exist and already mean this.
`GatewayError::parts()` is already on the error-registry MANIFEST with the guard
`every_gateway_code_is_declared_in_the_canonical_registry`, so both are covered without a new row.

**The 413 mechanism, stated correctly.** `DefaultBodyLimit` does not itself produce the 413: it
sets a request extension that `Bytes::from_request` honours, which is precisely why wrapping the
extractor works. (`mod.rs:85-86` says so, and `json.rs:293-316`'s existing 413 test proves the same
mechanism for `EnvelopeJson`.) The first draft wrote "the oversized body never reaches the handler,
so no change inside `chat_completions` can catch it", which reads as though the layer answers
before extraction and would leave a reader unable to see why AC-5 is satisfiable at all.

`EnvelopeBytes` therefore wraps the `Bytes` extractor and maps `BytesRejection` by **status**, the
same hybrid rule `json.rs` D1.1 established — `FailedToBufferBody` composes `LengthLimitError`
(413) and `UnknownBodyError` (400), so a variant-only match would render a 413 code on a 400
response:

| status | variant | reason |
| -- | -- | -- |
| 413 | `RequestTooLarge` | `request-too-large` (902) |
| other 4xx | `BadRequestBody` | `invalid-request-body` (901) |
| non-client-error | hand axum's own response back | — |

**`EnvelopeBytes` lives in `paigasus-gateway/src/adapters/http/bytes.rs`** — a new file, named here
because the obvious alternative is a fail-open (D5).

**The egress property is preserved.** `EnvelopeBytes` wraps extraction only; the handler still
forwards the original raw bytes verbatim and still never hands the OpenAI client any caller header.
Stated because `chat.rs`'s module docs call that property load-bearing and a reviewer must be able
to confirm the change does not touch it.

### D5 — The gate: three rows on, one row added, one self-assertion retired

| row | on | replacement | match |
| -- | -- | -- | -- |
| `Json` | yes (SMA-587) | `EnvelopeJson` | bare identifier, unchanged |
| `Query` | **on** | `EnvelopeQuery` | bare identifier |
| `Path` | **on** | `UuidPath` / `StringPath` | **requires a following `<`** |
| `Bytes` | **new row, on** | `EnvelopeBytes` | bare identifier |

**The flip breaks the gate's own self-test, and that is the first edit.** `check.py:640-642`
asserts the reserved rows are *off*:

```python
if any(on for name, on, _r in BANNED if name in ("Query", "Path")):
    print("  FAIL [BANNED] a reserved row is enabled — that is a follow-up's call", ...)
```

So SMA-587's "closing them later is a flag flip" is not quite true: the flip reds `--self-test`
until that block is deleted. It is listed here rather than only in D6 because it is the one
invalidated assertion that fails the build.

**What replaces the reserved-row scaffolding.** With every row enabled, `check.py:632-638`'s
`reserved` fixture — which proves a disabled row still *works* when flipped on — proves nothing,
because it tests a row that is now live. It is kept and re-pointed at a **synthetic name not in
`BANNED`** (`violations_in(src, "<reserved>", {"Widget": "EnvelopeWidget"})`), so it still proves
the enable-a-row mechanism works for a *future* reserved row. Deleting it would remove the only
thing that keeps that mechanism honest.

**The `Query` row needs no special handling.** `_banned_pattern`'s identifier boundaries already
exclude `PageQuery`, `AuditQuery`, `MembershipQuery`, `RoleGrantQuery`, `ServiceAccountQuery` and
`DeadLetterQuery` — in each the `Query` is preceded by an identifier character — while
`Query<PageQuery>` matches on the outer name. `EnvelopeQuery` is excluded the same way
`EnvelopeJson` is.

**The `Path` row needs the tighter match its own comment warns about, and this reverses a
documented decision.** `check.py:331-333` currently states: *"No `<` is required after the name.
Requiring one would be narrower for no benefit … matching the bare identifier fails CLOSED."* The
benefit now exists, so the `BANNED` tuple grows a per-row flag requiring a following `<`; `Json`,
`Query` and `Bytes` keep bare-identifier matching. That comment is on D6's rewrite list.

Two corrections to how the first draft justified this. **Measured: there is no `std::path`,
`PathBuf` or `&Path` anywhere in either service's `adapters/http` tree today**, so the flag is
prophylactic, not a live collision. And `PathBuf` is *already* excluded by the existing trailing
negative lookahead (`Path` followed by `B`), so only bare `&Path` needs the flag — the promised
fixture proves less than the first draft implied. Fixtures pin `p: &Path` as legal and
`Path<String>` as a violation.

**The `BANNED` tuple is a 3-tuple unpacked at six sites**, so adding a fourth element is not a
one-line change: `_PATTERNS` (`:337`), `violations_in` (`:347`), and four `self_test` unpacks
(`:620`, `:624`, `:628`, `:640`). Stated so the estimate is honest.

**The `Bytes` row inverts an existing fixture.** `FIXTURES` currently carries
*"legal — a body taken as Bytes (the gateway's shape); see README Limitations L3"*. That fixture
**flips to a planted violation** and its L3 cross-reference goes with it. A reviewer meeting a
deleted "legal" fixture must be able to see it was reassigned rather than dropped.

#### D5.1 — `ALLOW` is per-FILE, and that is why `EnvelopeBytes` needs its own file

`check.py:385,403-404` builds `allowed = {path for path, _ in ALLOW}` and `continue`s on the whole
file. **An ALLOW row switches off every enabled extractor for that file**, not just the row that
motivated it — and there is deliberately no stale-row red beyond path existence
(`check.py:61-68`), so the exemption is never re-examined. `check.py:58-60` calls this "the one
structural way this check could come to guard nothing", so widening it is a cost, not a formality.

The first draft wrote "`path.rs` needs one **under the `Path` row**", which the table cannot
express. Corrected, and each new row states what it *also* exempts:

| new ALLOW row | why | also exempts that file from |
| -- | -- | -- |
| `…/paigasus-iam/src/adapters/http/query.rs` | the extractor's own definition site; it wraps `axum::Query` by construction | `Json`, `Path`, `Bytes` |
| `…/paigasus-gateway/src/adapters/http/bytes.rs` | the extractor's own definition site; it wraps `axum::Bytes` by construction | `Json`, `Query`, `Path` |

`path.rs` gets **no** row. `StringPath` reaches `Path::<String>::from_request_parts` in a function
*body*, which is outside every parameter span, and the `<`-requiring pattern does not match a
turbofish `Path::<String>` either — so the file is clean without an exemption, and adding a
defensive one would blanket-exempt the house path module from three other rows for no measured
need. This differs from `json.rs`'s defensive row, which SMA-587 justified separately; that row
stays.

**`chat.rs` must NOT be an ALLOW row.** This is the fail-open the file-naming in D4.2 exists to
prevent: `chat.rs:74` is the one `body: Bytes` the new row is being added to catch, so exempting
that file would make the gate report green over the unconverted handler. `EnvelopeBytes` goes in
its own file for exactly this reason — and that file must sit **inside** `SCAN_GLOB`
(`rs/crates/services/*/src/adapters/http/**/*.rs`), since widening the glob would also require
editing `SELF_TASK_EXPECTED_GLOBS["http-extractor-envelope"]`, an exact-match pin.

**The liveness check needs no change.** `LIVENESS_IDENT = "EnvelopeJson"` stays a valid positive
control — `EnvelopeJson` is still in every converted body position. (The first draft said
"fourteen"; `README.md:77` says eighteen and SMA-587 established seventeen routes. The conclusion
holds on any of those numbers, and the README's count is the one that must stay consistent.)

### D6 — Prose and assertions this invalidates

1. **`check.py:640-642`'s reserved-rows-are-off assertion** — deleted (D5). The build-breaking one.
2. **`check.py:632-638`'s `reserved` fixture** — re-pointed at a synthetic name (D5).
3. **`check.py:331-333`'s "no `<` is required" comment** — reversed for one row (D5).
4. **`check.py`'s "RESERVED, NOT FORGOTTEN" block** (`:38-49`) and its `NOTE for whoever flips Path
   on` describe work this ticket does. Both rewritten.
5. **`error.proto`'s 901 comment** says "Reconverging the two is SMA-588, not an accident". After
   D4.1 the two *are* reconverged; the comment states the shared meaning instead.
6. **`error.proto`'s 906 comment** gains a second emitter. It describes a body "syntactically valid
   JSON but did not match the target type", which is true of both services after D4.1 — but the two
   answer it on **different statuses** (IAM 422, gateway 400, decision 5). The comment says so, for
   the same reason the 901 comment recorded IAM's narrowing: a consumer mapping code → status needs
   to know it is not one-to-one.
7. **`error.proto`'s 905 comment** says 905 is HTTP-only because tonic negotiates
   `application/grpc`. Still true, but now incomplete: it must also say the gateway does not emit
   it because it takes `Bytes` and never reads `Content-Type` — otherwise the next reader will file
   the same reconvergence ticket again.
8. **`ci/http-extractor/README.md`** — L3 retired (below), plus four more sites the first draft
   missed: "What it does NOT gate" (`:29-35`, a third copy of the reserved-rows prose), the
   identifier-boundary paragraph (`:60-61`), "The ALLOW table — One row" (`:86`), and the
   "Eighteen request positions" positive-control line (`:77`).
9. **`chat.rs`'s module doc and `chat_completions`'s doc** both say the `DefaultBodyLimit` layer
   "fails oversized bodies with a 413 before this handler is reached", describing the escape as
   correct behaviour. Rewritten for `EnvelopeBytes` — and so are three more copies the first draft
   missed: `gateway/src/adapters/http/mod.rs:47` and `:85-86`, and `tests/chat_proxy.rs:16,227-228`.
10. **`dto.rs`'s query-DTO comments** — **two**, not four. `RoleGrantQuery` (`dto.rs:366-367`) and
   `ServiceAccountQuery` (`:415-416`) justify keeping a field as a raw `Option<String>` by
   contrasting it with "axum's default plain-text query rejection", which these routes no longer
   produce. `AuditQuery` (`:587-592`) and `DeadLetterQuery` (`:651-653`) reference the handler
   funnel and each other, never axum, and need no edit. The reasoning in the first two still holds
   — a handler-validated field gives a *better* reason than 907 — but the contrast must be restated
   against `EnvelopeQuery`.

**L3 is retired, not narrowed.** The first draft contradicted itself, telling the implementer both
to retire L3 and to narrow it to `String` (§ *Review corrections*, C5). Retiring wins: `String:
FromRequest` composes `BytesRejection` exactly as `Bytes` does, so a narrowed L3 would repeat the
same false "no rejection is produced" claim about `String`. No `String` request body exists in
either tree, so no extractor is written for one; § *Out of scope* records that a future one carries
the identical hole.

## Registry mechanics

Adding a reason touches **three** coupled sites. SMA-586 called the third "the single easiest step
to miss" and SMA-587's design missed it on its first draft:

1. `contracts/proto/paigasus/common/v1/error.proto` — two additive values, 907 and 908, each
   commented with its wire string, meaning and transport availability.
2. `rs/crates/libs/paigasus-proto/src/error.rs` — the hand-transcribed `EXPECTED_REASONS` list.
   Independent by design: `ci/error-registry/check.py` cross-checks the two transcriptions, which
   can only agree if both are right.
3. `rs/crates/libs/paigasus-proto/src/error.rs:228` —
   `assert_eq!(actual.len(), 55, …)` becomes **57**. The literal appears **twice**: the assertion
   at `:228`, and the doc comment at `:141` that names it as an anchor. Nothing asserts the second,
   so it goes stale silently unless edited with the first. Note also that the SMA-587 spec's
   "52 → 54" is already stale; the live value is **55** (39 IAM + 9 gateway + 7 shared). Missing the
   assertion fails `the_registry_contains_exactly_the_expected_reasons` in `paigasus-proto`, *not*
   in the crate under change, so it reads as an unrelated breakage.

## Cross-transport divergence

`the_recorded_transport_divergences_still_hold` (`grpc/convert.rs`) records four divergences today.
This design adds two, both HTTP-only:

- **907 `invalid-query-parameter`** — gRPC has no query string.
- **908 `invalid-path-segment`** — gRPC has no URL path segments carrying request data.

**"Structurally" would be an overstatement, and D2 is why.** For 905 and 906 the property is
enforced by the transport itself: tonic negotiates `application/grpc`, and proto3 has no
schema-invalid state, so no gRPC code path *can* produce them. 907 and 908 live in `pub enum
TenancyError`, which `status_to_grpc` maps unconditionally — so any gRPC handler in this crate
*could* construct one, and the proto comment declaring them HTTP-only would quietly become false.
The first draft asserted the property and planned to record it as a comment only, which would have
written an unenforced normative claim into the append-only registry (§ *Review corrections*, C6).

Both are therefore **asserted**, not merely recorded: a test scans `adapters/grpc/**` for
constructions of the two variants and fails on any hit. This is the same shape as divergence 2
(`mutually-exclusive-fields`), which is pinned at `convert.rs:1252` rather than left as prose.

*Its limitation, stated:* a source scan is defeated by an alias or a re-export, exactly as
`ci/error-registry/check.py` documents for its own scan. It catches the realistic case — a handler
reaching for a convenient existing variant — and nothing more. The proto comments say "HTTP-only;
enforced by a source scan, not by the transport" rather than borrowing 905's stronger wording.

## Compatibility

**No route's status changes anywhere in this ticket** — in either service. Every new extractor
preserves the rejection's own status; all twelve IAM escapes are 400 today and 400 after; the
gateway's schema failures stay 400 (decision 5) and its oversized body stays 413. This restores the
property SMA-587 stated for its own work, which the first draft's 422 would have broken.

**One IAM wire change.** The twelve routes' refusal bodies change from axum's plain text to the
`{"error":{code,message}}` envelope. The first draft claimed a second — that these routes gain the
`paigasus-retryable` header — and that is **false**: `paigasus_observability::CorrelationLayer`
wraps the whole IAM app subtree (`adapters/http/mod.rs:906`) and fills a default on any error
response lacking the header (`correlation.rs:176-182`), using `Retryable::from_status`, which
returns `No` for a 400. These routes already answer `paigasus-retryable: false` and the value is
unchanged. (The contract test the first draft cited as `authn.rs:330-341` is at `authn.rs:166-177`;
that file is 191 lines.)

**One gateway code change, wider than it first appears.** `POST /v1/chat/completions` answers
`invalid-request-schema` instead of `invalid-request-body` for every `Category::Data` failure, on an
unchanged 400. Measured, that set includes not only a genuine schema mismatch but also a **scalar,
`null` or array body** — `"hello"`, `42`, `true`, `null`, `[]` — because `ChatCompletionRequest` is
a struct and any non-object is an `invalid type` data error. A syntactically malformed body keeps
`invalid-request-body`.

This is a change only a client reading `error.code` can observe. `error.rs:6-8` calls the envelope
"a compatibility contract" because "SDKs (the OpenAI client libraries our callers use) branch on
`error.type`" — and `type` is `invalid_request_error` before and after. The OpenAI SDKs also map
**status** to an exception class (400 → `BadRequestError`, 422 → `UnprocessableEntityError`), which
is why decision 5 kept 400: on 422 every one of these bodies would have changed exception class in
a caller's `except` block.

**One gateway body-shape change.** An oversized body answers inside the OpenAI envelope with
`request-too-large` instead of axum's plain text. The status stays 413.

**Refusal still precedes authorization.** `EnvelopeQuery` and `StringPath` reject before the
application-layer authorization inside `PolicyService::list` / `SystemRetirementService::retire`, so
an authenticated-but-unauthorized caller learns a malformed query or segment via 907/908 rather than
403. This is **unchanged** — axum's 400 does the same today — and is recorded only so the change is
not read as introducing it.

## Testing

Following SMA-587 D5's rule, which SMA-586 learned expensively: a synthetic
`Router::new().route(…)` proves the extractor, **not** the handler wiring, and that is exactly how
a mis-named `{sa}` segment survived SMA-586's entire suite. Route coverage runs against the **real
merged `router(...)`**.

| Level | Coverage |
| -- | -- |
| `query.rs` unit | a value-parse failure and a **repeated key** each map to 907 in the envelope; a well-formed query still extracts; a non-client-error rejection keeps axum's own response |
| `path.rs` unit | `StringPath` maps a non-UTF-8 segment (`/x/%FF`) to 908 naming `policy_id`; a well-formed segment extracts; a router-arity bug keeps its own 500 — mirroring `UuidPath`'s three existing tests |
| `path.rs` unit | `the_path_field_names_are_stable` gains a `PolicyId` row. It gets **no count assertion**: `path_field!` is a `macro_rules!` macro and those cannot accumulate across invocations, so there is no list to count against and any "count" here would compare a literal to itself — passing with every row deleted. The test's doc states that limit instead. Closing it means collapsing nine declarations into one registry-shaped invocation, which is more than this ticket justifies |
| IAM integration | each of the ten `Query` routes asserts a rejection → 400 `invalid-query-parameter`, and each table ends with a **well-formed query on the same route reaching the handler**, so every row asserts the query's shape and not a broken route. Both `Path<String>` routes assert a non-UTF-8 segment → 400 `invalid-path-segment` |
| gateway | the 901/906 split — a syntax error → 400/901, a type mismatch → 400/906, a bare scalar → 400/906 — asserting the **status is 400 on every row**, since that is the decision a future reader is most likely to undo; plus an oversized body → 413/`request-too-large` inside the OpenAI envelope |
| gRPC | the two new divergences: no `adapters/grpc/**` source constructs `InvalidQueryParameter` or `InvalidPathSegment` |
| gate | `--self-test` fixtures for the three newly-enabled rows and the new one, the `Path` row's `<` requirement (`&Path` legal, `Path<String>` a violation), the inverted `Bytes` fixture, and the re-pointed `reserved` fixture |

**Which probe each `Query` row uses.** Nine of the ten routes carry a numeric field and use
`?limit=abc` — every one except `list_role_grants`.
`RoleGrantQuery` (`list_role_grants`) carries **no numeric field**, so `?limit=abc` there is an
ignored unknown key and answers 200 — it must use a **repeated key**
(`?principal_prn=a&principal_prn=b`). `AuditQuery`/`DeadLetterQuery` carry `Option<u64>` and either
probe works. The first draft concluded from the first half of this that `list_role_grants` "is not
reachable by a 907 at all" and told the implementer to write a weaker row asserting
missing-required-field instead; that row would never have exercised `EnvelopeQuery` — the
"passes for the wrong reason" failure the same paragraph claimed to prevent (§ *Review
corrections*, C1). Keep the missing-required-field assertion as a **separate** row.

**Prerequisites the implementer must not rediscover:**

- **Every route is behind `require_bearer`** (`adapters/http/mod.rs:889`), a `route_layer` that runs
  *before* any extractor. An integration row without a valid bearer asserts a 401, not a 907.
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
- **Docker.** The IAM suites inherit `tests/support/docker.rs`'s policy — they skip when the daemon
  is unreachable, with `tests/docker_preflight.rs` as the canary turning a Docker-less run into one
  loud red rather than silent passes.

## Verification

- `moon ci` over the full marker-delimited target list in `CLAUDE.md`, not per-project tasks — this
  change touches `repo:http-extractor-envelope`, `repo:error-code-single-site` and `repo:breaking`.
- `repo:error-code-single-site` green: the two new codes resolve via `ErrorReason::from_wire_reason`
  and every emission site is on the MANIFEST (no new rows needed — D2).
- `buf breaking` clean: both proto changes are additive enum values plus comment edits.
- `repo:http-extractor-envelope` green with all four rows on, and its `--self-test` proving each
  newly-enabled row can red.

### No gate plumbing is required — verified, not assumed

This repo's gates guard each other densely and a new gate normally carries five registry
obligations. **This change needs none of them**, and the reasoning is recorded because "we checked
and the answer was nothing" is otherwise indistinguishable from "we forgot":

- `repo:http-extractor-envelope` is **already** in `ci.yml`'s `T=(…)` array and CLAUDE.md's
  marker-delimited command, and already in both `SELF_TASK_EXPECTED_GLOBS`
  (`ci/affected-graph/ci_targets.py:263-266`) and `SELF_SCHEDULED_GATES` (`:475-478`). Both are
  exact-match pins, and this change edits neither the task's `inputs` nor its three invocation
  lines, so both stay green. **SMA-587's D4.2 claim that this gate "is not script-pinned" and needs
  no entry in either registry is stale** and should be corrected there when convenient.
- `repo:affected-smoke` is unaffected: `lockfile->all-lint` (`ci/affected-graph/run.sh:339-345`) and
  `kernel->bindings` (`:280-281`) enumerate crates and tasks, and this change adds source files to
  two **existing** crates and no new crate.
- `repo:input-liveness` sees no new declaration, and `error.proto` is already an input of
  `repo:error-code-single-site`.

The one thing that *would* require plumbing is widening `SCAN_GLOB`, which D5.1 forbids by putting
`EnvelopeBytes` inside the existing tree.

## Out of scope

- **The `Query`/`Path` rows in other services.** `paigasus-gateway`'s `adapters/http` tree contains
  no `Query` or `Path` extractor at all — measured — so the two rows are green there on day one and
  the gate stops it growing the same hole.
- **A `String` request body.** `String: FromRequest` composes `BytesRejection` and therefore carries
  the identical 413 hole `EnvelopeBytes` closes for `Bytes`. No `String` request body exists in
  either tree today, so no extractor and no `BANNED` row is written for one — a row matching nothing
  cannot be exercised against the real tree. A future `String` body needs the same treatment, and
  the retirement of L3 (D6) is what stops the README claiming otherwise.
- **`repo:http-extractor-envelope`'s remaining residuals.** README L1 (an aliased import,
  `use axum::Json as J`), L2 and L4 are unchanged and stay recorded. Only L3 goes, because it is
  wrong rather than merely a limit.
- **`param` on the gateway's `invalid-request-schema`.** Populating it needs `serde_path_to_error`
  in the gateway's parse path (D4.2). Worth doing; not here.
- **Status parity for 906.** IAM answers 422 and the gateway 400, by decision 5. Harmonising them
  means changing one service's status on a live endpoint, which is a wire decision of its own and
  not something to fold into a reconvergence of `error.code`.
- **Handler-validated query fields.** `RoleGrantQuery::principal_prn`,
  `ServiceAccountQuery::owner_prn`, `AuditQuery`'s timestamps and cursors, and `Page::new`'s range
  checks keep funnelling through their existing, more specific `TenancyError`s. 907 is for a failure
  that happens *before* a handler runs; replacing a specific reason with a general one would be a
  regression.

## Review corrections

An adversarial review (Opus, 2026-08-29) returned **NEEDS REWORK** against the first draft. Every
finding below was verified against the tree before folding in — three by running a probe, the rest
by reading the cited source. Recorded so a reviewer can see what changed and why, per SMA-587's
precedent.

| # | Severity | Finding | Resolution |
| -- | -- | -- | -- |
| C1 | BLOCKER | The reachability analysis missed the **repeated-key** class, concluding `list_role_grants` was unreachable and instructing a weaker test row | § *Reachability* re-derived from a measured probe; § *Testing* gives that route a real 907 row |
| C2 | MAJOR | D1.1's premise ("the rejection cannot name the parameter") is false — axum uses `serde_path_to_error` | D1.1 rewritten around the real reason: `TenancyError`'s `&'static str` invariant |
| C3 | MAJOR | The exhaustive-match list was wrong (`tenancy_retryable` keys on `ErrorClass`) and omitted `field()` and its gRPC metadata consequence | New D2.1 |
| C4 | MAJOR | D4.1 needs a **second** new `GatewayError` variant, never declared, with an unspecified `type`/`param`/`retryable` | D4.2's table now carries both, fully specified |
| C5 | MAJOR | D6 and § *Out of scope* gave contradictory instructions for README L3 | Retired, not narrowed; the `String` case moved to § *Out of scope* |
| C6 | MAJOR | Nothing enforced the "HTTP-only" property once the reasons live in `TenancyError`, yet the spec planned to write it into the append-only registry | § *Cross-transport divergence* now asserts it with a source scan and states that scan's limit |
| C7 | MAJOR | `check.py:640-642` asserts the reserved rows are off — the flip reds the gate's own self-test | D5 leads with it; D6 lists it first |
| C8 | MAJOR | `ALLOW` is per-**file**, not per-row, so the proposed rows exempt more than intended | New D5.1, with a table of what each row also exempts; `path.rs` row dropped as unneeded |
| C9 | MAJOR | `EnvelopeBytes`'s home was unstated, and putting it in `chat.rs` would fail the gate **open** on the one file the new row targets | D4.2 names the file; D5.1 forbids a `chat.rs` row |
| C10 | MAJOR | The 422 decision was never weighed against OpenAI SDK compatibility, and `#[serde(flatten)]` widens it to scalar/`null`/array bodies | Raised at spec approval and **decision 5 was reversed to 400**. Only `error.code` moves; no route's status changes in either service |
| C11 | MAJOR | The `paigasus-retryable` "unclaimed win" does not exist — `CorrelationLayer` already supplies the header | Claim deleted from § *Compatibility*; citation corrected |
| C12 | MINOR ×8 | `BANNED` 3-tuple fan-out · the `<` flag reverses a documented comment · `PathBuf` already excluded · three more copies of the invalidated `chat.rs` prose and four more README sites · only two of four DTO comments are affected · the registry count appears twice · `the_path_field_names_are_stable` has no count assertion · both extractors must be `FromRequestParts` · bearer auth is a test prerequisite · D4.2's 413 mechanism sentence was misleading | All folded into D3, D4.2, D5, D6, § *Registry mechanics* and § *Testing* |

Two review findings were **not** folded in as stated:

- The review asked whether `serde_json::Error::classify()` survives `#[serde(flatten)]`. It does —
  measured, and the table is now in D4.1 — so no change beyond adding the evidence.
- The review suggested the `reserved` fixture might simply be deleted once all rows are on. Keeping
  it, re-pointed at a synthetic name, preserves the property it was written for at no cost (D5).
