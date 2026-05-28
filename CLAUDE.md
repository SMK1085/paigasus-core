# paigasus-core

Public, Apache-2.0 polyglot monorepo for Paigasus, orchestrated by Moon. Four workspaces:
`contracts/` (protobuf + buf), `rs/` (Cargo: libs/bindings/services), `py/` (uv),
`ts/` (pnpm). **Status: bootstrapping** — workspaces are scaffolded issue-by-issue, so a
dir may hold only a README until its setup issue lands.

## Setup & commands

Tooling runs through [Moon](https://moonrepo.dev), pinned via proto in `.prototools`.
First-time setup: see [CONTRIBUTING.md](./CONTRIBUTING.md#local-development) (`proto install` → `moon`).

- `moon ci :build` / `moon ci :test` — affected build/test graph. **Moon 2.x needs explicit
  targets**; bare `moon ci` errors in non-TTY.
- Task output style is set in `.moon/tasks.yml` (`taskOptions.outputStyle`,
  currently `buffer-only-failure`). Moon 2.2.5 has no per-invocation CLI flag
  for it; to stream a specific task locally, set `options.outputStyle: 'stream'`
  on the task definition.
- Rust (in `rs/`): `cargo build --workspace`, `cargo fmt --check`,
  `cargo clippy --workspace -- -D warnings`, `cargo nextest run --workspace`.

## Architecture

- `contracts/` — proto source of truth; buf generates Rust/Py/TS bindings (ADR-0004).
- `rs/crates/{libs,bindings,services}/` — `libs` = pure crates (e.g. `paigasus-kernel`),
  `bindings` = FFI shims (PyO3/napi/wasm), `services` = binaries. Service crates follow
  hexagonal architecture; libs/bindings do not.
- Cross-language behavior lives once in `paigasus-kernel` (Rust), bound to Py/Node/WASM —
  never reimplemented per language (ADR-0005).

## Conventions

- Every source file opens with an SPDX header: `// SPDX-License-Identifier: Apache-2.0`
  (`#` for Python).
- Branches: `feature/sma-NNN-<slug>` off `main` (NOT the old `sven/...` form).
- Conventional commits with a workspace scope: `feat(rs): …`, `fix(contracts): …`.
- Rust crates use **edition 2024 + rust-version 1.95**, even when an issue's AC says 2021.
- Significant choices get a Notion ADR before code; conventions live in the Notion
  Development Guidelines (both linked from CONTRIBUTING.md).

## Gotchas

- Moon is 2.2.5: `vcs.client` (not `manager`), `codeowners.sync` (not `syncOnRun`),
  Python/uv toolchains keyed `unstable_python` + a separate `unstable_uv`.
- `cargo nextest` exits non-zero on a workspace with **no tests** — use `--no-tests=pass`.
- `.github/CODEOWNERS` is Moon-generated — don't hand-edit.
- `vcs.hooks` is intentionally empty; lefthook will own `.git/hooks` (SMA-371).

## Workflow

Specs/plans live in `docs/superpowers/specs/` and `docs/superpowers/plans/` (date-prefixed,
per Linear issue). Work flows brainstorm → spec → plan → implement. Linear keys are `SMA-NNN`;
PRs auto-link to Linear by branch name (don't attach links manually).
