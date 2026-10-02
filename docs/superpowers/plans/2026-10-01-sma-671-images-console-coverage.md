<!-- moon-diagnosis:ok -->
# SMA-671 Images Console Coverage Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the staged-tree parity check of the console smoke gate in `images.yml`, record the
arm64 job figures, and record the `@paigasus/next-config` filter decision.

**Architecture:** `images.yml` gets a host build of both consoles on each leg, before the console
smoke. A dispatch helper `parity_required_flag` reads `CONSOLE_PARITY_REQUIRED` and passes
`--parity=required|optional` to `smoke_consoles`, which passes `required|optional` to a new row
function `console_staged_parity_row` (the parity block, extracted). A new `docker_context_leaks`
function fails the job when the host build leaves a file in a Docker build context. Self-test rows
in `ci/images/console-selftest.sh` prove each arm, and row W1 pins the workflow lines.

**Tech Stack:** bash (3.2 and 5), awk (BSD and GNU), GitHub Actions, moon 2.5.3, pnpm 11.3.0,
Next 16 (Turbopack), docker buildx.

**Spec:** `docs/superpowers/specs/2026-10-01-sma-671-images-console-coverage-design.md` (APPROVED,
with the decisions in its §0). Read it with this plan. Line numbers below are from `origin/main`
`ecc87bbd`; they move as the tasks edit the files, so use the quoted anchor text.

## Global Constraints

- Work only in `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-671-images-console-coverage`
  on branch `feature/sma-671-images-console-coverage`. Begin every Bash command with
  `cd <worktree> && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" &&`.
- Before each commit, `git branch --show-current` must print `feature/sma-671-images-console-coverage`.
- Never use `--no-verify`, `--no-gpg-sign`, `git commit --amend`, `git reset` or a force push. If
  signing fails with "failed to fill whole buffer", 1Password is locked: stop and report.
- Conventional commits with a workspace scope (`feat(ci)`, `test(ci)`, `docs(ci)`, `docs(ops)`).
  End each message with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Do not merge a PR. Do not move SMA-671 to Done. Do not install host software (no brew, no
  global npm). Do not leave a background job running.
- Every source file keeps its SPDX header.
- Self-test code is bash 3.2-safe: no `mapfile`, no `declare -A`, no here-string, no heredoc, no
  pipe into an early-exit reader (`head`, `grep -q`), no expansion of an array that can be empty,
  no `${v,,}`, no `[[ -v ]]`.
- Variable name `CONSOLE_PARITY_REQUIRED`. Values: unset, empty or `0` = not required; `1` =
  required; any other value = usage error with the text
  `CONSOLE_PARITY_REQUIRED must be '0' or '1' (unset or empty means 0), not '<value>'.`
- Only `images.yml` sets the variable, as step `env` `CONSOLE_PARITY_REQUIRED: '1'` on "Console
  release sequence" and "Build + smoke both consoles". Never at job or workflow level. Never on
  "Smoke each service on its own". `release.yml` does not change.
- Host-build command: `pnpm --dir ts install --frozen-lockfile` then
  `moon run iam-console-ts:build gateway-console-ts:build --upstream none`. Never run
  `contracts:generate`.
- The host-build step has no `if:`; it runs on amd64 and arm64 (Q3).
- `@paigasus/next-config` and `ts/apps/*/moon.yml` stay off the `images.yml` filter (Q4, Q5).
- **pnpm store cache: NOT used (Q2, decided here).** Reasons: the `images` job disk has overflowed
  once, and a restored store adds GBs before the image builds; an exact-key hit skips the save
  (memory `actions-cache-key-hit-skips-save`); Q-a accepts the install cost. AC 5 records the
  measured install time. If a later measurement shows the install is too slow, a follow-up adds a
  cache with the key `images-pnpm-${{ runner.os }}-${{ runner.arch }}-<lockfile hash>`.
- **moon does not install `node_modules` here**: `ci.yml` lines 171-173 record that the repo does
  not use moon's `javascript` toolchain install. So the explicit `pnpm install` line stays first.
- Mutation runs are authorized (Sven, 2026-09-28): edit, run, see red, restore with the Edit tool,
  confirm with `git diff`, never commit a mutation.
- Docs: ASD-STE100 Simplified Technical English. A doc that names the moon CI report file needs a
  `moon-diagnosis` marker (`repo:actionlint` check 12); none of the docs here should name it.

## Review Focus

1. **A near-miss value of `CONSOLE_PARITY_REQUIRED`** (` 1`, `01`, `yes`, `TRUE`). Expected: a
   usage error, never a silent `optional`. Pinned by rows PF3, PF3b, PF3c (Task 3).
2. **A failure on the first console key in "Console release sequence".** Expected: the second key
   still runs, and the step fails. `set -e` inside a subshell on the left of `||` is ignored by
   bash, so a naive `( set -e; … ) || rc=$?` runs every command of the failed key. Pinned by rows
   RS1-RS3, which run the real step body against stubs (Task 6).
3. **A host artifact with a space in its path, or a tracked file that the build rewrites.**
   Expected: the Docker-context check names the exact path and fails. Pinned by rows CX3 and CX7
   (Task 5).
4. **The `env` line turned into a comment, or written into the job `env`.** Expected: W1 reds; a
   comment or a job-level line must not satisfy it, and a job-level line would leak into the
   self-test step. Pinned by W1-m6 and W1-m7 (Task 6).
5. **A host build that exists but has no `BUILD_ID`, or a walk that prints nothing.** Expected: a
   named error, not a pass. Pinned by rows SP5c and SP5d (Task 4).

---

## File structure

| File | Responsibility | Tasks |
|---|---|---|
| `.github/workflows/images.yml` | Host-build step, tool install, step `env`, per-key rc loop | 1, 6 |
| `ci/images/run.sh` | `parity_required_flag`, `console_staged_parity_row`, `docker_context_leaks`, the `--parity` argument, the `context-check` arm | 3, 4, 5 |
| `ci/images/console-selftest.sh` | Rows PF, D2, D3, Z6, SP, P1d, CX, W1, RS | 3, 4, 5, 6 |
| `ts/.dockerignore` | Only if Task 1 finds an ignored or untracked host artifact under `ts/` | 5 |
| `docs/ops/RUNBOOK-containers.md` | Section 1 (next-config decision), section 6 (parity now gates; the limit; rollback) | 7 |
| `rs/CLAUDE.md` | "Container images": next-config sentence, parity limit, the four SMA-675 filter entries | 7 |
| `docs/superpowers/specs/2026-10-01-sma-671-measurements.md` | The working record of every CI measurement; the PR description and the Linear comments copy from it | 1, 2, 10 |

Decision recorded here: the spec (D3) puts the item-3 figures in the PR description and a Linear
comment. The implementer also needs a durable place between tasks, so the figures go first into a
measurements file, as SMA-502, SMA-513 and SMA-634 did. That file is not an ops doc, so the RUNBOOK
still holds no job figures.

---

### Task 1: Measure the host build in CI on both legs (R-a, R-b, R-c, R-d)

The spec (§6) says: measure first. This task adds only the tool install and the host-build step,
with the variable unset, and dispatches the workflow on the branch. The existing parity arm then
compares a real host build with the image on each leg.

**Files:**
- Modify: `.github/workflows/images.yml` (the "Install crane, syft and uv" step, lines 128-133; a
  new step after "Console image self-test", lines 175-181)
- Create: `docs/superpowers/specs/2026-10-01-sma-671-measurements.md`

**Interfaces:**
- Produces: the step name `Host build of both consoles (staged-tree parity reference)` and the
  line `moon run iam-console-ts:build gateway-console-ts:build --upstream none`. Task 6 row W1
  finds the step by that `moon run iam-console-ts:build` text.
- Produces: measurements M1-M8 (below) that Tasks 5, 6 and 10 read.

- [ ] **Step 1: Widen the tool install step**

Replace the step at lines 128-133 with:

```yaml
      # Narrow installs, not a bare `proto install`: this job needs only these six. SMA-671: moon,
      # node and pnpm are for the host build of both consoles (the staged-tree parity reference).
      - name: Install crane, syft, uv, moon, node and pnpm
        run: |
          proto install crane
          proto install syft
          proto install uv
          proto install moon
          proto install node
          proto install pnpm
```

- [ ] **Step 2: Add the host-build step**

Insert directly after the "Console image self-test" step (after the line
`        run: ci/images/console-selftest.sh`) and before the comment block of "Console release
sequence":

```yaml

      # SMA-671: the reference build for the staged-tree parity row of the console smoke
      # (console_staged_parity_row in ci/images/run.sh). It builds both consoles on THIS runner,
      # so the host build and the image's builder stage run on the same OS and architecture.
      # `--upstream none` skips contracts:generate: the build compiles the COMMITTED generated
      # tree and the committed wasm, as the Docker build does, and needs no buf and no BSR. moon
      # does not install node_modules in this repo (ci.yml), so pnpm installs first. No `if:`: it
      # runs on each leg (SMA-671 Q3), rs-only PRs included (Q-a).
      - name: Host build of both consoles (staged-tree parity reference)
        timeout-minutes: 20
        run: |
          df -h /
          SECONDS=0
          pnpm --dir ts install --frozen-lockfile
          echo "host-build: pnpm install took ${SECONDS}s"
          SECONDS=0
          rc=0
          moon run iam-console-ts:build gateway-console-ts:build --upstream none || rc=$?
          echo "host-build: moon run took ${SECONDS}s (rc ${rc})"
          df -h /
          if [ "$rc" -ne 0 ]; then exit "$rc"; fi
          git status --porcelain=v1 --ignored=matching --untracked-files=normal -- ts rs/crates/bindings
```

The last line is for this measurement only. Task 6 replaces it with `ci/images/run.sh context-check`.

- [ ] **Step 3: Run actionlint on the workflow**

Run: `cd <worktree> && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && actionlint .github/workflows/images.yml`
Expected: no output, exit 0. (This is the plain linter, not the full `repo:actionlint` gate. Task 9
runs the gate.)

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/images.yml
git commit -m "ci(ci): add a host build of both consoles to images.yml (SMA-671)

Measurement first (spec section 6): the staged-tree parity arm now
finds a host build on each leg. CONSOLE_PARITY_REQUIRED is not set yet.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

If commitlint refuses the `ci` type with the `ci` scope, use `feat(ci):` with the same subject.

- [ ] **Step 5: Push the branch and dispatch the workflow**

```bash
git push -u origin feature/sma-671-images-console-coverage
gh workflow run images.yml --ref feature/sma-671-images-console-coverage
```

Then find the run id. It must have the local HEAD as `headSha`:

```bash
gh run list --workflow images.yml --branch feature/sma-671-images-console-coverage \
  --event workflow_dispatch --limit 1 --json databaseId,headSha,status
git rev-parse HEAD
```

- [ ] **Step 6: Wait for the run with Monitor**

Use the Monitor tool with an until-loop (do not use a background sleep):
`until [ "$(gh run view <id> --json status --jq .status)" = completed ]; do sleep 30; done`.
Stay in the turn until it completes. The job took 11-13 min before SMA-675; expect 20-35 min now.

- [ ] **Step 7: Read the measurements from the logs**

```bash
S=/private/tmp/claude-501/sma-671 && mkdir -p "$S"
gh run view <id> --json jobs --jq '.jobs[] | [.databaseId, .name, .conclusion] | @tsv'
gh run view <id> --log --job <amd64-job-id> > "$S/t1-amd64.log"
gh run view <id> --log --job <arm64-job-id> > "$S/t1-arm64.log"
gh api repos/SMK1085/paigasus-core/actions/runs/<id>/jobs \
  --jq '.jobs[] | {name, started_at, completed_at, steps: [.steps[] | {name, conclusion, started_at, completed_at}]}' > "$S/t1-steps.json"
for f in "$S"/t1-*.log; do
  echo "== $f"
  grep -E 'staged tree matches|NOT CHECKED|different top-level|DIFFERENT sources|host-build:|Filesystem|/dev/root|^[^ ]+\s+[^ ]+\s+(!!|\?\?| M|M |A ) ' "$f" || true
done
```

Record, for each leg:

- **M1 (R-a)**: per console and per step, "staged tree matches the host build (N files, public=X)",
  or the mismatch message.
- **M2 (R-b)**: `df -h /` (Size, Used, Avail) before and after the host-build step; the
  `pnpm install` seconds; the `moon run` seconds; the step duration and the job duration (from
  `t1-steps.json`).
- **M3 (R-c)**: each line of the `git status` output (an empty output is the good result).
- **M4 (R-d)**: whether `moon run` passed. If it failed, its error text.
- **M5**: whether moon installed a toolchain other than node and pnpm (search the step log for
  `rust`, `python`, `uv` in moon's setup lines).
- **M6**: whether a tracked file changed (a ` M` or `M ` line in M3).

- [ ] **Step 8: Apply the stop rules**

- M1 shows a mismatch on any leg: **STOP**. Report R-a as a blocker with the two lists. Do not
  continue to Task 3. The design has no fallback (spec R-a).
- M4 failed because of a missing upstream output: **STOP** and report. Do not add
  `contracts:generate`, and do not add the napi kernel build, without a decision from Sven.
- M6 shows a tracked file changed under `ts/` or any entry under `rs/crates/bindings`: **STOP** and
  report. That is a repo defect (for example a moon sync or a stale `next-env.d.ts`), not a
  `.dockerignore` matter.
- Avail after the host-build step is less than 10 GB on a leg: **STOP** and report (disk risk for
  "Console release sequence", which added about 5 GB in run 36347690901).
- M3 shows an ignored (`!!`) or untracked (`??`) path under `ts/` that `ts/.dockerignore` does not
  exclude (for example `tsconfig.tsbuildinfo`): continue. Task 5 adds the pattern to
  `ts/.dockerignore` (decision: a `.dockerignore` entry, not a delete after the build, because it
  also protects a local build and keeps the `images.yml` and `release.yml` contexts equal).
- Timeout: set `timeout-minutes` of the step to `max(20, ceil(1.5 × the slowest measured step
  minutes))`. Task 6 writes the final value.

- [ ] **Step 9: Write the measurements file**

Create `docs/superpowers/specs/2026-10-01-sma-671-measurements.md`:

```markdown
# SMA-671 measurements

Run: [<id>](https://github.com/SMK1085/paigasus-core/actions/runs/<id>), `workflow_dispatch` on
`feature/sma-671-images-console-coverage` at `<sha>`, <date>. `CONSOLE_PARITY_REQUIRED` unset.

| ID | What | amd64 | arm64 |
|---|---|---|---|
| M1 | Parity, "Console release sequence", iam-console | … | … |
| M1 | Parity, "Console release sequence", gateway-console | … | … |
| M1 | Parity, "Build + smoke both consoles", iam-console | … | … |
| M1 | Parity, "Build + smoke both consoles", gateway-console | … | … |
| M2 | `df -h /` before / after the host build (Size, Used, Avail) | … | … |
| M2 | `pnpm install` time | … | … |
| M2 | `moon run` time | … | … |
| M2 | Host-build step / job duration | … | … |
| M3 | `git status --ignored=matching` after the build | … | … |
| M4 | `moon run --upstream none` result | … | … |
| M5 | Toolchains that moon set up | … | … |
| M6 | Tracked files changed | … | … |

Decisions from these figures: <the timeout value; the .dockerignore pattern, if any>.
```

Fill each cell from Step 7. Write facts only.

- [ ] **Step 10: Commit**

```bash
git add docs/superpowers/specs/2026-10-01-sma-671-measurements.md
git commit -m "docs(ci): record the SMA-671 host-build measurements

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Record item 3 (run 36347690901) and item 4 (the watch item)

Independent of Task 1. It can run while the Task 1 dispatch runs.

**Files:**
- Modify: `docs/superpowers/specs/2026-10-01-sma-671-measurements.md` (add a section; create the
  file with the heading from Task 1 Step 9 if Task 1 has not made it yet)

**Interfaces:**
- Produces: the text of the item-3 and item-4 Linear comment, and the section "Item 3" that the PR
  description copies.

- [ ] **Step 1: Read the run 36347690901 jobs and logs**

```bash
S=/private/tmp/claude-501/sma-671 && mkdir -p "$S"
gh run view 36347690901 --json headSha,createdAt,jobs --jq '.headSha, .createdAt, (.jobs[] | [.databaseId, .name, .conclusion, .startedAt, .completedAt] | @tsv)'
gh run view 36347690901 --log --job <amd64-job-id> > "$S/r363-amd64.log"
gh run view 36347690901 --log --job <arm64-job-id> > "$S/r363-arm64.log"
gh api repos/SMK1085/paigasus-core/actions/runs/36347690901/jobs \
  --jq '.jobs[] | {name, steps: [.steps[] | {name, started_at, completed_at}]}' > "$S/r363-steps.json"
grep -n -E 'Filesystem|/dev/root' "$S/r363-amd64.log" "$S/r363-arm64.log"
```

Confirm F7 and F8 of the spec. Read the amd64 disk Size from the `df -h /` header and data lines
(the gap in F8).

- [ ] **Step 2: Answer Q-c (is the 6 s a BuildKit cache hit?)**

In `r363-arm64.log`, take the lines of the step "Build + smoke both consoles". Count the BuildKit
lines with `CACHED` and the lines with `DONE` for the `RUN` steps, and find the `next build`
`RUN` line:

```bash
awk '/Build \+ smoke both consoles/' "$S/r363-arm64.log" > "$S/r363-arm64-bsc.log"
grep -c ' CACHED' "$S/r363-arm64-bsc.log" || true
grep -n -E 'RUN .*next build|pnpm exec next build' "$S/r363-arm64-bsc.log" || true
grep -n -E '== CONSOLE SMOKE OK ==|staged-tree parity' "$S/r363-arm64-bsc.log" || true
```

Record the answer: if the `next build` stage of both apps shows `CACHED`, the job pays for ONE
pair of Next builds, not two. Also record whether the step reached `== CONSOLE SMOKE OK ==`.

- [ ] **Step 3: Re-check the item-4 watch item (F10)**

```bash
gh pr list --state all --limit 100 --search 'author:app/dependabot docker' \
  --json number,title,state,mergedAt --jq '.[] | [.number, .state, .mergedAt, .title] | @tsv'
```

Record each PR that refreshes `ubuntu:24.04`, `rust:1.95.0-bookworm` or `node:24.16.0-bookworm`.
Item 4 stays open unless a `rust` or `node` digest-refresh PR exists.

- [ ] **Step 4: Add the section to the measurements file**

Append:

```markdown
## Item 3: the job figures of run 36347690901 (SMA-671 Q1)

Run [36347690901](https://github.com/SMK1085/paigasus-core/actions/runs/36347690901): `main` push,
`83d446fc`, <date>. This run predates SMA-675 (the Redis sidecar and the kernel rows of the console
smoke), so the current job does more work than this run.

| Leg | Disk size | Used at start / after reclaim / before release seq. / after it / after Build + smoke | "Console release sequence" | "Build + smoke both consoles" | Job |
|---|---|---|---|---|---|
| amd64 | … | … | 2 min 1 s | … | 12 min 42 s |
| arm64 | 145 GB | 37 / 29 / 35 / 40 / 40 GB | 1 min 56 s | 6 s | 11 min 7 s |

Q-c: <the answer from Step 2, with the CACHED count>.

## Item 4: the base-image digest watch item

<F10, re-checked on <date>: the PR list from Step 3>. Item 4 stays open.
```

- [ ] **Step 5: Post the Linear comment**

Post the two sections as one comment on SMA-671 (Linear `save_comment`). If this agent has no
Linear tool, add a line `Linear comment: PENDING (post the two sections above)` under the section,
and name it in the task report.

- [ ] **Step 6: Commit**

```bash
git add docs/superpowers/specs/2026-10-01-sma-671-measurements.md
git commit -m "docs(ci): record the SMA-671 item 3 job figures and the item 4 watch state

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: `parity_required_flag` and the `--parity` argument of `smoke_consoles`

Do not start this task if Task 1 stopped.

**Files:**
- Modify: `ci/images/run.sh` (after `kernel_control_flag`, lines 1545-1559; `smoke_consoles` lines
  1808-1825; the dispatch arms at lines 2467-2476 and 2510-2527; the header lines 11-20)
- Test: `ci/images/console-selftest.sh` (`FUNCS` line 36; new rows after KC3, line 1005; Z rows
  lines 1041-1067; a new D2 and D3 after D1, line 1080)

**Interfaces:**
- Produces: `parity_required_flag` (no arguments; reads `CONSOLE_PARITY_REQUIRED`; prints
  `--parity=optional` or `--parity=required`; on another value prints the usage error to stderr
  and returns 1).
- Produces: `smoke_consoles <--kernel-control=on|off> <--parity=required|optional> <zone>=<image>...`.
  Inside it, the local `parity` holds `required` or `optional`. Task 4 passes it to the row.

- [ ] **Step 1: Write the failing rows**

In `console-selftest.sh`:

(a) Directly after `export PROTO_REPORTER=text` (line 29), add:

```bash
# SMA-671: no row reads the parity mode from the environment. A developer's shell or a future
# job-level env must not change a row; the PF rows set the variable inside their own wrapper.
unset CONSOLE_PARITY_REQUIRED
```

(b) Append ` parity_required_flag` to the `FUNCS` string (line 36).

(c) After row KC3 (line 1005), add:

```bash
# --- SMA-671: the parity switch (PF rows) -----------------------------------------------------
# shellcheck disable=SC2034 # parity_required_flag reads the variable.
pf_set() { CONSOLE_PARITY_REQUIRED="$1"; parity_required_flag; }
pf_unset() { unset CONSOLE_PARITY_REQUIRED; parity_required_flag; }
stub_reset
run_fn PF0 0 "" "::error::" none pf_unset
expect_in PF0-out "$T/PF0.out" "--parity=optional"
stub_reset
run_fn PF0b 0 "" "::error::" none pf_set ""
expect_in PF0b-out "$T/PF0b.out" "--parity=optional"
stub_reset
run_fn PF0c 0 "" "::error::" none pf_set 0
expect_in PF0c-out "$T/PF0c.out" "--parity=optional"
stub_reset
run_fn PF1 0 "" "::error::" none pf_set 1
expect_in PF1-out "$T/PF1.out" "--parity=required"
stub_reset
run_fn PF2 1 "CONSOLE_PARITY_REQUIRED must be '0' or '1' (unset or empty means 0), not 'true'." "" none pf_set true
# Review Focus 1: a near miss is a usage error, never a silent optional.
stub_reset
run_fn PF3 1 "CONSOLE_PARITY_REQUIRED must be '0' or '1'" "" none pf_set " 1"
expect_not_in PF3-out "$T/PF3.out" "--parity="
stub_reset
run_fn PF3b 1 "CONSOLE_PARITY_REQUIRED must be '0' or '1'" "" none pf_set 01
expect_not_in PF3b-out "$T/PF3b.out" "--parity="
stub_reset
run_fn PF3c 1 "CONSOLE_PARITY_REQUIRED must be '0' or '1'" "" none pf_set yes
expect_not_in PF3c-out "$T/PF3c.out" "--parity="
```

(d) Change the Z rows. Z1, Z2 and Z5 get a second word; Z6 and Z6b are new. Replace lines
1041-1067 with:

```bash
stub_reset; z_curl Z1; rm -f "$Z_CALLS"; Z_REDIS_RC=0
run_fn Z1 1 "" "" any smoke_case --kernel-control=off --parity=optional iam=img:dev gateway=img:dev
expect_in Z1-skip-iam "$T/Z1.out" "iam-console: kernel control row skipped (PAIGASUS_SMOKE_KERNEL_CONTROL=off, the release path; SMA-675 Q5)"
expect_in Z1-skip-gw "$T/Z1.out" "gateway-console: kernel control row skipped (PAIGASUS_SMOKE_KERNEL_CONTROL=off"
expect_in Z1-route-iam "$Z_CALLS" "route /iam/orgs"
expect_in Z1-route-gw "$Z_CALLS" "route /gateway/overview"
expect_not_in Z1-nocontrol "$Z_CALLS" "control"
expect_no_call Z1 "-nokernel-"
expect_call Z1-net "--network smoke-net-z"
expect_call Z1-redis "PAIGASUS_SESSION_REDIS_URL=redis://smoke-redis-z:6379"
expect_no_call Z1-nomem "PAIGASUS_SESSION_STORE=memory"
stub_reset; z_curl Z2; rm -f "$Z_CALLS"; Z_REDIS_RC=0
run_fn Z2 1 "" "" any smoke_case --kernel-control=on --parity=required iam=img:dev gateway=img:dev
expect_in Z2-control-iam "$Z_CALLS" "control iam-console"
expect_in Z2-control-gw "$Z_CALLS" "control gateway-console"
expect_not_in Z2-noskip "$T/Z2.out" "kernel control row skipped"
stub_reset
run_fn Z3 1 "the first argument must be --kernel-control=on or --kernel-control=off, not 'iam=img:dev'" "" none smoke_case iam=img:dev
stub_reset
run_fn Z4 1 "not '--kernel-control=maybe'" "" none smoke_case --kernel-control=maybe iam=img:dev
# D6: with no Redis, the kernel rows do not run and the containers get the memory store.
stub_reset; z_curl Z5; rm -f "$Z_CALLS"; Z_REDIS_RC=1
run_fn Z5 1 "iam-console: kernel rows NOT run" "" any smoke_case --kernel-control=on --parity=optional iam=img:dev gateway=img:dev
expect_not_in Z5-norows "$Z_CALLS" "route"
expect_call Z5-mem "PAIGASUS_SESSION_STORE=memory"
expect_no_call Z5 "--network"
Z_REDIS_RC=0
# SMA-671: the second word is the parity mode. A bad or missing one stops before any docker call.
stub_reset
run_fn Z6 1 "the second argument must be --parity=required or --parity=optional, not '--parity=maybe'" "" none smoke_case --kernel-control=on --parity=maybe iam=img:dev
stub_reset
run_fn Z6b 1 "the second argument must be --parity=required or --parity=optional, not 'iam=img:dev'" "" none smoke_case --kernel-control=on iam=img:dev
```

(e) After the D1 rows (after line 1080, `… say_pass D1-nodocker; fi`), add:

```bash
# SMA-671 D2 and D3: the two console dispatch arms, through the REAL script. A bad parity value
# stops each one before any docker call (the build of all-consoles included).
rm -f "$T/argv"
D2_RC=0
( PATH="$T/stub:$PATH"; STUB_ARGV="$T/argv"; export PATH STUB_ARGV
  CONSOLE_PARITY_REQUIRED=bogus "$BASH" "$RUN_SH" all-consoles ) >"$T/D2.out" 2>"$T/D2.err" || D2_RC=$?
check_row D2 "$D2_RC" 1 "CONSOLE_PARITY_REQUIRED must be '0' or '1'" "" "$T/D2.out" "$T/D2.err"
if [ -e "$T/argv" ]; then say_fail D2-nodocker "the stub docker was called" "$T/argv"; else say_pass D2-nodocker; fi
rm -f "$T/argv"
D3_RC=0
( PATH="$T/stub:$PATH"; STUB_ARGV="$T/argv"; export PATH STUB_ARGV
  CONSOLE_PARITY_REQUIRED=bogus "$BASH" "$RUN_SH" smoke iam-console ) >"$T/D3.out" 2>"$T/D3.err" || D3_RC=$?
check_row D3 "$D3_RC" 1 "CONSOLE_PARITY_REQUIRED must be '0' or '1'" "" "$T/D3.out" "$T/D3.err"
if [ -e "$T/argv" ]; then say_fail D3-nodocker "the stub docker was called" "$T/argv"; else say_pass D3-nodocker; fi
```

- [ ] **Step 2: Run the self-test and see it fail**

Run: `cd <worktree> && /bin/bash ci/images/console-selftest.sh; echo "rc=$?"`
Expected: rc 2 and `console-selftest: infrastructure error (rc=2): no 'parity_required_flag() {' line in ci/images/run.sh`.

- [ ] **Step 3: Add the helper**

In `ci/images/run.sh`, directly after the closing `}` of `kernel_control_flag` (line 1559), add:

```bash

# SMA-671: whether the staged-tree parity row must find a host build. images.yml makes a host build
# and sets CONSOLE_PARITY_REQUIRED to 1 on its two console smoke steps. release.yml has no host
# build and does not set it, so an UNSET value must mean "not required": a release must not red for
# want of a host build. That is the OPPOSITE polarity of PAIGASUS_SMOKE_KERNEL_CONTROL, on purpose.
# Unset, empty or 0 means not required, and 1 means required. Any other value (true, yes, ' 1') is a
# usage error, so a typo cannot turn the gate off silently. It prints the second argument of
# smoke_consoles. The dispatch arms call it; no row function reads the variable (the SMA-675 rule).
parity_required_flag() {
  case "${CONSOLE_PARITY_REQUIRED-}" in
    ''|0) echo "--parity=optional" ;;
    1) echo "--parity=required" ;;
    *)
      echo "::error::CONSOLE_PARITY_REQUIRED must be '0' or '1' (unset or empty means 0), not '${CONSOLE_PARITY_REQUIRED-}'." >&2
      return 1
      ;;
  esac
}
```

- [ ] **Step 4: Parse the second argument in `smoke_consoles`**

In `smoke_consoles`, add `parity` to the `local` line that starts
`local kernel_control kernel_ok redis_name` so it reads
`local kernel_control kernel_ok parity redis_name work zones_json net redis_url args_file args_rc line sid cargs`.
Then replace the `shift` that follows the first `esac` (line 1825) with:

```bash
  shift
  # SMA-671: the second word says whether the staged-tree parity row must find a host build. It is
  # an argument, not a global (the SMA-675 rule); the dispatch arms derive it with
  # parity_required_flag. It is read before the trap, so a usage error makes no docker call.
  case "${1:-}" in
    --parity=required) parity="required" ;;
    --parity=optional) parity="optional" ;;
    *)
      echo "::error::smoke_consoles: the second argument must be --parity=required or --parity=optional, not '${1:-<none>}'." >&2
      return 1
      ;;
  esac
  shift
```

Task 4 uses `parity`. Until then it is set and not read.

- [ ] **Step 5: Wire the two dispatch arms**

In the `smoke)` arm, after `kc_flag="$(kernel_control_flag)" || exit 1`, add
`    pr_flag="$(parity_required_flag)" || exit 1`, and change `smoke_consoles "$kc_flag" "$@"` to
`smoke_consoles "$kc_flag" "$pr_flag" "$@"`. Change the comment line above `kc_flag` to:
`    # SMA-675 Q5 and SMA-671: only the console branch reads the two switches; a cargo smoke ignores them.`

In the `all-consoles)` arm, after `kc_flag="$(kernel_control_flag)" || exit 1`, add
`    pr_flag="$(parity_required_flag)" || exit 1`, change the comment above `kc_flag` to
`    # SMA-675 Q5 and SMA-671: read first, so a bad value stops before the build.`, and change
`smoke_consoles "$kc_flag" "$@"` to `smoke_consoles "$kc_flag" "$pr_flag" "$@"`.

- [ ] **Step 6: Document the variables in the header**

After the line `# <key> is a chain key of ci/images/chains.toml: iam, gateway, iam-console or gateway-console.`
(line 20), add:

```bash
# Two switches of the console smoke (`smoke <console-key>` and `all-consoles`):
#   PAIGASUS_SMOKE_KERNEL_CONTROL  unset or `on` runs the kernel control row; `off` skips it
#                                  (release.yml). Any other value is a usage error.
#   CONSOLE_PARITY_REQUIRED        `1` makes a missing host build an error (images.yml, SMA-671);
#                                  unset, empty or `0` prints "NOT CHECKED" (release.yml, a local
#                                  run). Any other value is a usage error.
```

- [ ] **Step 7: Run the self-test and see it pass**

Run: `cd <worktree> && bash -n ci/images/run.sh && /bin/bash ci/images/console-selftest.sh; echo "rc=$?"`
Expected: rc 0, and the summary line `console-selftest: N passed, 0 failed, M skipped`. The rows
PF0-PF3c, Z1-Z6b, D1-D3 and their `-out`/`-nodocker` rows print `PASS`.

- [ ] **Step 8: Commit**

```bash
git add ci/images/run.sh ci/images/console-selftest.sh
git commit -m "feat(ci): add the CONSOLE_PARITY_REQUIRED switch to the console smoke (SMA-671)

parity_required_flag reads the variable at dispatch level and passes
--parity=required|optional to smoke_consoles, as SMA-675 does for the
kernel control switch. Rows PF0-PF3c, Z6, Z6b, D2 and D3.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: Extract `console_staged_parity_row` and make the required arm fail

**Files:**
- Modify: `ci/images/run.sh` (remove the global `CONSOLE_STAGED_TREE_JS`, lines 1593-1621 with its
  comment; add the function before the line `# R-NODE (SMA-670 gap 1).`; replace the parity block in
  `smoke_consoles`, lines 2157-2247; trim the `local` lines 1810-1811)
- Test: `ci/images/console-selftest.sh` (header lines 11-15; `FUNCS`; new SP rows and P1d after
  row P1b; `smoke_case` lines 1013-1031; Z1 and Z2 checks)

**Interfaces:**
- Consumes: the local `parity` of `smoke_consoles` (Task 3).
- Produces: `console_staged_parity_row <app> <image> <required|optional>`; returns 0 on a match
  (or on "NOT CHECKED" in `optional` mode), 1 on any defect. Reads the global `ROOT` only.
- Produces: the pinned call line `console_staged_parity_row "$app" "$image" "$parity" || ec=1`.

- [ ] **Step 1: Write the failing rows**

In `console-selftest.sh`:

(a) Append ` console_staged_parity_row` to `FUNCS`.

(b) Directly after the line `pin_rows P1b "$T/fn-smoke_consoles.sh" 'console_image_config_row "$app" "$image" || ec=1'`
(line 756), add:

```bash

# --- SMA-671: console_staged_parity_row (SP rows) --------------------------------------------
# The argv that the row must hand to docker: the staged-tree walk of the IMAGE, never of a name
# that the row builds from <app> (the SMA-688 rule).
printf '%s\n' run --rm --entrypoint /nodejs/bin/node "$T_IMAGE" -e '<multi-line>' /app/apps/iam-console -- > "$T/argv-parity"

# sp_fx <name> <build-id> <file>... — a fake host build of iam-console under $T/fx-sp-<name>, and
# FX_ROOT points at it (call it AFTER stub_reset, which resets FX_ROOT). .next/BUILD_ID holds
# <build-id>; there is no BUILD_ID file when <build-id> is empty. Each <file> is made under
# .next/static, and a leading <BUILD_ID>/ becomes <build-id>/.
sp_fx() {
  local name="$1" id="$2" std f
  shift 2
  FX_ROOT="$T/fx-sp-$name"
  std="$FX_ROOT/ts/apps/iam-console/.next/standalone/apps/iam-console"
  rm -rf "$FX_ROOT"
  mkdir -p "$std/.next/static"
  if [ -n "$id" ]; then printf '%s\n' "$id" > "$std/.next/BUILD_ID"; fi
  for f in "$@"; do
    case "$f" in "<BUILD_ID>/"*) f="$id/${f#<BUILD_ID>/}" ;; esac
    mkdir -p "$std/.next/static/$(dirname "$f")"
    : > "$std/.next/static/$f"
  done
}
SP_ID="spBuildId0123"
# What the image walk prints for a tree equal to `sp_fx … chunks/a.js '<BUILD_ID>/_buildManifest.js'`.
SP_WALK_OK="$(printf '%s\n' public=0 'chunks/a.js' '<BUILD_ID>/_buildManifest.js')"

stub_reset; FX_ROOT="$T/fx-sp-none"; mkdir -p "$FX_ROOT"
run_fn SP1 0 "" "::error::" none console_staged_parity_row iam-console "$T_IMAGE" optional
expect_in SP1-out "$T/SP1.out" "staged-tree parity NOT CHECKED"
stub_reset; FX_ROOT="$T/fx-sp-none"
run_fn SP2 1 "parity was required but NOT checked" "" none console_staged_parity_row iam-console "$T_IMAGE" required
stub_reset; FX_ROOT="$T/fx-sp-none"
run_fn SP2b 1 "the third argument must be required or optional, not 'maybe'" "" none console_staged_parity_row iam-console "$T_IMAGE" maybe
stub_reset; sp_fx ok "$SP_ID" chunks/a.js '<BUILD_ID>/_buildManifest.js'; STUB_WALK_OUT="$SP_WALK_OK"
run_fn SP3 0 "" "::error::" "$T/argv-parity" console_staged_parity_row iam-console "$T_IMAGE" required
expect_in SP3-out "$T/SP3.out" "staged tree matches the host build (2 files, public=0)"
stub_reset; sp_fx ok "$SP_ID" chunks/a.js '<BUILD_ID>/_buildManifest.js'; STUB_WALK_OUT="$SP_WALK_OK"
run_fn SP3b 0 "" "::error::" "$T/argv-parity" console_staged_parity_row iam-console "$T_IMAGE" optional
expect_in SP3b-out "$T/SP3b.out" "staged tree matches the host build (2 files, public=0)"
stub_reset; sp_fx pub "$SP_ID" chunks/a.js '<BUILD_ID>/_buildManifest.js'
mkdir -p "$FX_ROOT/ts/apps/iam-console/.next/standalone/apps/iam-console/public"
STUB_WALK_OUT="$(printf '%s\n' public=1 'chunks/a.js' '<BUILD_ID>/_buildManifest.js')"
run_fn SP3c 0 "" "::error::" "$T/argv-parity" console_staged_parity_row iam-console "$T_IMAGE" required
expect_in SP3c-out "$T/SP3c.out" "staged tree matches the host build (2 files, public=1)"
stub_reset; sp_fx dirs "$SP_ID" media/f.woff '<BUILD_ID>/_buildManifest.js'; STUB_WALK_OUT="$SP_WALK_OK"
run_fn SP4 1 "different top-level directories" "" "$T/argv-parity" console_staged_parity_row iam-console "$T_IMAGE" required
stub_reset; sp_fx files "$SP_ID" chunks/b.js '<BUILD_ID>/_buildManifest.js'; STUB_WALK_OUT="$SP_WALK_OK"
run_fn SP4b 1 "the chunk-name assumption failing" "Re-run 'moon run" "$T/argv-parity" console_staged_parity_row iam-console "$T_IMAGE" required
stub_reset; sp_fx files "$SP_ID" chunks/b.js '<BUILD_ID>/_buildManifest.js'; STUB_WALK_OUT="$SP_WALK_OK"
run_fn SP4c 1 "DIFFERENT sources" "the chunk-name assumption failing" "$T/argv-parity" console_staged_parity_row iam-console "$T_IMAGE" optional
stub_reset; sp_fx ok "$SP_ID" chunks/a.js '<BUILD_ID>/_buildManifest.js'
STUB_WALK_OUT="$(printf '%s\n' public=1 'chunks/a.js' '<BUILD_ID>/_buildManifest.js')"
run_fn SP4d 1 "disagree on staging public/" "" "$T/argv-parity" console_staged_parity_row iam-console "$T_IMAGE" required
stub_reset; sp_fx ok "$SP_ID" chunks/a.js; STUB_WALK_OUT="Error: ENOENT: no such file or directory, scandir '/app/apps/iam-console/.next/static'"; STUB_WALK_RC=1
run_fn SP5 1 "the staging copy in ts/Dockerfile did not run" "" "$T/argv-parity" console_staged_parity_row iam-console "$T_IMAGE" required
stub_reset; sp_fx ok "$SP_ID" chunks/a.js; STUB_WALK_OUT=""; STUB_WALK_RC=125
run_fn SP5b 1 "NOT checked — docker exited 125" "" "$T/argv-parity" console_staged_parity_row iam-console "$T_IMAGE" required
# Review Focus 5: an empty walk and a missing BUILD_ID are named errors, never a pass.
stub_reset; sp_fx ok "$SP_ID" chunks/a.js; STUB_WALK_OUT=""; STUB_WALK_RC=0
run_fn SP5c 1 "printed nothing" "" "$T/argv-parity" console_staged_parity_row iam-console "$T_IMAGE" required
stub_reset; sp_fx noid "" chunks/a.js; STUB_WALK_OUT="$SP_WALK_OK"
run_fn SP5d 1 "has no .next/BUILD_ID" "" "$T/argv-parity" console_staged_parity_row iam-console "$T_IMAGE" required

# shellcheck disable=SC2016 # the pinned call line is literal text
pin_rows P1d "$T/fn-smoke_consoles.sh" 'console_staged_parity_row "$app" "$image" "$parity" || ec=1'
```

(c) In `smoke_case`, delete the line `  CONSOLE_STAGED_TREE_JS=""` and add, after the line
`  console_kernel_control_row() { echo "control $1" >> "$Z_CALLS"; }`:

```bash
  console_staged_parity_row() { echo "parity $1 $3" >> "$Z_CALLS"; }
```

(d) After `expect_no_call Z1-nomem "PAIGASUS_SESSION_STORE=memory"`, add:

```bash
expect_in Z1-parity-iam "$Z_CALLS" "parity iam-console optional"
expect_in Z1-parity-gw "$Z_CALLS" "parity gateway-console optional"
```

After `expect_not_in Z2-noskip "$T/Z2.out" "kernel control row skipped"`, add:

```bash
expect_in Z2-parity-iam "$Z_CALLS" "parity iam-console required"
expect_in Z2-parity-gw "$Z_CALLS" "parity gateway-console required"
```

(e) Replace the header lines 14-15 (`dispatch code at the end exits before any function could run.
ROOT is the only global that a` / `copied function reads, and the harness sets it inside each
row's subshell.`) with:

```bash
# dispatch code at the end exits before any function could run. A copied ROW function reads no
# global except ROOT, which run_fn sets to FX_ROOT inside each row's subshell. smoke_consoles reads
# more globals; smoke_case sets them and stubs every row that needs a real image.
```

- [ ] **Step 2: Run the self-test and see it fail**

Run: `cd <worktree> && /bin/bash ci/images/console-selftest.sh; echo "rc=$?"`
Expected: rc 2 and `no 'console_staged_parity_row() {' line in ci/images/run.sh`.

- [ ] **Step 3: Remove the global and add the row function**

In `ci/images/run.sh`, delete the comment block that starts
`# Walks the image's staged tree with the image's OWN node` and the whole
`CONSOLE_STAGED_TREE_JS='…'` assignment that follows it (through its closing `'` line).

Directly before the line `# R-NODE (SMA-670 gap 1). The runtime base pins only the Node MAJOR`,
add:

```bash
# Spec § 5.5 assertion 4 (SMA-513) — staged-tree parity, a row of its own since SMA-671.
# console_staged_parity_row <app> <image> <required|optional>. ts/Dockerfile's staging of
# .next/static and public/ and ts/apps/<app>/moon.yml's `build` script are a SECOND staging site
# each, created deliberately; this is what keeps the two from drifting apart.
#
# It compares the staged TREES, not the two scripts' text, so the two divergences the Task 2
# review recorded are tolerated by construction: moon.yml's `rm -rf .next/static` before
# `next build` (the Dockerfile needs no counterpart — `**/.next` in ts/.dockerignore makes every
# builder start cold) and moon.yml's app-name prefix on its two error messages both leave the
# staged tree identical.
#
# The third argument says whether a host build must exist. smoke_consoles passes it from its
# --parity= word, which parity_required_flag derives from CONSOLE_PARITY_REQUIRED at dispatch
# level; this row never reads the variable (the SMA-675 rule). images.yml makes a host build and
# asks for `required`. release.yml has no host build and gets `optional`.
console_staged_parity_row() {
  local app="${1:-}" image="${2:-}" parity="${3:-}" rc=0 tree_js
  local host_std host_static host_id host_public host_list img_rc img_out img_public img_list img_dirs host_dirs
  case "$parity" in
    required|optional) ;;
    *)
      echo "::error::${app}: console_staged_parity_row: the third argument must be required or optional, not '${parity}'." >&2
      return 1
      ;;
  esac
  # Walks the image's staged tree with the image's OWN node — the runtime base is distroless and
  # has no shell, so there is no `find` in there to call. Prints `public=0|1` on line 1 and one
  # staged .next/static path per line after it, with the BUILD_ID directory rewritten to the
  # literal <BUILD_ID>. MEASURED (SMA-513): Next generates BUILD_ID as a random nanoid — no
  # next.config.ts in this repo sets generateBuildId — so the host build and the image build never
  # share one, and an un-normalised comparison of the two trees can NEVER pass. Sorting is left to
  # the caller, which puts both sides through the same `LC_ALL=C sort`: also MEASURED, node's
  # Array.sort and the host's `sort` disagreed on `chunks/3_j6cf7txpq_5.js` vs
  # `chunks/3h4osm35n9wui.js`, which would have reported drift between two byte-identical trees.
  tree_js='
const fs = require("fs");
const root = process.argv[1];
const id = fs.readFileSync(root + "/.next/BUILD_ID", "utf8").trim();
const walk = (d, p = "") =>
  fs.readdirSync(d, { withFileTypes: true })
    .flatMap((e) => (e.isDirectory() ? walk(d + "/" + e.name, p + e.name + "/") : [p + e.name]));
const rel = walk(root + "/.next/static")
  .map((f) => (id !== "" && f.indexOf(id + "/") === 0 ? "<BUILD_ID>/" + f.slice(id.length + 1) : f));
console.log(["public=" + (fs.existsSync(root + "/public") ? "1" : "0")].concat(rel).join("\n"));
'
  host_std="$ROOT/ts/apps/${app}/.next/standalone/apps/${app}"
  host_static="$host_std/.next/static"
  if [ ! -d "$host_static" ]; then
    if [ "$parity" = required ]; then
      echo "::error::${app}: no host build at ${host_static}, and parity is required (CONSOLE_PARITY_REQUIRED=1) — staged-tree parity was required but NOT checked. Run 'moon run ${app}-ts:build --upstream none' before the smoke." >&2
      return 1
    fi
    # Not required: it says so out loud rather than passing silently, because a check that quietly
    # skips is the failure mode this repository has paid for repeatedly. The absence of a host
    # build is not a defect here: release.yml and a plain local run have none. images.yml makes
    # one and asks for `required`, so there this arm is the error above.
    echo "  ${app}: no host build at ${host_static}; staged-tree parity NOT CHECKED this run (images.yml makes a host build and sets CONSOLE_PARITY_REQUIRED=1; this run did not — run 'moon run ${app}-ts:build' first to check it)"
    return 0
  fi
  # Two causes, two messages, and the rc separates them. node's own uncaught ENOENT on
  # .next/static (or on .next/BUILD_ID) exits 1, and only THAT says the staging copy did not run.
  # docker refusing the image exits 125/126/127 before node starts, which proves nothing about
  # staging at all. stderr is captured rather than discarded: the tool's own message names the
  # cause.
  img_rc=0
  img_out="$(docker run --rm --entrypoint /nodejs/bin/node "$image" \
    -e "$tree_js" "/app/apps/${app}" 2>&1)" || img_rc=$?
  if [ "$img_rc" -eq 1 ]; then
    echo "::error::${app}: /app/apps/${app}/.next/static is absent or unreadable inside the image — the staging copy in ts/Dockerfile did not run. node's message follows." >&2
    printf '%s\n' "$img_out" >&2
    return 1
  elif [ "$img_rc" -ne 0 ]; then
    echo "::error::${app}: staged-tree parity NOT checked — docker exited ${img_rc} on ${image} before node ran, so the image is missing or unreadable and nothing was proved about staging. Its message follows." >&2
    printf '%s\n' "$img_out" >&2
    return 1
  elif [ -z "$img_out" ]; then
    echo "::error::${app}: the staged-tree walk exited 0 but printed nothing — that is neither a staging failure nor a docker failure; inspect ${image} by hand." >&2
    return 1
  fi
  # Line 1 is the public= marker, the rest is the tree. `sed -n 1p` / `sed -n '2,$p'` read their
  # whole input; neither is an early-exit reader.
  img_public="$(printf '%s\n' "$img_out" | sed -n 1p)"
  img_list="$(printf '%s\n' "$img_out" | sed -n '2,$p' | LC_ALL=C sort)"
  host_public="public=0"
  if [ -d "$host_std/public" ]; then host_public="public=1"; fi
  host_id="$(cat "$host_std/.next/BUILD_ID" 2>/dev/null)" || host_id=""
  if [ -z "$host_id" ]; then
    echo "::error::${app}: the host build at ${host_std} has no .next/BUILD_ID — that build is broken or half-written; re-run 'moon run ${app}-ts:build'." >&2
    return 1
  fi
  host_list="$(cd "$host_static" && find . -type f | sed 's#^\./##' \
    | sed "s#^${host_id}/#<BUILD_ID>/#" | LC_ALL=C sort)" || host_list=""
  if [ "$img_public" != "$host_public" ]; then
    echo "::error::${app}: the image staged ${img_public} but the host build staged ${host_public} — ts/Dockerfile and ts/apps/${app}/moon.yml disagree on staging public/." >&2
    rc=1
  fi
  if [ "$img_list" != "$host_list" ]; then
    # Two distinct causes produce a difference here, and they send a reader to different places,
    # so they get different messages. A missing or partial staging copy changes which TOP-LEVEL
    # directories exist under .next/static; two builds of different source keep the same
    # directories and change only the content-hashed file names inside them.
    #
    # ASSUMPTION, recorded deliberately: a host build and an image build of the SAME source
    # produce the same chunk file names. Measured true here — Turbopack derives them from content —
    # but nothing enforces it. Four things would break it, and all four are toolchain events rather
    # than code changes: a Next or Turbopack bump that changes chunk hashing; a compile-time
    # variable that differs between the host build and the builder stage; the platform split
    # (macOS host against a linux builder; it does not apply in images.yml, where both builds run
    # on the same runner); and the Dockerfile's filtered `pnpm install` resolving a different
    # optional platform dependency. When it breaks it breaks on EVERY run, loudly, into the branch
    # below whose message already says this is not a Dockerfile-vs-moon.yml drift. That is an
    # acceptable failure shape, so there is no shape-only fallback here on purpose.
    img_dirs="$(printf '%s\n' "$img_list" | sed 's#/.*##' | LC_ALL=C sort -u)"
    host_dirs="$(printf '%s\n' "$host_list" | sed 's#/.*##' | LC_ALL=C sort -u)"
    if [ "$img_dirs" != "$host_dirs" ]; then
      echo "::error::${app}: the image's staged .next/static holds different top-level directories from the host build's — ts/Dockerfile and ts/apps/${app}/moon.yml have drifted. Diff (< host, > image) follows." >&2
    elif [ "$parity" = required ]; then
      # images.yml: the host build is fresh and from the same commit as the image, so "re-run the
      # host build" is wrong advice here.
      echo "::error::${app}: the image's staged .next/static holds the same directories as the host build's but different files. Both builds come from the same commit in this run (CONSOLE_PARITY_REQUIRED=1), so this is most probably the chunk-name assumption failing (docs/ops/RUNBOOK-containers.md section 6), not a ts/Dockerfile vs ts/apps/${app}/moon.yml drift. Do NOT delete .next to silence it; the RUNBOOK names the rollback. Diff (< host, > image) follows." >&2
    else
      echo "::error::${app}: the image's staged .next/static holds the same directories as the host build's but different files, so the two were built from DIFFERENT sources. Re-run 'moon run ${app}-ts:build' so this parity claim compares like with like; on its own this is NOT a ts/Dockerfile vs ts/apps/${app}/moon.yml drift. If a FRESH build does not clear this, that is a finding — report it; do NOT delete .next to silence it, because that only moves this check into its 'not checked' arm. Diff (< host, > image) follows." >&2
    fi
    diff <(printf '%s\n' "$host_list") <(printf '%s\n' "$img_list") >&2 || true
    rc=1
  elif [ "$rc" -eq 0 ]; then
    echo "  ${app}: staged tree matches the host build ($(printf '%s\n' "$img_list" | grep -c . || true) files, ${img_public})"
  fi
  return "$rc"
}

```

Behaviour note: the old block printed "staged tree matches" even after a `public=` mismatch had
set `ec=1`. The new `elif [ "$rc" -eq 0 ]` prints it only when nothing failed. SP4d proves the
failure. No line of the function may be exactly `}` except the last one (the harness copies up
to the first such line); the JS above has none.

- [ ] **Step 4: Replace the block in `smoke_consoles`**

Replace everything from the line
`    # Spec § 5.5 assertion 4 — staged-tree parity. ts/Dockerfile's staging of .next/static and`
through the `    fi` that closes the `if [ -d "$host_static" ]; then … else … fi` block (just before
the `  done` of the zone loop) with:

```bash
    # Spec § 5.5 assertion 4 — staged-tree parity (console_staged_parity_row, SMA-671). The mode
    # comes from this function's --parity= word.
    console_staged_parity_row "$app" "$image" "$parity" || ec=1
```

Then trim the `local` lines of `smoke_consoles`:
- `local run_out img_out img_public img_list host_public host_list img_dirs host_dirs` becomes
  `local run_out`
- `local host_std host_static host_id run_rc sh_rc img_rc cstate` becomes
  `local run_rc sh_rc cstate`

Check that no other line of `smoke_consoles` reads a removed name:

```bash
awk '/^smoke_consoles\(\) \{/,/^\}$/' ci/images/run.sh \
  | grep -n -E '\b(img_out|img_public|img_list|host_public|host_list|img_dirs|host_dirs|host_std|host_static|host_id|img_rc|CONSOLE_STAGED_TREE_JS)\b' || echo "none"
grep -n 'CONSOLE_STAGED_TREE_JS' ci/images/run.sh ci/images/console-selftest.sh || echo "none"
```
Expected: `none` twice.

- [ ] **Step 5: Run the self-test and see it pass**

Run: `cd <worktree> && bash -n ci/images/run.sh && /bin/bash ci/images/console-selftest.sh; echo "rc=$?"`
Expected: rc 0. Rows SP1-SP5d, their `-out` rows, P1d, P1d-mut, Z1-parity-*, Z2-parity-* print `PASS`.

- [ ] **Step 6: Commit**

```bash
git add ci/images/run.sh ci/images/console-selftest.sh
git commit -m "feat(ci): make staged-tree parity a row that can require a host build (SMA-671)

console_staged_parity_row is the parity block of smoke_consoles,
extracted with its own locals and its JS program. With --parity=required
a missing host build is an error, and the same-directories message names
the chunk-name assumption. Rows SP1-SP5d and pin P1d.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: The Docker-context check (`docker_context_leaks`, AC 9)

**Files:**
- Modify: `ci/images/run.sh` (a new function after `console_staged_parity_row`; a new
  `context-check)` dispatch arm before `  *)`; the `USAGE` string; the header usage lines)
- Modify: `ts/.dockerignore` ONLY if Task 1 M3 found an ignored or untracked path under `ts/`
- Test: `ci/images/console-selftest.sh` (`FUNCS`; new CX rows after the SP rows)

**Interfaces:**
- Produces: `docker_context_leaks` (no arguments; reads the global `ROOT`; rc 0 and one stdout
  line `  docker context: no host artifact …` when clean; rc 1 with one `::error::docker context: '<path>' …`
  line per leak, or a `docker context NOT checked` error).
- Produces: `ci/images/run.sh context-check` (takes no argument). Task 6 calls it in the workflow.

- [ ] **Step 1: Write the failing rows**

Append ` docker_context_leaks` to `FUNCS`. After the P1d pin (end of the SP rows), add:

```bash

# --- SMA-671: docker_context_leaks (CX rows) ---------------------------------------------------
# A fixture git repository with one commit. It commits, so it follows ci/CLAUDE.md (SMA-708,
# SMA-714): no maintenance, no gc, no signing, no global or system config.
cx_git() { GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 git -C "$FX_ROOT" "$@"; }
# cx_fx <name> [<dockerignore-file>] — the fixture, with ts/.dockerignore copied from the real one
# (or from <dockerignore-file>), a root .gitignore like the repo's, two tracked files under
# ts/apps/a and one under rs/crates/bindings/b. Call it AFTER stub_reset; FX_ROOT points at it.
cx_fx() {
  FX_ROOT="$T/fx-cx-$1"
  rm -rf "$FX_ROOT"
  mkdir -p "$FX_ROOT/ts/apps/a" "$FX_ROOT/rs/crates/bindings/b"
  cp "${2:-$REPO/ts/.dockerignore}" "$FX_ROOT/ts/.dockerignore"
  printf '%s\n' 'node_modules/' '.next/' '*.tsbuildinfo' > "$FX_ROOT/.gitignore"
  printf '%s\n' 'export {};' > "$FX_ROOT/ts/apps/a/page.ts"
  printf '%s\n' '/// <reference types="next" />' > "$FX_ROOT/ts/apps/a/next-env.d.ts"
  printf '%s\n' 'module.exports = {};' > "$FX_ROOT/rs/crates/bindings/b/index.js"
  cx_git init -q || infra "git init failed for the CX fixture"
  cx_git config maintenance.auto false
  cx_git config gc.auto 0
  cx_git config commit.gpgsign false
  cx_git config tag.gpgsign false
  cx_git add -A || infra "git add failed for the CX fixture"
  cx_git -c user.name=selftest -c user.email=selftest@example.invalid commit -q -m fixture \
    || infra "the CX fixture commit failed"
}
# What a host build leaves that ts/.dockerignore DOES exclude.
cx_excluded() {
  mkdir -p "$FX_ROOT/ts/node_modules/p" "$FX_ROOT/ts/apps/a/node_modules/q" "$FX_ROOT/ts/apps/a/.next/static"
  : > "$FX_ROOT/ts/node_modules/p/index.js"
  : > "$FX_ROOT/ts/apps/a/node_modules/q/index.js"
  : > "$FX_ROOT/ts/apps/a/.next/static/x.js"
  : > "$FX_ROOT/ts/apps/a/.env.local"
}

stub_reset; cx_fx 0; cx_excluded
run_fn CX0 0 "" "::error::" none docker_context_leaks
expect_in CX0-out "$T/CX0.out" "docker context: no host artifact"
stub_reset; cx_fx 1; cx_excluded; : > "$FX_ROOT/ts/apps/a/tsconfig.tsbuildinfo"
run_fn CX1 1 "docker context: 'ts/apps/a/tsconfig.tsbuildinfo' (git status '!!')" "node_modules" none docker_context_leaks
stub_reset; cx_fx 2; : > "$FX_ROOT/ts/apps/a/new.txt"
run_fn CX2 1 "docker context: 'ts/apps/a/new.txt' (git status '??')" "" none docker_context_leaks
# Review Focus 3: a tracked file that the build rewrites is a leak too.
stub_reset; cx_fx 3; printf '%s\n' '// changed' >> "$FX_ROOT/ts/apps/a/next-env.d.ts"
run_fn CX3 1 "docker context: 'ts/apps/a/next-env.d.ts' (git status ' M')" "" none docker_context_leaks
stub_reset; cx_fx 4; : > "$FX_ROOT/rs/crates/bindings/b/x.node"
run_fn CX4 1 "docker context: 'rs/crates/bindings/b/x.node'" "" none docker_context_leaks
printf '%s\n' '**/node_modules' '!keep' > "$T/cx5-ignore"
stub_reset; cx_fx 5 "$T/cx5-ignore"
run_fn CX5 1 "cannot read the pattern '!keep'" "" none docker_context_leaks
printf '%s\n' '**/node_modules' 'apps/a/x' > "$T/cx5b-ignore"
stub_reset; cx_fx 5b "$T/cx5b-ignore"
run_fn CX5b 1 "cannot read the pattern 'apps/a/x'" "" none docker_context_leaks
# A pattern without **/ excludes the top level only, as in Docker.
printf '%s\n' 'node_modules' '**/.next' > "$T/cx6-ignore"
stub_reset; cx_fx 6 "$T/cx6-ignore"; cx_excluded
run_fn CX6 1 "docker context: 'ts/apps/a/node_modules/'" "'ts/node_modules/'" none docker_context_leaks
# Review Focus 3: a path with a space is named whole (-z output, no quoting).
stub_reset; cx_fx 7; : > "$FX_ROOT/ts/apps/a/my file.txt"
run_fn CX7 1 "docker context: 'ts/apps/a/my file.txt'" "" none docker_context_leaks
cx_nogit() { export GIT_CEILING_DIRECTORIES="$T"; docker_context_leaks; }
stub_reset; FX_ROOT="$T/fx-cx-nogit"; mkdir -p "$FX_ROOT/ts"; cp "$REPO/ts/.dockerignore" "$FX_ROOT/ts/.dockerignore"
run_fn CX8 1 "docker context NOT checked — git status exited" "" none cx_nogit
```

Note on CX6: `.env.local` is a leak there too (the fixture ignore file has no `.env` line), so the
row checks only the `node_modules` path it is about.

- [ ] **Step 2: Run the self-test and see it fail**

Run: `cd <worktree> && /bin/bash ci/images/console-selftest.sh; echo "rc=$?"`
Expected: rc 2 and `no 'docker_context_leaks() {' line in ci/images/run.sh`.

- [ ] **Step 3: Add the function**

In `ci/images/run.sh`, directly after the closing `}` of `console_staged_parity_row`, add:

```bash
# SMA-671 (spec § 4.2, AC 9): the host build of images.yml runs in the checkout that the later image
# builds use as their Docker context. .dockerignore filters a context, .gitignore does not, and
# release.yml builds from a clean checkout. So a host artifact that ts/.dockerignore does not
# exclude, a tracked file that the build rewrote, and any new file under rs/crates/bindings (the
# named context `bindings`, which has no .dockerignore) would give the two workflows different
# image inputs. This prints one ::error:: per such path and returns 1. It understands only the two
# pattern forms that ts/.dockerignore uses: `<name>` (the top level of ts/) and `**/<name>` (any
# depth), where <name> holds no `/` and may hold a glob. It refuses any other form (a `!`
# exception, a `/` inside the name), fail closed. `git status --ignored=matching` lists an ignored
# directory once, not each file in it; `-z` keeps a path with a space whole.
docker_context_leaks() {
  local ignore="$ROOT/ts/.dockerignore" line name n=0 i st st_rc=0 rec xy path rel rest comp first excluded leaks=0
  local -a pat_any pat_name
  if [ ! -r "$ignore" ]; then
    echo "::error::docker context NOT checked — ${ignore} is not readable." >&2
    return 1
  fi
  while IFS= read -r line || [ -n "$line" ]; do
    case "$line" in
      ''|'#'*) continue ;;
    esac
    case "$line" in
      '**/'*) name="${line#\*\*/}"; pat_any[$n]=1 ;;
      *) name="$line"; pat_any[$n]=0 ;;
    esac
    case "$name" in
      ''|'!'*|*/*|*' '*)
        echo "::error::docker context NOT checked — cannot read the pattern '${line}' in ts/.dockerignore. docker_context_leaks in ci/images/run.sh knows only '<name>' and '**/<name>', where <name> holds no '/'. Extend it for the new form." >&2
        return 1
        ;;
    esac
    pat_name[$n]="$name"
    n=$((n + 1))
  done < "$ignore"
  st="$(mktemp "${TMPDIR:-/tmp}/paigasus-context.XXXXXX")" || st=""
  if [ -z "$st" ]; then
    echo "::error::docker context NOT checked — mktemp failed." >&2
    return 1
  fi
  git -C "$ROOT" status --porcelain=v1 -z --ignored=matching --untracked-files=normal \
    -- ts rs/crates/bindings > "$st" 2> "$st.err" || st_rc=$?
  if [ "$st_rc" -ne 0 ]; then
    echo "::error::docker context NOT checked — git status exited ${st_rc} in ${ROOT}. Its message follows." >&2
    cat "$st.err" >&2 || true
    rm -f "$st" "$st.err"
    return 1
  fi
  while IFS= read -r -d '' rec; do
    xy="${rec:0:2}"
    path="${rec:3}"
    case "$xy" in
      # A rename or a copy has its source path as the next record.
      R*|C*) IFS= read -r -d '' rest || true ;;
    esac
    excluded=0
    case "$path" in
      ts/*)
        rel="${path#ts/}"
        rel="${rel%/}"
        rest="$rel"
        first=1
        while [ -n "$rest" ] && [ "$excluded" -eq 0 ]; do
          comp="${rest%%/*}"
          i=0
          while [ "$i" -lt "$n" ]; do
            if [ "${pat_any[$i]}" -eq 1 ] || [ "$first" -eq 1 ]; then
              # shellcheck disable=SC2254 # a glob on purpose, as in .dockerignore
              case "$comp" in ${pat_name[$i]}) excluded=1 ;; esac
            fi
            i=$((i + 1))
          done
          if [ "$comp" = "$rest" ]; then rest=""; else rest="${rest#*/}"; fi
          first=0
        done
        ;;
    esac
    if [ "$excluded" -eq 0 ]; then
      echo "::error::docker context: '${path}' (git status '${xy}') is in the image build context of images.yml, but not of release.yml, which builds from a clean checkout." >&2
      leaks=$((leaks + 1))
    fi
  done < "$st"
  rm -f "$st" "$st.err"
  if [ "$leaks" -ne 0 ]; then
    echo "::error::docker context: ${leaks} path(s) above. ts/.dockerignore does not exclude them, or they are under rs/crates/bindings (the named context 'bindings', which has no .dockerignore). Add the pattern to ts/.dockerignore, or stop the host build from writing the file." >&2
    return 1
  fi
  echo "  docker context: no host artifact outside ts/.dockerignore under ts/, and no new or changed file under rs/crates/bindings"
  return 0
}
```

- [ ] **Step 4: Add the dispatch arm and the usage text**

Before the `  *)` arm of the final `case "$cmd" in`, add:

```bash
  context-check)
    # SMA-671: images.yml runs this after the host build of both consoles.
    if [ -n "$target" ]; then
      echo "usage: ci/images/run.sh context-check takes no argument" >&2
      exit 1
    fi
    docker_context_leaks
    ;;
```

Append ` | ci/images/run.sh context-check` to the `USAGE=` string (before its closing `"`). In the
header usage block, after the `all-consoles` line, add:

```bash
#        ci/images/run.sh context-check                   # SMA-671: fail on a host artifact in a Docker build context
```

- [ ] **Step 5: Run the self-test and see it pass**

Run: `cd <worktree> && bash -n ci/images/run.sh && /bin/bash ci/images/console-selftest.sh; echo "rc=$?"`
Expected: rc 0. Rows CX0-CX8 and CX0-out print `PASS`.

- [ ] **Step 6: Run the check on this worktree (a smoke test, not a gate)**

Run: `cd <worktree> && ci/images/run.sh context-check; echo "rc=$?"`
Expected: rc 1 is normal on this Mac (local `tsconfig.tsbuildinfo`, `.wasmpack-test-out`, `.node`
files). Read the list: each line must name a real path, and `node_modules`, `.next` and `.env*`
paths must NOT appear.

- [ ] **Step 7: Apply the Task 1 M3 decision**

If Task 1 M3 listed an ignored or untracked path under `ts/` (for example
`ts/apps/iam-console/tsconfig.tsbuildinfo`), add one pattern per artifact kind to `ts/.dockerignore`,
after `**/playwright-report`, with a comment:

```text
# SMA-671: images.yml makes a host build of both consoles in the checkout that the image builds
# then use as their context. Next writes this file during that build; release.yml never has it.
**/*.tsbuildinfo
```

Then add a CX row that proves the real file now excludes it:

```bash
stub_reset; cx_fx 9; : > "$FX_ROOT/ts/apps/a/tsconfig.tsbuildinfo"
run_fn CX9 0 "" "::error::" none docker_context_leaks
```

(CX1 keeps its own ignore file in that case: change its `cx_fx 1` to
`cx_fx 1 "$T/cx1-ignore"` with `printf '%s\n' '**/node_modules' '**/.next' '**/.env' '**/.env.*' > "$T/cx1-ignore"`
before it, so CX1 still proves a leak.) If M3 was empty, skip this step and record "M3 empty, no
.dockerignore change" in the task report.

- [ ] **Step 8: Commit**

```bash
git add ci/images/run.sh ci/images/console-selftest.sh ts/.dockerignore
git commit -m "feat(ci): fail images.yml when a host artifact enters a Docker context (SMA-671)

docker_context_leaks reads ts/.dockerignore and git status --ignored
and names each path that the image builds of images.yml would see but
the clean checkout of release.yml would not. Rows CX0-CX8.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

(`git add ts/.dockerignore` is a no-op when Step 7 was skipped.)

---

### Task 6: Gate parity in `images.yml` and pin it (W1, RS rows)

**Files:**
- Modify: `.github/workflows/images.yml` (the host-build step from Task 1; "Console release
  sequence"; "Build + smoke both consoles"; their comments)
- Test: `ci/images/console-selftest.sh` (new W1 and RS rows before the `# --- summary` line)

**Interfaces:**
- Consumes: `ci/images/run.sh context-check` (Task 5); `--parity=required` through
  `CONSOLE_PARITY_REQUIRED: '1'` (Task 3); the step name and `moon run iam-console-ts:build` text
  (Task 1).
- Produces: functions `w1_scan`, `w1_check`, `w1_mut`, `rs_case` inside the harness only.

- [ ] **Step 1: Write the failing rows**

In `console-selftest.sh`, directly before `# --- summary ---`, add:

```bash
# --- SMA-671: the workflow pin (W1 rows) ------------------------------------------------------
# W1 keeps the production switch in place (guard-the-guard). It reads images.yml with awk (no YAML
# library) and fails unless each CONSOLE smoke step sets CONSOLE_PARITY_REQUIRED: '1' in its own
# step env, nothing else sets the variable, and the host-build step comes first. A console smoke
# step is a step whose run: calls `ci/images/run.sh all-consoles`, or `ci/images/run.sh smoke`
# with a first argument other than the cargo keys iam and gateway.
W1_WF="$REPO/.github/workflows/images.yml"

# w1_scan <workflow> — one line per step of the steps: list, in file order:
# <index> TAB <console|host|other> TAB <value of CONSOLE_PARITY_REQUIRED in the step env, or -> TAB <name>
# and a last line `stray TAB <n>`: the CONSOLE_PARITY_REQUIRED key lines outside a step env.
w1_scan() {
  awk -v q="'" '
    function flush() {
      if (n > 0) printf "%d\t%s\t%s\t%s\n", n, (console ? "console" : (host ? "host" : "other")), par, name
    }
    {
      line = $0
      if (match(line, /[^ ]/)) { ind = RSTART - 1; body = substr(line, RSTART) } else { ind = -1; body = "" }
      comment = (substr(body, 1, 1) == "#")
    }
    line == "    steps:" { insteps = 1; next }
    insteps && ind == 6 && substr(body, 1, 2) == "- " {
      flush(); n++; console = 0; host = 0; par = "-"; inenv = 0; name = ""
      if (substr(body, 1, 8) == "- name: ") name = substr(body, 9)
      next
    }
    inenv && ind >= 0 && ind < 10 { inenv = 0 }
    n > 0 && ind == 8 && name == "" && substr(body, 1, 6) == "name: " { name = substr(body, 7) }
    n > 0 && ind == 8 && body == "env:" { inenv = 1; next }
    !comment && substr(body, 1, 24) == "CONSOLE_PARITY_REQUIRED:" {
      if (inenv) { v = substr(body, 25); sub(/^ +/, "", v); sub(/ +$/, "", v); par = v } else stray++
      next
    }
    n > 0 && !comment {
      if (index(line, "ci/images/run.sh all-consoles")) console = 1
      if (index(line, "moon run iam-console-ts:build")) host = 1
      p = index(line, "ci/images/run.sh smoke")
      if (p) {
        rest = substr(line, p + 22)
        if (rest == "" || substr(rest, 1, 1) == " ") {
          sub(/^ +/, "", rest); w = rest; sub(/ .*/, "", w); gsub("[\"" q "]", "", w)
          if (w != "" && w != "iam" && w != "gateway") console = 1
        }
      }
    }
    END { flush(); printf "stray\t%d\t-\t-\n", stray + 0 }
  ' "$1"
}

# w1_check <workflow> — rc 0 when the rules above hold; one `W1:` line on stderr per defect.
w1_check() {
  local wf="$1" rc=0 idx kind par name first_idx="" first_name="" host_idx="" host_name="" n_console=0 tab
  tab="$(printf '\t')"
  w1_scan "$wf" > "$T/w1-scan" || { echo "W1: awk could not read ${wf}" >&2; return 1; }
  while IFS="$tab" read -r idx kind par name; do
    if [ "$idx" = stray ]; then
      if [ "$kind" -ne 0 ]; then
        echo "W1: ${kind} CONSOLE_PARITY_REQUIRED line(s) outside a step env (a job or workflow env, or a run body); the Console image self-test step must not inherit the variable." >&2
        rc=1
      fi
      continue
    fi
    if [ "$kind" = console ]; then
      n_console=$((n_console + 1))
      if [ -z "$first_idx" ]; then first_idx="$idx"; first_name="$name"; fi
      if [ "$par" = "-" ]; then
        echo "W1: step '${name}' runs a console smoke, but its own step env has no CONSOLE_PARITY_REQUIRED line." >&2
        rc=1
      elif [ "$par" != "'1'" ]; then
        echo "W1: step '${name}' sets CONSOLE_PARITY_REQUIRED: ${par}; the only accepted line is CONSOLE_PARITY_REQUIRED: '1'." >&2
        rc=1
      fi
    else
      if [ "$kind" = host ] && [ -z "$host_idx" ]; then host_idx="$idx"; host_name="$name"; fi
      if [ "$par" != "-" ]; then
        echo "W1: step '${name}' sets CONSOLE_PARITY_REQUIRED, but it runs no console smoke." >&2
        rc=1
      fi
    fi
  done < "$T/w1-scan"
  if [ "$n_console" -eq 0 ]; then
    echo "W1: found no console smoke step in ${wf##*/} (a step that runs 'ci/images/run.sh all-consoles' or 'ci/images/run.sh smoke <console-key>')." >&2
    return 1
  fi
  if [ -z "$host_idx" ]; then
    echo "W1: no host-build step in ${wf##*/} (no step holds 'moon run iam-console-ts:build')." >&2
    rc=1
  elif [ "$host_idx" -gt "$first_idx" ]; then
    echo "W1: the host-build step '${host_name}' comes after the console smoke step '${first_name}'." >&2
    rc=1
  fi
  if [ "$rc" -eq 0 ]; then
    echo "W1: ${n_console} console smoke step(s) set CONSOLE_PARITY_REQUIRED: '1', after the host-build step '${host_name}'."
  fi
  return "$rc"
}

# w1_mut <row> <awk-program> — images.yml through the program, into $T/<row>.yml. The program sees
# `step` (the name of the current step) and q (a single quote). A copy equal to the original is a
# FAIL row: the mutation did not apply.
w1_mut() {
  awk -v q="'" '/^      - name: / { step = substr($0, 15) } '"$2" "$W1_WF" > "$T/$1.yml" \
    || infra "awk could not mutate images.yml for $1"
  if cmp -s "$W1_WF" "$T/$1.yml"; then say_fail "$1-applied" "the mutation did not apply"; else say_pass "$1-applied"; fi
}

stub_reset
run_fn W1 0 "" "W1:" none w1_check "$W1_WF"
expect_in W1-out "$T/W1.out" "W1: 2 console smoke step(s) set CONSOLE_PARITY_REQUIRED: '1'"
# m1: the env line deleted.
w1_mut W1-m1 '{ l = $0; sub(/^ +/, "", l) } step == "Console release sequence" && l == "CONSOLE_PARITY_REQUIRED: " q "1" q { next } { print }'
stub_reset
run_fn W1-m1 1 "step 'Console release sequence' runs a console smoke, but its own step env has no CONSOLE_PARITY_REQUIRED line" "" none w1_check "$T/W1-m1.yml"
# m2: the env line moved to the cargo smoke step.
w1_mut W1-m2 '{ l = $0; sub(/^ +/, "", l) } step == "Build + smoke both consoles" && l == "CONSOLE_PARITY_REQUIRED: " q "1" q { next } { print } $0 == "      - name: Smoke each service on its own" { print "        env:"; print "          CONSOLE_PARITY_REQUIRED: " q "1" q }'
stub_reset
run_fn W1-m2 1 "step 'Build + smoke both consoles' runs a console smoke, but its own step env has no|step 'Smoke each service on its own' sets CONSOLE_PARITY_REQUIRED, but it runs no console smoke" "" none w1_check "$T/W1-m2.yml"
# m3: '1' changed to true.
w1_mut W1-m3 '{ l = $0; sub(/^ +/, "", l) } step == "Console release sequence" && l == "CONSOLE_PARITY_REQUIRED: " q "1" q { sub(q "1" q, "true") } { print }'
stub_reset
run_fn W1-m3 1 "step 'Console release sequence' sets CONSOLE_PARITY_REQUIRED: true; the only accepted line" "" none w1_check "$T/W1-m3.yml"
# m4: the host-build step moved after the release sequence.
w1_mut W1-m4 '/^      [-#]/ { if (inrel && hold != "") { printf "%s", hold; hold = "" } inrel = 0; inhost = 0 } /^      - name: Host build of both consoles/ { inhost = 1 } inhost { hold = hold $0 "\n"; next } { print } /^      - name: Console release sequence$/ { inrel = 1 }'
stub_reset
run_fn W1-m4 1 "the host-build step 'Host build of both consoles (staged-tree parity reference)' comes after the console smoke step 'Console release sequence'" "" none w1_check "$T/W1-m4.yml"
# m5: the cargo smoke step turned into a console smoke step.
w1_mut W1-m5 '/ci\/images\/run\.sh smoke iam$/ { sub(/smoke iam$/, "smoke iam-console") } { print }'
stub_reset
run_fn W1-m5 1 "step 'Smoke each service on its own' runs a console smoke, but its own step env has no" "" none w1_check "$T/W1-m5.yml"
# m6 (Review Focus 4): the variable at job level, where the self-test step would inherit it.
w1_mut W1-m6 '{ print } $0 == "      PROTO_REPORTER: text" { print "      CONSOLE_PARITY_REQUIRED: " q "1" q }'
stub_reset
run_fn W1-m6 1 "1 CONSOLE_PARITY_REQUIRED line(s) outside a step env" "" none w1_check "$T/W1-m6.yml"
# m7 (Review Focus 4): the env line turned into a comment.
w1_mut W1-m7 '{ l = $0; sub(/^ +/, "", l) } step == "Console release sequence" && l == "CONSOLE_PARITY_REQUIRED: " q "1" q { print "          # CONSOLE_PARITY_REQUIRED: " q "1" q; next } { print }'
stub_reset
run_fn W1-m7 1 "step 'Console release sequence' runs a console smoke, but its own step env has no CONSOLE_PARITY_REQUIRED line" "" none w1_check "$T/W1-m7.yml"
# m8: the host-build step deleted.
w1_mut W1-m8 '/^      [-#]/ { inhost = 0 } /^      - name: Host build of both consoles/ { inhost = 1 } inhost { next } { print }'
stub_reset
run_fn W1-m8 1 "W1: no host-build step in W1-m8.yml" "" none w1_check "$T/W1-m8.yml"
# m9: no console smoke step at all must not read as a pass.
printf '%s\n' 'jobs:' > "$T/W1-m9.yml"
stub_reset
run_fn W1-m9 1 "W1: found no console smoke step in W1-m9.yml" "" none w1_check "$T/W1-m9.yml"

# --- SMA-671: the "Console release sequence" loop (RS rows, Review Focus 2) -------------------
# The step's real run: body, copied out of images.yml, runs under GitHub's own shell flags against
# a stub ci/images/run.sh, syft and uv. A failure on iam-console must still run gateway-console,
# stop the rest of iam-console, and fail the step.
awk 'index($0, "      - name: Console release sequence") == 1 { on = 1; next }
     on && /^        run: \|/ { body = 1; next }
     on && body && /^          / { print substr($0, 11); next }
     on && body && /^ *$/ { print ""; next }
     on && body { exit }
     on && /^      [-#]/ { exit }' "$W1_WF" > "$T/rs-body.sh" || infra "awk could not read images.yml"
[ -s "$T/rs-body.sh" ] || infra "no run: body for 'Console release sequence' in images.yml"
mkdir -p "$T/rs-work/ci/images" "$T/rs-bin"
# shellcheck disable=SC2016 # the stub lines are written literally
printf '%s\n' '#!/usr/bin/env bash' 'printf "%s\n" "$*" >> "$RS_LOG"' \
  'if [ "${1:-}" = "$RS_FAIL_CMD" ] && [ "${2:-}" = "$RS_FAIL_KEY" ]; then exit 1; fi' 'exit 0' \
  > "$T/rs-work/ci/images/run.sh"
printf '%s\n' '#!/usr/bin/env bash' 'exit 0' > "$T/rs-bin/syft"
printf '%s\n' '#!/usr/bin/env bash' 'echo "packages=1"' 'exit 0' > "$T/rs-bin/uv"
chmod +x "$T/rs-work/ci/images/run.sh" "$T/rs-bin/syft" "$T/rs-bin/uv"
# rs_case <fail-cmd> <fail-key> <log-name> — the body, with run.sh failing on that one call.
rs_case() {
  RS_FAIL_CMD="$1"; RS_FAIL_KEY="$2"; RS_LOG="$T/rs-log-$3"
  rm -f "$RS_LOG"
  export RS_FAIL_CMD RS_FAIL_KEY RS_LOG
  ( cd "$T/rs-work" && PATH="$T/rs-bin:$PATH" ARCH=amd64 "$BASH" --noprofile --norc -eo pipefail "$T/rs-body.sh" )
}
stub_reset
run_fn RS1 1 "Console release sequence: iam-console failed (rc 1)" "" any rs_case build-oci iam-console RS1
expect_in RS1-gw-build "$T/rs-log-RS1" "build-oci gateway-console out"
expect_in RS1-gw-smoke "$T/rs-log-RS1" "smoke gateway-console"
expect_not_in RS1-iam-stopped "$T/rs-log-RS1" "smoke iam-console"
stub_reset
run_fn RS2 0 "" "::error::" any rs_case none none RS2
expect_in RS2-iam "$T/rs-log-RS2" "smoke iam-console"
expect_in RS2-gw "$T/rs-log-RS2" "smoke gateway-console"
stub_reset
run_fn RS3 1 "Console release sequence: iam-console failed (rc 1)" "" any rs_case smoke iam-console RS3
expect_in RS3-gw "$T/rs-log-RS3" "smoke gateway-console"
```

- [ ] **Step 2: Run the self-test and see it fail**

Run: `cd <worktree> && /bin/bash ci/images/console-selftest.sh; echo "rc=$?"`
Expected: rc 1. FAIL rows: `W1` (no env lines yet), `W1-out`, `RS1` (the current loop stops at
the first failure, so `RS1-gw-build` and `RS1-gw-smoke` fail too), `RS3`, `RS3-gw`. The `-applied`
rows of m1, m2, m3 and m7 FAIL too (no env line to mutate yet). Rows m4, m5, m6, m8, m9 may pass.

- [ ] **Step 3: Change the workflow**

(a) The host-build step: replace its last line
`          git status --porcelain=v1 --ignored=matching --untracked-files=normal -- ts rs/crates/bindings`
with:

```yaml
          # AC 9: no host artifact may enter a Docker build context of the image builds below.
          ci/images/run.sh context-check
```

Set its `timeout-minutes` to the value that Task 1 Step 8 computed. Add to the end of its comment
block: `# Row W1 in ci/images/console-selftest.sh fails when this step moves after a console smoke step.`

(b) Replace the whole "Console release sequence" step (its `- name:` line through `          df -h /`
before the blank line) with:

```yaml
      - name: Console release sequence
        # SMA-671: the parity row needs the host build above. Step env, not job env: the self-test
        # step must not inherit it. Row W1 pins this line. Quoted, as release.yml quotes 'off'.
        env:
          CONSOLE_PARITY_REQUIRED: '1'
        run: |
          df -h /
          rc=0
          for key in iam-console gateway-console; do
            # One subshell per key under errexit, OUTSIDE any `||` list: bash ignores `set -e` in a
            # subshell on the left of `||`, so `( set -e; … ) || rc=$?` would run every command of
            # a failed key. `set +e` around it keeps this shell alive instead, so a failure on
            # iam-console still runs gateway-console (SMA-671 AC 10; rows RS1-RS3).
            set +e
            (
              set -euo pipefail
              ci/images/run.sh build-oci "$key" out
              archive="out/paigasus-${key}-${ARCH}.oci.tar"
              ci/images/run.sh load-oci "$archive" "paigasus-${key}:dev"
              ci/images/run.sh smoke "$key"
              syft "oci-archive:${archive}" -o "spdx-json=sbom-paigasus-${key}-${ARCH}.spdx.json"
              summary="$(uv run --no-project --python '>=3.12' python3 ci/images/release_decision.py sbom-summary "sbom-paigasus-${key}-${ARCH}.spdx.json")"
              echo "M8 image=paigasus-${key} arch=${ARCH} $(printf '%s' "$summary" | tr '\n' ' ')"
              uv run --no-project --python '>=3.12' python3 ci/images/release_decision.py sbom-floor --service "$key" "sbom-paigasus-${key}-${ARCH}.spdx.json"
            )
            key_rc=$?
            set -e
            if [ "$key_rc" -ne 0 ]; then
              echo "::error::Console release sequence: ${key} failed (rc ${key_rc}); the next key still runs." >&2
              rc=1
            fi
          done
          df -h /
          exit "$rc"
```

(c) In "Build + smoke both consoles", add between the `- name:` line and `run: |`:

```yaml
        # SMA-671: see "Console release sequence". Row W1 pins this line.
        env:
          CONSOLE_PARITY_REQUIRED: '1'
```

(d) Comments. In the comment block above "Console release sequence", replace the paragraph that
starts `# The "Build + smoke both consoles" step below still runs.` with:

```yaml
      # The "Build + smoke both consoles" step below still runs. It smokes the build-console path
      # only. SMA-671: both console smoke steps set CONSOLE_PARITY_REQUIRED, so the staged-tree
      # parity row fails when the host build above is missing or differs from the image.
```

In the comment above "Console image self-test", no change. Do not change `branches:` or `paths:`.

- [ ] **Step 4: Run the self-test and see it pass**

Run: `cd <worktree> && /bin/bash ci/images/console-selftest.sh; echo "rc=$?"`
Expected: rc 0. W1, W1-out, every `W1-mN-applied` and `W1-mN`, RS1-RS3 and their checks print `PASS`.

- [ ] **Step 5: Lint the workflow**

Run: `cd <worktree> && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && actionlint .github/workflows/images.yml`
Expected: no output, exit 0.

- [ ] **Step 6: Commit**

```bash
git add .github/workflows/images.yml ci/images/console-selftest.sh
git commit -m "ci(ci): require staged-tree parity in images.yml and pin it (SMA-671)

Both console smoke steps set CONSOLE_PARITY_REQUIRED: '1'. The release
sequence keeps an rc per key, so a failure on iam-console still runs
gateway-console. The host-build step ends with context-check. Row W1
pins the env lines and the step order; rows RS1-RS3 run the real loop.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

(Use `feat(ci):` if commitlint refuses the `ci` type.)

---

### Task 7: Documentation (RUNBOOK sections 1 and 6, `rs/CLAUDE.md`)

**Files:**
- Modify: `docs/ops/RUNBOOK-containers.md` (section 1 after line 79; section 6 lines 473-476,
  484-508)
- Modify: `rs/CLAUDE.md` ("Container images", lines 226-254)

**Interfaces:**
- Consumes: the names from Tasks 3-6 (`CONSOLE_PARITY_REQUIRED`, `console_staged_parity_row`,
  `context-check`, rows W1, SP, PF, CX, RS).

- [ ] **Step 1: RUNBOOK section 1**

After the paragraph that ends `… and the gate still reds a pattern that` / `matches no file.`
(line 79), add a new paragraph:

```markdown
`@paigasus/next-config` (`ts/packages/paigasus-next-config/**`) is NOT on the filter (SMA-671).
Its `canonicalBasePath` sets the compiled `basePath`, and `ts/Dockerfile` fails the build when that
value is different from the `BASE_PATH` build argument. But the package is a Moon `input` of the
console `build`, `typecheck`, `test` and `test-e2e` tasks, so the rule above excludes it. The
primary control is the ordinary build: `base-path.test.ts` and `next-config.test.ts` pin `/iam`,
the iam-console e2e specs use literal `/iam/...` paths, and the `standalone-runtime.test.ts` of
gateway-console starts the built server and requests `/gateway/healthz`. The second control is
`chart.yml`. It runs `ts/Dockerfile`, with the `basePath` check, on each PR that changes the
package. It reports a failed console build as an infrastructure error (rc 2).
```

- [ ] **Step 2: RUNBOOK section 6, the switch list**

After the bullet that ends `… The smoke removes the control container as soon as` /
`its row finishes.` (line 476), add:

```markdown
  - `CONSOLE_PARITY_REQUIRED` is the second switch of the console smoke (SMA-671). It has the
    opposite polarity: unset, empty or `0` means "not required", `1` means "required", and any
    other value is a usage error. Only the two console smoke steps of `images.yml` set it.
```

- [ ] **Step 3: RUNBOOK section 6, the parity facts**

Replace the bullet that starts `- **The check runs locally only.**` (lines 486-491) with:

```markdown
- **The check gates in `images.yml` (SMA-671).** It compares the staged `.next/static` of the
  image with a host build at `ts/apps/<app>/.next/standalone/apps/<app>/.next/static`. The
  `images` job makes that host build on each leg, before the console smoke:
  `moon run iam-console-ts:build gateway-console-ts:build --upstream none`. It sets
  `CONSOLE_PARITY_REQUIRED: '1'` on its two console smoke steps. With that value, a missing host
  build is an error, not a "NOT CHECKED" line. So the check gates on amd64 on each PR that the
  `images.yml` filter selects, and on amd64 and arm64 on `main` and on a manual dispatch.
- **The limit.** The check does NOT gate a PR that changes only `ts/apps/*/moon.yml`. That file is
  not on the `images.yml` filter (SMA-671 Q5), and `chart.yml`, which covers `ts/apps/*/**`, runs no
  host build and no smoke. So a staging drift on the `moon.yml` side shows first on the `main` push
  after the merge. `tests/standalone-staging.test.ts` catches only the coarse case (the staging
  removed). Do not read the check as "parity gates every PR".
- `release.yml` does not set the variable, so the release smoke prints "NOT CHECKED" on purpose:
  it has no host build. Locally, run `moon run <app>-ts:build` before the smoke, or the check
  prints "NOT CHECKED".
- Row W1 of `ci/images/console-selftest.sh` fails when a console smoke step of `images.yml` loses
  the `CONSOLE_PARITY_REQUIRED: '1'` line, gets another value, or runs before the host-build step.
- After the host build, the same step runs `ci/images/run.sh context-check`. It fails when the
  host build leaves a file under `ts/` that `ts/.dockerignore` does not exclude, changes a tracked
  file there, or adds any file under `rs/crates/bindings` (the named context `bindings`, which has
  no `.dockerignore`). Such a file would enter the image build context of `images.yml` but not of
  `release.yml`, which builds from a clean checkout.
```

- [ ] **Step 4: RUNBOOK section 6, the chunk-name bullet**

In the bullet that starts `- **Parity depends on an assumption:`, replace its last two sentences
(`When the assumption fails, it fails on every run. The failure goes to the parity error that` /
`already states that the mismatch is not a drift between \`ts/Dockerfile\` and \`moon.yml\`.`) with:

```markdown
  In `images.yml` both builds run on the same linux runner, so the third cause does not apply
  there. When the assumption fails, it fails on every run. In `images.yml` the error then says
  that both builds come from the same commit and names "the chunk-name assumption failing". Read it
  as that, not as a drift between `ts/Dockerfile` and `moon.yml`. Do not delete `.next` to silence
  it. The rollback is a reviewed PR that removes the two `CONSOLE_PARITY_REQUIRED` lines from
  `images.yml` AND changes row W1 in the same PR.
```

- [ ] **Step 5: RUNBOOK section 6, the self-test paragraph**

In the paragraph that starts `` `ci/images/console-selftest.sh` proves the checks of this section. ``,
after its first sentence, add: `Since SMA-671 it also holds the parity rows (SP, PF), the
Docker-context rows (CX), the workflow pin (W1) and the release-sequence rows (RS).`

- [ ] **Step 6: `rs/CLAUDE.md` "Container images"**

(a) In the first bullet, change `` `ci/images/run.sh {build,smoke,all,build-oci,load-oci,rehearse}` ``
to `` `ci/images/run.sh {build,smoke,all,build-oci,load-oci,rehearse,context-check}` ``.

(b) In the second bullet, replace the sentence
`It also lists \`ci/images/**\`, the` / `workflow, \`.prototools\` and the two \`.proto/plugins/*.toml\` files.`
with:

```markdown
It also lists four smoke-runtime files
  (SMA-675, the second clause of RUNBOOK-containers.md section 1):
  `ts/packages/paigasus-auth/src/core/session.ts`, `ts/apps/*/app/*console*/layout.tsx`,
  `ts/apps/iam-console/app/*console*/orgs/page.tsx` and
  `ts/apps/gateway-console/app/*console*/overview/page.tsx`. It also lists `ci/images/**`, the
  workflow, `.prototools` and the two `.proto/plugins/*.toml` files.
```

(c) At the end of the second bullet (after `… regenerates them fresh every run.`), add:

```markdown
  `@paigasus/next-config` is off the filter too (SMA-671): it is a Moon input, the unit tests and
  `standalone-runtime.test.ts` pin both base paths, and `chart.yml` is the second control.
```

(d) After the third bullet (the one that ends `` is on `main`.) ``), add a new bullet:

```markdown
- Staged-tree parity of the console images gates in `images.yml` only (SMA-671). The job makes a
  host build of both consoles on each leg and sets `CONSOLE_PARITY_REQUIRED: '1'` on its two
  console smoke steps; unset means "not required", so `release.yml` needs no host build. It gates
  amd64 on a filtered PR and both legs on `main`. A PR that changes only `ts/apps/*/moon.yml` does
  not run it, so a `moon.yml`-side staging drift shows first on `main`. Row W1 of
  `ci/images/console-selftest.sh` pins the two `env` lines and the step order.
```

- [ ] **Step 7: Check that the three filter lists agree (AC 6)**

```bash
cd <worktree>
awk '/^  pull_request:/,/^# Build-and-verify/' .github/workflows/images.yml | grep -o "'[^']*'" | tr -d "'" | sort > /private/tmp/claude-501/sma-671/f-yml.txt
while IFS= read -r p; do
  printf '%s runbook=%s claude=%s\n' "$p" "$(grep -c -F -- "$p" docs/ops/RUNBOOK-containers.md)" "$(grep -c -F -- "$p" rs/CLAUDE.md)"
done < /private/tmp/claude-501/sma-671/f-yml.txt
```

Expected: `main` is not in the list (the `branches:` entry has no quotes). Each listed pattern has
`runbook>=1` and `claude>=1`. The `rs/Cargo.{lock,toml}` form in `rs/CLAUDE.md` gives `claude=0`
for `rs/Cargo.lock` and `rs/Cargo.toml`; that is the same entry written short, so accept it. Any
other `0` is a gap: fix the doc.

- [ ] **Step 8: Check the moon-diagnosis marker rule**

Run: `cd <worktree> && grep -n 'ciReport' docs/ops/RUNBOOK-containers.md rs/CLAUDE.md || echo "none"`
Expected: `none` (no doc here names the moon CI report file, so no marker is needed). This plan
names the file in the command above, so `repo:actionlint` check 12 needs the
`moon-diagnosis:ok` marker on line 1 of this plan. Keep it there.

- [ ] **Step 9: Commit**

```bash
git add docs/ops/RUNBOOK-containers.md rs/CLAUDE.md
git commit -m "docs(ops): parity gates in images.yml; next-config stays off the filter (SMA-671)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: Mutation proof (spec §6, AC 4)

Each mutation is a temporary edit, a run that must go red on the named row, and a restore with the
Edit tool. Do not use `git checkout` to restore (it would also drop uncommitted work). Never commit
a mutation. Run `bash -n ci/images/run.sh` after each edit: a mutation that does not compile proves
nothing (memory `mutation-must-compile-to-prove-anything`).

**Files:**
- Temporarily modify: `ci/images/run.sh`, `.github/workflows/images.yml`
- Record: `docs/superpowers/specs/2026-10-01-sma-671-measurements.md` (a "Mutation proof" table)

- [ ] **Step 1: Confirm a clean tree and a green baseline**

Run: `cd <worktree> && git status --short && /bin/bash ci/images/console-selftest.sh > /private/tmp/claude-501/sma-671/mut-base.txt; echo "rc=$?"; tail -1 /private/tmp/claude-501/sma-671/mut-base.txt`
Expected: no `git status` output, rc 0.

- [ ] **Step 2: Run each mutation**

For each row of the table: make the edit with the Edit tool, run
`bash -n ci/images/run.sh && /bin/bash ci/images/console-selftest.sh > /private/tmp/claude-501/sma-671/mut-<id>.txt; echo "rc=$?"; grep -E '^FAIL' /private/tmp/claude-501/sma-671/mut-<id>.txt`,
check the expected FAIL row, restore with the Edit tool, and run `git diff --stat` (it must be
empty).

| ID | File | Edit | Expected FAIL row |
|---|---|---|---|
| MU1 | run.sh `console_staged_parity_row` | In the "no host build" arm, change `if [ "$parity" = required ]; then` to `if false; then` | SP2 |
| MU2 | run.sh `parity_required_flag` | Replace the `*)` arm body (the `echo "::error::…"` and `return 1` lines) with `echo "--parity=optional"` | PF2, PF3, PF3b, PF3c, D2, D3 |
| MU3 | run.sh `smoke_consoles` | In the second-word `case`, replace the `*)` arm body with `parity="optional"` | Z6, Z6b |
| MU4 | run.sh `smoke_consoles` | Delete the line `console_staged_parity_row "$app" "$image" "$parity" \|\| ec=1` | P1d, Z1-parity-iam, Z2-parity-iam |
| MU5 | run.sh `console_staged_parity_row` | Replace the third-argument `*)` arm body with `parity=optional` | SP2b |
| MU6 | run.sh `console_staged_parity_row` | Change `elif [ "$parity" = required ]; then` (the same-directories arm) to `elif false; then` | SP4b |
| MU7 | run.sh `docker_context_leaks` | Change `leaks=$((leaks + 1))` to `leaks=$((leaks + 0))` | CX1, CX2, CX3, CX4, CX6, CX7 |
| MU8 | run.sh `all-consoles)` arm | Delete the line `pr_flag="$(parity_required_flag)" \|\| exit 1` | D2 (or D2-nodocker) |
| MU9 | run.sh `smoke)` arm | Delete the line `pr_flag="$(parity_required_flag)" \|\| exit 1` | D3 (or D3-nodocker) |
| MU10 | images.yml | Delete the line `CONSOLE_PARITY_REQUIRED: '1'` under "Console release sequence" | W1 |
| MU11 | images.yml | In "Console release sequence", delete the line `set +e` | RS1 (and RS1-gw-*) |
| MU12 | images.yml | Move the whole host-build step after "Build + smoke both consoles" | W1 |

The five in-harness W1 mutations (W1-m1 to W1-m5) and W1-m6 to W1-m9 run on every self-test run;
their PASS rows are the proof for those targets.

- [ ] **Step 3: Re-run the whole battery after any fix**

If a mutation did not red its row, fix the row or the code, commit the fix, and run ALL of MU1-MU12
again (memory `mutation-battery-rerun-whole`).

- [ ] **Step 4: Record the results**

Append to the measurements file:

```markdown
## Mutation proof (SMA-671 AC 4)

| ID | Mutation | Row(s) that went red | Restored, `git diff` empty |
|---|---|---|---|
| MU1 | … | … | yes |
```

If the permission system refuses a mutation run, restore the file, write the exact manual steps
under a heading `Mutation proof pending` in the same file, and continue.

- [ ] **Step 5: Commit the record**

```bash
git add docs/superpowers/specs/2026-10-01-sma-671-measurements.md
git commit -m "docs(ci): record the SMA-671 mutation proof

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 9: Local verification (both bashes, the actionlint gate, the full graph)

**Files:** none changed, unless a gate finds a defect (then fix it in the owning file and commit).

- [ ] **Step 1: The self-test under bash 3.2**

Run: `cd <worktree> && /bin/bash ci/images/console-selftest.sh; echo "rc=$?"`
Expected: rc 0.

- [ ] **Step 2: The self-test under bash 5**

Use the Monitor tool with a 300 s limit:
`cd <worktree> && /opt/homebrew/bin/bash ci/images/console-selftest.sh; echo "rc=$?"`.
If it does not finish (about 0% CPU), the Mac is in the 512-byte small-pipe state (SMA-612). Stop
it and run it in a Linux container instead:

```bash
docker run --rm -v /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-671-images-console-coverage:/w -w /w \
  ubuntu:24.04 bash -c 'apt-get -qq update >/dev/null && apt-get -qq install -y git >/dev/null && git config --global --add safe.directory "*" && bash ci/images/console-selftest.sh'
```

Expected: rc 0. Rows that need docker inside the container print `SKIP` (allowed when `CI` is not
`true`). Note: `git config --global` here writes only inside the throwaway container.

- [ ] **Step 3: The `repo:actionlint` gate**

Run: `cd <worktree> && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && /opt/homebrew/bin/bash ci/actionlint/run.sh; echo "rc=$?"`
Read the preflight line first. If it says the pipe is small (rc 2), run the gate in a Linux
container as memory `small-pipe-linux-container-workaround` describes. Expected: rc 0 (AC 8).

- [ ] **Step 4: The full gate graph**

Run the target list from the root `CLAUDE.md` (between the `ci-targets` markers) with
`--base origin/main --include-relations`, after `git fetch origin main`. Then re-run the gates that
need the other bash directly, per root `CLAUDE.md` "This development Mac only"
(`repo:affected-smoke` with bash 3.2; `repo:ruff-ci`, `repo:next-public-free`,
`repo:publish-metadata`, `repo:version-lockstep`, `repo:nats-permissions` with bash 4+). Expected:
every target passes, or a failure is a known host artifact that the root `CLAUDE.md` or the memory
index names. Record each such exception with its evidence in the task report.

- [ ] **Step 5: Commit any fix**

Only if a step found a defect: fix it, re-run Steps 1-4, and commit with a `fix(ci):` message.

---

### Task 10: Integration run on both legs and the final records (AC 1, 5, 9, 10)

**Files:**
- Modify: `docs/superpowers/specs/2026-10-01-sma-671-measurements.md`

- [ ] **Step 1: Rebase check and push**

```bash
git fetch origin main
git log --oneline origin/main..HEAD
git merge-base --is-ancestor origin/main HEAD && echo "up to date with main" || echo "main moved"
git push origin feature/sma-671-images-console-coverage
```

If `main moved` and it touched `ci/images/**`, `images.yml` or the docs of Task 7, rebase onto
`origin/main`, re-run Task 9 Steps 1 and 3, and push with `--force-with-lease` (the only allowed
force push).

- [ ] **Step 2: Dispatch and wait**

```bash
gh workflow run images.yml --ref feature/sma-671-images-console-coverage
gh run list --workflow images.yml --branch feature/sma-671-images-console-coverage \
  --event workflow_dispatch --limit 1 --json databaseId,headSha,status
```

Confirm `headSha` equals `git rev-parse HEAD`. Wait with the Monitor tool as in Task 1 Step 6.

- [ ] **Step 3: Read the evidence**

Save both job logs and the steps JSON as in Task 1 Step 7 (`t10-*`). Then, for each leg:

```bash
grep -c 'staged tree matches the host build' "$S/t10-<leg>.log"    # AC 1: expect 4 (2 consoles x 2 steps)
grep -c 'NOT CHECKED' "$S/t10-<leg>.log" || true                    # AC 1: expect 0
grep -n 'docker context: no host artifact' "$S/t10-<leg>.log"       # AC 9: expect 1 line
grep -n -E 'host-build:|Filesystem|/dev/root' "$S/t10-<leg>.log"    # AC 5
```

Both jobs must conclude `success` (AC 5). If a leg fails, read its log, do not re-run blindly
(memory `moon-ci-affected-model` and the root `CLAUDE.md` diagnosis rules), and report.

- [ ] **Step 4: Record the figures**

Append a section to the measurements file, for each leg: `df -h /` before and after the
host-build step, the `pnpm install` and `moon run` seconds, the host-build step duration, the job
duration, the four "matches" lines, and the context-check line. Add the AC 10 evidence: rows
RS1-RS3 (the CI log of the "Console image self-test" step shows them `PASS`).

- [ ] **Step 5: Post the Linear comment**

Post a comment on SMA-671 with the section from Step 4 and the run link. If this agent has no
Linear tool, mark it `Linear comment: PENDING` in the file and name it in the task report.

- [ ] **Step 6: Commit and push**

```bash
git add docs/superpowers/specs/2026-10-01-sma-671-measurements.md
git commit -m "docs(ci): record the SMA-671 integration run figures

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git push origin feature/sma-671-images-console-coverage
```

- [ ] **Step 7: Notes for the PR stage (no action here)**

The PR description copies from the measurements file: the item-3 figures (Task 2), the item-4
watch state, the Task 1 and Task 10 figures for each leg, the mutation table (Task 8), and any
`Mutation proof pending` steps. After the merge, the first `main` run of `images.yml` gives new
figures with the host build (spec D3, AC 5); the coordinator records them in a Linear comment.
Item 4 of the issue stays open.
