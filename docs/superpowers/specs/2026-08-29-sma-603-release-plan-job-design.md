<!-- SPDX-License-Identifier: Apache-2.0 -->

# SMA-603 — a `plan` job for `release.yml`

**Status:** design, approved 2026-08-29
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

## 2. What was measured, and what it overturns

`release.yml`'s header records SMA-579's decision not to build a `plan` job:

> Measured against the real repository: the dry-run FAILS every time, with `no matching package
> named paigasus-proto-derive` … That failure is permanent until a LIVE release publishes the
> derive crate first.

The issue argues that condition is now satisfied, because `paigasus-proto-derive@0.1.0` is on
crates.io and `cargo publish --dry-run -p paigasus-proto` now exits 0.

**That argument is wrong, and the measurements below show why.** The evidence in the issue was
taken at the *current* version, 0.1.0, where the derive crate at 0.1.0 is indeed on the index.
It does not transfer to a release. release-plz's `version_group` bumps the whole kernel family
in lockstep, and `rs/Cargo.toml` pins the in-tree dependency with a version requirement:

```toml
paigasus-proto-derive = { path = "crates/libs/paigasus-proto-derive", version = "0.1.0" }
```

So at the next release `paigasus-proto` at `0.1.1` requires `paigasus-proto-derive ^0.1.1`,
which reaches the index only *during* the live publish. **The derive blocker is permanent.**
SMA-579's header was right about the mechanism. It was wrong only about the remedy.

### 2.1 The four measurements

All taken 2026-08-29 against release-plz 0.3.158, the proto-pinned version, with
`CARGO_REGISTRY_TOKEN` unset. `--dry-run` is source-verified never to reach
`create_git_tag_and_release`, so nothing was published and no tag was cut.

**M1 — the dry-run needs a *valid* token, not merely a present one.** With
`GIT_TOKEN` set to an invalid value, on `main` at `a73d13c`:

```
ERROR Response body: {"message": "Bad credentials", "status": "401"}
Caused by: HTTP status client error (401 Unauthorized) for url
  (https://api.github.com/repos/SMK1085/paigasus-core/commits/a73d13c…/pulls)
EXIT=1
```

This extends what CLAUDE.md records. The entry says `get_git_client()` runs unconditionally.
It does more than construct a client: it makes a live, authenticated `GET
/repos/{owner}/{repo}/commits/{sha}/pulls`.

**M2 — the empty case exits 0 and reports an empty array, in about 0.5 s.** Same tree, with a
valid token:

```
INFO paigasus-kernel 0.1.0: Already published - Tag paigasus-kernel-v0.1.0 already exists
INFO paigasus-proto-derive 0.1.0: Already published - Tag paigasus-proto-derive-v0.1.0 already exists
INFO paigasus-proto 0.1.0: Already published - Tag paigasus-proto-v0.1.0 already exists
{"releases":[]}
EXIT=0
```

Note *why* it exits 0: it short-circuits on "tag already exists" and never invokes
`cargo publish --dry-run` at all. The derive resolution is never attempted in this case.

**M3 — the non-empty case exits 1, in about 6 s.** Measured in an isolated clone with the three
version-group packages bumped to 0.1.1 and committed (`cargo publish` refuses a dirty tree,
which is M3's own first finding):

```
INFO paigasus-kernel 0.1.1: due to dry, skipping the following: ["cargo registry upload", …]
INFO paigasus-proto-derive 0.1.1: due to dry, skipping the following: ["cargo registry upload", …]
ERROR failed to release package
Caused by: failed to publish paigasus-proto: failed to prepare local package for uploading
  Caused by: failed to select a version for the requirement `paigasus-proto-derive = "^0.1.1"`
    candidate versions found which didn't match: 0.1.0
EXIT=1
```

Nothing is printed on stdout in this case. The JSON line never appears.

**M4 — the dry-run requires HEAD to be on a branch with an upstream.** A detached checkout dies
with `cannot determine current branch … fatal: HEAD does not point to a branch`. This does not
affect CI: `actions/checkout` on a `push` or `workflow_dispatch` event checks out the branch ref
and creates a local branch tracking `origin/<branch>`. It is recorded because it will bite
anyone who reproduces M2 or M3 by checking out a SHA.

### 2.2 Why the job is viable anyway

SMA-579's design failed because it read the dry-run as a **pass/fail gate**: any non-zero exit
would red the whole graph on every push. M2 and M3 separate cleanly on a different reading:

| Outcome | Meaning | Decision |
| --- | --- | --- |
| exit 0 **and** `.releases` is empty | release-plz itself would do nothing | **skip** the matrix |
| exit non-zero | inconclusive — includes the permanent derive failure, a 401, an outage | **build** |
| exit 0 **and** `.releases` is non-empty | a real release is pending | **build** |
| stdout is absent or unparsable | inconclusive | **build** |

Read this way the permanent derive failure is not a defect. It is the *signal for the case where
we must build anyway*. The job never needs the dry-run to succeed. It needs only the empty,
exit-0 outcome to be trustworthy as "nothing to release", and M2 measures exactly that.

The one failure that matters is a **false skip**, which requires the dry-run to succeed and lie.
For that to happen release-plz would have to report an empty `releases` array while a live
`release-plz release` would publish something — that is, it would have to disagree with itself,
since the plan job runs the same command in the same working directory against the same
`rs/release-plz.toml`.

## 3. Design

### 3.1 The job graph

```
plan ──> { wheels, prebuild, proto-dist } ──> approve-release ──> release ──> { publish-pypi, publish-npm }
```

`plan` becomes the single holder of the literal flag gate:

```yaml
plan:
  if: vars.PAIGASUS_RELEASE_ENABLED == 'true'
```

`wheels`, `prebuild` and `proto-dist` drop their own literal gate and take:

```yaml
  needs: [plan]
  if: needs.plan.outputs.nothing_to_release != 'true'
```

`release-pr` is untouched. It stays push-only, stays on the `release-pr` environment, and stays
in `release_guard.py`'s `UNGATED_JOBS`. It must keep proposing the release PR while the flag is
off, so it must not depend on `plan`.

**The flag still reaches every job.** With the flag off, `plan` skips; a job whose `needs:`
dependency skipped is itself skipped, regardless of its own `if:`. So the three build jobs skip,
`approve-release` skips, and everything below it skips. `release_guard.py`'s `is_gated` accepts
this: the three build jobs are no longer gated directly, but every one of their `needs:` entries
resolves to `plan`, which carries the literal `GATE_EXPR`.

This also removes two of the three copies of the gate expression. The file header's current
"three ways, not one" note becomes "two ways": literal on `plan`, transitive everywhere else.

### 3.2 The decision

`nothing_to_release` is `'true'` on the conjunction of three conditions and on nothing else:

1. `github.event_name == 'push'`, and
2. the dry-run exited 0, and
3. `.releases | length` parsed to 0.

Per the approved answer to the dispatch question, **a `workflow_dispatch` always builds.** A
dispatch is a deliberate act meaning "release now". Keeping it unconditional preserves today's
behaviour as an escape hatch — including the SMA-580 case where the tags were already cut, the
dry-run therefore reports empty, and a human still needs the artifacts built. Making the
dispatch path skip would remove the only lever left in that state, which SMA-602 tracks
separately.

**The polarity is carried by the comparison operator, not by a code path.** The output is named
for the *skip* condition and is tested with `!=`, so any value other than the literal `'true'`
builds: `'false'`, the empty string, an unset output because the step never ran, an unset output
because the job died. The alternative naming — `should_build` tested with `== 'true'` — fails
closed, and a job that dies before writing its output would silently drop a release. That is the
exact failure the issue's Risks section names, so the naming is load-bearing and V9 (§3.4) pins
it.

`continue-on-error` cannot be used to survive M3's non-zero exit: `release_guard.py` V4 rejects
any value but literal `false` on a gated path, and the reason it does — a failed publish
counting as success for `needs:` — applies here too. The step therefore captures the status in
shell:

```yaml
      - name: Decide whether anything is releasable
        id: plan
        working-directory: rs
        env:
          GIT_TOKEN: ${{ secrets.GITHUB_TOKEN }}
          EVENT_NAME: ${{ github.event_name }}
        run: |
          set -euo pipefail
          OUT=""
          EC=0
          OUT="$(release-plz release --dry-run --output json)" || EC=$?
          echo "release-plz exit status: $EC"
          echo "release-plz stdout: $OUT"

          N=-1
          if [ "$EC" -eq 0 ]; then
            N="$(printf '%s' "$OUT" | jq -r '.releases | length' 2>/dev/null || echo -1)"
          fi

          if [ "$EVENT_NAME" = "push" ] && [ "$EC" -eq 0 ] && [ "$N" = "0" ]; then
            echo "nothing_to_release=true" >> "$GITHUB_OUTPUT"
            echo "::notice::Nothing to release — skipping the build matrix."
          else
            echo "nothing_to_release=false" >> "$GITHUB_OUTPUT"
          fi
```

Three details in that block are deliberate. `set -euo pipefail` with `|| EC=$?` is safe here
because the command is invoked directly, not through a nested `$( )` whose exit status errexit
would suspend. `N` is initialised to `-1` and only computed on `EC -eq 0`, so an unparsable or
absent stdout can never read as zero. The raw exit status and stdout are echoed so a log reader
can tell M2 from M3 without re-running anything.

### 3.3 Credential

`GIT_TOKEN: ${{ secrets.GITHUB_TOKEN }}`, with job-level permissions:

```yaml
    permissions:
      contents: read
      pull-requests: read
```

Job-level `permissions:` replaces the workflow-level block rather than merging with it, so
`contents: read` is restated. M1 shows the one API call the dry-run makes is
`GET /repos/{owner}/{repo}/commits/{sha}/pulls`, which `pull-requests: read` covers.

This deliberately avoids both a GitHub App token and an `environment:` key. `plan` sits upstream
of `approve-release`, and putting a write-capable credential or an approval-bearing environment
there would place a credential before the one human checkpoint in the file.

**This is the design's single unverified premise.** Whether `GITHUB_TOKEN` satisfies that call
cannot be tested locally; it is only observable on a real run. The consequence of being wrong is
bounded and is in the safe direction: the job 401s exactly as M1 shows, exits non-zero, and
`!= 'true'` builds. The workflow degrades to today's behaviour and never to a silent skip. If it
does turn out insufficient, the follow-up is to give `plan` the App token on its own
environment, which is a strictly larger change and is out of scope here.

### 3.4 Guard

Approach A puts the literal command `release-plz release` into a job that runs **upstream of the
approval gate**. Dropping `--dry-run` is a one-word edit whose effect is to publish to crates.io
and cut tags before any human approves — the split state that `release.yml`'s job-order comment
exists to prevent. `PUBLISH_MARKERS` already matches that command, but `check_main` applies the
detector only to `UNGATED_JOBS` members, so a gated `plan` job is invisible to it today. The
guard work is therefore mandatory, not optional.

Two new verdicts in `ci/actionlint/release_guard.py`. Both get rows in the existing
`release_guard` self-test table, so `ci/actionlint/run.sh`'s `SELF_TEST_COUNT` stays 12 — no
thirteenth `*_self_test` table is added.

**V8 — nothing upstream of the approval gate may publish for real.**

1. Floor: a job named `approve-release` exists and declares an `environment:` key. Without this
   the rule's premise is gone and the check would pass vacuously — the same failure shape as
   fix round 1's Minor 9 (an empty `jobs:` mapping returning a false-clean result).
2. For every job in `gated_path_jobs("approve-release", jobs)` — the job plus its whole `needs:`
   path, so `plan`, `wheels`, `prebuild` and `proto-dist` — every `run:` line is split with the
   existing `command_segments`, and a segment matching `_PUBLISH_RE` must also contain
   `--dry-run` **in that same segment**.

Per-segment scoping is what stops a decoy: a `--dry-run` mentioned in a comment or on an
adjacent line does not satisfy the rule. This is the same fix shape as V5's Important 4 and V6's
own per-line scoping.

V8 reuses `gated_path_jobs` and `command_segments` unchanged. It adds no new parsing.

**V9 — the fail-safe polarity is pinned, not reviewed.**

1. Floor, for the same reason V8 carries one: a job named `plan` exists, and at least one job
   names it in `needs:`. V9 keys on that literal job name, so without the floor a rename would
   leave it iterating an empty set and reporting clean — asserting nothing about the polarity it
   exists to pin.
2. Every job naming `plan` in `needs:` must carry an `if:` in one of exactly two accepted
   literal forms:

```python
PLAN_GATE_EXPR = "needs.plan.outputs.nothing_to_release != 'true'"
ACCEPTED_PLAN_FORMS = frozenset({PLAN_GATE_EXPR, "${{ " + PLAN_GATE_EXPR + " }}"})
```

Literal pinning, exactly as V2 pins `GATE_EXPR`, and for the same reason: a substring or
structural test would admit `== 'false'`, which is *not* equivalent — it fails closed on an
unset output. `== 'true'` (a full inversion) and a missing `if:` both red.

The accepted set is deliberately closed. A future job that needs a different condition on `plan`
reds the guard until someone edits the set. That friction is the point: it forces the polarity
decision to be made in the guard, in the open, rather than in a workflow diff.

### 3.5 Documentation

Three files, all correcting statements that are now measurably wrong:

1. **`release.yml`'s header.** The "NO `plan` JOB EXISTS" block states the opposite decision and
   gives an obsolete reason. Its replacement records what §2 measured: the derive blocker is
   **permanent**, not resolved; the issue's contrary premise was measured at the wrong version;
   and the job is viable because the dry-run is read as a three-way signal, not as a pass/fail
   gate. The header's "three ways, not one" gating note becomes two ways.
2. **`docs/ops/RUNBOOK-release-activation.md` §6.** It assumes every dispatch reaches the
   approval gate. That stays true for `workflow_dispatch` by §3.2's decision, and stops being
   true for a push. §6 says which is which.
3. **`CLAUDE.md`.** Add M1 through M4 to the release gotchas, and correct the existing entry
   that says the dry-run merely requires a git token — it makes a live authenticated API call.

## 4. Scope

**In scope:** the `plan` job; the gating change on `wheels`/`prebuild`/`proto-dist`; V8 and V9
with their self-test rows; the three documentation corrections above.

**Out of scope**, and each already has a home:

- A version-override input to make a re-dispatch recoverable after tags are cut — **SMA-602**.
- Removing the `workflow_dispatch` trigger after the first release — runbook step J, which no
  gate enforces.
- Any change to `release-pr`, to the environments, or to the credential boundary.
- Making the derive-crate dry-run resolvable. It is permanent, and §2.2 explains why this design
  does not need it fixed.

## 5. Testing

| What | How | Where it runs |
| --- | --- | --- |
| V8 and V9 verdicts, both directions | new rows in `release_guard.py`'s self-test table | `--self-test`, invoked by `ci/actionlint/run.sh` check 10 |
| The guard against the real `release.yml` | `moon run repo:actionlint` | every affected PR |
| Workflow syntax and trigger filters | `repo:actionlint`'s actionlint pass | every affected PR |
| The full gate graph | `moon ci …` with the marker-delimited target list | every affected PR |

**What CI cannot prove.** That a push to `main` with nothing to release actually skips the
matrix is observable only on `main`, after merge — `release.yml` has no `pull_request` trigger,
and it must never gain one. The same is true of §3.3's token premise. The spec states this
rather than implying the PR's green checks cover it. The first push to `main` after merge is the
acceptance evidence, and its expected shape is: `release-pr` runs, `plan` runs and reports
`nothing_to_release=true`, and every other job skips.

## 6. Risks

| Risk | Direction | Mitigation |
| --- | --- | --- |
| `GITHUB_TOKEN` lacks the scope for the `/pulls` call | safe — 401, non-zero exit, builds | §3.3; falls back to today's behaviour, and the log shows the 401 verbatim |
| A dropped `--dry-run` publishes before approval | **unsafe and irreversible** | V8 |
| An inverted `if:` polarity silently skips real releases | unsafe, fails green | V9 |
| release-plz's dry-run behaviour changes on a version bump | unknown | the pin is 0.3.158; §2's measurements are dated and versioned, and must be re-taken on a bump |
| `plan` adds about a minute to a real release | cost only | accepted; it removes about 15 minutes from every push that releases nothing |
