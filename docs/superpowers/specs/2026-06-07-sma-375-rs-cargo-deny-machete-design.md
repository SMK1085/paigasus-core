# SMA-375 — Add cargo-deny and cargo-machete to the `rs/` workspace

**Date:** 2026-06-07
**Linear:** [SMA-375](https://linear.app/smaschek/issue/SMA-375/add-cargo-deny-and-cargo-machete-to-the-rust-workspace)
**Status:** Design approved (brainstorm); staff-eng review (F1–F4) incorporated → pending spec
sign-off → implementation plan
**Related:** [SMA-357](https://linear.app/smaschek/issue/SMA-357/bootstrap-rs-cargo-workspace-with-libsbindingsservices-layout)
(rs/ workspace bootstrap; this is review follow-up N4, "Out of scope" at the time).
**Review:** [`2026-06-07-sma-375-rs-cargo-deny-machete-design-review.md`](./2026-06-07-sma-375-rs-cargo-deny-machete-design-review.md)

## Goal

Add supply-chain / license / hygiene tooling to the `rs/` Cargo workspace:
[**cargo-deny**](https://github.com/EmbarkStudios/cargo-deny) (license allowlist, source +
duplicate-version policing, RustSec advisories) and
[**cargo-machete**](https://github.com/bnjbvr/cargo-machete) (unused-dependency detection),
wired as blocking Moon tasks in the affected-graph CI gate. High signal for the open-core
(Apache-2.0) posture, and a gate that **grows teeth** as crates start consuming the ~16
workspace catalog dependencies.

**What this gate is (and isn't) — advisory scope.** The PR `:deny` gate is cached on the
manifests + lockfile, so it enforces **licenses, sources, and bans continuously** (those are
pure functions of those files) but checks **RustSec advisories only at dependency-change time**
(a CVE published against an unchanged dep is not re-detected until `rs/` deps move). Continuous
advisory coverage is intentionally **owned by GitHub Dependabot security alerts** (see
[Decision 2](#decisions-from-brainstorm--review)), not by this cached check. The cargo-deny
advisory check is a change-time backstop, not the continuous CVE monitor.

## Context (current state)

- `rs/` is a Cargo workspace with 4 stub crates: `paigasus-kernel`, `paigasus-proto`
  (libs), `paigasus-py-bindings` (binding `cdylib`), `paigasus-gateway` (service). Each
  declares **zero `[dependencies]`** today — only workspace-inherited `[package]` metadata
  and `[lints]`. `rs/Cargo.lock` holds only the 4 workspace packages, no external crates.
- **Tool pinning convention.** The repo pins external CLIs two ways: rust-toolchain bins in
  `.moon/toolchains.yml` `rust.bins` (`cargo-nextest@0.9.136`), and proto-managed CLIs in
  `.prototools` + vendored TOML plugins under `.proto/plugins/` (`buf`, `lefthook`,
  `release-plz`). This issue uses the **proto path** — see Decision 1.
- `.moon/tasks/rust.yml` defines **per-crate** tasks (`build`/`test`/`lint`/`fmt`),
  inherited by every Rust project via `inheritedBy.languages: ['rust']`.
- Root `moon.yml` is an `id: repo`, `language: bash` project hosting **workspace-level
  cross-cutting tasks** — `release-parity`, `release-parity-py`, `release-parity-ts`,
  `install-hooks`. The `release-parity*` tasks all use `toolchain: 'system'` and reach their
  Rust tool (`release-plz`) because it is **proto-managed** (on PATH via proto shims).
- CI (`.github/workflows/ci.yml`): a `proto install` step installs the `.prototools` CLIs,
  then `moon ci "${T[@]}"` runs the affected graph, where `T=(:build :test :lint :fmt
  :typecheck :breaking :release-parity :release-parity-py :release-parity-ts)`. New gates are
  added to that array.

**Key framing decision:** cargo-deny and cargo-machete are inherently **whole-workspace**
tools (one run over `rs/`, reading `rs/Cargo.lock` / walking all crate manifests), unlike the
per-crate `build`/`test`/`lint`/`fmt`. They are hosted on the root `repo` project (not
per-crate `rust.yml`, which would run them 4×), mirroring `release-parity`.

## Decisions (from brainstorm + review)

1. **Tool acquisition — proto-pin (not `rust.bins`).** Pin `cargo-deny` / `cargo-machete` in
   `.prototools` with vendored `.proto/plugins/*.toml`, exactly like `buf` / `lefthook` /
   `release-plz`. The Moon `deny` / `machete` tasks then use **`toolchain: 'system'`** and
   resolve the binaries through proto shims on PATH — the *proven* `release-parity` resolution
   path. **This deviates from the issue AC** (which says pin via `.moon/toolchain.yml` `bins`).
   Rationale (review F2): `rust.bins` is consumed only by rust-toolchain tasks (`cargo-nextest`
   via the `test` task), and whether a `rust.bin` is on PATH for a `toolchain: 'system'`
   task on the bash `repo` project is **unproven** — whereas proto resolution is already proven
   by `release-parity`. Both tools publish checksummed GitHub release binaries (verified), so
   proto schema plugins can resolve them with nothing to maintain. The AC deviation is recorded
   here and should be noted on SMA-375.
2. **Continuous advisory coverage — GitHub Dependabot security alerts** (review F1). The cached
   `:deny` PR gate keeps advisories as a *change-time* check; continuous RustSec coverage is
   owned by Dependabot security alerts. **Action item:** confirm Dependabot security alerts are
   enabled in repo Settings → Code security (they are a separate toggle from the SMA-362
   *version* updates). No new scheduled CI job is added (a nightly `cache: false` advisories
   run remains a possible future enhancement, explicitly out of scope).
3. **Enforcement posture — Pragmatic.** Licenses allowlist (permissive only) + crates.io-only
   sources are **hard-fail** (the open-core point). RustSec advisories are **hard-fail**
   (vulnerabilities deny-by-default in cargo-deny v2) with an `ignore` escape hatch for
   specific advisory IDs. Duplicate crate versions (`multiple-versions`) are **warn**, not
   block — common in early-stage graphs. `MPL-2.0` is in the allowlist (review F4: Apache-
   compatible weak file-level copyleft, common, routinely allow-listed by Apache projects).
4. **Tool versions + schema** — `cargo-deny@0.19.8`, `cargo-machete@0.9.2` (latest stable as
   of 2026-06-07). cargo-deny 0.19 uses the **v2 `deny.toml` schema**: the legacy
   `[advisories] vulnerability/unsound/notice/severity-threshold` and
   `[licenses] unlicensed/copyleft/allow-osi-fsf-free/default` keys were removed and now hard-
   error. The three subtle keys below are confirmed valid against the live 0.19 cfg reference
   (review F3): `[advisories] unmaintained` = scope selector `all`/`workspace`/`transitive`/
   `none` (distinct from the removed severity key), `[advisories] yanked` = `deny`/`warn`/
   `allow`, `[licenses] unused-allowed-license` = valid key.
5. **`deny.toml` location** — `rs/deny.toml` (next to the workspace `Cargo.toml`), so
   `cargo deny --manifest-path rs/Cargo.toml check` auto-discovers it. No SPDX header (matches
   repo config convention — `moon.yml` / `toolchains.yml` / proto plugins carry none).

## Scope

### In scope — files touched (6)

| Path | Change |
|------|--------|
| `.prototools` | Pin `cargo-deny = "0.19.8"`, `cargo-machete = "0.9.2"` + `[plugins]` entries. |
| `.proto/plugins/cargo-deny.toml` | **NEW** — vendored proto TOML plugin (GitHub release resolver). |
| `.proto/plugins/cargo-machete.toml` | **NEW** — vendored proto TOML plugin. |
| `rs/deny.toml` | **NEW** — cargo-deny v2 config (Pragmatic posture). |
| `moon.yml` (root `repo` project) | Add `deny` + `machete` tasks (`toolchain: 'system'`). |
| `.github/workflows/ci.yml` | Add `:deny :machete` to the `moon ci` `T=(…)` array; update the `proto install` step comment. |

The `deny` task's own `inputs` (`rs/deny.toml`, `rs/**/Cargo.toml`, `rs/Cargo.lock`) drive its
cache invalidation precisely. **Not** adding `rs/deny.toml` to `.moon/tasks.yml`
`implicitInputs`: that list is applied to every inherited task in every workspace, so it would
over-invalidate unrelated py/ts/rust task caches on a policy edit. `implicitInputs` is reserved
for genuinely global config (toolchains, task-definition files).

### Out of scope

- **Nightly / scheduled fresh advisories run.** Continuous CVE coverage is delegated to
  Dependabot security alerts (Decision 2). A `cache: false` cron advisories job is a possible
  future enhancement, not built here.
- **The `.moon/toolchains.yml` `rust.bins` path** for these two tools (superseded by Decision 1).
- **Real license `exceptions` / `clarify` entries** (e.g. `ring`'s OpenSSL component). The lock
  has no external crates today; these are seeded empty and filled in as real deps land.
- **Linux-aarch64 plugin resolution.** cargo-machete's Linux assets are libc-asymmetric
  (x86_64 = musl, aarch64 = gnu); the plugin targets the x86_64-musl asset that CI + local
  (macOS) need. Linux-arm resolution is deferred (parallels the existing `buf.toml` Linux-arm
  TODO / SMA-387).
- Adding this tooling to the **py/ts** workspaces; bindings/node/wasm crates.

## Detailed design

### 1. Tool pinning — `.prototools` + vendored proto plugins

`.prototools` gains two pinned tools and two plugin references:

```toml
buf = "1.70.0"
lefthook = "2.1.8"
moon = "2.2.5"
release-plz = "0.3.158"
cargo-deny = "0.19.8"        # NEW
cargo-machete = "0.9.2"      # NEW

[plugins]
buf = "file://./.proto/plugins/buf.toml"
lefthook = "file://./.proto/plugins/lefthook.toml"
release-plz = "file://./.proto/plugins/release-plz.toml"
cargo-deny = "file://./.proto/plugins/cargo-deny.toml"        # NEW
cargo-machete = "file://./.proto/plugins/cargo-machete.toml"  # NEW
```

Plugin sketches (final asset-path / archive-extraction details settled during implementation;
both follow the buf/release-plz pattern of resolving checksummed GitHub release tarballs):

- **`cargo-deny.toml`** — repo `EmbarkStudios/cargo-deny`, tag = **plain `{version}`** (no `v`
  prefix), assets `cargo-deny-{version}-{arch}-unknown-linux-musl.tar.gz` /
  `…-apple-darwin.tar.gz` with per-asset `.sha256`. Rust arch tokens (`x86_64`/`aarch64`) =
  proto defaults, so no `[install.arch]` remap. Linux is symmetric (musl both arches).
- **`cargo-machete.toml`** — repo `bnjbvr/cargo-machete`, tag = **`v{version}`** (v-prefixed),
  assets `cargo-machete-v{version}-{arch}-apple-darwin.tar.gz` and (Linux x86_64)
  `…-x86_64-unknown-linux-musl.tar.gz`, with per-asset `.sha256`. Linux libc is asymmetric (see
  Out of scope) — target the x86_64-musl asset.

CI already runs `proto install` (the "Install pinned CLIs from .prototools" step); it will now
also install these two. Only that step's **comment** changes (`buf, lefthook` →
`buf, lefthook, cargo-deny, cargo-machete`).

### 2. `rs/deny.toml` (cargo-deny v2 schema, Pragmatic posture)

```toml
[graph]
all-features = true   # see the full dep surface (service tokio/reqwest features, etc.)

[advisories]
db-urls = ["https://github.com/RustSec/advisory-db"]
yanked = "deny"             # vulnerabilities are deny-by-default in v2 (no key needed)
unmaintained = "workspace"  # police crates we pull in, not every deep transitive (noise control)
ignore = []                 # waive specific RUSTSEC-IDs here, each with a `reason`

[licenses]
allow = [                   # v2: anything NOT listed is denied
  "Apache-2.0",
  "Apache-2.0 WITH LLVM-exception",
  "MIT",
  "BSD-2-Clause",
  "BSD-3-Clause",
  "ISC",
  "Unicode-3.0",
  "Zlib",
  "MPL-2.0",                # Apache-compatible weak file-level copyleft (review F4)
]
confidence-threshold = 0.8
unused-allowed-license = "allow"   # near-empty lock today — don't warn on not-yet-seen licenses
exceptions = []             # per-crate carve-outs (e.g. ring's OpenSSL bit) land here as deps arrive

[bans]
multiple-versions = "warn"  # duplicates common early-stage — surface, don't block
wildcards = "deny"          # "*" version reqs are a hygiene smell; the catalog uses none

[sources]
unknown-registry = "deny"
unknown-git = "deny"
allow-registry = ["https://github.com/rust-lang/crates.io-index"]
```

Default `cargo deny check` runs all four sub-checks (advisories, bans, licenses, sources).

### 3. Moon tasks — root `moon.yml` (`id: repo`)

```yaml
  deny:
    description: 'cargo-deny supply-chain/license/advisory gate over the rs/ workspace (SMA-375).'
    command: 'cargo deny --manifest-path rs/Cargo.toml check'
    toolchain: 'system'      # cargo-deny resolves via proto shim on PATH (proven release-parity path)
    inputs:
      - 'rs/**/Cargo.toml'
      - 'rs/Cargo.lock'
      - 'rs/deny.toml'
  machete:
    description: 'cargo-machete unused-dependency check over the rs/ workspace (SMA-375).'
    command: 'cargo machete rs'
    toolchain: 'system'
    inputs:
      - 'rs/**/Cargo.toml'
      - 'rs/**/*.rs'
```

`cargo machete rs` walks `rs/` for crate manifests and checks each crate's real dependency
tables against source usage. The virtual workspace root (`rs/Cargo.toml`, no `[package]`) and
the `[workspace.dependencies]` catalog are **not** flagged — machete only inspects actual
per-crate `[dependencies]` / `[dev-dependencies]` / `[build-dependencies]`.

> **Invocation note.** `cargo-deny` / `cargo-machete` are installed as standalone proto-shimmed
> binaries, so `cargo deny …` (cargo subcommand dispatch) and direct `cargo-deny …` both work
> as long as the shim dir is on PATH. The exact command form is confirmed during the
> implementation spike (Verification below); fall back to the direct `cargo-deny` / `cargo-machete`
> invocation if cargo-subcommand dispatch doesn't see the proto shim.

### 4. CI wiring — `.github/workflows/ci.yml`

```diff
- T=(:build :test :lint :fmt :typecheck :breaking :release-parity :release-parity-py :release-parity-ts)
+ T=(:build :test :lint :fmt :deny :machete :typecheck :breaking :release-parity :release-parity-py :release-parity-ts)
```

Both run as blocking gates on the affected graph (PR: `--base origin/main`; push: `--base
$BEFORE` or full graph). `machete` is offline and fast; `deny` clones the public RustSec
advisory DB anonymously, which works under the workflow's `permissions: contents: read`.

## Behavior today

- **Both gates pass trivially.** The 4 stub crates declare no `[dependencies]`, and the lock
  holds only the 4 workspace packages — machete finds nothing to flag, and deny has no
  external crates to license/advisory-check. The value accrues as crates begin consuming the
  catalog deps; the gate is in place from day one so the first real dependency is policed.

## Testing / verification

- **Implementation spike (do first).** Confirm `proto install` resolves both plugins on this
  platform and that the binaries are on PATH for a `toolchain: 'system'` Moon task — i.e.
  `moon run repo:deny` actually finds `cargo-deny`. This is the one genuinely unproven mechanic
  (review F2); settle it before wiring the rest. If cargo-subcommand dispatch (`cargo deny`)
  doesn't see the shim, switch the task `command` to the direct `cargo-deny` form.
- `moon run repo:deny` → green locally (exit 0, no license/advisory/source violations).
- `moon run repo:machete` → green locally (no unused dependencies reported).
- `moon ci :deny :machete --base origin/main` → both tasks selected and pass in the affected
  graph.
- Negative sanity (manual, not committed): temporarily add an unused dep to a crate →
  `machete` fails; temporarily add a GPL-licensed dep → `deny` fails the license check.
- **Repo-settings check:** verify GitHub Dependabot **security alerts** are enabled
  (Settings → Code security) — the continuous advisory layer this gate delegates to (Decision 2).

## Trade-offs / risks (called out)

1. **Binary resolution is the real gate (review F2).** Resolved by choosing the proven proto
   path (Decision 1) over the unproven `rust.bins`-on-a-bash-project path, but the spike above
   still confirms it empirically on first run. Plugin authoring is low-risk (checksummed GitHub
   release tarballs, identical pattern to `buf`/`release-plz`).
2. **Advisory freshness (review F1).** The cached `:deny` gate checks advisories only at
   dependency-change time; continuous coverage is delegated to Dependabot security alerts —
   which must be enabled (verification above) for that delegation to be real. If they are off,
   there is no continuous CVE monitor and a scheduled `cargo deny check advisories` job should
   be reconsidered.
3. **AC deviation.** Pinning via proto instead of `rust.bins` departs from the issue AC
   (Decision 1); recorded here and to be noted on SMA-375.
4. **Network dependency.** `cargo deny check` clones the RustSec advisory DB from github.com —
   fine under `contents: read`, but it is the only new gate needing network egress (machete is
   offline). On a restricted/forked runner the clone can fail; a network failure is infra, not
   a policy violation, and should not be misread as one.
5. **License allowlist completeness.** The seeded permissive set (incl. `MPL-2.0`) covers the
   common Rust ecosystem licenses, but real transitive deps (e.g. `ring` = ISC/MIT/OpenSSL mix)
   may need `exceptions` / `clarify` entries when they first appear — handled via the empty
   `exceptions` block, not a blocker for this issue.
