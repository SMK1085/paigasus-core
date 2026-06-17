# Review — SMA-430 FFI freshness: symmetric py wheel-guard fix

**Reviews:** [`2026-06-17-sma-430-ffi-freshness-py-design.md`](./2026-06-17-sma-430-ffi-freshness-py-design.md)
**Reviewer perspective:** staff engineer
**Date:** 2026-06-17
**Sources cross-referenced:** Linear SMA-430 (+ SMA-419/420), and the live `py/packages/paigasus-kernel/moon.yml` (current) + `ts/packages/paigasus-kernel/moon.yml` (the fix template).

## Verdict

Correct, tightly scoped, and well-verified — ship it. This is the symmetric Python counterpart to the ts/napi freshness fix from SMA-420, and it directly closes the stale-artifact concern the SMA-419 (F4) and SMA-420 (F1) reviews raised — a concern the SMA-420 code review then *reproduced* (`sum(2,3) → 6` against an `a + b` kernel). The root-cause analysis is precise (`--reinstall-package` rebuilds the *wheel* but maturin's cargo is mtime-incremental, so a warm `rs/target` + a git mtime-inversion serves a stale `.so` inside a fresh wheel), the two-layer fix is right (Moon content-hash `inputs` so the task *runs* on a kernel edit + a `touch` so cargo *recompiles* when it runs), and it mirrors the ts template faithfully. The verification plan — a trap test that proves the unpatched task serves a stale wheel and the patched one doesn't, plus a real-regression check — is exactly how a freshness guard should be tested.

The findings are minor and mostly inherited from the ts template it (correctly) mirrors.

## What the spec gets right (calibration)

- **Closes the prior-review concern, symmetrically.** I confirmed the live py `test` task (`uv sync --reinstall-package … && uv run pytest tests`) has **no** touch and inputs of only `tests/**`/`src/**`/`pyproject.toml`/`uv.lock` — so it's genuinely exposed to the same stale-wheel bug the ts side fixed. The spec's diagnosis is accurate.
- **Both layers, correctly identified as orthogonal and both required.** The `inputs` additions key Moon's *content* hash (so a kernel-only edit re-runs the task rather than serving a cached green — the F4 gap), and the `touch` defeats *cargo's* mtime fingerprint (so the recompile actually happens when the task runs). They're coupled: the inputs make the task run, which fires the touch. The spec gets this.
- **Faithful mirror of the ts template.** Verified the py `touch`/inputs match the ts side's shape: touch both crates' `lib.rs`, add `/rs/.../kernel/src/**` + `/rs/.../<binding>/src/**` + the binding `Cargo.toml` + the binding manifest (`pyproject.toml` for py, the maturin analog of ts's `package.json`). `touch`-not-`cargo clean -p` is the right host-agnostic call (rejecting per-triple `cargo clean` for the documented reason — maturin's per-triple subdir).
- **`test`-only is correctly reasoned.** Confirmed the py `build` task is `deps: ['^:build']` with no script — it neither materializes nor asserts the wheel, so only `test` can assert against a stale artifact. The intentional asymmetry with the ts side (which touches in *both* build and test, because ts `build` produces the `.node`) is real and flagged for an inline comment.
- **Sound CI deferral.** The argument that the inversion is a local-only risk — CI's `actions/checkout` writes sources at "now," newer than `actions/cache`-restored (tar-mtime-preserved) artifacts, so cargo always recompiles — is correct, so the touch is load-bearing locally and belt-and-suspenders on CI. Deferring the broader `rs/target` cache policy (and non-FFI cargo-task protection) is the right scope.

## Findings

### F1 — [Low] Inputs don't re-key on an FFI-dependency bump (shared with the ts template, not py-specific)

Neither guard re-keys when the FFI dependency itself changes version: `pyo3`/`napi` are declared in `rs/Cargo.toml` `[workspace.dependencies]` (the binding's own `Cargo.toml` just says `pyo3.workspace = true`), and the *resolved* version lives in `rs/Cargo.lock` — none of which are task inputs. So a `pyo3` bump that altered the FFI value mapping wouldn't re-run the guard. The practical risk is low (a patch bump rarely changes integer marshalling), and the obvious fix (add `rs/Cargo.lock`) is noisy — it would re-key the guard on *any* workspace dep change. The py spec correctly mirrors the ts template's inputs exactly, and the ts notes already reason that the binding's behavior is determined by {rust src, tool version (lockfile), binding manifest} — for py that's {rust src, maturin version (in `uv.lock`, listed), `pyproject.toml` (listed)}, so the maturin-version axis *is* covered. Only the underlying FFI-crate version axis isn't. **Recommendation:** note it, don't block; if you ever tighten this, do it on **both** guards to preserve the symmetry that's the whole point of this issue.

### F2 — [Low] The per-guard `touch` compounds as the FFI guards multiply

Both the py guard (this) and the ts guard now `touch` the *same* `rs/crates/libs/paigasus-kernel/src/lib.rs` before their builds. In a full `moon ci`, each guard's touch bumps the shared kernel mtime, which can trigger redundant kernel/binding recompiles in sibling cargo tasks (`paigasus-kernel-rs:test`, `paigasus-py-bindings-rs:build`, `paigasus-node-bindings-rs:build`). cargo's target-dir lock serializes them (safe), so this is correctness-preserving — but it compounds as guards multiply (py, ts, and the deferred wasm). It's the same "double-compile" theme as SMA-419 F2 / SMA-420, now with touch-induced churn layered on. **Recommendation:** nothing to change here (the ts precedent set this, and per-guard touch is the right local fix), but flag it as the concrete reason the deferred broader `rs/target` freshness policy (Out of scope) is likely the cleaner long-term consolidation than N per-guard touches — when wasm lands, that's three guards touching the same source.

### F3 — [Nit] The CI-safety deferral rests on the cache action's mtime behavior

The "no inversion on CI" argument is correct *given* that `actions/cache` (tar) preserves restored-artifact mtimes and `checkout` writes sources at "now." Both hold today. Worth one line in the spec/comment noting the deferral's safety is contingent on that caching behavior — if the `rs/target` restore strategy ever changes (e.g. a restore that re-times files to "now"), the CI inversion reappears and the touch becomes load-bearing in CI too, not just locally. Keeps the deferral's justification honest as the cache setup evolves.

## Bottom line

Land it — accurate diagnosis, the correct two-layer fix (Moon inputs + cargo touch), a faithful and verified mirror of the ts template, and a trap test that genuinely proves the false-green is caught. The findings are all low: the FFI-dep-version input gap is shared with the ts template (tighten both or neither, F1), the per-guard touch churn is the reason the deferred `rs/target` policy is the eventual consolidation (F2), and the CI-safety deferral is contingent on the cache action preserving mtimes (F3). None block.

## Sources

- Spec under review: `docs/superpowers/specs/2026-06-17-sma-430-ffi-freshness-py-design.md`
- [Linear SMA-430 — FFI freshness, symmetric py fix](https://linear.app/smaschek/issue/SMA-430/ffi-freshness-fix-the-cargo-mtime-stale-artifact-caveat-symmetrically) (follow-up of SMA-420; related SMA-419)
- Repo: `py/packages/paigasus-kernel/moon.yml` (current `test`: `uv sync --reinstall-package … && uv run pytest tests`, no touch, inputs missing the rust sources — the gap this fixes), `ts/packages/paigasus-kernel/moon.yml` (the template: `touch …/kernel/src/lib.rs …/node-bindings/src/lib.rs && …` in both build+test, with the rust-src + binding-manifest inputs the py side mirrors)
