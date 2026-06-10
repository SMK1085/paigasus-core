# SMA-387: Fix Linux-aarch64 buf asset resolution — Design

**Issue:** [SMA-387](https://linear.app/smaschek/issue/SMA-387/fix-linux-aarch64-buf-asset-resolution-in-vendored-buftoml-proto)
**Date:** 2026-06-10 (revised same day after design review)
**Status:** Approved

## Problem

The vendored buf proto plugin (`.proto/plugins/buf.toml`, from SMA-360) remaps
`aarch64 = "arm64"` globally via `[install.arch]`. Verified against the
`bufbuild/buf` v1.70.0 release assets, the remap is correct for macOS
(`buf-Darwin-arm64`) and Windows (`buf-Windows-arm64.exe`) but wrong for Linux:
the real asset is `buf-Linux-aarch64`, while the remap resolves
`buf-Linux-arm64`, which does not exist. A contributor or CI runner on
Linux-arm64 gets a broken download with no hint why.

## Research findings

- **The fix the issue AC envisioned does not exist.** proto's TOML schema
  resolves `{arch}` from a single global `install.arch` map
  (`moonrepo/plugins` → `tools/internal-schema/src/proto.rs`, token
  interpolation); the per-platform `[platform.*]` sections (`PlatformMapper` in
  `schema.rs`) have no arch field. A platform-scoped arch remap cannot be
  expressed in any released proto.
- **Upstream already wants this.** [moonrepo/proto#896](https://github.com/moonrepo/proto/issues/896)
  (open, 2025-10-18) requests exactly this capability and cites buf as the
  motivating example, plus ripgrep's per-arch `gnu`/`musl` libc variance as a
  second case. No maintainer response in ~8 months and no PR attempts — but
  external PRs to `moonrepo/plugins` are merged within days (#132, #143, #147),
  so a PR is the viable path, not the issue queue.
- **Scope is buf only.** `lefthook.toml` carries the same remap, but lefthook
  publishes both `Linux_aarch64` and `Linux_arm64` assets, so it resolves
  correctly either way. The cargo-* plugins don't remap `{arch}`. proto's
  plugin registry has no alternative buf plugin (it lists the same upstream
  TOML we vendored).
- **No fork-based interim is practical.** proto loads the schema-engine WASM
  via `builtin_schema_plugin()` (`crates/core/src/config.rs`), a locator
  hardcoded per proto release (`schema_tool@0.17.8` from ghcr.io/moonrepo or
  GitHub releases). It never consults `[plugins]` in `.prototools`, so the
  schema engine cannot be pointed at a fork through configuration. The only
  fork route is building and hosting a standalone WASM plugin (forked
  `schema_tool` with the buf schema embedded) — the previously rejected
  vendor-a-WASM option with extra steps. Rejected, with a concrete revisit
  trigger: **if arm64 release CI lands before the upstream fix reaches a proto
  release, vendor the standalone WASM after all.** "Zero current users" has a
  shelf life — the planned release matrix ships napi/maturin Linux-arm64
  artifacts, GitHub's arm64 runners are free for public repos, and Docker on
  Apple Silicon is a Linux-aarch64 proto client today.

## Decision

Upstream PR for the real fix; loud-fail interim in the vendored TOML so
Linux-arm64 errors clearly instead of silently downloading a wrong asset.

### 1. Upstream PR to `moonrepo/plugins` (closes proto#896)

- Add `arch: HashMap<HostArch, String>` **and** `libc: HashMap<HostLibc, String>`
  to `PlatformMapper` (serde-defaulted → backwards compatible). The libc map is
  ~5 extra lines on the identical code path and covers proto#896's second
  example, making "closes #896" honest rather than partial.
- Resolution order for `{arch}`/`{libc}` tokens: platform-scoped map → global
  `install.*` map → raw Rust value. `interpolate_tokens` gains the current
  `PlatformMapper` as a parameter (callers already hold it via `get_platform`).
- Enables: `[platform.macos.arch] aarch64 = "arm64"`.
- **Identity overrides must work and carry an explicit test**: a platform map
  entry equal to the raw value (e.g. `[platform.linux.arch] aarch64 = "aarch64"`)
  must shadow the global map, not be skipped as a no-op. The flip-over plan
  below depends on exactly this semantic; the test protects it from being
  "simplified" away in review.
- Includes tests in the existing schema-plugin test style and a docs update
  for the non-WASM plugin page.
- No new upstream issue; the PR references and closes proto#896.

### 2. Interim repo change (lands now, this branch)

- `.proto/plugins/buf.toml`: add `archs = ["x86_64"]` under `[platform.linux]`.
  The schema plugin checks this list before downloading, so Linux-arm64 fails
  with a clear unsupported-arch error.
- Replace the `TODO(SMA-387)` comment with the real constraint (global-only
  remap), a link to the upstream PR, and the **compatible** flip-over plan
  (§3) — the comment is what the eventual flip-over executor will follow
  verbatim, so it must describe the stale-client-safe shape, not the naive
  remap move.
- `lefthook.toml`: one-line comment noting its `aarch64 = "arm64"` remap is
  safe only because lefthook publishes `Linux_aarch64` **and** `Linux_arm64`
  as duplicate assets (verified: identical sha256); if upstream drops the
  alias, this same bug reappears there. Apply the platform-scoped pattern to
  lefthook in the same pass as the buf flip-over.
- AC #2 (all four platforms resolve to existing assets) is verified
  empirically against the v1.70.0 release asset list; the PR description
  records the asset names. No verification script — not worth it for a file
  that effectively never changes. Residual risk accepted explicitly: buf pin
  bumps are manual (no Dependabot ecosystem for proto), so a future asset
  rename is detected at next install on the affected platform, not by CI.

### 3. Flip-over (after upstream ships)

The fix reaches contributors only after: PR merge → `schema_tool` release →
proto release bumping the pinned version → contributors updating proto.

**The flip-over must stay correct on stale proto clients.** The schema engine
ships inside proto releases, and `PlatformMapper` ignores unknown fields
(serde default, no `deny_unknown_fields`) — so an older engine parses the
flipped TOML cleanly and silently drops any `[platform.*.arch]` table. Moving
the remap to macOS/Windows and dropping the global `[install.arch]` would
therefore break buf for **every macOS/Windows-arm64 contributor on a stale
proto** (raw `aarch64` → nonexistent `buf-Darwin-aarch64`) — trading a bug on
a near-zero-population platform for one on the primary dev platform. Instead:

```toml
[install.arch]
aarch64 = "arm64"        # keep: old engines still resolve macOS/Windows correctly

[platform.linux.arch]
aarch64 = "aarch64"      # new engines: identity override beats the global remap
```

Outcome: new proto — all four platforms correct; stale proto — macOS/Windows
unchanged, Linux-aarch64 reverts to today's behavior only while stale. The
`archs = ["x86_64"]` restriction is dropped at flip-over (it would otherwise
block Linux-arm64 on new engines too).

In the same PR: **pin proto itself in `.prototools`** (`proto = ">=x.y.z"`,
the release carrying the new schema engine). Without a floor, "contributors
updating proto" is a hope, not a step; with it, CI (`moonrepo/setup-toolchain`
installs from `.prototools`) and proto's own pin check enforce the floor.

### 4. Issue bookkeeping

- Comment on SMA-387 with the research findings and amend the AC: Linux-aarch64
  "resolves correctly" splits into interim loud-fail (SMA-387) and correct
  resolution (successor issue, at flip-over).
- SMA-387 closes when the interim lands and the upstream PR is submitted. The
  flip-over (§3, including the proto floor and the lefthook rewrite) moves to
  a successor issue ("flip buf.toml to platform-scoped arch remap"), marked
  blocked-external with the upstream PR linked — holding one issue In Progress
  against a four-link external chain would rot on the board.

## Testing

- Happy paths: `proto install buf` still works on macOS-arm64 locally; CI
  (Linux-x86_64) exercises it on the PR.
- **The loud-fail itself** (the interim's only new behavior):
  `docker run --platform linux/arm64` on the Apple Silicon Mac (arm64 runs
  natively), install proto, `proto install buf`, and confirm the
  unsupported-arch error fires and is actually legible. The observed error
  text goes in the PR description as AC evidence.
- The upstream change carries its own Rust tests, including the identity-
  override case (§1).

## Out of scope

- cargo-* plugins (no `{arch}` remaps).
- `lefthook.toml` behavior changes (comment-only inoculation, see §2; the
  platform-scoped rewrite happens with the buf flip-over).
- Windows support beyond keeping the existing section correct.
- Any fork/self-hosted WASM interim (see findings; concrete revisit trigger
  recorded there).
