# SPDX-License-Identifier: Apache-2.0
# SMA-541 — CI target-array coverage gate.
#
# `.github/workflows/ci.yml` runs `moon ci` over a HAND-WRITTEN target array. Nothing asserted that
# array was complete, so a new `repo:*` gate could be added to moon.yml, be perfectly correct, pass
# locally via `moon run repo:<name>`, and never run in CI. There was no red check — the gate simply
# did not exist. That is the SMA-525 silent-omission class, one level up.
#
# Measured, and the reason the reverse check (C2) exists: `moon ci` exits **0** on a target that
# resolves to nothing, including the MIXED case where real targets surround one dead entry
# (`moon ci :promtool :bogus-target :actionlint` -> "Resolved targets: 1", rc 0). So a typo'd or
# renamed entry in `T` was a silent no-op on every PR. (`moon run` does exit 1, but the only
# `moon run "${T[@]}"` path is the initial-push fallback nobody exercises.)
#
# Follows ci/affected-graph/cargo_moon_parity.py's conventions: rc 0/1/2, a `--self-test` negative
# control wired into run.sh's `--negative-control` branch, and never parsing moon.yml.
#
# usage: ci_targets.py [--self-test]
import inspect
import json
import re
import subprocess
import sys
from itertools import zip_longest
from pathlib import Path


class GateAssertionError(RuntimeError):
    """An AUTHORIAL mistake -> rc 1, never rc 2.

    A missing `T=(...)` line, two of them, an absent CLAUDE.md marker: all of these mean someone
    edited a file into a shape this gate cannot read, which is a red with a fix, not a broken tool.
    Routing them to rc 2 would make run.sh `exit 2` the WHOLE affected-graph guard, destroying the
    diagnostics of all eight cascade cases, A1-A5 and assert_include_relations for that run — and
    labelling "you added a second example" as something that triages as "re-run the job" (D2).
    """


class MoonOutputError(RuntimeError):
    """Moon's query output did not have the shape this gate requires -> rc 2.

    Same contract as cargo_moon_parity.py's class of the same name: "moon told us nothing" must
    abort as infrastructure, so a moon upgrade that reshapes the task object fails loudly rather
    than quietly stopping the assertion.
    """


INFRA_ERRORS = (
    subprocess.CalledProcessError,
    json.JSONDecodeError,
    OSError,
    MoonOutputError,
)

# Any `T=` / `T+=` assignment line. Deliberately BROADER than T_ARRAY_RE so that an append
# (`T+=(:new-gate)`) or a second conditional array is REJECTED rather than silently unexamined:
# C1 would still pass while C2 never saw the appended entries.
T_ASSIGN_RE = re.compile(r"^[ \t]*T[ \t]*\+?=", re.MULTILINE)

# The canonical single-line array. `[ \t]*$` rather than `\s*$` is DEFENSIVE, not load-bearing:
# `(.*?)` cannot cross a newline without re.DOTALL, so `\s*$` would not in fact accept a multi-line
# array either. The one behaviour the stricter anchor really changes is CRLF — on a checkout with
# CRLF endings (this repo ships no .gitattributes) `T=(…)\r\n` matches `\s*$` but NOT `[ \t]*$`, so
# the gate reds with the "must stay on one line" message, which is misleading but red rather than
# silently unexamined. Kept as-is: the alternative to a misleading red is a parser that has to
# reason about line endings.
T_ARRAY_RE = re.compile(r"^[ \t]*T=\((.*?)\)[ \t]*$", re.MULTILINE)

# The literal invocation `T` must actually be fed to. C1-C3 assert the array's CONTENTS; nothing
# asserted the array is what `moon ci` is HANDED. Rewriting the call to `moon ci "${T[@]:0:5}"`
# leaves every entry of `T` correct (C1/C2/C3 green), keeps `assert_include_relations` matching —
# its grep is `moon ci +"`, and the flag is still there — and stops eighteen gates from running,
# all green. Deliberately fixed HERE and not by narrowing that grep: its job is "EVERY `moon ci`
# invocation carries the flag", so narrowing it would blind it to a future second invocation.
MOON_CI_INVOCATION = 'moon ci "${T[@]}"'

# What each invocation must actually HAND OVER. Checked instead of the contiguous
# MOON_CI_INVOCATION form above, which is kept only for the fix message: requiring contiguity
# would red `moon ci --base origin/main "${T[@]}"`, which is correct — argument order is not
# the property worth pinning, passing the whole array is.
T_ARRAY_EXPANSION = '"${T[@]}"'

# Which lines count as an invocation. Checked per LINE, not once over the whole file, because
# ci.yml carries TWO invocations (the PR path and the push path) — a whole-file substring test
# would pass with the PR one, the one every gate actually runs under, subsetted.
#
# This is DELIBERATELY BROADER than assert_include_relations' `moon ci +"` grep (run.sh:126), which
# an earlier version of this constant mirrored so the two would "agree on what they are looking
# at". That agreement was a shared BLIND SPOT, not a feature: `moon ci +"` requires the quote to
# follow `moon ci` immediately, so simply putting a flag first —
#
#     moon ci --base origin/main "${T[@]:0:5}" --include-relations
#
# — is seen by NEITHER check. The array is subsetted, eighteen gates stop running, C1/C2/C3 stay
# green because `T` itself is still correct, and the flag grep matches nothing so it does not red
# either (measured; CodeRabbit CLI). Matching the command rather than its first argument closes it.
#
# Anchored at COMMAND POSITION — `moon` must be the line's first token — with `[ \t]+` between the
# two words. Two more holes, both measured, drove this (CodeRabbit round 2):
#
#   echo moon ci "${T[@]}" …     a substring match accepts it; nothing runs, and the line even
#                                carries the expansion, so it looked canonical
#   moon    ci "${T[@]:0:5}" …   a literal single space misses it entirely, so the subsetted array
#                                was never examined
#
# The anchor also makes the old `#`/`name:` lookaheads unnecessary: a comment starts with `#` and
# the job/step titles start with `name:`/`- name:`, so none of them is at command position. Verified
# against the real ci.yml — matches exactly the two invocation lines, and none of the six prose
# comments or two `name:` fields (the `CI / moon ci` required check is keyed on those titles).
MOON_CI_LINE_RE = re.compile(r"^[ \t]*moon[ \t]+ci\b.*$", re.MULTILINE)

# ...and a FLOOR on how many there are, which is the half that actually closes the `echo` case. A
# form this regex stops recognising drops out of `lines` silently, and a per-line rule can say
# nothing about a line it never matched — the derived-set-shrinks-to-empty failure that
# cargo_moon_parity.py's REQUIRED_FFI_TASKS exists to prevent, in miniature. Pinning the count turns
# "the gate no longer understands the invocation" into a red. A genuinely new third invocation reds
# too, which is intended: it must be reviewed, the same default-deny stance D10 takes.
EXPECTED_MOON_CI_INVOCATIONS = 2

# The docs command is delimited EXPLICITLY, not recognised by prose shape. Prose-shape matching was
# fragile in both directions against ordinary doc edits: converting the command to a fenced code
# block zero-matches it, and CLAUDE.md already carries two neighbouring `moon ci …` spans that a
# reword could turn into a second match (D7). Markers also make the contract visible to whoever
# edits the file, and keep the illustrative gate list in the same bullet safely outside.
MARKER_BEGIN = "<!-- ci-targets:begin -->"
MARKER_END = "<!-- ci-targets:end -->"

# task name -> why this CI-eligible `repo` task is deliberately absent from ci.yml's `T`.
# SHIPS EMPTY, and that is the point: it is the sanctioned escape, not a live exemption.
#
# It exists because `runInCI: false` — the only exemption C1 would otherwise honour — is documented
# in this repo as BROKEN for this purpose: "Do NOT set `runInCI: false`: Moon also excludes such
# tasks from `moon run` whenever CI=true, which would make the CI gate resolve zero tasks and exit
# 1" (ts/moon.yml:31-32, repeated at :45-46). CI-eligible-but-not-in-`T` tasks already exist one
# project over — `build-release` on all 13 Rust crates, `contracts:generate`, `ts:commitlint`,
# `ts:check-config-only` — so the day a `repo:*` gate needs its own workflow step, the alternative
# to this table is someone deleting the assertion.
#
# An entry is a RECORDED DECISION, not a silent exemption: the reason string is required and a
# blank one is itself an assertion failure, mirroring cargo_moon_parity.py's ALLOW_NO_CARGO_BACKING.
# The reason must NAME WHERE THE TASK RUNS INSTEAD — the workflow file and step, or the job — not
# merely why it is out of `T`. Nothing here asserts an exempted task is invoked anywhere at all, so
# an exemption reopens this gate's own problem statement for that one task by construction, and the
# reason string is the only record a reviewer can check it against.
T_EXEMPT = {}

# The floor. C1 compares two derived sets, and two EMPTY sets compare equal — so a project-id
# filter that stops matching, or a moon output shape change, would print PASS while asserting
# nothing. Every task named here must be present and CI-eligible in the parsed `repo` set.
# Same role as cargo_moon_parity.py's REQUIRED_FFI_TASKS.
#
# The three release-parity* tasks joined the floor with SMA-530: they now carry a negative
# control, and check_forward's `want`/`got` shrink CONSISTENTLY when a task is dropped from
# `T` and made CI-ineligible in the same edit — so without a floor entry the control could be
# switched off entirely with every check green.
REQUIRED_REPO_TASKS = (
    "affected-smoke",
    "promtool",
    "publish-metadata",
    "release-parity",
    "release-parity-py",
    "release-parity-ts",
)

# SMA-553 D13 — repo:input-liveness's `inputs: ['**/*']` is load-bearing, and asserting it ONLY
# inside that gate would make it the sole judge of its own configuration. This is the second,
# independently-scheduled copy: it runs inside repo:affected-smoke.
#
# THE NAME IS HISTORICAL: each value is that gate's WHOLE authored input set — globs first
# (sorted), then literal files (sorted) — because moon resolves a wildcard `inputs:` entry into
# inputGlobs and a LITERAL path into inputFiles. While this table held only repo:input-liveness
# every value was a glob; repo:version-lockstep (SMA-576) declares sixteen literal paths and no
# glob at all, so a glob-only comparison would have read every one of them as absent and the entry
# could not have been written. The order is FIXED (globs, then files) purely so the comparison is
# deterministic and the failure message reads in a stable order; what actually catches a widening
# such as `rs/Cargo.toml` -> `rs/*.toml` is the changed STRING, which moves the entry between the
# buckets and out of the expected sequence at the same time. The constant keeps its name because
# CLAUDE.md's gotchas reference it verbatim.
#
# repo:version-lockstep's entry is the file-level twin of its SELF_SCHEDULED_GATES pin below: that
# one proves the gate still RUNS both halves, this one proves it is still SCHEDULED by every file
# that carries a version. Drop `py/uv.lock` here and the gate stops re-keying on the one file a
# `uv lock` rewrite touches — it would still report PASS, from cache, over a tree it never read.
SELF_TASK_EXPECTED_GLOBS = {
    "input-liveness": ("**/*",),
    "version-lockstep": (
        "ci/version-lockstep/run.sh",
        "py/packages/paigasus-kernel/pyproject.toml",
        "py/packages/paigasus-proto/pyproject.toml",
        "py/uv.lock",
        "rs/Cargo.lock",
        "rs/Cargo.toml",
        "rs/crates/bindings/paigasus-node-bindings/Cargo.toml",
        "rs/crates/bindings/paigasus-node-bindings/index.js",
        "rs/crates/bindings/paigasus-node-bindings/package.json",
        "rs/crates/bindings/paigasus-py-bindings/Cargo.toml",
        "rs/crates/bindings/paigasus-py-bindings/pyproject.toml",
        "rs/crates/bindings/paigasus-wasm/Cargo.toml",
        "rs/crates/bindings/paigasus-wasm/package.json",
        "rs/crates/libs/paigasus-kernel/Cargo.toml",
        "rs/crates/libs/paigasus-proto-derive/Cargo.toml",
        "rs/crates/libs/paigasus-proto/Cargo.toml",
    ),
    # SMA-572. repo:actionlint's whole authored declaration, and the reason the rest of this
    # file's pins are reachable at all. Checked from ci_targets.py, which runs inside
    # repo:affected-smoke — a DIFFERENT gate — so this is not self-judging: narrowing
    # repo:actionlint's inputs is a root-moon.yml edit, which schedules affected-smoke.
    "actionlint": ("**/*",),
    # SMA-572. These two were exempted in this design's first draft on the grounds that
    # repo:input-liveness asserts declared-glob liveness generically. That is wrong, and an
    # adversarial review caught it: task_inputs.py asserts a DECLARED glob still matches a
    # tracked file — it cannot see a REMOVED DECLARATION. Both lists carry entries whose
    # deletion moon.yml itself documents as fatal: publish-metadata's
    # .github/workflows/security-scan.yml ("Check 4 ASSERTS ON IT"; moon.yml:520-521) and
    # error-code's broad rs/crates/**/src/**/*.rs ("the one case it exists for would be the one
    # case it never runs on"; moon.yml:628-630). Both sets are STATIC — no runtime discovery —
    # so exact match is affordable, exactly as for version-lockstep's sixteen.
    "publish-metadata": (
        "rs/crates/**/*",
        ".github/workflows/security-scan.yml",
        ".gitignore",
        "ci/publish-metadata/categories.py",
        "ci/publish-metadata/crates-io-categories.txt",
        "ci/publish-metadata/run.sh",
        "rs/.cargo/config.toml",
        "rs/Cargo.lock",
        "rs/Cargo.toml",
        "rs/release-plz.toml",
        "rs/rust-toolchain.toml",
    ),
    "error-code-single-site": (
        "ci/error-registry/**/*",
        "rs/crates/**/src/**/*.rs",
        "contracts/proto/paigasus/common/v1/error.proto",
    ),
}

# C3 checks the flag tail too. The first spec draft omitted it on the stated grounds that
# assert_include_relations "already owns the flag question" — it does not: that function greps
# ci.yml only (run.sh:126) and never opens CLAUDE.md. Without this, the documented command could
# lose --include-relations and silently under-build, which is the very behaviour that makes
# checking the docs worth doing (D6).
REQUIRED_DOC_FLAGS = ("--base origin/main", "--include-relations")

# C4 — this gate's own call sites in run.sh, and (SMA-553) repo:input-liveness's inside its own
# task script. Placing a gate inside repo:affected-smoke rather than making it a repo:* task of its
# own (D1) means C1 does NOT cover it: its execution depends on these lines, and deleting one
# leaves everything green. Matched WITH their bash suffixes because the bare name
# `assert_ci_targets` also appears in the function definition, so a name-only match would survive
# deleting the call.
#
# SMA-542 closed the actionlint half of this class: ACTIONLINT_SH_CALL_SITES below pins
# repo:actionlint's own call sites from here. In return, and as of the SMA-542 residual closure
# (PR 150 follow-up), check 8c in ci/actionlint/run.sh pins THESE two entries — this gate's own
# call sites, copied verbatim there as T_AFFECTED_GRAPH_CALL_SITES — from that independently
# scheduled file. Neither gate is now the sole judge of its own SCHEDULING (check 8 still pins
# `:affected-smoke`'s presence in `T`) nor of its own WIRING (check 8c pins these two lines
# directly): deleting `assert_ci_targets || SUITE_RC=1` used to remove C4 silently along with
# itself, with nothing outside ci/affected-graph/ able to notice — check 8c catches that now.
#
# What remains, and is inherent rather than an oversight, is the same shape check 8c's own comment
# names for its production call site (ci/actionlint/run.sh, ACTIONLINT_SH_CALL_SITES below):
# deleting THAT call site (the `done < <(affected_graph_wiring_verdict ...)` line) and this entry's
# `assert_ci_targets || SUITE_RC=1` in the SAME edit still silences both directions at once — two
# independently-scheduled gates are the most the graph offers, and closing a combined deletion
# needs a third, which only moves the same problem one level out.
RUN_SH_CALL_SITES = (
    "assert_ci_targets || SUITE_RC=1",
    # The `|| NEG_RC=1` suffix is as load-bearing as the command. Matching the prefix alone left
    # `--self-test || true` looking identical to a wired call site: the self-test still RUNS, its
    # failure is simply swallowed, and the negative control silently stops being able to report red
    # — the rotted-self-test outcome moon.yml's affected-smoke comment was written about. Both
    # entries now pin their propagation, symmetrically (CodeRabbit round 3), and check 8c in
    # ci/actionlint/run.sh mirrors that same suffix requirement from its own copy of these two
    # strings.
    '"$HERE/ci_targets.py" --self-test || NEG_RC=1',
)

# repo:input-liveness's resolved script must run BOTH its negative control and the real check —
# UNDER `set -euo pipefail`, which is pinned here as a THIRD required line and is as load-bearing
# as either invocation. Moon's `script:` blocks do not enable errexit, so a script's exit status is
# simply its LAST command's; deleting the pipefail line touches neither python3 line's TEXT, so a
# check that pinned only the two invocations would stay green while a failing `--self-test` is
# silently swallowed and only the real check's result is reported — the exact SMA-526
# rotted-negative-control failure moon.yml's own comment on this task cites (measured: with the
# line removed and a deliberately-failing first line, `moon run repo:input-liveness --force`
# reported "Tasks: 1 completed"). Deleting either python3 line leaves a task that still schedules,
# still exits 0, and asserts half (or none) of what its name claims. Its `inputs: ['**/*']` is
# asserted separately, by SELF_TASK_EXPECTED_GLOBS above.
#
# Unlike RUN_SH_CALL_SITES these are matched as WHOLE LINES, and the asymmetry is deliberate:
# `python3 ci/affected-graph/task_inputs.py` is a strict PREFIX of the same line plus
# ` --self-test`, so under a substring test deleting the REAL RUN would leave C4 green while the
# gate no longer ran at all. The run.sh entries cannot be whole-line matched in return — they are
# indented in the file and the second is a mid-line fragment — but they do not need to be: each
# already carries its `|| RC=1` propagation suffix, which makes it unambiguous (SMA-553 D10).
SELF_SCHEDULED_GATES = {
    "input-liveness": (
        "set -euo pipefail",
        "python3 ci/affected-graph/task_inputs.py --self-test",
        "python3 ci/affected-graph/task_inputs.py",
    ),
    # SMA-530. Three sibling tasks over one script, each with its own control: their
    # ECOSYSTEM-SPECIFIC inputs are distinct (moon.yml:81-85, 96-101, 112-119), so a PR
    # touching only ts/packages/paigasus-sdk/.releaserc.json selects release-parity-ts and
    # neither sibling — one shared control would leave that PR running a parity gate with
    # nothing proving it can report red. The input sets are NOT disjoint overall: all three
    # also list ci/release-parity/**/* and .prototools, which is why an edit under
    # ci/release-parity/ schedules all three.
    # Measured net cost +890ms/+733ms/+1111ms per task (~20%).
    #
    # WHOLE-LINE matched, and that is load-bearing in one direction here: the real-run line
    # is a strict PREFIX of the control line in all three tasks, so a substring test would
    # let the REAL RUN be deleted while this pin stayed green. `set -euo pipefail` is pinned
    # as a first-class required line for the reason recorded at :211-221 — Moon's script:
    # blocks have no errexit, so deleting it leaves both invocations' text untouched while a
    # failing control is silently swallowed.
    #
    # These pin the moon.yml INVOCATION only. The control BLOCK they invoke
    # (ci/release-parity/run.sh:60-69) is pinned separately by RELEASE_PARITY_SH_CALL_SITES
    # below — deleting the block while leaving the flag parse makes --negative-control fall
    # through to the real suite and exit 0, which these entries cannot see.
    "release-parity": (
        "set -euo pipefail",
        "ci/release-parity/run.sh --negative-control",
        "ci/release-parity/run.sh",
    ),
    "release-parity-py": (
        "set -euo pipefail",
        "ci/release-parity/run.sh --ecosystem python-semantic-release --negative-control",
        "ci/release-parity/run.sh --ecosystem python-semantic-release",
    ),
    "release-parity-ts": (
        "set -euo pipefail",
        "ci/release-parity/run.sh --ecosystem semantic-release --negative-control",
        "ci/release-parity/run.sh --ecosystem semantic-release",
    ),
    # SMA-576 — repo:version-lockstep, same four-line shape and the same reasoning end to end.
    # The pipefail line is exactly as load-bearing as any invocation: Moon takes a `script:`
    # block's status from its LAST command, so without it a failing `--self-test` or
    # `--negative-control` that has stopped being able to report red is masked by the real run
    # passing, and the gate ships with no proof it bites. The whole-line discipline matters here
    # for the same prefix reason as above — `bash ci/version-lockstep/run.sh` is a strict prefix
    # of BOTH the `--self-test` and the `--negative-control` line, so under a substring test
    # deleting the REAL RUN would leave this check green while CI only ever ran fixture tables and
    # a control against a synthetic tree and never looked at the repo's actual versions.
    # `--self-test` (SMA-576 fix-wave finding 1) closes the same "declared but never invoked"
    # rot this whole table exists to prevent: before this entry, nothing in CI ever ran
    # `run_self_tests` / `SELF_TEST_COUNT` / `site_verdict_self_test`, so they could bit-rot
    # silently while ci/version-lockstep/README.md kept presenting `--self-test` as part of the
    # guard.
    "version-lockstep": (
        "set -euo pipefail",
        "bash ci/version-lockstep/run.sh --self-test",
        "bash ci/version-lockstep/run.sh --negative-control",
        "bash ci/version-lockstep/run.sh",
    ),
    # SMA-572 — the three gates SMA-530 left out, plus repo:actionlint. Same three-line shape
    # and the same reasoning: Moon takes a `script:` block's status from its LAST command, so
    # `set -euo pipefail` is exactly as load-bearing as either invocation. Whole-line matched,
    # which is what makes the first two safe — `bash ci/publish-metadata/run.sh` is a strict
    # PREFIX of its own --negative-control line, so a substring test would report the script
    # fully wired after the REAL RUN had been deleted.
    #
    # repo:affected-smoke's third entry has NO TRUE-POSITIVE COVERAGE and that is deliberate:
    # any state in which the bare `ci/affected-graph/run.sh` line is absent is a state in which
    # THIS function never runs (run.sh:405-409 exits inside the --negative-control branch,
    # before run_suite at :412). Its real enforcement is check 8e in ci/actionlint/run.sh,
    # which is scheduled independently. It is kept here so the table's contract stays "every
    # line, one rule" — do not read it as coverage.
    "affected-smoke": (
        "set -euo pipefail",
        "ci/affected-graph/run.sh --negative-control",
        "ci/affected-graph/run.sh",
    ),
    "publish-metadata": (
        "set -euo pipefail",
        "bash ci/publish-metadata/run.sh --negative-control",
        "bash ci/publish-metadata/run.sh",
    ),
    # No prefix hazard here (--self-test and --single-site are distinct suffixes), but matched
    # whole-line like every other entry: the table's contract is one rule, not per-entry rules.
    "error-code-single-site": (
        "set -euo pipefail",
        "python3 ci/error-registry/check.py --self-test",
        "python3 ci/error-registry/check.py --single-site",
    ),
    # One command, so the script's status IS its status — there is no pipefail line to pin.
    # Registered mainly so its `inputs` pin below is not an orphan_globs row: repo:actionlint's
    # `['**/*']` is the premise that every check in ci/actionlint/run.sh (8, 8b, 8c, 8d and the
    # new 8e) runs on every PR, and until SMA-572 nothing asserted it. Narrowing it to
    # `.github/workflows/**` was a green edit that silently switched all five off.
    "actionlint": (
        "ci/actionlint/run.sh",
    ),
}

# SMA-530. A script-pinned gate whose `inputs` are NOT separately pinned must say so here,
# with a reason — the repo's established idiom (T_EXEMPT, ALLOW_DEAD_INPUT,
# ALLOW_NO_CARGO_BACKING, BRANCH_SKIP, COE_SKIP all work this way).
#
# Why an exemption rather than dropping the pairing rule: repo:affected-smoke's own inputs
# are the most load-bearing input list in the repo (moon.yml:160-197, several entries
# carrying explicit do-not-remove comments), so when it is script-pinned later it MUST also
# have its globs pinned. A plain subset rule would let that be skipped in silence.
SELF_TASK_GLOBS_EXEMPT = {
    "release-parity": (
        "narrow ecosystem-specific globs, unlike input-liveness's `**/*` which IS the thing "
        "that gate exists to protect; declared-glob liveness is asserted generically by "
        "repo:input-liveness (ci/affected-graph/task_inputs.py), so a second exact-match copy "
        "here would red on every legitimate inputs edit and buy nothing"
    ),
    "release-parity-py": "as release-parity",
    "release-parity-ts": "as release-parity",
    # SMA-572/SMA-573. NOT a skip — a delegation, and the harder half of this issue. This gate's
    # nineteen inputs are the most load-bearing list in the repo (every pin in this file is
    # reachable only because it lists `moon.yml`), so they ARE pinned — by check 8e in
    # ci/actionlint/run.sh, which is scheduled independently of this gate. An entry in
    # SELF_TASK_EXPECTED_GLOBS instead would make repo:affected-smoke the sole judge of its own
    # reachability, which is the exact defect SMA-573 exists to close; it would also be an exact
    # match against a list that legitimately grows every time a gate keys on a new directory.
    # ACTIONLINT_SH_CALL_SITES pins check 8e's production call site AND its table's arity floor,
    # so this delegation cannot rot silently.
    "affected-smoke": (
        "inputs pinned by check 8e in ci/actionlint/run.sh instead — an entry here would make "
        "this gate the sole judge of its own reachability, and exact-match a nineteen-entry "
        "list that legitimately grows; ACTIONLINT_SH_CALL_SITES pins 8e's call site and arity "
        "floor so the delegation cannot rot (SMA-572/SMA-573)"
    ),
}

# C4, actionlint half (SMA-542). repo:actionlint's self-tests, mutation battery, and the check-8,
# check-8b, check-8c AND check-8d production call sites are each invoked from ONE call site inside
# ci/actionlint/run.sh. That script cannot assert its own invocation — deleting the self-test calls
# was the sole survivor of SMA-525's mutation battery, deleting the check-8 call site was fix-wave
# finding I1, deleting the check-8b call site reopened that SAME defect one round later (CodeRabbit
# round 4, finding C1) on the check that replaced check-8 as the primary guard, and check-8c and
# check-8d are each their own SMA-542 residual closure (PR 150 follow-up, closing L6 and L12
# respectively) — so the assertion lives here, in a gate scheduled independently of it. The reverse
# direction is check 8 (still-`:affected-smoke`-in-`T`) and check 8c (still-invokes-ci_targets.py,
# i.e. RUN_SH_CALL_SITES above) in that same script: neither gate is the sole judge of its own
# scheduling OR its own wiring.
#
# REACHABILITY IS NOT AUTOMATIC. This check only runs when repo:affected-smoke is scheduled, so
# moon.yml lists `ci/actionlint/**/*` among its inputs. Without that entry a PR deleting these
# lines would not schedule this task at all, while repo:actionlint (inputs: ['**/*']) still ran and
# asserted nothing — the exact defect this closes. Do not remove that input.
#
# Matched as WHOLE LINES, like SELF_SCHEDULED_GATES and unlike RUN_SH_CALL_SITES:
# `run_self_tests` is a strict substring of its own definition line `run_self_tests() {`, so a
# substring test would report the file as wired after the call had been deleted. The third,
# fourth, fifth and sixth entries, the `done < <(...)` lines closing check 8's, check 8b's, check
# 8c's and check 8d's production `while` loops, are unambiguous the same way: `ci_target_floor_
# verdict`, `invocation_allowlist_verdict`, `affected_graph_wiring_verdict` and
# `block_execution_verdict` are each also invoked from inside their own self-test fixtures (e.g.
# `ci_target_floor_verdict "$tmp"` / `invocation_allowlist_verdict "$tmp" "$skip"` /
# `affected_graph_wiring_verdict "$tmp"` / `block_execution_verdict "$tmp"`), but none of those
# calls is the whole `done < <(...)` line that schedules the PRODUCTION run, so a whole-line match
# cannot confuse them. The third entry closed fix-wave finding I1 (the reviewer deleted that exact
# block and measured: full gate rc 0, this gate PASS, with T_FLOOR/swallowed/continue-on-error
# asserting nothing until it closed). The fourth entry closes the SAME defect reopened one round
# later against check 8b (CodeRabbit round 4, finding C1) — measured the same way: deleting the
# whole "# Check 8b ..." block left run.sh at rc 0 and this gate PASSing, because
# invocation_allowlist_self_test still calls the function; only the production `done < <(...)` line
# proves it is also applied to the real ci.yml. The fifth entry closes check 8c's OWN production
# call site the same way — without it, deleting check 8c's block would leave run.sh at rc 0 and
# THIS gate PASSing, because affected_graph_wiring_self_test still calls the function, which is
# exactly the recursive version of the residual check 8c itself exists to close: pinning check 8c
# by anything OTHER than its own production line would make it the sole judge of its own wiring.
# The sixth entry closes check 8d's OWN production call site the same way (SMA-542 residual
# closure, PR 150 follow-up — closing README L12) — without it, deleting check 8d's block would
# leave run.sh at rc 0 and THIS gate PASSing, because block_execution_self_test still calls the
# function.
#
# COLUMN 0, not stripped-both-sides (CodeRabbit, PR 150). check_self_invocation used to build
# `actionlint_lines` with `line.strip()`, so a required line was satisfied by that exact TEXT
# appearing anywhere in the file — including indented inside `if false; then … fi` or a heredoc,
# neither of which ever executes. Wrapping one of these six calls in a conditional block is
# exactly the shape a false negative would take, and it conventionally INDENTS the wrapped line, so
# matching now requires no leading whitespace at all (trailing whitespace is still stripped). This
# is a deliberate ASYMMETRY with the other three haystacks, not an oversight: RUN_SH_CALL_SITES
# matches substrings because its lines are indented inside a bash function, and
# SELF_SCHEDULED_GATES and RELEASE_PARITY_SH_CALL_SITES strip both sides because their lines are
# indented (moon task scripts inside YAML; the `if` body inside run.sh) — all three would break
# under a column-0 requirement. This haystack is different: `run_self_tests`,
# `selftest_mutation_battery` and all four `done < <(...)` lines all sit at run.sh's TOP LEVEL
# (verified: none is nested in a function, `if`, or loop), so column 0 is where the real, executing
# call sites actually live, and is available as a signal here in a way it is not for the other three.
#
# THIS IS NOT REACHABILITY ANALYSIS, and does not claim to be — parsing bash control flow in
# Python is fragile and out of scope (spec decision). What it does NOT close: a required line
# copied into an UNINDENTED `if false; then … fi` block, or an unindented heredoc, still satisfies
# it — column 0 rejects the common case (an indented copy) without attempting the general one. That
# residual is recorded in ci/actionlint/README.md's Limitations section.
#
# PROPAGATION CONTRACT — these entries carry no `|| RC=1` suffix, and that is not the hole
# RUN_SH_CALL_SITES' suffixes close. `run_self_tests` and `selftest_mutation_battery` report through
# run.sh's global `FAILED`, as its nine self-tests already do (run.sh:43-46); NONE of the four
# `done < <(...)` lines has anything to propagate — each is the tail of a `while` loop whose body
# already calls `fail()` per verdict. The consequence is that a future `run_self_tests || FAILED=1` (or an
# equally harmless reformat of any `done < <(...)` line) would red this check even though it is
# harmless; restore the bare line, or update this constant.
ACTIONLINT_SH_CALL_SITES = (
    "run_self_tests",
    "selftest_mutation_battery",
    "done < <(ci_target_floor_verdict .github/workflows/ci.yml)",
    # Check 8b's production call site (SMA-542 CodeRabbit round 4, finding C1) — reopens fix-wave
    # I1 one check later: deleting the whole "# Check 8b ..." block from run.sh left the full gate
    # at rc 0 and this gate PASSing, because invocation_allowlist_self_test still calls the
    # FUNCTION; only this line proves it is also applied to the real ci.yml. Whole-line matched
    # for the same reason as the entry above it: `invocation_allowlist_verdict` also appears inside
    # the self-test fixtures (`invocation_allowlist_verdict "$tmp" "$skip"` and
    # `invocation_allowlist_verdict /nonexistent/ci.yml`), so a substring test would be satisfied
    # by those and survive deleting this exact line.
    'done < <(invocation_allowlist_verdict .github/workflows/ci.yml "$REPORTED_LINENOS")',
    # Check 8c's production call site (SMA-542 residual closure, PR 150 follow-up) — the reverse
    # pin: repo:actionlint's own check that ci/affected-graph/run.sh still invokes THIS gate
    # (RUN_SH_CALL_SITES, above). Same shape as the two entries above it: `affected_graph_wiring_
    # verdict` is also called from inside its own self-test fixture
    # (`affected_graph_wiring_verdict "$tmp"`), so a substring test would be satisfied by that call
    # and survive deleting this exact production line.
    "done < <(affected_graph_wiring_verdict ci/affected-graph/run.sh)",
    # Check 8d's production call site (SMA-542 residual closure, PR 150 follow-up — closes
    # ci/actionlint/README.md's L12): the block-execution check that closes the "wrap the whole
    # invocation block in an always-false conditional" gap check 8b's line-shaped matching cannot
    # see. Same shape as the three entries above it: `block_execution_verdict` also appears inside
    # its own self-test fixtures (`block_execution_verdict "$tmp"`,
    # `block_execution_verdict /nonexistent/ci.yml`, ...), so a substring test would be satisfied
    # by those and survive deleting this exact production line.
    "done < <(block_execution_verdict .github/workflows/ci.yml)",
)

# SMA-530. The moon.yml pins above prove the CONTROL IS INVOKED; these prove it still DOES
# something. run.sh:14 parses --negative-control into NEGATIVE, the guard at :60 gates the
# control body on it, :63 asserts the harness against a deliberately-wrong expectation, and the
# two report arms at :65-66 report the result — five lines in total, pinned here because a
# review MEASURED that pinning only the "act" block (:60-69) leaves two bypasses that defeat the
# control while every one of these five lines stays byte-identical:
#   (a) neuter the PARSE (`--negative-control) shift ;;`, dropping `NEGATIVE=1`): NEGATIVE stays
#       0 (initialised at :9, so `set -u` is satisfied), `run.sh --negative-control` falls
#       straight through to the real suite and exits 0 — the exact failure this registry exists
#       to close, just one line further up than the act block this used to pin alone.
#   (b) gut the ASSERT (`ec=0; check_case ... || ec=$?` → `ec=1`): the control never invokes the
#       harness at all, yet still prints "negative-control OK: harness reported red as expected"
#       and exits 0 — worse than (a), since the control now actively asserts a lie.
# These are the two bypasses a review MEASURED against pinning only the act block; they are not
# an exhaustive enumeration of ways to defeat the control. A third, measured against the five-line
# pin ITSELF, survives: an inserted `NEGATIVE=0` on its own line immediately before the :60 guard
# (same outcome as (a): falls through to the real suite at rc 0) or all five pinned lines parked
# verbatim inside a never-executed heredoc with the block deleted and the parse neutered. See
# ci/release-parity/README.md's Limitations section L5 for that residual and why closing it
# generally is out of scope.
# SELF_SCHEDULED_GATES cannot see any of this: it pins moon.yml text, not run.sh semantics. Same
# class as ACTIONLINT_SH_CALL_SITES above, and the same lesson SMA-542 I1 and CodeRabbit round 4
# C1 each cost a round to learn — a gate check's own call site is what goes unguarded.
#
# REACHABILITY IS NOT AUTOMATIC. This check only runs when repo:affected-smoke is scheduled,
# so moon.yml lists `ci/release-parity/**/*` among its inputs. Without that entry the PR
# deleting this block is exactly the PR that does not schedule this gate. Do not remove it.
#
# Matched as stripped WHOLE LINES, not substrings: for the two `echo` lines, a message-text
# substring match would survive `exit 0`/`exit 1` being swapped or dropped, since the message
# text does not change. Indentation tolerance is deliberate too (unlike ACTIONLINT_SH_CALL_SITES'
# column-0 rule): the `case` arms and the assert line are conventionally indented inside the `if`,
# so a column-0 requirement would reject the real, executing lines.
#
# TRADEOFF, worth recording: pinning the assert line couples this pin to the fixture case id
# ("neg-fix-bang") and its "0.1.1" wrong-expectation literal. If cases.tsv's contract for that
# case ever changes, this entry must be updated with it, or the pin will fire on a legitimate
# edit.
RELEASE_PARITY_SH_CALL_SITES = (
    '--negative-control) NEGATIVE=1; shift ;;',
    'if [ "$NEGATIVE" = 1 ]; then',
    'ec=0; check_case "neg-fix-bang" "fix!: deliberately wrong" "-" "0.1.1" || ec=$?',
    '1) echo "negative-control OK: harness reported red as expected"; exit 0 ;;',
    '0) echo "negative-control FAILED: harness accepted a wrong expectation" >&2; exit 1 ;;',
)


def read_input(path, label):
    """One of the gate's file inputs, with a MISSING file routed to rc 1 rather than rc 2.

    `OSError` is in INFRA_ERRORS, so an unhandled FileNotFoundError would abort the WHOLE
    affected-graph guard with `exit 2` and destroy every other assertion's diagnostics — for what
    is unambiguously an authorial mistake (someone deleted or renamed a tracked file), and would
    triage as "re-run the job" (D2). Every OTHER OSError — permissions, I/O — stays on the rc-2
    path, because those genuinely are environmental.
    """
    try:
        return path.read_text()
    except FileNotFoundError as exc:
        raise GateAssertionError(
            f"{label} does not exist, so this gate cannot read it. If the file was renamed or "
            "moved deliberately, update the paths in ci/affected-graph/ci_targets.py's main(); "
            "otherwise restore it."
        ) from exc


def parse_t(text):
    """The `T=(...)` array from ci.yml, as BARE task names (no leading colon).

    Bare names because that is what they are compared against: moon's task-name keys (C1/C2) and
    the doc's tokens (C3). Messages that name a `T` entry re-add the colon — at the print site in
    main() — so they name what the reader sees in ci.yml and CLAUDE.md; rows naming a
    REQUIRED_REPO_TASKS or T_EXEMPT key stay bare, because that is how they are written there.
    """
    arrays = T_ARRAY_RE.findall(text)
    if len(arrays) != 1:
        raise GateAssertionError(
            f"expected exactly one `T=(...)` line in .github/workflows/ci.yml, found {len(arrays)}. "
            "This gate parses the array with a single-line regex, so it must stay on one line with "
            "nothing after the closing paren (SMA-541 L1)."
        )
    assignments = T_ASSIGN_RE.findall(text)
    if len(assignments) != 1:
        raise GateAssertionError(
            f"found {len(assignments)} `T=`/`T+=` assignment lines in .github/workflows/ci.yml, "
            "expected exactly one. An appended or conditional second array would leave its entries "
            "unexamined by the reverse check while the forward check still passed."
        )
    targets = []
    for token in arrays[0].split():
        if not token.startswith(":"):
            raise GateAssertionError(
                f"`T` entry {token!r} is not a `:name` shorthand target. A project-scoped entry "
                "such as `repo:promtool` would be silently ignored by this gate — the array would "
                "contain something never examined — so it is rejected rather than skipped "
                "(SMA-541 D10). Use the `:name` form, or extend this parser deliberately."
            )
        targets.append(token[1:])
    if not targets:
        raise GateAssertionError(
            "`T=()` is empty — `moon ci` would run nothing at all."
        )
    return targets


def parse_doc_targets(text):
    """CLAUDE.md's documented full-graph command: (bare task names, normalised region text).

    Deliberately ASYMMETRIC with parse_t: a non-`:` token here is ignored, not fatal. The region
    legitimately contains prose punctuation, backticks, `moon`, `ci` and the flag tail, whereas
    every token of `T` is a target and an unrecognised one there means the array holds something
    unexamined (D10).
    """
    begins, ends = text.count(MARKER_BEGIN), text.count(MARKER_END)
    if begins != 1 or ends != 1:
        raise GateAssertionError(
            f"CLAUDE.md must contain exactly one {MARKER_BEGIN} and one {MARKER_END} "
            f"(found {begins} and {ends}). They delimit the documented full-graph command that this "
            "gate compares against ci.yml's `T=(...)` array (SMA-541 D7)."
        )
    start = text.index(MARKER_BEGIN) + len(MARKER_BEGIN)
    end = text.index(MARKER_END)
    if end < start:
        raise GateAssertionError(
            f"CLAUDE.md's markers are inverted — {MARKER_END} appears before {MARKER_BEGIN}."
        )
    region = " ".join(text[start:end].split())
    if not region:
        raise GateAssertionError(
            "CLAUDE.md's ci-targets region is empty — the documented full-graph command is gone."
        )
    targets = []
    for token in region.split():
        token = token.strip("`.,")
        if token.startswith(":"):
            targets.append(token[1:])
    return targets, region


def _eligibility(projects):
    """moon's parsed `{pid: {task: {...}}}` -> `{pid: {task: CI-eligible}}`.

    A PURE function, split out of the subprocess so `--self-test` can drive both of its
    MoonOutputError raises without a subprocess. The rc-2 paths are the ones a fixture table most
    needs: they are what a moon upgrade trips, and an unexercised raise is indistinguishable from
    an absent one — which is the drift class this whole gate exists to close.

    Eligibility polarity is deliberately `is not False`: an absent `runInCI`, or an absent
    `options` object, means ELIGIBLE. Defaulting toward inclusion means a moon output change
    cannot silently exempt a gate — it can only over-require, which is a loud red.
    """
    # Shape-check every level before walking it. Without these, a moon upgrade that turned any of
    # them into a list or a scalar would raise AttributeError out of this function; `main()` does
    # not catch that, so Python would exit 1 — read as an ASSERTION failure when it is in fact
    # "moon told us something we do not understand", which D2 reserves rc 2 for. The whole point of
    # the split is that a shape change is loud and correctly classified (CodeRabbit, SMA-541).
    if not isinstance(projects, dict):
        raise MoonOutputError(
            f"`moon query tasks` reported `tasks` as {type(projects).__name__}, expected an object"
        )
    if not projects:
        raise MoonOutputError("`moon query tasks` reported no projects at all")
    saw_options = False
    result = {}
    for pid, tasks in projects.items():
        # `None` is rejected too, not tolerated as "no tasks": moon emits neither a null nor an
        # empty task group for any of its 28 projects (measured), so a null is malformed output —
        # and letting it through parsed `repo` to an empty row, which the FLOOR then reported as
        # rc 1 (an assertion failure) instead of rc 2 (CodeRabbit round 2).
        if not isinstance(tasks, dict):
            raise MoonOutputError(
                f"`moon query tasks` reported project {pid!r}'s tasks as "
                f"{type(tasks).__name__}, expected an object"
            )
        row = {}
        for name, task in tasks.items():
            if not isinstance(task, dict):
                raise MoonOutputError(
                    f"`moon query tasks` reported task {pid}:{name} as "
                    f"{type(task).__name__}, expected an object"
                )
            options = task.get("options")
            if options is not None and not isinstance(options, dict):
                raise MoonOutputError(
                    f"`moon query tasks` reported {pid}:{name}'s `options` as "
                    f"{type(options).__name__}, expected an object"
                )
            if options is not None:
                saw_options = True
            row[name] = (options or {}).get("runInCI") is not False
        result[pid] = row
    if not saw_options:
        # Not one task carried `options` — moon's shape changed and runInCI can no longer be read.
        # Escalate rather than treat every task as eligible: a silent shape change is how a gate
        # starts asserting something other than what it claims.
        raise MoonOutputError(
            "no task in `moon query tasks` output carries an `options` key — moon's output shape "
            "changed, so `runInCI` can no longer be read (SMA-541 D8)"
        )
    return result


def moon_payload():
    """The raw `tasks` object from ONE `moon query tasks` call.

    The subprocess is isolated here so main() can feed the SAME payload to two independent PURE
    extractors — _eligibility() for CI-eligibility and _scripts() for resolved task scripts —
    without a second `moon` call and without widening _eligibility's return shape (SMA-553 D10).

    Filtered by project id in Python rather than with `--project repo`: moon's query filters are
    regex-based and unanchored, so a future project named e.g. `paigasus-repo-ts` would silently
    join the "repo task set" and false-red C1 (D8).
    """
    out = subprocess.run(
        ["moon", "query", "tasks"], capture_output=True, text=True, check=True
    ).stdout
    payload = json.loads(out)
    # The one shape check the subprocess boundary keeps out of the fixture table: everything below
    # `tasks` is validated inside _eligibility(), which --self-test drives directly. Guarded anyway
    # so a top-level shape change lands on rc 2 like the rest, rather than as a bare AttributeError.
    if not isinstance(payload, dict):
        raise MoonOutputError(
            f"`moon query tasks` returned {type(payload).__name__}, expected a JSON object"
        )
    return payload.get("tasks") or {}


def check_floor(tasks, floor=REQUIRED_REPO_TASKS):
    """Floor members absent from the parsed CI-eligible `repo` set."""
    repo = tasks.get("repo") or {}
    eligible = {name for name, ok in repo.items() if ok}
    return sorted(set(floor) - eligible)


def check_forward(tasks, t_targets, exempt=None):
    """(missing, unexpected, bad_exempt, stale_exempt) — strict equality over `T`'s repo partition.

    `got` deliberately counts every `T` entry that names ANY `repo` task, eligible or not. That is
    what makes flipping a gate to `runInCI: false` while leaving it in `T` show up as `unexpected`
    instead of passing three green checks (D3).

    `stale_exempt` names exemptions that match no `repo` task at all. A TYPO in the table is
    already loud — the real task shows up as `missing` — but an entry left behind after its task
    was renamed or deleted is silent, and stays dead weight forever.
    """
    exempt = T_EXEMPT if exempt is None else exempt
    repo = tasks.get("repo")
    if repo is None:
        raise MoonOutputError("`moon query tasks` reported no `repo` project")
    eligible = {name for name, ok in repo.items() if ok}
    want = eligible - set(exempt)
    got = {name for name in t_targets if name in repo}
    bad_exempt = sorted(name for name, reason in exempt.items() if not (reason or "").strip())
    stale_exempt = sorted(set(exempt) - set(repo))
    return sorted(want - got), sorted(got - want), bad_exempt, stale_exempt


def check_reverse(tasks, t_targets):
    """`T` entries that resolve to no CI-ELIGIBLE task anywhere in the graph.

    Eligibility, not mere existence: plain resolvability would let `:typecheck` pass while every
    task it names had been turned off. `moon ci` exits 0 on an unresolvable target — including in
    the mixed case — so nothing else in CI reports this (D4).
    """
    live = {name for row in tasks.values() for name, ok in row.items() if ok}
    return sorted(name for name in t_targets if name not in live)


def check_docs(t_targets, doc_targets, region):
    """Problems with CLAUDE.md's documented command: ordered mirror of `T`, plus the flag tail."""
    problems = []
    if doc_targets != t_targets:
        for i, (doc, want) in enumerate(zip_longest(doc_targets, t_targets)):
            if doc != want:
                problems.append(
                    f"first divergence at position {i}: CLAUDE.md has "
                    f"{':' + doc if doc else '<end of list>'}, ci.yml's T has "
                    f"{':' + want if want else '<end of list>'}"
                )
                break
        problems.append("CLAUDE.md: " + " ".join(":" + name for name in doc_targets))
        problems.append("ci.yml  T: " + " ".join(":" + name for name in t_targets))
    for flag in REQUIRED_DOC_FLAGS:
        if flag not in region:
            problems.append(f"the documented command is missing `{flag}`")
    return problems


def _strip_comment(line):
    """`line` with any trailing shell comment removed, for the expansion check only.

    A comment on the invocation line otherwise satisfies that check while the command runs
    something else entirely (measured):

        moon ci "${T[@]:0:5}" --base origin/main --include-relations  # restore "${T[@]}" later

    Cuts at the first `#` preceded by whitespace or line start. That is NOT shell-aware — a `#`
    inside a quoted argument truncates early — but the error direction is safe by construction:
    truncating only REMOVES text, so it can turn a clean line into a reported one (a false red the
    author can see and fix), never a violating line into a clean one.
    """
    m = re.search(r"(?:^|[ \t])#", line)
    return line[: m.start()] if m else line


def check_invocation(ci_yml_text):
    """`moon ci` invocations in ci.yml that do not hand it the whole `T` array.

    Pins the invocation's SHAPE, which C1-C3 do not: they assert what is in `T`, not that `T` is
    what runs. Subsetting or rewriting the expansion is not caught by either check.

    Deletion is weaker ground than that suggests. ci.yml carries TWO `moon ci "${T[@]}"`
    invocations (the PR path and the push path — see MOON_CI_LINE_RE above). Deleting BOTH is
    caught by `assert_include_relations` (run.sh): its grep matches nothing and it reds. Deleting
    ONLY ONE is caught by NEITHER check — `assert_include_relations`'s grep still matches the
    surviving line, and this function's own file-wide fallback (`MOON_CI_INVOCATION not in
    ci_yml_text`) is satisfied by that same surviving line. Today a lone deletion still reds, but
    only incidentally: it leaves an empty `then`/`elif` branch in ci.yml's shell block, which bash
    itself rejects — not because either gate caught it.

    EVERY matched line must carry the expansion, not merely one of them: a future second
    `moon ci` reading a different array reds here and the author extends this gate deliberately,
    the same default-deny stance D10 takes on a project-scoped `T` entry.

    What is required is `T_ARRAY_EXPANSION` ANYWHERE on the line, not the contiguous
    `MOON_CI_INVOCATION` form. Argument ORDER is not the property worth pinning — handing over the
    whole array is — and requiring contiguity reds a perfectly correct
    `moon ci --base origin/main "${T[@]}"`. Both fixtures are kept below so the distinction cannot
    quietly regress in either direction.
    """
    lines = MOON_CI_LINE_RE.findall(ci_yml_text)
    rows = [line.strip() for line in lines if T_ARRAY_EXPANSION not in _strip_comment(line)]
    if len(lines) != EXPECTED_MOON_CI_INVOCATIONS:
        # The count floor. `rows` is DERIVED from the matched lines, so it is silent about a line
        # the regex never matched — which is exactly how `echo moon ci …` slipped through before.
        rows.append(
            f"(found {len(lines)} executable `moon ci` invocation(s), expected "
            f"{EXPECTED_MOON_CI_INVOCATIONS}: either a branch was deleted, or one is written in a "
            f"form this gate does not recognise as executable — it must be `moon ci` at the start "
            f"of the line. A deliberate new invocation means updating "
            f"EXPECTED_MOON_CI_INVOCATIONS.)"
        )
    return rows


def _scripts(projects):
    """`{task: resolved script}` for the `repo` project. PURE.

    A SEPARATE extractor rather than a wider _eligibility return: that function's
    `{pid: {task: bool}}` shape is pinned by eight self-test fixtures, including an exact-equality
    polarity check, and reshaping it to carry `script` would break all of them for no gain.

    Non-dict `repo`/task shapes are tolerated here rather than escalated to rc 2, because
    _eligibility() walks the very same payload first and has already raised MoonOutputError on
    those. A malformed `script` FIELD is not something _eligibility looks at, so it is checked
    here: check_self_invocation calls `.splitlines()` on the returned value, and a non-string
    `script` would otherwise surface as a bare AttributeError -> rc 1, misreporting a moon output
    shape change as an authorial mistake (SMA-553 review finding 3).
    """
    repo = projects.get("repo") or {}
    if not isinstance(repo, dict):
        return {}
    scripts = {}
    for name, task in repo.items():
        if not isinstance(task, dict):
            continue
        script = task.get("script")
        if script is not None and not isinstance(script, str):
            raise MoonOutputError(
                f"`moon query tasks` reported repo:{name}'s `script` as "
                f"{type(script).__name__}, expected a string"
            )
        scripts[name] = script or ""
    return scripts


def check_self_invocation(run_sh_text, scripts, actionlint_sh_text, release_parity_sh_text):
    """Call sites of the affected-graph, actionlint and release-parity gates missing from where
    they must appear.

    Four haystacks, matched TWO different ways. run.sh sites are substrings, because they are
    indented and one is a mid-line fragment, and their `|| RC=1` suffixes already make them
    unambiguous. Task-script, actionlint and release-parity sites are whole stripped LINES —
    membership is checked against the set of a line's OWN full stripped text, not "does this
    substring appear anywhere in the file" — but for two DIFFERENT reasons, not one shared
    rationale. For task-script and actionlint, a required token is a strict PREFIX of something
    else in the file — `task_inputs.py` of `task_inputs.py --self-test`, and `run_self_tests` of
    `run_self_tests() {` — so a substring-over-the-whole-text match would be satisfied by the
    wrong occurrence. Release-parity has no such prefix hazard; there, whole-line matching is
    what makes a COMMENTED-OUT copy of a pinned line (e.g. `# if [ "$NEGATIVE" = 1 ]; then`)
    report missing rather than silently satisfy the pin — a substring-over-the-whole-text version
    would still find the required text inside the commented line and accept it, since commenting
    a line out does not remove its text, only prefix it.

    The four texts are checked SEPARATELY rather than against one concatenated haystack, so a call
    site living in the wrong file cannot satisfy another's requirement.

    `actionlint_sh_text` and `release_parity_sh_text` are REQUIRED positional parameters,
    deliberately. An optional one defaulting to "" would make every existing caller pass
    vacuously — re-creating the class of hole this check exists to close.
    """
    missing = [site for site in RUN_SH_CALL_SITES if site not in run_sh_text]
    for task, required in sorted(SELF_SCHEDULED_GATES.items()):
        present = {line.strip() for line in scripts.get(task, "").splitlines()}
        missing.extend(f"{task} script: {site}" for site in required if site not in present)
    # COLUMN 0 only (rstrip, no lstrip) — see the comment at ACTIONLINT_SH_CALL_SITES above for why
    # this one haystack, alone of the four, requires the line to carry NO leading whitespace: an
    # indented copy (e.g. wrapped in `if false; then … fi`) must not satisfy the pin.
    actionlint_lines = {
        line.rstrip() for line in actionlint_sh_text.splitlines() if line == line.lstrip()
    }
    missing.extend(
        f"ci/actionlint/run.sh: {site}"
        for site in ACTIONLINT_SH_CALL_SITES
        if site not in actionlint_lines
    )
    # Stripped whole lines, like the task-script haystack and unlike the column-0 actionlint
    # one: these three sit inside run.sh at varying indentation (the `case` arms are indented
    # four spaces), so a column-0 rule would reject the real, executing lines.
    release_parity_lines = {line.strip() for line in release_parity_sh_text.splitlines()}
    missing.extend(
        f"ci/release-parity/run.sh: {site}"
        for site in RELEASE_PARITY_SH_CALL_SITES
        if site not in release_parity_lines
    )
    return missing


def check_gate_inputs(projects, expected_table=SELF_TASK_EXPECTED_GLOBS):
    """SMA-553 D13, mirrored. Rows for a self-scheduled gate whose own inputs have drifted.

    `expected_table` defaults to the real registry and production never passes it — it exists so
    self_test() can drive a gate declaring BOTH globs and literal files, which no registered gate
    does today (repo:input-liveness is glob-only, repo:version-lockstep file-only) and which is
    therefore the one property of the comparison the live table cannot exercise. The default is
    asserted to still BE that registry, so this parameter cannot quietly point production at a
    stub the way an `actionlint_sh_text=""` default would have (SMA-576).
    """
    # Unlike _scripts(), no isinstance guard: main() runs _eligibility(raw_tasks) on this same
    # payload first, which raises MoonOutputError on any non-dict project value before this is
    # reached. Calling this standalone on malformed input will AttributeError.
    repo = projects.get("repo") or {}
    rows = []
    for task, expected in sorted(expected_table.items()):
        entry = repo.get(task)
        if not isinstance(entry, dict):
            rows.append(f"repo:{task} is absent from the graph, so its inputs cannot be checked")
            continue
        # A present-but-wrong-typed inputGlobs/inputFiles is a moon output shape change, not an
        # authored drift the rows below know how to describe — escalate it loudly to rc 2 instead
        # of letting `sorted(...)` either misread it or raise a bare, misclassified exception
        # (SMA-553 review finding 3). An ABSENT key is fine — it means no globs/files were declared.
        globs_raw, files_raw = entry.get("inputGlobs"), entry.get("inputFiles")
        if globs_raw is not None and not isinstance(globs_raw, dict):
            raise MoonOutputError(
                f"`moon query tasks` reported repo:{task}'s `inputGlobs` as "
                f"{type(globs_raw).__name__}, expected an object"
            )
        if files_raw is not None and not isinstance(files_raw, dict):
            raise MoonOutputError(
                f"`moon query tasks` reported repo:{task}'s `inputFiles` as "
                f"{type(files_raw).__name__}, expected an object"
            )
        # moon injects the workspace-config glob into EVERY task, so it is not authored drift.
        # Hardcoded rather than imported from task_inputs.INJECTED_GLOB, which is the source of
        # truth: these two gates stay independently runnable. Divergence fails SAFE — task_inputs'
        # D4 composition guard raises rc 2 with the accurate message, and the worst this
        # copy can do is red with a misleading "authored inputs changed".
        got = tuple(g for g in sorted(globs_raw or {})
                    if g != ".moon/*.{yml,yaml,jsonc,json,pkl,hcl,toml}")
        # moon resolves a LITERAL path in `inputs:` into inputFiles, not inputGlobs, so the glob
        # tuple alone can still read as wired while the input set has in fact changed. Both buckets
        # are therefore compared, as ONE sequence in a fixed order — globs then files (SMA-576).
        #
        # This used to be `got != expected or files`, i.e. "the globs must match AND there must be
        # no file inputs at all", which was adequate while repo:input-liveness was the only entry
        # in the table and its whole declaration was a single glob. It cannot express a gate whose
        # authored inputs are literal paths: repo:version-lockstep declares sixteen of them and no
        # glob, so that form would have reported it as drifted on every run. Comparing the combined
        # sequence keeps every assertion the old form made — for a glob-only gate `files` is empty,
        # so a stray file input still lands in the comparison and still reds — and adds the two the
        # old form could not make: a dropped and an added file input.
        files = tuple(sorted(files_raw or {}))
        if tuple(got) + files != tuple(expected):
            rows.append(
                f"repo:{task}'s authored inputs are {list(got) + list(files)}, "
                f"expected exactly {list(expected)} — changing them makes that gate stop "
                "noticing the drift it exists to catch (SMA-553 D13, SMA-576)"
            )
    return rows


def check_registry_pairing(scheduled=None, globs=None, exempt=None):
    """SMA-530. The three self-scheduled-gate registries must stay consistent.

    Returns (unpinned, bad_exempt, stale_exempt, both, orphan_globs), all sorted name lists.

    Replaces a bare `set(A) != set(B)` equality. Equality forced every script-pinned gate to
    duplicate its input globs here; a plain subset would have let repo:affected-smoke be
    script-pinned later WITHOUT pinning the inputs that make every pin in this file
    reachable. An exemption with a recorded reason keeps the decision explicit and visible.
    """
    scheduled = SELF_SCHEDULED_GATES if scheduled is None else scheduled
    globs = SELF_TASK_EXPECTED_GLOBS if globs is None else globs
    exempt = SELF_TASK_GLOBS_EXEMPT if exempt is None else exempt
    return (
        sorted(t for t in scheduled if t not in globs and t not in exempt),
        sorted(t for t, reason in exempt.items() if not (reason or "").strip()),
        sorted(set(exempt) - set(scheduled)),
        sorted(set(globs) & set(exempt)),
        sorted(set(globs) - set(scheduled)),
    )


def self_test():
    """Negative control: every assertion must FIRE on a synthetic violation.

    Drives the PARSERS as well as the checks. The parsers are the component this gate cannot
    self-detect a fault in — a total match failure hits the rc-1 path, but a PARTIAL mis-parse is
    silent — and hand-rolled text extraction "is exactly the kind of thing that silently does the
    wrong thing" (ci/actionlint/run.sh:284, which backs that claim with ~35 extractor fixtures).
    """
    failures = []

    def expect_targets(label, text, want):
        try:
            got = parse_t(text)
        except GateAssertionError as exc:
            failures.append(f"parse_t[{label}]: unexpected red: {exc}")
            return
        if got != want:
            failures.append(f"parse_t[{label}]: got {got}, want {want}")

    def expect_red(label, text):
        try:
            parse_t(text)
        except GateAssertionError:
            return
        failures.append(f"parse_t[{label}]: accepted input that should have been rejected")

    expect_targets("canonical", "          T=(:build :test :deny)\n", ["build", "test", "deny"])
    expect_targets(
        "indented-in-yaml",
        "jobs:\n  ci:\n    run: |\n      T=(:a :b)\n      moon ci \"${T[@]}\"\n",
        ["a", "b"],
    )
    expect_targets("hash-comment-is-not-an-assignment", "# T=(:ghost)\nT=(:real)\n", ["real"])
    expect_red("no-array", "moon ci --base origin/main\n")
    expect_red("two-arrays", "T=(:a)\nT=(:b)\n")
    expect_red("append", "T=(:a)\nT+=(:b)\n")
    expect_red("empty-array", "T=()\n")
    expect_red("trailing-comment", "T=(:a :b)  # note\n")
    expect_red("project-scoped-entry", "T=(:a repo:promtool)\n")
    expect_red("bare-token", "T=(:a build)\n")

    def expect_doc(label, text, want_targets, want_region_contains=()):
        try:
            got, region = parse_doc_targets(text)
        except GateAssertionError as exc:
            failures.append(f"parse_doc_targets[{label}]: unexpected red: {exc}")
            return
        if got != want_targets:
            failures.append(f"parse_doc_targets[{label}]: got {got}, want {want_targets}")
        for needle in want_region_contains:
            if needle not in region:
                failures.append(f"parse_doc_targets[{label}]: region lost {needle!r}")

    def expect_doc_red(label, text):
        try:
            parse_doc_targets(text)
        except GateAssertionError:
            return
        failures.append(f"parse_doc_targets[{label}]: accepted input that should have been rejected")

    wrapped = (
        "intro (e.g. `:deny`, `:osv`) prose\n"
        f"  {MARKER_BEGIN}\n"
        "  `moon ci :build :test\n"
        "  :deny :promtool\n"
        "  --base origin/main --include-relations`\n"
        f"  {MARKER_END}\n"
        "trailing prose with `moon ci :other --include-relations`\n"
    )
    expect_doc(
        "wrapped-span",
        wrapped,
        ["build", "test", "deny", "promtool"],
        ("--base origin/main", "--include-relations"),
    )
    expect_doc_red("no-markers", "`moon ci :build --include-relations`\n")
    expect_doc_red("only-begin", f"{MARKER_BEGIN}\n`moon ci :build`\n")
    expect_doc_red("duplicate-begin", f"{MARKER_BEGIN}\n{MARKER_BEGIN}\nx\n{MARKER_END}\n")
    expect_doc_red("inverted", f"{MARKER_END}\n`moon ci :build`\n{MARKER_BEGIN}\n")
    expect_doc_red("empty-region", f"{MARKER_BEGIN}\n\n{MARKER_END}\n")

    def expect_infra(label, call):
        try:
            call()
        except MoonOutputError:
            return
        except Exception as exc:  # any other exception type is itself the failure
            failures.append(
                f"{label}: raised {type(exc).__name__} instead of MoonOutputError: {exc}"
            )
            return
        failures.append(f"{label}: accepted an output shape that must abort as infrastructure")

    # The rc-2 raises, driven directly. moon_payload() holds the subprocess and _eligibility()
    # stays pure precisely so these are reachable from a fixture: an unexercised raise is
    # indistinguishable from an absent one, which is the drift class this gate exists to close.
    expect_infra("_eligibility[no-projects]", lambda: _eligibility({}))
    expect_infra("_eligibility[no-options-anywhere]", lambda: _eligibility({"repo": {"deny": {}}}))
    # ...and every level's SHAPE. Each of these would otherwise raise AttributeError out of
    # _eligibility, which main() does not catch — Python would exit 1, misreporting "moon's output
    # changed" as an assertion failure (CodeRabbit, SMA-541).
    expect_infra("_eligibility[projects-not-a-map]", lambda: _eligibility(["repo"]))
    expect_infra("_eligibility[task-group-not-a-map]", lambda: _eligibility({"repo": ["deny"]}))
    # A NULL task group specifically: it used to parse to an empty row, so the floor reported rc 1
    # rather than this raising rc 2. The sibling project carries `options` so `saw_options` cannot
    # be what fires here.
    expect_infra(
        "_eligibility[task-group-null]",
        lambda: _eligibility({"repo": None, "other": {"build": {"options": {"runInCI": True}}}}),
    )
    expect_infra("_eligibility[task-not-a-map]", lambda: _eligibility({"repo": {"deny": "yes"}}))
    expect_infra(
        "_eligibility[options-not-a-map]",
        lambda: _eligibility({"repo": {"deny": {"options": "runInCI=true"}}}),
    )

    # ...and the POLARITY itself, pinned in both directions so the default-toward-inclusion rule
    # (D8) is asserted rather than assumed: only an explicit `runInCI: false` is ineligible.
    polarity = _eligibility(
        {
            "repo": {
                "install-hooks": {"options": {"runInCI": False}},
                "deny": {"options": {}},
                "promtool": {},
            }
        }
    )
    want_polarity = {"repo": {"install-hooks": False, "deny": True, "promtool": True}}
    if polarity != want_polarity:
        failures.append(f"_eligibility[polarity]: got {polarity}, want {want_polarity}")

    # project id -> task name -> CI-eligible. Mirrors _eligibility()'s return shape.
    tasks_fixture = {
        "repo": {"deny": True, "promtool": True, "affected-smoke": True,
                 "publish-metadata": True, "install-hooks": False,
                 # SMA-530 — floor members, so they must be CI-eligible here too.
                 "release-parity": True, "release-parity-py": True,
                 "release-parity-ts": True},
        "some-crate-rs": {"build": True, "test": True, "build-release": True},
    }
    aligned_t = ["build", "test", "deny", "promtool", "affected-smoke", "publish-metadata",
                 "release-parity", "release-parity-py", "release-parity-ts"]

    def forward(label, tasks, t, exempt, want_missing, want_unexpected, want_bad_exempt=(),
                want_stale_exempt=()):
        got = check_forward(tasks, t, exempt)
        want = (list(want_missing), list(want_unexpected), list(want_bad_exempt),
                list(want_stale_exempt))
        if got != want:
            failures.append(f"check_forward[{label}]: got {list(got)}, want {list(want)}")

    forward("aligned", tasks_fixture, aligned_t, {}, [], [])
    # AC #3: a runInCI:false task absent from T must not trip the gate — asserted with SEVERAL of
    # them, so the exclusion is a rule and not an accident of install-hooks happening to be alone.
    # (A fixture identical to "aligned" would restate that case without testing anything new.)
    two_disabled = {**tasks_fixture,
                    "repo": {**tasks_fixture["repo"], "install-hooks": False, "second-hook": False}}
    forward("runInCI-false-absent", two_disabled, aligned_t, {}, [], [])
    # A new repo gate that nobody added to T.
    forward("missing-gate", {**tasks_fixture, "repo": {**tasks_fixture["repo"], "new-gate": True}},
            aligned_t, {}, ["new-gate"], [])
    # THE BLOCKER: a gate flipped to runInCI:false but LEFT in T. A subset test passes this.
    forward("disabled-but-still-in-T",
            {**tasks_fixture, "repo": {**tasks_fixture["repo"], "promtool": False}},
            aligned_t, {}, [], ["promtool"])
    # A task in T_EXEMPT with a reason may be absent from T...
    forward("exempt-absent", tasks_fixture,
            [t for t in aligned_t if t != "promtool"], {"promtool": "runs in its own step"}, [], [])
    # ...but present-AND-exempt is contradictory and must be reported.
    forward("exempt-but-present", tasks_fixture, aligned_t,
            {"promtool": "runs in its own step"}, [], ["promtool"])
    # A bare-membership exemption with no reason is unreviewable — reject it.
    forward("exempt-without-reason", tasks_fixture,
            [t for t in aligned_t if t != "promtool"], {"promtool": "  "}, [], [], ["promtool"])
    # An exemption naming no `repo` task — the entry its task outlived. Reported on its own row: a
    # typo would ALSO show the real task as `missing`, but a leftover after a deletion shows
    # nothing at all.
    forward("exempt-names-no-task", tasks_fixture, aligned_t,
            {"ghost-gate": "kept after the task was deleted"}, [], [], [], ["ghost-gate"])
    # An output with no `repo` project at all is moon telling us nothing -> infra, never a
    # comparison against an empty set.
    expect_infra("check_forward[no-repo-project]",
                 lambda: check_forward({"other-project": {"build": True}}, aligned_t, {}))

    if check_floor(tasks_fixture) != []:
        failures.append("check_floor: fired on a fixture containing every floor member")
    thin = {"repo": {"deny": True}}
    if check_floor(thin) != sorted(REQUIRED_REPO_TASKS):
        failures.append(f"check_floor: did not name every absent floor member: {check_floor(thin)}")

    def reverse(label, tasks, t, want):
        got = check_reverse(tasks, t)
        if got != list(want):
            failures.append(f"check_reverse[{label}]: got {got}, want {list(want)}")

    # A generic target owned by another project resolves — it must NOT be reported.
    reverse("generic-resolves", tasks_fixture, aligned_t, [])
    reverse("dead-entry", tasks_fixture, [*aligned_t, "ghost"], ["ghost"])
    # A name whose every task is runInCI:false is present but would run NOTHING (D4).
    reverse("resolves-only-to-disabled", tasks_fixture, [*aligned_t, "install-hooks"],
            ["install-hooks"])

    def docs(label, t, doc, region, want_empty):
        got = check_docs(t, doc, region)
        if bool(got) == want_empty:
            failures.append(f"check_docs[{label}]: got {got}, want_empty={want_empty}")

    full_flags = "moon ci --base origin/main --include-relations"
    docs("aligned", aligned_t, list(aligned_t), full_flags, True)
    docs("doc-missing-target", aligned_t, aligned_t[:-1], full_flags, False)
    docs("doc-extra-target", aligned_t, [*aligned_t, "extra"], full_flags, False)
    docs("doc-reordered", aligned_t, list(reversed(aligned_t)), full_flags, False)
    docs("doc-missing-include-relations", aligned_t, list(aligned_t),
         "moon ci --base origin/main", False)
    docs("doc-missing-base", aligned_t, list(aligned_t), "moon ci --include-relations", False)

    # The invocation shape. The subsetted variant is the one that keeps EVERY other check green
    # while stopping eighteen gates from running. Mirrors ci.yml's real two-branch shape, because
    # "one of the two lines was rewritten" is the case a whole-file substring test would miss.
    invoked = (
        "      - name: moon ci (affected graph)\n"
        "        run: |\n"
        '          if [ "$EVENT" = "pull_request" ]; then\n'
        '            moon ci "${T[@]}" --base origin/main --include-relations\n'
        "          else\n"
        '            moon ci "${T[@]}" --base "$BEFORE" --include-relations\n'
        "          fi\n"
    )
    if check_invocation(invoked):
        failures.append(
            f"check_invocation: fired on the canonical call: {check_invocation(invoked)}"
        )
    subsetted = invoked.replace('"${T[@]}" --base origin/main', '"${T[@]:0:5}" --base origin/main')
    if not check_invocation(subsetted):
        failures.append("check_invocation: missed a subsetted `${T[@]:0:5}` on ONE of two lines")
    if not check_invocation(invoked.replace('moon ci "${T[@]}"', "moon ci $T")):
        failures.append("check_invocation: missed an unquoted `$T` expansion")
    if not check_invocation(invoked.replace('moon ci "${T[@]}"', "moon ci :build")):
        failures.append("check_invocation: missed a `moon ci` that bypasses `T` entirely")
    # Flag FIRST, array subsetted after it. The old `moon ci +"` shape — shared with
    # assert_include_relations — matched neither this line nor anything else, so the file-wide
    # fallback was satisfied by the sibling invocation and the whole thing passed (CodeRabbit CLI).
    reordered = invoked.replace(
        'moon ci "${T[@]}" --base origin/main',
        'moon ci --base origin/main "${T[@]:0:5}"',
    )
    if not check_invocation(reordered):
        failures.append("check_invocation: missed a subsetted array behind a leading flag")
    # NOT executed: the line carries the expansion and reads canonical, but nothing runs. Caught by
    # the count floor, not by the per-line rule — the regex never matches it (CodeRabbit round 2).
    if not check_invocation(
        invoked.replace('moon ci "${T[@]}" --base origin/main', 'echo moon ci "${T[@]}" --base origin/main')
    ):
        failures.append("check_invocation: missed an `echo`-prefixed, non-executing invocation")
    # Multiple spaces between the two words: a literal single space missed this entirely, so the
    # subsetted array was never examined.
    if not check_invocation(
        invoked.replace('moon ci "${T[@]}" --base origin/main', 'moon    ci "${T[@]:0:5}" --base origin/main')
    ):
        failures.append("check_invocation: missed a subsetted array behind multiple spaces")
    # ...and multiple spaces with the array INTACT must stay clean, so the whitespace tolerance is
    # a real tolerance rather than a blanket rejection of the spacing.
    if check_invocation(
        invoked.replace('moon ci "${T[@]}" --base origin/main', 'moon    ci "${T[@]}" --base origin/main')
    ):
        failures.append("check_invocation: fired on multiple spaces with the array intact")
    # A trailing comment carrying the expansion must not satisfy the per-line check while the
    # command itself runs a subset (CodeRabbit round 4).
    commented_subset = invoked.replace(
        'moon ci "${T[@]}" --base origin/main --include-relations',
        'moon ci "${T[@]:0:5}" --base origin/main --include-relations  # restore "${T[@]}" later',
    )
    if not check_invocation(commented_subset):
        failures.append("check_invocation: let a trailing comment satisfy the expansion check")
    # ...and a trailing comment on an otherwise CORRECT line must stay clean, so comment-stripping
    # is not simply rejecting every commented invocation.
    commented_ok = invoked.replace(
        'moon ci "${T[@]}" --base origin/main --include-relations',
        'moon ci "${T[@]}" --base origin/main --include-relations  # PR path',
    )
    if check_invocation(commented_ok):
        failures.append("check_invocation: fired on a correct invocation carrying a comment")
    # ...and the same shape with the array INTACT must stay clean, so the fix above did not simply
    # start rejecting every line that fails to put the array first.
    if check_invocation(
        invoked.replace('moon ci "${T[@]}" --base origin/main', 'moon ci --base origin/main "${T[@]}"')
    ):
        failures.append("check_invocation: fired on a reordered but CANONICAL invocation")
    # A prose comment and the step `name:` field both mention `moon ci` in the real ci.yml and must
    # not be mistaken for invocations — that exclusion is all the old quote-gate actually bought.
    if check_invocation(
        invoked + "          # `moon ci` is affected-only, so a PR touching no Rust never rebuilds\n"
        "      - name: moon ci (affected graph)\n"
    ):
        failures.append("check_invocation: fired on a comment or a `name:` field")

    # A DELETED input file is an authorial mistake (rc 1), not a broken tool (rc 2). Driven with
    # stubs rather than real paths so the control needs no filesystem state at all.
    class _Raises:
        def __init__(self, exc):
            self.exc = exc

        def read_text(self):
            raise self.exc

    class _Present:
        def read_text(self):
            return "content"

    try:
        read_input(_Raises(FileNotFoundError(2, "No such file or directory")), "CLAUDE.md")
    except GateAssertionError:
        pass
    except OSError as exc:
        failures.append(f"read_input: a missing input stayed on the rc-2 infra path: {exc}")
    else:
        failures.append("read_input: accepted a missing input")
    try:
        read_input(_Raises(PermissionError(13, "Permission denied")), "CLAUDE.md")
    except GateAssertionError:
        failures.append("read_input: routed a PermissionError to rc 1; only a missing file is rc 1")
    except PermissionError:
        pass
    else:
        failures.append("read_input: swallowed a PermissionError instead of propagating it")
    if read_input(_Present(), "CLAUDE.md") != "content":
        failures.append("read_input: did not return the file's text")

    wired = (
        # Load-bearing: with the function DEFINITION present, `no_call` below still contains
        # the bare name `assert_ci_targets`, so a name-only RUN_SH_CALL_SITES entry would
        # survive deleting the call. Dropping this line silently de-fangs that assertion.
        'assert_ci_targets() {\n  :\n}\n'
        '  assert_ci_targets || SUITE_RC=1\n'
        '  python3 "$HERE/ci_targets.py" --self-test || NEG_RC=1\n'
    )
    def wired_scripts(**overrides):
        """A fully-wired `scripts` dict for EVERY SELF_SCHEDULED_GATES key.

        Every negative fixture below asserts `if not check_self_invocation(...)`, which is
        satisfied by ANY missing entry. A literal one-key dict therefore starts passing for
        the WRONG reason the moment a second gate is registered — the exact vacuity this
        gate exists to prevent, and measured on SMA-530: adding three keys turned ~24 of
        these assertions into no-ops while only the positive control red. Building from the
        registry itself means a future gate cannot reopen it; each fixture then mutates
        exactly ONE gate and leaves the rest wired.
        """
        built = {
            task: "".join(f"{line}\n" for line in lines)
            for task, lines in SELF_SCHEDULED_GATES.items()
        }
        built.update(overrides)
        return built

    def broken_script(task, drop):
        """`task`'s wired script with exactly one required line removed."""
        return "".join(
            f"{line}\n" for line in SELF_SCHEDULED_GATES[task] if line != drop
        )

    wired_script = (
        "set -euo pipefail\n"
        "python3 ci/affected-graph/task_inputs.py --self-test\n"
        "python3 ci/affected-graph/task_inputs.py\n"
    )
    scripts = wired_scripts()
    # The builder must not silently under-cover: a typo'd comprehension that dropped a gate
    # would restore the very vacuity it exists to close, and every fixture below would go
    # green together.
    if set(scripts) != set(SELF_SCHEDULED_GATES):
        failures.append(
            f"wired_scripts: covers {sorted(scripts)}, registry has "
            f"{sorted(SELF_SCHEDULED_GATES)}"
        )
    # ...and the table must still spell repo:input-liveness's script out line for line
    # (SMA-576). `wired_script` is an INDEPENDENT literal, not a slice of the registry, so a
    # typo introduced into SELF_SCHEDULED_GATES reds here rather than silently propagating
    # into every fixture built from it. It is also the haystack the two "swapped texts"
    # fixtures below need in its own right.
    if scripts.get("input-liveness") != wired_script:
        failures.append(
            "check_self_invocation fixture: SELF_SCHEDULED_GATES no longer spells out "
            "repo:input-liveness's wired script line for line"
        )
    wired_actionlint = (
        # Load-bearing, exactly as `assert_ci_targets() {` is above: with the DEFINITION present,
        # `no_actionlint_call` below still contains the bare name `run_self_tests`, so a
        # name-only entry would survive deleting the call. Whole-line matching is what separates
        # them, and dropping this line silently de-fangs that assertion.
        "run_self_tests() {\n  :\n}\n"
        "run_self_tests\n"
        "selftest_mutation_battery\n"
        # Check 8's production call site (SMA-542 fix-wave I1). Also contains `ci_target_floor_verdict`
        # as a substring — the self-test fixtures call it too, as `ci_target_floor_verdict "$tmp"`
        # and `ci_target_floor_verdict /nonexistent/ci.yml` — so this MUST be whole-line matched,
        # exactly like the two entries above.
        "done < <(ci_target_floor_verdict .github/workflows/ci.yml)\n"
        # Check 8b's production call site (SMA-542 CodeRabbit round 4, finding C1) — same shape,
        # same reason: `invocation_allowlist_verdict` is also called from inside its own self-test
        # fixtures, so this MUST be whole-line matched too.
        'done < <(invocation_allowlist_verdict .github/workflows/ci.yml "$REPORTED_LINENOS")\n'
        # Check 8c's production call site (SMA-542 residual closure, PR 150 follow-up) — same
        # shape again: `affected_graph_wiring_verdict` is also called from inside its own
        # self-test fixture, so this MUST be whole-line matched too.
        'done < <(affected_graph_wiring_verdict ci/affected-graph/run.sh)\n'
        # Check 8d's production call site (SMA-542 residual closure, PR 150 follow-up — closes
        # README L12) — same shape again: `block_execution_verdict` is also called from inside its
        # own self-test fixtures, so this MUST be whole-line matched too.
        'done < <(block_execution_verdict .github/workflows/ci.yml)\n'
    )
    wired_release_parity = (
        '    --negative-control) NEGATIVE=1; shift ;;\n'
        'if [ "$NEGATIVE" = 1 ]; then\n'
        '  echo "== negative control ... =="\n'
        '  ec=0; check_case "neg-fix-bang" "fix!: deliberately wrong" "-" "0.1.1" || ec=$?\n'
        '  case "$ec" in\n'
        '    1) echo "negative-control OK: harness reported red as expected"; exit 0 ;;\n'
        '    0) echo "negative-control FAILED: harness accepted a wrong expectation" >&2; exit 1 ;;\n'
        '  esac\n'
        'fi\n'
    )
    if check_self_invocation(wired, scripts, wired_actionlint, wired_release_parity):
        failures.append(
            "check_self_invocation: fired on a wired tree: "
            f"{check_self_invocation(wired, scripts, wired_actionlint, wired_release_parity)}"
        )
    no_call = wired.replace("  assert_ci_targets || SUITE_RC=1\n", "")
    if not check_self_invocation(no_call, scripts, wired_actionlint, wired_release_parity):
        failures.append("check_self_invocation: missed a deleted run_suite call")
    no_selftest = wired.replace('  python3 "$HERE/ci_targets.py" --self-test || NEG_RC=1\n', "")
    if not check_self_invocation(no_selftest, scripts, wired_actionlint, wired_release_parity):
        failures.append("check_self_invocation: missed a deleted --self-test call")
    silenced = wired.replace("--self-test || NEG_RC=1", "--self-test || true")
    if not check_self_invocation(silenced, scripts, wired_actionlint, wired_release_parity):
        failures.append("check_self_invocation: missed a --self-test whose failure is swallowed")
    # SMA-553 D10 + review finding 1, generalised (SMA-530). These three named fixtures used
    # to be spelled out for input-liveness only: the deleted REAL RUN (a strict PREFIX of the
    # --self-test line, so a substring test would report the script fully wired while the gate
    # no longer ran at all), the deleted --self-test, and the deleted `set -euo pipefail`
    # (Moon's script: blocks have no errexit, so deleting it leaves both invocations' TEXT
    # untouched while a failing self-test is silently swallowed — SMA-526). Driving the loop
    # from the registry keeps all three properties asserted for EVERY gate, including ones
    # added later, and covers the same prefix hazard in release-parity*, where the real-run
    # line is likewise a strict prefix of the control line.
    for _task, _lines in sorted(SELF_SCHEDULED_GATES.items()):
        for _line in _lines:
            if not check_self_invocation(
                wired, wired_scripts(**{_task: broken_script(_task, _line)}), wired_actionlint,
                wired_release_parity,
            ):
                failures.append(
                    f"check_self_invocation: missed {_line!r} deleted from repo:{_task}'s script"
                )
    if not check_self_invocation(
        wired, wired_scripts(**{"input-liveness": ""}), wired_actionlint, wired_release_parity
    ):
        failures.append("check_self_invocation: missed an absent input-liveness script entirely")
    # SMA-576, generalised. A registered gate whose script is missing from the payload
    # ALTOGETHER — not merely short a line — must red just as loudly: that is what a gate
    # silently dropped from moon.yml looks like from here. Driven from the registry for the
    # same reason the line-deletion loop above is: the input-liveness-only spelling this
    # replaces went vacuous the moment a second gate was registered.
    for _task in sorted(SELF_SCHEDULED_GATES):
        if not check_self_invocation(
            wired, {k: v for k, v in wired_scripts().items() if k != _task}, wired_actionlint,
            wired_release_parity,
        ):
            failures.append(
                f"check_self_invocation: missed an absent repo:{_task} script entirely"
            )
    # The two texts are checked SEPARATELY: a call site in the wrong file must not satisfy the
    # other's requirement, which a concatenated haystack would allow.
    if not check_self_invocation(
        wired_script, wired_scripts(**{"input-liveness": wired}), wired_actionlint,
        wired_release_parity,
    ):
        failures.append("check_self_invocation: accepted the two texts swapped")
    # ...and the reverse direction, which the swap fixture above does not reach: script text must
    # not satisfy a run.sh requirement either.
    if not check_self_invocation(
        no_call, wired_scripts(**{"input-liveness": wired + wired_script}), wired_actionlint,
        wired_release_parity,
    ):
        failures.append("check_self_invocation: a run.sh call site was satisfied by script text")
    # The "fired on a wired tree" positive control above already covers all four haystacks
    # simultaneously wired, including wired_actionlint — a second, argument-identical repeat here
    # would only ever fire alongside that one and add no coverage (SMA-542 review, smaller
    # correction 2).
    no_actionlint_call = wired_actionlint.replace("\nrun_self_tests\n", "\n")
    if not check_self_invocation(wired, scripts, no_actionlint_call, wired_release_parity):
        failures.append("check_self_invocation: missed a deleted run_self_tests call")
    no_battery = wired_actionlint.replace("selftest_mutation_battery\n", "")
    if not check_self_invocation(wired, scripts, no_battery, wired_release_parity):
        failures.append("check_self_invocation: missed a deleted mutation-battery call")
    # SMA-542 fix-wave I1 — the reviewer deleted this exact block from run.sh and measured: full
    # gate rc 0, this gate PASS, with check 8's T floor/swallowed/continue-on-error verdicts
    # asserting nothing until the third ACTIONLINT_SH_CALL_SITES entry closed it.
    no_floor_call = wired_actionlint.replace(
        "done < <(ci_target_floor_verdict .github/workflows/ci.yml)\n", ""
    )
    if not check_self_invocation(wired, scripts, no_floor_call, wired_release_parity):
        failures.append(
            "check_self_invocation: missed a deleted check-8 production call site (fix-wave I1)"
        )
    # CodeRabbit round 4, finding C1 — the SAME defect as fix-wave I1 above, reopened one round
    # later against check 8b: deleting its production call site left the real run.sh at rc 0 and
    # this gate PASSing, because invocation_allowlist_self_test still calls the FUNCTION. Only the
    # production `done < <(...)` line proves it is also applied to the real ci.yml.
    no_check8b_call = wired_actionlint.replace(
        'done < <(invocation_allowlist_verdict .github/workflows/ci.yml "$REPORTED_LINENOS")\n', ""
    )
    if not check_self_invocation(wired, scripts, no_check8b_call, wired_release_parity):
        failures.append(
            "check_self_invocation: missed a deleted check-8b production call site "
            "(CodeRabbit round 4, finding C1)"
        )
    # SMA-542 residual closure (PR 150 follow-up) — the SAME defect one level further out, against
    # check 8c: deleting ITS production call site must be caught too, or check 8c would be the sole
    # judge of its own wiring, exactly the problem it exists to close for RUN_SH_CALL_SITES.
    no_check8c_call = wired_actionlint.replace(
        "done < <(affected_graph_wiring_verdict ci/affected-graph/run.sh)\n", ""
    )
    if not check_self_invocation(wired, scripts, no_check8c_call, wired_release_parity):
        failures.append(
            "check_self_invocation: missed a deleted check-8c production call site "
            "(SMA-542 residual closure)"
        )
    # The SAME defect one level further out again, against check 8d (SMA-542 residual closure, PR
    # 150 follow-up — closes README L12): deleting ITS production call site must be caught too, or
    # check 8d would be the sole judge of its own wiring.
    no_check8d_call = wired_actionlint.replace(
        "done < <(block_execution_verdict .github/workflows/ci.yml)\n", ""
    )
    if not check_self_invocation(wired, scripts, no_check8d_call, wired_release_parity):
        failures.append(
            "check_self_invocation: missed a deleted check-8d production call site "
            "(SMA-542 residual closure, README L12)"
        )
    # CodeRabbit (PR 150) — the column-0 requirement itself. Before this fix, `check_self_invocation`
    # matched actionlint call sites by STRIPPED line (both sides), so a required line was satisfied
    # by that exact text appearing anywhere, including indented inside an `if false; then … fi`
    # block or a heredoc — neither of which ever executes. Wrapping a call in a conditional
    # conventionally INDENTS it, so an indented copy of each of the six required lines must now be
    # reported missing — one row per line, so a mutant that widened the column-0 check back to
    # "matches anywhere" is caught regardless of which entry it is tested against. The "fired on a
    # wired tree" assertion above already proves the real, column-0 tree keeps passing under this
    # tighter rule.
    indented_run_self_tests = wired_actionlint.replace("run_self_tests\n", "  run_self_tests\n", 1)
    if not check_self_invocation(wired, scripts, indented_run_self_tests, wired_release_parity):
        failures.append(
            "check_self_invocation: an INDENTED run_self_tests call satisfied the column-0 pin"
        )
    indented_battery = wired_actionlint.replace(
        "selftest_mutation_battery\n", "  selftest_mutation_battery\n"
    )
    if not check_self_invocation(wired, scripts, indented_battery, wired_release_parity):
        failures.append(
            "check_self_invocation: an INDENTED selftest_mutation_battery call satisfied the "
            "column-0 pin"
        )
    indented_floor_call = wired_actionlint.replace(
        "done < <(ci_target_floor_verdict .github/workflows/ci.yml)\n",
        "  done < <(ci_target_floor_verdict .github/workflows/ci.yml)\n",
    )
    if not check_self_invocation(wired, scripts, indented_floor_call, wired_release_parity):
        failures.append(
            "check_self_invocation: an INDENTED check-8 call site satisfied the column-0 pin"
        )
    indented_check8b_call = wired_actionlint.replace(
        'done < <(invocation_allowlist_verdict .github/workflows/ci.yml "$REPORTED_LINENOS")\n',
        '  done < <(invocation_allowlist_verdict .github/workflows/ci.yml "$REPORTED_LINENOS")\n',
    )
    if not check_self_invocation(wired, scripts, indented_check8b_call, wired_release_parity):
        failures.append(
            "check_self_invocation: an INDENTED check-8b call site satisfied the column-0 pin"
        )
    indented_check8c_call = wired_actionlint.replace(
        "done < <(affected_graph_wiring_verdict ci/affected-graph/run.sh)\n",
        "  done < <(affected_graph_wiring_verdict ci/affected-graph/run.sh)\n",
    )
    if not check_self_invocation(wired, scripts, indented_check8c_call, wired_release_parity):
        failures.append(
            "check_self_invocation: an INDENTED check-8c call site satisfied the column-0 pin"
        )
    indented_check8d_call = wired_actionlint.replace(
        "done < <(block_execution_verdict .github/workflows/ci.yml)\n",
        "  done < <(block_execution_verdict .github/workflows/ci.yml)\n",
    )
    if not check_self_invocation(wired, scripts, indented_check8d_call, wired_release_parity):
        failures.append(
            "check_self_invocation: an INDENTED check-8d call site satisfied the column-0 pin"
        )
    # Contamination cases, THREE of them (SMA-542 review finding I1, plus a round-2 addition). The
    # obvious "swap the two texts wholesale" version tried first passed unconditionally, because it
    # only proves the required site is ABSENT from the wrong haystack — never exercising whether
    # the haystacks are actually checked separately. An 8-mutant battery against
    # check_self_invocation found three survivors of that version: actionlint sites satisfied by
    # run_sh_text, run.sh sites satisfied by actionlint_sh_text, and actionlint sites satisfied by
    # task-script text. Each case below concatenates the WRONG haystack's fully-wired text onto the
    # ALREADY-BROKEN text under test: if the two are ever read as one, the missing site would be
    # masked by the appended text and this would wrongly pass. Round 1 landed only the first two —
    # the task-script pairing is a DISTINCT haystack combination neither of them exercises, so a
    # check that concatenated task-script and actionlint text would have survived undetected.
    if not check_self_invocation(
        wired + wired_actionlint, scripts, no_actionlint_call, wired_release_parity
    ):
        failures.append("check_self_invocation: an actionlint site was satisfied by run.sh text")
    if not check_self_invocation(no_call, scripts, wired_actionlint + wired, wired_release_parity):
        failures.append("check_self_invocation: a run.sh site was satisfied by actionlint text")
    if not check_self_invocation(
        wired, wired_scripts(**{"input-liveness": wired_script + wired_actionlint}),
        no_actionlint_call, wired_release_parity,
    ):
        failures.append(
            "check_self_invocation: an actionlint site was satisfied by task-script text"
        )
    # The docstring's "REQUIRED positional parameter" claim (SMA-542, extended SMA-530) is
    # otherwise unenforced: every caller above already passes both explicitly, so a future
    # `actionlint_sh_text=""` or `release_parity_sh_text=""` default would make all of them pass
    # vacuously — the exact class of hole these parameters exist to close — while every
    # call-site-shaped assertion above stayed green. Only introspecting the signature itself
    # catches that regression (SMA-542 review, smaller correction 3; looped over both parameter
    # names so the SMA-530 addition gets the same guarantee, not just the pre-existing one).
    for _param_name in ("actionlint_sh_text", "release_parity_sh_text"):
        _default = inspect.signature(check_self_invocation).parameters[_param_name].default
        if _default is not inspect.Parameter.empty:
            failures.append(
                f"check_self_invocation: {_param_name} must stay a REQUIRED parameter"
            )
    # The task-script haystack strips BOTH sides (:673) — unlike the actionlint haystack's
    # column-0 rule — because Moon task scripts are indented inside YAML. Assert that
    # tolerance directly: a wired-but-indented script must NOT be reported missing.
    indented_task_script = "".join(
        f"  {line}\n" for line in SELF_SCHEDULED_GATES["input-liveness"]
    )
    if check_self_invocation(
        wired, wired_scripts(**{"input-liveness": indented_task_script}), wired_actionlint,
        wired_release_parity,
    ):
        failures.append("check_self_invocation: an indented but fully wired script was reported missing")

    # SMA-530 — one row per pinned line, so a mutant that widened the match back to
    # "matches anywhere" is caught regardless of which entry it is tested against.
    for _site in RELEASE_PARITY_SH_CALL_SITES:
        _broken = "".join(
            line for line in wired_release_parity.splitlines(keepends=True)
            if line.strip() != _site
        )
        if not check_self_invocation(wired, scripts, wired_actionlint, _broken):
            failures.append(
                f"check_self_invocation: missed {_site!r} deleted from ci/release-parity/run.sh"
            )
    # Contamination: a release-parity site must not be satisfiable from another haystack.
    if not check_self_invocation(
        wired + wired_release_parity, scripts, wired_actionlint,
        "".join(line for line in wired_release_parity.splitlines(keepends=True)
                if line.strip() != RELEASE_PARITY_SH_CALL_SITES[0])
    ):
        failures.append(
            "check_self_invocation: a release-parity site was satisfied by run.sh text"
        )
    # The release-parity haystack is whole-LINE, not substring, matched, and this fixture proves
    # that is load-bearing, not decorative: commenting out a pinned line
    # (`# if [ "$NEGATIVE" = 1 ]; then`) changes its stripped text and must be reported missing,
    # but a widened match (`site not in text` as a plain substring over the whole file) would
    # accept a line that never executes. This is the opposite direction from the
    # indentation-tolerance property already exercised by every fixture above (an indented copy of
    # a case arm must still be ACCEPTED, by design) — a commented-out copy must NOT be, and only a
    # whole-line comparison tells the two apart.
    commented_out = wired_release_parity.replace(
        'if [ "$NEGATIVE" = 1 ]; then\n', '# if [ "$NEGATIVE" = 1 ]; then\n'
    )
    if not check_self_invocation(wired, scripts, wired_actionlint, commented_out):
        failures.append(
            "check_self_invocation: a COMMENTED-OUT release-parity line satisfied the pin "
            "(widened to substring matching)"
        )

    # _scripts (SMA-553 D10) — a second pure extractor, so _eligibility's shape is untouched.
    got_scripts = _scripts({"repo": {"input-liveness": {"script": "hi"}}, "ts": {"lint": {}}})
    if got_scripts != {"input-liveness": "hi"}:
        failures.append(f"_scripts: returned {got_scripts!r}")
    if _scripts({"repo": {"a": {"command": "true"}}}) != {"a": ""}:
        failures.append("_scripts: a task with no script must map to an empty string, not raise")
    # Review finding 3 — a non-string `script` must be routed to rc 2 (MoonOutputError), not left
    # to reach check_self_invocation's `.splitlines()` and raise a bare, misclassified AttributeError.
    expect_infra("_scripts[non-string-script]",
                 lambda: _scripts({"repo": {"input-liveness": {"script": ["not", "a", "string"]}}}))

    # SMA-530 — the three registries, driven with fixtures rather than asserted inline, so
    # each row can be shown to fire.
    def pairing(label, scheduled, globs, exempt, want):
        got = check_registry_pairing(scheduled, globs, exempt)
        if got != want:
            failures.append(f"check_registry_pairing[{label}]: got {got}, want {want}")

    pairing("real-registries", None, None, None, ([], [], [], [], []))
    pairing("unpinned", {"g": ()}, {}, {}, (["g"], [], [], [], []))
    pairing("pinned-by-globs", {"g": ()}, {"g": ("**/*",)}, {}, ([], [], [], [], []))
    pairing("pinned-by-exemption", {"g": ()}, {}, {"g": "reason"}, ([], [], [], [], []))
    pairing("empty-reason", {"g": ()}, {}, {"g": "   "}, ([], ["g"], [], [], []))
    pairing("stale-exemption", {}, {}, {"ghost": "outlived its task"}, ([], [], ["ghost"], [], []))
    pairing("exempt-and-pinned", {"g": ()}, {"g": ("**/*",)}, {"g": "r"}, ([], [], [], ["g"], []))
    pairing("orphan-globs", {}, {"ghost": ("**/*",)}, {}, ([], [], [], [], ["ghost"]))

    # SMA-553 D13, mirrored here so repo:input-liveness is not the sole judge of its own inputs.
    # The wired row carries the implicit .moon glob moon injects into every task, which must be
    # tolerated rather than counted as drift.
    #
    # EVERY fixture below is built by `wired()`, which supplies a correctly-wired entry for each
    # gate in SELF_TASK_EXPECTED_GLOBS and then replaces only the one under test (SMA-576). The
    # rows used to hardcode a single-task payload, which was fine while the table had one entry
    # and silently vacuous the moment it had two: check_gate_inputs reports every REGISTERED gate
    # missing from the payload, so the "wired" row would have fired on the absent second gate
    # rather than on what it was testing, and each negative row would have passed for that same
    # wrong reason. Deriving the payload from the table also means it cannot go stale.
    injected = ".moon/*.{yml,yaml,jsonc,json,pkl,hcl,toml}"

    def wired(overrides=None):
        repo = {}
        for gate, want in SELF_TASK_EXPECTED_GLOBS.items():
            globs = [g for g in want if any(c in g for c in "*?[{")]
            repo[gate] = {
                "inputGlobs": {**{g: {} for g in globs}, injected: {}},
                "inputFiles": {f: {} for f in want if f not in globs},
            }
        repo.update(overrides or {})
        return {"repo": repo}

    if check_gate_inputs(wired()):
        failures.append("check_gate_inputs: fired on a wired inputs declaration")
    if not check_gate_inputs(wired({"input-liveness": {"inputGlobs": {"ops/**/*": {}}}})):
        failures.append("check_gate_inputs: missed inputs narrowed away from **/*")
    # ...and WIDENED with an extra glob, which is equally a change to a load-bearing input set.
    # Relaxing the exact `!=` to a subset test keeps every other row here green.
    if not check_gate_inputs(
        wired({"input-liveness": {"inputGlobs": {"**/*": {}, "ops/**/*": {}}}})
    ):
        failures.append("check_gate_inputs: missed an extra glob alongside **/*")
    # ...and a file input on a GLOB-ONLY gate, which nothing but the files half of the comparison
    # can catch: the glob tuple is untouched here, so dropping `files` from it leaves every other
    # row green. (This was the `or files` clause before SMA-576 generalised the comparison.)
    if not check_gate_inputs(wired({"input-liveness": {
            "inputGlobs": {"**/*": {}}, "inputFiles": {".prototools": {}}}})):
        failures.append("check_gate_inputs: missed a file input")
    # ...and both directions on a FILES-ONLY gate (SMA-576). repo:version-lockstep declares
    # sixteen literal paths and no glob, so neither of these rows is visible to the glob tuple at
    # all: dropping an input silently shrinks the set of files that re-key the gate — it then
    # reports PASS from cache over a version site it never read — and adding one is equally a
    # change to a set two independently scheduled gates are supposed to agree on.
    files_only = wired()["repo"]["version-lockstep"]
    dropped = dict(files_only, inputFiles=dict(list(files_only["inputFiles"].items())[1:]))
    if not check_gate_inputs(wired({"version-lockstep": dropped})):
        failures.append("check_gate_inputs: missed a dropped file input on a files-only gate")
    added = dict(files_only, inputFiles={**files_only["inputFiles"], "rs/deny.toml": {}})
    if not check_gate_inputs(wired({"version-lockstep": added})):
        failures.append("check_gate_inputs: missed an extra file input on a files-only gate")
    # ...and the ORDER the two buckets are compared in (SMA-576). Every row above is blind to it:
    # both registered gates declare exactly one kind of input, so "globs then files" and "files
    # then globs" agree on all of them, and a mutation reversing the two survived the whole battery
    # until this pair. Driven through `expected_table` rather than by adding a fake gate to the
    # live registry, so nothing about the real graph is disturbed.
    mixed_payload = {"repo": {"mixed": {
        "inputGlobs": {"a/**/*": {}, injected: {}}, "inputFiles": {"b.txt": {}}
    }}}
    if check_gate_inputs(mixed_payload, {"mixed": ("a/**/*", "b.txt")}):
        failures.append("check_gate_inputs: fired on a wired globs-and-files declaration")
    if not check_gate_inputs(mixed_payload, {"mixed": ("b.txt", "a/**/*")}):
        failures.append("check_gate_inputs: accepted globs and files in the wrong order")
    # ...and that the parameter above still defaults to the LIVE registry. Without this row a
    # future refactor could hand production an empty or stubbed table and every check here would
    # keep passing while the real gates went unasserted — the same hole the actionlint_sh_text
    # signature row below exists to close.
    gate_default = inspect.signature(check_gate_inputs).parameters["expected_table"].default
    if gate_default is not SELF_TASK_EXPECTED_GLOBS:
        failures.append(
            "check_gate_inputs: expected_table must default to SELF_TASK_EXPECTED_GLOBS"
        )
    # ...and the task vanishing from the graph, which must be a LOUD row rather than a silent
    # `continue`: a task that cannot be found cannot be checked, so skipping it asserts nothing.
    if not check_gate_inputs({"repo": {}}):
        failures.append("check_gate_inputs: missed input-liveness being absent from the graph")
    # Review finding 3 — a present-but-wrong-typed inputGlobs/inputFiles is a moon output shape
    # change, not authored drift; it must be routed to rc 2 (MoonOutputError) rather than silently
    # misread by `sorted(...)` or raising an uncaught, misclassified exception.
    expect_infra(
        "check_gate_inputs[non-dict-inputGlobs]",
        lambda: check_gate_inputs({"repo": {"input-liveness": {"inputGlobs": ["**/*"]}}}),
    )
    expect_infra(
        "check_gate_inputs[non-dict-inputFiles]",
        lambda: check_gate_inputs(
            {"repo": {"input-liveness": {"inputGlobs": {"**/*": {}}, "inputFiles": ["x"]}}}
        ),
    )

    if failures:
        print("ci-targets self-test FAILED:", file=sys.stderr)
        for f in failures:
            print(f"  {f}", file=sys.stderr)
        return 1
    print("ci-targets self-test OK")
    return 0


def main():
    root = Path(__file__).resolve().parents[2]
    try:
        raw_tasks = moon_payload()
        tasks = _eligibility(raw_tasks)
        ci_yml = read_input(root / ".github" / "workflows" / "ci.yml", ".github/workflows/ci.yml")
        t_targets = parse_t(ci_yml)
        doc_targets, region = parse_doc_targets(
            read_input(root / "CLAUDE.md", "CLAUDE.md")
        )
        run_sh = read_input(
            root / "ci" / "affected-graph" / "run.sh", "ci/affected-graph/run.sh"
        )
        actionlint_sh = read_input(
            root / "ci" / "actionlint" / "run.sh", "ci/actionlint/run.sh"
        )
        release_parity_sh = read_input(
            root / "ci" / "release-parity" / "run.sh", "ci/release-parity/run.sh"
        )
        floor = check_floor(tasks)
        missing, unexpected, bad_exempt, stale_exempt = check_forward(tasks, t_targets)
        # SMA-553 review finding 1 — these two also raise MoonOutputError (INFRA_ERRORS), so their
        # call sites belong inside this same try: called from the validation flow below it, a raise
        # would escape main() uncaught and exit 1, misreporting an infrastructure fault (a moon
        # output shape change) as the rc-1 authorial-mistake path. Bound to locals here and reused
        # below so the try block stays the single place these two extractors are invoked.
        scripts = _scripts(raw_tasks)
        bad_gate_inputs = check_gate_inputs(raw_tasks)
    except GateAssertionError as exc:
        # An authorial mistake, NOT a broken tool: rc 1 so run.sh records a red suite instead of
        # aborting the whole affected-graph guard and losing every other assertion's output (D2).
        print(f"FAIL  [ci-targets] {exc}", file=sys.stderr)
        return 1
    except INFRA_ERRORS as exc:
        print(f"FATAL [ci-targets] could not read the inputs: {exc}", file=sys.stderr)
        return 2

    dead = check_reverse(tasks, t_targets)
    doc_problems = check_docs(t_targets, doc_targets, region)
    missing_sites = check_self_invocation(run_sh, scripts, actionlint_sh, release_parity_sh)
    bad_invocation = check_invocation(ci_yml)

    if not (floor or missing or unexpected or bad_exempt or stale_exempt or dead or doc_problems
            or missing_sites or bad_invocation or bad_gate_inputs):
        print(
            f"PASS  {'ci-targets':<18} -> {len(t_targets)} targets: every CI-eligible repo task is "
            "in ci.yml's T, every entry resolves, CLAUDE.md mirrors it"
        )
        return 0

    print("FAIL  [ci-targets] ci.yml's moon ci target array is out of sync", file=sys.stderr)
    # Rows that name a `T` ENTRY are printed WITH the leading colon, so they read as what the
    # reader sees in ci.yml and CLAUDE.md and as what the fix line tells them to type — a forgotten
    # gate printed bare `new-gate` under "append `:<name>`" made the reader do the translation.
    # `floor` and `bad_exempt`/`stale_exempt` stay BARE deliberately: their fix sites are
    # REQUIRED_REPO_TASKS and T_EXEMPT in this file, where the names are written without a colon.
    # `doc_problems`, `missing_sites` and `bad_invocation` are sentences and command text, not names.
    for rows, title in (
        (floor,
         "A task this gate REQUIRES to be present is absent from the parsed `repo` set, so the\n"
         "    comparison below may be between two empty sets and assert nothing.\n"
         "    Fix: if the task was genuinely renamed or removed, update REQUIRED_REPO_TASKS in\n"
         "    ci/affected-graph/ci_targets.py. Otherwise the project filter or moon's output\n"
         "    shape has changed — investigate before touching anything else."),
        ([":" + name for name in missing],
         "A CI-eligible `repo:*` task is NOT in ci.yml's `T=(...)` array, so it does not run in\n"
         "    CI at all — it passes locally and silently does not exist on any PR (SMA-541).\n"
         "    Fix: append `:<name>` to `T` in .github/workflows/ci.yml AND to the command\n"
         "    between the <!-- ci-targets:begin/end --> markers in CLAUDE.md."),
        ([":" + name for name in unexpected],
         "`T` contains a `repo` task that is NOT CI-eligible (runInCI: false) or is listed in\n"
         "    T_EXEMPT. `moon ci` will resolve nothing for it and still exit 0, so the gate reads\n"
         "    as running while it is off.\n"
         "    Fix: remove the entry from `T` and from CLAUDE.md, or drop the `runInCI: false` /\n"
         "    the T_EXEMPT entry if the task is meant to run."),
        (bad_exempt,
         "A T_EXEMPT entry has no reason string. An exemption is a recorded decision, so the\n"
         "    record is what earns it.\n"
         "    Fix: give it a non-empty reason in ci/affected-graph/ci_targets.py, or delete it."),
        (stale_exempt,
         "A T_EXEMPT entry names no `repo` task at all — the task it exempted was renamed or\n"
         "    deleted and the exemption outlived it. A typo is loud (the real task shows up under\n"
         "    `missing` above); a leftover is silent, and exempts nothing forever.\n"
         "    Fix: delete the entry from T_EXEMPT in ci/affected-graph/ci_targets.py, or correct\n"
         "    its name."),
        ([":" + name for name in dead],
         "A `T` entry resolves to no CI-eligible task anywhere in the graph — a typo, or a task\n"
         "    that was renamed, deleted or turned off. `moon ci` exits 0 on such a target, even\n"
         "    when real targets surround it, so nothing else in CI reports this.\n"
         "    Fix: correct the entry in .github/workflows/ci.yml and CLAUDE.md, or delete it."),
        (doc_problems,
         "CLAUDE.md's documented full-graph command no longer mirrors `T`, so the documented way\n"
         "    to reproduce CI locally does not reproduce it.\n"
         "    Fix: copy `T` verbatim between the <!-- ci-targets:begin/end --> markers, keeping\n"
         "    the `--base origin/main --include-relations` tail."),
        (missing_sites,
         "A gate's own call site is missing: this gate's, from\n"
         "    ci/affected-graph/run.sh; a self-scheduled gate's own invocation from inside its\n"
         "    moon.yml task script; or repo:actionlint's, from ci/actionlint/run.sh — so that\n"
         "    gate (or its negative control) would not run at all.\n"
         "    Fix: restore the exact line; see RUN_SH_CALL_SITES, SELF_SCHEDULED_GATES,\n"
         "    ACTIONLINT_SH_CALL_SITES and RELEASE_PARITY_SH_CALL_SITES in\n"
         "    ci/affected-graph/ci_targets.py.\n"
         "    A row prefixed `ci/actionlint/run.sh:` means repo:actionlint would run its checks\n"
         "    while asserting nothing — its self-tests or its mutation battery are no longer\n"
         "    invoked.\n"
         "    A row prefixed `ci/release-parity/run.sh:` means one of the five pinned\n"
         "    --negative-control lines — the flag parse, the NEGATIVE guard, the check_case\n"
         "    assertion, or either report arm — is gone from run.sh: whichever one the row\n"
         "    names is missing, so the control can no longer do its job (a missing parse or\n"
         "    guard falls straight through to the real suite and reports nothing; a missing\n"
         "    assertion or report arm breaks or misreports the control's own verdict)."),
        (bad_invocation,
         "A `moon ci` invocation in .github/workflows/ci.yml does not hand it the WHOLE `T`\n"
         "    array. Every check above asserts what is IN `T`; this one asserts `T` is what runs.\n"
         "    A subsetted or rewritten expansion (`\"${T[@]:0:5}\"`, an unquoted `$T`) leaves the\n"
         "    array perfectly correct, keeps run.sh's --include-relations grep matching, and\n"
         "    silently stops most of the graph from running.\n"
         "    Fix depends on which row this is. A line below that IS a real invocation: make it\n"
         "    read `" + MOON_CI_INVOCATION + "` verbatim. A `(no ... invocation anywhere in the\n"
         "    file)` line: nothing below names a line to fix — restore one. A line below that is\n"
         "    actually a YAML comment matched by the quote-gated regex, not a real invocation:\n"
         "    that is a false positive, so reword the COMMENT instead. If a second,\n"
         "    deliberately-different invocation is genuinely wanted, extend check_invocation in\n"
         "    ci/affected-graph/ci_targets.py rather than loosening it."),
        (bad_gate_inputs,
         "A self-scheduled gate's own `inputs` no longer match what it needs to see. This is the\n"
         "    second, independently-scheduled copy of an assertion that gate also makes about\n"
         "    itself — it exists so the gate is not the sole judge of its own configuration.\n"
         "    Fix: restore `inputs: ['**/*']` on the task in moon.yml."),
    ):
        if rows:
            print(f"  {title}", file=sys.stderr)
            for row in rows:
                print(f"      {row}", file=sys.stderr)
    return 1


if __name__ == "__main__":
    sys.exit(self_test() if "--self-test" in sys.argv[1:] else main())
