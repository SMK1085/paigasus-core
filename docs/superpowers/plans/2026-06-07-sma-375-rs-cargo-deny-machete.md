# cargo-deny + cargo-machete for the `rs/` workspace — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add cargo-deny (license/source/bans + RustSec advisories) and cargo-machete (unused-dependency detection) as blocking, whole-workspace Moon CI gates for the `rs/` Cargo workspace.

**Architecture:** Both tools are pinned via **proto** (`.prototools` + vendored `.proto/plugins/*.toml`, like `buf`/`lefthook`/`release-plz`) — NOT `rust.bins` — so they resolve through proto shims on PATH, the proven `release-parity` path. They run as two tasks on the root `repo` (bash) project with `toolchain: 'system'`, with explicit `rs/` inputs, added to the `moon ci` target array. cargo-deny's policy lives in `rs/deny.toml` (v2 schema, "Pragmatic" posture). On today's near-empty workspace both gates pass trivially; they grow teeth as crates consume the `[workspace.dependencies]` catalog.

**Tech Stack:** proto (toolchain manager), Moon 2.2.5 (task graph), cargo-deny 0.19.8, cargo-machete 0.9.2, GitHub Actions.

**Spec:** [`docs/superpowers/specs/2026-06-07-sma-375-rs-cargo-deny-machete-design.md`](../specs/2026-06-07-sma-375-rs-cargo-deny-machete-design.md)
**Review incorporated:** [`…-review.md`](../specs/2026-06-07-sma-375-rs-cargo-deny-machete-design-review.md) (F1–F4)
**Branch:** `feature/sma-375-add-cargo-deny-and-cargo-machete-to-the-rust-workspace` (already checked out; the spec is committed as `eb9b9b7`).

---

## Environment setup (read first)

`proto`, `moon`, and the proto-shimmed CLIs are **not on the Bash tool's default PATH** in this
repo (proto-managed tools live under `~/.proto`). Every shell that runs `proto` / `moon` /
`cargo-deny` / `cargo-machete` directly must first export:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
```

`cargo` (the Rust toolchain) is on PATH once `moon setup` has run for this repo (it is, locally).
The **canonical** verification is always through `moon run …`, which sets up the correct task
environment itself; direct invocations are for the early plugin spike only. There is no macOS
`timeout` binary — don't wrap commands in `timeout`.

---

## File structure

| File | Responsibility | Action |
|------|----------------|--------|
| `.proto/plugins/cargo-deny.toml` | proto schema plugin: resolve checksummed cargo-deny GitHub release tarballs. | Create |
| `.proto/plugins/cargo-machete.toml` | proto schema plugin: resolve checksummed cargo-machete release tarballs. | Create |
| `.prototools` | Pin both tool versions + register the two plugins. | Modify |
| `rs/deny.toml` | cargo-deny policy (licenses/advisories/bans/sources), v2 schema. | Create |
| `moon.yml` (root `repo` project) | Define the `deny` + `machete` whole-workspace tasks. | Modify |
| `.github/workflows/ci.yml` | Add `:deny :machete` to the `moon ci` target array; refresh the `proto install` step label. | Modify |

Task order is deliberate: **prove binary resolution first** (Tasks 1–2, the one genuinely
unproven mechanic — review F2), then policy (Task 3), then the Moon wiring that depends on both
(Task 4), then CI (Task 5), then teeth + delegation checks (Tasks 6–7).

---

## Task 1: Pin cargo-deny via a proto plugin

**Files:**
- Create: `.proto/plugins/cargo-deny.toml`
- Modify: `.prototools`

- [ ] **Step 1: Write the cargo-deny proto plugin**

Create `.proto/plugins/cargo-deny.toml` with exactly:

```toml
# Vendored proto TOML plugin for cargo-deny (SMA-375).
#
# Resolves official, checksummed EmbarkStudios/cargo-deny GitHub release tarballs.
# Same vendoring rationale as buf/release-plz: a static schema over official release
# assets — nothing upstream to maintain.
#
# Tag convention: PLAIN version, no "v" prefix (e.g. 0.19.8). Linux assets are musl for
# BOTH arches (symmetric). cargo-dist-style nested tarball
# (cargo-deny-{version}-{target}/cargo-deny) — proto auto-locates the binary inside the
# top-level dir (same as release-plz), so no exe-path is needed. Arches are Rust triples
# (x86_64/aarch64) = proto's default {arch} tokens, so no [install.arch] remap.

name = "cargo-deny"
type = "cli"

[platform.linux]
download-file = "cargo-deny-{version}-{arch}-unknown-linux-musl.tar.gz"
checksum-file = "cargo-deny-{version}-{arch}-unknown-linux-musl.tar.gz.sha256"

[platform.macos]
download-file = "cargo-deny-{version}-{arch}-apple-darwin.tar.gz"
checksum-file = "cargo-deny-{version}-{arch}-apple-darwin.tar.gz.sha256"

[platform.windows]
download-file = "cargo-deny-{version}-{arch}-pc-windows-msvc.tar.gz"
checksum-file = "cargo-deny-{version}-{arch}-pc-windows-msvc.tar.gz.sha256"

[install]
download-url = "https://github.com/EmbarkStudios/cargo-deny/releases/download/{version}/{download_file}"
checksum-url = "https://github.com/EmbarkStudios/cargo-deny/releases/download/{version}/{checksum_file}"

[resolve]
git-url = "https://github.com/EmbarkStudios/cargo-deny"
```

- [ ] **Step 2: Register the tool + plugin in `.prototools`**

The current `.prototools` is:

```toml
buf = "1.70.0"
lefthook = "2.1.8"
moon = "2.2.5"
release-plz = "0.3.158"

[plugins]
buf = "file://./.proto/plugins/buf.toml"
lefthook = "file://./.proto/plugins/lefthook.toml"
release-plz = "file://./.proto/plugins/release-plz.toml"
```

Add the `cargo-deny` version line (after `buf = "1.70.0"`) and the plugin line (after the `buf`
plugin entry) so the file reads:

```toml
buf = "1.70.0"
cargo-deny = "0.19.8"
lefthook = "2.1.8"
moon = "2.2.5"
release-plz = "0.3.158"

[plugins]
buf = "file://./.proto/plugins/buf.toml"
cargo-deny = "file://./.proto/plugins/cargo-deny.toml"
lefthook = "file://./.proto/plugins/lefthook.toml"
release-plz = "file://./.proto/plugins/release-plz.toml"
```

- [ ] **Step 3: Install and verify resolution (the F2 spike, half 1)**

Run (from repo root):

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
proto install cargo-deny && cargo-deny --version
```

Expected: proto downloads + checksum-verifies the tarball, and the last line prints
`cargo-deny 0.19.8`.

**If `proto install` fails on the checksum** (the per-asset `.sha256` format proto doesn't
accept): remove the two `checksum-file` lines and the `checksum-url` line from the plugin (this
is exactly what `release-plz.toml` does, with a documented note) and re-run. Add a one-line
comment in the plugin explaining checksums were dropped due to format. Do NOT proceed until
`cargo-deny --version` prints the pinned version.

**If `proto install` fails to find the binary in the archive** (unlikely — release-plz proves
auto-location): inspect `tar tzf` of the downloaded archive under `~/.proto/` and add an
`exe-path = "cargo-deny-{version}-{arch}-apple-darwin/cargo-deny"`-style entry; prefer fixing
resolution over abandoning the approach.

- [ ] **Step 4: Verify cargo-subcommand dispatch works**

Run:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cargo deny --version
```

Expected: `cargo deny 0.19.8` (proves `cargo` finds the `cargo-deny` shim — the Moon task uses
this dispatch form). If this prints "no such subcommand" but `cargo-deny --version` worked, note
it: Task 4's Moon command will use the direct `cargo-deny` binary instead of `cargo deny`.

- [ ] **Step 5: Commit**

```bash
git add .proto/plugins/cargo-deny.toml .prototools
git commit -m "build(rs): pin cargo-deny via proto plugin (SMA-375)"
```

---

## Task 2: Pin cargo-machete via a proto plugin

**Files:**
- Create: `.proto/plugins/cargo-machete.toml`
- Modify: `.prototools`

- [ ] **Step 1: Write the cargo-machete proto plugin**

Create `.proto/plugins/cargo-machete.toml` with exactly:

```toml
# Vendored proto TOML plugin for cargo-machete (SMA-375).
#
# Resolves official, checksummed bnjbvr/cargo-machete GitHub release tarballs (cargo-dist).
# Tag convention: "v"-PREFIXED (e.g. v0.9.2). cargo-dist nested tarball
# (cargo-machete-v{version}-{target}/cargo-machete) — proto auto-locates the binary, so no
# exe-path is needed. Arches are Rust triples = proto's default {arch} tokens.
#
# Linux assets are libc-ASYMMETRIC: x86_64 = musl, aarch64 = gnu. This plugin targets the
# x86_64-musl asset (CI runners + local macOS). Linux-aarch64 is deferred — it would need a
# per-arch libc, paralleling the buf.toml Linux-arm TODO / SMA-387.

name = "cargo-machete"
type = "cli"

[platform.linux]
download-file = "cargo-machete-v{version}-{arch}-unknown-linux-musl.tar.gz"
checksum-file = "cargo-machete-v{version}-{arch}-unknown-linux-musl.tar.gz.sha256"

[platform.macos]
download-file = "cargo-machete-v{version}-{arch}-apple-darwin.tar.gz"
checksum-file = "cargo-machete-v{version}-{arch}-apple-darwin.tar.gz.sha256"

[platform.windows]
download-file = "cargo-machete-v{version}-{arch}-pc-windows-msvc.tar.gz"
checksum-file = "cargo-machete-v{version}-{arch}-pc-windows-msvc.tar.gz.sha256"

[install]
download-url = "https://github.com/bnjbvr/cargo-machete/releases/download/v{version}/{download_file}"
checksum-url = "https://github.com/bnjbvr/cargo-machete/releases/download/v{version}/{checksum_file}"

[resolve]
git-url = "https://github.com/bnjbvr/cargo-machete"
```

- [ ] **Step 2: Register the tool + plugin in `.prototools`**

Update `.prototools` so the version block and plugin block read (cargo-machete added in
alphabetical position):

```toml
buf = "1.70.0"
cargo-deny = "0.19.8"
cargo-machete = "0.9.2"
lefthook = "2.1.8"
moon = "2.2.5"
release-plz = "0.3.158"

[plugins]
buf = "file://./.proto/plugins/buf.toml"
cargo-deny = "file://./.proto/plugins/cargo-deny.toml"
cargo-machete = "file://./.proto/plugins/cargo-machete.toml"
lefthook = "file://./.proto/plugins/lefthook.toml"
release-plz = "file://./.proto/plugins/release-plz.toml"
```

- [ ] **Step 3: Install and verify resolution (the F2 spike, half 2)**

Run:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
proto install cargo-machete && cargo-machete --version && cargo machete --version
```

Expected: `cargo-machete 0.9.2` printed twice (direct + cargo-dispatch). Apply the same
checksum/exe-path fallbacks from Task 1 Step 3 if needed.

- [ ] **Step 4: Commit**

```bash
git add .proto/plugins/cargo-machete.toml .prototools
git commit -m "build(rs): pin cargo-machete via proto plugin (SMA-375)"
```

---

## Task 3: Add the cargo-deny policy (`rs/deny.toml`)

**Files:**
- Create: `rs/deny.toml`

- [ ] **Step 1: Write `rs/deny.toml`**

Create `rs/deny.toml` with exactly (no SPDX header — matches repo config convention):

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

- [ ] **Step 2: Verify the config parses and the gate passes on the current workspace**

Run:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cargo-deny --manifest-path rs/Cargo.toml check
```

Expected: cargo-deny clones the advisory DB (first run, ~network), then reports all four checks
(`advisories`, `bans`, `licenses`, `sources`) with **0 errors** and exits `0`. (The lock has only
the 4 workspace crates, so there is nothing to flag.)

A **non-zero exit or a "unknown field"/"failed to deserialize" error** means a v2 schema mismatch
— re-check the three subtle keys against <https://embarkstudios.github.io/cargo-deny/checks/>:
`[advisories] unmaintained` must be the scope selector (`all`/`workspace`/`transitive`/`none`),
`yanked` must be `deny`/`warn`/`allow`, `[licenses] unused-allowed-license` must be
`allow`/`warn`/`deny`. Fix the offending key; do not proceed on a parse error.

- [ ] **Step 3: Commit**

```bash
git add rs/deny.toml
git commit -m "build(rs): add cargo-deny policy config (deny.toml) (SMA-375)"
```

---

## Task 4: Wire the Moon `deny` + `machete` tasks

**Files:**
- Modify: `moon.yml` (root `repo` project)

- [ ] **Step 1: Add the two tasks to the root `moon.yml`**

Insert these two tasks into the `tasks:` map in `moon.yml`, immediately after the `install-hooks:`
task and before `release-parity:` (2-space indent, matching the existing tasks):

```yaml
  deny:
    description: 'cargo-deny supply-chain/license/advisory gate over the rs/ workspace (SMA-375).'
    command: 'cargo deny --manifest-path rs/Cargo.toml check'
    toolchain: 'system'
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

> If Task 1 Step 4 found that `cargo deny` dispatch does **not** work but `cargo-deny` direct
> does, change the two `command:` lines to the direct binaries instead:
> `command: 'cargo-deny --manifest-path rs/Cargo.toml check'` and `command: 'cargo-machete rs'`.

- [ ] **Step 2: Verify Moon resolves the tools under `toolchain: 'system'` (the end-to-end F2 proof)**

Run:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:machete
moon run repo:deny
```

Expected: both tasks succeed (exit `0`). `repo:machete` reports no unused dependencies;
`repo:deny` reports the four checks clean. This confirms a Moon `toolchain: 'system'` task on the
bash `repo` project resolves the proto-shimmed binaries — the core unknown from review F2.

> **Troubleshooting — `cargo: command not found` / "could not execute `cargo metadata`" in
> `repo:deny`:** cargo-deny shells out to `cargo metadata`, so the `deny` task needs `cargo` on
> PATH. `release-parity` (also `toolchain: 'system'`) reaches `cargo` via release-plz, so this is
> expected to work; if it does NOT, set the `deny` task's `toolchain:` to `'rust'` (which puts the
> Rust toolchain's `cargo` on PATH) and re-run. Leave `machete` on `'system'` — default machete
> parses manifests directly and never calls `cargo`. Record the final `toolchain:` value chosen.

- [ ] **Step 3: Commit**

```bash
git add moon.yml
git commit -m "build(rs): add moon deny + machete workspace tasks (SMA-375)"
```

---

## Task 5: Add the gates to `moon ci`

**Files:**
- Modify: `.github/workflows/ci.yml`

- [ ] **Step 1: Add `:deny :machete` to the CI target array**

In `.github/workflows/ci.yml`, find the line inside the "moon ci (affected graph)" step:

```bash
          T=(:build :test :lint :fmt :typecheck :breaking :release-parity :release-parity-py :release-parity-ts)
```

Replace it with (insert `:deny :machete` after `:fmt`):

```bash
          T=(:build :test :lint :fmt :deny :machete :typecheck :breaking :release-parity :release-parity-py :release-parity-ts)
```

- [ ] **Step 2: Refresh the `proto install` step label**

Find:

```yaml
      - name: Install pinned CLIs from .prototools (buf, lefthook)
        run: proto install
```

Replace the `name:` line with (the `run:` stays — `proto install` already installs every
`.prototools` tool, now including the two new ones):

```yaml
      - name: Install pinned CLIs from .prototools (buf, lefthook, cargo-deny, cargo-machete)
        run: proto install
```

- [ ] **Step 3: Verify the affected-graph selection locally**

Run (simulates how CI selects targets against `main`):

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking :release-parity :release-parity-py :release-parity-ts --base main
```

Expected: the run includes `repo:deny` and `repo:machete` among the executed/affected tasks and
exits `0`. (If nothing rs-related is affected vs `main`, confirm instead via the explicit
`moon run repo:deny repo:machete` from Task 4 — the point is the targets are valid and pass.)

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/ci.yml
git commit -m "ci(rs): gate cargo-deny + cargo-machete in moon ci (SMA-375)"
```

---

## Task 6: Prove the gates have teeth (verification only — DO NOT COMMIT)

This exercises both gates against a real violation on the otherwise-empty workspace, then reverts.
Nothing here is committed.

- [ ] **Step 1: Add a deliberately-unused dependency to one crate**

Edit `rs/crates/libs/paigasus-kernel/Cargo.toml` to add a `[dependencies]` table that pulls a
catalog dep the source does not use:

```toml
[dependencies]
serde = { workspace = true }
```

Then lock it:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cargo update -p paigasus-kernel --manifest-path rs/Cargo.toml >/dev/null 2>&1 || cargo build --manifest-path rs/Cargo.toml -p paigasus-kernel
```

- [ ] **Step 2: Confirm cargo-machete FAILS (unused dep detected)**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:machete
```

Expected: **non-zero exit**, output naming `serde` as unused in `paigasus-kernel`. (If it passes,
the gate is not wired correctly — stop and fix Task 4.)

- [ ] **Step 3: Confirm cargo-deny's license gate FAILS when the allowlist is narrowed**

Temporarily remove `"MIT"` and `"Apache-2.0"` from the `allow` list in `rs/deny.toml`, then:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:deny
```

Expected: **non-zero exit** with a `licenses` rejection for `serde` (now an unallowed license).
This proves the license gate bites.

- [ ] **Step 4: Revert ALL teeth-check changes**

```bash
git checkout -- rs/deny.toml rs/crates/libs/paigasus-kernel/Cargo.toml rs/Cargo.lock
```

Then confirm a clean baseline:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
git status --porcelain    # expect: no rs/ changes
moon run repo:deny repo:machete   # expect: both pass, exit 0
```

---

## Task 7: Verify the delegated continuous-advisory layer (review F1 / Decision 2)

The cached `:deny` gate checks advisories only at dependency-change time; continuous RustSec
coverage is delegated to **GitHub Dependabot security alerts**, which must actually be enabled.

- [ ] **Step 1: Check whether Dependabot security alerts are enabled**

```bash
gh api -X GET repos/SMK1085/paigasus-core/vulnerability-alerts \
  && echo "ENABLED" || echo "NOT ENABLED (HTTP 404)"
```

Expected: prints `ENABLED` (the endpoint returns HTTP 204 when on). If it prints `NOT ENABLED`,
enable it (requires repo admin, which the owner has):

```bash
gh api -X PUT repos/SMK1085/paigasus-core/vulnerability-alerts
```

Re-run the GET to confirm `ENABLED`. (No file change / no commit — this is a repo setting.)

- [ ] **Step 2 (optional): Note the AC deviation on the Linear issue**

The spec pins via proto, not `.moon/toolchain.yml` `bins` as SMA-375's AC states. If the user
wants it recorded on the issue (they were asked), add a short comment to SMA-375 explaining the
proto-pin choice and linking the spec's Decision 1. Skip if they preferred to leave it in the spec.

---

## Finishing

- [ ] **Step 1: Confirm the full local gate is green**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:deny repo:machete
git status     # working tree clean (teeth-check fully reverted)
```

- [ ] **Step 2: Push and open the PR**

```bash
git push -u origin feature/sma-375-add-cargo-deny-and-cargo-machete-to-the-rust-workspace
gh pr create --fill --base main
```

The PR auto-links to SMA-375 by branch name — do NOT attach the Linear link manually. CI on the
PR is the real proof of the gates on Linux x86_64 (the local runs prove macOS); confirm the
`moon ci` job passes with `repo:deny` and `repo:machete` in the graph.

---

## Self-review notes (author)

- **Spec coverage:** every spec file (`.prototools`, 2 plugins, `rs/deny.toml`, `moon.yml`,
  `ci.yml`) maps to a task (1,2 → plugins+`.prototools`; 3 → `deny.toml`; 4 → `moon.yml`; 5 →
  `ci.yml`). Decision 2 (Dependabot delegation) → Task 7; the F2 spike → Tasks 1–2 + 4; F4
  MPL-2.0 → Task 3; teeth → Task 6.
- **No placeholders:** every file is given in full; every verify step has an exact command +
  expected result + a concrete fallback.
- **Consistency:** tool/binary names (`cargo-deny`/`cargo-machete`), the `repo:deny`/`repo:machete`
  target ids, and the `toolchain: 'system'` choice (with the documented `'rust'` fallback for
  `deny` only) are used identically across Tasks 4–6.
