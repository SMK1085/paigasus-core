# SMA-498 — Canonical error code registry in `common/v1/error.proto`

Linear: [SMA-498](https://linear.app/smaschek/issue/SMA-498/contracts-canonical-error-code-registry-in-commonv1errorproto)
ADR: [ADR-0019 — Canonical error model](https://app.notion.com/p/3bb830e8fbaa8152aa01e1aec0160bab) (accepted 2026-08-13)
Blocks: [SMA-504](https://linear.app/smaschek/issue/SMA-504) (adopt `google.rpc.ErrorInfo` in iam+gateway),
[SMA-507](https://linear.app/smaschek/issue/SMA-507) (two-way drift gate)

## 1. Context

### 1.1 What exists today

Paigasus has four client-facing error surfaces and a real error vocabulary on exactly one of them.

| Surface | Machine-readable code? |
| --- | --- |
| IAM HTTP (`adapters/http/error.rs`) | Yes — `{"error":{"code","message"}}`, `code` from `TenancyError::code()` |
| IAM gRPC (`adapters/grpc/convert.rs::status_to_grpc`) | Only in-band: `Status::new(code, format!("{}: {}", e.code(), e))` |
| IAM authn gRPC (`convert.rs::authn_status`) | **No** — static message strings only |
| Gateway (`adapters/http/error.rs`) | Yes, but a private snake_case vocabulary (`invalid_api_key`, `upstream_timeout`, …) |

`TenancyError::code()` returns 26 stable kebab-case codes. They are already a tested contract —
`rs/crates/services/paigasus-iam/src/application/error.rs` asserts specific spellings, and the HTTP
adapter's tests assert them on the wire.

### 1.2 What this issue delivers

ADR-0019 decision 8 places the registry in `contracts/proto/paigasus/common/v1/error.proto`. This
issue builds **only the registry** — the vocabulary and its generated symbols. It does not change
any error path: SMA-504 emits `ErrorInfo`, SMA-507 gates drift.

## 2. Findings that constrain the design

Two facts were established experimentally in the worktree before designing. Both contradict a
plain reading of the issue's scope notes, so they are recorded here with their evidence.

### 2.1 Importing `google/rpc/error_details.proto` silently produces broken output

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

Generating the googleapis module as well would satisfy the import, but it pulls `error_details.proto`
and its transitive dependencies into all three committed binding trees for a type each language
already has (`tonic-types` in Rust; the Connect/gRPC-web runtimes in TS/Python).

**Therefore: `error.proto` imports nothing.** `(domain, reason)` is described in prose; the wire type
is `google.rpc.ErrorInfo`, obtained per-language from that language's own ecosystem.

### 2.2 No language generates the wire string

An enum-only probe proto generated cleanly in all three languages, and doc comments carried through
to every one (Rust `///`, TS JSDoc, Python docstring). `buf lint` and `buf format --exit-code` both
passed. But the generated symbols are name-based, not value-based:

| | Symbol | Proto name available? | Kebab string available? |
| --- | --- | --- | --- |
| Rust (prost) | `ProbeReason::SlugConflict` | `as_str_name()` → `"PROBE_REASON_SLUG_CONFLICT"` | **No** |
| TypeScript | `ProbeReason.SLUG_CONFLICT` | via `ProbeReasonSchema` descriptor | **No** |
| Python | `ProbeReason.SLUG_CONFLICT` | `betterproto_value_to_renamed_proto_names()` | **No** |

The `ERROR_REASON_X_Y` → `x-y` transform must therefore be implemented somewhere. D4 decides where.

### 2.3 The codegen-drift gate is not a Moon task

AC4 requires the codegen-drift gate green. That gate is a **step in `.github/workflows/ci.yml`**
(`moon run contracts:generate` followed by `git diff --exit-code` over the three generated trees),
not a Moon task. It is absent from the `moon ci` target list in `CLAUDE.md`, so the documented
full-graph command does **not** cover it. Verification must run it explicitly.

## 3. Decisions

### D1 — The registry is a proto `enum`; the wire stays a `string`

proto3 has no string constants. Three shapes were considered:

- **Comment-only registry** — a structured doc block, no declarations. Literally what ADR-0019's
  wording suggests, and generates zero churn. Rejected: it generates *nothing*, making AC1 vacuous,
  leaving the console and SDK with no symbols, and forcing SMA-507's gate to parse proto comments.
- **Enum + a parallel "literal strings" message** — pins the kebab spellings as generated data.
  Rejected: two lists to keep in sync, and the message is never instantiated in any language.
- **Enum as registry, string on the wire** — chosen.

`ErrorReason` and `ErrorDomain` are **registries, never wire types**. Nothing in any `.proto` has a
field of either type. The wire carries `google.rpc.ErrorInfo.reason` / `.domain` as strings, exactly
as ADR-0019 decision 2 requires.

This is compatible with ADR-0019's rejection of "a proto enum for error codes". That rejection
targets an enum *as the wire type*, and both its stated reasons are about the wire: a new code
forcing bindings regeneration on every consumer, and an old client receiving an opaque number.
Neither applies to a registry a consumer may ignore — an old client still receives the kebab string
and can log it verbatim.

The cost that *does* survive: adding a code regenerates bindings in all three languages, and the
codegen-drift gate requires committing that churn. Accepted — codes are added rarely, and the churn
is confined to generated trees.

### D2 — `buf breaking` is a bonus, not the guard

Because the registry is an enum, `buf breaking` will now catch a *deleted enum value*. That is a
welcome side effect, not the protection: it does not see the kebab strings, cannot tell that a code
is still emitted by Rust or consumed by the console, and does not run against non-proto sources.
SMA-507's two-way gate remains the real guard. The file's doc comment says so explicitly, so nobody
later mistakes green `buf breaking` for a checked registry.

### D3 — One flat `ErrorReason` enum across all domains

ADR-0019 decision 1: "One vocabulary underneath them." Per-domain enums (`IamErrorReason`,
`GatewayErrorReason`) would duplicate shared codes such as `internal` and force SMA-507's gate to
know which enum each crate may draw from. A single enum with `ErrorDomain` as the disambiguator
matches the ADR's own reasoning for why `ErrorInfo` beat dotted namespaces like
`iam.tenancy.slug-conflict`.

### D4 — The transform lives in `paigasus-proto`, derived rather than tabulated

`paigasus-proto` gains a hand-written module `src/error_reason.rs`, following the existing
`src/audit.rs` precedent of layering a thin contract over generated types:

```rust
ErrorReason::SlugConflict.as_wire_reason()      // "slug-conflict"
ErrorReason::from_wire_reason("slug-conflict")  // Some(ErrorReason::SlugConflict)
ErrorDomain::Iam.as_wire_domain()               // "iam.paigasus.io"
ErrorDomain::from_wire_domain("iam.paigasus.io")
```

Both directions are **derived** from prost's `as_str_name()` / `from_str_name()` by applying the D6
mapping rule, not written as a 40-arm match. A match would be a second copy of the registry inside
Rust — precisely the "three unlinked places" failure ADR-0019 cites from the observability metrics.
A derived function has nothing to drift against.

`as_wire_reason` returns `String` rather than `&'static str`: a borrowed static would require a
const table, i.e. the second list this decision exists to avoid. The allocation is free in practice —
it happens once per error response, and `tonic_types::ErrorDetails::with_error_info` takes
`impl Into<String>`, so `String` is exactly what the consumer wants.

### D5 — `from_wire_reason` is strict

Reconstructing the proto name from an arbitrary string admits inputs that are not valid wire
reasons. Two are rejected explicitly:

- **Wrong separator or casing.** `"slug_conflict"` and `"SLUG-CONFLICT"` both uppercase-and-
  substitute into `ERROR_REASON_SLUG_CONFLICT` and would otherwise resolve. Input containing `_`, or
  any ASCII uppercase character, returns `None`.
- **The zero sentinel.** `"unspecified"` must not resolve to `ErrorReason::Unspecified`. The sentinel
  exists to satisfy buf's `ENUM_ZERO_VALUE_SUFFIX` lint rule; it is not a code any surface emits.

A lenient parser here would quietly widen SMA-507's gate, letting a misspelled emitted code pass the
"emitted ⊆ registry" check.

`from_wire_domain` applies the same strictness: it requires the `.paigasus.io` suffix and rejects
`"unspecified"` and the uppercase/underscore variants for the same reasons.

### D6 — Two mechanical mapping rules, stated normatively in the proto

- **Reason:** strip the `ERROR_REASON_` prefix, lowercase, replace `_` with `-`.
- **Domain:** strip the `ERROR_DOMAIN_` prefix, lowercase, append `.paigasus.io`.

Every value's doc comment repeats the resulting literal verbatim as its first token
(`// "slug-conflict" — the slug is already taken in this scope.`). The comment is the human-readable
contract and reaches all three generated languages (§2.2); the rule is the machine-readable one. They
must agree, and SMA-507 is where that agreement becomes enforced.

All 26 current `TenancyError::code()` values round-trip through the reason rule with no exceptions,
so AC2 holds without a single special case.

### D7 — Gateway codes are registered in kebab, ahead of the emitter

ADR-0019 keeps the OpenAI envelope intact and puts the canonical reason in its `code` field. The
registry therefore declares the gateway's codes in kebab (`upstream-timeout`, not `upstream_timeout`).
`type` is untouched and keeps its OpenAI semantics — ADR-0019 notes SDKs branch on `type`, and the
Paigasus-specific values (`iam_unavailable`, `missing_scope`) appear in no OpenAI SDK.

**Consequence:** between this issue and SMA-504, the registry declares spellings the gateway does not
yet emit. This is deliberate and is the reason §6 adds no gateway-side test.

### D8 — Reuse `internal`; keep three "unavailable" codes distinct

`AuthnError::Backend` and `GatewayError::Internal` both map to the existing `internal` rather than
minting synonyms. For the gateway this is a small improvement on today's behaviour, where
`GatewayError::Internal` emits a **null** `code`.

Three codes that read as near-synonyms are kept separate because they name different failures:

| Code | Meaning |
| --- | --- |
| `authn-unavailable` | IAM's own authentication backend is unreachable |
| `iam-unavailable` | the gateway cannot reach IAM |
| `upstream-unavailable` | the gateway cannot reach the model provider |

Likewise the gateway's `insufficient-permissions` is **not** folded into IAM's `forbidden`. They
share an HTTP status but not a surface or an audience, `domain` already separates them, and merging
them would be a second wire-visible change to the gateway beyond the casing change D7 already
commits to.

### D9 — Export the registry from the TypeScript barrel

`ts/packages/paigasus-proto/src/index.ts` is a hand-maintained selective barrel; generated files are
not re-exported automatically. `ErrorReason` and `ErrorDomain` are added to it so `@paigasus/proto`
consumers can reach the registry. This is a re-export beyond the issue's stated scope, recorded here
so it is visible rather than incidental. No TypeScript or Python *helper* is added — the SDK issue
owns those.

## 4. The registry

`contracts/proto/paigasus/common/v1/error.proto`, package `paigasus.common.v1`, no imports.

### 4.1 `ErrorDomain` — 2 values

| Proto value | Wire string |
| --- | --- |
| `ERROR_DOMAIN_IAM` | `iam.paigasus.io` |
| `ERROR_DOMAIN_GATEWAY` | `gateway.paigasus.io` |

### 4.2 `ErrorReason` — 39 values

**IAM tenancy (26)** — verbatim from `TenancyError::code()`, in declaration order:

`slug-conflict`, `duplicate-membership`, `email-conflict`, `service-account-name-conflict`,
`invalid-email`, `invalid-slug`, `invalid-name`, `invalid-prn`, `prn-mismatch`, `invalid-pagination`,
`nothing-to-rename`, `not-found`, `parent-archived`, `node-archived`, `missing-org-membership`,
`forbidden`, `unknown-role`, `invalid-scope`, `system-immutable`, `policy-invalid`, `policy-conflict`,
`invalid-action`, `invalid-bulk-replay`, `not-system-owned`, `fleet-not-converged`, `internal`

**IAM authn (5 new)** — from `AuthnError` via `authn_status`; `Backend` reuses `internal` (D8):

`invalid-token`, `identity-not-provisioned`, `provisioning-failed`, `principal-inactive`,
`authn-unavailable`

**Gateway (8 new)** — from `GatewayError::parts()`; `Internal` reuses `internal` (D8):

`missing-authorization`, `invalid-api-key`, `insufficient-permissions`, `missing-scope`,
`iam-unavailable`, `invalid-request-body`, `upstream-unavailable`, `upstream-timeout`

Enum values are numbered sequentially in the order above starting at 1, preceded by the mandatory
`ERROR_REASON_UNSPECIFIED = 0` sentinel — 40 declarations in total. The three groups are separated by
comment banners inside the one enum (D3). `ErrorDomain` follows the same pattern with its own
`ERROR_DOMAIN_UNSPECIFIED = 0`.

### 4.3 File-level doc comment

The header states, normatively:

1. Error identity is `(domain, reason)`, carried on the wire as `google.rpc.ErrorInfo` — which this
   file deliberately does not import (§2.1).
2. Both mapping rules (D6), and that each value's doc comment repeats its literal.
3. Kebab casing is a deliberate, documented deviation from Google's UPPER_SNAKE `reason` convention,
   because the existing codes are already a tested contract (ADR-0019 decision 3).
4. **The registry is append-only. Removing a value is a breaking change**, and `buf breaking` is not
   what protects it (D2).
5. The two standard `ErrorInfo.metadata` keys SMA-504 will populate — `retryable` and
   `correlation_id`. These are documented in prose, not enumerated: `metadata` is an open map, not a
   closed vocabulary consumers branch on exhaustively, and SMA-507's gate does not check it.

## 5. Files touched

| Path | Change |
| --- | --- |
| `contracts/proto/paigasus/common/v1/error.proto` | new |
| `rs/crates/libs/paigasus-proto/src/generated/paigasus/common/v1/paigasus.common.v1.rs` | regenerated — prost emits one file per *package*, so the enums land beside `AuditMetadata`; **`lib.rs` needs no change** |
| `ts/packages/paigasus-proto/src/generated/paigasus/common/v1/error_pb.ts` | new (generated) |
| `py/packages/paigasus-proto/src/paigasus_proto/generated/paigasus/common/v1/__init__.py` | regenerated — betterproto2 merges by package |
| `rs/crates/libs/paigasus-proto/src/error_reason.rs` | new, hand-written (D4) |
| `rs/crates/libs/paigasus-proto/src/lib.rs` | one line: `pub mod error_reason;` |
| `ts/packages/paigasus-proto/src/index.ts` | two re-exports (D9) |
| `rs/crates/services/paigasus-iam/src/application/error.rs` | test module only (§6) |

No `Cargo.toml`, `buf.yaml`, `buf.gen.yaml` or `moon.yml` change. No new dependency in any workspace,
so `cargo-deny` needs no waiver and `cargo-machete` no allowlist entry.

## 6. Testing

**In `paigasus-proto` (`error_reason.rs`):**

- Round-trip: for every `ErrorReason` variant except `Unspecified`,
  `from_wire_reason(v.as_wire_reason()) == Some(v)`. Same for `ErrorDomain`.
- Shape: every `as_wire_reason()` output is non-empty, ASCII-lowercase, contains no `_`, and does not
  start or end with `-`.
- Spot-check the literals from ADR-0019's own examples: `slug-conflict`, `parent-archived`,
  `nothing-to-rename`, and `iam.paigasus.io`.
- D5 rejections: `from_wire_reason` returns `None` for `"slug_conflict"`, `"SLUG-CONFLICT"`,
  `"unspecified"`, `""`, and an unknown code.

**In `paigasus-iam` (`application/error.rs`):**

- **AC2.** The test holds an explicit array of `TenancyError` *values* — one per variant, not a list
  of code strings — and asserts `ErrorReason::from_wire_reason(err.code()).is_some()` for each. Going
  through `code()` means the assertion exercises the real emitter, so renaming a code in
  `TenancyError` without registering it fails the test.

This test is what turns AC2 from "verified by reading the diff" into a fact CI checks. It does not
replace SMA-507: a hand-listed array cannot notice a *newly added* variant. That is exactly the gap
SMA-507's gate closes, and stating it here keeps the two issues from appearing redundant.

**No gateway test.** Per D7 the registry is intentionally ahead of the gateway's emitter until
SMA-504; an equivalent assertion would fail today.

## 7. Verification

In order:

1. `buf format -w` in `contracts/` — mandatory before commit, or `contracts:fmt` reds `moon ci`
   silently (`CLAUDE.md`).
2. `moon run contracts:generate`, then `git diff --exit-code` over the three generated trees — the
   AC4 gate, which the `moon ci` list does not include (§2.3).
3. `cargo nextest run -p paigasus-proto -p paigasus-iam` in `rs/`. The Docker-gated `paigasus-iam`
   suites need `CI=1` to fail loudly instead of silently skipping.
4. The full CI graph as documented in `CLAUDE.md`:
   `moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke
   :parity-corpus-drift :wasm-getrandom-free :redis-connect-single-site :promtool
   :observability-drift :nats-permissions :release-parity :release-parity-py :release-parity-ts
   --base origin/main --include-relations`.

`:breaking` is expected to pass: a new file with new enums is additive. `:affected-smoke` is
unaffected — its strict-equality guard covers crates depending on `paigasus-kernel-rs`, and
`paigasus-proto` is not one.

## 8. Out of scope

- **Emitting `ErrorInfo`** — SMA-504. Includes removing the in-band `"{code}: {message}"` prefix,
  updating the gateway's `IamError::Rpc` branching in the same change, renaming the gateway's eight
  `code` values per D7, and populating `retryable` / `correlation_id`.
- **The two-way drift gate** — SMA-507.
- **TypeScript and Python `as_wire_reason` equivalents** — the SDK issue.
- **Moving canonical error types into `paigasus-kernel`.** ADR-0019 names the kernel their natural
  long-term home but explicitly defers it: this defines the wire contract first.

## 9. Sequencing note for SMA-507

SMA-507's "emitted ⊆ registry" direction will red on the gateway crate for as long as it emits
snake_case codes (D7). SMA-507 must therefore either land after SMA-504, or ship its emitted-side
check scoped to IAM first — which its own description already contemplates for the consumed side.
