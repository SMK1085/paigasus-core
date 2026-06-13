# Review — SMA-389 proto build-graph wiring + first real proto

**Reviews:** [`2026-06-13-sma-389-proto-build-graph-wiring-design.md`](./2026-06-13-sma-389-proto-build-graph-wiring-design.md)
**Reviewer perspective:** staff engineer
**Date:** 2026-06-13
**Sources cross-referenced:** Linear SMA-389 (+ SMA-360/374/401/388), ADR-0004, the live `contracts/buf.gen.yaml` + proto packages + `ci.yml` + git log.

## Verdict

This is the codegen capstone, and it lands well. It closes all three high-severity findings from the SMA-360 review at once — the `bytes=.`/`no_include`/`import_extension=.js` plugin opts (H1) are already in `buf.gen.yaml`, `clean: true` (H2) is added here, and the orphan `contracts:generate` edge (H3) is finally wired — and it does the right thing by landing a *real* gRPC `HealthService` (exercising both prost messages and tonic stubs) rather than wiring against an empty `generate`. The edge mechanism (task-level `deps`, which both order generation and propagate affected-detection) is correctly chosen, and generated code is kept out of all three lint/fmt gates while staying typecheck-compiled. Implement it.

But there's a determinism gap underneath the whole committed-codegen story that this issue is the right place to fix and currently doesn't: **the remote codegen plugins are unpinned.** Combined with `clean: true` + build-time regeneration, that undermines the very reproducibility the spec depends on — and the drift guard that would catch the resulting gaps is deferred. Those two things (F1, F2) are what I'd resolve before merge.

## What the spec gets right (calibration)

- **Resolves the SMA-360 codegen trilogy.** H1 (the dropped plugin opts) is already implemented in `buf.gen.yaml` (verified: `bytes=.`/`file_descriptor_set`, `no_include`/`compile_well_known_types`, `target=ts`/`import_extension=.js`); H2 (`clean: true`) is added here with the real protos; H3 (the orphan `contracts:generate`) is wired. Full circle.
- **Lands a real, well-chosen proto.** A gRPC `HealthService` exercises the fuller prost+tonic path (riskier than a messages-only `common/v1` type), so AC #3 ("a proto edit triggers regen + downstream rebuilds") is actually meaningful. Names satisfy buf `STANDARD` lint.
- **Correct edge mechanism.** Task-level `deps: ['contracts:generate']` both orders generation before build *and* makes Moon treat `contracts` as a project-dep for affected propagation. The rejected alternatives are correctly rejected — project-level `dependsOn: contracts` has nothing to bind `^:build` to (contracts exposes no `build`), and global `^:build` is out of scope (and was, correctly, *not* landed by SMA-374).
- **Generated code stays out of the gates, in all three languages** — clippy `allow` + rustfmt `ignore` (Rust), ruff + basedpyright `exclude` (Py), eslint `ignores` + `.prettierignore` (TS) — while remaining typecheck-compiled and smoke-tested. Thorough and correct.
- **buf-on-PATH (SMA-360 M3) is actually already de-risked.** `contracts:lint`/`:breaking` (both buf) already run in CI via the `:lint`/`:breaking` targets, so buf-on-PATH is proven working today (`ci.yml` does `proto install` → `moon setup` before `moon ci`). The new edges just add more buf calls on an already-functioning PATH; the spec frames M3 as "now due" but it's already mitigated. Keep the verification, but the risk is lower than stated.

## Findings

### F1 — [High] The remote codegen plugins are unpinned — the determinism the whole issue rests on isn't there yet

`buf.gen.yaml` references all four plugins without a version tag (`remote: buf.build/community/neoeinstein-prost`, `…-tonic`, `…danielgtaylor-betterproto`, `bufbuild/es`), so buf resolves **latest** at generation time. With committed codegen + `clean: true` + build→generate, that's the root of three problems the spec circles but never names:

1. **Crate↔plugin skew (the footgun §4/§9 worries about) starts at the *plugin*.** The spec says "pin crate versions to the remote plugin output" — but if the plugin is unpinned, "the plugin output" is a moving target. Pin the prost/tonic crate to today's plugin, the plugin ships a new version next month, the next regen emits code expecting a different prost → skew, with no proto change.
2. **The deferred drift nightly will false-positive on every upstream plugin release.** Drift = "regenerate, `git diff`." With an unpinned plugin, a regen after any plugin update produces different bytes than the committed code *even though no `.proto` changed* — so the drift guard fires on plugin churn, not contract drift. Determinism is a prerequisite for drift detection to mean anything.
3. **CI build→generate compiles whatever-latest.** Because the build edge regenerates in CI (with `clean`), CI compiles the latest-plugin output, which may differ from both the committed diff a reviewer sees and the pinned crate.

Pinning the remote plugin versions (`remote: buf.build/community/neoeinstein-prost:vX.Y.Z`, etc.) is the missing foundation, and **this is the issue where it belongs** — it's where generated code first lands and where reproducibility first matters (and where SMA-388's publish flip, which this unblocks, will depend on it). **Recommendation:** pin all four plugins in `buf.gen.yaml` as part of this change, and pin the prost/tonic crates to match those exact plugin versions.

### F2 — [Medium] `clean: true` + build→generate makes CI validate regenerated code, not the committed artifact — and the drift guard is deferred

This is the SMA-374 review's F1, now live with real generated code. The build edge runs `contracts:generate`, and `clean: true` wipes the `out:` dirs first — so in CI, `moon ci :build` **regenerates over the committed code and compiles the fresh output**. Consequence: on a PR that edits a `.proto` but forgets to regenerate-and-commit, CI wipes the stale committed code, regenerates, builds green — and the **stale committed code merges uncaught** (CI never commits its regeneration). The diff a reviewer approves is not what CI built, undercutting ADR-0004's stated reasons for committing (reviewable wire diffs; build-without-prebuild). And the guard that would catch this — the codegen-drift check — is explicitly deferred to a follow-up nightly.

The fix is nearly free *because build already pays the expensive part*: since `contracts:generate` runs in the build graph and leaves regenerated files in the workspace, add a **PR-level** `git diff --exit-code` on the generated dirs immediately after generation. That *is* the drift check, it catches committed staleness at PR time, and it costs one step rather than a separate nightly. **Recommendation:** don't ship `clean: true` + build→generate + the first real committed generated code *without* that PR-level diff gate — the combination actively masks the drift while landing the artifact that makes drift possible. (Pairs with F1: the diff gate only works if regeneration is deterministic, i.e. plugins pinned.)

### F3 — [Medium] The py-root generate edge puts buf-generate on the hot path of every Python check (SMA-401 × SMA-389 interaction)

Decision #6 adds `contracts:generate` to `py:typecheck` and `py:test`. That's *forced* by SMA-401: it consolidated Python `typecheck`/`test` onto the `py` root (whole-tree, run on **any** py change), so there's no per-package `paigasus-proto-py` check to attach the edge to. The result: editing `paigasus-ml/src/foo.py` — a pure-Python change touching no proto — now triggers `contracts:generate` (buf, network) before basedpyright/pytest run. The spec accepts this as a PATH concern, but the real cost is buf-generate latency + network egress on the *most frequent* Python tasks, for proto-unrelated work. (The py *build* edge is fine — it's per-package on `paigasus-proto-py`; only the whole-tree typecheck/test are over-coupled.)

This is the SMA-401-consolidation amplifying SMA-389's edge. **Recommendation:** surface it as the genuine cost it is, and decide deliberately — either accept it, or treat the proto-py typecheck/test as the one justified exception to SMA-401's whole-tree model (a per-package proto-py check carrying the edge, leaving the root checks proto-free). At minimum it shouldn't be filed under "PATH is fine."

### F4 — [Low] Close the SMA-374 doc trail

SMA-374's *spec* described Model B wiring (`paigasus-proto-rs:build → contracts:generate` + `^:build` on `rust.yml`), but the landed SMA-374 (#32) dropped/deferred that — `paigasus-proto-rs/moon.yml` is bare today and `rust.yml` has no `^:build` (it kept only `build-release`). SMA-389 now wires the edge via task-level `deps` and explicitly rejects `^:build`. The net state is coherent (and dropping the edge from SMA-374 to land it here, scoped, is effectively what the SMA-374 review recommended) — but SMA-389 doesn't mention SMA-374 at all. A one-line "supersedes SMA-374's deferred Model B wiring approach" closes the trail so the next reader isn't confused by SMA-374's spec describing wiring that never shipped.

## Bottom line

Land it — it lands the first real contract, closes the SMA-360 codegen trilogy, and wires the affected graph correctly. Two things first, both about determinism and drift: **pin the remote codegen plugins in `buf.gen.yaml`** (F1 — without it, the crate-pin desyncs, the future drift check false-positives, and CI compiles a moving target), and **add a PR-level `git diff --exit-code` after generation** (F2 — build already regenerates, so the drift guard is nearly free and shouldn't be deferred while `clean: true` + build→generate actively mask committed staleness). Then make a deliberate call on the py-root generate edge's blast radius (F3), and note the SMA-374 supersession (F4).

## Sources

- Spec under review: `docs/superpowers/specs/2026-06-13-sma-389-proto-build-graph-wiring-design.md`
- [Linear SMA-389 — wire paigasus-proto build → contracts:generate](https://linear.app/smaschek/issue/SMA-389/wire-paigasus-proto-build-contractsgenerate-dependency-edges-when) (resolves SMA-360 H3; blocks SMA-388; related SMA-360)
- [Notion — Scoping §2 + ADR-0004](https://www.notion.so/368830e8fbaa8101b0ffded7a3de3b53) (committed codegen rationale: reviewable diffs, no prebuild, drift nightly)
- Repo: `contracts/buf.gen.yaml` (opts present — SMA-360 H1 closed; `clean` still omitted with a comment deferring to this PR; **remote plugins unpinned**), `contracts/proto/paigasus/common/v1/reserved.proto` (to delete) + `gateway/v1/.gitkeep`, `rs|py|ts` `paigasus-proto/moon.yml` (all bare — SMA-374 did not wire the edge), `.moon/tasks/rust.yml` (`build-release` present, no `^:build`), `.github/workflows/ci.yml` (`proto install` → `moon setup` → `moon ci`; `:lint`/`:breaking` already exercise buf), git log (`b2e5cc1` SMA-374, `13e4d16` SMA-375 landed)
