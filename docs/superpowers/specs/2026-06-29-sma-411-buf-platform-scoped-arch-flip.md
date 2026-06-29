# SMA-411: Flip buf.toml to platform-scoped arch remap — Spec

**Issue:** [SMA-411](https://linear.app/smaschek/issue/SMA-411/flip-buftoml-to-platform-scoped-arch-remap-blocked-on-upstream-proto)
**Date:** 2026-06-29
**Status:** Awaiting approval (GATE 1)
**Predecessor:** [SMA-387 design §3](./2026-06-10-sma-387-buf-linux-aarch64-design.md) — the original flip-over plan; this doc is the
unblocked, verified-and-revised version after the adversarial spec challenge.

## Problem

The vendored proto plugins remap `aarch64 → arm64` *globally* via `[install.arch]`.
Correct for macOS (`buf-Darwin-arm64`) and Windows (`buf-Windows-arm64.exe`), wrong
for Linux, whose buf asset is `buf-Linux-aarch64`. SMA-387 shipped an interim
loud-fail (`archs = ["x86_64"]` blocks Linux-arm64 cleanly). Upstream now supports
platform-scoped arch overrides, so we flip to the real fix: a **Linux-only identity
override** that shadows the global remap, leaving macOS/Windows untouched.

## Verified external state (confirmed 2026-06-29)

The blocker chain is fully cleared, and the load-bearing assumptions from §3 are now
empirically verified against shipped artifacts (not just intent):

| Fact | Evidence |
|---|---|
| `moonrepo/plugins#151` merged | 2026-06-11 |
| schema engine carrying it | `schema_tool-v0.18.0`, released 2026-06-26 |
| **proto floor** | **0.58.1** — `config.rs:140` pins `schema_tool` `0.18.0`; proto 0.58.0 & 0.57.5 pin `0.17.8` (no platform arch). 0.58.1 release notes add `[platform.*.arch]`/`[platform.*.libc]`. |
| identity-override semantic actually shipped | PR #151 resolution is `platform.arch → install.arch → raw`; CHANGELOG: *"Identity overrides … shadow the global map"*; test `platform_identity_override_beats_global_remap` uses `[platform.linux.arch] aarch64="aarch64"` + `[install.arch] aarch64="arm64"`, host Linux/Arm64 → asserts `tool-Linux-aarch64`. A sibling test proves macOS/Arm64 still falls through to the global `arm64`. |
| lefthook arm64 asset names | v2.1.8 ships `Linux_aarch64` **and** `Linux_arm64`, but only `MacOS_arm64` / `Windows_arm64.exe` (no `aarch64` variants) → lefthook needs the **same** shape as buf: keep global, add Linux-only identity. |
| proto pin enforcement | Pinning `proto` in `.prototools` makes proto resolve/run that version via its shims + version detection + auto-install (it switches to the pinned version). CI installs **latest** proto via `setup-toolchain` (no `proto-version` input) → already ≥0.58.1 since 2026-06-26, so every recent CI run already exercises schema_tool 0.18.0 across all seven vendored plugins. |

## Changes (one PR)

### 1. `.proto/plugins/buf.toml`
- **Drop** `archs = ["x86_64"]` from `[platform.linux]` (the interim loud-fail guard).
- **Keep** `[install.arch] aarch64 = "arm64"` (macOS/Windows need it; stale engines still resolve them).
- **Add** `[platform.linux.arch] aarch64 = "aarch64"` (Linux identity override; shadows the global on ≥0.58.1).
- Place `[platform.linux.arch]` as its own table grouped with `[install.arch]` near the
  end (after all bare keys of `[platform.linux]`) — avoids the TOML gotcha where a
  subtable opened mid-block captures subsequent bare keys.
- Rewrite the header comment: describe the *final* platform-scoped shape and why the
  global remap is kept, replacing the SMA-387 interim/“flip-over is SMA-411” framing.

### 2. `.prototools`
- Add `proto = "0.58.1"` (exact pin, matching every other entry's convention — not the
  `>=` the §3 draft wrote). Brief comment tying it to schema_tool 0.18.0 / #151.

### 3. `.proto/plugins/lefthook.toml`
- Same Linux-only identity override as buf (verified-correct exact TOML). **Do not** add
  macOS/Windows arch tables — those assets are `arm64` and rely on the kept global remap.
- Rewrite the header comment: the Linux-only override now makes lefthook robust against
  upstream dropping the duplicate `Linux_arm64` alias, instead of merely depending on it.

### 4. Doc bookkeeping
- This spec doc captures the merge gates (addresses “no dedicated SMA-411 spec”).
- Add a one-line post-flip status note to the SMA-387 design doc pointing here.

## Verification (merge gates, not optional)

1. **Linux-arm64 (the only platform the change affects).** On the Apple-Silicon Mac
   (arm64 runs linux/arm64 natively): `docker run --rm --platform linux/arm64 -v <repo>:/w -w /w …`,
   install proto **≥0.58.1 inside the container** (the host's 0.57.2 is irrelevant), run
   from repo root so the relative `file://./.proto/plugins/buf.toml` resolves, then
   `proto install buf` and confirm it downloads **`buf-Linux-aarch64`**. Paste the
   observed download line into the PR as AC evidence.
2. **macOS-arm64 fallthrough (primary dev platform, new engine code path).** With proto
   ≥0.58.1 + the flipped TOML, confirm `proto install buf` resolves `buf-Darwin-arm64` and
   `proto install lefthook` resolves `lefthook_*_MacOS_arm64` locally (buf uses the `Darwin`
   OS token, lefthook uses `MacOS`). (Upstream-tested, but a one-command check.)
3. **CI.** Green on `ubuntu-latest` (x86_64) — the `[platform.linux.arch]` table has only an
   `aarch64` key, inert on x86_64, so x86_64 resolution is unchanged.

## Decisions & residual risk

- **Exact pin `0.58.1`** over `>=0.58.1`: matches repo convention; deterministic.
- **Stale-proto Linux-arm64**: on proto <0.58.1 with the guard dropped, Linux-arm64 would
  silently resolve the nonexistent `buf-Linux-arm64` (404) rather than loud-fail. Accepted:
  (a) near-zero population (no current Linux-arm64 users), (b) the proto pin auto-switches
  local proto to 0.58.1, closing the gap, (c) macOS/Windows-arm64 — the real dev population —
  are fully protected by the kept global remap. The §3 phrase "reverts to today's behavior"
  is corrected here: today's behavior is the loud-fail; the actual stale fallback is the
  pre-SMA-387 silent 404.
- **moon 2.3.2 ↔ proto 0.58.1**: already coexisting — `setup-toolchain` installs both
  (latest proto ≥0.58.1) on every recent CI run.

## Out of scope (with rationale)

- **Permanent Linux-arm64 CI smoke test.** The challenger noted `prebuild.yml` already has
  `ubuntu-24.04-arm` runners, so a `proto install buf` + asset assertion would be cheap
  insurance against future asset renames / resolution regressions. The SMA-387 design
  explicitly deferred standing verification tooling, and it's beyond this issue's four ACs —
  **flagged for Sven's decision at GATE 1**, defaulting to out-of-scope.
- Other vendored plugins (cargo-deny/machete/nextest, wasm-pack, release-plz): no `{arch}`
  remaps; already proven on proto ≥0.58.1 by current CI.
- Windows support beyond keeping the existing section correct.
