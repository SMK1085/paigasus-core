# SMA-433 — Stand up the cross-binding behavioral parity test harness (ADR-0005)

**Status:** approved design (brainstorm complete; ready for plan)
**Linear:** [SMA-433](https://linear.app/smaschek/issue/SMA-433/stand-up-the-cross-binding-behavioral-parity-test-harness-adr-0005)
**Branch:** `feature/sma-433-stand-up-the-cross-binding-behavioral-parity-test-harness`
**Date:** 2026-06-18
**ADR:** ADR-0005 (kernel-once — one pure Rust kernel bound to Py/Node/WASM, never reimplemented per
language). The Development Guidelines spell out the safety net: *"a property-based suite that runs
against the Rust kernel impl AND each binding"* to catch cross-language drift.
**Follow-up of:** [SMA-419](https://linear.app/smaschek/issue/SMA-419) (PyO3 wheel),
[SMA-420](https://linear.app/smaschek/issue/SMA-420) (napi `.node`),
[SMA-427](https://linear.app/smaschek/issue/SMA-427) (wasm glue) — the three bindings now exist, each
with only a **local smoke test**. This issue stands up the parity harness deferred as SMA-427 §8 **L1**.

## Goal

Make a **single shared set of cases**, computed once from the Rust kernel, prove that **every binding**
(Python/PyO3, Node/napi, browser/wasm) and the Rust impl produce the **same observable result** — so the
moment real domain logic replaces `paigasus_kernel::sum`, any cross-language drift (a wrapper bug, an
FFI-boundary conversion bug) fails a test red instead of shipping silently. Wire it into the Moon graph
so a kernel edit re-runs the parity suite across all bindings, and guard the shared corpus against going
stale.

**This issue does not touch kernel logic** — `paigasus_kernel::sum(a: i64, b: i64) -> i64` stays the
deliberate placeholder from SMA-409. It is harness-only. The recommendation (issue + SMA-427 §8) is to
land this **before** the first real kernel domain logic, which is exactly when the net must already exist.

## Why now, before real logic

While `sum` is a trivial placeholder, every binding's result is independently re-derivable (`a + b`), so
the existing per-binding smoke tests (each asserting a value crosses *its own* boundary) appear to cover
parity. They do not: nothing asserts the bindings agree **with each other / with the kernel**. The moment
the kernel computes something non-trivial, you *cannot* re-derive the answer in Python/TS (that would be
reimplementing the kernel — the exact thing ADR-0005 forbids), so the only correct oracle is the kernel
itself. A harness that treats the kernel as the single oracle is therefore the design that survives the
jump to real logic — and it must already be in place when that jump happens.

## Decisions resolved during brainstorming

1. **Sharing model: kernel-as-oracle golden corpus.** A Rust generator computes a corpus of
   `{a, b, expected}` **from the kernel** (`expected = paigasus_kernel::sum(a, b)`); each language replays
   the *same* file against its binding, asserting it reproduces `expected`. Rejected alternatives:
   *collect-and-compare* (each binding writes outputs, a final stage asserts all agree) — needs
   cross-runtime output collection + a comparison stage, no clear oracle; *per-language property tests*
   (proptest + hypothesis + fast-check, each re-encoding the properties) — makes the *properties*, not the
   kernel, the shared spec, and risks re-encoding logic per language (against kernel-once). The corpus
   model is the only one where **only the kernel computes the answer**, so it scales to real domain logic.

2. **Corpus lifecycle: committed + drift guard, with a separate Rust proptest.** ADR-0005's sentence has
   two halves, satisfied by two mechanisms:
   - *"property-based against the Rust impl"* → a **proptest** suite on the kernel itself (randomized,
     fresh each run, seed-reproducible via proptest's regression file).
   - *"AND each binding"* → a **deterministic, committed** corpus that all three bindings replay.

   Randomization lives where it is cheap and reproducible (one in-process Rust runtime); the cross-binding
   vehicle is a **frozen, reviewable** corpus, because you cannot run *the same* random proptest in four
   runtimes. A `repo`-level **drift guard** (regenerate + `git diff --exit-code`) keeps the committed
   corpus in lockstep with the kernel. Rejected: *freshly-generated, uncommitted* (cross-workspace
   task-output plumbing, nondeterministic failures, unreviewable in PRs) and *committed-only, no proptest*
   (drops the "against the Rust impl" half).

3. **Parity domain: the i32-safe intersection of all binding surfaces.** The kernel is `i64`; napi and
   wasm narrow to `i32` (`paigasus_kernel::sum(a as i64, b as i64) as i32` — silent wrap outside i32, the
   SMA-427 **L5** debt); the PyO3 binding returns a *stringified i64* (`sum_as_string`). Parity therefore
   holds cleanly only where `a`, `b`, **and `a + b`** all fit in `i32`. The generator **enforces that
   invariant**, so `expected` (the i64 kernel result) equals every binding's observable result. The
   `i64`/`i32` and string/number surface differences are recorded as **out of the parity domain**,
   deferred to L5 (retired across all bindings at once when a kernel fn actually needs the range). Per
   binding, the comparison is: py `sum_as_string(a,b) == str(expected)`; napi/wasm `sum(a,b) === expected`;
   rust `paigasus_kernel::sum(a,b) == expected`.

4. **Corpus location: co-located with the generator crate** —
   `rs/crates/libs/paigasus-kernel-parity/vectors/sum.json`, read by py/ts via relative paths (the repo
   already reads FFI artifacts cross-workspace this way). Consistent with the co-location convention;
   keeps the corpus next to the only thing that may write it. One file **per kernel function** so a future
   `<fn>.json` lands without restructuring.

5. **Dedicated `paigasus-kernel-parity` crate, kernel stays pure.** ADR-0005 keeps `paigasus-kernel` free
   of adapter/tooling deps; the generator (`serde_json` + a seeded PRNG) and the corpus live in a separate
   `library`-layer crate. `proptest` is a **dev-dependency** of the kernel (dev-deps never enter the
   published artifact), so the kernel's own property test lives with the kernel.

6. **Keep a minimal Rust replay test** for a symmetric 4-way conformance set and to self-validate the
   committed corpus inside `cargo nextest` (it overlaps the drift guard slightly, but is ~10 lines and
   makes the corpus a true four-runtime vector).

## Components

### Rust — kernel proptest (`rs/crates/libs/paigasus-kernel/`)
- Add `proptest` as a **dev-dependency**.
- A property test (`tests/props.rs` or an inline `#[cfg(test)]` module) asserting kernel properties over
  the i32-safe domain: `sum(a,b) == a + b`, commutativity (`sum(a,b) == sum(b,a)`), and identity
  (`sum(a,0) == a`). Generators are constrained so `a + b` stays in `i32` (mirrors the corpus domain).
- The kernel's existing `sums_two_integers` unit test stays.

### Rust — parity crate (`rs/crates/libs/paigasus-kernel-parity/`)
- `moon.yml`: id `paigasus-kernel-parity-rs`, `layer: library`, `language: rust`,
  `dependsOn: [paigasus-kernel-rs]`, `build`/`test` with `deps: ['^:build']` (mirrors the binding crates,
  so a kernel edit cascades — SMA-389 D3).
- `Cargo.toml`: depends on `paigasus-kernel`; `serde`/`serde_json` for the corpus; a seeded PRNG
  (`rand`'s `StdRng::seed_from_u64` with an arbitrary fixed constant) for the random sample.
  `publish = false`.
- A `gen-parity-vectors` **bin** that:
  - assembles **curated edge cases** (`0`, `±1`, `i32::MAX`, `i32::MIN`, and pairs whose sum approaches
    but stays within the i32 boundary) **+ a fixed-seed pseudo-random sample**, all within the parity
    domain (reject/skip any draw whose `a + b` overflows `i32`);
  - computes `expected = paigasus_kernel::sum(a, b)` for each;
  - writes **byte-stable** JSON to `vectors/sum.json` (stable case order, deterministic formatting,
    trailing newline) so regeneration is reproducible and `git diff` is meaningful.
- A **replay test** deserializing `vectors/sum.json` and asserting
  `paigasus_kernel::sum(a, b) == expected` for every case (also a structural guard: non-empty, every case
  inside the parity domain).

### The committed corpus (`rs/crates/libs/paigasus-kernel-parity/vectors/sum.json`)
- A flat JSON array of `{ "a": i32, "b": i32, "expected": i64 }`. One file per kernel function.

### Python replay (`py/packages/paigasus-kernel/tests/`)
- Replace `test_ffi_roundtrip.py` with a corpus-driven test: load `sum.json` (path resolved relative to
  this file, walking up to the parity crate), `pytest.mark.parametrize` over the cases, assert
  `sum_as_string(a, b) == str(expected)`.
- Add the corpus to the `paigasus-kernel-py:test` task `inputs` so a corpus change re-runs it (the
  existing `--reinstall-package` wheel-freshness machinery is unchanged).

### TypeScript replay (`ts/packages/paigasus-kernel/tests/`)
- Both existing vitest projects (`node` + `browser`/wasm) replay the corpus instead of hardcoded values:
  load `sum.json`, iterate, assert `sum(a, b) === expected`. (`sum.test.ts` → node/napi,
  `sum.wasm.test.ts` → browser/wasm; the existing project/alias wiring in `vitest.config.ts` is unchanged.)
- Add the corpus to the `paigasus-kernel-ts:build`/`:test` task `inputs`.

### Drift guard (`repo` project + `ci/`)
- A `repo:parity-corpus-drift` task (`toolchain: system`, like `affected-smoke`/`release-parity`): runs
  `cargo run --manifest-path rs/Cargo.toml -p paigasus-kernel-parity --bin gen-parity-vectors` then
  `git diff --exit-code` over `vectors/sum.json`. A kernel change without a corpus regen → **red**.
- **Narrow `inputs`** (kernel src, parity crate src, the committed corpus) — `repo` owns the whole tree,
  so without narrow inputs the task would run on every change.
- Add `:parity-corpus-drift` to the `moon ci` `T=()` array in `.github/workflows/ci.yml`.
- Document the guard in the parity crate's `README.md` (the generator lives there; this guard is a
  one-line `cargo run … && git diff` inline in `moon.yml`, so it needs no `ci/<guard>/` script dir of
  its own — unlike `affected-graph`/`release-parity`).

## Data & control flow

```
proptest ─→ randomized props on paigasus_kernel::sum        (rs only; fresh, seed-reproducible)
gen bin  ─→ vectors/sum.json   [COMMITTED, byte-stable; expected = kernel output]
                │
  drift guard: regen + git diff --exit-code                 (repo task, in the moon ci array)
                │
   ┌────────────┼───────────────┬────────────────┐
 rs replay   py replay        ts/node replay   ts/wasm replay
 ==kernel    ==str(expected)   ==expected       ==expected
```

## Moon wiring + affected-graph guard update

The existing cascade (`paigasus-kernel-rs → {py,node,wasm} bindings → paigasus-kernel-{py,ts}`) already
re-runs all three binding tests on a kernel edit. This issue adds:

- `vectors/sum.json` to the `inputs` of `paigasus-kernel-py:test` and `paigasus-kernel-ts:{build,test}`.
- The new `paigasus-kernel-parity-rs` crate as a kernel dependent.

**The strict-equality affected-graph guard (`ci/affected-graph/run.sh`, SMA-429) MUST be updated** —
default-deny means an unlisted-but-present project fails the case:
- `kernel->bindings` expected set **gains** `paigasus-kernel-parity-rs` (else "unexpected" → red).
- A new **`parity-oneway`** case: editing `rs/crates/libs/paigasus-kernel-parity/src|vectors` affects
  **only** `paigasus-kernel-parity-rs` — `paigasus-kernel-rs` is deliberately absent (a parity edit must
  not rebuild the kernel), one-directional w.r.t. the kernel like the binding-oneway cases.
- `ci/affected-graph/README.md` updated to match both.

## Verification (maps to acceptance criteria)

1. **Shared cases against every binding (scope bullet 1).** `cargo nextest` runs the kernel proptest and
   the Rust corpus replay; `paigasus-kernel-py:test`, and the `node` + `browser` vitest projects, each
   replay `vectors/sum.json` against their binding and pass. A deliberately wrong wrapper (e.g. napi `a -
   b`) fails that binding's replay red.
2. **Moon-graph wiring (scope bullet 2).** `moon ci :build :test --include-relations` after a kernel edit
   re-runs the parity replay in all three bindings + the kernel proptest/replay; `moon run
   repo:parity-corpus-drift` passes on a clean tree and fails if the corpus is stale relative to the
   kernel.
3. **Affected-graph guard green & extended.** `moon run repo:affected-smoke` passes with
   `paigasus-kernel-parity-rs` in the `kernel->bindings` set and the new `parity-oneway` case;
   `--negative-control` still fails red.
4. **Cross-stack isolation preserved.** A kernel edit does not drag in `contracts`, the `*-py`/`*-ts`
   packages other than the kernel wrappers, or unrelated crates; a parity-crate edit affects only itself.
5. **Existing gates green.** `cargo deny` / `cargo machete` stay green over `rs/` with the new crate +
   dev-deps (proptest/rand/serde mainstream, Apache/MIT-compatible, all actually used).

## Out of scope (deferred, with follow-ups)

- **L5 — `i32` FFI surface over the `i64` kernel.** The parity domain is the i32-safe intersection; the
  `i64`/`i32` and string/number surface differences are documented, not exercised. Retired across all
  bindings at once (explicit `BigInt`/checked conversion) when a kernel fn needs the range.
- **L2 — committed-glue drift CI check.** This drift guard covers the **parity corpus only**. The
  separate, systemic check that the committed napi/wasm glue matches a fresh build (SMA-427 §8 L2) stays
  its own ticket.
- **Additional kernel functions.** One corpus file per function; only `sum` exists today. New functions
  add `<fn>.json` + their replay rows without restructuring.
- **Real kernel domain logic.** `sum` stays the deliberate placeholder; this harness is the prerequisite
  net for it.
- **Per-binding fuzz/property generation in the binding's own language.** Out of scope by decision #1
  (the corpus is the shared vehicle; languages replay, they do not re-randomize).
