# SMA-409 — Wire the kernel→bindings cascade on real PyO3 code + add an affected-graph regression guard

**Status:** approved design (brainstorm + staff review incorporated, ready for plan)
**Linear:** [SMA-409](https://linear.app/smaschek/issue/SMA-409/wire-cross-language-affected-graph-cascade-re-verify-at-phase-2-entry)
**Date:** 2026-06-14
**ADR:** ADR-0004 (protobuf + buf as the wire-contract source of truth), ADR-0005 (kernel-once — pure Rust kernel bound to Py/Node/WASM)
**Spun out of:** [SMA-363](https://linear.app/smaschek/issue/SMA-363/foundation-acceptance-gate) (foundation acceptance gate), 2026-06-09 design review, findings 1 + 11.
**Builds on:** [SMA-389](https://linear.app/smaschek/issue/SMA-389/wire-paigasus-proto-build-contractsgenerate-dependency-edges-when) (wired + verified the proto half of the cascade), [SMA-363](https://linear.app/smaschek/issue/SMA-363/foundation-acceptance-gate) (verified affected-graph *resolution* and the `kernel→gateway` edge manually).
**Reviewed by:** staff-engineer design review; dispositions in the final section.

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
and locks the entire cascade behind a CI regression guard so a deleted edge (or a removed
`--include-relations`) fails red instead of silently under-building.

## Decisions resolved during brainstorming

1. **Land real code, not an artificial edge (mirror SMA-389 decision #1).** A bare
   `dependsOn` edge on an empty `cdylib` stub would be artificial, and the root
   `cargo machete rs` gate fails on any *unused* dependency — so the edge cannot be
   faked with a dead `use`. SMA-409 lands a real kernel function that the binding
   genuinely calls, the same way SMA-389 landed a real `health.proto` rather than wiring
   against an empty `generate`.
2. **Prove the edge at compile level; defer the runtime/wheel chain.** The topology is
   `paigasus-kernel` (Rust) → `paigasus-py-bindings` (PyO3 → maturin wheel) →
   `paigasus-kernel-py` (uv package wrapping the wheel) → … The affected-graph cascade —
   this issue's actual subject — is fully proven once `cargo build -p paigasus-py-bindings`
   compiles the binding (which genuinely calls the kernel) and the `paigasus-kernel-rs →
   paigasus-py-bindings-rs` Moon edge resolves. The **runtime** round-trip proof (building
   the maturin wheel and importing it from Python) and wiring it into the uv workspace are
   a separate concern with unresolved build-isolation mechanics (uv↔maturin editable
   installs); both are **deferred to the wheel-integration follow-up**, where the mechanics
   get solved once. TS/napi/wasm cannot mirror this at all — no binding crate exists yet —
   and is likewise deferred. (Reverses the earlier "smoke test at the Rust/maturin level"
   decision per review finding F4 — see dispositions.)
3. **First kernel function = a tiny pure compute fn (input → output).** Canonical
   PyO3-style (`sum_as_string(a, b)` or a small deterministic helper), unit-tested at the
   Rust level. Dependency-free, no premature commitment to kernel domain semantics,
   swappable for real logic later (as `reserved.proto` was a placeholder). A
   constant-returning `version()` was rejected: it asserts a static string, only marginally
   above a stub.
4. **Regression guard = a script driven by a root Moon task (release-parity pattern).**
   `ci/affected-graph/run.sh` + a `repo:affected-smoke` task listed in the `ci.yml` task
   array, exactly like the existing `repo:release-parity*` gates. Hermetic (synthetic
   touched-files piped to `moon query`, no scratch-branch git mutation), locally runnable via
   `moon run repo:affected-smoke`, and the logic lives in a testable script rather than in
   workflow YAML. Inline-bash-in-`ci.yml` and a stateful scratch-branch integration test
   were both rejected. The core mechanism is **verified working** (see §4), not deferred.

## 1. Real code — kernel function + PyO3 binding

- **`rs/crates/libs/paigasus-kernel/src/lib.rs`:** one pure, dependency-free function
  taking arguments and returning a computed result, plus a `#[cfg(test)]` unit test
  asserting the computation. Genuinely the kernel's first primitive; explicitly a
  placeholder for real domain logic. The unit test runs under the existing
  `paigasus-kernel-rs:test` (`cargo nextest`) — real logic coverage with no FFI.
- **`rs/crates/bindings/paigasus-py-bindings`:**
  - `Cargo.toml`: add `pyo3` (with `abi3` + `extension-module`) and
    `paigasus-kernel = { workspace = true }` (`paigasus-kernel` added to
    `[workspace.dependencies]` in `rs/Cargo.toml` as a workspace-member dep). Add
    `[package.metadata.cargo-machete] ignored = ["pyo3"]` — pyo3 is consumed entirely
    through attribute macros, the canonical cargo-machete false-positive, and `:machete`
    is a blocking gate (SMA-375). `paigasus-kernel` is called directly and needs no ignore.
  - `src/lib.rs`: a `#[pyfunction]` that **calls the kernel function** and a `#[pymodule]`
    that registers it. The binding therefore genuinely consumes `paigasus-kernel` — the
    Cargo edge is real and `cargo machete` stays green.

Why `abi3` + `extension-module`: it compiles under plain `cargo build --workspace`
**without linking libpython**, so the existing workspace builds and the `:deny` / `:machete`
gates (which run over the whole `rs/` workspace) stay green, and `moon ci :build` compiles
the binding through the normal `paigasus-py-bindings-rs:build` task. The trade-off — that the
binding can't be exercised by `cargo test`/`nextest` (link error on Python symbols) — is moot
here: the FFI boundary is proven by compilation, the kernel *logic* is proven by the Rust unit
test, and the runtime round-trip proof is deferred (decision #2).

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

## 3. Affected-graph regression guard (AC #4 / SMA-363 finding 11)

`moon ci` *uses* the affected graph but never *asserts* it is correct — a broken edge just
under-builds and the run stays green. The guard closes that gap.

### Mechanism (verified working — moon 2.3.2)

A synthetic touched-file piped to `moon query projects --affected` with downstream relations
returns the affected set **and its dependents**, as JSON, hermetically (no git mutation):

```sh
printf '<touched-file>\n' | moon query projects --affected --downstream deep
# parse .projects[].id
```

Empirically confirmed against the live graph:

| Synthetic touch | Observed `.projects[].id` today |
| --- | --- |
| `rs/crates/libs/paigasus-kernel/src/lib.rs` | `paigasus-kernel-rs`, `paigasus-gateway-rs`, `repo` — **note: no `paigasus-py-bindings-rs`** (the missing edge this issue adds) |
| `contracts/proto/paigasus/gateway/v1/health.proto` | `contracts`, `paigasus-proto-rs`, `paigasus-proto-py`, `paigasus-proto-ts`, `paigasus-gateway-rs`, `repo` |
| `rs/crates/bindings/paigasus-py-bindings/src/lib.rs` | `paigasus-py-bindings-rs`, `repo` (one-directional — does not drag in the kernel) |

**`repo` appears in every set** (its source is `.`, the repo root, so it owns every file) — the
guard must filter `repo` out before asserting.

### Shape & assertions

- `ci/affected-graph/run.sh` + a root `repo:affected-smoke` task, added to the `ci.yml`
  `T=(…)` task array as `:affected-smoke` (resolves to `repo:affected-smoke`, the only project
  defining it — same as `:release-parity`).
- Per case, filter out `repo`, then assert **positive-superset + explicit-negative** (not strict
  equality, to stay robust as projects are added):

  | Synthetic touch | Must include (post-wiring) | Must exclude |
  | --- | --- | --- |
  | `contracts/proto/…/health.proto` | `contracts`, `paigasus-proto-rs`, `-py`, `-ts`, `paigasus-gateway-rs` | — |
  | `rs/crates/libs/paigasus-kernel/src/lib.rs` | `paigasus-kernel-rs`, `paigasus-py-bindings-rs`, `paigasus-gateway-rs` | any `contracts` / `*-py` / `*-ts` project (cross-stack isolation) |
  | `rs/crates/bindings/paigasus-py-bindings/src/lib.rs` | `paigasus-py-bindings-rs` | `paigasus-kernel-rs` (edge is one-directional) |

### Guarding the activating flag, not just the edges (F1)

The cascade depends on **two** things: the edges *and* `moon ci … --include-relations`. The
guard's own query hardcodes `--downstream deep`, so on its own it would stay green if someone
removed `--include-relations` from `ci.yml` while the real `moon ci` silently under-built. So:

- Add `.github/workflows/ci.yml` to the guard task's `inputs` (so removing the flag re-triggers
  the guard).
- The guard asserts the `moon ci` invocation(s) in `ci.yml` carry `--include-relations`.

### Inputs

`inputs` cover the graph-defining files — `**/moon.yml`, `.moon/**`, the dependency manifests
(`**/Cargo.toml`, `**/pyproject.toml`, `**/package.json`), `.github/workflows/ci.yml`, and
`ci/affected-graph/**` — so the guard is affected exactly when wiring (or the activating flag)
could change. A PR that deletes a `dependsOn` edge changes a `moon.yml`, trips the guard's
inputs, and fails red.

### Maintenance note (F5)

The **must-exclude** assertions encode a cross-stack-isolation invariant that holds *only*
because the py/ts kernel wrappers are deferred. When the deferred uv↔maturin integration lands
and `paigasus-kernel-py` genuinely wraps the wheel, a kernel edit *should* affect the py wrapper,
and this guard's must-exclude will correctly need updating. The must-include set is the durable
half; the must-exclude set is intentionally tied to the current topology and must be revised as
each deferred binding lands — a guard failure there is the expected next edge, not a regression.

## Verification (maps to acceptance criteria)

1. **AC #1 (edges where real deps exist) — partially satisfied, honestly.** New:
   `moon project paigasus-py-bindings-rs` lists `paigasus-kernel-rs` as a dependency with
   `^:build` on build/test; `cargo build -p paigasus-py-bindings` consumes the kernel;
   `cargo machete` / `cargo deny` stay green. Already-done: proto→contracts edges (SMA-389).
   **Deferred (not this issue):** the `paigasus-kernel-py` / TS "kernel wrappers in py/ts" half
   of AC #1 — no consumer crate exists yet (see Out of scope).
2. **AC #2 (contracts cascade re-verified).** The guard's contracts case asserts
   `contracts → proto-{rs,py,ts} → gateway`; corroborated by SMA-389's existing end-to-end proof.
3. **AC #3 (kernel edit → kernel + dependents).** The guard's kernel case asserts
   `kernel-rs → {kernel-rs, py-bindings-rs, gateway-rs}` and nothing cross-stack.
4. **AC #4 (regression guard).** `repo:affected-smoke` runs in CI; it **passes** on the wired
   graph and **fails** when a required edge is removed *or* `--include-relations` is dropped from
   `ci.yml` (both demonstrated during verification).
5. **Kernel logic exercised.** The kernel `#[cfg(test)]` unit test asserts the pure function's
   computation via `cargo nextest`. (Runtime FFI round-trip proof is deferred — decision #2.)

## Out of scope (deferred, with follow-ups)

- **Runtime PyO3 round-trip proof** (maturin wheel build + Python import/pytest) — moves to the
  wheel-integration issue below, where the uv↔maturin build-isolation is solved once.
- `paigasus-kernel-py` wrapping the maturin wheel + the uv↔maturin workspace integration —
  its own issue (the deferred half of AC #1, py side).
- TS kernel binding (`paigasus-kernel-ts`) and any napi/wasm binding crate — its own issue
  (the deferred half of AC #1, ts side; no binding crate exists yet).
- Flipping `paigasus-kernel` / `paigasus-py-bindings` `publish` or version off `0.0.0`
  (SMA-376, SMA-407).
- Real `paigasus-gateway` consumption of any kernel/proto API.
- Committing the kernel's real domain logic — the first function is a deliberate placeholder.

## Follow-ups

- File/track: `paigasus-kernel-py` consumes the wheel (uv↔maturin integration) **+ the runtime
  PyO3 round-trip smoke test** (moved here from this issue per F4).
- File/track: TS kernel binding via wasm or napi.
- Existing: SMA-376 (`paigasus-kernel` publish), SMA-407 (release activation / 0.1.0 floor).
- Minor/aside (not this issue): CLAUDE.md's "Moon is 2.2.5" gotcha is stale — the workspace is
  on **2.3.2**.

## Review dispositions (staff review, 2026-06-14)

- **F1 (Medium) — guard the `--include-relations` flag, not just the edges.** Accepted. §3 adds
  `.github/workflows/ci.yml` to the guard inputs and asserts the `moon ci` invocation carries
  `--include-relations`.
- **F2 (Medium) — core mechanism unverified.** Resolved by verification, not deferral. The
  `printf '<file>' | moon query projects --affected --downstream deep` mechanism was confirmed
  working on the live graph (moon 2.3.2), including the empirical sets in §3 and the `repo`-always
  -present caveat. No spike needed; no scratch-branch fallback.
- **F3 (Medium) — cargo-machete + pyo3.** Accepted and pre-committed:
  `[package.metadata.cargo-machete] ignored = ["pyo3"]` on the binding crate (§1).
- **F4 (Medium) — maturin runtime smoke test is adjacent + highest-risk.** Accepted. The runtime
  proof is deferred to the wheel-integration issue; the cascade edge is proven at compile level +
  a Rust unit test on the kernel logic. Reverses brainstorm decision #2's earlier "smoke at the
  Rust/maturin level"; removes the entire maturin/venv/pyproject surface from this issue.
- **F5 (Low) — must-exclude assertions tied to deferral boundary.** Accepted; documented as the
  maintenance note in §3.
- **F6 (Low) — AC #1 py/ts wrappers deferred, not satisfied.** Accepted; Verification §1 now
  states AC #1 is *partially* satisfied (py-bindings edge yes; py/ts wrappers deferred).
