# SMA-583 — gRPC `ListAuditEntries` silently unfilters a malformed timestamp bound

Date: 2026-08-23
Linear: [SMA-583](https://linear.app/smaschek/issue/SMA-583/iam-grpc-audit-list-silently-unfilters-a-malformed-timestamp-bound)
Classification: bounded
Revision: 2 (post spec-challenge)

## Problem

`grpc::audit::to_filter` maps the wire's time bounds as:

```rust
from: req.from.and_then(convert::from_ts),
to:   req.to.and_then(convert::from_ts),
```

`convert::from_ts` returns `None` for a `prost_types::Timestamp` `chrono` cannot
represent — a negative `nanos` (`u32::try_from(t.nanos).ok()?`) or an out-of-range
`seconds`. On a **filter** field `None` means *unfiltered*. So a client sending
`from { nanos: -1 }` gets no error and an **unbounded** query: the bound it asked for
is silently discarded.

The HTTP twin (`http::audit::parse_ts`) rejects the equivalent with a `400`, so this
is also an HTTP/gRPC parity break.

**Reachability.** `ListAuditEntries` is Root-only, enforced in
`application::audit::AuditQueryService::list` (`src/application/audit.rs:36-39`), so
the unbounded query is only reachable by a platform admin. That is the honest
counterweight to SMA-501's D10 deferral — and also why this is a correctness/parity
fix rather than an urgent one.

**The malformed value genuinely reaches the handler.** This is the fix's load-bearing
assumption, and it is already proven green in this repo rather than merely plausible:
`prost_types::Timestamp` is a plain `Message` derive with no encode-time
normalization (`normalize()` exists but is opt-in), and
`tests/grpc_dead_letters.rs:299-339` already round-trips `nanos: -1` end-to-end and
asserts `InvalidArgument`.

`convert::parse_opt_ts` — introduced by SMA-501 for the dead-letter surface, and
already unit-tested — keeps the three cases distinct: absent ⇒ unfiltered, valid ⇒
converted, present-but-unrepresentable ⇒ client error. SMA-501 left `audit.rs`
alone deliberately (design D10: a read widens a result set, not a mutation) and
recorded a `KNOWN LATENT` comment pointing here.

## Design

### D1 — Use `parse_opt_ts` at both `audit.rs` call sites

`to_filter` already returns `Result<AuditFilter, TenancyError>`, so `?` fits with no
signature change and no change to the caller.

```rust
from: convert::parse_opt_ts(req.from, "from")?,
to:   convert::parse_opt_ts(req.to, "to")?,
```

`TenancyError::InvalidPrn`-as-sentinel maps through `ErrorClass::Validation` ⇒
`Code::InvalidArgument`, matching the HTTP twin's 400 and `grpc::dead_letters`.

### D2 — Fix the three comments this makes stale, not just the one

AC3 asks for the `KNOWN LATENT` comment's removal. Deleting it outright would leave
the call site with no explanation of why the non-obvious helper is used. Replace it
with the rationale `grpc::dead_letters.rs:86-87` already carries — that wording is
transport-generic ("`None` on a filter field means UNFILTERED") and does not drag in
the bulk-replay blast-radius argument, which would not apply to a read.

Two further comments go stale or are already wrong, and are corrected in the same
pass:

- **`http/audit.rs:65-69`** currently asserts *"the gRPC wire has no equivalent
  failure mode to mirror here (its `from`/`to` are already-structured
  `prost_types::Timestamp`, never a string that can fail to parse)"*. That is exactly
  the false belief this issue exists to correct. Left alone, the two transports would
  agree in code and disagree in prose.
- **`convert.rs:203`** describes "the `req.field.and_then(convert::from_ts)` shape
  **used in `grpc::audit`**" — a call site D1 deletes.

### D3 — Make `from_ts` module-private instead of adding a `repo:*` gate (AC4)

AC4 asks whether a gate should ban the `and_then(from_ts)` shape. It should not.

`from_ts` has exactly two callers outside `convert.rs`, both in this crate (verified:
no `pub use` re-export, no integration test, bench or example touches it, and
`paigasus-gateway` does not depend on `paigasus-iam` at all):

| Call site | Shape | Verdict |
|---|---|---|
| `grpc/audit.rs:96-97` | `and_then(from_ts)` | the bug — D1 fixes it |
| `grpc/service_accounts.rs:185` | `.map(\|t\| from_ts(t).ok_or_else(..)).transpose()` | correct, but a hand-rolled `parse_opt_ts` |

Fold the second onto `parse_opt_ts` (D5) and `from_ts` has **no callers outside
`convert.rs`**, so it becomes **module-private — no visibility modifier at all**.
Not `pub(crate)` and not `pub(super)`: both would keep it reachable from
`grpc::audit` and `grpc::service_accounts`, which are siblings under `adapters::grpc`,
and so would deliver none of the property claimed here.

**What this actually closes — and what it does not.** The visibility change closes
*misuse of `from_ts` from outside `convert.rs`*. It does **not** close the bug class.
`parse_opt_ts` returns `Result<Option<_>, _>`, so a future call site can still write
`parse_opt_ts(req.from, "from").ok().flatten()` or `.unwrap_or(None)` and reproduce
the identical silent-unfilter, using the helper. The realistic trigger is a call site
that cannot use `?` — inside a `.map()` closure, or a handler returning
`Result<_, Status>` rather than `Result<_, TenancyError>` — where `.ok()` is the path
of least resistance.

That residual is the honest reason a gate buys nothing: **a grep gate on
`and_then(from_ts)` would not catch those shapes either.** So the choice is not
"compiler vs. gate on equal coverage" — it is a compile-time check that closes the
one shape a gate would close, at zero CI cost and without the registration burden a
new `repo:*` gate carries (`ci.yml`'s `T=(…)` array, the CLAUDE.md marker block,
`repo:input-liveness`, `repo:affected-smoke`). The repo's existing single-site gates
(`redis-connect-single-site`, `iam-docker-policy-single-site`) exist precisely where
the compiler *cannot* reach — a type name, an env var read from tests.

Confirmed no error-registry work follows: neither `grpc/audit.rs` nor
`grpc/service_accounts.rs` spells a kebab error code, and `grpc/convert.rs` is
already an `asserts` row in `ci/error-registry/check.py`.

**Rejected alternative — delete `from_ts` entirely.** After D5 it has one production
use (`convert.rs:213`) plus one sanity assertion (`:732`); inlining its two lines into
`parse_opt_ts` would remove the `Option`-returning primitive altogether. Rejected:
it loses the documented `ts`/`from_ts` inverse-pair symmetry, makes the `:732`
assertion vacuous, and its main advantage — eliminating the in-`convert.rs` residual —
is worth little now that the realistic residual is shown above to be `.ok()` at a
*caller*, not `and_then` inside `convert.rs`.

**Rustdoc consequence.** `parse_opt_ts` is `pub` and its doc links ``[`from_ts`]``
(`convert.rs:201`). Once `from_ts` is private that emits
`rustdoc::private_intra_doc_links` and degrades to plain text. The workspace's
`[workspace.lints.rust] warnings = "deny"` is a *rustc* lint group and does not cover
rustdoc lints, and the repo has no `cargo doc` task, so nothing would catch it —
change the link to a plain code span in the same edit.

### D4 — Tests: unit tier carries the proof, integration tier carries the wire

The original plan proposed only a Docker-backed integration test. That is not enough:
an implementation that rejects malformed bounds but **drops valid ones** would pass
every step of it. And there is currently no test anywhere that `grpc::audit::to_filter`
maps `from`/`to` at all — `to_filter_treats_empty_wire_fields_as_unfiltered`
(`src/adapters/grpc/audit.rs:159-176`) asserts `actor_prn`/`resource_prn`/`action`/
`outcome`/`cursor` and pointedly **not** `from`/`to`.

**Unit tier (new, primary).** Mirror the HTTP twin one-for-one in `audit.rs`'s
existing `mod tests`, which already has `default_request()`. No Docker:

1. absent ⇒ `filter.from.is_none()` && `filter.to.is_none()` (extend the existing
   `to_filter_treats_empty_wire_fields_as_unfiltered`, closing its gap).
2. present-valid ⇒ `filter.from == Some(<exact instant>)`, likewise `to` — this is
   the case that catches "rejects malformed but drops valid".
3. present-invalid `from` (with `to: None`) ⇒ `Err(InvalidPrn)`.
4. present-invalid `to` (with **`from: None`**) ⇒ `Err(InvalidPrn)`. Stated
   explicitly: setting *both* to the bad value would pass even if only the `from`
   line were fixed, defeating the test's purpose.

The HTTP twin pins the same three cases at `http/audit.rs:156-157` (absent),
`:209-216` (malformed), `:219-228` (valid), so this also removes a real
unit-coverage asymmetry between the transports.

**Integration tier (wire-level).** Extend the existing
`list_audit_entries_over_grpc_returns_seeded_rows_for_a_platform_admin`
(`tests/grpc_audit.rs:117-154`) rather than adding a fourth test. It already seeds a
row and asserts an unfiltered list returns it — that *is* the control, already
written — so appending the two malformed-bound assertions costs **zero additional
Postgres containers**, which matters given CLAUDE.md's documented container-concurrency
and flakiness pressure. Rename it to reflect the expanded role.

Trade-off, taken deliberately: on failure this yields one red instead of two. That is
acceptable *because* the unit tier above now provides the fine-grained diagnosis; the
integration test's remaining job is only end-to-end status mapping.

Ordering is sound: the control asserts `entries.len() == 1` **before** the malformed
calls, and the malformed calls return errors without listing, so the control cannot be
perturbed by them.

**Why the control is there.** Not "a regression returning nothing would pass" — a
query returning nothing yields `Ok` with zero entries, which already fails an
`InvalidArgument` assertion. The control's real job is to prove the seeded rows are
genuinely matchable, so that the rejection is not vacuous: without it the test would
pass green against an *empty* database, proving nothing about unfiltering.

### D5 — Fold `service_accounts.rs` onto `parse_opt_ts`

Load-bearing for D3, not scope creep: without it `from_ts` keeps a caller outside
`convert.rs` and cannot drop `pub`.

```rust
let expires_at = convert::parse_opt_ts(req.expires_at, "expires_at").map_err(convert::status_to_grpc)?;
```

Semantically identical: `.map(|t| from_ts(t).ok_or_else(..)).transpose()` ≡
`parse_opt_ts(t, "expires_at")`, including the absent ⇒ `None` ⇒ non-expiring case.

The error detail changes from `"expires_at out of range"` to `"invalid timestamp for
expires_at"`, which is **wire-invisible**: `TenancyError::InvalidPrn`'s `Display` is
the static `"invalid resource prn"` (`src/application/error.rs:41-42`), gRPC puts
`e.to_string()` on the wire (`convert.rs:125`) and HTTP the same in the body
(`http/error.rs:32`). `convert.rs`'s own
`parse_opt_ts_detail_is_swallowed_by_invalid_prns_static_display` test pins exactly
this, and no test asserts the old string.

**This does not inherit coverage.** `parse_opt_ts`'s unit tests cover the *helper*,
not this call site, and `tests/api_keys_grpc.rs` sends `expires_at: None` at all four
of its sites (`:102`, `:170`, `:198`, `:281`) — the out-of-range path has no test at
all today. Add one assertion that `IssueApiKey` with
`expires_at: Some(Timestamp { seconds: 0, nanos: -1 })` returns `InvalidArgument`.
It is the only thing that makes this refactor's "no behaviour change" claim checkable.

### D6 — Document the rule in the proto

`contracts/` is the source of truth (ADR-0004) and this is a client-visible behaviour
change. `ListDeadLettersRequest` already carries the rule
(`contracts/proto/paigasus/iam/v1/iam.proto:622-625`): *"An ABSENT timestamp means
unfiltered; a PRESENT but unrepresentable one is INVALID_ARGUMENT and never silently
unfiltered (design D10)."* — and says it *mirrors `ListAuditEntriesRequest`*.

`ListAuditEntriesRequest` (`:484-486`) says only: *"an empty string / zero timestamp
means 'unfiltered' on that field."* That is independently **wrong** on the timestamp
half: a zero `Timestamp` is *present* and denotes the epoch, so it filters from
1970-01-01. Only an *absent* one is unfiltered. Fix both halves in one edit — correct
the zero-timestamp claim and add the same absent/valid/unrepresentable sentence.

Regen obligation, flagged up front because it changes the PR's file set and CI cost:
`buf format -w`, then regenerate the Rust/Py/TS bindings (proto comments become
generated doc comments), so `contracts:generate` output and the codegen-drift gate
must be refreshed. `buf breaking` is unaffected — comments are not breaking.

## Acceptance criteria → design

| AC | Covered by |
|---|---|
| 1. Rejects present-but-unrepresentable `from`/`to` with `INVALID_ARGUMENT` | D1 |
| 2. A test proves the negative, not just the status code | D4 (unit tier is the proof; integration tier the wire) |
| 3. `KNOWN LATENT` comment removed | D2, which also fixes two further stale/wrong comments |
| 4. Consider a gate banning `and_then(from_ts)` | D3 — rejected, with a scoped compile-time mechanism and a stated residual |

## Compatibility

This is an intentional behaviour change on a mounted RPC: a client sending a malformed
bound previously got `OK` plus a wider-than-requested result set and now gets
`INVALID_ARGUMENT`. `buf breaking` cannot see it. Accepted without a deprecation
window — the surface is Root-only, and a client relying on the old behaviour was
already receiving results it did not ask for.

## Out of scope

- The HTTP twin's *behaviour*: already correct (`http::audit::parse_ts` returns 400).
  Only its stale doc comment is touched (D2).
- `grpc::dead_letters`: already on `parse_opt_ts` at all four sites (SMA-501).
- **A dedicated `invalid-timestamp` error reason.** Both transports report
  `ErrorInfo.reason = "invalid-prn"` / message `"invalid resource prn"` for a
  malformed *timestamp*; this fix makes that the third surface overloading the
  sentinel. `ci/error-registry/check.py` would make adding a code cheap, but it is a
  registry change touching both transports and all existing call sites — worth a
  follow-up issue, not this fix.
- **chrono-valid but Postgres-invalid instants.** `parse_opt_ts` accepts chrono's
  range (~±262,000 years); `timestamptz` bottoms out at 4713 BC, so a value in the gap
  falls through and surfaces as `Internal`/500 from the store rather than
  `InvalidArgument`. HTTP has the identical hole via `parse_from_rfc3339`, so parity
  holds either way and this fix neither widens nor narrows it.
- **`from > to` ordering.** Neither transport validates it; both silently return an
  empty page. Unchanged here.

## Verification

- `cargo nextest run -p paigasus-iam` with Docker reachable
  (`PAIGASUS_REQUIRE_DOCKER=1` for any filtered run — the Docker-gated suites skip
  quietly otherwise).
- The full CI graph per CLAUDE.md's `ci-targets` block — required here, not optional:
  D6 touches `contracts/`, which pulls in `contracts:fmt`, codegen drift and
  `:breaking`, and D3's visibility change touches a crate other gates key on.
