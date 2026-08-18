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
  :deny :osv :machete :actionlint :typecheck :breaking :affected-smoke :parity-corpus-drift
  :next-env-drift :wasm-getrandom-free :redis-connect-single-site :iam-docker-policy-single-site
  :promtool :observability-drift :nats-permissions :release-parity :release-parity-py
  :release-parity-ts :publish-metadata --base origin/main --include-relations`.
- A new Rust crate reds `:affected-smoke` until it's added to the `lockfile->all-lint` expected set
  in `ci/affected-graph/run.sh` — that case lists **every** crate, so **every** new crate changes it
  (SMA-534) — and, if it `dependsOn` `paigasus-kernel-rs`, to the `kernel->bindings` set as well
  (strict-equality guard, SMA-409). The parity gate's A4 needs no update: a new crate inherits
  `lint`'s workspace inputs from `.moon/tasks/rust.yml`. New workspace deps may need
  `rs/deny.toml` `[licenses] exceptions` or a dev-only
  `[advisories] ignore` (Rust); an npm/pip advisory needs a version bump — a pnpm-workspace
  `overrides:` selector or `uv lock --upgrade-package` — or a justified `osv-scanner.toml`
  waiver; a dep consumed only by a later commit needs a temporary
  `[package.metadata.cargo-machete] ignored` allowlist (prune once consumed).
- Moon 2.3.2's Rust toolchain resolves `path = "…"` Cargo deps into the project graph **automatically**
  (`moon query projects` labels them `source=implicit`), but does **not** resolve `workspace = true`
  inheritance. So a `{ workspace = true }` in-tree dep — the repo's default form — **must** be
  hand-declared in `dependsOn`, while a `path` dep needs nothing. This is why the drift was scattered
  rather than systematic, and it is the opposite of the "Cargo path deps are NOT auto-synced" claim
  that SMA-389 recorded and SMA-524 disproved. Either way the project edge alone is **not enough**:
  `dependsOn` is what `moon query projects --affected` follows, and a task-level `^:build` is what
  actually schedules the upstream's build under `moon ci --include-relations` — neither implies the
  other. `repo:affected-smoke` now asserts both generically for every crate
  (`ci/affected-graph/cargo_moon_parity.py`), so a new in-tree dep that forgets either one reds CI
  instead of silently under-building (SMA-524).
- Bash tool PATH lacks the proto-managed CLIs; prefix commands with
  `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` so moon/uv/buf/nextest resolve to
  the repo-pinned versions (shims first).
- `paigasus-iam`'s Docker-backed suites get their retry budget and container-concurrency cap from
  `rs/.config/nextest.toml` (`profile.default`), so **Moon, `moon run …:test`, and a bare
  `cargo nextest` all pick it up** — but `cargo test` does NOT, since nextest config is
  nextest-only. Don't add `--retries` to a Moon task or a doc: that recreates the
  documented-vs-executed split SMA-521 closed. A test that fails every attempt still reds; one
  that passes on a retry is reported FLAKY. The JUnit report itself is NOT on `profile.default` —
  nextest resolves a profile's report path relative to the shared workspace `target/`, so `moon
  ci`'s 15+ concurrent nextest runs would clobber a report left on `default`. It lives on a
  dedicated `[profile.iam]` instead, selected only by `paigasus-iam-rs:test`'s `args: ['--profile',
  'iam']` — CI uploads it as the `nextest-junit` artifact, but a bare `cargo nextest run -p
  paigasus-iam` writes no report at all.
- `paigasus-iam`'s **Docker-backed** suites (58 of its 62 integration binaries) skip when the
  daemon is unreachable, and that skip is deliberately quiet — nextest discards a passing test's
  stderr and Moon discards a passing task's output, so no message can surface there. What makes
  it visible is `tests/docker_preflight.rs`, a canary that FAILS when Docker is unreachable: a
  Docker-less run yields exactly one red instead of 57 silent passes (SMA-538). The policy itself
  lives once, in `tests/support/docker.rs`, and `repo:iam-docker-policy-single-site` fails if a
  new suite hand-rolls its own copy. Two env vars, both parsing `1`/`true`/`yes` (anything else,
  including `0`, is off — unlike `CI`, which is presence-based):
  `PAIGASUS_REQUIRE_DOCKER=1` turns every suite's skip into a panic, which is what a FILTERED run
  (`--test relay_pg`, `-E 'test(foo)'`) needs, since the canary is not in that filter.
  `PAIGASUS_SKIP_DOCKER=1` restores skipping everywhere including the canary — it is a
  per-invocation escape hatch for a Docker Hub rate limit or a daemon restart, **not** a
  shell-profile setting, and a `moon run` that greened under it leaves a cached PASS that replays
  after Docker returns, so follow it with `moon run … --force`. `CI` outranks both, so no
  workflow-file env var can green a CI run that tested nothing. A container that fails with a
  REACHABLE daemon is a hard failure by default — including `keycloak_e2e`'s 240s startup
  timeout, which used to be a fast local skip — though `PAIGASUS_SKIP_DOCKER=1` still downgrades
  it to a skip, since that hatch is checked before any classification happens. A stray `CI=false` still counts as "CI present" (the
  check is presence-based, not value-based) — clear it with `env -u CI cargo nextest run -p
  paigasus-iam`.
- Broad `inputs: ['**/*']` Moon tasks (e.g. `repo:actionlint`) stay cheap only because
  `.moon/workspace.yml`'s `hasher.ignorePatterns` filters gitignored trees out of the hash walk.

## Workflow

Specs/plans live in `docs/superpowers/specs/` and `docs/superpowers/plans/` (date-prefixed,
per Linear issue). Work flows brainstorm → spec → plan → implement. Linear keys are `SMA-NNN`;
PRs auto-link to Linear by branch name (don't attach links manually).
