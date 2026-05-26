# SMA-357 — Bootstrap `rs/` Cargo workspace (libs / bindings / services)

**Date:** 2026-05-26
**Linear:** [SMA-357](https://linear.app/smaschek/issue/SMA-357/bootstrap-rs-cargo-workspace-with-libsbindingsservices-layout)
**Status:** Design approved (brainstorm); staff-eng review pass incorporated (see "Review
incorporation"); pending final spec sign-off → implementation plan
**Blocked by:** SMA-355 (bootstrap monorepo) — merged; SMA-356 (Moon config) — merged.
**Blocks:** SMA-363 (foundation acceptance gate).

## Goal

Stand up the Rust workspace under `rs/` as a single [Cargo](https://doc.rust-lang.org/cargo/)
workspace with the sub-grouped crate layout from **ADR-0003** (`libs` / `bindings`
/ `services`), and wire it into Moon's affected-graph at the language layer. One
empty placeholder crate per sub-group so the workspace builds, the Cargo member
glob resolves, and the Moon project globs (from SMA-356) resolve. No real
behavior yet — this is scaffolding the later Rust issues build on.

## Scope

### In scope

| Path | Purpose |
|------|---------|
| `rs/Cargo.toml` | Workspace root: `members`, `resolver`, `[workspace.package]`, `[workspace.dependencies]`, `[workspace.lints]`. |
| `rs/Cargo.lock` | Committed (the workspace contains a binary — `paigasus-gateway`). |
| `rs/crates/libs/paigasus-kernel/` | Empty library crate — pure-logic kernel (ADR-0005). |
| `rs/crates/bindings/paigasus-py-bindings/` | Empty `cdylib` stub (no `pyo3` yet). |
| `rs/crates/services/paigasus-gateway/` | Empty binary stub (`fn main() {}`). |
| `.moon/tasks/rust.yml` | **NEW.** Language-scoped inherited tasks (`build`/`test`/`lint`/`fmt`) for all Rust projects. |
| `.moon/toolchain.yml` (edit) | Pin `cargo-nextest` to a concrete version in `bins` (the `cargo-nextest@<ver>` form) so the `--no-tests=pass` behavior is version-locked (review S5). |
| `rs/README.md` (edit) | Flip the status line from "empty until the Cargo workspace lands" to landed. |

### Out of scope (owned by other issues / later work)

- `paigasus-proto` crate + `contracts:generate` task → **SMA-360** (contracts/buf).
- `paigasus-node-bindings`, `paigasus-wasm`, and real `pyo3`/maturin wiring → later
  binding issues (post-MVP per ADR-0005).
- The gateway's hexagonal skeleton (`domain`/`inbound`/`outbound`/`config`) and real
  dependencies (axum/tower/etc.) → when the gateway gets actual logic.
- Per-crate `moon.yml` override files → authored only when a crate needs crate-specific
  config (gateway `--release` + `contracts:generate` dep once SMA-360 lands; bindings'
  maturin tasks). Until then the inherited `.moon/tasks/rust.yml` covers everything.
- Slimming the `.moon/templates/rust/` generator (defer standard tasks to `rust.yml`) AND
  resolving the service archetype's `--release` build vs `rust.yml`'s debug `build` profile
  split → **SMA-374**, not just "future cleanup". Review S8 showed this is a latent
  mixed-profile bug (a generated service `moon.yml` would build release while sibling crates
  build debug under one `moon ci :build`), not cosmetic. Not touched here to keep SMA-356's
  deliverable stable.
- `cargo-deny` (license allowlist + advisories) and `cargo-machete` (unused-dep detection)
  → **SMA-375** (review N4); high signal for the open-core posture.
- Flipping `paigasus-kernel` to `publish = true` + a real `0.x` version when it gains
  release-worthy logic → **SMA-376** (review S9); a `TODO(SMA-376)` comment marks it
  in `paigasus-kernel/Cargo.toml` so the stub state is not silently permanent.
- Workspace `[profile.release]` (lto/codegen-units/strip) and a `[workspace.metadata]`
  reservation → deferred to the first service / the ADR-0010 release-tooling work
  (review S7/N5); nothing in this issue produces a release build or runs release tooling.

## Key design decisions

Resolved during brainstorming (2026-05-26).

### 1. Moon integration depth — Option B: language-scoped inherited tasks

Three options were weighed:

- **A — Cargo-only (strict AC).** Smallest diff, but `moon ci` does nothing for Rust
  (task-less projects), the affected-graph stays Rust-blind until a later issue, and
  the `.moon/tasks/rust.yml` that SMA-356 deferred "to SMA-357/358/359" stays unowned.
- **B — Cargo + `.moon/tasks/rust.yml` (chosen).** One inherited task file gives every
  Rust crate `build`/`test`/`lint`/`fmt` via Moon's `$project` token — no per-crate
  boilerplate, no dangling references, and Rust actually participates in the
  affected-graph (the point of ADR-0008). Discharges the SMA-356 deferral.
- **C — Cargo + per-crate `moon.yml` from the generator template.** Most complete, but
  the `service` template emits `dependsOn: [paigasus-proto, paigasus-kernel]` +
  `deps: [contracts:generate]`, which don't exist until SMA-360 — forcing hand-edits
  that get reverted later — and duplicates the standard tasks against `rust.yml` (two
  sources of truth). Over-reaches into SMA-360's work.

**Decision: B.** It delivers the durable baseline (Moon-aware Rust CI) with the least
config and zero cross-issue churn. Per-crate `moon.yml` files stay reserved for genuine
overrides and arrive when crates need them.

### 2. Rust edition 2024 (deviates from the AC's `2021`)

The AC lists `edition = "2021"`, but the toolchain is pinned at **1.95.0** and edition
2024 has been stable since 1.85. For a greenfield repo with no legacy code, edition 2024
is the natural default. **Decision: `edition = "2024"`.** The AC's `2021` is treated as
stale (same pattern as the `codeowners.sync` correction in SMA-356) and **flagged for
correction in Linear**. `rust-version = "1.95"` (edition 2024 floor is 1.85, satisfied).

### 3. Lint enforcement centralized in `[workspace.lints]` (rust `deny` / clippy `warn`)

Lint levels are declared once at the workspace and inherited by every crate
(`[lints] workspace = true`):

```toml
[workspace.lints.rust]
warnings = "deny"

[workspace.lints.clippy]
all = "warn"
```

**Why the split (review B2).** A blanket `clippy::all = "deny"` is brittle: the `all`
group grows across Rust releases, so a toolchain bump (1.95 → 1.96) can turn
previously-clean code CI-red on lints unrelated to the change — a recurring chore for the
maintainer and an "unmergeable first PR for unrelated reasons" trap for contributors. So:

- **`[workspace.lints.rust] warnings = "deny"`** — rustc warnings fail plain `cargo build`
  and surface in editors (the in-source enforcement we want; rustc is conservative about
  adding new warn-by-default lints, so denying them is stable enough).
- **`[workspace.lints.clippy] all = "warn"`** — clippy lints are *visible* in IDE /
  `cargo clippy` but don't fail local builds; the Moon `lint` task's
  `cargo clippy -- -D warnings` is the **CI hard gate**. Clippy stays strict where it
  counts (CI) while toolchain bumps become a non-event for local builds.

`unsafe_code` is deliberately **not** forbidden at the workspace level — binding crates
(PyO3/napi/wasm) will need `unsafe`. A pure-logic crate that wants to forbid it
(`paigasus-kernel`) should use a `#![forbid(unsafe_code)]` crate attribute when it gains
real code — cleaner than overriding the inherited `[lints]` block, since Cargo's `[lints]`
can't mix `workspace = true` with extra crate-specific lints (review N1).

### 4. Minimal stubs (YAGNI)

Each crate is the smallest thing that compiles cleanly: blank `lib.rs` / `fn main() {}`,
each opening with the required SPDX header. The gateway's hexagonal structure (per the
Rust guidelines) is **not** scaffolded now — it lands when the gateway gets real logic,
avoiding empty-module noise and premature structure.

### 5. `resolver = "3"` (deviates from the AC's `"2"`; MSRV-aware)

The AC specifies `resolver = "2"`, but we adopt **`resolver = "3"`** — the
edition-2024-aligned, MSRV-aware resolver (review S2). With resolver 2, the
`rust-version = "1.95"` declaration is *not* honored during dependency selection: Cargo
picks the newest semver-compatible version of each dep even if it raised its MSRV above
1.95, surfacing as a downstream compile error rather than a resolution-time one — i.e.
MSRV declared but not enforced. Resolver 3 makes `rust-version` enforceable at resolution,
which matters because `paigasus-kernel` is destined for crates.io (ADR-0005), where the
shipped MSRV is a public contract. The AC's `"2"` is flagged for correction in Linear
alongside the edition fix.

### 6. Naming and identity alignment

Crate **directory name = Cargo package name = Moon project id** for all three crates, so
`cargo -p <name>` and Moon's `$project` token resolve to the same identifier. Crate names
are taken verbatim from ADR-0005: `paigasus-kernel`, `paigasus-py-bindings`. The service
is `paigasus-gateway` (the AC's example; matches the composition-root example in the Rust
guidelines).

### 7. `Cargo.lock` committed

The workspace contains a binary (`paigasus-gateway`), so `Cargo.lock` is committed
(standard guidance for workspaces with applications), giving reproducible builds. The
existing `.gitignore` already ignores `target/` and does not ignore `Cargo.lock`.

## Configuration content

### `rs/Cargo.toml`

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
# `<dep>.workspace = true`. The three stubs consume none of these yet, so today's lock
# pins almost nothing — each dep is resolved by its first real consumer. Feature lists are
# a MINIMAL baseline: they union across the workspace and can't be removed per-crate, so
# crates ADD the features they need rather than this table enabling everything.
[workspace.dependencies]
axum = "0.8"
tower = "0.5"
tower-http = "0.6"
# tokio: no feature baseline — services add `rt-multi-thread`/`macros`/`time`/`sync`/`signal`;
# libraries take a strict subset (avoid `macros`, which pulls in `#[tokio::main]`/`tokio::test`).
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

# Lint posture (decision §3): rustc warnings deny in-source; clippy warns in-source and is
# denied in CI via the Moon `lint` task's `-D warnings`.
[workspace.lints.rust]
warnings = "deny"

[workspace.lints.clippy]
all = "warn"
```

Caret ranges; exact minor/patch versions confirmed against the registry at implementation
time. Note (review B1): because the stubs reference none of these deps, the committed
`Cargo.lock` pins only the implicit toolchain crates at this stage — the workspace deps are
locked lazily, each when its first real consumer lands. The commit-`Cargo.lock` decision
(§7) is still correct (the workspace has a binary); only the lock's *contents* are
intentionally minimal now.

### Per-crate `Cargo.toml`

Each crate inherits workspace settings. Library (`paigasus-kernel`):

```toml
[package]
name         = "paigasus-kernel"
version      = "0.0.0"
edition      .workspace = true
license      .workspace = true
rust-version .workspace = true
authors      .workspace = true
publish      = false

[lints]
workspace = true
```

Binding (`paigasus-py-bindings`) adds a `cdylib` crate type; no `pyo3` dependency yet.
Only `cdylib` (no `rlib`): Python loads the binding, no other Rust crate consumes it —
`rlib` is added later only if a test or debug tool needs to import it (review S1):

```toml
[lib]
crate-type = ["cdylib"]
```

Service (`paigasus-gateway`) uses the same `[package]`/`[lints]` form as the library (no
`[lib]` section); its `src/main.rs` is `fn main() {}`. `version = "0.0.0"` +
`publish = false` mark all three as not-yet-real placeholders. `paigasus-kernel` carries a
`# TODO(SMA-376): flip publish = true + choose a real 0.x version once the kernel has
release-worthy logic` comment so the unpublishable stub state isn't silently permanent
(review S9; kernel is crates.io-bound per ADR-0005).

### `.moon/tasks/rust.yml`

Inherited by every project Moon detects as Rust (presence of `Cargo.toml`), so no
per-crate `moon.yml` is required:

```yaml
$schema: 'https://moonrepo.dev/schemas/tasks.json'

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

`$project` resolves to the Moon project id = crate dir name = Cargo package name, so
`-p $project` selects the right package. Cargo discovers the workspace root by walking up
to `rs/Cargo.toml`, so the tasks run correctly from each crate's own directory.

`--all-targets` on the lint task (review N2) lints tests/examples too, not just lib/bin —
hence `@group(tests)` is also an input. The AC's `--workspace`-mode `cargo nextest` / `cargo
fmt` gates are satisfied by Moon's **per-project** task graph (one task per crate); that
per-project shape is exactly what enables the affected-graph (review S6), so we add no
separate workspace-mode task. A CI-only `test-workspace` task (one `cargo nextest run
--workspace` for cross-crate parallelism) is noted as a future addition once crate count
grows. Heads-up (review N3): a contributor running bare `cargo fmt --check` at `rs/` gets
whole-package scope vs the task's `-p $project` — same result, different scope.

**Relationship to `.moon/templates/rust/` (from SMA-356):** `rust.yml` is the *baseline*;
the generator template stays for producing per-crate `moon.yml` *overrides* when a crate
needs them. Generated per-crate tasks override the inherited ones by name (Moon task
inheritance). The template's standard task definitions are now redundant with `rust.yml`,
and its `service` archetype builds `--release` while `rust.yml`'s `build` is debug — a
mixed-profile inconsistency under one `moon ci :build` (review S8). Both are deferred to a
**tracked follow-up issue** (Out of scope), not done here.

### Source files

Every `.rs` file starts with `// SPDX-License-Identifier: Apache-2.0` (CONTRIBUTING
requirement). `paigasus-kernel/src/lib.rs` and `paigasus-py-bindings/src/lib.rs` are
otherwise empty; `paigasus-gateway/src/main.rs` is `fn main() {}`.

## Verification / done criteria

Maps to the AC, plus Moon gates proving the Option-B wiring.

- `cargo build --workspace` → exit 0.
- `cargo fmt --check` (all crates) → exit 0.
- `cargo clippy --workspace -- -D warnings` → exit 0.
- `cargo nextest run --workspace` → exit 0 (no tests; **see open item #1**).
- `moon ci :build` → exit 0 — all three Rust crates build through Moon's affected-graph.
- `moon ci :test` → exit 0 — Rust test tasks run cleanly as a no-op.
- `git status` clean after commit: `target/` ignored, `Cargo.lock` committed, no stray
  Moon cache state.
- `rs/README.md` status line updated.
- PR opened against `main` from the feature branch; `moon ci` green in CI.

> The raw `cargo …` gates assume the Rust toolchain is already provisioned by a prior Moon
> run (`moon ci`/`moon sync` installs it per `.moon/toolchain.yml`). On a fresh clone, run a
> Moon target first or the `cargo` binary won't be on PATH (review N6 — this feeds SMA-363's
> fresh-clone AC). The `moon ci :build`/`:test` gates are the toolchain-agnostic entry point.

## Open items to confirm during implementation

1. **`cargo-nextest` on zero tests.** Recent nextest versions make "no tests found" a
   non-zero exit by default (to catch filter typos), which would fail the AC's
   "runs cleanly" gate on an empty workspace. Two-part mitigation: `--no-tests=pass` on the
   test command, **and** `cargo-nextest` pinned to a concrete version in
   `.moon/toolchain.yml` via the `cargo-nextest@<ver>` `bins` form (verified Moon 2.2.5's
   `BinEntry` schema accepts `bin@version`, installed through `cargo binstall`) so the flag's
   availability is locked, not left to whatever `binstall` resolves. Confirm the exact flag
   spelling against the pinned version at implementation.
2. **`$project` token + Rust language auto-detection** in the pinned Moon 2.2.5 — confirm
   `$project` interpolates in `command`, and that Moon applies `.moon/tasks/rust.yml` to
   the crates via `Cargo.toml`-based language detection (no explicit `language: rust`
   needed). Fallback if detection misbehaves: add a one-line `moon.yml` (`language: rust`)
   per crate, or tag-scope the task file.
3. **Resolved dependency versions.** Confirm the latest compatible minor for each
   `[workspace.dependencies]` entry at implementation (esp. axum 0.8.x, reqwest 0.12.x,
   thiserror 2.x) and freeze in `Cargo.lock`.

## Linear AC corrections to flag

- `edition` should be **2024**, not 2021 (decision §2).
- `resolver` should be **3**, not 2 (decision §5; MSRV-aware, edition-2024-aligned).
- The `cargo nextest run --workspace` gate needs **`--no-tests=pass`** (or equivalent) to
  exit cleanly on a test-less workspace (open item #1).

## Review incorporation

Staff-eng review pass (review doc removed after incorporation, per repo convention). Each
item was evaluated against the codebase and the prior brainstorm decisions before acting.

| Item | Disposition |
|------|-------------|
| **B1** Cargo.lock pins almost nothing | **Adopted (claim corrected).** Fixed the misleading "frozen in Cargo.lock" wording; added a comment in `[workspace.dependencies]`. **Rejected B1(b)** (gateway consuming `tokio`) — violates the minimal-stub decision and adds compile cost to demonstrate a known Cargo mechanism. |
| **B2** `clippy::all = "deny"` brittle | **Adopted (hybrid).** rustc `warnings = "deny"` (in-source), clippy `all = "warn"` + CI `-D warnings` as the hard gate. Reverses the brainstorm's deny-everywhere after the user re-decided. |
| **S1** cdylib without rlib | **Adopted** — explanatory comment; `rlib` deferred (YAGNI). |
| **S2** MSRV symbolic under resolver 2 | **Adopted** — switched to `resolver = "3"` (user re-decided); `rust-version` now enforced at resolution. |
| **S3** tokio features too minimal | **Partly rejected.** Nothing consumes tokio yet and features union/can't be removed per-crate, so the baseline stays minimal with per-crate opt-in (consistent with N8). reqwest posture documented. |
| **S4** async-trait reflex | **Adopted** — discipline comment by the workspace dep. |
| **S5** nextest not version-pinned | **Adopted** — pin `cargo-nextest@<ver>` in `.moon/toolchain.yml` (verified schema). |
| **S6** workspace-mode vs per-project | **Adopted (note).** Documented that per-project tasks satisfy the AC's `--workspace` gates; `test-workspace` task deferred. |
| **S7** no `[profile.release]` | **Deferred.** No release build in this issue; add at first service. |
| **S8** template `--release` vs `rust.yml` debug | **Adopted as tracked follow-up issue** — a latent mixed-profile bug, promoted from "future cleanup". |
| **S9** kernel `publish=false` flip | **Adopted** — `TODO(<issue>)` comment + follow-up issue. |
| **N1** kernel forbid unsafe | **Adopted (note).** Use `#![forbid(unsafe_code)]` crate attribute when kernel has code (Cargo `[lints]` can't mix `workspace = true` with extras). |
| **N2** clippy `--all-targets` | **Adopted.** |
| **N3** `fmt -p` vs `--all` | **Adopted (note).** |
| **N4** cargo-deny / cargo-machete | **Adopted as follow-up issue.** |
| **N5** reserve `[workspace.metadata]` | **Deferred** to release-tooling work (ADR-0010). |
| **N6** cargo-direct needs provisioned toolchain | **Adopted (note)** in verification — feeds SMA-363. |
| **N7** Notion scoping-doc drift | **Adopted as external action** (needs sign-off; not a repo edit). |
| **N8** tokio `macros` in libs | **Adopted** — minimal baseline + comment (see S3). |

## References

- **ADR-0003** — One public polyglot monorepo; `libs`/`bindings`/`services` sub-grouping.
- **ADR-0005** — `paigasus-kernel` (pure logic) bound via PyO3/napi/wasm; binding crate
  names (`paigasus-py-bindings`, `paigasus-node-bindings`, `paigasus-wasm`).
- **ADR-0008** — Moon as the polyglot task orchestrator (why Rust joins the affected-graph).
- **Rust development guidelines** (Notion) — `fmt`/`clippy -D warnings`/`nextest`
  mandatory; `thiserror` in libs; hexagonal layout for services (deferred here).
- **SMA-356 design** — `docs/superpowers/specs/2026-05-26-moon-configuration-design.md`
  (deferred `.moon/tasks/rust.yml` to this issue; Rust generator template; toolchain pins
  Rust 1.95.0 / cargo-nextest).
