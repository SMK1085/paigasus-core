# SMA-387: Fix Linux-aarch64 buf asset resolution — Design Review

**Reviews:** [`2026-06-10-sma-387-buf-linux-aarch64-design.md`](./2026-06-10-sma-387-buf-linux-aarch64-design.md)
**Reviewer perspective:** staff engineering — "what bites us at a later stage"
**Sources cross-checked:** `bufbuild/buf` v1.70.0 release assets (live); `evilmartians/lefthook` v2.1.8 release assets (live); `moonrepo/plugins` `tools/internal-schema/src/{schema.rs,proto.rs}` at master (live); [moonrepo/proto#896](https://github.com/moonrepo/proto/issues/896) (live); repo state (`.prototools`, `.proto/plugins/buf.toml`, `.proto/plugins/lefthook.toml`). Linear was unreachable this session; AC statements are taken from the spec's own quotations of SMA-387.
**Date:** 2026-06-10

## Verdict

The research section is the strongest part of this spec — and I verified it independently rather than taking it on faith. Every checkable claim holds: `buf-Linux-aarch64` exists and `buf-Linux-arm64` does not (v1.70.0 asset list); `PlatformMapper` has `archs: Vec<HostArch>` and **no** per-platform arch map; `interpolate_tokens` consults only the global `schema.install.arch`; the `archs` gate in `download_prebuilt` runs `check_supported_os_and_arch` *before* download, so the interim loud-fail mechanism is real; proto#896 says what the spec says it says; and lefthook v2.1.8 really does publish both `Linux_aarch64` and `Linux_arm64` (byte-identical, same sha256). The upstream PR design (platform-scoped map → global map → raw, serde-defaulted) is the right shape and genuinely backwards-compatible.

The spec has one significant analytical gap: **the flip-over plan (§3) was never analyzed against stale proto clients, and as written it breaks every macOS-arm64 contributor running an older proto** — the project's primary development platform. There's a strictly safer flip-over shape that the new upstream capability enables, and the spec should commit to it now, while the upstream PR's semantics are being designed, not at flip-over time. Secondary findings: the interim's *only* behavioral change is never tested, and the "zero current users / revisit if ARM Linux CI becomes real" framing undersells how soon that trigger fires given the release matrix the scoping doc already plans.

## Severity summary

| # | Severity | Issue |
|---|----------|-------|
| 1 | **High** | Flip-over as specified (move remap to macos/windows, drop global `[install.arch]`) silently breaks macOS/Windows-arm64 for every contributor on a stale proto. A backwards-compatible flip-over exists and should be the plan of record. |
| 2 | **Medium** | The interim change's only new behavior — loud-fail on Linux-aarch64 — is never exercised by the testing plan. One `docker run --platform linux/arm64` proves it. |
| 3 | **Medium** | "Zero current users" / "revisit only if ARM Linux CI becomes real" understates exposure: the planned release matrix (napi/maturin Linux arm64 artifacts, per the scoping doc) and free GitHub arm64 runners make the trigger likely to fire during release-tooling work, and Docker-on-Apple-Silicon contributors are Linux-aarch64 today. |
| 4 | **Low** | No proto version floor exists (`.prototools` doesn't pin proto itself), so "contributors updating proto" — the last link in the flip-over chain — is unenforceable. |
| 5 | **Low** | SMA-387 held In Progress until an external four-link chain completes will rot on the board; split the flip-over into its own blocked-on-external issue. |
| 6 | **Low** | lefthook is safe today only by the accident of duplicate-named release assets; if upstream ever drops the `Linux_arm64` alias, the identical bug reappears there. Worth one sentence in the lefthook TOML. |
| 7 | **Low** | AC #2's empirical verification is point-in-time against v1.70.0; buf pin bumps are manual and nothing re-validates asset names on non-CI platforms. Accepting this is fine — but say so explicitly. |

## Findings

### 1. The flip-over breaks stale-proto macOS contributors (high)

§3 says: at flip-over, "move the remap to `[platform.macos.arch]` / `[platform.windows.arch]`, drop the global `[install.arch]`, drop the `archs` restriction." Analyze that against the deployed base:

The schema engine ships *inside* proto releases (the spec's own finding: `schema_tool` is hardcoded per proto release). `PlatformMapper` is `#[serde(default)]` without `deny_unknown_fields`, so an **older** schema_tool parses the flipped TOML cleanly and silently ignores the new `[platform.*.arch]` tables. With the global `[install.arch]` gone, `{arch}` falls through to `env.arch.to_rust_arch()` — raw `aarch64` — and resolves `buf-Darwin-aarch64`, which does not exist. Net effect: **every macOS-arm64 and Windows-arm64 contributor on any proto older than the fix gets a broken buf install**, silently, on the platform where all current development happens. The flip-over converts a bug on a platform with zero users into a bug on the platform with all the users, gated only by whether each contributor has run `proto upgrade` recently. Nothing enforces that (finding 4).

There is a strictly dominant alternative, available *because* the upstream PR's resolution order is platform-scoped → global → raw:

```toml
[install.arch]
aarch64 = "arm64"        # keep: old schema engines still resolve macOS/Windows correctly

[platform.linux.arch]
aarch64 = "aarch64"      # new engines: identity override beats the global remap
```

Outcome matrix: new proto — all four platforms correct; old proto — macOS/Windows keep working exactly as today, Linux-aarch64 reverts to the pre-SMA-387 silent wrong-asset (only while stale, only on a near-zero-population platform). Dropping the `archs` restriction remains necessary for Linux-arm64 to work on new proto — that part of §3 stands — but the *remap move* should be replaced by the override-keep-global shape.

Two implications worth acting on now:

- **The upstream PR must support identity overrides** (platform map entry equal to the raw value, shadowing the global map). The proposed resolution order already implies it, but it's exactly the kind of semantics a reviewer might "simplify" away (e.g. skipping platform entries that equal the raw arch). Encode it as an explicit test case in the PR — that's the test that protects *this repo's* flip-over.
- **§3 of the spec should be rewritten to the compatible shape** before the interim lands, because the `TODO(SMA-387)` replacement comment (§2) is supposed to document "the flip-over plan." Documenting the breaking version of the plan plants the failure eighteen months out, when whoever executes the flip follows the comment verbatim.

### 2. The interim's only behavior change is never tested (medium)

The entire point of §2 is that Linux-aarch64 now fails *loudly*. The testing plan exercises macOS-arm64 locally and Linux-x86_64 in CI — both happy paths that were already green and would stay green if the `archs` line were typo'd, misplaced, or spelled with a wrong `HostArch` value. The one code path the change introduces is the one path never run.

The check is cheap and local: on the Apple Silicon Mac, `docker run --platform linux/arm64 -v $PWD:/repo ubuntu` + install proto + `proto install buf` — arm64 containers run natively on M-series. That confirms (a) `archs = ["x86_64"]` parses as `Vec<HostArch>` and gates as expected, and (b) the `check_supported_os_and_arch` error message actually names the tool and arch clearly enough to count as "loud" (the spec asserts the error quality without having seen it). Capture the observed error text in the PR description as the AC evidence.

### 3. "Zero current users" has a shorter shelf life than the spec implies (medium)

Two concrete paths make Linux-aarch64 real sooner than "revisit if ARM Linux CI becomes real" suggests:

- The *Polyglot Monorepo Scoping* doc's release plan ships napi pre-built binaries and maturin wheels for **Linux arm64 (glibc + musl)** — that's the documented matrix. The moment release CI (SMA-376-era work) builds those artifacts, an arm64 Linux runner enters the picture, and GitHub's arm64 runners are free for public repos. The flip-over's external chain (PR merge → schema_tool release → proto release → contributor upgrade) is months at best; release tooling may well arrive first.
- Any macOS contributor using Docker, devcontainers, or `act` on Apple Silicon is already a Linux-aarch64 proto client today. The loud-fail makes this a clear error rather than a confusing one — good — but it means the "platform with zero current users" framing shouldn't drive the *priority* of the upstream PR.

Recommendation: keep the rejected-fork decision (correct), but replace the vague revisit clause with a concrete trigger: "if arm64 release CI lands before the upstream fix reaches a proto release, vendor the standalone WASM after all." That converts a judgment call under pressure into a pre-made decision.

### 4. The flip-over's last link is unenforceable without a proto floor (low)

`.prototools` pins seven tools but not proto itself, and proto *can* be pinned there (`proto = "x.y.z"`). Without a floor, "contributors updating proto" is a hope, not a step — and finding 1's stale-client window stays open indefinitely. When the flip-over lands, bump a pinned proto version in the same PR. (`ci.yml` uses `moonrepo/setup-toolchain`, which installs from `.prototools`, so CI would enforce it automatically; local contributors get nudged by proto's own pin check.) Worth a one-line addition to §3 now.

### 5. Issue bookkeeping: don't hold SMA-387 open against an external chain (low)

§4 keeps SMA-387 In Progress until flip-over — a four-link chain entirely outside this repo's control. Long-lived In Progress issues decay into board noise and ambiguous standup status. Cleaner: land the interim + submit the upstream PR, close SMA-387 with the amended-AC rationale, and open a successor issue ("flip buf.toml to platform-scoped arch remap") that is explicitly blocked-external with the upstream PR link. This also matches the repo's own fix-in-the-right-issue discipline — the flip-over is genuinely different work with a different trigger.

### 6. lefthook works by coincidence; one sentence would inoculate it (low)

The spec correctly scopes lefthook out: both `Linux_aarch64` and `Linux_arm64` assets exist. Verified — and they're the *same file* (identical sha256), i.e. upstream publishes a courtesy alias. If lefthook's goreleaser config ever drops the alias, `lefthook.toml`'s global `aarch64 = "arm64"` remap reproduces this exact bug there. When the flip-over lands for buf, apply the same platform-scoped pattern to lefthook in the same pass; until then, a one-line comment in `lefthook.toml` noting the alias dependency is cheap insurance.

### 7. Point-in-time verification is fine — but own it (low)

AC #2 is verified against the v1.70.0 asset list, once, by hand. The buf pin in `.prototools` is bumped manually (Dependabot has no proto ecosystem), and each future bump re-validates asset naming only on platforms CI runs — macOS-arm64 and any future arm64 lane stay unverified until a human hits them. The spec's "no verification script" call is reasonable for the plugin file itself; just extend the reasoning to version bumps explicitly: the residual risk is "buf renames assets in a future release," it's detected at next install on the affected platform, and that's accepted. Saying so heads off a future re-litigation.

## What's already right (keep it)

The research quality is exemplary — every claim is sourced to a file and line, and all of them survived independent verification against the live release assets and plugin source. Rejecting the fork/self-hosted WASM interim is the right call for the population at risk. Making the upstream PR carry the `libc` map so "closes #896" is honest rather than partial is good open-source citizenship and improves merge odds. The loud-fail interim is the correct interim *shape* — failing clearly beats resolving a nonexistent URL. And amending the AC into interim/flip-over halves rather than quietly redefining "resolves correctly" is exactly the right bookkeeping instinct; finding 5 only argues about which issue carries the second half.
