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
  (e.g. `:deny`, `:osv`, `:machete`, `:affected-smoke`, codegen-drift, CODEOWNERS). Before pushing
  new crates/deps/proto, run the full graph like CI does. The command between the markers below is
  gated against `ci.yml`'s `T=(…)` array by `repo:affected-smoke` — keep the two identical, and do
  not remove **or quote** the markers: a second copy of either one anywhere in this file, even
  inside backticks in prose, makes the count 2 and reds the gate (SMA-541):
  <!-- ci-targets:begin -->
  `moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site
  :input-liveness :promtool :observability-drift :nats-permissions :release-parity
  :release-parity-py :release-parity-ts :publish-metadata --base origin/main --include-relations`
  <!-- ci-targets:end -->
- A new `repo:*` gate reds `:affected-smoke` until it is in **both** `ci.yml`'s `T=(…)` array and
  the marker-delimited command above — `ci/affected-graph/ci_targets.py` asserts the two agree, and
  that every `T` entry still resolves to a CI-eligible task. That last half matters because
  `moon ci` exits **0** on a target that resolves to nothing (even with real targets around it), so
  a typo is otherwise a silent no-op on every PR. A gate that must stay out of `T` needs a
  `T_EXEMPT` entry with a reason — `runInCI: false` is NOT a general escape, since Moon then drops
  the task from `moon run` under `CI=true` too (see the comments in `ts/moon.yml`). `T` must also
  stay a single-line bash array (SMA-541).
- A `repo:*` task's `inputs` are now asserted **live**: `repo:input-liveness`
  (`ci/affected-graph/task_inputs.py`) fails if a declared glob matches zero tracked files or a
  declared file is untracked, so moving a directory a gate keys on reds CI instead of silently
  switching that gate off. It also asserts its OWN `inputs: ['**/*']` is unchanged — narrowing it
  for cost would make it stop noticing exactly the renames it exists to catch. A genuinely dead
  input needs an `ALLOW_DEAD_INPUT` entry with a reason (SMA-553).
- A new Rust crate reds `:affected-smoke` until it's added to the `lockfile->all-lint` expected set
  in `ci/affected-graph/run.sh` — that case lists **every** crate, so **every** new crate changes it
  (SMA-534) — and, if it `dependsOn` `paigasus-kernel-rs`, to the `kernel->bindings` set as well
  (strict-equality guard, SMA-409). The parity gate's A4 needs no update: a new crate inherits
  `lint`'s workspace inputs from `.moon/tasks/rust.yml`. That case now also carries three non-lint
  rows — `paigasus-kernel-ts:{build,test}` and `paigasus-kernel-py:test`, the tasks that link the
  cdylibs and compile `wasm32` (SMA-546) — so keep them when re-baselining; a new Rust crate does
  not change them. New workspace deps may need
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
  Neither is enough on its own either: task `inputs` are the **only** thing that confers
  affectedness in Moon 2.3.2. `dependsOn` and `^:build` schedule an upstream's build but never
  **select** a downstream — a dependent runs only if independently affected, and neither
  `--include-relations` nor `--downstream` changes that for `moon ci` (measured at the full
  24-target shape: identical action sets with and without both, SMA-528). Every Rust crate therefore
  declares its transitive upstream sources in `fileGroups.upstreams`, consumed by build/test/lint via
  `@group(upstreams)` in `.moon/tasks/rust.yml`. Omitting the group is a hard graph-load error
  (`project::unknown_file_group`) for every moon command; mis-declaring it reds
  `repo:affected-smoke`'s A6 — and **nothing else can**, because a crate's own `moon.yml` is not an
  input to its tasks, so a wrong group otherwise serves a cached PASS. `^:build` has a second job
  here: it orders `contracts:generate` before a downstream that keys on
  `paigasus-proto/src/generated/**`, so removing it as "vestigial" would make those cache keys
  nondeterministic.
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
- `paigasus-iam`'s **Docker-backed** suites (65 of its 69 integration binaries) skip when the
  daemon is unreachable, and that skip is deliberately quiet — nextest discards a passing test's
  stderr and Moon discards a passing task's output, so no message can surface there. What makes
  it visible is `tests/docker_preflight.rs`, a canary that FAILS when Docker is unreachable: a
  Docker-less run yields exactly one red instead of 60 silent passes (SMA-538). The policy itself
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
- Adding a **new error-code emission site** in Rust reds `repo:error-code-single-site` until the file
  is added to `ci/error-registry/check.py`'s `MANIFEST` — as `emits` (which also requires a
  membership test asserting every code it emits resolves via `ErrorReason::from_wire_reason`),
  `asserts`, or `excluded` with a stated reason. The gate matches the registry's **declared**
  vocabulary, so it cannot see a code you invented and never added to
  `contracts/proto/paigasus/common/v1/error.proto`; adding the code there is what makes it
  resolvable on any consumer. Code **removal** needs no gate — both service crates carry
  `test: deps: ['^:build']`, so a contracts change already runs their membership tests.
  (Until SMA-528 this was aspirational: `^:build` schedules an upstream's build, it does not make a
  crate affected. What makes it true is `@group(upstreams)` — both service crates' `test` now key on
  `paigasus-proto`'s sources, so a contracts change that regenerates them selects the test.)
- Workflow trigger filters are gated by `repo:actionlint`. Write `branches:`, `paths:` **and their
  `-ignore` variants** as **block sequences**, never the inline `branches: [main]` form — the
  gate's extractor does not parse inline flow and fails all four keys loudly rather than skipping
  them in silence. Every wildcard-free
  `branches:` entry must resolve as `refs/remotes/origin/<name>`; a branch that does not exist yet,
  or any entry carrying a glob character (`*`, `?`, `+`, `[]` — `+` included, since GitHub reads it
  as a quantifier), needs a justified `BRANCH_SKIP` entry in `ci/actionlint/run.sh`. A typo'd
  branch name otherwise disables a workflow silently and permanently (SMA-540).
- Container images (SMA-500) live behind `ci/images/run.sh {build,smoke,all}` and
  `.github/workflows/images.yml`, **not** Moon — a `repo:*` task would have to join `ci.yml`'s
  `T=(…)` array (a `--release` build on every affected PR) or become a `T_EXEMPT` entry. The
  workflow is **not a required check**, so a broken image build reds `main`, not the PR. Its
  `pull_request` trigger already covers `rs/Dockerfile`, `rs/Cargo.{lock,toml}`,
  `rs/rust-toolchain.toml`, `rs/.dockerignore`, `ci/images/**` and the workflow itself, so a PR
  touching any of those runs it automatically — no manual step needed there.
  `workflow_dispatch` it instead for a PR touching `rs/**` but **none** of those filtered
  inputs (a plain service code change, say) — that's the one case the narrower `pull_request`
  filter misses, and it can still break an image build. (`gh workflow run images.yml --ref
  <branch>` 404s until `images.yml` itself is on `main`.)
- The runtime base is a `chisel cut` of Ubuntu 24.04 into `FROM scratch`. Four traps, all
  measured: `libgcc-s1_libs` is REQUIRED (Rust panic unwinding links `libgcc_s.so.1`) and its
  absence fails at container START, not build; `ca-certificates_data` is the right variant
  (`-with-certs` adds ~120 PEMs nothing reads); there is **no `/etc/passwd`**, so `USER` must be
  numeric; and `chisel cut --root DIR` does not create `DIR`. `/etc/nsswitch.conf` is also absent
  and that is FINE — glibc falls back to a compiled-in `files dns` default and the NSS modules
  ship in `libc6_libs`. The smoke suite pins this by reaching Postgres at a CONTAINER
  HOSTNAME rather than an IP literal; public-name resolution was verified once by hand during
  design and is NOT covered continuously.
- `FROM rust:X.Y.Z` does **not** pin the compiler: `rust-toolchain.toml` is inside the build
  context and rustup honours it over the image, so a channel bump silently changes the compiler
  behind a pinned-looking `FROM`. `rs/Dockerfile` sets `RUSTUP_TOOLCHAIN` and
  `ci/images/run.sh` asserts the two agree. The related invariant — builder glibc ≤ runtime
  glibc (bookworm 2.36 ≤ noble 2.39) — is also asserted there; inverting it fails at container
  start with `GLIBC_2.4x not found`.
- Exec-form `ENTRYPOINT`/`HEALTHCHECK` do **not** expand `ARG`/`ENV`, which is why one
  parameterized `rs/Dockerfile` installs both binaries to the fixed path
  `/usr/local/bin/paigasus-service`. Service identity comes from `paigasus_logging::init`, not
  `argv[0]`.
- This repo now has **four** CA-bundle config knobs and they do NOT share semantics. `authn.extra_ca_bundle_path`
  and `upstream.openai.extra_ca_bundle_path` (SMA-558) **ADD** to the trust store — reqwest builds one
  `RootCertStore` by unioning `add_root_certificate` calls with the webpki roots and the platform store, so
  the workspace pins BOTH `rustls-tls` and `rustls-tls-native-roots` (dropping the former is not a
  simplification: reqwest accepts an EMPTY platform store silently, and webpki is the floor that stops a bad
  mount becoming a per-request failure). `outbox.publisher.root_ca_bundle` and the gateway's
  `iam.tls.ca_cert_path` **REPLACE** it. The `extra_` prefix is the marker — a fifth knob must pick a side
  and say which in its doc. Anything in an added bundle becomes an **unconstrained** anchor (no `cA` check),
  so it must contain roots only; a self-signed LEAF works too (put its own cert in the bundle) since rustls
  applies no `cA` check to a trust anchor.
- `repo:actionlint` and `repo:affected-smoke` now **guard each other**, and neither can guard
  itself (SMA-542). `ci/actionlint/run.sh`'s check 8 asserts `:affected-smoke` is still in
  `ci.yml`'s `T=(…)` array, that no `moon` line discards its exit status (a `||`/`&&`/`;`/`|`
  tail), and that no step's `continue-on-error:` value suppresses it (anything but the literal
  `false`) — escape-hatched per line via `COE_SKIP`, keyed by BOTH the line number and the line's
  own text, so a shifted entry stops matching instead of silently absorbing a different occurrence
  that lands on the vacated line. In return, `ci_targets.py`'s `ACTIONLINT_SH_CALL_SITES` pins
  `run_self_tests` and `selftest_mutation_battery` as **whole lines** in `run.sh` (a substring
  match would survive deleting the call, since the name is a prefix of its own definition). That
  pin only works because `repo:affected-smoke` lists `ci/actionlint/**/*` in its `inputs` — remove
  that and the pin stays green on exactly the PR that breaks it. Adding a tenth-and-later
  `*_self_test` table means bumping `SELF_TEST_COUNT` (currently 9): the gate asserts invocations
  AND definitions. The cycle's second half is now closed too (SMA-542 residual closure): check 8c
  in `ci/actionlint/run.sh` pins `ci/affected-graph/run.sh`'s own two call sites into
  `ci_targets.py`, mirroring `ci_targets.py`'s `RUN_SH_CALL_SITES` from the other, independently
  scheduled file — see `ci/actionlint/README.md`'s Limitations section (L6) for what residual
  still remains (a single combined edit deleting both gates' own call sites at once, the same
  bounded shape as the `T`-array cycle above).
- All three `repo:release-parity*` tasks run `ci/release-parity/run.sh --negative-control`
  before their real run, under an explicit `set -euo pipefail` (SMA-530). Each carries its
  own control because their *ecosystem-specific* `inputs` are distinct — a PR touching only
  a `.releaserc.json` selects `-ts` alone. They are not disjoint overall: all three also
  list `ci/release-parity/**/*` and `.prototools`, so an edit there schedules all three. Two pins guard it, both living in `ci/affected-graph/ci_targets.py`
  and both running inside `repo:affected-smoke`: `SELF_SCHEDULED_GATES` pins the nine
  `moon.yml` lines (whole lines, compared after stripping — reordering a flag or adding a
  trailing comment still reds it), and `RELEASE_PARITY_SH_CALL_SITES` pins five discrete
  lines inside `run.sh` itself — the flag parse, the `NEGATIVE` guard, the assertion body,
  and both report arms — because pinning the span as one block left two MEASURED bypasses
  with different failure shapes: neutering the flag parse (dropping `NEGATIVE=1`) leaves
  `NEGATIVE` at its initialized 0, so the control branch is never entered and the invocation
  falls through to the real suite, which then just runs twice and proves nothing; gutting
  the assertion body (replacing the `check_case` call with a bare `ec=1`) never calls the
  harness at all yet still prints "reported red as expected" — a control that actively lies
  rather than one that merely no-ops. Those are the two bypasses closed by pinning five lines
  instead of one span, not an exhaustive list — see `ci/release-parity/README.md`'s
  Limitations section L5 for a residual (an inserted `NEGATIVE=0` before the guard, or all
  five lines parked in a never-executed heredoc) that survives all five pins, and why closing
  it generally is out of scope. That second pin is reachable only because
  `repo:affected-smoke` lists `ci/release-parity/**/*` in its `inputs` — do not remove it. A
  script-pinned gate needs either a `SELF_TASK_EXPECTED_GLOBS` entry or a reasoned
  `SELF_TASK_GLOBS_EXEMPT` one. Note a `moon.yml`-only edit does NOT select the
  `release-parity*` tasks (their own `script:` is not among their inputs), so a PR changing
  those blocks should also touch `ci/release-parity/**` if it wants CI to execute them.

## Workflow

Specs/plans live in `docs/superpowers/specs/` and `docs/superpowers/plans/` (date-prefixed,
per Linear issue). Work flows brainstorm → spec → plan → implement. Linear keys are `SMA-NNN`;
PRs auto-link to Linear by branch name (don't attach links manually).
