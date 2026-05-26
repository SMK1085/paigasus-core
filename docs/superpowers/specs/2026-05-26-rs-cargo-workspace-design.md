# SMA-357 — Bootstrap `rs/` Cargo workspace (libs / bindings / services)

**Date:** 2026-05-26
**Linear:** [SMA-357](https://linear.app/smaschek/issue/SMA-357/bootstrap-rs-cargo-workspace-with-libsbindingsservices-layout)
**Status:** Design approved (brainstorm); pending spec sign-off → implementation plan
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
- Slimming the existing `.moon/templates/rust/` generator so it defers standard tasks
  to `rust.yml` → **future cleanup** (see decision §3); not touched here to keep
  SMA-356's deliverable stable and this diff focused.

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

### 3. Lint enforcement centralized in `[workspace.lints]`

Rather than relying only on the `cargo clippy -- -D warnings` CLI flag, lint levels are
declared once at the workspace and inherited by every crate (`[lints] workspace = true`):

```toml
[workspace.lints.rust]
warnings = "deny"

[workspace.lints.clippy]
all = "deny"
```

This makes "warnings are errors" bite uniformly — in editors, on plain `cargo build`,
and in CI — not just at the clippy task. **Accepted trade-off:** `deny` fails *local*
builds on any warning, which is strict by design. The softer alternative (`"warn"`
in-source + `-D warnings` only in the Moon `lint`/CI task) was considered and can be
adopted later by flipping the levels if the strictness proves annoying.

`unsafe_code` is deliberately **not** forbidden at the workspace level — binding crates
(PyO3/napi/wasm) will need `unsafe`.

### 4. Minimal stubs (YAGNI)

Each crate is the smallest thing that compiles cleanly: blank `lib.rs` / `fn main() {}`,
each opening with the required SPDX header. The gateway's hexagonal structure (per the
Rust guidelines) is **not** scaffolded now — it lands when the gateway gets real logic,
avoiding empty-module noise and premature structure.

### 5. `resolver = "2"` (per AC)

Kept at `"2"` as the AC specifies. It works fine with edition 2024. The edition-aligned,
MSRV-aware `resolver = "3"` is available as a later bump if desired; not adopted now to
limit deviation to the one (edition) change that materially matters.

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
resolver = "2"

[workspace.package]
edition      = "2024"
license      = "Apache-2.0"
rust-version = "1.95"
authors      = ["Paigasus contributors"]

[workspace.dependencies]
axum = "0.8"
tower = "0.5"
tower-http = "0.6"
tokio = { version = "1", features = ["rt-multi-thread", "macros", "net"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
chrono = { version = "0.4", features = ["serde"] }
uuid = { version = "1", features = ["v4", "serde"] }
bytes = "1"
thiserror = "2"
anyhow = "1"
async-trait = "0.1"
futures = "0.3"
reqwest = { version = "0.12", default-features = false, features = ["rustls-tls", "json"] }

[workspace.lints.rust]
warnings = "deny"

[workspace.lints.clippy]
all = "deny"
```

Caret ranges; exact patches resolved by Cargo and frozen in the committed `Cargo.lock`.
Workspace deps that the stubs don't reference are harmless — Cargo only fetches a dep when
a member opts in via `<dep>.workspace = true`. Exact minor/patch versions are confirmed
against the registry at implementation time.

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

Binding (`paigasus-py-bindings`) adds a `cdylib` crate type; no `pyo3` dependency yet:

```toml
[lib]
crate-type = ["cdylib"]
```

Service (`paigasus-gateway`) uses the same `[package]`/`[lints]` form as the library (no
`[lib]` section); its `src/main.rs` is `fn main() {}`. `version = "0.0.0"` +
`publish = false` mark all three as not-yet-real placeholders.

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
    command: 'cargo clippy -p $project -- -D warnings'
    inputs: ['@group(sources)', 'Cargo.toml']
  fmt:
    command: 'cargo fmt -p $project --check'
    inputs: ['@group(sources)']
```

`$project` resolves to the Moon project id = crate dir name = Cargo package name, so
`-p $project` selects the right package. Cargo discovers the workspace root by walking up
to `rs/Cargo.toml`, so the tasks run correctly from each crate's own directory.

**Relationship to `.moon/templates/rust/` (from SMA-356):** `rust.yml` is the *baseline*;
the generator template stays for producing per-crate `moon.yml` *overrides* when a crate
needs them. Generated per-crate tasks override the inherited ones by name (Moon task
inheritance). The template's standard task definitions are now redundant with `rust.yml`
— noted as a future cleanup (Out of scope), not done here.

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

## Open items to confirm during implementation

1. **`cargo-nextest` on zero tests.** Recent nextest versions make "no tests found" a
   non-zero exit by default (to catch filter typos), which would fail the AC's
   "runs cleanly" gate on an empty workspace. Mitigation baked in: `--no-tests=pass` on
   the test command. Confirm exact behavior/flag spelling against the nextest version
   pinned in `.moon/toolchain.yml` (`cargo-nextest` bin) and adjust if the flag differs.
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
- The `cargo nextest run --workspace` gate needs **`--no-tests=pass`** (or equivalent) to
  exit cleanly on a test-less workspace (open item #1).

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
