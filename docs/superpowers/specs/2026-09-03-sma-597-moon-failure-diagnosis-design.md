# SMA-597 — Diagnosing an unattributed `moon ci` failure

**Status:** design (rev 1)
**Issue:** SMA-597
**Branch:** `feature/sma-597-ci-cireport-moon-failure-diagnosis`
**Verified against `main` @ `87b9dfc` (moon 2.5.3, proto 0.61.1, actionlint 1.7.12).**

Sixty-seven documents in `docs/superpowers/` tell the reader that an unattributed `moon ci`
failure is diagnosed by reading `.moon/cache/ciReport.json`. Following that advice produces a
confident dead end: the reader gets a `status: "failed"` row, reads `null` where an exit code
should be, and concludes there is nothing more to find. SMA-595 did exactly this and shipped a
CLAUDE.md gotcha saying the cause was unknown — while the whole diagnosis sat in a file nothing in
this repo has ever mentioned.

**The issue's premise is half wrong, and the wrong half is what makes the file look worthless.**
`ciReport.json` does carry the failing task's command and its real exit code. What it lacks is
stdout and stderr. §1 establishes both, and §1.3 establishes where the output actually lives.

**The fix is not the one the issue proposes.** AC3 asks for the plan template to be corrected;
there is no plan template (§1.5). The advice propagates by imitation from prior plans, which is
why it is still spreading — 53 of 104 documents when the issue was filed six days ago, 62 of 119
today. So the corrected procedure goes where every session reads it, and a gate stops the corpus
growing.

Counted precisely, because the issue and this spec measure different sets: the issue reported 53
of 104 **plan** documents on 2026-08-28. Today that same set is 62 of 119. The issue did not count
the **spec** documents, of which 5 more carry the advice — 67 files in total (§1.6).

---

## 1. Measured baseline

All measurements in this section were taken on this branch at `87b9dfc`, moon 2.5.3, using a
temporary `repo:ow-probe` / `repo:diagnose-probe` task added to `moon.yml` and reverted before
any commit. The probe scripts and their raw output are reproduced inline rather than summarized,
because AC1 and AC2 both require the measurement to be on the record.

### 1.1 There is no action-level `exitCode` key

The advice's characteristic query is `jq '.actions[] | select(.status == "failed")'`. Applied to
a real failure it reports:

```
$ jq '.actions[] | select(.status=="failed") | {label, status, exitCode: .exitCode}' \
     .moon/cache/ciReport.json
{ "label": "RunTask(repo:osv)", "status": "failed", "exitCode": null }
```

That `null` is **absence, not a written null**:

```
$ jq '.actions[] | select(.status=="failed") | has("exitCode")' .moon/cache/ciReport.json
false
```

moon never writes an `exitCode` key at action level. Every reader who concluded "moon records a
null exit code" was reading `jq`'s report of a key that does not exist. This is the single fact
that turned a usable file into an apparent dead end.

### 1.2 The report does carry the command and the real exit code

They live one level down, in the action's `operations[]`, on the entry whose `meta.type` is
`task-execution`. Measured against a probe scripted to `exit 3`:

```
$ jq '.actions[] | select(.label|test("diagnose-probe")) | .operations[]
      | select(.meta.type=="task-execution") | .meta' .moon/cache/ciReport.json
{
  "type": "task-execution",
  "command": "echo \"PROBE-STDOUT-MARKER\"; echo \"PROBE-STDERR-MARKER\" >&2; exit 3",
  "exitCode": 3
}
```

Note that the issue's own quoted query — `select(.status == "failed")` with no field projection —
prints the entire action *including* `operations`, so it already surfaces this. The dead end came
from the projected form (`{label, status, exitCode}`), which is the shape most of the 67 documents
actually use.

**Consequence for the design:** the file is not to be abandoned. It is step 1 of a working
procedure, queried correctly.

### 1.3 The output lives in per-task log files

Undocumented anywhere in this repo before this issue:

```
.moon/cache/states/<project>/<task>/stdout.log
.moon/cache/states/<project>/<task>/stderr.log
.moon/cache/states/<project>/<task>/lastRun.json
```

Measured on the same probe:

```
$ cat .moon/cache/states/repo/diagnose-probe/stdout.log
PROBE-STDOUT-MARKER
$ cat .moon/cache/states/repo/diagnose-probe/stderr.log
PROBE-STDERR-MARKER
$ cat .moon/cache/states/repo/diagnose-probe/lastRun.json
{"exitCode":3,"hash":"","lastRunTime":1788420461906,"target":"repo:diagnose-probe"}
```

110 such log pairs were present in the main checkout at the time of measurement. They are written
even for a task declaring `options.cache: false` — both probes did, as does `repo:osv`, so the
mechanism is not a side effect of caching.

### 1.4 No moon 2.5.3 invocation puts output in the report (AC2)

`moon ci` exposes `-s, --summary [none|minimal|normal|detailed]`. Measured, running the same
failing probe twice, once plain and once with `--summary detailed`:

- **Console output is byte-identical** apart from timing values. `diff` of the two captures,
  with the probe's own marker lines filtered out, reports only four lines, all of the form
  `Time: 35ms` → `Time: 30ms`.
- **The report is unchanged.** The union of `meta` keys across every operation is
  `{command, exitCode, hash, type}` under both invocations. No output field appears.

`--log-file` and `MOON_LOG_FILE` capture moon's *own* tracing output, not a task's stdio, and are
not a substitute.

**AC2's answer is therefore negative and final for 2.5.3: `ciReport.json` cannot be made to carry
stdout/stderr.** This is a version-pinned measurement and must be re-taken on a moon bump, the
same treatment CLAUDE.md gives its other measured moon claims.

### 1.5 There is no plan template

AC3 says "the plan template is the place to fix that". No such template exists:

```
$ grep -rn "ciReport" ~/.claude/plugins/cache/
$ # (no output — superpowers:writing-plans does not ship the line)
$ git ls-files | grep -v '^docs/' | xargs grep -ln 'ciReport'
$ # (no output — nothing outside docs/ mentions it)
```

The advice propagates by **imitation**: an agent writing a new plan reads recent plans as
examples and copies the phrasing. That mechanism has no single edit point, which is why the
corpus is still growing and why AC3 cannot be satisfied as written. §3 delivers its intent
instead.

### 1.6 The corpus, and the token's selectivity

```
$ git ls-files -- 'docs/**/*.md' | xargs grep -l 'ciReport' | sed 's|/[^/]*$||' | sort | uniq -c
  62 docs/superpowers/plans
   5 docs/superpowers/specs
```

67 files. The token `ciReport` appears in **zero** tracked files outside `docs/` — not in
CLAUDE.md, not in CONTRIBUTING.md, not in `ci/`. That selectivity is what makes a bare-token gate
viable in §3.1 without a single false positive on today's tree.

The 5 spec files are worth naming because the issue counted only the 62 plans: they are the
SMA-376, SMA-534, SMA-546, SMA-528 and SMA-596 design documents.

### 1.7 Re-running destroys all four artifacts (AC4)

The issue states that `buffer-only-failure` discards a passing task's console output, so a
non-reproducing failure's evidence is lost on re-run. That is true but understates the problem,
and misidentifies the mechanism.

Measured directly. A probe scripted to fail, then edited to pass, then re-run:

```
# after the FAILING run
lastRun: {"exitCode":7,"hash":"","lastRunTime":1788424159921,"target":"repo:ow-probe"}
stdout : RUN-ONE-FAILING
stderr : ERR-ONE

# after the PASSING re-run
lastRun: {"exitCode":0,"hash":"","lastRunTime":1788424167811,"target":"repo:ow-probe"}
stdout : RUN-TWO-PASSING
stderr : []                      <- truncated to zero bytes
ciReport: ['passed']             <- the failed action is gone from the report
```

So a re-run does not merely fail to *show* the old output. It **overwrites every artifact that
held it**: both task logs (stderr truncated to empty), `lastRun.json`, and the failing action's
row in `ciReport.json`. The hazard is overwrite, not discard, and a passing run is just as
destructive as another failing one.

Corroborated independently on real data. The `repo:osv` failure snapshotted from the main
checkout records `exitCode: 1` at 00:24, while that task's `lastRun.json` and `stdout.log` on
disk are from a *passing* 07:15 run. The 00:24 evidence was already gone before anyone looked.

**Consequence for the design:** "capture before re-running" is not advice, it is the first step.
It must appear in the procedure ahead of the diagnostic steps, not as a footnote after them.

---

## 2. The corrected procedure

Lands in CLAUDE.md, wrapped in `<!-- moon-diagnosis:begin -->` / `<!-- moon-diagnosis:end -->`
markers. CLAUDE.md is chosen because it is the only document loaded into every session — which is
the same channel imitation travels on, and therefore the only place a correction outruns it.

Substance, in order:

0. **Capture first.** Copy `.moon/cache/ciReport.json` and
   `.moon/cache/states/<project>/<task>/` somewhere outside the repo before re-running anything.
   §1.7 is the reason.
1. **Which task, what command, what exit code** — from the report, at the correct path:

   ```
   jq '.actions[] | select(.status=="failed")
       | {label, error,
          exec: (.operations[] | select(.meta.type=="task-execution")
                 | {command, exitCode})}' .moon/cache/ciReport.json
   ```

   With the explicit note that there is no action-level `exitCode` key, so the older projected
   query reports `null` for a key moon never writes.
2. **Why** — `cat .moon/cache/states/<project>/<task>/stdout.log` and `stderr.log`. This is the
   only place task output exists.
3. **If it still does not reproduce** — `moon run <target> --force`, with the note that
   `buffer-only-failure` shows a failing task's output on the console but discards a passing
   task's.

Plus the negative result from §1.4, so the next reader does not re-litigate `--summary detailed`.

The entry also corrects the `buffer-only-failure` framing per §1.7: a passing task's output is
not discarded into nothing, it is written over the failing run's log.

---

## 3. The gate — check 12 in `repo:actionlint`

### 3.1 Placement: no new gate

A new `repo:*` gate carries seven registration obligations (CLAUDE.md's `repo:ruff-ci` entry
enumerates them). `repo:actionlint` already hosts cross-cutting pins that are not about
workflows — checks 8b–8f pin other gates' wiring into `ci_targets.py` and `ci.yml`, checks 10–11
delegate to `release_guard.py` and `release_plan.py`. A docs-corpus check is the same kind of
tenant.

Decisive practical reason: `repo:actionlint` declares `inputs: ['**/*']`, pinned from
`SELF_TASK_EXPECTED_GLOBS["actionlint"]`. That is exactly the reachability a docs check needs —
it must run on the PR that adds a new plan document, and a narrower `inputs` list would be the
SMA-553 failure class all over again.

Cost, stated: `ci/actionlint/run.sh` grows further as a grab-bag, and `SELF_TEST_COUNT` moves
13 → 14.

### 3.2 Assertion A — corpus freeze

Every tracked file containing the token `ciReport` must appear in `CIREPORT_MENTIONS_ALLOWED`, a
table of path → reason. Seeded with:

- the 67 files from §1.6, reason `historical record, pre-SMA-597`;
- `CLAUDE.md`, reason `carries the corrected procedure`;
- `docs/superpowers/specs/2026-09-03-sma-597-moon-failure-diagnosis-design.md` and the
  implementation plan written from it, reason `documents the defect itself`. The plan's exact
  path is known only once `superpowers:writing-plans` has produced it, so it is added to the
  table in the same commit that adds the plan — the gate reds until it is, which is the intended
  behaviour.

Any other tracked file containing the token reds the gate.

**Subset, not strict equality.** The gate asserts `offenders ⊆ allowlist`, so removing the advice
from a grandfathered file is not a failure. This departs from the repo's usual strict-equality
pins (`EXPECTED_PR_SUBJECTS`, `CONTRACTS_GENERATE_INPUTS`), and the departure is deliberate: those
pin a set that should stay put, while this one pins a set that should only ever shrink. Making a
cleanup red the gate that authorized it would push people toward loosening the gate. The reason
goes in a comment at the table, in the repo's house style.

**Why the bare token rather than a pattern.** §1.6 measured the token at zero occurrences outside
`docs/`, so it is fully selective today. The alternative — pattern-matching the advice's shape
(`jq` + `select(.status=="failed")` + `ciReport` in proximity) — was rejected on this repo's own
evidence: SMA-554 is an open issue recording that its pattern-matched check was bypassed four
separate times during review while the exact-literal check next to it was bypassed once. A
literal set has no tail to enumerate.

### 3.3 Assertion B — marker integrity

The `moon-diagnosis` block exists in CLAUDE.md; each marker appears exactly once; they appear in
order; the block between them is non-empty. This mirrors the `ci-targets` marker discipline
CLAUDE.md already documents, including its warning that a second copy of a marker anywhere in the
file — even inside backticks in prose — breaks the count.

Assertion B is what stops the §2 correction being silently deleted, which would switch the whole
fix off while leaving the gate green.

### 3.4 Self-test

One new table, `doc_diagnosis_self_test()`, driving a pure verdict function that takes
(offending paths, allowlist) and returns rows — no filesystem, consistent with the other twelve
tables. `SELF_TEST_COUNT` 13 → 14. Check 7 asserts invocations *and* definitions, so both halves
must move together; check 9's mutation battery already proves that counter fires and the new
table inherits that coverage without modification.

Cases: a new offender reds; a grandfathered offender passes; a *removed* offender passes (the
subset rule of §3.2); a missing marker reds; a duplicated marker reds; markers out of order red;
an empty block reds.

---

## 4. Files touched

| File | Change |
| -- | -- |
| `CLAUDE.md` | new marker-delimited gotcha (§2); correction to the existing `buffer-only-failure` claim |
| `ci/actionlint/run.sh` | check 12 + `doc_diagnosis_self_test()`; `SELF_TEST_COUNT` 13 → 14 |
| `ci/actionlint/README.md` | check 12 documented; its residual added to Limitations |
| `docs/superpowers/specs/2026-09-03-sma-597-moon-failure-diagnosis-design.md` | this document |

No `moon.yml` change, no `ci.yml` change, no `ci_targets.py` change — that is the whole point of
§3.1. The probe tasks used throughout §1 are reverted; `moon.yml` is byte-identical to `main`.

---

## 5. Non-goals and limitations

1. **The 67 existing documents are not edited.** They are dated records of what was believed when
   written, and the issue argues this explicitly. The gate grandfathers them.
2. **L1 — the gate keys on files, not content.** A grandfathered file that gains a *new*
   paragraph of broken advice passes. Closing this needs content analysis of prose, which is the
   pattern-matching approach §3.2 rejected on SMA-554's evidence. Accepted, and recorded in the
   README's Limitations section.
3. **L2 — nothing gates the procedure's *correctness*, only its presence.** Assertion B proves
   the block is there and non-empty. If someone edits the `jq` inside it into something wrong,
   the gate stays green. A gate that executed the procedure against a deliberately failed task
   would close this; it is out of scope here and worth its own issue if the procedure ever drifts.
4. **L3 — §1.4's negative result is pinned to moon 2.5.3.** A bump can change it. The CLAUDE.md
   entry says so, matching how the repo already treats its other measured moon claims.
5. **The `repo:affected-smoke` flake is not diagnosed here.** This issue delivers the procedure.
   Applying it to that flake — which needs a live reproduction, and §1.7 explains why the last
   two were destroyed — stays with SMA-595.
6. **No change to `buffer-only-failure`.** Switching Moon to a streaming output style would make
   failures self-attributing, but it would also make every green CI run enormous. Out of scope.

---

## 6. Acceptance criteria mapping

| AC | Where | Note |
| -- | -- | -- |
| 1 — demonstrated procedure, output recorded | §1.1–1.3, §2 | proven against two deliberately failing probes; raw output in §1 |
| 2 — can the report carry more, measured either way | §1.4 | negative, with the `--summary detailed` A/B on the record |
| 3 — new plans stop reproducing the advice | §3.2 | **AC as written is unachievable** — §1.5 shows there is no template. The gate delivers its intent. |
| 4 — `buffer-only-failure` interaction stated | §1.7, §2 | corrected: the mechanism is overwrite, not discard, and it destroys all four artifacts |

**Issue correction to carry back to Linear.** The title's second clause — "and a null
`exitCode`" — is wrong (§1.1). The exit code is present, at a path nobody queried. The premise
section should be corrected when the issue closes, because the wrong half is what made the file
look worthless and is the reason the working path went unfound for so long.
