# SMA-693: scheduled lockstep updater for the wasm-bindgen family and the committed wasm glue

- **Linear:** [SMA-693](https://linear.app/smaschek/issue/SMA-693)
- **Branch:** `feature/sma-693-wasm-bindgen-lockstep-updater`
- **Related:** SMA-683 (the freeze and the runbook, spec section 7 is the origin of this issue),
  SMA-680 (`dependencies_update = false`, the token-scope rule), SMA-634 (the committed wasm glue,
  F12 host-dependent binary), SMA-593 (the YAML-parsed workflow gate, fourteen text-scan bypasses).
- **Status:** draft, written by an unattended Stage 1 run on 2026-09-27, then revised after one
  spec-challenger pass (verdict NEEDS REWORK, see the changelog at the end). On 2026-10-02 the
  file was recovered from the session transcript, because the scratchpad copy was lost. On
  2026-10-02 Sven decided Q1, Q2, Q7 and Q10 (section 12). The other questions use the defaults
  that section 12 states.

## 1. Problem

`js-sys`, `web-sys` and `wasm-bindgen-futures` pin `wasm-bindgen` with `=`. Dependabot updates one
package at a time, so `cargo update -p wasm-bindgen` locks 0 packages (SMA-683 M0, MEASURED).
Since SMA-680 the release PR does not run a full `cargo update` either. So `wasm-bindgen` stays
frozen until a person runs the runbook in `rs/CLAUDE.md` ("The wasm-bindgen family does not move
through dependabot", lines 171-224).

The runbook has two steps that a machine can do: the four-package `cargo update -p` and
`moon run paigasus-kernel-ts:generate-wasm`. It has one step that a person must do: read the
`Cargo.lock` diff before and after the build. This issue automates the first two and keeps the
third as a PR review.

## 2. Goal and acceptance criteria

Goal (from the issue): a scheduled workflow proposes the lockstep bump as a PR, with the
regenerated wasm glue, and no person runs the runbook.

- **AC1.** A weekly scheduled run, and a manual `workflow_dispatch` run on `main`, run the
  four-package `cargo update -p` on the current `main`. When the lock does not change, the run
  ends green with a notice and opens no PR. When a bot PR is still open at that time, the run
  closes it with a comment (5.2).
- **AC2.** When the lock changes, the run regenerates the five wasm artifacts on Linux and opens
  (or updates) ONE pull request. The PR changes `rs/Cargo.lock` and at most the five artifacts,
  and no other path. (A glue file that is byte-identical after the build is not in the diff.)
- **AC3.** No process that runs third-party build code (cargo, build scripts, proc-macros,
  wasm-pack, `wasm-bindgen-cli`, pnpm packages) can read a write-capable token, a
  repository/environment secret, or the runner's `ACTIONS_RUNTIME_TOKEN`. So that code cannot
  write a cache entry, and cannot write an artifact other than the one named artifact (safety
  constraint 1; section 5.1 and 9).
- **AC4.** The privileged part pushes only when all of these hold (issue outline):
  1. the artifact holds `rs/Cargo.lock` and a subset of the five artifact paths, all regular
     files, and nothing else;
  2. the `Cargo.lock` diff touches only family packages (the seven of F1, or a subset of them),
     each at most once, all with the crates.io source and a checksum, and each version string
     matches a strict semver pattern;
  3. `main` is still the SHA that the build ran on, checked immediately before the push. When it
     moved, the run does not push and ends GREEN with a notice (see Q8 about this condition).
- **AC5.** A refusal from AC4.1 or AC4.2 fails the run red, with a message that names the failed
  condition and points to the runbook. It never pushes a partial result.
- **AC6.** The PR's own `CI` run (`moon ci`, including `committed-wasm.test.ts`) passes, and a human
  reviews the lock diff before the merge (safety constraint 2). The PR body says so. Section 9
  states that nothing enforces the review.
- **AC7.** Safety constraint 4 is answered: this design adds no `workflow_run` workflow, and
  section 7 states why `repo:workflow-credentials` then needs no new trigger (Q1 changes this).
- **AC8.** The AC4 checker and the workflow pin check have a `--self-test` and a
  `--negative-control`. A new `repo:wasm-lockstep` gate runs them, registered with all seven
  obligations of `ci/CLAUDE.md` (5.4).
- **AC9.** `rs/CLAUDE.md`, `ts/CLAUDE.md`, `ts/packages/paigasus-kernel/moon.yml`,
  `rs/Cargo.toml:143-151`, `.github/dependabot.yml` (the SMA-683 comment) and the `REGENERATE`
  text in `committed-wasm.test.ts` stop saying that "nothing schedules" the bump. The "ONE host
  regenerates" and "CI never regenerates" texts change to the new rule (5.5).

## 3. Facts

| # | Fact | Source |
|---|------|--------|
| F1 | The four-package update moves seven packages at M0: `wasm-bindgen`, `-macro`, `-macro-support`, `-shared`, `js-sys`, `web-sys`, `wasm-bindgen-futures`. | SMA-683 spec M0 step 2 (MEASURED there) |
| F2 | `generate-wasm` is `runInCI: false` and `cache: false`. `ci/CLAUDE.md` ("Registering a new `repo:*` gate") and `ts/moon.yml:42-43` state that Moon drops a `runInCI: false` task from `moon run` too when `CI=true`. GitHub Actions sets `CI=true` and `GITHUB_ACTIONS=true`. | `ts/packages/paigasus-kernel/moon.yml:189-248`; `ts/moon.yml:42` (READ) |
| F3 | `generate-wasm --post` rejects a binary that holds `/Users/`, `/home/`, the user name, or the last segment of `$HOME`. | `ts/packages/paigasus-kernel/scripts/generate-wasm.mjs:147-165` (READ) |
| F4 | `generate-wasm --pre` refuses to run when `RUSTFLAGS` or `CARGO_ENCODED_RUSTFLAGS` is set, and when `wasm-pack --version` differs from the `.prototools` pin. | `generate-wasm.mjs:102-124` (READ) |
| F5 | The App credentials `PAIGASUS_BOT_APP_ID` (the client ID) and `PAIGASUS_BOT_PRIVATE_KEY` are ENVIRONMENT secrets on `release-pr`. That environment has a custom deployment branch policy; `release.yml:188-199` records it as `main`-only. `release.yml:262-272` mints the token with `actions/create-github-app-token@bcd2ba49…` (v3.2.0), with the explicit inputs `permission-contents: write` and `permission-pull-requests: write`, and commits as `paigasusbot[bot]`. | `release.yml:188-272, 386-394`; `gh api …/environments` (READ 2026-09-27) |
| F6 | The `main` ruleset requires the `moon ci` check with strict up-to-date branches, and blocks deletion and non-fast-forward. It does not require signed commits. It has no `pull_request` rule, so nothing requires a review before a merge. | `gh api repos/SMK1085/paigasus-core/rules/branches/main` (READ 2026-09-27); `.github/CLAUDE.md:43` |
| F7 | `ci.yml` runs commitlint on every PR commit. The scope enum holds `deps`; the header maximum is 100. | `ci.yml:211-222`; `ts/packages/commitlint-config/index.cjs:42-45` (READ) |
| F8 | `repo:workflow-credentials` treats a workflow as a subject only when it has a `pull_request`, `pull_request_target` or `issue_comment` trigger. Its README lists `workflow_run` as a non-goal "until a workflow uses it". | `ci/workflow-credentials/README.md` "Non-goals"; `workflow_credentials.py:278` (READ) |
| F9 | A job reads an environment secret only when it declares that `environment:`. A job without it cannot read `PAIGASUS_BOT_*`. | GitHub Actions environments docs (READ, not measured here) |
| F10 | The Linux-built binary differs from the macOS-built one (SMA-634 F12). `committed-wasm.test.ts` compares only the glue and the interfaces. | SMA-634 F12; SMA-683 spec (READ) |
| F11 | Every step of a hosted-runner job can get `ACTIONS_RUNTIME_TOKEN` from the `Runner.Worker` process, because hosted runners give passwordless `sudo` (the technique of the Cacheract research and the tj-actions memory dump). That token can write cache entries in the scope of the run's ref. A `schedule` or `workflow_dispatch` run on `main` has the scope `refs/heads/main`. | Challenger finding (public research; not measured here) |
| F12 | `prebuild.yml:107-121` and `wheels.yml:159-172` restore `~/.cargo/registry`, `~/.cargo/git` and `rs/target` with a `restore-keys` prefix. `release.yml:491,501` calls both. A release commit changes `rs/Cargo.lock`, so the primary key misses and the newest prefix match restores. `ci.yml:114-120` restores `.moon/cache` by the prefix `moon-Linux-`. | READ 2026-09-27 |
| F13 | `actions/upload-artifact` with several paths uses their least common ancestor as the artifact root. `images-rehearsal.yml:81-83` uploads `out/…` files and `:166` reads them without the `out/` prefix. The repo pins `upload-artifact@043fb46…` (v7.0.1) and `download-artifact@3e5f45b…` (v8.0.1). | READ 2026-09-27 |
| F14 | `workflow_credentials.py` parses workflows with PyYAML from its own uv project, which a dependabot `uv` entry watches (`.github/dependabot.yml:81-100`). `ci/CLAUDE.md:264-270` lists five PyYAML coercions and the anchor/alias rule that every workflow parser in `ci/` must handle. `moon.yml:812-815` records fourteen measured bypasses of the text scan it replaced. | READ 2026-09-27 |
| F15 | An unlocked cargo call inside `moon ci` can rewrite the working-tree `rs/Cargo.lock` during the run. A gate that reads the working-tree lock races that repair. | `rs/CLAUDE.md:109-131` (READ) |
| F16 | `rs/Cargo.lock:6122-6123` holds `wasm-bindgen` 0.2.128. SMA-683 M0 measured that 0.2.129 is published. | READ 2026-09-27; SMA-683 spec |

## 4. Approaches

### A. One workflow, two jobs, split by environment (RECOMMENDED)

`.github/workflows/wasm-lockstep.yml`, triggers `schedule` and `workflow_dispatch`, top-level
`permissions: contents: read`.

- Job `build`: no `environment:`, no `secrets` context in any form. Runs the update and
  `generate-wasm` inside a container that gets no runner environment (5.1), and uploads the
  artifact.
- Job `propose`: `needs: build`, `environment: release-pr`. Runs no build and executes no artifact
  content. Checks the artifact, mints the App token, pushes and opens the PR.

Why it meets the constraints: the build job cannot read the App key (F9), and its `GITHUB_TOKEN`
is read-only. The container keeps the build code away from `ACTIONS_RUNTIME_TOKEN` (F11). A
dispatch on a non-`main` ref runs an edited copy of the file, but the environment's `main`-only
branch policy then refuses the `propose` job, the same boundary `release.yml` uses (F5). It adds
no `workflow_run` trigger, so it adds no event-origin checks and no `repo:workflow-credentials`
change.

### B. Two workflows, `schedule` then `workflow_run` (the issue outline)

Workflow 1 as in A's `build` job. Workflow 2 on `workflow_run`, downloads the artifact from the
other run, holds the App token.

Extra cost against A: `workflow_run` fires for EVERY run of a workflow with the watched `name:`,
including a run that a fork pull request starts with an edited copy that adds a `pull_request`
trigger. Workflow 2 must then check `workflow_run.event`, `head_repository.full_name`,
`head_branch` and `conclusion` before it trusts the artifact, and `repo:workflow-credentials` must
add `workflow_run` to its trigger set plus an allowlist entry for the App-key read (R4). The one
benefit is that workflow 2 always runs from the default-branch definition. The environment
policy in A gives the same result for the token. B has the same cache-scope problem as A (F11),
so it needs the same container.

### C. Dependabot or release-plz

Rejected by SMA-683 M0 and SMA-680. Neither can move the family.

**Recommendation: A.** It meets every safety constraint of the issue with less attack surface
and no gate extension. It deviates from the issue's outline, so Q1 asks Sven to confirm.

## 5. Design (approach A)

### 5.1 Job `build`

- `runs-on: ubuntu-latest`, `timeout-minutes: 60` (M0 records the real time), `permissions:
  contents: read`.
- `concurrency: { group: wasm-lockstep, cancel-in-progress: false }` on the workflow.
- The job has no `environment`, no job-level `uses:` or `secrets:`, and no `secrets` context.
- **The isolation rule.** Every step that runs third-party code runs inside
  `docker run --rm` (image `ubuntu:24.04` pinned by digest, or a pinned `rust` image; the plan
  decides). The container gets:
  - one bind mount: a COPY of the tree without `.git` at `$RUNNER_TEMP/work` (made with
    `git archive HEAD | tar -x`), and nothing else;
  - no `-e`/`--env-file` pass-through of the runner environment, so no `CI`, no
    `GITHUB_ACTIONS`, no `ACTIONS_RUNTIME_TOKEN`, no `GITHUB_TOKEN`, no `GITHUB_OUTPUT`;
  - no Docker socket, no `--privileged`, no added capabilities, no `--pid=host`/`--network=host`;
  - `--user` with the runner's uid/gid, and an explicit `HOME` inside the container (M0 and M2
    record the identity marker that `--post` then checks, F3).
  Network access stays on: cargo needs crates.io and proto needs its downloads.
- **What the host does after the compile container.** It runs no code from the work copy and no
  `git` command on it (a `git` call can run a hook or an `fsmonitor` command from `.git/config`;
  the copy has no `.git`, and this rule keeps it so). It only copies the six files with `cp` into a
  staging directory, after `test -f` and `! test -L` on each, and uploads with the pinned action.
- Steps:
  1. Reclaim disk (the same inline `rm` step as `ci.yml`).
  2. `actions/checkout` at the same pinned SHA as `ci.yml`, `persist-credentials: false`,
     `fetch-depth: 1`. Refuse (exit 1) unless `github.ref == 'refs/heads/main'`. This is a
     convenience check, not the boundary (5.2 is).
  3. Make the work copy (above). Keep a pristine copy of `rs/Cargo.lock` at
     `$RUNNER_TEMP/old.lock` from `git show HEAD:rs/Cargo.lock`.
  4. Container run 1: `( cd rs && cargo update -p wasm-bindgen -p js-sys -p web-sys -p
     wasm-bindgen-futures )`. This needs only the pinned Rust toolchain in the container.
     `cargo update` resolves the index and runs no build script.
  5. On the host, from the CHECKOUT (not the work copy): `python3
     ci/wasm-lockstep/lockstep_check.py lock --old "$RUNNER_TEMP/old.lock" --new
     "$RUNNER_TEMP/work/rs/Cargo.lock"`. Exit 4 means no change: write the job-summary notice
     ("the family is current at <version>", the version from the checker's validated output),
     set output `changed=false`, stop. Exit 3 means a refusal: the job fails red before any
     compile, the same order as the runbook. This is an early exit, not the control; 5.2 runs the
     check again on the downloaded bytes. The host sets `changed=true` here, before any
     third-party code runs, so the compile cannot set it.
  6. Container run 2: the setup that `ci.yml:122-142` does, then the compile. In this order:
     `proto install`, `moon setup`, the serial `rustup component add rustfmt clippy` and
     `rustup target add wasm32-unknown-unknown` (the `ci.yml` race note), `pnpm -C ts install
     --frozen-lockfile` if M0 shows that `generate-wasm`'s `^:build` closure needs it, then
     `moon run paigasus-kernel-ts:generate-wasm` with `PROTO_REPORTER=text` and no `RUSTFLAGS`
     (F4). The container has no `CI` variable, so F2 does not drop the task; M0 confirms this.
  7. Stage the six files under `$RUNNER_TEMP/stage/rs/...` (same relative paths as the repo),
     then `actions/upload-artifact@043fb46…` (v7.0.1) with the ONE path `$RUNNER_TEMP/stage/`,
     name `wasm-lockstep`, `if-no-files-found: error`, `retention-days: 7`. The downloaded tree
     then holds `rs/Cargo.lock` and `rs/crates/bindings/paigasus-wasm/...` (F13; M0 records the
     real downloaded tree).
- Outputs: only `changed`. The job outputs no version string.

### 5.2 Job `propose`

- `needs: build`, `if: needs.build.result == 'success'`.
- `environment: release-pr` (Q2), `permissions: contents: read`, `timeout-minutes: 15`.
- **Rule for `build` outputs.** `propose` reads `needs.build.outputs.changed` ONLY in `if:`
  expressions (job or step). No `run:`, `env:` or `with:` in `propose` contains
  `needs.build.outputs`. Every value that reaches a command (the versions, the PR title, the
  PR body) comes from the trusted checker's output on the downloaded bytes, passes through
  `env:` or a file, and is never placed in a `run:` block by `${{ }}`.
- **The real control is not the step order.** The runner holds the job's secrets for the whole
  job. So the control is that no step in `propose` executes artifact content: no toolchain setup,
  no `pnpm`, no `cargo`, no `uv`, no `moon`, and no script from the artifact. The token step still
  comes after every check, so a refusal never mints a token.
- Steps when `changed == 'true'`, in this order:
  1. `actions/checkout` at `github.sha`, `persist-credentials: false`.
  2. `actions/download-artifact@3e5f45b…` (v8.0.1) of `wasm-lockstep` into
     `$RUNNER_TEMP/lockstep` (outside the checkout).
  3. `python3 ci/wasm-lockstep/lockstep_check.py artifact --dir "$RUNNER_TEMP/lockstep" --old
     rs/Cargo.lock --body-file "$RUNNER_TEMP/pr-body.md" --title-file "$RUNNER_TEMP/pr-title.txt"`.
     This runs the trusted checker from the checked-out `github.sha`. AC4.1 and AC4.2. Exit 3 on a
     refusal (AC5). The checker writes the PR title and body only after every version string
     passed its semver check.
  4. Copy the files over the checkout. `git status --porcelain` must list `rs/Cargo.lock` and a
     subset of the five artifact paths, and nothing else.
  5. Mint the App token (`actions/create-github-app-token@bcd2ba49…`, with `client-id`,
     `private-key`, `permission-contents: write` and `permission-pull-requests: write`, copied
     from `release.yml:262-272`).
  6. Commit as `paigasusbot[bot]` (the identity of `release.yml:393-394`), with the message from
     the title file (F7: `build(deps): move wasm-bindgen to <version> and regenerate the wasm
     glue`). No body line of the form `token: value` or `#NNN` (commitlint footer trap). Q9 asks
     whether to commit through the API instead.
  7. Base check, immediately before the push: `gh api repos/${GITHUB_REPOSITORY}/git/ref/heads/main
     --jq .object.sha` with `GH_TOKEN` from `github.token`. Branch on the exit status of `gh api`,
     never on an `|| echo` fallback (the `gh api` 404 trap in `.github/CLAUDE.md`). A non-zero
     status is red. A different SHA is a GREEN notice ("main moved from <a> to <b>; the next run
     proposes on the new base"), and the job stops without a push (AC4.3, Q8).
  8. Branch-owner check: when `refs/heads/deps/wasm-bindgen-lockstep` exists, its head commit
     must have the author `paigasusbot[bot]`. Else refuse red with "a person pushed to the bot
     branch; merge or close that PR first". Then push with
     `--force-with-lease=refs/heads/deps/wasm-bindgen-lockstep:<the SHA read in this step>` to
     exactly `HEAD:refs/heads/deps/wasm-bindgen-lockstep`. The refspec is fixed text (5.4 pins
     it).
  9. `gh pr list --head deps/wasm-bindgen-lockstep --state open`: none -> `gh pr create --base
     main --title-file`-equivalent (`--title "$(cat …)"` through `env:`) `--body-file`; one ->
     `gh pr edit --body-file`. Both with the App token, so the PR's `CI` runs (a `GITHUB_TOKEN`
     push starts no workflow).
- Steps when `changed == 'false'`: mint the token, and when an open bot PR exists on
  `deps/wasm-bindgen-lockstep`, close it with the comment "the family is current on main; this
  proposal is obsolete". No checkout of artifact content happens on this path.

The PR body (written by the checker) lists: the old and new version of each family package, the
statement that the artifacts come from a build that ran third-party code, that the path and lock
checks do not make their content safe, and a reviewer checklist (read the lock diff; confirm
`CI` is green; F10 explains the binary diff; do not push to this branch). AC6.

### 5.3 The checker — `ci/wasm-lockstep/lockstep_check.py`

Stdlib only (`tomllib`, Python 3.11+; `ubuntu-latest` has 3.12), so the `propose` job needs no
package install. Two subcommands and two test modes:

- `lock --old <path> --new <path>`: the lock verdict.
  - Parse both locks. Key each package by `(name, version, source)`.
  - Removed set R and added set A. Every name in R and A is one of the seven family names.
    Each family name has at most one removed and one added entry.
  - Each family name occurs exactly once in the new lock (no two versions side by side).
  - Every added entry has `source = "registry+https://github.com/rust-lang/crates.io-index"` and
    a `checksum`.
  - Every old and new family version matches a strict semver regex
    (`^[0-9]+\.[0-9]+\.[0-9]+$`, no pre-release, no build metadata). Anything else is a refusal.
  - Every non-family package is identical in both locks, after the checker normalises a family
    reference in `dependencies`. Cargo writes a reference in three forms: `"name"`,
    `"name version"` and `"name version (source)"`. The checker handles all three.
    SMA-738 note (2026-10-05): this invariant is replaced. `R-NONFAMILY` now compares the
    dependency references of a non-family package by bare name, in order. The new refusal `R-EDGE`
    accepts a moved reference in one case only. The old and the new reference must each be the
    exact form that cargo writes for a package of its lock. See
    `docs/superpowers/specs/2026-10-05-sma-738-wasm-lockstep-edge-flips-design.md`.
  - The top-level `version` of the lock format is unchanged, and no `[patch]`/`[metadata]` table
    changed.
  - R and A empty: exit 4 ("no change").
  - The seven-count is NOT asserted. A new family release can drop one of the seven (pass) or
    bring a new transitive dependency (refused, AC5, the message points to the runbook).
  - Output on pass: one line per family package, `name old new`, each version already validated.
- `artifact --dir <d> --old <lock> [--body-file <f>] [--title-file <f>]`: the artifact verdict,
  then the `lock` verdict on `<d>/rs/Cargo.lock`, then the PR title and body files.
  - The directory tree holds `rs/Cargo.lock` and a subset of
    `rs/crates/bindings/paigasus-wasm/{paigasus_wasm.js, paigasus_wasm_bg.js, paigasus_wasm.d.ts,
    paigasus_wasm_bg.wasm.d.ts, paigasus_wasm_bg.wasm}`, all regular files, no other entry, no
    symlink, no directory other than the path prefixes.
  - Each file is under a size cap (8 MiB each; the plan records the committed binary's size).
- Exit codes: 0 pass, **3 refusal**, 2 infrastructure (unreadable file, TOML parse error), 4 no
  change. Not 1: `uv` exits 1 on a failed resolution, and `ci/CLAUDE.md` records the same choice
  for `workflow_credentials.py`. `run.sh` maps 3 -> 1 and every other non-zero code -> 2.
- `--self-test`: an in-process fixture table. Each refusal row must fail for its own named
  reason, not only exit 3. Rows at least: the M0 bump (pass); a bump that drops one family package
  (pass); an eighth package added (refuse); a non-family package changed (refuse); a git source on
  a family entry (refuse); a missing checksum (refuse); a family name moved twice (refuse); two
  versions of one family name in the new lock (refuse); a non-semver version string, for example
  `0.2.129"; rm -rf /` and `0.2.129-rc.1` (refuse); no change (4); each of the three reference
  forms in a non-family `dependencies` list (pass when only the family version changed); an
  artifact in the real downloaded layout (pass); the artifact under an extra `rs/` level or
  without the `rs/` level (refuse); a sixth artifact file (refuse); a missing `rs/Cargo.lock`
  (refuse); a symlink (refuse); an oversize file (refuse); a lock that does not parse (2).
- `--negative-control`: runs the real `lock` verdict against `git show HEAD:rs/Cargo.lock` (not the
  working tree, F15) with a mutated copy (one non-family version changed) and asserts exit 3, and
  against itself and asserts exit 4.

### 5.4 The gate — `repo:wasm-lockstep`

A root `moon.yml` task in the shape of `repo:workflow-credentials`:

```
set -euo pipefail
bash ci/wasm-lockstep/run.sh --self-test
bash ci/wasm-lockstep/run.sh --negative-control
bash ci/wasm-lockstep/run.sh
```

`run.sh` runs both checkers in each mode: `lockstep_check.py` (stdlib) and `pin_check.py`.
`run.sh` exports `PROTO_REPORTER=text` at the top.

**The pin check — `ci/wasm-lockstep/pin_check.py`.** It parses `wasm-lockstep.yml` with PyYAML
(F14), and handles the five coercions and aliases of `ci/CLAUDE.md:264-270` (a bare `on:` key as
`True`, boolean `if`/`continue-on-error`, a scalar `needs:`). It asserts:

- the job set is exactly `{build, propose}`;
- the triggers are exactly `schedule` and `workflow_dispatch`;
- `build` has no `environment`, no job-level `uses:` or `secrets:`, and no `secrets` context in
  any form in any value (`secrets.`, `secrets[`, `toJSON(secrets)`; the check searches every
  string for the word `secrets` as an expression context);
- `build` runs the third-party steps only inside the `docker run` form of 5.1: the `docker run`
  lines carry no `-e`, `--env`, `--env-file`, `--privileged`, `--cap-add`, `-v
  /var/run/docker.sock`, `--pid=host` or `--network=host`;
- in `propose`, every `uses:` value is on an allowlist (checkout, download-artifact,
  create-github-app-token, each at its pinned SHA), and every `run:` command word is on an
  allowlist (`python3 ci/wasm-lockstep/lockstep_check.py`, `git`, `gh`, `cp`, `test`, `cat`,
  shell built-ins). The check reads command words, not free text, so a PR body that says
  "cargo update" does not fail it;
- the `propose` step order is checkout, download, `lockstep_check.py artifact`, copy, token,
  commit, base check, branch-owner check and push, PR;
- the push refspec is exactly `HEAD:refs/heads/deps/wasm-bindgen-lockstep` with
  `--force-with-lease`;
- `needs.build.outputs` occurs in `propose` only in `if:` values;
- the top-level `permissions` is `contents: read`, and no job widens it.

The pin check has its own `--self-test` rows (T3) and a `--negative-control` that runs it on a
mutated copy of the real workflow (a `secrets` read added to `build`) and asserts a refusal.

The pin check needs PyYAML, so `ci/wasm-lockstep/` gets its own uv project (`pyproject.toml` with
a bounded `pyyaml` pin, `uv.lock`) and a new dependabot `uv` entry with the settings of the two
existing gate entries (`.github/dependabot.yml:81-123`). `run.sh` calls it with `uv run --locked
--project ci/wasm-lockstep --python '>=3.12'`. `lockstep_check.py` stays stdlib-only, because
`propose` runs it with the runner's `python3`. (Reusing the `ci/workflow-credentials` project is
the alternative; this spec prefers a separate project so the two gates do not share a lock.)

**Inputs:** `ci/wasm-lockstep/**/*`, `.github/workflows/wasm-lockstep.yml`, `rs/Cargo.lock`,
`.prototools` (the gate shells out to the pinned `uv`, the same reason as `moon.yml:844-846`).

**Registration: all seven obligations of `ci/CLAUDE.md`.** Only the first two are self-enforcing;
a new gate with no other entries still passes, so the plan does each one by hand:

1. `:wasm-lockstep` in `ci.yml`'s single-line `T=(…)` array.
2. `:wasm-lockstep` in the root `CLAUDE.md` `ci-targets` marker block, identical to 1.
3. `SELF_SCHEDULED_GATES["wasm-lockstep"]` in `ci/affected-graph/ci_targets.py` (the four
   `moon.yml` lines; `check_self_scheduled_coverage` enforces this one).
4. `SELF_TASK_EXPECTED_GLOBS["wasm-lockstep"]` (the literal inputs).
5. A `WASM_LOCKSTEP_SH_CALL_SITES` pin for the `run.sh` lines: the flag parse, the dispatch arms,
   the capture of each checker's run, the guard on its exit status and the 3 -> 1 mapping.
6. `REQUIRED_REPO_TASKS` gets `wasm-lockstep`, because the gate has a `--negative-control`.
7. `ci/wasm-lockstep/**/*` in `repo:affected-smoke`'s `inputs`, floored by a
   `T_AFFECTED_SMOKE_REQUIRED_INPUTS` entry in `ci/actionlint/run.sh`.

`repo:input-liveness` then checks the inputs. The new Python files must pass `repo:ruff-ci`.

### 5.5 Documentation

- `rs/CLAUDE.md`: the wasm-bindgen bullet names the workflow as the normal path. The manual
  runbook stays for the refusal cases (AC5), the `reqwest` case and a `wasm-pack` bump. The
  "`generate-wasm` on ONE host" text (line 194) changes to the new rule.
- The new rule, in `ts/packages/paigasus-kernel/moon.yml:189-191,215`, `ts/CLAUDE.md:132` and
  `rs/CLAUDE.md:194`: "ONE host per PR regenerates the artifacts. A family bump from the
  `wasm-lockstep` workflow regenerates on Linux inside its build container; a kernel or binding
  edit regenerates on the author's host. The binary therefore changes host between PRs (F10).
  `CI` never regenerates; it only compares."
- `rs/Cargo.toml:143-151`: the runbook pointer adds the workflow.
- `.github/dependabot.yml`: the comment above `/rs` says the family moves through
  `wasm-lockstep.yml`. The new `uv` entry for `/ci/wasm-lockstep` (5.4).
- `committed-wasm.test.ts`: the `REGENERATE` sentence adds "or run the `wasm-lockstep` workflow".
- `ci/wasm-lockstep/README.md`: the verdict rules, the exit codes, the trust model (5.1 and 5.2),
  the output rule, and what the checks do not prove (section 9).

## 6. Error handling

| Case | Result |
|---|---|
| The family is current | `build` green, notice. `propose` closes an open bot PR, if one exists, and ends green. |
| The lock checker refuses in `build` step 5 | `build` red before any compile. The message names the entry and the runbook. |
| `generate-wasm` fails (for example the pinned `wasm-pack` does not support the new 0.2.z) | `build` red. The runbook's `.prototools` bump is manual; the error text is recorded in `rs/CLAUDE.md` when it first occurs. Sven watches the red runs (Q10). |
| The identity guard fires on the container's user or `HOME` (F3) | `build` red. Pick a container `HOME` and user that the guard accepts, or widen `redactions()` in a separate change; never weaken the guard. M0 and M2 measure this before the merge. |
| The artifact or lock check refuses in `propose` | `propose` red, no token minted, nothing pushed. |
| `main` moved | `propose` green with a notice and the two SHAs, no push. The next scheduled run uses the new base. |
| A person pushed to the bot branch | `propose` red with a named reason, no push. |
| A bot PR is open already | `--force-with-lease` push and `gh pr edit`. |
| The environment secret is missing | The token step fails red. Unlike `release.yml`, no silent skip: the workflow has no other purpose. |
| GitHub refuses the push because the App token lacks the `workflows` permission | `propose` red. Q11 decides the fix. |

## 7. Safety constraint 4 — `repo:workflow-credentials`

Approach A adds no `workflow_run` trigger. The new workflow's triggers are `schedule` and
`workflow_dispatch`, which carry no pull-request code. So it is not a subject of the gate, and the
gate's README non-goal for `workflow_run` stays true ("not used in this repository"). No change to
`workflow_credentials.py`. If Sven picks approach B (Q1), the change is: add `workflow_run` to
`PR_TRIGGERS`, two `TRIGGER_CASES` rows, add the workflow to `EXPECTED_PR_SUBJECTS`, and one
`PR_CREDENTIAL_ALLOWED[("wasm-lockstep-pr.yml", "R4")]` entry with the reason; plus the four
event-origin checks in 4.B as the first step of workflow 2, covered by the pin check.

## 8. Test strategy and verification

**M0. A pre-merge measurement on a real runner.** Before the merge, push a scratch branch that
holds ONLY the `build` job as a `push`-triggered workflow, with `contents: read`, no
`environment` and no secrets. Its cache scope is that branch, not `main`. Record:

- whether `moon run paigasus-kernel-ts:generate-wasm` runs inside the container (no `CI`, no
  `GITHUB_ACTIONS`), and, for reference, whether it runs on the host with `CI` and
  `GITHUB_ACTIONS` both set, and which override works there (`env -u CI -u GITHUB_ACTIONS` or
  `CI=false`);
- which `^:build` tasks run, and whether `pnpm install` is needed;
- the `--post` result on the real container paths and user (F3);
- the downloaded artifact tree (a second job in the scratch workflow downloads it and lists it);
- that `ACTIONS_RUNTIME_TOKEN` is absent from `env` inside the container, and that
  `/proc/*/environ` of the runner processes is not visible there;
- the job time and the artifact size.

Delete the scratch branch afterwards. The results go into the plan and the README.

- **M1.** (Folded into M0.) A local `CI=true moon run …` is not a substitute, because the
  runner also sets `GITHUB_ACTIONS`.
- **M2.** Before M0, run container run 2 locally in Docker with the same image, user and `HOME`
  as 5.1, to catch a `--post` failure early. M0 is the proof; M2 is a fast pre-check.
- **M3.** One `workflow_dispatch` run on `main` after the merge. `rs/Cargo.lock` holds 0.2.128 and
  0.2.129 is published (F16), so this run proves AC2 with a real change, unless a person runs the
  runbook first. Record the `propose` result and that the PR's `CI` started.

Tests that stay in the repo:

- **T1.** `lockstep_check.py --self-test`: the fixture table in 5.3.
- **T2.** `lockstep_check.py --negative-control` on `git show HEAD:rs/Cargo.lock` (5.3).
- **T3.** `pin_check.py --self-test`, with rows that must red: a `secrets.X` read in `build`; a
  `secrets['X']` read; a `toJSON(secrets)` read; a third job with `environment: release-pr`; a
  job-level `uses:` with `secrets: inherit` in `build`; a `docker run -e …` in `build`; the token
  step before the checker step; a `cargo` command in `propose`; a push refspec to
  `refs/heads/main`; `needs.build.outputs.x` in a `run:` in `propose`; a YAML alias that injects a
  `secrets` read; a bare `on:`. A row that must pass: a correct workflow whose PR body text says
  "cargo update".
- **T4.** Mutation proof (memory `red-first-is-not-proof-delete-the-feature`): delete the source
  check, the semver check, the extra-file check, the output-rule check and the job-set check one
  at a time, and show a self-test row reds for each. The mutation must run, not only compile.
- **T5.** `repo:actionlint` passes on the new workflow (actionlint plus shellcheck on every `run:`).
- **T6.** `repo:affected-smoke` passes with all seven registry entries; `repo:input-liveness`
  passes with the new inputs; `repo:ruff-ci` passes on the new Python files.
- **T7.** `repo:workflow-credentials` stays green and unchanged (approach A).
- **T8.** The full `moon ci` graph from the root `CLAUDE.md` in CI on the PR.

## 9. Residual risk

- The artifacts come from a build that ran third-party code. The checks prove paths and the
  lock shape, not content. The control is the PR's `CI` and a human review (safety constraint 2).
  An attacker who controls a new family release controls the glue; a human reads the lock diff,
  not the five files.
- **The review is a procedure only.** The `main` ruleset has no `pull_request` rule (F6), so
  nothing stops a merge before a person reads the lock diff. Also, the PR's own `CI` runs the
  unreviewed code before any review. It runs in the PR cache scope, with `contents: read` and no
  secrets, the same exposure as a dependabot PR today.
- **Cache scope.** The container keeps the build code away from `ACTIONS_RUNTIME_TOKEN` (F11). If
  a container escape exists on the hosted runner, the build code can write to the `main` cache
  scope, and `prebuild.yml`/`wheels.yml` can restore that entry into a release build (F12). The
  container reduces this risk; it does not prove it is zero. Q7 asks Sven to accept this or to
  choose another control.
- The host steps after the compile trust `cp`, `test` and the pinned upload action with files
  that the build wrote. They execute no file from the work copy.
- Between the base check and the push, `main` can still move. The PR is based on `github.sha`,
  and the strict up-to-date rule (F6) then forces an update before merge. The update can conflict
  in `rs/Cargo.lock`; the next scheduled run replaces the branch.
- The `propose` job holds the App key for the whole job, not only after the token step. A defect
  in `download-artifact`'s unpack code can reach it. The control is that no step executes
  artifact content (5.2).
- The `reqwest` case of SMA-683 (a dependabot PR that moves `wasm-bindgen`) is NOT covered. The
  issue lists it as optional ("It can also cover"). It stays on the manual runbook.
- The `propose` job trusts `ubuntu-latest`'s `python3`, `git` and `gh`.
- GitHub delays or drops scheduled runs (`security-scan.yml:21-22`). A failed scheduled run
  notifies only the user who last changed the cron line. Sven owns the scheduled runs (Q10), so
  the cron line must be last changed by Sven, or the notification goes to someone else.

## 10. Files

| File | Change |
|---|---|
| `.github/workflows/wasm-lockstep.yml` | New. |
| `ci/wasm-lockstep/lockstep_check.py` | New. Stdlib only. |
| `ci/wasm-lockstep/pin_check.py` | New. PyYAML. |
| `ci/wasm-lockstep/run.sh` | New. Exit-code mapping (3 -> 1, else -> 2), `PROTO_REPORTER=text` export. |
| `ci/wasm-lockstep/pyproject.toml`, `uv.lock` | New. A bounded `pyyaml` pin. |
| `ci/wasm-lockstep/README.md` | New. |
| `moon.yml` | New `wasm-lockstep` task; `ci/wasm-lockstep/**/*` in `repo:affected-smoke`'s inputs. |
| `.github/workflows/ci.yml` | `:wasm-lockstep` in `T=(…)`. |
| `CLAUDE.md` | `:wasm-lockstep` in the `ci-targets` marker block only. |
| `ci/affected-graph/ci_targets.py` | `SELF_SCHEDULED_GATES`, `SELF_TASK_EXPECTED_GLOBS`, `WASM_LOCKSTEP_SH_CALL_SITES`, `REQUIRED_REPO_TASKS` entries. |
| `ci/actionlint/run.sh` | `T_AFFECTED_SMOKE_REQUIRED_INPUTS` entry. |
| `.github/dependabot.yml` | New `uv` entry for `/ci/wasm-lockstep`; the `/rs` comment. |
| `rs/CLAUDE.md`, `ts/CLAUDE.md`, `ts/packages/paigasus-kernel/moon.yml`, `rs/Cargo.toml`, `ts/packages/paigasus-kernel/tests/committed-wasm.test.ts` | Text (5.5). |
| `.github/CODEOWNERS` | Regenerated by Moon if the new directory needs an owner; never by hand. |

Estimated size: large. One workflow with a container build, two checkers with fixture tables,
seven registry entries, a pre-merge runner measurement, and text.

## 11. Out of scope

- The `reqwest` case (9).
- An automatic `wasm-pack` bump in `.prototools`.
- Moving the family on a dependabot branch.
- Any change to `release.yml` or to the SMA-680 settings.
- A `dry-run` dispatch input (F16 makes it unnecessary; the former Q4).

## 12. Open questions

- **Q1. DECIDED (Sven, 2026-10-02): approach A.** One workflow, two jobs, environment boundary.
  `repo:workflow-credentials` does not change (section 7).
- **Q2. DECIDED (Sven, 2026-10-02): reuse `release-pr`.** No new secrets. The deployment history
  of that environment then holds release PRs and lockstep PRs.
- **Q3. Default unless Sven objects.** The schedule: weekly is from the issue. Which day and time? The spec proposes Tuesday
  06:17 UTC, so it does not collide with the Monday 06:00 UTC dependabot run
  (`rs/CLAUDE.md:223`) or with `security-scan.yml`'s 07:17.
- **Q5. Default unless Sven objects: `deps/`.** The bot branch name `deps/wasm-bindgen-lockstep` does not follow `feature/sma-NNN-<slug>`.
  The convention in the root `CLAUDE.md` is for human branches. Is a `deps/` prefix acceptable, or
  should the PR link a standing Linear issue?
- **Q7. DECIDED (Sven, 2026-10-02): the container is the control.** Sven accepts the residual in
  section 9 (a container escape can still write to the `main` cache scope). No `sudo` removal.
- **Q8. Default unless Sven objects: keep it as a green notice.** AC4.3 ("`main` did not move") comes from the issue outline. In approach A it has no
  security purpose: `propose` checks the artifact against the lock at `github.sha` and bases the
  PR on `github.sha`. The spec keeps it but makes it a green notice. Do you want to remove it?
- **Q9. Default unless Sven objects: commit with git, as `release.yml` does.** Should `propose` make the commit through the API (`createCommitOnBranch`)? The commit is
  then signed as the App, and the design keeps working if the ruleset later requires signed
  commits. The cost is a different commit path than `release.yml`.
- **Q10. DECIDED (Sven, 2026-10-02): a refusal stays red, and Sven watches the runs.** No
  tracking issue. A refusal (a newer `syn`, a `wasm-pack` bump) stays red every week until a person
  runs the runbook.
- **Q11. Default unless Sven objects: measure it at M3, then decide.** The force push moves `deps/wasm-bindgen-lockstep` across `main` commits, some of which
  can change `.github/workflows/*`. GitHub can refuse that push from an App token that does not
  have the `workflows` permission. Nobody measured this. If GitHub refuses it: a new branch per
  run (and close the old PR), or request the `workflows` permission (more token scope)?

(Q4 and Q6 are closed: see the changelog.)

## Challenge changelog

**Pass 1, 2026-09-27. Verdict: NEEDS REWORK.** Each finding was checked against the repo before
it was folded. The evidence the critique cites matched the repo in every case that was read
(`prebuild.yml:107-121`, `wheels.yml:159-172`, `release.yml:262-272,491,501`, `ci.yml:105-142`,
`images-rehearsal.yml:81-83,166-167`, the artifact action pins, `moon.yml:806-846`,
`.github/dependabot.yml:81-123`, `ci/CLAUDE.md:110-230,264-270`, `rs/CLAUDE.md:109-131`,
`.github/CLAUDE.md:43`, `ts/packages/paigasus-kernel/moon.yml:189-216`, `rs/Cargo.lock:6122-6123`).

Folded:

- BLOCKER cache poisoning: the build runs third-party code inside a container with no runner
  environment, over a copy without `.git` (5.1). AC3 now covers the runtime token, cache writes
  and artifact writes. F11 and F12 added. The residual and the sudo alternative went to section 9
  and Q7, because the choice of control is Sven's.
- BLOCKER untrusted outputs: `build` outputs only `changed`, set by the host before the compile.
  `propose` reads it only in `if:`. Versions come from the checker, validated by a semver regex,
  and reach commands through files and `env:` (5.2, 5.3). Pin-check and self-test rows added.
- MAJOR artifact layout: stage under `$RUNNER_TEMP/stage/rs/...` and upload one directory (F13,
  5.1). Self-test rows for the real layout and the two wrong layouts. M0 records the real tree.
- MAJOR text-scan pin check: replaced by a PyYAML parse with the coercions, the exact job set,
  the allowlists, the refspec pin and the outputs rule (5.4). A uv project and a dependabot entry
  added.
- MAJOR registration: all seven obligations listed in 5.4 and section 10. `.prototools` added to
  the inputs. `repo:ruff-ci` added to T6.
- MAJOR substitute measurements: M0, a pre-merge scratch-branch run on a real runner. The
  `moon setup` and serial `rustup` steps of `ci.yml` added to the container run.
- MINOR one-host rule: the new rule and the texts to change are in AC9 and 5.5.
- MINOR AC4.3: kept (it is in the issue outline), but "main moved" is now a green notice, and the
  check uses the exit status of `gh api`. Q8 asks whether to remove it.
- MINOR force push and stale PR: a branch-owner check plus `--force-with-lease`; a `changed=false`
  run closes an open bot PR.
- MINOR exit code: refusal is 3, mapped in `run.sh`.
- MINOR negative control: reads `git show HEAD:rs/Cargo.lock`.
- MINOR lock-checker rules: the three reference forms, the exactly-once rule, and the version
  validation. The case/whitespace fixture idea is replaced by the reference forms.
- MINOR AC wording: AC2 and AC4 now match 5.2 and 5.3 (a subset of the five files; no
  seven-count).
- MINOR stdlib project: `lockstep_check.py` stays stdlib; the uv project exists only for PyYAML.
  The "or v4" artifact fallback is removed; the v7.0.1/v8.0.1 pins are named.
- MINOR step order: section 5.2 and 9 now state that the control is "no step executes artifact
  content", not the token step order.
- MINOR dry-run input: removed (F16); the former Q4 is closed.
- MINOR Q6: answered by `release.yml:262-272`; the explicit permission inputs are copied. Closed.
- MINOR watcher: added to section 9 and as Q10.
- MINOR human review: section 9 states that the ruleset does not enforce it and that the PR's `CI`
  runs the code before the review.
- QUESTIONS: added as Q9 (API commit), Q10 (weekly refusal), Q11 (`workflows` permission).

Rejected:

- None rejected outright. One part was only partly folded: the critique offered "remove AC4.3".
  The spec keeps the check because the issue outline states it, makes it non-blocking, and asks
  Sven (Q8).
- The critique says "add `repo:ruff-ci` to the T-list". `repo:ruff-ci` is already in `T`
  (root `CLAUDE.md` marker block). The spec reads this as "the new Python files must pass it" and
  added that to T6.

Implementation notes (2026-10-02):

- The build container runs as `nobody` (65534), not as the runner uid. Node `os.userInfo()` needs
  a passwd entry, and the image has none for the runner uid.
- The host gives the work copy to 65534 with `sudo chown -R`. M0 run 1 failed with EPERM on a
  `utimes` call with an explicit time, because the runner uid owned the copy.
- The container runs with `--cap-drop=ALL --security-opt=no-new-privileges`.
- The build downloads rustup and Rust 1.95.0 at run time, through proto.
