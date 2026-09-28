<!-- moon-diagnosis:ok -->
# SMA-714: a gate that runs the moon-diagnosis procedure against a real failed report

- Linear: SMA-714 (related: SMA-711, SMA-597)
- Path: architectural (a new `repo:*` gate with seven registration obligations)
- Date: 2026-09-27 (draft), 2026-09-28 (approved)
- Status: APPROVED by Sven on 2026-09-28. The adversarial challenge (Stage 2) ran; see the
  "Challenge changelog" at the end. The "Approval decisions" section records the decisions, and
  the "Re-check against origin/main" section records the corrections made at approval.

## Approval decisions (Sven, 2026-09-28)

These decisions override the defaults of this spec.

- D1. The gate name is `repo:moon-diagnosis-exec` (accepted, Open question 1).
- D2. Steps 1, 2 and 2a all stay in scope. This includes the Step 2a doc-text change inside the
  root `CLAUDE.md` `moon-diagnosis` block (§7, AC7, Open question 2).
- D3. The Step 2a tolerance is `0 <= finishedAt_ms - lastRunTime <= 1000` ms (accepted, Open
  question 3). The plan must not be final before T7 has measured the Linux data in a Linux
  container.
- D4. The root `CLAUDE.md` has two marker blocks that gates compare byte for byte (`ci-targets`
  and `moon-diagnosis`). The new target in the `ci-targets` block must match the `T=(…)` array
  in `ci.yml` exactly. Each marker must occur exactly once in the file. Do not quote a marker
  anywhere else in the file, also not inside backticks.
- D5. The local-review stage runs at most ONE CodeRabbit CLI pass, because the rate limit is
  about one to two reviews per hour and another pipeline shares it. If the pass is rate limited,
  skip it and record that in the PR.
- D6. The Mac can be in the 512-byte small-pipe state (SMA-612). Run bash-5 gates in a Linux
  container when the host is in that state.

## Problem

The root `CLAUDE.md` holds the procedure "Diagnosing an unattributed `moon ci` failure" between
the `moon-diagnosis:begin` and `moon-diagnosis:end` markers. Check 12 of `repo:actionlint` gates
that block. It checks only that five literals are present (`DOC_DIAGNOSIS_REQUIRED_LITERALS`,
`ci/actionlint/run.sh:4698`). It does not run the procedure.

`ci/actionlint/README.md` L29 (line 548) records this limit. L32 (lines 561-569) points to the
same missing gate. The root `CLAUDE.md` block says "No gate runs this query (limitation L29 in
`ci/actionlint/README.md`, SMA-714)" (lines 189-190).

The gap caused a real defect. SMA-597 wrote the Step 1 `jq` query with `{command, exitCode}`.
Moon writes those two fields under `.meta`, so the query printed `null` for both values from the
day it landed. SMA-711 (commit `81451a82`) fixed the query. Every gate stayed green during the
full life of the defect, and a new edit can break the query again with no red.

## Measured facts (2026-09-27, moon 2.5.3, macOS, this session)

A throwaway workspace was made under the session scratchpad
(`scratchpad/specs/probe-714/probe`). It has `.prototools` with `moon = "2.5.3"`, a
`.moon/workspace.yml` with one project `probe` and `vcs.client: git`, a `.gitignore` with
`.moon/cache`, and two commits (the second commit changes the task input). The SMA-711 probe
(`0314622e…/scratchpad/moon-probe`) used the same shape with five tasks. A second probe
(`scratchpad/specs/probe-714/ts/p`, 40 failing tasks) was made after the challenge for M7 and M9.
All measurements are from macOS. None is from Linux (see T7).

- **M1. Speed.** `moon ci probe:envdump --base <first commit>` finished in 0.67 s wall time.
  The SMA-711 report for five tasks (one of them a 2 s timeout) shows the run from
  `14:49:28.939` (first action start) to `14:49:30.969` (last action end), about 2.03 s. So the
  cost of one nested run is a few seconds. A full three-mode run was not measured.
- **M2. Moon's exit code follows the task only for one failed task.** With one failed task that
  exits 3, `moon ci` exited **3**. SMA-711 recorded exit 1 with several failed tasks. The 40-task
  probe (exit codes 1, 2 and 3 mixed) exited 1 on six of six runs. The gate therefore accepts any
  non-zero exit code from the nested run and does not expect a specific value.
- **M3. Moon passes its own environment to a task.** A task that ran `env` saw
  `MOON_WORKSPACE_ROOT`, `MOON_WORKING_DIR`, `MOON_CACHE_DIR`, `MOON_PROJECT_ID`, `MOON_TARGET`,
  `MOON_TASK_HASH`, `MOON_VERSION` and eleven other `MOON_*` variables. It also saw these
  `PROTO_*` variables (from `probe-714/probe/.moon/cache/states/probe/envdump/stdout.log`):
  `PROTO_APP_LOG=info`, `PROTO_AUTO_INSTALL=false`, `PROTO_CLI_VERSION=0.60.2`, `PROTO_HOME`,
  `PROTO_IGNORE_MIGRATE_WARNING=true`, `PROTO_MOON_VERSION=2.5.3`, `PROTO_NO_PROGRESS=true`,
  `PROTO_OFFLINE_TIMEOUT=750`, `PROTO_REPORTER=text`, `PROTO_SHIM_NAME=moon`,
  `PROTO_SHIM_PATH` and `PROTO_VERSION=0.60.2`. proto reads `PROTO_<TOOL>_VERSION` before any
  `.prototools`. So an inherited `PROTO_MOON_VERSION` overrides a fixture pin. This is why unit 4
  uses an allowlist environment and not a denylist.
- **M4. An inherited `MOON_WORKSPACE_ROOT` moves the nested run to the wrong workspace.** With
  `MOON_WORKSPACE_ROOT` set to another directory, the nested `moon ci` failed with
  `app::missing_config` ("Unable to locate .moon/workspace.{yml,…}") and wrote no report. Inside a
  real `repo:*` task, this variable points to the repository root. A nested run that keeps it
  would run `moon ci` against the REAL workspace, and would write its report there, not in the
  fixture. `MOON_CACHE_DIR`, `MOON_WORKING_DIR` and `MOON_PROJECT_ID` did not change the result in
  the same test.
- **M5. GitHub environment variables change how moon reads `--base`. CORRECTED after the
  challenge.** With `CI=true`, `GITHUB_ACTIONS=true`, a `GITHUB_SHA` that does not exist in the
  fixture, `GITHUB_BASE_REF`, `GITHUB_HEAD_REF`, `GITHUB_EVENT_NAME=pull_request` and
  `GITHUB_REF` set, `moon ci … --base <sha>` ran the task and wrote the report. But its output
  printed `Base revision: N/A` (`probe-714/g2.out`, line 2). The same command without those
  variables printed the base sha (`probe-714/out.txt`, line 2). So moon did NOT use `--base`
  under the GitHub variables. It probably compared HEAD with its parent, and the fixture passed
  only because commit 2 is the only change. The earlier text of this fact ("an explicit `--base`
  is sufficient") was wrong. A `push` event was not measured. Unit 4 therefore runs the nested
  moon with an allowlist environment that contains no `CI` and no `GITHUB_*` variable (see M9).
- **M6. Report and state files (from the SMA-711 probe).** `meta.command` and `meta.exitCode`
  per task are: `exit 3` → 3, a two-line `script:` (`echo a`, `exit 4`) → `echo a exit 4` and 4,
  `command: 'no-such-binary-xyz'` → 127, `command: 'sleep 20'` with `options.timeout: 2` →
  operation `status` `timed-out` and no `exitCode` key. `.moon/cache/states/probe/<task>/` holds
  `stdout.log`, `stderr.log` and `lastRun.json`. `missing/stderr.log` holds
  `bash: line 1: no-such-binary-xyz: command not found`. For the timed-out task,
  `lastRun.json` has `"exitCode":-1`.
- **M7. Step 2a's two timestamps come from SEPARATE clock reads. CORRECTED after the
  challenge.** `lastRun.json` has `lastRunTime` in epoch milliseconds. The report action has
  `finishedAt` as a naive ISO time in UTC with microseconds and no zone suffix (for example
  `"2026-09-27T14:49:28.972597"`). The first version of this spec had one sample in which the two
  values agreed to the millisecond. The 40-task probe measured `finishedAt_ms - lastRunTime`
  over 6 runs (240 task results): the minimum was 0.127 ms, the maximum 15.611 ms, no value was
  negative, and 134 of 240 values were 1 ms or more. So `lastRunTime` is read BEFORE the action
  `finishedAt`, and an exact-equality check gives a false red on most runs. The gate uses a
  bounded compare (unit 7). A stale log from another run is seconds or more old, so a bound of
  1000 ms keeps the check useful.
- **M8. `jq` is on the host** (`/opt/homebrew/bin/jq`). No gate script in `ci/` calls `jq` today.
  The GitHub Ubuntu runner image ships `jq`; it is not pinned by `.prototools`.
- **M9. An allowlist environment works.** In the 40-task probe the nested moon ran as
  `env -i HOME="$HOME" PATH="$PATH" TMPDIR="$TMPDIR" PROTO_HOME="$HOME/.proto"
  PROTO_REPORTER=text moon ci … --base <sha>`. moon printed the real base sha, ran all 40 tasks
  and wrote `.moon/cache/ciReport.json` in the fixture, on six of six runs.
- **M10. The host signs every commit.** The global git config on this host has
  `commit.gpgsign=true`, `tag.gpgsign=true` and `gpg.ssh.program` set to 1Password's
  `op-ssh-sign`. `ci/release-plan/run.sh:220-238` records that an unguarded fixture commit here
  can hang or fail. The 40-task probe committed with `GIT_CONFIG_GLOBAL=/dev/null
  GIT_CONFIG_NOSYSTEM=1` and did not sign.

## Design

### 1. Where the check lives: a new gate, `repo:moon-diagnosis-exec`

Three options were compared.

| Option | For | Against |
|---|---|---|
| **A. New gate `repo:moon-diagnosis-exec` (chosen)** | Narrow `inputs`, so it runs only when the procedure, the moon pin or the gate changes. Its own bash rules (3.2-compatible). Its own rc 2 for nested-moon infrastructure errors. | Seven registration obligations (§6). |
| B. A new check inside `repo:actionlint` | No new registration. | `repo:actionlint` has `inputs: ['**/*']`, so the nested `moon ci` would run on every PR. The gate already needs bash 5 and a healthy pipe locally. A nested-moon abort (the proto-shim EACCES in `ci/CLAUDE.md`) would red the largest gate. |
| C. Run the query against a committed `ciReport.json` fixture | Fast and deterministic. No nested moon. | It cannot see a field move on a moon bump, which is the class of defect SMA-711 found. The fixture would itself need a check-12 marker and would get old. |

Option A is the recommendation. Option C is rejected, not deferred: a live report is what the
issue asks for.

### 2. Files

- `ci/moon-diagnosis/run.sh` (new). The gate. Bash, compatible with system bash 3.2: no
  `mapfile`, no `declare -A`, no here-string or pipe that writes more than 512 bytes before its
  reader starts (use temp files). No pipe into a reader that can exit early (`| grep -q`,
  `| grep -m`, `| head`, `awk … exit`, a `sed` script with `q`), per `ci/CLAUDE.md` lines
  292-316; check 13 of `repo:actionlint` scans this file. The first lines export
  `PROTO_REPORTER=text` (standing rule, SMA-609) and carry a `# moon-diagnosis:ok` comment,
  because the file names `ciReport` and check 12 scans every tracked file for that token. The
  pre-SMA-711 query literal (§4, negative control) carries a comment directly above it that says
  it is the control input and is broken on purpose.
- `ci/moon-diagnosis/README.md` (new). What the gate asserts, its exit codes, and its limits.
  It also carries `<!-- moon-diagnosis:ok -->`.
- `moon.yml` (root): the new task (§5), and a new `ci/moon-diagnosis/**/*` entry in the inputs of
  `repo:affected-smoke`, with a comment in the same form as the `ci/helm-render/**/*` entry
  (`moon.yml:252-257`).
- `.github/workflows/ci.yml`: add `:moon-diagnosis-exec` to the `T=(…)` array, directly after
  `:helm-render` and before `:test-e2e`.
- `CLAUDE.md` (root): add `:moon-diagnosis-exec` to the `ci-targets` block in the same position
  (after `:helm-render`, before `:test-e2e`), and change the one sentence at lines 189-190 inside the
  `moon-diagnosis` block and the Step 2a text at lines 202-204 (§7).
- `ci/affected-graph/ci_targets.py`: `SELF_SCHEDULED_GATES`, `SELF_TASK_EXPECTED_GLOBS`,
  `REQUIRED_REPO_TASKS`, a new `MOON_DIAGNOSIS_SH_CALL_SITES` pin table and its wiring.
- `ci/actionlint/run.sh`: a new `'ci/moon-diagnosis/**/*'` entry in
  `T_AFFECTED_SMOKE_REQUIRED_INPUTS` (lines 2127-2166), after the `ci/helm-render/**/*` entry
(lines 2160-2163) and before the `'CLAUDE.md'` entry,
  with a comment in the same form. The broad `ci/**/*` entry already covers the path for
  scheduling. The narrow entry follows the repo policy to keep narrow globs (the
  `ci/next-public` and `ci/helm-render` comments state it).
- `ci/actionlint/README.md`: narrow L29 and L32 (§7).
- `ci/CLAUDE.md`: one new entry that records M3, M4 and M5 (§7).

**Files that will contain the token `ciReport`.** Only `ci/moon-diagnosis/run.sh` and
`ci/moon-diagnosis/README.md` (both carry the `:ok` marker), plus the lines 189-190 edit inside the
root `CLAUDE.md` block (allowlisted as the authority). `ci/CLAUDE.md` and
`.github/workflows/ci.yml` contain the token already on origin/main and pass check 12 today; the
new `ci/CLAUDE.md` entry (§7) does not need the token and must not add it. `run.sh` keeps the report path in one
variable (`REPORT_REL=".moon/cache/ciReport.json"`, one line). No pinned line, no `moon.yml`
comment and no `ci_targets.py` row contains the token, so `ci_targets.py` and `moon.yml` need no
check-12 marker. The plan re-checks this with `git grep -l ciReport` after the change.

### 3. The gate's units

Each unit is a bash function with one job. The pure units take files as arguments, so
`--self-test` can drive them with fixtures and no moon.

Every verdict unit routes the exit status of its own `jq` or `python3` calls. A non-zero status
from a verdict's own tool is never "clean": it emits a `verdict-error <unit> rc=<n>` row, and
the gate exits 1. This closes the fail-open path in which a crashed verdict prints nothing.

1. **`extract_step1_query CLAUDE_MD OUT`.** Reads the text between the single
   `moon-diagnosis:begin` and `moon-diagnosis:end` lines. In that text, finds the first line that
   contains `Step 1`, then the next line that is a fence opening with `bash`, then the next
   closing fence. Writes the lines between the two fences to `OUT`, with the common leading
   indentation removed. Verdicts: `ok`, `no-block`, `no-step1`, `no-fence`, `unclosed-fence`,
   `multiple-step1`. `multiple-step1` is emitted when the block holds a second line that contains
   `**Step 1` (the bold heading form) or a second `bash` fence between `Step 1` and `Step 2`. A
   second Step 1 query is an rc 1 defect, because a broken second copy would otherwise ship green.
2. **`query_shape_verdict QUERY_FILE PROG_OUT`.** A positive-shape check, not a denylist. The
   file, with line continuations (`\` at the end of a line) joined, must match exactly this
   shape: the word `jq`; zero or more flags from the allowlist `-r`, `-c`, `-s`, `-e`; one
   single-quoted program (it can span lines and contains no `'`); then the path
   `.moon/cache/ciReport.json`; then only white space. The unit writes the program text to
   `PROG_OUT` and the flags to a sibling file. Any other shape is `bad-shape <reason>`, rc 1. The
   gate never runs doc text through `bash`. Unit 5 calls `jq` with the extracted flags and
   program. This removes the need for a quote-aware tokenizer. It is not a security boundary: the
   file is reviewed repo content.
3. **`make_fixture DIR PIN`.** Makes the throwaway workspace: `.prototools` with the single line
   `moon = "<PIN>"`, where `<PIN>` is read from the repo's `.prototools` (`^moon = "…"`);
   `.moon/workspace.yml` (one project `probe`, `vcs.client: git`, `defaultBranch: main`);
   `.gitignore` with `.moon/cache`; `probe/file.txt`; `probe/moon.yml` with the five tasks below.
   Every `git` call in the fixture runs with `GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1`
   (as in `ci/release-plan/release_plan.py:1428-1446`), so the host's signing config (M10) and
   any global hook or identity never apply. In addition, directly after `git init -b main`, the
   unit sets local `commit.gpgsign false`, `tag.gpgsign false`, `maintenance.auto false` and
   `gc.auto 0` (SMA-708 rule: this fixture runs `git commit`), and local `user.name` and
   `user.email`. Commit 1, change `probe/file.txt`, commit 2. Prints the sha of commit 1.

   | Task | Definition | Expected Step 1 row |
   |---|---|---|
   | `fail3` | `script: 'exit 3'` | `status` `failed`, `exitCode` 3, `command` `exit 3` |
   | `multi` | `script:` with two lines `echo a`, `exit 4` | `failed`, 4, `echo a exit 4` |
   | `missing` | `command: 'no-such-binary-xyz'` | `failed`, 127, `no-such-binary-xyz` |
   | `slow` | `command: 'sleep 20'`, `options.timeout: 2` | `timed-out`, `null`, `sleep 20` |
   | `ok` | `command: 'true'` | no row |

   Every task has `toolchain: 'system'`, `inputs: ['file.txt']` and `options.cache: false`.
4. **`run_nested_moon DIR BASE`.** Runs the nested moon in `DIR` with an ALLOWLIST environment:
   `env -i HOME="$HOME" PATH="$PATH" TMPDIR="${TMPDIR:-/tmp}" LANG="${LANG:-C}"
   PROTO_HOME="${PROTO_HOME:-$HOME/.proto}" PROTO_REPORTER=text MOON_WORKSPACE_ROOT="$DIR"`.
   This removes every `MOON_*` variable (M4), every `PROTO_*` variable except the two above, so
   that the fixture `.prototools` controls the moon version (M3), and every `CI` and `GITHUB_*`
   variable, so that the nested run is a local run (M5). The procedure says it is "for local
   runs", so the nested run models a local run. The allowlist needs no parse of `env` output,
   which can contain multi-line values. It also stops moon from writing `::group::` commands
   into the captured output. The command is `moon ci probe:fail3 probe:multi probe:missing
   probe:slow probe:ok --base BASE`. Before that, `moon --version` in `DIR`, under the same
   allowlist environment, must print `moon <PIN>`. Because the fixture pin now controls the
   version, this check is meaningful.
   Before the nested call, the unit records a hash (`cksum`) of the outer workspace's report
   file, or the word `absent`. After the call it compares again. A change is rc 2 ("the nested
   run wrote to the real workspace", M4). Captures stdout and stderr to files in the gate's temp
   directory. Verdicts:
   - the outer report hash changed → infrastructure error.
   - rc 0 → infrastructure error (the fixture did not fail, so the report proves nothing).
   - the fixture report is absent or is not valid JSON → infrastructure error. If the captured
     stderr contains `proto-shim`, the message says so (`ci/CLAUDE.md`, SMA-592 entry).
   - a fixture-run check with the gate's OWN `jq` program (not the doc query) does not find
     exactly five `RunTask(probe:*)` actions with statuses `failed` for `fail3`, `multi` and
     `missing`, `timed-out` or `failed` for `slow` (the plan fixes the exact action status from a
     measurement), and `passed` for `ok` → infrastructure error. This proves that the fixture ran
     as designed before unit 6 reads the doc query's output, so a `missing-row` from unit 6 can
     only mean a doc defect.
   - any other non-zero rc with a valid report → continue (M2).
5. **`run_step1_query PROG_FILE FLAGS_FILE DIR OUT`.** Runs `jq <flags> "$(cat PROG_FILE)"
   "$DIR/$REPORT_REL"`, stdout to `OUT`. The gate calls `jq` directly; it never passes doc text
   to `bash`, so the `bash` on `PATH` does not matter. A non-zero rc from `jq` is an assertion
   failure (rc 1), not an infrastructure error: a program that does not parse is a broken
   procedure.
6. **`step1_verdict OUT`.** Reads `OUT` with `jq -s`. Emits one row per violation, nothing when
   clean:
   - `empty-output` when `OUT` holds no JSON value.
   - `missing-row <label>` for each expected label in the table above with no object whose
     `label` equals it.
   - `bad-exec <label> <type>` when the object's `exec` is not an array (an object, `null`, or
     absent). The pre-SMA-711 query makes `exec` an object
     (`docs/superpowers/specs/2026-09-03-sma-597-moon-failure-diagnosis-design.md:311-314`).
   - `wrong-exec <label>` when `exec` is an array but has no entry whose `status`, `exitCode` and
     `command` equal the expected values. `null` is compared as `null`.
   - `unexpected-row <label>` for any object whose label is not one of the four expected labels
     (the `ok` row among them).
   - `verdict-error step1 rc=<n>` when the verdict's own `jq` exits non-zero.
   Extra keys in an object (for example `error`) are allowed. The row shape is fixed to `label`
   plus `exec[]` of `{status, exitCode, command}`; a deliberate change of that shape is a
   re-baseline of this function.
7. **`step2_verdict DIR`** (Step 2 and Step 2a). For each of the four failed tasks:
   `.moon/cache/states/probe/<task>/` holds `stdout.log`, `stderr.log` and `lastRun.json`;
   `missing/stderr.log` contains `command not found`; `multi/stdout.log` contains a line `a`.
   Step 2a: `0 <= finishedAt_ms - lastRunTime <= 1000`, where `finishedAt_ms` is the report
   action's `finishedAt` read as a naive UTC ISO time and converted to epoch milliseconds
   (M7). The conversion uses `python3` (`datetime.fromisoformat(…).replace(tzinfo=utc)`), because
   BSD and GNU `date` differ on fractional seconds. `python3` is on the host and on the runner
   image. The plan confirms that no other gate treats `python3` as a new dependency (the
   affected-graph gate already runs `ci_targets.py`). These rows check moon behaviour that the
   procedure relies on, not the doc text. The directory prefix is taken from the literal
   `.moon/cache/states/` that check 12 already requires in the block. A non-zero status from the
   verdict's own tools emits `verdict-error step2 rc=<n>`.

### 4. Modes and exit codes

`run.sh` follows the three-mode shape of the other `repo:*` gates.

- `--self-test`: fixture tables for units 1, 2, 6 and 7, with no moon and no git. Cases at a
  minimum:
  - unit 1: the current `CLAUDE.md` block shape; no block; no `Step 1`; a `Step 1` with no fence;
    an unclosed fence; two `Step 1` fences (`multiple-step1`).
  - unit 2: the current query (`ok`); the pre-SMA-711 query (`ok`, the shape is the same); a
    query with a `| head` after the path; a query with `> out` after the path; a query with a
    flag that is not in the allowlist; a query that reads another file; a program with a `'`
    in it.
  - unit 6: the correct Step 1 output (from M6); the old null output (four `wrong-exec` or
    `bad-exec` rows, exact set); empty output; an output with the `ok` row; an output with one
    row missing; `exec` as an object, as `null`, and absent; a verdict whose `jq` input is not
    JSON (`verdict-error`).
  - unit 7: a Step 2a pair with a difference of 0 ms, of 1000 ms (at the bound, clean), of
    1001 ms (past the bound, red), and of -1 ms (negative, red); a missing `stderr.log`; a
    `missing/stderr.log` without `command not found`.
  Each case asserts the exact verdict string. The table also asserts its own row count, so a
  deleted case reds.
- `--negative-control`: builds a copy of the working-tree `CLAUDE.md` in the temp directory in
  which the Step 1 fence body is replaced with the pre-SMA-711 query (the `{command, exitCode}`
  projection from `git show 81451a82^:CLAUDE.md`, embedded in `run.sh` as a literal with the
  comment from §2). It runs the FULL real path (units 1 to 7, with a real nested `moon ci`) on
  that copy, as a child `bash "$0"` call with a hidden `--claude-md <path>` argument. The rc map
  of the control is fixed:
  - child rc 1 and the child's row set equals EXACTLY the expected set (four rows, one per failed
    label, of the kind that the old query produces; the plan fixes whether that kind is
    `wrong-exec` or `bad-exec` from one measurement and writes it into the table), and no other
    row → control rc 0.
  - child rc 0 → control rc 1 (the gate is blind to the historical defect).
  - child rc 1 with any other row set → control rc 1.
  - child rc 2 → control rc 2.
  This is the acceptance criterion "a change that breaks the Step 1 query reds the gate", proved
  by the production path, not by a fixture.
- no flag: the real run on the working-tree `CLAUDE.md`.

Exit codes: 0 clean; 1 an assertion failed; 2 an infrastructure error (no `moon`, no `jq`, no
`git`, no `python3`, a moon version that differs from the pin, no report, a nested run that
wrote to the real workspace, a fixture that did not run as designed). Each rc 2 message contains
the text `infrastructure error (rc=2)`, the classifier the other gates use. A temp directory
made with `mktemp -d` is removed by an `EXIT` trap. No fallback resolves `moon` from another
path: if `command -v moon` fails, the gate exits 2.

### 5. The moon task

```yaml
moon-diagnosis-exec:
  description: 'Run the CLAUDE.md moon-diagnosis Step 1 query against a real failed moon ci report (SMA-714).'
  script: |
    set -euo pipefail
    bash ci/moon-diagnosis/run.sh --self-test
    bash ci/moon-diagnosis/run.sh --negative-control
    bash ci/moon-diagnosis/run.sh
  toolchain: 'system'
  inputs:
    - 'CLAUDE.md'
    - '.prototools'
    - 'ci/moon-diagnosis/**/*'
```

`.prototools` is an input on purpose: a moon bump is the event most likely to move a report
field. Each of the two nested runs (the control and the real run) is about 2 s, most of it the
`slow` timeout task (M1). The plan measures the three modes from start to end, on macOS and in
the Linux container (T7).

### 6. Registration (seven obligations, `ci/CLAUDE.md` "Registering a new `repo:*` gate")

1. `ci.yml` `T=(…)`: add `:moon-diagnosis-exec` after `:helm-render`, before `:test-e2e`.
2. Root `CLAUDE.md` `ci-targets` block: the same target in the same position. Do not add a
   second copy of either marker anywhere.
3. `SELF_SCHEDULED_GATES["moon-diagnosis-exec"]`: the four `moon.yml` lines.
4. `SELF_TASK_EXPECTED_GLOBS["moon-diagnosis-exec"]`: the three `inputs`.
5. `MOON_DIAGNOSIS_SH_CALL_SITES` in `ci_targets.py`: whole-line pins of each call that carries a
   check, and of its rc guard:
   - the flag parse and the dispatch arms for `--self-test` and `--negative-control`;
   - the self-test table call and its row-count assertion;
   - the call of `extract_step1_query`, `query_shape_verdict`, `run_step1_query`,
     `step1_verdict` and `step2_verdict` in the real path, each with its rc guard;
   - the fixture-run check of unit 4 and the outer report hash compare;
   - the `env -i` allowlist line of the nested call, the capture of the nested run and its rc
     guard;
   - the `moon --version` check against the pin;
   - the negative control's rc map lines and its exact row-set assertion.
   Rationale: the `workflow-credentials` entry measured that pins on report lines alone let a
   control assert nothing. No pinned line contains the token `ciReport` (§2).
6. `REQUIRED_REPO_TASKS`: add the task, because it carries a `--negative-control`.
7. Reachability: `repo:affected-smoke` lists `ci/moon-diagnosis/**/*` in its `moon.yml` inputs,
   and `T_AFFECTED_SMOKE_REQUIRED_INPUTS` floors it (§2). So a PR that deletes a pinned line
   schedules the pin check.

`repo:input-liveness` asserts that each of the three `inputs` matches a tracked file; all three
do after the change.

### 7. Documentation changes

- `CLAUDE.md` lines 189-190, inside the `moon-diagnosis` block. Replace "No gate runs this query
  (limitation L29 in `ci/actionlint/README.md`, SMA-714)." with a sentence that says
  `repo:moon-diagnosis-exec` runs this query and Steps 2 and 2a against a real failed report
  (SMA-714). The five required literals stay in the block. The `moon-diagnosis` block must keep
  exactly one begin and one end marker.
- `CLAUDE.md` Step 2a text, inside the same block. Change "Compare the report action's
  `finishedAt` against `lastRun.json`'s `lastRunTime`. **If they disagree, stop**" so that it
  states the units and the tolerance: `finishedAt` is a UTC time with no zone suffix,
  `lastRunTime` is epoch milliseconds, and moon reads `lastRunTime` up to about 16 ms BEFORE
  `finishedAt` (M7). A difference from 0 to 1000 ms is the same run. A negative difference, or a
  difference of more than 1000 ms, means the logs are from a different run: stop. Without this
  change, a reader who follows the text gets the same false disagreement that the gate must
  avoid. The plan re-checks that the five required literals of check 12 stay in the block.
- `ci/actionlint/README.md` L29: narrow it. Check 12 still gates presence only, but Step 1's query
  and Steps 2 and 2a are now executed by `repo:moon-diagnosis-exec`. What stays open: Step 0 and
  Step 3 (prose, not executable), the "What cannot work" paragraph, and the CI note.
- `ci/actionlint/README.md` L32: change "the same procedure-execution gate L29 defers to a
  follow-up issue" so that it says the execution gate covers only the root `CLAUDE.md` block, not
  diagnosis advice in other files of the corpus. That part of L32 stays open.
- `ci/CLAUDE.md`: a new entry in "Affectedness and task inputs" or a new short section. Scope: a
  nested `moon` call inside a gate that targets a DIFFERENT workspace (a fixture). Such a call
  must run with an allowlist environment (`env -i` plus the named variables), because an
  inherited `MOON_WORKSPACE_ROOT` moves it to the real workspace (M4), an inherited
  `PROTO_MOON_VERSION` overrides the fixture pin (M3), and inherited `CI`/`GITHUB_*` variables
  make moon ignore `--base` (M5). The entry states that it does NOT apply to a nested call that
  must target the real workspace, for example the nested `moon query` in `repo:affected-smoke`.

## Acceptance criteria

| # | Criterion | Source |
|---|---|---|
| AC1 | A change of the Step 1 query back to `{command, exitCode}` reds the gate with rc 1. | Issue |
| AC2 | README L29 is narrowed to what stays open, and L32 no longer points to a missing gate. | Issue |
| AC3 | The gate passes on the current `CLAUDE.md` with moon 2.5.3, locally (with 1Password locked) and in CI. | Added |
| AC4 | `--negative-control` exits 0 only when the full production path reds on the old query with exactly the expected row set. | Added |
| AC5 | A nested run never touches the real workspace: it runs with the allowlist environment, and a change of the outer report hash is rc 2. | Added (M4) |
| AC6 | A deleted `Step 1` fence, a deleted block, a second `Step 1` fence, a query with a changed shape and a program that does not parse each red the gate with rc 1. | Added |
| AC7 | Steps 2 and 2a: a missing state file, or a `finishedAt - lastRunTime` difference outside 0 to 1000 ms, reds the gate. The Step 2a doc text states the same rule. | Issue open question, answered yes |
| AC8 | The gate is registered in all seven places, and `repo:affected-smoke`, `repo:actionlint` (check 12 markers, check 13 early-exit readers) and `repo:input-liveness` pass. | Added |
| AC9 | A verdict function whose own tool fails reds the gate (rc 1), never passes it. | Added (challenge) |

## Test strategy

- **T1. Self-test tables** (`--self-test`), run under system `/bin/bash` 3.2 and under Homebrew
  bash 5.3.15. Both must pass: the script is written for 3.2 and uses no here-strings.
- **T2. Real run** on the working tree: rc 0.
- **T3. Negative control**: rc 0, and its output shows the exact expected row set.
- **T4. Mutation battery** (per the repo memory "Red-first is not proof", "A mutation must
  compile", "Re-run a mutation battery whole"). Each mutation is applied to a scratch copy, run,
  and has ONE predicted outcome:
  - M-a: the Step 1 query in a copy of `CLAUDE.md` reads `.exitCode` instead of `.meta.exitCode`
    → rc 1.
  - M-b: remove `status` from the projection in a copy of `CLAUDE.md` → rc 1 (`wrong-exec`).
  - M-c: replace the `env -i` allowlist line in `run.sh` with a plain `env` (all variables
    inherited) and run the gate from inside `moon run repo:moon-diagnosis-exec` in a SCRATCH
    CLONE of the repo, never in the main checkout → rc 2. The plan measures which arm fires: the
    outer report hash changes, or moon errors on the unknown base sha and writes no fixture
    report. Both arms are rc 2.
  - M-d: delete the `step1_verdict` call in the real run → `ci_targets.py`'s pin reds
    `repo:affected-smoke`.
  - M-d2: delete the `step2_verdict` call in the real run → `ci_targets.py`'s pin reds
    `repo:affected-smoke`.
  - M-d3: make the Step 2a compare always pass (for example replace the bound with a constant
    true) → `--self-test` reds on the 1001 ms and -1 ms cases.
  - M-e: make the fixture's `fail3` task pass → rc 2 (the fixture-run check of unit 4).
  - M-f: change the fixture `.prototools` pin to `2.5.2` while the repo pin is 2.5.3 → rc 2 (the
    version check, or a failed start of an uninstalled version; the plan records which).
  - M-g: delete the negative control's exact row-set assertion → `ci_targets.py`'s pin reds
    `repo:affected-smoke`.
  - M-h: make the unit 6 `jq` program invalid → rc 1 with a `verdict-error` row, never rc 0.
  After any fix, re-run the whole battery.
- **T5. Gate graph**: `git add` the new files first (check 12 reads the index, README L31). Then
  run `repo:affected-smoke` under `/bin/bash` 3.2, `repo:actionlint` under bash 5 (only if its
  pipe preflight passes; else in a Linux container, per the memory "Small pipe: run gates in a
  Linux container"), and `repo:input-liveness`. CI is the final verdict.
- **T6. CI shape**: set `CI=true` and the `GITHUB_*` variables from M5 in the OUTER environment
  and run the real mode once more. Expected rc 0, and the captured nested output prints the real
  base sha (not `N/A`). This proves that the allowlist removes the CI variables.
- **T7. Linux**: before the plan is final, run the gate one time in `docker run ubuntu:24.04`
  with moon 2.5.3 installed through proto (no host software installed). Record the
  `command not found` text, exit code 127, the timeout behaviour, the Step 2a difference range and
  the run time. The plan fixes any expectation that differs from macOS.

## Constraints

- This spec and the plan carry `<!-- moon-diagnosis:ok -->` on line 1, because they name
  `ciReport` and describe the corrected procedure (check 12).
- Do not quote the two `ci-targets` marker comments in any new text in the root `CLAUDE.md`.
- New text in the repo is written in STE.
- The fixture follows the SMA-708 rule for git fixtures and turns off commit and tag signing.
- The gate script must not call `proto` and capture its output. If a later change does, the top
  `export PROTO_REPORTER=text` covers it.

## Out of scope

- Step 0 and Step 3 of the procedure and the "What cannot work" paragraph. They are prose and
  commands that change state; executing them proves little.
- Diagnosis advice in other files of the check-12 corpus (README L32). The gate reads only the
  root `CLAUDE.md` block.
- The CI note ("In CI this procedure does not apply as written").
- Retries (`retryCount > 0`). SMA-711 did not measure them. The `exec` array form allows several
  entries, and the verdict accepts any matching entry.
- Pinning `jq` through `.prototools`.

## Residual risk

- The gate runs only when `CLAUDE.md`, `.prototools` or `ci/moon-diagnosis/**` changes. A moon
  upgrade that arrives by another path (for example a changed `moonrepo/setup-toolchain` action
  that ignores the pin) does not schedule it. The version assertion in unit 4 reds only when the
  gate runs.
- `jq` comes from the runner image in CI and from Homebrew locally. A `jq` version difference can
  change the output of the documented query. That is also what a reader sees, so it is
  acceptable.
- The nested `moon ci` can abort with the proto-shim EACCES error that `ci/CLAUDE.md` records for
  `repo:affected-smoke` under a concurrent `moon ci`. The gate reports it as rc 2. It was not
  measured for this gate.
- A fresh CI runner may download moon plugins on the first nested start. M1 was measured on a warm
  host only.
- The Step 2a bound of 1000 ms rests on 240 macOS samples with a maximum of 15.6 ms. A heavily
  loaded CI runner can have a larger gap. T7 measures Linux. If a false red occurs later, the
  bound is the first thing to re-measure, not to widen without data.
- The nested run models a local run only. The CI-mode behaviour of `--base` (M5) is not checked
  by this gate, and the procedure does not claim it.

## Open questions

All three questions are answered (Sven, 2026-09-28). See "Approval decisions".

1. Name of the gate: `repo:moon-diagnosis-exec` is proposed. Sven may prefer another name.
   **ANSWERED: `repo:moon-diagnosis-exec` is accepted (D1).**
2. Steps 2 and 2a (the issue's second open question) are in scope in this spec (AC7). They check
   moon behaviour, not doc text, and they now also change the Step 2a doc text (§7). Sven may
   want Step 1 only, to keep the gate small.
   **ANSWERED: Steps 1, 2 and 2a all stay in scope, with the Step 2a doc-text change (D2).**
3. The Step 2a tolerance of 1000 ms is a design choice from macOS data (M7). Sven may prefer a
   different bound. T7 adds the Linux data before the plan is final.
   **ANSWERED: 0 to 1000 ms is accepted. T7 runs in a Linux container before the plan is
   final (D3).**

## Challenge changelog

Verdict: **APPROVE WITH CHANGES** (spec-challenger, Opus, 2026-09-27). Each finding was checked
against the repo and the probe files before it was folded.

Folded:
- BLOCKER (Step 2a exact equality). Confirmed and made stronger by a new measurement (M7): over
  240 task results the difference was 0.127 ms to 15.611 ms, and 134 values were 1 ms or more.
  Exact equality would red most runs. Unit 7 now uses `0 <= finishedAt_ms - lastRunTime <= 1000`,
  the self-test has cases at 0, 1000, 1001 and -1 ms, and the Step 2a doc text states the
  conversion and the tolerance (§7, AC7).
- MAJOR (`PROTO_MOON_VERSION` overrides the fixture pin). Confirmed in the envdump. Option (a):
  the allowlist environment removes every `PROTO_*` variable except `PROTO_HOME` and
  `PROTO_REPORTER`. M3 lists the full `PROTO_*` set. M-f is kept, now valid.
- MAJOR (M5 misread). Confirmed: `g2.out` line 2 prints `Base revision: N/A`, `out.txt` line 2
  prints the sha. M5 is corrected. Unit 4 uses `env -i` with an allowlist; M9 measures that it
  works. T6 now asserts the real base sha in the nested output.
- MAJOR (fixture commits do not turn off signing). Confirmed (M10). Unit 3 now uses
  `GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1` and local `commit.gpgsign false` and
  `tag.gpgsign false`. AC3 says "with 1Password locked".
- MAJOR (verdict functions can fail open). Folded: every verdict routes its own tool status to a
  `verdict-error` row; `bad-exec` covers an object, `null` or absent `exec`; the negative control
  asserts the exact row set; AC9 and M-h added.
- MAJOR (pin list incomplete). Folded: §6.5 now pins every check-carrying call and its rc guard;
  M-d2 and M-d3 added.
- MINOR (Open question 4 has an answer). Folded: `ci/moon-diagnosis/**/*` goes into both
  `moon.yml` and `T_AFFECTED_SMOKE_REQUIRED_INPUTS`; the `moon.yml` edit is listed in §2.
- MINOR (more files can need a check-12 marker). Folded: the path lives in one `REPORT_REL`
  variable, and §2 lists every file that will contain the token. `git grep` confirmed that
  `ci_targets.py` and `moon.yml` do not contain it today.
- MINOR (`:ok` marker on a file with the broken query). Folded with the comment option: the
  literal carries a comment that names it as the control input. No allowlist row, because the
  file also carries the corrected procedure use.
- MINOR (nothing checks that the fixture ran). Folded into unit 4 as a gate-owned `jq` check,
  rc 2.
- MINOR (M4 reason for the "no probe action" arm). Folded: unit 4 hashes the outer report file
  before and after the nested call. M-c now has one predicted outcome (rc 2) with two possible
  arms that the plan records.
- MINOR (Open question 3). Folded: a second `Step 1` fence is `multiple-step1`, rc 1.
- MINOR (`query_shape_verdict` denylist). Folded: unit 2 is a positive shape check, and the gate
  calls `jq` directly with the extracted program. This also answers the challenger's question
  about which `bash` runs the query: none does.
- MINOR (check 13 applies). Folded into §2.
- MINOR (the `ci/CLAUDE.md` rule is too wide). Folded: the rule is limited to a nested call that
  targets a different workspace, and names the `repo:affected-smoke` exception.
- MINOR (cost evidence is weak). Folded: M1 cites the 2.03 s timestamps; the plan measures all
  three modes.
- MINOR (all measurements are from macOS). Folded as T7, before the plan is final.
- MINOR (M-g has two outcomes). Folded: M-g predicts the pin red, and §4 defines the control's rc
  map.
- MINOR (position in `T` not given). Folded: after `:helm-render`, before `:test-e2e`.
- QUESTIONS: local or CI run → local (the procedure says "for local runs"). Doc or behaviour for
  Step 2a → both; the doc now states the conversion and the tolerance. Which `bash` → none; the
  gate calls `jq` directly.

Rejected: none. One detail differs from the suggestion: M-f keeps a single predicted rc (2) but
leaves the arm (version mismatch or failed start) to a plan measurement, because an uninstalled
2.5.2 with `PROTO_AUTO_INSTALL` unset was not measured.

## Re-check against origin/main (2026-09-28, base `52d51f82`)

Each file path, line number and assumption in this spec was checked against origin/main at
`52d51f82`. These items changed:

- The root `CLAUDE.md` sentence "No gate runs this query …" is at lines 189-190, not line 190.
  The Problem section, §2 and §7 now say lines 189-190. §2 also names the Step 2a text at lines
  202-204.
- `ci/actionlint/README.md` L32 starts at line 561, not line 563. The range is now 561-569.
- The `ci/helm-render/**/*` entry with its comment in the root `moon.yml` inputs of
  `repo:affected-smoke` is at lines 252-257, not 251-256.
- `T_AFFECTED_SMOKE_REQUIRED_INPUTS` ends at line 2166, not 2165. Its `ci/helm-render/**/*`
  entry is at lines 2160-2163, directly before the `'CLAUDE.md'` and `'.prototools'` entries.
  The new entry goes between `ci/helm-render/**/*` and `'CLAUDE.md'`.
- §2 now states that `ci/CLAUDE.md` and `.github/workflows/ci.yml` already contain the token
  `ciReport` on origin/main. The earlier list named only new files, and a reader could think
  that these two files were new check-12 cases.

These items were checked and are unchanged: `DOC_DIAGNOSIS_REQUIRED_LITERALS` at
`ci/actionlint/run.sh:4698` (five literals); README L29 at line 548; the `ci.yml` `T=(…)` array
(line 267) ends `:helm-render :test-e2e`, and the root `CLAUDE.md` `ci-targets` block ends the
same way; `.prototools` pins `moon = "2.5.3"`; commit `81451a82` is the SMA-711 fix; the
pre-SMA-711 query is in the fence at lines 310-315 of the SMA-597 spec (the cited lines 311-314
are the query inside that fence); `ci/release-plan/run.sh:220-238` and
`ci/release-plan/release_plan.py:1428-1446` hold the cited signing and `GIT_CONFIG_GLOBAL`
text; `ci/CLAUDE.md` lines 292-316 hold the early-exit-reader rule, the section "Registering a
new `repo:*` gate" is at line 109, and "Affectedness and task inputs" is at line 12;
`ci/affected-graph/ci_targets.py` has `REQUIRED_REPO_TASKS` (line 186),
`SELF_TASK_EXPECTED_GLOBS` (line 234) and `SELF_SCHEDULED_GATES` (line 504), and no
`MOON_DIAGNOSIS_SH_CALL_SITES` yet; no `ci/moon-diagnosis/` directory and no
`moon-diagnosis-exec` task exist yet.
