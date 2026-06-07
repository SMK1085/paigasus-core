# Review — SMA-375 cargo-deny + cargo-machete

**Reviews:** [`2026-06-07-sma-375-rs-cargo-deny-machete-design.md`](./2026-06-07-sma-375-rs-cargo-deny-machete-design.md)
**Reviewer perspective:** staff engineer
**Date:** 2026-06-07
**Sources cross-referenced:** Linear SMA-375 (+ SMA-357), the live `.moon/toolchains.yml` + root `moon.yml` + `rs/` crates, and cargo-deny's v2 (0.19) schema changelog.

## Verdict

Detail-correct and well-scoped — ship it. This is the rare supply-chain spec that actually did the homework on the tool's current schema: I confirmed the cargo-deny **v2 (0.19) breaking changes** are reflected accurately (the legacy `[advisories]` severity keys and `[licenses]` `unlicensed`/`copyleft`/`default`/`deny` are removed in favor of deny-by-default + an `allow` allowlist), so the `deny.toml` won't fail on stale keys — the most common way these configs break. It also uses the correct current toolchain filename and the right architectural home (whole-workspace tools on the root `repo` project, not per-crate).

Two things are worth pushing on, both about *what the gate actually protects*: the advisory check is cached, so it's much weaker than "advisory protection" implies; and the one genuine unknown (getting a `rust.bin` onto PATH for a bash-project task) isn't actually covered by the `release-parity` precedent the spec leans on.

## What the spec gets right (calibration)

- **Correct, current toolchain file.** It targets `.moon/toolchains.yml` (plural) with the exact `rust.bins: ['cargo-nextest@0.9.136']` / `version: '1.95.0'` — the AC's `.moon/toolchain.yml` (singular) is the stale Moon-1.x name; the spec silently corrects it.
- **cargo-deny v2 schema is accurate.** Verified against the 0.19 changelog: `[advisories]` `vulnerability`/`unmaintained`(severity)/`unsound`/`notice`/`severity-threshold` removed (deny-by-default); `[licenses]` `unlicensed`/`allow-osi-fsf-free`/`copyleft`/`default`/`deny` removed (allow-list only); deprecated keys now hard-error. The config matches v2 — this is exactly the migration that silently bites people who copy an old `deny.toml`.
- **Right architectural call.** cargo-deny/machete are whole-workspace (one run over `rs/`), so hosting them on the root `repo` project instead of per-crate `rust.yml` (which would run them 4×) is correct, and consistent with the SMA-401 "route a task to the level it belongs to" philosophy.
- **Precise cache wiring.** Per-task `inputs` (`rs/**/Cargo.toml`, `Cargo.lock`, `deny.toml`) rather than `implicitInputs` — with the correct reasoning that `implicitInputs` would over-invalidate py/ts caches on a Rust policy edit.
- **Well-calibrated posture.** License + crates.io-only sources hard-fail (the open-core point), advisories hard-fail with an `ignore` escape, `multiple-versions` warn (sensible for an early graph). The current state (4 stub crates, 0 deps, 4-package lock) is confirmed, so both gates pass trivially today and grow teeth as deps land.

## Findings

### F1 — [Medium] A cached `:deny` gate is not continuous advisory protection — and "advisory" is a headline value here

cargo-deny bundles four checks, and three of them (licenses, sources, bans) are **deterministic functions of the manifests + lockfile** — caching them on `inputs` is correct and lossless. The fourth, **advisories**, is the opposite: new RustSec advisories are published continuously against *unchanged* dependency versions. But the `:deny` task is cached on `rs/**/Cargo.toml` + `Cargo.lock` + `deny.toml`, so **a CVE published tomorrow against a dependency you already have will not be detected until those files change** — which, for a stable dependency, could be months.

The spec flags this (Trade-off #2) and defers a nightly `cache: false` job as out of scope. That's defensible mechanically, but the gate is *sold* as supply-chain/advisory protection (the open-core security framing in the Goal), and as shipped the advisory portion is largely inert between dependency changes. **Recommendation:** either pull the scheduled advisories run into this work (a small `schedule:`-triggered job running `cargo deny check advisories` with caching off is cheap), or split the framing explicitly — the PR gate guarantees license/source/bans + advisories-at-dependency-change-time, and continuous advisory coverage comes from elsewhere. Note that GitHub Dependabot **security** alerts (the Settings toggle SMA-362 mentioned) already provide continuous RustSec-overlapping coverage; if that's the intended continuous layer, say so, so the cached cargo-deny advisory check isn't mistaken for the security backstop it isn't.

### F2 — [Medium] The `toolchain: 'rust'`-on-a-bash-project unknown isn't covered by the `release-parity` precedent

The spec hosts `deny`/`machete` on the `repo` (`language: bash`) project "mirroring how `release-parity` already hosts Rust-specific cross-cutting gates," with `toolchain: 'rust'` as primary and `toolchain: 'system'` as the fallback (Trade-off #1). But the precedent doesn't actually transfer on the dimension that matters — **binary resolution**:

- The live `release-parity*` tasks all use `toolchain: 'system'` and reach their Rust tool (**release-plz**) because it's **proto-managed** (pinned in `.prototools`, on PATH via proto's shims).
- `cargo-deny`/`cargo-machete` are **`rust.bins`** (rust-toolchain-managed via `moon setup`), a *different* acquisition path. Whether a `rust.bin` lands on the **system** PATH (so `toolchain: 'system'` finds it) or only on the **rust-toolchain** PATH (requiring `toolchain: 'rust'`) is precisely the open question — and `release-parity` doesn't answer it, because it never resolves a `rust.bin` from this project.

So neither the primary nor the fallback is actually proven by precedent; this is the real gate on the whole issue. **Recommendation:** prototype the binary resolution *first*, and consider the cleaner alternative the spec doesn't weigh: pin `cargo-deny`/`cargo-machete` via **`.prototools`** (like `buf`/`lefthook`/`release-plz`) so they resolve through the *proven* `toolchain: 'system'` + proto path that `release-parity` already uses — rather than `rust.bins`, whose PATH exposure to a non-Rust project is unverified. The "proven by cargo-nextest" rationale is weaker than it looks: `cargo-nextest` is consumed by rust-toolchain `test` tasks, never from the bash `repo` project.

### F3 — [Low] Confirm the three subtle keys that v2 hard-errors on

Because v2 cargo-deny turns unknown/removed keys into hard errors, the config will self-catch most mistakes on first run — but three lines are the subtle ones worth confirming explicitly during implementation:

- `[advisories] unmaintained = "workspace"` — this is the **scope-selector** `unmaintained` (values `all`/`workspace`/`transitive`/`none`, added ~0.16), *not* the severity-level `unmaintained` that v2 removed. The spec uses it correctly, but the two share a key name with different value enums, so it's the line most likely to surprise; confirm 0.19.8 accepts the scope form.
- `[licenses] unused-allowed-license = "allow"` and `[advisories] yanked = "deny"` — both valid in recent cargo-deny, but less common and would hard-error if the name/value drifted in v2. Worth a glance at the 0.19.8 schema.

(These aren't likely bugs — the spec's v2 fluency is good — just the lines where a hard-error would land.)

### F4 — [Low] Two early-graph footguns to pre-decide

- **`MPL-2.0` is not in the allow list.** It's common in the Rust ecosystem, Apache-compatible (file-level weak copyleft), and routinely allow-listed by Apache projects (the cargo-deny ecosystem PRs literally include "add MPL-2.0"). Under the deny-by-default posture the first MPL-2.0 transitive dep hard-fails — intended per "fill exceptions as deps arrive," but it'll trip early; decide upfront whether `MPL-2.0` belongs in `allow` rather than `exceptions`.
- **Network dependency.** `cargo deny check` clones the RustSec advisory DB from github.com — fine under `contents: read`, but it's the only new gate that needs network egress (machete is offline). On a restricted runner or fork PR with limited egress, the clone can fail; worth noting so a network failure isn't read as a policy violation.

## Bottom line

Land it — the v2 schema is correct (the thing most likely to silently break, and it doesn't), the architecture and cache wiring are right, and the posture is well-judged for an early open-core graph. Before implementing, resolve the binary-PATH question first (F2) — and consider proto-pinning the two tools to reuse the proven resolution path rather than betting on `rust.bins` reaching a bash-project task. And decide how much advisory protection this gate is really claiming: cached, it covers license/source/bans continuously but advisories only at dependency-change time (F1), so either add the scheduled advisories run now or point continuous coverage at Dependabot security alerts explicitly.

## Sources

- Spec under review: `docs/superpowers/specs/2026-06-07-sma-375-rs-cargo-deny-machete-design.md`
- [Linear SMA-375 — add cargo-deny + cargo-machete](https://linear.app/smaschek/issue/SMA-375/add-cargo-deny-and-cargo-machete-to-the-rust-workspace) (follow-up from SMA-357)
- [cargo-deny CHANGELOG (v2 / 0.19 breaking schema changes)](https://github.com/EmbarkStudios/cargo-deny/blob/main/CHANGELOG.md) — confirms removed `[advisories]`/`[licenses]` keys and deny-by-default
- Repo: `.moon/toolchains.yml` (plural — current Moon 2.x name; `rust.bins: ['cargo-nextest@0.9.136']`, `version: '1.95.0'`), root `moon.yml` (`language: bash`; `release-parity*` tasks all `toolchain: 'system'` resolving **proto**-managed tools), `rs/crates/*` (zero `[dependencies]`; `rs/Cargo.lock` = 4 packages)
