# `ci/wasm-lockstep/` — the lockstep updater and its gate (SMA-693)

`.github/workflows/wasm-lockstep.yml` moves the wasm-bindgen family in lockstep and regenerates the
five committed wasm artifacts. It opens or updates ONE pull request on the bot branch
`deps/wasm-bindgen-lockstep`. A person reviews the lock diff before the merge.

`repo:wasm-lockstep` is the gate over that workflow and its checker. Spec:
`docs/superpowers/specs/2026-09-27-sma-693-wasm-bindgen-lockstep-updater-design.md`.

## Files

| File | Job |
|---|---|
| `lockstep_check.py` | The lock and artifact checker. Stdlib only: the `propose` job runs it with the runner's own `python3`. |
| `pin_check.py` | The pin check of the workflow. PyYAML, from this directory's own uv project. |
| `container.sh` | The two container runs of the `build` job. It runs inside the build container only. |
| `run.sh` | The gate wrapper: `--self-test`, `--negative-control`, and the real run. |
| `pyproject.toml`, `uv.lock` | PyYAML for `pin_check.py`. Dependabot and `repo:osv` watch the lock. |

## Exit codes

| Code | `lockstep_check.py` | `pin_check.py` | `run.sh` |
|---|---|---|---|
| 0 | pass | pass | pass |
| 1 | (a Python traceback) | (a Python traceback) | an assertion failed |
| 2 | infrastructure | infrastructure | infrastructure |
| 3 | refusal | a rule failed, or the YAML does not parse (rule P0) | (not used) |
| 4 | no change | (not used) | (not used) |

A YAML parse error in `pin_check.py` is a P0 refusal. It exits 3, not 2.

`run.sh` maps 3 to 1 and every other non-zero code to 2. `uv` and a Python traceback both exit 1,
so a checker that exits 1 for a refusal would make a PyPI outage read as "the lock is wrong". Do not
"normalize" a checker to 1.

## The lock verdict (`lockstep_check.py lock`)

The family is the seven packages the four-package `cargo update -p` moved at SMA-683 M0:
`wasm-bindgen`, `wasm-bindgen-macro`, `wasm-bindgen-macro-support`, `wasm-bindgen-shared`,
`js-sys`, `web-sys` and `wasm-bindgen-futures`. Each refusal has a code. The self-test asserts the
code of each row, so a row cannot pass because a different check refused it.

| Code | The refusal |
|---|---|
| `R-FORMAT` | The lock format version or a top-level table (`[patch]`, `[metadata]`) changed. |
| `R-SHAPE` | A `[[package]]` entry has no string name and version, or a key appears twice. |
| `R-NONFAMILY` | A package outside the family changed, was added or was removed. Cargo writes a dependency reference in three forms (`name`, `name version`, `name version (source)`); a reference to a family package becomes the bare name before the comparison. |
| `R-SEMVER` | A family version is not a strict `X.Y.Z`: no pre-release, no build metadata, no leading zero, ASCII digits only, no trailing newline. |
| `R-TWICE` | A family name has more than one removed or more than one added entry. |
| `R-DUPLICATE` | The new lock holds two versions of one family name side by side. |
| `R-SOURCE` | An added family entry does not come from crates.io. |
| `R-CHECKSUM` | An added family entry has no 64-hex checksum. |
| `R-INPLACE` | A family entry kept its version and changed its checksum. |
| `R-ABSENT` | The new lock holds no `wasm-bindgen` entry. `current` gives the same code for a lock without it. |
| `R-DANGLING` | A remaining package still refers to a family name that the new lock does not hold. |

No change exits 4. The count of seven is NOT asserted: a family release can drop a package (pass) or
bring a new transitive dependency (`R-NONFAMILY`, so the manual runbook applies).

`artifact` adds `R-LAYOUT`, `R-SYMLINK` and `R-SIZE` (the tree holds `rs/Cargo.lock` and a subset of
the five artifact files, all regular files of 8 MiB or less), `R-NOCHANGE` (the artifact lock equals
the checked-out lock) and `R-TITLE` (the commit title is over 100 characters). It writes the PR title
and body files only after every check passed. `status` (`R-STATUS`) checks `git status --porcelain`
after the copy. `current` checks the family invariant on one lock; the real run of the gate runs it
on `HEAD:rs/Cargo.lock`, so a PR that puts a second `wasm-bindgen` into the lock goes red at once,
not on the next Tuesday.

## The pin rules (`pin_check.py`)

| Rule | The assertion |
|---|---|
| P0 | The file is one YAML mapping, with no duplicate key and no merge key (`<<:`). GitHub Actions does not merge a merge key. A YAML parse error is also a P0 refusal. |
| P1 | The jobs are exactly `build` and `propose`. |
| P2 | The triggers are exactly `schedule` and `workflow_dispatch` (a bare `on:` parses as `True`; both keys are read). |
| P3 | `build` declares no environment and reads no `secrets` context. The workflow level reads none. `propose` reads it only in the step `token` (its `env:` and `if:` are not checked). |
| P4 | Every `docker run` uses exactly `--rm --cap-drop=ALL --security-opt=no-new-privileges --user 65534:65534 --volume "$RUNNER_TEMP/work:/work" --workdir /work "$LOCKSTEP_IMAGE"` and runs `bash ci/wasm-lockstep/container.sh update` or `build`. `LOCKSTEP_IMAGE` is a rust image pinned by a sha256 digest. No script and no `env:` key below the workflow level can set `LOCKSTEP_IMAGE`. |
| P5 | Every command word of a `run:` script is on the job's allowlist. `python3` runs `lockstep_check.py` only. A `$(...)` is read too. A backtick, a here-document, an arithmetic expansion, a process substitution, `case` and a bare `!` negation are refused. |
| P6 | Every `uses:` is an allowlisted action, pinned to a full 40-hex commit SHA. |
| P7 | The `propose` step ids are exactly `checkout, download, verify, apply, token, commit, base, push, pr, close`, and `verify` runs `lockstep_check.py artifact`. |
| P8 | The workflow holds ONE `git push`, in step `push`, with one `--force-with-lease=refs/heads/deps/wasm-bindgen-lockstep:<sha>`, the App-token remote and the refspec `HEAD:refs/heads/deps/wasm-bindgen-lockstep`. |
| P9 | The `needs` context appears only in `if:` values. `build` outputs only `changed`. |
| P10 | The top-level and each job's `permissions` are exactly `contents: read`. |
| P11 | `propose` has `environment: release-pr`, `needs: build` and `if: needs.build.result == 'success'`. |
| P12 | No job declares `container`, `services`, `uses`, `secrets` or `defaults`. |
| P13 | No step sets a shell other than `bash`, and the workflow has no `defaults:`. |
| P14 | No step and no job sets `continue-on-error` to anything but false. |
| P15 | No step `if:` calls `always()`, `failure()` or `cancelled()`. |
| P16 | The token step uses `actions/create-github-app-token` with exactly `client-id`, `private-key`, `permission-contents: write` and `permission-pull-requests: write`. |
| P17 | Every checkout sets `persist-credentials: false`. |
| P18 | The `verify` and `status` commands of `lockstep_check.py` are whole commands: the last command of the step, with no `\|\|`, `;`, pipe, `&&` guard, `if` or later `exit` joined to them. A refusal then fails the step. |
| P19 | No step sets `working-directory`. No `env:` key at any level (workflow, job or step) is `BASH_ENV`, `ENV` or `ACTIONS_ALLOW_UNSECURE_COMMANDS`. No script names `GITHUB_ENV` or `GITHUB_PATH`. |
| P20 | `propose` makes no `gh api` call other than GET: no `-X` or `--method` with another verb, no `-f`, `-F`, `--field`, `--raw-field` or `--input`, and no `graphql`. No `git` call takes `-c` or `--config-env`. |
| P21 | The `gh pr list --head` call passes `--json` and `--jq` that name `isCrossRepository`, and the jq selects `select(.isCrossRepository \| not)`. The App token then never edits a pull request from a fork. |
| P22 | `propose` runs only these `gh` commands: `api`, `pr list`, `pr create`, `pr edit` and `pr close`. Any other one (`gh pr merge`, `gh workflow run`, `gh repo`, `gh secret`) is refused. |
| P23 | Every `gh pr list` passes `--head deps/wasm-bindgen-lockstep`. A step that runs `gh pr close` or `gh pr edit` must also run such a list. The check does not follow the number from the list to the close. It requires the list in the same step. |
| P24 | No expression in the workflow reads the outputs of the build steps `update` or `build` (the container runs), in any case or index form. |

The P6 rule pins the action NAME and a full SHA, not one specific SHA, so a dependabot action bump
does not turn this gate red. A new action, or a tag in place of a SHA, does.

## The trust model

- **`build`** has no environment and reads no secret. Its `GITHUB_TOKEN` is `contents: read`. Every
  step that runs third-party code (cargo, build scripts, proc-macros, `wasm-pack`,
  `wasm-bindgen-cli`, pnpm packages) runs inside `docker run`, as the user `nobody`, over a copy of
  the tree without `.git`. The container gets no runner environment: no `CI`, no `GITHUB_ACTIONS`,
  no `ACTIONS_RUNTIME_TOKEN`, no `GITHUB_TOKEN`, no `GITHUB_OUTPUT`. So that code cannot write a
  cache entry in the `refs/heads/main` scope or an artifact (spec F11, AC3).
- After the compile, the host runs no file of the work copy and no `git` command on it. It checks
  each path component for a symlink, then copies the six files with `cp`.
- The host writes `changed` BEFORE the compile, so third-party code cannot set it.
- **`propose`** holds the App key for the whole job (environment `release-pr`, main-only branch
  policy). The control is that no step executes artifact content: no toolchain, no cargo, no pnpm,
  no moon, and no script from the artifact. A refusal fails the `verify` step, so the token step
  never runs.
- **The output rule.** `propose` reads `needs.build.outputs.changed` only in `if:` values. Every
  value that reaches a command (the versions, the PR title and body) comes from `lockstep_check.py`
  on the downloaded bytes, through a file or `env:`.
- **Why the user `nobody`.** `generate-wasm --post` calls Node's `os.userInfo()`, which throws when
  the uid has no passwd entry. The image has no entry for the runner's uid 1001. `nobody` (65534)
  has one. The identity markers that `--post` then rejects are `nobody` and `lockstep-home`.

## What the checks do not prove

- The artifacts come from a build that ran third-party code. The checks prove paths and the lock
  shape, not content. A person reads the lock diff, not the five files.
- Nothing enforces the review. The `main` ruleset has no `pull_request` rule. The PR's own `CI`
  runs the unreviewed code before any review, in the PR cache scope, with no secrets: the same
  exposure as a dependabot PR.
- A container escape on the hosted runner can still reach the `main` cache scope. Sven accepted
  this residual (SMA-693 Q7).
- `pin_check.py` reads command words, not shell semantics. It reads a quoted `$(...)` by bracket
  depth only.
- A hostile author can edit `pin_check.py` in the same PR. PR review is the control for that.
- `pin_check.py` does not refuse `cp` from the work copy into the checkout. It does not refuse
  `git -C <work copy>`.
- `tar --checkpoint-action` and GNU `sed e` can run a command through a word on the allowlist.
- `git config <key>` in `propose` acts like `git -c`. P20 does not refuse it.
- The `GIT_CONFIG_COUNT` environment variable, `gh alias set` and `gh extension exec` pass the
  `propose` allowlist.
- P19 does not see an indirect form, such as `${!f}`.
- A bundled short flag, such as `gh api -iXPOST`, passes P20.
- P21 matches the jq `select` with a regular expression. It does not parse the jq.
- The build container downloads rustup and the Rust 1.95.0 toolchain at run time through proto.
  Nobody verified that proto checks the rustup download (M2 notes).
- `container.sh` fetches `proto.sh` with `curl` and no checksum. This happens inside the
  container, and `propose` checks the output again.
- The branch-owner check uses `.author.login`. GitHub takes that value from the author email. The
  check guards against a mistake. It does not stop an attacker with push access.
- `pin_check.py` does not detect the removal of the per-component symlink loop of the stage step.
  It does not detect an upload of `$RUNNER_TEMP/work/` in place of the stage directory. The
  `propose` re-check of the layout (`R-LAYOUT`, `R-SYMLINK`) catches a wrong layout, not the
  removal of the loop.
- `propose` runs for the first time at M3. It cannot run on a scratch branch, because of the
  `release-pr` environment (main-only branch policy).
- A refusal stays red every week until a person runs the manual runbook in `rs/CLAUDE.md`
  ("The wasm-bindgen family does not move through dependabot"). Sven watches the runs (Q10).

## Local runs

`run.sh` runs under `/bin/bash` 3.2.57 and Homebrew bash 5. It has no `mapfile`, no `declare -A`, no
here-string and no pipe into an early-exit reader. It needs `uv` on `PATH` (the proto shims).

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
./ci/wasm-lockstep/run.sh --self-test
./ci/wasm-lockstep/run.sh --negative-control
./ci/wasm-lockstep/run.sh
```

## Measurements

### M2 — the local container pre-check

M2 ran container runs 1 and 2 on a Mac in the pinned image, with the workflow's flags (`--cap-drop=ALL`, `--security-opt=no-new-privileges`, `--user 65534:65534`). M2 is a pre-check. M0 is the proof.

| Item | Result |
|---|---|
| Date | 2026-10-02 |
| Platform | amd64 (emulated on an arm64 host) |
| Container run 1 | exit 0 |
| Lock verdict | `lock=0`, seven `family-moved` lines: `wasm-bindgen`, `-macro`, `-macro-support`, `-shared` 0.2.128 to 0.2.129; `js-sys` and `web-sys` 0.3.105 to 0.3.106; `wasm-bindgen-futures` 0.4.78 to 0.4.79 |
| Container run 2 | exit 0, 163 s (emulated, cold) |
| `--post` result | `generate-wasm: wrote 5 files into rs/crates/bindings/paigasus-wasm/` |
| `getent passwd 1001` | exit 2 (no entry) |
| `getent passwd 65534` | exit 0 (`nobody`) |
| Container environment names | `CARGO_HOME`, `HOME`, `HOSTNAME`, `PATH`, `PWD`, `RUSTUP_HOME`, `RUST_VERSION` (no `CI`, no `GITHUB_*`, no `ACTIONS_*`) |
| Artifact check | `artifact=0`, title `build(deps): move wasm-bindgen to 0.2.129 and regenerate the wasm glue` |
| Glue files that differ | `paigasus_wasm_bg.js` and `paigasus_wasm_bg.wasm` |
| Glue files that are the same | `paigasus_wasm.js`, `paigasus_wasm.d.ts`, `paigasus_wasm_bg.wasm.d.ts` |
| `paigasus_wasm_bg.wasm` size | 50902 bytes |

Two findings changed the setup:

- A native arm64 container cannot run `proto install`. The vendored `cargo-machete` plugin has no Linux arm64 target, so the download failed. The runner is amd64, so M2 used `--platform linux/amd64`.
- The image sets `RUSTUP_HOME` and `CARGO_HOME` under `/usr/local`. Moon looks for the toolchain under `HOME`. `container.sh build` now sets both to `$HOME`. Without this change, run 2 failed with `proto::locate::missing_executable`.

After this change the build container does not use the Rust toolchain of the image. It downloads rustup and the Rust 1.95.0 toolchain at run time, through proto. `.moon/toolchains.yml` (`rust.version: 1.95.0`) and `rs/rust-toolchain.toml` (`channel = "1.95.0"`) pin the version. Whether proto verifies the rustup download is not verified: M2 did not check it, and the build log was not kept.

The build log also holds the line `error: rustup is not installed at '/tmp/lockstep-home/.cargo'`. The build still finished with exit 0. M0 found where the line comes from. It appears directly after proto sets the default toolchain, inside the Rust setup of proto. The build continues and succeeds, so the line is not fatal. The cause inside the proto Rust plugin is not traced further.

### M0 — the scratch-branch run on a GitHub runner

Date: 2026-10-02. The runs used the scratch branch `feature/sma-693-m0-scratch`. The pre-push hook allows only `feature/*` branches, so this name replaces `scratch/sma-693-m0`. The branch never merges.

Run 1 failed. The step "Regenerate the wasm artifacts in the container" stopped with `EPERM: operation not permitted, utime '/work/rs/crates/libs/paigasus-kernel/src/lib.rs'`. The `--pre` step of `generate-wasm.mjs` made this call. The cause: `git archive | tar -x` gives the work copy to the runner uid. The container runs as uid 65534 with `--cap-drop=ALL`. A `utimes` call with an explicit time needs the file owner or CAP_FOWNER. M2 did not see the error, because Docker Desktop on macOS does not keep the owner of a bind mount. Commit 8e79ad0f fixed it. The work-copy step now runs `sudo chown -R 65534:65534 "$RUNNER_TEMP/work"` before the first `docker run`, and `pin_check` allows exactly that form.

Run 2 passed. These are its values.

| Item | Value |
|---|---|
| Run 1 (failed) | https://github.com/SMK1085/paigasus-core/actions/runs/37066309139 (head 1d550ddf) |
| Run 2 (passed) | https://github.com/SMK1085/paigasus-core/actions/runs/37067214900 (head 96b21680) |
| Job `build` | success, 21:30:18Z to 21:34:09Z (3 min 51 s) |
| Job `host-reference` | success, 21:30:18Z to 21:31:56Z (1 min 38 s) |
| Job `inspect` | success, 21:34:13Z to 21:34:19Z |
| Container options (both runs) | `--cap-drop=ALL --security-opt=no-new-privileges --user 65534:65534`, same image |
| Container env names | `CARGO_HOME`, `HOME=/nonexistent`, `HOSTNAME`, `PATH`, `RUSTUP_HOME`, `RUST_VERSION=1.95.0`, `SHLVL`, `_`, and one line that the runner log masks as `***`. The masked line sorts between `PATH` and `RUSTUP_HOME`. It is most likely `PWD=/work`. No `CI`, no `GITHUB_*`, no `ACTIONS_*`. |
| Runtime token in env | `runtime token in env: absent` |
| Process view | 4 numeric pids in `/proc`. This is the pid namespace of the container. Pid 1 is the probe's own bash. |
| `getent passwd 1001` | no entry |
| `getent passwd 65534` | `nobody:x:65534:65534:nobody:/nonexistent:/usr/sbin/nologin` |
| Lock verdict (host, rc 0) | `family-moved` js-sys 0.3.105 to 0.3.106; wasm-bindgen 0.2.128 to 0.2.129; wasm-bindgen-futures 0.4.78 to 0.4.79; wasm-bindgen-macro 0.2.128 to 0.2.129; wasm-bindgen-macro-support 0.2.128 to 0.2.129; wasm-bindgen-shared 0.2.128 to 0.2.129; web-sys 0.3.105 to 0.3.106 |
| Compile container setup | Moon installed proto 0.60.2, rust 1.95.0 and `unstable_python` 3.12.13. Rustup downloaded `1.95.0-x86_64-unknown-linux-gnu` with 6 components and set it as the default. |
| Moon tasks | `rustup-component-add`, `rustup-target-add`, then `paigasus-kernel-rs:build` (4.3 s), `paigasus-wasm-rs:build` (6.8 s), `paigasus-node-bindings-rs:build` (15.8 s), `paigasus-kernel-ts:generate-wasm` (12.9 s) |
| `generate-wasm` lines | `generate-wasm: wasm-pack 0.15.0, sources touched` and `generate-wasm: wrote 5 files into rs/crates/bindings/paigasus-wasm/` |
| Downloaded tree | `<dir>/rs/Cargo.lock` (156952 bytes) and `<dir>/rs/crates/bindings/paigasus-wasm/` with `paigasus_wasm.d.ts` (2465), `paigasus_wasm.js` (415), `paigasus_wasm_bg.js` (14016), `paigasus_wasm_bg.wasm` (50902), `paigasus_wasm_bg.wasm.d.ts` (1703). There is no extra `rs/` level and no `stage/` level. Files are mode 644 and directories are mode 755. |
| Artifact verdict | `lockstep_check.py artifact` passed with the seven `family-moved` lines. Title: `build(deps): move wasm-bindgen to 0.2.129 and regenerate the wasm glue` |
| Artifact size | `wasm-lockstep`, 60577 bytes (zip) |
| `host-reference`, with `CI=true` and `GITHUB_ACTIONS=true` | rc 1. This confirms spec F2: Moon fails the `runInCI: false` task. |
| `host-reference`, with `env -u CI -u GITHUB_ACTIONS` | rc 0. It wrote 5 files. `git status --porcelain` showed only ` M rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm`. The binary differs between hosts (F10). |
| `host-reference`, with `CI=false` | rc 1 |
| `timeout-minutes` of `build` | The build took 3 min 51 s, under 30 minutes. The value stays 60. |

### M3 — the first `workflow_dispatch` run on `main`

Not measured yet. This happens after the merge.

M3 creates the bot branch. So M3 does not answer Q11: whether a force-push across `main` commits that edit `.github/workflows/*` needs the `workflows` permission of the App. Only a later run can answer Q11, because only then does the bot branch exist and differ from `main` in those files.

### M4 — a second run with an open bot PR

Not measured yet. This run starts while the bot PR from M3 is open and `main` has moved. It shows whether the force-push, the branch-owner check and the PR update work on an existing bot branch (Q11).
