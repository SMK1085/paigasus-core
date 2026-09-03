<!-- moon-diagnosis:ok -->

# SMA-597 — Diagnosing an unattributed `moon ci` failure

**Status:** design (rev 2 — reworked after adversarial challenge; see §8 for the changelog)
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

Three axes were varied, not one. `moon ci` exposes `-s, --summary [none|minimal|normal|detailed]`;
`.moon/tasks.yml` sets `outputStyle`. Seven cells, each a forced run of the same failing probe,
reporting the union of the action's own keys plus every operation's `meta` keys:

```
--summary none      -> allowFailure,createdAt,duration,error,finishedAt,flaky,label,
                       meta.command,meta.exitCode,meta.hash,meta.type,node,nodeIndex,
                       operations,startedAt,status
--summary minimal   -> (identical)
--summary normal    -> (identical)
--summary detailed  -> (identical)
outputStyle stream  -> (identical)
outputStyle buffer  -> (identical)
outputStyle none    -> (identical)
```

All seven are byte-identical. Console output under `--summary detailed` is likewise identical to
plain `moon ci` apart from timing values.

A key-name walk over the **entire** failing action — every nesting level, not just
`operations[].meta` — finds no key whose name suggests output. The only free-text field is
`error`, whose value is the fixed string `Task repo:m-fail failed to run.`

`--log-file` / `MOON_LOG_FILE` capture moon's *own* tracing output, not a task's stdio, and are
not a substitute.

**AC2's answer is negative across everything measured: no `--summary` level, no `outputStyle`
value, and no field anywhere in a failing action carries stdout or stderr.** Stated with its
bounds, per the challenge: three axes were varied and the action was walked exhaustively; moon's
config surface as a whole was not enumerated. Pinned to 2.5.3 and to be re-taken on a bump, the
treatment CLAUDE.md gives its other measured moon claims.

### 1.5 There is no plan template

AC3 says "the plan template is the place to fix that". No such template exists. Searched for the
*advice*, not only the token — the challenge's objection to rev 1's single-token grep:

```
$ grep -rn "ciReport" ~/.claude/plugins/cache/          # (no output)
$ grep -rln 'moonrepo\|\bmoon\b' <superpowers 6.3.0>/skills/
                                                        # (no output — no skill mentions moon AT ALL)
$ grep -rin 'cache\|report\|jq\|exit code' <…>/skills/writing-plans/
                                                        # (no output)
$ git ls-files | grep -v '^docs/' | xargs grep -ln 'ciReport'
                                                        # (no output — nothing outside docs/)
```

superpowers 6.3.0 does not mention moon under any phrasing, so `writing-plans` cannot be emitting
moon-specific diagnosis advice in different words. The claim is now "no skill in the installed
plugin set mentions moon at all", which is strictly stronger and does not depend on the token.

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
CLAUDE.md, not in CONTRIBUTING.md, not in `ci/`.

**That command is wrong, and the gate must not copy it.** `git ls-files -- 'docs/**/*.md'` matches
**zero** top-level `docs/*.md` files, because git matches a pathspec's `**` without `FNM_PATHNAME`
and the literal `/` is still required — the same trap CLAUDE.md records for `repo:ruff-ci`'s
corpus. Measured: `docs/dev-setup.md` is tracked and is not matched. The count of 67 is correct
only by luck, because that file happens not to contain the token. §3.2 therefore specifies the
gate's corpus as **unfiltered `git ls-files`** over the whole tracked tree, which has no pathspec
to get wrong and closes the `docs/anything.md` bypass at the same time.

Selectivity is what makes a bare-token rule viable — but only until the gate exists. See §3.2's
seed, which must include the gate's own two files (§1.9).

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

**Consequence for the design:** "capture before re-running" is not advice, it is the first step.
It must appear in the procedure ahead of the diagnostic steps, not as a footnote after them.

### 1.8 The report and the logs are not one snapshot — and rev 1 got this wrong

Rev 1 cited the `repo:osv` case as corroboration of §1.7. The adversarial challenge was right that
it is **the opposite**: a counterexample that rev 1 mislabelled. The snapshot shows
`ciReport.json` recording `exitCode: 1` at 00:24 while that same task's `lastRun.json` and
`stdout.log` are from a *passing* 07:15 run. If a re-run overwrote everything, those two could not
disagree.

Two further measurements explain why, and both matter to the procedure:

- **`moon run` does not write `ciReport.json`.** It writes `runReport.json`. Measured by deleting
  `ciReport.json`, running `moon run repo:m-fail --force`, and finding it absent afterwards while
  `runReport.json` appeared. So a `moon run` re-run rewrites the **task logs** and leaves the
  **report** untouched — which is exactly how `repo:osv` came to hold a 00:24 report row beside a
  07:15 log.
- **A cache hit rewrites neither.** Measured: a forced run, then an unforced re-run that hit the
  cache, left `lastRunTime` and the log's mtime unchanged. So a log can be arbitrarily older than
  the run being diagnosed, with nothing in either file saying so.

**This is the most dangerous finding in the spec, because it makes the naive procedure produce a
confident wrong answer** — the same defect class SMA-597 exists to fix, one level up. A reader who
takes the command and exit code from step 1 and the output from step 2 may be pairing two
different runs, and neither file warns them.

The procedure therefore requires a **cross-check**, not a footnote: compare the report action's
`finishedAt` against `lastRun.json`'s `lastRunTime`. If they disagree, the logs belong to a
different run and the evidence for this one is gone. §2 makes that step 2a, mandatory.

### 1.9 The gate's own source becomes an offender

A bare-token rule needs the literal string `ciReport` in `ci/actionlint/run.sh` (as the search
pattern) and in `ci/actionlint/README.md` (documenting check 12). Measured: **no file under `ci/`
contains the token today**, so both become new offenders at the instant check 12 is introduced,
and §1.6's "no false positives" holds only up to that commit.

Rev 1 did not notice this. The three ways out, and why one wins:

- **(a) allowlist both files.** The gate is then structurally blind to the token in its own two
  files. Chosen — it is honest, one table entry each, and the blindness is recorded in §5.
- **(b) obfuscate the pattern** (`ci''Report`). Rejected: it defeats §3.2's "a literal set has no
  tail" argument, and `run.sh:92-93` already records a ShellCheck-directive bug caused by exactly
  this kind of clever string.
- **(c) exclude `ci/**` from the corpus.** Rejected: it creates a free bypass — put the advice in
  any `ci/**/README.md` and no gate sees it.

### 1.10 In CI the procedure is unusable, and the logs are actively misleading

Rev 1 never mentioned CI. "An unattributed `moon ci` failure" most often means a red CI check, so
this is the common case, not an edge.

- **Step 0 is unexecutable.** There is no interactive shell in GitHub Actions and the runner is
  destroyed at job end. "Copy the files somewhere before re-running" has no meaning there.
- **The logs may predate the commit.** `ci.yml:113-120` caches `.moon/cache` with
  `restore-keys: moon-${{ runner.os }}-`, so `.moon/cache/states/**` is restored from a previous
  run on a *different commit*. Combined with §1.8's cache-hit result — a hit rewrites nothing — a
  task that hit the cache in this run carries logs from an older one. Reading them as this run's
  output is the §1.8 skew with a wider gap.

`ci.yml:197-206` already deletes a stale `junit.xml` defensively for exactly this class, so the
hazard is recognised in this repo; it is simply unguarded for moon's states dir.

**Design consequence, and a scope addition over rev 1:** the CLAUDE.md block states plainly that
its steps are for **local** runs, and `ci.yml` gains an `if: failure()` step that uploads
`.moon/cache/ciReport.json` and `.moon/cache/states/**` as an artifact — mirroring the existing
`nextest-junit` upload at `ci.yml:257-259`. Without it, the procedure helps in the case that
matters least and misleads in the case that matters most. This means **§4's rev-1 claim of "no
`ci.yml` change" is withdrawn.**

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
   only place task output exists. Measured to work for the motivating class: a task whose command
   does not exist at all (exit 127) writes an empty `stdout.log` and a `stderr.log` containing
   `bash: line 1: …: command not found` — so the proto-shim abort that started this issue is
   recoverable here.
2a. **Prove the logs belong to this run** — compare the report action's `finishedAt` with
   `lastRun.json`'s `lastRunTime`. **If they disagree, stop**: the logs are from a different run
   (§1.8), and pairing them with step 1's command is how you get a confident wrong answer. This
   step is mandatory, not advisory, and it is the one that distinguishes this procedure from the
   one it replaces.
3. **If it still does not reproduce** — `moon run <target> --force`, with two warnings:
   `buffer-only-failure` shows a failing task's output on the console but discards a passing
   task's, and `moon run` rewrites the logs while leaving `ciReport.json` untouched, which is
   precisely how the two fall out of step (§1.8).

Plus the negative result from §1.4, so the next reader does not re-litigate `--summary`, and the
scope statement from §1.10 that these steps are for local runs.

The entry also corrects the `buffer-only-failure` framing per §1.7: a passing task's output is
not discarded into nothing, it is written over the failing run's log.

**On when a failure is "unattributed" at all.** `buffer-only-failure` does print a failing task's
output, so the procedure's value is *evidence recovery* in the three cases where that print is
absent or useless: the scrollback was not captured (an agent session that moved on), the task
produced no console output before dying (the SMA-592 proto-shim abort), or the failure is being
diagnosed after the fact from a cache directory. The block says so, so a reader knows whether it
applies before working through it.

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

Cost, stated fully — rev 1 understated this:

- `ci/actionlint/run.sh` grows further as a grab-bag, and the README's "## Why" section (entirely
  about workflow `paths:` filters going dead) no longer explains its own contents. §4 amends it.
- `SELF_TEST_COUNT` moves 13 → 14, and check 9's mutation battery goes from 14 to 15 concurrent
  `--self-test` subprocesses. The new table is pure — no filesystem, no subprocess — so the
  marginal wall-clock cost is a few milliseconds against the battery's existing cost, but the
  measured table at `ci/actionlint/README.md:661-667` is invalidated and must be re-taken.
- `repo:actionlint` is **not** on `REQUIRED_REPO_TASKS` (`ci_targets.py:156-173`). That floor
  exists for gates carrying a `--negative-control`, which this one does not, so no entry is
  added — but it means check 12's only protection against `repo:actionlint` being dropped from
  `T` is check 8's own `:affected-smoke` entry. Recorded here rather than left implicit.
- **Contrary to rev 1, `ci_targets.py` IS touched** (§3.5).

### 3.2 Assertion A — corpus freeze

**Corpus derivation, specified verbatim** (§1.6 explains why this is not a detail):

```bash
git ls-files -z | xargs -0 grep -l 'ciReport' 2>/dev/null
```

Unfiltered `git ls-files` over the whole tracked tree — no pathspec, so there is no `**` trap to
fall into and no `docs/anything.md` bypass. It reads the **index**, so a file that is written but
not yet `git add`ed is invisible: a local run can be green where CI is red. §3.2's "the gate reds
until the entry is added" is true only post-`git add`, and the README says so.

Every tracked file containing the token `ciReport` must satisfy **one** of three conditions
(§7 — the marker route was accepted at review, which is what keeps the table small):

1. it carries `<!-- moon-diagnosis:superseded -->` — a historical record, annotated;
2. it carries `<!-- moon-diagnosis:ok -->` — a deliberate reference to the corrected procedure;
3. it appears in `CIREPORT_MENTIONS_ALLOWED`, a table of path → reason with a **non-empty**
   reason (a blank one is itself an assertion failure, matching `T_EXEMPT` at
   `ci_targets.py:139-145` and `ALLOW_NO_CARGO_BACKING`).

The allowlist is deliberately tiny — **three entries**, for the files where a markdown comment
does not belong or where self-certification would be circular:

- `ci/actionlint/run.sh`, reason `the gate's own search pattern` (§1.9);
- `ci/actionlint/README.md`, reason `the gate's own documentation` (§1.9);
- `CLAUDE.md`, reason `the corrected procedure itself — the authority does not self-certify`.

The 67 historical documents take route 1. This spec and its plan take route 2, so neither needs an
allowlist row and the plan's path — unknown until `superpowers:writing-plans` runs — stops being a
bootstrap problem.

Any other tracked file containing the token reds the gate.

**Subset, not strict equality — with an arity floor, which rev 1 lacked.** The gate asserts
`offenders ⊆ allowlist`, so removing the advice from a grandfathered file is not a failure. That
departs from the repo's strict-equality pins (`EXPECTED_PR_SUBJECTS`, `CONTRACTS_GENERATE_INPUTS`)
because those pin a set that should stay put, while this one pins a set that should only ever
shrink; making a cleanup red the gate that authorised it would push people toward loosening it.

But subset alone is a **vacuous pass**: if the corpus command yields nothing — a `docs/` reorg, a
shell change, a `git ls-files` that errors — then `∅ ⊆ allowlist` and the gate prints PASS forever
having asserted nothing. That is not hypothetical here; `ci_targets.py:721-730` records it as
**MEASURED** for check 8e, whose table emptied to `()` emitted zero verdicts against a fully wired
file. The fix is the same one 8e uses:

```bash
[ "${#offenders[@]}" -ge 60 ] || infra "check 12: discovered ${#offenders[@]} files carrying the
  token, expected at least 60 — the corpus command has probably stopped matching"
```

`infra` (rc 2), not `fail` (rc 1): a corpus that vanished is a broken gate, not a clean repo. A
non-zero status from the discovery pipeline routes to `infra` for the same reason. The floor is
60 rather than 67 so that genuine cleanup of a handful of files does not require re-baselining.

**Why the bare token rather than a pattern.** §1.6 measured the token at zero occurrences outside
`docs/`, so it is fully selective today. The alternative — pattern-matching the advice's shape
(`jq` + `select(.status=="failed")` + `ciReport` in proximity) — was rejected on this repo's own
evidence: SMA-554 is an open issue recording that its pattern-matched check was bypassed four
separate times during review while the exact-literal check next to it was bypassed once. A
literal set has no tail to enumerate.

**The ongoing false-positive cost, which rev 1 did not state.** A future plan that references the
*corrected* procedure — "diagnose per CLAUDE.md's `moon-diagnosis` block, then read
`ciReport.json`'s `operations[]`" — is a bare-token offender, indistinguishable from one copying
the broken advice. Left as is, the gate's false-positive rate for correct mentions is 100%, and
the habit it trains ("gate red, add an allowlist row, move on") admits the broken advice just as
readily as the correct one. That is a real cost and it grows.

So the gate provides an **inline opt-in**: a file containing the marker `<!-- moon-diagnosis:ok -->`
is not an offender. A correct reference then costs one marker in the author's own document rather
than an edit to `run.sh`, and `CIREPORT_MENTIONS_ALLOWED` stays a small hand-curated set that
nobody touches routinely — which is what keeps it meaningful. Writing the marker is an explicit
claim ("I checked this against the corrected procedure"), which is the behaviour worth
encouraging; the allowlist is reserved for files that cannot carry a marker or predate it.

### 3.3 Assertion B — marker integrity

The `moon-diagnosis` block exists in CLAUDE.md; each marker appears exactly once; they appear in
order; the block between them is non-empty.

**Non-empty is not enough, and rev 1 claimed a parity it did not have.** The `ci-targets`
mechanism does not stop at markers: `parse_doc_targets` (`ci_targets.py:1137-1160`) enforces
exactly-one/order/non-empty and then `compare_doc_targets` (`:1293-1304`) asserts the region's
*content* is a verbatim ordered mirror of `T`. Rev 1 kept only the first half, under which a
single space or `TODO` passes — so the likeliest real failure, someone deleting the body during an
unrelated CLAUDE.md trim and leaving the markers, would not have been caught by the assertion
whose stated job is to catch it.

Assertion B therefore also requires a small content table, matched by containment inside the
block, with its own arity floor — the shape `T_AFFECTED_SMOKE_REQUIRED_SCRIPT` uses:

```
operations[]          — the correct path, the whole point of §1.1/§1.2
task-execution        — the operation type carrying command and exitCode
.moon/cache/states/   — where the output actually is
stderr.log            — the file the motivating failure's diagnosis was in
lastRunTime           — the cross-check of §1.8, the step that stops a wrong pairing
```

Five literals, floored at `-ge 5`. This does not gate the procedure's *correctness* (§5, L2), but
it does gate the presence of every load-bearing element, which is strictly more than rev 1 had.

Assertion B is what stops the §2 correction being silently deleted, which would switch the whole
fix off while leaving the gate green.

### 3.4 Self-test

One new table, `doc_diagnosis_self_test()`, driving a pure verdict function that takes
(offending paths, allowlist) and returns rows — no filesystem, consistent with the other twelve
tables. `SELF_TEST_COUNT` 13 → 14. Check 7 asserts invocations *and* definitions, so both halves
must move together; check 9's mutation battery already proves that counter fires and the new
table inherits that coverage without modification.

Cases: a new offender reds; a grandfathered offender passes; a *removed* offender passes (the
subset rule of §3.2); a file carrying `<!-- moon-diagnosis:ok -->` passes; an allowlist entry with
a blank reason reds; a missing marker reds; a duplicated marker reds; markers out of order red; an
empty block reds; a block missing one required literal reds; an emptied corpus routes to `infra`,
not `fail`.

### 3.5 The production call site must be pinned — rev 1 missed this

Rev 1 asserted "no `ci_targets.py` change … that is the whole point of §3.1". **That is wrong**,
and it is the finding that would have shipped a gate capable of being switched off in one line.

Every check added to `ci/actionlint/run.sh` since SMA-542 pins its production call site in
`ACTIONLINT_SH_CALL_SITES`, and the table records the measured reason. For check 8b
(`ci_targets.py:699`): *"deleting the whole '# Check 8b …' block from run.sh left the full gate at
rc 0 and this gate PASSing, because `invocation_allowlist_self_test` still calls the FUNCTION;
only this line proves it is also applied to the real ci.yml."* Checks 8c, 8d, 8e, 10 and 11 each
carry the same pin for the same reason, and 8e additionally pins **both** its tables' arity floors.

Check 12 has exactly that shape — a pure verdict function driven by a fixture table plus one
production call. Without the pin, deleting the single line that applies `doc_diagnosis_verdict` to
the real tree leaves check 7's counter, check 9's battery and `SELF_TEST_COUNT` all green while the
corpus stops being guarded. So `ACTIONLINT_SH_CALL_SITES` gains three whole-line, column-0 entries:

1. the production call, `done < <(doc_diagnosis_verdict)`;
2. the corpus arity floor from §3.2;
3. Assertion B's required-content arity floor from §3.3.

Column 0 matters: the table is matched with `rstrip` and no `lstrip`, deliberately, so that a copy
indented inside `if false; then … fi` does not satisfy it.

Reachability is already satisfied — `moon.yml:196` lists `ci/actionlint/**/*` among
`repo:affected-smoke`'s inputs, floored by `T_AFFECTED_SMOKE_REQUIRED_INPUTS` — so this is a
three-tuple edit, not a new registration obligation. But it is an edit to `ci_targets.py`, and §4
now says so.

### 3.6 A stale allowlist entry is reported

Subset in both directions means an entry naming a deleted or renamed file would never be noticed.
Every comparable hatch in this file reports one — check 8e emits `stale-skip` for a
`REQUIRED_INPUT_SKIP` naming a glob no longer required, and `COE_SKIP` is keyed by line number
*and* line text so a shifted entry stops matching rather than silently absorbing another. Check 12
emits `stale-allowlist <path>` as a non-fatal row for any entry whose file is absent or no longer
carries the token, so the table is prompted to shrink rather than rotting.

---

## 4. Files touched

Rev 1's table was wrong in three places, all of them "this file is untouched" claims. The
13 → 14 bump in particular has stale-count sites scattered across four files — and
`ci/actionlint/README.md:412` records this exact drift happening before ("ALL TWELVE" against an
actual thirteen for the whole of SMA-602's final review), with `moon.yml:670-672` recording it a
second time for SMA-601/SMA-603. Repeating it while asserting the files are untouched would make a
reviewer trust the claim.

| File | Change |
| -- | -- |
| `CLAUDE.md` | new marker-delimited gotcha (§2); correction to the `buffer-only-failure` claim; **and** the `SELF_TEST_COUNT` prose at `:308-311` ("currently 13 … SMA-603 the thirteenth") |
| `ci/actionlint/run.sh` | check 12 + `doc_diagnosis_self_test()`; `SELF_TEST_COUNT=13` → 14 at `:48`; the count prose at `:66` (`usage()`) and `:4597` |
| `ci/actionlint/README.md` | check 12 documented (new row); rows 7 and 9 re-counted at `:33`/`:40`; `:669`, `:677-678`, `:701`, `:704` re-counted; the "## Why" section amended so it explains a docs check too; L-entries for §5's residuals; check 9's timing table at `:661-667` re-measured |
| `ci/affected-graph/ci_targets.py` | three `ACTIONLINT_SH_CALL_SITES` entries (§3.5) |
| `moon.yml` | the `repo:actionlint` comment block at `:667`, `:671`, `:686` ("THIRTEEN fixture tables", "thirteen mutants") |
| `.github/workflows/ci.yml` | `if: failure()` upload of `ciReport.json` + `states/**` (§1.10) |
| `docs/superpowers/specs/2026-09-03-sma-597-moon-failure-diagnosis-design.md` | this document |

The probe tasks used throughout §1 are reverted and `moon.yml`'s task definitions are unchanged —
but the file is **not** byte-identical to `main`, because of the comment block above.

---

## 5. Non-goals and limitations

1. **The 67 existing documents keep their advice; they gain an annotation** (§7, accepted at
   review). Each receives an appended `<!-- moon-diagnosis:superseded -->` marker and one pointer
   sentence. The broken text itself is left exactly as written, so the documents remain accurate
   records of what was believed at the time — which was the issue's actual objection to editing
   them. Nothing else about them changes.
2. **L1 — the gate keys on files, not content.** A grandfathered file that gains a *new*
   paragraph of broken advice passes. Closing this needs content analysis of prose, which is the
   pattern-matching approach §3.2 rejected on SMA-554's evidence. Accepted, and recorded in the
   README's Limitations section.
3. **L2 — nothing gates the procedure's *correctness*, only the presence of its parts.**
   Assertion B now proves five load-bearing literals are present (§3.3), not merely that the
   block is non-empty. It still cannot tell a correct `jq` from a subtly wrong one. Closing this
   needs a gate that *executes* the procedure against a deliberately failed task and asserts it
   recovers the marker — genuinely feasible, since §1 does exactly that by hand, but it is a
   second gate with its own registration, and it is the right follow-up issue rather than scope
   here. Stated plainly because this repo's history is full of gates that passed for the wrong
   reason, and this is the one place check 12 could join them.
3a. **L4 — the gate is blind to its own two files** (§1.9). `ci/actionlint/run.sh` and its README
   are allowlisted, so broken advice written *into the gate's own source* is invisible to it.
   Accepted as the least-bad of three options; the alternatives were a bypass or a clever string.
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
look worthless and is the reason the working path went unfound for so long. Nothing in this repo
asserts that a closed issue's premise was corrected; this is a manual step, recorded here so it
is not forgotten.

---

## 7. Decision taken at review: both accepted

**Resolved 2026-09-03 — approved.** Both the `ci.yml` artifact upload (§1.10) and the supersession
marker below are in scope. The consequences for the design are folded into §3.2 and §5.1, and are
summarised here because they simplify the gate substantially:

- The 67 historical documents each gain `<!-- moon-diagnosis:superseded -->` plus one pointer
  sentence, appended by a single scripted edit. They are **not** otherwise modified — the broken
  advice stays on the page, annotated rather than rewritten, which is what preserves them as dated
  records.
- **Assertion A becomes the stronger rule:** a tracked file carrying the token must carry
  `<!-- moon-diagnosis:superseded -->` (a historical record) or `<!-- moon-diagnosis:ok -->` (a
  deliberate correct reference), or appear in `CIREPORT_MENTIONS_ALLOWED`.
- **The allowlist collapses from 69 entries to three:** `ci/actionlint/run.sh`,
  `ci/actionlint/README.md` (§1.9, neither being markdown prose a marker belongs in), and
  `CLAUDE.md` (the authority itself, which should not have to self-certify). This spec and its
  plan carry `:ok` markers instead of allowlist rows.
- The `-ge 60` corpus floor of §3.2 is **unaffected**: a marker does not remove the token, so
  discovery still finds ~72 files and then partitions them. The floor still catches a corpus
  command that stopped matching.

The original framing of the decision follows, for the record.

### 7.1 The decision as put

**Should the 67 grandfathered documents receive an appended supersession marker?**

§5.1 leaves them untouched, on the issue's reasoning that dated records should not be edited. The
challenge proposed a third option that neither the issue nor rev 1 considered, and it is
materially different from "rewrite the advice": append to each file a marker plus one sentence —
`<!-- moon-diagnosis:superseded -->` and a pointer to CLAUDE.md — as a single scripted edit.

The case for it is strong. It **annotates** a dated record rather than falsifying it, which is
what the issue's objection was actually about. It attacks the propagation mechanism at the source
identified in §1.5, where the current design only punishes the imitator while leaving the thing
being imitated intact. And it would let Assertion A become the stronger and much quieter rule *"a
file carrying the token must carry either the supersession marker or the `:ok` marker, or be
allowlisted"* — collapsing the 67-entry seed to near nothing and largely dissolving the
false-positive cost §3.2 has to argue its way around.

The case against: it touches 67 files in one commit, and a marker appended to a historical
document is still an edit to a historical document.

**Recommendation: do it.** The design is better in three independent ways and the objection it
answers is weaker than it first appears. But it reverses an explicit earlier decision, so it is
carried to review rather than taken here. If accepted, §3.2's seed and §5.1 change; nothing else
in this spec does.

**→ Accepted at review. See §7's header for what changed.**

---

## 8. Changelog — rev 1 → rev 2

Rev 1 was reviewed by an adversarial challenge which returned **NEEDS REWORK** with five blockers.
All five were verified against the repo and all five were justified. Folded in:

| # | Finding | Resolution |
| -- | -- | -- |
| B1 | §2 steps 1 and 2 can read different runs; rev 1's `repo:osv` "corroboration" was a counterexample | §1.8 added, measured (`moon run` writes `runReport.json` not `ciReport.json`; a cache hit rewrites neither). Procedure gains mandatory step 2a |
| B2 | Procedure unusable in CI, and restored `.moon/cache` makes logs misleading | §1.10 added; block scoped to local runs; `ci.yml` gains an `if: failure()` artifact upload. Rev 1's "no `ci.yml` change" withdrawn |
| B3 | `∅ ⊆ allowlist` is a vacuous pass, and the subset rule removed the control | §3.2 gains a `-ge 60` corpus floor routed to `infra`, on check 8e's measured precedent |
| B4 | Production call site unpinned; "no `ci_targets.py` change" was wrong | §3.5 added — three `ACTIONLINT_SH_CALL_SITES` entries |
| B5 | The gate's own source and README must contain the token | §1.9 added; both allowlisted; blindness recorded as L4 |
| M6 | Assertion B weaker than the `ci-targets` discipline it claimed to mirror | §3.3 gains a five-literal content table with an arity floor |
| M7 | 13 → 14 touches files rev 1 called untouched | §4 rewritten; eight stale-count sites enumerated |
| M8 | "negative and final" over-claimed from one flag value | §1.4 re-measured across 4 `--summary` levels × 3 `outputStyle` values plus a full-depth key walk; claim restated with its bounds |
| M9 | AC3-unachievable rested on a single-token grep | §1.5 re-run against the plugin's whole skill set; no skill mentions moon at all |
| M10 | 100% false-positive rate for correct future mentions | §3.2 gains the `<!-- moon-diagnosis:ok -->` inline opt-in |
| M11 | Propagation source left intact; alternative not named | §7 — carried to review as an explicit decision, with a recommendation |
| M12 | `docs/**/*.md` misses top-level docs (the documented git pathspec trap) | §1.6 corrected; §3.2 specifies unfiltered `git ls-files` |
| minors | stale-allowlist rows, non-empty reasons, index-vs-worktree, check 9 cost, failure-to-start case, `REQUIRED_REPO_TASKS`, README "## Why" | §3.6, §3.2, §3.2, §3.1, §2 step 2, §3.1, §4 |

Rev 1's §1.1/§1.2 measurements and §3.1's placement argument were assessed as sound and are
unchanged.
