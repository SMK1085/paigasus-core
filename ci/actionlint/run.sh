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

cd "$(git rev-parse --show-toplevel)" || exit 2

FAILED=0

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
  echo "  --self-test    run the fixture tables only (extractor, path filters, config allowlist)" >&2
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
    # Print a KEY with no ITEMs for any path filter it declares AT THE CALLER TARGET DEPTH, so
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

      # A sequence entry outside a paths block (a `branches:` or `schedule:` list) introduces no
      # mapping level, so it must not perturb the depth stack below.
      if (stripped ~ /^-([ \t]|$)/) next

      # DEPTH INSIDE `on:`. `on:` is level 0; an event key (push:, pull_request:,
      # workflow_dispatch:, ...) is level 1; a key belonging to that event is level 2. Only a
      # level-2 `paths:`/`paths-ignore:` is a real path filter.
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
        # that actionlint accepts, and its `paths:` never reaches the level-2 branch below. Left
        # alone it would make checks 5 and 6 silently guard nothing at all. Target depth 1: event
        # -> paths (an `inputs.paths` here, e.g. `push: { inputs: { paths: x } }`, is at depth 2
        # and must be ignored, same rule as above).
        val = stripped
        sub(/^[^:]*:[ \t]*/, "", val)
        flow_keys(val, NR, 1)
        next
      }
      if (depth != 2) next

      # A quoted key ("paths":, 'paths-ignore':) is valid YAML actionlint accepts; the bare-only
      # regex silently dropped it — no KEY record, so checks 5/6 skipped the filter with no
      # message (SMA-525 round-2 review finding A). Whitespace before the colon (`paths :`) gets
      # the same tolerance as `on :` above, for the same reason.
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

  check_fixture 'a paths: line inside a run block is ignored' \
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
# unhelpful-message problem the canary exists to avoid, one level down.
origin_candidates() {
  load_origin_refs
  printf '%s\n' "$ORIGIN_REFS" | head -8 | tr '\n' ' '
}

# Exits 2. MAIN SHELL ONLY — called from the production call site and from
# branch_filter_self_test, never from inside a $( ), where it would exit only the subshell.
no_origin_main_infra() {
  infra "refs/remotes/origin/main does not resolve in this checkout, so no 'branches:' entry can
      be verified. This is an environment problem, not a workflow defect: run 'git fetch origin',
      or re-clone without --single-branch. If main was genuinely RENAMED, every branches: filter
      in this repo is now dead — update them and this canary together."
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
  # ref, so a repo whose branch filters are all wildcards or skip-listed never pays it, and a
  # checkout without origin/main does not lose checks 1-6 as well. Returned as a TOKEN, not an
  # infra call: this function always runs inside $( ), where exit 2 would kill only the subshell.
  origin_has 'main' || { echo 'no-origin-main'; return; }

  origin_has "$b" && { echo 'ok'; return; }

  echo 'unresolved'
}

# ---------------------------------------------------------------------------------------------
# Check 6 (definitions) — every extracted `paths:` KEY must carry at least one sequence item, and
# at least one of those items must be a POSITIVE (non-'!') pattern.
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
    elif [ "$key_kind" = 'paths' ] && [ "$key_items" -gt 0 ] && [ "$key_positive" -eq 0 ]; then
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
        if [ "$kind" = 'paths' ]; then
          verdict="$(pattern_verdict "$value")"
          case "$verdict" in
            ok|skipped|negated) ;;
            *) printf 'PATTERN\t%s\t%s\n' "$verdict" "$value" ;;
          esac
          case "$value" in '!'*) ;; *) key_positive=$((key_positive + 1)) ;; esac
        fi ;;
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
  local rc=0 saved_skip saved_origin_refs saved_origin_refs_loaded

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

case "$#:${1:-}" in
  '0:')
    ;;
  '1:--self-test')
    # --self-test never shells out to actionlint, so it runs everything that does not need the
    # binary and exits before the PATH guard below. It DOES need a git repo carrying origin/main,
    # since branch_filter_self_test's control pair asserts a real ref resolves (SMA-540 D7).
    extractor_self_test
    path_filter_self_test
    branch_filter_self_test
    config_self_test
    exit "$FAILED" ;;
  *)
    usage ;;
esac

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
      KEY)
        case "$verdict" in
          no-items)
            fail "$wf:$f2: '$f1:' has no sequence entries this gate could read. Two forms produce
      that and neither is parsed: an inline sequence (paths: [a, b]) and a flow mapping on the
      event itself (push: { paths: [a, b] }). Rewrite the event and its filter in block style —
      skipping either one silently is exactly the failure this gate exists to prevent." ;;
          all-negated)
            fail "$wf:$f2: 'paths:' has $f3 entries but every one is a '!'-negated
      exclusion. GitHub includes a changed file only when it matches at least one POSITIVE
      pattern, so this filter can never match anything and the trigger it guards is dead. Add at
      least one non-'!' pattern." ;;
          *)
            infra "unhandled key verdict '$verdict' in $wf" ;;
        esac ;;
    esac
  done <<< "$findings"
done

# ---------------------------------------------------------------------------------------------
# Check 7 — the self-tests, invoked for real.
#
# All three are defined earlier so the `--self-test` early exit near the top of this script can
# run them standalone for fast iteration. These are the unconditional invocations that actually
# make the fixture tables guard the gate on every real run — without them the tables are dead
# code in CI.
# ---------------------------------------------------------------------------------------------
extractor_self_test
path_filter_self_test
branch_filter_self_test
config_self_test

exit "$FAILED"
