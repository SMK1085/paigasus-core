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
(Python/PyO3, Node/napi, browser/wasm) and the Rust impl produce the **same decoded value** — so the
moment real domain logic replaces `paigasus_kernel::sum`, any cross-language drift (a wrapper bug, an
FFI-boundary conversion bug) fails a test red instead of shipping silently. Wire it into the Moon graph
so a kernel edit re-runs the parity suite across all bindings, and guard the shared corpus against going
stale.

**Scope of "parity" (precise).** This harness asserts *decoded-value* equality, **not** *surface*
identity. The bindings do not share a surface today — the PyO3 binding returns a stringified i64
(`sum_as_string`), napi/wasm return a `number` — so each replay normalizes to a common value (`str(...)`
for py, a number for node/wasm). Unifying the surfaces (retiring `sum_as_string` for a numeric/typed
return) is real, load-bearing work, tracked under L5 (§ Out of scope), arguably a harder prerequisite for
real domain logic than this harness. And this harness proves binding↔kernel **fidelity**, never kernel
**correctness** — for `sum`, `a + b` is a complete independent oracle (the proptest), but for real logic
the proptest *properties* become the only correctness check (see *What parity does and does not prove*,
below).

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

## What parity does and does not prove

This harness proves **binding↔kernel fidelity**: every binding reproduces what the kernel computes. It
does **not** prove the kernel is **correct** — by construction the corpus bakes in whatever the kernel
currently returns (`expected = paigasus_kernel::sum(a, b)`), so all four replays would happily agree on a
*wrong-but-consistent* kernel. Correctness is the **proptest's** job, and that is the unsolved-by-this-issue
half: for `sum`, `sum(a,b) == a + b` is a complete independent oracle, but for real domain logic there is
no `a + b` to check against (re-deriving it is the reimplementation ADR-0005 forbids), so the proptest
*properties* (commutativity, identity, and whatever invariants the real function admits) become the
**only** correctness check. Weak or incomplete properties = a correctness net full of holes while parity
stays green. **Convention for real logic:** every kernel function ships with properties that pin its
*behavior*, not merely its parity. This issue scaffolds that discipline for `sum`; designing strong
properties is per-function work due when each real function lands.

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

   **The committed corpus uses a deterministic *enumerated* sample — no PRNG.** The drift
   guard regenerates and `git diff --exit-code`s the corpus, so generation must be byte-stable across
   toolchain and dependency bumps. `rand`'s `StdRng` is explicitly documented as **not** reproducible
   across `rand` releases (its backing algorithm has already changed once), so a routine `cargo update`
   would silently change the sample and red the drift guard with an alarming, kernel-unrelated diff. A
   fixed *enumerated* sample (a lattice of `(a, b)` over the i32-safe domain + the curated edges) is fully
   reproducible with no PRNG at all, just as reviewable, and keeps `rand` out of the parity crate. If
   breadth ever demands pseudo-randomness, use `rand_chacha::ChaCha*Rng` (documented-stable, version-pinned)
   — not `StdRng`. Randomized exploration already lives in the in-process proptest, so the committed
   vehicle does not need a PRNG.

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
   of adapter/tooling deps; the generator (`serde`/`serde_json`, no PRNG — see decision #2) and the corpus
   live in a separate `library`-layer crate. `proptest` is a **dev-dependency** of the kernel (dev-deps
   never enter the published artifact), so the kernel's own property test lives with the kernel.

6. **Keep a minimal Rust replay test** for a symmetric 4-way conformance set and to self-validate the
   committed corpus inside `cargo nextest` (it overlaps the drift guard slightly, but is ~10 lines and
   makes the corpus a true four-runtime vector).

7. **Every language replay carries a corpus-integrity guard.** A replay that merely
   iterates-and-asserts goes **green** if the corpus fails to load or comes back empty — and a
   cross-workspace relative-path load is the most likely failure. So *each* of the four replays
   (rust/py/node/wasm), not just Rust, independently asserts the corpus loaded and contains the committed
   case count before/independently of the per-case comparison. This closes the only way a parity net can
   silently lie.

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
- `Cargo.toml`: depends on `paigasus-kernel`; `serde`/`serde_json` for the corpus. **No `rand`** — the
  sample is deterministically enumerated (decision #2). `publish = false`.
- A `gen-parity-vectors` **bin** that:
  - assembles **curated edge cases** (`0`, `±1`, `i32::MAX`, `i32::MIN`, and pairs whose sum approaches
    but stays within the i32 boundary) **+ a deterministic enumerated sample** (a fixed lattice of
    `(a, b)` across the i32-safe domain), all within the parity domain (any pair whose `a + b` would
    overflow `i32` is excluded by construction);
  - computes `expected = paigasus_kernel::sum(a, b)` for each;
  - writes **byte-stable** JSON to `vectors/sum.json` (stable case order, deterministic formatting,
    trailing newline) so regeneration is reproducible and `git diff` is meaningful.
- A **replay test** deserializing `vectors/sum.json` and asserting
  `paigasus_kernel::sum(a, b) == expected` for every case, plus the shared **corpus-integrity invariant**
 : the corpus loaded, is non-empty, matches the committed case count, and every case lies
  inside the parity domain.

### The committed corpus (`rs/crates/libs/paigasus-kernel-parity/vectors/sum.json`)
- A flat JSON array of `{ "a": i32, "b": i32, "expected": i64 }`. One file per kernel function.

### Python replay (`py/packages/paigasus-kernel/tests/`)
- Replace `test_ffi_roundtrip.py` with a corpus-driven test: load `sum.json` via a **single resolved
  path constant** (one helper that resolves the corpus from `__file__`, not an ad-hoc relative path per
  call), `pytest.mark.parametrize` over the cases, assert `sum_as_string(a, b) == str(expected)`.
- **Corpus-integrity guard:** a *separate, non-parametrized* test asserts the corpus loaded,
  is non-empty, and contains the committed case count. Without it, a bad path → an empty parametrize set,
  which pytest reports as **skipped** (`got empty parameter set`), i.e. a green run that compared nothing —
  the worst failure mode for a safety net. The integrity test fails red on a zero/short load.
- Add the corpus to the `paigasus-kernel-py:test` task `inputs` so a corpus change re-runs it (the
  existing `--reinstall-package` wheel-freshness machinery is unchanged).

### TypeScript replay (`ts/packages/paigasus-kernel/tests/`)
- Both existing vitest projects (`node` + `browser`/wasm) replay the corpus instead of hardcoded values:
  load `sum.json` via a **single resolved path constant**, iterate, assert
  `sum(a, b) === expected`. (`sum.test.ts` → node/napi, `sum.wasm.test.ts` → browser/wasm; the existing
  project/alias wiring in `vitest.config.ts` is unchanged.)
- **Corpus-integrity guard:** an `expect(cases.length).toBe(<committed count>)` (or `>0`)
  assertion that runs regardless of the per-case loop — a zero-length load (a wrong cwd/relative path)
  otherwise registers **no `it()`s** and the file passes green having compared nothing.
- Add the corpus to the `paigasus-kernel-ts:build`/`:test` task `inputs`.

### Drift guard (`repo` project + `ci/`)
- A `repo:parity-corpus-drift` task (`toolchain: system`, like `affected-smoke`/`release-parity`): runs
  the generator **crate-scoped** (`cargo run -p paigasus-kernel-parity --bin gen-parity-vectors`, from
  `rs/` so `rs/.cargo/config.toml` is in scope) then `git diff --exit-code` over `vectors/sum.json`. A
  kernel change without a corpus regen → **red**. Keep it `-p`-scoped, never `--workspace`:
  the parity crate is a plain lib+bin with no cdylib, so it needs none of the apple-darwin
  `-undefined dynamic_lookup` link flags — broadening to `--workspace` would pull in the FFI cdylibs and
  hit the macOS link trap `rs/.cargo/config.toml` exists to avoid.
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
- `kernel->bindings` expected set **gains** `paigasus-kernel-parity-rs` (else "unexpected" → red). The
  live baseline already lists `paigasus-wasm-rs` (SMA-427 landed — verified), so this adds exactly one
  entry; no rebase needed on a branch cut from current `main`.
- A new **`parity-oneway`** case: editing `rs/crates/libs/paigasus-kernel-parity/src|vectors` affects
  **only** `paigasus-kernel-parity-rs` — `paigasus-kernel-rs` is deliberately absent (a parity edit must
  not rebuild the kernel), one-directional w.r.t. the kernel like the binding-oneway cases.
- `ci/affected-graph/README.md` updated to match both.

**Moon affected-semantics to verify at implementation time.** `moon query projects
--affected` tracks a project's `source` dir + `dependsOn` relations; a cross-project task `inputs` glob
(py/ts listing the corpus under `rs/`) drives task *hashing/caching*, **not** project-affected status. So
the expectation above — a corpus edit affects only `paigasus-kernel-parity-rs`, py/ts do not appear — is
the likely behavior, but it must be confirmed: the strict-equality guard is **self-verifying** here, so
if Moon disagrees the new case fails red during implementation and we set the expected set to whatever
Moon actually reports. The **coverage consequence** is accepted explicitly: a *corpus-only* change (not
accompanied by a kernel edit) will **not** re-run the py/ts replays via affected CI. That gap is narrow
and covered three ways — (a) the normal workflow is a kernel edit + corpus regen in one PR, where the
kernel edit cascades to py/ts; (b) a hand-edited or stale corpus is caught by the drift guard
(`git diff --exit-code`); (c) the full push-to-`main` run replays everything. We therefore do **not** add
an artificial py/ts→parity dependency edge (it would couple the binding packages to a fixture they only
read). If that gap ever proves to matter, revisit with a real edge.

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
   deps (`serde`/`serde_json`, `proptest` as a kernel dev-dep — mainstream, Apache/MIT-compatible, all
   actually used; **no `rand`**, per H2).
6. **Corpus-integrity guard bites.** Pointing a replay at a nonexistent corpus path fails the
   py/ts/rust integrity assertion **red** (not a skipped/zero-`it()` green) — i.e. the net cannot pass
   while comparing zero cases.

## Out of scope (deferred, with follow-ups)

- **L5 — surface unification (`i32`/`i64` + string vs number).** The parity domain is the i32-safe
  intersection, and parity is *decoded-value* equality, not *surface* identity (see Goal): the
  py binding returns a stringified i64 (`sum_as_string`), napi/wasm return a `number` narrowed to i32.
  Retiring `sum_as_string` for a numeric/typed return and widening the i32 boundary (explicit
  `BigInt`/checked conversion) happens across **all** bindings at once when a kernel fn needs the range —
  a real prerequisite for non-trivial domain logic. The TS side already has a surface-parity guard
  (`binding-parity.types.ts`, SMA-427 M5) holding napi and wasm `sum` type-identical; **Python sits
  outside it** today because its surface genuinely differs — bringing Python under the same
  "surfaces-must-match" discipline is part of this L5 unification, not buildable before it.
- **L2 — committed-glue drift CI check.** This drift guard covers the **parity corpus only**. The
  separate, systemic check that the committed napi/wasm glue matches a fresh build (SMA-427 §8 L2) stays
  its own ticket.
- **Additional kernel functions.** One corpus file per function; only `sum` exists today. New functions
  add `<fn>.json` + their replay rows without restructuring.
- **Real kernel domain logic.** `sum` stays the deliberate placeholder; this harness is the prerequisite
  net for it.
- **Per-binding fuzz/property generation in the binding's own language.** Out of scope by decision #1
  (the corpus is the shared vehicle; languages replay, they do not re-randomize).
