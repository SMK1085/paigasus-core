# SMA-439 — Structured `Actor` message replacing opaque `created_by` / `modified_by`

**Issue:** [SMA-439](https://linear.app/smaschek/issue/SMA-439/structured-actor-message-to-replace-opaque-created-by-modified-by)
**Follow-up from:** SMA-425 (explicitly out of scope there)
**ADR context:** [ADR-0012 — per-language thin wrappers](https://app.notion.com/p/386830e8fbaa816d8a43e941ef5b4c4f), ADR-0014 (PRN), ADR-0019 A1.2 (reserve+add)
**Date:** 2026-08-27

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
`AuthContext.principal_id` is a `Prn`; `grpc/dead_letters.rs::actor_prn` derives the acting
identity straight from it; and `AuditEntry.actor_prn` is the established audit-log shape.

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
message Actor {
  // Canonical PRN of the actor (ADR-0014), e.g. prn:pgs:iam:::principal/<uuid>.
  // The PRN's resource-type segment carries the kind (user / service-account /
  // principal), so there is deliberately no separate kind enum.
  string prn = 1;
}
```

### D2 — Absence is the only "unknown/system" representation

An unset `Actor` field means unknown-or-system, exactly what `""` means today. A **present**
`Actor` always carries a real, parseable PRN — that is the invariant the type buys.

This is why `Actor` is a message despite holding a single field. A bare
`string creator_prn = 5` would have no presence and would drag the empty-string sentinel
straight back in; a message field is genuinely absent-or-present. The one-field message is
load-bearing, not ceremony.

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
| Also reserve the names, then reuse them | ❌ `buf lint`: *"use of reserved message field name"* — protobuf forbids it, so the two requirements are mutually exclusive |
| Change the type in place at 3/4 | ❌ `FIELD_SAME_TYPE`, plus cardinality `implicit presence` → `explicit presence` |
| `reserved 3, 4` + names, new fields at 5/6 | ✅ `buf lint` 0, `buf breaking` 0, `buf format --exit-code` 0 |

`creator` / `modifier` over `created_by_actor` / `modified_by_actor`: the `_actor` suffix
restates the field's own type, and it breaks the rhyme with the `created_at` / `modified_at`
pair it sits beside. This matches the `IntrospectResponse.role_group_prns` → `role_grants`
precedent (`iam.proto:241`) of picking a genuinely better name rather than a suffixed old one.

Loosening `contracts/buf.yaml` — adding `FIELD_NO_DELETE_UNLESS_NAME_RESERVED` to `except` —
was considered and rejected: it would weaken the breaking gate for every message in the repo,
permanently, to save one rename, and contradicts the posture that file's own comments record.

### D4 — Clean break, no deprecation window

The strings are deleted, not deprecated alongside. There are zero git tags in the repo;
SMA-577 only *made* the proto family publishable at 0.1.0, and nothing has been released.
There is no external wire consumer to ease over, so a deprecation window would buy nothing
and would leave two representations of one fact to drift.

### D5 — Rust surface

`rs/crates/libs/paigasus-proto/src/audit.rs` re-exports `Actor` beside `AuditMetadata` — the
same one-stable-anchor rationale the file already documents, so consumers never name the
codegen module layout that `clean: true` regenerates. The two accessors swap:

```rust
fn creator(&self)  -> Option<&Actor> { self.audit().and_then(|a| a.creator.as_ref()) }
fn modifier(&self) -> Option<&Actor> { self.audit().and_then(|a| a.modifier.as_ref()) }
```

`created_by()` / `modified_by()` are removed outright (D4).

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

It stays unset because it is genuinely blocked, not merely deferred: no tenancy aggregate
persists a creator (`iam-core/src/tenancy.rs:187` — `Organization` carries `created_at` /
`updated_at` only, as do `Team` and `Project`), and no migration defines a `created_by`
column anywhere in `rs/`. Populating it needs a domain change, a schema migration, and a
repository change per aggregate — the M2 work the existing comment already defers to.

## Testing

Existing tests port to the new shape; the suite does not grow.

- `paigasus-proto/src/audit.rs` unit tests — one assertion changes character usefully:
  today's `assert_eq!(dto.modified_by(), Some(""))`, commented "empty actor is a meaningful
  value (unknown/system), distinct from absent audit", becomes
  `assert_eq!(dto.modifier(), None)`. That is D2 made executable — the sentinel is gone, so
  the case it documented no longer exists.
- `paigasus-proto/tests/auditable_derive.rs` — the distinct-sentinel table builds
  `Actor { prn: … }` per type instead of a bare string.
- `paigasus-proto/tests/auditable_derive_drift.rs` — **no change**. Its biconditional keys on
  `audit: Option<AuditMetadata>` fields; neither `Actor` nor the retyped `creator`/`modifier`
  is one.
- `py/.../tests/test_audit_protocol.py` and `ts/.../src/audit.test.ts` — construct an `Actor`
  where they construct `created_by="svc"` / `createdBy: 'svc'` today. The TS test's
  `asAuditable` compile-time identity check is the real assertion and is unaffected.
- `paigasus-iam` — `convert.rs`'s own tests, and any integration assertion reading the audit
  fields, follow the `None` change.

## Verification

`contracts:{lint,fmt,breaking}` are already measured green on this exact proto shape (D3).
The full graph must still run before push — the marker-delimited `moon ci …` command in
CLAUDE.md — with codegen-drift the one that matters most here, since the regenerated Rust,
Python and TypeScript sources must all be committed.

## Out of scope

- Migrating `AuditEntry.actor_prn` (`iam.proto`) to `Actor`. Worth doing; a separate change.
- `display_name` on `Actor`. Adding a field to a proto message is non-breaking, so it costs
  nothing to defer and can land the day a consumer needs it. No producer could populate it
  today — IAM has no persisted creator to name (D8).
- Wiring a real actor through IAM's request context, and the `created_by` column and
  migration that would require (M2).
