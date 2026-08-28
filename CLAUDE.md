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
  currently `buffer-only-failure`). Moon 2.5.3 still has no per-invocation CLI flag
  for it (re-checked on the 2.5.3 bump, SMA-595); to stream a specific task locally, set
  `options.outputStyle: 'stream'` on the task definition. Note moon 2.4.3 changed the
  setting to apply only to TRANSITIVE targets, not the primary one you name.
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

- Moon is 2.5.3 and proto is 0.61.1 (SMA-595): `vcs.client` (not `manager`), `codeowners.sync`
  (not `syncOnRun`), Python/uv toolchains keyed `unstable_python` + a separate `unstable_uv`.
  The two pins are **coupled**: moon 2.5.3's Python toolchain plugin requires proto >= 0.60.0,
  and moon reads the proto CLI at the fixed path `~/.proto/bin/proto` — neither `PATH` order nor
  the `.prototools` pin overrides that, so a moon bump can hard-fail with
  `proto::tool::minimum_version_requirement` until the local proto BINARY moves. `proto upgrade`
  reports the target version and then no-ops inside an agent session; upgrade it from a normal
  shell, or set `PROTO_HOME` to an isolated root and `proto install` into it.
- `proto` prints **NDJSON on stdout** when it detects an agent environment (`AI_AGENT`,
  `CLAUDECODE`, `CLAUDE_CODE_ENTRYPOINT`), including a `Detected an AI agent environment…`
  preamble line. That breaks every `$(proto bin <tool>)` capture: the variable becomes a JSON
  blob, not a path. `ci/release-parity/ecosystems/release-plz.sh` does exactly this, so all three
  `repo:release-parity*` gates abort `INCONCLUSIVE (rc=2)` — **not red** — in any agent-driven
  local run. CI has no agent detection, so it never shows there. This is NOT new in proto 0.61.1;
  0.58.1 behaves identically (measured both, SMA-595). To verify those gates locally, `unset
  AI_AGENT CLAUDECODE CLAUDE_CODE_ENTRYPOINT` first — with that, all three pass.
- `repo:affected-smoke` has aborted **twice**, both times under a concurrent `moon ci` on 2.5.3,
  at ~2.4s against its usual 6–8s: once on SMA-595, which captured no output, and once on
  SMA-592, which captured its output. The two are matched on SYMPTOM SHAPE alone — a sub-3s abort
  under a concurrent `moon ci` — so nothing proves they are one and the same failure. Neither
  session reproduced it: four attempts on SMA-595 (warm, cold `.moon/cache`, cold `MOON_HOME`, and
  cold `rs/target` with cargo compiling alongside), and three more on SMA-592. An inherited
  `MOON_BASE` was tested and ruled out (the gate passes with it set). The gate is otherwise green
  everywhere. If you see a sub-3s `affected-smoke` failure, capture the full task output before
  re-running, because a re-run passes and destroys the evidence.
  **The mechanism below is measured on the ONE session that captured output (SMA-592), not on
  both.** In that occurrence the failure is an infrastructure ABORT, not a red verdict: the gate's
  own nested `moon query projects` dies with `Error: proto-shim:
  Failed to execute proto for the shimmed command: Permission denied (os error 13)`, writes
  nothing to stdout, and the reader then raises `JSONDecodeError: Expecting value: line 1 column
  1`, so `run.sh` prints `FATAL [contracts->proto]: moon query failed` and
  `== affected-graph guard ABORTED: infrastructure error (rc=2) ==`. So the proximate cause THERE
  is the **proto shim failing to exec `proto` with EACCES** while a `moon ci` runs concurrently —
  why the shim is briefly non-executable is still unknown, and SMA-595's four hypotheses above
  stay ruled out.
  Two consequences. The gate FAILS SAFE — rc=2 is distinct from rc=1, and it never reports a false
  green. And the `proto-shim` line is the tell, so grep the captured output for it: if that line
  is there, the failure is not about the affected graph at all, and re-running the task alone
  (`moon run repo:affected-smoke --force`) passes in the usual 6s. If it is absent, this entry
  does not explain your failure — diagnose it on its own terms.
  The NDJSON entry above is the same root tool, a different symptom; both mean a `moon`/`proto`
  call inside a gate is the fragile part of an agent-driven local run, never in CI.
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
  :http-extractor-envelope :input-liveness :promtool :observability-drift
  :nats-permissions :release-parity :release-parity-py :release-parity-ts
  :publish-metadata :version-lockstep :workflow-credentials --base origin/main
  --include-relations`
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
- Moon 2.5.3's Rust toolchain resolves `path = "…"` Cargo deps into the project graph **automatically**
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
  affectedness in Moon 2.5.3. `dependsOn` and `^:build` schedule an upstream's build but never
  **select** a downstream — a dependent runs only if independently affected. `--include-relations`
  is very nearly, but no longer entirely, inert: re-measured at the full 27-target shape on 2.5.3
  (SMA-595) it selects exactly ONE task the same command without it does not,
  `paigasus-kernel-py:build` — 44 RunTasks against 43, stable across repeated runs. On 2.3.2 the
  two sets were byte-identical (SMA-528). One added `build` is NOT a dependent closure, so the rule
  above still holds; do not read the flag as a working cascade. **Re-run that A/B on the next moon
  bump** — the delta moved once and can move again. Every Rust crate therefore
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
  Docker-less run yields exactly one red instead of 64 silent passes (SMA-538). The policy itself
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
  that and the pin stays green on exactly the PR that breaks it. Adding an eleventh-and-later
  `*_self_test` table means bumping `SELF_TEST_COUNT` (currently 10): the gate asserts invocations
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
  and both running inside `repo:affected-smoke`: `SELF_SCHEDULED_GATES` pins every
  self-scheduled gate's `moon.yml` invocation lines — `set -euo pipefail` included — for
  `input-liveness`, the three `release-parity*` tasks, `version-lockstep`,
  `publish-metadata`, `error-code-single-site`, `affected-smoke`, `actionlint` and (SMA-587)
  `http-extractor-envelope` (whole
  lines, compared after stripping — reordering a flag or adding a trailing comment still
  reds it; a bare number here would only rot again as the registry grows, which is why this
  names its current membership instead), and `RELEASE_PARITY_SH_CALL_SITES` pins five discrete
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
  `repo:affected-smoke`'s OWN `moon.yml` block is the one member of that registry pinned
  twice over (SMA-572/SMA-573): its invocation lines are pinned here as above, but its
  `inputs` are deliberately NOT — those, and its invocation lines' ORDER, are pinned instead
  by check 8e in `ci/actionlint/run.sh`, a gate scheduled independently of
  `repo:affected-smoke` itself, since a pin living inside `ci_targets.py` would make that gate
  the sole judge of its own reachability; `repo:actionlint`'s own `inputs: ['**/*']` — the
  premise check 8e (and 8/8b/8c/8d) runs on every PR at all — is pinned the ordinary way, from
  `SELF_TASK_EXPECTED_GLOBS["actionlint"]` in `ci_targets.py`.
- The kernel family (`paigasus-kernel` + the three binding crates + their `pyproject.toml` /
  `package.json` faces) carries **one version** across eighteen sites, asserted by
  `repo:version-lockstep` (`ci/version-lockstep/run.sh`). release-plz owns every Cargo
  `[package] version` — via per-package `version_group` — **and** the `[workspace.dependencies]`
  version requirements; both were measured against the pinned 0.3.158, as was the fact that
  `version_group` applies to crates whose Cargo manifest says `publish = false`. The script owns
  the six sites Cargo cannot reach (`--write`) and checks all eighteen, because a `version_group`
  that silently stopped applying would otherwise go unnoticed. Two of the sites drift SILENTLY
  without it: `py/uv.lock` (its `moon.yml` runs bare `uv sync`, not `--locked`) and the 26
  `bindingPackageVersion` guards in the committed napi glue (the codegen-drift gate covers only
  the three `**/generated` proto dirs). `repo:version-lockstep` is script-pinned the same way the
  `release-parity*` tasks are — `SELF_SCHEDULED_GATES` pins its **four** `moon.yml` lines
  (`--self-test`, `--negative-control`, the real run, and `set -euo pipefail`; one more than the
  `release-parity*` tasks, which have no self-test invocation) — and takes the
  `SELF_TASK_EXPECTED_GLOBS` route through the
  pairing rule above, listing all sixteen of its literal `inputs`, so it needs no
  `SELF_TASK_GLOBS_EXEMPT` entry (holding both would itself be reported).
- `rs/release-plz.toml` declares releasability **per package**, never workspace-wide. A
  `[workspace] release = false` makes release-plz hard-error (`no public packages found`), and
  simply deleting it is worse: `dependencies_update = true` cascades a patch bump into every
  transitive dependent — a crate neither in the version group nor touched by the commit still
  gets bumped ("dependencies changed") — and Cargo's `publish = false` suppresses publishing but
  **not tagging**, so the first release would permanently tag most of the workspace. Per-package
  `release = false` removes a package from the proposal entirely; every non-family crate needs
  one explicitly. `paigasus-gateway` / `paigasus-iam` stay at `0.0.0` deliberately: their
  `env!("CARGO_PKG_VERSION")` feeds `ServiceInfo`, and ADR-0020 skew reporting is parked on that
  value (SMA-505 R7).
- release-plz's `release_pr()` does all its work in a **tempdir copy** (`copy_to_temp_dir`,
  measured against the pinned 0.3.158) — it never touches the local working tree or `HEAD`. This
  nearly shipped a direct push to `main`: deriving the push target with `git rev-parse
  --abbrev-ref HEAD` after `release-plz release-pr` still reads `main`, so `git push origin
  "HEAD:$BRANCH"` becomes an unreviewed push to protected `main` (the `Protect main` ruleset has
  no `pull_request` rule and a `bypass_actors` entry for admin). Always derive the branch from
  `release-plz release-pr --output json`'s `.prs[0].head_branch`; the `prs` array is empty
  whenever no release is needed — see the next entry.
- release-plz's version baseline is the **crates.io registry**, not git tags (no `git_only` is
  set in `rs/release-plz.toml`). Measured on this repo at the `0.1.0` floor: it logs `WARN
  Package 'paigasus-kernel@*.*.*' not found`, then proposes `next version is 0.1.0` — the
  manifest version, no bump. Measured live on 2026-08-28: it still OPENS a PR (`chore: release
  v0.1.0`) listing all three packages — "empty" means it proposes no version CHANGE, not that no
  PR appears — so "the release PR is the acceptance evidence" does not hold for the first run. The real hazard here is name
  squatting — release-plz performs a crates.io lookup for every workspace member name, so a
  squatted name silently becomes the comparison baseline — not a runaway version proposal.
- `release.yml` authenticates with a **GitHub App installation token minted per run**
  (`actions/create-github-app-token`), never a stored secret: an installation token lives one
  hour, so it CANNOT be a repository secret, and the original `RELEASE_PLZ_TOKEN` shape could
  only ever have held a long-lived PAT (SMA-589). Three traps. The secret `PAIGASUS_BOT_APP_ID`
  holds the App's **Client ID**, not the numeric App ID — the NAME is the only stale thing about
  it, so do not "correct" it by storing the numeric id. The token must request
  `permission-contents: write` + `permission-pull-requests: write` **explicitly**: without the
  `permission-*` inputs it inherits every permission the installation holds, so granting the App
  an unrelated scope later silently widens it (zizmor `github-app`) — and because the requested
  set must actually be granted, an under-granted App reds at mint time rather than half-working
  later. And the preflight makes the whole job skip **green** when the App id is absent, so a
  broken token path is invisible in CI: the only proof is a real run on `main`.
- Any crate flipping `publish = true` must carry **its own `[lints.*]` table** and **its own
  `include` allowlist** — enforced by `repo:publish-metadata` Checks 1c/1d (SMA-577). Cargo
  inlines the resolved lint table into the published manifest and docs.rs builds published
  crates as the root package on nightly, where `--cap-lints allow` does NOT apply, so an
  inherited `warnings = "deny"` silently kills docs.rs builds on the first new rustc lint —
  months after the PR. 1d's membership is LITERAL: `include = ["**/*"]` is rejected, since it
  would "cover" README.md/LICENSE while reinstating the `moon.yml` leak Check 2b catches.
  Check 2 runs one `cargo publish --dry-run` per **publish group** (a connected component of
  the in-set dependency graph), NOT per package: a per-package dry-run of `paigasus-proto`
  exits 101 (`no matching package named 'paigasus-proto-derive'`) until the derive crate is on
  crates.io, while `-p paigasus-proto-derive -p paigasus-proto` exits 0. That combined form is
  registry-faithful, not a workspace shortcut — measured by breaking the derive crate's
  `include` and watching the run fail. Grouping keeps `paigasus-kernel` in a group of one so
  it retains its standalone assertion.
- `paigasus-py-bindings` ships to PyPI as **`cp312-abi3` wheels (six matrix legs, seven wheels)
  plus a source-verified sdist**, built by `.github/workflows/wheels.yml` (SMA-578) — a
  *reusable* workflow (`on: workflow_call`) that SMA-579's gated `release` job will consume. It
  must **never** declare `secrets:` or `id-token: write`: it carries a `pull_request` trigger, so
  a same-repo PR would receive the credential — `repo:workflow-credentials` asserts this, and it
  applies the same ban to EVERY `pull_request`/`pull_request_target`-triggered workflow, not to
  `wheels.yml` alone (SMA-593; it was `repo:publish-metadata`'s P-D6 until then). Four facts
  that cost a measurement each: (1) maturin injects the apple-darwin `-undefined dynamic_lookup`
  args **itself**, so an sdist builds on macOS without `rs/.cargo/config.toml` — that file exists
  for plain `cargo build`, as its own comment says, and the old "no sdist" rule rested on a false
  premise (measured on ONE host / maturin 1.9.6 / one target, natively — which is why the sdist
  is verified on three platforms rather than trusted); (2) maturin builds the sdist from `cargo
  package --list`, so the crate's **Cargo** `include` allowlist is what keeps `moon.yml` out —
  `[tool.maturin] include` is not needed, and Checks 1c/1d/2b/2c never reach this crate because it
  is `publish = false`, so the only assertion holding that allowlist honest lives in `wheels.yml`;
  (3) the sdist ships the **workspace** `Cargo.toml` verbatim, `[workspace.lints.rust] warnings =
  "deny"` included, and a consumer builds as the ROOT package where `--cap-lints allow` does NOT
  apply — so every sdist-shipped crate needs its own non-denying `[lints.rust]` table, the
  Check-1c rule extended past `publish = true`; (4) `pyo3`'s `abi3-py312` means one wheel per
  (OS, arch) covers CPython 3.12+, so the matrix never multiplies by Python version. maturin also
  relocates `pyproject.toml` to the sdist **root**, not the crate dir, so the sdist content
  assertions match on basename.
- All four **Linux** wheel legs cross-compile with `--zig` — not only the musl ones, unlike
  `prebuild.yml`. `ubuntu-latest` ships glibc 2.39, so a *native* build tags `manylinux_2_39`,
  which almost nothing can install. The floor comes from **`--zig` together with
  `--compatibility`, both passed as FLAGS** (maturin's own `--help`: "`--zig` … Default to
  manylinux2014/manylinux_2_17 if you do not specify a `--compatibility`"). It does **not** come
  from cargo-zigbuild's decorated triple: maturin hands `--target` straight to `cargo metadata`,
  so `x86_64-unknown-linux-gnu.2.17` dies with `could not find specification for target` —
  measured, it failed both manylinux legs on this workflow's first CI run while the other ten
  jobs passed.
  Pass `--compatibility` explicitly so maturin's auditwheel **errors** instead of silently
  emitting a PyPI-rejected `linux_*` tag, and set `-C target-feature=-crt-static` on musl (the
  target defaults to a static CRT a cdylib cannot use). A wheel's **tag is not its binary**:
  assert the compressed tag *set* (split on `.` — `manylinux_2_17_x86_64.manylinux2014_x86_64` is
  ONE platform FIELD carrying TWO tags, so a cardinality check that counts fields as tags is
  wrong), and separately assert the binary via `otool -l`'s minimum-macOS on darwin and a
  max-`GLIBC_` symbol check on manylinux. An ELF-class check proves only the machine type and
  passes for a wheel that fails at import. **Only the `aarch64-apple-darwin` wheel and the macOS
  sdist path have been built locally.** The macOS / Windows / manylinux / musllinux tag sets, the
  `macosx_10_12` minimum-macOS value have all now been **MEASURED green on CI**. The GLIBC floor
  is per-arch and the two values legitimately differ: x86_64 tops out at **`GLIBC_2.14`** (its
  base is `GLIBC_2.2.5`; 2.14 is `memcpy`'s versioned symbol) while aarch64 reaches
  **`GLIBC_2.17`** — do not harmonise them. x86_64 was pinned at 2.17 on the first run and the
  assertion red with *"needs only [GLIBC_2.14] … safe, but re-pin"*, which is the intended
  behaviour: a wheel needing LESS than its `manylinux_2_17` tag promises is correct, since the
  tag declares a minimum platform. When one of these reds, read what the tool produced, confirm
  it is correct, and re-pin the constant — never loosen the comparison to an inequality.
- `moon query projects --json` **errors** on Moon 2.5.3 too (`unexpected argument '--json' found`,
  re-checked on the 2.5.3 bump, SMA-595) —
  bare `moon query projects` already emits JSON. **Measure its exit status UNPIPED (2):** `jq`
  returns 0 on empty input, so `moon query projects --json | jq …` reports 0 unless `pipefail`
  is set, and the failure reads as "the reader found nothing" rather than "the flag is invalid".
  That is not hypothetical — it cost a cycle on this very branch, where the first measurement
  read `head`'s status through a pipe and recorded exit 0.
- The **codegen-drift gate is an inline `ci.yml` step** (`.github/workflows/ci.yml:249-262`), NOT
  a `repo:*` Moon task — searching `moon.yml` for it finds nothing. That placement is deliberate
  and load-bearing: the step carries no `if:`, so it runs on EVERY CI run and cannot be
  deselected, where a `T`-array task would run only when affected and a wrong `inputs` list would
  switch it off silently. It delegates its freshness to `moon run contracts:generate`, so that
  task's `inputs` are what make the diff real: they now include `/.prototools` (which pins `buf`
  itself) and `/py/uv.lock` (which pins the `local:` betterproto2 plugin, run via `uv run
  --project ../py`), alongside `buf.gen.yaml` which pins the three REMOTE plugins. Before SMA-592
  the first two were absent, so a generator bump left the hash unchanged, Moon served a cached
  pass, `buf generate` never ran, and the diff compared the committed output against itself —
  vacuously green. `.moon/cache` is restored across CI runs (`ci.yml:113-119`), so that was a real
  CI hole, not a local-only one. `contracts:generate` still declares no `outputs:`; this makes its
  cache KEY honest, not its output restorable, which is the second reason the drift step stays
  unconditional. The inputs are pinned to exact equality by `CONTRACTS_GENERATE_INPUTS` in
  `ci/affected-graph/ci_targets.py` — reachable because `repo:affected-smoke` lists `*/moon.yml`.
  Cost of the two added inputs, measured on 2.5.3: one `buf generate` is ~0.7s warm, and over
  `main`'s 163 commits they select it on 32 commits (19%) that no old input would have selected.
- Two limits on that fix, both measured, neither closed. First, **`repo:input-liveness` cannot see
  `contracts:generate`.** `ci/affected-graph/task_inputs.py`'s `_repo_tasks` is keyed to
  `projects.get("repo")` by exact project id, so it liveness-checks `repo:*` tasks and nothing
  else. If `py/uv.lock` moved, `contracts:generate` would silently stop keying on the betterproto2
  pin while `CONTRACTS_GENERATE_INPUTS` stayed green — the SMA-553 failure class, on a task the
  liveness gate cannot reach. Second, the fix is a **CACHE-KEY fix, not an execution fix**:
  `uv run` executes the installed `py/.venv`, not `py/uv.lock`, and `contracts:generate` declares
  no `deps:` that syncs `py`. A stale venv can still run a different betterproto2 than the key
  implies.
- `rs/.cargo/config.toml` is now an input of every task that runs cargo from `rs/`: all thirteen
  crates' `build`/`build-release`/`test`/`lint`, the three FFI wrapper tasks, and three `repo:*`
  gates that shell out to cargo (`repo:parity-corpus-drift`, `repo:observability-drift`,
  `repo:nats-permissions`). Editing it selects 61 tasks against 3 before — the 52 crate tasks, the
  3 FFI tasks, and 6 `repo:*` gates (those three, plus `repo:actionlint`, `repo:input-liveness`
  and `repo:publish-metadata`, which select on everything). **Only 16 of those 61 declarations are
  asserted**: A4 (via `WORKSPACE_LINT_INPUTS`) covers the thirteen `lint` declarations and A5 (via
  the `FFI_TASK_INPUTS` splat) the three FFI tasks, because `check_task_inputs` is called for
  `lint` and `fmt` only. The 39 `build`/`build-release`/`test` declarations and the three gates
  are declared by hand and asserted by nothing — delete one and CI stays green. It is deliberately
  NOT on `fmt`: `cargo fmt --check` neither compiles nor links, so rustflags cannot change its result.
  `repo:wasm-getrandom-free` is excluded for the same kind of reason — it runs `cargo tree`, which
  resolves the dependency graph and never applies rustflags. This REVERSES SMA-546's deliberate
  exclusion, which reasoned that CI is Linux and the darwin flags are inert there. Both are true;
  the criterion changed to "does this file influence the output" rather than "is it strictly
  required", because `rustflags` affect every darwin build from `rs/`. Note maturin injects the
  `-undefined dynamic_lookup` args ITSELF (SMA-578), so the py wheel does not NEED the file — it
  is keyed on it anyway, under the same one rule, which is why `REQUIRED_FFI_TASKS` needs no
  carve-out.
- **Nothing enforces that one rule.** A4 covers each crate's `lint`/`fmt`, A5 the three derived FFI
  tasks, and `repo:input-liveness` proves DECLARED inputs are live — never that NEEDED ones are
  declared. A future `repo:*` task that runs cargo from `rs/` can omit `rs/.cargo/config.toml` and
  nothing reds. That is exactly how the three gates named above were missed until SMA-594; assume
  the next one will be missed the same way, and check by hand when adding a cargo-invoking gate.
- A hand-written `.pyi` next to a PyO3 crate is an interface contract that basedpyright reads
  INSTEAD of the Rust, and it lives at the crate ROOT where `src/**/*` does not match it. A7 now
  demands every `{upstream}/*.pyi` found on disk, disk-conditional exactly like its `build.rs`
  clause. Do NOT read this as closing SMA-535: it makes a stub edit re-run the FFI smoke test, it
  does NOT make a stub that disagrees with the Rust fail. That needs a three-set drift gate
  (`#[pyfunction]` idents × `wrap_pyfunction!` registrations × stub `def` names), which is SMA-535
  proper and pairs with SMA-536.
- `moon query tasks --affected` emits each selected task's `deps[]`, and every dep entry carries a
  `"target"` key of its own. So `grep -o '"target": "[^"]*"'` over the raw JSON counts SCHEDULED
  upstreams as if they were SELECTIONS — it reported 15 tasks for a `.prototools` edit where the
  real answer is 12. Parse the JSON and take one target per `tasks[project][task]`. This matters
  because scheduled-vs-selected is the exact distinction every affectedness measurement in this
  repo turns on; an extraction that conflates the two cannot measure it. It inflated this branch's
  own spec table before the numbers were re-derived.

## Workflow

Specs/plans live in `docs/superpowers/specs/` and `docs/superpowers/plans/` (date-prefixed,
per Linear issue). Work flows brainstorm → spec → plan → implement. Linear keys are `SMA-NNN`;
PRs auto-link to Linear by branch name (don't attach links manually).
