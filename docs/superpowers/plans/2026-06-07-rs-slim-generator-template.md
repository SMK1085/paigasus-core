# Slim Rust generator template & unify build profile — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `.moon/tasks/rust.yml` the single source of Rust build tasks, add a dedicated `build-release` task, slim the generator's `service` archetype to inherit those tasks (removing the latent `cargo build --release` mixed-profile bug), and reconcile the checked-in gateway crate's `dependsOn`.

**Architecture:** Purely mechanical Moon config change across three files. No code, no codegen wiring. The `contracts:generate` build edges and `^:build` affected-graph ordering are explicitly **out of scope** — they are owned by SMA-389 and land with the first real protos. Builds keep compiling the committed generated code offline (ADR-0004).

**Tech Stack:** Moon 2.2.5 (proto-managed), Cargo workspace under `rs/`, Tera templates (`.moon/templates/rust/`).

**Spec:** [`docs/superpowers/specs/2026-06-07-rs-slim-generator-template-design.md`](../specs/2026-06-07-rs-slim-generator-template-design.md)

---

## Setup (read first)

Moon/cargo/buf are **proto-managed and off the Bash tool's default PATH**. Every command block below begins by exporting the proto dirs. There is **no macOS `timeout`** — don't wrap commands in it. First `cargo build` may fetch crates from crates.io (needs network); subsequent builds are cached.

```bash
export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core
```

## File Structure

| File | Change | Responsibility |
|------|--------|----------------|
| `.moon/tasks/rust.yml` | Modify | Language-scoped Rust tasks for every crate. Gains `build-release` (debug `build` unchanged). |
| `.moon/templates/rust/moon.yml` | Modify | Generator output for new crates. `service` archetype slimmed to `dependsOn` only; inherits all tasks. |
| `rs/crates/services/paigasus-gateway/moon.yml` | Modify | The one live service. Gains the static `dependsOn` the slimmed template now emits. |
| `rs/crates/libs/paigasus-proto/moon.yml` | **Not touched** | Its `contracts:generate` edge is SMA-389's, not this issue's. |

---

## Task 1: Add `build-release` task to `.moon/tasks/rust.yml`

**Files:**
- Modify: `.moon/tasks/rust.yml`

- [ ] **Step 1: Verify the task does not exist yet (failing baseline)**

```bash
export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core
moon run :build-release
```

Expected: Moon **fails / reports no matching task** for `build-release` (no project defines it yet). This confirms the task is genuinely new.

- [ ] **Step 2: Add the `build-release` task**

Edit `.moon/tasks/rust.yml`. Insert `build-release` immediately after the `build` task. The full `tasks:` block must read exactly:

```yaml
tasks:
  build:
    command: 'cargo build'
    inputs: ['@group(sources)', 'Cargo.toml']
  build-release:
    command: 'cargo build --release'
    inputs: ['@group(sources)', 'Cargo.toml']
  test:
    command: 'cargo nextest run --no-tests=pass'
    inputs: ['@group(sources)', '@group(tests)', 'Cargo.toml']
  lint:
    command: 'cargo clippy --all-targets -- -D warnings'
    inputs: ['@group(sources)', '@group(tests)', 'Cargo.toml']
  fmt:
    command: 'cargo fmt --check'
    inputs: ['@group(sources)']
```

(Only the two `build-release` lines are added; `build`/`test`/`lint`/`fmt` and everything above `tasks:` stay byte-for-byte the same.)

- [ ] **Step 3: Verify the task now exists and runs**

```bash
export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core
moon run :build-release
```

Expected: PASS — Moon runs `build-release` for all four Rust crates (`paigasus-kernel-rs`, `paigasus-proto-rs`, `paigasus-py-bindings-rs`, `paigasus-gateway-rs`).

- [ ] **Step 4: Confirm it produced release artifacts**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core
ls rs/target/
```

Expected: a `release/` directory is present.

- [ ] **Step 5: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core
git add .moon/tasks/rust.yml
git commit -m "feat(rs): add build-release task to rust.yml (SMA-374)"
```

(The `commit-msg` lefthook runs commitlint — the message above is Conventional-Commit compliant.)

---

## Task 2: Slim the `service` archetype in `.moon/templates/rust/moon.yml`

**Files:**
- Modify: `.moon/templates/rust/moon.yml`

- [ ] **Step 1: Capture the current (buggy) template output as a baseline**

```bash
export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core
moon generate rust --to .tmp-sma374-smoke/before --force -- --name throwaway-svc --archetype service
cat .tmp-sma374-smoke/before/moon.yml
```

Expected: the generated file still contains a `tasks:` block with `command: 'cargo build --release'` and `deps: ['contracts:generate', '^:build']` — i.e. the bug, present before the fix.

- [ ] **Step 2: Replace the template with the slimmed version**

Overwrite `.moon/templates/rust/moon.yml` so its **entire** contents are exactly:

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'
id: '{{ name }}-rs'
layer: '{% if archetype == "service" %}application{% else %}library{% endif %}'
language: 'rust'
{%- if archetype == "service" %}
dependsOn:
  - 'paigasus-proto-rs'
  - 'paigasus-kernel-rs'
{%- endif %}
```

The whole `tasks:` block is removed; the `{%- if archetype == "service" %}` now wraps only `dependsOn`.

- [ ] **Step 3: Verify the slimmed service output**

```bash
export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core
moon generate rust --to .tmp-sma374-smoke/svc --force -- --name throwaway-svc --archetype service
cat .tmp-sma374-smoke/svc/moon.yml
```

Expected output exactly:

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'
id: 'throwaway-svc-rs'
layer: 'application'
language: 'rust'
dependsOn:
  - 'paigasus-proto-rs'
  - 'paigasus-kernel-rs'
```

Confirm: **no `tasks:` block**, **no `cargo build --release`**, **no `contracts:generate`**.

- [ ] **Step 4: Verify the `library` archetype is unchanged (no deps, no tasks)**

```bash
export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core
moon generate rust --to .tmp-sma374-smoke/lib --force -- --name throwaway-lib --archetype library
cat .tmp-sma374-smoke/lib/moon.yml
```

Expected output exactly:

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'
id: 'throwaway-lib-rs'
layer: 'library'
language: 'rust'
```

Confirm: **no `dependsOn`**, **no `tasks:`**.

- [ ] **Step 5: Clean up smoke-test residue (do NOT trust `git status` alone — F4)**

```bash
export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core
rm -rf .tmp-sma374-smoke
moon clean
git status --short
```

Expected: `git status --short` shows only the modified `.moon/templates/rust/moon.yml` (the `.tmp-sma374-smoke/` dir is gone; `moon clean` clears any cache residue). The smoke dir was written to a top-level path **not** matched by any `projects.globs`, so Moon never registered it as a project — `moon clean` is belt-and-suspenders.

- [ ] **Step 6: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core
git add .moon/templates/rust/moon.yml
git commit -m "fix(rs): slim service template, drop --release override (SMA-374)"
```

---

## Task 3: Reconcile `paigasus-gateway` `dependsOn`

**Files:**
- Modify: `rs/crates/services/paigasus-gateway/moon.yml`

- [ ] **Step 1: Confirm the gateway currently has no `dependsOn`**

```bash
export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core
moon project paigasus-gateway-rs
```

Expected: the printed config shows the inherited `build`/`build-release`/`test`/`lint`/`fmt` tasks but **no** `Depends on` entries.

- [ ] **Step 2: Add the static `dependsOn`**

Overwrite `rs/crates/services/paigasus-gateway/moon.yml` so its **entire** contents are exactly:

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

id: 'paigasus-gateway-rs'
layer: 'application'
language: 'rust'

dependsOn:
  - 'paigasus-proto-rs'
  - 'paigasus-kernel-rs'
```

(No task overrides — the gateway inherits everything from `rust.yml`.)

- [ ] **Step 3: Verify the dependency edge and inherited tasks**

```bash
export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core
moon project paigasus-gateway-rs
```

Expected: the config now lists `paigasus-proto-rs` and `paigasus-kernel-rs` under depends-on, and still shows the inherited `build`/`build-release`/`test`/`lint`/`fmt` tasks with **no** per-crate command overrides (no `--release`).

- [ ] **Step 4: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core
git add rs/crates/services/paigasus-gateway/moon.yml
git commit -m "feat(rs): wire paigasus-gateway dependsOn proto/kernel (SMA-374)"
```

---

## Final Verification (clean-room uniform-debug + regression sweep)

No code changes here — this proves the spec's primary acceptance criterion ("`moon ci :build` produces a uniform debug profile"). Run after Task 3.

- [ ] **Step 1: Clean the cargo target so the profile check is unambiguous**

```bash
export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core
rm -rf rs/target
```

- [ ] **Step 2: Build all Rust crates via Moon's `build` task (debug)**

```bash
export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core
moon run paigasus-kernel-rs:build paigasus-proto-rs:build paigasus-py-bindings-rs:build paigasus-gateway-rs:build
```

Expected: PASS for all four crates. (Explicit crate list keeps this Rust-only; `moon ci :build --base origin/main` is the CI-affected equivalent.)

- [ ] **Step 3: Assert a uniform debug profile — no `release/` from `:build`**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core
ls rs/target/
```

Expected: `debug/` is present and **`release/` is ABSENT**. This is the mixed-profile bug fix verified: no crate's `build` produces release artifacts.

- [ ] **Step 4: Confirm `build-release` still produces release artifacts on demand**

```bash
export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core
moon run paigasus-gateway-rs:build-release
ls rs/target/
```

Expected: PASS, and now both `debug/` and `release/` are present.

- [ ] **Step 5: Regression sweep — the rest of the Rust task graph still passes**

```bash
export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core
moon run paigasus-kernel-rs:test paigasus-proto-rs:test paigasus-py-bindings-rs:test paigasus-gateway-rs:test
moon run paigasus-kernel-rs:fmt paigasus-proto-rs:fmt paigasus-py-bindings-rs:fmt paigasus-gateway-rs:fmt
moon run paigasus-kernel-rs:lint paigasus-proto-rs:lint paigasus-py-bindings-rs:lint paigasus-gateway-rs:lint
```

Expected: tests PASS (`--no-tests=pass` tolerates crates with no tests), `fmt` and `lint` PASS (only YAML changed). Scoped to the Rust crates so unrelated `contracts`/`py`/`ts` `fmt`/`lint` tasks (which need `buf`/network) don't enter this sweep.

- [ ] **Step 6: Confirm the working tree is clean**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core
git status --short
```

Expected: empty (all three edits committed, no stray smoke/temp files).

---

## Definition of Done

- [ ] `.moon/tasks/rust.yml` defines `build-release` (`cargo build --release`); `build` remains debug. *(Spec §Changes 1)*
- [ ] `.moon/templates/rust/moon.yml` `service` archetype emits `dependsOn` only — no `tasks:`, no `--release`, no `contracts:generate`. `library` archetype unchanged. *(Spec §Changes 2)*
- [ ] `rs/crates/services/paigasus-gateway/moon.yml` declares `dependsOn: [paigasus-proto-rs, paigasus-kernel-rs]` with no task overrides. *(Spec §Changes 3)*
- [ ] `rs/crates/libs/paigasus-proto/moon.yml` is untouched. *(Spec §Not changed)*
- [ ] `moon run :build` across the Rust crates yields a uniform debug profile (`rs/target/release` absent). *(Spec §Verification)*
- [ ] `build-release` is **not** added to CI's `T=(…)` array (release profile activation tracked by SMA-407). *(Spec §Design decisions)*
- [ ] No `contracts:generate` / `^:build` edges introduced (deferred to SMA-389). *(Spec §Out of scope)*
- [ ] Three commits, working tree clean.
```
