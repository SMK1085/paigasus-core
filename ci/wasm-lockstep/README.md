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
| `R-NONFAMILY` | A package outside the family was added or removed, or it changed its version, source, checksum or the names it depends on. The check compares the dependency references of a non-family package by bare name, in order, with their count (SMA-738). The check of `R-EDGE` decides which version a reference points to. |
| `R-EDGE` | A dependency reference moved in a way that cargo does not write, or moved to another source only (SMA-738). The five cases are in the list below the table. |
| `R-SEMVER` | A family version is not a strict `X.Y.Z`: no pre-release, no build metadata, no leading zero, ASCII digits only, no trailing newline. |
| `R-TWICE` | A family name has more than one removed or more than one added entry. |
| `R-DUPLICATE` | The new lock holds two versions of one family name side by side. |
| `R-SOURCE` | An added family entry does not come from crates.io. |
| `R-CHECKSUM` | An added family entry has no 64-hex checksum. |
| `R-INPLACE` | A family entry kept its version and changed its checksum. |
| `R-ABSENT` | The new lock holds no `wasm-bindgen` entry. `current` gives the same code for a lock without it. |
| `R-DANGLING` | A remaining package still refers to a family name that the new lock does not hold. |
| `R-RUN2` | (`same` only) The staged `rs/Cargo.lock` does not have the SHA-256 that the `lock` step printed. Or it is a symlink, it is not a regular file, or it is over 8 MiB. Container run 2 changed the lock (SMA-738). |

`R-EDGE` refuses in five cases:

1. One `dependencies` list of the new lock holds a reference two times.
2. An added reference is not the exact form that cargo writes for a package of the new lock.
3. A removed reference is not that form in the old lock.
4. One package has more than one removed or more than one added reference to one name.
5. The old and the new reference point to the same name and version, with another source. An
   example is a move from crates.io to git. An edge row cannot show this move. The message prints
   both references. This case was added after the final review (2026-10-05).

The form that cargo writes is `name` when the lock holds one package of that name. It is
`name version` when the lock holds one package with that name and version. Otherwise it is
`name version (source)`. A moved reference to a family name gets cases 1 to 3. `R-EDGE` runs
before the family checks and before the no-change decision.

No change exits 4. The count of seven is NOT asserted: a family release can drop a package (pass) or
bring a new transitive dependency (`R-NONFAMILY`, so the manual runbook applies).

**Moved edges (SMA-738).** `cargo update` can re-point a dependency edge of a non-family package
to another version that is already in the lock. Each such edge is an edge row: the package, its
version, the dependency, and the old and the new version, all from the old lock. `lock` prints one
`edge-moved <package> <package version> <dependency> <old> <new>` line per row, after the
`family-moved` lines. On exit 4 it prints them after the `family-current` lines, so the weekly log
shows when cargo re-points edges again. `artifact` prints the same lines, and the PR body then has a
second table, "Dependency edges that moved".

`lock` prints `lock-sha256 <64 hex>` on exit 0 only. The value is the SHA-256 of exactly the bytes
that the verdict judged. The reader reads the file once, in binary mode, with no newline
translation. `artifact` also prints `lock-sha256` for the downloaded lock. A reviewer can then match
the three values (`lock`, `same`, `artifact`) in the job logs.

`same --sha256 <64 hex> --file <path>` compares the bytes of one file with that value. It refuses
with `R-RUN2` when they differ. These cases exit 2:

- a malformed or empty `--sha256`;
- a missing or unreadable file;
- a flag that appears two times.

`artifact` adds `R-LAYOUT`, `R-SYMLINK` and `R-SIZE` (the tree holds `rs/Cargo.lock` and a subset of
the five artifact files, all regular files of 8 MiB or less), `R-NOCHANGE` (the family did not
move; the two locks can differ in dependency edges only) and `R-TITLE` (the commit title is over
100 characters). It writes the PR title and body files only after every check passed. `status`
(`R-STATUS`) checks `git status --porcelain` after the copy. `current` checks the family invariant
on one lock. The real run of the gate runs it on `HEAD:rs/Cargo.lock`. So a PR that puts a second
`wasm-bindgen` into the lock fails at once. It does not wait for the next Tuesday.

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
| P18 | The `artifact`, `status` and `same` commands of `lockstep_check.py` are whole commands, in every job: the last command of the step, with no `\|\|`, `;`, pipe, `&&` guard, `if` or later `exit` joined to them. A refusal then fails the step. The `lock` command is not one of them: its step keeps `\|\| rc=$?`. |
| P19 | No step sets `working-directory`. No `env:` key at any level (workflow, job or step) is `BASH_ENV`, `ENV` or `ACTIONS_ALLOW_UNSECURE_COMMANDS`. No script names `GITHUB_ENV` or `GITHUB_PATH`. |
| P20 | `propose` makes no `gh api` call other than GET: no `-X` or `--method` with another verb, no `-f`, `-F`, `--field`, `--raw-field` or `--input`, and no `graphql`. No `git` call takes `-c` or `--config-env`. |
| P21 | The `gh pr list --head` call passes `--json` and `--jq` that name `isCrossRepository`, and the jq selects `select(.isCrossRepository \| not)`. The App token then never edits a pull request from a fork. |
| P22 | `propose` runs only these `gh` commands: `api`, `pr list`, `pr create`, `pr edit` and `pr close`. Any other one (`gh pr merge`, `gh workflow run`, `gh repo`, `gh secret`) is refused. |
| P23 | Every `gh pr list` passes `--head deps/wasm-bindgen-lockstep`. A step that runs `gh pr close` or `gh pr edit` must also run such a list. The check does not follow the number from the list to the close. It requires the list in the same step. |
| P24 | No expression in the workflow reads the outputs of the build steps `update` or `build` (the container runs), in any case or index form. |
| P25 | The `build` job around the run-1 lock compare (SMA-738). The checks are in the list below the table. |

P25 holds these checks:

1. The `build` step ids are exactly `reclaim, checkout, ref, copy, update, lock, build, stage, upload`.
2. The `if:` of `build`, `stage` and `upload` is exactly `steps.lock.outputs.changed == 'true'`.
3. The exact pin. The whole `run:` text of the `lock` step equals `LOCK_RUN` in `pin_check.py`.
   The whole `run:` text of the `stage` step equals `STAGE_RUN`. If you edit one of these two
   scripts, change `pin_check.py` in the same commit. The pin refuses every change to the two
   scripts. So no shell code in them can set `LOCK_SHA256`, skip the compare or stop bash from
   running it.
4. The env allow-list. The workflow `env:` holds only `LOCKSTEP_IMAGE`. The `build` job has no
   `env:`. So no `env:` key at these two levels can set `PATH`, `PYTHONPATH` or `LD_PRELOAD` for
   the `stage` step. This covers `env:` keys only. The list "What the checks do not prove" names
   two other ways to set a variable for the `stage` step.
5. The `lock` step runs one checker command only:
   `python3 ci/wasm-lockstep/lockstep_check.py lock --old "$RUNNER_TEMP/old.lock" --new "$RUNNER_TEMP/work/rs/Cargo.lock"`.
6. The `stage` step runs one checker command only:
   `python3 ci/wasm-lockstep/lockstep_check.py same --sha256 "$LOCK_SHA256" --file "$RUNNER_TEMP/stage/rs/Cargo.lock"`.
7. The `env:` of the `stage` step is exactly `LOCK_SHA256: ${{ steps.lock.outputs.lock_sha256 }}`.
8. No other `env:` key, at any level, is named `LOCK_SHA256`.
9. `upload.with.path` is exactly `${{ runner.temp }}/stage/`.
10. No script and no `env:` key at any level sets `RUNNER_TEMP`.

Checks 5 and 6 only repeat the exact pin. They give a clearer message. Checks 4, 7, 9 and the `env:`
part of check 10 carry control on their own, because the pin of check 3 reads only the `run:` text.
Check 8 does not carry control on its own. Check 4 already forbids a `LOCK_SHA256` key at the
workflow and job level, and the `env:` of another step does not reach the `stage` step. So check 8
only keeps the name unique.

On the `lock` and `stage` steps, three more rules also repeat the pin. They are the `lock_sha256`
output line, the `exit` rule of `stage` and the `LOCK_SHA256` script rule. The script part of check 10
is the `RUNNER_TEMP` script rule, and it also repeats the pin. The `LOCK_SHA256` script rule and the
`RUNNER_TEMP` script rule also apply to the unpinned build steps. Self-test rows prove them on the
`update` step.

The P6 rule pins the action NAME and a full SHA, not one specific SHA, so a dependabot action bump
does not turn this gate red. A new action, or a tag in place of a SHA, does.

## The trust model

- **`build`** has no environment and reads no secret. Its `GITHUB_TOKEN` is `contents: read`. Every
  step that runs third-party code (cargo, build scripts, proc-macros, `wasm-pack`,
  `wasm-bindgen-cli`, binaryen's `wasm-opt` since SMA-435, pnpm packages) runs inside
  `docker run`, as the user `nobody`, over a copy of
  the tree without `.git`. The container gets no runner environment: no `CI`, no `GITHUB_ACTIONS`,
  no `ACTIONS_RUNTIME_TOKEN`, no `GITHUB_TOKEN`, no `GITHUB_OUTPUT`. So that code cannot write a
  cache entry in the `refs/heads/main` scope or an artifact (spec F11, AC3).
- After the compile, the host runs no file of the work copy and no `git` command on it. It checks
  each path component for a symlink, then copies the six files with `cp`.
- The host writes `changed` and `lock_sha256` BEFORE the compile, so third-party code cannot set
  them.
- **The run-1 lock compare (SMA-738).** On exit 0 the `lock` step writes `lock_sha256` as a STEP
  output, before container run 2. The value is the SHA-256 of the exact bytes that the verdict
  judged. A step output cannot change after its step ends, and no file holds it. The last command
  of the `stage` step is `lockstep_check.py same`. It refuses a staged `rs/Cargo.lock` with other
  bytes (`R-RUN2`), so `upload` and `propose` do not run.
  - An edge move or a family entry that reaches `propose` can then come only from `cargo update`
    in run 1. That run executes no build script.
  - This control is in `build` only. `propose` cannot check it again, because that needs a second
    job output (P9).
  - It depends on `pin_check.py` (P18, P25) and on the build runner host. P25 pins the whole
    `run:` text of the `lock` step and of the `stage` step. A person changes `pin_check.py`
    together with the workflow.
  - The workflow `env:` holds only `LOCKSTEP_IMAGE`, and the `build` job has no `env:`. So no
    `env:` key can change how the `stage` step runs `python3`. Two other ways are not covered.
    "What the checks do not prove" names them.
  - A container escape in run 2 can reach the runner and defeat the compare. This is the same
    residual as the cache scope (Q7).
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
- `cargo update` in container run 1 can re-point a non-family dependency edge to another version
  that is already in the lock. This can change which code a target compiles. No new package enters
  the lock. The reviewer reads the edge table of the PR body. `cargo-lock-integrity` in `CI`
  catches only an edge outside the range that a manifest allows (SMA-738).
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
- An earlier build step can write the runner env files indirectly. Two examples are `${!m}` on
  `GITHUB_ENV` and a write to `$RUNNER_TEMP/_runner_file_commands/*`. Such a write can set
  `PATH` or `PYTHONPATH` for the `stage` step. P19 does not see it. Only a workflow author can
  write it: run 2 cannot reach these files without a container escape. PR review is the control.
- A SHA-pinned `uses:` action before `stage` can export variables for the later steps. The
  checks trust each pinned action (P6).
- In the `propose` step `verify`, an `exit 0` or a `set -n` before the `lockstep_check.py artifact`
  command passes `pin_check.py`. The gap existed before SMA-738 and is outside G6. SMA-739
  tracks it.
- A bundled short flag, such as `gh api -iXPOST`, passes P20.
- P21 matches the jq `select` with a regular expression. It does not parse the jq.
- The build container downloads rustup and the Rust 1.95.0 toolchain at run time through proto.
  Nobody verified that proto checks the rustup download (M2 notes).
- `container.sh` fetches `proto.sh` with `curl` and no checksum. This happens inside the
  container, and `propose` checks the output again.
- The branch-owner check uses `.author.login`. GitHub takes that value from the author email. The
  check guards against a mistake. It does not stop an attacker with push access.
- `pin_check.py` does not detect the removal of the per-component symlink loop of the stage step.
  The `propose` re-check of the layout (`R-LAYOUT`, `R-SYMLINK`) catches a wrong layout, not the
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

Sven started the run on 2026-10-04 with `gh workflow run wasm-lockstep.yml --ref main`. The run used a GitHub-hosted `ubuntu-latest` runner (image `ubuntu24/20260927.320`). Both jobs passed. The bot opened its pull request.

| Item | Value |
|---|---|
| Run | https://github.com/SMK1085/paigasus-core/actions/runs/37203038886 (`workflow_dispatch`, head `c9df6f09`) |
| Conclusion | success |
| `build` job | success, 12:42:41Z to 12:46:45Z (4 min 4 s) |
| `propose` job | success, 12:46:49Z to 12:47:00Z (11 s), environment `release-pr` |
| Lock verdict on the host | rc 0, with seven `family-moved` lines: `js-sys 0.3.105 0.3.106`, `wasm-bindgen 0.2.128 0.2.129`, `wasm-bindgen-futures 0.4.78 0.4.79`, `wasm-bindgen-macro 0.2.128 0.2.129`, `wasm-bindgen-macro-support 0.2.128 0.2.129`, `wasm-bindgen-shared 0.2.128 0.2.129`, `web-sys 0.3.105 0.3.106` |
| Compile container | Moon `SetupToolchain(rust:1.95.0)` passed. Moon printed that 2.5.6 is available. The repo pins 2.5.3. |
| `generate-wasm` line | `generate-wasm: wrote 5 files into rs/crates/bindings/paigasus-wasm/` |
| `propose` steps | All passed: checkout of the commit that `build` used, artifact download, artifact and lock check (the same seven `family-moved` lines), apply the files, mint the App installation token, commit as the bot, check that `main` did not move (no notice), check the bot branch owner and push, open or update the pull request |
| Skipped step | "Close an obsolete pull request" was skipped, because `changed=true` |
| Branch-owner check | `gh api` for `refs/heads/deps/wasm-bindgen-lockstep` returned `Not Found (HTTP 404)`. The step accepts this as "no bot branch yet". |
| Push | `* [new branch] HEAD -> deps/wasm-bindgen-lockstep`. GitHub accepted the push from the App token. |
| Bot commit | `c0fcb3ce`, `build(deps): move wasm-bindgen to 0.2.129 and regenerate the wasm glue`, author `paigasusbot[bot]` |
| Files in the bot commit | 3 files: `rs/Cargo.lock` (+15 -14), `rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.js` (+1 -1), `rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm` (binary) |
| Why only 3 files | The other three glue files were byte-identical, so they are not in the diff. AC2 allows this. |
| Bot pull request | https://github.com/SMK1085/paigasus-core/pull/380, opened by `app/paigasusbot` |
| CI on the pull request | The CI and images workflows started (event `pull_request`). So the App token opened the pull request, not `GITHUB_TOKEN` (AC6). |

The review of the `rs/Cargo.lock` diff and the merge of the bot pull request are for Sven.

M3 creates the bot branch. So M3 does not answer Q11: whether a force-push across `main` commits that edit `.github/workflows/*` needs the `workflows` permission of the App. Only a later run can answer Q11, because only then does the bot branch exist and differ from `main` in those files. M3 confirms this: the push created a new branch, so Q11 is still open for M4.

### M4 — a second run with an open bot PR

Not measured yet. This run starts while the bot PR from M3 is open and `main` has moved. It shows whether the force-push, the branch-owner check and the PR update work on an existing bot branch (Q11).

### M5 — the run-1 lock compare on a runner (SMA-738)

This measurement checks the `stage` compare of SMA-738 on a real GitHub runner. A positive run shows that container run 2 does not change the lock. A negative run shows that the compare refuses a lock that run 2 changed. The positive run answers the open question of spec section 4.5.

The runs used the scratch branch `feature/sma-738-m5-scratch`. The branch was deleted after the runs. The scratch workflow `.github/workflows/wasm-lockstep-m5.yml` is a copy of the reviewed file. It has no `ref` step and no `propose` job. It has a `push` trigger on the scratch branch only, because GitHub dispatches `workflow_dispatch` only for a workflow file on the default branch. M0 also used `push`. It has its own concurrency group `wasm-lockstep-m5`. It has no environment, no secret and no App token. This is the recorded `diff` of the scratch workflow against the reviewed file:

```diff
--- .github/workflows/wasm-lockstep.yml	2026-10-05 21:50:57
+++ .github/workflows/wasm-lockstep-m5.yml	2026-10-05 22:22:28
@@ -1,5 +1,9 @@
 # SPDX-License-Identifier: Apache-2.0
 #
+# SMA-738 M5 SCRATCH copy of wasm-lockstep.yml, on feature/sma-738-m5-scratch ONLY. NEVER merge it.
+# Changes: no `ref` step, no `propose` job (no environment, no secret, no App token), a push
+# trigger on this branch only, and its own concurrency group. Each push to the branch starts one run.
+#
 # wasm-lockstep — propose the wasm-bindgen family bump as ONE pull request (SMA-693).
 #
 # js-sys, web-sys and wasm-bindgen-futures pin wasm-bindgen with `=`, so dependabot cannot move the
@@ -22,21 +26,18 @@
 # repo:wasm-lockstep (ci/wasm-lockstep/pin_check.py) asserts these rules on every PR that edits
 # this file. A refusal of the checker stays red until a person runs the manual runbook in
 # rs/CLAUDE.md (SMA-693 Q10).
-name: wasm-lockstep
+name: wasm-lockstep-m5
 
 on:
-  schedule:
-    # Tuesday 06:17 UTC (SMA-693 Q3): after the Monday 06:00 UTC dependabot run, and not on a round
-    # minute, where GitHub delays or drops scheduled runs. GitHub sends the failure mail of a
-    # scheduled run to the person who last changed this cron line (spec section 9).
-    - cron: '17 6 * * 2'
-  workflow_dispatch:
+  push:
+    branches:
+      - feature/sma-738-m5-scratch
 
 permissions:
   contents: read
 
 concurrency:
-  group: wasm-lockstep
+  group: wasm-lockstep-m5
   cancel-in-progress: false
 
 env:
@@ -70,18 +71,6 @@
           fetch-depth: 1
           persist-credentials: false
 
-      # A convenience check, not the boundary. The boundary is the main-only branch policy of the
-      # release-pr environment, which refuses the propose job on any other ref.
-      - name: Refuse a ref other than main
-        id: ref
-        env:
-          REF: ${{ github.ref }}
-        run: |
-          if [ "$REF" != "refs/heads/main" ]; then
-            echo "::error::wasm-lockstep runs on refs/heads/main only, not on $REF."
-            exit 1
-          fi
-
       # The container gets this copy and nothing else. It has no .git, and the host runs no git
       # command and no file from it after this step: a git call can run a hook or an fsmonitor
       # command from .git/config. The old lock comes from git show, not from the copy.
@@ -185,171 +174,3 @@
           path: ${{ runner.temp }}/stage/
           if-no-files-found: error
           retention-days: 7
-
-  propose:
-    name: check the artifact and propose the pull request
-    needs: build
-    if: needs.build.result == 'success'
-    runs-on: ubuntu-latest
-    timeout-minutes: 15
-    # The credential boundary, the same one release.yml uses (SMA-580): PAIGASUS_BOT_* are
-    # environment secrets on release-pr, whose deployment branch policy is main-only.
-    environment: release-pr
-    permissions:
-      contents: read
-    steps:
-      - name: Checkout the commit the build ran on
-        id: checkout
-        if: needs.build.outputs.changed == 'true'
-        uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1  # v7.0.1
-        with:
-          ref: ${{ github.sha }}
-          persist-credentials: false
-
-      - name: Download the artifact
-        id: download
-        if: needs.build.outputs.changed == 'true'
-        uses: actions/download-artifact@3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c  # v8.0.1
-        with:
-          name: wasm-lockstep
-          path: ${{ runner.temp }}/lockstep
-
-      # AC4.1 and AC4.2, from the checked-out github.sha (trusted), on the downloaded bytes. A
-      # refusal exits 3 and fails this step, so no later step runs and no token is minted.
-      - name: Check the artifact and the lock
-        id: verify
-        if: needs.build.outputs.changed == 'true'
-        run: |
-          set -euo pipefail
-          python3 ci/wasm-lockstep/lockstep_check.py artifact --dir "$RUNNER_TEMP/lockstep" --old rs/Cargo.lock --body-file "$RUNNER_TEMP/pr-body.md" --title-file "$RUNNER_TEMP/pr-title.txt"
-
-      - name: Apply the files to the checkout
-        id: apply
-        if: needs.build.outputs.changed == 'true'
-        run: |
-          set -euo pipefail
-          for f in rs/Cargo.lock rs/crates/bindings/paigasus-wasm/paigasus_wasm.js rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.js rs/crates/bindings/paigasus-wasm/paigasus_wasm.d.ts rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm.d.ts rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm; do
-            if test -f "$RUNNER_TEMP/lockstep/$f"; then
-              cp "$RUNNER_TEMP/lockstep/$f" "$f"
-            fi
-          done
-          git status --porcelain --untracked-files=all > "$RUNNER_TEMP/status.txt"
-          cat "$RUNNER_TEMP/status.txt"
-          python3 ci/wasm-lockstep/lockstep_check.py status --file "$RUNNER_TEMP/status.txt"
-
-      # Runs on both paths: changed == 'true' needs it to push, changed == 'false' to close an
-      # obsolete pull request. The two permissions are explicit, as in release.yml (zizmor
-      # github-app): without them the token carries every permission of the installation.
-      - name: Mint the App installation token
-        id: token
-        uses: actions/create-github-app-token@bcd2ba49218906704ab6c1aa796996da409d3eb1  # v3.2.0
-        with:
-          client-id: ${{ secrets.PAIGASUS_BOT_APP_ID }}
-          private-key: ${{ secrets.PAIGASUS_BOT_PRIVATE_KEY }}
-          permission-contents: write
-          permission-pull-requests: write
-
-      # The identity of release.yml: the App's bot user in the resolvable <id>+<login>@ form. The
-      # message is the checker's title only, with no body (the commitlint footer trap).
-      - name: Commit as the bot
-        id: commit
-        if: needs.build.outputs.changed == 'true'
-        run: |
-          set -euo pipefail
-          git config user.name "paigasusbot[bot]"
-          git config user.email "285361405+paigasusbot[bot]@users.noreply.github.com"
-          git add -- rs/Cargo.lock rs/crates/bindings/paigasus-wasm
-          git commit -F "$RUNNER_TEMP/pr-title.txt"
-
-      # AC4.3, immediately before the push. It branches on the exit status of gh api, never on an
-      # `|| echo` fallback: gh api prints a 404 body on stdout (.github/CLAUDE.md). A moved main is
-      # a GREEN notice (SMA-693 Q8): the next run proposes on the new base.
-      - name: Check that main did not move
-        id: base
-        if: needs.build.outputs.changed == 'true'
-        env:
-          GH_TOKEN: ${{ github.token }}
-          BUILT_ON: ${{ github.sha }}
-        run: |
-          set -euo pipefail
-          rc=0
-          gh api "repos/${GITHUB_REPOSITORY}/git/ref/heads/main" --jq .object.sha > "$RUNNER_TEMP/main-sha.txt" || rc=$?
-          if [ "$rc" -ne 0 ]; then
-            cat "$RUNNER_TEMP/main-sha.txt"
-            echo "::error::gh api could not read refs/heads/main (exit ${rc})."
-            exit 1
-          fi
-          now="$(cat "$RUNNER_TEMP/main-sha.txt")"
-          if [ "${#now}" -ne 40 ]; then
-            echo "::error::gh api returned '${now}', not a commit SHA."
-            exit 1
-          fi
-          if [ "$now" != "$BUILT_ON" ]; then
-            echo "moved=true" >> "$GITHUB_OUTPUT"
-            echo "::notice::main moved from ${BUILT_ON} to ${now}. Nothing was pushed; the next run proposes on the new base."
-          else
-            echo "moved=false" >> "$GITHUB_OUTPUT"
-          fi
-
-      # The branch-owner check, then ONE push with a lease on the SHA read here. The refspec is
-      # fixed text; repo:wasm-lockstep pins it. An absent branch leases on the empty value, so the
-      # push fails if the branch appears in between.
-      - name: Check the bot branch owner and push
-        id: push
-        if: needs.build.outputs.changed == 'true' && steps.base.outputs.moved == 'false'
-        env:
-          GH_TOKEN: ${{ steps.token.outputs.token }}
-          PUSH_TOKEN: ${{ steps.token.outputs.token }}
-        run: |
-          set -euo pipefail
-          rc=0
-          gh api "repos/${GITHUB_REPOSITORY}/git/ref/heads/deps/wasm-bindgen-lockstep" --jq .object.sha > "$RUNNER_TEMP/branch-sha.txt" || rc=$?
-          if [ "$rc" -eq 0 ]; then
-            lease="$(cat "$RUNNER_TEMP/branch-sha.txt")"
-            author="$(gh api "repos/${GITHUB_REPOSITORY}/commits/${lease}" --jq .author.login)"
-            # .author.login comes from the commit's author EMAIL, which a pusher can set. This check
-            # guards against a mistake (a person pushed to the bot branch), not against an attacker
-            # with push access.
-            if [ "$author" != "paigasusbot[bot]" ]; then
-              echo "::error::A person pushed to the bot branch (head ${lease} by '${author}'). Merge or close that pull request, or delete the branch deps/wasm-bindgen-lockstep, first."
-              exit 1
-            fi
-          elif grep -qF '"status":"404"' "$RUNNER_TEMP/branch-sha.txt"; then
-            lease=""
-          else
-            cat "$RUNNER_TEMP/branch-sha.txt"
-            echo "::error::gh api could not read the bot branch (exit ${rc})."
-            exit 1
-          fi
-          git push --force-with-lease="refs/heads/deps/wasm-bindgen-lockstep:${lease}" "https://x-access-token:${PUSH_TOKEN}@github.com/${GITHUB_REPOSITORY}.git" HEAD:refs/heads/deps/wasm-bindgen-lockstep
-
-      # The App token opens the pull request, so its CI runs (a GITHUB_TOKEN event starts no
-      # workflow). `--head` matches the branch NAME only, so the list keeps same-repository pull
-      # requests (isCrossRepository false): a fork pull request from a branch with this name must
-      # not be edited or closed (P21).
-      - name: Open or update the pull request
-        id: pr
-        if: needs.build.outputs.changed == 'true' && steps.base.outputs.moved == 'false'
-        env:
-          GH_TOKEN: ${{ steps.token.outputs.token }}
-        run: |
-          set -euo pipefail
-          gh pr list --repo "$GITHUB_REPOSITORY" --head deps/wasm-bindgen-lockstep --state open --json number,isCrossRepository --jq '.[] | select(.isCrossRepository | not) | .number' > "$RUNNER_TEMP/open-prs.txt"
-          number="$(sed -n 1p "$RUNNER_TEMP/open-prs.txt")"
-          if [ -z "$number" ]; then
-            gh pr create --repo "$GITHUB_REPOSITORY" --base main --head deps/wasm-bindgen-lockstep --title "$(cat "$RUNNER_TEMP/pr-title.txt")" --body-file "$RUNNER_TEMP/pr-body.md"
-          else
-            gh pr edit "$number" --repo "$GITHUB_REPOSITORY" --title "$(cat "$RUNNER_TEMP/pr-title.txt")" --body-file "$RUNNER_TEMP/pr-body.md"
-          fi
-
-      - name: Close an obsolete pull request
-        id: close
-        if: needs.build.outputs.changed == 'false'
-        env:
-          GH_TOKEN: ${{ steps.token.outputs.token }}
-        run: |
-          set -euo pipefail
-          gh pr list --repo "$GITHUB_REPOSITORY" --head deps/wasm-bindgen-lockstep --state open --json number,isCrossRepository --jq '.[] | select(.isCrossRepository | not) | .number' > "$RUNNER_TEMP/open-prs.txt"
-          while read -r number; do
-            gh pr close "$number" --repo "$GITHUB_REPOSITORY" --comment "The wasm-bindgen family is current on main. This proposal is obsolete."
-          done < "$RUNNER_TEMP/open-prs.txt"
```

The scratch lock holds the seven family entries of `c9df6f09` (0.2.128) in the lock of `main`. A local pre-check used cargo 1.95.0. The four-package `cargo update -p` locked 7 packages. It gave a lock that is byte-identical to the lock of `main` (sha256 `08afa11169edf7ad8f9176baddaf9029e702bb69fc9bf795a9b9991183f0bcfb`). It flipped no dependency edge. The five edge flips of SMA-738 appear only on the no-op update of a current family.

| Item | Value |
|---|---|
| Date | 2026-10-05 |
| Positive run | https://github.com/SMK1085/paigasus-core/actions/runs/37369513364 (head `f3cdd188`) |
| Attempts 1 to 3 | Failed with "The job was not acquired by Runner of type hosted even after multiple attempts" during a GitHub incident. No step ran. |
| Attempt 4 | Success. All steps passed. |
| `lock` step output | Seven `family-moved` lines (js-sys and web-sys 0.3.105 to 0.3.106; wasm-bindgen, wasm-bindgen-macro, wasm-bindgen-macro-support and wasm-bindgen-shared 0.2.128 to 0.2.129; wasm-bindgen-futures 0.4.78 to 0.4.79), zero `edge-moved` lines, and `lock-sha256 08afa11169edf7ad8f9176baddaf9029e702bb69fc9bf795a9b9991183f0bcfb` |
| `stage` step output | `same: rs/Cargo.lock sha256 08afa11169edf7ad8f9176baddaf9029e702bb69fc9bf795a9b9991183f0bcfb did not change after the lock verdict` |
| Result | Container run 2 did not write the lock. The `upload` step ran. |
| Negative run | https://github.com/SMK1085/paigasus-core/actions/runs/37375594364 (head `ba9a3da2`) |
| Negative change | One scratch-only line at the end of the `build` case of `container.sh`: `printf '\n' >> rs/Cargo.lock` |
| Negative result | The step "Stage the six files" failed. The step "Upload the artifact" was skipped. The run conclusion was failure. |
| Negative log | `lockstep_check: REFUSED R-RUN2: rs/Cargo.lock changed after the lock verdict: the lock step judged sha256 08afa11169edf7ad8f9176baddaf9029e702bb69fc9bf795a9b9991183f0bcfb, /home/runner/work/_temp/stage/rs/Cargo.lock has sha256 863dd98a6018757d6a60a9aa8337254dca0e7ac1d876ac9005bd53b035d9ea3b (157605 bytes). …` |

The negative run proves the refusal on the real mount, with the real owner and the real paths.
