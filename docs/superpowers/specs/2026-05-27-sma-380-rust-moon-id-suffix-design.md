# SMA-380 — Suffix Rust Moon project ids with `-rs` for cross-stack consistency

**Status:** Designed (brainstorming complete; staff-eng review incorporated)
**Date:** 2026-05-27
**Linear:** [SMA-380](https://linear.app/smaschek/issue/SMA-380/suffix-rust-moon-project-ids-with-rs-for-cross-stack-consistency)
**Branch:** `feature/sma-380-suffix-rust-moon-project-ids-with-rs-for-cross-stack`
**References:** SMA-358 (shipped the `-py` side), SMA-357 (landed the Rust crates), SMA-356 (Moon config + templates).

## Goal

Give every Rust crate's Moon project an explicit `-rs`-suffixed id (`paigasus-kernel-rs`,
`paigasus-py-bindings-rs`, `paigasus-gateway-rs`), mirroring the `-py` suffix SMA-358 applied to
the Python packages. This is a **consistency / symmetry** change, not a collision fix: SMA-358
already moved the Python side to `-py`, so the bare Rust ids clash with nothing today. The work
closes the cross-stack naming symmetry so the convention (`<name>-<stack>` for every leaf project)
holds uniformly.

Only **Moon project ids** change. Cargo crate names, the `rs/` Cargo workspace, and the directory
layout are untouched — `cargo build --workspace`, `clippy`, `fmt`, `nextest` all behave exactly as
before.

**What is being mirrored is the naming convention, not the topology.** After this PR both stacks
are *named* consistently (`-py` / `-rs` on every leaf), but they remain structurally asymmetric by
design: the py side has a task-bearing parent project (`py`) with alias-only leaves, whereas the rs
side has task-bearing leaves and no parent — Rust's language-inherited `.moon/tasks/rust.yml` is the
equivalent of py's parent project, just expressed through a different Moon primitive. Concretely,
`moon run py:lint` works but there is no `rs:lint`; `moon ci :lint` works for both. Do not read
"mirror the `-py` work" as "the two sides will look identical."

## Context discovered during brainstorming

- **No parent `rs` Moon project exists.** The only Rust-side Moon projects are the three leaf crates
  (`workspace.yml` globs `rs/crates/{libs,bindings,services}/*`). `rs/` is the Cargo workspace root
  but carries no `moon.yml`. This differs from the Python side, where a `py` parent project owns the
  workspace-wide gates; Rust's shared config instead lives in the language-inherited
  `.moon/tasks/rust.yml`. So "suffix the leaf crates" is the entire scope — there is nothing above
  the crates to rename. (Decision: do **not** add a parent `rs` project; it has no functional payoff.)
- **Moon runs each Rust task from the crate directory.** Verified via `moon run … --log debug`:
  the cargo command executes with `cwd=…/rs/crates/libs/paigasus-kernel` (and `MOON_PROJECT_ROOT`
  set to the same). So bare `cargo build` (no `-p`) targets exactly that crate's package.
- **There are no Moon project aliases.** The graph build logs `Loaded 0 project aliases`, and
  `moon project` reports `Toolchain: system` for the crates — Moon runs the cargo commands as plain
  `bash -c "cargo …"` and does **not** parse `Cargo.toml`. Therefore the `$projectAlias` token would
  resolve to nothing; the issue's `$projectAlias` suggestion is not viable without enabling deeper
  Rust-toolchain-plugin integration (a separate initiative — see Out of scope). Getting the alias
  would also re-create `paigasus-kernel` as a live Moon target, partially undoing the disambiguation
  this issue exists to create.

## Key decisions

1. **Keep the `-p` fix id-agnostic by dropping `-p` entirely.** The shared `.moon/tasks/rust.yml`
   currently runs `cargo <cmd> -p $project`. After the rename, `$project` becomes the `-rs` id, which
   Cargo rejects (`-p` expects the crate name). Rather than reintroduce the crate name via an alias
   or hardcode it per crate, drop `-p` and rely on the verified per-crate cwd. The shared task file
   becomes independent of project ids, so no future id change can break it again.
2. **Leaf crates only; `contracts` stays bare as a documented carve-out.** SMA-358 suffixed *all*
   py packages uniformly (including non-clashing ones like `paigasus-ml-py`), so the convention is
   "every language-stack leaf carries its stack suffix" — `-rs` / `-py` / `-ts`. `contracts` is the
   one leaf that is *not* a language stack: it is the language-neutral protobuf/buf IDL source. It is
   therefore intentionally unsuffixed — a principled carve-out (a different category, not different
   treatment of the same category), not an oversight. This is the single documented exception and it
   matches the issue's own "likely leave bare" lean. Note there is no actual collision risk even
   after SMA-360, because the proto *binding* packages it adds are named `paigasus-proto-{rs,py,ts}`
   — a different name from `contracts`.
3. **Update the scaffold template so generated crates are consistent.** Beyond the three existing
   crates, the `.moon/templates/rust/` scaffold must emit `-rs` ids and `-rs` `dependsOn`, and adopt
   the no-`-p` convention — otherwise `moon generate` would keep producing inconsistent/broken ids.
4. **No manual `alias:` either — one canonical name.** Moon supports a one-line `alias:` per
   `moon.yml` that would make `moon run paigasus-kernel:build` *also* resolve. We deliberately do not
   add one: the `-rs` suffix is the single canonical Moon id, and having the bare `paigasus-kernel`
   work as an alias would re-create the asymmetric, ambiguous naming this issue exists to remove.
   (This is separate from the `$projectAlias` / toolchain-plugin path, also rejected — see Context.)

## A. Crate `moon.yml` — explicit ids

Add one line to each crate's `moon.yml` (no SPDX header — these config files carry none today, matching the `-py` files):

| File | Add |
|---|---|
| `rs/crates/libs/paigasus-kernel/moon.yml` | `id: 'paigasus-kernel-rs'` |
| `rs/crates/bindings/paigasus-py-bindings/moon.yml` | `id: 'paigasus-py-bindings-rs'` |
| `rs/crates/services/paigasus-gateway/moon.yml` | `id: 'paigasus-gateway-rs'` |

`paigasus-py-bindings-rs` doubles the language hint by design: the crate is a Rust artifact (`-rs`)
that *produces* Python bindings (`py-bindings`). The two hints describe different things (the `-rs`
is the stack the crate lives in; `py-bindings` is what it emits), so this is consistent with the
convention. Renaming the underlying Cargo crate to remove the doubled hint is intentionally out of
scope (churn for a cosmetic gain). The same will apply to future `paigasus-node-bindings-rs` /
`paigasus-wasm-rs`.

## B. `.moon/tasks/rust.yml` — drop `-p $project`

Commands become bare; `inputs`, `inheritedBy`, and `fileGroups` are unchanged:

```yaml
tasks:
  build:
    command: 'cargo build'
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

`--no-tests=pass` stays (nextest exits non-zero on a crate with no tests — a CLAUDE.md gotcha).

## C. `.moon/templates/rust/` — consistent generated crates

The current template (a) sets no `id`, (b) has `dependsOn: ['paigasus-proto', 'paigasus-kernel']`
(bare ids that won't exist post-rename), and (c) redundantly redefines all four tasks — libraries
duplicate the inherited file, and the template's `test` is even missing `--no-tests=pass`. The new
`moon.yml`:

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'
id: '{{ name }}-rs'
type: '{% if archetype == "service" %}application{% else %}library{% endif %}'
language: 'rust'
{%- if archetype == "service" %}
dependsOn:
  - 'paigasus-proto-rs'
  - 'paigasus-kernel-rs'
tasks:
  build:
    command: 'cargo build --release'
    inputs: ['@group(sources)', 'Cargo.toml']
    deps: ['contracts:generate', '^:build']
  test:
    command: 'cargo nextest run --no-tests=pass'
    inputs: ['@group(sources)', '@group(tests)', 'Cargo.toml']
    deps: ['contracts:generate']
{%- endif %}
```

Result: **libraries inherit all four tasks** from `.moon/tasks/rust.yml` (matching the hand-written
crates — no redundant block); **services override only `build`/`test`** for their `--release` +
`contracts:generate` deltas and inherit `lint`/`fmt`.

`template.yml`'s description prose updates too (both must land — easy to miss): the `service`
caveat paragraph (`paigasus-proto` / `paigasus-kernel` → `paigasus-proto-rs` / `paigasus-kernel-rs`)
**and** the `library` caveat's hand-edit note (`Add dependsOn: ['paigasus-kernel']` →
`['paigasus-kernel-rs']`).

**Cargo vs. Moon dependency names are independent layers.** The template's `dependsOn` uses Moon
*project ids* (now `-rs`), but any consuming crate's Cargo `[dependencies]` / `[workspace.dependencies]`
table continues to use the bare crate name (`paigasus-kernel`). Moon's `dependsOn` drives the
affected-graph; Cargo's `[dependencies]` drives compilation — the rename touches only the former.
(No inter-crate Cargo deps exist among the three stubs today, so this is forward-looking guidance.)

## D. Docs

Effectively none. Every `paigasus-kernel` / `paigasus-gateway` / `paigasus-py-bindings` mention in
CLAUDE.md, CONTRIBUTING.md, and the READMEs refers to the **crate name** (unchanged); the
date-prefixed plan/spec docs under `docs/superpowers/` are historical records and stay as-is. A
grep found **no live `moon run <crate>:target` references**. Implementation re-greps to confirm
nothing references a bare Moon target.

## Verification

- `moon project paigasus-kernel-rs` lists the four bare-cargo tasks; the old `moon project
  paigasus-kernel` no longer resolves (renamed).
- `moon run paigasus-kernel-rs:{build,test,lint,fmt}` each succeed and build/test/lint/fmt **only**
  that crate.
- `moon ci :build` and `moon ci :test` resolve across all three `-rs` crates; the `paigasus-*-py`
  projects still resolve (no regression to the Python side).
- Rendering the template (`moon generate`) for a `library` and a `service` produces correct `-rs`
  ids and `-rs` `dependsOn`; the service emits `build`/`test` overrides and inherits `lint`/`fmt`.
- `cargo build --workspace` (run directly in `rs/`) is unaffected — crate names did not change.
- Confirm empirically that bare `cargo fmt --check` (no `-p`, no `--all`) run from a crate dir
  targets **only that crate's** package, not the whole workspace — the pinned `rust 1.95.0` should
  make this stable, but verify rather than assume.
- Expect a **single cold-cache `moon ci` run** the first time after merge: renaming three project
  ids changes the workspace-graph hash, invalidating `.moon/cache/states/workspaceGraph.json` (and
  any CI layer keyed on the Moon graph hash) once before steady-state caching resumes. Not a failure.

## Out of scope

- Enabling the Rust toolchain plugin's deeper integration (Cargo.toml-derived aliases,
  `addMsrvConstraint`, `syncToolchainConfig`, `$projectAlias`). Tracked conceptually as a possible
  future initiative, not here.
- Adding a parent `rs` Moon project for structural symmetry with `py`.
- Harmonizing the rs/py *topologies* (rs has task-bearing leaves; py has a task-bearing parent +
  alias-only leaves). The asymmetry is functional today; revisit when ts lands (SMA-359) and/or when
  py packages get promoted to per-package task-bearing projects.
- Any change to Cargo crate names, the Cargo workspace, or directory layout.
- A `contracts` suffix (see Key decision §2 — intentional carve-out).
