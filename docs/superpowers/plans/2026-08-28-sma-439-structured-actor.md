# Structured `Actor` Message Implementation Plan (SMA-439)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace `AuditMetadata`'s opaque `created_by` / `modified_by` strings with a structured, PRN-canonical `paigasus.common.v1.Actor` message, and update the per-language `Auditable` wrappers and the one producer.

**Architecture:** A new one-field `Actor` message (`string prn = 1`) lands in its own proto file. `AuditMetadata` reserves field numbers **and** names 3/4 and adds `Actor creator = 5` / `Actor modifier = 6` — the only route that passes buf (measured; see spec D3). Absence of an `Actor` is the canonical "unknown/system"; an empty or unparseable `prn` collapses to the same meaning by a **documented producer/consumer contract**, not by enforcement. The Rust `Auditable` trait swaps its two string accessors for `Actor` ones; Python and TypeScript wrappers are structurally unchanged because they declare only `audit`.

**Tech Stack:** protobuf 3 + buf (lint/format/breaking/generate) · Rust (prost, tonic, a `#[derive(Auditable)]` proc-macro) · Python (betterproto2, uv) · TypeScript (protobuf-es v2, pnpm, vitest) · Moon task orchestration.

**Spec:** `docs/superpowers/specs/2026-08-27-sma-439-structured-actor-design.md` — read it alongside this plan. Every decision below is argued there (D1–D8).

## Global Constraints

- **Working directory:** `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-439`, branch `feature/sma-439-structured-actor-message`. This is a git worktree — run everything from here; never `cd` to the main checkout.
- **PATH:** the Bash tool's PATH lacks the proto-managed CLIs. Prefix every command that runs `moon` / `buf` / `uv` / `cargo nextest` with:
  `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` (shims FIRST — that is what selects the repo-pinned versions).
- **SPDX header:** every source file starts with `// SPDX-License-Identifier: Apache-2.0` (`#` for Python). No gate enforces this; an omission ships green.
- **Rust edition:** 2024, rust-version 1.95 — inherited from the workspace; do not add per-crate overrides.
- **Do NOT hand-edit any version field.** `paigasus-proto` sits in release-plz's `proto` `version_group`; `repo:version-lockstep` checks eighteen sites and a partial bump reds it. release-plz owns the bump.
- **Do NOT add a `paigasus-kernel` dependency to `paigasus-proto`** (spec D6). `Actor.prn` stays a `String`. The dependency would red `repo:affected-smoke`'s `kernel->bindings` strict-equality set and cost `paigasus-kernel` its standalone publish assertion.
- **Do NOT bypass git hooks** with `--no-verify`. The worktree is already provisioned (pnpm, uv, cargo all installed), so `commitlint` works.
- **Commit subjects start lowercase** and are ≤100 chars. Keep `#NNN` out of commit bodies — it breaks `footer-leading-blank` in commitlint.
- **`buf format` is mandatory** after any `.proto` edit, or `contracts:fmt` reds CI silently.
- **Regenerated code is committed.** `contracts:generate` writes into all three languages; the codegen-drift gate fails if the working tree differs from what `buf generate` produces.

---

## File Structure

**Created:**
- `contracts/proto/paigasus/common/v1/actor.proto` — the `Actor` message and its normative contract comment. Its own file because `common/v1` splits by concept and `Actor` is broader than audit (spec D1).
- `ts/packages/paigasus-proto/src/generated/paigasus/common/v1/actor_pb.ts` — **generated, not hand-written.** protobuf-es emits one module per proto file. Must be committed.

**Modified:**
- `contracts/proto/paigasus/common/v1/audit.proto` — reserve 3/4 + names; add `creator`/`modifier`; import `actor.proto`.
- `rs/crates/libs/paigasus-proto/src/generated/paigasus/common/v1/paigasus.common.v1.rs` — generated; prost appends `Actor` to the existing per-package module.
- `py/packages/paigasus-proto/src/paigasus_proto/generated/paigasus/common/v1/__init__.py` — generated; betterproto2 appends `Actor` to the existing package module.
- `ts/packages/paigasus-proto/src/generated/paigasus/common/v1/audit_pb.ts` — generated.
- `rs/crates/libs/paigasus-proto/src/audit.rs` — re-export `Actor`; swap the two trait accessors; port two unit tests.
- `rs/crates/libs/paigasus-proto/tests/auditable_derive.rs` — port **two** blocks: the `HandWritten` tests and the generated-type macro table.
- `ts/packages/paigasus-proto/src/index.ts` — add `ActorSchema` / `Actor` re-exports.
- `ts/packages/paigasus-proto/src/audit.test.ts` — build an `Actor`.
- `py/packages/paigasus-proto/tests/test_audit_protocol.py` — build an `Actor`, and assert a field so the test proves something.
- `rs/crates/services/paigasus-iam/src/adapters/grpc/convert.rs` — `audit()` writes `None`; comment updated; **new** unit test.

**Unchanged (verified, do not touch):**
- `contracts/buf.gen.yaml` — the `message_attribute=` injection list keys on messages that *embed* `AuditMetadata`. `Actor` embeds nothing; `AuditMetadata`'s membership is unchanged.
- `rs/crates/libs/paigasus-proto/tests/auditable_derive_drift.rs` — its biconditional keys on `audit: Option<AuditMetadata>` fields. Neither `Actor` nor `creator`/`modifier` is one. Its `assert_eq!(total, 7)` non-vacuity anchor also survives.
- `rs/crates/libs/paigasus-proto-derive/` — the macro hard-codes only the field name `audit` and emits only `fn audit()`. The renamed accessors are trait defaults.
- `py/packages/paigasus-proto/src/paigasus_proto/audit.py` and `ts/packages/paigasus-proto/src/audit.ts` — both declare only `audit`, so neither changes shape (spec D7).
- `py/packages/paigasus-proto/src/paigasus_proto/__init__.py` — empty, stays empty.

---

## Task 1: The wire contract

Creates `Actor`, retires the string fields, and regenerates all three languages. Everything downstream fails to compile until this lands, so it goes first and carries its own gate run.

**Files:**
- Create: `contracts/proto/paigasus/common/v1/actor.proto`
- Modify: `contracts/proto/paigasus/common/v1/audit.proto`
- Generated (commit, do not hand-edit): `rs/crates/libs/paigasus-proto/src/generated/paigasus/common/v1/paigasus.common.v1.rs`, `py/packages/paigasus-proto/src/paigasus_proto/generated/paigasus/common/v1/__init__.py`, `ts/packages/paigasus-proto/src/generated/paigasus/common/v1/audit_pb.ts`, `ts/packages/paigasus-proto/src/generated/paigasus/common/v1/actor_pb.ts`

**Interfaces:**
- Consumes: nothing.
- Produces: the generated types every later task uses —
  - Rust: `paigasus_proto::paigasus::common::v1::Actor { pub prn: String }`, and `AuditMetadata { created_at, modified_at, creator: Option<Actor>, modifier: Option<Actor> }`. Both derive `Clone, PartialEq, Eq, Hash, ::prost::Message` (verified against real `buf generate` output — do not assume, but this was measured).
  - Python: `paigasus_proto.generated.paigasus.common.v1.Actor(prn=...)`, `AuditMetadata(creator=..., modifier=...)`.
  - TypeScript: `ActorSchema` / `Actor` from `./generated/paigasus/common/v1/actor_pb.js`; `AuditMetadata` fields `creator` / `modifier` (protobuf-es lowerCamelCases field names, but these two are already single words).

- [ ] **Step 1: Create the Actor proto**

Create `contracts/proto/paigasus/common/v1/actor.proto`:

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
// what stops that from becoming a second spelling of "unknown".
//
// Deliberately carries no `kind` enum: the PRN's resource-type segment already
// encodes it (user / service-account / principal), and `prnResourceType` is
// exported from the kernel bindings in every language, so branching on kind does
// not require a duplicated field. It also carries no `display_name`: adding a
// proto field is non-breaking, so it can land the day a consumer needs one.
message Actor {
  // Canonical PRN of the actor, e.g. prn:pgs:iam:::principal/<uuid>.
  string prn = 1;
}
```

- [ ] **Step 2: Retire the string fields in audit.proto**

In `contracts/proto/paigasus/common/v1/audit.proto`, add the import directly below the existing timestamp import:

```proto
import "google/protobuf/timestamp.proto";
import "paigasus/common/v1/actor.proto";
```

Then replace the two `created_by` / `modified_by` field declarations (and their doc comments) with:

```proto
  // RESERVED PERMANENTLY. These held opaque actor strings until SMA-439 replaced
  // them with `Actor`. buf keys its field-deletion check on the NUMBER, so
  // retiring them requires reserving the names too — which means `created_by` and
  // `modified_by` can never again name a field in this message, in any language.
  reserved 3, 4;
  reserved "created_by", "modified_by";

  // Who created the entity. ABSENT means unknown-or-system; see Actor's contract,
  // under which an empty or unparseable `prn` means the same thing.
  Actor creator = 5;

  // Who last modified the entity. Same absence semantics as `creator`.
  Actor modifier = 6;
```

- [ ] **Step 3: Format, then verify the gates pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd contracts
buf format -w
buf lint;     echo "lint=$?"
buf breaking --against '../.git#branch=main,subdir=contracts'; echo "breaking=$?"
```

Expected: `lint=0` and `breaking=0`. If `breaking` reports *"deleted without reserving the name"*, the `reserved "created_by", "modified_by";` line is missing or misspelled. If `buf lint` reports *"use of reserved message field name"*, a field is still **named** `created_by` or `modified_by` — that is a protobuf compiler error, not a lint rule, and cannot be waived.

- [ ] **Step 4: Regenerate all three languages**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd contracts && buf generate
```

A `duplicate generated file name "paigasus/common/v1/__init__.py"` warning from the betterproto2 plugin is **pre-existing and harmless** — it appears on `main` too. Exit code is 0.

- [ ] **Step 5: Verify the generated shape is what later tasks expect**

```bash
grep -n -B1 "pub struct Actor\b" rs/crates/libs/paigasus-proto/src/generated/paigasus/common/v1/paigasus.common.v1.rs
sed -n '/pub struct AuditMetadata/,/^}/p' rs/crates/libs/paigasus-proto/src/generated/paigasus/common/v1/paigasus.common.v1.rs
ls ts/packages/paigasus-proto/src/generated/paigasus/common/v1/actor_pb.ts
```

Expected: `Actor` carries `#[derive(Clone, PartialEq, Eq, Hash, ::prost::Message)]`; `AuditMetadata` has `creator` and `modifier` as `::core::option::Option<Actor>` at tags 5 and 6; `actor_pb.ts` exists.

**If `Eq`/`Hash` is missing from either struct, STOP and report** — six IAM messages embed `AuditMetadata` and would lose those derives too. (It was verified present during design; this step is the tripwire, not an expectation to hand-wave.)

- [ ] **Step 6: Commit**

The Rust/Py/TS code does not compile yet — that is expected and is fixed by Tasks 2–5. Commit the contract and its generated output together so the tree matches `buf generate` at every commit.

```bash
git add contracts/ rs/crates/libs/paigasus-proto/src/generated/ py/packages/paigasus-proto/src/paigasus_proto/generated/ ts/packages/paigasus-proto/src/generated/
git commit -m "feat(contracts)!: carry a structured Actor in AuditMetadata (SMA-439)"
```

---

## Task 2: Rust `Auditable` trait

**Files:**
- Modify: `rs/crates/libs/paigasus-proto/src/audit.rs`
- Test: `rs/crates/libs/paigasus-proto/src/audit.rs` (its own `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `Actor` and the retyped `AuditMetadata` from Task 1.
- Produces: `paigasus_proto::audit::Actor` (re-export), and on the `Auditable` trait:
  `fn creator(&self) -> Option<&Actor>` and `fn modifier(&self) -> Option<&Actor>`.
  `created_by()` / `modified_by()` **no longer exist**. Tasks 3 and 5 depend on these exact names.

- [ ] **Step 1: Write the failing tests**

Replace the two tests inside `#[cfg(test)] mod tests` in `rs/crates/libs/paigasus-proto/src/audit.rs`. Note the import line gains `Actor`:

```rust
#[cfg(test)]
mod tests {
    use super::Auditable;
    use crate::paigasus::common::v1::{Actor, AuditMetadata, AuditableExample};

    // No manual impl here any more: `AuditableExample` now carries #[derive(Auditable)] via
    // codegen (SMA-438), so the two tests below exercise the DERIVED impl. Re-adding a manual
    // one is an E0119 conflict. Note this makes the fixture's impl public API, reversing
    // SMA-425's decision to keep it test-only — deliberate, see SMA-438 spec D8.

    const PRN: &str = "prn:pgs:iam:::principal/0192f1c0-0000-7000-8000-000000000001";

    #[test]
    fn accessors_read_through_embedded_metadata() {
        let dto = AuditableExample {
            id: "x".to_string(),
            audit: Some(AuditMetadata {
                creator: Some(Actor { prn: PRN.to_string() }),
                ..Default::default()
            }),
        };
        assert_eq!(dto.creator().map(|a| a.prn.as_str()), Some(PRN));
        // SMA-439: an unknown actor and an absent actor are now the SAME fact about the
        // actor, so `modifier()` is None here where `modified_by()` used to be Some("").
        // The present-vs-absent distinction did not vanish — it lives on `audit()` alone
        // now, and asserting both together is what pins that collapse as intended.
        assert!(dto.audit().is_some());
        assert_eq!(dto.modifier(), None);
        // created_at was never set, so the timestamp accessor is None even though audit is Some.
        assert_eq!(dto.created_at(), None);
    }

    #[test]
    fn absent_audit_yields_none_accessors() {
        let dto = AuditableExample { id: "y".to_string(), audit: None };
        assert_eq!(dto.audit(), None);
        assert_eq!(dto.creator(), None);
        assert_eq!(dto.modifier(), None);
        assert_eq!(dto.created_at(), None);
        assert_eq!(dto.modified_at(), None);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo test -p paigasus-proto --lib 2>&1 | tail -20
```

Expected: FAIL to compile — `no method named 'creator' found for reference '&AuditableExample'`.

- [ ] **Step 3: Swap the trait accessors and re-export `Actor`**

In `rs/crates/libs/paigasus-proto/src/audit.rs`, change the first re-export line to also carry `Actor`, and replace the two string accessors. The re-export comment above it already explains the one-stable-anchor rationale and still applies:

```rust
pub use crate::paigasus::common::v1::{Actor, AuditMetadata};
```

```rust
    /// Who created the entity, or `None` if unknown/system.
    ///
    /// Per `Actor`'s contract an empty or unparseable `prn` ALSO means unknown, but this
    /// accessor deliberately does not normalise that away: the rule is a producer
    /// obligation stated once in the proto, and enforcing it here — in one of three
    /// languages, on one of two access paths, since `.creator` stays readable directly —
    /// would make the trait and the field disagree (SMA-439 spec D2).
    fn creator(&self) -> Option<&Actor> {
        self.audit().and_then(|a| a.creator.as_ref())
    }
    /// Who last modified the entity, or `None` if unknown/system. See [`Auditable::creator`].
    fn modifier(&self) -> Option<&Actor> {
        self.audit().and_then(|a| a.modifier.as_ref())
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo test -p paigasus-proto --lib 2>&1 | tail -20
```

Expected: PASS, 2 tests.

- [ ] **Step 5: Commit**

```bash
git add rs/crates/libs/paigasus-proto/src/audit.rs
git commit -m "feat(rs): return Actor from the Auditable accessors (SMA-439)"
```

---

## Task 3: Rust derive integration tests

The `auditable_derive.rs` suite has **two** blocks that both construct `AuditMetadata`, and both must be ported. Missing the second leaves the crate uncompilable.

**Files:**
- Modify: `rs/crates/libs/paigasus-proto/tests/auditable_derive.rs`

**Interfaces:**
- Consumes: `creator()` / `modifier()` and the `Actor` re-export from Task 2.
- Produces: nothing later tasks use.

- [ ] **Step 1: Port the `HandWritten` block**

Change the import at the top of `rs/crates/libs/paigasus-proto/tests/auditable_derive.rs`:

```rust
use paigasus_proto::audit::{Actor, AuditMetadata, Auditable};
```

Add a PRN helper directly below the `HandWritten` struct definition, and replace the two tests:

```rust
/// A distinct canonical PRN per fixture. Test fixtures obey `Actor`'s producer obligation
/// (SMA-439 spec D2) rather than modelling what it tells producers not to write — a bare
/// type name is not a parseable PRN.
fn actor(n: u32) -> Actor {
    Actor { prn: format!("prn:pgs:iam:::principal/0192f1c0-0000-7000-8000-{n:012}") }
}

#[test]
fn derived_impl_reads_through_to_embedded_metadata() {
    let dto = HandWritten {
        prn: "p".to_string(),
        audit: Some(AuditMetadata {
            creator: Some(actor(1)),
            ..Default::default()
        }),
    };
    // A sentinel value, not just `is_some()` — a derive emitting `{ None }` must fail here.
    assert_eq!(dto.creator(), Some(&actor(1)));
    // SMA-439: an unknown actor reads as None, exactly like an absent one. The
    // present-vs-absent distinction now lives on `audit()` alone, so assert both.
    assert!(dto.audit().is_some());
    assert_eq!(dto.modifier(), None);
    assert_eq!(dto.created_at(), None);
}

#[test]
fn absent_audit_yields_none_accessors() {
    let dto = HandWritten::default();
    assert_eq!(dto.audit(), None);
    assert_eq!(dto.creator(), None);
    assert_eq!(dto.modified_at(), None);
}
```

- [ ] **Step 2: Port the generated-type macro table**

Further down the same file, replace the `stamped` helper and the macro body. The block comment above them mentions `created_by` — update it too:

```rust
// ─── Generated messages (SMA-438) ────────────────────────────────────────────────────────────
//
// Each type is built with a DISTINCT actor PRN in `creator` and asserted to return exactly
// that. A bare `fn assert_auditable<T: Auditable>()` bound would prove only that an impl
// EXISTS — a derive emitting `{ None }` would satisfy it for six of the seven types.

use paigasus_proto::paigasus::common::v1::AuditableExample;
use paigasus_proto::paigasus::iam::v1::{ApiKey, Membership, Organization, Project, ServiceAccount, Team};

fn stamped(who: &Actor) -> Option<AuditMetadata> {
    Some(AuditMetadata {
        creator: Some(who.clone()),
        ..Default::default()
    })
}

macro_rules! generated_type_reads_through {
    ($($name:ident => $ty:ty => $n:expr),+ $(,)?) => {$(
        #[test]
        fn $name() {
            let sentinel = actor($n);
            let mut dto = <$ty>::default();
            dto.audit = stamped(&sentinel);
            assert_eq!(dto.creator(), Some(&sentinel), "derived impl did not read the audit field");
            assert_eq!(dto.audit().and_then(|a| a.creator.as_ref()), Some(&sentinel));

            let empty = <$ty>::default();
            assert_eq!(empty.audit(), None, "absent audit must yield None");
            assert_eq!(empty.creator(), None);
            assert_eq!(empty.modified_at(), None);
        }
    )+};
}

generated_type_reads_through! {
    auditable_example_reads_through => AuditableExample  => 10,
    organization_reads_through      => Organization      => 11,
    team_reads_through              => Team              => 12,
    project_reads_through           => Project           => 13,
    membership_reads_through        => Membership        => 14,
    service_account_reads_through   => ServiceAccount    => 15,
    api_key_reads_through           => ApiKey            => 16,
}
```

The macro gains a third `$n` argument because `stringify!($ty)` no longer works as a sentinel — the distinct-value property that makes this table meaningful now comes from a distinct PRN per type.

- [ ] **Step 3: Run the full crate test suite**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo test -p paigasus-proto 2>&1 | tail -25
```

Expected: PASS — 2 lib tests, 9 in `auditable_derive`, and the `auditable_derive_drift` tests **unchanged and still passing** (its `assert_eq!(total, 7)` anchor must still hold; if it does not, a `buf.gen.yaml` injection line was disturbed, which this change should not have touched).

- [ ] **Step 4: Commit**

```bash
git add rs/crates/libs/paigasus-proto/tests/auditable_derive.rs
git commit -m "test(rs): port the Auditable derive suite to Actor (SMA-439)"
```

---

## Task 4: Python and TypeScript surfaces

Grouped because neither wrapper changes shape (spec D7) — the work is a re-export and two test ports, too small to gate separately.

**Files:**
- Modify: `ts/packages/paigasus-proto/src/index.ts`
- Modify: `ts/packages/paigasus-proto/src/audit.test.ts`
- Modify: `py/packages/paigasus-proto/tests/test_audit_protocol.py`
- **Do not modify:** `ts/packages/paigasus-proto/src/audit.ts`, `py/.../paigasus_proto/audit.py` — both declare only `audit` and are correct as-is.

**Interfaces:**
- Consumes: the generated `Actor` types from Task 1.
- Produces: `Actor` / `ActorSchema` exported from `@paigasus/proto`'s barrel.

- [ ] **Step 1: Add the TypeScript re-exports**

In `ts/packages/paigasus-proto/src/index.ts`, add two lines beside the existing `AuditMetadata` exports, keeping the file's alphabetical-by-path grouping:

```typescript
export { ActorSchema } from './generated/paigasus/common/v1/actor_pb.js';
export type { Actor } from './generated/paigasus/common/v1/actor_pb.js';
```

- [ ] **Step 2: Port the TypeScript test**

In `ts/packages/paigasus-proto/src/audit.test.ts`, add the `ActorSchema` import beside the others and rewrite the first test's body:

```typescript
import { ActorSchema } from './generated/paigasus/common/v1/actor_pb.js';
```

```typescript
  it('the generated AuditableExample structurally satisfies Auditable', () => {
    const prn = 'prn:pgs:iam:::principal/0192f1c0-0000-7000-8000-000000000001';
    const dto = asAuditable(
      create(AuditableExampleSchema, {
        id: 'x',
        audit: create(AuditMetadataSchema, { creator: create(ActorSchema, { prn }) }),
      }),
    );
    expect(dto.audit?.creator?.prn).toBe(prn);
  });
```

- [ ] **Step 3: Run the TypeScript checks**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run paigasus-proto-ts:typecheck paigasus-proto-ts:test
```

Expected: both PASS. The `asAuditable` call is the real assertion here — it is a compile-time identity check that `tsc` rejects if the generated message stops satisfying `Auditable`.

- [ ] **Step 4: Port the Python test**

In `py/packages/paigasus-proto/tests/test_audit_protocol.py`, update the import line and the first test:

```python
from paigasus_proto.generated.paigasus.common.v1 import Actor, AuditableExample, AuditMetadata


def test_generated_example_satisfies_auditable() -> None:
    prn = "prn:pgs:iam:::principal/0192f1c0-0000-7000-8000-000000000001"
    obj = AuditableExample(
        id="x",
        audit=AuditMetadata(
            creator=Actor(prn=prn),
            created_at=datetime(2026, 1, 1, tzinfo=UTC),
        ),
    )
    assert isinstance(obj, Auditable)
    # `isinstance` against a runtime_checkable Protocol declaring only `audit` checks
    # attribute PRESENCE and would pass with AuditMetadata empty. Assert a field so this
    # test actually proves something about the Actor rename (SMA-439).
    assert obj.audit is not None
    assert obj.audit.creator is not None
    assert obj.audit.creator.prn == prn
```

Leave the other two tests untouched — they assert Protocol structure, not field content.

- [ ] **Step 5: Run the Python and formatting checks**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run paigasus-proto-py:test
moon run ts:fmt
```

Expected: both PASS. `ts:fmt` is a whole-tree Prettier gate decoupled from lint/typecheck — run it whenever any `.ts` file changes, or CI reds separately.

- [ ] **Step 6: Commit**

```bash
git add ts/packages/paigasus-proto/src/index.ts ts/packages/paigasus-proto/src/audit.test.ts py/packages/paigasus-proto/tests/test_audit_protocol.py
git commit -m "feat(ts): export Actor and port the audit tests to it (SMA-439)"
```

---

## Task 5: The IAM producer

`convert::audit()` is the only code in the repo that writes these fields. It currently writes empty strings; it now writes `None`. It ships with a **new** test because no test touches these fields today.

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/grpc/convert.rs` (the `audit()` fn at ~line 245, and its `#[cfg(test)] mod tests` at ~line 523)

**Interfaces:**
- Consumes: the retyped `AuditMetadata` from Task 1.
- Produces: `pub fn audit(created: DateTime<Utc>, updated: DateTime<Utc>) -> AuditMetadata` — signature **unchanged**. Its seven call sites are untouched.

- [ ] **Step 1: Write the failing test**

Add to the existing `#[cfg(test)] mod tests` block in `convert.rs`:

```rust
    #[test]
    fn audit_leaves_the_actor_unset() {
        let t = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let meta = audit(t, t);

        // IAM cannot name the actor: no tenancy aggregate persists a creator and no
        // migration defines the column (SMA-439 spec D8). Absent is the canonical
        // "unknown" — a synthetic "system" PRN here would violate Actor's contract,
        // and writing the request's actor on create only would make the field
        // inconsistent across later reads of the same entity.
        assert!(meta.creator.is_none());
        assert!(meta.modifier.is_none());
        // The timestamps are what this builder DOES know.
        assert_eq!(meta.created_at, Some(ts(t)));
        assert_eq!(meta.modified_at, Some(ts(t)));
    }
```

- [ ] **Step 2: Run it to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo test -p paigasus-iam --lib audit_leaves_the_actor_unset 2>&1 | tail -20
```

Expected: FAIL to compile — `struct 'AuditMetadata' has no field named 'created_by'` at the `audit()` builder, which still writes the removed fields.

- [ ] **Step 3: Update the builder**

Replace the `audit()` function and its doc comment in `convert.rs`:

```rust
/// Builds `AuditMetadata` from created/modified timestamps. `creator`/`modifier` stay ABSENT —
/// the canonical "unknown/system" (SMA-439) — until the actor is PERSISTED. The acting
/// principal is already available at every mutation (`actor_prn(&AuthContext)`), but no
/// tenancy aggregate stores a creator and no migration defines the column, so writing it on
/// create only would leave the field inconsistent across later reads (M2, task-16 brief).
pub fn audit(created: DateTime<Utc>, updated: DateTime<Utc>) -> AuditMetadata {
    AuditMetadata {
        created_at: Some(ts(created)),
        modified_at: Some(ts(updated)),
        creator: None,
        modifier: None,
    }
}
```

- [ ] **Step 4: Run the test to verify it passes**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo test -p paigasus-iam --lib audit_leaves_the_actor_unset 2>&1 | tail -10
```

Expected: PASS.

- [ ] **Step 5: Build the whole workspace**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo build --workspace 2>&1 | tail -20
```

Expected: clean build. Any remaining `created_by` / `modified_by` reference anywhere in `rs/` surfaces here. If one appears in a crate this plan does not list, **stop and report it** — the spec's blast-radius analysis missed a consumer.

- [ ] **Step 6: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/adapters/grpc/convert.rs
git commit -m "feat(rs): leave the iam audit actor absent rather than empty (SMA-439)"
```

---

## Task 6: Full-graph verification

Per-project Moon tasks do **not** run the repo-level gates. This change adds a proto file, renames proto fields, and regenerates three languages — exactly the shape that trips codegen-drift and `:affected-smoke`. Run the graph the way CI does before pushing.

**Files:** none modified unless a gate reds.

- [ ] **Step 1: Confirm the working tree matches `buf generate`**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd contracts && buf generate && cd ..
git status --short
```

Expected: **no output**. Any modified file here means a generated artifact was hand-edited or a regeneration was not committed — re-stage and amend the Task 1 commit rather than adding a fixup.

- [ ] **Step 2: Run the full CI graph**

Copy the command from `CLAUDE.md`'s `ci-targets` markers verbatim (do not retype from memory — `repo:affected-smoke` asserts it matches `ci.yml`'s `T=(…)` array exactly):

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :input-liveness :promtool :observability-drift :nats-permissions :release-parity \
  :release-parity-py :release-parity-ts :publish-metadata :version-lockstep --base origin/main \
  --include-relations
```

Expected: all green.

Notes for reading a failure:
- Moon reports failures unattributed. Identify the failing task with
  `jq '.actions[] | select(.status=="failed") | .label' .moon/cache/ciReport.json`.
- `paigasus-iam`'s Docker-backed suites need a reachable daemon. If Docker is down, `tests/docker_preflight.rs` fails deliberately as a canary — that is one red standing in for 64 silent skips, not a bug in this change. Start Docker and re-run rather than setting `PAIGASUS_SKIP_DOCKER=1`.
- **Do not hand-fix a `:version-lockstep` failure by editing a version.** Report it — this change should not touch any version site.

- [ ] **Step 3: Verify no stale references survive**

```bash
grep -rn "created_by\|modified_by\|createdBy\|modifiedBy" --include="*.rs" --include="*.py" --include="*.ts" --include="*.proto" . \
  | grep -v node_modules | grep -v "/target/" | grep -v "\.venv"
```

Expected: only the `reserved "created_by", "modified_by";` line in `audit.proto` and the comment above it. Anything else is a missed consumer — report it.

- [ ] **Step 4: Commit any gate-driven fixes**

Only if Step 2 required changes:

```bash
git add -A
git commit -m "fix(repo): satisfy <gate name> for the Actor change (SMA-439)"
```

---

## Self-Review

**Spec coverage.** D1 → Task 1 Steps 1–2 (own file, SPDX, normative contract comment, no kind enum, no display_name) and Step 5 (`actor_pb.ts`). D2 → the contract comment in Task 1 Step 1, plus Task 2 Step 3's explicit non-normalisation note and the paired `audit().is_some()` / `modifier().is_none()` assertions in Tasks 2 and 3. D3 → Task 1 Steps 2–3, with the failure modes of both rejected routes named as diagnostics. D4 → Task 1 Step 6's `feat(contracts)!:` subject and the Global Constraint forbidding hand-bumps; the skew analysis needs no task (no code follows from it). D5 → Task 2. D6 → Global Constraints (no kernel dep). D7 → Task 4, including the two files explicitly *not* to modify. D8 → Task 5. Testing section → Tasks 2, 3, 4 Steps 2/4, and 5 Step 1. Verification section → Task 6. Out-of-scope items correctly have no task.

**Placeholders.** None: every code step carries the literal code, every run step carries the exact command and expected output.

**Type consistency.** `creator()` / `modifier()` returning `Option<&Actor>` are defined in Task 2 and used under those exact names in Tasks 3 and 5. `actor(n) -> Actor` is defined once in Task 3 Step 1 and used by Step 2's macro table. `audit()`'s signature is unchanged, so Task 5 touches no call site. The Task 3 macro's added `$n` argument is introduced and consumed in the same step.

**One deliberate ordering property.** Task 1 leaves the tree uncompilable, which is unavoidable: the proto retype and the Rust/Py/TS fixes cannot land in one commit without also merging four independently reviewable changes. Each later task restores a language to green, and Task 5 Step 5 is the first whole-workspace build.
