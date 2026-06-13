# SMA-389 — Proto Build-Graph Wiring + First Real Proto — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land the first real proto (a gRPC `HealthService`), commit its generated Rust/Python/TS code, and wire `contracts:generate` into the build graph so a proto edit regenerates and rebuilds the affected packages via `moon ci`.

**Architecture:** `contracts/` (buf) is the proto source of truth; `buf generate` writes committed code into three `paigasus-proto` packages (ADR-0004). Each proto package's build/test gains a task-level `deps: ['contracts:generate']` edge, which both orders generation before build and makes Moon treat `contracts` as a project-dependency for affected propagation (`contracts → proto → gateway`). Generated code is kept out of the strict lint/fmt gates but is typecheck-compiled and smoke-tested. Determinism is guaranteed by pinning all codegen plugins; a PR-level `git diff` gate catches committed-codegen drift.

**Tech stack:** Moon 2.2.5, buf v2, prost/tonic 0.14 (+ tonic-prost), betterproto2 0.10 (local plugin via uv), protobuf-es v2 (`@bufbuild/protobuf` 2.12), Rust edition 2024, uv, pnpm.

**Spec:** `docs/superpowers/specs/2026-06-13-sma-389-proto-build-graph-wiring-design.md`

---

## Resolved version matrix (pins)

| Lang | buf plugin (pin) | Runtime crate/pkg | Pin |
|------|------------------|-------------------|-----|
| Rust | `buf.build/community/neoeinstein-prost:v0.5.0` + `…-tonic:v0.5.0` | `prost`, `tonic`, `tonic-prost` | `0.14` |
| Py | local `protoc-gen-python_betterproto2` (pkg `betterproto2-compiler` 0.10.x) | `betterproto2[grpclib]` (runtime) | `>=0.10,<0.11` |
| TS | `buf.build/bufbuild/es:v2.12.0` | `@bufbuild/protobuf` | `^2.12.0` |

**Before pinning, re-verify latest patches** (versions drift):
```bash
curl -s https://crates.io/api/v1/crates/tonic        | python3 -c "import sys,json;print(json.load(sys.stdin)['crate']['max_stable_version'])"
curl -s https://crates.io/api/v1/crates/prost        | python3 -c "import sys,json;print(json.load(sys.stdin)['crate']['max_stable_version'])"
npm view @bufbuild/protobuf version
curl -s https://pypi.org/pypi/betterproto2/json          | python3 -c "import sys,json;print(json.load(sys.stdin)['info']['version'])"
curl -s https://pypi.org/pypi/betterproto2-compiler/json | python3 -c "import sys,json;print(json.load(sys.stdin)['info']['version'])"
# buf plugin tags:
curl -s "https://api.github.com/repos/bufbuild/plugins/contents/plugins/community/neoeinstein-prost" | python3 -c "import sys,json;print([x['name'] for x in json.load(sys.stdin)])"
```

## Environment note (every Bash step)

`moon`, `buf`, `uv`, `cargo`, `pnpm` are proto-managed and **off the default Bash PATH**. Export the proto shims first in any shell that runs them (per project memory):
```bash
export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"
```
There is no macOS `timeout`; don't wrap commands in it.

## File map

| File | Action | Responsibility |
|------|--------|----------------|
| `contracts/proto/paigasus/gateway/v1/health.proto` | create | The first real schema |
| `contracts/proto/paigasus/common/v1/reserved.proto` | delete | Placeholder no longer needed |
| `contracts/buf.gen.yaml` | modify | Pin plugins, `clean: true`, py → local plugin |
| `rs/Cargo.toml` | modify | Add `prost`/`tonic`/`tonic-prost` workspace deps |
| `rs/crates/libs/paigasus-proto/Cargo.toml` | modify | Consume the three crates |
| `rs/crates/libs/paigasus-proto/src/lib.rs` | modify | `include!` generated module under `#[allow]` |
| `rs/crates/libs/paigasus-proto/src/generated/**` | create (generated) | Committed prost/tonic output |
| `rs/crates/libs/paigasus-proto/rustfmt.toml` | create | `ignore = ["src/generated"]` |
| `rs/crates/libs/paigasus-proto/tests/health_smoke.rs` | create | Rust smoke test |
| `rs/crates/libs/paigasus-proto/moon.yml` | modify | `build`/`test` → `contracts:generate` |
| `py/pyproject.toml` | modify | Add `betterproto2-compiler` dev dep; ruff/pyright exclude generated |
| `py/packages/paigasus-proto/pyproject.toml` | modify | Add `betterproto2[grpclib]` runtime dep |
| `py/packages/paigasus-proto/src/paigasus_proto/generated/**` | create (generated) | Committed betterproto2 output |
| `py/packages/paigasus-proto/tests/test_health_smoke.py` | create | Python smoke test |
| `py/packages/paigasus-proto/moon.yml` | modify | `build` → `contracts:generate` |
| `ts/packages/paigasus-proto/package.json` | modify | Add `@bufbuild/protobuf` runtime dep |
| `ts/packages/paigasus-proto/src/generated/**` | create (generated) | Committed protobuf-es output |
| `ts/packages/paigasus-proto/src/health.test.ts` | create | TS smoke test |
| `ts/packages/paigasus-proto/moon.yml` | modify | `build`/`typecheck`/`test` → `contracts:generate` |
| `ts/eslint.config.js` | modify | Ignore `**/generated/**` |
| `ts/.prettierignore` | modify | Ignore generated |
| `.github/workflows/ci.yml` | modify | PR-level codegen drift gate |

> **Note on TDD shape:** codegen integration is *generate-then-observe*, not test-first — the generated API surface is discovered, so smoke tests are written against real output (Tasks 9–11) after generation (Task 5). The designable parts (proto, edges, lint excludes, drift gate) are verified test-first where it fits.

---

## Task 1: Author the first proto, remove the placeholder, lint clean

**Files:**
- Create: `contracts/proto/paigasus/gateway/v1/health.proto`
- Delete: `contracts/proto/paigasus/common/v1/reserved.proto`

- [ ] **Step 1: Write the proto**

```proto
// SPDX-License-Identifier: Apache-2.0
syntax = "proto3";

package paigasus.gateway.v1;

// Minimal liveness probe — the first real contract. Exercises the full
// prost + tonic / betterproto2 / protobuf-es codegen path end-to-end.
service HealthService {
  rpc Check(CheckRequest) returns (CheckResponse);
}

message CheckRequest {}

message CheckResponse {
  string status = 1;
}
```

- [ ] **Step 2: Remove the placeholder**

```bash
git rm contracts/proto/paigasus/common/v1/reserved.proto
```

- [ ] **Step 3: Lint (verify buf STANDARD passes)**

Run:
```bash
export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"
moon run contracts:lint
```
Expected: PASS. If buf complains about `SERVICE_SUFFIX` / `RPC_REQUEST_STANDARD_NAME`, the names above already satisfy them (`HealthService`, `CheckRequest`/`CheckResponse`); a failure means a typo. Do **not** run `contracts:generate` yet — plugins aren't pinned/wired (Tasks 2–4).

- [ ] **Step 4: Commit**

```bash
git add contracts/proto contracts/proto/paigasus/common/v1/reserved.proto
git commit -m "feat(contracts): first real proto — gateway/v1 HealthService (SMA-389)"
```

---

## Task 2: Pin buf plugins, add `clean: true`, switch Python to the local betterproto2 plugin

**Files:**
- Modify: `contracts/buf.gen.yaml`

- [ ] **Step 1: Rewrite `buf.gen.yaml`**

Replace the file with (re-verify pins per the matrix above first):

```yaml
version: v2
# clean wipes each out: dir before regeneration so deleted protos can't leave
# orphan generated files. Safe: every out: dir below holds ONLY generated code.
clean: true

plugins:
  # ─── Rust: prost (messages) + tonic (gRPC stubs), pinned (SMA-389/F1) ──────
  - remote: buf.build/community/neoeinstein-prost:v0.5.0
    out: ../rs/crates/libs/paigasus-proto/src/generated
    opt:
      - bytes=.
      - file_descriptor_set
  - remote: buf.build/community/neoeinstein-tonic:v0.5.0
    out: ../rs/crates/libs/paigasus-proto/src/generated
    opt:
      - no_include
      - compile_well_known_types

  # ─── Python: betterproto2 via LOCAL plugin (ADR-0004). Run through `uv run`
  #     so it resolves in the py workspace venv (which also provides the `ruff`
  #     the compiler shells out to). betterproto2-compiler is a py dev dep. ────
  - local: ['uv', 'run', '--project', '../py', 'protoc-gen-python_betterproto2']
    out: ../py/packages/paigasus-proto/src/paigasus_proto/generated

  # ─── TypeScript: protobuf-es v2, pinned ───────────────────────────────────
  - remote: buf.build/bufbuild/es:v2.12.0
    out: ../ts/packages/paigasus-proto/src/generated
    opt:
      - target=ts
      - import_extension=.js
```

- [ ] **Step 2: Sanity-check YAML parses**

Run:
```bash
export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"
cd contracts && buf generate --help >/dev/null && echo "buf OK" ; cd ..
```
Expected: `buf OK` (we only confirm buf reads the config later, after py deps land). Don't run `buf generate` yet — the py local plugin isn't installed (Task 3).

- [ ] **Step 3: Commit**

```bash
git add contracts/buf.gen.yaml
git commit -m "build(contracts): pin codegen plugins, clean:true, py local betterproto2 (SMA-389)"
```

---

## Task 3: Add the Python betterproto2 toolchain (compiler dev dep + runtime dep)

**Files:**
- Modify: `py/pyproject.toml` (root — `[dependency-groups] dev`)
- Modify: `py/packages/paigasus-proto/pyproject.toml` (runtime dep)

- [ ] **Step 1: Add the compiler to the py root dev group**

In `py/pyproject.toml`, add to the existing `[dependency-groups] dev` list (keep the others):

```toml
    "betterproto2-compiler>=0.10,<0.11",
```

- [ ] **Step 2: Add the runtime dep to the proto package**

In `py/packages/paigasus-proto/pyproject.toml`, replace `dependencies = []` with:

```toml
dependencies = [
  # Runtime for generated betterproto2 code. The [grpclib] extra is required
  # because the generated HealthService stub imports grpclib at module level.
  # Minor must match betterproto2-compiler (compiler enforces ~=0.10.0).
  "betterproto2[grpclib]>=0.10,<0.11",
]
```

- [ ] **Step 3: Lock + verify the plugin console-script resolves**

Run:
```bash
export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"
cd py && uv lock && uv sync --all-packages
uv run --project . protoc-gen-python_betterproto2 </dev/null >/dev/null 2>&1; echo "exit=$?"
cd ..
```
Expected: `uv lock`/`uv sync` succeed; the plugin invocation prints nothing and `exit=1` (a protoc plugin given empty stdin fails to parse a CodeGeneratorRequest — that's fine; it proves the executable resolves). A `command not found` / non-resolution is the failure to fix.

- [ ] **Step 4: Commit**

```bash
git add py/pyproject.toml py/packages/paigasus-proto/pyproject.toml py/uv.lock
git commit -m "build(py): betterproto2 compiler (dev) + runtime for proto codegen (SMA-389)"
```

---

## Task 4: Add the Rust codegen runtime crates

**Files:**
- Modify: `rs/Cargo.toml` (`[workspace.dependencies]`)
- Modify: `rs/crates/libs/paigasus-proto/Cargo.toml`

- [ ] **Step 1: Add workspace deps**

In `rs/Cargo.toml` under `[workspace.dependencies]` (re-verify 0.14 latest first):

```toml
# Proto codegen runtimes — versions track the pinned neoeinstein-prost/tonic
# v0.5.0 plugins (prost 0.14 / tonic 0.14). tonic 0.14 split the prost codec into
# the separate tonic-prost crate; generated service stubs need both.
prost       = "0.14"
tonic       = "0.14"
tonic-prost = "0.14"
```

- [ ] **Step 2: Consume them in the proto crate**

In `rs/crates/libs/paigasus-proto/Cargo.toml`, add after the `[package]` block:

```toml
[dependencies]
prost.workspace = true
tonic.workspace = true
tonic-prost.workspace = true
```

- [ ] **Step 3: Commit (compile happens in Task 6 after generation)**

```bash
git add rs/Cargo.toml rs/crates/libs/paigasus-proto/Cargo.toml
git commit -m "build(rs): add prost/tonic/tonic-prost for proto codegen (SMA-389)"
```

---

## Task 5: Generate, observe the output layout, commit the generated code

This is the bring-up step. Run generation once, record the actual file/module paths the three plugins emit — Tasks 6–11 reference them.

**Files:**
- Create (generated): the three `generated/` trees.
- Delete: the four `generated/.gitkeep` stubs (`clean: true` removes them; `git rm` any that linger).

- [ ] **Step 1: Generate**

```bash
export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"
moon run contracts:generate
```
Expected: succeeds. If the py `local:` plugin errors with `ruff: command not found`, the `uv run` env is missing ruff — confirm `betterproto2-compiler` (which depends on ruff) is synced (Task 3). If `uv` isn't found, the generate task lacks the proto shims on PATH — see Task 12.

- [ ] **Step 2: Record the emitted paths** (you will need these verbatim)

```bash
find rs/crates/libs/paigasus-proto/src/generated -type f
find py/packages/paigasus-proto/src/paigasus_proto/generated -type f
find ts/packages/paigasus-proto/src/generated -type f
```
Expected (names may differ — **use what you actually see**):
- Rust: `paigasus.gateway.v1.rs` (+ a `file_descriptor_set` `.bin`).
- Python: a `paigasus/gateway/v1/` package tree (+ `message_pool.py`, `__init__.py`s) under `generated/`.
- TS: `paigasus/gateway/v1/health_pb.ts` (protobuf-es names files `<proto>_pb.ts`).

- [ ] **Step 3: Remove any leftover stubs**

```bash
git rm --ignore-unmatch \
  rs/crates/libs/paigasus-proto/src/generated/.gitkeep \
  py/packages/paigasus-proto/src/paigasus_proto/generated/.gitkeep \
  ts/packages/paigasus-proto/src/generated/.gitkeep
```

- [ ] **Step 4: Stage the generated code (committed per ADR-0004)**

```bash
git add rs/crates/libs/paigasus-proto/src/generated \
        py/packages/paigasus-proto/src/paigasus_proto/generated \
        ts/packages/paigasus-proto/src/generated
git commit -m "feat(contracts): commit generated HealthService code (rs/py/ts) (SMA-389)"
```

---

## Task 6: Wire the Rust generated module + keep it out of clippy/fmt

**Files:**
- Modify: `rs/crates/libs/paigasus-proto/src/lib.rs`
- Create: `rs/crates/libs/paigasus-proto/rustfmt.toml`

- [ ] **Step 1: Replace `lib.rs` to include the generated module**

Use the **actual** generated filename from Task 5 Step 2 (shown here as `paigasus.gateway.v1.rs`):

```rust
// SPDX-License-Identifier: Apache-2.0

//! Generated protobuf + gRPC bindings for Paigasus (prost + tonic).
//!
//! Source of truth: `contracts/proto`; generated by `buf generate` into
//! `src/generated/` and committed (ADR-0004). Regenerate via
//! `moon run contracts:generate`.

pub mod gateway {
    pub mod v1 {
        // Generated code is excluded from the strict lint gate.
        #![allow(clippy::all)]
        include!("generated/paigasus.gateway.v1.rs");
    }
}
```

- [ ] **Step 2: Add the rustfmt ignore**

Create `rs/crates/libs/paigasus-proto/rustfmt.toml`:

```toml
# prost output (prettyplease) is not byte-identical to rustfmt; keep generated
# code out of the `cargo fmt --check` gate.
ignore = ["src/generated"]
```

- [ ] **Step 3: Build, clippy, fmt — verify clean**

```bash
export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"
cd rs
cargo build -p paigasus-proto
cargo clippy -p paigasus-proto --all-targets -- -D warnings
cargo fmt -p paigasus-proto --check
cd ..
```
Expected: all PASS. If `cargo build` emits **denied rustc warnings** from generated code (workspace `warnings = "deny"`), widen the module attribute to `#![allow(clippy::all, warnings)]`. If clippy still flags generated code, confirm the `#![allow]` is the first line *inside* `mod v1`.

- [ ] **Step 4: Commit**

```bash
git add rs/crates/libs/paigasus-proto/src/lib.rs rs/crates/libs/paigasus-proto/rustfmt.toml
git commit -m "feat(rs): wire generated gateway::v1 module, exclude from lint/fmt (SMA-389)"
```

---

## Task 7: Keep Python generated code out of ruff + basedpyright

**Files:**
- Modify: `py/pyproject.toml`

- [ ] **Step 1: Add excludes**

In `py/pyproject.toml`:
- Under `[tool.ruff]`, add:
```toml
extend-exclude = ["**/generated/**"]
```
- Under `[tool.basedpyright]`, append `"**/generated/**"` to the existing `exclude` list:
```toml
exclude = ["**/__pycache__", "**/node_modules", "**/.venv", "**/dist", "**/build", "**/generated/**"]
```

- [ ] **Step 2: Verify whole-tree py checks pass with generated present**

```bash
export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"
cd py
uv run ruff check .
uv run ruff format --check .
uv run basedpyright
cd ..
```
Expected: all PASS (generated code is excluded; the rest of the tree is unaffected). A flood of errors from `generated/` means an exclude glob didn't match — confirm the path matches what Task 5 emitted.

- [ ] **Step 3: Commit**

```bash
git add py/pyproject.toml
git commit -m "build(py): exclude generated code from ruff + basedpyright (SMA-389)"
```

---

## Task 8: Add the TS runtime dep + keep generated code out of eslint/prettier

**Files:**
- Modify: `ts/packages/paigasus-proto/package.json`
- Modify: `ts/eslint.config.js`
- Modify: `ts/.prettierignore`

- [ ] **Step 1: Add the runtime dependency**

In `ts/packages/paigasus-proto/package.json`, add a `dependencies` block (re-verify 2.12 latest):

```json
  "dependencies": {
    "@bufbuild/protobuf": "^2.12.0"
  },
```

- [ ] **Step 2: Ignore generated in eslint**

In `ts/eslint.config.js`, add `'**/generated/**'` to the existing global `ignores` array:

```js
  { ignores: ['**/dist/**', '**/.next/**', '**/node_modules/**', '**/*.d.ts', '**/generated/**'] },
```

- [ ] **Step 3: Ignore generated in prettier**

Append to `ts/.prettierignore`:

```
generated
```

- [ ] **Step 4: Install + verify typecheck/lint/format**

```bash
export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"
cd ts
pnpm install
pnpm exec tsc -p packages/paigasus-proto/tsconfig.json --noEmit
pnpm exec eslint .
pnpm exec prettier --check .
cd ..
```
Expected: all PASS. `tsc` **compiles** the generated `.ts` (that's the typecheck); eslint/prettier skip it. A tsc error in generated code usually means `@bufbuild/protobuf` version skew with the `bufbuild/es` plugin — align them.

- [ ] **Step 5: Commit**

```bash
git add ts/packages/paigasus-proto/package.json ts/eslint.config.js ts/.prettierignore ts/pnpm-lock.yaml
git commit -m "build(ts): add @bufbuild/protobuf, exclude generated from lint/fmt (SMA-389)"
```

---

## Task 9: Rust smoke test

**Files:**
- Create: `rs/crates/libs/paigasus-proto/tests/health_smoke.rs`

- [ ] **Step 1: Write the test** (use the real type path from Task 5 / Task 6)

```rust
// SPDX-License-Identifier: Apache-2.0
use paigasus_proto::gateway::v1::CheckResponse;

#[test]
fn check_response_carries_status() {
    let resp = CheckResponse {
        status: "ok".to_string(),
    };
    assert_eq!(resp.status, "ok");
}
```

- [ ] **Step 2: Run it**

```bash
export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"
cd rs && cargo nextest run -p paigasus-proto && cd ..
```
Expected: PASS (1 test). If `CheckResponse` isn't found, correct the module path to match Task 6's `include!` structure.

- [ ] **Step 3: Commit**

```bash
git add rs/crates/libs/paigasus-proto/tests/health_smoke.rs
git commit -m "test(rs): smoke-test generated CheckResponse (SMA-389)"
```

---

## Task 10: Python smoke test

**Files:**
- Create: `py/packages/paigasus-proto/tests/test_health_smoke.py`

- [ ] **Step 1: Write the test** (use the real import path from Task 5 Step 2)

```python
# SPDX-License-Identifier: Apache-2.0
from paigasus_proto.generated.paigasus.gateway.v1 import CheckResponse


def test_check_response_carries_status() -> None:
    resp = CheckResponse(status="ok")
    assert resp.status == "ok"
```

> Adjust the `import` to the actual generated package path printed in Task 5 (betterproto2 nests by proto package). Importing the module pulls in `grpclib` (service stub) — already provided by the `[grpclib]` extra (Task 3).

- [ ] **Step 2: Run it**

```bash
export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"
cd py && uv run pytest packages/paigasus-proto/tests/test_health_smoke.py -q && cd ..
```
Expected: PASS (1 test). `ModuleNotFoundError: grpclib` → the `[grpclib]` extra didn't install (re-check Task 3). Wrong import path → fix to match the generated tree.

- [ ] **Step 3: Commit**

```bash
git add py/packages/paigasus-proto/tests/test_health_smoke.py
git commit -m "test(py): smoke-test generated CheckResponse (SMA-389)"
```

---

## Task 11: TypeScript smoke test

**Files:**
- Create: `ts/packages/paigasus-proto/src/health.test.ts`

- [ ] **Step 1: Write the test** (protobuf-es v2 uses `create(Schema, {...})`; use the real `_pb.js` path + schema export from Task 5)

```ts
// SPDX-License-Identifier: Apache-2.0
import { create } from "@bufbuild/protobuf";
import { describe, expect, it } from "vitest";
import { CheckResponseSchema } from "./generated/paigasus/gateway/v1/health_pb.js";

describe("generated HealthService types", () => {
  it("CheckResponse carries status", () => {
    const resp = create(CheckResponseSchema, { status: "ok" });
    expect(resp.status).toBe("ok");
  });
});
```

> The `.js` import extension is correct — `buf.gen.yaml` sets `import_extension=.js`. Confirm the exported schema name (`CheckResponseSchema`) and path against Task 5's output.

- [ ] **Step 2: Ensure vitest is available, run it**

```bash
export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"
cd ts
# vitest is the test runner for typescript-project.yml's `test` task; add it as a
# devDependency of this package if not already present, then:
pnpm install
pnpm --filter @paigasus/proto exec vitest run
cd ..
```
Expected: PASS (1 test). If `vitest` is missing, add `"vitest": "catalog:"` (or the repo's pinned version) to the package `devDependencies`, `pnpm install`, re-run. Wrong schema export/path → fix to match generated output.

- [ ] **Step 3: Commit**

```bash
git add ts/packages/paigasus-proto/src/health.test.ts ts/packages/paigasus-proto/package.json ts/pnpm-lock.yaml
git commit -m "test(ts): smoke-test generated CheckResponse (SMA-389)"
```

---

## Task 12: Wire the build-graph edges

**Files:**
- Modify: `rs/crates/libs/paigasus-proto/moon.yml`
- Modify: `py/packages/paigasus-proto/moon.yml`
- Modify: `ts/packages/paigasus-proto/moon.yml`

- [ ] **Step 1: Rust proto edges** — append to `rs/crates/libs/paigasus-proto/moon.yml`:

```yaml
tasks:
  build:
    deps: ['contracts:generate']
  test:
    deps: ['contracts:generate']
```

- [ ] **Step 2: Python proto edge** — append to `py/packages/paigasus-proto/moon.yml`:

```yaml
tasks:
  build:
    deps: ['contracts:generate']
```

- [ ] **Step 3: TS proto edges** — append to `ts/packages/paigasus-proto/moon.yml`:

```yaml
tasks:
  build:
    deps: ['contracts:generate']
  typecheck:
    deps: ['contracts:generate']
  test:
    deps: ['contracts:generate']
```

- [ ] **Step 4: Verify the edges resolve in the task graph**

```bash
export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"
moon task paigasus-proto-rs:build     | grep -i contracts:generate
moon task paigasus-proto-rs:test      | grep -i contracts:generate
moon task paigasus-proto-py:build     | grep -i contracts:generate
moon task paigasus-proto-ts:typecheck | grep -i contracts:generate
```
Expected: each `moon task` printout lists `contracts:generate` among the task's dependencies, and Moon now treats `contracts` as a project-dep of each proto package. A Moon parse error means the `tasks:` block was mis-merged — confirm it's top-level YAML (sibling of `id:`/`layer:`).

- [ ] **Step 5: Verify `contracts:generate` actually runs under Moon (uv/buf on PATH)**

```bash
export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"
moon run paigasus-proto-rs:build
```
Expected: Moon runs `contracts:generate` first (you'll see it in the task list), then the cargo build. If `contracts:generate` fails under Moon with `uv: command not found`, the system-toolchain task isn't inheriting the proto shims; fix by ensuring `~/.proto/shims` is on PATH for Moon (it is in CI via `moon setup`; locally via `proto install`). Document any required shim export in CONTRIBUTING if newly discovered.

- [ ] **Step 6: Commit**

```bash
git add rs/crates/libs/paigasus-proto/moon.yml py/packages/paigasus-proto/moon.yml ts/packages/paigasus-proto/moon.yml
git commit -m "build: wire paigasus-proto build/test -> contracts:generate (SMA-389)"
```

---

## Task 13: PR-level codegen drift gate

**Files:**
- Modify: `.github/workflows/ci.yml`

- [ ] **Step 1: Add a drift-gate step after the `moon ci` step**

Insert immediately after the existing `- name: moon ci (affected graph)` step:

```yaml
      - name: Codegen drift gate (committed generated code matches protos)
        run: |
          moon run contracts:generate
          if ! git diff --exit-code -- \
              rs/crates/libs/paigasus-proto/src/generated \
              py/packages/paigasus-proto/src/paigasus_proto/generated \
              ts/packages/paigasus-proto/src/generated; then
            echo "::error::Generated code is stale. Run 'moon run contracts:generate' and commit the result."
            exit 1
          fi
```

> `moon run contracts:generate` is a **cache hit** (instant, no buf/uv) when protos are unchanged — the restored Moon task cache (existing ci.yml step) skips it — so this is cheap. On a proto-changing PR it regenerates and the `git diff` fails iff the author forgot to commit the regenerated code.

- [ ] **Step 2: Verify the gate locally (simulate drift)**

The fail-case must change a generator **input** (the proto) so `contracts:generate`
cache-misses and actually regenerates — corrupting a generated file alone is
cache-skipped and wouldn't exercise the gate.

```bash
export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"
GEN_DIRS=(rs/crates/libs/paigasus-proto/src/generated \
          py/packages/paigasus-proto/src/paigasus_proto/generated \
          ts/packages/paigasus-proto/src/generated)

# Pass case: clean tree, committed code matches the proto
moon run contracts:generate && git diff --exit-code -- "${GEN_DIRS[@]}" && echo "PASS (clean)"

# Fail case: change the proto WITHOUT regenerating-and-committing, then run the gate
cat >> contracts/proto/paigasus/gateway/v1/health.proto <<'EOF'

message DriftProbe {
  string x = 1;
}
EOF
moon run contracts:generate >/dev/null
git diff --quiet -- "${GEN_DIRS[@]}" && echo "UNEXPECTED CLEAN" || echo "PASS (drift caught)"

# Restore proto + committed generated code
git checkout -- contracts/proto/paigasus/gateway/v1/health.proto "${GEN_DIRS[@]}"
```
Expected: `PASS (clean)` then `PASS (drift caught)`.

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/ci.yml
git commit -m "ci: PR-level codegen drift gate for committed proto output (SMA-389)"
```

---

## Task 14: Full verification — affected-graph demo + green `moon ci`

No new files; this proves the acceptance criteria.

- [ ] **Step 1: Full affected run is green**

```bash
export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"
moon run :build :test :lint :fmt :typecheck
```
Expected: all PASS, including the three `paigasus-proto-*` packages and `contracts:generate`.

- [ ] **Step 2: AC#3 — a proto edit triggers regen + downstream rebuilds**

```bash
# touch a comment in the proto, confirm the affected set
printf '\n// touch for affected-graph check\n' >> contracts/proto/paigasus/gateway/v1/health.proto
moon ci :build --base main 2>&1 | tee /tmp/sma389-affected.txt
grep -E 'contracts:generate|paigasus-proto-(rs|py|ts):build|paigasus-gateway-rs:build' /tmp/sma389-affected.txt
git checkout -- contracts/proto/paigasus/gateway/v1/health.proto
```
Expected: the affected set includes `contracts:generate`, the three `paigasus-proto-*:build`, and `paigasus-gateway-rs:build` (gateway already `dependsOn` paigasus-proto-rs). Order: `contracts:generate` before the proto builds.

- [ ] **Step 3: AC#1/#2 — edges present**

```bash
moon task paigasus-proto-rs:build  | grep -i contracts:generate
moon task paigasus-proto-rs:test   | grep -i contracts:generate
moon task paigasus-proto-py:build  | grep -i contracts:generate
moon task paigasus-proto-ts:build  | grep -i contracts:generate
moon task paigasus-proto-ts:typecheck | grep -i contracts:generate
moon task paigasus-proto-ts:test   | grep -i contracts:generate
```
Expected: each prints a `contracts:generate` dependency.

- [ ] **Step 4: Clean-shell buf/uv resolution (CI proxy)**

```bash
env -i HOME="$HOME" PATH="$HOME/.proto/bin:$HOME/.proto/shims:/usr/bin:/bin" \
  bash -lc 'cd '"$PWD"' && moon run contracts:generate && echo CLEAN_SHELL_OK'
```
Expected: `CLEAN_SHELL_OK` (proves buf + the `uv run` betterproto2 plugin resolve without interactive shell rc).

- [ ] **Step 5: Final commit (if anything changed) + push the branch**

```bash
git status --porcelain   # expect clean if no fixups were needed
git push -u origin feature/sma-389-wire-paigasus-proto-build-contractsgenerate-dependency-edges
```

- [ ] **Step 6: Open the PR**

```bash
gh pr create --fill --base main
```
PR auto-links to SMA-389 by branch name (don't attach the Linear link manually).

---

## Done criteria (acceptance)

- [ ] `paigasus-proto-rs:build` and `:test` depend on `contracts:generate` (AC#1).
- [ ] `paigasus-proto-py:build` and `paigasus-proto-ts:{build,typecheck,test}` depend on `contracts:generate` (AC#2).
- [ ] A proto edit's `moon ci :build --base main` regenerates and rebuilds `contracts → proto-{rs,py,ts} → gateway` (AC#3).
- [ ] Generated code committed; `clean: true` set; `reserved.proto` gone; all four codegen plugins pinned.
- [ ] Generated code excluded from clippy/rustfmt, ruff/basedpyright, eslint/prettier; smoke tests green in all three languages.
- [ ] PR-level codegen drift gate live in `ci.yml`.
- [ ] Full `moon ci`-equivalent run green.
