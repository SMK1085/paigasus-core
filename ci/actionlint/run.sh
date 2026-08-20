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
SELF_TEST_COUNT=6   # extractor, path-filter, branch-filter, config, ci-target-floor, kill-predicate

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
  echo "  --self-test    run the six fixture tables only — extractor, path-filter verdicts," >&2
  echo "                 branch-filter verdicts, config allowlist, ci-target floor, kill" >&2
  echo "                 predicate. No actionlint binary is required, but the branch-filter" >&2
  echo "                 table needs a git repo carrying refs/remotes/origin/main. The check-9" >&2
  echo "                 mutation battery is NOT part of this — it runs on the full gate only." >&2
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
  # Both halves are required on the SAME physical line — bash 3.2's `grep -E` is POSIX ERE with no
  # lookahead, so "wrapper token AND moon ci/run" cannot be one regex. Piped as two greps instead:
  # the first anchors the wrapper at column 0 (mirroring the `moon` anchor just above), the second
  # narrows to lines that also invoke `moon ci`/`moon run` specifically — `moon setup`/`moon sync`
  # are not gated by T and are out of scope. `grep -n` on the first stage keeps the "N:text" record
  # shape the read loop expects; the second stage matches against that whole "N:text" string, which
  # is safe because a line number is numeric and cannot itself contain "moon ci"/"moon run".
  while IFS=: read -r lineno text; do
    is_swallowed_skipped "$lineno:$text" || echo "wrapped $lineno"
  done < <(grep -nE '^[[:blank:]]*(command|env|time|eval|exec|if|while|until|!)[[:blank:]]' "$f" \
             | grep -E 'moon[[:blank:]]+(ci|run)([[:blank:]]|$)')

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

# ---------------------------------------------------------------------------------------------
# Check 9's kill predicate (SMA-542 review M3 / spec T3) — extracted so it can be driven directly
# by a fixture table instead of living inline in the mutant-collection loop below, where nothing
# proved it. It is correct today; what was missing is the STANDING proof, in a file that cites
# SMA-466's all-firing-fixture lesson four times over.
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

# The sixth self-test. Synthetic (rc, captured-output) pairs, no subprocess and no actionlint
# binary needed — same style as config_self_test's expect_config above.
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
    'actionlint gate: self-test counter: 4 of 6 self-tests ran. An invocation is missing.'
  # rc 2/126/127 must NEVER be a kill, even carrying the exact message — an infra abort proves
  # nothing about the assertion, and must not be mistaken for having reached it (SMA-542 D10).
  expect_kill 'rc 2 (infra abort) is never a kill, even with the message' 'not-killed' 2 \
    'actionlint gate: self-test counter: 4 of 6 self-tests ran. An invocation is missing.'
  expect_kill 'rc 126 (not executable) is never a kill' 'not-killed' 126 ''
  expect_kill 'rc 127 (missing file) is never a kill' 'not-killed' 127 ''
  # rc 1 without the message is not a kill either — some OTHER fail() fired, not the counter's.
  expect_kill 'rc 1 without the counter message is not a kill' 'not-killed' 1 \
    'actionlint gate: some unrelated assertion failed'
  expect_kill 'rc 0 (mutant did not fail at all) is never a kill' 'not-killed' 0 ''

  return $rc
}

# ---------------------------------------------------------------------------------------------
# Check 7 — the self-tests, and the counter that proves they were invoked.
#
# All six are defined above so this block can run them from ONE call site, reached by both the
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
  kill_predicate_self_test

  assert_self_tests_ran "$SELF_TEST_COUNT"

  # The counter proves the KNOWN tables ran; it cannot notice a table added tomorrow and never
  # wired up, because the count would still match. Asserting the DEFINITION count closes that —
  # adding a table without calling it reds, and so does deleting one without decrementing
  # SELF_TEST_COUNT. Adding a table is the highest-probability future edit here (SMA-542 D13).
  #
  # Tolerant of blank-before-paren (`seventh_self_test () {`) and the `function` keyword form
  # (`function seventh_self_test {`, with or without `()`) — a table written either way must still
  # be counted, or D13's own hole reopens for the style it does not recognise (SMA-542 review M8).
  # Not tolerant of a definition split across lines (`seventh_self_test()\n{`) — a rarer style this
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
# ---------------------------------------------------------------------------------------------
while IFS= read -r verdict; do
  case "$verdict" in
    '') ;;
    no-file)
      fail ".github/workflows/ci.yml does not exist, so this gate cannot confirm that
      repo:affected-smoke is still scheduled. If the workflow was renamed, update this check." ;;
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
      requirement 'no-array' already makes of T=( … ) itself." ;;
    'swallowed '*)
      fail ".github/workflows/ci.yml:${verdict#swallowed } runs 'moon' but discards its exit
      status (a '||', '&&', ';' or '|' tail). If this is the invocation that guards T, that greens
      every gate in T while leaving T itself perfectly correct, so no other check in this repo can
      see it — remove the tail. If it is a DIFFERENT, harmless 'moon' line (a diagnostic
      'moon run x | tee log' in an unrelated job, say), this check cannot tell the two apart and it
      belongs in SWALLOWED_SKIP (above ci_target_floor_verdict in $0) with a reason." ;;
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
      (above ci_target_floor_verdict in $0) with a reason." ;;
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
selftest_mutation_battery

exit "$FAILED"
