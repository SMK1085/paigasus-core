# SMA-387: Fix Linux-aarch64 buf asset resolution — Design

**Issue:** [SMA-387](https://linear.app/smaschek/issue/SMA-387/fix-linux-aarch64-buf-asset-resolution-in-vendored-buftoml-proto)
**Date:** 2026-06-10
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
  vendor-a-WASM option with extra steps, for a platform with zero current
  users. Rejected; revisit only if ARM Linux CI becomes real before the
  upstream fix ships.

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
- Includes tests in the existing schema-plugin test style and a docs update
  for the non-WASM plugin page.
- No new upstream issue; the PR references and closes proto#896.

### 2. Interim repo change (lands now, this branch)

- `.proto/plugins/buf.toml`: add `archs = ["x86_64"]` under `[platform.linux]`.
  The schema plugin checks this list before downloading, so Linux-arm64 fails
  with a clear unsupported-arch error.
- Replace the `TODO(SMA-387)` comment with the real constraint (global-only
  remap), a link to the upstream PR, and the flip-over plan.
- AC #2 (all four platforms resolve to existing assets) is verified
  empirically against the v1.70.0 release asset list; the PR description
  records the asset names. No verification script — not worth it for a file
  that effectively never changes.

### 3. Flip-over (after upstream ships)

The fix reaches contributors only after: PR merge → `schema_tool` release →
proto release bumping the pinned version → contributors updating proto. When
that lands: move the remap to `[platform.macos.arch]` / `[platform.windows.arch]`,
drop the global `[install.arch]`, drop the `archs` restriction.

### 4. Issue bookkeeping

- Comment on SMA-387 with the research findings and amend the AC: Linux-aarch64
  "resolves correctly" splits into interim loud-fail (now) and correct
  resolution (at flip-over).
- SMA-387 stays In Progress until the flip-over; the interim PR does not
  close it.

## Testing

Interim change is declarative TOML: `proto install buf` must still work on
macOS-arm64 locally, and CI (Linux-x86_64) exercises it on the PR. The
upstream change carries its own Rust tests.

## Out of scope

- `lefthook.toml` and cargo-* plugins (unaffected, see findings).
- Windows support beyond keeping the existing section correct.
- Any fork/self-hosted WASM interim (see findings).
