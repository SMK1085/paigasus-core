# Review — SMA-419 paigasus-kernel-py PyO3 wheel (uv↔maturin)

**Reviews:** [`2026-06-15-sma-419-paigasus-kernel-py-pyo3-wheel-design.md`](./2026-06-15-sma-419-paigasus-kernel-py-pyo3-wheel-design.md)
**Reviewer perspective:** staff engineer
**Date:** 2026-06-15
**Sources cross-referenced:** Linear SMA-419 (+ SMA-409), ADR-0005, the Notion Python guidelines + Polyglot Monorepo Scoping §1, and the live `rs/crates/bindings/paigasus-py-bindings/*` + `py/packages/*` + `ci/affected-graph/*`.

## Verdict

Sound, and it lands the deferred runtime half of SMA-409 the way that review recommended. I confirmed the foundation: SMA-409 shipped (`sum_as_string` calling `paigasus_kernel::sum`, the `abi3`/`extension-module` cdylib, the affected-graph guard, and — gratifyingly — the `[package.metadata.cargo-machete] ignored = ["pyo3"]` mitigation the SMA-409 review asked for). The two-package split (a maturin wheel package + a pure-`uv_build` `paigasus-kernel` that re-exports it) matches the Notion Python guidelines' "thin FFI wrapper — just re-exports from the native module" model, and decision #2 correctly *reverses* the issue's `.prototools` speculation: maturin is a PEP 517 backend, so pinning it in `[build-system].requires` (locked via `uv.lock`) is the right single-source-of-truth call. Implement it — after de-risking the build integration, which the spec rightly flags as the headline risk.

The findings are about that integration: the chosen package layout takes on exactly the cross-directory risk the canonical guidance documented a way to avoid, and the wheel build double-compiles the crate in a way that echoes the SMA-391 build-collision class.

## What the spec gets right (calibration)

- **Closes the SMA-409 deferrals as designed.** This *is* the wheel-integration issue F4 deferred the runtime smoke test to, and §5 implements the F5 guard revision (move `kernel-py`/`py-bindings-py` from forbid to must-include; narrow the forbid-regex but keep the unrelated py packages forbidden). Both explicitly referenced.
- **maturin home is correct (decision #2).** `[build-system] requires = ["maturin>=1.7,<2"]` locked in `uv.lock`, uv driving the build — reversing the issue's `.prototools` guess. A standalone maturin CLI would have added a second pinning system; this keeps `uv.lock` authoritative. Good judgment, better than the issue assumed.
- **Two-package separation matches the canonical model.** The Python guidelines explicitly classify `paigasus-kernel` as a "thin FFI wrapper — just re-exports from the native module," so a compiled wheel package + a pure re-export wrapper is the sanctioned shape. Decision #3 (own the provisional `paigasus_py_bindings` name via `[tool.maturin] module-name` + re-export, no rename) is clean.
- **Guard extension is coherent** — adding the `binding-rs` touch case with the one-way assertion (must reach `py-bindings-py`/`kernel-py`, must **not** drag in `paigasus-kernel-rs`) is the right completeness move, and `--negative-control` staying red is preserved.
- **"De-risk first" posture.** Flagging the uv↔maturin build isolation as a throwaway spike before anything else is the correct sequencing for the fiddliest part.

## Findings

### F1 — [Medium] The chosen layout takes on the exact cross-directory risk the canonical guidance documented a way to avoid

The spec's own "primary risk" is "uv↔maturin build isolation with an out-of-tree `manifest-path`" (`manifest-path = "../../../rs/crates/bindings/.../Cargo.toml"`). That is precisely the concern Polyglot Monorepo Scoping §1 named: *"maturin's cross-directory workspace support has historical sharp edges — if you hit them, the fallback is to give `paigasus-py-bindings` its own thin `pyproject.toml` **inside** `rs/crates/bindings/paigasus-py-bindings/` and let the Python package depend on the wheel directly."* The §1 fallback avoids the cross-directory `manifest-path` by **co-locating** the maturin pyproject with the crate (maturin builds in-place, no reach across trees).

SMA-419 instead invents a third layout — a new `py/packages/paigasus-py-bindings/` with a cross-directory `manifest-path` into `rs/` — and doesn't reference the §1 guidance or weigh the co-located fallback, even though it self-identifies that `manifest-path` as the thing most likely to fail. To be fair, the choice is defensible: co-locating would instead require uv to reference a workspace member *outside* `py/packages/*`, so both layouts carry a cross-tree reference — the question is just which tool reaches across (maturin's `manifest-path` here, vs uv's member path in the fallback). But that trade isn't surfaced, and the spec picks the option whose failure mode is the documented one. **Recommendation:** reconcile with the canonical maturin guidance — confirm **ADR-0006** (cited as governing "Python packaging") actually sanctions this layout, and if not, weigh the §1 co-located fallback explicitly, noting the manifest-path-vs-uv-member trade so the choice reads as deliberate rather than a third path around the documented one.

### F2 — [Medium] The wheel build double-compiles the binding crate — and may contend on cargo's target lock

In a full `moon ci :build`, `paigasus-py-bindings-rs:build` runs `cargo build` (the extension-module cdylib) **and** `paigasus-py-bindings-py:build` runs maturin → cargo (the wheel) — both compiling the same crate (plus the kernel). The spec acknowledges maturin compiles the crate ("the `dependsOn` edge is primarily for the cascade rather than strict build ordering") but doesn't address the consequences:

- **Double compilation** of the binding crate + kernel per CI build — a real time cost.
- **Possible cargo target-lock contention.** If Moon schedules `py-bindings-rs:build` (cargo) and `py-bindings-py:build` (maturin→cargo) concurrently against the same `rs/target/`, cargo serializes on its target-dir lock (safe, but the second blocks) — or maturin uses its own target dir (no lock contention, but double disk + double compile). Either way it's the same *two-builders-into-one-artifact-area* class as the SMA-391 `.next`-lock collision, in cargo form.

**Recommendation:** in the spike, confirm maturin's target dir vs `rs/target/` and whether the two builds contend, and decide deliberately — accept the double-compile as the cost of a uniform `:build` gate, share/separate the target dir intentionally, or reconsider whether `py-bindings-rs:build` is still needed for compile-checking once maturin compiles the crate anyway. At minimum, name it; right now it reads as free.

### F3 — [Low] cargo-on-PATH inside uv's build isolation is an unstated sub-dependency of the headline risk

The "primary risk" is framed as `manifest-path` resolution, but there's a second cross-tool dependency in the same step: maturin runs inside uv's *isolated* PEP 517 build env, and it shells out to **cargo** — which uv's build isolation does **not** provide. So `cargo` (and the rust toolchain) must be on the **system** PATH when `uv sync` triggers the maturin build. In CI it should be (moon setup installs the rust toolchain before the py tasks), and locally after `proto install` — but it's exactly the kind of cross-tool-PATH assumption that breaks, and it's distinct from the `manifest-path` concern the spec names. **Recommendation:** add "cargo reachable from maturin inside uv's build isolation" to the spike's explicit checks.

### F4 — [Low] Confirm CI doesn't serve a stale wheel when only the Rust source changes

The spec correctly notes editable installs won't auto-recompile on a Rust edit (re-`uv sync` needed) and calls this "irrelevant to CI's clean build." Worth confirming that's actually true given SMA-361's CI caches the **uv cache**: if a cached editable install survives across runs, a kernel/binding Rust edit could be tested against a **stale compiled wheel** unless the affected-graph cascade forces `py-bindings-py:build` to re-run maturin (not just re-resolve uv). This is the affected-graph correctness the whole arc is about — a kernel edit must invalidate the *maturin build*, not merely the uv resolution. **Recommendation:** verify in the spike that a Rust-source change actually triggers recompilation in CI (the cascade edge must bust the maturin build cache), so the runtime smoke test isn't asserting against last run's wheel.

## Bottom line

Land it — the deferred SMA-409 runtime half is the right next step, the two-package split matches the canonical thin-wrapper model, and pinning maturin via `uv.lock` (not `.prototools`) is the correct call. Do the spike first, and broaden it beyond `manifest-path`: reconcile the package layout with the documented §1 maturin guidance / ADR-0006 (F1), confirm whether the wheel build double-compiles and contends on cargo's target lock (F2), check cargo is reachable from maturin inside uv's build isolation (F3), and verify a Rust edit actually busts the wheel build in cached CI (F4). The guard revision and the no-rename re-export are both well-judged and need no change.

## Sources

- Spec under review: `docs/superpowers/specs/2026-06-15-sma-419-paigasus-kernel-py-pyo3-wheel-design.md`
- [Linear SMA-419 — wire paigasus-kernel-py to the PyO3 wheel](https://linear.app/smaschek/issue/SMA-419/wire-paigasus-kernel-py-to-the-pyo3-wheel-uvmaturin-runtime-smoke-test) (follow-up of SMA-409)
- [Notion — Python guidelines](https://app.notion.com/p/368830e8fbaa8182b5e7fe4400e92c9b) ("`paigasus-kernel` — just re-exports from the native module") and [Polyglot Monorepo Scoping §1](https://www.notion.so/368830e8fbaa8101b0ffded7a3de3b53) (maturin cross-directory caveat + the co-located thin-pyproject fallback)
- Repo: `rs/crates/bindings/paigasus-py-bindings/{Cargo.toml,src/lib.rs}` (SMA-409 landed: `pyo3`+`paigasus-kernel` deps, `sum_as_string`, `test=false`, **`cargo-machete ignored=["pyo3"]`** — SMA-409 F3 implemented), `rs/crates/libs/paigasus-kernel/src/lib.rs` (`pub fn sum`), `py/packages/paigasus-kernel/pyproject.toml` (`uv_build`, `ADR-0006` referenced), `ci/affected-graph/{run.sh,README.md}` (the guard to extend)
