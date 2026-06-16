# SMA-420 — Stand up a TS kernel binding (napi-rs) + wire the cascade to `paigasus-kernel-ts`

**Status:** approved design (brainstorm + staff review incorporated, ready for plan)
**Linear:** [SMA-420](https://linear.app/smaschek/issue/SMA-420/stand-up-a-ts-kernel-binding-wasmnapi-wire-the-cascade-to-paigasus)
**Date:** 2026-06-15
**ADR:** ADR-0005 (kernel-once — pure Rust kernel bound to Py/Node/WASM). ADR-0005 already names the three binding crates (`paigasus-py-bindings`, `paigasus-node-bindings`, `paigasus-wasm`), binds **Node via napi-rs** and **browser/Edge via wasm-bindgen**, and explicitly *rejected* "WASM only" in favour of the hybrid. This issue stands up the first of the two TS-facing bindings.
**Layout authority:** Notion *Polyglot Monorepo Scoping* §1 (Directory layout) + §3 (Shared Rust core + FFI) — the same co-located-artifact fallback SMA-419 used for the Python wheel.
**Follow-up of:** [SMA-409](https://linear.app/smaschek/issue/SMA-409/wire-cross-language-affected-graph-cascade-re-verify-at-phase-2-entry) — completes the deferred **ts side of AC #1** ("kernel wrappers in ts"). SMA-409 could not mirror the kernel→bindings cascade on the TS side because no TS binding crate existed.
**Mirrors:** [SMA-419](https://linear.app/smaschek/issue/SMA-419/wire-paigasus-kernel-py-to-the-pyo3-wheel-uvmaturin-runtime-smoke-test) — the Python runtime round-trip (uv↔maturin). This is the structurally-symmetric Node (napi-rs) version; design context `docs/superpowers/specs/2026-06-15-sma-419-paigasus-kernel-py-pyo3-wheel-design.md`.
**Reviewed by:** staff-engineer design review (incorporated; dispositions in the final section).

## Goal

Make the kernel value cross `Rust kernel → napi-rs → .node addon → Node/TypeScript` at **runtime**,
prove it with a `vitest`, and extend the affected-graph guard so a kernel (or binding) edit now
legitimately cascades into the TS stack. This completes the deferred **ts side of SMA-409 AC #1**
("kernel wrappers in ts") and keeps the two language bindings structurally symmetric with the
Python side shipped in SMA-419.

SMA-409 shipped the Rust kernel primitive (`paigasus_kernel::sum`) and the PyO3 binding; SMA-419
proved the Python runtime chain. **This issue does not re-touch the kernel logic** — it stands up
the Node binding crate, builds the Node consumption chain on top, and extends the existing guard.

## Decisions resolved during brainstorming

1. **napi-rs first; wasm (`paigasus-wasm`) deferred.** Per ADR-0005 the kernel is bound to Node
   via napi-rs and to browsers/Edge via wasm-bindgen — two separate crates, both eventually. We
   stand up `paigasus-node-bindings` (napi-rs) first because it is the closest structural mirror of
   the PyO3 crate (a native `cdylib` whose runtime symbols resolve at load), reuses the existing
   macOS link config, and the kernel's natural first consumer is server-side (Node/Next.js server,
   future services). **Consequence, called out explicitly:** `@paigasus/kernel` becomes **Node-only**
   until the `paigasus-wasm` sibling lands (ADR-0005's browser/Edge path). A browser consumer (the
   Next.js console's client components) cannot use `@paigasus/kernel` yet — this is the documented
   intermediate state, not a regression.
2. **Blessed `@napi-rs/cli` path with a co-located `package.json`.** A `package.json`
   (`@paigasus/node-bindings`) lives **inside the binding crate** (`rs/crates/bindings/paigasus-node-bindings/`,
   beside its `Cargo.toml`) — the direct analog of SMA-419's co-located maturin `pyproject.toml`.
   `napi build` drives cargo and emits `index.node` + generated `index.js` + `index.d.ts`. We take
   the documented napi-rs path rather than hand-rolling a loader (SMA-419 F1 lesson: take the
   documented layout, don't invent a third one), getting typed bindings for free. The co-located
   `package.json` is **not** a pnpm workspace member (it is outside `ts/`); it is reached only via a
   `file:` specifier (below).
3. **Runtime round-trip, single host platform** (mirror SMA-419, not the compile-only SMA-409).
   The AC requires the binding be *consumed by* and *genuinely re-exported from* `paigasus-kernel-ts`,
   so we prove `import { sum } from '@paigasus/kernel'` works at runtime in Node. The cross-platform
   `.node` prebuild matrix and npm publish are **deferred** (see Out of scope).
4. **Double-compile accepted** (mirror SMA-419 §6). In a full `moon ci :build` the binding crate
   compiles twice: `paigasus-node-bindings-rs:build` runs `cargo build` (for the crate's own
   `fmt`/`clippy`/`nextest` gates), and the `napi build` step compiles it again to produce the
   shippable `.node`. Both invoke cargo against `rs/target/`; cargo serializes on its target-dir lock
   if scheduled concurrently. Accepted as the cost of a uniform `:build` gate plus an independently
   buildable Rust crate.
5. **Keep the placeholder `sum` as the public surface.** The kernel fn is a deliberate placeholder
   (SMA-409 decision #3). `@paigasus/kernel` re-exports `sum(a, b): number` (napi-idiomatic; the
   kernel `sum` returns `i64`, which napi-rs maps to a JS `number` — safe for the placeholder's small
   inputs). The `i64`→`number` mapping is **spike-confirmed** (§6 check #5 / review F3): some napi-rs
   versions surface `i64` as `BigInt` (so `sum(2,3)` returns `5n` and `5n !== 5` fails the test
   silently) — if so the binding narrows to `i32`→`number` (or the test asserts `5n`). The smoke test
   asserts the FFI round-trip, not a domain contract. *(Parity note: the
   Python binding exposes `sum_as_string(): str`. We diverge to an idiomatic `number` on the Node
   side rather than mirroring the string return; the shared thing that matters for cross-binding
   parity (ADR-0005) is the kernel operation, not the FFI return type.)*

## 1. Package layout (co-located fallback)

### `rs/crates/bindings/paigasus-node-bindings/` — new napi-rs binding crate

Mirrors the shape of `paigasus-py-bindings`:

```
rs/crates/bindings/paigasus-node-bindings/
  Cargo.toml        # [lib] crate-type = ["cdylib"]; test = false / doctest = false
  package.json      # @paigasus/node-bindings — napi config + @napi-rs/cli devDep (co-located)
  src/lib.rs        # #[napi] fn sum(a, b) -> calls paigasus_kernel::sum  (real call → cargo-machete honest)
  moon.yml          # id: paigasus-node-bindings-rs
```

- `Cargo.toml`: `crate-type = ["cdylib"]`, `test = false`/`doctest = false` (a napi `cdylib` leaves
  N-API symbols undefined, so a Rust test harness for this target can't link — same as the PyO3 crate;
  kernel logic is unit-tested in `paigasus-kernel`, the FFI boundary is proven by compilation +
  runtime smoke test). Dependencies: `napi`/`napi-derive` (added to `rs/Cargo.toml`
  `[workspace.dependencies]`) and `paigasus-kernel.workspace = true`.
- `[package.metadata.cargo-machete] ignored = ["napi"]` — `napi`/`napi-derive` are consumed only
  through attribute macros (`#[napi]`), the canonical cargo-machete false-positive (exactly like
  `pyo3`); `:machete` is a blocking gate (SMA-375). `paigasus-kernel` is called directly and needs no
  ignore. *(Spike confirms whether `napi-derive` needs a separate ignore entry.)*
- `src/lib.rs`: a `#[napi]`-annotated `sum` that **calls `paigasus_kernel::sum`** — the Cargo edge is
  real and `cargo machete` stays green.
- `package.json` (`@paigasus/node-bindings`): napi config (binary name) + `@napi-rs/cli` devDep + a
  `build` script (`napi build`). SPDX per the CONTRIBUTING config-file exemption.

### macOS link flags (reused, comment broadened)

`rs/.cargo/config.toml` already carries `-undefined dynamic_lookup` for `*-apple-darwin` (added for
the PyO3 `extension-module` cdylib in SMA-409). A napi-rs cdylib needs the **same** flags — its
N-API symbols are resolved by the Node runtime at load, exactly as libpython resolves PyO3's. The
flags are reused unchanged; only the comment is broadened to name napi alongside PyO3. As that file's
note already warns, cargo discovers it by walking up from the working directory, so the napi build's
cargo invocation must run with **cwd inside `rs/`** (spike check #1).

### `ts/packages/paigasus-kernel/` — the public wrapper (`@paigasus/kernel`)

```jsonc
// package.json additions
"dependencies": { "@paigasus/node-bindings": "file:../../../rs/crates/bindings/paigasus-node-bindings" },
// Conditional exports (Scoping §3 shape), pointing at SOURCE until tsup/dist lands (see below).
// node → the napi re-export; default (browser/Edge/workerd) → a stub that throws a legible error
// until paigasus-wasm exists, so a browser bundler fails cleanly instead of choking on a .node.
"exports": {
  ".": {
    "node": "./src/index.ts",
    "default": "./src/unsupported.ts"
  }
}
```

```typescript
// src/index.ts  (node condition — keeps its SPDX header)
export { sum } from "@paigasus/node-bindings";

// src/unsupported.ts  (default/browser/Edge — keeps its SPDX header)
throw new Error(
  "@paigasus/kernel has no browser/Edge binding yet — wasm (paigasus-wasm) is a tracked follow-up",
);
```

- The `file:` specifier is the pnpm analog of SMA-419's uv path source
  (`{ path = "../../../rs/crates/bindings/paigasus-py-bindings" }`) — a cross-`ts/` link, **not** an
  extension of the pnpm workspace globs. pnpm links it and installs its devDeps (`@napi-rs/cli`).
- **Conditional `exports` set up now (review F2).** Scoping §3's canonical `@paigasus/kernel` uses a
  `node`/`browser`/`workerd`/`default` `exports` map (→ napi / wasm). We adopt that **structure** now,
  with a `node` condition + a `default` stub that throws — so the Node-only intermediate state is a
  legible runtime error (not a bundler explosion) and the wasm follow-up slots into the `default`
  (and explicit `browser`/`workerd`) branch without reworking the wrapper. **Adjustment vs. §3:** the
  conditions point at **source** (`./src/…`), not §3's `./dist/…`, because the tsup/dist build is
  deferred (the package's `_comment_exports` note); they flip to `dist` when tsup wiring lands.
- Re-exporting a native artifact means `@paigasus/kernel` is no longer pure-source; the
  `_comment_exports` note is updated to record that the `node` path now loads a compiled `.node` via
  its binding dependency. `private: true` and `version: 0.0.0` are unchanged (publish deferred).

## 2. Build-graph edges (Moon)

The cascade `paigasus-kernel-rs → paigasus-node-bindings-rs → paigasus-kernel-ts`, propagated by
task-level `^:build` under `moon ci --include-relations` (a project-level `dependsOn` alone does not
mark a dependent task-affected — SMA-389 D3 / `moon-ci-affected-model`):

- **`rs/crates/bindings/paigasus-node-bindings/moon.yml`** (`id: paigasus-node-bindings-rs`,
  `layer: library`, `language: rust`): `dependsOn: ['paigasus-kernel-rs']`, `build`/`test` tasks with
  `deps: ['^:build']`. A near-copy of `paigasus-py-bindings-rs/moon.yml`. Its `build` is the plain
  `cargo build` gate (what `fmt`/`clippy`/`nextest` compile against).
- **`ts/packages/paigasus-kernel/moon.yml`** (`id: paigasus-kernel-ts`): gains
  `dependsOn: ['paigasus-node-bindings-rs']` and **overrides its `build`/`test` to produce the `.node`
  themselves** — `^:build` is **not** sufficient (review F1). `^:build` builds *upstream*
  (`paigasus-node-bindings-rs:build` = plain `cargo build` → the cdylib in `rs/target`), **not** the
  `index.node` that `napi build` post-processes; and Moon does not auto-order a project's `test` after
  its own `build`. Concretely:
  - `build`: runs `napi build` against the co-located crate (emitting `index.node` + `index.js` +
    `index.d.ts`, declared as `outputs:` for caching) — overriding the inherited `tsc --noEmit`.
    Typecheck stays covered by the separate inherited `typecheck` task (append `tsc --noEmit` to
    `build` if CI's `moon ci` target list doesn't run `typecheck`). `deps: ['^:build']` for the
    kernel→binding cascade.
  - `test`: a `script` that runs `napi build` (fresh) **then** `pnpm exec vitest run` — the direct
    mirror of the py `test` task's `uv sync --reinstall-package paigasus-py-bindings && uv run pytest`
    (verified in `py/packages/paigasus-kernel/moon.yml`; the py side is likewise **explicit**, not the
    implicit `uv run` build the review first described). Building inside the test task buys both the
    **ordering** (the `.node` exists before the import) and the **cache-bust** (a Rust edit re-runs the
    napi compile, not a stale `.node`) in one step. `deps: ['^:build']`.
  - The `dependsOn` to a **Rust** project also provisions the Rust toolchain in this task's context —
    the SMA-419 cross-toolchain note in reverse (there a Python project invoked cargo via maturin; here
    a TypeScript project invokes cargo via napi).
- **No new `paigasus-node-bindings-ts` Moon project** — the `.node` is built as part of the
  `paigasus-kernel-ts` build chain (mirroring "no new `paigasus-py-bindings-py` project" — the wheel
  was built as part of `paigasus-kernel-py:build`). The graph stays
  `kernel-rs → node-bindings-rs → kernel-ts`.

**Resolved by this revision (was open):** the **test→build ordering** (review F1). Because `vitest`
does not build the `.node` the way the py `test` rebuilds the wheel, the `napi build` is run *inside*
`paigasus-kernel-ts`'s own `build` and `test` tasks (above) rather than left to `^:build` — closing the
most likely "green locally, fails in a clean CI ordering" trap. **Still spike-confirmed (§6):** the
exact `napi build` invocation and that its cargo cwd stays **inside `rs/`** (the macOS link-flag
hazard — `napi build` is run via `pnpm exec napi build` pointed at the co-located crate dir, napi-cli
resolvable from the Node project via the `file:` link), plus `@napi-rs/cli`/`file:` resolution.

## 3. Public surface & runtime smoke test

The round-trip test lives in **`ts/packages/paigasus-kernel/tests/sum.test.ts`** (exercising the
public surface, so it transitively proves the whole chain). The runner stays `vitest` (the inherited
`test` is `pnpm exec vitest run --passWithNoTests`), but `paigasus-kernel-ts`'s `test` is **overridden**
to run `napi build` first (§2 / review F1) so the `.node` is fresh before the import — the test body is
unchanged:

```typescript
// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from "vitest";
import { sum } from "@paigasus/kernel";

describe("kernel FFI", () => {
  it("crosses the napi boundary", () => {
    expect(sum(2, 3)).toBe(5);
  });
});
```

This is the **first real test in the TS stack**; the `--passWithNoTests` "no tests" mask becomes moot
for this package (and stays harmless elsewhere) — the direct analog of SMA-419 making the py
"no tests collected" mask moot for `paigasus-kernel-py`.

## 4. Affected-graph regression guard (`ci/affected-graph/run.sh` + README)

Now that a kernel edit legitimately reaches the TS stack, update the two cases that asserted TS
isolation. This is the maintenance-note revision SMA-409 F5 / SMA-419 §5 anticipated — a guard change
here is the expected next edge, not a regression. Verified against the live graph: no ts package
consumes `@paigasus/kernel` today, so a kernel edit cascades to `paigasus-kernel-ts` and **stops there**
on the ts side; all six other `-ts` projects stay legitimately in must-exclude.

- **`kernel->bindings` case** (`rs/crates/libs/paigasus-kernel/src/lib.rs`): add
  `paigasus-node-bindings-rs` **and** `paigasus-kernel-ts` to must-include. **Drop the blanket `-ts$`**
  from the forbid-regex (it no longer holds), but keep asserting the negatives that remain true — the
  unrelated ts projects and the existing non-ts negatives:

  ```
  must-include: paigasus-kernel-rs, paigasus-py-bindings-rs, paigasus-gateway-rs,
                paigasus-kernel-py, paigasus-node-bindings-rs, paigasus-kernel-ts
  forbid-regex: ^(commitlint-config|paigasus-console|paigasus-docs|paigasus-proto|paigasus-sdk|paigasus-ui)-ts$|^contracts$|^py$|^ts$|^paigasus-(proto|workflows|ml)-py$
  ```

  (The explicit `-ts` enumeration replaces the blanket `-ts$`; `paigasus-kernel-ts` is intentionally
  absent from it. Note the forbid-regex now starts with `^(...)`, not `-`, so the `grep -E --`
  guard against leading-`-` parsing still applies but is no longer strictly required for this case.)
- **New `binding-oneway-node` case** (`rs/crates/bindings/paigasus-node-bindings/src/lib.rs`):
  must-include `paigasus-node-bindings-rs`, `paigasus-kernel-ts`; must-exclude `^paigasus-kernel-rs$`
  (one-directional w.r.t. the kernel — mirrors the existing `binding-oneway` py case).
- The existing `contracts->proto` and `binding-oneway` (py) cases, the `--negative-control`, and the
  `assert_include_relations` check are unchanged.
- Update `ci/affected-graph/README.md`'s maintenance note to reflect the new topology (kernel now
  reaches `paigasus-kernel-ts`; the remaining ts isolation set).

## 5. Build mechanics & the double-compile

As in SMA-419 §6, the binding crate compiles twice in a full `moon ci :build`:
`paigasus-node-bindings-rs:build` (`cargo build`, for the `fmt`/`clippy`/`nextest` gates) and the
`napi build` step (producing the `.node`). Both run cargo against `rs/target/`; if Moon schedules them
concurrently, cargo serializes on its target-dir lock (safe; the second waits). Accepted deliberately —
the same two-builders-into-one-area class as SMA-419's double-compile, in napi form. The spike confirms
whether `napi build` shares `rs/target/` or uses its own; the shared dir is preferred (cache reuse).

`paigasus-kernel-ts:test` runs `napi build` again (the F1 fix — fresh `.node` before `vitest`), so a
full `moon ci :build` + `:test` triggers the napi compile in both `kernel-ts:build` and `kernel-ts:test`.
This is deliberate: it is the cache-bust that guarantees the test asserts against a freshly-compiled
`.node` (the SMA-419 `uv sync --reinstall-package`-in-`test` analog), not a regression. cargo's
target-dir lock keeps the concurrent invocations safe.

## 6. Primary risk — de-risk first (spike before anything else)

The napi build orchestration across the `rs/` ↔ `ts/` pnpm boundary is the real unknown. The first
implementation step is a throwaway spike proving the chain end-to-end on the user's macOS host,
checking **all** of:

1. **macOS link via cwd.** `napi build` runs cargo with cwd **inside `rs/`** so the
   `rs/.cargo/config.toml` `-undefined dynamic_lookup` flags resolve and the `.node` links (no
   undefined N-API/`_napi*` symbols). This is the SMA-419 F3 hazard in napi form.
2. **`@napi-rs/cli` provisioning + `file:` resolution.** How the co-located `@paigasus/node-bindings`
   package's `@napi-rs/cli` devDep installs and how the `file:` link from `@paigasus/kernel` resolves
   under the ts pnpm workspace (including in CI's `pnpm install`). Confirm cargo + the Rust toolchain
   are on PATH when the napi build triggers (CI: Moon provisions via the `dependsOn`; locally: after
   `proto install`).
3. **Consume + import.** `@paigasus/kernel` re-exports and `sum` is callable from a Node/vitest
   process (`import { sum } from '@paigasus/kernel'` → `sum(2,3) === 5`).
4. **Cache-bust on a Rust edit.** The `napi build` is run inside `paigasus-kernel-ts`'s `build`/`test`
   tasks (§2, the F1 fix) precisely so a kernel/binding **Rust-source** change re-runs the `napi`
   **compile** rather than asserting against a stale `.node` (the SMA-419 S4 analog). Confirm a Rust
   edit actually triggers the recompile here (a real compile, not a cache hit) and that the
   test-after-build ordering holds in a clean CI run.
5. **`i64`→JS `number` mapping (review F3).** Confirm the pinned napi-rs maps the kernel's `i64`
   return to a JS `number` (so `sum(2,3) === 5`), not a `BigInt` (`5n !== 5`, a silent test failure).
   If it is `BigInt`, narrow the binding to `i32`→`number` (or assert `5n`).

Known non-blocking caveat: a `.node` won't auto-recompile on a Rust edit without a rebuild — handled in
CI/test by the in-task `napi build`, acceptable for ad-hoc dev iteration otherwise.

## 7. ADR note (AC #4)

ADR-0005 already names `paigasus-node-bindings` and `paigasus-wasm` and decides the napi/wasm hybrid,
so **no new ADR** is needed. AC #4 ("binding mechanism choice recorded") is satisfied by a short note
appended to ADR-0005 recording that **napi-rs was stood up first and wasm-bindgen
(`paigasus-wasm`) is the tracked follow-up**, with a pointer to this spec. (Recorded in Notion, where
the ADRs live.)

## Verification (maps to acceptance criteria)

1. **AC #1** — `paigasus-node-bindings` wraps `paigasus_kernel::sum` and is consumed by
   `paigasus-kernel-ts`; `napi build` produces the `.node`; the ESM import
   (`import { sum } from '@paigasus/kernel'`, the vitest round-trip) succeeds; `cargo machete` /
   `cargo deny` stay green over the whole `rs/` workspace.
2. **AC #2** — `moon ci :build`/`:test` cascade a kernel edit to `paigasus-kernel-ts` under
   `--include-relations` (the new `dependsOn` + `^:build` edges); the vitest round-trip passes at
   runtime.
3. **AC #3** — `moon run repo:affected-smoke` passes with the updated must-include + narrowed
   forbid-regex + new `binding-oneway-node` case; `ci/affected-graph/run.sh --negative-control` still
   fails red; existing `moon ci` gates unaffected.
4. **AC #4** — binding mechanism choice (napi-rs first) recorded as a note on ADR-0005.
5. **Cross-stack isolation preserved** — a kernel edit does **not** drag in `contracts`, the `*-py`
   packages other than `paigasus-kernel-py`, or the `-ts` packages other than `paigasus-kernel-ts`.

## Out of scope (deferred, with follow-ups)

- **`paigasus-wasm` (wasm-bindgen) binding** for browsers/Edge — the second TS-facing binding ADR-0005
  calls for; unblocks `@paigasus/kernel` in the Next.js console's client components. Its own issue.
- **Cross-platform `.node` prebuild matrix** (the napi-rs `@napi-rs/cli` per-platform prebuild +
  `optionalDependencies` packaging) and **npm publish** (`private: false` / version off `0.0.0` for
  `@paigasus/kernel` / `@paigasus/node-bindings`) — the napi analog of the deferred Python wheel
  publish (ADR-0006, SMA-376/407). Single-host build only here.
- **Real kernel domain logic** — `sum` remains a deliberate placeholder.
- **Affected-graph guard completeness meta-check** (review F4) — the kernel-touch forbid-regex is now a
  hand-maintained enumeration on its third revision (blanket `-ts$` → narrowed → explicit per-package),
  so a newly-added ts/py package is **silently unasserted** until hand-added. A completeness check
  (every affected project is either in must-include or matched by the forbid-regex → fail on an
  unclassified project), or a default-deny "affected set == must-include" assertion, would remove the
  smell. **Deferred to its own guard-refinement issue**, not folded in here: it reverses SMA-409's
  deliberate "positive-superset + explicit-negative, *not* strict equality (to stay robust as projects
  are added)" choice, so it merits its own decision. (Offer to file as a follow-up issue.)

## Review dispositions (staff review, 2026-06-16)

- **F1 (Medium — `vitest` won't build the `.node` like `uv run`; test→build ordering) — accepted,
  design changed.** `^:build` only builds *upstream* (`paigasus-node-bindings-rs` = `cargo build` → a
  cdylib in `rs/target`, not `index.node`), and Moon doesn't auto-order a project's `test` after its own
  `build`. §2 now runs `napi build` *inside* `paigasus-kernel-ts`'s own `build` and `test` tasks; the
  `test` task mirrors the py `test`'s explicit `uv sync --reinstall-package … && pytest` (verified —
  the py side was likewise explicit, *not* the implicit `uv run` build the review described), giving
  ordering + cache-bust in one step. Folded into §5 and spike §6 check #4.
- **F2 (Low — diverges from Scoping §3 conditional exports) — accepted with adjustment.** Verified §3
  shows `@paigasus/kernel` with `node`/`browser`/`workerd`/`default` conditional `exports` (→ napi /
  wasm). §1/§3 now adopt that structure with a `node` condition + a `default` stub that throws a legible
  "no browser binding yet" error, so the browser path fails cleanly and the wasm follow-up slots into
  `default` without reworking the wrapper. **Adjustment:** the conditions point at **source**, not §3's
  `./dist/…`, because the tsup/dist build is deferred (the package's `_comment_exports`); they flip to
  `dist` when tsup lands.
- **F3 (Low — confirm `i64`→`number`, not `BigInt`) — accepted as a spike check.** Can't be verified
  without the pinned napi-rs (not yet chosen), so §6 spike check #5 confirms it; if it surfaces as
  `BigInt`, the binding narrows to `i32`→`number` (or the test asserts `5n`). Noted in decision #5.
- **F4 (Low — guard forbid-regex is a growing hand-maintained enumeration) — accepted as a follow-up,
  not this issue.** A completeness/default-deny meta-check reverses SMA-409's deliberate
  positive-superset choice, so it merits its own decision rather than expanding SMA-420. Recorded in
  Out of scope.
