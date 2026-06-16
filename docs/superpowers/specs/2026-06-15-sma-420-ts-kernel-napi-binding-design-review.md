# Review — SMA-420 TS kernel binding (napi-rs)

**Reviews:** [`2026-06-15-sma-420-ts-kernel-napi-binding-design.md`](./2026-06-15-sma-420-ts-kernel-napi-binding-design.md)
**Reviewer perspective:** staff engineer
**Date:** 2026-06-15
**Sources cross-referenced:** Linear SMA-420 (+ SMA-409/419/389), ADR-0005, Notion Scoping §1/§3, and the live `rs/crates/bindings/*`, `rs/.cargo/config.toml`, `ts/packages/paigasus-kernel/*`, git log.

## Verdict

Strong — and visibly shaped by the prior reviews in this series. It's the Node mirror of SMA-419, and it correctly mirrors the layout SMA-419 actually *landed*: I confirmed SMA-419 shipped with the **co-located** binding (maturin `pyproject.toml` inside `rs/crates/bindings/paigasus-py-bindings/`, consumed via a `path` source) — the Scoping §1 fallback the SMA-419 review recommended — and SMA-420 puts `@paigasus/node-bindings`'s `package.json` in the same co-located spot, consumed via a `file:` specifier. So the "structurally symmetric with the Python side" claim is true, the two FFI bindings share one layout, and the spec explicitly carries forward the SMA-419 lessons (double-compile accepted, cargo-on-PATH and cache-bust spike checks, `cargo-machete ignored`). Implement it after the spike.

The findings are light. The one worth nailing is a real Python↔Node asymmetry the "mirror SMA-419" framing can paper over: `vitest` does not build the `.node`, whereas `uv run` builds the wheel — so the test→build ordering needs explicit wiring it didn't on the Python side.

## What the spec gets right (calibration)

- **Co-located layout, genuinely symmetric.** Verified: SMA-419 landed co-located (`rs/crates/bindings/paigasus-py-bindings/pyproject.toml` + `[tool.uv.sources] … { path = "../../../rs/…" }`), not the separate `py/packages` layout its draft proposed. SMA-420 mirrors that exactly (`package.json` in `rs/crates/bindings/paigasus-node-bindings/`, `file:` link from `@paigasus/kernel`). The spec's "take the documented layout, don't invent a third one (SMA-419 F1 lesson)" is applied correctly.
- **Link flags reused correctly.** `rs/.cargo/config.toml` carries `-undefined dynamic_lookup` for both apple-darwin targets (added for PyO3 in SMA-409), and a napi cdylib needs the same (N-API symbols resolve at load, exactly like libpython). Reusing it + broadening the comment is right, and the file's existing "run cargo from inside `rs/`" warning is exactly the spike's check #1.
- **The series' lessons are baked in:** double-compile accepted deliberately (SMA-419 §6), `[package.metadata.cargo-machete] ignored = ["napi"]` for the macro-only false-positive (the SMA-409/375 pattern), and spike checks for cargo-on-PATH and Rust-edit cache-bust (SMA-419 F3/F4). This reads like a spec written *with* the review history.
- **Honest intermediate state.** Decision #1 calls out that `@paigasus/kernel` becomes Node-only until `paigasus-wasm` lands; verified safe today (no ts package consumes `@paigasus/kernel`; it's an `export {}` stub).
- **Thoughtful parity note (decision #5):** diverging to an idiomatic `number` on Node vs `str` on Python is correct — ADR-0005's cross-binding parity is about the kernel *operation*, not the FFI return type. And the ADR-0005 note (napi-first, wasm tracked) is the right lightweight way to satisfy AC #4 without a new ADR.

## Findings

### F1 — [Medium] `vitest` won't build the `.node` the way `uv run` builds the wheel — the test→build ordering needs explicit wiring

This is the one place the "mirror SMA-419" symmetry breaks. On the Python side, `paigasus-kernel-py:test` runs `uv run pytest`, and `uv run` *itself* builds/syncs the wheel (maturin) before the test — so test-after-build is implicit. On the Node side, `vitest` does **not** build the `.node`; the `napi build` is a separate step (the spec puts it in the `paigasus-kernel-ts` build per §6). So `paigasus-kernel-ts:test` (vitest, importing `@paigasus/kernel` → the `.node`) must run **after** the napi-build step, or the import resolves to a missing/stale addon.

The spec gives `paigasus-kernel-ts:test` `deps: ['^:build']` — but `^:build` is *upstream dependencies'* builds (`paigasus-node-bindings-rs:build` = plain `cargo build`, which produces the rlib/cdylib in `rs/target`, **not** the `index.node`). It does **not** guarantee `paigasus-kernel-ts`'s **own** build (where `napi build` produces the `.node`) ran first. Moon doesn't auto-order a project's test after its own build unless declared. So as written, `kernel-ts:test` could run before the `.node` exists → import failure.

**Recommendation:** make `kernel-ts:test` depend on the napi-build step (the own-project build, e.g. a `~:build`/explicit task dep), not just `^:build`. The §6 spike already flags "which task runs `napi build` and from which cwd" — extend it to nail the **test-depends-on-the-napi-build** edge, since this is the concrete asymmetry with the Python side (`uv run` auto-builds; `vitest` doesn't). It's the most likely "green locally, fails in a clean CI ordering" trap here.

### F2 — [Low] The wrapper diverges from the Scoping §3 conditional-exports design; set up the `node` condition now

Scoping §3 shows `@paigasus/kernel` as the canonical place for **conditional exports** (`node` → napi, `browser`/`workerd` → wasm), so a consumer gets the right binding per runtime. SMA-420 instead re-exports unconditionally (`export { sum } from "@paigasus/node-bindings"`), which is Node-only and — because it pulls a `.node` — will make a browser bundler (Next.js client) **choke** rather than fail cleanly. When `paigasus-wasm` lands, the wrapper has to be restructured to §3's conditional `exports`. **Recommendation:** set up the `exports` map with a `node` condition now (even if the `browser`/`default` branch just throws a clear "not available until wasm" error), so the browser path is a legible failure rather than a bundler explosion, the §3 design is honored, and the wasm follow-up slots in without reworking the wrapper. Low cost, and it's the documented target shape.

### F3 — [Low] Confirm napi-rs maps `i64` → JS `number` (not `BigInt`) in the pinned version

The smoke test asserts `sum(2, 3)).toBe(5)` — a `number`. napi-rs documents `i64` → JS `number` (lossy beyond 2^53), so the spec is *probably* right, but some configurations/versions surface `i64` as `BigInt`, in which case `sum(2,3)` returns `5n` and `5n !== 5` fails the test silently. **Recommendation:** confirm the `i64`→`number` mapping for the pinned napi-rs in the spike (it's the kind of thing the smoke test exists to catch but the spec asserts in the wrong direction would still fail); if it's `BigInt`, narrow the binding return to `i32` (→`number`) or assert `5n`.

### F4 — [Low] The guard's forbid-regex is now a hand-maintained enumeration on its third revision

The kernel-touch forbid-regex has grown from a blanket `-ts$` (SMA-409) → narrowed (SMA-419) → an explicit enumeration of *every* ts package except `paigasus-kernel-ts` (this spec), plus enumerated `-py` exclusions. Each new ts/py package must now be hand-added to this regex or it's **silently unasserted** — there's no completeness meta-check. This is the recurring negative-assertion maintenance smell flagged in the SMA-409/419 reviews, now compounding. **Recommendation (guard refinement, can be its own follow-up):** add a completeness check — assert every project is either in must-include or matched by the forbid-regex (fail on an unclassified project) — or move to a default-deny "affected set equals must-include" assertion, so a new package can't slip through unasserted as the enumeration grows.

## Bottom line

Land it — the co-located layout correctly mirrors what SMA-419 actually shipped (the two FFI bindings are genuinely symmetric), the link-flag reuse and machete ignore are right, and the spec applies the prior reviews' lessons directly. In the spike, beyond the spec's four checks, nail the **test-depends-on-the-napi-build** ordering (F1) — it's the real Python↔Node asymmetry, since `vitest` won't build the `.node` the way `uv run` builds the wheel. Set up the §3 `node`-conditioned `exports` now so the browser path fails cleanly and the wasm follow-up slots in (F2), and confirm the `i64`→`number` mapping (F3). The guard's growing forbid enumeration is worth consolidating soon (F4).

## Sources

- Spec under review: `docs/superpowers/specs/2026-06-15-sma-420-ts-kernel-napi-binding-design.md`
- [Linear SMA-420 — stand up a TS kernel binding (napi-rs)](https://linear.app/smaschek/issue/SMA-420/stand-up-a-ts-kernel-binding-wasmnapi-wire-the-cascade-to-paigasus) (follow-up of SMA-409; mirrors SMA-419)
- [Notion — ADR-0005](https://www.notion.so/368830e8fbaa817e8184d8f22f4d487c) (napi/wasm hybrid; names the three binding crates) and [Polyglot Monorepo Scoping §1/§3](https://www.notion.so/368830e8fbaa8101b0ffded7a3de3b53) (co-located FFI layout; `@paigasus/kernel` conditional `node`/`browser` exports)
- Repo: git log (`6fc8b7c` SMA-419, `f0605d2` SMA-409 landed), `rs/crates/bindings/paigasus-py-bindings/pyproject.toml` (co-located — the layout SMA-420 mirrors) + `py/packages/paigasus-kernel/pyproject.toml` (`[tool.uv.sources] { path = "../../../rs/…" }`), `rs/.cargo/config.toml` (`-undefined dynamic_lookup`, apple-darwin, "run cargo from inside rs/"), `rs/crates/bindings/` (only `paigasus-py-bindings` today), `ts/packages/paigasus-kernel/{package.json,src/index.ts}` (`export {}` stub, source-only exports, no consumers)
