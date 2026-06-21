# SMA-425 — AuditMetadata + per-language Auditable interface — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Define `paigasus.common.v1.AuditMetadata` once in proto, fan it out to Rust/Python/TS via buf, and layer a thin `Auditable` contract per language (Rust trait / TS interface / Python Protocol), proven against a generated `AuditableExample` embedding fixture.

**Architecture:** Proto is the single source of truth (ADR-0004); generated code is committed. The hand-written `Auditable` wrappers live *outside* the `generated/` dirs (which `buf.gen.yaml` wipes via `clean: true`). A small `AuditableExample` fixture message embeds `AuditMetadata audit = 2;` so each language's conformance test runs against a *generated* type, proving the cross-language embedding codegen path end-to-end.

**Tech Stack:** buf v2 + remote/local plugins (neoeinstein-prost/tonic 0.5.0, betterproto2, protobuf-es 2.12), Moon 2.3.2, Rust edition 2024 (prost/prost-types/tonic 0.14, cargo-nextest), pnpm + vitest + tsc, uv + basedpyright + ruff + pytest.

**Spec:** `docs/superpowers/specs/2026-06-21-sma-425-audit-metadata-auditable-design.md`

## Global Constraints

Every task implicitly includes these (exact values from the spec / CLAUDE.md):

- **SPDX header on every source file:** `// SPDX-License-Identifier: Apache-2.0` (`#` for Python, `//` for Rust/TS/proto). First line.
- **proto-managed tools are OFF the Bash PATH.** Prefix every `moon`/`buf`/`uv` invocation (shell state does not persist between Bash calls) with: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` — **shims FIRST** (repo-pinned versions), then `bin` (global pins). Wrong order runs the wrong tool version.
- **Generated code is committed** (ADR-0004). Run `contracts:generate`, then commit its output; the checks (clippy / basedpyright / vitest) read committed artifacts — they do **not** all depend on the regen task. Never add new `contracts:generate` dep edges.
- **Hand-written interface files live OUTSIDE `generated/`.** `buf.gen.yaml` has `clean: true`.
- **Rust:** edition 2024 + rust-version 1.95; `prost`/`prost-types`/`tonic` pinned `0.14`; clippy gate is `cargo clippy --workspace -- -D warnings` (warnings denied); test runner is `cargo nextest` (`--no-tests=pass` on empty crates).
- **TypeScript:** `exactOptionalPropertyTypes: true` (no `audit: undefined`); `ts:fmt` is a whole-tree Prettier gate — run it after editing the **hand-written** TS files only (`generated/` is prettier-ignored).
- **Python:** `>=3.12`; ruff `TCH` is enabled (type-only imports go under `if TYPE_CHECKING:`); basedpyright `typeCheckingMode = "all"`; basedpyright excludes `**/generated/**` from checking but still resolves symbols from it.
- **buf:** STANDARD lint + `buf format --exit-code` must pass; `PACKAGE_DIRECTORY_MATCH` is excepted; `google/protobuf/timestamp.proto` is a bundled WKT needing no buf dep / `buf.lock` change.
- **Commits:** Conventional Commits, **scope required, subject lowercase** (commitlint `scope-empty: never`, `subject-case` forbids leading uppercase). Allowed scopes include `contracts`, `rs`, `ts`, `py`. Commits are **SSH-signed via 1Password**; if a commit fails with `1Password: failed to fill whole buffer`, the vault is locked — ask the user to unlock, then retry. End commit messages with the `Co-Authored-By` trailer (blank line before it).
- **Branch:** `feature/sma-425-add-paigasuscommonv1auditmetadata-per-language-auditable` (already created).

## File Structure

- `contracts/proto/paigasus/common/v1/audit.proto` — `AuditMetadata` message (the shared shape).
- `contracts/proto/paigasus/common/v1/auditable_example.proto` — `AuditableExample` embedding fixture.
- `rs/crates/libs/paigasus-proto/src/audit.rs` — `Auditable` trait + `#[cfg(test)]` conformance impl.
- `rs/crates/libs/paigasus-proto/src/lib.rs` — add `paigasus::common::v1` module + `pub mod audit;`.
- `rs/crates/libs/paigasus-proto/Cargo.toml` + `rs/Cargo.toml` — `prost-types` dep.
- `ts/packages/paigasus-proto/src/audit.ts` — `Auditable` interface.
- `ts/packages/paigasus-proto/src/index.ts` — re-exports.
- `ts/packages/paigasus-proto/src/audit.test.ts` — TS conformance test.
- `py/packages/paigasus-proto/src/paigasus_proto/audit.py` — `Auditable` Protocol.
- `py/packages/paigasus-proto/tests/test_audit_protocol.py` — Python conformance test.
- All `generated/` artifacts under each package (regenerated + committed).

**Task dependency:** Task 0 (ADR) and Task 1 (proto+generate) come first. Tasks 2/3/4 each depend only on Task 1 and are independent of one another (parallelizable).

---

## Task 0: Author ADR-0012 in Notion (orchestrator, pre-code)

**Not a code task** — performed by the orchestrator (has Notion access + house-style context), not a code subagent. No TDD. Per CLAUDE.md, the ADR lands *before* implementation code.

- [ ] **Step 1: Re-confirm the next free ADR number.** Fetch the Notion "Architecture Decision Records" index page (`368830e8-fbaa-816c-b411-c7ee1682c175`). Confirm `0012` is still the next free sequential number (0001–0011 currently exist). If another in-flight issue claimed it, use the next free number and update the title everywhere.

- [ ] **Step 2: Create the ADR sub-page** under the ADR index, MADR house style (match ADR-0004's structure: **Status / Date / Deciders / Context / Decision / Rationale / Consequences / Alternatives considered / References**):
  - **Title:** `ADR-0012: Cross-language shared data shapes via embedded proto messages + thin per-language interfaces`
  - **Status:** Accepted · **Date:** 2026-06-21 · **Deciders:** Sven
  - **Context / Decision / Consequences / Alternatives:** copy from the spec's "ADR-0012" section (the embed-vs-flat + thin-wrapper decision; the leaf-WKT divergence consequence: prost→`prost_types::Timestamp`, protobuf-es→`wkt.Timestamp`, betterproto2→`datetime.datetime`; alternatives = flat fields / kernel-FFI trait, both rejected).
  - **References:** ADR-0004, ADR-0005, SMA-425, Polyglot Monorepo Scoping § 2.

- [ ] **Step 3: Add the row to the ADR index table** (`| 0012 | <mention-page> | Accepted | 2026-06-21 |`) and link the new page in the index's page list, matching the existing rows.

- [ ] **Step 4: Link the ADR on SMA-425.** Add the ADR URL to the issue (resolves the AC's last checkbox). Do **not** attach a GitHub PR link to Linear later — the integration auto-links by branch name.

---

## Task 1: Add the proto + generate + commit

**Files:**
- Create: `contracts/proto/paigasus/common/v1/audit.proto`
- Create: `contracts/proto/paigasus/common/v1/auditable_example.proto`
- Generated (committed, do not hand-edit): `rs/crates/libs/paigasus-proto/src/generated/paigasus/common/v1/**`, `ts/packages/paigasus-proto/src/generated/paigasus/common/v1/**`, `py/packages/paigasus-proto/src/paigasus_proto/generated/paigasus/common/**`

**Interfaces:**
- Produces (consumed by Tasks 2/3/4):
  - Rust: `paigasus_proto::paigasus::common::v1::{AuditMetadata, AuditableExample}` — prost structs; `AuditMetadata { created_at: Option<::prost_types::Timestamp>, modified_at: Option<...>, created_by: String, modified_by: String }`; `AuditableExample { id: String, audit: Option<AuditMetadata> }`.
  - TS: `@paigasus/proto` generated `audit_pb.ts` (`AuditMetadata`, `AuditMetadataSchema`) + `auditable_example_pb.ts` (`AuditableExample`, `AuditableExampleSchema`); fields camelCased (`createdAt`, `modifiedAt`, `createdBy`, `modifiedBy`, `audit`).
  - Python: `paigasus_proto.generated.paigasus.common.v1.{AuditMetadata, AuditableExample}` (betterproto2 dataclasses; `audit: "AuditMetadata | None"`, timestamps as `datetime`).

- [ ] **Step 1: Write `audit.proto`**

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

- [ ] **Step 2: Write `auditable_example.proto`**

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

- [ ] **Step 3: Format + lint the protos (expect PASS)**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/contracts
buf format -w        # canonicalize in place
buf format --exit-code   # now a no-op; must exit 0
moon run contracts:lint
```
Expected: `buf format --exit-code` exits 0; `contracts:lint` passes (STANDARD: PascalCase messages, snake_case fields, `v1` suffix all satisfied; no comment requirement).

- [ ] **Step 4: Generate (expect new committed artifacts)**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core
moon run contracts:generate
```

- [ ] **Step 5: Verify the generated file set (the service-less-proto trap)**

Run:
```bash
ls rs/crates/libs/paigasus-proto/src/generated/paigasus/common/v1/
ls ts/packages/paigasus-proto/src/generated/paigasus/common/v1/
ls py/packages/paigasus-proto/src/paigasus_proto/generated/paigasus/common/v1/
```
Expected:
- Rust: exactly **one** `paigasus.common.v1.rs` (prost), containing both `AuditMetadata` and `AuditableExample`. **No `paigasus.common.v1.tonic.rs`** (no service). If a `.tonic.rs` *is* emitted, open it — only `include!` it in Task 2 if it is non-empty/needed.
- TS: `audit_pb.ts` **and** `auditable_example_pb.ts`.
- Python: `__init__.py` exporting `AuditMetadata` and `AuditableExample` (+ regenerated intermediate `paigasus/__init__.py`, `common/__init__.py`, `v1/__init__.py`).

Confirm the symbols exist:
```bash
grep -l 'AuditableExample' rs/crates/libs/paigasus-proto/src/generated/paigasus/common/v1/*.rs
grep -rl 'AuditableExample' ts/packages/paigasus-proto/src/generated/paigasus/common/v1/
grep -l 'AuditableExample' py/packages/paigasus-proto/src/paigasus_proto/generated/paigasus/common/v1/__init__.py
```

- [ ] **Step 6: Confirm no hand-written files were touched / no stray writes (clean-tree check)**

Run: `git status --porcelain`
Expected: only **new** files under the three `generated/` dirs and the two new `.proto` files. No modifications to unrelated files. (`clean: true` should only touch `generated/` dirs.)

- [ ] **Step 7: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core
git add contracts/proto/paigasus/common/v1/audit.proto \
        contracts/proto/paigasus/common/v1/auditable_example.proto \
        rs/crates/libs/paigasus-proto/src/generated \
        ts/packages/paigasus-proto/src/generated \
        py/packages/paigasus-proto/src/paigasus_proto/generated
git commit -m "$(cat <<'EOF'
feat(contracts): add common.v1 AuditMetadata + AuditableExample fixture (SMA-425)

First real paigasus.common.v1 contract: AuditMetadata (created_at/modified_at
as google.protobuf.Timestamp; created_by/modified_by as opaque strings) plus a
minimal AuditableExample fixture embedding it, to prove the cross-language
embedding codegen path. Generated code committed for Rust/Python/TS (ADR-0004).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```
Expected: commitlint passes; commit succeeds (if `1Password: failed to fill whole buffer`, unlock the vault and retry).

---

## Task 2: Rust — `Auditable` trait + wiring + conformance test

**Files:**
- Modify: `rs/Cargo.toml` (`[workspace.dependencies]`)
- Modify: `rs/crates/libs/paigasus-proto/Cargo.toml` (`[dependencies]`)
- Modify: `rs/crates/libs/paigasus-proto/src/lib.rs`
- Create: `rs/crates/libs/paigasus-proto/src/audit.rs`

**Interfaces:**
- Consumes: `crate::paigasus::common::v1::{AuditMetadata, AuditableExample}` (Task 1).
- Produces: `paigasus_proto::audit::Auditable` trait — `fn audit(&self) -> Option<&AuditMetadata>` (required) + default methods `created_by()/modified_by() -> Option<&str>`, `created_at()/modified_at() -> Option<&::prost_types::Timestamp>`.

- [ ] **Step 1: Add `prost-types` to the workspace dependency table**

In `rs/Cargo.toml`, in `[workspace.dependencies]`, next to the existing `prost`/`tonic` lines, add:
```toml
prost-types = "0.14"
```

- [ ] **Step 2: Add the crate dependency**

In `rs/crates/libs/paigasus-proto/Cargo.toml`, under `[dependencies]` (with `prost`/`tonic`/`tonic-prost`):
```toml
prost-types.workspace = true
```

- [ ] **Step 3: Wire `lib.rs`** — add the `common::v1` module (prost include only) and the `audit` module. Insert a sibling `common` module next to `gateway` inside `pub mod paigasus`, and add `pub mod audit;` at the crate root:

```rust
pub mod common {
    pub mod v1 {
        // Generated code is excluded from the strict lint gate.
        #![allow(clippy::all, warnings)]
        include!("generated/paigasus/common/v1/paigasus.common.v1.rs");
        // NOTE: no `.tonic.rs` include — audit.proto declares no service, so
        // neoeinstein-tonic emits no tonic file for this package (verified in
        // Task 1, Step 5). Only add a tonic include if Task 1 actually produced one.
    }
}
```
and after the `pub mod paigasus { … }` block:
```rust
pub mod audit;
```

- [ ] **Step 4: Write the failing conformance test first** — create `src/audit.rs` with ONLY the test module (trait not yet defined), so the failure is meaningful:

```rust
// SPDX-License-Identifier: Apache-2.0

#[cfg(test)]
mod tests {
    use super::Auditable;
    use crate::paigasus::common::v1::{AuditMetadata, AuditableExample};

    impl Auditable for AuditableExample {
        fn audit(&self) -> Option<&AuditMetadata> {
            self.audit.as_ref()
        }
    }

    #[test]
    fn accessors_read_through_embedded_metadata() {
        let dto = AuditableExample {
            id: "x".to_string(),
            audit: Some(AuditMetadata {
                created_by: "svc".to_string(),
                ..Default::default()
            }),
        };
        assert_eq!(dto.created_by(), Some("svc"));
        // Empty actor is a meaningful value (system), distinct from absent audit.
        assert_eq!(dto.modified_by(), Some(""));
    }

    #[test]
    fn absent_audit_yields_none_accessors() {
        let dto = AuditableExample { id: "y".to_string(), audit: None };
        assert_eq!(dto.audit(), None);
        assert_eq!(dto.created_by(), None);
        assert_eq!(dto.created_at(), None);
    }
}
```

- [ ] **Step 5: Run the test — expect COMPILE FAILURE**

Run:
```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/rs
cargo nextest run -p paigasus-proto --no-tests=pass
```
Expected: FAIL — `cannot find trait Auditable in this scope` (and unresolved default methods). This proves the test exercises the not-yet-written trait.

- [ ] **Step 6: Add the trait** — prepend to `src/audit.rs` (above the `#[cfg(test)]` module):

```rust
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
(The `use` line sits at the top of the file, right after the SPDX header.)

- [ ] **Step 7: Run the test — expect PASS**

Run:
```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/rs
cargo nextest run -p paigasus-proto --no-tests=pass
```
Expected: PASS (3 tests: the 2 new + `health_smoke`).

- [ ] **Step 8: Build + clippy gates (expect clean)**

Run:
```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/rs
cargo build --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```
Expected: all clean. (`audit.rs` is outside the generated `#![allow]` shield, so it must pass clippy; the `#[cfg(test)]` impl is linted under `--all-targets`.) If `cargo fmt --check` flags `audit.rs`, run `cargo fmt` and re-verify.

- [ ] **Step 9: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core
git add rs/Cargo.toml rs/Cargo.lock rs/crates/libs/paigasus-proto/Cargo.toml \
        rs/crates/libs/paigasus-proto/src/lib.rs rs/crates/libs/paigasus-proto/src/audit.rs
git commit -m "$(cat <<'EOF'
feat(rs): add Auditable trait over generated AuditMetadata (SMA-425)

Thin trait in paigasus-proto layered on the generated common.v1 types, with a
cfg(test) conformance impl on the generated AuditableExample fixture. Wires the
common::v1 prost module into lib.rs (no tonic include — service-less package)
and adds the prost-types dependency for google.protobuf.Timestamp.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```
Expected: commit succeeds (unlock 1Password if needed).

---

## Task 3: TypeScript — `Auditable` interface + re-exports + test

**Files:**
- Create: `ts/packages/paigasus-proto/src/audit.ts`
- Modify: `ts/packages/paigasus-proto/src/index.ts`
- Create: `ts/packages/paigasus-proto/src/audit.test.ts`

**Interfaces:**
- Consumes: generated `AuditMetadata`/`AuditMetadataSchema` (`audit_pb.ts`), `AuditableExample`/`AuditableExampleSchema` (`auditable_example_pb.ts`) (Task 1).
- Produces: `export interface Auditable { audit?: AuditMetadata }` from `@paigasus/proto`.

- [ ] **Step 1: Write the failing test first** — create `src/audit.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { create } from '@bufbuild/protobuf';
import { describe, expect, it } from 'vitest';
import type { Auditable } from './audit.js';
import { AuditMetadataSchema } from './generated/paigasus/common/v1/audit_pb.js';
import { AuditableExampleSchema } from './generated/paigasus/common/v1/auditable_example_pb.js';

// Compile-time identity helper: tsc rejects the call below if the generated
// AuditableExample does not structurally satisfy Auditable.
const asAuditable = (a: Auditable): Auditable => a;

describe('Auditable', () => {
  it('the generated AuditableExample structurally satisfies Auditable', () => {
    const dto = asAuditable(
      create(AuditableExampleSchema, {
        id: 'x',
        audit: create(AuditMetadataSchema, { createdBy: 'svc' }),
      }),
    );
    expect(dto.audit?.createdBy).toBe('svc');
  });

  it('audit is optional', () => {
    const empty: Auditable = {};
    expect(empty.audit).toBeUndefined();
  });
});
```

- [ ] **Step 2: Run the test — expect FAILURE**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core
moon run paigasus-proto-ts:test
```
Expected: FAIL — cannot resolve `./audit.js` (module not found).

- [ ] **Step 3: Write the interface** — create `src/audit.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import type { AuditMetadata } from './generated/paigasus/common/v1/audit_pb.js';

/** Structural interface satisfied by any generated message embedding AuditMetadata. */
export interface Auditable {
  // `| undefined` is required (not redundant) under exactOptionalPropertyTypes:
  // protobuf-es types optional message fields as `AuditMetadata | undefined`, and a
  // bare `audit?: AuditMetadata` would make the generated messages non-assignable (TS2379).
  audit?: AuditMetadata | undefined;
}
```

- [ ] **Step 4: Update `index.ts` re-exports** — replace `export {};` with:

```ts
// SPDX-License-Identifier: Apache-2.0

export type { Auditable } from './audit.js';
export { AuditMetadataSchema } from './generated/paigasus/common/v1/audit_pb.js';
export type { AuditMetadata } from './generated/paigasus/common/v1/audit_pb.js';
```

- [ ] **Step 5: Run the test — expect PASS**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core
moon run paigasus-proto-ts:test
```
Expected: PASS (the 2 new tests + `health` test).

- [ ] **Step 6: Typecheck + format gates (expect clean)**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core
moon run paigasus-proto-ts:typecheck
moon run ts:fmt
```
Expected: `typecheck` clean (`tsc --noEmit`; under `exactOptionalPropertyTypes` the `{}` literal is valid for `audit?`). `ts:fmt` clean — if it rewrites `audit.ts`/`index.ts`/`audit.test.ts`, re-stage them. (`generated/` is prettier-ignored.)

- [ ] **Step 7: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core
git add ts/packages/paigasus-proto/src/audit.ts \
        ts/packages/paigasus-proto/src/index.ts \
        ts/packages/paigasus-proto/src/audit.test.ts
git commit -m "$(cat <<'EOF'
feat(ts): add Auditable interface over generated AuditMetadata (SMA-425)

Structural Auditable interface in @paigasus/proto, re-exported from index, with
a vitest test proving the generated AuditableExample satisfies it via a
compile-time typed helper (not a no-op runtime check).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Python — `Auditable` Protocol + test

**Files:**
- Create: `py/packages/paigasus-proto/src/paigasus_proto/audit.py`
- Create: `py/packages/paigasus-proto/tests/test_audit_protocol.py`

**Interfaces:**
- Consumes: `paigasus_proto.generated.paigasus.common.v1.{AuditMetadata, AuditableExample}` (Task 1).
- Produces: `paigasus_proto.audit.Auditable` — a `@runtime_checkable` `Protocol` with `audit: "AuditMetadata | None"`.

- [ ] **Step 1: Write the failing test first** — create `tests/test_audit_protocol.py`:

```python
# SPDX-License-Identifier: Apache-2.0
from datetime import datetime, timezone

from paigasus_proto.audit import Auditable
from paigasus_proto.generated.paigasus.common.v1 import AuditMetadata, AuditableExample


def test_generated_example_satisfies_auditable() -> None:
    obj = AuditableExample(
        id="x",
        audit=AuditMetadata(
            created_by="svc",
            created_at=datetime(2026, 1, 1, tzinfo=timezone.utc),
        ),
    )
    assert isinstance(obj, Auditable)


def test_example_with_no_audit_still_satisfies() -> None:
    # The `audit` attribute is present (defaults to None) → still structural-match.
    assert isinstance(AuditableExample(id="y"), Auditable)


def test_object_without_audit_is_not_auditable() -> None:
    class NotAuditable:
        id: str = "z"

    assert not isinstance(NotAuditable(), Auditable)
```

- [ ] **Step 2: Run the test — expect FAILURE**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core
moon run py:test
```
Expected: FAIL — `ModuleNotFoundError: No module named 'paigasus_proto.audit'`.

- [ ] **Step 3: Write the Protocol** — create `src/paigasus_proto/audit.py`:

```python
# SPDX-License-Identifier: Apache-2.0
from typing import TYPE_CHECKING, Protocol, runtime_checkable

if TYPE_CHECKING:
    from .generated.paigasus.common.v1 import AuditMetadata


@runtime_checkable
class Auditable(Protocol):
    audit: "AuditMetadata | None"
```

- [ ] **Step 4: Run the test — expect PASS**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core
moon run py:test
```
Expected: PASS (3 new tests + `test_health_smoke`).

> If `isinstance` against the Protocol raises or misbehaves, confirm Python ≥ 3.12 is in use and that `AuditableExample` exposes `audit` as an attribute (betterproto2 generates it as a dataclass field defaulting to `None`). The negative test must rely on the *absence* of an `audit` attribute.

- [ ] **Step 5: Lint + typecheck gates (expect clean)**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core
moon run py:lint
moon run py:typecheck
```
Expected: clean. ruff `TCH` is satisfied (the `AuditMetadata` import is under `TYPE_CHECKING`; the *test* keeps a runtime import because it instantiates the type). basedpyright resolves `audit.py`'s import from the excluded `generated/` dir without `reportMissingImports`, and the SMA-436 coverage floor still counts `audit.py`. If ruff/basedpyright flags anything, fix per their guidance — the structural Protocol behaves identically.

- [ ] **Step 6: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core
git add py/packages/paigasus-proto/src/paigasus_proto/audit.py \
        py/packages/paigasus-proto/tests/test_audit_protocol.py
git commit -m "$(cat <<'EOF'
feat(py): add Auditable Protocol over generated AuditMetadata (SMA-425)

runtime_checkable structural Protocol in paigasus-proto, with a test proving the
generated AuditableExample satisfies it (isinstance) plus a negative case. The
type-only AuditMetadata import sits under TYPE_CHECKING (ruff TCH).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Final verification (after all tasks)

- [ ] **Affected-graph build/test green:**
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core
moon ci :build
moon ci :test
```
Expected: green across affected projects (contracts + the three paigasus-proto packages).

- [ ] **Regen idempotency / `clean: true` blast radius:** from a clean tree, re-run `moon run contracts:generate` then `git status --porcelain`. Expected: **no diff** (generation is idempotent) and the hand-written `audit.rs`/`audit.ts`/`audit.py` are untouched (they live outside `generated/`).

- [ ] **Spec ACs all checked**, ADR-0012 linked on SMA-425, spec + plan committed.

## Self-review notes

- **Spec coverage:** proto (Task 1), Rust trait+wiring+dep+test (Task 2), TS interface+re-export+test (Task 3), Python Protocol+test (Task 4), embedding fixture (Task 1 + each test), ADR-0012 (Task 0), clean-regen verification (Final). All spec sections map to a task.
- **Type consistency:** `Auditable.audit()` (Rust) / `audit?` (TS) / `audit:` (Py) and the generated `AuditableExample.audit` field are used consistently across tasks; `AuditMetadataSchema`/`AuditableExampleSchema` (TS) and the `created_by`/`createdBy` casing per language are correct.
- **Independence:** Tasks 2/3/4 depend only on Task 1, not each other — safe to parallelize under subagent-driven development.
