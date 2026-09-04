# actionlint gate

Lints `.github/workflows/**`, proves every `paths:` filter glob still matches the tree, and
proves every `branches:` filter entry names a branch that exists.

## Why

A `paths:` filter that comes to match nothing does not error. The workflow stops running,
forever, with no red check and no notification — `prebuild.yml` triggers only on
push-to-`main`, `workflow_dispatch` and a narrow `pull_request` filter, so its 7-platform
verification would silently cease. See SMA-525 and
`docs/superpowers/specs/2026-08-16-sma-525-actionlint-gate-design.md`.

actionlint alone is **not** sufficient: it validates syntax and has no view of the file tree,
so a valid-but-never-matching glob (`rz/**`) passes it cleanly. Checks 5–7 close that.

`branches:` has the identical property and was SMA-525's stated limitation L5. `branches: [mian]`
is a valid glob, actionlint accepts it, and the workflow stops running — silently and permanently,
one key over. All three workflows here trigger off a `branches:` filter naming `main`, including
the required check. See SMA-540 and
`docs/superpowers/specs/2026-08-19-sma-540-branches-filter-gate-design.md`.

Checks 8b–8f, 10–11 and 12 are not about workflow filters. They live here because this gate
declares `inputs: ['**/*']` and therefore runs on every PR, which is the reachability a
cross-cutting pin needs — a narrower `inputs` list would be the SMA-553 failure class. Check 12
(SMA-597) is the clearest case: a docs-corpus freeze has to see the PR that adds a new document.

## The checks

| # | Check |
|---|---|
| 1 | `actionlint` over the auto-discovered workflow set, with shellcheck wired in (`-shellcheck=$SHELLCHECK_BIN`) so every `run:` block's inline bash is inspected too (SMA-539) — see "shellcheck provenance" below |
| 2 | `.github/actionlint.{yaml,yml}` declares nothing but `self-hosted-runner`, and no `ignore` key in any style (either would neuter check 1 invisibly) |
| 3 | Five stdin fixtures, one per defect class, each must fail **with its expected rule tag** |
| 4 | A healthy stdin fixture must pass — the control for check 3 |
| 5 | Every `paths:` glob is in the supported vocabulary and matches the tracked tree, and every `branches:` entry resolves as a ref or is skip-listed |
| 6 | Every extracted filter key carries at least one sequence entry; a `paths:`/`branches:` key must also have at least one of them positive (the `-ignore` variants are exempt) |
| 7 | Fourteen self-tests against fixture tables — extractor, path-filter verdicts, branch-filter verdicts, config allowlist, ci-target floor, invocation allowlist, affected-graph wiring, block execution, kill predicate, affected-smoke block, release guard, cargo-lock step, release-plan, doc-diagnosis — plus a counter (`SELF_TESTS_RAN`/`SELF_TEST_COUNT`) asserting all fourteen ran, and a definition-count check catching a fifteenth table that is defined but never wired into `run_self_tests` (`run.sh --self-test`) |
| 8 | `ci.yml`'s `T=(…)` still schedules the gate that guards `T` itself, and nothing silences that gate's result. Six verdict families: **(a)** the floor — `:affected-smoke` present in `T` (`missing`), or the array can't even be read (`no-array`/`no-file`); **(b)** no `moon` command line is continued onto another physical line, where a discarded exit status would be invisible to this check (`continued`); **(c)** no single-line `moon` command line discards its own exit status (`swallowed`), with `SWALLOWED_SKIP` as the escape hatch for an unrelated `moon` line this check cannot know is harmless; **(d)** no line CLOSING a block (`fi`/`done`/`}`) discards its own exit status either, the same tail on a different line (`block-swallowed`), sharing `SWALLOWED_SKIP`; **(e)** no `moon ci`/`moon run` invocation sits behind a known command wrapper (`command`/`env`/`time`/`eval`/`exec`/`if`/`while`/`until`/`!`) on the same line, where propagation cannot be confirmed (`wrapped`), sharing `SWALLOWED_SKIP` as its escape hatch; **(f)** no step's `continue-on-error:` value suppresses it — any spelling but the literal `false` (`continue-on-error`), with `COE_SKIP` as the escape hatch for an unrelated later step |
| 8b | Every line in `ci.yml` carrying the target-array expansion `"${T[@]}"` matches one of `T_INVOCATION_ALLOWLIST` (declared with `T_FLOOR`) **exactly** — indentation included — and the number of such lines matches the array's length. This is the PRIMARY guard on the INVOCATION LINES themselves (SMA-542 CodeRabbit round 3, finding B — a bare `VAR=value` assignment prefix defeated BOTH check 8's `swallowed` and `wrapped`, since it has neither `moon` at column 0 nor a recognized wrapper token there); check 8's `continued`/`swallowed`/`block-swallowed`/`wrapped` stay for their more specific diagnostics and are consulted first, so a line they already explain is not also reported here as `not-allowlisted`. It matches each LINE against a SET of allowed forms, with no notion of which branch a line sits under, so it is NOT a complete guard on the step's control flow — check 8d, below, closes the concretely-identified gaps; see L12 for what (if anything) still isn't |
| 8c | `ci/affected-graph/run.sh` still contains its own two call sites into `ci_targets.py` — `assert_ci_targets \|\| SUITE_RC=1` and `"$HERE/ci_targets.py" --self-test \|\| NEG_RC=1` — WITH each `\|\| RC=1` propagation suffix intact (`missing <site>`), and that the file itself exists and is readable (`no-file`). Closes L6 (SMA-542 residual closure, PR 150 follow-up): check 8 above pins only `:affected-smoke`'s *scheduling*; this pins the two lines that actually INVOKE the gate which, in turn, pins THIS file's own call sites back (`ci_targets.py`'s `ACTIONLINT_SH_CALL_SITES`). Scheduled independently of `ci/affected-graph/`, so it survives a deletion there that would otherwise green both directions of the cycle silently |
| 8d | The `"moon ci (affected graph)"` step's `run:` block — extracted from `ci.yml`, dedented, then EXECUTED once per GitHub event path (`pull_request`; `push` with a real `BEFORE` sha; `push` with the all-zero `BEFORE`; `push` with an empty `BEFORE`) against a `moon` stubbed in a `mktemp -d` bin directory placed first on a minimal PATH. Each path must invoke `moon` **exactly once**, with the exact subcommand + the WHOLE `T` array + the `--base`/`--include-relations` shape that path requires (`no-step`/`multi-step <n>` when the step can't be found unambiguously, `no-run-block` when its `run:` block can't be extracted, `no-target-array` when `T` can't either, `zero-invocations <path>`, `wrong-count <path> <n>`, `bad-args <path>`). Closes README L12 (SMA-542 residual closure, PR 150 follow-up): 8b matches invocation LINES; this proves the CONTROL FLOW around them actually reaches one, on every path — an outer `if false; then … fi` (byte-identical lines, zero executions) now reds, and so does a `"${T[@]}"` line moved to the wrong branch (individually allowlisted, wrong condition) |
| 8e | `moon.yml`'s `repo:affected-smoke` task still declares every input that schedules a pin in `ci/affected-graph/ci_targets.py`, and still runs its `set -euo pipefail` / `--negative-control` / real-run script lines in the right order (SMA-572/SMA-573). Two tables: `T_AFFECTED_SMOKE_REQUIRED_INPUTS` (20 globs/files) is matched by **containment** — the block's `inputs:` sequence must be a superset, since the list legitimately grows every time a gate keys on a new directory — while `T_AFFECTED_SMOKE_REQUIRED_SCRIPT` (3 lines) is matched **whole-line, in order**: unlike the inputs table, a set-membership check would accept `set -euo pipefail` moved below the invocations, and Moon takes a `script:` block's status from its LAST command, so that reordering silently stops a failing `--negative-control` from propagating. Verdicts: `no-file`/`no-task`/`bad-task-form`/`bad-script-form`/`bad-inputs-form`/`duplicate-key <name>` (the block could not be parsed — `no-task` means the extractor saw no key at exactly two spaces of indentation whose name is `affected-smoke`; it identifies the task by INDENTATION AND NAME ONLY and never checks that the key is nested under a `tasks:` mapping, see L18), `missing-input <glob>`, `missing-script <line>` (a commented-out copy counts as absent), `out-of-order-script <line>`, `skip-without-reason <glob>`, `stale-skip <glob>`. Each table carries an `-ge` arity floor (20 / 3, pinned back from `ci_targets.py`'s `ACTIONLINT_SH_CALL_SITES`) so an EMPTIED table cannot pass by asserting nothing — `check_self_invocation` alone cannot buy this, since 8e's tables are not a dual copy of anything else the way check 8c's is. `REQUIRED_INPUT_SKIP` is the escape hatch for a legitimately-removed input, mirroring `COE_SKIP`/`SWALLOWED_SKIP`/`BRANCH_SKIP`: an entry with no stated reason is reported (`skip-without-reason`), and one naming a glob no longer required is reported too (`stale-skip`), so a waiver cannot outlive its glob. Unconditional, like check 8c — it reads `moon.yml`, not `ci.yml`, so gating it on `ci.yml`'s existence would switch it off for an unrelated reason — and COLUMN 0 for both floor lines, the same discipline as checks 8/8b/8c/8d's own call-site pins |
| 8f | The `cargo-lock-integrity` step in `ci.yml` is still wired, and the script it invokes still asserts something (SMA-601). Two tables. `T_CARGO_LOCK_STEP_REQUIRED` (6 lines) pins the step: entry 0 is its `- name:` line, matched against the whole stripped file and used to LOCATE the step; the other five — `run: \|`, `set -euo pipefail`, and the `--self-test` / `--negative-control` / bare invocations — are matched **whole-line and in order, inside the step's own window only**, because `run: \|` and `set -euo pipefail` occur in other `ci.yml` steps and a whole-file match on them would be vacuous. The step must also PRECEDE the `moon ci` step (`out-of-order`), carry no `continue-on-error:` other than the literal `false` (`continue-on-error <value>`), and carry **no `if:` at all** (`conditional <expr>`). Both protected keys are matched after normalising the quoted (`"if":`) and spaced (`if :`) spellings, and YAML's explicit-key form (`? if` / `: always()`) is REJECTED outright (`explicit-key <key>`) rather than parsed — measured, that form yields a real `if` key and actionlint accepts it at rc 0, so it would clear check 1 and evade every same-line scan — a skipped step is a green step, so any `if:` switches the guarantee off for every event it excludes, `pull_request` included, which is exactly where a Dependabot PR ships a truncated lock. `T_CARGO_LOCK_SH_CALL_SITES` (6 lines) pins `ci/cargo-lock-integrity/run.sh` itself (`missing-site <text>`, `no-file`): the two flag-parse arms, the `cargo metadata --locked` line, the negative control's call into the real assertion, that control's rc=1 report arm, and the real run's own call. MEASURED: deleting `--locked` from that one line makes the command exit 0 **and repair the lock**, so the gate prints "satisfies every manifest" and becomes the first repairer — the SMA-530 "control that actively lies" shape. This file is the right home for both because `repo:actionlint` carries `inputs: ['**/*']`, so it is scheduled on every PR without a new input registration, and unlike a pin inside `ci/affected-graph/` it is not the sole judge of its own reachability |
| 9 | A mutation battery, full-gate only: each of the fourteen self-test invocations inside `run_self_tests`, deleted one at a time, run concurrently against the real unmutated control — every mutant must die at the counter's own message (a kill predicate driven by its own fixture table, not merely "non-zero"), or the battery itself reds |
| 10 | (SMA-579) The release guard, whose VERDICT lives in `ci/actionlint/release_guard.py` because it needs YAML structure (a job-level `if:` told apart from eight identical step-level ones, `needs:` chains walked) rather than line-oriented text scanning. Two parts: `release_guard_self_test`, in the battery above, asserts `release_guard.py --fixture-count` reports at least 105 fixtures and that `--self-test` itself reports a healthy verdict; the full-gate-only half runs `release_guard.py` over the real `.github/workflows/release.yml` and fails on anything it reports, capturing its output to a file first since a process substitution would silently discard its exit status. Fail-closed on EVERY status, not only the guard's own 2: an unreadable file or unparseable YAML gives 2, a missing `uv` gives **127 from the wrapper**, and a kill gives 137 — all three abort the gate. An earlier revision of this row claimed a missing `uv` was covered by the exit-2 routing; it was not, and a status the routing did not recognise left the gate passing having asserted nothing (measured at rc 127, SMA-579 fix round 3). rc 1 with no output aborts too, since that contradicts the guard's own contract |
| 11 | (SMA-603) The release-plan decision, whose VERDICT lives in `ci/release-plan/release_plan.py` — TAG EXISTENCE against the derived releasable set, not a `release-plz release --dry-run` read (see that project's own README for why the dry-run reading is silently, permanently wrong). Two parts: `release_plan_self_test`, in the battery above, reads `release_plan.py --fixture-count` directly rather than through `ci/release-plan/run.sh` (that wrapper's flag parser rejects `--fixture-count` outright), asserts it reports at least 8 fixtures (a floor against 9 actual — one row of headroom so a legitimate row removal does not abort the gate as infra), and asserts `ci/release-plan/run.sh --self-test` and `--negative-control` both report a healthy verdict; the full-gate-only half runs `ci/release-plan/run.sh --assert` over the real repository and fails on anything it reports. Fail-closed on every status the wrapper can produce, the same shape as check 10: exit 2 aborts the gate (uv or the interpreter failed, not an assertion), exit 1 fails it (the derived releasable set, a crate version, or the tag-name format changed), and anything else non-zero also aborts — this file is `set -uo pipefail` with **no** `-e`, so an unrouted status would finish the gate rc 0 having asserted nothing |
| 12 | Every tracked file carrying the token `ciReport` must carry `<!-- moon-diagnosis:superseded -->` (a dated record), `<!-- moon-diagnosis:ok -->` (a deliberate reference to the corrected procedure), or a `CIREPORT_MENTIONS_ALLOWED` row with a non-empty reason — plus CLAUDE.md's `moon-diagnosis` block must exist, have exactly one ordered marker pair, be non-empty, and contain all five entries of `DOC_DIAGNOSIS_REQUIRED_LITERALS` (SMA-597). Three `-ge` arity floors keep an emptied table from passing having asserted nothing: the corpus command must find at least 60 tracked files carrying the token, `DOC_DIAGNOSIS_REQUIRED_LITERALS` must have at least 5 entries, and `CIREPORT_MENTIONS_ALLOWED` must have at least 3 — the third floor closes a bash-3.2-specific hole (macOS's system bash, this repo's stated compat target): an emptied `CIREPORT_MENTIONS_ALLOWED` makes `for entry in "${CIREPORT_MENTIONS_ALLOWED[@]}"` an unbound-variable error under `set -u` on bash 3.2 (measured; bash 4.4+ reds instead), which kills the process substitution rather than the gate and lets the assertion pass having asserted nothing |

Only a `paths:`/`paths-ignore:`/`branches:`/`branches-ignore:` key **two levels deep** inside
`on:` — `on.<event>.paths` — is a filter. A workflow input may legitimately be *named* `paths` or
`branches`, and it sits one level deeper, under `on.workflow_dispatch.inputs`; checks 5 and 6
ignore it. This depth rule holds in flow style too,
not just block style: a top-level flow `on: { workflow_dispatch: { inputs: { paths: {...} } } }`
or an event's own flow value `push: { inputs: { paths: x } }` both correctly ignore the nested
`inputs.paths`, quoted or not — the extractor tracks brace depth rather than matching a `paths`
token at any nesting level. Conversely a flow-mapping event value, `push: { paths: [...] }`, is
not parsed for entries, so it is reported by check 6 as a key with no entries rather than skipped
in silence — same for the equivalent depth in a fully flow-style `on: { push: { paths: [...] } }`.

## Supported glob vocabulary

`git ls-files ':(glob)P'` is not a sound model of GitHub filter patterns, so check 5 accepts
only the subset where both provably agree:

- **literals** — must be an *exact* tracked file path. A bare directory name (`rs`) matches
  nothing on GitHub, though git's pathspec would match everything beneath it.
- **`dir/**`**, **`**/name`** — `**` as a whole path component.
- **`*`** within a single segment.

Rejected loudly, never guessed at: `?`, `+`, `[]`, and `**` embedded in a segment (`**.js`).

## Branch filter entries

`branches:` is read as a **block sequence** — the inline `branches: [main]` form is deliberately
not parsed and fails check 6 by design, exactly as `paths: [a, b]` does. Each entry must:

- **resolve** as `refs/remotes/origin/<name>`, or
- appear in `BRANCH_SKIP` in `run.sh` with a comment justifying it.

Local `refs/heads/*` is deliberately **not** consulted: a workflow triggers on branches as they
exist on GitHub, and a local-only branch does not. A glob metacharacter (`*`, `**`, `?`, `+`,
`[]`) makes an entry a pattern rather than a name, so it cannot be resolved and must be
skip-listed — `+` counts as a glob even though git allows it in a ref name, because GitHub reads
it as "one or more of the preceding character".

`branches-ignore:` is extracted and counted but never resolved: a typo'd exclusion makes a
workflow run *more* often, which is the fail-safe direction.

`tags:` and `tags-ignore:` are not covered — see the spec's §7 L4.

## Escape hatches

- A **new GitHub runner label** the pinned actionlint does not know: add it to
  `self-hosted-runner.labels` in `.github/actionlint.yaml`. Check 2 permits that file, and
  `self-hosted-runner` is the one top-level key it allows there.
- A **GitHub-valid pattern outside the vocabulary**: add it to `SKIP_PATTERNS` in `run.sh` with
  a comment justifying it and saying what verifies it instead.
- A **branch that does not exist yet**, or a branch pattern: add it to `BRANCH_SKIP` in `run.sh`
  with a comment justifying it and saying what verifies it instead.
- A **`moon` command line check 8 cannot know is harmless** (any `moon` line other than the one
  guarding `T`, carrying a `;`/`|`/`&&`/`||` tail for its own legitimate reason, **or** a `moon
  ci`/`moon run` line sitting behind a wrapper check 8 recognizes but this one is harmless): add
  it to `SWALLOWED_SKIP` in `run.sh` with a comment justifying it and saying what verifies its own
  failure instead — the one skip list covers both `swallowed` and `wrapped`, since they are the
  same underlying problem spelled two ways. There is deliberately **no** equivalent hatch for
  `continued` — a backslash-continued `moon` invocation is rejected outright, the same way
  `no-array` never skips; put it back on one physical line.
- A **deliberate, reviewed change to how ci.yml invokes `moon`** (a genuinely new invocation form,
  or a genuinely new number of invocations): update `T_INVOCATION_ALLOWLIST` in `run.sh`, copying
  the new line(s) VERBATIM from the file, indentation included. There is deliberately **no**
  separate skip list for check 8b — the array IS the reviewed exception mechanism, the same way
  `T_FLOOR` has none.
- **Anything worse**: drop `:actionlint` from `T=(…)` in `.github/workflows/ci.yml`. This must
  also be removed from the CLAUDE.md `ci-targets` block, since `repo:affected-smoke` asserts the
  two agree — **and** needs a `T_EXEMPT` entry in `ci/affected-graph/ci_targets.py` with a stated
  reason, or C1's strict equality reds on the now-missing entry (true since SMA-541 shipped).

## shellcheck provenance

SMA-525 shipped check 1 with `-shellcheck=` and `-pyflakes=` both empty, refusing an opportunistic
`PATH` lookup because it would make the gate's strictness a property of the host — a dev box with
shellcheck installed catches an `SC2086` a clean CI runner never sees. SMA-539 turns shellcheck
back on, sourced instead from a **hash-pinned PyPI package**: `shellcheck-py`, added to
`py/pyproject.toml`'s `[dependency-groups] dev`. `run.sh` resolves the binary via `uv run --locked
--project py`, after the `--self-test` early exit (see the SMA-539 comment beside the `command -v
actionlint` guard) — never on `PATH` — so a dev box and CI always run the exact same shellcheck.
Resolution is fail-closed: there is no fallback to a bare `-shellcheck=`, and an unresolvable
binary aborts the gate at rc 2 rather than silently linting nothing.

**Provenance, not just presence (whole-branch review, I1).** "Fail-closed" above was
incomplete on its own: `shutil.which` is `PATH`-based, so if the `py` uv project does
not actually contain `shellcheck` — a `[dependency-groups]` rename, a `[tool.uv]
default-groups` change, or `UV_NO_DEV=1` at invocation time, none of which trips `uv
run`'s own exit status — `which` would silently fall through to whatever `shellcheck`
is first on the OUTER host `PATH`, and `[ -x ]` would pass on that impostor. MEASURED:
with `shellcheck` removed from `py/.venv/bin` and a host impostor earlier on `PATH`, the
bare `shutil.which` resolver printed the impostor's path at exit 0 — precisely the
strictness-is-a-property-of-the-host failure this whole section exists to refuse, now
green. The resolver now also requires the resolved path to start with `sys.prefix`,
which under `uv run --project py` IS `py/.venv` (measured) — a same-process check no
outer-`PATH` manipulation can spoof. The same setup now exits 1 from the `-c` program
(routed to `infra`, rc 2) instead of printing the impostor's path.

**shellcheck findings at `info` severity still red this gate.** actionlint hands
shellcheck's full output straight through; nothing here filters by severity. A rule
that shellcheck reports at `info` (e.g. `SC2250`) fails check 1 exactly the same as a
`warning`- or `error`-level one — do not assume only warnings-and-above are covered.

**Waiving a shellcheck finding.** `.github/actionlint.yaml`'s `ignore:` key is banned
outright by check 2 (a repo-wide `ignore` would neuter check 1 invisibly for every
workflow, not just the one with the false positive). The one hatch that works is an
inline `# shellcheck disable=SCxxxx` comment inside the offending `run:` block itself —
verified live: actionlint passes such a comment straight through to shellcheck, and the
disabled rule is then silent for that block only, leaving every other workflow and rule
covered. Prefer the narrowest form (`# shellcheck disable=SC2086` on the one line, not a
file-wide directive) and state the reason next to it, the same discipline as this
repo's other escape hatches.

`shellcheck-py` was chosen over a `proto` plugin because upstream shellcheck's own GitHub release
ships 13 platform archives and **no checksums asset** (re-measured 2026-09-02), which SMA-525's
own D2 decision already refused as a supply-chain shape. `shellcheck-py` republishes shellcheck as
checksummed wheels: `uv.lock` pins a `sha256` per wheel and sdist, and three of the republisher's
digests were verified by hand against koalaman/shellcheck's own release assets at the pinned
version. **A version bump re-opens that verification — it is not a one-time check.** `-pyflakes=`
stays disabled: actionlint only ever applies pyflakes to a step declaring `shell: python`, and
`wheels.yml`'s Python-shaped blocks are actually bash heredocs, so nothing in this repository would
be covered by turning it on.

## Limitations

**L1 — Deleting both `T` entries in one edit.** Removing `:affected-smoke` *and* `:actionlint`
from `T=(…)` together silences both halves of the cycle: neither gate runs, so neither complains.
Inherent — two independently-scheduled gates are the most the graph offers, and a third would only
move the pair to a triple. Bounded: `moon ci`'s target list is a single, short, reviewed line.

**L2 — Coordinated multi-line edits inside `run_self_tests`.** The counter, the definition count
and the mutation battery each red on a single-line change. Editing the body *and* `SELF_TEST_COUNT`
*and* the definitions consistently would pass.

**L3 — The whole-line pin is brittle against reformatting.** A future `run_self_tests || FAILED=1`
reds `ci_targets.py`'s C4 even though it is harmless — propagation is already via the global
`FAILED`. Restore the bare line, or update `ACTIONLINT_SH_CALL_SITES`.

**L4 — The battery proves invocation, not correctness.** A self-test whose fixtures were weakened
still runs, still increments, and still passes. That is check 7's own tables' job.

**L5 — `.git` state remains outside Moon's input hash.** See the `actionlint:` task in `moon.yml`.
The `T` floor reads a tracked file, so it is unaffected; check 5's branch half still is.

**L6 — The cycle's second half is now closed (SMA-542 residual closure, PR 150 follow-up).**
Previously `repo:actionlint` pinned only `:affected-smoke`'s *presence in `T`* — its scheduling —
never `ci/affected-graph/run.sh`'s own two call sites into `ci_targets.py`. Check 8c above closes
that: it reads `ci/affected-graph/run.sh` directly and asserts `assert_ci_targets || SUITE_RC=1`
and `"$HERE/ci_targets.py" --self-test || NEG_RC=1` are both still present, propagation suffix
intact — mirroring `ci_targets.py`'s own `RUN_SH_CALL_SITES`, which is the ONE place that pin used
to live (and which pins `ACTIONLINT_SH_CALL_SITES`, this file's six call sites, in return). Both
directions are now guarded from a location independent of the file being guarded.

What remains, and is inherent rather than an oversight: deleting check 8c's OWN production call
site (`done < <(affected_graph_wiring_verdict ...)`, below) AND `assert_ci_targets || SUITE_RC=1`
in the SAME edit still silences both directions at once — the former is pinned by
`ci_targets.py`'s `ACTIONLINT_SH_CALL_SITES`, which runs FROM the very call the latter deletes.
This is the same shape L1 already names for `T`'s own two entries: two independently-scheduled
gates are the most the graph offers, and closing a combined deletion needs a third, which only
moves the same problem one level out. Bounded for the same reason L1 is: the two lines this
residual depends on sit next to each other inside a five-line function, not scattered across the
tree. Check 8e (SMA-572/SMA-573) is the same bounded two-gate shape one more time: deleting its
whole block from `run.sh` *and* both of its `ACTIONLINT_SH_CALL_SITES` entries (the production
call site and the `_INPUTS` arity floor — or, for the `_SCRIPT` floor, that entry instead) from
`ci_targets.py`, in one edit, silences both directions of that pair at once too.

**L7 — `COE_SKIP` and `SWALLOWED_SKIP` are exact-text, not semantic.** Each entry is keyed by both
line number and the matched line's exact text (leading blanks included), so a later reformat of
that one line — even whitespace-only — makes the entry stop matching. That is the fail-safe
direction: the check fires again and asks for the entry to be updated, rather than silently
continuing to skip a line whose content has since changed underneath it. Both ship empty today.

**L8 — `continued` cannot see past its own continuation.** A backslash-continued `moon` line is
always rejected, whether or not a tail on a later physical line would actually have been a
problem — this check reads one physical line at a time and does not reassemble a continuation
before scanning it. Deliberate (SMA-542 fix-wave finding I2): guessing at what follows the backslash risks
the exact misdiagnosis ("swallowed" when the tail wasn't there, or vice versa) this check exists to
avoid, so it demands the invocation be rejoined onto one line instead.

**L9 — `wrapped` recognizes a closed, enumerated vocabulary, not "any wrapper," and `moon` must
be the very next command word.** Only `command`/`env`/`time`/`eval`/`exec`/`if`/`while`/`until`/`!`
are checked, only when the wrapper and the `moon ci`/`moon run` invocation share one physical line,
and only with whitespace-separated "glue" tolerated in between: further wrapper tokens
(`command env moon ci …`), a negation (`if ! moon ci …`), or a `VAR=value` assignment
(`env FOO=bar moon ci …`). A wrapper outside that list (a shell function, `sudo`, `nice`, a `case`
arm, ...), or `moon` reached through anything other than whitespace (`true && moon ci …`), is
invisible to it. A wrapper on one physical line with the invocation on a LATER one shares L8's
physical-line-only parsing — but unlike L8, where a plain backslash-continued `moon` line IS
reported (as `continued`), a wrapper split across lines this way is not reported by check 8 at
all: `command \` alone on its line has no `moon` for this pattern to match, and `moon ci …` alone
on the next line has no wrapper for `wrapped` to see (it may not even reach `swallowed` either, if
that second line carries no tail of its own). T_INVOCATION_ALLOWLIST does not close this either —
see L11. This IS what closes the
CodeRabbit round-2 false positive (`if test -n "$X"; then echo "moon ci failed"; fi` — a wrapper at
line start with "moon ci" appearing only inside a string later on the line): with `test` as the
next command word rather than `moon` or recognized glue, the pattern does not match that line at
all. Deliberate (CodeRabbit, PR 150): parsing arbitrary bash wrapping accurately needs the same
kind of control-flow analysis this file's own history shows goes wrong (flow vs block style,
SMA-525/540 rounds 2–3) — reject the enumerated shapes loudly rather than guess at the rest.

**L10 — `ci_targets.py`'s column-0 pin on `ACTIONLINT_SH_CALL_SITES` is not reachability
analysis.** It closes the common case — an indented copy of a required line, the shape wrapping
one of the three calls in a conditional block conventionally produces — but a required line copied
into an **unindented** `if false; then … fi` block, or an **unindented** heredoc, still sits at
column 0 and still satisfies the pin even though neither ever executes. Deliberate (CodeRabbit, PR
150): genuine bash-reachability analysis in Python is fragile and out of scope; see the comment at
`ACTIONLINT_SH_CALL_SITES` in `ci/affected-graph/ci_targets.py`.

**L11 — `T_INVOCATION_ALLOWLIST` matches per LINE, so a wrapper split across physical lines (L9's
own residual) slips past it too.** `invocation_allowlist_verdict` only examines lines that
literally contain `"${T[@]}"`; a line reading `command \` (the wrapper alone, continued) does not
contain that substring at all, so it is never scanned, and the SECOND line — `moon ci "${T[@]}"
--base origin/main --include-relations`, say — can be BYTE-IDENTICAL to an allowed entry once
split onto its own line this way, so it passes both the exact-match rule and the count (the line
still carries the expansion and still counts toward the total). Closing this needs either
reassembling continuations before matching (parsing the same bash control flow this file has
repeatedly gotten wrong, per L9) or literally forbidding a backslash on any line adjacent to one
containing `"${T[@]}"` — neither attempted here. Not believed reachable through an *unwrapped*
continuation: a plain `moon ci \` (no wrapper) still starts with `moon` at column 0, so check 8's
`continued` verdict catches THAT shape before `invocation_allowlist_verdict` ever needs to (see
Check 8b's row in the table above — `continued`/`swallowed`/`wrapped` are consulted first).

**L12 — check 8b pins the invocation LINES, not the control flow around them, so the table
calling it "the PRIMARY guard" should be read that narrowly. Two concretely-identified shapes that
defeated it are now CLOSED; what remains is bounded and named below.** Two shapes were
independently reviewed and found to leave the gate at rc 0 while silencing every gate in `T`
(SMA-542, independent review of PR 150, finding I4):

```
fi || true      # ci.yml's real closing 'fi', a tail appended — no moon, no "${T[@]}", no
                # wrapper token; T_INVOCATION_ALLOWLIST never even scans this line
{ … } || true   # the WHOLE if/fi wrapped in a brace group — the invocation lines inside stay
                # byte-identical to T_INVOCATION_ALLOWLIST, so check 8b sees nothing wrong there
```

The first — `fi || true` — was closed by check 8's `block-swallowed` verdict (a `fi`/`done`/`}`
line is checked for a discarded exit status the same way a `moon` line already was). That verdict
also closes the second shape's OWN closing line (the `}` in `{ … } || true` is itself a
`}`-first-token line with a tail).

A THIRD shape survived both of those: an always-false OUTER conditional WRAPPING the whole block
(`if false; then … fi`), where all three invocation lines stay byte-identical to
`T_INVOCATION_ALLOWLIST` and check 8b sees nothing wrong while nothing executes on any event path
at all (CodeRabbit round 5 on PR 150) — no terminator line carries a tail for `block-swallowed` to
catch, so that verdict has nothing to fire on. **Check 8d (above) closes this**, by declining to
analyse the shape at all: it extracts the step's `run:` block, dedents it, and EXECUTES it — once
per GitHub event path, against a `moon` stubbed on a minimal PATH — asserting a real invocation
happens, carrying the whole `T` array and the right `--base`/`--include-relations` shape. Because
it runs the actual bash rather than pattern-matching it, it needs no enumerated vocabulary the way
`block-swallowed`/`wrapped` do; `if false; then … fi`, and anything else that prevents every path's
control flow from ever reaching an invocation while leaving the invocation LINES untouched, now
reds. It also, as a consequence of comparing actual arguments rather than source text, catches a
line that is individually allowlisted but sits under the WRONG branch (the `if` and `else` bodies
swapped, say — invisible to 8b's position-blind set membership, see check 8d's own row above).

What check 8d does NOT close, and is inherent to executing the block in ISOLATION rather than
simulating the whole job: a `if:` condition on the STEP's own YAML key (a sibling of `env:` and
`run:`) that would make GitHub skip the step entirely is invisible to it — check 8d never reads
that key, so the extracted `run:` block still executes fine standalone even if the step itself
would never run for real. And, as originally named here, a LATER STEP in the SAME job silently
overwriting or masking this step's outcome is outside a check that only ever looks at this one
step's own content. Closing either needs simulating the surrounding job, not just this one step's
block — the same reachability-analysis line every other limitation in this file (L9, L11,
ci_targets.py's L10) declines to cross, one level further out.

**L13 — Check 8e matches lines, not reachability.** Same class as L3's and L10's residual: a
required input or script line parked in an unindented never-executed block (an `if false; then …
fi`, an unindented heredoc) still sits at column 0 and still satisfies the pin even though it
never executes. Deliberate for the same reason L10 states it — genuine bash-reachability analysis
in Python is fragile and out of scope.

**L14 — `REQUIRED_INPUT_SKIP` is an unguarded escape hatch**, exactly as `COE_SKIP`,
`SWALLOWED_SKIP` (L7) and `BRANCH_SKIP` are. Nothing stops an entry with a plausible-sounding
reason waiving a glob that is still genuinely load-bearing. The defence is human review at the
point the entry is added, not an automated check; the value it buys is that a waiver is explicit
and greppable, not that it is correct.

**L15 — the `-ge` arity floors catch a SHRUNK table, not a same-size SWAP.** They assert count
only. Replacing all twenty entries of `T_AFFECTED_SMOKE_REQUIRED_INPUTS` with twenty
different globs — or all three of `T_AFFECTED_SMOKE_REQUIRED_SCRIPT`'s — passes both floors
unchanged; only the per-entry `missing-input`/`missing-script` verdicts would catch that, and
only for globs still named somewhere in the (now-different) table.

**L16 — `ACTIONLINT_SH_CALL_SITES` itself has no arity floor**, the same hole one level up.
Deleting an entry from that tuple un-pins only the one line it names — every OTHER pinned line
keeps its own production effect — so the hole is bounded, not compounding, and a floor here would
itself need a pin to be honest, which does not terminate the regress, only moves it out one more
level. Recorded rather than closed, deliberately (SMA-572).

**L17 — five of check 8e's fixture rows hard-code the literal globs `CLAUDE.md` / `moon.yml`**,
rather than deriving them from `T_AFFECTED_SMOKE_REQUIRED_INPUTS` the way the twenty per-glob
deletion rows do. They must: the fixture compares exact verdict strings, and deriving the
expectation from the same table the production code reads would make the fixture adapt to
whatever the table says — the self-referentiality the arity floors exist to break. Cost:
legitimately dropping either glob from the table requires re-baselining those five rows by hand,
which fails loudly (a red self-test), not silently.

**L18 — check 8e identifies its task by indentation and name, not by nesting.** The extractor's
task-key rule is `/^  [^ \t#][^:]*:/` — any key at exactly two spaces — and it then compares that
key's text to `affected-smoke`; nothing requires the key to sit under `tasks:`. A two-space
`affected-smoke:` key under some other top-level mapping would therefore be read as the task, and
`no-task` means only "no such two-space key was found", not "moon.yml declares no such task".
This is a precision-of-documentation point rather than a rot path, and is recorded on those terms:
to green the check through a decoy key you would have to reproduce all twenty `inputs:` entries
and all three `script:` lines, in order, underneath it — at which point you have written the real
block a second time rather than switched the gate off, and the real block is still whatever it is.
The extractor is deliberately left alone; tightening it to require `tasks:` would buy nothing that
the cost of defeating it does not already buy.

**L19 — two of `affected_smoke_block_verdict`'s own membership tests are anchor-dependent, and
both anchors are now fixture-covered.** The `INPUT` needle is anchored at BOTH ends
(`${nl}INPUT${tab}${glob}${nl}`), not just the leading one, so a declared glob that merely EXTENDS
a required one (`ci/actionlint/**/*.sh` against the required `ci/actionlint/**/*`) is still
reported missing rather than silently satisfying it. The `ERR` needle is anchored to the START of
a record (`${nl}ERR${tab}`), so a `SCRIPT` line whose own text happens to contain `ERR<TAB>` is not
misread as an ERR record — which would otherwise short-circuit the verdict to empty before any
`missing-input`/`missing-script` check runs, waiving all requirements at once. Losing either anchor
is exactly the kind of narrowing edit L15's same-size-swap gap does not catch; the two rows added
alongside this entry (SMA-572 follow-up) close it by mutating each anchor in turn and asserting the
row fails.

**L20 — the release guard is a YAML/regex verdict, not a shell or a semantic one (SMA-579).**
Three residuals, each accepted deliberately rather than overlooked.

`command_segments` is a bare regex split on `&&`, `||`, `;` and `|` with no quote or escape
awareness, so a separator inside a quoted string is mishandled. Ruling 10 accepted that: a full
tokeniser would close a bypass nobody has demonstrated, at the cost of new parsing surface in the
file where a bug is most expensive.

`PUBLISH_MARKERS` is a closed vocabulary and cannot see a publish mechanism it does not name — a
`curl` upload, a bespoke action, a script whose name gives nothing away. That is why V1 is
inverted (every job is gated unless pinned in `UNGATED_JOBS`) rather than derived from detection.
Detection now carries a second job, V7: it also asserts an `UNGATED_JOBS` member contains no
publish step. V7 therefore inherits the vocabulary's blind spot, and this is the honest statement
of what it buys — it closes the known verbs (measured: a `release-pr` job running `cargo publish`
+ `npm publish` + `pypa/gh-action-pypi-publish` used to pass at exit 0), not the unknown ones. A
new publishing tool must be added to `PUBLISH_MARKERS` with a fixture row.

Callee resolution follows local `uses: ./` calls ONE level out of the main workflow only. No
workflow in this repository calls a second local workflow; if one ever does, its callee is
unguarded until this is extended.

**L21 — `PUBLISH_MARKERS` is a closed vocabulary, and V8b/V8c are only as complete as that list
(SMA-603).** L20 already states this for V7; the same blind spot applies to `job_publishes()`
everywhere it is used, V8b and V8c included. Measured: each of the following reads clean in a
pre-approval job — `uses: JS-DevTools/npm-publish@v3`, a composite action, a shell script that
publishes, a raw `curl` to the crates.io API, and `gh release create` — because none of them
matches an entry in `PUBLISH_MARKERS`. The two checks are not equally exposed. V1's inverted
design (every job is gated unless pinned in `UNGATED_JOBS`) is a compensating control for V8b:
an unrecognized publish step in a job that is not gated at all still reds V1, independent of
whether `PUBLISH_MARKERS` ever saw it. V8c has no such compensating control — a publish step
`PUBLISH_MARKERS` cannot see, added to a job that IS gated but sits off `approve-release`'s own
`needs:` path, passes both checks clean. A new publishing tool or mechanism must still be added
to `PUBLISH_MARKERS` with a fixture row.

**L22 — a self-test helper's registration in `self_test`'s tuple is unpinned; deleting a helper's
row leaves the suite green (SMA-603).** This is the pre-existing shape L4 and L15 already name
for other tables. It is shared by ALL SIXTEEN registered helpers, not by two. SMA-603 added nine
of them (`_v8d_pre_approval_callee_publish`, `_v8d_sneak_shape`,
`_v8d_unverifiable_remote_uses`, `_v8d_unverifiable_nested_local_callee`,
`_v8d_dedup_shared_callee`, `_v8d_dedup_shared_nested_target`, `_v8d_approval_gate_self_case`,
`_v8d_missing_local_callee_direct` and `_v8_fix4_dry_run_boundary_cases`) alongside the two that
were already there (`_critical2_end_to_end`, `_minor9_empty_jobs_floor`). SMA-602 added five:
fix round 1 added `_v10_minor6_scalar_env_fails_closed` (V10 Minor 6 below), the final review
added `_v10_rule1_strict_equality`, and the fix wave added `_v11_id_token_write_required` (F2,
L25), `_v12_npm_floor_pinned` (F3, L26) and `_non_list_steps_fails_closed` (F7). The
`--fixture-count >= 120` floor counts fixture-table rows, not registered helpers, so it does not
reach this table and cannot catch a deleted registration — and nine of the sixteen now exposed
are the V8d controls this branch relies on.

COUNT THIS BY HAND WHEN YOU ADD ONE. The number above is prose, and nothing asserts it: it read
"ALL TWELVE" against an actual thirteen for the whole of SMA-602's final review, because
`_v10_rule1_strict_equality` was registered without the sentence being updated. `self_test`'s
tuple is the only authority.

**L23 — V10 is a NAME-based scan, and `secrets: inherit` names nothing (SMA-602).** V10 bans
`PYPI_API_TOKEN`, `NPM_TOKEN` and `NODE_AUTH_TOKEN` by literal name, plus an npmrc `_authToken`
write, wherever any of them can appear: job env:, job container:/services: env:, job-level
secrets:/with: (the reusable-workflow-call shape), the workflow-level env:, and step
env:/run:/with:. Two things it deliberately does not, and cannot, close by scanning harder.
`secrets: inherit` on a job that calls a reusable workflow forwards EVERY secret the caller holds
— PYPI_API_TOKEN and NPM_TOKEN included, if either is ever reintroduced as a repository secret —
without naming any of them; a name-based check has nothing to match. And any credential that
reaches the workflow by a path carrying no literal name at all — an action that reads a value
from a URL, a secret referenced only through an indirect expression, a credential baked into a
third-party action's own defaults — is invisible the same way `PUBLISH_MARKERS` (L20/L21) cannot
see a publish mechanism it does not name. V10 is a compensating control alongside OIDC trusted
publishing, not a replacement for it: OIDC removes the credential from the workflow entirely,
V10 makes its reintroduction loud rather than silent.

Rule 1 is no longer bound to one SPELLING, and that was a real defect (SMA-602 fix wave, F1).
`secrets.NAME` was the only form the extraction recognised; `secrets['NAME']`, `secrets["NAME"]`,
`Secrets.NAME` and `SECRETS.NAME` — all accepted by GitHub Actions — each returned no name at
all, and each was MEASURED as a live bypass at guard exit 0 against a copy of the real
`release.yml`. The fix REUSES `ci/workflow-credentials/workflow_credentials.py`'s `EXPR_SPAN` /
`STRING_LITERAL` / `SECRETS_CTX` machinery, where those same four spellings were already pinned
as live fixtures, rather than inventing a second and weaker regex. A reference that names NO
secret (`toJSON(secrets)`, `secrets[format(...)]`) now reds as unresolvable: a strict-equality
pin of names cannot judge a name that does not exist until run time, so reporting it clean would
be a lie.

**L24 — V10 rule 2 is bound to ONE action name; rule 1 is what generalises (SMA-602).**
`uses.startswith(PYPI_PUBLISH_ACTION)` matches `pypa/gh-action-pypi-publish` and nothing else, so
a step running `uv run twine upload -u __token__ -p "$PYPI_CRED"` with
`env: PYPI_CRED: ${{ secrets['PYPI_NEW'] }}` matches no rule 2 at all. It reds ANYWAY, through
rule 1: the secret name is not in `EXPECTED_RELEASE_SECRETS`. That is the honest statement of the
division of labour — rule 2 is a KEY-based rule for one action, rule 1 is the general one — and
it is deliberately not fixed by enumerating upload tools, which would be `PUBLISH_MARKERS`' own
closed-vocabulary problem (L20/L21) in a second place. The residual is the same one L23 already
states: a credential that reaches the workflow with no literal secret name is invisible to both
rules, whatever tool consumes it.

**L25 — V11 asserts the OIDC grant exists, in `release.yml` only (SMA-602).** V10 bans the OLD
mechanism; before V11 nothing asserted the NEW one was still wired. Deleting `id-token: write`
from `publish-pypi` or `publish-npm` — or adding a narrower job-level `permissions:` block, which
sets every scope it omits to `none` — left every gate green, while at run time the runner sets no
`ACTIONS_ID_TOKEN_REQUEST_*` variables, npm's `oidc.js` returns undefined without throwing, and
the publish dies `ENEEDAUTH` after crates.io has published. V11 is scoped to `release.yml` by
name and to the two literal job names: a CALLED workflow legitimately declares no grant, and
`repo:workflow-credentials` actively BANS one in any `pull_request`-triggered workflow, so a
file-wide rule would red a correct repository. It asserts the GRANT, not that the token is
usable: a repository-level or organisation-level setting that disables OIDC is out of reach here.

**L26 — V12 pins the npm 11.5.1 floor across BOTH workflows that carry it (SMA-602).**
`release.yml`'s `publish-npm` job and `prebuild.yml`'s `assemble` job each carry a copy of the
same npm provisioning and floor assertion, and nothing cross-pinned them: `grep -rn '11.5.1' ci/`
found nothing, so deleting both steps — or lowering only ONE copy — kept `moon ci` fully green.
V12 pins seven discrete stripped whole lines, matched against the PARSED `run:` bodies so a copy
living only in a comment cannot satisfy it. It lives in `release_guard.py` rather than
`ci/affected-graph/ci_targets.py` for the reason check 8f records for its own choice
(`repo:actionlint` carries `inputs: ['**/*']`, so the pin needs no new input registration and is
not the sole judge of its own reachability), and because the guard already READS both files —
check 10 runs it on `release.yml`, whose `prebuild` job carries
`uses: ./.github/workflows/prebuild.yml`, so `check_called` reaches the second copy for free.
Two limits. V12 pins TEXT, not behaviour: a line kept but reordered, or moved into a step that
never runs, still satisfies it. And it says nothing about `wheels.yml` or any future third copy —
a new subject must be added to `NPM_OIDC_FLOOR_SUBJECTS` by hand.

**L27 — shellcheck never sees a `${{ }}` expression (SMA-539).** actionlint substitutes every
`${{ ... }}` GitHub Actions expression with an inert placeholder BEFORE handing a `run:` block to
shellcheck, so an unquoted expression can never trigger `SC2086` — only an unquoted **shell**
variable can. Measured A/B on check 3's fixture: `- run: rm -rf $TARGET` fails with `[shellcheck]`
as expected, while the same fixture rewritten as `- run: rm -rf ${{ github.workspace }}` (unquoted,
and a real, defined expression) passes cleanly at rc 0, asserting nothing. The same A/B was
repeated against a real workflow, not just the check-3 fixture: mutating an unquoted shell
variable into `images.yml` reds this gate, while the same mutation expressed as an unquoted
`${{ }}` expression passes at rc 0. Both measurements land on the same conclusion — this is why
the check-3 fixture uses a shell variable rather than an expression, an expression-shaped fixture
would be silently decorative — and why a real workflow with an unquoted, attacker-influenced
`${{ }}` expression in a `run:` block is a gap this gate does not close; that is a job-injection
concern outside this gate's scope.

**L28 — `SC2148`/`SC2164` cannot fire here, structurally.** actionlint always supplies the shell
(there is no missing shebang for `SC2148` to complain about) and always wraps a `run:` block's
script in its own generated harness, which sets `-e` — so `cd` failures that `SC2164` warns about
are already fatal by construction rather than silently ignored. Neither rule is disabled by
configuration; both are simply unreachable given how actionlint constructs the script it hands to
shellcheck.

**L29 (SMA-597).** Check 12 gates the PRESENCE of the procedure's five load-bearing literals,
not its correctness. Editing the `jq` inside CLAUDE.md's block into something subtly wrong stays
green. Closing this needs a gate that EXECUTES the procedure against a deliberately failed task;
that is a follow-up issue, not scope here.

**L30 (SMA-597).** Check 12 is structurally blind to the token in its own two files
(`ci/actionlint/run.sh`, `ci/actionlint/README.md`), both allowlisted because run.sh must contain
the search pattern and the README must document it. Broken advice written into the gate's own
source is invisible to it.

**L31 (SMA-597).** The corpus reads the git INDEX (`git ls-files`), so a file written but not yet
`git add`ed is invisible. A local run can be green where CI is red.

**L32 (SMA-597).** Check 12 keys on which files carry the token `ciReport`, not on what those
files say about it. A file already marked or allowlisted can gain a fresh paragraph of broken
diagnosis advice right next to the token and still pass — the same class of gap L29 already names
for CLAUDE.md's own procedure, generalised to every file the corpus covers. The adjacent, and
more likely, escape is the inverse: writing that same broken advice while paraphrasing around the
literal token — describing a `ciReport` field, a captured task output, or a moon failure diagnosis
step without ever spelling the four characters — passes the corpus scan cleanly, since Assertion A
never reads for meaning, only for the token's presence. Closing either needs the same
procedure-execution gate L29 defers to a follow-up issue, not a bigger token list.

## Cost

`inputs: ['**/*']` is deliberate (see the WHY comment on the `actionlint:` task in `moon.yml`),
and it was benchmarked before being accepted (SMA-525): Moon's own per-task floor in this repo is
~9–11s regardless of what a task does — an existing narrow-input task (`repo:promtool`) measures
about the same. Once `.moon/workspace.yml`'s `hasher.ignorePatterns` excludes gitignored
dependency trees (`node_modules`, `target`, `.venv`) from the hash walk, broad `inputs: ['**/*']`
costs only ~1s over a narrow input list.

Without that filter it costs **~87s**. Alternating `moon run repo:actionlint --force` runs
(macOS, warm):

| Configuration | Time |
|---|---|
| `repo:promtool` — existing narrow-input task, i.e. Moon's floor | ~8.7s |
| this gate, narrow input list | ~10.4s |
| this gate, `inputs: ['**/*']` **with** `hasher.ignorePatterns` | ~11.6s |
| this gate, `inputs: ['**/*']` **without** it | ~98.6s |

Narrowing this task's inputs would not meaningfully help; do not do it without also revisiting
`hasher.ignorePatterns`.

**Standalone cost, since SMA-542.** Check 9's mutation battery — ten mutants plus an
unmutated control, each a full `--self-test` invocation, run concurrently, full-gate only — is the
dominant addition; check 8's floor/`continued`/`swallowed`/`continue-on-error` assertions, check
8b's allowlist/count assertions and check 8c's two-call-site assertion are a handful of
`grep`/`sed` passes over one file each and cost nothing worth measuring by comparison. Check 8d is
the exception to that "cheap" pattern: unlike 8/8b/8c it does not stay at the text-scanning level —
its self-test's fixtures that reach the per-event-path loop each spawn real subprocesses (a scratch
`mktemp -d`, then a `bash -c` per event path, itself shelling out to `grep` and a stubbed `moon`),
so it is the second-largest addition after the battery itself. Four tables below, EACH LABELED WITH
THE STATE IT MEASURES (independent review of PR 150 round 4, finding F3 — an unlabeled table reads
as "the current numbers" regardless): measured min-of-7 (`ci/actionlint/run.sh`, bypassing Moon;
`uptime` immediately before read load averages 2.02/3.35/4.36 — this box runs other concurrent
sessions and a mean can read several times inflated under a load spike, hence min-of-7 rather than
a mean).

State: six fixture tables, six mutants (the ORIGINAL SMA-542 fix wave — superseded by the second
table below; kept for the before/after narrative that follows it, not as current numbers):

| Invocation | Min-of-7 |
|---|---|
| `ci/actionlint/run.sh` (full gate, with the battery) | ~4.11s |
| `ci/actionlint/run.sh --self-test` (six fixture tables, no battery) | ~1.26s |

Before SMA-542 the full gate measured ~1.5s standalone and `--self-test` ~1.0s. SMA-542 itself
(five self-tests, five-mutant battery) brought those to ~3.68s / ~1.25s. That fix wave added the
sixth self-test (`kill_predicate_self_test`, closing spec T3) and check 8's `continued` verdict,
taking the mutant count to six (seven concurrent `--self-test` subprocesses, control included).
A later wave (SMA-542 CodeRabbit rounds 3-4) added the SEVENTH self-test
(`invocation_allowlist_self_test`, closing round-3 finding B) and check 8b's allowlist/count
verdicts, taking the mutant count to seven (eight concurrent `--self-test` subprocesses, control
included) — superseded by the third table below; numbers kept here for the before/after
narrative, not as current figures. A still later wave (SMA-542 residual closure, PR 150
follow-up) added the EIGHTH self-test (`affected_graph_wiring_self_test`, closing L6) and check
8c's two-call-site assertion, taking the mutant count to eight (nine concurrent `--self-test`
subprocesses, control included) — superseded by the fourth table below; numbers kept here for the
before/after narrative, not as current figures. A still later wave (SMA-542 residual closure, PR
150 follow-up — closing L12) added the NINTH self-test (`block_execution_self_test`) and check
8d's block-execution assertion, taking the mutant count to nine (ten concurrent `--self-test`
subprocesses, control included) — superseded by the fifth table below; numbers kept here for the
before/after narrative, not as current figures.

State: seven fixture tables, seven mutants (superseded by the fourth table below; kept for the
before/after narrative, not as current numbers; load averages 2.87/2.49/2.79 immediately before):

| Invocation | Min-of-7 |
|---|---|
| `ci/actionlint/run.sh` (full gate, with the battery) | ~5.20s |
| `ci/actionlint/run.sh --self-test` (seven fixture tables, no battery) | ~1.66s |

State: eight fixture tables, eight mutants (superseded by the fourth table below; kept for the
before/after narrative, not as current numbers; load averages 3.79/5.68/5.27 immediately before,
after waiting for the box's load to settle from an initial 6.82/9.18 — see the gotcha on this
box's shared-session spikes):

| Invocation | Min-of-7 |
|---|---|
| `ci/actionlint/run.sh` (full gate, with the battery) | ~6.33s |
| `ci/actionlint/run.sh --self-test` (eight fixture tables, no battery) | ~2.01s |

State: nine fixture tables, nine mutants (superseded by SMA-572, below; kept for the before/after
narrative, not as current numbers; load averages 4.23/4.93/4.47 immediately before this table's
full-gate runs and 8.52/5.86/4.86 before the `--self-test` runs — this box had two peer sessions
actively `busy` throughout, per the shared-session-spikes gotcha; min-of-7 is chosen specifically
because it is far less sensitive to that than a mean would be):

| Invocation | Min-of-7 |
|---|---|
| `ci/actionlint/run.sh` (full gate, with the battery) | ~14.42s |
| `ci/actionlint/run.sh --self-test` (nine fixture tables, no battery) | ~3.52s |

Both numbers grew more than the earlier waves': check 8d's self-test is the first fixture table in
this file that spawns real subprocesses per fixture (a scratch `mktemp -d`, then a `bash -c` per
event path, itself shelling out to `grep` and a stubbed `moon`) rather than staying at the
`grep`/`sed`/`awk` text-scanning level every earlier check used — see the WHY comment on the
`actionlint:` task in `moon.yml` for the breakdown.

State: ten fixture tables, ten mutants (SMA-572/SMA-573 added the TENTH self-test,
`affected_smoke_block_self_test`, and check 8e's inputs/script verdict plus its two `-ge`
arity-floor assertions) — superseded by the eleven-table state below (SMA-579); kept for the
before/after narrative, not as current numbers.

**Measured by INTERLEAVING the arms, not by min-of-N per arm.** Every table above times one arm
as a contiguous block and then the other; on this box — whose load average wanders between about
11 and 42 while peer sessions work — that samples the two arms under different load, and it
produced a WRONG number here. The first figure recorded for this wave was a single unaveraged
`ci/actionlint/run.sh` run at ~20.85s, read as "comfortably under the ~38.1s budget". The
sequential comparison it was read against was the error: measured properly, the branch is ~50%
slower than its merge-base, not comfortably inside anything. The runs below therefore alternate
base, branch, base, branch within a single sweep, so host load lands on both arms alike and the
DELTA is trustworthy even where neither absolute is. `base` is `af09a4a`, this branch's
merge-base; `pre-fix-wave` is `c03b44b`, check 8e as first written; `current` is the same check
with `affected_smoke_block_verdict`'s membership tests rewritten fork-free (below).

| Invocation | base | pre-fix-wave | current |
|---|---|---|---|
| `run.sh --self-test` | 3.31 / 3.32 / 3.33 | 5.92 / 5.96 / 5.64 | 3.90 / 3.87 / 3.83 |
| `run.sh` (full gate, sweep A) | 15.24 / 14.76 / 15.40 | 22.58 / 23.02 / 25.56 | 16.57 / 18.94 / 22.17 |
| `run.sh` (full gate, sweep B) | 16.26 / 15.07 / 14.88 | 25.83 / 23.45 / 22.15 | 17.68 / 18.87 / 16.78 |

Read column-wise within a row: each triple is the three runs of one arm in one interleaved sweep,
so `base`'s first run and `pre-fix-wave`'s first run are adjacent in time, and so on. The full
gate's `current` column is the noisiest of the three because check 9's battery runs eleven
`--self-test` subprocesses concurrently and is therefore the most load-sensitive arm; its two
sweeps bracket the same ~17–19s centre.

Where the added time GOES, and what removed most of it: check 8e's fixture table is ~40 rows,
each a `mktemp` plus an awk pass plus — as first written — one `printf … | grep -qxF` subshell per
required input and a four-process `grep -nxF | head | cut` pipeline per required script line, so
roughly fifty forks per verdict call, ~45 verdict calls per `--self-test`, times eleven concurrent
mutants in the battery. The fix wave replaced every one of those membership tests with bash
pattern matching against a newline-delimited haystack (see the comment in
`affected_smoke_block_verdict`); the two implementations were proved equivalent over 4,548
differential comparisons across two mutation corpora, and shown to have an identical
mutation-detection profile row-for-row against the fixture table. That recovered about two thirds
of the `--self-test` cost and roughly half of the full gate's.

**The remaining cost is ACCEPTED, and the budget was NOT met — say so rather than rounding it
away.** The design doc's trigger is "reconsider above baseline + 10%"; on the figures above the
full gate sits ~21% over base (sweep A ~27%, sweep B ~15%) and `--self-test` ~17% over, so the
trigger fires and this paragraph is the reconsideration it asks for. Accepted on three grounds: the gate's ABSOLUTE time
stays around 17–19s, well inside the range this section has treated as ordinary for it; the added
work is exactly what buys this branch's central guarantee, that `repo:affected-smoke` still
declares the inputs which schedule every pin in `ci_targets.py` and still runs both halves of its
script in order — a guarantee with no cheaper implementation that keeps the fixture table honest;
and a gate that quietly passed a budget check it had not actually met would be the very thing
this branch exists to stop. A future wave that wants the last ~15% back should look at the
per-row `mktemp` and the per-row awk pass, not at thinning the fixture table.

**Do not conclude `hasher.ignorePatterns` is inert from the log.** It does *not* silence the
~2000 `only files can be hashed` warnings about pnpm's symlinked store — those appear identically
with and without it (verified). The warnings come from input collection; the filter skips the
hashing that follows. Judge it by the wall time above, not by the warnings.

**SMA-579 added an ELEVENTH self-test.** `release_guard_self_test` (check 10) shells out to
`uv run --project py python3 ci/actionlint/release_guard.py`, a real subprocess rather than the
grep/sed/awk level most checks 1-8 sit at. It was re-measured via five INTERLEAVED
`--self-test`/full-gate pairs (moon 2.5.3; sequential min-of-N is invalid on this shared host, per
the note above — this measurement follows the interleaved method, not min-of-N):

| Pair | `run.sh --self-test` | `run.sh` (full gate) |
|---|---|---|
| 1 | 4.20s | 17.86s |
| 2 | 4.49s | 20.32s |
| 3 | 3.61s | 15.40s |
| 4 | 3.59s | 16.03s |
| 5 | 4.29s | 19.48s |

**SMA-603 added a THIRTEENTH self-test.** `release_plan_self_test` (check 11) shells out to `uv
run --locked --project ci/release-plan python3 ci/release-plan/release_plan.py --fixture-count`
plus two more subprocess invocations of `ci/release-plan/run.sh` (`--self-test`,
`--negative-control`) — the same class of real-subprocess self-test check 10 added. Measured as a
single before/after pair on one host, not the five-pair interleaved method above — treat this as
a single-host measurement, not a statistically robust one: `run.sh --self-test` moved 4.22s ->
4.71s; the full gate moved 17.64s -> 20.31s.

**SMA-608 added an eighth row to `negative_control()`** (check 11's own negative control, not a
new self-test), a shape-validation mutation that neuters `config_sections`'s `[workspace]`
type check. It raises `negative_control()`'s own `uv run` subprocess count from six to seven per
invocation: four direct `uv run` calls in the function body (rows 3, 4, 7, 8) plus three indirect
ones reached through `run_checker`/`github_output` (rows 1, 2, 5). Measured standalone — this
project's own `--self-test` and `--negative-control`, bypassing `ci/actionlint/run.sh` and Moon
entirely — min-of-3 on this session's sandbox host: `ci/release-plan/run.sh --self-test` ~0.10s,
`ci/release-plan/run.sh --negative-control` ~0.81s. Check 9's battery re-invokes the whole of
`ci/actionlint/run.sh --self-test` (which calls `release_plan_self_test`, which calls both of
these) roughly fifteen times per full gate run — fourteen mutants plus the unmutated control — so
the new row's `uv run` is paid roughly 15x per `moon run repo:actionlint`, not once. This does not
change check 9's own fixture-table or mutant count (still fourteen and fourteen, below); it only
grows the per-invocation subprocess count nested inside check 11.

**SMA-597 added a FOURTEENTH self-test.** `doc_diagnosis_self_test` (check 12) is a fixture-table
check at the same `grep`/`sed` level as checks 1-8f, not a real-subprocess check like 10 and 11 —
it drives `doc_diagnosis_verdict` and `claude_md_block_verdict` against `mktemp -d` fixtures, with
no `uv run` or external tool. Measured min-of-3 on this session's sandbox host, not the interleaved
or paired methods above and not comparable to the ~15.4-20.3s band recorded there — this host runs
noticeably slower overall (`--self-test` 6.47-6.48s, full gate 37.2-38.0s) — so only the row/mutant
counts below are asserted from this measurement, not a delta against any prior figure.

State: CURRENT — fourteen fixture tables, fourteen mutants (fifteen concurrent `--self-test`
subprocesses in check 9's battery: fourteen mutants plus the unmutated control). Nested one level
in, check 11's own `negative_control()` now runs seven `uv run` subprocesses per invocation
(SMA-608, above), not six, so the battery's fifteen concurrent `--self-test` subprocesses each pay
that extra `uv run` too — check 9's fixture-table and mutant counts are unchanged by this, only the
subprocess count nested inside check 11 grew. `--self-test` and
full-gate timings continue to vary by host and load, as the notes above already establish; see the
SMA-597 paragraph immediately above for the most recent measurement. The toolchain also moved moon
2.3.2 -> 2.5.3 earlier under this branch (SMA-595), so no figure in this section should be read as
a delta against another host's number — only the row/mutant/subprocess counts are load-bearing.

**SMA-539 added shellcheck to check 1, one `uv run` on the full-gate path plus a shellcheck pass
over six workflows.** CLAUDE.md's re-measured baseline for `moon run repo:actionlint --force` on
2.5.3 is **35.1s**; `moon run repo:actionlint --force` with this gate's shellcheck integration
wired in measures **33.7-35.6s** — inside the pre-change figure's own spread, not a step change.
(Independently re-measured in the whole-branch review at **34.0s**, the same band.) The `uv run`
that resolves the `shellcheck` binary is paid once, on the full-gate path only — `--self-test`
shells out to neither `uv` nor `shellcheck`, by the same "must stay runnable with neither binary
installed" placement rule the actionlint-binary guard already follows (see "shellcheck
provenance" above).

## Running it

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"   # proto CLIs (moon, actionlint) aren't
                                                            # on a default shell PATH
moon run repo:actionlint      # via Moon, as CI does
ci/actionlint/run.sh          # directly, bypassing the Moon cache
ci/actionlint/run.sh --self-test   # the fourteen fixture tables only, for fast iteration
```

`--self-test` runs the fourteen fixture tables and nothing else — check 9's mutation battery is
full-gate-only, which is what keeps `--self-test` the fast path and what makes the battery's own
mutants (each internally invoked with `--self-test`) unable to recurse into a battery of their
own.

`--self-test` still needs no `actionlint` binary — that is the point of it — but since SMA-540 it
does need a git checkout carrying `refs/remotes/origin/main`, because the branch-filter table's
control pair asserts that a real ref resolves. Since SMA-542 the self-tests run **before** checks
1–6, not after, so on a `--single-branch` or `--depth 1` clone the canary now fires — and the gate
exits 2 — before `actionlint` itself is ever invoked. You therefore lose whatever checks 1–6 would
have found on that run, not merely the self-test tables. Recover with the **explicit refspec** —
a bare `git fetch origin` re-uses the clone's single-branch refspec and fetches nothing else, and
`git fetch origin main` updates only `FETCH_HEAD`, so neither creates the ref (both measured):

```bash
git fetch origin +refs/heads/main:refs/remotes/origin/main
```

then re-run; the whole gate costs a few seconds.

Any other argument exits 2 with a usage line — a typo'd `--selftest` must not run the full gate
and report a pass for something you did not ask for.

Exit codes: `1` = assertion failure, `2` = infrastructure error.
