# SMA-586 — Split the `invalid-prn` catch-all into a per-kind error taxonomy

**Issue:** [SMA-586](https://linear.app/smaschek/issue/SMA-586/iam-invalid-prn-is-a-catch-all-sentinel-for-four-unrelated-validation)
**Date:** 2026-08-23
**Status:** Design approved, pending adversarial challenge

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

## Scope

18 production sentinel sites across four kinds, on both transports. Genuine PRN uses
(`application/roles.rs`, `application/memberships.rs`, `grpc/convert.rs::node_uuid`, and the
per-adapter `Prn::parse` helpers) are out of scope and keep `InvalidPrn`.

## Design decisions

### D1 — Six reasons, not four

The ticket proposed four candidates (`invalid-timestamp`, `invalid-uuid`, `invalid-outcome`,
`invalid-argument`). This design splits two of them, for six total.

**`invalid-argument` is rejected.** It echoes gRPC's `INVALID_ARGUMENT` status code and is
broad enough to re-accrete exactly the catch-all this work removes. The five argument sites
divide cleanly along a line clients act on differently — three are "field X is required"
(the client omitted something) and two are "provide exactly one of A|B" (the client sent too
much). Those become `missing-required-field` and `conflicting-fields`.

**Cursors are split from ids.** The eight uuid sites are four opaque, server-issued
pagination cursors and four caller-supplied resource ids. A console handles a rejected cursor
by resetting pagination; it handles a rejected id by asking the user to fix their input. The
registry is append-only, so `invalid-cursor` could be added later — but a client that had
already branched on `invalid-uuid` for cursors would break when it was. Splitting now is free;
splitting later is not.

### D2 — Variants carry a `&'static str` field name, interpolated into `Display`

The ticket requires a deliberate decision on whether the swallowed detail now reaches the
wire, and states that if it stays discarded the argument should be dropped rather than left
as a trap. Three options were considered:

1. **No payload, static `Display`.** Strictest reading of SMA-504 D7. Rejected: a client
   sending two bad timestamp bounds cannot tell which one failed, and today's server-side
   context is lost from logs too.
2. **`String` payload, full detail on the wire.** Rejected: today's details interpolate
   caller-supplied values (`unknown audit outcome: {raw}`, `invalid RFC3339 timestamp: {s}`),
   so this reflects untrusted input into the error body and leaves every future call site free
   to interpolate whatever it likes — a policy no type enforces.
3. **`&'static str` field name (chosen).** Each variant carries a server-authored field name
   and `Display` interpolates only that.

Option 3 makes "never reflect caller input" a **structural** property rather than a discipline
the next call site has to remember: caller data is unrepresentable in a `&'static str`. It
preserves SMA-504 D7's posture — a field name the client itself sent is not backend detail —
while giving humans a message worth reading. Machine consumers still branch on `reason` alone,
per SMA-508 AC2.

Interpolated field names are the **wire** field names of each transport, so HTTP's
`principal|node` and gRPC's `principal_prn|node_prn` legitimately differ in message text.
AC-3 requires the two transports to agree on the *reason*, not the message.

### D3 — `InvalidPrn` is untouched

It keeps its `String` payload and its static `Display`. Its remaining callers pass either the
kernel's stable error-kind token or a canonical PRN, and changing its `Display` is explicitly
out of scope per the ticket. After migration it is emitted only for genuine PRN parse and
shape failures, which is AC-2.

### D4 — Parity guard pinned against registry values, not literals

The AC-3 guard is a table test that feeds equivalent malformed input to both transports'
parse helpers and asserts they yield the same reason. It pins the expected reason as
`ErrorReason::InvalidCursor` and compares via `as_wire_reason()`, never as a kebab literal.
This has three benefits: it routes through the registry (so an unregistered rename fails), it
matches `grpc/convert.rs`'s documented posture that every code leaves that file through an
`ErrorReason` static rather than a literal, and it adds no `ci/error-registry/check.py`
MANIFEST row.

## The vocabulary

Appended to `contracts/proto/paigasus/common/v1/error.proto` at numbers 33–38 — the next free
values in the IAM 1–299 range (current IAM max is 32) — under a new
`// ---- IAM: request validation (1-299)` sub-banner. Sub-banners carry no numbering meaning
per the file's own rule, so this does not disturb existing values.

| Proto value | Wire reason | Sites |
|---|---|---|
| `ERROR_REASON_INVALID_TIMESTAMP = 33` | `invalid-timestamp` | 3 helpers, 7 effective |
| `ERROR_REASON_INVALID_UUID = 34` | `invalid-uuid` | 4 |
| `ERROR_REASON_INVALID_CURSOR = 35` | `invalid-cursor` | 4 |
| `ERROR_REASON_INVALID_OUTCOME = 36` | `invalid-outcome` | 2 |
| `ERROR_REASON_MISSING_REQUIRED_FIELD = 37` | `missing-required-field` | 3 |
| `ERROR_REASON_CONFLICTING_FIELDS = 38` | `conflicting-fields` | 2 |

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
InvalidOutcome(&'static str),
#[error("{0} is required")]
MissingRequiredField(&'static str),
#[error("provide exactly one of {0}")]
ConflictingFields(&'static str),
```

`strum::EnumIter` (derived under `cfg(test)`) requires each field type to implement `Default`;
`&'static str` does, so the derive keeps working and the AC-4 membership test picks the new
variants up automatically.

## Site migration

Field-name literals are the wire field names on each transport.

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

`grpc/service_accounts.rs:213` (`"id"`), `grpc/authz.rs:217` (`"id"`),
`grpc/tenancy.rs:592` (`"id"`), `grpc/dead_letters.rs:114` (`"id"`).

### `invalid-cursor`

`grpc/audit.rs:74`, `grpc/dead_letters.rs:79`, `http/audit.rs:61`,
`http/dead_letters.rs:76` — all `"cursor"`.

### `invalid-outcome`

`grpc/audit.rs:64`, `http/audit.rs:52` — both `"outcome"`.

### `missing-required-field`

`http/service_accounts.rs:71` (`"owner_prn"`), `http/api_keys.rs:85` (`"scope_prn"`),
`http/authz.rs:145` (`"principal_prn"`).

### `conflicting-fields`

`grpc/tenancy.rs:614` (`"principal_prn|node_prn"`), `http/memberships.rs:96`
(`"principal|node"`).

## Registry mechanics

Three coupled edits; a partial change reds CI rather than passing silently.

1. `contracts/proto/paigasus/common/v1/error.proto` — six values, each with the comment
   repeating its wire literal verbatim, per the file's stated convention.
2. `rs/crates/libs/paigasus-proto/src/error.rs` — six strings appended to
   `EXPECTED_REASONS`. `check.py` treats this and its own proto parse as **independent
   transcriptions** and cross-checks them, so both must move.
3. Same file — `assert_eq!(actual.len(), 46, "the registry should hold 46 reasons")` becomes
   **52**. This is the single easiest step to miss.

Then `buf format -w` followed by `buf generate`. A comment-and-constant change shifts the
embedded `FILE_DESCRIPTOR_SET`, so all three generated trees (Rust, Python, TypeScript) move
and must be committed, or the codegen-drift gate reds.

The gateway needs no change: it never resolves IAM reasons, only asserts its own are
registered.

## Testing

### AC-3 — the transport parity guard

A table test in `grpc/convert.rs`'s test module, alongside the existing
`dead_letter_entry_projects_identically_for_http_and_grpc` drift guard, which is the
established precedent for this shape. For each logical failure it drives both transports'
helpers with equivalent malformed input and asserts a single expected `ErrorReason`:

| Logical failure | HTTP driver | gRPC driver | Expected |
|---|---|---|---|
| malformed timestamp | `http::audit::parse_ts` | `convert::parse_opt_ts` | `InvalidTimestamp` |
| malformed cursor | `http::audit::parse_cursor` | `grpc::audit::parse_cursor` | `InvalidCursor` |
| unknown outcome | `http::audit::parse_outcome` | `grpc::audit::parse_outcome` | `InvalidOutcome` |
| malformed dead-letter timestamp | `http::dead_letters::parse_ts` | `convert::parse_opt_ts` | `InvalidTimestamp` |
| malformed dead-letter cursor | `http::dead_letters::parse_cursor` | `grpc::dead_letters::parse_cursor` | `InvalidCursor` |

This requires widening the eight private `parse_*` helpers to `pub(crate)`, documented at each
as existing for the drift guard. The `missing-required-field` and `conflicting-fields` rows
have no cross-transport pair to compare at helper level (HTTP-only for the former;
`conflicting-fields` is one site per transport, inside handler bodies rather than helpers), so
those are asserted per-site instead and noted as such in the test's doc comment rather than
faked into the table.

### Tests that must change, not merely be added

- `grpc/convert.rs::parse_opt_ts_detail_is_swallowed_by_invalid_prns_static_display` pins the
  exact behaviour being removed. It is **replaced** by its inverse, asserting the field name
  now surfaces in `Display`.
- `grpc/audit.rs`, `grpc/dead_letters.rs`, `http/audit.rs`, `http/dead_letters.rs` each carry
  `matches!(err, TenancyError::InvalidPrn(_))` assertions that must be retargeted to the new
  variants. Retargeting them is what proves the migration landed; leaving one behind is a
  compile-time pass and a silent behavioural miss.
- `application/error.rs::error_classes_are_correct` gains the six new variants.

### Tests needing no change

`grpc/convert.rs::every_tenancy_code_is_declared_in_the_canonical_registry` (AC-4) iterates
via `strum::EnumIter`, so it covers the new variants automatically. That is by design — there
is no second list to leave un-extended.

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

- **`:breaking`** — clean. Adding enum values is additive (AC-5).
- **`:error-code-single-site`** — green. No new file spells a code literal: the six codes
  appear only in `application/error.rs` (already a MANIFEST `emits` row) and the parity table
  uses `ErrorReason` values rather than strings. If the gate demands a MANIFEST row anyway,
  add it with a stated reason rather than working around it.
- **codegen-drift** — requires all three generated trees committed. Note `contracts:generate`
  has no `outputs:`, so it can serve stale cached output; run `buf generate` directly.

## Out of scope

- Whether `Display` should carry interpolated detail *in general* — decided per-variant here
  (D2) for the six new variants only, and explicitly not changed for `InvalidPrn`.
- The gateway's own error vocabulary.
- Any change to HTTP status codes or gRPC status codes. All six are `Validation`, exactly as
  today.
