# SMA-425 — `paigasus.common.v1.AuditMetadata` + per-language `Auditable` interface

**Status:** Design proposed · **Date:** 2026-06-21 · **Linear:** SMA-425 · Relates to SMA-388 / SMA-360 / SMA-389

## Problem / goal

Define a single source-of-truth shape for audit fields on auditable entities and DTOs —
`created_at`, `modified_at`, `created_by`, `modified_by` — once in proto (ADR-0004) and
fanned out by buf to Rust / Python / TypeScript, with a thin, idiomatic `Auditable`
contract layered on the generated type per language.

This lands the **first real `paigasus.common.v1` contract**. The package was scaffolded
empty in SMA-360 and its placeholder removed in SMA-389. `paigasus-proto` already has a
working end-to-end codegen path proven by `gateway/v1/health.proto` (prost+tonic /
betterproto2 / protobuf-es), so this issue adds files to an established pipeline rather
than building new machinery.

## Design decisions (resolved in the issue)

These were settled before this spec; restated here so the plan and the challenge have a
single durable source.

- **Defined in proto, not Rust.** Audit fields are a *data shape* crossing the language
  boundary, so the single definition lives in `contracts/` and buf fans it out (ADR-0004).
  The `paigasus-kernel` → FFI path (ADR-0005) is for *behavior*: PyO3/napi expose
  functions, and a Rust trait does not transit FFI as an "interface" in Py/TS — wrong tool.
- **Embedded message, not flat fields.** Carry the metadata by embedding `AuditMetadata`
  as a field (`AuditMetadata audit = N;`) rather than repeating four fields per message.
  Proto3 has no inheritance/mixins, so embedding is how "defined once" is achieved.
- **`google.protobuf.Timestamp`** for the two timestamps — a well-known type bundled with
  buf. It is **not** part of the existing `googleapis` buf dep, and needs no new buf dep or
  `buf.lock` change.
- **Opaque `string`** for `created_by` / `modified_by` (user id / subject claim / service
  name). Empty string = unknown/system, pending a future structured `Actor` type.
- **`_at` field naming.** Timestamp fields use the protobuf-idiomatic `created_at` /
  `modified_at`. Settled before regen — renaming after generated code lands is a breaking
  wire change.
- **"Interface" = thin per-language wrapper.** The generated message gives the shape in all
  three languages; the programmable `Auditable` contract is a few idiomatic lines per
  language (Rust `trait`, TS `interface`, Python `Protocol`) over the generated type — no
  logic reimplemented, consistent with ADR-0005.
- **Hand-written interface files live OUTSIDE `generated/`.** `buf.gen.yaml` sets
  `clean: true`, which wipes each `out:` dir on every regen. The trait/interface/Protocol
  files sit in each package's `src/` root, never under `generated/`.

## The proto

`contracts/proto/paigasus/common/v1/audit.proto`

```proto
// SPDX-License-Identifier: Apache-2.0
syntax = "proto3";

package paigasus.common.v1;

import "google/protobuf/timestamp.proto";

// Shared audit metadata for auditable entities and DTOs across all Paigasus
// services and languages. Carried by *embedding* this message as a field
// (`AuditMetadata audit = N;`) rather than repeating the four fields per
// message — the shape is defined once here. The per-language `Auditable`
// trait / interface / Protocol is layered on top of the generated type.
message AuditMetadata {
  // When the entity was first created (UTC).
  google.protobuf.Timestamp created_at = 1;

  // When the entity was last modified (UTC). Equals created_at on first write.
  google.protobuf.Timestamp modified_at = 2;

  // Opaque identifier of the actor that created the entity (user id / subject
  // claim / service name). Empty = unknown/system, pending a structured Actor.
  string created_by = 3;

  // Opaque identifier of the actor that last modified the entity.
  string modified_by = 4;
}
```

### Embedding-conformance fixture proto

To prove the *embedding* decision through the actual codegen pipeline (not just hand-written
test DTOs), a minimal fixture message lives in its own file:

`contracts/proto/paigasus/common/v1/auditable_example.proto`

```proto
// SPDX-License-Identifier: Apache-2.0
syntax = "proto3";

package paigasus.common.v1;

import "paigasus/common/v1/audit.proto";

// Conformance/codegen fixture: the canonical example of *embedding* AuditMetadata
// in a message (`AuditMetadata audit = N;`). It exists to prove the cross-language
// embedding path generates correctly and to back the per-language Auditable
// conformance tests against a *generated* type. Not a domain type — real auditable
// aggregates embed AuditMetadata exactly this way.
message AuditableExample {
  string id = 1;

  // Embedded shared audit metadata. Generated as Option<AuditMetadata> (Rust) /
  // AuditMetadata | None (Python) / optional AuditMetadata (TS).
  AuditMetadata audit = 2;
}
```

`AuditMetadata` is referenced unqualified (same package) with an explicit `import` of
`audit.proto`. The generated `AuditableExample` type is unavoidably public (all generated
types are, like `CheckResponse`); only its *trait impl* is test-only (Rust, below). This is
a deliberately small, permanent fixture in `common.v1` — the cost accepted at GATE 1 in
exchange for proving the embedding path end-to-end.

No *domain* consumer DTO (the `Widget audit = 15;` example in the issue is illustrative only)
is added — a real auditable aggregate remains a follow-up; `AuditableExample` is a fixture,
not a domain type.

`moon run contracts:lint` and `buf format --exit-code` must pass for **both** proto files;
buf STANDARD requires no comments (the COMMENTS category is not in STANDARD) but does enforce
the `v1` package version suffix and snake_case fields, both satisfied. `contracts:generate`
must be run so the generated artifacts are committed (ADR-0004). prost emits **one** file per
package (`paigasus.common.v1.rs`) containing **both** `AuditMetadata` and `AuditableExample`,
so the Rust include is unchanged; protobuf-es emits one file per proto file, so a second
`auditable_example_pb.ts` appears; betterproto2 puts both messages in `common/v1/__init__.py`.

## Per-language design

### Rust — `rs/crates/libs/paigasus-proto`

**Generated.** `contracts:generate` produces `src/generated/paigasus/common/v1/paigasus.common.v1.rs`
(prost). prost emits `::prost_types::Timestamp` for the two timestamp fields because
`compile_well_known_types` is set **only** on the tonic plugin, not prost (verified in
`buf.gen.yaml`) — so `paigasus-proto` gains a real `prost-types` dependency.

> **Service-less-proto verification (key risk).** `audit.proto` declares no `service`.
> The current `lib.rs` for `gateway::v1` `include!`s **both** the prost `.rs` and the
> tonic `.tonic.rs`. neoeinstein-tonic emits a `<package>.tonic.rs` only for packages that
> contain services, so `paigasus.common.v1.tonic.rs` is expected **not** to be generated.
> The `common::v1` module must therefore `include!` **only** the prost file. The plan's
> first generate step must confirm exactly which files land under
> `generated/paigasus/common/v1/` and wire `lib.rs` to match (include the prost file;
> include a tonic file only if one is actually emitted). Including a non-existent file is a
> compile error, so this is verified, not assumed.

**Hand-written** `src/audit.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0
use crate::paigasus::common::v1::AuditMetadata;

/// Implemented by any DTO/entity that carries [`AuditMetadata`].
pub trait Auditable {
    /// The embedded audit metadata, if present.
    fn audit(&self) -> Option<&AuditMetadata>;

    fn created_by(&self) -> Option<&str> {
        self.audit().map(|a| a.created_by.as_str())
    }
    fn modified_by(&self) -> Option<&str> {
        self.audit().map(|a| a.modified_by.as_str())
    }
    fn created_at(&self) -> Option<&::prost_types::Timestamp> {
        self.audit().and_then(|a| a.created_at.as_ref())
    }
    fn modified_at(&self) -> Option<&::prost_types::Timestamp> {
        self.audit().and_then(|a| a.modified_at.as_ref())
    }
}
```

`audit.rs` is **not** under the generated `#![allow(clippy::all, warnings)]` shield, so it
must be clippy-clean under the workspace `-D warnings` gate (it is: doc comment on the
trait, default methods only). Note `paigasus-proto`'s Moon `lint` is **not** wired to
`contracts:generate` — clippy is the workspace-wide `cargo clippy --all-targets` task. It
sees `common::v1` only because generated code is committed (ADR-0004), so the regen must be
committed before relying on a green clippy run.

**Deliberate accessor asymmetry (document in the trait + test).** `created_at()` /
`modified_at()` use `and_then` over an `Option<Timestamp>`, so they return `None` both when
no audit metadata is present *and* when the timestamp is unset. `created_by()` /
`modified_by()` use `map` over an always-present `String`, so they return `Some("")` when
audit is present but the actor is empty, vs `None` when audit is absent. This is intentional:
per the proto comment, empty string = unknown/system (a meaningful value), distinct from
"no audit metadata at all" (`None`). The test asserts both arms (audit present + empty actor
→ `Some("")`; `audit() == None` → all accessors `None`) so the asymmetry is pinned, not
accidental.

**Wiring** `src/lib.rs`: add a `paigasus::common::v1` module that includes the generated
prost file (mirroring the existing `gateway::v1` block, minus the tonic include per the
risk note), then `pub mod audit;` at crate root.

**Workspace deps** `rs/Cargo.toml`: add `prost-types = "0.14"` to
`[workspace.dependencies]` (tracks the pinned prost 0.14), and add `prost-types.workspace = true`
to the `paigasus-proto` crate `[dependencies]`.

**Test — orphan rule forces an in-crate impl.** The conformance test impls `Auditable` for
the **generated** `AuditableExample`, which proves the generated prost field is
`Option<AuditMetadata>` and that the trait works on a real generated embedded type. Rust's
orphan rule blocks `impl Auditable for AuditableExample` from an *integration* test in
`tests/` (neither the trait nor the type is local to that separate crate), so the impl + its
assertions go in a `#[cfg(test)] mod` **inside the lib** (e.g. in `src/audit.rs`), where both
are local. The impl is test-only (not shipped):

```rust
#[cfg(test)]
mod tests {
    use super::Auditable;
    use crate::paigasus::common::v1::{AuditMetadata, AuditableExample};

    impl Auditable for AuditableExample {
        fn audit(&self) -> Option<&AuditMetadata> {
            self.audit.as_ref()
        }
    }
    // assert: audit set with created_by="svc" → created_by() == Some("svc");
    // audit set but actor empty → Some(""); audit == None → all accessors None.
}
```

This `#[cfg(test)]` impl is still clippy-linted under `--all-targets -D warnings`, so it must
be clean. (Shipping a public reference impl was considered and rejected: it would make a
fixture type part of the crate's public API; keeping it test-only is faithful to the fixture's
purpose. The "manual impl is the first-consumer pattern" from the issue is still demonstrated —
just in test scope.)

### TypeScript — `ts/packages/paigasus-proto`

**Generated.** protobuf-es v2 produces `src/generated/paigasus/common/v1/audit_pb.ts`
exporting the `AuditMetadata` message type + `AuditMetadataSchema`; the four fields are
camelCased (`createdAt`, `modifiedAt`, `createdBy`, `modifiedBy`) and the timestamps are
`@bufbuild/protobuf/wkt` `Timestamp` messages.

**Hand-written** `src/audit.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import type { AuditMetadata } from './generated/paigasus/common/v1/audit_pb.js';

/** Structural interface satisfied by any generated message embedding AuditMetadata. */
export interface Auditable {
  audit?: AuditMetadata | undefined;
}
```

protobuf-es generates message-typed fields as optional and TS is structurally typed, so any
generated DTO with `audit?: AuditMetadata | undefined` satisfies `Auditable` for free.

> **`| undefined` is required, not redundant (verified at implementation).** protobuf-es v2
> types optional message fields as `audit?: AuditMetadata | undefined` (the `| undefined` is
> explicit in the generated `.d.ts`/type). Under `exactOptionalPropertyTypes: true`, a bare
> `audit?: AuditMetadata` means "absent, or exactly `AuditMetadata` when present" — which the
> generated `AuditMetadata | undefined` field is **not** assignable to (`tsc` TS2379). Mirroring
> protobuf-es's idiom (`| undefined`) makes the generated messages structurally satisfy
> `Auditable`. ESLint does not flag this as redundant.

**Public surface** `src/index.ts` (currently `export {};`): re-export the hand-written
`Auditable` interface and the generated `AuditMetadata` type + `AuditMetadataSchema` value,
so `@paigasus/proto` exposes a clean audit entry point:

```ts
export type { Auditable } from './audit.js';
export { AuditMetadataSchema } from './generated/paigasus/common/v1/audit_pb.js';
export type { AuditMetadata } from './generated/paigasus/common/v1/audit_pb.js';
```

**Test** `src/audit.test.ts` (vitest, mirroring `health.test.ts`). The real proof of
`Auditable` is the **compile-time** structural check, so make it load-bearing rather than a
no-op runtime assertion: define a typed identity helper `const asAuditable = (a: Auditable) => a;`
and pass it the **generated** `AuditableExample` —
`asAuditable(create(AuditableExampleSchema, { id: 'x', audit: create(AuditMetadataSchema, { createdBy: 'svc' }) }))`.
This proves the generated protobuf-es message structurally satisfies `Auditable` for free;
`tsc` rejects it if the generated `audit?: AuditMetadata` field or the interface shape is
wrong. Then a runtime assertion on a **set** value
(`expect(dto.audit?.createdBy).toBe('svc')`) so the body isn't reducible to optional-chaining
over `undefined`.

> **`exactOptionalPropertyTypes: true`** is set in `ts/tsconfig.base.json`. Under it,
> `{ audit: undefined }` is **not** assignable to `audit?: AuditMetadata` — the test must
> either omit the `audit` key or set it to a real `AuditMetadata`, never `undefined`. A
> separate `const empty: Auditable = {};` line proves the field is genuinely optional.

`tsc --noEmit` must stay clean. `moon run ts:fmt` (the whole-tree Prettier gate) must be run
after editing the **hand-written** TS files (`audit.ts`, `index.ts`, `audit.test.ts`); the
generated `audit_pb.ts` is exempt — `ts/.prettierignore` ignores `generated`.

### Python — `py/packages/paigasus-proto`

**Generated.** betterproto2 produces `src/paigasus_proto/generated/paigasus/common/v1/__init__.py`
exporting `AuditMetadata` and `AuditableExample` (plus regenerated intermediate
`paigasus/__init__.py`, `common/__init__.py`, `v1/__init__.py` — all under the `clean: true`
`generated/` dir). Because `audit.proto` imports `google.protobuf.Timestamp`, betterproto2 also
emits a **new** `generated/google/protobuf/__init__.py` WKT tree (it inlines the `Timestamp`
type rather than depending on a runtime stub) — this is new committed output. That generated
WKT code does `import dateutil.parser`; `python-dateutil` is already a **runtime** dependency of
`betterproto2` (verified in `uv.lock`), so `paigasus-proto` gets it transitively — **no
pyproject change is needed**, even for a published wheel.

> **Timestamp leaf-type divergence (verified).** betterproto2 maps
> `google.protobuf.Timestamp` to a wrapped `datetime.datetime` (and `Duration` →
> `timedelta`), so the generated `AuditMetadata.created_at` is typed `datetime | None` — a
> different *leaf* type from Rust's `prost_types::Timestamp` and TS's `@bufbuild/protobuf/wkt`
> `Timestamp`. The *embedded field* type is consistent across languages (`AuditMetadata`);
> only the timestamp leaf differs. This is documented in ADR-0012's Consequences so no one
> reading it assumes symmetric `Timestamp` types.

**Hand-written** `src/paigasus_proto/audit.py`. ruff's flake8-type-checking rules **are**
enabled (`"TCH"` in `py/pyproject.toml`), and `AuditMetadata` is used only inside the quoted
annotation, so the import goes under `if TYPE_CHECKING:` — deterministic, not a "follow what
lint says":

```python
# SPDX-License-Identifier: Apache-2.0
from typing import TYPE_CHECKING, Protocol, runtime_checkable

if TYPE_CHECKING:
    from .generated.paigasus.common.v1 import AuditMetadata


@runtime_checkable
class Auditable(Protocol):
    audit: "AuditMetadata | None"
```

A `@runtime_checkable` Protocol with a data member supports `isinstance` on Python ≥ 3.12
(the package's floor) — it checks for the *presence* of an `audit` attribute, not its type,
and never evaluates the (string) annotation at runtime, so the `TYPE_CHECKING`-only import is
safe. basedpyright excludes `**/generated/**` from *checking* but still resolves symbols from
it — verify `audit.py` raises no `reportMissingImports` and that the SMA-436 coverage-floor
guard still counts the file.

**Test** `tests/test_audit_protocol.py` (mirroring `test_health_smoke.py`): the test imports
the **generated** `AuditableExample` and `AuditMetadata` as **normal runtime imports** (it
instantiates them). Construct `AuditableExample(id="x", audit=AuditMetadata(created_by="svc",
created_at=<datetime>))` (timestamp as a `datetime`, per the divergence note) and assert
`isinstance(obj, Auditable)` is `True` — proving the generated betterproto2 message satisfies
the Protocol. Assert `AuditableExample(id="y")` (audit defaulting to `None`) is also an
instance (the `audit` attribute is present). Assert a bare class lacking `audit` entirely is
**not** an instance (negative case — this is what makes the test non-vacuous). basedpyright
must stay clean.

**Public surface:** keep `paigasus_proto/__init__.py` minimal — consumers import the Protocol
via `from paigasus_proto.audit import Auditable` and the runtime `AuditMetadata` **value** via
its full generated path (`from paigasus_proto.generated.paigasus.common.v1 import AuditMetadata`),
matching how gateway types are accessed today. No top-level re-export; asymmetric with TS by
deliberate convention (Python consumers import from the submodule / generated path; the TS
index re-export is the idiomatic JS package entry). This asymmetry is intentional and called
out so it doesn't read as an oversight.

## ADR-0012 (decision: write a new ADR)

Per the AC's last item, a new Notion ADR will be authored — **ADR-0012** (0001–0011 exist;
next free sequential number) — as a sub-page under *Development → Architecture Decision
Records*, in the house MADR style (Status / Date / Deciders / Context / Decision / Rationale
/ Consequences / Alternatives considered / References). It qualifies as an ADR (not a mere
guideline) by the index's own test — both core choices had viable alternatives.

Working title: **"ADR-0012: Cross-language shared data shapes via embedded proto messages +
thin per-language interfaces."**

- **Context.** Some shapes (audit metadata, and future common types) must be identical
  across Rust/Py/TS. ADR-0004 makes proto the source of truth for wire types; ADR-0005 puts
  *behavior* in the kernel behind FFI. A shared *data shape* with an ergonomic programmable
  contract falls between them and needs a stated pattern.
- **Decision.** (1) Shared data shapes that cross the language boundary are defined once as
  a proto message and reused by **embedding** that message as a field (proto3 has no
  inheritance). (2) The ergonomic cross-language "interface" over such a shape is a **thin,
  hand-written per-language wrapper** (Rust trait / TS interface / Python Protocol) layered
  on the generated type, kept outside the `clean: true` `generated/` dirs.
- **Consequences.** "Defined once" holds at the *message* level: the embedded field type is
  `AuditMetadata` in all three languages. Leaf well-known types are **not** uniform — each
  generator idiomatizes them (prost → `prost_types::Timestamp`, protobuf-es → `wkt.Timestamp`,
  betterproto2 → `datetime.datetime`). The cross-language guarantee is the shared *shape*
  (which fields exist, what they mean), not a shared timestamp representation. The thin
  wrappers carry no logic, so they cannot drift — they only re-surface the generated type.
- **Alternatives considered.** Flat repeated fields per message (rejected: the duplication
  ADR-0003/0004 exist to prevent). A Rust trait in `paigasus-kernel` exposed via FFI
  (rejected: FFI exposes *functions*, not interfaces; a Rust trait does not transit PyO3/napi
  as a Py `Protocol` / TS `interface` — ADR-0005 is for behavior, not data shapes).
- **References.** ADR-0004, ADR-0005, this issue (SMA-425), Polyglot Monorepo Scoping § 2.

**Sequencing.** Per CLAUDE.md ("significant choices get a Notion ADR before code"), the
ADR-0012 page is created **after GATE 1 approval and before the implementation code lands**
(first task of Stage 4), then linked on SMA-425. It is created post-GATE-1 so a design
change at the gate doesn't strand a published ADR. **Re-fetch the ADR index immediately
before creating the page** to confirm `0012` is still the next free number (no local source
of truth for ADR sequence — the Notion index is authoritative; another in-flight issue could
have claimed it). Update the title's number if so.

## Risks & verification items (carried into the plan)

1. **Rust `lib.rs` tonic include** for the service-less package — confirm generate output,
   include only what exists (see Rust section). *Highest-confidence trap.*
2. **betterproto2 `Timestamp` → `datetime` (verified)** — confirm `AuditMetadata` generates,
   the import resolves under basedpyright, and the timestamp leaf is `datetime` (see the
   Python divergence note + ADR-0012 Consequences). The Protocol is unaffected (it checks
   `audit` presence only), but the test constructs with a `datetime`.
3. **ruff TYPE_CHECKING import** for `audit.py` — deterministic (`TCH` is enabled); see Python
   section.
4. **Prettier `ts:fmt`** whole-tree gate must be run after TS edits.
5. **`buf format`** canonicalization — run it; commit the canonical form.
6. **Why no Moon wiring change is needed (corrected).** The `contracts:generate` dep edges
   are **not** uniform across the three packages, and the checks do **not** all depend on
   regen:
   - `py/.../paigasus-proto/moon.yml` wires only `build` to `contracts:generate`; `py:test`
     and `py:typecheck` are whole-tree tasks at the py config root (`.moon/tasks/python.yml`)
     with no such dep.
   - `rs/.../paigasus-proto/moon.yml` wires `build` and `test` but **not** `lint`; clippy is
     the workspace-wide `cargo clippy` (`.moon/tasks/rust.yml`), with no regen dep — yet the
     AC requires `clippy --workspace -- -D warnings` clean.
   - only `paigasus-proto-ts` wires `build` + `typecheck` + `test`.

   The reason adding protos is still safe is **not** a dependency edge — it's that generated
   code is **committed** (ADR-0004), so clippy / basedpyright / vitest read the committed
   `common/v1` artifacts from git. The invariant is therefore *commit the regen output*, not
   *rely on regen ordering*. The implementation must run `contracts:generate`, commit its
   output, then run the checks. Verify a clean `moon ci :build`/`:test` graph locally; do
   **not** add new dep edges (it would diverge from how the committed `gateway/v1` types are
   already consumed without them).
7. **Regen idempotency / `clean: true` blast radius.** `contracts:generate` writes into
   sibling project dirs (`../rs`, `../py`, `../ts`) and `clean: true` wipes each `out:` dir.
   Run the regen + `git status` check **from a clean working tree** so an unrelated dirty file
   can't mask a problem, and confirm `clean` only touches `generated/` dirs (never the
   hand-written `audit.rs`/`audit.ts`/`audit.py` or other siblings).

## Out of scope / follow-ups

- A `derive`/macro to auto-impl the Rust `Auditable` trait (manual impls fine for now).
- A structured `Actor` message replacing the opaque `created_by` / `modified_by` strings.
- Server-side stamping (who/when set on write) — service-layer behavior, tracked when the
  first auditable aggregate lands.
- **A real *domain* auditable aggregate.** `AuditableExample` proves the embedding *codegen*
  path (decided at GATE 1, see the fixture section), but it is a fixture, not a domain type. A
  real auditable aggregate (with server-side stamping, persistence, etc.) is a follow-up when
  the first such aggregate lands.

## Acceptance criteria

- [ ] `contracts/proto/paigasus/common/v1/audit.proto` (AuditMetadata) **and**
  `auditable_example.proto` (AuditableExample embedding fixture) added; `moon run
  contracts:lint` and `buf format --exit-code` pass for both.
- [ ] `moon run contracts:generate` produces `AuditMetadata` **and** `AuditableExample` in all
  three targets; generated code committed (ADR-0004).
- [ ] Rust: `audit.rs` trait + `lib.rs` wiring for `common::v1` (prost include; tonic only
  if emitted); `prost-types` added (workspace dep + crate dep); `cargo build --workspace`
  and `cargo clippy --workspace -- -D warnings` clean; a `#[cfg(test)]` in-crate impl of
  `Auditable` on the **generated** `AuditableExample` asserts the accessors (set actor →
  `Some(...)`; empty actor → `Some("")`; `audit() == None` → all `None`).
- [ ] TS: `audit.ts` interface + `index.ts` re-export; `tsc --noEmit` clean; `ts:fmt` clean;
  a vitest test proves the **generated** `AuditableExample` structurally satisfies `Auditable`
  via a compile-time typed helper (not a no-op runtime check), plus a set-value runtime assert.
- [ ] Python: `audit.py` Protocol (import under `TYPE_CHECKING`); basedpyright clean; a test
  asserts the **generated** `AuditableExample` satisfies `Auditable` (`isinstance` via
  `@runtime_checkable`), an `audit=None` instance, and a negative case.
- [ ] Hand-written interface files confirmed to survive a `clean: true` regen (they live
  outside `generated/`) — verified by a regen + `git status` from a clean tree showing them
  untouched.
- [ ] ADR-0012 authored in Notion (post-GATE-1, pre-code; number re-confirmed at creation)
  and linked on SMA-425.

## Files touched

- `contracts/proto/paigasus/common/v1/audit.proto` — new proto (`AuditMetadata`).
- `contracts/proto/paigasus/common/v1/auditable_example.proto` — new embedding fixture
  (`AuditableExample`). Both add committed generated output under each package's `generated/` dir.
- `rs/crates/libs/paigasus-proto/src/audit.rs` — new `Auditable` trait + `#[cfg(test)]`
  conformance impl on the generated `AuditableExample`.
- `rs/crates/libs/paigasus-proto/src/lib.rs` — `common::v1` module + `pub mod audit;`.
- `rs/crates/libs/paigasus-proto/Cargo.toml` — `prost-types` dep.
- `rs/Cargo.toml` — `prost-types` workspace dep.
- `ts/packages/paigasus-proto/src/audit.ts` — new `Auditable` interface.
- `ts/packages/paigasus-proto/src/index.ts` — re-exports.
- `ts/packages/paigasus-proto/src/audit.test.ts` — new test (against generated `AuditableExample`).
- `py/packages/paigasus-proto/src/paigasus_proto/audit.py` — new `Auditable` Protocol.
- `py/packages/paigasus-proto/tests/test_audit_protocol.py` — new test (against generated
  `AuditableExample`).
- Notion: ADR-0012 page (external).
