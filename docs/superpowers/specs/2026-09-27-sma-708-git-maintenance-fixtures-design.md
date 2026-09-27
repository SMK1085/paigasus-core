# SMA-708: Stop background git maintenance in the CI fixture repositories

- **Linear:** [SMA-708](https://linear.app/smaschek/issue/SMA-708)
- **Branch:** `feature/sma-708-git-maintenance-fixtures`
- **Status:** design approved in chat on 2026-09-27. Revised after the Stage 2 challenge (§11).

## 1. Problem

The `CI` run on `main` at `1dd4d090` failed in `repo:actionlint`
([run 36273324959](https://github.com/SMK1085/paigasus-core/actions/runs/36273324959)).
Check 9 runs an unmutated control, `bash ci/actionlint/run.sh --self-test`
(`ci/actionlint/run.sh:5459`), at the same time as 15 mutants (`:5451-5461`). Each of the 16
copies runs `release_plan.py --self-test` four times: once in check 11, and once in each of the
release-plan negative-control rows 2, 7 and 8. One row of the control failed:

```text
check 9 [control]: FAIL 'SMA-688 a console package.json with a non-string version is
inconclusive for that key only': raised FileNotFoundError: [Errno 2] No such file or
directory: 'maintenance.lock'
```

The failure is intermittent. The PR that made the commit did not change `ci/release-plan/`,
and the same task passed on both PR runs.

## 2. Mechanism

1. The row builds a throwaway repository with `_complete_chain_tree`, which calls
   `_git_commit_and_tag`. That helper runs `git init` and `git commit`.
2. After a commit, git starts `git maintenance run --auto --quiet --detach` as a detached
   background process. That process creates and then deletes `.git/objects/maintenance.lock`.
3. The row's `finally: shutil.rmtree(tmp)` lists a directory, then deletes each entry by its
   bare name. On Python 3.12, if the background process deletes the lock between the two
   steps, `rmtree` raises `FileNotFoundError` with the bare name `maintenance.lock`.

## 3. Measurements (2026-09-27)

All scripts are in the session scratchpad. None of them is part of the change.

| ID | What | Result |
|---|---|---|
| M1 | `GIT_TRACE=1 git commit` in a new repository, macOS git 2.54.0 | git starts `git maintenance run --auto --quiet --detach` |
| M2 | The same, after `git config maintenance.auto false` only | git starts no maintenance or gc process |
| M3 | Poll the fixture tree for 500 ms after `git commit` returns, Ubuntu 24.04, git 2.55.0 (git-core PPA), Python 3.12.3 | `.git/objects/maintenance.lock` present after 1 of 20 commits |
| M4 | M3 with `maintenance.auto false` and `gc.auto 0` | present after 0 of 20 commits |
| M5 | Count of `FileNotFoundError` in the `shutil` source | Python 3.12.14: 1. Python 3.13.15: 8. Python 3.14.7: 8 |
| M6 | init + commit + immediate `rmtree`, one loop, no load: Ubuntu 24.04 git 2.55.0 Python 3.12.3 (1,000 runs); Debian git 2.47.3 Python 3.12.14 (1,000 runs, with and without the setting); macOS Python 3.14.7 (300 runs, with and without) | 0 failures in every configuration |
| M7 | M6 on Ubuntu 24.04, git 2.55.0, Python 3.12.3, as **16 parallel loops of 150 runs** (2,400 runs) | **7 failures**, each `FileNotFoundError: [Errno 2] No such file or directory: 'maintenance.lock'` |
| M8 | M7 with `maintenance.auto false` and `gc.auto 0` | 0 failures in 2,400 runs |

**What the measurements show.** M7 reproduces the CI error text exactly, under a load like the
check 9 load. M8 shows that the setting removes it. M1 to M4 show the background process, the
lock inside the fixture tree, and the effect of the setting. M5 shows why Python 3.12 is exposed.
M6 shows that the race needs load to occur at a measurable rate. The mechanism in §2 is therefore
**measured**.

CI runs Python 3.12: `.moon/toolchains.yml:33-34` pins `unstable_python` 3.12.13, and the
release-plan gate runs under `uv run --locked --project ci/release-plan --python '>=3.12'`.

A second failure form is possible from the same race: `rmtree` lists `.git/objects` before the
lock exists, and the `rmdir` of `.git/objects` then fails with `OSError: Directory not empty`.
The fix removes this form too.

## 4. Decision: persisted configuration in each fixture repository

After each `git init` of a fixture repository in scope (§5.1), run:

```text
git -C <dir> config maintenance.auto false
git -C <dir> config gc.auto 0
```

- `maintenance.auto false` stops `git maintenance run --auto`. M2 shows that this key alone
  stops every background maintenance and gc process in the measured git versions. It is the
  fix.
- `gc.auto 0` is a second layer of defence only. It stops `git gc --auto` if a git version or a
  command runs gc directly. In the measured versions it has no effect on this race: a fixture
  holds far fewer loose objects than the `gc.auto` default of 6700. The code comment says this.

**Why this form.** The keys are in the fixture's own `.git/config`, so every later git process in
that repository obeys them. This includes the git calls that the external release tools make in
the fixture, and `receive-pack` in a bare repository that receives a push. The shell sites
already persist `user.email`, `user.name` and the signing keys this way. In C1 (§5.1), the
signing keys are per-call `-c` flags, and only the identity is persisted. C1 persists the new keys
anyway, because the new self-test row must read them back from the fixture (§6.1).

**Rejected alternatives.**

- `-c maintenance.auto=false` on each `git commit` call. It does not cover the git calls that the
  external tools make, or a push into a bare repository. A self-test cannot read it back.
- `GIT_CONFIG_COUNT` / `GIT_CONFIG_KEY_n` environment variables at the top of each script. In
  `release_plan.py` they would also change the git calls on the real repository. Git removes
  these variables for the transport child of a local push, so they would also miss C6.
- `shutil.rmtree(..., ignore_errors=True)` or a retry around `rmtree`. It hides real cleanup
  faults, and the issue forbids it.
- A shared `ci/lib/` helper. SMA-596 D4 rejected a shared `ci/lib/` layer. Each gate stays
  self-contained, and the two lines are repeated at each site.
- A CI workflow step `git config --global maintenance.auto false`. It does not cover local runs,
  and it changes workflow files that other gates check. The per-site keys are sufficient (M8).

## 5. Scope: the fixture inventory

A search of `ci/` for `git init`, `git clone` and `git worktree add` finds 13 fixture sites and no
`clone` or `worktree add`. No `git init` fixture exists outside `ci/`. The commands that start
automatic maintenance are `git commit`, `git merge`, `git fetch`, `git pull`, `git am`,
`git rebase`, `git cherry-pick`, and `receive-pack` for a push into the repository. `git init`,
`git add`, `git tag`, `git config`, `git ls-files`, `git rm --cached` and `git diff` do not.

### 5.1 Sites that change

| # | Site | What starts maintenance |
|---|---|---|
| C1 | `ci/release-plan/release_plan.py` `_git_commit_and_tag` (init at line 1021) | `git commit` |
| C2 | `ci/release-plan/run.sh` `_build_synthetic_tree` (init at line 236) | `git commit` |
| C3 | `ci/release-parity/ecosystems/release-plz.sh` `ecosystem::build_fixture` (init at line 120) | `git commit` in `build_fixture` and `apply_commit`; git calls by `release-plz` |
| C4 | `ci/release-parity/ecosystems/python-semantic-release.sh` `ecosystem::build_fixture` (init at line 163, once per slot) | `git commit` in `build_fixture` and `apply_commit`; git calls by PSR |
| C5 | `ci/release-parity/ecosystems/semantic-release.sh` `ecosystem::build_fixture`, working repository (init at line 86) | `git commit`; `git fetch` by semantic-release |
| C6 | the same function, bare `origin.git` (init at line 126) | `receive-pack` for `git push` |

C5 and C6 are one module but two repositories. Each one gets its own two lines. For C6, the
config must be set between `git init --bare` and `git push`.

### 5.2 Sites that do not change (no command that starts maintenance)

Sven chose this scope on 2026-09-27: committing sites only, with the reason recorded for the rest.

| Site | Why no setting is needed |
|---|---|
| `ci/release-plan/release_plan.py:1247` (`_changelog_undecodable_asserts_three`) | Runs `git init`, then `_assert_repo`, which runs only `git tag -l`. |
| `ci/next-public/run.sh:233` (`make_fixture`), `:293`, `:470` | `git init`, `git add -A`, `git rm --cached` and `git ls-files` only. |
| `ci/ruff/run.sh:136`, `:176`, `:225` | `git init`, `git add -A` and `git ls-files` only. |

The PR description repeats this table, as the acceptance criteria require.

## 6. Components

### 6.1 `release_plan.py`

- `_git_commit_and_tag` runs the two `git -C tmp config` calls directly after `git init`, before
  `git add` and `git commit`. A comment says why, names SMA-708, and says that `gc.auto 0` is
  a second layer only (§4).
- `_git_commit_and_tag` and `_complete_chain_tree` get an optional keyword argument
  `git_env: dict[str, str] | None = None`. When it is set, every `subprocess.run` of git in
  `_git_commit_and_tag` gets `env=git_env`. All existing callers pass nothing, so their
  behaviour does not change. The row does not change `os.environ`, because the rows share one
  process.
- A new `COLLECTION_ROWS` row, `"SMA-708 a committed fixture repository starts no background
  git maintenance"`, with the helper `_fixture_repo_starts_no_maintenance`:
  1. `tmp = tempfile.mkdtemp()`. The trace file is `<tmp>.trace2.json`, outside the tree that
     git writes, and it is removed in the `finally` block.
  2. `git_env` is a copy of `os.environ` plus `GIT_TRACE2_EVENT=<trace file>`,
     `GIT_CONFIG_GLOBAL=/dev/null` and `GIT_CONFIG_NOSYSTEM=1`. The two config variables stop a
     host setting (for example a global `maintenance.auto false`) from making the row pass on a
     tree without the fix.
  3. Call `_complete_chain_tree(tmp, git_env=git_env)`.
  4. **Behaviour assertion.** Parse the trace file as one JSON object per line. Return an error
     if any `child_start` event has an `argv` that contains `maintenance` or `gc`.
  5. **Non-vacuous check.** Return an error unless the trace holds a `start` event whose `argv`
     contains `commit`. Without this check, an empty or missing trace file would pass step 4.
  6. **Key assertion.** Read `git -C tmp config --local --get maintenance.auto`. Return an error
     unless the value is exactly `false`. `--local` reads only the fixture's own `.git/config`.
     The row does not assert `gc.auto`, because that key is a second layer with no effect in the
     measured versions (§4).
  7. `finally`: `shutil.rmtree(tmp)` and remove the trace file.
- Each error string names what the row read and what it wants.
- The comment above `COLLECTION_ROWS` ("Fourteen of the fifteen … row 15") is stale today. The
  same edit corrects it, so that it does not state a row count.

**Mutation proof.** Run each mutation with
`uv run --locked --project ci/release-plan --python '>=3.12' python3 ci/release-plan/release_plan.py --self-test`.
Each mutation must give rc 3, and stderr must contain the SMA-708 row label. Note that
`ci/release-plan/run.sh --self-test` maps rc 3 to 1 (`run.sh:58-65`), so use the direct command.

1. Delete the two config lines in `_git_commit_and_tag`. Both the behaviour and the key assertion
   must fail.
2. Move the two config lines after the `git commit` call. The behaviour assertion must fail,
   although the key assertion passes. This is the reason the row has a behaviour assertion.
3. Make the row ignore `git_env` (drop `env=git_env` in `_git_commit_and_tag`). The non-vacuous
   check must fail.

Restore each mutation by an edit that removes it, not by `git checkout --`, which would also
revert the fix.

### 6.2 `ci/release-plan/README.md`

`ci/release-plan/README.md:96` says "thirty-seven collection-layer rows". Change it to
thirty-eight, and add one sentence about the SMA-708 row.

### 6.3 `ci/release-plan/run.sh`

`_build_synthetic_tree` gets the two lines after its existing `git -C "$dir" config` block.

### 6.4 The three `release-parity` ecosystem modules

Each `ecosystem::build_fixture` gets the two lines after its existing `git config` block, in the
same form as that block. For `semantic-release.sh`, the working repository gets the two lines in
the first subshell chain (line 86-88). The bare repository gets `git -C "$dir/origin.git" config`
for both keys, between `git init --bare` and `git remote add`, inside the same `&&` chain.

The two new lines in `release-plz.sh` move the `release-plz update` line from 152 to 154. Four
documents cite `release-plz.sh:152`: `ci/affected-graph/cargo_moon_parity.py:249` and `:270`,
`ci/affected-graph/README.md:163`, and `rs/CLAUDE.md:104`. Update all four to the new line.
The waiver in `cargo_moon_parity.py` is keyed by text, not by line, so no gate depends on the
number.

### 6.5 `ci/CLAUDE.md`

One new rule, which agrees with §5: a fixture repository under `ci/` in which any process runs
`git commit`, `merge`, `fetch`, `pull`, `am`, `rebase` or `cherry-pick`, or which receives a
push, sets `maintenance.auto false` and `gc.auto 0` directly after `git init`. This includes git
calls that an external tool makes in the fixture. If you add such a command to a fixture that
has only `init` and `add`, add the two keys in the same edit. The rule names SMA-708 and points to
this spec.

## 7. Error handling

- A failed `git config` call in `release_plan.py` raises `CalledProcessError` (`check=True`). The
  `self_test()` wrapper reports it as a `FAIL` row. It does not exit the interpreter at 1.
- `ci/release-parity/run.sh:54` calls `build_fixture` on the left of `||`, so `set -e` is off
  for the whole function, and only the status of the last command reaches `check_case`. This is
  true today, and the change does not alter it. A failed new config line in C3, C4 or C5 is not
  reported by itself. C6 is inside the last `&&` chain of the function, so a failure there stops
  the push and reaches `check_case`. No code change is made to the error handling.
- In C2 the new lines follow the error handling of the existing `git config` lines next to them.

## 8. Testing and verification

1. `uv run --locked --project ci/release-plan --python '>=3.12' python3 ci/release-plan/release_plan.py --self-test`
   exits 0.
2. The three mutations in §6.1 each give rc 3 with the SMA-708 label in stderr. After the
   restore, step 1 passes again.
3. `ci/release-plan/run.sh --negative-control` passes.
4. **Trace check of the shell sites.** Export `GIT_TRACE2_EVENT` to a scratch file for one run
   of `ci/release-plan/run.sh --negative-control` and one run of each `release-parity` gate. The
   trace variable reaches the git children of the external tools and `receive-pack`. Assert zero
   `child_start` events with `maintenance` or `gc` in `argv`, and at least one `commit` start
   event per run. Run the same check once on a tree without the shell-site edits, and show that it
   finds maintenance events. This also answers whether `release-plz` works in the fixture or in a
   clone that it makes.
5. Repeat M7 against the helper as changed (16 parallel loops in the Ubuntu 24.04 container):
   0 failures.
6. The full gate graph: the marker-delimited `moon ci` command in the root `CLAUDE.md`, with
   `--base origin/main`. It includes `repo:actionlint`, `repo:ruff-ci`, `repo:affected-smoke`
   and the three `repo:release-parity*` gates. Per the root `CLAUDE.md`, gates that need a
   different local bash are re-run directly with the right bash, and `repo:actionlint` gives a
   local verdict only when its pipe preflight passes. If it does not pass, run the gate in a Linux
   container or rely on CI, and say which.

## 9. Residual risk

- **No gate enforces the rule for a new fixture.** A future script can add a committing fixture
  without the two lines. The `ci/CLAUDE.md` rule is the only control. A grep gate over `ci/` was
  considered and rejected: it cannot tell a committing fixture from an add-only one without a
  parse of each script, and the failure it prevents is a rare flake, not a wrong verdict.
- **The shell sites have no standing assertion.** Only `release_plan.py` gets a row, as the issue
  asks. Step 4 of §8 checks the shell sites once, at implementation time.
- **The collection floors do not protect the new row.** Both floors are 14
  (`release_plan.py:1718`, `ci/actionlint/run.sh:4650`), and the table will hold 38 rows. A
  deletion of the new row reds nothing. This is true today for 23 other rows. It is out of scope
  here; a follow-up issue records it.

## 10. Acceptance criteria (from SMA-708)

- The fixture repositories in `release_plan.py` start no background git maintenance. → §6.1.
- `release_plan.py --self-test` and `repo:actionlint` pass. → §8 steps 1 and 6.
- Every `git init` fixture under `ci/` gets the setting, or the PR records why it does not need
  it. → §5.

## 11. Stage 2 challenge record

Verdict: APPROVE WITH CHANGES. No blocker.

**Folded in:**

- MAJOR: §8 step 7 (0 locks in 20) could pass on unfixed code. Replaced by the trace checks in
  §6.1 and §8 step 4, which each have a failing control.
- MAJOR: the row proved that the keys exist, not that the commit obeys them. Added the trace
  behaviour assertion, the non-vacuous check, and mutation 2 (keys after the commit).
- MINOR: no measurement for the external tools. Added §8 step 4.
- MINOR: M6 had no load. Added M7 and M8. The race reproduces under load, and the fix removes it.
- MINOR: README row count. Added §6.2 and the stale comment fix.
- MINOR: four citations of `release-plz.sh:152`. Added to §6.4.
- MINOR: §4 was wrong about the C1 idiom. Corrected.
- MINOR: §7 described the shell error path wrongly. Rewritten.
- MINOR: wrong exit code in the mutation proof, and rc alone is weak. §6.1 now names the direct
  command and requires the label in stderr.
- MINOR: `gc.auto 0` has no effect on this race. §4 now says it is a second layer only, and the
  row asserts only `maintenance.auto`.
- MINOR: the `ci/CLAUDE.md` rule named `tag` and missed commands. Rewritten to agree with §5.
- MINOR: `repo:ruff-ci` and `repo:affected-smoke` were missing from §8. §8 step 6 now runs the
  full gate graph.

**Not folded in:**

- MINOR, part: "set the keys at all 13 sites". Sven chose committing sites only. The rule in §6.5
  now tells an author what to do when a fixture gains a commit.
- MINOR: "raise both collection floors to 37". It changes `repo:actionlint` check 11 and concerns
  23 existing rows. Recorded in §9 as a follow-up.
- QUESTION: `core.fsmonitor`. It is not set in the global or system git config on the development
  Mac, and M8 shows no residual failure. No change.
- QUESTION: a global CI config step. Rejected in §4.
- QUESTION: does `release-plz` clone the fixture? §8 step 4 answers it at implementation time.
