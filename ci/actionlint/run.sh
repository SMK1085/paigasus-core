#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# actionlint gate over .github/workflows/** (SMA-525).
#
# WHY THIS EXISTS. A `paths:` filter that comes to match nothing does not error — the workflow
# simply stops running, forever, with no red check and no notification. prebuild.yml triggers
# only on push-to-main/workflow_dispatch plus a narrow pull_request filter, so its 7-platform
# verification would silently cease. Nothing in this repo linted workflow YAML before this.
#
# actionlint alone is NOT sufficient: it validates syntax and has no view of the file tree, so
# a syntactically valid glob that matches nothing (`rz/**`) passes it cleanly. Checks 5-7 below
# are what actually close the failure this gate was filed for — and because they are, they carry
# their own standing control: `path_filter_self_test` (part of check 7) asserts the verdict of
# every vocabulary rule against a fixture table, so neutering one of them reds the gate.
#
# EXIT CODES (ci/ convention): 1 = assertion failure, 2 = infrastructure error. Without the
# split, a broken tool reads as a lint failure — or, if anyone wraps this in `|| true`, as a pass.
#
# NOT `set -e`: several checks deliberately expect and inspect non-zero exits — check 3 requires
# actionlint to FAIL on each fixture, and the verdict helpers of checks 5/6 signal through their
# status. Each check captures status explicitly and sets FAILED instead.
set -uo pipefail

# Absolute path to THIS file, captured BEFORE the cd below. `$0` is not usable after it: invoked
# as `cd ci/actionlint && ./run.sh`, `$0` is './run.sh', which stops resolving the moment we move
# to the repo root. Check 9 copies this file, and run_self_tests greps it (SMA-542 D11).
SELF_SRC="$(CDPATH= cd -- "$(dirname "${BASH_SOURCE[0]}")" && pwd)/$(basename "${BASH_SOURCE[0]}")"

cd "$(git rev-parse --show-toplevel)" || exit 2

FAILED=0

# Check 7's counter. A fixture table that is never CALLED is dead code, and deleting the calls was
# the sole survivor of SMA-525's mutation battery. The increment lives inside each self-test (not
# at the call site) so it survives reformatting and cannot be spoofed by a stranded increment.
# Deliberately NOT `readonly`: without `set -e` a reassignment only warns, so readonly buys no
# protection and would break a future harness that sources this file twice (SMA-542 D3).
SELF_TESTS_RAN=0
SELF_TEST_COUNT=13  # extractor, path-filter, branch-filter, config, ci-target-floor,
                    # invocation-allowlist, affected-graph-wiring, block-execution,
                    # kill-predicate, affected-smoke-block, release-guard, cargo-lock-step,
                    # release-plan

fail() {
  echo "actionlint gate: $*" >&2
  FAILED=1
}

infra() {
  echo "actionlint gate: INFRASTRUCTURE ERROR: $*" >&2
  exit 2
}

usage() {
  echo "usage: $(basename "$0") [--self-test]" >&2
  echo "  (no argument)  run the full gate" >&2
  echo "  --self-test    run the thirteen fixture tables only — extractor, path-filter verdicts," >&2
  echo "                 branch-filter verdicts, config allowlist, ci-target floor, invocation" >&2
  echo "                 allowlist, affected-graph wiring, block execution, kill predicate," >&2
  echo "                 affected-smoke block, release guard, cargo-lock step, release-plan." >&2
  echo "                 No actionlint binary is required, but the branch-filter table needs a" >&2
  echo "                 git repo carrying refs/remotes/origin/main, and the release-guard table" >&2
  echo "                 shells out to 'uv run --locked --project py', so it needs uv on PATH and" >&2
  echo "                 a py workspace whose uv.lock is up to date. The release-plan table" >&2
  echo "                 shells out to 'uv run --project ci/release-plan', which needs uv on" >&2
  echo "                 PATH and that project's own uv.lock up to date." >&2
  echo "                 The check-9 mutation battery is NOT part of this — full gate only." >&2
  exit 2
}

# ONE shared flag array for checks 1, 3 and 4. Written twice, an `-ignore` added to check 1
# would be invisible to check 3 BY CONSTRUCTION and the self-test would be decorative.
#
# ShellCheck/pyflakes are disabled DELIBERATELY (spec D2): actionlint shells out to them when
# it finds them on PATH, which would make this gate's strictness a property of the host.
# (Capital "ShellCheck" above is intentional — a lowercase "# shellcheck/..." comment is parsed
# by ShellCheck itself as a malformed inline directive and aborts analysis of this whole file.)
ARGS=(-shellcheck= -pyflakes=)

# Workflow discovery for checks 5-7. Non-recursive, both extensions — matching GitHub's own
# execution semantics. Check 1 does NOT use this list (see below).
WORKFLOW_FILES=()
for f in .github/workflows/*.yml .github/workflows/*.yaml; do
  [ -e "$f" ] && WORKFLOW_FILES+=("$f")
done
[ ${#WORKFLOW_FILES[@]} -gt 0 ] || infra "no workflow files found under .github/workflows/"

# Extract the four filter keys — paths:, paths-ignore:, branches:, branches-ignore: — and their
# sequence entries from one workflow file. Output records, TAB-separated, in file order:
#   KEY\t<kind>\t<lineno>
#   ITEM\t<kind>\t<pattern-or-branch>
# See the contract in docs/superpowers/plans/2026-08-16-sma-525-actionlint-gate.md (Task 4) and
# ci/actionlint/README.md. Every clause below has a fixture in extractor_self_test.
extract_filter_keys() {
  awk '
    # Strip a quoted scalar to its contents; strip an unquoted one to its pre-comment text.
    function scalar(s,   q, i, c, out) {
      q = substr(s, 1, 1)
      if (q == "\"" || q == "\047") {
        out = ""
        for (i = 2; i <= length(s); i++) {
          c = substr(s, i, 1)
          if (c == q) break
          out = out c
        }
        return out
      }
      sub(/[ \t]+#.*$/, "", s)     # trailing comment, only when preceded by whitespace
      sub(/[ \t]+$/, "", s)
      return s
    }

    # A flow mapping (`{ branches: [main], paths: [a] }`) is deliberately NOT parsed for entries.
    # Print a KEY with no ITEMs for any filter key it declares AT THE CALLER TARGET DEPTH, so
    # check 6 fails loudly instead of the whole event being skipped in silence. `target` mirrors
    # the block-form depth rule above: 2 when v is the top-level `on: { ... }` scalar (on -> event
    # -> paths), 1 when v is one event own flow value (event -> paths). A `paths` key one level
    # DEEPER than target is a workflow input legitimately NAMED `paths` —
    # `on: { workflow_dispatch: { inputs: { paths: {...} } } }` (depth 3, target 2) or
    # `push: { inputs: { paths: x } }` (depth 2, target 1) — and must be ignored, exactly like
    # `on.workflow_dispatch.inputs.paths` is ignored in block style. Grepping for the `paths`
    # token at ANY depth (the previous implementation) is what let a flow-style `inputs.paths`
    # false-red check 6 (SMA-525 round-2 review). So this tracks brace depth and quoted-string
    # spans char by char instead of a single depth-blind regex. Returns nothing when v is not a
    # flow mapping.
    function flow_keys(v, lineno, target,    depth, i, n, c, instr, qc, prevc, rest, fkey) {
      if (v !~ /^[{]/) return
      sub(/[}][^}]*$/, "}", v)     # drop a trailing comment after the closing brace
      depth = 0
      instr = 0                    # 1 while scanning inside a quoted VALUE we chose not to parse
      qc = ""                      # the quote character (double or single) that will close it
      n = length(v)
      for (i = 1; i <= n; i++) {
        c = substr(v, i, 1)

        if (instr) {
          if (c == qc) instr = 0
          continue
        }

        # A key can only start right after `{`, `,` or whitespace — same boundary the old regex
        # required. Tried BEFORE the generic brace/quote handling below so a quoted key
        # ("paths": ...) is matched here, by the optional leading quote in the pattern itself,
        # rather than being swallowed as an opaque quoted string first.
        prevc = (i > 1) ? substr(v, i - 1, 1) : ""
        if (prevc == "{" || prevc == "," || prevc == " " || prevc == "\t") {
          rest = substr(v, i)
          if (match(rest, /^["\047]?(paths|branches)(-ignore)?["\047]?[ \t]*:/)) {
            fkey = substr(rest, 1, RLENGTH)
            sub(/["\047]?[ \t]*:$/, "", fkey)
            sub(/^["\047]/, "", fkey)
            if (depth == target) print "KEY\t" fkey "\t" lineno
            i += RLENGTH - 1   # the for loop own i++ then makes the net advance RLENGTH
            continue
          }
        }

        if (c == "{") { depth++; continue }
        if (c == "}") { depth--; continue }
        if (c == "\"" || c == "\047") { instr = 1; qc = c; continue }
      }
    }

    {
      line = $0
      sub(/\r$/, "", line)                       # tolerate CRLF
      match(line, /^[ ]*/); ind = RLENGTH
      stripped = line
      sub(/^[ ]*/, "", stripped)

      if (stripped == "")   next                 # blank lines never close a block
      if (stripped ~ /^#/)  next                 # whole-line comments never close a block

      if (in_block) {
        is_item = (stripped ~ /^-([ \t]|$)/)
        if (ind >= key_ind && is_item) {
          item = stripped
          sub(/^-[ \t]*/, "", item)
          item = scalar(item)
          if (item != "") print "ITEM\t" kind "\t" item
          next
        } else if (ind <= key_ind) {
          # A non-item line at or below key_ind closes the block — this is NOT simply "dedent":
          # items at exactly key_ind (a flush sequence, no extra indent — valid YAML, emitted by
          # e.g. Prettier) must stay ITEMs, which the `ind >= key_ind && is_item` branch above
          # already caught. Only a non-item line at/below key_ind reaches this branch.
          in_block = 0                           # closes; fall through to key handling below
        } else {
          next                                   # deeper non-item line: not ours, keep the block
        }
      }

      # Track the top-level `on:` mapping. A quoted "on": is accepted (a common YAML 1.1
      # truthiness workaround), and so is whitespace before the colon (`on :`) — both are valid
      # YAML that actionlint accepts, and the regex not accepting them was a silent skip of the
      # WHOLE file, not just one filter (SMA-525 round-2 review finding B). Any other column-0
      # key closes it. Strip a trailing comment before classifying: `on:  # comment` must still be
      # recognized as the block form, not misread as the inline-flow form (which would silently
      # drop every paths: key in the whole file).
      if (ind == 0) {
        key0 = stripped
        sub(/[ \t]+#.*$/, "", key0)
        depth = 0                                # a column-0 key closes every nested level
        if (key0 ~ /^["\047]?on["\047]?[ \t]*:[ \t]*$/)      { in_on = 1; next }
        if (key0 ~ /^["\047]?on["\047]?[ \t]*:/) {
          # Inline `on: [push]` — nothing to extract. But `on: {push: {paths: [a]}}` is the same
          # silently-guards-nothing hole as a flow-mapping EVENT value, one level up, so it gets
          # the same treatment. Target depth 2: on -> event -> paths, matching the block-form
          # depth rule below (a `paths` at depth 3, e.g. an `inputs.paths` input, is ignored).
          in_on = 0
          val = key0
          sub(/^[^:]*:[ \t]*/, "", val)
          flow_keys(val, NR, 2)
          next
        }
        in_on = 0
        next
      }

      if (!in_on) next

      # A sequence entry outside a recognized filter block (a `schedule:` list, say) introduces no
      # mapping level, so it must not perturb the depth stack below. A `branches:`/`paths:`/etc.
      # entry never reaches here — it is consumed as an ITEM one clause earlier, by the `in_block`
      # handling above, while its block is still open.
      if (stripped ~ /^-([ \t]|$)/) next

      # DEPTH INSIDE `on:`. `on:` is level 0; an event key (push:, pull_request:,
      # workflow_dispatch:, ...) is level 1; a key belonging to that event is level 2. Only a
      # level-2 `paths:`/`paths-ignore:`/`branches:`/`branches-ignore:` is a real filter key.
      #
      # Depth, not "anywhere under on:", because a workflow input may legitimately be NAMED
      # `paths` — `on.workflow_dispatch.inputs.paths` sits at level 3. Emitting a KEY for it made
      # check 6 fail with advice its author could not act on (there is no block sequence to write),
      # inside the only required check in this repo, with no escape hatch: SKIP_PATTERNS filters
      # patterns, not keys.
      while (depth > 0 && indstack[depth] >= ind) depth--
      depth++
      indstack[depth] = ind

      if (depth == 1) {
        # A flow-mapping event value — `push: { branches: [main], paths: [a] }` — is valid YAML
        # that actionlint accepts, and its `paths:`/`branches:` never reach the level-2 branch
        # below. Left alone it would make checks 5 and 6 silently guard nothing at all. Target
        # depth 1: event -> paths/branches (an `inputs.paths` here, e.g.
        # `push: { inputs: { paths: x } }`, is at depth 2 and must be ignored, same rule as above).
        val = stripped
        sub(/^[^:]*:[ \t]*/, "", val)
        flow_keys(val, NR, 1)
        next
      }
      if (depth != 2) next

      # Four filter keys, matched by one pattern and then read back out of the line, rather than
      # four near-identical regexes. A quoted key ("paths":, 'branches-ignore':) is valid YAML
      # actionlint accepts; the bare-only regex silently dropped it — no KEY record, so the checks
      # skipped the filter with no message (SMA-525 round-2 review finding A). Whitespace before
      # the colon (`paths :`) gets the same tolerance as `on :` above, for the same reason.
      if (stripped !~ /^["\047]?(paths|branches)(-ignore)?["\047]?[ \t]*:/) next
      kind = stripped
      sub(/["\047]?[ \t]*:.*$/, "", kind)   # drop the colon, any value, any trailing comment
      sub(/^["\047]/, "", kind)             # drop a leading quote

      print "KEY\t" kind "\t" NR

      # A block opens only when the value after the colon is empty. A non-empty value is the
      # inline flow form, which is deliberately not parsed — the KEY above, with no ITEMs
      # following, is what makes check 6 fail loudly instead of skipping silently.
      #
      # Strip the key label FIRST, then the comment, then surrounding blanks — in that order.
      # Stripping the label with a greedy `[ \t]*` (consuming the whitespace right after the
      # colon) would eat the whitespace a trailing `#comment` needs to match on, leaving a
      # `paths:   # the filter` line looking non-empty and silently skipping the block. The label
      # pattern mirrors the KEY match above (quotes and pre-colon whitespace optional) so a
      # quoted inline `"paths": [a, b]` strips down to its flow value, not a still-quoted label.
      rest = stripped
      sub(/^["\047]?(paths|branches)(-ignore)?["\047]?[ \t]*:/, "", rest)
      sub(/[ \t]+#.*$/, "", rest)
      sub(/^[ \t]+/, "", rest)
      sub(/[ \t]+$/, "", rest)
      if (rest == "") { in_block = 1; key_ind = ind }
    }
  ' "$1"
}

# ---------------------------------------------------------------------------------------------
# Extractor self-test (definition only — actually invoked, unconditionally, as part of check 7
# near the end of this script, so the fixture table guards the parser on every real gate run, not
# only under --self-test). Defined here, ahead of check 1, purely so the `--self-test` early exit
# below can run it standalone and return fast while editing the awk.
#
# The extractor is hand-rolled YAML parsing, which is exactly the kind of thing that silently
# does the wrong thing. Each clause of the documented contract gets a fixture.
# ---------------------------------------------------------------------------------------------
extractor_self_test() {
  local name expected actual tmp yaml rc=0
  SELF_TESTS_RAN=$((SELF_TESTS_RAN + 1))

  check_fixture() {
    name="$1"; expected="$2"; yaml="$3"
    tmp="$(mktemp)"
    printf '%s' "$yaml" > "$tmp"
    actual="$(extract_filter_keys "$tmp")"
    rm -f "$tmp"
    if [ "$actual" != "$expected" ]; then
      fail "extractor self-test '$name' mismatch.
--- expected ---
$expected
--- actual ---
$actual"
      rc=1
    fi
  }

  check_fixture 'simple block' \
"$(printf 'KEY\tpaths\t4\nITEM\tpaths\trs/**\nITEM\tpaths\t.prototools')" \
'name: t
on:
  push:
    paths:
      - "rs/**"
      - ".prototools"
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
'

  check_fixture 'a branches block is extracted' \
"$(printf 'KEY\tbranches\t4\nITEM\tbranches\tmain')" \
'name: t
on:
  push:
    branches:
      - main
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
'

  check_fixture 'a branches block followed by a sibling paths block keeps both' \
"$(printf 'KEY\tbranches\t4\nITEM\tbranches\tmain\nKEY\tpaths\t6\nITEM\tpaths\trs/**')" \
'name: t
on:
  push:
    branches:
      - main
    paths:
      - "rs/**"
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
'

  check_fixture 'a branches flow sequence emits KEY with no ITEMs' \
"$(printf 'KEY\tbranches\t4')" \
'name: t
on:
  push:
    branches: [main]
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
'

  check_fixture 'a branches-ignore block is extracted' \
"$(printf 'KEY\tbranches-ignore\t4\nITEM\tbranches-ignore\tdev')" \
'name: t
on:
  push:
    branches-ignore:
      - dev
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
'

  check_fixture 'a workflow_dispatch input named branches is not a filter' \
"" \
'name: t
on:
  workflow_dispatch:
    inputs:
      branches:
        description: which branches
        required: false
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
'

  check_fixture 'interior comments do not close the block' \
"$(printf 'KEY\tpaths\t4\nITEM\tpaths\ta/**\nITEM\tpaths\tb/**')" \
'name: t
on:
  push:
    paths:
      - "a/**"
      # a comment in the middle of the sequence
      #
      - "b/**"
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
'

  check_fixture 'trailing comments stripped, quotes stripped' \
"$(printf 'KEY\tpaths\t4\nITEM\tpaths\trs/**\nITEM\tpaths\tts/x.json\nITEM\tpaths\tbare/path')" \
'name: t
on:
  push:
    paths:
      - "rs/**"                 # includes rs/Cargo.lock
      - '"'"'ts/x.json'"'"'
      - bare/path
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
'

  check_fixture 'dedent closes the block' \
"$(printf 'KEY\tpaths\t4\nITEM\tpaths\ta/**\nKEY\tbranches\t6')" \
'name: t
on:
  push:
    paths:
      - "a/**"
    branches: [main]
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
'

  check_fixture 'paths-ignore is tagged distinctly' \
"$(printf 'KEY\tpaths-ignore\t4\nITEM\tpaths-ignore\tdocs/**')" \
'name: t
on:
  push:
    paths-ignore:
      - "docs/**"
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
'

  check_fixture 'inline flow form emits KEY with no ITEMs' \
"$(printf 'KEY\tpaths\t4')" \
'name: t
on:
  push:
    paths: ["a/**", "b/**"]
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
'

  check_fixture 'a paths: line inside a run block is ignored, while a sibling branches: flow sequence still emits KEY' \
"$(printf 'KEY\tbranches\t4')" \
'name: t
on:
  push:
    branches: [main]
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - run: |
          paths:
            - "not/a/filter"
'

  check_fixture 'negated entries are extracted, not dropped' \
"$(printf 'KEY\tpaths\t4\nITEM\tpaths\ta/**\nITEM\tpaths\t!a/docs/**')" \
'name: t
on:
  push:
    paths:
      - "a/**"
      - "!a/docs/**"
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
'

  # --- Round-1 review additions below: each kills a specific mutant that survived the table
  # above (findings 1, 2, 3 and 5 of the round-1 review). ---

  check_fixture 'blank line inside a sequence does not close the block' \
"$(printf 'KEY\tpaths\t4\nITEM\tpaths\ta/**\nITEM\tpaths\tb/**')" \
'name: t
on:
  push:
    paths:
      - "a/**"

      - "b/**"
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
'

  check_fixture 'CRLF line endings are tolerated' \
"$(printf 'KEY\tpaths\t4\nITEM\tpaths\ta/**\nITEM\tpaths\tb/**')" \
"$(printf 'name: t\r\non:\r\n  push:\r\n    paths:\r\n      - "a/**"\r\n      - "b/**"\r\njobs:\r\n  j:\r\n    runs-on: ubuntu-latest\r\n    steps:\r\n      - run: echo hi\r\n')"

  check_fixture '"on": quoted form is recognized' \
"$(printf 'KEY\tpaths\t4\nITEM\tpaths\ta/**')" \
'name: t
"on":
  push:
    paths:
      - "a/**"
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
'

  check_fixture 'on: [push] inline form yields no records' \
"" \
'name: t
on: [push]
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
'

  check_fixture 'unquoted item keeps a trailing comment stripped' \
"$(printf 'KEY\tpaths\t4\nITEM\tpaths\tbare/path')" \
'name: t
on:
  push:
    paths:
      - bare/path   # a note
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
'

  check_fixture 'a # inside a quoted scalar is not stripped' \
"$(printf 'KEY\tpaths\t4\nITEM\tpaths\ta/b#c')" \
'name: t
on:
  push:
    paths:
      - "a/b#c"
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
'

  check_fixture 'on: with a trailing comment still opens the block' \
"$(printf 'KEY\tpaths\t4\nITEM\tpaths\ta/**')" \
'name: t
on:  # yamllint disable-line rule:truthy
  push:
    paths:
      - "a/**"
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
'

  check_fixture 'paths: with a trailing comment still opens the block' \
"$(printf 'KEY\tpaths\t4\nITEM\tpaths\ta/**')" \
'name: t
on:
  push:
    paths:   # the filter
      - "a/**"
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
'

  check_fixture 'a flush block sequence (items at key indent) is still parsed' \
"$(printf 'KEY\tpaths\t4\nITEM\tpaths\ta/**\nITEM\tpaths\tb/**')" \
'name: t
on:
  push:
    paths:
    - "a/**"
    - "b/**"
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
'

  # --- Final-review additions. The two comment fixtures below make the `stripped ~ /^#/` clause
  # load-bearing: every comment in the fixtures above is indented DEEPER than its key, so the
  # "deeper non-item line" branch already retained it and deleting the comment clause changed
  # nothing (round-3 finding M6). These two are not deeper. ---

  check_fixture 'a column-0 comment does not close the on: mapping' \
"$(printf 'KEY\tpaths\t4\nITEM\tpaths\ta/**\nKEY\tpaths\t8\nITEM\tpaths\tb/**')" \
'name: t
on:
  push:
    paths:
      - "a/**"
# a column-0 comment, halfway through the on: mapping
  pull_request:
    paths:
      - "b/**"
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
'

  check_fixture 'a comment dedented to the key indent does not close the block' \
"$(printf 'KEY\tpaths\t4\nITEM\tpaths\ta/**\nITEM\tpaths\tb/**')" \
'name: t
on:
  push:
    paths:
      - "a/**"
    # dedented to the key indent, still inside the sequence
      - "b/**"
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
'

  # --- Final-review additions for the level rule (round-3 findings F1 and F2). ---

  check_fixture 'a workflow input NAMED paths yields no records' \
"" \
'name: t
on:
  workflow_dispatch:
    inputs:
      paths:
        description: which paths to build
        required: false
        type: string
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
'

  check_fixture 'a level-2 paths: alongside a level-3 input named paths still parses' \
"$(printf 'KEY\tpaths\t9\nITEM\tpaths\ta/**')" \
'name: t
on:
  workflow_dispatch:
    inputs:
      paths:
        description: which paths to build
        type: string
  push:
    paths:
      - "a/**"
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
'

  check_fixture 'a flow-mapping event with paths emits KEY with no ITEMs' \
"$(printf 'KEY\tbranches\t3\nKEY\tpaths\t3')" \
'name: t
on:
  push: { branches: [main], paths: ["rz/**"] }
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
'

  check_fixture 'a flow-mapping event with paths-ignore emits KEY with no ITEMs' \
"$(printf 'KEY\tbranches\t3\nKEY\tpaths-ignore\t3')" \
'name: t
on:
  push: {branches: [main], paths-ignore: ["docs/**"]}
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
'

  check_fixture 'a flow-mapping event with only a branch filter still emits a KEY' \
"$(printf 'KEY\tbranches\t3')" \
'name: t
on:
  push: { branches: [main] }   # no paths filter here
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
'

  check_fixture 'a flow mapping on on: itself emits KEY with no ITEMs' \
"$(printf 'KEY\tbranches\t2\nKEY\tpaths\t2')" \
'name: t
on: {push: {branches: [main], paths: ["rz/**"]}}
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
'

  # --- Round-2 review additions: a quoted paths:/paths-ignore: key and a spaced `on :` are both
  # valid YAML actionlint accepts, and the bare-only regexes silently dropped them — no KEY
  # record, so checks 5/6 skipped the filter with no message at all (findings A and B). ---

  check_fixture 'a double-quoted "paths": key is recognized' \
"$(printf 'KEY\tpaths\t4\nITEM\tpaths\ta/**')" \
'name: t
on:
  push:
    "paths":
      - "a/**"
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
'

  check_fixture "a single-quoted 'paths-ignore': key is recognized" \
"$(printf 'KEY\tpaths-ignore\t4\nITEM\tpaths-ignore\tdocs/**')" \
'name: t
on:
  push:
    '"'"'paths-ignore'"'"':
      - "docs/**"
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
'

  check_fixture 'on : with a space before the colon still opens the block' \
"$(printf 'KEY\tpaths\t4\nITEM\tpaths\ta/**')" \
'name: t
on :
  push:
    paths:
      - "a/**"
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
'

  check_fixture '"on" : quoted with a space before the colon still opens the block' \
"$(printf 'KEY\tpaths\t4\nITEM\tpaths\ta/**')" \
'name: t
"on" :
  push:
    paths:
      - "a/**"
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
'

  check_fixture 'a quoted inline-flow "paths": [a, b] emits KEY with no ITEMs' \
"$(printf 'KEY\tpaths\t4')" \
'name: t
on:
  push:
    "paths": ["a/**", "b/**"]
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
'

  check_fixture 'a quoted key at input depth (inputs: { "paths": ... }) yields no records' \
"" \
'name: t
on:
  workflow_dispatch:
    inputs: { "paths": { type: string } }
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
'

  # --- Round-2 (second pass) review addition: flow_keys() scanned a flow scalar for a `paths`
  # token at ANY brace depth, unlike the depth-aware block-form rule above. A workflow input
  # legitimately named `paths` sitting under `inputs:` inside a flow-style `on: {...}` or a
  # flow-style event value therefore false-red check 6. Fixed by having flow_keys() track brace
  # depth and only count a `paths`/`paths-ignore` key at the caller's target depth (2 for the
  # top-level `on: {...}` scalar, 1 for a single event's own flow value) — mirroring, in flow
  # style, exactly the depth rule already enforced above for block style. ---

  check_fixture 'a workflow input NAMED paths inside a top-level on: flow mapping (unquoted key) yields no records' \
"" \
'name: t
on: { workflow_dispatch: { inputs: { paths: { type: string } } } }
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
'

  check_fixture 'a workflow input NAMED paths inside a top-level on: flow mapping (quoted key) yields no records' \
"" \
'name: t
on: { workflow_dispatch: { inputs: { "paths": { type: string } } } }
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
'

  check_fixture 'a genuine paths: filter inside a top-level on: flow mapping still emits KEY with no ITEMs' \
"$(printf 'KEY\tpaths\t2')" \
'name: t
on: { push: { paths: ["rz/**"] } }
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
'

  check_fixture 'a workflow input NAMED paths inside a level-1 event flow mapping (unquoted key) yields no records' \
"" \
'name: t
on:
  push: { inputs: { paths: x } }
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
'

  check_fixture 'a workflow input NAMED paths inside a level-1 event flow mapping (quoted key) yields no records' \
"" \
'name: t
on:
  push: { inputs: { "paths": x } }
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
'

  check_fixture 'a genuine paths: filter inside a level-1 event flow mapping still emits KEY with no ITEMs' \
"$(printf 'KEY\tpaths\t3')" \
'name: t
on:
  push: { paths: ["a/**"] }
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
'

  check_fixture 'a genuine quoted "paths": filter inside a level-1 event flow mapping is still recognized' \
"$(printf 'KEY\tpaths\t3')" \
'name: t
on:
  push: { "paths": ["a/**"] }
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
'

  return $rc
}

# ---------------------------------------------------------------------------------------------
# Check 5 (definitions) — every `paths:` glob must be expressible AND must match the tree.
#
# THIS is the check that closes the failure this gate was filed for; actionlint cannot see it.
# The verdict function is defined here, ahead of check 1, so `path_filter_self_test` below and
# the `--self-test` early exit can exercise it without running the linter. The production call
# site — which turns a non-`ok` verdict into a `fail` — is checks 5/6, further down.
#
# `git ls-files ':(glob)P'` is NOT a sound model of GitHub filter patterns, in both directions:
#
#   - Wildcard-free patterns take DIRECTORY-PREFIX semantics under git. ':(glob)rs' matches 320
#     tracked files; GitHub matches NOTHING (no file is named `rs`). A dropped '/**' is among the
#     likeliest hand-edits, so literals are required to be an EXACT tracked path, never a prefix.
#   - '**' differs. GitHub: "zero or more of any character", slash-crossing anywhere. git: only
#     crosses '/' as a whole path component. GitHub documents '**.js' as "all .js files in the
#     repository"; ':(glob)**.js' yields 0 — a false red on the ONLY required check.
#
# Hence a restricted vocabulary where both matchers agree for the forms in use here, and a
# LOUD rejection otherwise. Never a silently-wrong verdict in either direction. (One vocabulary
# form is NOT provably general: git collapses a leading '**/' to zero directories, so
# ':(glob)**/README.md' matches a root-level README.md, where GitHub's literal "zero or more of
# any character" reading of '**/' would not obviously agree. Nothing in this repo uses that
# form; flagged here rather than silently assumed sound.)
#
# `paths-ignore:` is deliberately EXCLUDED. For `paths:`, matching nothing kills the workflow;
# for `paths-ignore:`, matching nothing is a no-op and the dangerous direction is matching
# EVERYTHING. Requiring paths-ignore globs to match would add false-red surface while guarding
# the wrong end (spec §7, non-goal).
#
# SKIP_PATTERNS is the escape hatch of spec §6: a GitHub-valid pattern outside the vocabulary.
# Every entry needs a comment justifying it, same shape as deny.toml's license exceptions.
# ---------------------------------------------------------------------------------------------
SKIP_PATTERNS=(
  # (empty — add entries as "pattern"  # why, and what verifies it instead)
)

is_skipped() {
  local p="$1" s
  for s in ${SKIP_PATTERNS+"${SKIP_PATTERNS[@]}"}; do
    [ "$s" = "$p" ] && return 0
  done
  return 1
}

# 0 if every '**' in the pattern is a whole path component ('a/**', '**/b'), 1 if any '**' is
# embedded in a larger segment ('**.js', 'a**b') — where git and GitHub disagree.
globstars_are_components() {
  local seg
  while IFS= read -r seg; do
    case "$seg" in
      '**') ;;
      *'**'*) return 1 ;;
    esac
  done <<< "${1//\//$'\n'}"
  return 0
}

# 0 if $1 names an exactly-tracked file (NOT a directory prefix).
tracked_exact() {
  local p="$1" f
  while IFS= read -r -d '' f; do
    [ "$f" = "$p" ] && return 0
  done < <(git -c core.quotePath=false ls-files -z -- "$p" 2>/dev/null)
  return 1
}

# 0 if the pattern contains a '.', '..', or empty path segment: a leading './', an interior
# '/./' or '/../', or a doubled '//'. git's :(glob)/ls-files normalizes these away when
# resolving a pathspec (measured: './rs/**', 'rs/../rs/**' and 'rs//**' all match the same 320
# files as plain 'rs/**'); GitHub filter patterns match the literal path text and do not, so
# each of those forms is dead on GitHub while this gate would otherwise wave it through.
has_dotty_segment() {
  local seg
  while IFS= read -r seg; do
    case "$seg" in
      ''|'.'|'..') return 0 ;;
    esac
  done <<< "${1//\//$'\n'}"
  return 1
}

# The single source of truth for check 5's verdict on one pattern. Echoes exactly one stable
# token; every non-'ok' token has a user-facing message at the production call site, and every
# token has a fixture in path_filter_self_test:
#
#   ok | skipped | negated | rejected-charclass | rejected-charset | rejected-dotty
#   rejected-globstar | dead | not-exact
#
# Separated from the messages ON PURPOSE. As one function that both decided and printed, nothing
# could assert what it decides, and a mutation battery showed the whole of checks 5/6 could be
# neutered with the gate still exiting 0 (round-3 finding F4).
pattern_verdict() {
  local p="$1" n

  is_skipped "$p" && { echo 'skipped'; return; }

  # Negated entries are exclusions — requiring them to match a file would be wrong. They are
  # still COUNTED by check 6, which counts raw sequence items before any filtering, so an
  # all-negated block cannot hard-fail as "key with no items" — check 6 instead fails it as
  # "no positive pattern", a distinct and more specific verdict.
  case "$p" in '!'*) echo 'negated'; return ;; esac

  # '?' is "zero or one of the PRECEDING character" on GitHub but "any single character" in git;
  # '+' is "one or more of the preceding" on GitHub but a literal in git; '[]' is one alphanumeric
  # on GitHub but ranges/negation in git. All three would give a wrong verdict, so reject.
  #
  # Deliberately ABOVE the pathspec-injection guard below: that guard's character class also
  # rejects all four characters, so if it ran first this specific, actionable message would be
  # unreachable dead code and a pattern like GitHub's own documented '*.jsx?' would be told it
  # "contains characters this gate will not pass to git" — true, but not the actual reason, and
  # it gives the author nothing to act on.
  case "$p" in
    *'?'*|*'+'*|*'['*|*']'*) echo 'rejected-charclass'; return ;;
  esac

  # Pathspec-injection guard: a pattern starting with ':' would be read by git as pathspec
  # magic. The '--' separator and quoting are necessary but not sufficient. Anything outside
  # this conservative class is rejected rather than passed to git. Acts as the catch-all for
  # every remaining unsupported character, now that '?'/'+'/'[]' are handled above with their
  # own message.
  if ! printf '%s' "$p" | grep -qE '^[A-Za-z0-9._/*-]+$'; then
    echo 'rejected-charset'; return
  fi

  if has_dotty_segment "$p"; then
    echo 'rejected-dotty'; return
  fi

  if ! globstars_are_components "$p"; then
    echo 'rejected-globstar'; return
  fi

  case "$p" in
    *'*'*)
      n="$(git -c core.quotePath=false ls-files -- ":(glob)$p" 2>/dev/null | wc -l | tr -d ' ')"
      if [ "${n:-0}" -eq 0 ]; then
        echo 'dead'; return
      fi ;;
    *)
      if ! tracked_exact "$p"; then
        echo 'not-exact'; return
      fi ;;
  esac

  echo 'ok'
}

# ---------------------------------------------------------------------------------------------
# Check 5, branch half (definitions) — every `branches:` entry must resolve as a ref, or be
# skip-listed (SMA-540 D2).
#
# `branches: [mian]` is a valid glob that actionlint accepts, so the workflow simply stops
# running — the same silent, permanent failure `paths:` had, one key over.
#
# The ref namespace is refs/remotes/origin/* ONLY (D3). A workflow triggers on branches as they
# exist on GitHub, which is exactly that set; refs/heads/* is a developer's private branch set
# locally and just `main` (push) or nothing (PR) in CI — the one namespace guaranteed to disagree.
# Measured: actions/checkout at fetch-depth 0 fetches +refs/heads/*:refs/remotes/origin/* on both
# the push and pull_request paths, so CI and a fetched local checkout see the same set.
#
# `branches-ignore:` is deliberately EXCLUDED from resolution (D6), mirroring paths-ignore: a
# typo'd exclusion makes the workflow run MORE often, which is the fail-safe direction.
# ---------------------------------------------------------------------------------------------
BRANCH_SKIP=(
  # (empty — add entries as "branch-or-pattern"  # why, and what verifies it instead)
)

is_branch_skipped() {
  local b="$1" s
  for s in ${BRANCH_SKIP+"${BRANCH_SKIP[@]}"}; do
    [ "$s" = "$b" ] && return 0
  done
  return 1
}

# The remote-tracking branch names, read ONCE. `refname:lstrip=3` drops `refs/remotes/origin/`, so
# a nested name (feature/x) survives intact where `refname:short` would prefix it with `origin/`.
# origin/HEAD is a symref to the default branch, not a branch, and is filtered out so it cannot
# make a literal entry named `HEAD` resolve.
#
# Loaded from the two MAIN-SHELL entry points below rather than lazily inside branch_verdict:
# verdicts are computed in nested command substitutions, so a cache populated there would be
# discarded with the subshell and re-run git once per entry.
ORIGIN_REFS=''
ORIGIN_REFS_LOADED=0

load_origin_refs() {
  [ "$ORIGIN_REFS_LOADED" -eq 1 ] && return 0
  ORIGIN_REFS="$(git for-each-ref --format='%(refname:lstrip=3)' refs/remotes/origin/ 2>/dev/null \
    | grep -vx 'HEAD')"
  ORIGIN_REFS_LOADED=1
  return 0
}

origin_has() {
  load_origin_refs
  printf '%s\n' "$ORIGIN_REFS" | grep -qxF -- "$1"
}

# A sample of what DOES exist, for the unresolved message. A bare "did not resolve" is the same
# unhelpful-message problem the canary exists to avoid, one level down. $1, if given, is the
# unresolved entry itself.
#
# Two failure modes of a naive `head -8` on ORIGIN_REFS, fixed here:
#   - An empty ref list: `printf '%s\n' ""` emits a blank line, so a bare `head -8 | tr '\n' ' '`
#     would return a single space and the message would read "include: ." — printf '(none)'
#     instead, so an empty cache reads as empty, not as a truncated list.
#   - `head -8` on an alphabetically-sorted ORIGIN_REFS can, in a repo with many branches, omit
#     'main' — the one name a reader chasing an unresolved entry most needs to see — and has no
#     reason to prefer a near-match over an arbitrary alphabetical one. Ranked instead: 'main'
#     first (group 0), then anything sharing the entry's slash-prefix (group 1, the likeliest
#     near-match for a typo like 'release/1.x' vs 'release/1.0'), then everything else (group 2);
#     alphabetical within each group; capped at 8 total.
origin_candidates() {
  local entry="${1:-}" prefix
  load_origin_refs
  [ -n "$ORIGIN_REFS" ] || { printf '(none)'; return; }

  case "$entry" in
    */*) prefix="${entry%%/*}/" ;;
    *) prefix='' ;;
  esac

  printf '%s\n' "$ORIGIN_REFS" | awk -v prefix="$prefix" '
    $0 == "main"                           { print "0\t" $0; next }
    prefix != "" && index($0, prefix) == 1 { print "1\t" $0; next }
                                            { print "2\t" $0 }
  ' | sort -t $'\t' -k1,1 -k2,2 | cut -f2- | head -8 | tr '\n' ' ' | sed 's/ *$//'
}

# Exits 2. MAIN SHELL ONLY — called from the production call site and from
# branch_filter_self_test, never from inside a $( ), where it would exit only the subshell.
no_origin_main_infra() {
  infra "refs/remotes/origin/main does not resolve in this checkout, so no 'branches:' entry can
      be verified. This is an environment problem, not a workflow defect. Recover with the EXPLICIT
      refspec:
          git fetch origin +refs/heads/main:refs/remotes/origin/main
      A bare 'git fetch origin' is NOT enough and neither is 'git fetch origin main' — the case
      that lands you here is usually a --single-branch clone, whose remote.origin.fetch names only
      the branch it was cloned for, and the two-argument form updates FETCH_HEAD without ever
      writing a remote-tracking ref (both measured). If main was genuinely RENAMED, every
      branches: filter in this repo is now dead — update them and this canary together."
}

# The single source of truth for check 5's verdict on one branch entry. Echoes exactly one stable
# token; every non-'ok' token has a user-facing message at the production call site, and every
# token has a fixture in branch_filter_self_test. The vocabulary is deliberately DISJOINT from
# pattern_verdict's: PATTERN and BRANCH findings are separate record types and the call site
# dispatches on the record type, so a shared token would print the wrong message (D8).
branch_verdict() {
  local b="$1"

  is_branch_skipped "$b" && { echo 'skipped'; return; }

  # Exclusions are not resolved — requiring them to name a live branch would be wrong. They are
  # still COUNTED by check 6, so an all-negated block fails there with a specific verdict (L3).
  case "$b" in '!'*) echo 'negated'; return ;; esac

  # Glob metacharacters, tried FIRST and deliberately above check-ref-format (D4). GitHub reads
  # '*', '**', '?', '+' and '[]' as patterns, so the entry names a set rather than a branch and
  # cannot be resolved. Ordering is load-bearing twice over: check-ref-format would reject '*' and
  # '?' as illegal ref characters and report a true but useless reason; and it keeps a glob-shaped
  # entry from ever reaching the for-each-ref/grep -qxF lookup below. Unlike pattern_verdict, which
  # relies on an explicit charset allowlist because it hands the pattern to git ls-files ':(glob)',
  # that lookup does a fixed-string match (grep -F) — already immune to being misread as a pattern
  # — so this ordering is belt-and-braces there, not the sole guarantee.
  #
  # '+' counts as a glob even though it is LEGAL in a git ref name: GitHub reads it as "one or
  # more of the preceding character", so 'foo+' matches the branch 'foo', and a branch literally
  # named 'foo+' would otherwise resolve and yield a confidently wrong 'ok'.
  case "$b" in
    *'*'*|*'?'*|*'+'*|*'['*|*']'*) echo 'unverifiable'; return ;;
  esac

  # git's own validity rule, so this gate does not enumerate one: it catches '..', '~', '^', ':',
  # control characters and a trailing '.lock'.
  if ! git check-ref-format "refs/heads/$b" 2>/dev/null; then
    echo 'invalid-name'; return
  fi

  # The canary is LAZY (D7) — only an entry that has survived every filter above actually needs a
  # ref, so checks 1-6 report their own findings before this canary fires. Since SMA-542 that is
  # no longer the whole story for a FULL run: check 7 runs first and asserts the same origin/main
  # precondition unconditionally (branch_filter_self_test), so a checkout without the ref now
  # exits 2 BEFORE actionlint is invoked, and you lose the checks 1-6 findings you used to see.
  # Accepted: README.md gives the one-command recovery and the gate is ~4s standalone (measured;
  # see the cost table in README.md). Returned as a
  # TOKEN, not an infra call: this function always runs inside $( ), where exit 2 would kill only
  # the subshell.
  origin_has 'main' || { echo 'no-origin-main'; return; }

  origin_has "$b" && { echo 'ok'; return; }

  echo 'unresolved'
}

# ---------------------------------------------------------------------------------------------
# Check 6 (definitions) — every extracted filter KEY (`paths:`, `paths-ignore:`, `branches:`,
# `branches-ignore:`) must carry at least one sequence item, and a `paths:`/`branches:` KEY must
# also have at least one of those items be a POSITIVE (non-'!') pattern.
#
# Two distinct failures, two distinct messages, both keyed off the RAW item count (before the
# '!' filtering in pattern_verdict):
#
#   - RAW count is 0: the extractor found nothing to read. This is what converts an unsupported
#     YAML form — the inline flow `paths: [a, b]`, and the flow-mapping event `push: { paths: … }`
#     that the extractor deliberately does not parse — from a silent skip into a loud failure that
#     names the file. The difference between a limitation and a hole.
#   - RAW count is >0 but every item is '!'-negated: GitHub includes a changed file only when it
#     matches at least one POSITIVE pattern, so an all-negated `paths:` block can never match
#     anything — the trigger it guards is dead, silently, forever. Same failure class the gate
#     exists to catch, just spelled with '!' instead of a typo (round-1 finding 1).
#
# `paths-ignore:` is exempt from the second failure: an all-negated paths-ignore is a no-op, not
# a dead trigger — mirrors check 5's header, which excludes paths-ignore from matching entirely.
#
# scan_workflow_records consumes extractor records and emits one FINDING record per problem,
# TAB-separated, and NOTHING for a clean file:
#   PATTERN\t<verdict>\t<pattern>
#   KEY\t<no-items|all-negated>\t<kind>\t<lineno>\t<raw item count>
# Deciding here and printing at the call site is what lets path_filter_self_test assert the whole
# of checks 5 and 6 against a fixture table.
# ---------------------------------------------------------------------------------------------
scan_workflow_records() {
  local rec kind value verdict
  local key_kind='' key_line='' key_items=0 key_positive=0

  flush_key() {
    if [ -n "$key_kind" ] && [ "$key_items" -eq 0 ]; then
      printf 'KEY\tno-items\t%s\t%s\t%s\n' "$key_kind" "$key_line" "$key_items"
    elif { [ "$key_kind" = 'paths' ] || [ "$key_kind" = 'branches' ]; } \
      && [ "$key_items" -gt 0 ] && [ "$key_positive" -eq 0 ]; then
      printf 'KEY\tall-negated\t%s\t%s\t%s\n' "$key_kind" "$key_line" "$key_items"
    fi
  }

  while IFS=$'\t' read -r rec kind value; do
    case "$rec" in
      KEY)
        flush_key
        key_kind="$kind"; key_line="$value"; key_items=0; key_positive=0 ;;
      ITEM)
        key_items=$((key_items + 1))
        # COUNTING IS KIND-GENERIC; only the verdict dispatch below is kind-specific. This
        # increment used to live inside the paths-only guard, where a branches: block would
        # report key_positive=0 and fire 'all-negated' on all five real filters — redding the
        # only required check on a clean tree (SMA-540 D5).
        case "$value" in '!'*) ;; *) key_positive=$((key_positive + 1)) ;; esac
        case "$kind" in
          paths)
            verdict="$(pattern_verdict "$value")"
            case "$verdict" in
              ok|skipped|negated) ;;
              *) printf 'PATTERN\t%s\t%s\n' "$verdict" "$value" ;;
            esac ;;
          branches)
            # A separate record type, carrying the key's line number: PATTERN's shape stays
            # byte-identical for AC-3, the two verdict vocabularies cannot collide into the wrong
            # message, and a message can name WHICH of two identical `branches:` entries is wrong.
            verdict="$(branch_verdict "$value")"
            case "$verdict" in
              ok|skipped|negated) ;;
              *) printf 'BRANCH\t%s\t%s\t%s\n' "$verdict" "$value" "$key_line" ;;
            esac ;;
        esac ;;
    esac
  done <<< "$1"

  flush_key
}

# ---------------------------------------------------------------------------------------------
# Path-filter self-test (definition only — invoked unconditionally as part of check 7).
#
# The standing control for checks 5 and 6, the two that actually close the failure this gate was
# filed for. Before it existed, a mutation battery neutered pattern matching, exact-path
# checking, the dead-glob branch and the key flush one at a time and the gate still exited 0:
# the only thing exercising those code paths was the repo's own three workflow files, all of
# which are clean. Every vocabulary rule and both check-6 verdicts now have a fixture.
# ---------------------------------------------------------------------------------------------
path_filter_self_test() {
  local rc=0 quoted_key_tmp
  SELF_TESTS_RAN=$((SELF_TESTS_RAN + 1))

  expect_pattern() {
    local pattern="$1" expected="$2" got
    got="$(pattern_verdict "$pattern")"
    if [ "$got" != "$expected" ]; then
      fail "path-filter self-test: pattern_verdict '$pattern' returned '$got', expected
      '$expected'. Check 5 is not deciding what it is documented to decide."
      rc=1
    fi
  }

  expect_scan() {
    local name="$1" expected="$2" records="$3" got
    got="$(scan_workflow_records "$records")"
    if [ "$got" != "$expected" ]; then
      fail "path-filter self-test '$name' mismatch.
--- expected ---
$expected
--- actual ---
$got"
      rc=1
    fi
  }

  # Vocabulary and tree-matching verdicts. The 'ok'/'dead' pairs are asserted against the REAL
  # tracked tree, so they also prove `git ls-files` is being consulted at all.
  expect_pattern 'rs/**'          'ok'                  # a live directory glob
  expect_pattern 'rz/**'          'dead'                # the headline failure: matches nothing
  expect_pattern 'rs/Cargo.toml'  'ok'                  # an exact tracked literal
  expect_pattern 'rs'             'not-exact'           # a dropped '/**' — 320 files under git,
                                                        # nothing at all on GitHub
  expect_pattern '**.js'          'rejected-globstar'   # '**' embedded in a segment
  expect_pattern './rs/**'        'rejected-dotty'      # git normalizes './' away, GitHub does not
  expect_pattern '*.jsx?'         'rejected-charclass'  # '?' means different things
  expect_pattern 'a+/**'          'rejected-charclass'
  expect_pattern 'a[0-9]/**'      'rejected-charclass'
  expect_pattern ':(glob)rs/**'   'rejected-charset'    # pathspec-injection guard
  expect_pattern '!rs/docs/**'    'negated'             # exclusions are not required to match

  # Check 6: the two key-level verdicts, plus the clean cases that keep them honest.
  expect_scan 'an all-negated paths: block is a dead trigger' \
"$(printf 'KEY\tall-negated\tpaths\t7\t2')" \
"$(printf 'KEY\tpaths\t7\nITEM\tpaths\t!a/**\nITEM\tpaths\t!b/**')"

  expect_scan 'a mixed positive and negated block is clean' \
"" \
"$(printf 'KEY\tpaths\t7\nITEM\tpaths\trs/**\nITEM\tpaths\t!rs/docs/**')"

  expect_scan 'a KEY with no items (inline flow or flow mapping) is a failure' \
"$(printf 'KEY\tno-items\tpaths\t7\t0')" \
"$(printf 'KEY\tpaths\t7')"

  expect_scan 'a paths-ignore KEY with no items is also a failure' \
"$(printf 'KEY\tno-items\tpaths-ignore\t7\t0')" \
"$(printf 'KEY\tpaths-ignore\t7')"

  expect_scan 'an all-negated paths-ignore is a no-op, not a finding' \
"" \
"$(printf 'KEY\tpaths-ignore\t7\nITEM\tpaths-ignore\t!docs/**')"

  expect_scan 'a dead glob is reported per pattern, naming the pattern' \
"$(printf 'PATTERN\tdead\trz/**')" \
"$(printf 'KEY\tpaths\t7\nITEM\tpaths\trs/**\nITEM\tpaths\trz/**')"

  expect_scan 'every key in a file is flushed, not just the last' \
"$(printf 'KEY\tno-items\tpaths\t7\t0\nPATTERN\tdead\trz/**')" \
"$(printf 'KEY\tpaths\t7\nKEY\tpaths\t11\nITEM\tpaths\trz/**')"

  # Round-2 review addition: runs the REAL extractor (not a hand-built records table) against a
  # quoted "paths": key, then feeds its output through scan_workflow_records — proving the full
  # check-5/6 pipeline, not just pattern_verdict in isolation, now reports a dead glob that a
  # quoted key previously hid completely (finding A).
  quoted_key_tmp="$(mktemp)"
  printf '%s' 'name: t
on:
  push:
    "paths":
      - "rz/**"
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
' > "$quoted_key_tmp"
  expect_scan 'a dead glob under a quoted "paths": key is reported end-to-end' \
"$(printf 'PATTERN\tdead\trz/**')" \
"$(extract_filter_keys "$quoted_key_tmp")"
  rm -f "$quoted_key_tmp"

  return $rc
}

# ---------------------------------------------------------------------------------------------
# Branch-filter self-test (definition only — invoked unconditionally as part of check 7).
#
# The standing control for check 5's branch half. It carries BOTH directions of the control pair:
# a name that must resolve and one that must not. A table whose verdicts all fire cannot tell a
# working check from a stuck one (SMA-466), and SMA-525's finding F4 was that a one-off mutation
# battery is not a standing control.
# ---------------------------------------------------------------------------------------------
branch_filter_self_test() {
  local rc=0 saved_skip saved_origin_refs saved_origin_refs_loaded tmp
  SELF_TESTS_RAN=$((SELF_TESTS_RAN + 1))

  # This table asserts a real ref resolves, so it shares the canary's precondition. Asserted here
  # rather than left to a confusing per-fixture mismatch: --self-test still needs no actionlint
  # binary, but it does now need a git repo carrying origin/main.
  load_origin_refs
  origin_has 'main' || no_origin_main_infra

  expect_branch() {
    local entry="$1" expected="$2" got
    got="$(branch_verdict "$entry")"
    if [ "$got" != "$expected" ]; then
      fail "branch-filter self-test: branch_verdict '$entry' returned '$got', expected
      '$expected'. Check 5's branch half is not deciding what it is documented to decide."
      rc=1
    fi
  }

  # The control pair. 'main' is asserted against the REAL ref store, so it also proves the lookup
  # is being consulted at all rather than short-circuiting to 'ok'.
  expect_branch 'main'                        'ok'
  expect_branch 'mian-sma540-absent'          'unresolved'

  # Glob metacharacters — rejected BEFORE check-ref-format so the message names the real reason.
  expect_branch 'release/**'                  'unverifiable'
  expect_branch 'v1.?'                        'unverifiable'
  expect_branch 'a+b'                         'unverifiable'
  expect_branch 'rel[0-9]'                    'unverifiable'

  # git's own ref-name rule, for everything the glob test lets through.
  expect_branch 'has space'                   'invalid-name'
  expect_branch 'a..b'                        'invalid-name'

  # Exclusions are never resolved (L3).
  expect_branch '!main'                       'negated'

  # The documented escape hatch actually silences an entry. BRANCH_SKIP ships empty, so this
  # overrides it for one assertion and restores it — the ${arr+"${arr[@]}"} idiom is required
  # because a bare "${arr[@]}" on an empty array is an unbound-variable error under `set -u`.
  saved_skip=(${BRANCH_SKIP+"${BRANCH_SKIP[@]}"})
  BRANCH_SKIP=('release/**')
  expect_branch 'release/**'                  'skipped'
  BRANCH_SKIP=(${saved_skip+"${saved_skip[@]}"})

  # 'no-origin-main' is the one token this table cannot exercise under real refs — the table's
  # own precondition above already asserts origin/main resolves. Simulated instead by swapping
  # ORIGIN_REFS for a main-free list and forcing ORIGIN_REFS_LOADED so load_origin_refs treats the
  # cache as already populated and will not overwrite it. 'dev' is in that fake list — it would
  # otherwise resolve to 'ok' — so this proves the lazy canary still fires ahead of the per-entry
  # lookup, not merely that a name absent from everywhere reports something non-'ok'. Placed LAST
  # and restored immediately: a failure mid-fixture must not leave the cache poisoned for a row
  # above, which is why every other row in this table runs first.
  saved_origin_refs="$ORIGIN_REFS"
  saved_origin_refs_loaded="$ORIGIN_REFS_LOADED"
  ORIGIN_REFS="$(printf 'dev\nrelease/1.0')"
  ORIGIN_REFS_LOADED=1
  expect_branch 'dev'                         'no-origin-main'
  ORIGIN_REFS="$saved_origin_refs"
  ORIGIN_REFS_LOADED="$saved_origin_refs_loaded"

  expect_branch_scan() {
    local name="$1" expected="$2" records="$3" got
    got="$(scan_workflow_records "$records")"
    if [ "$got" != "$expected" ]; then
      fail "branch-filter self-test '$name' mismatch.
--- expected ---
$expected
--- actual ---
$got"
      rc=1
    fi
  }

  # Check 6, branch half. Without this fixture the all-negated widening has no standing control
  # and could be reverted silently — SMA-525 finding F4.
  expect_branch_scan 'an all-negated branches: block is a dead trigger' \
"$(printf 'KEY\tall-negated\tbranches\t5\t2')" \
"$(printf 'KEY\tbranches\t5\nITEM\tbranches\t!main\nITEM\tbranches\t!dev')"

  # The control that keeps the one above honest: a normal block must produce NOTHING. This is the
  # fixture that catches key_positive being left inside a paths-only guard, which would fire
  # all-negated on all five real filters.
  expect_branch_scan 'a branches block with a positive entry is clean' \
"" \
"$(printf 'KEY\tbranches\t5\nITEM\tbranches\tmain')"

  # Proves scan_workflow_records actually DISPATCHES branch items to branch_verdict, and that the
  # finding carries the key's line number so the message can say which filter.
  expect_branch_scan 'an unresolvable branch is reported with its key line' \
"$(printf 'BRANCH\tunresolved\tmian-sma540-absent\t5')" \
"$(printf 'KEY\tbranches\t5\nITEM\tbranches\tmain\nITEM\tbranches\tmian-sma540-absent')"

  # branches-ignore is counted but never resolved (D6) — a nonexistent branch here is a no-op.
  expect_branch_scan 'a branches-ignore entry is never resolved' \
"" \
"$(printf 'KEY\tbranches-ignore\t5\nITEM\tbranches-ignore\tmian-sma540-absent')"

  # ...but an unreadable branches-ignore is still a loud failure, not a silent skip.
  expect_branch_scan 'a branches-ignore KEY with no items is a failure' \
"$(printf 'KEY\tno-items\tbranches-ignore\t5\t0')" \
"$(printf 'KEY\tbranches-ignore\t5')"

  # End-to-end through the REAL extractor, not a hand-built records table — the only fixture that
  # proves the whole extractor -> scanner -> verdict pipeline reports a typo'd branch.
  tmp="$(mktemp)"
  printf '%s' 'name: t
on:
  push:
    branches:
      - mian-sma540-absent
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
' > "$tmp"
  expect_branch_scan 'a typod branch is reported end-to-end through the extractor' \
"$(printf 'BRANCH\tunresolved\tmian-sma540-absent\t4')" \
"$(extract_filter_keys "$tmp")"
  rm -f "$tmp"

  return $rc
}

# ---------------------------------------------------------------------------------------------
# Check 2 (definitions) — no actionlint config may neuter check 1.
#
# actionlint reads .github/actionlint.yaml, whose `paths:` map takes per-path `ignore:` regexes.
# A blanket `ignore: [".*"]` makes check 1 exit 0 on a workflow with an unknown runner label —
# VERIFIED. And the stdin fixtures of checks 3/4 are NOT suppressed by that config even when
# -stdin-filename names a matching path (also verified), so those self-tests cannot detect it.
# An explicit assertion is the only thing that can.
#
# An ALLOWLIST, not a blocklist. The earlier `grep '^[[:space:]]*ignore:'` was block-style only,
# so the whole linter could be switched off by a single flow-style line —
# `paths: {".github/workflows/**": {ignore: [".*"]}}` — with the gate still exiting 0 (round-3
# finding F3). `self-hosted-runner` is the one key this repo permits: the documented escape hatch
# (spec §6) for a new GitHub runner label the pinned binary does not know.
# ---------------------------------------------------------------------------------------------
CONFIG_ALLOWED_KEYS='self-hosted-runner'

# Echoes one verdict token per problem, and nothing for an acceptable config:
#   banned-ignore          an `ignore` key in ANY style (block, flow, sequence entry)
#   unknown-key <key>      a top-level key outside CONFIG_ALLOWED_KEYS
config_verdict() {
  local cfg="$1" q='["'"'"']?'

  if grep -qE "(^|[[:space:]{,-])${q}ignore${q}[[:space:]]*:" "$cfg"; then
    echo 'banned-ignore'
  fi

  awk -v allowed="$CONFIG_ALLOWED_KEYS" '
    { line = $0; sub(/\r$/, "", line) }
    line ~ /^[ \t]*$/             { next }
    line ~ /^[ \t]*#/             { next }
    line ~ /^(---|\.\.\.)[ \t]*$/ { next }
    line ~ /^[^ \t]/ {
      key = line
      sub(/:.*$/, "", key)
      gsub(/^["\047]|["\047]$/, "", key)
      if (key != allowed) print "unknown-key " key
    }
  ' "$cfg" | sort -u
}

config_self_test() {
  local rc=0
  SELF_TESTS_RAN=$((SELF_TESTS_RAN + 1))

  expect_config() {
    local name="$1" expected="$2" body="$3" tmp got
    tmp="$(mktemp)"
    printf '%s' "$body" > "$tmp"
    got="$(config_verdict "$tmp")"
    rm -f "$tmp"
    if [ "$got" != "$expected" ]; then
      fail "actionlint-config self-test '$name' mismatch.
--- expected ---
$expected
--- actual ---
$got"
      rc=1
    fi
  }

  expect_config 'the documented escape hatch is permitted' "" \
'self-hosted-runner:
  labels:
    - my-new-label
'

  expect_config 'a block-style ignore: is rejected' \
"$(printf 'banned-ignore\nunknown-key paths')" \
'paths:
  ".github/workflows/**":
    ignore:
      - ".*"
'

  expect_config 'a one-line flow-style ignore is rejected' \
"$(printf 'banned-ignore\nunknown-key paths')" \
'paths: {".github/workflows/**": {ignore: [".*"]}}
'

  expect_config 'a quoted ignore key is rejected' \
"$(printf 'banned-ignore\nunknown-key paths')" \
'paths: {"x": {"ignore": [".*"]}}
'

  expect_config 'an unknown top-level key is rejected even without ignore' \
'unknown-key paths' \
'paths:
  ".github/workflows/**":
    something-else: true
'

  return $rc
}

# ---------------------------------------------------------------------------------------------
# Check 8 (definitions) — ci.yml's `T=(…)` must still schedule the gate that guards `T`.
#
# Deleting `:affected-smoke` from that array stops repo:affected-smoke running, which in ONE edit
# removes ci_targets.py's C1-C5, all eight cascade cases, cargo_moon_parity's A1-A5 and
# assert_include_relations — every one of them green, because the thing that would complain is the
# thing that stopped running. It is unclosable from inside ci/affected-graph/ for the same reason
# SMA-542 exists, and this gate is the natural host: independently scheduled, and already reading
# every workflow file.
#
# ':actionlint' is DELIBERATELY NOT in the floor. ci_targets.py's C1 (check_forward) is a strict
# equality over T's repo partition, so dropping :actionlint already reds repo:affected-smoke.
# Asserting it HERE would be vacuous: the only run in which the entry is missing is the run in
# which this assertion does not execute (SMA-542 D8).
# ---------------------------------------------------------------------------------------------
T_FLOOR=(':affected-smoke')

# T_INVOCATION_ALLOWLIST — a WHITELIST, not another blacklist (SMA-542 CodeRabbit round 3, finding
# B). Three rounds running, a new spelling of "moon at effective command position" got past
# whatever precedent the `swallowed`/`wrapped` checks below had just learned to reject:
#   round 1  moon ci "${T[@]}" ... || true              (swallowed's own motivating case)
#   round 2  command moon ci "${T[@]}" ...  /  if moon ci "${T[@]}" ...; then :; fi
#   round 3  FOO=bar moon ci "${T[@]}" ... || true       (measured: full gate rc 0, NO verdict —
#            `swallowed` requires `moon` at column 0, `wrapped` requires a recognized wrapper
#            token at column 0; a bare shell assignment prefix is neither, and it is idiomatic in
#            CI scripts, so it is not a corner case)
# Enumerating what may PRECEDE `moon` is an open set — a blacklist can only ever list the forms
# already seen. So this inverts the question, exactly as CodeRabbit's round-1 review originally
# asked ("define and enforce the exact allowed `moon ci` and `moon run` invocation forms"): every
# line in ci.yml carrying the target-array expansion `"${T[@]}"` must match one of THESE strings
# EXACTLY — indentation included, nothing before or after. `invocation_allowlist_verdict` below is
# the enforcement; `swallowed`/`continued`/`wrapped`/`continue-on-error` stay in place for their
# more specific diagnostics ("you appended `|| true`" beats "does not match an allowlisted form").
# `invocation_allowlist_verdict` runs LAST, after `continued`/`swallowed`/`wrapped` have already had
# a chance to explain a given `moon` line: a line already reported by one of those is not ALSO
# reported here (see the verdict function for how "already reported" is decided). `continue-on-error`
# is orthogonal — it names a different line, the step's own key, not the invocation — so it neither
# feeds nor is fed by this suppression.
#
# These are copied VERBATIM from .github/workflows/ci.yml, indentation included — re-verify
# against the real file (`grep -n 'T\[@\]' .github/workflows/ci.yml`) before editing this array; do
# not hand-format a new entry. A genuinely new, reviewed invocation form is added here; there is
# deliberately no separate skip list — this array IS the reviewed exception mechanism.
T_INVOCATION_ALLOWLIST=(
  '            moon ci "${T[@]}" --base origin/main --include-relations'
  '            moon ci "${T[@]}" --base "$BEFORE" --include-relations'
  '            moon run "${T[@]}"'
)

# COE_SKIP is the escape hatch for a `continue-on-error:` line the verdict below cannot know is
# harmless — e.g. a step that runs strictly AFTER "moon ci (affected graph)", whose own
# success/failure can no longer un-run or un-honor gates that already finished. Same shape and
# same reason as SKIP_PATTERNS/BRANCH_SKIP above (spec §6): the verdict is deliberately syntactic,
# like `swallowed`, rather than parsing YAML step-block boundaries to work out "which step is
# this key under" for itself — this file's own history (flow vs block style) is exactly where
# that kind of parsing has hidden real bugs.
#
# Keyed by BOTH the 1-based line number AND the exact matched line text — entries have the form
# "<lineno>:<text>", reconstructing the raw `grep -n` record the reader loop below already splits
# into those two pieces (`IFS=: read -r lineno text`). This is the only positionally-keyed skip
# list in this file — SKIP_PATTERNS and BRANCH_SKIP key by content alone and are drift-immune by
# construction. A bare line number is not: an edit that shifts the file leaves a stale entry
# sitting on whatever line now occupies its old number, silently absorbing a DIFFERENT
# continue-on-error occurrence that happens to land there — a fail-open in the one check whose
# purpose is closing fail-opens. Pinning the text too means a shifted entry simply stops matching
# and reds instead of silently skipping. Every entry needs a comment naming what verifies the
# suppressed step's own failure instead (SMA-542 task 4b review, I2; keyed by content, SMA-542
# follow-up — it shipped empty, so this was the cheapest point to close it).
COE_SKIP=(
  # (empty — add entries as "<lineno>:<exact text of the matched line, leading blanks included>"
  # # why, and what verifies it instead)
)

is_coe_skipped() {
  local key="$1" s
  for s in ${COE_SKIP+"${COE_SKIP[@]}"}; do
    [ "$s" = "$key" ] && return 0
  done
  return 1
}

# SWALLOWED_SKIP is the escape hatch for a `moon` command line the verdict below cannot know is
# harmless. `swallowed` is file-wide over ci.yml and fires on ANY `moon`-prefixed line carrying a
# trailing `||`/`&&`/`;`/`|` — deliberately, for the same reason COE_SKIP's key is over-inclusive
# rather than parsed: telling "the invocation guarding T" apart from a different, harmless `moon`
# line (a diagnostic `moon run x | tee log` in an unrelated job, say) needs YAML step/job-boundary
# parsing, which is exactly where this file's real bugs have lived (flow vs block style). Without
# an escape hatch, that future line has no remediation but "remove the tail" even when the tail is
# fine (SMA-542 review M6). Same "<lineno>:<exact text>" format as COE_SKIP and the same reasoning
# for keying by both: a bare line number would silently absorb whatever line drifts onto it after
# an edit; pinning the text too makes a shifted entry stop matching and red instead. Ships empty.
#
# Also the escape hatch for `wrapped` (PR 150 CodeRabbit fix), not a fourth list: a wrapped and a
# bare-`moon` swallowed line are the same underlying problem — "this check cannot confirm the
# invocation's exit status propagates" — spelled two different ways depending on where the wrapper
# sits, so one skip list keyed the same way (lineno + exact text) covers both.
SWALLOWED_SKIP=(
  # (empty — add entries as "<lineno>:<exact text of the matched line, leading blanks included>"
  # # why, and what verifies the suppressed invocation's own failure instead)
)

is_swallowed_skipped() {
  local key="$1" s
  for s in ${SWALLOWED_SKIP+"${SWALLOWED_SKIP[@]}"}; do
    [ "$s" = "$key" ] && return 0
  done
  return 1
}

# Echoes one verdict token per problem, and nothing for an acceptable file:
#   no-file                      the workflow does not exist
#   no-array                     zero, or more than one, single-line T=( … )
#   missing <entry>              the array parsed; <entry> is not among its tokens
#   continued <lineno>           a `moon` command line is continued onto another physical line
#   swallowed <lineno>           a `moon` command line discards its own exit status
#   block-swallowed <lineno>     a block terminator (`fi`/`done`/`}`) discards its OWN exit status
#   wrapped <lineno>             a `moon ci`/`moon run` invocation sits behind a known command
#                                 wrapper (command/env/time/eval/exec/if/while/until/!) instead of
#                                 at command position, so propagation cannot be confirmed
#   continue-on-error <lineno>   a step's continue-on-error value is not literally `false`
ci_target_floor_verdict() {
  local f="$1" arrays body tok w found lineno text q value

  [ -e "$f" ] || { echo 'no-file'; return; }

  # Anchored like ci_targets.py's T_ARRAY_RE, not a bare `T=(` — which would also match `EXPECT=(`.
  # Zero or two matches is a FAILURE, never a skip: an array reformatted across lines is exactly
  # the condition under which this check would otherwise stop asserting anything.
  arrays="$(grep -cE '^[[:blank:]]*T=\(.*\)[[:blank:]]*$' "$f")"
  # Hardened exactly like $defs (run_self_tests) and $n (selftest_mutation_battery) below: without
  # this, a $arrays that comes back empty (grep itself failing rather than matching zero times)
  # makes `[ "$arrays" -ne 1 ]` exit 2 under `set -uo pipefail`'s no-`set -e`, which the `if` reads
  # as false — skipping the 'no-array' report and falling through to parse a body from zero matches
  # (SMA-542 review M5).
  case "$arrays" in ''|*[!0-9]*) echo 'no-array'; return ;; esac
  if [ "$arrays" -ne 1 ]; then
    echo 'no-array'
    return
  fi
  body="$(sed -nE 's/^[[:blank:]]*T=\((.*)\)[[:blank:]]*$/\1/p' "$f")"

  # set -f for the unquoted `$body` expansion below: a bare `for w in $body` word-splits AND
  # glob-expands, so a future T_FLOOR (or T=(…)) entry containing `[`, `*` or `?` would compare
  # against whatever that glob happened to match in cwd instead of the literal token. No entry
  # today has a glob metacharacter, so this is currently a no-op — restored right after the loop,
  # not left disabled for the rest of the function.
  set -f
  for tok in "${T_FLOOR[@]}"; do
    found=0
    # Whole-token comparison: ':affected-smoke' is a prefix of ':affected-smoke-disabled', so a
    # substring test would accept a renamed-away gate.
    for w in $body; do
      if [ "$w" = "$tok" ]; then found=1; break; fi
    done
    [ "$found" -eq 1 ] || echo "missing $tok"
  done
  set +f

  # D14 — `|| true` on the moon line silences every gate in T while leaving T itself perfectly
  # correct: C1/C2/C3 pass, C5's expansion test passes, and `set -euo pipefail` does not help
  # because the step exits 0. Complementary to C5, which asserts T is HANDED OVER; this asserts
  # the result is PROPAGATED. It lives here because this is the half that survives
  # repo:affected-smoke being silenced.
  #
  # A backslash-continued invocation is checked FIRST and reported as its own verdict, never as
  # 'swallowed' (SMA-542 fix-wave finding I2 — distinct from the earlier task-4b-review I1/I2
  # labels elsewhere in this file). `grep -n` returns only the FIRST physical line of a wrapped
  # command, so:
  #   moon ci "${T[@]}" \
  #     --base origin/main \
  #     --include-relations || true
  # matches the loop below on a line reading `moon ci "${T[@]}" \` — no `||`/`&&`/`;`/`|` in THAT
  # text, so the real tail two lines down was invisible and the verdict came back empty (measured).
  # Reporting this as 'swallowed' would be a misdiagnosis: the tail might not even be there, and
  # the fix is not "remove the tail" but "put the invocation back on one line" — the same demand
  # `no-array` already makes of `T=( … )` itself. This check cannot see past a continuation either
  # way, so it rejects the shape outright rather than guessing what follows it.
  while IFS=: read -r lineno text; do
    case "$text" in
      *'\') echo "continued $lineno" ;;
      *'||'*|*'&&'*|*';'*|*'|'*)
        is_swallowed_skipped "$lineno:$text" || echo "swallowed $lineno" ;;
    esac
  done < <(grep -nE '^[[:blank:]]*moon[[:blank:]]' "$f")

  # 'block-swallowed' — a discarded exit status on the LINE THAT CLOSES a block (`fi`/`done`/`}`)
  # is just as silent as one on the `moon` line itself, and neither T_INVOCATION_ALLOWLIST nor
  # anything above sees it: T_INVOCATION_ALLOWLIST only scans lines carrying the literal
  # `"${T[@]}"` substring, and a terminator line carries neither that nor `moon` nor a wrapper
  # token (independent review of PR 150 round 3, finding I4). Two measured cases:
  #   fi || true          # ci.yml's real 'fi' closing the if/elif/else, tail appended
  #   { … } || true       # the WHOLE if/fi wrapped in a brace group; the invocation lines inside
  #                        # stay byte-identical to T_INVOCATION_ALLOWLIST, so check 8b sees
  #                        # nothing wrong either — only the closing '}' line carries the tail
  # A CLOSED, BOUNDED vocabulary, same convention as 'wrapped' above: only `fi`, `done` and `}` are
  # recognized, each required to be the line's FIRST token (immediately followed by a blank, `;`,
  # `&`, `|`, or end of line — so `fill`/`donetime`/etc. cannot false-match). Whether the tail
  # actually belongs to the terminator or to something appended after it on the same physical line
  # is not distinguished, deliberately over-inclusive in the same direction 'swallowed' already is
  # (a bare `fi;` next-statement idiom would also fire) — SWALLOWED_SKIP is the same documented
  # escape hatch for a case this check cannot itself know is harmless. Reported as its own verdict,
  # not folded into 'swallowed', because that verdict's message specifically says "runs 'moon'" —
  # which a `fi`/`done`/`}` line does not.
  while IFS=: read -r lineno text; do
    case "$text" in
      *'||'*|*'&&'*|*';'*|*'|'*)
        is_swallowed_skipped "$lineno:$text" || echo "block-swallowed $lineno" ;;
    esac
  done < <(grep -nE '^[[:blank:]]*(fi|done|\})([[:blank:]]|[;&|]|$)' "$f")

  # 'wrapped' — a `moon ci`/`moon run` invocation hidden behind a known command wrapper on the SAME
  # physical line evades the swallowed loop above BY CONSTRUCTION: that loop's grep requires `moon`
  # to be the line's first token, and none of these wrapper forms puts it there. Two motivating
  # cases, both measured to produce NO verdict at all under the loop above (CodeRabbit, PR 150):
  #   command moon ci "${T[@]}" --base origin/main --include-relations || true
  #   if moon ci "${T[@]}" --base origin/main --include-relations; then :; fi
  # The first is 'swallowed' in every way that matters, just spelled so the anchor misses it. The
  # second has no `||`/`&&`/`;`/`|` TAIL at all — its `;` belongs to the `if` syntax, not a
  # discarded exit status — yet it is exactly as silent: a failing `if` CONDITION does not fail the
  # `if` STATEMENT, so the step exits 0 either way.
  #
  # A CLOSED, ENUMERATED vocabulary, matching this file's own convention for a restricted-and-loud
  # vocabulary (pattern_verdict's SKIP_PATTERNS, branch_verdict's BRANCH_SKIP): reject the shape
  # outright rather than try to guess whether a given wrapper actually swallows the exit status.
  # `command`/`env`/`time`/`eval`/`exec`/`if`/`while`/`until`/`!` are the forms most likely to
  # appear in a CI script; a wrapper outside this list (a shell function, `sudo`, `nice`, ...) is
  # invisible to it — the same residual `swallowed` and `continued` already carry for constructs
  # outside THEIR vocabulary. Reported as its own verdict, never folded into 'swallowed': the fix is
  # always "put the invocation back at command position on one physical line", which for the `if`
  # case is not "remove the tail" (there may be none).
  #
  # `moon` MUST BE THE NEXT COMMAND WORD after the wrapper, not merely present somewhere on the
  # line (CodeRabbit, PR 150 round 2 — a false positive, measured):
  #   if test -n "$X"; then echo "moon ci failed"; fi
  # begins with `if` and the STRING "moon ci" appears later on the line, but no `moon` is ever
  # executed — this is the exact "any line containing 'moon ci'" trap already deliberately avoided
  # at file scope (ci_targets.py's MOON_CI_LINE_RE / this file's own `moon`-at-command-position
  # anchor above), reappearing one level down inside a wrapper-prefixed line. So the pattern below
  # requires `moon` immediately after the leading wrapper, tolerating only what genuinely occurs
  # between them: a CHAIN of further wrapper tokens (`command env moon ci …`), a negation
  # (`if ! moon ci …`), or a `VAR=value` assignment (`env FOO=bar moon ci …`) — any number of these,
  # each blank-separated — but nothing else. `if test -n "$X"; then …` fails to match: after `if`,
  # `test` is neither a wrapper token, a negation, nor an assignment, so the chain cannot reach
  # `moon` and the whole anchored pattern does not match that line at all.
  #
  # ONE ERE (bash 3.2's `grep -E` is POSIX ERE with no lookahead, but no lookahead is needed once
  # the whole chain is spelled out as `(glue)*` before the required `moon`). Anchored at column 0,
  # mirroring the `moon` anchor on the swallowed loop just above. `moon setup`/`moon sync` are not
  # gated by T and stay out of scope via the trailing `(ci|run)`.
  #
  # RESIDUAL, same shape as the vocabulary residual above: a wrapper reached through anything other
  # than whitespace — `true && moon ci …`, a `case` arm, a custom shell function — is invisible to
  # this check, same as it would be to `swallowed`'s own vocabulary. Not attempting general
  # reachability analysis here either (ci_targets.py's `ACTIONLINT_SH_CALL_SITES` comment makes the
  # same call for the same reason).
  while IFS=: read -r lineno text; do
    is_swallowed_skipped "$lineno:$text" || echo "wrapped $lineno"
  done < <(grep -nE '^[[:blank:]]*(command|env|time|eval|exec|if|while|until|!)([[:blank:]]+(command|env|time|eval|exec|if|while|until|!|[A-Za-z_][A-Za-z0-9_]*=[^[:blank:]]*))*[[:blank:]]+moon[[:blank:]]+(ci|run)([[:blank:]]|$)' "$f")

  # continue-on-error: — the third spelling of D14's idea, at step rather than command-line
  # granularity, and deliberately as syntactic and over-inclusive as `swallowed` above (SMA-542
  # task 4b review, I1): GitHub Actions accepts far more spellings of "suppress this step" than
  # the bare word `true` (`True`, `TRUE`, `yes`, `on`, a `${{ }}` expression, ...), so requiring an
  # exact `true` left every one of those silently unguarded — the exact hole this check exists to
  # close. `false` is the one spelling GitHub Actions itself treats as "do not suppress this
  # step", so it is the only value this check leaves silent; every other value fires. This is
  # file-wide rather than scoped to the step that runs `moon` (I2): telling that step apart from
  # an unrelated later one needs YAML step-block-boundary parsing, which is exactly where this
  # file's real bugs have lived (flow vs block style) — COE_SKIP above is the documented way to
  # silence a line this check cannot itself know is harmless. Key matched at any indentation and
  # quote style, same spirit as config_verdict's `$q` above; a leading `#` (comment) can never
  # match because the anchored key immediately follows the indentation, with no room for a
  # comment marker before it.
  q='["'"'"']?'
  while IFS=: read -r lineno text; do
    value="${text#*:}"                             # drop through the key:value colon
    value="${value%%#*}"                           # drop a trailing comment
    value="${value#"${value%%[![:blank:]]*}"}"     # trim leading blanks
    value="${value%"${value##*[![:blank:]]}"}"     # trim trailing blanks
    case "$value" in
      false) : ;;
      *) is_coe_skipped "$lineno:$text" || echo "continue-on-error $lineno" ;;
    esac
  done < <(grep -nE "^[[:blank:]]*${q}continue-on-error${q}[[:blank:]]*:" "$f")
}

# ---------------------------------------------------------------------------------------------
# invocation_allowlist_verdict — the enforcement for T_INVOCATION_ALLOWLIST above (SMA-542
# CodeRabbit round 3, finding B). A SEPARATE function and a SEPARATE self-test table rather than
# folded into ci_target_floor_verdict/ci_target_floor_self_test above: that table's ~25 fixtures
# are small, synthetic, hand-indented bodies built to exercise ONE verdict at a time, and this
# check's two rules (an exact allowlist match, and a count pinned to
# `${#T_INVOCATION_ALLOWLIST[@]}`) are properties of the file as a WHOLE — every one of those
# existing fixtures would need padding out to exactly 3 correctly-indented matching lines just to
# stop tripping `invocation-count`, coupling ~25 unrelated rows to this array's literal contents
# for no reason. Splitting it out keeps both tables honest instead.
#
# $1 is the workflow file. $2 (optional) is a SPACE-SEPARATED list of line numbers already
# explained by a more specific verdict (`continued`/`swallowed`/`wrapped`) — built at the
# production call site from ci_target_floor_verdict's own output, so "you appended `|| true`" is
# reported once, not twice under two different names. Self-tests below pass a synthetic list
# directly, to prove the suppression itself independent of ci_target_floor_verdict's output.
#
# Echoes one verdict token per problem, and nothing for an acceptable file:
#   no-file                     the workflow does not exist
#   not-allowlisted <lineno>    a `"${T[@]}"`-bearing line matches none of T_INVOCATION_ALLOWLIST
#                                exactly, and was not already explained by a more specific verdict
#   invocation-count <n>        <n> lines carry `"${T[@]}"`, not the expected
#                                `${#T_INVOCATION_ALLOWLIST[@]}` — checked regardless of whether
#                                every individual line matches, so a DELETED invocation (or one
#                                quietly subsetted to `"${T[@]:0:5}"`, which no longer contains the
#                                literal expansion at all) still reds even though no single
#                                surviving line looks wrong on its own
#   count-unreadable             `grep -c` itself failed (not "matched zero", the command did not
#                                run cleanly), so the count cannot be trusted either way
# ---------------------------------------------------------------------------------------------

# The single source of truth for turning a captured '"${T[@]}"' occurrence count into a verdict
# token (or nothing, for a count that already matches the expected floor). Extracted out of
# invocation_allowlist_verdict below so a self-test can drive it directly with synthetic,
# already-malformed count strings instead of trying to force a real `grep -c` read failure via a
# directory in place of the file — that trick is NOT portable. BSD grep (macOS) fails outright
# reading a directory and writes nothing to stdout, so $n comes back empty and lands on
# 'count-unreadable' below; GNU grep (Linux, what CI actually runs) instead prints a literal '0'
# for the exact same input — a well-formed-but-wrong count that lands on 'invocation-count 0'
# instead, a DIFFERENT verdict than the fixture that used to live here asserted (reds Linux CI,
# passes macOS: SMA-542, CodeRabbit review of PR 150). A malformed synthetic string never touches
# grep at all, so it cannot disagree by platform.
#
# $1 the captured count text (possibly malformed/non-numeric), $2 the expected count.
invocation_allowlist_count_verdict() {
  local n="$1" expected="$2"
  case "$n" in ''|*[!0-9]*) echo 'count-unreadable'; return ;; esac
  if [ "$n" -ne "$expected" ]; then
    echo "invocation-count $n"
  fi
}

invocation_allowlist_verdict() {
  local f="$1" skip=" ${2:-} " lineno text n matched allowed count_verdict

  [ -e "$f" ] || { echo 'no-file'; return; }

  # NOT `infra` (CodeRabbit round 4, finding F1) — this function is invoked at the production call
  # site as `done < <(invocation_allowlist_verdict ...)`, so it runs inside the process
  # substitution's OWN subshell. `infra`'s `exit 2` would exit only that subshell; the parent
  # `while` simply reads EOF, FAILED never gets set, and the gate finishes rc 0 with nothing but a
  # stderr line — measured. A verdict function reachable from `< <(...)` must ECHO a token and
  # `return` instead, exactly like `ci_target_floor_verdict`'s own `$arrays` hardening just above
  # (`case "$arrays" in ''|*[!0-9]*) echo 'no-array'; return ;; esac`) — the call site is what
  # turns `count-unreadable` into an actual `infra` exit, from the MAIN shell, where it works.
  # Without this, a $n that comes back empty (grep itself failing rather than matching zero times)
  # makes the numeric comparison below exit 2 under `set -uo pipefail`'s no-`set -e`, which the
  # `if` reads as false — skipping the 'invocation-count' report and falling through to a false
  # 'ok' the same way `$arrays` would (SMA-542 CodeRabbit round 4 finding F1).
  n="$(grep -cF -- '"${T[@]}"' "$f")"
  count_verdict="$(invocation_allowlist_count_verdict "$n" "${#T_INVOCATION_ALLOWLIST[@]}")"
  if [ "$count_verdict" = 'count-unreadable' ]; then
    echo "$count_verdict"
    return
  fi
  [ -n "$count_verdict" ] && echo "$count_verdict"

  while IFS=: read -r lineno text; do
    case "$skip" in
      *" $lineno "*) continue ;;
    esac
    matched=0
    for allowed in "${T_INVOCATION_ALLOWLIST[@]}"; do
      if [ "$text" = "$allowed" ]; then
        matched=1
        break
      fi
    done
    [ "$matched" -eq 1 ] || echo "not-allowlisted $lineno"
  done < <(grep -nF -- '"${T[@]}"' "$f")
}

# ---------------------------------------------------------------------------------------------
# T_AFFECTED_GRAPH_CALL_SITES / affected_graph_wiring_verdict — Check 8c (SMA-542 residual
# closure, PR 150 follow-up). This file's own README named the gap as L6: check 8 above pins only
# `:affected-smoke`'s SCHEDULING — its presence in ci.yml's `T=(…)` — never the two lines inside
# `ci/affected-graph/run.sh` that actually INVOKE `ci_targets.py`, the file that in turn pins THIS
# file's own call sites (ACTIONLINT_SH_CALL_SITES, in ci_targets.py). Those two lines were pinned
# only by `RUN_SH_CALL_SITES`, which lives INSIDE ci_targets.py — so deleting
# `assert_ci_targets || SUITE_RC=1` from run.sh removed ci_targets.py's own C1-C5 AND its
# self-invocation check on THIS file in one edit, and nothing inside ci/affected-graph/ could
# notice its own deletion (ci_targets.py's RUN_SH_CALL_SITES comment used to say exactly that:
# "remains self-guarded"). This check closes it from the other side: independently scheduled
# (`inputs: ['**/*']` on repo:actionlint), so it survives exactly that deletion.
#
# Copied VERBATIM from ci_targets.py's RUN_SH_CALL_SITES, including the `|| RC=1` propagation
# suffix on each — that suffix is as load-bearing here as it is there: a CodeRabbit round on this
# very branch found that matching the command prefix alone let `--self-test || true` masquerade
# as wired, since the self-test still RUNS but its failure is silently swallowed and the negative
# control can no longer report red. Matching the whole suffixed string closes that the same way
# RUN_SH_CALL_SITES does. Not sourced/imported — bash has no cross-script constant sharing here —
# so a deliberate edit to either copy must be mirrored in the other; that drift is exactly what
# neither gate can catch on its own, the same cost T_FLOOR/T_INVOCATION_ALLOWLIST/
# SELF_TASK_EXPECTED_GLOBS already accept as the price of not being the sole judge of your own
# configuration.
T_AFFECTED_GRAPH_CALL_SITES=(
  'assert_ci_targets || SUITE_RC=1'
  '"$HERE/ci_targets.py" --self-test || NEG_RC=1'
)

# Echoes one verdict token per problem, and nothing for a wired file:
#   no-file          ci/affected-graph/run.sh does not exist, or is not a readable regular file
#   missing <site>    the exact substring named is absent — see T_AFFECTED_GRAPH_CALL_SITES above
#
# Substring-matched (grep -F), exactly like ci_targets.py's own RUN_SH_CALL_SITES and for the same
# reason: `assert_ci_targets` is also the bare name of its own function definition
# (`assert_ci_targets() {`, ci/affected-graph/run.sh) and of a self-test fixture's synthetic
# definition below, so a name-only match would survive the CALL being deleted while the
# DEFINITION remains — the same substring trap ACTIONLINT_SH_CALL_SITES documents from the other
# direction.
#
# `[ -f "$f" ] && [ -r "$f" ]`, not `[ -e "$f" ]`: a directory in place of the file must report
# 'no-file' too, not silently fall through to two 'missing' rows that misdescribe a renamed
# *directory* as "both call sites deleted" (mirrors invocation_allowlist_verdict's own file check
# above, one cluster up).
#
# Never `infra` from inside this function: it is invoked at the production call site below as
# `done < <(affected_graph_wiring_verdict ...)`, so it runs inside that process substitution's OWN
# subshell — `infra`'s `exit 2` would exit only the subshell, FAILED would never be set, and the
# gate would finish rc 0 having asserted nothing. Exactly the bug CodeRabbit found on
# invocation_allowlist_verdict two rounds ago on this same branch (see the comment above that
# function): a verdict function reachable from `< <(...)` must echo a token and `return`, always.
affected_graph_wiring_verdict() {
  local f="$1" site

  [ -f "$f" ] && [ -r "$f" ] || { echo 'no-file'; return; }

  for site in "${T_AFFECTED_GRAPH_CALL_SITES[@]}"; do
    grep -qF -- "$site" "$f" || echo "missing $site"
  done
}

# ---------------------------------------------------------------------------------------------
# T_AFFECTED_SMOKE_* / affected_smoke_block_verdict — Check 8e (SMA-572 / SMA-573).
#
# Every pin in ci/affected-graph/ci_targets.py — RUN_SH_CALL_SITES, SELF_SCHEDULED_GATES,
# ACTIONLINT_SH_CALL_SITES, RELEASE_PARITY_SH_CALL_SITES — fires only when repo:affected-smoke is
# SCHEDULED, and until this check nothing pinned the `inputs` list that schedules it. Removing
# `- 'moon.yml'` is self-concealing: the removal is itself a root-moon.yml edit, and afterwards
# the task's remaining globs do not match that file (`*/moon.yml` matches rs/moon.yml, not
# moon.yml; `.moon/**/*` does not match it either), so the removal PR does not schedule the gate
# and every later PR can delete a pinned line with nothing red. MEASURED at moon 2.5.3: the root
# moon.yml is NOT an implicit input to the repo project's own tasks (repo:deny resolves to
# inputFiles ['rs/Cargo.lock','rs/deny.toml'] and no moon.yml), so there is no fallback.
#
# repo:input-liveness does not close this: it asserts a DECLARED glob still matches a tracked
# file, not that a required glob is still DECLARED.
#
# This lives HERE, not in ci_targets.py, because that file runs INSIDE repo:affected-smoke and
# would be the sole judge of its own reachability. repo:actionlint's inputs are ['**/*'] —
# MEASURED to match dot-prefixed paths, so a .github/-only PR does schedule it — and that
# premise is itself now pinned, from ci_targets.py's SELF_TASK_EXPECTED_GLOBS["actionlint"].
#
# CONTAINMENT, not equality: the list is twenty entries and legitimately grows every time a
# gate keys on a new directory, so an exact match would red on every honest addition. The set
# below is the WHOLE current list rather than a judged subset — a floor, not a judgement call.
# The first design draft picked seven by a stated principle and an adversarial review showed the
# principle pulls in most of the rest anyway: cargo_moon_parity.py reads every crate Cargo.toml
# from disk (so rs/**/Cargo.toml qualifies), and a crate's own moon.yml is not an input to its own
# tasks (SMA-528 F5), which is exactly WHY this gate must key on the four */moon.yml families —
# drop rs/crates/*/*/moon.yml and a PR changing only a crate's dependsOn or fileGroups.upstreams
# serves a cached PASS on the very edit A5/A6 exist to catch. Making it the whole list removes the
# "is this one load-bearing?" question the next reviewer would otherwise have to re-litigate.
T_AFFECTED_SMOKE_REQUIRED_INPUTS=(
  'ci/affected-graph/**/*'
  '.github/workflows/ci.yml'
  '.moon/**/*'
  'moon.yml'
  '*/moon.yml'
  'rs/crates/*/*/moon.yml'
  'py/packages/*/moon.yml'
  'ts/packages/*/moon.yml'
  'ts/apps/*/moon.yml'
  'rs/**/Cargo.toml'
  'rs/Dockerfile'
  'py/packages/*/pyproject.toml'
  'rs/crates/*/*/pyproject.toml'
  'rs/crates/*/*/package.json'
  'ts/packages/*/package.json'
  'ts/apps/*/package.json'
  'ci/actionlint/**/*'
  'ci/release-parity/**/*'
  'ci/workflow-credentials/**/*'
  'ci/**/*'
  # SMA-603 — floors the input that makes RELEASE_PLAN_SH_CALL_SITES reachable. Without it the
  # PR deleting those nine lines is exactly the PR that does not schedule repo:affected-smoke.
  'ci/release-plan/**/*'
  'CLAUDE.md'
  '.prototools'
)

# SMA-601 — check 8f. The lockfile-integrity step is a plain ci.yml step, not a Moon task, so
# none of ci_targets.py's registries can see it: no T entry, no SELF_SCHEDULED_GATES row. The
# codegen-drift step has the same exposure and carries no pin; this one does. Whole lines,
# compared after stripping.
#
# ENTRY 0 IS SPECIAL. It is the step's `- name:` line, matched against the whole stripped file
# and used to LOCATE the step. Every later entry is matched inside the step's own window only
# (the name line up to the next `- ` list item), because `run: |` and `set -euo pipefail` are
# not distinctive: ci.yml carries both in other steps, so a whole-file match would stay green
# with the lockfile step's own run block gutted.
#
# The step runs all three modes (SMA-601 review I2). The bare mode alone was measured to be a
# gate that can lie: with `--locked` removed from run.sh's `cargo metadata` line the command
# exits 0 and repairs the lock itself, so only --self-test and --negative-control catch it.
# Order is asserted too, for the reason T_AFFECTED_SMOKE_REQUIRED_SCRIPT records: moving
# `set -euo pipefail` below the invocations keeps every line byte-identical while a failing
# earlier mode stops aborting the block.
T_CARGO_LOCK_STEP_REQUIRED=(
  '- name: Cargo lockfile integrity (rs/Cargo.lock satisfies every manifest)'
  'run: |'
  'set -euo pipefail'
  'bash ci/cargo-lock-integrity/run.sh --self-test'
  'bash ci/cargo-lock-integrity/run.sh --negative-control'
  'bash ci/cargo-lock-integrity/run.sh'
)

# SMA-601 review I2(b) — the same guard-the-guard class as RELEASE_PARITY_SH_CALL_SITES and
# WORKFLOW_CREDENTIALS_SH_CALL_SITES, for ci/cargo-lock-integrity/run.sh. The step pin above
# proves the three modes are INVOKED; it has no view of whether the script still asserts
# anything. MEASURED: deleting `--locked` from the `cargo metadata` line makes the real run exit
# 0 while cargo REPAIRS the lock in place — the gate then prints "satisfies every manifest" and
# becomes the first repairer, defeating SMA-601 silently and permanently.
#
# Check 8f is the right home rather than ci_targets.py: repo:actionlint carries
# `inputs: ['**/*']`, so it is scheduled on every PR and this pin needs no new input
# registration — and, unlike a pin inside ci/affected-graph/, it is not the sole judge of its own
# reachability.
#
# Six discrete stripped WHOLE lines, not one span, for the reason RELEASE_PARITY_SH_CALL_SITES
# records: a span pin is satisfied by any single surviving copy, and the bypasses have different
# shapes. Whole lines rather than substrings because a substring rule would let a COMMENTED-OUT
# copy satisfy the pin, and because `--locked` is exactly the token an attacker deletes while the
# rest of the line is unchanged. Each was verified to occur EXACTLY ONCE in run.sh.
#   entries 1-2  the flag parse — neutering it makes --self-test/--negative-control fall through
#                to the real run, which then exits 0 having asserted nothing new
#   entry 3      the assertion itself, with --locked. The whole exploit is this one token.
#   entry 4      the negative control calling the REAL assertion function. Without it the control
#                is the SMA-530 "control that actively lies" shape.
#   entry 5      the control's rc=1 report arm — pinned because the message text alone would
#                survive `exit 0`/`exit 1` being swapped
#   entry 6      the real run's own call. Deleting it leaves report() printing a green line.
T_CARGO_LOCK_SH_CALL_SITES=(
  '--self-test)        self_test; return $? ;;'
  '--negative-control) negative_control; return $? ;;'
  'if ( cd "$dir" && cargo metadata --locked --format-version 1 >/dev/null ) 2>"$out"; then'
  'assert_lock_satisfies_manifests "$tmp/rs" || rc=$?'
  '1) echo "cargo-lock-integrity --negative-control: reported red (rc=1) as expected" ;;'
  'assert_lock_satisfies_manifests "$RS_DIR" || rc=$?'
)

# The same three lines ci_targets.py's SELF_SCHEDULED_GATES["affected-smoke"] pins — but pinned
# from here as well, and IN ORDER, for two reasons that copy cannot cover:
#   1. ci/affected-graph/run.sh exits inside its --negative-control branch, before run_suite, so
#      deleting the bare `ci/affected-graph/run.sh` line leaves only the control, which asserts
#      against synthetic fixtures and exits 0. ci_targets.py never runs, so its own pin on that
#      line has no true-positive coverage at all. This check is scheduled independently, so it
#      survives exactly that deletion.
#   2. check_self_invocation compares a SET of stripped lines, so moving `set -euo pipefail`
#      below the invocations keeps every registry entry green while Moon — which takes a script
#      block's status from its LAST command — silently stops propagating a failing control.
#      Reading the block in order costs nothing here and closes that.
T_AFFECTED_SMOKE_REQUIRED_SCRIPT=(
  'set -euo pipefail'
  'ci/affected-graph/run.sh --negative-control'
  'ci/affected-graph/run.sh'
)

# The escape hatch, mirroring T_EXEMPT / ALLOW_DEAD_INPUT / BRANCH_SKIP / COE_SKIP: a required
# input can only be legitimately removed with a stated reason, so the edit is reviewable rather
# than indistinguishable from an attacker's. Entries are "<glob> # <why, and what covers it
# instead>". An entry naming a glob that is no longer required is reported as stale, so a skip
# cannot outlive its glob.
#
# This is also the resolution of the one conflict with repo:input-liveness: if a directory a
# required glob names is ever RENAMED, task_inputs.py demands the dead glob be removed while this
# check demands it stay. Update T_AFFECTED_SMOKE_REQUIRED_INPUTS in the same commit —
# ALLOW_DEAD_INPUT is NOT an escape from this check.
REQUIRED_INPUT_SKIP=(
  # (empty — add entries as "<glob> # why, and what verifies it instead")
)

# rc 0 if $1 is skip-listed with a non-empty reason; rc 2 if it is listed with no reason (the
# caller reports that and still requires the glob); rc 1 if it is not listed at all.
is_required_input_skipped() {
  local key="$1" s glob reason
  for s in ${REQUIRED_INPUT_SKIP+"${REQUIRED_INPUT_SKIP[@]}"}; do
    glob="${s%%#*}"; glob="${glob%"${glob##*[![:space:]]}"}"
    [ "$glob" = "$key" ] || continue
    case "$s" in *'#'*) ;; *) return 2 ;; esac
    reason="${s#*#}"
    [ -n "${reason//[[:space:]]/}" ] || return 2
    return 0
  done
  return 1
}

# Emits TAB-separated records for the repo:affected-smoke task block, in FILE ORDER:
#   INPUT\t<glob>    one `inputs:` sequence entry, surrounding quotes and any trailing comment
#                    on an UNQUOTED value stripped
#   SCRIPT\t<line>   one line of the `script: |` literal block, dedented by six
#   ERR\t<token>     a shape this extractor refuses to guess at
#
# Hand-rolled YAML, held to moon.yml's actual block style: `tasks:` at column 0, task keys at two
# spaces, field keys at four, sequence entries and script body at six. Anything else emits an ERR
# token rather than falling through in silence — the same rule CLAUDE.md already records for this
# file's workflow-filter extractor, and for the same reason: a parser that skips quietly turns the
# check it feeds into a vacuous pass.
#
# THIS IS NOT REACHABILITY ANALYSIS. Like checks 8/8b/8c/8d it matches lines; a required line
# parked in a never-executed block still satisfies it. See README Limitations.
affected_smoke_block_extract() {
  awk '
    function err(tok) { print "ERR\t" tok }

    # A task key sits at EXACTLY two spaces. Matching every such line — not only the one for this
    # task — is what closes the block when the NEXT task starts; without it a required input
    # declared on a later task would satisfy this one. (Worded to avoid an apostrophe on purpose:
    # this whole awk program is single-quoted, so a straight one would terminate it mid-program —
    # the same constraint the \047 note below spells out. Comments in here are not free text.)
    /^  [^ \t#][^:]*:/ {
      intask = 0; inscript = 0; ininputs = 0
      key = $0; sub(/^  /, "", key); sub(/:.*$/, "", key)
      if (key != "affected-smoke") next
      seen_task = 1
      tail = $0; sub(/^  [^:]*:[ \t]*/, "", tail); sub(/[ \t]*#.*$/, "", tail)
      if (tail != "") { err("bad-task-form"); next }
      intask = 1; task_ok = 1
      next
    }

    !intask { next }

    /^    script:/ {
      inscript = 0; ininputs = 0
      v = $0; sub(/^    script:[ \t]*/, "", v); sub(/[ \t]*#.*$/, "", v)
      if (seen_script) { err("duplicate-key script"); next }
      seen_script = 1
      if (v != "|") { err("bad-script-form"); next }
      inscript = 1; next
    }

    /^    inputs:/ {
      inscript = 0; ininputs = 0
      v = $0; sub(/^    inputs:[ \t]*/, "", v); sub(/[ \t]*#.*$/, "", v)
      if (seen_inputs) { err("duplicate-key inputs"); next }
      seen_inputs = 1
      if (v != "") { err("bad-inputs-form"); next }
      ininputs = 1; next
    }

    # Any other four-space field key closes whichever block was open.
    /^    [^ \t]/ { inscript = 0; ininputs = 0; next }

    inscript {
      if ($0 ~ /^[ \t]*$/) next                  # a blank line is literal-block content
      if ($0 ~ /^      /) { s = $0; sub(/^      /, "", s); print "SCRIPT\t" s; next }
      inscript = 0; next
    }

    ininputs {
      if ($0 ~ /^[ \t]*$/) next
      if ($0 ~ /^      #/) next                  # an interleaved YAML comment, not an entry
      if ($0 ~ /^      -[ \t]/) {
        v = $0; sub(/^      -[ \t]*/, "", v)
        # A trailing comment is stripped only on an UNQUOTED value: a quoted glob may legitimately
        # contain a `#`, and moon would read it as part of the pattern. \047 is a single quote —
        # spelled numerically so this awk program can stay inside single quotes.
        if (v ~ /^\047/)  { sub(/^\047/, "", v); sub(/\047[ \t]*(#.*)?$/, "", v) }
        else if (v ~ /^"/) { sub(/^"/, "", v); sub(/"[ \t]*(#.*)?$/, "", v) }
        else               { sub(/[ \t]*#.*$/, "", v); sub(/[ \t]+$/, "", v) }
        print "INPUT\t" v; next
      }
      ininputs = 0; next
    }

    END {
      if (!seen_task) { print "ERR\tno-task"; exit }
      if (!task_ok) exit
      if (!seen_script) print "ERR\tbad-script-form"
      if (!seen_inputs) print "ERR\tbad-inputs-form"
    }
  ' "$1"
}

# Echoes one verdict token per problem, and nothing for a wired block:
#   no-file                        moon.yml missing, or not a readable regular file
#   no-task | bad-task-form | bad-script-form | bad-inputs-form | duplicate-key <name>
#                                  the block could not be parsed — see the extractor's contract
#   missing-input <glob>           a required input is no longer declared
#   missing-script <line>          a required script line is absent (a COMMENTED-OUT copy counts
#                                  as absent — that is what whole-line matching buys)
#   out-of-order-script <line>     present, but at or before a required line that must precede it
#   skip-without-reason <glob>     a REQUIRED_INPUT_SKIP entry with no stated reason
#   stale-skip <glob>              a REQUIRED_INPUT_SKIP entry naming a non-required glob
#
# Never `infra` from inside this function: it is invoked at the production call site as
# `done < <(affected_smoke_block_verdict ...)`, so it runs inside that process substitution's OWN
# subshell — an `exit 2` would exit only the subshell, FAILED would never be set, and the gate
# would finish rc 0 having asserted nothing. Echo a token and `return`, always. (Same bug
# CodeRabbit found on invocation_allowlist_verdict; see the comment above that function.)
affected_smoke_block_verdict() {
  local f="$1" recs glob line s idx prev=0 tab nl entry sl i
  local recs_hay required_hay script_lines

  [ -f "$f" ] && [ -r "$f" ] || { echo 'no-file'; return; }

  tab="$(printf '\t')"
  nl='
'
  recs="$(affected_smoke_block_extract "$f")"

  # EVERY membership test below is bash pattern matching against a newline-DELIMITED haystack,
  # not one `printf … | grep -qxF` subshell per entry. Nineteen required inputs plus three
  # required script lines meant ~50 forks per verdict call, ~45 verdict calls per `--self-test`,
  # and check 9's battery runs eleven of those concurrently — those forks, not the awk pass,
  # dominated this gate's wall clock (measured in the SMA-572 fix wave; see README's cost note).
  #
  # The semantics are IDENTICAL to `grep -qxF`, not merely close, and that is load-bearing:
  # wrapping BOTH the haystack and the needle in newlines is what makes each match WHOLE-LINE,
  # so a declared glob that is a strict PREFIX of a required one (`ci/actionlint/**` against the
  # required `ci/actionlint/**/*`) still reports missing — the exact-match property the fixture
  # table pins in both directions. The needles are QUOTED inside every `case` pattern, so the
  # `*` and `?` characters a glob is made of are matched literally rather than as wildcards;
  # unquoting one would make `ci/**` match anything, which is why they are never bare.
  recs_hay="$nl$recs$nl"

  # A block we could not parse cannot support a per-line answer, and twenty missing-input rows
  # on top of the real problem would bury it. Report the structural verdict alone.
  case "$recs_hay" in
    *"${nl}ERR${tab}"*)
      printf '%s\n' "$recs" | sed -n "s/^ERR$tab//p"
      return
      ;;
  esac

  required_hay="$nl"
  for entry in "${T_AFFECTED_SMOKE_REQUIRED_INPUTS[@]}"; do
    required_hay="$required_hay$entry$nl"
  done

  # Stale and reasonless skips are reported before the requirements they claim to waive, so a
  # typo'd entry cannot silently un-require a glob.
  for s in ${REQUIRED_INPUT_SKIP+"${REQUIRED_INPUT_SKIP[@]}"}; do
    glob="${s%%#*}"; glob="${glob%"${glob##*[![:space:]]}"}"
    [ -n "$glob" ] || continue
    case "$required_hay" in
      *"$nl$glob$nl"*) ;;
      *) echo "stale-skip $glob" ;;
    esac
  done

  for glob in "${T_AFFECTED_SMOKE_REQUIRED_INPUTS[@]}"; do
    is_required_input_skipped "$glob"
    case $? in
      0) continue ;;
      2) echo "skip-without-reason $glob" ;;
    esac
    case "$recs_hay" in
      *"${nl}INPUT${tab}${glob}${nl}"*) ;;
      *) echo "missing-input $glob" ;;
    esac
  done

  # The SCRIPT records, in file order, as an indexed array — the same 1-based numbering the
  # `grep -nxF | head -1 | cut` pipeline this replaces produced, so the `-le "$prev"` ordering
  # comparison below is unchanged. `IFS= read -r` splits on newlines only and keeps backslashes
  # literal, so every element is one WHOLE line and `=` below is again a whole-line comparison.
  script_lines=()
  while IFS= read -r sl; do
    case "$sl" in
      "SCRIPT$tab"*) script_lines+=("${sl#"SCRIPT$tab"}") ;;
    esac
  done <<<"$recs"

  for line in "${T_AFFECTED_SMOKE_REQUIRED_SCRIPT[@]}"; do
    idx=''
    for ((i = 0; i < ${#script_lines[@]}; i++)); do
      if [ "${script_lines[i]}" = "$line" ]; then idx=$((i + 1)); break; fi
    done
    if [ -z "$idx" ]; then
      echo "missing-script $line"
    elif [ "$idx" -le "$prev" ]; then
      echo "out-of-order-script $line"
    else
      prev="$idx"
    fi
  done
}

# Check 8f (SMA-601). Echoes one row per violation, nothing when clean. Row vocabulary:
#   missing-line <text>          a required line is absent
#   out-of-order-script <text>   a required run-block line sits before the one listed above it
#   out-of-order                 the step does not precede the `moon ci` step
#   continue-on-error <value>    the step's continue-on-error is anything but the literal false
#   conditional <expr>           the step carries an `if:` — see the case arm for why that is fatal
#   no-file                      the workflow file is absent or is not a readable regular file
#
# Never `infra` from in here: this function is consumed as `done < <(cargo_lock_step_verdict ...)`,
# so it runs in a process substitution's own subshell where `exit 2` would exit only that
# subshell. Echo a token and return, always (the rule affected_graph_wiring_verdict records).
cargo_lock_step_verdict() { # $1 workflow file
  local f="$1" line stripped window keys n_step n_moon n_end idx prev coe cond

  [ -f "$f" ] && [ -r "$f" ] || { echo 'no-file'; return; }

  stripped="$(sed 's/^[[:space:]]*//' "$f")"

  # Entry 0 LOCATES the step; without it there is no window to search, and reporting the five
  # run-block lines as individually missing would misdescribe one deletion as six.
  n_step="$(printf '%s\n' "$stripped" \
    | grep -nxF -e "${T_CARGO_LOCK_STEP_REQUIRED[0]}" | head -1 | cut -d: -f1)"
  if [ -z "$n_step" ]; then
    echo "missing-line ${T_CARGO_LOCK_STEP_REQUIRED[0]}"
    return
  fi

  # The step's own window: everything after its `- name:` line up to the next `- ` list item
  # (stripped, so every step in the job starts at column 0). Falls back to end-of-file for a
  # step that is last in its job.
  n_end="$(printf '%s\n' "$stripped" | tail -n +"$((n_step + 1))" \
    | grep -nE '^- ' | head -1 | cut -d: -f1)"
  if [ -n "$n_end" ]; then
    n_end=$((n_step + n_end - 1))
  else
    n_end="$(printf '%s\n' "$stripped" | wc -l | tr -d '[:space:]')"
  fi
  window="$(printf '%s\n' "$stripped" | sed -n "$((n_step + 1)),${n_end}p")"

  # Presence AND order, inside the window only. `run: |` and `set -euo pipefail` occur in other
  # ci.yml steps, so a whole-file match on them would be vacuous here. Order is asserted for the
  # reason T_AFFECTED_SMOKE_REQUIRED_SCRIPT records: moving `set -euo pipefail` below the
  # invocations keeps every line byte-identical while a failing mode stops aborting the block.
  prev=0
  for line in "${T_CARGO_LOCK_STEP_REQUIRED[@]:1}"; do
    idx="$(printf '%s\n' "$window" | grep -nxF -e "$line" | head -1 | cut -d: -f1)"
    if [ -z "$idx" ]; then
      echo "missing-line $line"
    elif [ "$idx" -le "$prev" ]; then
      echo "out-of-order-script $line"
    else
      prev="$idx"
    fi
  done

  # Placement is the guarantee, so ordering is asserted, not assumed. Anchored on the stripped
  # text so indentation changes do not defeat it.
  n_moon="$(printf '%s\n' "$stripped" | grep -nxF \
    -e '- name: moon ci (affected graph)' | head -1 | cut -d: -f1)"
  if [ -n "$n_moon" ] && [ "$n_step" -gt "$n_moon" ]; then
    echo "out-of-order"
  fi

  # YAML permits a QUOTED key, so `"if": …` and `'continue-on-error': …` name exactly the same
  # keys as their bare forms and GitHub honours them identically. Normalise both spellings before
  # the two key scans below; without this a quoted `"if":` is a complete bypass of the conditional
  # rule — the rule that stops the step being switched off for `pull_request`, which is the event
  # a Dependabot PR ships a truncated lock on. Found by CodeRabbit in SMA-601's local review.
  # ERE with `()` alternation and two separate expressions, never a BRE `\|` or a backreference
  # across the quote character: `\|` is a GNU extension BSD sed does not honour, and this file
  # is authored on macOS but runs on Linux CI.
  # Applied ONLY to the key scans, never to the T_CARGO_LOCK_STEP_REQUIRED matching above, whose
  # pinned lines are exact text rather than keys.
  # The third expression closes the same bypass class for WHITESPACE BEFORE THE COLON. YAML
  # accepts `if : always()` and `continue-on-error : true`, and GitHub honours both; this file
  # already treats `on :` as a real spelling in extractor_self_test. Without it each scan is a
  # complete bypass by one space (CodeRabbit, PR 185 round 1).
  keys="$(printf '%s\n' "$window" \
    | sed -E -e 's/^"(if|continue-on-error)"[[:space:]]*:/\1:/' \
             -e "s/^'(if|continue-on-error)'[[:space:]]*:/\1:/" \
             -e 's/^(if|continue-on-error)[[:space:]]+:/\1:/')"

  # YAML's EXPLICIT KEY form puts the key and its value on separate lines:
  #
  #     ? if
  #     : always()
  #
  # MEASURED: that parses to a real `if` key (python yaml reports the step's keys as
  # ['name', 'if', 'run']) and `actionlint` accepts the workflow at rc 0, so it would clear
  # check 1 and then evade every same-line scan below. REJECTED rather than normalised: pairing
  # `?` lines with their `:` lines means multi-line parsing for a construct nobody writes by
  # accident, and refusing it is strictly safer than half-understanding it. Reported for either
  # protected key (CodeRabbit, PR 185 full review).
  while IFS= read -r line; do
    case "$line" in
      '? if'|'? if '*|'?	if') echo "explicit-key if" ;;
      '? continue-on-error'|'? continue-on-error '*|'?	continue-on-error') echo "explicit-key continue-on-error" ;;
    esac
  done < <(printf '%s\n' "$window")

  # Anything but the literal `false` suppresses the step's failure. Same rule check 8 applies to
  # the moon ci step. Scanned over the whole window rather than a fixed line count: the run block
  # is multi-line now, so continue-on-error legitimately sits several lines below the name.
  coe="$(printf '%s\n' "$keys" \
    | grep -m1 '^continue-on-error:' | sed 's/^continue-on-error:[[:space:]]*//')"
  if [ -n "$coe" ] && [ "$coe" != "false" ]; then
    echo "continue-on-error $coe"
  fi

  # SMA-601 review I1, MEASURED: inserting an `if: github.event_name == push` expression above
  # the step run: line left the whole gate at exit 0, and the step would then be skipped on
  # every pull request — switching the entire guarantee off for exactly the event on which a
  # Dependabot PR is reviewed. Any `if:` at all is reported; "carries no if:" is what ci.yml's
  # own comment and CLAUDE.md both name as load-bearing, so there is no value worth allowlisting.
  #
  # An `if:` written BEFORE the `name:` key needs no separate rule: the step then opens with
  # `- if: ...` and its name line is no longer `- name: ...`, so entry 0 is reported missing.
  cond="$(printf '%s\n' "$keys" | grep -m1 '^if:' | sed 's/^if:[[:space:]]*//')"
  if [ -n "$cond" ]; then
    echo "conditional $cond"
  fi
}

# Check 8f, second half (SMA-601 review I2b). The script pin — see T_CARGO_LOCK_SH_CALL_SITES.
# Row vocabulary:
#   no-file             ci/cargo-lock-integrity/run.sh is absent or is not a readable file
#   missing-site <text> a pinned whole line is gone
#
# Stripped whole-line comparison, matching T_CARGO_LOCK_SH_CALL_SITES' own note. Same
# no-`infra` rule as above: consumed from a process substitution.
cargo_lock_script_verdict() { # $1 script file
  local f="$1" site

  [ -f "$f" ] && [ -r "$f" ] || { echo 'no-file'; return; }

  for site in "${T_CARGO_LOCK_SH_CALL_SITES[@]}"; do
    grep -qxF -e "$site" <(sed 's/^[[:space:]]*//;s/[[:space:]]*$//' "$f") \
      || echo "missing-site $site"
  done
}

# The standing control for check 8. Both directions on every verdict: a table whose rows all fire
# cannot tell a working check from a stuck one (SMA-466).
ci_target_floor_self_test() {
  SELF_TESTS_RAN=$((SELF_TESTS_RAN + 1))
  local rc=0 tmp got saved_coe_skip saved_swallowed_skip

  expect_floor() {
    local name="$1" expected="$2" body="$3"
    tmp="$(mktemp)"
    printf '%s' "$body" > "$tmp"
    got="$(ci_target_floor_verdict "$tmp")"
    rm -f "$tmp"
    if [ "$got" != "$expected" ]; then
      fail "ci-target-floor self-test '$name': got '$got', expected '$expected'. Check 8 is not
      deciding what it is documented to decide."
      rc=1
    fi
  }

  expect_floor 'healthy array and invocations' '' \
'          T=(:build :affected-smoke :actionlint)
          if [ "$EVENT" = "pull_request" ]; then
            moon ci "${T[@]}" --base origin/main --include-relations
          else
            moon run "${T[@]}"
          fi
'
  expect_floor 'floor entry absent' 'missing :affected-smoke' \
'          T=(:build :actionlint)
          moon ci "${T[@]}" --base origin/main --include-relations
'
  # A prefix of the floor entry must NOT satisfy it.
  expect_floor 'floor entry renamed away' 'missing :affected-smoke' \
'          T=(:build :affected-smoke-disabled)
          moon ci "${T[@]}"
'
  expect_floor 'no array at all' 'no-array' \
'          moon ci --base origin/main
'
  expect_floor 'two arrays' 'no-array' \
'          T=(:affected-smoke)
          T=(:build)
          moon ci "${T[@]}"
'
  expect_floor 'array reformatted across lines' 'no-array' \
'          T=(
            :affected-smoke
          )
          moon ci "${T[@]}"
'
  # A similarly-shaped assignment must not be read as the array.
  expect_floor 'lookalike assignment is not T' 'no-array' \
'          EXPECT=(:affected-smoke)
          moon ci "${T[@]}"
'
  expect_floor 'moon failure swallowed' 'swallowed 2' \
'          T=(:affected-smoke)
          moon ci "${T[@]}" --base origin/main --include-relations || true
'
  # SWALLOWED_SKIP (SMA-542 review M6), same three-row shape as COE_SKIP's below: (1) an entry
  # whose lineno AND text match the real line is silenced; (2) an identical-text occurrence one
  # line later, NOT listed, still fires — the skip doesn't leak past its own line; (3) a STALE
  # entry whose line number matches but whose text does not must NOT silence the line that now
  # sits there. Ships empty, so overridden per assertion and restored immediately.
  saved_swallowed_skip=(${SWALLOWED_SKIP+"${SWALLOWED_SKIP[@]}"})
  SWALLOWED_SKIP=('2:          moon ci "${T[@]}" --base origin/main --include-relations || true')
  expect_floor 'SWALLOWED_SKIP silences an exact lineno+text match' '' \
'          T=(:affected-smoke)
          moon ci "${T[@]}" --base origin/main --include-relations || true
'
  expect_floor 'SWALLOWED_SKIP does not leak to a different line with identical text' \
    'swallowed 3' \
'          T=(:affected-smoke)
          moon run "${T[@]}"
          moon ci "${T[@]}" --base origin/main --include-relations || true
'
  expect_floor 'a stale SWALLOWED_SKIP entry (matching lineno, drifted text) does not silence the line' \
    'swallowed 2' \
'          T=(:affected-smoke)
          moon run "${T[@]}" || true
'
  SWALLOWED_SKIP=(${saved_swallowed_skip+"${saved_swallowed_skip[@]}"})

  # 'block-swallowed' (independent review of PR 150, finding I4): a tail on the line that CLOSES a
  # block is just as silent as one on the 'moon' line, and is invisible to the moon-anchored loop
  # above by construction. Both directions per SMA-466.
  expect_floor 'fi with a swallowing tail is block-swallowed' 'block-swallowed 2' \
'          T=(:affected-smoke)
          fi || true
'
  expect_floor 'done with a swallowing tail is block-swallowed' 'block-swallowed 2' \
'          T=(:affected-smoke)
          done || true
'
  # The whole if/fi wrapped in a brace group: the invocation lines inside stay byte-identical to
  # T_INVOCATION_ALLOWLIST, so only the closing brace's own tail gives it away.
  expect_floor 'a closing brace with a swallowing tail is block-swallowed' 'block-swallowed 2' \
'          T=(:affected-smoke)
          } || true
'
  # ';' counts as a tail here exactly as it does for 'swallowed' above — deliberately
  # over-inclusive, same reasoning: a bare 'fi;next_command' one-liner idiom also fires.
  expect_floor 'fi followed by a semicolon and another command is block-swallowed' 'block-swallowed 2' \
'          T=(:affected-smoke)
          fi;next_command
'
  # The negative control that matters most: ci.yml's REAL closing 'fi', with no tail at all,
  # must stay silent — this is the healthy, unmodified shape on every PR today.
  expect_floor 'a bare fi with no tail does not fire' '' \
'          T=(:affected-smoke)
          moon ci "${T[@]}"
          fi
'
  # Word-boundary proof: 'fi'/'done' must be the line's WHOLE first token, not a prefix of a
  # longer one. Without the boundary requirement, this would misfire on 'fill'/'donetime'.
  expect_floor 'a word merely starting with fi does not fire' '' \
'          T=(:affected-smoke)
          fill_the_cache || true
'
  expect_floor 'a word merely starting with done does not fire' '' \
'          T=(:affected-smoke)
          donetime_metric || true
'
  # SWALLOWED_SKIP is REUSED here too (same reasoning as 'wrapped' above): a block-swallowed line
  # is the same underlying problem as a moon-swallowed line, spelled a third way.
  saved_swallowed_skip=(${SWALLOWED_SKIP+"${SWALLOWED_SKIP[@]}"})
  SWALLOWED_SKIP=('2:          fi || true')
  expect_floor 'SWALLOWED_SKIP also silences a block-swallowed line' '' \
'          T=(:affected-smoke)
          fi || true
'
  SWALLOWED_SKIP=(${saved_swallowed_skip+"${saved_swallowed_skip[@]}"})

  # `moon` not at command position is not an invocation — it must not fire.
  expect_floor 'moon mentioned in a comment' '' \
'          T=(:affected-smoke)
          # run moon ci; it will pass
          moon ci "${T[@]}"
'

  # Fix-wave finding I2: a backslash-continued invocation hides its own tail from the line-at-a-
  # time scan above. The reviewer's own reproduction — measured to return an EMPTY verdict before
  # this fix, silencing every gate in T while T itself stayed correct — now reads as 'continued',
  # never as the misdiagnosing 'swallowed'.
  expect_floor 'moon invocation continued via backslash is rejected, not misread as swallowed' \
    'continued 2' \
'          T=(:affected-smoke)
          moon ci "${T[@]}" \
            --base origin/main \
            --include-relations || true
'
  # The negative control: the REAL ci.yml shape — three separate single-line moon invocations
  # across a pull_request/push/else branch — kept unwrapped. Must not fire, and must not be
  # confused for the continued form above just because it also spans several physical lines.
  expect_floor 'the real if/elif/else form, each moon invocation kept on one line, does not fire' \
    '' \
'          T=(:affected-smoke)
          if [ "$EVENT" = "pull_request" ]; then
            moon ci "${T[@]}" --base origin/main --include-relations
          elif [ -n "$BEFORE" ]; then
            moon ci "${T[@]}" --base "$BEFORE" --include-relations
          else
            moon run "${T[@]}"
          fi
'

  # 'wrapped' (CodeRabbit, PR 150): a command wrapper on the SAME physical line as the invocation
  # evades the swallowed loop above by construction — that loop's anchor requires `moon` to be the
  # line's first token, and neither motivating case puts it there. Both directions per SMA-466.
  expect_floor 'command-wrapped moon invocation with a swallowing tail is flagged as wrapped, not missed' \
    'wrapped 2' \
'          T=(:affected-smoke)
          command moon ci "${T[@]}" --base origin/main --include-relations || true
'
  # No `||`/`&&`/`;`/`|` TAIL at all here — the `;` belongs to `if` syntax — yet it is just as
  # silent: a failing `if` CONDITION does not fail the `if` STATEMENT, so this must still fire.
  expect_floor 'if-wrapped moon invocation (a failing condition would not fail the if) is flagged as wrapped' \
    'wrapped 2' \
'          T=(:affected-smoke)
          if moon ci "${T[@]}" --base origin/main --include-relations; then :; fi
'
  # Glue tolerance, round 2 (CodeRabbit): a negation between the wrapper and `moon` is exactly the
  # `if ! cmd` shape a reviewer would actually write, and must still fire.
  expect_floor 'if-wrapped with a negation (if ! moon ci) still fires as wrapped' \
    'wrapped 2' \
'          T=(:affected-smoke)
          if ! moon ci "${T[@]}" --base origin/main --include-relations; then exit 1; fi
'
  # ...a CHAINED wrapper (a second wrapper token between the first and `moon`) still fires.
  expect_floor 'a chained wrapper (command env moon ci) still fires as wrapped' \
    'wrapped 2' \
'          T=(:affected-smoke)
          command env moon ci "${T[@]}" --base origin/main --include-relations || true
'
  # ...and a `VAR=value` assignment between `env` and `moon` (the one non-wrapper, non-negation
  # token this check tolerates) still fires.
  expect_floor 'an env assignment before moon (env FOO=bar moon ci) still fires as wrapped' \
    'wrapped 2' \
'          T=(:affected-smoke)
          env FOO=bar moon ci "${T[@]}" --base origin/main --include-relations || true
'
  # THE FALSE POSITIVE (CodeRabbit, PR 150 round 2, measured): a wrapper token at line start with
  # `moon ci` appearing only INSIDE A STRING later on the line, while the actual next command word
  # is something else entirely. No `moon` ever runs here. This is the "any line containing
  # 'moon ci'" trap the file-scope anchor above was written to avoid, one level down inside a
  # wrapper-prefixed line — proves `moon` must be the NEXT COMMAND WORD, not merely present.
  expect_floor 'a wrapper whose real command is not moon, with "moon ci" only inside a string, does not fire' \
    '' \
'          T=(:affected-smoke)
          moon ci "${T[@]}"
          if test -n "$X"; then echo "moon ci failed"; fi
'
  # The same trap, for `moon run` and a different wrapper (`while`), so the fix is proven for both
  # verbs and is not accidentally scoped to `if`/`moon ci` alone.
  expect_floor 'a wrapper whose real command is not moon, with "moon run" only inside a string, does not fire' \
    '' \
'          T=(:affected-smoke)
          moon ci "${T[@]}"
          while read -r line; do echo "moon run scheduled: $line"; done < list.txt
'
  # A wrapper token followed by a line that only MENTIONS moon (in a string), not an actual
  # `moon ci`/`moon run` invocation — real ci.yml line 179's shape. Proves the AND-condition: the
  # wrapper-prefix match alone is not enough to fire.
  expect_floor 'a wrapper token on a line that only mentions moon, not moon ci/run, does not fire' \
    '' \
'          T=(:affected-smoke)
          moon ci "${T[@]}"
          command -v pnpm >/dev/null || { echo "moon setup failed"; exit 1; }
'
  # `moon setup`/`moon sync` are not gated by T and out of scope for this floor — a wrapper in
  # front of one of those must not fire either. Proves the moon-verb match is narrowed to ci/run.
  expect_floor 'a wrapper around moon setup (not moon ci/run) does not fire' '' \
'          T=(:affected-smoke)
          moon ci "${T[@]}"
          command moon setup || true
'
  # SWALLOWED_SKIP is REUSED for 'wrapped' rather than a fourth skip list (spec decision, PR 150):
  # a wrapped and a bare-`moon` swallowed line are the same underlying problem spelled two ways, so
  # the same lineno+text-keyed escape hatch applies to both.
  saved_swallowed_skip=(${SWALLOWED_SKIP+"${SWALLOWED_SKIP[@]}"})
  SWALLOWED_SKIP=('2:          command moon ci "${T[@]}" --base origin/main --include-relations || true')
  expect_floor 'SWALLOWED_SKIP also silences a wrapped line (reused skip list, not a fourth one)' \
    '' \
'          T=(:affected-smoke)
          command moon ci "${T[@]}" --base origin/main --include-relations || true
'
  SWALLOWED_SKIP=(${saved_swallowed_skip+"${saved_swallowed_skip[@]}"})

  # continue-on-error: — the third D14 spelling (SMA-542 task 4b). Both directions per SMA-466:
  # each positive row below must fire, and each negative control must specifically be what stops
  # firing — not just an unrelated healthy file.
  expect_floor 'continue-on-error true silences a step' 'continue-on-error 3' \
'          T=(:affected-smoke)
          moon ci "${T[@]}"
            continue-on-error: true
'
  # I1: the match is deliberately over-inclusive, not just `true`. If a future edit narrowed it
  # back to the literal word `true`, these two rows are what would catch it.
  expect_floor 'continue-on-error True (capitalized) fires' 'continue-on-error 3' \
'          T=(:affected-smoke)
          moon ci "${T[@]}"
            continue-on-error: True
'
  expect_floor 'continue-on-error as a ${{ }} expression fires' 'continue-on-error 3' \
'          T=(:affected-smoke)
          moon ci "${T[@]}"
            continue-on-error: ${{ true }}
'
  # `false` is the ONE spelling GitHub Actions itself treats as "do not suppress" — under the I1
  # inversion this is the load-bearing negative control. If the verdict logic ever stopped
  # checking the value and fired on the key alone, this is the row that would catch it.
  expect_floor 'continue-on-error false is excluded' '' \
'          T=(:affected-smoke)
          moon ci "${T[@]}"
          continue-on-error: false
'
  # A commented-out occurrence must not fire. If the key match ever stopped anchoring to the
  # start of the (post-indentation) line, this is the row that would catch it.
  expect_floor 'continue-on-error commented out' '' \
'          T=(:affected-smoke)
          moon ci "${T[@]}"
          # continue-on-error: true
'
  # I2's escape hatch, now keyed by lineno AND text (SMA-542 follow-up). Three rows, both
  # directions: (1) an entry whose lineno AND text match the real line is silenced; (2) an
  # identical-text occurrence one line later, NOT listed, still fires — the skip doesn't leak past
  # its own line; (3) the case this key format exists for — a STALE entry whose line number
  # matches but whose text does not (the file shifted and a DIFFERENT continue-on-error now sits
  # on that line) must NOT be silenced. A bare-lineno key would wrongly skip row 3; only the
  # lineno+text pair tells them apart. COE_SKIP ships empty, so it is overridden per assertion and
  # restored immediately.
  saved_coe_skip=(${COE_SKIP+"${COE_SKIP[@]}"})
  COE_SKIP=('3:            continue-on-error: true')
  expect_floor 'COE_SKIP silences an exact lineno+text match' '' \
'          T=(:affected-smoke)
          moon ci "${T[@]}"
            continue-on-error: true
'
  expect_floor 'COE_SKIP does not leak to a different line with identical text' 'continue-on-error 4' \
'          T=(:affected-smoke)
          moon ci "${T[@]}"
            continue-on-error: true
            continue-on-error: true
'
  expect_floor 'a stale COE_SKIP entry (matching lineno, drifted text) does not silence the line' 'continue-on-error 3' \
'          T=(:affected-smoke)
          moon ci "${T[@]}"
        continue-on-error: true
'
  COE_SKIP=(${saved_coe_skip+"${saved_coe_skip[@]}"})

  got="$(ci_target_floor_verdict /nonexistent/ci.yml)"
  if [ "$got" != 'no-file' ]; then
    fail "ci-target-floor self-test 'missing file': got '$got', expected 'no-file'. A renamed
      workflow must not report the misleading 'keep T on one line' remediation."
    rc=1
  fi

  return $rc
}

# The standing control for invocation_allowlist_verdict (SMA-542 CodeRabbit round 3). Both
# directions on every rule per SMA-466: the exact-match rule and the count rule are each proven to
# fire AND to stay silent on a healthy file, and the suppression argument is proven to actually
# suppress rather than just happening to agree with an empty expectation.
invocation_allowlist_self_test() {
  SELF_TESTS_RAN=$((SELF_TESTS_RAN + 1))
  local rc=0 tmp got

  expect_invocation() {
    local name="$1" expected="$2" body="$3" skip="${4:-}"
    tmp="$(mktemp)"
    printf '%s' "$body" > "$tmp"
    got="$(invocation_allowlist_verdict "$tmp" "$skip")"
    rm -f "$tmp"
    if [ "$got" != "$expected" ]; then
      fail "invocation-allowlist self-test '$name': got '$got', expected '$expected'. This check
      is not deciding what it is documented to decide."
      rc=1
    fi
  }

  # The healthy control: the real ci.yml if/elif/else shape, all three lines exact. Every OTHER
  # row below is a one-line mutation of this same body, so a failure here would mean the fixture
  # itself is wrong, not the check.
  local healthy='          if [ "$EVENT" = "pull_request" ]; then
            moon ci "${T[@]}" --base origin/main --include-relations
          elif [ -n "${BEFORE:-}" ]; then
            moon ci "${T[@]}" --base "$BEFORE" --include-relations
          else
            moon run "${T[@]}"
          fi
'
  expect_invocation 'the real if/elif/else shape, all three lines exact, is clean' '' "$healthy"

  # Count rule, alone: one correctly-formed line is still wrong because only ONE of the three
  # required invocations is present. Proves the count check does not just defer to the per-line
  # check (the one line present matches perfectly).
  expect_invocation 'a single correct line, alone, is still an invocation-count mismatch' \
    'invocation-count 1' \
    '            moon run "${T[@]}"
'
  # ...and the other direction: one EXTRA line (a duplicate of an allowed form) is a mismatch too,
  # even though every line still matches the allowlist individually.
  expect_invocation 'a fourth (duplicate) line is also an invocation-count mismatch' \
    'invocation-count 4' \
    "$healthy"'            moon run "${T[@]}"
'

  # not-allowlisted, isolated from the count check: each variant below replaces exactly one of the
  # three healthy lines, so the count stays at 3 and only the per-line rule can fire.
  local prefixed='          if [ "$EVENT" = "pull_request" ]; then
            FOO=bar moon ci "${T[@]}" --base origin/main --include-relations
          elif [ -n "${BEFORE:-}" ]; then
            moon ci "${T[@]}" --base "$BEFORE" --include-relations
          else
            moon run "${T[@]}"
          fi
'
  expect_invocation 'an assignment-prefixed line (round 3 shape, no tail) is not-allowlisted' \
    'not-allowlisted 2' "$prefixed"

  local suffixed='          if [ "$EVENT" = "pull_request" ]; then
            moon ci "${T[@]}" --base origin/main --include-relations
          elif [ -n "${BEFORE:-}" ]; then
            moon ci "${T[@]}" --base "$BEFORE" --include-relations
          else
            moon run "${T[@]}" --dry-run
          fi
'
  expect_invocation 'a suffixed line (extra trailing flag) is not-allowlisted' \
    'not-allowlisted 6' "$suffixed"

  local wrapped='          if [ "$EVENT" = "pull_request" ]; then
            command moon ci "${T[@]}" --base origin/main --include-relations || true
          elif [ -n "${BEFORE:-}" ]; then
            moon ci "${T[@]}" --base "$BEFORE" --include-relations
          else
            moon run "${T[@]}"
          fi
'
  expect_invocation 'a wrapped line, with no skip argument, is not-allowlisted' \
    'not-allowlisted 2' "$wrapped"
  # ...and the SAME body, told line 2 is already explained elsewhere (as the production call site
  # would, from ci_target_floor_verdict's own 'wrapped 2'), is clean. Proves the skip argument
  # actually suppresses — not merely that this shape happens to be silent regardless.
  expect_invocation 'the same wrapped line, told it is already explained via skip, is clean' \
    '' "$wrapped" '2'
  # ...and a DIFFERENT lineno in the skip list must NOT suppress line 2 — the skip is positional,
  # not "any problem exists somewhere, so say nothing".
  expect_invocation 'a skip argument naming a DIFFERENT line does not suppress this one' \
    'not-allowlisted 2' "$wrapped" '99'

  # THE motivating case (CodeRabbit round 3, finding B), reproduced exactly: an assignment prefix
  # WITH a swallowing tail. Measured on the real file before this fix: full gate rc 0, no verdict
  # at all — `swallowed` needs `moon` at column 0, `wrapped` needs a recognized wrapper token at
  # column 0, and a bare `VAR=value` prefix is neither.
  local finding_b='          if [ "$EVENT" = "pull_request" ]; then
            FOO=bar moon ci "${T[@]}" --base origin/main --include-relations || true
          elif [ -n "${BEFORE:-}" ]; then
            moon ci "${T[@]}" --base "$BEFORE" --include-relations
          else
            moon run "${T[@]}"
          fi
'
  expect_invocation 'an assignment prefix WITH a swallowing tail (round 3 finding B) is not-allowlisted' \
    'not-allowlisted 2' "$finding_b"

  # Exactness, not a loose/trimmed comparison: trailing whitespace on an otherwise-correct line
  # must still fail. Guards against an implementation that trims before comparing.
  local trailing_ws='          if [ "$EVENT" = "pull_request" ]; then
            moon ci "${T[@]}" --base origin/main --include-relations
          elif [ -n "${BEFORE:-}" ]; then
            moon ci "${T[@]}" --base "$BEFORE" --include-relations
          else
            moon run "${T[@]}" 
          fi
'
  expect_invocation 'trailing whitespace on an otherwise-correct line is not-allowlisted' \
    'not-allowlisted 6' "$trailing_ws"

  # Indentation exactness (independent review of PR 150, finding minor-2): mutating the comparison
  # to strip LEADING blanks from both sides before comparing would let this row through. 8 spaces
  # (shallower than the required 12) and the text otherwise byte-identical to the allowed form.
  local under_indented='          if [ "$EVENT" = "pull_request" ]; then
            moon ci "${T[@]}" --base origin/main --include-relations
          elif [ -n "${BEFORE:-}" ]; then
            moon ci "${T[@]}" --base "$BEFORE" --include-relations
          else
        moon run "${T[@]}"
          fi
'
  expect_invocation 'a correct line at the wrong indent (8 spaces, not 12) is not-allowlisted' \
    'not-allowlisted 6' "$under_indented"
  # ...and the other direction, 16 spaces (deeper than required).
  local over_indented='          if [ "$EVENT" = "pull_request" ]; then
            moon ci "${T[@]}" --base origin/main --include-relations
          elif [ -n "${BEFORE:-}" ]; then
            moon ci "${T[@]}" --base "$BEFORE" --include-relations
          else
                moon run "${T[@]}"
          fi
'
  expect_invocation 'a correct line at the wrong indent (16 spaces, not 12) is not-allowlisted' \
    'not-allowlisted 6' "$over_indented"

  # Skip-list padding (independent review of PR 150, finding minor-3): the skip argument is
  # SPACE-PADDED and matched as a whole token (" $lineno "), not a bare substring test. Without
  # that padding, a caller-supplied skip value that merely CONTAINS this line's number as a
  # substring — '12' contains '2' — would wrongly suppress line 2, a real fail-open (a reported
  # line 219 would silently suppress an unrelated line 19). skip='12' here must NOT suppress the
  # bad line at line 2.
  expect_invocation 'a skip value containing this lineno as a SUBSTRING, not a whole token, does not suppress it' \
    'not-allowlisted 2' "$prefixed" '12'

  got="$(invocation_allowlist_verdict /nonexistent/ci.yml)"
  if [ "$got" != 'no-file' ]; then
    fail "invocation-allowlist self-test 'missing file': got '$got', expected 'no-file'."
    rc=1
  fi

  # count-unreadable (CodeRabbit round 4, finding F1) — the round-trip half this self-test CAN
  # prove: the verdict function itself must echo 'count-unreadable' and return, never call
  # `infra` (which would only exit the process-substitution subshell at the production call
  # site — see the comment above invocation_allowlist_verdict). The other half of the round-trip —
  # the call site turning this token into an actual `infra` exit — is proven live against a
  # forced real-file scenario, not here; --self-test never reaches that call site at all.
  #
  # Driven directly through invocation_allowlist_count_verdict with synthetic malformed counts —
  # NOT via a directory in place of a file. A directory used to sit here on the theory that it
  # "portably forces `grep -c` to fail without ever writing to stdout"; that is true of BSD grep
  # (macOS: the read fails outright, $n comes back empty, landing on 'count-unreadable') but false
  # of GNU grep (Linux, what CI actually runs), which prints a literal '0' to stdout for the same
  # directory argument — a well-formed-but-wrong count that instead lands on 'invocation-count 0'.
  # That split is exactly what reds this fixture on Linux CI while passing on a macOS dev box
  # (SMA-542, CodeRabbit review of PR 150). A malformed synthetic string never touches grep at
  # all, so no platform's `grep` behaviour can make it disagree.
  local n
  for n in '' 'x' ' '; do
    got="$(invocation_allowlist_count_verdict "$n" "${#T_INVOCATION_ALLOWLIST[@]}")"
    if [ "$got" != 'count-unreadable' ]; then
      fail "invocation-allowlist self-test 'unreadable count (n=\"$n\")': got '$got', expected
      'count-unreadable'. A malformed count must fail loudly, not silently read as zero matches."
      rc=1
    fi
  done

  return $rc
}

# The standing control for Check 8c (SMA-542 residual closure). Both directions per SMA-466: a
# healthy tree stays silent, EACH call site's deletion fires on its own, a swallowed propagation
# suffix fires exactly the same as an outright deletion, and the substring trap (a function's own
# definition satisfying a name-only match) is proven NOT to satisfy this one.
affected_graph_wiring_self_test() {
  SELF_TESTS_RAN=$((SELF_TESTS_RAN + 1))
  local rc=0 tmp got

  expect_wiring() {
    local name="$1" expected="$2" body="$3"
    tmp="$(mktemp)"
    printf '%s' "$body" > "$tmp"
    got="$(affected_graph_wiring_verdict "$tmp")"
    rm -f "$tmp"
    if [ "$got" != "$expected" ]; then
      fail "affected-graph-wiring self-test '$name': got '$got', expected '$expected'. Check 8c
      is not deciding what it is documented to decide."
      rc=1
    fi
  }

  # The healthy control: both call sites present, exactly as ci/affected-graph/run.sh's real
  # run_suite() and its --negative-control branch have them, plus the function DEFINITION that
  # makes the substring-trap fixture below meaningful (mirrors ci_targets.py's own `wired`
  # self-test fixture for this exact pair of lines).
  local wired='assert_ci_targets() {
  :
}
  assert_ci_targets || SUITE_RC=1
  python3 "$HERE/ci_targets.py" --self-test || NEG_RC=1
'
  expect_wiring 'both call sites present, suffixes intact, is clean' '' "$wired"

  # Each site deleted OUTRIGHT, one at a time — the plainest form of the residual this check
  # closes.
  local no_assert_call='  python3 "$HERE/ci_targets.py" --self-test || NEG_RC=1
'
  expect_wiring 'assert_ci_targets call site deleted outright fires' \
    'missing assert_ci_targets || SUITE_RC=1' "$no_assert_call"

  local no_selftest_call='assert_ci_targets() {
  :
}
  assert_ci_targets || SUITE_RC=1
'
  expect_wiring 'the --self-test call site deleted outright fires' \
    'missing "$HERE/ci_targets.py" --self-test || NEG_RC=1' "$no_selftest_call"

  # The substring trap (same reasoning as ci_targets.py's own `wired` fixture, and as
  # ACTIONLINT_SH_CALL_SITES' comment on this file's `run_self_tests() {` definition): the bare
  # name `assert_ci_targets` is present here too, but ONLY as its own function definition — never
  # as the suffixed call — so a name-only match would wrongly read this as wired.
  local def_only_no_call='assert_ci_targets() {
  :
}
  python3 "$HERE/ci_targets.py" --self-test || NEG_RC=1
'
  expect_wiring 'assert_ci_targets present only in its own definition still fires' \
    'missing assert_ci_targets || SUITE_RC=1' "$def_only_no_call"

  # The propagation suffix swallowed rather than the line deleted — the CodeRabbit-found hole
  # this check exists to close (same class as ci_targets.py's own `silenced` fixture): the call
  # still RUNS, but its failure no longer reaches SUITE_RC/NEG_RC, so a red ci_targets.py would
  # silently stop failing the suite.
  local assert_swallowed='assert_ci_targets() {
  :
}
  assert_ci_targets || true
  python3 "$HERE/ci_targets.py" --self-test || NEG_RC=1
'
  expect_wiring 'assert_ci_targets suffix swallowed by || true fires' \
    'missing assert_ci_targets || SUITE_RC=1' "$assert_swallowed"

  local selftest_swallowed='assert_ci_targets() {
  :
}
  assert_ci_targets || SUITE_RC=1
  python3 "$HERE/ci_targets.py" --self-test || true
'
  expect_wiring 'the --self-test suffix swallowed by || true fires' \
    'missing "$HERE/ci_targets.py" --self-test || NEG_RC=1' "$selftest_swallowed"

  # Both missing at once — proves the two are reported independently, in
  # T_AFFECTED_GRAPH_CALL_SITES' own order, not merged into a single verdict that could mask the
  # second.
  expect_wiring 'both call sites missing fires both, independently' \
    'missing assert_ci_targets || SUITE_RC=1
missing "$HERE/ci_targets.py" --self-test || NEG_RC=1' \
    'echo "nothing relevant here"
'

  got="$(affected_graph_wiring_verdict /nonexistent/ci/affected-graph/run.sh)"
  if [ "$got" != 'no-file' ]; then
    fail "affected-graph-wiring self-test 'missing file': got '$got', expected 'no-file'. A
      renamed ci/affected-graph/run.sh must not be misread as \"both call sites deleted\"."
    rc=1
  fi

  # A directory in place of the file — same "distinct verdict, not a silent skip" requirement,
  # proven without relying on chmod (which can silently no-op when tests run as root). This one
  # IS portable, unlike the directory tricks that used to sit in invocation_allowlist_self_test
  # and block_execution_self_test (both replaced with synthetic malformed counts, SMA-542
  # CodeRabbit review of PR 150 — see the WHY comment on invocation_allowlist_count_verdict):
  # affected_graph_wiring_verdict guards with `[ -f "$f" ] && [ -r "$f" ]`, a bash builtin `test`,
  # BEFORE it ever reaches a `grep`, so a directory is rejected identically on BSD and GNU — there
  # is no grep-behaviour split to disagree across platforms here.
  local unreadable_dir
  unreadable_dir="$(mktemp -d)"
  got="$(affected_graph_wiring_verdict "$unreadable_dir")"
  rmdir "$unreadable_dir"
  if [ "$got" != 'no-file' ]; then
    fail "affected-graph-wiring self-test 'directory in place of file': got '$got', expected
      'no-file'. A directory must not be read as two missing call sites."
    rc=1
  fi

  return $rc
}

# The standing control for Check 8e (SMA-572 / SMA-573). Both directions per SMA-466: a fully
# wired block stays silent, and EVERY required entry — input glob and script line alike — fires on
# its own when it is removed.
#
# The wired control is BUILT FROM the live arrays rather than spelled out, so a twentieth required
# input added tomorrow is covered automatically and cannot leave this control passing for the
# wrong reason (the vacuity SMA-530 measured on wired_scripts()).
affected_smoke_block_self_test() {
  SELF_TESTS_RAN=$((SELF_TESTS_RAN + 1))
  local rc=0 tmp got saved_skip

  expect_smoke_block() {
    local name="$1" expected="$2" body="$3"
    tmp="$(mktemp)"
    printf '%s' "$body" > "$tmp"
    got="$(affected_smoke_block_verdict "$tmp")"
    rm -f "$tmp"
    if [ "$got" != "$expected" ]; then
      fail "affected-smoke-block self-test '$name': got '$got', expected '$expected'. Check 8e
      is not deciding what it is documented to decide."
      rc=1
    fi
  }

  # Rewrites every line of body $1 that equals $2 into $3, which may itself span several lines.
  # Pure bash, deliberately NOT `sed`: a `\n` in a sed REPLACEMENT is a GNU extension BSD sed does
  # not honour, so a fixture built with one means something different on macOS than it does on
  # Linux CI — the exact platform split PR 150's review found in this file's directory fixtures
  # (SMA-542). `|| [ -n "$line" ]` so a body with no trailing newline keeps its last line.
  rewrite_line() {
    local body="$1" from="$2" to="$3" out='' line
    while IFS= read -r line || [ -n "$line" ]; do
      [ "$line" = "$from" ] && line="$to"
      out="$out$line
"
    done < <(printf '%s' "$body")
    printf '%s' "$out"
  }

  # Deletes every line of body $1 that equals $2 — whole-line and fixed-string, so a `*` inside a
  # glob is a literal.
  drop_line() {
    printf '%s' "$1" | grep -vxF -e "$2"
  }

  # `q` rather than the '"'"' idiom: these fixtures are almost entirely single-quoted YAML
  # scalars, and the escaped form is unreadable twenty entries deep.
  local q="'" wired glob line

  wired="tasks:
  affected-smoke:
    description: ${q}Assert the cross-language affected graph still cascades.${q}
    script: |
"
  for line in "${T_AFFECTED_SMOKE_REQUIRED_SCRIPT[@]}"; do
    wired="$wired      $line
"
  done
  wired="$wired    toolchain: ${q}system${q}
    inputs:
"
  for glob in "${T_AFFECTED_SMOKE_REQUIRED_INPUTS[@]}"; do
    wired="$wired      - ${q}${glob}${q}
"
  done
  # A SECOND task, present in every fixture derived from this one: the two-space key rule that
  # closes the block is only exercised if there is something after it to close against.
  wired="$wired
  other-task:
    script: ${q}true${q}
"

  expect_smoke_block 'a fully wired block is clean' '' "$wired"

  # Each required input deleted in turn. Driven from the array so a twentieth-and-later entry is
  # covered automatically.
  for glob in "${T_AFFECTED_SMOKE_REQUIRED_INPUTS[@]}"; do
    expect_smoke_block "required input '$glob' deleted fires" "missing-input $glob" \
      "$(drop_line "$wired" "      - ${q}${glob}${q}")"
  done

  # Each required script line deleted in turn. Whole-line deletion, so removing
  # `ci/affected-graph/run.sh` leaves the `--negative-control` line untouched — which is the
  # deletion that matters most, since run.sh exits inside the control branch and the control alone
  # exits 0 having asserted only against synthetic fixtures.
  for line in "${T_AFFECTED_SMOKE_REQUIRED_SCRIPT[@]}"; do
    expect_smoke_block "required script line '$line' deleted fires" "missing-script $line" \
      "$(drop_line "$wired" "      $line")"
  done

  # ORDER, not merely presence: ci_targets.py's check_self_invocation compares a SET of stripped
  # lines, so moving `set -euo pipefail` below the invocations leaves every registry entry green
  # while errexit silently stops mattering. 8e reads the block in order, so it closes that here.
  #
  # BOTH invocations are named rather than `set -euo pipefail` itself: the verdict walks
  # T_AFFECTED_SMOKE_REQUIRED_SCRIPT in its own order and reports each line landing at or before
  # the previously-matched one, so with `set -euo pipefail` parked last, the two lines that must
  # follow it are the two out of place relative to it. Naming both is what keeps the report
  # meaningful for a future reorder involving more than one line.
  local reordered
  reordered="tasks:
  affected-smoke:
    script: |
      ci/affected-graph/run.sh --negative-control
      ci/affected-graph/run.sh
      set -euo pipefail
    inputs:
"
  for glob in "${T_AFFECTED_SMOKE_REQUIRED_INPUTS[@]}"; do
    reordered="$reordered      - ${q}${glob}${q}
"
  done
  expect_smoke_block 'set -euo pipefail moved below the invocations fires out-of-order' \
    'out-of-order-script ci/affected-graph/run.sh --negative-control
out-of-order-script ci/affected-graph/run.sh' "$reordered"

  # A commented-out required line must report MISSING. This is the property whole-line matching
  # buys: commenting a line out does not remove its text, only prefix it.
  expect_smoke_block 'a commented-out required script line still fires' \
    'missing-script ci/affected-graph/run.sh' \
    "$(rewrite_line "$wired" '      ci/affected-graph/run.sh' '      # ci/affected-graph/run.sh')"

  # SEPARATE STREAMS: the inputs entry planted VERBATIM inside the script block must not satisfy
  # the inputs table. A single concatenated haystack — the shape ci_targets.py documents as wrong
  # — would read this as wired.
  local no_claude
  no_claude="$(drop_line "$wired" "      - ${q}CLAUDE.md${q}")"
  expect_smoke_block 'an inputs entry planted in the script body does not satisfy the inputs table' \
    'missing-input CLAUDE.md' \
    "$(rewrite_line "$no_claude" '      set -euo pipefail' "      set -euo pipefail
      - ${q}CLAUDE.md${q}")"

  # Quote styles: moon accepts all three, so all three must be recognised.
  expect_smoke_block 'a double-quoted input is recognised' '' \
    "$(rewrite_line "$wired" "      - ${q}CLAUDE.md${q}" '      - "CLAUDE.md"')"
  expect_smoke_block 'an unquoted input is recognised' '' \
    "$(rewrite_line "$wired" "      - ${q}CLAUDE.md${q}" '      - CLAUDE.md')"
  expect_smoke_block 'a trailing comment on an unquoted input is stripped' '' \
    "$(rewrite_line "$wired" "      - ${q}CLAUDE.md${q}" '      - CLAUDE.md  # the docs pin')"
  # The QUOTED arm strips its trailing comment through a DIFFERENT sub() than the unquoted arm
  # above (the quoted one has to close the quote first, so a `#` inside the glob stays part of the
  # pattern). Without this row that second sub() is uncovered, and the live moon.yml carries
  # exactly this shape — a single-quoted glob followed by an SMA reference.
  expect_smoke_block 'a trailing comment on a QUOTED input is stripped' '' \
    "$(rewrite_line "$wired" "      - ${q}CLAUDE.md${q}" "      - ${q}CLAUDE.md${q}  # SMA-541")"

  # A blank line inside the `script: |` literal block is legal YAML and legal bash, and separating
  # `set -euo pipefail` from the invocations for readability is a plausible edit. The extractor
  # skips it explicitly; without this row, deleting that skip closes the block on the blank line
  # and every script line after it reads as missing — a red on a change that broke nothing.
  expect_smoke_block 'a blank line inside the script block is tolerated' '' \
    "$(rewrite_line "$wired" '      set -euo pipefail' '      set -euo pipefail
')"

  # An interleaved YAML comment inside the sequence is not an entry. The live file carries several
  # of these (moon.yml's `# SMA-542 …`, `# SMA-530 …` and `# SMA-541 …` blocks all sit between
  # sequence entries), so "any six-space line" is not a sufficient rule.
  expect_smoke_block 'an interleaved comment in the inputs block is skipped' '' \
    "$(rewrite_line "$wired" "      - ${q}CLAUDE.md${q}" "      # SMA-541 — do not remove
      - ${q}CLAUDE.md${q}")"

  # Shapes this extractor refuses to guess at. Each reports its OWN token and nothing else: a
  # block we could not parse cannot support a per-line answer, and twenty missing-input rows on
  # top would bury the real problem.
  expect_smoke_block 'a folded script scalar fires bad-script-form' 'bad-script-form' \
    "$(rewrite_line "$wired" '    script: |' '    script: >')"
  expect_smoke_block 'an inline inputs sequence fires bad-inputs-form' 'bad-inputs-form' \
"tasks:
  affected-smoke:
    script: |
      set -euo pipefail
    inputs: [moon.yml]
"
  expect_smoke_block 'a non-comment tail on the task key fires bad-task-form' 'bad-task-form' \
    "$(rewrite_line "$wired" '  affected-smoke:' '  affected-smoke: &anchor')"
  expect_smoke_block 'a trailing comment on the task key is tolerated' '' \
    "$(rewrite_line "$wired" '  affected-smoke:' '  affected-smoke:  # the cascade gate')"
  expect_smoke_block 'a second inputs key fires duplicate-key' 'duplicate-key inputs' \
"tasks:
  affected-smoke:
    script: |
      set -euo pipefail
      ci/affected-graph/run.sh --negative-control
      ci/affected-graph/run.sh
    inputs:
      - ${q}moon.yml${q}
    inputs:
      - ${q}moon.yml${q}
"
  expect_smoke_block 'a second script key fires duplicate-key' 'duplicate-key script' \
"tasks:
  affected-smoke:
    script: |
      set -euo pipefail
    script: |
      set -euo pipefail
    inputs:
      - ${q}moon.yml${q}
"
  expect_smoke_block 'the task being absent entirely fires no-task' 'no-task' \
"tasks:
  other-task:
    script: ${q}true${q}
"

  # A LATER task must not be read as part of this one: the two-space key rule is what stops it.
  # Assembled with an explicit newline rather than by concatenating a command substitution with a
  # following literal — `$( )` strips trailing newlines, so the two would join on one line and the
  # fixture would silently test something else.
  local elsewhere
  elsewhere="$no_claude
    inputs:
      - ${q}CLAUDE.md${q}
"
  expect_smoke_block 'a required input declared on a DIFFERENT task does not count' \
    'missing-input CLAUDE.md' "$elsewhere"

  # REQUIRED_INPUT_SKIP, both directions.
  saved_skip=(${REQUIRED_INPUT_SKIP+"${REQUIRED_INPUT_SKIP[@]}"})
  REQUIRED_INPUT_SKIP=("CLAUDE.md # moved to a different gate, verified by X")
  expect_smoke_block 'a skipped required input is not reported' '' "$no_claude"
  expect_smoke_block 'a skip does not leak to a different glob' 'missing-input moon.yml' \
    "$(drop_line "$wired" "      - ${q}moon.yml${q}")"
  REQUIRED_INPUT_SKIP=("CLAUDE.md")
  expect_smoke_block 'a skip with no reason is rejected' \
    'skip-without-reason CLAUDE.md
missing-input CLAUDE.md' "$no_claude"
  # The `#`-present-but-empty form is a SECOND, distinct `return 2` in is_required_input_skipped,
  # and the row above exercises only the first (no `#` at all). Covering it is what stops a later
  # cleanup folding the two paths together — after which `"moon.yml #"` would read as a stated
  # reason and silently un-require the one glob that schedules this whole family of pins.
  REQUIRED_INPUT_SKIP=("CLAUDE.md #")
  expect_smoke_block 'a skip whose reason is blank is rejected too' \
    'skip-without-reason CLAUDE.md
missing-input CLAUDE.md' "$no_claude"
  REQUIRED_INPUT_SKIP=("ops/**/* # names a glob that is not required")
  expect_smoke_block 'a skip naming a non-required glob is reported stale' \
    'stale-skip ops/**/*' "$wired"
  # The row above cannot see whether the stale-skip match is EXACT: `ops/**/*` is not a substring
  # of any required entry, so `grep -qxF` degraded to `grep -qF` still reports it. This one is a
  # strict PREFIX of the required `ci/actionlint/**/*` — the shape a typo'd or half-updated waiver
  # actually takes — so it is reported stale only while the match stays whole-line.
  REQUIRED_INPUT_SKIP=("ci/actionlint/** # a mere PREFIX of a required glob, not one of them")
  expect_smoke_block 'a stale skip that is a SUBSTRING of a required glob is still reported' \
    'stale-skip ci/actionlint/**' "$wired"
  REQUIRED_INPUT_SKIP=(${saved_skip+"${saved_skip[@]}"})

  # The INPUT membership test must be anchored at BOTH ends, not merely at the start: a declared
  # glob that only EXTENDS a required one (`ci/actionlint/**/*.sh` for the required
  # `ci/actionlint/**/*`) must still be reported missing. Losing the trailing newline anchor turns
  # the required glob into a mere PREFIX match, which is exactly the cost-driven "narrow the
  # gate's inputs" edit CLAUDE.md already warns about.
  expect_smoke_block 'an input that only EXTENDS a required glob does not satisfy it' \
    'missing-input ci/actionlint/**/*' \
    "$(rewrite_line "$wired" "      - ${q}ci/actionlint/**/*${q}" \
                             "      - ${q}ci/actionlint/**/*.sh${q}")"

  # The ERR record match must be anchored to the START of a record, not merely present anywhere in
  # the haystack. Losing that anchor lets a SCRIPT line whose own text happens to contain
  # `ERR<TAB>` be misread as an actual ERR record, which short-circuits the verdict to empty
  # before missing-input/missing-script are ever checked — silently waiving every requirement.
  local t; t="$(printf '\t')"
  expect_smoke_block 'a script line containing ERR<TAB> does not silence the verdict' \
    'missing-input CLAUDE.md' \
    "$(rewrite_line "$no_claude" '      set -euo pipefail' "      set -euo pipefail
      echo \"ERR${t}x\"")"

  # File-level verdicts, mirroring affected_graph_wiring_self_test's own two rows. The directory
  # case is portable because the guard is a bash builtin `test`, reached before any grep — there
  # is no BSD/GNU grep-on-a-directory split to disagree across platforms here.
  got="$(affected_smoke_block_verdict /nonexistent/moon.yml)"
  if [ "$got" != 'no-file' ]; then
    fail "affected-smoke-block self-test 'missing file': got '$got', expected 'no-file'. A
      renamed moon.yml must not be misread as every required input deleted."
    rc=1
  fi
  local unreadable_dir
  unreadable_dir="$(mktemp -d)"
  got="$(affected_smoke_block_verdict "$unreadable_dir")"
  rmdir "$unreadable_dir"
  if [ "$got" != 'no-file' ]; then
    fail "affected-smoke-block self-test 'directory in place of file': got '$got', expected
      'no-file'. A directory must not be read as every required input deleted."
    rc=1
  fi

  return $rc
}

# The standing control for check 8f (SMA-601). Both directions per SMA-466: a wired,
# correctly-ordered step is clean, and each required line, the run-block order, the ordering
# guarantee, continue-on-error, the `if:` ban and every script pin fire on their own.
#
# Fixtures are built in PURE BASH (heredocs and `${var/old/new}`), never with a `sed`
# substitution carrying a newline — that is a GNU extension BSD sed does not honour, and a
# fixture that silently fails to mutate on macOS is a self-test that proves nothing there.
cargo_lock_step_self_test() {
  SELF_TESTS_RAN=$((SELF_TESTS_RAN + 1))
  local rc=0 tmp got

  expect_step() { # $1 name  $2 expected-verdict  $3 body
    tmp="$(mktemp)"
    printf '%s' "$3" > "$tmp"
    got="$(cargo_lock_step_verdict "$tmp")"
    rm -f "$tmp"
    if [ "$got" != "$2" ]; then
      fail "cargo-lock-step self-test '$1': got '$got', expected '$2'. Check 8f is not
      deciding what it is documented to decide."
      rc=1
    fi
  }

  expect_script() { # $1 name  $2 expected-verdict  $3 body
    tmp="$(mktemp)"
    printf '%s' "$3" > "$tmp"
    got="$(cargo_lock_script_verdict "$tmp")"
    rm -f "$tmp"
    if [ "$got" != "$2" ]; then
      fail "cargo-lock-script self-test '$1': got '$got', expected '$2'. Check 8f's script pin
      is not deciding what it is documented to decide."
      rc=1
    fi
  }

  local wired
  wired="jobs:
  ci:
    steps:
      - name: Cargo lockfile integrity (rs/Cargo.lock satisfies every manifest)
        run: |
          set -euo pipefail
          bash ci/cargo-lock-integrity/run.sh --self-test
          bash ci/cargo-lock-integrity/run.sh --negative-control
          bash ci/cargo-lock-integrity/run.sh

      - name: moon ci (affected graph)
        run: moon ci
"
  expect_step 'a wired, correctly ordered step is clean' '' "$wired"

  # Placement IS the guarantee: run after `moon ci` and an unlocked task has already repaired
  # the lock, so an order-blind pin would be vacuous.
  local reordered
  reordered="jobs:
  ci:
    steps:
      - name: moon ci (affected graph)
        run: moon ci

      - name: Cargo lockfile integrity (rs/Cargo.lock satisfies every manifest)
        run: |
          set -euo pipefail
          bash ci/cargo-lock-integrity/run.sh --self-test
          bash ci/cargo-lock-integrity/run.sh --negative-control
          bash ci/cargo-lock-integrity/run.sh
"
  expect_step 'the step after moon ci is out of order' 'out-of-order' "$reordered"

  expect_step 'a missing real-run line is reported' \
    'missing-line bash ci/cargo-lock-integrity/run.sh' \
    "$(printf '%s' "$wired" | grep -vxF -e '          bash ci/cargo-lock-integrity/run.sh')"

  # I2: the two control modes are the only thing that catches a `--locked` deletion inside
  # run.sh, so dropping either one from ci.yml must red.
  expect_step 'a missing --self-test invocation is reported' \
    'missing-line bash ci/cargo-lock-integrity/run.sh --self-test' \
    "$(printf '%s' "$wired" | grep -vxF \
        -e '          bash ci/cargo-lock-integrity/run.sh --self-test')"

  expect_step 'a missing --negative-control invocation is reported' \
    'missing-line bash ci/cargo-lock-integrity/run.sh --negative-control' \
    "$(printf '%s' "$wired" | grep -vxF \
        -e '          bash ci/cargo-lock-integrity/run.sh --negative-control')"

  expect_step 'a missing set -euo pipefail is reported' \
    'missing-line set -euo pipefail' \
    "$(printf '%s' "$wired" | grep -vxF -e '          set -euo pipefail')"

  expect_step 'a missing name: line is reported, and nothing else' \
    'missing-line - name: Cargo lockfile integrity (rs/Cargo.lock satisfies every manifest)' \
    "$(printf '%s' "$wired" | grep -vxF \
        -e '      - name: Cargo lockfile integrity (rs/Cargo.lock satisfies every manifest)')"

  # `set -euo pipefail` moved below the invocations leaves every pinned line byte-identical
  # while a failing --self-test no longer aborts the block.
  local shuffled
  shuffled="jobs:
  ci:
    steps:
      - name: Cargo lockfile integrity (rs/Cargo.lock satisfies every manifest)
        run: |
          bash ci/cargo-lock-integrity/run.sh --self-test
          bash ci/cargo-lock-integrity/run.sh --negative-control
          set -euo pipefail
          bash ci/cargo-lock-integrity/run.sh

      - name: moon ci (affected graph)
        run: moon ci
"
  local shuffled_expected
  shuffled_expected='out-of-order-script bash ci/cargo-lock-integrity/run.sh --self-test
out-of-order-script bash ci/cargo-lock-integrity/run.sh --negative-control'
  expect_step 'set -euo pipefail below the invocations is out of order' \
    "$shuffled_expected" "$shuffled"

  # continue-on-error: true would let the step red and the job stay green — a silent bypass.
  local coe_true coe_false
  coe_true="${wired/          bash ci\/cargo-lock-integrity\/run.sh --self-test/          bash ci\/cargo-lock-integrity\/run.sh --self-test
        continue-on-error: true}"
  expect_step 'continue-on-error: true is reported' 'continue-on-error true' "$coe_true"

  coe_false="${wired/          bash ci\/cargo-lock-integrity\/run.sh --self-test/          bash ci\/cargo-lock-integrity\/run.sh --self-test
        continue-on-error: false}"
  expect_step 'continue-on-error: false is clean' '' "$coe_false"

  # SMA-601 review I1. An `if:` makes the step SKIP on the events it excludes, and a skipped
  # step is green — so the guarantee is off for exactly the PR event a Dependabot lock lands on.
  # Written out in full rather than derived with ${wired/.../...}: parameter-expansion
  # replacement undergoes quote removal, so the single quotes around 'push' — the realistic
  # form, and the one the reviewer measured — would be eaten before the fixture is written.
  local conditional
  conditional="jobs:
  ci:
    steps:
      - name: Cargo lockfile integrity (rs/Cargo.lock satisfies every manifest)
        if: github.event_name == 'push'
        run: |
          set -euo pipefail
          bash ci/cargo-lock-integrity/run.sh --self-test
          bash ci/cargo-lock-integrity/run.sh --negative-control
          bash ci/cargo-lock-integrity/run.sh

      - name: moon ci (affected graph)
        run: moon ci
"
  expect_step 'an if: on the step is reported' \
    "conditional github.event_name == 'push'" "$conditional"

  # And the other direction, so a stuck rule cannot masquerade as a working one: the wired
  # fixture above already carries no `if:` and is clean, and so is one whose `if:` belongs to a
  # DIFFERENT step further down.
  local if_elsewhere
  if_elsewhere="${wired/      - name: moon ci (affected graph)/      - name: moon ci (affected graph)
        if: always()}"
  expect_step 'an if: on a later step is not attributed to this one' '' "$if_elsewhere"

  # QUOTED KEYS. YAML lets a key be quoted, and GitHub honours `"if":` exactly as `if:`, so a
  # scan anchored on the bare spelling alone is a complete bypass of both rules. Found by
  # CodeRabbit in SMA-601's local review; before the normalisation these two fixtures were clean.
  local q_if q_coe
  # `always()` rather than a github.event_name comparison: the expression only has to be
  # non-empty for the rule, and a quote-free one keeps the fixture readable inside a
  # `${var/from/to}` replacement.
  q_if="${wired/        run: |/        \"if\": always()
        run: |}"
  expect_step 'a double-quoted "if" key is still reported' \
    'conditional always()' "$q_if"

  q_coe="${wired/        run: |/        'continue-on-error': true
        run: |}"
  expect_step "a single-quoted 'continue-on-error' key is still reported" \
    'continue-on-error true' "$q_coe"

  # SPACE BEFORE THE COLON — the same bypass class, one space wide. Both were clean before the
  # third normalising expression was added.
  local sp_if sp_coe
  sp_if="${wired/        run: |/        if : always()
        run: |}"
  expect_step 'a spaced "if :" key is still reported' 'conditional always()' "$sp_if"

  sp_coe="${wired/        run: |/        continue-on-error : true
        run: |}"
  expect_step 'a spaced "continue-on-error :" key is still reported' \
    'continue-on-error true' "$sp_coe"

  # EXPLICIT-KEY form. Measured: this parses to a real key and actionlint accepts it, so it
  # would clear check 1 and evade every same-line scan. Rejected outright.
  local ex_if ex_coe
  ex_if="${wired/        run: |/        ? if
        : always()
        run: |}"
  expect_step 'an explicit-key "? if" entry is rejected' 'explicit-key if' "$ex_if"

  ex_coe="${wired/        run: |/        ? continue-on-error
        : true
        run: |}"
  expect_step 'an explicit-key "? continue-on-error" entry is rejected' \
    'explicit-key continue-on-error' "$ex_coe"

  # ---- the script pin (T_CARGO_LOCK_SH_CALL_SITES) ----
  local script
  script="$(printf '%s\n' \
    'set -euo pipefail' \
    'classify_cargo_failure() {' \
    "  if grep -qF 'because --locked was passed to prevent this' \"\$1\"; then" \
    '    return 1' \
    '  fi' \
    '}' \
    'assert_lock_satisfies_manifests() {' \
    '  if ( cd "$dir" && cargo metadata --locked --format-version 1 >/dev/null ) 2>"$out"; then' \
    '    return 0' \
    '  fi' \
    '}' \
    'negative_control() {' \
    '  assert_lock_satisfies_manifests "$tmp/rs" || rc=$?' \
    '  case "$rc" in' \
    '    1) echo "cargo-lock-integrity --negative-control: reported red (rc=1) as expected" ;;' \
    '  esac' \
    '}' \
    'main() {' \
    '  case "${1:-}" in' \
    '    --self-test)        self_test; return $? ;;' \
    '    --negative-control) negative_control; return $? ;;' \
    '  esac' \
    '  assert_lock_satisfies_manifests "$RS_DIR" || rc=$?' \
    '}')"
  expect_script 'a wired run.sh is clean' '' "$script"

  # THE measured exploit: `--locked` gone from that one line makes cargo exit 0 AND repair the
  # lock, so the gate reports green and becomes the first repairer.
  local unlocked
  unlocked="${script/cargo metadata --locked --format-version 1/cargo metadata --format-version 1}"
  expect_script 'dropping --locked from the cargo line is reported' \
    'missing-site if ( cd "$dir" && cargo metadata --locked --format-version 1 >/dev/null ) 2>"$out"; then' \
    "$unlocked"

  # Neutering the flag parse makes both control modes fall through to the real run, which then
  # exits 0 having asserted nothing new.
  expect_script 'a deleted --self-test flag parse is reported' \
    'missing-site --self-test)        self_test; return $? ;;' \
    "$(printf '%s' "$script" | grep -vxF -e '    --self-test)        self_test; return $? ;;')"

  expect_script 'a deleted --negative-control flag parse is reported' \
    'missing-site --negative-control) negative_control; return $? ;;' \
    "$(printf '%s' "$script" \
        | grep -vxF -e '    --negative-control) negative_control; return $? ;;')"

  # The SMA-530 shape: a control that never calls the real assertion still prints its message.
  expect_script 'a control that no longer calls the assertion is reported' \
    'missing-site assert_lock_satisfies_manifests "$tmp/rs" || rc=$?' \
    "$(printf '%s' "$script" \
        | grep -vxF -e '  assert_lock_satisfies_manifests "$tmp/rs" || rc=$?')"

  expect_script 'a deleted control report arm is reported' \
    'missing-site 1) echo "cargo-lock-integrity --negative-control: reported red (rc=1) as expected" ;;' \
    "$(printf '%s' "$script" | grep -vxF \
        -e '    1) echo "cargo-lock-integrity --negative-control: reported red (rc=1) as expected" ;;')"

  expect_script 'a deleted real-run call is reported' \
    'missing-site assert_lock_satisfies_manifests "$RS_DIR" || rc=$?' \
    "$(printf '%s' "$script" \
        | grep -vxF -e '  assert_lock_satisfies_manifests "$RS_DIR" || rc=$?')"

  # A renamed script, or a directory left in its place, must not read as "every pin satisfied" —
  # six 'missing-site' rows would misdescribe one rename as six deliberate deletions, and
  # `[ -e ]` would let a DIRECTORY fall through to exactly that (the rule
  # affected_graph_wiring_verdict records).
  local scratch path name
  scratch="$(mktemp -d)"
  for name in absent directory; do
    if [ "$name" = absent ]; then path="$scratch/does-not-exist"; else path="$scratch"; fi
    got="$(cargo_lock_script_verdict "$path")"
    if [ "$got" != 'no-file' ]; then
      fail "cargo-lock-script self-test 'a $name script path is reported': got '$got',
      expected 'no-file'."
      rc=1
    fi
  done
  rmdir "$scratch"

  return "$rc"
}

# ---------------------------------------------------------------------------------------------
# Check 8d (definitions) — the "moon ci (affected graph)" step's `run:` block, extracted from
# ci.yml and EXECUTED once per GitHub event path against a stubbed `moon` (SMA-542 residual
# closure, PR 150 follow-up; closes README L12). Check 8b pins the three invocation LINES
# byte-for-byte; it has no view of the CONTROL FLOW around them, so wrapping the whole if/elif/else
# in an always-false outer conditional —
#
#   if false; then
#     if [ "$EVENT" = "pull_request" ]; then
#       moon ci "${T[@]}" --base origin/main --include-relations
#     ...
#     fi
#   fi
#
# — leaves all three lines byte-identical to T_INVOCATION_ALLOWLIST while nothing executes on any
# event path at all (CodeRabbit round 5, PR 150 — L12's own worked example; measured: full gate
# rc 0 under this shape before this check existed). Every OTHER verdict in checks 8/8b matches a
# fixed, enumerated TEXT shape (swallowed/wrapped/not-allowlisted) — the same reachability-analysis
# trap this file's own history warns against crossing (L9/L10/L11, and ci_targets.py's own L10
# comment on ACTIONLINT_SH_CALL_SITES). Rather than add a seventh enumerated shape to that list,
# this check sidesteps the analysis entirely: it runs the actual bash and asks whether `moon` was
# actually invoked — the one property that matters, on every path the step's own logic claims to
# support.
#
# SAFETY. This executes text extracted from a TRACKED, REVIEWED workflow file
# (.github/workflows/ci.yml) — the same trust boundary check 1 (actionlint) and every other check
# in this file already cross by reading it; it is not untrusted input. `moon` is stubbed in a fresh
# `mktemp -d` bin directory and put FIRST on a deliberately minimal PATH (the stub dir, then
# /usr/bin:/bin) BEFORE the block ever runs, so the real `moon` — wherever `proto install` put it —
# is never reachable, even if the block's own logic behaved unexpectedly. The block's own source
# (the step named "moon ci (affected graph)" in ci.yml) uses nothing besides `printf`/`grep`/
# `moon`; the first is a bash builtin and the second sits on that minimal PATH on both the macOS
# and Ubuntu runners this repo runs on.
# ---------------------------------------------------------------------------------------------

# A 40-zero and a 40-nonzero value, matching the two shapes GitHub's `github.event.before` takes on
# a push event: the literal all-zero SHA on a branch's first push (nothing to diff against), and a
# real SHA otherwise. Built with `printf '%040d'` rather than a hand-typed 40-character literal, so
# the length is not something to get wrong by hand — `$BEFORE`'s own `grep -qE '^0+$'` check in the
# real block cares about exactly this distinction.
MOON_STEP_BEFORE_ZERO="$(printf '%040d' 0)"
MOON_STEP_BEFORE_NONZERO="$(printf '%040d' 1)"

# One row per GitHub event path the step's own if/elif/else supports, as
# "<label>:<EVENT value>:<BEFORE value>:<expected moon subcommand>" — colon-separated, since none
# of these four fields can itself contain a colon. Bash 3.2 has no associative array to hold this
# instead, the same fixed-row-of-strings convention T_INVOCATION_ALLOWLIST/
# T_AFFECTED_GRAPH_CALL_SITES already use for a small table. <expected moon subcommand> plus
# <EVENT>/<BEFORE> is enough for block_execution_verdict, below, to DERIVE the full expected
# invocation independently, rather than comparing the block's behavior against a copy of its own
# logic — the same "re-derive the oracle" principle the rest of this check's design follows.
MOON_STEP_EVENT_PATHS=(
  "pull_request:pull_request::ci"
  "push-nonzero-before:push:${MOON_STEP_BEFORE_NONZERO}:ci"
  "push-zero-before:push:${MOON_STEP_BEFORE_ZERO}:run"
  "push-empty-before:push::run"
)

# The T=( … ) body, tokenized — a second, independent read of the SAME array check 8's
# ci_target_floor_verdict already extracts. Not shared by calling that function: it returns a list
# of PROBLEMS, not a list of tokens, and reshaping it would couple this check's fixtures to check
# 8's own ~25-row table for no reason (the same reason invocation_allowlist_verdict stayed a
# separate function from ci_target_floor_verdict). Duplicating the one-line sed costs the same kind
# of drift risk T_FLOOR/T_INVOCATION_ALLOWLIST/T_AFFECTED_GRAPH_CALL_SITES already accept as the
# price of not being the sole judge of your own configuration.
#
# Echoes nothing and returns 1 when the array cannot be read unambiguously (zero, or more than one,
# single-line 'T=( … )') — the SAME condition, by the SAME regex, as ci_target_floor_verdict's own
# 'no-array', so the two independently agree on when T cannot be trusted. Otherwise echoes one
# token per line and returns 0.
moon_target_array_tokens() {
  local f="$1" arrays body w
  arrays="$(grep -cE '^[[:blank:]]*T=\(.*\)[[:blank:]]*$' "$f")"
  case "$arrays" in ''|*[!0-9]*) return 1 ;; esac
  [ "$arrays" -eq 1 ] || return 1
  body="$(sed -nE 's/^[[:blank:]]*T=\((.*)\)[[:blank:]]*$/\1/p' "$f")"
  # set -f for the unquoted `$body` expansion, same reasoning and same no-op-today status as
  # ci_target_floor_verdict's own identical guard above.
  set -f
  for w in $body; do
    printf '%s\n' "$w"
  done
  set +f
  return 0
}

# Extracts the `run:` block body of the step whose name is EXACTLY "moon ci (affected graph)" —
# matched as stripped text, the same "read the file's shape, don't parse full YAML" approach
# extract_filter_keys takes above, not a general parser. Prints the block, dedented to column 0, to
# stdout on success. Prints NOTHING on any failure (no matching step, no run: found before the step
# ends) — this function only ever processes the FIRST such step, by design; block_execution_verdict
# below counts occurrences itself, FIRST, and never calls this unless that count is exactly one.
extract_moon_step_block() {
  local f="$1"
  awk '
    BEGIN { state = "seek-step" }
    {
      line = $0
      sub(/\r$/, "", line)                       # tolerate CRLF, same as extract_filter_keys
      match(line, /^[ ]*/); ind = RLENGTH
      stripped = line
      sub(/^[ ]*/, "", stripped)

      if (state == "seek-step") {
        if (stripped == "- name: moon ci (affected graph)") {
          step_ind = ind
          state = "seek-run"
        }
        next
      }

      if (state == "seek-run") {
        if (stripped == "") next
        if (ind <= step_ind) { exit }              # left the step with no run: key found
        if (stripped ~ /^run:[ \t]*[|>][-+]?([ \t]|$)/) {
          run_ind = ind
          state = "in-run"
          next
        }
        next                                       # some other step key (env:, if:, ...) — skip
      }

      # state == "in-run"
      if (stripped == "") { print ""; next }        # a blank line inside the block scalar
      if (ind > run_ind) {
        if (body_ind == "") body_ind = ind          # dedent relative to the FIRST body line
        print substr(line, body_ind + 1)
        next
      }
      exit                                          # dedented back out — block scalar is done
    }
  ' "$f"
}

# block_execution_verdict — the enforcement for check 8d. $1 is the workflow file.
#
# Echoes one verdict token per problem, and nothing for a healthy step:
#   no-file                      the workflow does not exist
#   count-unreadable              could not count occurrences of the step's name line (the same
#                                grep-fails-outright condition invocation_allowlist_verdict's own
#                                'count-unreadable' guards against — e.g. a permissions failure.
#                                NOT reliably a directory in place of the file: that reads as an
#                                outright read failure on BSD grep but as a well-formed zero on
#                                GNU grep, so it cannot be used to portably force this branch — see
#                                the WHY comment on block_step_count_verdict below)
#   no-step                      no step named exactly "moon ci (affected graph)" was found
#   multi-step <n>                that name was found more than once — which one guards T is
#                                ambiguous, so this check refuses to guess
#   no-run-block                  the step was found exactly once, but no `run: |`/`run: >` block
#                                could be extracted from it
#   no-target-array               the file has no single, unambiguous 'T=( … )' — the same
#                                condition ci_target_floor_verdict's own 'no-array' names, read
#                                independently here
#   setup-failed                  could not resolve a `bash` to execute the block with, could not
#                                create or truncate a scratch directory/file for one of the
#                                event-path runs, or could not read back a usable invocation count
#                                after running one (a malformed `wc`/`tr` result) — infrastructure,
#                                not a defect in ci.yml
#   zero-invocations <path>       the block, executed with EVENT/BEFORE set for <path>, never
#                                invoked `moon` at all — an outer `if false; then … fi` (or any
#                                other shape with the same effect) produces exactly this
#   wrong-count <path> <n>        <path> invoked `moon` <n> times, not once
#   bad-args <path>               <path> invoked `moon` exactly once, but not with the exact
#                                subcommand + WHOLE `T` array + `--base`/`--include-relations`
#                                shape that path requires. T_INVOCATION_ALLOWLIST (check 8b) checks
#                                each line against a SET of allowed forms with no notion of WHICH
#                                branch a line sits under, so three lines that individually match
#                                the set, sitting under the WRONG conditions (the `if` and `else`
#                                bodies swapped, say), pass 8b outright — count and per-line match
#                                both stay clean. This check derives the expected invocation from
#                                the path itself, so a swap like that is exactly what it catches. A
#                                same-line subset (`moon ci "${T[@]:0:5}" ...`) also lands here, but
#                                that specific shape is already caught by 8b's own invocation-count
#                                (it stops containing the literal `"${T[@]}"` substring) — this
#                                check catching it too is redundant-but-consistent with that path,
#                                not an exclusive closure of it.
#
# NEVER `infra` from inside this function (SMA-542 CodeRabbit round 4 finding F1, and the SAME bug
# reopened once already on invocation_allowlist_verdict): it is invoked at the production call site
# below as `done < <(block_execution_verdict ...)`, so it runs inside that process substitution's
# OWN subshell — an `exit 2` there would exit only the subshell, FAILED would never be set, and the
# gate would finish rc 0 having asserted nothing. Every genuine infrastructure failure inside this
# function echoes 'setup-failed' and returns instead; the call site is what turns that into an
# actual `infra` exit, from the main shell, where it works.

# The single source of truth for turning ONE path's captured invocation count into a verdict
# token. Extracted from the per-path loop below for the same reason mutant_is_killed (check 9,
# further down) is extracted from its own collection loop: a real `wc`/`tr` failure that leaves
# `$n` malformed is not something a portable, root-safe self-test can reliably force live (a
# scratch directory made read-only is a no-op under root, per this file's own existing
# unreadable_dir caveat elsewhere), so the DECISION is pulled out where a fixture table can drive
# it directly with a synthetic, already-malformed `$n` instead.
#
# $1 label, $2 the captured count text (possibly malformed), $3 actual logged args (only
# meaningful when $2 is exactly "1"), $4 the expected args for this path.
invocation_count_verdict() {
  local label="$1" n="$2" actual="$3" expected="$4"
  # Hardened exactly like $arrays/$defs/$n elsewhere in this file (ci_target_floor_verdict,
  # run_self_tests, selftest_mutation_battery): without this, a `wc`/`tr` failure leaves $n empty
  # (or non-numeric), and the numeric `case` below falls through to its `*)` arm — reporting
  # 'wrong-count <label> ' with an EMPTY count, which blames ci.yml for what is actually an
  # environment failure (CodeRabbit, PR 150, finding F1).
  case "$n" in
    ''|*[!0-9]*) echo 'setup-failed'; return ;;
  esac
  case "$n" in
    0) echo "zero-invocations $label" ;;
    1) [ "$actual" = "$expected" ] || echo "bad-args $label" ;;
    *) echo "wrong-count $label $n" ;;
  esac
}

# The single source of truth for turning the step-name occurrence count into a verdict token (or
# nothing, for exactly one match). Extracted out of block_execution_verdict below for the same
# reason invocation_allowlist_count_verdict was extracted from invocation_allowlist_verdict above:
# a self-test needs to drive a malformed count directly, not by forcing a real `grep -c` read
# failure via a directory in place of the file. That trick is not portable — BSD grep (macOS)
# fails outright reading a directory and writes nothing to stdout, landing on 'count-unreadable'
# below, while GNU grep (Linux, what CI runs) prints a literal '0' for the same input, landing on
# 'no-step' instead (a DIFFERENT verdict — the same platform split as
# invocation_allowlist_count_verdict's comment above, and the second of the two fixtures that
# actually reproduced it: reds Linux CI, passes macOS, SMA-542 / CodeRabbit review of PR 150). A
# malformed synthetic string never touches grep at all.
#
# $1 the captured step-name occurrence count (possibly malformed/non-numeric).
block_step_count_verdict() {
  local n="$1"
  case "$n" in ''|*[!0-9]*) echo 'count-unreadable'; return ;; esac
  if [ "$n" -eq 0 ]; then echo 'no-step'; return; fi
  if [ "$n" -gt 1 ]; then echo "multi-step $n"; return; fi
}

block_execution_verdict() {
  local f="$1" step_count run_block bash_bin step_verdict
  local -a t_arr
  local t_joined tok row label event before sub expected
  local bindir logf actual n verdict_out

  [ -e "$f" ] || { echo 'no-file'; return; }

  step_count="$(grep -cE '^[[:blank:]]*- name: moon ci \(affected graph\)[[:blank:]]*$' "$f")"
  step_verdict="$(block_step_count_verdict "$step_count")"
  if [ -n "$step_verdict" ]; then
    echo "$step_verdict"
    return
  fi

  run_block="$(extract_moon_step_block "$f")"
  if [ -z "$run_block" ]; then echo 'no-run-block'; return; fi

  t_arr=()
  while IFS= read -r tok; do
    t_arr+=("$tok")
  done < <(moon_target_array_tokens "$f")
  if [ "${#t_arr[@]}" -eq 0 ]; then echo 'no-target-array'; return; fi
  t_joined="${t_arr[*]}"

  # Resolved via the CURRENT, unrestricted PATH — before the per-row loop below ever narrows it.
  # `PATH=narrow bash -c ...` would make THIS shell's own command-word lookup for `bash` use the
  # narrowed value too (measured: `PATH=/nonexistent bash -c 'echo hi'` reports "bash: command not
  # found" from the OUTER shell, never even reaching the inner one) — resolving to an absolute
  # path first and invoking THAT sidesteps PATH lookup for the exec itself, so only the block's OWN
  # internal commands (printf/grep/moon) are subject to the minimal PATH below.
  bash_bin="$(command -v bash)"
  if [ -z "$bash_bin" ]; then echo 'setup-failed'; return; fi

  # ONE scratch directory and ONE stub script for all four paths below, not one per path — this
  # function runs inside check 9's mutation battery, which re-invokes the WHOLE of `--self-test`
  # (this self-test's every fixture, every path) up to ten times concurrently, so a `mktemp -d` +
  # `chmod` per path multiplied that cost by 4x for no benefit: the stub's CONTENT never varies
  # across paths, only which log file it appends to (MOON_STUB_LOG, an env var, not baked into the
  # script). The log itself is a fixed path inside this same scratch dir, truncated with a shell
  # redirection (a builtin, not a subprocess) at the top of each iteration instead of re-created
  # with `mktemp`.
  bindir="$(mktemp -d)" || { echo 'setup-failed'; return; }
  logf="$bindir/invocation.log"
  # The stub: logs its OWN full argument list (never "moon" itself, which is $0 from its own point
  # of view) and exits 0 unconditionally — a stubbed `moon` that could fail would make a
  # 'zero-invocations' verdict ambiguous between "never called" and "called, then failed".
  # Both the write and the chmod are checked, for the same reason the count below is: an
  # unexecutable stub leaves NO `moon` on the narrow PATH, every path then logs nothing, and the
  # loop would report 'zero-invocations' — blaming ci.yml and citing README L12 for what is
  # actually an environment failure. Same misdiagnosis class as an unreadable count (CodeRabbit
  # CLI review of PR 150).
  if ! cat > "$bindir/moon" <<'STUB'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "$MOON_STUB_LOG"
exit 0
STUB
  then
    echo 'setup-failed'
    rm -rf "$bindir"
    return
  fi
  if ! chmod +x "$bindir/moon"; then
    echo 'setup-failed'
    rm -rf "$bindir"
    return
  fi

  for row in "${MOON_STEP_EVENT_PATHS[@]}"; do
    IFS=: read -r label event before sub <<< "$row"

    case "$sub" in
      ci)
        if [ "$event" = "pull_request" ]; then
          expected="ci $t_joined --base origin/main --include-relations"
        else
          expected="ci $t_joined --base $before --include-relations"
        fi ;;
      run)
        expected="run $t_joined" ;;
    esac

    # Truncating (not re-`mktemp`ing) the log is what makes 'setup-failed' reachable here: without
    # this check, a failed truncation (a full disk, a scratch dir removed out from under this
    # function) leaves the PREVIOUS iteration's log in place — `wc -l` still succeeds, on stale
    # content, and the verdict silently describes the WRONG path.
    if ! : > "$logf"; then
      echo 'setup-failed'
      rm -rf "$bindir"
      return
    fi

    # PATH restricted to the stub dir FIRST, then /usr/bin:/bin ONLY — deliberately minimal, so the
    # real `moon` (wherever proto installed it) can never be found even if the block's own PATH
    # handling behaved unexpectedly. The block's own source uses nothing besides printf (a bash
    # builtin, not subject to PATH at all) and grep (present at /usr/bin/grep on macOS and at
    # /usr/bin/grep or /bin/grep on the Ubuntu runner this repo's CI uses).
    PATH="$bindir:/usr/bin:/bin" MOON_STUB_LOG="$logf" EVENT="$event" BEFORE="$before" \
      "$bash_bin" -c "$run_block" >/dev/null 2>&1

    n="$(wc -l < "$logf" | tr -d ' ')"
    actual="$(cat "$logf" 2>/dev/null)"
    verdict_out="$(invocation_count_verdict "$label" "$n" "$actual" "$expected")"
    [ -n "$verdict_out" ] && echo "$verdict_out"
    case "$verdict_out" in
      setup-failed) rm -rf "$bindir"; return ;;
    esac
  done

  rm -rf "$bindir"
}

# The standing control for check 8d. Both directions per SMA-466: the real if/elif/else shape
# passes on all four event paths, that SAME shape wrapped in `if false; then … fi` (this check's
# own motivating case, L12) fires on all four, a subsetted "${T[@]:0:1}" fires on all four, the
# `if`/`else` BODIES SWAPPED (each line individually still matches T_INVOCATION_ALLOWLIST's SET,
# under the wrong condition — invisible to check 8b's position-blind matching) fires on the three
# paths that land in the wrong branch, a duplicated invocation on ONE path fires only on that path,
# and a HEALTHY re-run passes again after the two most control-flow-sensitive mutations (if-false,
# branch-swap) and once more at the very end, aggregating the rest — proving no state leaks between
# calls (each call creates its own mktemp scratch files and touches no global except
# SELF_TESTS_RAN, so one aggregate proof at the end is as strong as one after every row).
block_execution_self_test() {
  SELF_TESTS_RAN=$((SELF_TESTS_RAN + 1))
  local rc=0 tmp got

  expect_block() {
    local name="$1" expected="$2" body="$3"
    tmp="$(mktemp)"
    printf '%s' "$body" > "$tmp"
    got="$(block_execution_verdict "$tmp")"
    rm -f "$tmp"
    if [ "$got" != "$expected" ]; then
      fail "block-execution self-test '$name' mismatch.
--- expected ---
$expected
--- actual ---
$got"
      rc=1
    fi
  }

  # invocation_count_verdict, driven directly with synthetic counts (CodeRabbit, PR 150, finding
  # F1). A real `wc`/`tr` failure that leaves the captured count empty or non-numeric is not
  # something a portable, root-safe fixture can force live — a scratch directory made read-only is
  # a no-op under root, the same caveat this file's own unreadable_dir fixtures already document —
  # so this proves the DECISION directly instead: 'setup-failed' for a malformed count (empty, or
  # carrying a non-digit), never silently read as the numeric `case`'s `*)` arm ('wrong-count'
  # with an EMPTY number, which would misdiagnose an environment failure as a ci.yml defect).
  expect_count() {
    local name="$1" expected="$2" n="$3" actual="${4:-}" want="${5:-}"
    got="$(invocation_count_verdict 'some-path' "$n" "$actual" "$want")"
    if [ "$got" != "$expected" ]; then
      fail "block-execution self-test '$name': invocation_count_verdict('$n') returned '$got',
      expected '$expected'."
      rc=1
    fi
  }
  expect_count 'an empty count is setup-failed, not an empty wrong-count' \
    'setup-failed' ''
  expect_count 'a non-numeric count is setup-failed' 'setup-failed' 'abc'
  expect_count 'a count with a trailing non-digit is setup-failed' 'setup-failed' '12x'
  expect_count 'a genuine zero count is zero-invocations, not setup-failed' \
    'zero-invocations some-path' '0'
  expect_count 'a genuine one-count matching the expected args is clean' \
    '' '1' 'ci :a --base origin/main --include-relations' 'ci :a --base origin/main --include-relations'
  expect_count 'a genuine one-count NOT matching the expected args is bad-args' \
    'bad-args some-path' '1' 'ci :a' 'ci :a --base origin/main --include-relations'
  expect_count 'a genuine count above one is wrong-count, with the real number' \
    'wrong-count some-path 3' '3'

  # The healthy control: a small, faithful copy of the real if/elif/else shape (including the
  # zero-SHA grep check the elif condition needs — without it, "40 zeros" would read as merely
  # non-empty and misroute to the wrong branch) with a 3-entry T so the fixture stays short. Every
  # OTHER fixture below is a mutation of this exact body, so a failure here would mean the fixture
  # itself is wrong, not the check.
  local healthy='name: t
on:
  push:
    branches:
      - main
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - name: moon ci (affected graph)
        env:
          EVENT: ${{ github.event_name }}
          BEFORE: ${{ github.event.before }}
        run: |
          set -euo pipefail
          T=(:a :b :c)
          if [ "$EVENT" = "pull_request" ]; then
            moon ci "${T[@]}" --base origin/main --include-relations
          elif [ -n "${BEFORE:-}" ] && ! printf '"'"'%s'"'"' "$BEFORE" | grep -qE '"'"'^0+$'"'"'; then
            moon ci "${T[@]}" --base "$BEFORE" --include-relations
          else
            moon run "${T[@]}"
          fi
'
  expect_block 'a healthy step invokes moon exactly once, correctly, on all four event paths' \
    '' "$healthy"

  # THE motivating case (CodeRabbit round 5, PR 150 — README L12): the whole if/elif/else wrapped
  # in an always-false outer conditional. T_INVOCATION_ALLOWLIST's three lines stay byte-identical
  # to the allowed forms; only EXECUTING the block reveals that nothing ever runs.
  local if_false='name: t
on:
  push:
    branches:
      - main
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - name: moon ci (affected graph)
        env:
          EVENT: ${{ github.event_name }}
          BEFORE: ${{ github.event.before }}
        run: |
          set -euo pipefail
          T=(:a :b :c)
          if false; then
          if [ "$EVENT" = "pull_request" ]; then
            moon ci "${T[@]}" --base origin/main --include-relations
          elif [ -n "${BEFORE:-}" ] && ! printf '"'"'%s'"'"' "$BEFORE" | grep -qE '"'"'^0+$'"'"'; then
            moon ci "${T[@]}" --base "$BEFORE" --include-relations
          else
            moon run "${T[@]}"
          fi
          fi
'
  expect_block 'an always-false outer conditional produces zero invocations on every path (L12)' \
"$(printf 'zero-invocations pull_request\nzero-invocations push-nonzero-before\nzero-invocations push-zero-before\nzero-invocations push-empty-before')" \
    "$if_false"

  expect_block 'a healthy step still passes after the if-false mutation' '' "$healthy"

  # A subsetted "${T[@]:0:1}" expansion in all three invocations. This shape is ALSO caught by
  # check 8b's own invocation-count (a subset no longer contains the literal '"${T[@]}"'
  # substring, so the count drops) — this fixture is not proving an exclusive gap, only that this
  # check independently reaches the same conclusion by executing the block rather than counting
  # substrings. The branch-swap fixture below is the one that is genuinely invisible to 8b.
  local subset='name: t
on:
  push:
    branches:
      - main
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - name: moon ci (affected graph)
        env:
          EVENT: ${{ github.event_name }}
          BEFORE: ${{ github.event.before }}
        run: |
          set -euo pipefail
          T=(:a :b :c)
          if [ "$EVENT" = "pull_request" ]; then
            moon ci "${T[@]:0:1}" --base origin/main --include-relations
          elif [ -n "${BEFORE:-}" ] && ! printf '"'"'%s'"'"' "$BEFORE" | grep -qE '"'"'^0+$'"'"'; then
            moon ci "${T[@]:0:1}" --base "$BEFORE" --include-relations
          else
            moon run "${T[@]:0:1}"
          fi
'
  expect_block 'a subsetted "${T[@]:0:1}" expansion is a bad-args mismatch on every path' \
"$(printf 'bad-args pull_request\nbad-args push-nonzero-before\nbad-args push-zero-before\nbad-args push-empty-before')" \
    "$subset"

  # THE case that is genuinely invisible to check 8b: the `if` and `else` BODIES swapped, `elif`
  # left untouched. Every invocation line still matches SOME entry of T_INVOCATION_ALLOWLIST — the
  # set of three allowed forms doesn't care which branch a line sits under — and the count is still
  # 3, so check 8b sees nothing wrong at all (measured live against a mutated .github/workflows/
  # ci.yml during this check's development: rc 0 from checks 8/8b, only this check fired). The
  # `elif` branch is untouched, so the push-nonzero-before path stays correct; the other three land
  # in the wrong branch.
  local branch_swap='name: t
on:
  push:
    branches:
      - main
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - name: moon ci (affected graph)
        env:
          EVENT: ${{ github.event_name }}
          BEFORE: ${{ github.event.before }}
        run: |
          set -euo pipefail
          T=(:a :b :c)
          if [ "$EVENT" = "pull_request" ]; then
            moon run "${T[@]}"
          elif [ -n "${BEFORE:-}" ] && ! printf '"'"'%s'"'"' "$BEFORE" | grep -qE '"'"'^0+$'"'"'; then
            moon ci "${T[@]}" --base "$BEFORE" --include-relations
          else
            moon ci "${T[@]}" --base origin/main --include-relations
          fi
'
  expect_block 'the if/else bodies swapped is bad-args on every path except the untouched elif' \
"$(printf 'bad-args pull_request\nbad-args push-zero-before\nbad-args push-empty-before')" \
    "$branch_swap"

  expect_block 'a healthy step still passes after the branch-swap mutation' '' "$healthy"

  # A duplicated invocation on ONE branch only — the other three paths stay correct, proving
  # 'wrong-count' is reported per path, not as a whole-file verdict.
  local double_invoke='name: t
on:
  push:
    branches:
      - main
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - name: moon ci (affected graph)
        env:
          EVENT: ${{ github.event_name }}
          BEFORE: ${{ github.event.before }}
        run: |
          set -euo pipefail
          T=(:a :b :c)
          if [ "$EVENT" = "pull_request" ]; then
            moon ci "${T[@]}" --base origin/main --include-relations
            moon ci "${T[@]}" --base origin/main --include-relations
          elif [ -n "${BEFORE:-}" ] && ! printf '"'"'%s'"'"' "$BEFORE" | grep -qE '"'"'^0+$'"'"'; then
            moon ci "${T[@]}" --base "$BEFORE" --include-relations
          else
            moon run "${T[@]}"
          fi
'
  expect_block 'a duplicated invocation on one path only is a wrong-count on that path alone' \
    'wrong-count pull_request 2' "$double_invoke"

  expect_block 'a step named differently is no-step' 'no-step' \
'name: t
on:
  push:
    branches:
      - main
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - name: some other step
        run: |
          echo hi
'

  expect_block 'the step name appearing twice is multi-step, not a guess at which one' \
    'multi-step 2' \
'name: t
on:
  push:
    branches:
      - main
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - name: moon ci (affected graph)
        run: |
          echo hi
      - name: moon ci (affected graph)
        run: |
          echo hi
'

  expect_block 'a step with no run: block at all is no-run-block' 'no-run-block' \
'name: t
on:
  push:
    branches:
      - main
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - name: moon ci (affected graph)
        uses: some/action@v1
'

  expect_block 'a run: block with no T=( … ) at all is no-target-array' 'no-target-array' \
'name: t
on:
  push:
    branches:
      - main
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - name: moon ci (affected graph)
        run: |
          echo hi
'

  got="$(block_execution_verdict /nonexistent/ci.yml)"
  if [ "$got" != 'no-file' ]; then
    fail "block-execution self-test 'missing file': got '$got', expected 'no-file'."
    rc=1
  fi

  # count-unreadable, driven directly through block_step_count_verdict with synthetic malformed
  # counts — NOT via a directory in place of the file. A directory used to sit here (same
  # "distinct verdict, not a silent skip" requirement, proven without chmod, which can silently
  # no-op when tests run as root), on the theory that it portably forces a `grep -c` read failure.
  # It does not: BSD grep (macOS) fails outright reading a directory and writes nothing to stdout,
  # landing on 'count-unreadable' below, but GNU grep (Linux, what CI actually runs) prints a
  # literal '0' for the same directory argument — a well-formed-but-wrong count that instead lands
  # on 'no-step'. That split is exactly what reds this fixture on Linux CI while passing on a
  # macOS dev box (SMA-542, CodeRabbit review of PR 150 — the sibling of
  # invocation_allowlist_count_verdict's own fix above; see its WHY comment for the measured
  # behaviour). A malformed synthetic string never touches grep at all.
  local n
  for n in '' 'x' ' '; do
    got="$(block_step_count_verdict "$n")"
    if [ "$got" != 'count-unreadable' ]; then
      fail "block-execution self-test 'unreadable step count (n=\"$n\")': got '$got', expected
      'count-unreadable'. A malformed count must not be silently read as zero step occurrences."
      rc=1
    fi
  done

  # One final aggregate re-run, covering everything since the if-false/branch-swap rechecks above:
  # the subset, duplicate-invocation, no-step, multi-step, no-run-block, no-target-array and
  # missing-file fixtures. None of those mutate global state (every call above gets its own mktemp
  # scratch file/directory), so one aggregate proof here — rather than a dedicated recheck wedged
  # after each individual row — is enough to show nothing accumulated.
  expect_block 'a healthy step still passes after every mutation above' '' "$healthy"

  return $rc
}

# ---------------------------------------------------------------------------------------------
# Check 9's kill predicate (SMA-542 review M3 / spec T3) — extracted so it can be driven directly
# by a fixture table instead of living inline in the mutant-collection loop below, where nothing
# proved it. It is correct today; what was missing is the STANDING proof, in a file that cites
# SMA-466's all-firing-fixture lesson five times over.
#
# A mutant counts as killed only when it exits 1 (fail()'s exit code) AND its captured output
# carries assert_self_tests_ran's own distinctive message. rc 2 (infra()), rc 126 and rc 127
# (a missing or unexecutable file) must NEVER score as a kill — scoring an infrastructure abort as
# a kill would let the battery report "N/N killed" without ever having exercised the counter
# assertion at all (SMA-542 D10).
# ---------------------------------------------------------------------------------------------
mutant_is_killed() {
  local rc="$1" outfile="$2"
  [ "$rc" -eq 1 ] && grep -q 'self-test counter:' "$outfile"
}

# The ninth (and last-called) self-test. Synthetic (rc, captured-output) pairs, no subprocess and
# no actionlint binary needed — same style as config_self_test's expect_config above.
kill_predicate_self_test() {
  local rc=0
  SELF_TESTS_RAN=$((SELF_TESTS_RAN + 1))

  expect_kill() {
    local name="$1" want="$2" mrc="$3" body="$4" tmp got
    tmp="$(mktemp)"
    printf '%s' "$body" > "$tmp"
    if mutant_is_killed "$mrc" "$tmp"; then got=killed; else got=not-killed; fi
    rm -f "$tmp"
    if [ "$got" != "$want" ]; then
      fail "kill-predicate self-test '$name': got '$got', expected '$want'. Check 9's kill
      predicate is not deciding what it is documented to decide."
      rc=1
    fi
  }

  expect_kill 'rc 1 with the counter message is a kill' 'killed' 1 \
    'actionlint gate: self-test counter: 4 of 7 self-tests ran. An invocation is missing.'
  # rc 2/126/127 must NEVER be a kill, even carrying the exact message — an infra abort proves
  # nothing about the assertion, and must not be mistaken for having reached it (SMA-542 D10).
  expect_kill 'rc 2 (infra abort) is never a kill, even with the message' 'not-killed' 2 \
    'actionlint gate: self-test counter: 4 of 7 self-tests ran. An invocation is missing.'
  expect_kill 'rc 126 (not executable) is never a kill' 'not-killed' 126 ''
  expect_kill 'rc 127 (missing file) is never a kill' 'not-killed' 127 ''
  # rc 1 without the message is not a kill either — some OTHER fail() fired, not the counter's.
  expect_kill 'rc 1 without the counter message is not a kill' 'not-killed' 1 \
    'actionlint gate: some unrelated assertion failed'
  expect_kill 'rc 0 (mutant did not fail at all) is never a kill' 'not-killed' 0 ''

  return $rc
}

# ---------------------------------------------------------------------------------------------
# Check 10 (SMA-579) — the release guard. The VERDICT lives in ci/actionlint/release_guard.py
# because it needs YAML STRUCTURE: a job-level `if:` must be told from the eight identical
# step-level ones release.yml carries, and `needs:` chains must be walked. Neither is a
# line-oriented question, and SMA-593 is the standing evidence that a hand-rolled scanner for
# this class rots into a control that lies.
#
# WHY A BASH WRAPPER AT ALL. Check 7 counts bash `*_self_test` DEFINITIONS and check 9 mutates
# lines inside run_self_tests — both see bash only. A Python fixture table is invisible to them,
# so EMPTYING it would leave this gate passing having asserted nothing. The arity floor below is
# what closes that, exactly as check 8e's two floors do for its own tables.
#
# THE FLOOR MUST TRACK THE TABLE (SMA-602 final review, Minor 1). It sat at 20 against an actual
# 98, so 78 of the 98 rows — the whole V10 credential control among them — could be deleted with
# the gate still green. FIXTURES is the ONLY pin on that control. Re-baseline the floor whenever
# rows are added: set it just under the current `--fixture-count`, never far below it.
# `--locked` (Minor 5, fix round 3): a bare `uv run --project py` RE-LOCKS py/uv.lock as a side
# effect of running the gate. CLAUDE.md already records py/uv.lock as one of the two sites that
# drift SILENTLY, and a lint gate that rewrites a lockfile it is not asserting is exactly that
# drift. `--locked` makes uv fail instead of writing. That failure is nonzero but not 2, which is
# precisely the class check 10's `elif` arm now routes — before it, a stale lock would have made
# this gate pass having asserted nothing.
release_guard_py() {
  uv run --locked --project py python3 ci/actionlint/release_guard.py "$@"
}

release_guard_self_test() {
  local rc=0 n
  SELF_TESTS_RAN=$((SELF_TESTS_RAN + 1))

  n="$(release_guard_py --fixture-count)" || infra "check 10: release_guard.py --fixture-count failed"
  case "$n" in ''|*[!0-9]*) infra "check 10: --fixture-count printed '$n', expected an integer" ;; esac
  [ "$n" -ge 105 ] || infra "check 10: release_guard.py reports $n fixtures, expected at least 105"

  release_guard_py --self-test || { fail "check 10: release_guard.py --self-test reported a broken
      verdict. The release guard is not deciding what it is documented to decide."; rc=1; }

  return $rc
}

# ---------------------------------------------------------------------------------------------
# Check 11 (SMA-603) — the release-plan decision. Same bash-wrapper rationale as check 10: check
# 7 counts bash `*_self_test` DEFINITIONS and check 9 mutates lines inside run_self_tests, so a
# Python fixture table is invisible to both. Emptying it would leave this gate passing having
# asserted nothing; the arity floor closes that.
# ---------------------------------------------------------------------------------------------
release_plan_sh() {
  bash ci/release-plan/run.sh "$@"
}

release_plan_self_test() {
  local rc=0 n
  SELF_TESTS_RAN=$((SELF_TESTS_RAN + 1))

  # Bypasses release_plan_sh (ci/release-plan/run.sh) on purpose: that wrapper's flag parser
  # rejects anything but --self-test/--negative-control/--assert/--github-output, dying with
  # `die_infra "unknown flag"` on --fixture-count. `--locked` mirrors check 10's release_guard_py
  # wrapper (see the comment above it) — inert today since ci/release-plan is zero-dependency,
  # live the moment that project gains one. Verified against the current lock: exits 0.
  n="$(uv run --locked --project ci/release-plan --python '>=3.12' python3 \
    ci/release-plan/release_plan.py --fixture-count)" \
    || infra "check 11: release_plan.py --fixture-count failed"
  case "$n" in ''|*[!0-9]*) infra "check 11: --fixture-count printed '$n', expected an integer" ;; esac
  # Floor, not a count: it exists to catch an EMPTIED table, and one row of headroom keeps a
  # legitimate row removal from aborting the gate as infra. Check 10's own floor is equally
  # loose (20 against 84 actual — that citation read 44 until the SMA-603 fix wave; the table
  # has grown with every V8/V9 round since).
  [ "$n" -ge 8 ] || infra "check 11: release_plan.py reports $n fixtures, expected at least 8"

  release_plan_sh --self-test || { fail "check 11: release_plan.py --self-test reported a broken
      verdict. The release-plan decision is not deciding what it is documented to decide."; rc=1; }

  release_plan_sh --negative-control || { fail "check 11: ci/release-plan/run.sh
      --negative-control failed. The control that proves the checker can report each direction is
      itself broken."; rc=1; }

  return $rc
}

# ---------------------------------------------------------------------------------------------
# Check 7 — the self-tests, and the counter that proves they were invoked.
#
# All THIRTEEN are defined above so this block can run them from ONE call site, reached by both the
# --self-test path and the full gate. One call site rather than two is deliberate: ci_targets.py's
# C4 pins this by whole stripped line, and two identical lines would let one be deleted while the
# pin still matched (SMA-542 D2).
# ---------------------------------------------------------------------------------------------
assert_self_tests_ran() {
  local want="$1"
  if [ "$SELF_TESTS_RAN" -ne "$want" ]; then
    fail "self-test counter: $SELF_TESTS_RAN of $want self-tests ran. An invocation is missing from
      run_self_tests. A fixture table that is never called is dead code that guards nothing — this
      is the failure SMA-542 exists to catch. Restore the call."
  fi
}

run_self_tests() {
  local defs
  SELF_TESTS_RAN=0

  extractor_self_test
  path_filter_self_test
  branch_filter_self_test
  config_self_test
  ci_target_floor_self_test
  invocation_allowlist_self_test
  affected_graph_wiring_self_test
  block_execution_self_test
  kill_predicate_self_test
  affected_smoke_block_self_test
  release_guard_self_test
  cargo_lock_step_self_test
  release_plan_self_test

  assert_self_tests_ran "$SELF_TEST_COUNT"

  # The counter proves the KNOWN tables ran; it cannot notice a table added tomorrow and never
  # wired up, because the count would still match. Asserting the DEFINITION count closes that —
  # adding a table without calling it reds, and so does deleting one without decrementing
  # SELF_TEST_COUNT. Adding a table is the highest-probability future edit here (SMA-542 D13).
  #
  # Tolerant of blank-before-paren (`tenth_self_test () {`) and the `function` keyword form
  # (`function tenth_self_test {`, with or without `()`) — a table written either way must still
  # be counted, or D13's own hole reopens for the style it does not recognise (SMA-542 review M8).
  # Not tolerant of a definition split across lines (`tenth_self_test()\n{`) — a rarer style this
  # file uses nowhere today; the residual is accepted rather than chasing every valid bash form.
  [ -f "$SELF_SRC" ] && [ -r "$SELF_SRC" ] \
    || infra "cannot read \$SELF_SRC ($SELF_SRC) to count self-test definitions"
  defs="$(grep -cE '^(function[[:blank:]]+)?[a-z_]+_self_test([[:blank:]]*\(\))?[[:blank:]]*\{' "$SELF_SRC")"
  case "$defs" in ''|*[!0-9]*) infra "could not count self-test definitions in $SELF_SRC" ;; esac
  if [ "$defs" -ne "$SELF_TEST_COUNT" ]; then
    fail "self-test definitions: $defs '*_self_test' functions are defined but SELF_TEST_COUNT is
      $SELF_TEST_COUNT. A fixture table that is not called from run_self_tests guards nothing.
      Wire it up and bump SELF_TEST_COUNT, or delete it."
  fi

  # branch_filter_self_test reaches its 'no-origin-main' fixture by swapping ORIGIN_REFS for a
  # main-free list and forcing ORIGIN_REFS_LOADED=1, restoring both immediately. That was harmless
  # while the self-tests ran LAST; now they run FIRST, and load_origin_refs early-returns on
  # ORIGIN_REFS_LOADED=1 — so a future botched restore would feed checks 5/6 a fake ref list and
  # turn every real branches: entry into a false 'unresolved' or an infra exit. Resetting here
  # makes checks 5/6 independent of that fixture's bookkeeping rather than trusting it.
  ORIGIN_REFS_LOADED=0
}

# ---------------------------------------------------------------------------------------------
# Check 9 — prove the counter actually fires, by mutation rather than assertion.
#
# SMA-525's F4: a one-off mutation battery is not a standing control. So the battery runs in CI.
#
# NO RECURSION, BY CONSTRUCTION: mutants are invoked with --self-test, which exits before this
# function is reached. That is why nothing here needs a bypass env var (which would be a live
# switch for turning the gate off) and why each mutant deletes exactly ONE line (SMA-542 D5).
#
# The invocation list is DERIVED from run_self_tests' own body, not hardcoded: adding a table
# extends the battery automatically, and a mismatch against SELF_TEST_COUNT reds.
# ---------------------------------------------------------------------------------------------
selftest_mutation_battery() {
  local dir lines line n removed rc i label pid
  local pids='' labels=''

  [ -f "$SELF_SRC" ] && [ -r "$SELF_SRC" ] || infra "check 9: cannot read \$SELF_SRC ($SELF_SRC)"

  lines="$(awk '
    /^run_self_tests\(\) \{$/ { inside = 1; next }
    inside && /^\}$/          { exit }
    inside && /^  [a-z_]+_self_test$/ { print $1 }
  ' "$SELF_SRC")"
  n="$(printf '%s\n' "$lines" | grep -c '[^[:space:]]')"
  case "$n" in ''|*[!0-9]*) infra "check 9: could not count self-test invocations in $SELF_SRC" ;; esac
  if [ "$n" -ne "$SELF_TEST_COUNT" ]; then
    fail "check 9: found $n self-test invocations inside run_self_tests, expected
      \$SELF_TEST_COUNT=$SELF_TEST_COUNT. Either a call is missing or the count is stale."
    return
  fi

  dir="$(mktemp -d)" || infra "check 9: mktemp -d failed"
  # Expanded at trap-SET time (double quotes), because $dir is local and would be out of scope by
  # the time an EXIT trap fired.
  trap "rm -rf '$dir'" EXIT

  # EVERY precondition is validated BEFORE any subprocess is created. A sed that matched nothing
  # would otherwise produce a mutant byte-identical to the original, which exits 0 and reads as a
  # survivor — an accurate red for a completely misleading reason.
  for line in $lines; do
    sed "/^  ${line}\$/d" "$SELF_SRC" > "$dir/$line.sh" \
      || infra "check 9: sed failed while mutating '$line'"
    removed=$(( $(wc -l < "$SELF_SRC") - $(wc -l < "$dir/$line.sh") ))
    if [ "$removed" -ne 1 ]; then
      fail "check 9: mutating '$line' removed $removed lines, expected exactly 1. The invocation
        inside run_self_tests is not on a line of its own, or is duplicated. Check 9 cannot build a
        meaningful mutant until it is."
      return
    fi
  done

  # Spawned concurrently: running these --self-test invocations one after another would multiply
  # this gate's standalone cost by the size of the table, and they are independent. Collected by
  # PID, so results do not depend on completion order (SMA-542 D12).
  for line in $lines; do
    bash "$dir/$line.sh" --self-test > "$dir/$line.out" 2>&1 &
    pids="$pids $!"
    labels="$labels $line"
  done
  # The control (D6): the REAL file, unmutated, which must exit 0. One mutant per invocation, all
  # firing, cannot tell a working battery from a stuck one (SMA-466). This proves the harness
  # itself — the bash invocation, the cwd, the argument passing — yields 0 on a healthy tree.
  bash "$SELF_SRC" --self-test > "$dir/__control__.out" 2>&1 &
  pids="$pids $!"
  labels="$labels __control__"

  i=0
  set -- $labels
  for pid in $pids; do
    i=$((i + 1))
    eval "label=\${$i}"
    wait "$pid"
    rc=$?
    if [ "$label" = '__control__' ]; then
      if [ "$rc" -ne 0 ]; then
        fail "check 9: the unmutated control exited $rc, expected 0. The battery's own harness is
      broken, so its $n dead mutants prove nothing. Output follows."
        sed 's/^/      check 9 [control]: /' "$dir/__control__.out" >&2
      fi
      continue
    fi
    # A KILL is rc 1 carrying the counter's own message — not merely "non-zero". infra() exits 2,
    # a missing file exits 127, and branch_filter_self_test's own precondition exits 2; scoring any
    # of those as a kill would let a transient fault stand in for the proof (SMA-542 D10). The
    # decision itself is mutant_is_killed (above, next to kill_predicate_self_test's fixture table
    # driving it) rather than inline here, so it has a standing proof of its own (SMA-542 review M3).
    if mutant_is_killed "$rc" "$dir/$label.out"; then
      continue
    fi
    case "$rc" in
      2|126|127)
        fail "check 9: mutant '$label' aborted with an infrastructure error (rc $rc) before
      reaching the counter, so it proves nothing. Output follows." ;;
      *)
        fail "check 9: mutant '$label' exited $rc without the counter's message. Deleting that
      invocation did NOT red the gate — assert_self_tests_ran is missing or neutered, which is
      exactly the silent pass SMA-542 exists to prevent. Output follows." ;;
    esac
    sed "s/^/      check 9 [mutant $label]: /" "$dir/$label.out" >&2
  done
}

SELF_TEST_ONLY=0
case "$#:${1:-}" in
  '0:')
    ;;
  '1:--self-test')
    SELF_TEST_ONLY=1 ;;
  *)
    usage ;;
esac

# Check 7 runs FIRST, and from a single call site. --self-test never shells out to actionlint, so
# this sits AHEAD of the PATH guard below and stays runnable on a machine without the binary. It
# DOES need a git repo carrying origin/main: branch_filter_self_test's control pair asserts a real
# ref resolves (SMA-540 D7). Running the controls before the checks they guard matches the
# convention moon.yml states for repo:affected-smoke, repo:publish-metadata and
# repo:error-code-single-site — a rotted checker must red rather than ship green.
run_self_tests

if [ "$SELF_TEST_ONLY" = 1 ]; then
  exit "$FAILED"
fi

# ---------------------------------------------------------------------------------------------
# Check 8 — the T=() floor, and the propagation of `moon`'s exit status. Rationale and fixtures
# are with ci_target_floor_verdict above.
#
# REPORTED_LINENOS accumulates the line numbers this loop already explains via 'continued',
# 'swallowed', 'block-swallowed' or 'wrapped', so the invocation-allowlist check just below
# (Check 8b) can skip reporting the SAME line a second time under a less specific name — "you
# appended '|| true'" beats "does not match an allowlisted form". CI_YML_MISSING is the same idea
# at file granularity: a missing ci.yml is reported ONCE, here, rather than a second time by check
# 8b below (independent review of PR 150, finding minor-4) — check 8b has nothing useful to add
# once check 8 has already said the file does not exist. Both plain (not local): this is top-level
# script, not a function.
# ---------------------------------------------------------------------------------------------
REPORTED_LINENOS=''
CI_YML_MISSING=0
while IFS= read -r verdict; do
  case "$verdict" in
    '') ;;
    no-file)
      fail ".github/workflows/ci.yml does not exist, so this gate cannot confirm that
      repo:affected-smoke is still scheduled. If the workflow was renamed, update this check."
      CI_YML_MISSING=1 ;;
    no-array)
      fail ".github/workflows/ci.yml has no single-line 'T=( … )' array, or has more than one, so
      the target list cannot be read. Keep T on ONE line — ci_targets.py's C3 requires it too."  ;;
    'missing '*)
      fail ".github/workflows/ci.yml's T=( … ) no longer contains '${verdict#missing }'. That
      entry schedules the gate which guards T itself: without it ci_targets.py's C1-C5, the
      affected-graph cascade cases and cargo_moon_parity all stop running, every one of them
      green. Restore it — and the matching entry in CLAUDE.md's ci-targets block." ;;
    'continued '*)
      fail ".github/workflows/ci.yml:${verdict#continued } invokes 'moon' across more than one
      physical line (a trailing backslash continuation). This check reads one physical line at a
      time, so it cannot see a '||'/'&&'/';'/'|' tail sitting on a later line of the same
      invocation — reporting 'swallowed' here would name a problem this check cannot actually
      confirm. Put the whole 'moon ci'/'moon run' invocation back on ONE physical line, the same
      requirement 'no-array' already makes of T=( … ) itself."
      REPORTED_LINENOS="$REPORTED_LINENOS ${verdict#continued }" ;;
    'swallowed '*)
      fail ".github/workflows/ci.yml:${verdict#swallowed } runs 'moon' but discards its exit
      status (a '||', '&&', ';' or '|' tail). If this is the invocation that guards T, that greens
      every gate in T while leaving T itself perfectly correct, so no other check in this repo can
      see it — remove the tail. If it is a DIFFERENT, harmless 'moon' line (a diagnostic
      'moon run x | tee log' in an unrelated job, say), this check cannot tell the two apart and it
      belongs in SWALLOWED_SKIP (above ci_target_floor_verdict in $0) with a reason."
      REPORTED_LINENOS="$REPORTED_LINENOS ${verdict#swallowed }" ;;
    'block-swallowed '*)
      fail ".github/workflows/ci.yml:${verdict#block-swallowed } closes a block ('fi'/'done'/'}')
      but that closing line itself discards an exit status (a '||', '&&', ';' or '|' tail) — just
      as silent as the same tail on the 'moon' line, and invisible to EVERY other check here:
      T_INVOCATION_ALLOWLIST only scans lines carrying the literal '\${T[@]}' expansion, which a
      terminator line does not carry, and neither does it carry 'moon' or a recognized wrapper
      token. 'fi || true' on ci.yml's real if/elif/else, or the whole block wrapped in
      '{ ... } || true', both take this shape. Remove the tail. If it is a DIFFERENT, harmless
      terminator line this check cannot know is unrelated to T, it belongs in SWALLOWED_SKIP
      (above ci_target_floor_verdict in $0) with a reason." ;;
    'wrapped '*)
      fail ".github/workflows/ci.yml:${verdict#wrapped } runs 'moon ci'/'moon run' behind a known
      command wrapper (command/env/time/eval/exec/if/while/until/!) on the same physical line, so
      this check cannot confirm the invocation's exit status propagates — a wrapper can swallow a
      failure exactly like a '||'/'&&'/';'/'|' tail does ('command moon ci ... || true'), or even
      with no tail at all ('if moon ci ...; then :; fi' exits 0 regardless of moon's exit code,
      because a failing 'if' CONDITION does not fail the 'if' STATEMENT). Put the invocation at
      command position on ONE physical line, unwrapped — the same requirement 'swallowed' already
      makes. This check only recognizes that fixed list of wrappers and cannot see an arbitrary
      one; if this IS a different, harmless wrapped 'moon' line, it belongs in SWALLOWED_SKIP
      (above ci_target_floor_verdict in $0) with a reason."
      REPORTED_LINENOS="$REPORTED_LINENOS ${verdict#wrapped }" ;;
    'continue-on-error '*)
      fail ".github/workflows/ci.yml:${verdict#continue-on-error } suppresses that step's own
      failure (a 'continue-on-error:' value other than 'false'). If this is the step that runs
      'moon ci', every gate in T is silenced while T itself stays perfectly correct, so no other
      check in this repo can see it — remove the key, or set it to false. If it is a different,
      later step, this line does not silence T's gates and belongs in COE_SKIP (above
      ci_target_floor_verdict in $0) with a reason." ;;
    *)
      infra "unhandled ci-target-floor verdict '$verdict'" ;;
  esac
done < <(ci_target_floor_verdict .github/workflows/ci.yml)

# ---------------------------------------------------------------------------------------------
# Check 8b — every "${T[@]}"-bearing line matches an ALLOWLISTED invocation form exactly, and the
# invocation count is pinned (SMA-542 CodeRabbit round 3, finding B). Rationale, the T_INVOCATION_
# ALLOWLIST array, and the verdict function are all above, with T_FLOOR. This is the PRIMARY guard
# on the invocation lines themselves — 'swallowed'/'continued'/'block-swallowed'/'wrapped' above
# stay for their more specific diagnostics, and REPORTED_LINENOS (built above) keeps a line from
# being reported under both names.
#
# Skipped entirely when CI_YML_MISSING (set above): invocation_allowlist_verdict would itself
# report 'no-file' redundantly — check 8 has already said the file does not exist, and check 8b's
# verdict function still WORKS standalone (its own self-test drives that path directly), so this
# is a call-site de-dup, not a defect in the function.
# ---------------------------------------------------------------------------------------------
if [ "$CI_YML_MISSING" -eq 0 ]; then
while IFS= read -r verdict; do
  case "$verdict" in
    '') ;;
    'not-allowlisted '*)
      fail ".github/workflows/ci.yml:${verdict#not-allowlisted } carries the target-array
      expansion '\${T[@]}' but does not match any entry in T_INVOCATION_ALLOWLIST (above,
      with T_FLOOR) EXACTLY — indentation included, nothing before or after. Three rounds of
      CodeRabbit review each found a new way to precede 'moon' that the 'swallowed'/'wrapped'
      blacklist above did not anticipate (a wrapper, quoted text after a wrapper, a bare
      'VAR=value' assignment prefix), so this line is checked against the allowed forms directly
      instead. If this is a deliberate, reviewed change to how ci.yml invokes moon, update
      T_INVOCATION_ALLOWLIST to match — copied verbatim from the file, indentation included."  ;;
    'invocation-count '*)
      fail ".github/workflows/ci.yml has ${verdict#invocation-count } line(s) carrying the
      target-array expansion '\${T[@]}', not the ${#T_INVOCATION_ALLOWLIST[@]} this gate
      expects. This fires independently of whether every individual line matches — a DELETED
      invocation (or one quietly subsetted to '\${T[@]:0:5}', which no longer contains the
      literal expansion at all) drops the count even though no surviving line looks wrong on its
      own. If this is a deliberate, reviewed change in how many invocations ci.yml has, update
      T_INVOCATION_ALLOWLIST to match." ;;
    count-unreadable)
      infra "could not count \"\${T[@]}\"-bearing lines in .github/workflows/ci.yml — grep
      itself failed rather than matching zero times. This runs in the MAIN shell (this while
      loop), unlike the verdict function above, which runs inside \`< <(...)\` and cannot exit the
      gate itself (SMA-542 CodeRabbit round 4, finding F1)." ;;
    *)
      infra "unhandled invocation-allowlist verdict '$verdict'" ;;
  esac
done < <(invocation_allowlist_verdict .github/workflows/ci.yml "$REPORTED_LINENOS")
fi

# ---------------------------------------------------------------------------------------------
# Check 8c — ci/affected-graph/run.sh's own two call sites into ci_targets.py (SMA-542 residual
# closure — this file's README L6, now closed; ci_targets.py's RUN_SH_CALL_SITES comment updated
# to match). Rationale and fixtures are with T_AFFECTED_GRAPH_CALL_SITES and
# affected_graph_wiring_verdict above.
#
# Unconditional — reads ci/affected-graph/run.sh, a file ci.yml's own state cannot affect, so
# there is no CI_YML_MISSING-style de-dup to make against checks 8/8b.
# ---------------------------------------------------------------------------------------------
while IFS= read -r verdict; do
  case "$verdict" in
    '') ;;
    no-file)
      fail "ci/affected-graph/run.sh does not exist, or is not a readable regular file, so this
      gate cannot confirm ci_targets.py is still invoked from it. If the file was renamed, update
      T_AFFECTED_GRAPH_CALL_SITES (above affected_graph_wiring_verdict in $0)." ;;
    'missing '*)
      fail "ci/affected-graph/run.sh no longer contains the exact text
      '${verdict#missing }' (its '|| SUITE_RC=1'/'|| NEG_RC=1' propagation suffix included). That
      call is what runs ci_targets.py's C1-C5 AND ci_targets.py's own check of THIS file's call
      sites (ACTIONLINT_SH_CALL_SITES) — delete it, or swallow its suffix with e.g. '|| true', and
      BOTH stop running, silently, with nothing inside ci/affected-graph/ able to notice its own
      deletion. This check exists to close exactly that: it is scheduled independently of
      ci/affected-graph/ (repo:actionlint's inputs are ['**/*']), so it survives the deletion.
      Restore the exact line, suffix included." ;;
    *)
      infra "unhandled affected-graph-wiring verdict '$verdict'" ;;
  esac
done < <(affected_graph_wiring_verdict ci/affected-graph/run.sh)

# ---------------------------------------------------------------------------------------------
# Check 8d — the step's `run:` block, EXECUTED once per event path against a stubbed `moon`.
# Rationale, MOON_STEP_EVENT_PATHS and block_execution_verdict are all above, with check 8c.
#
# Skipped entirely when CI_YML_MISSING (set by check 8, above): block_execution_verdict would
# itself report 'no-file' redundantly — check 8 has already said the file does not exist, and this
# check's own verdict function still WORKS standalone (its own self-test drives that path
# directly), so this is a call-site de-dup, not a defect in the function. NOT deduped against
# check 8's 'no-array': block_execution_verdict reads T independently (moon_target_array_tokens,
# above), and a redundant-but-truthful second complaint about the same malformed array is
# tolerated here the same way this file already tolerates checks 8/8b overlapping in other ways —
# neither is the sole judge of ci.yml's shape.
# ---------------------------------------------------------------------------------------------
if [ "$CI_YML_MISSING" -eq 0 ]; then
while IFS= read -r verdict; do
  case "$verdict" in
    '') ;;
    count-unreadable)
      infra "could not count occurrences of the \"moon ci (affected graph)\" step name in
      .github/workflows/ci.yml — grep itself failed rather than matching zero times." ;;
    no-step)
      fail ".github/workflows/ci.yml has no step named exactly \"moon ci (affected graph)\", so
      this check cannot find the block that guards T to execute it. If the step was renamed,
      update the literal name check 8d matches on (above block_execution_verdict in $0)." ;;
    'multi-step '*)
      fail ".github/workflows/ci.yml has ${verdict#multi-step } steps named exactly
      \"moon ci (affected graph)\" — which one actually guards T is ambiguous, so this check
      refuses to guess. Rename all but the real one." ;;
    no-run-block)
      fail ".github/workflows/ci.yml's \"moon ci (affected graph)\" step has no extractable
      'run: |'/'run: >' block, so this check cannot execute it." ;;
    no-target-array)
      fail ".github/workflows/ci.yml has no single, unambiguous 'T=( … )' array — see check 8's
      'no-array' above for the same underlying problem." ;;
    setup-failed)
      infra "could not resolve a bash binary, could not create or truncate a scratch
      directory/file, or could not read back a usable invocation count, while trying to execute
      .github/workflows/ci.yml's \"moon ci (affected graph)\" step. This is an environment
      problem, not a defect in ci.yml." ;;
    'zero-invocations '*)
      fail ".github/workflows/ci.yml's \"moon ci (affected graph)\" step, executed with EVENT/
      BEFORE set for the '${verdict#zero-invocations }' event path, never invoked 'moon' at all.
      T_INVOCATION_ALLOWLIST's three lines can stay byte-identical to the allowed forms while this
      still fires — an outer 'if false; then … fi' (or any other shape with the same effect)
      produces exactly this. This is the failure README L12 names and this check exists to close."
      ;;
    'wrong-count '*)
      # "wrong-count <path> <n>" — split on the LAST space (n is always the final word; every
      # path label above is hyphenated, never space-separated, so this is unambiguous).
      WRONG_COUNT_REST="${verdict#wrong-count }"
      fail ".github/workflows/ci.yml's \"moon ci (affected graph)\" step, executed for the
      '${WRONG_COUNT_REST% *}' event path, invoked 'moon' ${WRONG_COUNT_REST##* } time(s), not
      once." ;;
    'bad-args '*)
      fail ".github/workflows/ci.yml's \"moon ci (affected graph)\" step, executed for the
      '${verdict#bad-args }' event path, invoked 'moon' exactly once but not with the exact
      subcommand + WHOLE T array + --base/--include-relations shape that path requires. Check 8b
      matches each invocation LINE against a SET of allowed forms with no notion of which branch
      it sits under, so three lines that individually match the set, in the WRONG branches (the
      'if' and 'else' bodies swapped, say), pass 8b outright — this check derives what the path
      itself requires and catches exactly that." ;;
    *)
      infra "unhandled block-execution verdict '$verdict'" ;;
  esac
done < <(block_execution_verdict .github/workflows/ci.yml)
fi

# ---------------------------------------------------------------------------------------------
# Check 8e — repo:affected-smoke still declares the inputs that schedule every pin in
# ci/affected-graph/ci_targets.py, and still runs both halves of its own script, in order.
# Rationale, tables and fixtures are with affected_smoke_block_verdict above.
#
# UNCONDITIONAL, deliberately — NOT inside the `if [ "$CI_YML_MISSING" -eq 0 ]` that guards
# checks 8b/8d just above. That guard is a de-dup about ci.yml's existence; this check reads
# moon.yml, so gating it on ci.yml would switch it off for a wholly unrelated reason. Check 8c is
# unconditional for exactly the same reason and is the precedent here.
#
# THE ARITY FLOORS ARE PART OF THE CHECK, not a sanity nicety. affected_smoke_block_verdict
# iterates its tables, so an EMPTIED table emits zero verdicts and passes — the "green while
# asserting nothing" failure this whole registry exists to prevent (MEASURED: with
# T_AFFECTED_SMOKE_REQUIRED_INPUTS replaced by `()`, affected_smoke_block_verdict moon.yml emitted
# 0 lines against the real, wired file). Check 8c is immune to that only because its table is a
# verbatim dual copy of ci_targets.py's RUN_SH_CALL_SITES and the other copy still asserts the
# same lines; 8e keeps its set at ONE site (a second copy would add drift risk with no added
# coverage, since repo:actionlint runs on every PR), so it buys non-emptiability this way instead.
# Both floors are pinned from ci_targets.py's ACTIONLINT_SH_CALL_SITES, which makes shrinking
# either table a two-file edit across two independently scheduled gates. `-ge`, not `-eq`, so
# honest GROWTH needs no second edit.
#
# COLUMN 0 for both floor lines: ACTIONLINT_SH_CALL_SITES matches its entries with no leading
# whitespace, so indenting either one (by wrapping this block in a conditional, say) reds that pin
# rather than silently satisfying it.
# ---------------------------------------------------------------------------------------------
[ "${#T_AFFECTED_SMOKE_REQUIRED_INPUTS[@]}" -ge 20 ] || infra "check 8e: T_AFFECTED_SMOKE_REQUIRED_INPUTS has ${#T_AFFECTED_SMOKE_REQUIRED_INPUTS[@]} entries, expected at least 20"
[ "${#T_AFFECTED_SMOKE_REQUIRED_SCRIPT[@]}" -ge 3 ] || infra "check 8e: T_AFFECTED_SMOKE_REQUIRED_SCRIPT has ${#T_AFFECTED_SMOKE_REQUIRED_SCRIPT[@]} entries, expected at least 3"

while IFS= read -r verdict; do
  case "$verdict" in
    '') ;;
    no-file)
      fail "moon.yml does not exist, or is not a readable regular file, so this gate cannot
      confirm repo:affected-smoke still declares the inputs that schedule every pin in
      ci/affected-graph/ci_targets.py. If it was renamed, update check 8e in $0." ;;
    no-task)
      fail "moon.yml no longer declares a task named 'affected-smoke'. That task is where
      ci_targets.py runs, so removing it switches off C1-C5, the cascade cases, A1-A6 and every
      call-site pin in that file at once." ;;
    bad-task-form|bad-script-form|bad-inputs-form)
      fail "check 8e could not parse repo:affected-smoke's block in moon.yml ($verdict). The
      extractor is held to this file's block style — task keys at two spaces, fields at four,
      'script: |' as a literal block and 'inputs:' as a block sequence. It fails loudly rather
      than skipping in silence, because a parser that skips quietly makes this check vacuous.
      Restore the block style, or teach affected_smoke_block_extract the new one (with a
      fixture)." ;;
    'duplicate-key '*)
      fail "repo:affected-smoke declares '${verdict#duplicate-key }' twice in moon.yml. Which one
      moon honours is not something this check will guess at — merge them." ;;
    'missing-input '*)
      fail "repo:affected-smoke no longer declares the input '${verdict#missing-input }'. Every
      pin in ci/affected-graph/ci_targets.py fires only when this task is SCHEDULED, and its
      inputs are what schedule it — so dropping one silently un-reaches a whole family of pins
      for every later PR. The 'moon.yml' entry is the worst case: removing it is itself a
      root-moon.yml edit, and afterwards nothing in this task's remaining globs matches that
      file, so the removal PR is the last one that would have noticed. Restore the entry, or add
      a reasoned REQUIRED_INPUT_SKIP entry in $0 saying what covers it instead." ;;
    'missing-script '*)
      fail "repo:affected-smoke's script no longer contains the exact line
      '${verdict#missing-script }' (a commented-out copy counts as absent). Moon takes a script
      block's status from its LAST command, so all three lines are load-bearing: without
      'set -euo pipefail' a failing negative control is swallowed, and without the bare
      'ci/affected-graph/run.sh' only the control runs — which asserts against synthetic
      fixtures and exits 0. ci_targets.py cannot catch that last one: it is what would have run
      the checker. This check is scheduled independently, so it survives the deletion." ;;
    'out-of-order-script '*)
      fail "repo:affected-smoke's script line '${verdict#out-of-order-script }' appears at or
      before a required line that must precede it. ci_targets.py compares a set of lines and
      cannot see order, so moving 'set -euo pipefail' below the invocations leaves it green while
      errexit stops applying to them. The required order is:
$(printf '        %s\n' "${T_AFFECTED_SMOKE_REQUIRED_SCRIPT[@]}")
      Compare the block against that order rather than assuming the line named above is itself
      the one that moved: the scan walks the table forwards and reports every line landing at or
      before the previously-matched one, so ONE displaced line is commonly reported as several." ;;
    'skip-without-reason '*)
      fail "REQUIRED_INPUT_SKIP entry for '${verdict#skip-without-reason }' states no reason.
      Write it as \"<glob> # why, and what verifies it instead\" — an unexplained waiver is
      indistinguishable from the deletion this check exists to catch." ;;
    'stale-skip '*)
      fail "REQUIRED_INPUT_SKIP names '${verdict#stale-skip }', which is not in
      T_AFFECTED_SMOKE_REQUIRED_INPUTS. A waiver that has outlived its requirement hides the next
      one; delete it." ;;
    *)
      infra "unhandled affected-smoke-block verdict '$verdict'" ;;
  esac
done < <(affected_smoke_block_verdict moon.yml)

# ---------------------------------------------------------------------------------------------
# Check 8f (SMA-601) — the cargo-lock-integrity step still exists in ci.yml, still precedes the
# `moon ci` step, and still propagates its own failure. Rationale, T_CARGO_LOCK_STEP_REQUIRED and
# cargo_lock_step_verdict are above, with affected_smoke_block_verdict.
#
# Skipped entirely when CI_YML_MISSING (set by check 8, above): check 8 has already reported the
# missing file once, and cargo_lock_step_verdict would only say 'missing-line' twice for the same
# underlying cause.
# ---------------------------------------------------------------------------------------------
if [ "$CI_YML_MISSING" -eq 0 ]; then
while IFS= read -r verdict; do
  case "$verdict" in
    '') ;;
    'missing-line '*)
      fail ".github/workflows/ci.yml no longer contains the line
      '${verdict#missing-line }' of the cargo-lock-integrity step. That step is a plain
      workflow step, not a Moon task, so no T entry and no SELF_SCHEDULED_GATES row can see its
      deletion — this check is the only thing that does. Restore it." ;;
    out-of-order)
      fail ".github/workflows/ci.yml's cargo-lock-integrity step no longer precedes the
      'moon ci (affected graph)' step. Placement is the whole guarantee: an unlocked cargo
      invocation inside the moon graph repairs a truncated lock in place, so a check that runs
      after moon ci would pass on a lock the PR never shipped. Move the step back above 'moon ci
      (affected graph)'." ;;
    'out-of-order-script '*)
      fail ".github/workflows/ci.yml's cargo-lock-integrity step runs
      '${verdict#out-of-order-script }' before the line listed above it in
      T_CARGO_LOCK_STEP_REQUIRED. The order is load-bearing: 'set -euo pipefail' below the
      invocations leaves every line byte-identical while a failing --self-test or
      --negative-control stops aborting the block, and the real run must come last so it never
      masks a control that already reported. Restore the documented order." ;;
    'continue-on-error '*)
      fail ".github/workflows/ci.yml's cargo-lock-integrity step sets
      'continue-on-error: ${verdict#continue-on-error }', which lets the step fail while the job
      stays green — a silent bypass of the whole check. Remove the key, or set it to false." ;;
    'conditional '*)
      fail ".github/workflows/ci.yml's cargo-lock-integrity step carries
      'if: ${verdict#conditional }'. A skipped step is a GREEN step, so any 'if:' switches the
      whole guarantee off for every event the expression excludes — including pull_request,
      which is exactly where a Dependabot PR ships a truncated lock. The step must run on EVERY
      CI run, for the same reason the codegen-drift step below carries no 'if:'. Remove it." ;;
    no-file)
      fail ".github/workflows/ci.yml is not a readable regular file, so check 8f could not read
      the cargo-lock-integrity step at all." ;;
    *)
      infra "unhandled cargo-lock-step verdict '$verdict'" ;;
  esac
done < <(cargo_lock_step_verdict .github/workflows/ci.yml)
fi

# ---------------------------------------------------------------------------------------------
# Check 8f, second half (SMA-601 review I2b) — ci/cargo-lock-integrity/run.sh still asserts
# something. The half above proves the three modes are INVOKED; this one proves the script they
# invoke has not been gutted. MEASURED: with `--locked` deleted from its `cargo metadata` line the
# real run exits 0 AND cargo repairs the lock, so the gate prints "satisfies every manifest" and
# becomes the first repairer. Rationale and the pinned lines are with T_CARGO_LOCK_SH_CALL_SITES.
#
# Not gated on CI_YML_MISSING: this reads a different file, and a missing ci.yml says nothing
# about the script.
# ---------------------------------------------------------------------------------------------
while IFS= read -r verdict; do
  case "$verdict" in
    '') ;;
    no-file)
      fail "ci/cargo-lock-integrity/run.sh is missing, or is not a readable regular file. The
      ci.yml step pinned by the half of check 8f above invokes exactly that path, so the whole
      lockfile guarantee is gone. Restore the script, or — if it moved deliberately — update
      T_CARGO_LOCK_SH_CALL_SITES' path here and the three invocations in ci.yml together." ;;
    'missing-site '*)
      fail "ci/cargo-lock-integrity/run.sh no longer contains the line
      '${verdict#missing-site }'. Each pinned line is load-bearing on its own: the two flag-parse
      arms make --self-test and --negative-control fall through to the real run when neutered,
      the 'cargo metadata --locked' line IS the assertion (dropping --locked makes the gate
      repair the lock it exists to police), the negative control's call to
      assert_lock_satisfies_manifests is what stops it reporting red while asserting nothing, and
      the real run's own call is what makes the bare mode mean anything. Restore it." ;;
    *)
      infra "unhandled cargo-lock-script verdict '$verdict'" ;;
  esac
done < <(cargo_lock_script_verdict ci/cargo-lock-integrity/run.sh)

# Guard lives here, AFTER the --self-test early exit: --self-test never shells out to actionlint,
# so it must not infra-exit on a machine that simply doesn't have the binary on PATH yet.
command -v actionlint >/dev/null 2>&1 || infra "actionlint not on PATH — run 'proto install actionlint'"

# ---------------------------------------------------------------------------------------------
# Check 1 — lint every workflow.
#
# Invoked BARE, with no file arguments, relying on actionlint's repository auto-discovery. Two
# reasons: a `*.yml` argument list would silently miss a `.yaml`-suffixed workflow, and
# actionlint's exit-3-on-empty behaviour (which is what makes "the directory vanished" loud
# rather than a vacuous pass) applies ONLY to the auto-discovery path — an explicit glob that
# expands to nothing would exit 0 as "no errors found".
# ---------------------------------------------------------------------------------------------
actionlint "${ARGS[@]}"
rc=$?
if [ "$rc" -ne 0 ]; then
  if [ "$rc" -eq 3 ]; then
    infra "actionlint found no workflow files to lint (exit 3)"
  fi
  fail "actionlint reported findings (exit $rc)"
fi

# ---------------------------------------------------------------------------------------------
# Check 2 — the allowlist assertion itself. Rationale and fixtures are with config_verdict above.
# ---------------------------------------------------------------------------------------------
for cfg in .github/actionlint.yaml .github/actionlint.yml; do
  [ -e "$cfg" ] || continue
  while IFS= read -r verdict; do
    case "$verdict" in
      '') ;;
      banned-ignore)
        fail "$cfg contains an 'ignore' key, which can silently suppress every finding in check 1.
      Remove it. To teach actionlint a new runner label, use self-hosted-runner.labels instead." ;;
      'unknown-key '*)
        fail "$cfg declares top-level key '${verdict#unknown-key }'. The only key permitted here is
      '$CONFIG_ALLOWED_KEYS' — the documented escape hatch for a runner label the pinned actionlint
      does not know. Every other key in this file can weaken check 1, so it is rejected rather
      than reasoned about." ;;
      *)
        infra "unhandled actionlint-config verdict '$verdict' for $cfg" ;;
    esac
  done < <(config_verdict "$cfg")
done

# ---------------------------------------------------------------------------------------------
# Check 3 — the linter must still REJECT each class of defect the issue names (AC-1).
#
# One fixture per class, asserting the RULE TAG appears — not merely that the exit was non-zero.
# A status-only assertion is satisfied by a YAML parse error and proves nothing about runner
# labels or expressions, and it stays green under a targeted `-ignore` on check 1.
#
# Fixtures go through stdin (`actionlint -`), so nothing broken ever lands in .github/workflows/
# where GitHub itself would try to parse it. The workflow schema applies regardless of
# -stdin-filename (verified).
# ---------------------------------------------------------------------------------------------
selftest_expect_tag() {
  local label="$1" tag="$2" yaml="$3" out rc
  out="$(printf '%s' "$yaml" | actionlint "${ARGS[@]}" -stdin-filename .github/workflows/selftest.yml - 2>&1)"
  rc=$?
  if [ "$rc" -eq 0 ]; then
    fail "self-test '$label': actionlint ACCEPTED a deliberately broken workflow. The gate is not
      guarding anything — check for an -ignore flag or a narrowed rule set."
    return
  fi
  if ! printf '%s' "$out" | grep -qF "[$tag]"; then
    fail "self-test '$label': actionlint failed, but not with the expected [$tag] rule. Got:
$out"
  fi
}

selftest_expect_tag 'paths nested under workflow_dispatch' 'syntax-check' 'name: selftest
on:
  workflow_dispatch:
    paths:
      - "rs/**"
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
'

selftest_expect_tag 'malformed glob' 'glob' 'name: selftest
on:
  push:
    branches: [main]
    paths:
      - "rs/[**"
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
'

selftest_expect_tag 'unknown runner label' 'runner-label' 'name: selftest
on: [push]
jobs:
  j:
    runs-on: ubunut-latest
    steps:
      - run: echo hi
'

# shellcheck disable=SC2016  # the ${{ }} below is deliberate GHA expression syntax inside a
                              # single-quoted fixture, not an un-expanded shell variable.
selftest_expect_tag 'undefined step output' 'expression' 'name: selftest
on: [push]
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - run: echo ${{ steps.nope.outputs.x }}
'

# ---------------------------------------------------------------------------------------------
# Check 4 — control for check 3.
#
# A globally broken invocation (bad flag, missing binary, unreadable stdin) makes EVERY fixture
# "fail", which would read as "malformed input correctly rejected" four times over. This healthy
# fixture must pass, which is what distinguishes a working linter from a broken one.
#
# Keep this fixture MINIMAL. Anything schema-adjacent risks becoming a false red on an actionlint
# pin bump, and this gate sits inside the only required check.
# ---------------------------------------------------------------------------------------------
healthy='name: selftest
on:
  push:
    branches: [main]
    paths:
      - "rs/**"
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
'
if ! printf '%s' "$healthy" | actionlint "${ARGS[@]}" -stdin-filename .github/workflows/selftest.yml -; then
  fail "self-test control: actionlint REJECTED a known-good workflow. The invocation itself is
    broken, so the check-3 rejections above prove nothing."
fi

# ---------------------------------------------------------------------------------------------
# Checks 5 and 6 — the real files. Rationale and fixtures are with pattern_verdict and
# scan_workflow_records above; this is the production call site that turns each finding record
# into the message its author has to act on.
# ---------------------------------------------------------------------------------------------

# Populate the ref cache in the MAIN SHELL. Verdicts are computed in nested command
# substitutions, so a cache first populated there would be thrown away with the subshell and
# git would run once per entry instead of once per gate run.
load_origin_refs

for wf in "${WORKFLOW_FILES[@]}"; do
  records="$(extract_filter_keys "$wf")" || infra "extractor failed on $wf"
  [ -n "$records" ] || continue

  findings="$(scan_workflow_records "$records")"
  [ -n "$findings" ] || continue

  while IFS=$'\t' read -r rec verdict f1 f2 f3; do
    case "$rec" in
      PATTERN)
        p="$f1"
        case "$verdict" in
          rejected-charclass)
            fail "$wf: pattern '$p' uses '?', '+' or '[]', whose meaning differs between GitHub
      filter patterns and git pathspecs, so this gate cannot verify it. Rewrite it, or add it to
      SKIP_PATTERNS in $0 with a justification." ;;
          rejected-charset)
            fail "$wf: pattern '$p' contains characters this gate will not pass to git.
      Supported: letters, digits, '.', '_', '/', '*', '-'. If GitHub accepts it, add it to
      SKIP_PATTERNS in $0 with a justification." ;;
          rejected-dotty)
            fail "$wf: pattern '$p' contains a '.', '..', or empty path segment ('./', '/./', '/../',
      or '//'). git's :(glob) matcher normalizes these away when resolving the pattern; GitHub
      filter patterns match the literal path text and do not, so this gate cannot verify it.
      Rewrite the pattern without them, or add it to SKIP_PATTERNS in $0 with a justification." ;;
          rejected-globstar)
            fail "$wf: pattern '$p' uses '**' inside a path segment. GitHub treats that as
      slash-crossing ('**.js' = every .js file); git does not, so this gate cannot verify it.
      Write '**/*.js' instead, or add it to SKIP_PATTERNS in $0 with a justification." ;;
          dead)
            fail "$wf: paths glob '$p' matches NO tracked file. The workflow's trigger is
      (or will become) dead — GitHub reports nothing when a filter matches nothing." ;;
          not-exact)
            fail "$wf: paths entry '$p' is not an exact tracked file path. GitHub filter patterns
      match FILE paths — a bare directory name matches nothing. Did you mean '$p/**'?" ;;
          *)
            infra "unhandled pattern verdict '$verdict' for '$p' in $wf" ;;
        esac ;;
      BRANCH)
        b="$f1"; bline="$f2"
        case "$verdict" in
          unverifiable)
            fail "$wf:$bline: branches entry '$b' contains a glob metacharacter ('*', '?', '+' or
      '[]'), so it names a pattern rather than a branch and cannot be resolved against a ref. Add
      it to BRANCH_SKIP in $0 with a justification saying what verifies it instead — wildcard
      branch filters ('release/*', 'dependabot/**') are idiomatic GitHub. Or, if a literal branch
      name was intended, rewrite it as one." ;;
          invalid-name)
            fail "$wf:$bline: branches entry '$b' is not a legal git branch name — git
      check-ref-format rejects it. No branch can ever carry that name, so the trigger it guards is
      dead." ;;
          unresolved)
            fail "$wf:$bline: branches entry '$b' does not resolve as refs/remotes/origin/$b. The
      trigger it guards is (or will become) dead — GitHub reports nothing when a branch filter
      matches nothing. This checkout's view of origin can be stale — verdicts read .git state,
      which sits outside Moon's input hash — so run 'git fetch --prune origin' before concluding
      this is a typo. Existing branches include: $(origin_candidates "$b"). If the branch does not
      exist yet, add '$b' to BRANCH_SKIP in $0 with a justification." ;;
          no-origin-main)
            no_origin_main_infra ;;
          *)
            infra "unhandled branch verdict '$verdict' for '$b' in $wf" ;;
        esac ;;
      KEY)
        case "$verdict" in
          no-items)
            fail "$wf:$f2: '$f1:' has no sequence entries this gate could read. Two forms produce
      that and neither is parsed: an inline sequence ($f1: [a, b]) and a flow mapping on the
      triggering event itself (an 'event: { $f1: [a, b] }' shape). Rewrite the event and its
      filter in block style — skipping either one silently is exactly the failure this gate exists
      to prevent." ;;
          all-negated)
            fail "$wf:$f2: '$f1:' has $f3 entries but every one is a '!'-negated exclusion. GitHub
      requires at least one non-'!' entry — a filter made only of exclusions can never match, so
      the trigger it guards is dead. Add at least one positive entry." ;;
          *)
            infra "unhandled key verdict '$verdict' in $wf" ;;
        esac ;;
      *)
        # A finding record whose type nothing handles must not read as "nothing to report" —
        # that is the silent pass this whole gate exists to prevent, one layer up. Before
        # SMA-540 added BRANCH, an unknown record type fell out of this case with no action.
        infra "unhandled finding record type '$rec' in $wf" ;;
    esac
  done <<< "$findings"
done

# ---------------------------------------------------------------------------------------------
# Check 7 ran near the top, from run_self_tests — see the comment at its call site for why the
# controls precede the checks they guard.
#
# Check 9 runs HERE, and only here: it is deliberately NOT part of --self-test. That is what makes
# recursion structurally impossible (mutants are invoked with --self-test and exit before reaching
# it), and it keeps --self-test the fast iteration path README.md advertises.
# ---------------------------------------------------------------------------------------------
# Check 10 — the release guard, over the real workflow. Runs here (not in --self-test) because it
# reads the actual .github/workflows tree, like checks 5/6.
#
# UNLIKE every other `done < <(verdict)` call site in this file, the verdict here is an EXTERNAL
# PROCESS whose fail-closed contract is a process exit, not an echoed token. Process substitution
# discards that status (and `set -uo pipefail` above does not cover a redirection), so reading it
# through `< <(...)` would let an exit 2 — an unreadable file or unparseable YAML — finish the gate
# rc 0 having asserted nothing. That is the bug the comment on affected_graph_wiring_verdict
# (~line 2050) records for the bash verdicts; it applies with more force here, since there is no
# way to make an external process "echo and return" instead of exiting. Capture to a file, inspect
# the status, THEN read.
#
# ROUTE EVERY STATUS, NOT JUST 2 (fix round 3, Critical 1). This block first shipped routing only
# rc 2, and the earlier version of this very comment claimed a "missing uv" was among the cases it
# covered. It was not, and the wording is now corrected: a missing `uv` makes the WRAPPER exit 127,
# which is not the guard's own 2 and was therefore unrouted. MEASURED on this branch: with the
# production call replaced by `( exit 127 )`, the full gate exited 0 — $RG_OUT empty, the read loop
# below saw nothing, FAILED stayed 0. The `elif` arm closes that for 127 and for every other
# unexpected status (a 137 kill, a `uv run` resolution failure). The rc-1-with-empty-output arm
# below closes the remaining shape: rc 1 means "violations found", so producing none contradicts
# the guard's contract and must not read as a clean run.
#
# What the full gate ALSO catches, and what it does not: `release_guard_self_test` above runs
# `release_guard_py --fixture-count` under `|| infra`, so on the full-gate path a missing `uv`
# already aborts there, before this block is reached. That does not make this routing redundant —
# it is what covers a status that appears only at the production call (a transient, a kill, an
# interpreter that resolves for one invocation and not the next), and it is what makes the claim
# in ci/actionlint/README.md's row 10 true of this block rather than of a different check.
# ---------------------------------------------------------------------------------------------
RG_OUT="$(mktemp)" || infra "check 10: mktemp failed"
release_guard_py .github/workflows/release.yml > "$RG_OUT"
rg_rc=$?
if [ "$rg_rc" -eq 2 ]; then
  rm -f "$RG_OUT"
  infra "check 10: release_guard.py aborted (exit 2) — its stderr is above. The guard failed
      closed, as designed; the workflow could not be read or parsed."
elif [ "$rg_rc" -ne 0 ] && [ "$rg_rc" -ne 1 ]; then
  rm -f "$RG_OUT"
  infra "check 10: release_guard.py exited $rg_rc, which is none of its three documented statuses
      (0 clean, 1 violations, 2 infra). This file is 'set -uo pipefail' with NO -e, so an
      unrouted status left \$RG_OUT empty, the read loop below saw nothing, and the gate finished
      rc 0 having asserted nothing — MEASURED at rc 127. A missing 'uv' produces 127 from the
      WRAPPER, not the guard's own 2, so routing 2 alone was not enough."
fi
if [ "$rg_rc" -eq 1 ] && [ ! -s "$RG_OUT" ]; then
  rm -f "$RG_OUT"
  infra "check 10: release_guard.py exited 1 (its 'violations found' status) but printed nothing.
      That contradicts the guard's own contract, so the loop below would report zero findings for
      a run that found some. Treat it as infra, never as a pass."
fi
while IFS= read -r v; do
  [ -n "$v" ] && fail "check 10: $v"
done < "$RG_OUT"
rm -f "$RG_OUT"

# ---------------------------------------------------------------------------------------------
# Check 11 — the release-plan decision, over the real repository. Runs here (not in --self-test)
# because it reads the actual rs/ tree and the tag list, like checks 5/6/10.
#
# ROUTE EVERY STATUS, not just 2 — see check 10's comment for the measurement. run.sh maps the
# checker's 3 to 1 and everything else to 2, so 0, 1 and 2 are the only documented statuses here.
# ---------------------------------------------------------------------------------------------
rp_rc=0
release_plan_sh --assert || rp_rc=$?
if [ "$rp_rc" -eq 2 ]; then
  infra "check 11: ci/release-plan/run.sh --assert aborted (exit 2) — uv or the interpreter
      failed, not an assertion."
elif [ "$rp_rc" -eq 1 ]; then
  fail "check 11: ci/release-plan/run.sh --assert reported the repository is wrong — its stderr
      is above. The derived releasable set, a crate version, or the tag-name format changed."
elif [ "$rp_rc" -ne 0 ]; then
  infra "check 11: ci/release-plan/run.sh --assert exited $rp_rc, which is none of its three
      documented statuses (0 clean, 1 repo wrong, 2 infra). This file is 'set -uo pipefail' with
      NO -e, so an unrouted status would finish the gate rc 0 having asserted nothing."
fi

selftest_mutation_battery

exit "$FAILED"
