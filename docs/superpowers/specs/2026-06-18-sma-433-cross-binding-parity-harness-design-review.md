# Design review — SMA-433 cross-binding behavioral parity harness

**Reviews:** `docs/superpowers/specs/2026-06-18-sma-433-cross-binding-parity-harness-design.md`
**Reviewer perspective:** staff engineering — "what bites us later"
**Date:** 2026-06-18
**Sources checked:** the spec; Linear SMA-433 (scope + relations) and SMA-409/419/420/427; ADR-0005 +
the Notion Development Guidelines line it quotes; and the live repo — the three bindings as they
actually exist now (`rs/crates/bindings/paigasus-wasm` glue, `paigasus-node-bindings`,
`paigasus-py-bindings`), `py/packages/paigasus-kernel/tests/test_ffi_roundtrip.py` +
`moon.yml`, `ts/packages/paigasus-kernel/{vitest.config.ts,src/binding-parity.types.ts}`,
`ci/affected-graph/run.sh`, and the `rand`/`StdRng` reproducibility guarantees.

## Verdict

This is the right harness, and it's the one I flagged as the missing safety net in the SMA-427
review (§8 L1). Decision #1 (kernel-as-oracle golden corpus) is architecturally correct, and the
"why now, before real logic" argument is exactly right: once the kernel computes something
non-trivial, the kernel is the *only* legitimate oracle, so the harness must predate that jump. The
decomposition of ADR-0005's sentence into "proptest for *against the Rust impl*" + "committed corpus
for *each binding*" (decision #2) is clean, and the crate separation respects the pure-kernel
constraint. The repo is in good shape to receive it: SMA-427 has landed (the `kernel->bindings`
guard set already lists `paigasus-wasm-rs`), and my SMA-427 M5 recommendation is in as
`binding-parity.types.ts`.

Two findings would let the net pass without actually catching anything, which is the worst failure
mode for a safety net specifically. A third is about what "parity" here really means — it's
narrower than the spec implies, and the narrowness is exactly where real domain logic will bite.
Details, severity-ordered.

| ID | Severity | One-line |
|----|----------|----------|
| H1 | High | Only the Rust replay asserts the corpus is non-empty; a py/ts path-resolution or empty-corpus failure passes/skips green — a parity net that can't fail |
| H2 | High | Byte-stable corpus + git-diff drift guard on `rand::StdRng`, which is explicitly *not* reproducible across versions → a routine `cargo update` reds the guard with a confusing diff |
| M1 | Medium | The harness checks decoded-*value* parity, not *surface* parity, and per-language normalization masks the real divergence (py returns a string, node/wasm a number) |
| M2 | Medium | Corpus→py/ts validation is transitive-through-kernel only; a corpus-only edit isn't re-run against py/ts in affected CI, and the `parity-oneway` guard case asserts Moon behavior the spec doesn't verify |
| M3 | Medium | Once real logic lands, kernel *correctness* rests entirely on proptest property quality — the hard part the harness scaffolds but doesn't solve |
| L1 | Low | Cross-workspace relative-path corpus reads hardcode the `rs/` layout into py+ts tests |
| L2 | Low | Drift-guard `cargo run` from repo root is fine only because it's crate-scoped — keep it off `--workspace` (macOS cdylib link trap) |
| L3 | Low | Confirm the `kernel->bindings` baseline already has `paigasus-wasm-rs` before adding parity-rs (it does now); rebase if this branch predates SMA-427 |

---

## High

### H1 — three of the four replays can pass without comparing anything

Decision #6 and the Rust replay component specify a structural guard — "non-empty, every case
inside the parity domain" — for the **Rust** replay. The Python and TypeScript replays (the
components that actually prove cross-*language* parity, the whole point) get no such guard: they
"load `sum.json`, iterate, assert per case." That's the classic safety-net trap — if the corpus
fails to load or comes back empty, an iterate-and-assert test has nothing to assert and goes green.

And the most probable failure mode is exactly a load failure. The corpus lives under
`rs/crates/libs/paigasus-kernel-parity/vectors/`, and py/ts reach it by walking relative paths
across workspace boundaries (decision #4). Relative-path resolution is fragile across pytest's
rootdir, vitest's cwd, and CI working directories (L1). Depending on how the load is written, a bad
path yields either a hard error (fine) or — via `pytest.mark.parametrize` over an empty list (pytest
*skips* with "empty parameter set") or a vitest loop that registers zero `it()`s — a **soft green**.
A parity harness that reports success while comparing zero cases is worse than no harness, because it
manufactures false confidence right before real domain logic ships.

**Recommendation:** make every language replay assert, independently of the per-case loop, that the
corpus loaded and contains the expected number of cases (or matches a committed count / content
hash) — promote the Rust replay's non-empty guard to a cross-language invariant. A mismatch between
"cases the generator wrote" and "cases this binding replayed" must fail red. Cheap, and it closes the
only way this harness can lie.

### H2 — the drift guard is built on a PRNG that isn't reproducible across versions

Decision #2 and the generator component hinge on a **byte-stable** committed corpus: the
`repo:parity-corpus-drift` task regenerates `vectors/sum.json` and `git diff --exit-code`s it, so any
nondeterminism in generation reds the guard. But the generator's random sample uses
`rand`'s `StdRng::seed_from_u64`, and `StdRng` is **explicitly documented as not reproducible across
`rand` releases** — "the algorithm … should not be considered reproducible … possible replacement in
future library versions," and in fact `StdRng`'s backing implementation has already changed (it
moved off `rand_chacha`). So a routine `cargo update` that bumps `rand` can silently change the
generated sequence; the next drift-guard run regenerates a *different* corpus and fails
`git diff --exit-code` — on a dependency bump that has nothing to do with the kernel, with a diff
that looks alarming (every random row changed) and no obvious cause. That's a confusing,
recurring-by-construction false red in the exact guard meant to be a trustworthy tripwire.

**Recommendation:** use a PRNG with a documented stability guarantee — `rand_chacha::ChaCha8Rng`
(or `ChaCha20Rng`), which `rand` itself recommends "for a secure reproducible generator" — seeded
explicitly; and/or pin `rand`/the PRNG crate to an exact version in the parity crate. Better still,
consider whether the random sample earns its keep at all: a *deterministic enumerated* sample
(e.g., a fixed lattice of `(a,b)` across the i32-safe domain plus the curated edges) removes the PRNG
reproducibility risk entirely and is just as reviewable. Randomness already lives where it belongs —
the in-process proptest (decision #2) — so the committed corpus arguably doesn't need a PRNG.

---

## Medium

### M1 — "same observable result" is actually "same value after per-language normalization"

The Goal says prove every binding produces "the same observable result." What the harness actually
asserts (decision #3) is: py `sum_as_string(a,b) == str(expected)`; napi/wasm `sum(a,b) === expected`;
rust `sum(a,b) == expected`. Those are three *different* observable surfaces normalized to a common
value: the Python binding returns a **string** (`sum_as_string`, confirmed in the live
`test_ffi_roundtrip.py`), while node/wasm return a **number**. So the harness validates decoded-value
equality, not surface parity — and it does so by encoding the known surface divergence into the
comparison.

That's a defensible scope for a placeholder, but it quietly tolerates a real ADR-0005 tension: the
bindings do *not* share a surface today (string vs number), and the harness is being built on top of
that inconsistency rather than flagging it. Worth noting that the TS side already has a
surface-parity guard — `binding-parity.types.ts` asserts the napi and wasm `sum` types are mutually
assignable (the SMA-427 M5 guard) — but **Python is entirely outside it**, both because it's a
different language and because its surface genuinely differs. When real domain logic arrives with
richer return types (structs, typed errors), "normalize then compare" will keep masking surface drift
across languages, which is precisely the drift ADR-0005 exists to prevent.

**Recommendation:** state explicitly in the spec that parity here means *decoded-value equality*, not
*surface identity*, and that unifying the surfaces (the deferred L5 — including retiring
`sum_as_string` in favor of a numeric/typed return) is a real prerequisite for real domain logic —
arguably more load-bearing than the harness itself. At minimum, track the Python surface in the same
"surfaces must match" discipline `binding-parity.types.ts` gives the TS bindings.

### M2 — a corpus-only change isn't validated against py/ts in affected CI, and the guard case assumes unverified Moon behavior

The spec adds `vectors/sum.json` to the `inputs` of `paigasus-kernel-py:test` and
`paigasus-kernel-ts:{build,test}` (cache-keying), but also adds a `parity-oneway` strict-equality
case asserting that editing `paigasus-kernel-parity/src|vectors` affects **only**
`paigasus-kernel-parity-rs`. Those two statements are in tension, and which one holds depends on Moon
semantics the spec doesn't pin down. The parity crate has **no downstream dependents** (nothing
`dependsOn` it), so py/ts can only become "affected" by a corpus edit via the cross-project `inputs`
entry. Two outcomes:

- If cross-project `inputs` *do* confer project-affected, then a corpus edit marks py/ts affected, and
  the `parity-oneway` expected set (`paigasus-kernel-parity-rs` only) is **wrong** → strict-equality
  guard reds on the unexpected py/ts.
- If they *don't* (the more likely Moon behavior — project-affected tracks the project `source` dir +
  upstream dependents, while `inputs` drive task hashing), then `parity-oneway` is correct, but a
  **corpus-only** change (a hand-edit, or a kernel change whose regen is committed separately) won't
  re-run the py/ts replays in affected CI — they're validated only when a *kernel* edit co-occurs
  (which cascades to py/ts) or on the full push-to-main run.

The normal workflow (kernel edit + corpus regen in one PR) is covered because the kernel edit
cascades. But the design's affected-graph story for the corpus itself is incomplete, and the spec
asserts the second outcome's guard case without confirming Moon behaves that way.

**Recommendation:** verify Moon's `query projects --affected` treatment of cross-project task inputs
at spike time, and reconcile: either make py/ts validation of a corpus change *direct* (a real
dependency edge so the cascade fires and the guard case lists py/ts), or accept the gap explicitly —
documenting that the corpus-vs-binding check rides on the kernel cascade + the drift guard + the full
push-to-main run, not on a corpus-only affected run.

### M3 — for real domain logic, the net is only as strong as the proptest properties

By construction the corpus bakes in whatever the kernel currently computes (`expected =
paigasus_kernel::sum(a,b)`), so the harness proves binding↔kernel fidelity, never kernel
*correctness* — correctness is the proptest's job. For `sum`, the proptest's `sum(a,b) == a+b` is a
complete independent oracle. For real domain logic there is no `a+b` to check against (re-deriving it
would be the reimplementation ADR-0005 forbids), so the proptest *properties* (commutativity,
identity, and whatever invariants the real function admits) become the **only** correctness check —
and weak or incomplete properties mean a correctness net full of holes, while every binding still
passes parity against a wrong-but-consistent kernel.

This isn't a flaw in SMA-433 — it's the honest boundary of what a parity harness can do — but the
spec frames the harness as *the* safety net for the jump to real logic, when in fact the proptest
property design (a per-function effort this issue only scaffolds for `sum`) is the harder, unsolved
half.

**Recommendation:** add a sentence making the boundary explicit: parity catches *binding/FFI* drift;
*kernel* correctness depends entirely on proptest property quality, which is per-function work due
when each real function lands. Consider a lightweight checklist ("every kernel fn ships with
properties that pin its behavior, not just its parity") so the gap is owned rather than assumed
covered.

---

## Low

**L1 — cross-workspace path coupling.** py "walks up to the parity crate" and ts uses a relative
`new URL(...)` into `rs/`. Both hardcode the `rs/` layout into the py and ts test suites; an `rs/`
reorg breaks them, and (per H1) a silently-wrong path can soft-pass. It's house-consistent (vitest
already aliases into `rs/`), but centralize the corpus path (a single resolved constant per language)
and pair it with the H1 non-empty assertion so a bad path fails loudly.

**L2 — drift-guard cargo invocation.** `cargo run --manifest-path rs/Cargo.toml -p
paigasus-kernel-parity …` launched from the repo root won't pick up `rs/.cargo/config.toml`'s
apple-darwin link flags — harmless here because the parity crate is a normal lib+bin (no cdylib, no
undefined FFI symbols), so it's crate-scoped and safe. Keep it crate-scoped: if it ever broadens to
`--workspace`, it hits the macOS cdylib link failure that `rs/.cargo/config.toml`'s own comment warns
about.

**L3 — guard baseline.** The spec says `kernel->bindings` "gains `paigasus-kernel-parity-rs`." The
live `run.sh` baseline already includes `paigasus-wasm-rs` (SMA-427 landed), so SMA-433 should add
exactly one entry. If this branch was cut before SMA-427 merged, rebase first so the parity edit
doesn't accidentally drop `paigasus-wasm-rs` from the set.

---

## What's solid (so it isn't lost in the critique)

- **Kernel-as-oracle (decision #1)** is the correct, future-proof model, and the rejection of
  collect-and-compare and per-language property re-encoding is well reasoned — re-encoding properties
  per language would itself violate kernel-once.
- **The ADR-sentence decomposition (decision #2)** — proptest for "against the Rust impl," committed
  corpus for "each binding" — maps the Development Guidelines requirement cleanly onto two mechanisms
  with the right reproducibility properties (randomness in-process, frozen vehicle cross-binding).
- **Crate hygiene (decision #5)** keeps `paigasus-kernel` pure and `proptest` a dev-dep; consistent
  with ADR-0005 and the existing binding-crate shape.
- **The affected-graph update** mirrors the existing `binding-oneway` cases faithfully, and the
  one-file-per-function corpus layout (decision #4) scales without restructuring.
- **Sequencing instinct is right** — landing this before the first real domain logic is exactly when
  the net must exist, and the repo is ready for it.

## Suggested spec edits before "ready to plan"

1. Require a non-empty / expected-count (or hash) assertion in *every* language replay, not just Rust
   (H1).
2. Replace `StdRng` with `rand_chacha::ChaCha*Rng` (or a deterministic enumerated sample) and pin it
   (H2).
3. State that parity = decoded-value equality, not surface identity; fold the Python `sum_as_string`
   surface into the L5 unification and a surface guard (M1).
4. Verify Moon's cross-project-input affected semantics and reconcile the `parity-oneway` case vs the
   corpus→py/ts coverage gap (M2).
5. Add the "kernel correctness depends on proptest property quality" boundary note (M3).
6. Centralize the cross-workspace corpus path per language (L1).
