# rs/ Cargo Workspace Bootstrap (SMA-357) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stand up the `rs/` Cargo workspace (libs/bindings/services per ADR-0003) with three empty placeholder crates, centralized workspace deps/lints, and Moon affected-graph wiring, so the workspace builds/lints/tests cleanly and later Rust issues have a foundation.

**Architecture:** A virtual Cargo workspace at `rs/Cargo.toml` with `members = ["crates/*/*"]`. One stub crate per sub-group: `paigasus-kernel` (lib), `paigasus-py-bindings` (cdylib), `paigasus-gateway` (bin). Shared versions/lints live in `[workspace.*]` tables; crates inherit. Moon drives per-crate `build`/`test`/`lint`/`fmt` via one language-scoped inherited task file (`.moon/tasks/rust.yml`) — no per-crate `moon.yml`.

**Tech Stack:** Rust (edition 2024, toolchain 1.95.0), Cargo (resolver 3), cargo-nextest, clippy, rustfmt, Moon 2.2.5.

**Spec:** `docs/superpowers/specs/2026-05-26-rs-cargo-workspace-design.md`

---

## Prerequisites

These must be available before running the verification gates. Do this once, before Task 1.

- [ ] **proto + Moon installed** (per `CONTRIBUTING.md` → Local development). Verify:

  Run: `moon --version`
  Expected: `2.2.5` (matches `.prototools`).

- [ ] **Rust 1.95 toolchain + components available on PATH.** Moon provisions these from
  `.moon/toolchain.yml`, but the raw `cargo` gates need them on PATH. Easiest: let Moon
  provision and sync, or use rustup. Verify:

  Run: `cargo --version && cargo clippy --version && cargo fmt --version`
  Expected: cargo ~1.95, clippy and rustfmt present.

- [ ] **cargo-nextest available** for the early per-task gates (Task 4 pins it for Moon/CI):

  Run: `cargo nextest --version` (if missing: `cargo binstall cargo-nextest` or `cargo install cargo-nextest`)
  Expected: a version ≥ 0.9.85 (so `--no-tests=pass` exists).

All `cargo` commands below are run from `rs/`. All `moon` commands are run from the repo root.

---

## File Structure

| Path | Responsibility |
|------|----------------|
| `rs/Cargo.toml` | Virtual workspace root: members glob, resolver, shared `[workspace.package]`, `[workspace.dependencies]`, `[workspace.lints]`. |
| `rs/crates/libs/paigasus-kernel/Cargo.toml` + `src/lib.rs` | Pure-logic kernel stub (lib). |
| `rs/crates/bindings/paigasus-py-bindings/Cargo.toml` + `src/lib.rs` | PyO3 binding stub (`cdylib`, no pyo3 yet). |
| `rs/crates/services/paigasus-gateway/Cargo.toml` + `src/main.rs` | Gateway service stub (bin). |
| `rs/Cargo.lock` | Generated + committed (workspace has a binary). |
| `.moon/tasks/rust.yml` | Language-scoped inherited Moon tasks for all Rust crates. |
| `.moon/toolchain.yml` | Edit: pin `cargo-nextest` version in `bins`. |
| `rs/README.md` | Edit: status line. |

> **Note on TDD:** these crates are empty scaffolding with no behavior, so there are no unit tests to write first (the spec mandates truly-minimal stubs + `--no-tests=pass`). The equivalent "test" for each task is the build/clippy/fmt gate — run it and confirm it passes before committing.

> **SPDX headers:** every `.rs` file starts with `// SPDX-License-Identifier: Apache-2.0`. Config manifests (`Cargo.toml`, `.moon/*.yml`) do **not** carry SPDX headers (matches the repo's existing config files).

---

## Task 1: Workspace root + `paigasus-kernel`

The workspace won't build with zero members, so the root manifest and the first crate land together.

**Files:**
- Create: `rs/Cargo.toml`
- Create: `rs/crates/libs/paigasus-kernel/Cargo.toml`
- Create: `rs/crates/libs/paigasus-kernel/src/lib.rs`

- [ ] **Step 1: Create the workspace root `rs/Cargo.toml`**

```toml
[workspace]
members  = ["crates/*/*"]
resolver = "3"

[workspace.package]
edition      = "2024"
license      = "Apache-2.0"
rust-version = "1.95"
authors      = ["Paigasus contributors"]

# Versions here are only frozen in Cargo.lock once a workspace member opts in via
# `<dep>.workspace = true`. The stubs consume none of these yet, so today's lock pins
# almost nothing — each dep is resolved by its first real consumer. Feature lists are a
# MINIMAL baseline: they union across the workspace and can't be removed per-crate, so
# crates ADD the features they need rather than this table enabling everything.
[workspace.dependencies]
axum = "0.8"
tower = "0.5"
tower-http = "0.6"
# tokio: no feature baseline — services add rt-multi-thread/macros/time/sync/signal;
# libraries take a strict subset (avoid `macros`, which pulls in #[tokio::main]/tokio::test).
tokio = { version = "1" }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
chrono = { version = "0.4", features = ["serde"] }
uuid = { version = "1", features = ["v4", "serde"] }
bytes = "1"
thiserror = "2"
anyhow = "1"
# Prefer native `async fn` in traits (stable since 1.75); use async-trait only when
# `dyn Trait` is required (per the Notion Rust guidelines).
async-trait = "0.1"
futures = "0.3"
# TLS baseline is rustls + json. If a service needs gzip/brotli/cookies, opt in PER CRATE —
# do NOT set default-features = true here (it re-introduces openssl across the workspace).
reqwest = { version = "0.12", default-features = false, features = ["rustls-tls", "json"] }

# Lint posture: rustc warnings deny in-source; clippy warns in-source and is denied in CI
# via the Moon `lint` task's `-D warnings`.
[workspace.lints.rust]
warnings = "deny"

[workspace.lints.clippy]
all = "warn"
```

- [ ] **Step 2: Create `rs/crates/libs/paigasus-kernel/Cargo.toml`**

```toml
[package]
name = "paigasus-kernel"
version = "0.0.0"
edition.workspace = true
license.workspace = true
rust-version.workspace = true
authors.workspace = true
# TODO(SMA-376): flip publish = true + choose a real 0.x version once the kernel has
# release-worthy logic (kernel is crates.io-bound per ADR-0005).
publish = false

[lints]
workspace = true
```

- [ ] **Step 3: Create `rs/crates/libs/paigasus-kernel/src/lib.rs`**

```rust
// SPDX-License-Identifier: Apache-2.0

//! Pure-logic behavioral kernel for Paigasus.
//!
//! Bound to Python / Node / WASM via the crates under `rs/crates/bindings/`. No FFI or
//! adapter dependencies live here (ADR-0005). Empty until real logic lands.
```

- [ ] **Step 4: Build the workspace (this is the "test")**

Run: `cd rs && cargo build --workspace`
Expected: `Finished` with one member compiled (`paigasus-kernel`); a new `rs/Cargo.lock` appears.

- [ ] **Step 5: Run the fmt + clippy gates**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings`
Expected: no output / exit 0 (no formatting diff, no clippy warnings).

- [ ] **Step 6: Commit**

```bash
git add rs/Cargo.toml rs/Cargo.lock rs/crates/libs/paigasus-kernel
git commit -m "feat(rs): bootstrap Cargo workspace with paigasus-kernel stub (SMA-357)"
```

---

## Task 2: `paigasus-py-bindings` (cdylib stub)

**Files:**
- Create: `rs/crates/bindings/paigasus-py-bindings/Cargo.toml`
- Create: `rs/crates/bindings/paigasus-py-bindings/src/lib.rs`

- [ ] **Step 1: Create `rs/crates/bindings/paigasus-py-bindings/Cargo.toml`**

```toml
[package]
name = "paigasus-py-bindings"
version = "0.0.0"
edition.workspace = true
license.workspace = true
rust-version.workspace = true
authors.workspace = true
publish = false

[lib]
# Only cdylib: Python loads this artifact; no other Rust crate consumes it. Add "rlib"
# only if a test or debug tool later needs to import it.
crate-type = ["cdylib"]

[lints]
workspace = true
```

- [ ] **Step 2: Create `rs/crates/bindings/paigasus-py-bindings/src/lib.rs`**

```rust
// SPDX-License-Identifier: Apache-2.0

//! PyO3 binding shim for `paigasus-kernel` (ADR-0005). No `pyo3` dependency or exported
//! symbols yet — empty cdylib stub.
```

- [ ] **Step 3: Build (the "test")**

Run: `cd rs && cargo build --workspace`
Expected: `Finished`; `paigasus-py-bindings` compiles to a cdylib (`.dylib` on macOS) with no warnings.

- [ ] **Step 4: fmt + clippy gates**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings`
Expected: exit 0.

- [ ] **Step 5: Commit**

```bash
git add rs/Cargo.toml rs/Cargo.lock rs/crates/bindings/paigasus-py-bindings
git commit -m "feat(rs): add paigasus-py-bindings cdylib stub (SMA-357)"
```

---

## Task 3: `paigasus-gateway` (binary stub)

**Files:**
- Create: `rs/crates/services/paigasus-gateway/Cargo.toml`
- Create: `rs/crates/services/paigasus-gateway/src/main.rs`

- [ ] **Step 1: Create `rs/crates/services/paigasus-gateway/Cargo.toml`**

```toml
[package]
name = "paigasus-gateway"
version = "0.0.0"
edition.workspace = true
license.workspace = true
rust-version.workspace = true
authors.workspace = true
publish = false

[lints]
workspace = true
```

- [ ] **Step 2: Create `rs/crates/services/paigasus-gateway/src/main.rs`**

```rust
// SPDX-License-Identifier: Apache-2.0

fn main() {}
```

- [ ] **Step 3: Build (the "test")**

Run: `cd rs && cargo build --workspace`
Expected: `Finished`; all three crates compile; `paigasus-gateway` produces a binary.

- [ ] **Step 4: fmt + clippy gates**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings`
Expected: exit 0.

- [ ] **Step 5: Commit**

```bash
git add rs/Cargo.toml rs/Cargo.lock rs/crates/services/paigasus-gateway
git commit -m "feat(rs): add paigasus-gateway binary stub (SMA-357)"
```

---

## Task 4: Moon wiring (nextest pin + inherited Rust tasks)

**Files:**
- Modify: `.moon/toolchain.yml` (the `rust.bins` list)
- Create: `.moon/tasks/rust.yml`

- [ ] **Step 1: Resolve the latest stable `cargo-nextest` version**

Run: `curl -s https://crates.io/api/v1/crates/cargo-nextest | grep -o '"max_stable_version":"[^"]*"'`
Expected: e.g. `"max_stable_version":"0.9.136"`. Use that exact version in Step 2.

- [ ] **Step 2: Pin `cargo-nextest` in `.moon/toolchain.yml`**

Find the `rust:` block's `bins:` entry:

```yaml
  bins:
    - 'cargo-nextest'
```

Replace with the resolved version (example shows 0.9.136 — use the value from Step 1):

```yaml
  bins:
    - 'cargo-nextest@0.9.136'
```

- [ ] **Step 3: Create `.moon/tasks/rust.yml`**

```yaml
$schema: 'https://moonrepo.dev/schemas/tasks.json'

# Language-scoped tasks inherited by every project Moon detects as Rust (presence of a
# Cargo.toml). $project = the Moon project id = crate dir name = Cargo package name.
tasks:
  build:
    command: 'cargo build -p $project'
    inputs: ['@group(sources)', 'Cargo.toml']
  test:
    command: 'cargo nextest run -p $project --no-tests=pass'
    inputs: ['@group(sources)', '@group(tests)', 'Cargo.toml']
  lint:
    command: 'cargo clippy -p $project --all-targets -- -D warnings'
    inputs: ['@group(sources)', '@group(tests)', 'Cargo.toml']
  fmt:
    command: 'cargo fmt -p $project --check'
    inputs: ['@group(sources)']
```

- [ ] **Step 4: Confirm Moon resolves the three crates as Rust projects with tasks**

Run: `moon query projects --json | grep -o '"id":"paigasus[^"]*"'`
Expected: `paigasus-kernel`, `paigasus-py-bindings`, `paigasus-gateway` all listed.

If the crates do NOT appear or have no tasks, apply the spec's open-item-#2 fallback: add a one-line `moon.yml` (`language: 'rust'`) to each crate dir, then re-run.

- [ ] **Step 5: Run the Moon build/test graph (the "test")**

Run: `moon run :build :test :lint :fmt`
Expected: all tasks succeed across the three crates (test = "no tests to run", exit 0 via `--no-tests=pass`). If `cargo`/`cargo-nextest` aren't on PATH for the task context, run `moon sync projects` first so Moon provisions the toolchain.

- [ ] **Step 6: Confirm no stray Moon/cargo state is tracked**

Run: `git status --short`
Expected: only `.moon/toolchain.yml` and `.moon/tasks/rust.yml` show as changes; no `.moon/cache/` or `target/` (both gitignored).

- [ ] **Step 7: Commit**

```bash
git add .moon/toolchain.yml .moon/tasks/rust.yml
git commit -m "feat(rs): wire Rust crates into Moon affected-graph; pin cargo-nextest (SMA-357)"
```

---

## Task 5: README + full acceptance verification + PR

**Files:**
- Modify: `rs/README.md`

- [ ] **Step 1: Update the status line in `rs/README.md`**

Find:

```markdown
**Status:** scaffolded in SMA-357. Empty until the Cargo workspace lands.
```

Replace with:

```markdown
**Status:** Cargo workspace landed in SMA-357. The three crates are empty placeholders
until their behavior lands in later issues.
```

- [ ] **Step 2: Run the complete acceptance gate suite**

Run (from `rs/`):

```bash
cargo build --workspace \
  && cargo fmt --all --check \
  && cargo clippy --workspace --all-targets -- -D warnings \
  && cargo nextest run --workspace --no-tests=pass
```

Expected: build `Finished`; fmt no diff; clippy exit 0; nextest reports no tests and exits 0.

- [ ] **Step 3: Run the Moon CI gates**

Run (from repo root): `moon run :build :test`
Expected: exit 0 across all three Rust crates. (CI itself uses `moon ci :build :test` with `--base origin/main`; the local `moon run` form verifies the same tasks without needing the affected-graph base.)

- [ ] **Step 4: Confirm a clean tree**

Run: `git status --short`
Expected: only `rs/README.md` modified; `target/` and `.moon/cache/` absent (gitignored); `rs/Cargo.lock` already committed.

- [ ] **Step 5: Commit**

```bash
git add rs/README.md
git commit -m "docs(rs): mark Cargo workspace as landed (SMA-357)"
```

- [ ] **Step 6: Push and open the PR**

```bash
git push -u origin feature/sma-357-bootstrap-rs-cargo-workspace-with-libsbindingsservices
gh pr create --base main \
  --title "feat(rs): bootstrap Cargo workspace with libs/bindings/services layout (SMA-357)" \
  --body "Implements SMA-357. See docs/superpowers/specs/2026-05-26-rs-cargo-workspace-design.md.

## Acceptance criteria
- [x] rs/Cargo.toml with members = [\"crates/*/*\"] + resolver (3, see spec §5)
- [x] libs/ bindings/ services/ dirs with one placeholder crate each
- [x] [workspace.dependencies] with shared crate versions
- [x] [workspace.package] (edition 2024 — see spec §2, license, rust-version, authors)
- [x] cargo build / fmt --check / clippy -D warnings / nextest --workspace pass
- [x] Rust wired into Moon affected-graph (.moon/tasks/rust.yml)

## AC deviations (flagged for Linear correction)
- edition 2024 (not 2021); resolver 3 (not 2); nextest needs --no-tests=pass — see spec.

🤖 Generated with [Claude Code](https://claude.com/claude-code)"
```

- [ ] **Step 7: Confirm CI is green**

Run: `gh pr checks --watch`
Expected: `moon ci` workflow passes (once the CI workflow from SMA-361 exists; if CI isn't wired yet, note that and rely on the local gates).

---

## Post-implementation follow-ups (not this PR)

- Update the Linear SMA-357 AC for the three deviations (edition 2024, resolver 3, `--no-tests=pass`).
- Flag the Notion "Polyglot Monorepo Scoping § 1" / scoping-doc `rs/Cargo.toml` snippet as stale (review N7) — needs maintainer sign-off.
- Tracked issues already created: **SMA-374** (slim template + unify build profile), **SMA-375** (cargo-deny/machete), **SMA-376** (kernel publish-flip).
