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
  currently `buffer-only-failure`). Moon 2.3.2 has no per-invocation CLI flag
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

- Moon is 2.3.2: `vcs.client` (not `manager`), `codeowners.sync` (not `syncOnRun`),
  Python/uv toolchains keyed `unstable_python` + a separate `unstable_uv`.
- `cargo nextest` exits non-zero on a workspace with **no tests** — use `--no-tests=pass`.
- `.github/CODEOWNERS` is Moon-generated — don't hand-edit.
- `vcs.hooks` is intentionally empty; lefthook will own `.git/hooks` (SMA-371).
- A fresh `git worktree` starts with **no installed deps** (empty `ts/node_modules`, no
  `py/.venv`, unfetched cargo) — but lefthook's git hooks are shared across worktrees via the
  common `.git`, so `commit-msg` runs `commitlint` and **fails the commit** (`commitlint not
  found`) until deps exist. After `git worktree add`, provision the worktree before committing:
  `proto install` → `pnpm -C ts install` (installs commitlint + re-syncs hooks via its
  `prepare` step) → `uv sync` in `py/` → `cargo fetch` in `rs/`. Do **not** bypass the hook with
  `--no-verify`; install the deps.
- Never name a source file with a base name that is a **Windows reserved device name**
  (`CON`, `PRN`, `AUX`, `NUL`, `COM1`–`COM9`, `LPT1`–`LPT9`) — `PRN.<ext>` etc. are reserved
  too, so git can't check the file out on Windows (`error: invalid path …`). The Linux-only
  `CI` gate passes; only the Windows `prebuild` matrix job catches it — and `prebuild` runs
  ONLY on push-to-`main` / `workflow_dispatch`, NOT on PRs, so the bad path is green on the PR
  and reds `main` after merge (SMA-448: `prn.rs` → `resource_name.rs`). An underscore/hyphen
  suffix (`prn_canonical`, `prn-fields`) is fine.
- Per-project Moon tasks (`<proj>:build/test/lint/fmt`) do NOT run the repo-level gates
  (`:deny`, `:osv`, `:machete`, `:affected-smoke`, codegen-drift, CODEOWNERS). Before pushing
  new crates/deps/proto, run the full graph like CI does: `moon ci :build :test :lint :fmt
  :deny :osv :machete :typecheck :breaking :affected-smoke :parity-corpus-drift
  :next-env-drift :wasm-getrandom-free :redis-connect-single-site :promtool :observability-drift
  :nats-permissions :release-parity :release-parity-py :release-parity-ts --base origin/main
  --include-relations`.
- A new crate that `dependsOn` `paigasus-kernel-rs` reds `:affected-smoke` until it's added to
  the `kernel->bindings` expected set in `ci/affected-graph/run.sh` (strict-equality guard,
  SMA-409). New workspace deps may need `rs/deny.toml` `[licenses] exceptions` or a dev-only
  `[advisories] ignore` (Rust); an npm/pip advisory needs a version bump — a pnpm-workspace
  `overrides:` selector or `uv lock --upgrade-package` — or a justified `osv-scanner.toml`
  waiver; a dep consumed only by a later commit needs a temporary
  `[package.metadata.cargo-machete] ignored` allowlist (prune once consumed).
- Bash tool PATH lacks the proto-managed CLIs; prefix commands with
  `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` so moon/uv/buf/nextest resolve to
  the repo-pinned versions (shims first).

## Workflow

Specs/plans live in `docs/superpowers/specs/` and `docs/superpowers/plans/` (date-prefixed,
per Linear issue). Work flows brainstorm → spec → plan → implement. Linear keys are `SMA-NNN`;
PRs auto-link to Linear by branch name (don't attach links manually).
