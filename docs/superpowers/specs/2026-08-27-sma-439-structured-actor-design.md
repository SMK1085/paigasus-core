# SMA-439 — Structured `Actor` message replacing opaque `created_by` / `modified_by`

**Issue:** [SMA-439](https://linear.app/smaschek/issue/SMA-439/structured-actor-message-to-replace-opaque-created-by-modified-by)
**Follow-up from:** SMA-425 (explicitly out of scope there)
**ADR context:** [ADR-0012 — per-language thin wrappers](https://app.notion.com/p/386830e8fbaa816d8a43e941ef5b4c4f), ADR-0014 (PRN), ADR-0019 A1.2 (reserve+add)
**Date:** 2026-08-27 (revised 2026-08-28 after adversarial review)

## Problem

`paigasus.common.v1.AuditMetadata` carries the actor as two bare strings:

```proto
string created_by = 3;  // user id / subject claim / service name; empty = unknown/system
string modified_by = 4;
```

Two defects. The identifier is opaque — nothing states what namespace it draws from, so a
consumer cannot resolve it or branch on what kind of actor it names. And `""` is an
overloaded sentinel meaning "unknown or system", which a proto3 scalar cannot distinguish
from a deliberately-written empty value, because proto3 scalars have no presence.

## What changed since the issue was filed

SMA-439 was written in June and deferred pending "once the actor model firms up". It has
since firmed up: the repo standardised on **PRNs** (ADR-0014). In the running system today
`AuthContext.principal_id` is a `PrincipalId` wrapping a `Prn`
(`rs/crates/services/paigasus-iam/src/adapters/auth.rs:19`, reached via `.prn()`);
`grpc/dead_letters.rs::actor_prn` derives the acting identity straight from it; and
`AuditEntry.actor_prn` is the established audit-log shape.

The PRN's `resource_type` segment already encodes the actor kind — `principal`, `user`,
`service-account`. The issue's June sketch of "a `kind` enum plus an id and optional display
name" would therefore reintroduce, as a second representation, a distinction PRN already
carries, and would keep `id` opaque — the very defect this issue exists to remove.

**Decision:** `Actor` is PRN-canonical. No kind enum.

## Design

### D1 — `Actor`

New file `contracts/proto/paigasus/common/v1/actor.proto`. Its own file rather than an
addition to `audit.proto`: `common/v1` already splits by concept (`audit`, `error`,
`service_info`, `auditable_example`), and `Actor` is broader than audit — `AuditEntry.actor_prn`
is a plausible future adopter.

```proto
// SPDX-License-Identifier: Apache-2.0
syntax = "proto3";

package paigasus.common.v1;

// The actor behind a change: a principal named by its canonical PRN (ADR-0014).
//
// CONTRACT. Producers MUST write a canonical PRN. Consumers MUST treat an `Actor`
// whose `prn` is empty or unparseable as *unknown* — identically to an absent
// `Actor` — and never as an error. proto3 gives message presence but not field
// presence, so `Actor{prn: ""}` is constructible in every language; this rule is
// what keeps that from becoming a second meaning.
message Actor {
  // Canonical PRN of the actor, e.g. prn:pgs:iam:::principal/<uuid>. The PRN's
  // resource-type segment carries the kind (user / service-account / principal),
  // so there is deliberately no separate kind enum.
  string prn = 1;
}
```

A separate proto file produces a **new generated TypeScript module**,
`ts/packages/paigasus-proto/src/generated/paigasus/common/v1/actor_pb.ts` — protobuf-es emits
one module per proto file, whereas prost appends to the existing per-package
`paigasus.common.v1.rs` and betterproto2 appends to the existing `common/v1/__init__.py`.
That new file must be committed; codegen-drift sees it because CI stages with
`git add --intent-to-add`. (Verified by running `buf generate` on this exact shape.)

### D2 — Absence is the canonical "unknown/system" representation

An unset `Actor` field means unknown-or-system, exactly what `""` means today.

`Actor` is a message despite holding a single field because a bare `string creator_prn = 5`
would have no presence at all, leaving the empty-string sentinel as the *only* way to say
"unknown". A message field is genuinely absent-or-present, so absence becomes the canonical
"unknown". The one-field message is load-bearing, not ceremony.

**This is a producer obligation, not an enforced invariant.** proto3 gives message presence
but not field presence, so `Actor{prn: ""}` is constructible and wire-encodable in all three
languages (`Actor::default()`, `Actor()`, `create(ActorSchema, {})`), and `contracts/buf.yaml`
carries no `protovalidate` dependency that could forbid it. The design therefore states the
rule normatively in `actor.proto`'s own comment (D1) and collapses the degenerate cases into
the canonical one: **empty or unparseable `prn` means unknown, identically to an absent
`Actor`, and is never an error.** Without that rule the change would ship three spellings of
"unknown" where there was one — exactly the multiplicity D4 refuses elsewhere.

Today's field already conflates "unknown" with "system", so collapsing both to absence
preserves the current information content exactly and invents no distinction any reader has
a use for. A service that later needs to record itself as the writer should use its genuine
service-account PRN (the M3 machine-identity type already in `iam.proto`) rather than a
sentinel — no magic PRN value is minted, now or later.

### D3 — Field migration: reserve + add, with a rename

`AuditMetadata` retires 3/4 and adds the replacements at 5/6:

```proto
reserved 3, 4;
reserved "created_by", "modified_by";

Actor creator = 5;
Actor modifier = 6;
```

The names **must** change. This was established by measurement, not argument — all four
candidate routes were run against the real gates:

| Route | Result |
|---|---|
| `reserved 3, 4;` + `Actor created_by = 5;` | ❌ `buf breaking`: *"Previously present field "3" with name "created_by" … was deleted without reserving the name "created_by""* — buf keys the deletion check on the **number**, so a same-named field at another number does not satisfy it |
| Also reserve the names, then reuse them | ❌ *"use of reserved message field name"*, surfaced by `buf lint` but raised by the protobuf **compiler**, not a lint rule — so it cannot be `except`-ed away. The two requirements are mutually exclusive |
| Change the type in place at 3/4 | ❌ `FIELD_SAME_TYPE`, plus cardinality `implicit presence` → `explicit presence` |
| `reserved 3, 4` + names, new fields at 5/6 | ✅ `buf lint` 0, `buf breaking` 0, `buf format --exit-code` 0 |

`creator` / `modifier` over `created_by_actor` / `modified_by_actor`: the `_actor` suffix
restates the field's own type, and it breaks the rhyme with the `created_at` / `modified_at`
pair it sits beside. This matches the `IntrospectResponse.role_group_prns` → `role_grants`
precedent (`iam.proto:241`) of picking a genuinely better name rather than a suffixed old one.

Loosening `contracts/buf.yaml` — adding `FIELD_NO_DELETE_UNLESS_NAME_RESERVED` to `except` —
was considered and rejected: it would weaken the breaking gate for every message in the repo,
permanently, to save one rename, and contradicts the posture that file's own comments record.

Note the reservation is **permanent**: `created_by` / `modified_by` can never again name a
field in `AuditMetadata`, in any language. That is accepted, and the proto carries a short
comment saying so, so a future author does not rediscover it the hard way.

### D4 — Clean break, no deprecation window

The strings are deleted, not deprecated alongside. There are zero git tags in the repo
(verify with `git tag | wc -l` at implementation time — `rs/release-plz.toml` warns that a
first release tags permanently); SMA-577 only *made* the proto family publishable at 0.1.0,
and nothing has been released. There is no external crates.io/npm/PyPI consumer to ease over,
so a deprecation window would buy nothing and would leave two representations of one fact to
drift.

**Rolling-deploy skew.** `AuditMetadata` is embedded in seven live RPC message types
(`iam.proto:47,56,66,72,379,401`), so "no released consumer" is not the whole story. Both skew
directions were considered and both degrade safely: an old client decoding a new message sees
5/6 as unknown fields and reads `created_by == ""` → unknown; a new client decoding an old
message sees `creator` absent → unknown. Since both spellings already mean unknown today (the
fields are unconditionally empty, D8), no deployment order is required and no reader changes
meaning. There is also no stored-message migration: nothing persists encoded `AuditMetadata` —
IAM's outbox/NATS path emits JSON CloudEvents
(`rs/crates/services/paigasus-iam/src/adapters/events/nats_publisher.rs`).

**Commit classification.** Commit as `feat(contracts)!:` (or with a `BREAKING CHANGE:`
footer). Do **not** hand-edit any version site: `paigasus-proto` is in release-plz's `proto`
`version_group`, `repo:version-lockstep` checks eighteen sites, and a partial bump reds it.
release-plz owns the bump.

### D5 — Rust surface

`rs/crates/libs/paigasus-proto/src/audit.rs` re-exports `Actor` beside `AuditMetadata` — the
same one-stable-anchor rationale the file already documents, so consumers never name the
codegen module layout that `clean: true` regenerates. The two accessors swap:

```rust
fn creator(&self)  -> Option<&Actor> { self.audit().and_then(|a| a.creator.as_ref()) }
fn modifier(&self) -> Option<&Actor> { self.audit().and_then(|a| a.modifier.as_ref()) }
```

`created_by()` / `modified_by()` are removed outright (D4).

**The accessor loses a distinction, deliberately.** Today `created_by()` returns `Some("")`
for "audit present, actor unknown" and `None` for "no audit at all"; `creator()` returns
`None` for both. That distinction does not vanish — it moves to `audit()`, which still
separates the two — but it leaves the actor accessors, so a consumer branching on
`created_by().is_none()` to mean "unaudited entity" must branch on `audit().is_none()`
instead. This is the point of the change (an unknown actor and an absent actor are the same
fact about the actor), but it is a real API semantics change on a `publish = true` crate, so
the ported test asserts `audit().is_some()` and `modifier().is_none()` **together** — pinning
the collapse as intended rather than letting it happen silently.

### D6 — `paigasus-proto` does NOT gain a `paigasus-kernel` dependency

`Actor.prn` stays a `String`. A typed accessor returning a parsed `Prn` was considered and
rejected: validation belongs to the consumer, and IAM — the only producer — already holds a
real `Prn` and calls `.canonical()`, so it never needs to parse back.

The dependency would cost real gate churn for no gain:

- it reds `repo:affected-smoke`'s `kernel->bindings` strict-equality set (SMA-409),
- it needs a `dependsOn` edge **and** `fileGroups.upstreams` entries (SMA-524/528),
- and it pulls `paigasus-kernel` into `paigasus-proto`'s **publish group**, costing the kernel
  the standalone `cargo publish --dry-run` assertion `repo:publish-metadata` Check 2 gives it
  today (that check runs one dry-run per connected component of the in-set dependency graph).

A consumer that needs the actor *kind* parses the PRN, and that is supported in every
language rather than Rust-only: `prnResourceType` is exported from the kernel bindings and
pinned for napi/wasm parity (`ts/packages/paigasus-kernel/src/index.ts:5`,
`binding-parity.types.ts:24`, `rs/crates/bindings/paigasus-node-bindings/src/lib.rs:77`).
That cross-language availability is what makes "no kind enum" (D1) defensible.

### D7 — Python and TypeScript surfaces are structurally unchanged

Both thin wrappers declare only the `audit` attribute — `py/.../audit.py`'s `Protocol` and
`ts/.../audit.ts`'s `interface` — so neither changes shape. Per ADR-0012 the wrappers
re-surface the generated type and nothing more; the accessor rename is a Rust-only concern
because Rust is the only language whose wrapper names the inner fields.

`ts/packages/paigasus-proto/src/index.ts` gains `ActorSchema` / `Actor` re-exports, mirroring
exactly how `AuditMetadataSchema` / `AuditMetadata` are surfaced today.
`py/.../paigasus_proto/__init__.py` is empty and stays empty — the Python tests import from
`paigasus_proto.generated…` directly.

No `contracts/buf.gen.yaml` change is needed. The `message_attribute=` injection list keys on
messages that *embed* `AuditMetadata`; `Actor` embeds nothing and `AuditMetadata`'s own
membership is unchanged.

### D8 — The single producer stays unset

`paigasus-iam/src/adapters/grpc/convert.rs::audit()` writes `None` for both fields where it
writes `String::new()` today, and its comment is updated to name the new fields.

It stays unset because the actor is not **persisted**, not because it is unavailable. The
plumbing already exists: `actor_prn(&AuthContext)` helpers sit in both
`adapters/http/organizations.rs:48` and `adapters/grpc/dead_letters.rs:64`, and
`application/organizations.rs:42`'s `create(&self, actor: &PrincipalId, …)` already threads
the creating principal through to an owner grant. What is missing is storage: no tenancy
aggregate carries a creator (`rs/crates/libs/paigasus-iam-core/src/tenancy.rs:187` —
`Organization` has `created_at` / `updated_at` only, as do `Team` and `Project`), and no
migration defines a `created_by` column anywhere in `rs/`.

That is why writing the actor on create only would be *worse* than leaving it unset: a
subsequent GET could not return what the CREATE reported, so the field would be inconsistent
across reads of the same entity. Populating it properly needs a domain field, a schema
migration, and a repository change per aggregate — the M2 work the existing comment defers
to. `convert::audit(created, updated)`'s two-timestamp signature therefore stays unchanged.

## Testing

Existing tests port to the new shape; the suite does not grow.

- `paigasus-proto/src/audit.rs` unit tests — one assertion changes character usefully:
  today's `assert_eq!(dto.modified_by(), Some(""))`, commented "empty actor is a meaningful
  value (unknown/system), distinct from absent audit", becomes
  `assert_eq!(dto.modifier(), None)` **paired in the same test with**
  `assert!(dto.audit().is_some())`. That is D5's collapse made executable: the sentinel is
  gone from the actor accessor, and the present-vs-absent distinction is pinned where it now
  lives.
- `paigasus-proto/tests/auditable_derive.rs` — **two** blocks, not one. The `HandWritten`
  struct and its tests (lines 10-39) construct `AuditMetadata { created_by: "svc", .. }` and
  assert `Some("")`; the distinct-sentinel macro table (lines 41-83) uses `stringify!($ty)`.
  Both build `Actor { prn: … }` instead. The table's sentinels become **real canonical PRNs**
  with a per-type discriminating uuid rather than `"Organization"` / `"Team"` — a bare type
  name is not a parseable PRN, and fixtures should not model what D2 tells producers not to
  write.
- `paigasus-proto/tests/auditable_derive_drift.rs` — **no change**. Its biconditional keys on
  `audit: Option<AuditMetadata>` fields; neither `Actor` nor the retyped `creator`/`modifier`
  is one.
- `py/.../tests/test_audit_protocol.py` and `ts/.../src/audit.test.ts` — construct an `Actor`
  where they construct `created_by="svc"` / `createdBy: 'svc'` today. The TS test's
  `asAuditable` compile-time identity check is the real assertion and is unaffected. The
  Python test is weaker than it looks — `isinstance` against a `runtime_checkable` Protocol
  declaring only `audit` checks attribute *presence* and would pass with `AuditMetadata`
  empty — so it gains `assert obj.audit is not None and obj.audit.creator.prn == …` to make
  it prove something about the rename.
- `paigasus-iam` — **a new test, because there is none today.** No file under
  `paigasus-iam/src/` or `tests/` asserts on `created_by` / `modified_by` (verified by grep:
  zero matches outside `convert.rs`'s own two write sites), so D8's behaviour would otherwise
  ship unasserted. Add one unit test in `convert.rs` asserting
  `audit(t1, t2).creator.is_none()` and `.modifier.is_none()` — cheap, and it is the tripwire
  for a future author tempted to write a synthetic "system" PRN there.

## Verification

`contracts:{lint,fmt,breaking}` are already measured green on this exact proto shape (D3).
The full graph must still run before push — the marker-delimited `moon ci …` command in
CLAUDE.md — with codegen-drift the one that matters most here, since the regenerated Rust,
Python and TypeScript sources must all be committed.

## Out of scope

- Migrating `AuditEntry.actor_prn` (`iam.proto:475,493,619`) to `Actor`. Worth doing; a
  separate change. Acknowledged tension: shipping this leaves the repo with `Actor{prn}` in
  `common/v1` and bare `string actor_prn` in `iam/v1` — two spellings of one idea, which is
  the state D4 uses to reject a deprecation window. The difference is that these are
  different messages on independently sequenced work, not two live encodings of the *same*
  field; but it should be closed rather than left indefinitely.
- `display_name` on `Actor`. Adding a field to a proto message is non-breaking, so it costs
  nothing to defer and can land the day a consumer needs it. No producer could populate it
  today — IAM has no persisted creator to name (D8).
- Wiring a real actor through IAM's request context, and the `created_by` column and
  migration that would require (M2).
