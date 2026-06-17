# Design review — SMA-428 napi-rs cross-platform `.node` prebuild matrix

**Reviews:** `docs/superpowers/specs/2026-06-17-sma-428-napi-prebuild-matrix-design.md`
**Reviewer perspective:** staff engineering — "what bites us later"
**Date:** 2026-06-17
**Sources checked:** the spec; Linear SMA-428 (scope + relations) and SMA-407/419/420/434; Notion
ADR-0005/0006/0011; the live repo (`.github/workflows/ci.yml`,
`rs/crates/bindings/paigasus-node-bindings/{package.json,.gitignore,moon.yml,index.js}`,
`ts/packages/paigasus-kernel/package.json`, `.prototools`, `rs/rust-toolchain.toml`); and the
napi-rs v3 docs (`pre-publish`, `create-npm-dirs`, `artifacts`, the v2→v3 migration guide).

## Verdict

The shape is right and the "land it dormant, activate in SMA-407" framing is sound and
house-consistent (it mirrors SMA-398→407 and the SMA-419 wheel deferral). The `files` `*.node` catch
(§3) is a sharp, correct bug-spot, the `rustup target add` "run from `rs/`" reasoning (§2) is right
and corroborated by the existing `ci.yml`, and `napi.targets` (§3) is the correct v3 schema key
(verified against the migration guide). The safety posture — `private:true`/`0.0.0`,
`contents: read`, dry-run only, no publish — is exactly what you want for dormant infra.

Two findings are load-bearing. First, the matrix as written **fails on first dispatch**: it pins
`darwin-x64` to a runner GitHub retired six months ago. Second, the verification proves the
*structure* of the packaging but never exercises the one behavior the issue exists to deliver —
install-time platform resolution — so "SMA-407 inherits a verified pipeline" is only partly true.
Details below, severity-ordered.

| ID | Severity | One-line |
|----|----------|----------|
| H1 | High | `macos-13` was fully retired Dec 2025 — the `darwin-x64` leg is dead on arrival, not a future risk |
| H2 | High | Dry-run verifies packaging *structure*, never install-time *resolution* — the headline capability is never tested; the "verified pipeline" claim is partial |
| M1 | Medium | Committing `npm/<platform>/` dirs contradicts napi v3's explicit guidance; defensible only if the drift gate (SMA-434) lands *with* this, not after |
| M2 | Medium | musl Alpine legs use the image's toolchain, not the proto-pinned one — the step ordering doesn't make `.prototools` apply in-container |
| M3 | Medium | `napi prepublish` defaults (`ghRelease:true`, `tagStyle:lerna`) are masked by `--dry-run`; SMA-407 inherits them unverified and they may fight release-plz |
| M4 | Medium | 15 hand-committed `0.0.0` version strings must bump in lockstep at activation — a drift surface handed to SMA-407 |
| L1 | Low | Intel fallback cross-builds `darwin-x64` on arm64 → never runtime-tested on Intel |
| L2 | Low | `prebuild.yml` caching is unspecified — 7 cold cross builds will be slow/flaky on push-to-main |
| L3 | Process | The spec narrows the Linear ticket (drops publish + resolution testing); reconcile so it isn't closed with literal ACs unmet |

---

## High

### H1 — the `darwin-x64` leg targets a runner that no longer exists

§1 pins `darwin-x64` → `macos-13`, and §6 risk #3 treats macos-13 retirement as a future "if
retired" contingency with the cross-compile fallback as plan B. That framing is out of date:
**GitHub began the macos-13 brownout in September 2025 and the image was fully unsupported by early
December 2025** — six months before this spec. Jobs requesting `macos-13` now fail. So the matrix's
Intel leg reds on the very first dispatch, and the "fallback" is actually the only viable path.

Two correct options today, neither of which is the spec's primary:

- **`macos-15-intel`** — GitHub's dedicated Intel label, the last native-Intel image, available
  **until ~August 2027**. Keeps "native arch per leg" (decision #5) intact for now.
- **Cross-compile** `x86_64-apple-darwin` on `macos-latest` (arm64) via `rustup target add`. This is
  *not* the zig sharp-edge decision #5 warned about — Rust + the Apple SDK cross-compile x86_64↔arm64
  natively on macOS, no zig. The cost is that the artifact is never *run* on Intel (see L1).

Either way, flag the bigger clock: **GitHub drops x86_64 macOS entirely in Fall 2027.** Whichever
path you pick is a sunset path — note it in the spec so `darwin-x64` support is a conscious,
time-boxed commitment, not a permanent matrix row.

**Recommendation:** replace `macos-13` in the §1 table now (don't leave it as the documented
primary), pick Intel-native vs cross-build deliberately, and record the Fall-2027 x86_64-macOS EOL
as a known future decision.

### H2 — the verification proves structure, not the behavior the issue is for

§3/AC #2 says the `assemble` job's `napi prepublish --dry-run` + `npm pack --dry-run` will "assert
os/cpu/libc, `main` paths, and `optionalDependencies` all resolve." Per the napi docs, `prepublish
--dry-run` is "dry run without touching the file system" (it logs the package.json edits + addon
copies it *would* make), and `npm pack --dry-run` prints the would-be tarball file list. So between
them you verify: metadata fields are present, and the main tarball ships loader-only (this is what
catches the `*.node` bug — good). What you do **not** verify is the thing the Linear scope actually
promises — "automatic platform resolution on install": npm choosing the right
`@paigasus/node-bindings-<platform>` from `os`/`cpu`/`libc` at `npm install` time. Neither dry-run
installs anything, so the `optionalDependencies` *resolution* is asserted by inspection, not
exercised.

The spec explicitly considered and declined per-OS install/import smoke for a placeholder kernel.
Fair — but it jumped from "full per-OS matrix (expensive)" straight to "dry-run only (cheap)" and
skipped the cheap, high-value middle: a **single-platform real-install check on the CI host itself**.
The build runner is `ubuntu-latest` = `linux-x64-gnu`, one of your seven targets. After assembly,
`npm pack` all eight tarballs, `npm install` the main package into a scratch dir on that host, and
assert (a) exactly the `linux-x64-gnu` optional dep got installed and (b) `require()` /
`import { sum }` loads the `.node`. That proves the optionalDependencies mechanism end-to-end for one
real target at near-zero cost — and it's the difference between "the metadata looks right" and "the
install resolution works."

This also tempers the "SMA-407 inherits a verified pipeline" claim (decision #3): the steps SMA-407
actually turns on — `npm publish`, and `napi prepublish` *without* `--dry-run` (which triggers the
GitHub-release/tag path, see M3) — are precisely the ones this issue never runs. The pipeline is
verified up to, but not including, its riskiest links.

**Recommendation:** add the single-host install-resolution check; and soften decision #3 to state
explicitly which links remain unverified for SMA-407 (publish, gh-release, tag-style).

---

## Medium

### M1 — committing `npm/<platform>/` dirs runs against napi v3's own guidance

§2/§4 commit the seven `npm/<platform>/package.json` scaffolds and frame regenerating them in CI as
"SMA-434's drift concern, not this job's." But the napi v3→ migration guide is explicit: *"it's not
recommended to commit all `npm/*` files anymore; you can use `napi create-npm-dirs` to create the
`npm/` files in the CI."* So the design deliberately diverges from the tool's recommended v3
workflow.

It's a *defensible* divergence — it matches this repo's established "commit generated code, gate the
drift" posture (the proto codegen drift gate in `ci.yml` does exactly this), and committed scaffolds
are reviewable. The problem is sequencing: the drift gate that makes committed-generated-code safe
(**SMA-434**) is listed as deferred. So for the entire life of SMA-428, the committed scaffolds are
*unguarded* — a `@napi-rs/cli` minor that changes the scaffold shape, or a hand-edit, drifts silently
from what `create-npm-dirs`/`prepublish` would emit, and you find out at SMA-407 publish time.

**Recommendation:** either (a) land SMA-434's drift check *with* SMA-428 (so the committed scaffolds
are gated from day one), or (b) follow upstream and generate `npm/` in CI, dropping the committed
scaffolds. Don't commit generated artifacts and defer their only guard.

### M2 — the musl Alpine legs won't pick up the proto-pinned toolchain as drawn

§2 lists the build job steps as: checkout → `setup-toolchain` + `proto install` → `rustup target
add` → `pnpm install` → `napi build`, with "musl legs run steps 3–5 inside the official napi-rs
Alpine container." But `setup-toolchain`/`proto install` (step 2) run on the *host*; an Alpine
*container* has its own filesystem, PATH, and a pre-installed Rust/Node that is **not** the host's
`~/.proto` shims. So inside the container the build uses the *image's* toolchain, and `.prototools` /
`rs/rust-toolchain.toml` pinning silently does not apply to the two musl legs — exactly the kind of
"which toolchain am I actually using" gap §risk#1 worries about, but structural rather than a cwd
issue.

This is lower-risk than the SMA-389 cross-version `E0514` hazard, because the `.node` is a *leaf*
artifact — nothing else links its rmeta across the npm boundary — so a musl leg built with the
image's rustc is generally fine. But it should be a conscious, written decision, not an accident of
step ordering.

**Recommendation:** specify how the musl legs get their tools — either run `proto install` *inside*
the container (so the pin holds) or accept the image's Rust with the written rationale §risk#1 itself
calls for. Either way, make the container leg's toolchain explicit in the workflow, since the
canonical napi-rs template uses the image's own toolchain and the spec's step list implies otherwise.

### M3 — `napi prepublish`'s defaults are footguns that `--dry-run` is currently hiding

Per the docs, `napi prepublish` defaults to `ghRelease: true` and `tagStyle: lerna`. Under
`--dry-run` (this issue) that's inert. But SMA-407 activates by removing `--dry-run`, at which point
`prepublish` will try to **create a GitHub release** (needing `contents: write` + a token this
workflow deliberately doesn't grant) and will tag in **lerna style** — while this repo's release
machinery is **release-plz**, whose vendored proto plugin tags `release-plz-v*` / per-crate patterns
(SMA-398). Two release tools with two tagging schemes pointed at the same repo is a classic
double-publish/duplicate-tag incident waiting for activation day.

**Recommendation:** even though it's dry-run now, pin the flags that SMA-407 will inherit —
`--no-gh-release` and an explicit `--tag-style` — and add a sentence to §2/decision #3 on how
`napi prepublish` and release-plz divide responsibility (who tags, who publishes). Better to settle
the release-tool boundary while it's dormant than during the first live release.

### M4 — 15 lockstep `0.0.0` strings handed to SMA-407

§3 commits the 7 `optionalDependencies` pinned at `0.0.0`; §4 commits 7 per-platform
`package.json`s at `0.0.0`; the main package is the 15th. At activation, all 15 must move to the real
version in lockstep with each other *and* with the loader's expectations, and they must match what
`napi prepublish`/`napi version` would generate — otherwise install resolves a version that was never
published. Committing them by hand (for reviewability) is reasonable, but it creates a hand-maintained
set that can drift from the tool's output.

**Recommendation:** make explicit that `napi version`/`prepublish` *owns* the version bump at SMA-407
(not hand edits), and that the SMA-434 drift check covers the per-platform `package.json` versions,
not just `index.js`. Note the 15-string lockstep in §1/§5 so SMA-407 plans for it.

---

## Low / systemic

**L1 — `darwin-x64` is build-verified only.** Whichever H1 path you choose, the Intel `.node` is
either cross-built (never run on Intel) or, on `macos-15-intel`, run on a soon-retired image. Combined
with the declined install smoke (H2), `darwin-x64` is the least-exercised leg. Acceptable for a
placeholder kernel; revisit before real domain logic ships on Intel.

**L2 — `prebuild.yml` caching is unspecified.** The existing `ci.yml` carefully caches
`~/.cargo` + `rs/target` keyed on the toolchain + lockfile hash. The new workflow describes
`rustup target add` + a fresh `napi build` per leg with no cache story. Seven cold cross-target cargo
builds on every push-to-main will be slow and more flaky (network, registry). Reuse the `ci.yml`
cache pattern (per-target key, since artifacts differ by triple), or accept the cost explicitly.

**L3 — the spec narrows the Linear ticket; reconcile it.** SMA-428's title and scope include "npm
publish — flip `private:false`… version off `0.0.0`" and "automatic platform resolution on install."
The spec deliberately defers publish to SMA-407 and declines resolution testing (H2). The descope is
well-justified (ADR-0011 S3, the SMA-385 Helikon trap), but the ticket as written would be closed
with two of its literal scope bullets unmet. Update the Linear issue's scope/title to match the
spec's boundary so tracking stays honest, or note the explicit hand-off in the ticket.

---

## What's solid (so it isn't lost in the critique)

- **The `files` `*.node` fix (§3)** is the best catch in the spec: in the optionalDependencies model
  the main package must ship loader-only, and the current
  `files: ["index.js","index.d.ts","*.node"]` would wrongly bundle a host `.node`. Verified against
  the live `package.json`. The `npm pack --dry-run` is the right tool to keep it fixed.
- **`rustup target add … from rs/` (§2)** is correct — the target must attach to the pinned 1.95.0
  toolchain the `rust-toolchain.toml` override selects — and the existing `ci.yml` already uses this
  exact pattern (its serial pre-install step), so it's proven in this repo.
- **`napi.targets` (§3)** is the correct v3 key (v2's `napi.triples` was renamed); the spec didn't
  trip the most common v3 migration gotcha.
- **Dormant-pipeline posture** (`private:true`/`0.0.0`, `contents:read`, dry-run, publish→SMA-407) is
  sound and consistent with how SMA-398 landed dormant for SMA-407 to activate.
- **Native-arch-per-leg + Alpine-for-musl (decision #5)** is the canonical, lowest-risk matrix shape;
  avoiding zig is the right call.

## Suggested spec edits before "ready to plan"

1. Replace `macos-13` in the §1 table (H1); pick `macos-15-intel` vs cross-build; record the
   Fall-2027 x86_64-macOS EOL.
2. Add a single-host (`linux-x64-gnu`) real install-resolution check to the `assemble` job, and
   re-scope the "verified pipeline" claim (H2).
3. Sequence SMA-434's drift gate with this issue, or generate `npm/` in CI per upstream (M1).
4. Specify the musl container's toolchain provisioning (M2).
5. Pin `--no-gh-release` + explicit `--tag-style` and document the release-plz boundary (M3).
6. State that `napi version`/`prepublish` owns the 15-string version bump at SMA-407 (M4).
7. Reconcile the Linear ticket scope/title with the descoped spec (L3).
