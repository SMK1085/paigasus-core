# SMA-409 — Wire the kernel→bindings cascade on real PyO3 code + add an affected-graph regression guard

**Status:** approved design (brainstorm complete, ready for plan)
**Linear:** [SMA-409](https://linear.app/smaschek/issue/SMA-409/wire-cross-language-affected-graph-cascade-re-verify-at-phase-2-entry)
**Date:** 2026-06-14
**ADR:** ADR-0004 (protobuf + buf as the wire-contract source of truth), ADR-0005 (kernel-once — pure Rust kernel bound to Py/Node/WASM)
**Spun out of:** [SMA-363](https://linear.app/smaschek/issue/SMA-363/foundation-acceptance-gate) (foundation acceptance gate), 2026-06-09 design review, findings 1 + 11.
**Builds on:** [SMA-389](https://linear.app/smaschek/issue/SMA-389/wire-paigasus-proto-build-contractsgenerate-dependency-edges-when) (wired + verified the proto half of the cascade), [SMA-363](https://linear.app/smaschek/issue/SMA-363/foundation-acceptance-gate) (verified affected-graph *resolution* and the `kernel→gateway` edge manually).

## Goal

Verify the design's headline capability at the Phase-2 entry checkpoint: a proto or
kernel edit cascading rebuilds across languages, on **real code with declared edges**,
**guarded against silent regression** in CI.

The proto half already ships. SMA-389 wired and verified
`contracts/proto edit → contracts:generate → paigasus-proto-{rs,py,ts}:build →
paigasus-gateway-rs` in `moon ci` (via task-level `contracts:generate` edges + a
`^:build` edge on the gateway + `--include-relations`). SMA-363 separately verified
`kernel edit → paigasus-gateway-rs` (the gateway is the kernel's one declared consumer
today). **This issue therefore does not re-wire those edges.** It adds the single
genuinely-missing edge — `paigasus-kernel-rs → paigasus-py-bindings-rs` — on real code,
and locks the entire cascade behind a CI regression guard so a deleted edge fails red
instead of silently under-building.

## Decisions resolved during brainstorming

1. **Land real code, not an artificial edge (mirror SMA-389 decision #1).** A bare
   `dependsOn` edge on an empty `cdylib` stub would be artificial, and the root
   `cargo machete rs` gate fails on any *unused* dependency — so the edge cannot be
   faked with a dead `use`. SMA-409 lands a real kernel function that the binding
   genuinely calls, the same way SMA-389 landed a real `health.proto` rather than wiring
   against an empty `generate`.
2. **Stop at the PyO3 wheel; defer the rest of the chain.** The topology is
   `paigasus-kernel` (Rust) → `paigasus-py-bindings` (PyO3 → maturin wheel) →
   `paigasus-kernel-py` (uv package wrapping the wheel) → … The affected-graph cascade —
   this issue's actual subject — is fully satisfied by the
   `paigasus-kernel-rs → paigasus-py-bindings-rs` Moon edge. Wiring the maturin wheel
   into the uv workspace so `paigasus-kernel-py` imports it is a separate, non-trivial
   integration (uv↔maturin editable installs, build isolation) and is **deferred to its
   own issue**. TS/napi/wasm cannot mirror this at all — no binding crate exists yet — and
   is likewise deferred.
3. **First kernel function = a tiny pure compute fn (input → output).** Canonical
   PyO3-style (`sum_as_string(a, b)` or a small deterministic helper). The smoke test then
   asserts a real value round-trips `kernel → PyO3 → Python`, mirroring SMA-389's
   "construct `CheckResponse`, assert the field round-trips." Dependency-free, no premature
   commitment to kernel domain semantics, swappable for real logic later (as
   `reserved.proto` was a placeholder). A constant-returning `version()` was rejected: it
   asserts a static string, only marginally above a stub.
4. **Regression guard = a script driven by a root Moon task (release-parity pattern).**
   `ci/affected-graph/run.sh` + a `repo:affected-smoke` task listed in the `ci.yml` task
   array, exactly like the existing `repo:release-parity*` gates. Hermetic (synthetic
   touched-files, no scratch-branch git mutation), locally runnable via
   `moon run repo:affected-smoke`, and the logic lives in a testable script rather than in
   workflow YAML. Inline-bash-in-`ci.yml` and a stateful scratch-branch integration test
   were both rejected.

## 1. Real code — kernel function + PyO3 binding

- **`rs/crates/libs/paigasus-kernel/src/lib.rs`:** one pure, dependency-free function
  taking arguments and returning a computed result. Genuinely the kernel's first
  primitive; explicitly a placeholder for real domain logic.
- **`rs/crates/bindings/paigasus-py-bindings`:**
  - `Cargo.toml`: add `pyo3` (with `abi3` + `extension-module`) and
    `paigasus-kernel = { workspace = true }`. (`paigasus-kernel` is added to
    `[workspace.dependencies]` in `rs/Cargo.toml` as a path/workspace member dep.)
  - `src/lib.rs`: a `#[pyfunction]` that **calls the kernel function** and a `#[pymodule]`
    that registers it. The binding therefore genuinely consumes `paigasus-kernel` — the
    Cargo edge is real and `cargo machete` stays green.

Why `abi3` + `extension-module`: it compiles under plain `cargo build --workspace`
**without linking libpython**, so the existing workspace builds and the `:deny` / `:machete`
gates (which run over the whole `rs/` workspace) are unaffected. The trade-off is that the
binding cannot be exercised by `cargo test`/`nextest` (link error on Python symbols) — hence
the maturin-based smoke test in §3.

## 2. Build-graph edges (Moon)

`rs/crates/bindings/paigasus-py-bindings/moon.yml` gains, mirroring `paigasus-gateway-rs`:

```yaml
dependsOn:
  - 'paigasus-kernel-rs'

tasks:
  build:
    deps: ['^:build']
  test:
    deps: ['^:build']
```

This is the mechanism that makes the cascade *propagate*: a project-level `dependsOn` alone
does **not** mark a dependent as task-affected in `moon ci` (it defaults to changed-files-only)
— only a task-level `^:build` edge does, activated in CI by `--include-relations` (SMA-389
delta D3). Both `--include-relations` and the task-level pattern already exist; this issue
extends them to the binding crate.

**Proto edges are NOT changed.** `paigasus-proto-{rs,py,ts}` keep their task-level
`deps: ['contracts:generate']` (SMA-389 decision #4 — a project-level `dependsOn: contracts`
has nothing to bind `^:build` to, since `contracts` exposes no `build` task). AC #1's
"contracts → per-language proto packages" is therefore **already satisfied**; this spec records
it as done rather than re-doing it.

## 3. Smoke test & build model (highest-risk area)

The binding is exercised through maturin, not `cargo test`:

- Add `rs/crates/bindings/paigasus-py-bindings/pyproject.toml` (maturin build backend) and a
  minimal `tests/test_smoke.py`.
- The crate's `test` task is **overridden** for this crate — replacing the inherited
  `cargo nextest` (which has no Rust tests to run on an `extension-module` cdylib anyway) with
  a step that builds/develops the extension into an isolated environment and runs `pytest`,
  which imports the compiled module and asserts the value round-trips `kernel → PyO3 → Python`.
  It keeps the `deps: ['^:build']` edge from §2 so the kernel is built first.

**Open implementation details / risks to resolve in the plan (likely a short spike):**

- **maturin on PATH** — pin it (probably `.prototools`, proto-managed, consistent with the
  rest of the toolchain) so CI and contributors resolve the same version.
- **venv / uv mechanics** for the Moon `test` task (how the wheel is built and made importable
  hermetically without polluting the uv workspace, since `paigasus-kernel-py` stays empty).
- **Provisional native-module import name** — the wheel currently has no `paigasus-kernel-py`
  wrapper to re-export it; pick a sensible interim module name and record that it'll be
  reconciled when the wrapper lands.
- Confirm `cargo machete` accepts macro-only use of `pyo3`, and that
  `cargo build --workspace` + `:deny`/`:machete` stay green with the new `extension-module` crate.

## 4. Affected-graph regression guard (AC #4 / SMA-363 finding 11)

`moon ci` *uses* the affected graph but never *asserts* it is correct — a broken edge just
under-builds and the run stays green. The guard closes that gap.

- **Shape:** `ci/affected-graph/run.sh` + a root `repo:affected-smoke` task, added to the
  `ci.yml` `T=(…)` task array as `:affected-smoke` (resolves to `repo:affected-smoke`, the
  only project defining it — same as `:release-parity`).
- **Mechanism:** for each known touch case, feed a **synthetic touched-file set** to
  `moon query projects --affected` (with downstream relations included, matching how CI
  resolves the graph) and assert the resulting project set.
- **Cases & assertions** (positive-superset + explicit-negative, *not* strict equality, to
  stay robust as projects are added):

  | Synthetic touch | Must include | Must exclude |
  | --- | --- | --- |
  | `contracts/proto/paigasus/gateway/v1/health.proto` | `contracts`, `paigasus-proto-rs`, `paigasus-proto-py`, `paigasus-proto-ts`, `paigasus-gateway-rs` | — |
  | `rs/crates/libs/paigasus-kernel/src/lib.rs` | `paigasus-kernel-rs`, `paigasus-py-bindings-rs`, `paigasus-gateway-rs` | any `contracts`/`*-py`/`*-ts` project (cross-stack isolation) |

- **Task `inputs`:** the graph-defining files — `**/moon.yml`, `.moon/**`, the dependency
  manifests (`**/Cargo.toml`, `**/pyproject.toml`, `**/package.json`), and
  `ci/affected-graph/**` — so the guard is affected exactly when wiring could change. A PR that
  deletes a `dependsOn` edge changes a `moon.yml`, trips the guard's inputs, and fails red.
- **To verify in the plan:** the exact `moon query projects --affected` invocation for
  injecting synthetic touched-files (stdin form) and for including downstream relations in the
  result.

## Verification (maps to acceptance criteria)

1. **AC #1 (edges where real deps exist).** `moon project paigasus-py-bindings-rs` lists
   `paigasus-kernel-rs` as a dependency and `^:build` on its build/test tasks;
   `cargo build -p paigasus-py-bindings` consumes the kernel; `cargo machete` / `cargo deny`
   stay green. Proto→contracts edges documented as already-satisfied (SMA-389).
2. **AC #2 (contracts cascade re-verified).** The guard's contracts case asserts
   `contracts → proto-{rs,py,ts} → gateway`; corroborated by SMA-389's existing end-to-end proof.
3. **AC #3 (kernel edit → kernel + dependents).** The guard's kernel case asserts
   `kernel-rs → {kernel-rs, py-bindings-rs, gateway-rs}` and nothing cross-stack.
4. **AC #4 (regression guard).** `repo:affected-smoke` runs in CI; it **passes** on the wired
   graph and **fails** when a required edge is removed (demonstrated by a deliberately-broken
   `dependsOn` during verification).
5. **Binding exercised.** The maturin smoke `pytest` imports the module and asserts the
   round-tripped value, proving the kernel function executes through PyO3.

## Out of scope (deferred, with follow-ups)

- `paigasus-kernel-py` wrapping the maturin wheel + the uv↔maturin workspace integration —
  its own issue.
- TS kernel binding (`paigasus-kernel-ts`) and any napi/wasm binding crate — its own issue
  (no binding crate exists yet).
- Flipping `paigasus-kernel` / `paigasus-py-bindings` `publish` or version off `0.0.0`
  (SMA-376, SMA-407).
- Real `paigasus-gateway` consumption of any kernel/proto API.
- Committing the kernel's real domain logic — the first function is a deliberate placeholder.

## Follow-ups

- File/track: `paigasus-kernel-py` consumes the wheel (uv↔maturin integration).
- File/track: TS kernel binding via wasm or napi.
- Existing: SMA-376 (`paigasus-kernel` publish), SMA-407 (release activation / 0.1.0 floor).
