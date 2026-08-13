# SMA-498 — Canonical error code registry in `common/v1/error.proto`

Linear: [SMA-498](https://linear.app/smaschek/issue/SMA-498/contracts-canonical-error-code-registry-in-commonv1errorproto)
ADR: [ADR-0019 — Canonical error model](https://app.notion.com/p/3bb830e8fbaa8152aa01e1aec0160bab) (accepted 2026-08-13)
Blocks: [SMA-504](https://linear.app/smaschek/issue/SMA-504) (adopt `google.rpc.ErrorInfo` in iam+gateway),
[SMA-507](https://linear.app/smaschek/issue/SMA-507) (two-way drift gate)

## 1. Context

### 1.1 Every error code Paigasus emits today

ADR-0019 says a real error vocabulary exists on "exactly one" surface. That is not accurate, and
designing from `TenancyError::code()` alone under-registers the vocabulary by 13 codes. There are
**six** emission sites across the two service crates:

| # | Site | Codes | Casing |
| --- | --- | --- | --- |
| 1 | `paigasus-iam` `adapters/http/error.rs:35` | the 26 from `TenancyError::code()` | kebab |
| 2 | `paigasus-iam` `adapters/http/authn.rs:53` | `invalid_token`, `identity_not_provisioned`, `provisioning_failed`, `principal_inactive`, `unavailable`, `internal` | **snake** |
| 3 | `paigasus-iam` `adapters/http/authn.rs:83` | `request_too_large`, `invalid_request` (extractor rejections) | **snake** |
| 4 | `paigasus-iam` `adapters/http/system_retirement.rs:147` | `grants-survive`, `decision-change-unacknowledged` | kebab |
| 5 | `paigasus-gateway` `adapters/http/error.rs:88` | 8 snake codes, plus a **`null`** for `GatewayError::Internal` | **snake** |
| 6 | `paigasus-gateway` `adapters/http/chat.rs:52` | `upstream_error`, in the terminal SSE frame | **snake** |

Sites 2–6 are all tested against their current spellings (e.g. `authn.rs:223`, `authn.rs:250`,
`system_retirement.rs:294`, `chat.rs:206`), so every recasing below is a real, test-visible wire
change — not a cosmetic one.

The gRPC surfaces add no codes of their own: `status_to_grpc` reuses site 1's vocabulary in-band,
and `authn_status` (`adapters/grpc/convert.rs:53`) emits no machine code at all.

### 1.2 What this issue delivers

ADR-0019 decision 8 places the registry in `contracts/proto/paigasus/common/v1/error.proto`. This
issue builds **only the registry** — the vocabulary and its generated symbols. It changes no error
path: SMA-504 emits `ErrorInfo` and performs the renames, SMA-507 gates drift.

## 2. Findings that constrain the design

Four facts were established experimentally in the worktree before designing. Three contradict a plain
reading of the issue's scope notes, so each is recorded with its evidence.

### 2.1 Referencing `google/rpc/error_details.proto` silently produces broken output

The issue says to "add `google/rpc/error_details.proto` to `buf.yaml` deps if not already reachable
via `buf.build/googleapis/googleapis`". It **is** already reachable — `buf.lock` pins googleapis and
buf resolved the import without complaint. The problem is *referencing* it.

A probe proto containing `google.rpc.ErrorInfo info = 1;` was generated. `buf generate` **exited 0**
and produced:

- **Rust (prost):** `pub info: ::core::option::Option<super::super::super::google::rpc::ErrorInfo>`
  — no `google/rpc` module is generated. Compile error.
- **TypeScript (protobuf-es):** `import type { ErrorInfo } from "../../../google/rpc/error_details_pb.js"`
  — that file is not generated. `tsc` failure.
- **Python (betterproto2):** generated `google/rpc/__init__.py` locally. Works.

`google.protobuf.Timestamp` works in `audit.proto` only because it is a *well-known type*, special-
cased by both plugins (TS resolves it to `@bufbuild/protobuf/wkt`, prost to `prost-types`).
`google.rpc.ErrorInfo` is not a WKT and gets no such treatment.

**Therefore: `error.proto` imports nothing.** `(domain, reason)` is described in prose. See R2 for the
consequence this pushes onto SMA-504, which is not free.

### 2.2 No language generates the wire string

An enum-only probe generated cleanly in all three languages, and doc comments carried through to
every one (Rust `///`, TS JSDoc, Python docstring). `buf lint` and `buf format --exit-code` both
passed. But the generated symbols are name-based, not value-based:

| | Symbol | Proto name | Kebab string |
| --- | --- | --- | --- |
| Rust (prost) | `ProbeReason::SlugConflict` | `as_str_name()` → `"PROBE_REASON_SLUG_CONFLICT"` | **No** |
| TypeScript | `ProbeReason.SLUG_CONFLICT` | via `ProbeReasonSchema` | **No** |
| Python | `ProbeReason.SLUG_CONFLICT` | `betterproto_value_to_renamed_proto_names()` | **No** |

The `ERROR_REASON_X_Y` → `x-y` transform must be implemented. D4 decides where.

### 2.3 `buf breaking` rejects a *reserved* enum value

`contracts/buf.yaml:15-26` uses the `FILE` breaking category and already swaps `FIELD_NO_DELETE` for
the reserve-tolerant `FIELD_NO_DELETE_UNLESS_NAME_RESERVED` / `_NUMBER_RESERVED` (SMA-444). The
**`ENUM_VALUE_` siblings were never swapped**, so `ENUM_VALUE_NO_DELETE` is active.

Verified by temporarily retiring `NODE_STATUS_ARCHIVED` with both `reserved 2;` and
`reserved "NODE_STATUS_ARCHIVED";`:

```
proto/paigasus/iam/v1/iam.proto:24:1: Previously present enum value "2" on enum "NodeStatus" was deleted.
```

Without a `buf.yaml` change, **a wrong spelling in the registry can never be retracted**. D2 fixes
this, mirroring the precedent the repo already set for fields.

### 2.4 The codegen-drift gate is not a Moon task, and prost embeds comments

AC4's gate is a **step in `.github/workflows/ci.yml:194-207`** (`moon run contracts:generate` then
`git diff --exit-code` over the three generated trees), not a Moon task. It is absent from the
`moon ci` target list in `CLAUDE.md`, so the documented full-graph command does **not** cover it.

Separately, prost's `FILE_DESCRIPTOR_SET` embeds `SourceCodeInfo` — every doc-comment byte appears as
hex in the generated Rust (confirmed: the text of `audit.proto`'s header is readable as hex bytes in
`paigasus.common.v1.rs`). Per-value comments are therefore kept to **one short line**, and §4.4's
normative header is kept tight, with the long-form rationale living here rather than in the proto.

## 3. Decisions

### D1 — The registry is a proto `enum`; the wire stays a `string`

proto3 has no string constants. Three shapes were considered:

- **Comment-only registry** — a structured doc block, no declarations. Closest to ADR-0019's literal
  wording, and generates zero churn. Rejected: it generates *nothing*, making AC1 vacuous, leaving
  the console and SDK with no symbols, and forcing SMA-507's gate to parse proto comments.
- **Enum + a parallel "literal strings" message** — pins the kebab spellings as generated data.
  Rejected: two lists to keep in sync, and the message is never instantiated in any language.
- **Enum as registry, string on the wire** — chosen.

`ErrorReason` and `ErrorDomain` are **registries, never wire types**. No field of either type exists
in any `.proto`. The wire carries `google.rpc.ErrorInfo.reason` / `.domain` as strings, exactly as
ADR-0019 decision 2 requires.

**This partially deviates from ADR-0019, and the deviation should be acknowledged rather than argued
away.** The ADR rejects "a proto enum for error codes" for two reasons. The second — an old client
receiving an opaque number instead of a loggable name — genuinely does not apply, because the wire
stays a string and an old client still receives `"slug-conflict"` verbatim. **The first does apply
in full:** adding a code regenerates bindings in all three languages, and the codegen-drift gate
requires committing that churn. Concretely, adding one code means editing `error.proto`, running
`contracts:generate`, and committing three generated files — including a multi-line hex delta in the
Rust `FILE_DESCRIPTOR_SET` (§2.4).

That cost is accepted here because codes are added rarely, the churn is confined to generated trees,
and the alternative (a comment-only registry) gives the console and the drift gate nothing to hold
onto. But it is a cost the ADR declined, so **this warrants either an ADR-0019 amendment note or an
explicit sign-off recorded against this spec** — `CLAUDE.md` requires significant choices to go
through an ADR.

### D2 — Make enum values retractable, and don't mistake `buf breaking` for the guard

`contracts/buf.yaml` gains the enum-value counterparts of the field rules it already carries:

```yaml
breaking:
  use:
    - FILE
    - FIELD_NO_DELETE_UNLESS_NAME_RESERVED
    - FIELD_NO_DELETE_UNLESS_NUMBER_RESERVED
    - ENUM_VALUE_NO_DELETE_UNLESS_NAME_RESERVED     # new
    - ENUM_VALUE_NO_DELETE_UNLESS_NUMBER_RESERVED   # new
  except:
    - FIELD_NO_DELETE
    - ENUM_VALUE_NO_DELETE                          # new
```

This is not a loosening — it is the same reserve-and-retire escape hatch SMA-444 established for
fields, applied to the construct this issue introduces. Without it, §2.3 shows the 15 forward-
declared spellings (D7) are permanent on first commit.

Because the registry is an enum, `buf breaking` now also catches a *deleted enum value*. That is a
welcome side effect, not the protection: it cannot see the kebab strings, cannot tell whether a code
is still emitted by Rust or consumed by the console, and does not run against non-proto sources.
SMA-507's two-way gate remains the real guard, and §4.4 says so in the file itself so nobody later
mistakes green `buf breaking` for a checked registry.

### D3 — One flat `ErrorReason` enum, with number ranges encoding the emitting domain

ADR-0019 decision 1: "One vocabulary underneath them." Per-domain enums would duplicate genuinely
shared codes and complicate SMA-507's per-crate check.

But SMA-507's "emitted ⊆ registry" direction is inherently per-crate: it must answer *may the gateway
emit `slug-conflict`?* A flat enum with comment banners cannot answer that without hardcoding the
mapping in a shell script. Number ranges can, and cost nothing:

| Range | Meaning |
| --- | --- |
| 1–299 | IAM only (`iam.paigasus.io`) |
| 300–599 | gateway only (`gateway.paigasus.io`) |
| 900–999 | **shared** — any domain may emit |

Ranges rather than per-domain enums because two codes are genuinely shared: `internal` (both
services) and `invalid-request-body` (D8). SMA-507 derives the check arithmetically: a code is
legal for domain *D* if its number is in *D*'s range or in the shared range. The scheme is
documentation until SMA-507 enforces it.

The IAM sub-groupings (tenancy / authn / retirement) are comment banners only, and carry no
numbering meaning — a future IAM code takes the next free number in 1–299 regardless of which banner
it sits under.

### D4 — The transform lives in `paigasus-proto`, derived rather than tabulated

`paigasus-proto` gains a hand-written module `src/error.rs`, following the existing `src/audit.rs`
precedent of layering a thin contract over generated types. Both helpers are **inherent impls** on
the generated enums (legal — the enums are local to the crate), so callers need no extra `use`:

```rust
ErrorReason::SlugConflict.as_wire_reason()      // Some("slug-conflict".to_string())
ErrorReason::from_wire_reason("slug-conflict")  // Some(ErrorReason::SlugConflict)
ErrorDomain::Iam.as_wire_domain()               // Some("iam.paigasus.io".to_string())
ErrorDomain::from_wire_domain("iam.paigasus.io")
```

Both directions are **derived** from prost's `as_str_name()` / `from_str_name()` by applying the D6
rules, not written as a 44-arm match. A match would be a second copy of the registry inside Rust —
precisely the "three unlinked places" failure ADR-0019 cites from the observability metrics. The
alternative (a match table plus a test proving it equals the derivation) is zero-allocation in both
directions and was rejected only because it is more code for a path that runs once per error
response; §6's completeness test makes either choice equally safe.

Both `as_wire_reason` and `from_wire_reason` allocate a `String`. This is deliberate and cheap: a
borrowed `&'static str` would require a const table, i.e. the second list this decision exists to
avoid, and `tonic_types::ErrorDetails::with_error_info` takes `impl Into<String>` — so `String` is
exactly what the consumer wants.

**Both directions return `Option`,** and the two are exact inverses. `as_wire_reason` returns `None`
for `ErrorReason::Unspecified`, which is reachable because it is the prost `Default`. The sentinel
exists only to satisfy buf's `ENUM_ZERO_VALUE_SUFFIX` lint rule; it is not a code any surface emits,
so emitting `"unspecified"` on the wire would be a bug. Returning `None` makes that a caller-visible
decision rather than a silent one.

### D5 — Both parsers validate before transforming

Reconstructing a proto name from an arbitrary string admits inputs that are not valid wire reasons.
Rejecting them by *listing* forbidden characters is not sufficient: `"ınternal"` (U+0131 dotless i)
contains no `_` and no ASCII uppercase, yet `str::to_uppercase()` folds it to `"INTERNAL"`, which
would resolve to `ErrorReason::Internal`. `"ſlug-conflict"` (U+017F) behaves the same way.

So validation is a **positive** match, applied before any transform:

- `from_wire_reason` requires `^[a-z][a-z0-9]*(-[a-z0-9]+)*$`.
- `from_wire_domain` requires the same shape followed by the literal suffix `.paigasus.io`.
- Both then use `to_ascii_uppercase`, never `to_uppercase`.
- Both reject the zero sentinel: `"unspecified"` → `None`.

A lenient parser here would quietly widen SMA-507's gate, letting a misspelled emitted code pass the
"emitted ⊆ registry" check — which is the gate's whole purpose.

### D6 — Two mechanical mapping rules, stated normatively in the proto

- **Reason:** strip the `ERROR_REASON_` prefix, lowercase, replace `_` with `-`.
- **Domain:** strip the `ERROR_DOMAIN_` prefix, lowercase, replace `_` with `-`, append
  `.paigasus.io`. The `_`→`-` step is stated even though today's two values are single-word, so a
  later `ERROR_DOMAIN_MODEL_ROUTER` yields `model-router.paigasus.io` and stays parseable by D5's
  regex — `model_router.paigasus.io` would not be.

Every value's doc comment repeats the resulting literal verbatim as its first token, on one line
(§2.4): `// "slug-conflict" — slug already taken in this scope.` The comment is the human-readable
contract and reaches all three generated languages (§2.2); the rule is the machine-readable one.

All 26 current `TenancyError::code()` values round-trip through the reason rule with no exceptions,
so AC2 holds without a single special case.

### D7 — The registry declares canonical spellings ahead of every emitter

ADR-0019 keeps all four envelopes intact and puts the canonical reason in the code field. The
registry therefore declares kebab throughout, and **15 of the 43 codes are spellings no emitter
produces yet**, reached by the 17 rename operations below. The count differs because two rows
converge on the same new spelling (D8's `invalid-request-body` merge) and one targets `internal`,
which tenancy already emits. Every rename SMA-504 must perform:

| Site | Today | Registered | Note |
| --- | --- | --- | --- |
| iam authn HTTP | `invalid_token` | `invalid-token` | see RFC 6750 caveat below |
| iam authn HTTP | `identity_not_provisioned` | `identity-not-provisioned` | |
| iam authn HTTP | `provisioning_failed` | `provisioning-failed` | |
| iam authn HTTP | `principal_inactive` | `principal-inactive` | |
| iam authn HTTP | `unavailable` | `authn-unavailable` | **rename, not recasing** (D8) |
| iam extractor | `request_too_large` | `request-too-large` | |
| iam extractor | `invalid_request` | `invalid-request-body` | merged with the gateway's (D8) |
| gateway | `missing_authorization` | `missing-authorization` | |
| gateway | `invalid_api_key` | `invalid-api-key` | |
| gateway | `insufficient_permissions` | `insufficient-permissions` | |
| gateway | `missing_scope` | `missing-scope` | |
| gateway | `iam_unavailable` | `iam-unavailable` | |
| gateway | `invalid_request_body` | `invalid-request-body` | |
| gateway | `upstream_unavailable` | `upstream-unavailable` | |
| gateway | `upstream_timeout` | `upstream-timeout` | |
| gateway | `upstream_error` (SSE) | `upstream-error` | terminal SSE frame, `chat.rs:52` |
| gateway | `null` (`Internal`) | `internal` | **null → string** (D8) |

`type` is untouched on the OpenAI envelope and keeps its OpenAI semantics — ADR-0019 notes SDKs
branch on `type`, and the Paigasus-specific `code` values appear in no OpenAI SDK.

**RFC 6750 caveat.** `authn.rs:25` also emits `invalid_token` inside the `WWW-Authenticate` header
(`Bearer error="invalid_token"`). That value is standardised by RFC 6750 §3.1 and is **not ours to
rename**; only the JSON body's `code` becomes `invalid-token`. SMA-504 must leave the challenge
header alone, and the registry's doc comment for this value says so.

**Consequence:** between this issue and SMA-504 the registry describes spellings the services do not
emit. That is deliberate, and it is why §6 adds no emitter-side test beyond the tenancy codes, and
why §10 constrains SMA-507's sequencing.

### D8 — Reuse `internal`, merge `invalid-request-body`, keep the three "unavailable" codes distinct

`AuthnError::Backend` and `GatewayError::Internal` both map to the existing `internal` rather than
minting synonyms. For the gateway this also fixes a real gap: `GatewayError::Internal` currently
emits a **`null`** `code` (`gateway/.../error.rs:110`, asserted at `:197`), so a caller gets no
machine-readable identity at all for a 500.

IAM's `invalid_request` and the gateway's `invalid_request_body` are the same failure — a body that
failed to deserialize — with two names. They merge into `invalid-request-body`.

Three codes that read as near-synonyms stay separate because they name different failures:

| Code | Meaning |
| --- | --- |
| `authn-unavailable` | IAM's own authentication backend is unreachable |
| `iam-unavailable` | the gateway cannot reach IAM |
| `upstream-unavailable` | the gateway cannot reach the model provider |

IAM's bare `unavailable` becomes `authn-unavailable` for exactly this reason: unqualified, it reads
as a generic service-down code and would collide in meaning with the other two the moment a console
renders both.

Likewise the gateway's `insufficient-permissions` is **not** folded into IAM's `forbidden`. They
share an HTTP status but not a surface or an audience, `domain` already separates them, and merging
them would be a further wire change on top of the seventeen D7 already commits to.

## 4. The registry

`contracts/proto/paigasus/common/v1/error.proto`, package `paigasus.common.v1`, **no imports**,
SPDX header first line per `CLAUDE.md`.

### 4.1 `ErrorDomain` — 2 values

| Value | Number | Wire string |
| --- | --- | --- |
| `ERROR_DOMAIN_UNSPECIFIED` | 0 | — (sentinel, never emitted) |
| `ERROR_DOMAIN_IAM` | 1 | `iam.paigasus.io` |
| `ERROR_DOMAIN_GATEWAY` | 2 | `gateway.paigasus.io` |

`ErrorDomain` is **not** emitted on the gateway's OpenAI envelope, which has no domain field — that
surface carries only `code`. It applies wherever `ErrorInfo` itself is carried: gRPC
`grpc-status-details-bin` trailers, logs, and IAM's HTTP body if SMA-504 extends it there.

### 4.2 `ErrorReason` — 43 values

**IAM only, 1–299.** Tenancy (1–25), verbatim from `TenancyError::code()` in declaration order,
minus `internal` which is shared (D8):

`slug-conflict`, `duplicate-membership`, `email-conflict`, `service-account-name-conflict`,
`invalid-email`, `invalid-slug`, `invalid-name`, `invalid-prn`, `prn-mismatch`, `invalid-pagination`,
`nothing-to-rename`, `not-found`, `parent-archived`, `node-archived`, `missing-org-membership`,
`forbidden`, `unknown-role`, `invalid-scope`, `system-immutable`, `policy-invalid`, `policy-conflict`,
`invalid-action`, `invalid-bulk-replay`, `not-system-owned`, `fleet-not-converged`

Authn (26–30): `invalid-token`, `identity-not-provisioned`, `provisioning-failed`,
`principal-inactive`, `authn-unavailable`

HTTP envelope (31): `request-too-large`

System retirement (32–33), already kebab today: `grants-survive`, `decision-change-unacknowledged`

**Gateway only, 300–599** (300–307): `missing-authorization`, `invalid-api-key`,
`insufficient-permissions`, `missing-scope`, `iam-unavailable`, `upstream-unavailable`,
`upstream-timeout`, `upstream-error`

**Shared, 900–999** (900–901): `internal`, `invalid-request-body`

Plus the mandatory `ERROR_REASON_UNSPECIFIED = 0` sentinel — **44 declarations**.

### 4.3 Provenance

Every value traces to a site in §1.1. **28** of the 43 already exist verbatim on the wire — 25
tenancy, `internal`, and the 2 retirement codes. The remaining **15** are D7 renames SMA-504
performs. 28 + 15 = 43.

### 4.4 File-level doc comment

Kept short (§2.4). It states:

1. Error identity is `(domain, reason)`, carried on the wire as `google.rpc.ErrorInfo` — which this
   file deliberately does not import, with a pointer to this spec's §2.1.
2. Both D6 mapping rules, and that each value's comment repeats its literal.
3. Kebab casing is a deliberate, documented deviation from Google's UPPER_SNAKE `reason` convention
   (ADR-0019 decision 3).
4. **The registry is append-only. Removing a value is a breaking change** — and `buf breaking` is not
   what protects the vocabulary; SMA-507 is (D2).
5. The D3 number ranges.
6. The two standard `ErrorInfo.metadata` keys SMA-504 will populate — `retryable` and
   `correlation_id`. Documented in prose, not enumerated: `metadata` is an open map, not a closed
   vocabulary consumers branch on exhaustively. **The append-only guarantee in (4) covers reasons and
   domains only, not metadata keys** — stated explicitly so SMA-507 need not guess.

## 5. Files touched

| Path | Change |
| --- | --- |
| `contracts/proto/paigasus/common/v1/error.proto` | new — SPDX header first line |
| `contracts/buf.yaml` | +2 breaking rules, +1 except (D2) |
| `rs/.../paigasus-proto/src/generated/paigasus/common/v1/paigasus.common.v1.rs` | regenerated — prost emits one file per *package*, so the enums land beside `AuditMetadata`; **`lib.rs` needs no `include!` change** |
| `ts/packages/paigasus-proto/src/generated/paigasus/common/v1/error_pb.ts` | new (generated) |
| `py/.../paigasus_proto/generated/paigasus/common/v1/__init__.py` | regenerated — betterproto2 merges by package |
| `rs/.../paigasus-proto/src/error.rs` | new, hand-written (D4) — SPDX header first line |
| `rs/.../paigasus-proto/src/lib.rs` | one line: `pub mod error;` |
| `rs/.../paigasus-iam/src/adapters/grpc/convert.rs` | test module only (§6) |

Named `error.rs`, not `error_reason.rs`, because it hosts both `ErrorReason` and `ErrorDomain`
helpers.

No `Cargo.toml`, `buf.gen.yaml` or `moon.yml` change, and **no new dependency in any workspace** — so
`cargo-deny` needs no waiver and `cargo-machete` no allowlist entry. (SMA-504 is a different story:
see R2.)

The TypeScript barrel (`ts/packages/paigasus-proto/src/index.ts`) is **not** touched. Re-exporting
`ErrorReason` would hand consumers a symbol with no way to obtain its wire string until the SDK issue
adds a TS transform, and the existing barrel exports each type alongside its `…Schema` descriptor —
so a half-export now would be four lines of surface that nothing can use.

## 6. Testing

**In `paigasus-proto` (`src/error.rs`):**

- **Completeness.** A test holds the full expected list of 43 wire strings and asserts *set equality*
  against `as_wire_reason()` over every non-`Unspecified` `ErrorReason` variant. Same for the 2
  domains. This deliberately duplicates the registry — in a test, which is the right place for a
  redundant assertion, and it is what makes a typo such as `ERROR_REASON_UPSTREAM_TIMOUT` fail
  instead of shipping green. Without it, §6's other tests are self-consistent by construction and 17
  of the 43 codes would have zero coverage.
- **Round-trip.** For every variant except `Unspecified`,
  `from_wire_reason(v.as_wire_reason().unwrap()) == Some(v)`. Same for `ErrorDomain`.
- **Sentinel.** `ErrorReason::Unspecified.as_wire_reason() == None`, and likewise for `ErrorDomain`
  (D4).
- **Shape.** Every `as_wire_reason()` output matches D5's regex.
- **D5 rejections.** `from_wire_reason` returns `None` for `"slug_conflict"`, `"SLUG-CONFLICT"`,
  `"unspecified"`, `""`, `"-slug"`, `"slug-"`, an unknown code, and the Unicode fold cases
  `"ınternal"` (U+0131) and `"ſlug-conflict"` (U+017F).
- **Ranges.** Every variant's number falls in one of D3's three ranges.

**In `paigasus-iam` (`adapters/grpc/convert.rs`):**

- **AC2.** For each `TenancyError` variant, assert `ErrorReason::from_wire_reason(err.code()).is_some()`.
  The variant list is produced by an **exhaustive, wildcard-free `match` over `&TenancyError`**, so
  adding a variant to the enum is a *compile error* in this test until it is registered — closing the
  "a hand-listed array cannot notice a new variant" gap without waiting for SMA-507. Going through
  `code()` means the assertion exercises the real emitter, so a rename without registration also
  fails.

This test lives in `convert.rs`, not `application/error.rs`, because the application layer imports
only `paigasus_iam_core` (`application/error.rs:5`) and must stay transport-agnostic per `CLAUDE.md`'s
hexagonal-architecture rule; `convert.rs` already imports `paigasus_proto` (`:14-19`).

**No emitter-side test for the other five sites.** Per D7 the registry is intentionally ahead of them
until SMA-504; such a test would fail today. SMA-507 is where that becomes enforceable.

## 7. Acceptance criteria mapping

| AC | Satisfied by |
| --- | --- |
| 1. `error.proto` exists and generates cleanly to Rust, Python and TypeScript | D1 + §2.2 (proven by probe in all three); verified by §8 step 2 |
| 2. Every `TenancyError::code()` value appears unchanged; kebab retained deliberately | D6 (all 26 round-trip, no exceptions) + §4.2 + the §6 AC2 test; the deliberate deviation is recorded in §4.4 item 3 |
| 3. Doc comment states the append-only rule and that removal is breaking | §4.4 item 4 |
| 4. Committed generated output in sync (codegen-drift gate green) | §8 step 2 — the gate is a workflow step, not a Moon task (§2.4) |

## 8. Verification

1. `buf format -w` in `contracts/` — mandatory before commit, or `contracts:fmt` reds `moon ci`
   silently (`CLAUDE.md`).
2. `moon run contracts:generate`, then `git diff --exit-code` over the three generated trees — the
   AC4 gate, which the `moon ci` list does not include (§2.4).
3. `cargo nextest run -p paigasus-proto -p paigasus-iam` in `rs/`. The Docker-gated `paigasus-iam`
   suites need `CI=1` to fail loudly instead of silently reporting PASS having run nothing.
4. The full CI graph as documented in `CLAUDE.md`:
   `moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke
   :parity-corpus-drift :wasm-getrandom-free :redis-connect-single-site :promtool
   :observability-drift :nats-permissions :release-parity :release-parity-py :release-parity-ts
   --base origin/main --include-relations`.

`:breaking` is expected to pass — a new file with new enums is additive, and D2's rule additions only
widen what is tolerated. `:affected-smoke` is expected to pass because its `contracts->proto` case
(`ci/affected-graph/run.sh:106-107`) probes a **hardcoded synthetic path**
(`contracts/proto/paigasus/gateway/v1/health.proto`), so adding a file does not change its strict-
equality set. Note this is *not* because `paigasus-proto` sits outside the guard — that case's
expected set already names `paigasus-proto-rs/-py/-ts`, `paigasus-gateway-rs` and `paigasus-iam-rs`.

## 9. Risks

- **R1 — Irretractable spellings.** 15 of 43 codes are declared before any emitter validates them
  (D7). D2 makes retraction possible; without D2 a wrong spelling is permanent (§2.3). Mitigation:
  D2, plus reviewing §4.2 as a wire contract rather than a draft.
- **R2 — SMA-504's `ErrorInfo` source is not in the workspace.** §2.1 concludes each language gets
  `ErrorInfo` from its own ecosystem, but `tonic-types` appears nowhere in `rs/` today, so SMA-504
  must **add a new workspace dependency** — which may need an `rs/deny.toml` `[licenses] exceptions`
  entry per `CLAUDE.md`. The TypeScript path is **unverified**: Connect-ES may require a generated
  `google.rpc.ErrorInfo` descriptor, which §2.1 proved is not produced. If it does, SMA-504 must
  generate the googleapis module after all. This does not change this issue's design — the registry
  imports nothing either way — but it must not surprise SMA-504.
- **R3 — Generated-diff size.** prost embeds comment text in `FILE_DESCRIPTOR_SET` (§2.4). 44 values
  with one-line comments plus a tight header keep this bounded, but the Rust diff will still be
  dominated by hex.
- **R4 — SMA-504 is larger than its description says.** That issue's scope names the gRPC surfaces
  and the gateway. D7 adds IAM's authn HTTP body, the extractor rejections, the SSE terminal frame,
  and a null→string change. SMA-504 should be updated before it is picked up.

**Rollback.** Nothing at runtime depends on this change, so reverting the commit is sufficient at any
point before SMA-504. After SMA-504 the registry is load-bearing and D2's reserve-and-retire path is
the only safe retraction.

## 10. Out of scope

- **Emitting `ErrorInfo`** — SMA-504: removing the in-band `"{code}: {message}"` prefix, updating the
  gateway's `IamError::Rpc` branching in the same change, D7's 17 rename operations, and populating
  `retryable` / `correlation_id`.
- **The two-way drift gate** — SMA-507.
- **TypeScript and Python transforms, and the TS barrel export** — the SDK issue.
- **Structured `TokenDefect` / `ProvisioningDefect` detail in `ErrorInfo.metadata`.** These stay
  log-only: they describe *why* a token was rejected, and surfacing them would undo the deliberate
  non-revealing posture ADR-0019 decision 5 preserves.
- **Moving canonical error types into `paigasus-kernel`.** ADR-0019 names the kernel their natural
  long-term home but explicitly defers it: this defines the wire contract first.

## 11. Sequencing note for SMA-507

SMA-507's "emitted ⊆ registry" direction will red on **both** service crates for as long as they emit
the pre-D7 spellings — not just the gateway. SMA-507 must therefore land after SMA-504, or ship its
emitted-side check scoped to the sites already canonical today (site 1's 26 tenancy codes and site
4's 2 retirement codes), which its own description already contemplates for the consumed side.
