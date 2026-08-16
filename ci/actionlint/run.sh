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
# are what actually close the failure this gate was filed for.
#
# EXIT CODES (ci/ convention): 1 = assertion failure, 2 = infrastructure error. Without the
# split, a broken tool reads as a lint failure — or, if anyone wraps this in `|| true`, as a pass.
#
# NOT `set -e`: check 3 deliberately EXPECTS non-zero exits, and grep-based extraction
# legitimately finds nothing. Each check captures status explicitly and sets FAILED instead.
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

command -v actionlint >/dev/null 2>&1 || infra "actionlint not on PATH — run 'proto install actionlint'"

# ONE shared flag array for checks 1, 3 and 4. Written twice, an `-ignore` added to check 1
# would be invisible to check 3 BY CONSTRUCTION and the self-test would be decorative.
#
# shellcheck/pyflakes are disabled DELIBERATELY (spec D2): actionlint shells out to them when
# it finds them on PATH, which would make this gate's strictness a property of the host.
ARGS=(-shellcheck= -pyflakes=)

# Workflow discovery for checks 5-7. Non-recursive, both extensions — matching GitHub's own
# execution semantics. Check 1 does NOT use this list (see below).
WORKFLOW_FILES=()
for f in .github/workflows/*.yml .github/workflows/*.yaml; do
  [ -e "$f" ] && WORKFLOW_FILES+=("$f")
done
[ ${#WORKFLOW_FILES[@]} -gt 0 ] || infra "no workflow files found under .github/workflows/"

# Extract paths:/paths-ignore: keys and their sequence entries from one workflow file.
# Output records, TAB-separated, in file order:
#   KEY\t<paths|paths-ignore>\t<lineno>
#   ITEM\t<paths|paths-ignore>\t<pattern>
# See the contract in docs/superpowers/plans/2026-08-16-sma-525-actionlint-gate.md (Task 4) and
# ci/actionlint/README.md. Every clause below has a fixture in extractor_self_test.
extract_paths_keys() {
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

    {
      line = $0
      sub(/\r$/, "", line)                       # tolerate CRLF
      match(line, /^[ ]*/); ind = RLENGTH
      stripped = line
      sub(/^[ ]*/, "", stripped)

      if (stripped == "")   next                 # blank lines never close a block
      if (stripped ~ /^#/)  next                 # whole-line comments never close a block

      if (in_block) {
        if (ind <= key_ind) {
          in_block = 0                           # dedent closes; fall through to key handling
        } else if (stripped ~ /^-([ \t]|$)/) {
          item = stripped
          sub(/^-[ \t]*/, "", item)
          item = scalar(item)
          if (item != "") print "ITEM\t" kind "\t" item
          next
        } else {
          next                                   # deeper non-item line: not ours, keep the block
        }
      }

      # Track the top-level `on:` mapping. A quoted "on": is accepted (a common YAML 1.1
      # truthiness workaround). Any other column-0 key closes it.
      if (ind == 0) {
        if (stripped ~ /^["\047]?on["\047]?:[ \t]*$/)      { in_on = 1; next }
        if (stripped ~ /^["\047]?on["\047]?:/)             { in_on = 0; next }  # inline `on: [push]`
        in_on = 0
        next
      }

      if (!in_on) next

      if (stripped ~ /^paths:/)        { kind = "paths" }
      else if (stripped ~ /^paths-ignore:/) { kind = "paths-ignore" }
      else next

      print "KEY\t" kind "\t" NR

      # A block opens only when the value after the colon is empty. A non-empty value is the
      # inline flow form, which is deliberately not parsed — the KEY above, with no ITEMs
      # following, is what makes check 6 fail loudly instead of skipping silently.
      rest = stripped
      sub(/^paths(-ignore)?:[ \t]*/, "", rest)
      sub(/[ \t]+#.*$/, "", rest)
      if (rest == "") { in_block = 1; key_ind = ind }
    }
  ' "$1"
}

# ---------------------------------------------------------------------------------------------
# Check 7 — extractor self-test.
#
# The extractor is hand-rolled YAML parsing, which is exactly the kind of thing that silently
# does the wrong thing. Each clause of the documented contract gets a fixture. Runs on every
# invocation; `--self-test` runs ONLY this, for fast iteration while editing the awk.
# ---------------------------------------------------------------------------------------------
extractor_self_test() {
  local name expected actual tmp rc=0

  check_fixture() {
    name="$1"; expected="$2"; yaml="$3"
    tmp="$(mktemp)"
    printf '%s' "$yaml" > "$tmp"
    actual="$(extract_paths_keys "$tmp")"
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
"$(printf 'KEY\tpaths\t4\nITEM\tpaths\ta/**')" \
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
"" \
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

  return $rc
}

if [ "${1:-}" = "--self-test" ]; then
  extractor_self_test
  exit "$FAILED"
fi

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
# Check 2 — no actionlint config may neuter check 1.
#
# actionlint reads .github/actionlint.yaml, whose `paths:` map takes per-path `ignore:` regexes.
# A blanket `ignore: [".*"]` makes check 1 exit 0 on a workflow with an unknown runner label —
# VERIFIED. And the stdin fixtures of checks 3/4 are NOT suppressed by that config even when
# -stdin-filename names a matching path (also verified), so the self-tests cannot detect it.
# An explicit assertion is the only thing that can.
#
# The file itself is permitted: `self-hosted-runner.labels` is the documented escape hatch for a
# new GitHub runner label the pinned binary does not know (spec §6). Only `ignore:` is banned.
# ---------------------------------------------------------------------------------------------
for cfg in .github/actionlint.yaml .github/actionlint.yml; do
  [ -e "$cfg" ] || continue
  if grep -qE '^[[:space:]]*ignore:' "$cfg"; then
    fail "$cfg contains an 'ignore:' key, which can silently suppress every finding in check 1.
      Remove it. To teach actionlint a new runner label, use self-hosted-runner.labels instead."
  fi
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

exit "$FAILED"
