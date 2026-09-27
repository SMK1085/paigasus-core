<!-- moon-diagnosis:ok -->
# SMA-711: the moon-diagnosis Step 1 query reads `.meta`

- Linear: SMA-711
- Path: bounded (a correction in one gated document block, plus a marker change in seven dated files)
- Date: 2026-09-27
- Revised after the adversarial challenge (verdict: APPROVE WITH CHANGES, no BLOCKER)

## Problem

The root `CLAUDE.md` holds the procedure "Diagnosing an unattributed `moon ci` failure" inside
the gated `<!-- moon-diagnosis:begin/end -->` block. Step 1 (line 180) projects
`{command, exitCode}` from each `task-execution` operation of `.moon/cache/ciReport.json`.

Moon writes `command` and `exitCode` under the operation's `.meta` object, not on the operation.
So the query prints `null` for both values. Step 1 exists only to find these two values.

The fields did not move. The SMA-597 spec (§1.2, lines 66-76) measured them under `.meta` on
moon 2.5.3. Its §2 query was then written with `{command, exitCode}` and was never run against a
report. `ci/actionlint/README.md` L29 records this class of gap: check 12 gates the presence of
five literals, not the correctness of the query. The follow-up issue that L29 names does not
exist in Linear (searched 2026-09-27).

The query has a second defect on the same line. jq builds zero objects from an empty stream. So
a failed action with no `task-execution` operation (for example, a failure in hash generation or
in a sync action) prints nothing at all, not even its `label`. Empty output then reads as "no
failed action".

## Measured facts (2026-09-27, moon 2.5.3)

Two reports, both copied into the session scratchpad and read only from there:

- **Recorded:** the main checkout's report from a failed run on 2026-09-20
  (sha256 `328750d0…72536bf`).
- **Fresh:** a throwaway moon workspace, `.prototools` pinned to `moon = "2.5.3"`, `moon --version`
  printed `moon 2.5.3`. Five explicit targets, `moon ci … --base <first commit>`, exit code 1
  (sha256 `38cd4a78…c03e41f`).

| Task (fresh unless noted) | `meta.command` | `meta.exitCode` | operation `status` |
|---|---|---|---|
| `script: 'exit 3'` | `exit 3` | `3` | `failed` |
| `script:` with two lines `echo a`, `exit 4` | `echo a exit 4` | `4` | `failed` |
| `command: 'no-such-binary-xyz'` | `no-such-binary-xyz` | `127` | `failed` |
| `command: 'sleep 20'`, `options.timeout: 2` | `sleep 20` | key absent, so jq prints `null` | `timed-out` |
| recorded `repo:actionlint` | `ci/actionlint/run.sh` | `1` | `failed` |
| recorded `repo:next-public-free` | `set -euo pipefail bash ci/next-public/run.sh --self-test bash ci/next-public/run.sh --negative-control bash ci/next-public/run.sh` | `1` | `failed` |

Other measured facts:

- The old projection `{command, exitCode}` prints `null` for both values on every failed action in
  both reports.
- The action-level `status` is only `failed` or `passed` in both reports. The timed-out task has
  the action status `failed`. `timed-out` appears only on the operation.
- Each `RunTask` action has exactly one `task-execution` operation. Retries (`retryCount > 0`) are
  not measured.
- `meta.command` is a one-line display string. Moon joins the lines of a multi-line `script:` with
  spaces. The recorded `repo:next-public-free` string is not a runnable command: in a shell, `set`
  takes the rest of the line as positional arguments.
- A synthetic report with one failed action that has only a `hash-generation` operation: the old
  query and the single-object form of the new query print nothing. The array form prints
  `{"label":"RunTask(x:y)","error":"hash failed","exec":[]}`.

## Design

### 1. The Step 1 query (`CLAUDE.md`, inside the block)

Replace the `exec:` projection with an array of operation objects:

```
exec: [.operations[] | select(.meta.type=="task-execution")
       | {status, command: .meta.command, exitCode: .meta.exitCode}]
```

- `.meta.command` and `.meta.exitCode` fix the `null` defect.
- The `[ … ]` array prints `exec: []` for a failed action that did not execute, so the action's
  `label` and `error` still appear. It prints one entry per attempt if a task has several.
- `status` is the operation's status. It tells a timeout (`timed-out`, no exit code) from a
  failure with an exit code.
- The `select(.status=="failed")` filter on the action stays. The measurement shows no other
  action-level failure status on moon 2.5.3.

### 2. The prose under the query (`CLAUDE.md`, inside the block)

Rewrite only the sentences that change. The surrounding paragraph stays as it is. New and
changed sentences are written in STE.

- The values are the `command` and `exitCode` fields of the `.meta` object of the operation
  whose `meta.type` is `task-execution`. This keeps the literals `operations[]` and
  `task-execution` in the block.
- `exec: []` means that the action failed before a task ran. Read `error`, and the other
  operations of that action.
- A `null` exit code with `status` `timed-out` means that moon stopped the task. There is no exit
  code to read.
- `command` is moon's one-line display string, not the script. For a `script:` task, read the
  script in the project's `moon.yml`, and reproduce with Step 3.
- One sentence records the defect: before SMA-711 this query projected the two fields from the
  operation itself and printed `null`, and no gate runs the query (README L29).

### 3. The seven dated copies take `:superseded`, not `:ok`

Seven dated files hold a copy of the old query and carry `<!-- moon-diagnosis:ok -->`. Check 12
defines `:ok` as "a deliberate reference to the corrected procedure". After this change, that
marker is false in these files. In five of them, the marker sits directly above a copyable
broken query.

In each file, change `:ok` to `:superseded` and add one pointer line under it, in the format that
SMA-597 used (`docs/superpowers/specs/2026-08-16-sma-376-kernel-cratesio-publish-design.md:549-553`):
the Step 1 query in this file prints `null`; the corrected procedure is in `CLAUDE.md` between
the `moon-diagnosis` markers (SMA-711). The rest of each file stays as it is. The files:

- `docs/superpowers/specs/2026-09-03-sma-597-moon-failure-diagnosis-design.md:1`
- `docs/superpowers/plans/2026-09-03-sma-597-moon-failure-diagnosis.md:1`
- `docs/superpowers/plans/2026-09-08-sma-502-next-config.md:2652`
- `docs/superpowers/plans/2026-09-09-sma-508-sdk-package-transport-iam.md:1422`
- `docs/superpowers/plans/2026-09-10-sma-509-capability-discovery.md:3601`
- `docs/superpowers/plans/2026-09-20-sma-632-whoami-rpc.md:1924`
- `docs/superpowers/plans/2026-09-20-sma-633-introspect-role-grants.md:817`

This is a scope change against the issue, which said these files stay unchanged. It changes only
the marker and adds a pointer. The historical text is not edited. Check 12 accepts either marker,
so no gate change is needed.

## Out of scope

- A gate that runs the Step 1 query against a real report (README L29). This is SMA-714, opened
  after Gate 1.
- A sixth `DOC_DIAGNOSIS_REQUIRED_LITERALS` entry such as `.meta.exitCode`. It would not have caught
  this defect, and it would not catch the next one. The execution gate is the real fix.
- Steps 0, 2, 2a and 3 and the "What cannot work" paragraph. The challenger checked their field
  paths and found them correct. Its optional Step 2a clarification (time formats of `finishedAt`
  and `lastRunTime`) is not part of this defect.
- The wording of the comment at `ci/actionlint/run.sh:4695`. It names `operations[]` and the
  `task-execution` entry, which is correct.

## Constraints

- The block keeps exactly one `moon-diagnosis:begin` and one `moon-diagnosis:end` marker, in
  that order, with a non-empty body.
- All five required literals stay between the markers: `operations[]`, `task-execution`,
  `.moon/cache/states/`, `stderr.log`, `lastRunTime`. Production check 12 is a containment check
  over the whole block, so the line a literal is on does not matter.
- Check 12 reads `git ls-files`. This spec, the plan and every changed file must be `git add`ed
  before any run of the gate or of a copy of check 12 (README L31). This spec and the plan carry
  `<!-- moon-diagnosis:ok -->` on line 1, because each file names `ciReport` and describes the
  corrected procedure.
- The root `CLAUDE.md` has a `CIREPORT_MENTIONS_ALLOWED` row, so it needs no marker.

## Acceptance criteria and verification

| SMA-711 acceptance criterion | Verification |
|---|---|
| The Step 1 query, run against a report from a failed run on the current moon, prints the real command and a non-null exit code. | V1, V2, V3 |
| `repo:actionlint` passes. | V5 |
| (added) A failed action with no task execution still appears. | V4 |
| (added) The seven dated files no longer certify the old query. | V6 |

- **V1. Fresh report.** Run the query text extracted from the edited `CLAUDE.md` (see V3) in
  `scratchpad/moon-probe/` against `ciReport.fresh.json` (sha256 `38cd4a78…`). Expected: the
  four failed probe actions with the command strings and exit codes `3`, `4`, `127`, and `null`
  with `status` `timed-out`, as in the table above. Empty output is a failed measurement.
- **V2. Recorded report.** The same extracted query in `scratchpad/recorded/`, whose
  `.moon/cache/ciReport.json` is the recorded copy (sha256 `328750d0…`). Expected: the two
  recorded actions with the exact strings in the table above and exit code `1`.
- **V3. Text in the file.** Extract the query from the edited `CLAUDE.md` with a script (the
  lines between the fence that follows "Step 1" and its closing fence), and run that text, not a
  retyped copy. Control: the same extraction from `git show origin/main:CLAUDE.md` must print
  `null` for both values in V2's directory.
- **V4. No-execution action.** The extracted query against the synthetic report prints the
  action with `exec: []`. Control: the old query prints nothing.
- **V5. Gate.** `git add` all changed files first. Run `repo:actionlint`. If the host pipe
  preflight exits rc 2 (512-byte pipe), run the gate in a Linux container, or replicate
  `claude_md_block_verdict` and check 12 with a script, and say which method was used. CI is the
  final verdict.
- **V6. Markers.** Each of the seven files has exactly one line that is exactly
  `<!-- moon-diagnosis:superseded -->`, and no line that is exactly `<!-- moon-diagnosis:ok -->`.
  (A plain search for `command, exitCode` also finds this spec and the plan, which quote the old
  query.) The new `CLAUDE.md` text must not contain the string `command, exitCode`.

## Residual risk

No gate runs the Step 1 query against a real report (README L29). A future edit can break the
query again, and nothing reds. The array form and the `status` field are measured only for
`retryCount: 0`.
