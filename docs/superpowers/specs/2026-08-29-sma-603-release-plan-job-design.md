<!-- SPDX-License-Identifier: Apache-2.0 -->

# SMA-603 — a `plan` job for `release.yml`

**Status:** design, revision 2 (revision 1's approach was disproven by measurement M6 below)
**Issue:** [SMA-603](https://linear.app/smaschek/issue/SMA-603/release-every-push-to-main-now-builds-the-full-matrix-and-waits-at-the)
**Related:** SMA-579 (the gated release job), SMA-580 (the first live release), SMA-602 (a
version-override input for re-dispatch recovery)

## 1. The problem

`vars.PAIGASUS_RELEASE_ENABLED` is now `'true'`. With the flag on, **every** push to `main`:

1. builds the full 12-leg wheel and napi matrix, about 15 minutes of runner time, then
2. stops at the `approve-release` environment and waits for a human, without a time limit.

Approving achieves nothing when there is no release. `release-plz release` finds the tags
already present, returns an empty `releases` array, and both publish jobs skip. A human must
cancel the run by hand. Observed on run 33265567805, the merge of PR 187.

The interim mitigation is to keep the flag `false` between releases and flip it on only for a
release. Nothing enforces that discipline.

## 2. Measurements

All taken 2026-08-29 against release-plz 0.3.158, the proto-pinned version, with
`CARGO_REGISTRY_TOKEN` unset. `--dry-run` is source-verified never to reach
`create_git_tag_and_release`, so nothing was published and no tag was cut. M3 and M6 ran in
isolated clones under the scratchpad, never in the worktree.

### 2.1 The dry-run

**M1 — the dry-run needs a *valid* GitHub token, not merely a present one.** With `GIT_TOKEN`
set to an invalid value, on `main` at `a73d13c`:

```
ERROR Response body: {"message": "Bad credentials", "status": "401"}
Caused by: HTTP status client error (401 Unauthorized) for url
  (https://api.github.com/repos/SMK1085/paigasus-core/commits/a73d13c…/pulls)
EXIT=1
```

This extends what CLAUDE.md records. The entry says `get_git_client()` runs unconditionally. It
does more than construct a client: it makes a live, authenticated
`GET /repos/{owner}/{repo}/commits/{sha}/pulls`.

**M2 — nothing to release: exit 0, empty array, about 0.5 s.** Same tree, valid token:

```
stderr: INFO paigasus-kernel 0.1.0: Already published - Tag paigasus-kernel-v0.1.0 already exists
stderr: INFO paigasus-proto-derive 0.1.0: Already published - Tag paigasus-proto-derive-v0.1.0 already exists
stderr: INFO paigasus-proto 0.1.0: Already published - Tag paigasus-proto-v0.1.0 already exists
stdout: {"releases":[]}
EXIT=0
```

It short-circuits on **tag existence** and never invokes `cargo publish --dry-run`.

**M3 — a `proto`-group release: exit 1, about 6 s.** Isolated clone, all three version-group
packages bumped to 0.1.1 and committed (`cargo publish` refuses a dirty tree, M3's own first
finding):

```
ERROR failed to publish paigasus-proto: failed to prepare local package for uploading
  Caused by: failed to select a version for the requirement `paigasus-proto-derive = "^0.1.1"`
    candidate versions found which didn't match: 0.1.0
EXIT=1     (stdout empty — the JSON line never appears)
```

**M4 — the dry-run requires HEAD to be on a branch with an upstream.** A detached checkout dies
with `cannot determine current branch … fatal: HEAD does not point to a branch`. Recorded
because it will bite anyone reproducing M2/M3/M6 by checking out a SHA. It does not affect CI:
`actions/checkout` creates a local branch tracking `origin/<branch>`.

**M5 — stdout carries only the JSON.** Streams captured separately: stdout is exactly the
16-byte line `{"releases":[]}`; every `INFO` line goes to stderr. Revision 1 asserted this
without evidence.

**M6 — THE DECISIVE ONE. The dry-run reports an empty `releases` array even when it would
publish.** Isolated clone with **only** the `kernel` group bumped to 0.1.1 — `paigasus-kernel`
is a publish group of one with no in-tree dependency, so M3's derive blocker cannot apply:

```
stderr: INFO paigasus-kernel 0.1.1: due to dry, skipping the following:
        ["cargo registry upload", "creation of tag 'paigasus-kernel-v0.1.1'", "creation of git release"]
stderr: INFO paigasus-proto-derive 0.1.0: Already published - Tag …-v0.1.0 already exists
stderr: INFO paigasus-proto      0.1.0: Already published - Tag …-v0.1.0 already exists
stdout: {"releases":[]}
EXIT=0, 3 s
```

release-plz states it **would publish `paigasus-kernel` and cut `paigasus-kernel-v0.1.1`**, and
reports `{"releases":[]}` at exit 0. In dry mode the `releases` array is never populated: it
records *performed* releases, and a dry run performs none.

### 2.2 What M6 kills

Revision 1 read the dry-run as a three-way signal and skipped on `exit 0 AND empty releases`.
M6 shows that conjunction is **true in both** the "nothing to release" state (M2) and the
"a kernel-group release is pending" state (M6). Revision 1 argued a false skip would require
release-plz to "succeed and lie". It does not have to: the field it was being asked to
interpret is simply not populated in dry mode.

Approach A would therefore have silently, greenly and permanently skipped every kernel-group
release — the exact catastrophic outcome the design exists to prevent. It is rejected, and §7
records it so nobody reintroduces it.

Two corrections to revision 1 follow from the same reading:

- **`rs/release-plz.toml` has two version groups, `kernel` and `proto`.** So "the derive blocker
  is permanent" is true only of the `proto` group. A `kernel`-only release never touches the
  derive crate.
- SMA-579's header conclusion — that a dry-run-based `plan` job is not viable — **stands**, for
  a better reason than the one it gives. The blocker is not only the derive resolution. It is
  that the dry-run's structured output cannot distinguish the two states at all.

### 2.3 The signal that does work

M2 and M6 show release-plz deciding on **tag existence**, before any registry or cargo work, and
saying so: `Already published - Tag <pkg>-v<version> already exists` versus `due to dry,
skipping … creation of tag '<pkg>-v<version>'`. The predicate the `plan` job needs is exactly
that, and it is answerable from local state:

> For every releasable package, does the tag `<package>-v<version>` already exist?

CLAUDE.md records the matching measurement from the first live release: release-plz **only tags
what it publishes** — three tags, not six. The repository carries exactly
`paigasus-kernel-v0.1.0`, `paigasus-proto-v0.1.0`, `paigasus-proto-derive-v0.1.0`, confirmed by
`git tag -l`.

This needs no token, no network, no `get_git_client()`, and no `cargo`. It is a pure function of
`rs/release-plz.toml`, the crate manifests and the tag list — so it can be unit-tested with a
fixture table, which is how every other control in this repo is built and is what would have
caught revision 1's defects before they reached a spec.

Its cost is that it restates release-plz's releasable set in a second place. §3.2 answers that
by **deriving** the set from `rs/release-plz.toml` rather than hard-coding it, and asserting the
derivation in CI.

## 3. Design

### 3.1 The job graph

```
plan ──> { wheels, prebuild, proto-dist } ──> approve-release ──> release ──> { publish-pypi, publish-npm }
```

`plan` becomes the single holder of the literal flag gate. `wheels`, `prebuild` and `proto-dist`
drop their own literal gate and take `needs: [plan]` plus the fail-safe condition. `release-pr`
is untouched: it stays push-only, on the `release-pr` environment, and in `UNGATED_JOBS`.

**The flag still reaches every job.** Flag off → `plan` skips → a job whose `needs:` dependency
skipped is itself skipped → everything below skips. `release_guard.py`'s `is_gated`
(`release_guard.py:185`) accepts this: the three build jobs are no longer gated directly, but
every `needs:` entry resolves to `plan`, which carries the literal `GATE_EXPR`.

The job in full — revision 1 omitted the `outputs:` block, without which the whole design is
inert, and omitted `runs-on`/`timeout-minutes`:

```yaml
  plan:
    name: decide whether anything is releasable
    if: vars.PAIGASUS_RELEASE_ENABLED == 'true'
    runs-on: ubuntu-latest
    timeout-minutes: 10
    outputs:
      # A STEP output is not a JOB output. Without this mapping every
      # `needs.plan.outputs.nothing_to_release` below is the empty string. Guard V9 asserts
      # both this key and that `steps.decide` names a step that exists in this job.
      nothing_to_release: ${{ steps.decide.outputs.nothing_to_release }}
    steps:
      - name: Checkout
        uses: actions/checkout@…  # v7.0.1, pinned as everywhere else
        with:
          # LOAD-BEARING. The tags ARE the signal. A shallow checkout has no tags, which
          # decide.sh detects and reports as inconclusive rather than as "nothing to release".
          fetch-depth: 0
          persist-credentials: false
      - name: Decide
        id: decide
        env:
          GITHUB_EVENT_NAME: ${{ github.event_name }}
        run: ci/release-plan/run.sh --github-output
```

No credential, no `environment:`, no network. This also removes revision 1's contradiction with
the file header's claim that every credential in this workflow is an environment secret.

### 3.2 `ci/release-plan/` — the decision, as a tested script

Layout mirrors `ci/workflow-credentials/` (SMA-593): a **dedicated zero-dependency uv project**,
not `uv run --project py`, which would compile a PyO3 cdylib on every run.

```
ci/release-plan/
  release_plan.py     the decision + its fixture table
  run.sh              mode dispatch, exit-code mapping, $GITHUB_OUTPUT
  pyproject.toml      requires-python >=3.12, dependencies = []   (tomllib is stdlib)
  uv.lock
  README.md           limitations, in the style of ci/actionlint/README.md
```

**The decision, as a pure function** of `(event_name, release_plz_toml, crate_manifests, tags)`:

1. If `event_name != "push"` → `false`. A `workflow_dispatch` always builds (§3.4).
2. Derive the releasable set from `rs/release-plz.toml`: every `[[package]]` with `release` not
   `false` **and** `publish` not `false`. That is the set release-plz tags, per CLAUDE.md's
   measured "it only tags what it publishes".
3. Resolve each name to its manifest by walking `rs/crates/**/Cargo.toml` and reading
   `[package] name`, then read that manifest's literal `[package] version`. A `version.workspace
   = true`, a missing manifest, or a duplicate name → **inconclusive**.
4. Floor: if the repository reports **zero** tags at all, → **inconclusive**. This floor is
   **redundant for safety** and is kept for legibility: with no tags every wanted tag is absent,
   so step 6 already builds. What it adds is a log line naming the misconfiguration, instead of a
   list of "not yet cut" tags that were never really looked for. A shallow checkout therefore
   costs runner time, never a missed release.
5. Floor: if `rs/release-plz.toml` sets `git_tag_name` anywhere, → **inconclusive**. The
   `<package>-v<version>` format is release-plz's default and step 6 assumes it.
6. `true` if and only if every releasable package's `<name>-v<version>` tag exists. Otherwise
   `false`.

Every inconclusive outcome yields `false`, which builds.

**Modes**, matching the repo's gate idiom:

- default — print `nothing_to_release=true|false`; `run.sh --github-output` appends it to
  `$GITHUB_OUTPUT`.
- `--self-test` — the fixture table, in-process.
- `--negative-control` — assert the checker reports `false` on a fixture that must not skip.
- `--assert` — the CI-side assertions of §3.5.

**`run.sh`'s exit-code contract, and why it is not the usual one.** `workflow_credentials.py`
exits 3 for an assertion failure and `run.sh` maps 3 → 1 and everything else → 2, so a `uv`
resolution failure cannot read as a real violation. Here the **runtime** path inverts that
deliberately: on any non-zero status from the checker, `run.sh --github-output` writes
`nothing_to_release=false`, prints a `::warning::` naming the status, and **exits 0**. A broken
decision must not fail the `plan` job, because a failed `plan` job skips its dependents (§3.4) —
it must build. The `--self-test`, `--negative-control` and `--assert` modes keep the normal
contract and exit non-zero, and CI runs those (§3.5).

**The producer polarity is covered by the fixture table**, not by a pinned shell line. Revision
1 put the decision in an inline `run:` block, where swapping two `echo` lines inverted the
result with no check anywhere. Moving it into a fixture-tested function is what closes that,
and it is why this is a script rather than a workflow step.

### 3.3 Guard: V8 and V9

**V8 — the approval boundary, asserted in both directions.** Implemented with the existing
`job_publishes()` (`release_guard.py:236`), which already reads `run` **and** `uses`, splits per
command segment, and exempts a segment containing `--dry-run`. Revision 1 re-derived this loop
over `run:` only and would have missed `uses: pypa/gh-action-pypi-publish`.

- **V8a, floor:** a job named `approve-release` exists and declares an `environment:` key.
  Without it the rule's premise is gone and the check passes vacuously — the shape of fix round
  1's Minor 9.
- **V8b:** no job in `gated_path_jobs("approve-release", jobs)` may satisfy `job_publishes()`.
- **V8c, the complement:** every job satisfying `job_publishes()` must have `approve-release` in
  its own `gated_path_jobs(job_id, jobs)`. Without this, deleting `approve-release` from
  `release`'s `needs:` at `release.yml:409` removes the only human gate in the file and passes
  V1, V3, V4, V7 and V8a/V8b.
- **V8d, callees (superseded by fix round 1, Critical 1 — the shipped rule is broader and
  simpler than what this paragraph originally described):** `check_called`
  (`release_guard.py:657`) *permits* a publish step in a `workflow_call`-only workflow, and the
  fixture at `:884` asserts that is clean. The rule is not scoped to jobs already known to sit
  upstream of the approval gate — it is ONE rule over every job's own callee, implemented in
  `callee_boundary_violations`: for every job carrying `uses: ./X`, the callee `X` is loaded and,
  if any job inside it publishes, `approve-release` must be on the CALLING job's own `needs:`
  path. Restricting the check to a pre-approval SET, as the original wording implied, would miss
  a job that sits neither on `approve-release`'s needs: path nor needs `approve-release` itself —
  e.g. a `sneak` job hanging off `build`, gated on V1, calling a `workflow_call`-only local
  callee that publishes — which the general rule still catches because it scans every job's
  `uses:`, not only ones already known to be upstream of the gate. The pre-approval case (`wheels`
  and `prebuild`) is a special case of this general rule, not a separate mechanism: a caller
  upstream of the gate can never have the gate on its own `needs:` path, since `needs:` walks
  upstream, never down, so it reds under the same one rule.

**V9 — the plan job's contract.**

- **V9a, floor:** a job named `plan` exists and at least one job names it in `needs:`. V9 keys on
  that literal name, so without the floor a rename leaves it iterating an empty set.
- **V9b:** every job naming `plan` in `needs:` carries `if:` in one of exactly two accepted
  literal forms — `needs.plan.outputs.nothing_to_release != 'true'` and its `${{ }}` wrapping.
  Literal pinning, as V2 pins `GATE_EXPR`. `== 'true'` (inverted) and `== 'false'` (fails closed
  on an unset output) both red.
- **V9c:** `plan` declares `outputs.nothing_to_release`, and the `steps.<id>` it interpolates
  names a step that exists in `plan`. Catches the near-miss revision 1 could not: a typo'd step
  id yields `''` forever, silently.
- **V9d:** `plan`'s decision step invokes `ci/release-plan/run.sh`. Without it V9c passes on an
  inline `echo nothing_to_release=true`.
- **V9e:** that step runs **exactly one** command — the checker invocation and nothing else. A
  second command in the same step can overwrite `$GITHUB_OUTPUT` after the checker has written
  it, which passes both V9c and V9d and silently drops every release. This clause was added
  during review: the shape was first parked as unreachable by any structural guard, and that
  ruling was wrong — a segment count catches it. A `plan` job that legitimately needs setup work
  does it in its own steps, which §3.1 asks for anyway. The residual is narrower and stated in §6:
  a *later* step in `plan` can still overwrite the output.

**Constraint on the invocation.** `command_segments` is per **physical line**
(`release_guard.py:214`), so any command in this file must not be split across a backslash
continuation — the first fragment would be judged without its flags. `release.yml:808` already
records this class for `napi prepublish`. A comment says so at the plan step.

### 3.4 Two corrections revision 1 got wrong

**A failed `plan` job skips its dependents; it does not build them.** GitHub applies an implicit
`success()` to a job-level `if:` containing no status function. Revision 1 claimed "an unset
output because the job died → builds". That is false. The run is red, so this is not a *silent*
failure — but `plan` is a new single point of failure ahead of the entire release path, and
`continue-on-error` cannot mitigate it (V4 rejects any value but literal `false` on a gated
path). Mitigations, all in the design: the job has two steps and no toolchain, it makes no
network call, it takes seconds, and it carries `timeout-minutes: 10`.

**The `workflow_dispatch` trigger becomes permanent, and that is a decision this change makes.**
§3.2 step 1 makes a dispatch always build; that is the lever for the state where tags are cut
but a registry is missing (SMA-580's npm half). But `release.yml:60` and
`RUNBOOK-release-activation.md:696` both instruct removing the trigger once the first release
has published — which happened on 2026-08-29 — and no gate enforces or prevents that removal.
Leaving both statements standing would leave the design's stated recovery lever scheduled for
deletion. So this change **declares the trigger permanent** and amends both the trigger comment
and the runbook row accordingly. The file header's authorization argument is unaffected: it
states the boundary is the environments and their branch policies, explicitly *not* the trigger.

This is the one decision in the spec that is a judgement call rather than a measurement, and it
is flagged for review as such.

### 3.5 Where the new code is exercised

`release_plan.py` is invoked the way `release_guard.py` already is — from `ci/actionlint/run.sh`,
not as a new `repo:*` Moon task. A new Moon gate would carry five registry obligations (the
`T=(…)` array, the marker-delimited command in CLAUDE.md, `SELF_SCHEDULED_GATES`,
`SELF_TASK_EXPECTED_GLOBS`, `T_AFFECTED_SMOKE_REQUIRED_INPUTS`); this route carries two, and
`repo:actionlint` already has `inputs: ['**/*']`, so the check runs on every PR.

- **`ci/actionlint/run.sh` gains check 11**, running `--self-test`, `--negative-control` and
  `--assert` under an explicit `set -euo pipefail`, and routing **every** exit status of its
  wrapper. CLAUDE.md records why: `run.sh` is `set -uo pipefail` with no `-e`, so an unrouted
  status leaves the output empty and the check asserts nothing — measured at rc 127 from a
  missing `uv`.
- **`SELF_TEST_COUNT` goes 12 → 13**, and `ci/actionlint/run.sh` gains a
  `release_plan_self_test` table. The gate asserts invocations *and* definitions.
- **`ACTIONLINT_SH_CALL_SITES`** in `ci/affected-graph/ci_targets.py` gains check 11's call
  sites as whole lines, exactly as `run_self_tests` and `selftest_mutation_battery` are pinned.

`--assert` mode asserts, against the real repository: the derived releasable set equals a pinned
`EXPECTED_RELEASABLE = {paigasus-kernel, paigasus-proto, paigasus-proto-derive}` (strict
equality, the `EXPECTED_PR_SUBJECTS` idiom — a newly publishable crate reds until someone
re-baselines deliberately); every member resolves to exactly one manifest with a literal
version; and `git_tag_name` is unset. The **runtime** path deliberately does not use the pinned
set — it derives, so a new publishable crate is honoured immediately even if the re-baseline was
forgotten. The pin exists to force that re-baseline to be conscious, not to drive the decision.

### 3.6 The guard's fixture corpus

Not additive. `_OK_MAIN` (`release_guard.py:416`) already contains a `plan` job, contains **no**
`approve-release` job, and its `release` job carries `needs: [plan]` with no `if:` — so V8a reds
every `kind == "main"` row and V9b reds the *healthy control*. **34 of the 45 `FIXTURES` rows are
`.replace()` calls anchored to `_OK_MAIN`'s exact text**, plus `_critical2_end_to_end`
(`release_guard.py:621`), which builds its YAML from it.

`_OK_MAIN` is therefore restructured to mirror the real graph — `release-pr`, `plan` (gated,
with `outputs:`), `build` (`needs: [plan]` + the accepted `if:`), `approve-release` (with
`environment:`), `release` (`needs: [build, approve-release]`) — and all 34 anchors are
re-derived. This is the largest single piece of work in the change and the plan must budget for
it. (The row count read 44 in the first revision; the measured base was 45. Corrected in the
SMA-603 fix wave — the 34-anchor figure was measured and is unchanged.)

### 3.7 Documentation

1. **`release.yml`'s header** — the "NO `plan` JOB EXISTS" block. Its *conclusion* survives; its
   *reason* is replaced by M6, which is a stronger and more general one. It records that the
   dry-run's `releases` array is empty in dry mode even for a real release, so no dry-run-based
   plan job can work, and that the tag check is what replaced it.
2. **`release.yml:474`** — a second comment saying "There is no `plan` job … a job that does not
   exist". Becomes false and actively misleading.
3. **`release.yml:60`** — the `workflow_dispatch` removal instruction, per §3.4.
4. **`RUNBOOK-release-activation.md` §6** (assumes every dispatch reaches the gate) and **§8/step
   J** (the trigger removal).
5. **`CLAUDE.md`** — three entries. Correct the one saying the dry-run merely *requires* a git
   token (M1: it makes a live authenticated API call). Correct the one saying "the dry-run cannot
   pass until `paigasus-proto-derive` is published … this is why the release job graph carries no
   `plan`-stage dry-run" (true for the `proto` group only, and no longer the operative reason).
   Add M6 as a new entry.

## 4. Scope

**In:** `ci/release-plan/`; the `plan` job and the gating change on
`wheels`/`prebuild`/`proto-dist`; V8a–d and V9a–e; the `_OK_MAIN` restructure and 34 re-derived
fixture rows; check 11 with `SELF_TEST_COUNT` 12 → 13 and its `ACTIONLINT_SH_CALL_SITES` pins;
the five documentation items.

**Out**, each with a home:

- A version-override input making a re-dispatch recoverable after tags are cut — **SMA-602**.
- Any change to `release-pr`, to the environments, or to the credential boundary.
- Whether `release-approval` has required reviewers configured. That is a repository setting,
  not code; V8a asserts only that the `environment:` key is present, and the README says so.
- Making the `proto`-group dry-run resolvable. It is permanent, and irrelevant under §2.3.

## 5. Testing

| What | How | Where |
| --- | --- | --- |
| The decision, all branches | `release_plan.py --self-test` fixture table | `ci/actionlint/run.sh` check 11, every PR |
| The self-test can fail | `--negative-control` | check 11, every PR |
| The real repo's releasable set, manifests, `git_tag_name` | `--assert` | check 11, every PR |
| V8a–d, V9a–d, both directions | new rows in `release_guard.py`'s table | `--self-test`, check 10 |
| The guard against the real `release.yml` | `moon run repo:actionlint` | every affected PR |
| Check 11 is actually invoked and defined | `SELF_TEST_COUNT`, `ACTIONLINT_SH_CALL_SITES` | `repo:affected-smoke` |

Running any of the `moon` or `uv` commands above by hand needs
`export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` first, shims before bin. Without it a
shell resolves a global binary rather than the repo-pinned one, which CLAUDE.md records as a
standing trap — the versions differ and the gate's result is then not the one CI would get.

Fixture rows must cover, at minimum: every tag present → `true`; one tag missing → `false`; zero
tags in the repo → `false`; `version.workspace = true` → `false`; a `git_tag_name` override →
`false`; a package name resolving to no manifest → `false`; to two manifests → `false`;
`event_name = workflow_dispatch` with every tag present → `false`.

**What CI cannot prove.** That a push to `main` with nothing to release actually skips the matrix
is observable only on `main` after merge — `release.yml` has no `pull_request` trigger and must
never gain one. The first push to `main` after merge is the acceptance evidence; its expected
shape is `release-pr` runs, `plan` runs and reports `nothing_to_release=true`, every other job
skips. Unlike revision 1, the *decision logic* is no longer in that untestable region — only its
wiring is, and V9c/V9d assert the wiring statically.

## 6. Risks

| Risk | Direction | Mitigation |
| --- | --- | --- |
| The derived releasable set drifts from what release-plz tags | **unsafe, fails green** | derived at runtime, not hard-coded; `--assert` pins the expected set with strict equality |
| A tag naming scheme change (`git_tag_name`) | unsafe | floor 5 → inconclusive → builds |
| A shallow checkout removes the tags | **safe** — every tag reads as absent, so it builds | `fetch-depth: 0` is declared; floor 4 makes the cause legible in the log rather than adding safety |
| An inverted decision | unsafe | fixture table covers polarity directly; V9b pins the consumer side |
| A publish step added upstream of approval | **unsafe, irreversible** | V8b, and V8d for callees |
| `approve-release` removed from `release`'s `needs:` | **unsafe, irreversible** | V8c |
| `plan` fails and blocks a real release | safe — red, not green | §3.4: two steps, no toolchain, no network, `timeout-minutes: 10` |
| release-plz changes its tag-existence short-circuit on a bump | unsafe | the pin is 0.3.158; M2/M6 are dated and versioned, and must be re-taken on a bump. Recorded in CLAUDE.md |

## 7. Alternatives rejected

**A — read `release-plz release --dry-run --output json` as a three-way signal.** This was
revision 1's approved design. **M6 disproves it:** the dry-run reports `{"releases":[]}` at exit
0 while stating it would publish `paigasus-kernel` and cut its tag. The array records performed
releases and a dry run performs none, so the output cannot distinguish "nothing to release" from
"a release is pending". It also required a valid token (M1), a network call, and put a single
point of failure upstream of the approval gate. Recorded here so it is not reintroduced.

**B — parse the dry-run's stderr** for `Already published - Tag …` versus `due to dry,
skipping`. This *does* carry the information M6 shows is missing from the JSON. Rejected: it
parses unstructured human-readable log text that carries no stability promise, is version-
sensitive on every release-plz bump, and keeps A's token, network call and single point of
failure — while answering a question about purely local state.

**C — gate on the release commit's message**, e.g. `startsWith(github.event.head_commit.message,
'chore: release')`. Cheapest of all, and the repo's own release commit is literally `chore:
release v0.1.0` (64c9624). Rejected: the message is set by release-plz's changelog config and by
whoever squashes the PR, neither of which is asserted anywhere, and a reworded squash silently
skips a real release with no signal.
