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
      # truthiness workaround). Any other column-0 key closes it. Strip a trailing comment before
      # classifying: `on:  # comment` must still be recognized as the block form, not misread as
      # the inline-flow form (which would silently drop every paths: key in the whole file).
      if (ind == 0) {
        key0 = stripped
        sub(/[ \t]+#.*$/, "", key0)
        if (key0 ~ /^["\047]?on["\047]?:[ \t]*$/)      { in_on = 1; next }
        if (key0 ~ /^["\047]?on["\047]?:/)             { in_on = 0; next }  # inline `on: [push]`
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
      #
      # Strip the key label FIRST, then the comment, then surrounding blanks — in that order.
      # Stripping the label with a greedy `[ \t]*` (consuming the whitespace right after the
      # colon) would eat the whitespace a trailing `#comment` needs to match on, leaving a
      # `paths:   # the filter` line looking non-empty and silently skipping the block.
      rest = stripped
      sub(/^paths(-ignore)?:/, "", rest)
      sub(/[ \t]+#.*$/, "", rest)
      sub(/^[ \t]+/, "", rest)
      sub(/[ \t]+$/, "", rest)
      if (rest == "") { in_block = 1; key_ind = ind }
    }
  ' "$1"
}

# ---------------------------------------------------------------------------------------------
# Extractor self-test (definition only — actually invoked, unconditionally, as check 7 near the
# end of this script, so the fixture table guards the parser on every real gate run, not only
# under --self-test). Defined here, ahead of check 1, purely so the `--self-test` early exit
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

  return $rc
}

if [ "${1:-}" = "--self-test" ]; then
  extractor_self_test
  exit "$FAILED"
fi

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
# Check 5 — every `paths:` glob must be expressible AND must match the tree.
#
# THIS is the check that closes the failure this gate was filed for; actionlint cannot see it.
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

check_pattern() {
  local file="$1" p="$2" n

  is_skipped "$p" && return

  # Negated entries are exclusions — requiring them to match a file would be wrong. They are
  # still COUNTED by check 6, which counts raw sequence items before any filtering, so an
  # all-negated block cannot hard-fail as "key with no items" — check 6 instead fails it as
  # "no positive pattern", a distinct and more specific verdict.
  case "$p" in '!'*) return ;; esac

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
    *'?'*|*'+'*|*'['*|*']'*)
      fail "$file: pattern '$p' uses '?', '+' or '[]', whose meaning differs between GitHub
      filter patterns and git pathspecs, so this gate cannot verify it. Rewrite it, or add it to
      SKIP_PATTERNS in $0 with a justification."
      return ;;
  esac

  # Pathspec-injection guard: a pattern starting with ':' would be read by git as pathspec
  # magic. The '--' separator and quoting are necessary but not sufficient. Anything outside
  # this conservative class is rejected rather than passed to git. Acts as the catch-all for
  # every remaining unsupported character, now that '?'/'+'/'[]' are handled above with their
  # own message.
  if ! printf '%s' "$p" | grep -qE '^[A-Za-z0-9._/*-]+$'; then
    fail "$file: pattern '$p' contains characters this gate will not pass to git.
      Supported: letters, digits, '.', '_', '/', '*', '-'. If GitHub accepts it, add it to
      SKIP_PATTERNS in $0 with a justification."
    return
  fi

  if has_dotty_segment "$p"; then
    fail "$file: pattern '$p' contains a '.', '..', or empty path segment ('./', '/./', '/../',
      or '//'). git's :(glob) matcher normalizes these away when resolving the pattern; GitHub
      filter patterns match the literal path text and do not, so this gate cannot verify it.
      Rewrite the pattern without them, or add it to SKIP_PATTERNS in $0 with a justification."
    return
  fi

  if ! globstars_are_components "$p"; then
    fail "$file: pattern '$p' uses '**' inside a path segment. GitHub treats that as
      slash-crossing ('**.js' = every .js file); git does not, so this gate cannot verify it.
      Write '**/*.js' instead, or add it to SKIP_PATTERNS in $0 with a justification."
    return
  fi

  case "$p" in
    *'*'*)
      n="$(git -c core.quotePath=false ls-files -- ":(glob)$p" 2>/dev/null | wc -l | tr -d ' ')"
      if [ "${n:-0}" -eq 0 ]; then
        fail "$file: paths glob '$p' matches NO tracked file. The workflow's trigger is
      (or will become) dead — GitHub reports nothing when a filter matches nothing."
      fi ;;
    *)
      if ! tracked_exact "$p"; then
        fail "$file: paths entry '$p' is not an exact tracked file path. GitHub filter patterns
      match FILE paths — a bare directory name matches nothing. Did you mean '$p/**'?"
      fi ;;
  esac
}

# ---------------------------------------------------------------------------------------------
# Check 6 — every extracted `paths:` KEY must carry at least one sequence item, and at least
# one of those items must be a POSITIVE (non-'!') pattern.
#
# Two distinct failures, two distinct messages, both keyed off the RAW item count (before the
# '!' filtering in check_pattern):
#
#   - RAW count is 0: the extractor found nothing to read. This is what converts an unsupported
#     YAML form — the inline flow `paths: [a, b]`, which the extractor deliberately does not
#     parse — from a silent skip into a loud failure that names the file. The difference between
#     a limitation and a hole.
#   - RAW count is >0 but every item is '!'-negated: GitHub includes a changed file only when it
#     matches at least one POSITIVE pattern, so an all-negated `paths:` block can never match
#     anything — the trigger it guards is dead, silently, forever. Same failure class the gate
#     exists to catch, just spelled with '!' instead of a typo (round-1 finding 1).
#
# `paths-ignore:` is exempt from the second failure: an all-negated paths-ignore is a no-op, not
# a dead trigger — mirrors check 5's header, which excludes paths-ignore from matching entirely.
# ---------------------------------------------------------------------------------------------
for wf in "${WORKFLOW_FILES[@]}"; do
  records="$(extract_paths_keys "$wf")" || infra "extractor failed on $wf"
  [ -n "$records" ] || continue

  key_kind=""; key_line=""; key_items=0; key_positive=0

  flush_key() {
    if [ -n "$key_kind" ] && [ "$key_items" -eq 0 ]; then
      fail "$wf:$key_line: '$key_kind:' has no sequence entries this gate could read. If it uses
      the inline form (paths: [a, b]), rewrite it as a block sequence — the extractor parses only
      block sequences, and skipping it silently is exactly the failure this gate exists to prevent."
    elif [ "$key_kind" = "paths" ] && [ "$key_items" -gt 0 ] && [ "$key_positive" -eq 0 ]; then
      fail "$wf:$key_line: 'paths:' has $key_items entries but every one is a '!'-negated
      exclusion. GitHub includes a changed file only when it matches at least one POSITIVE
      pattern, so this filter can never match anything and the trigger it guards is dead. Add at
      least one non-'!' pattern."
    fi
  }

  while IFS=$'\t' read -r rec kind value; do
    case "$rec" in
      KEY)
        flush_key
        key_kind="$kind"; key_line="$value"; key_items=0; key_positive=0 ;;
      ITEM)
        key_items=$((key_items + 1))
        if [ "$kind" = "paths" ]; then
          check_pattern "$wf" "$value"
          case "$value" in '!'*) ;; *) key_positive=$((key_positive + 1)) ;; esac
        fi ;;
    esac
  done <<< "$records"

  flush_key
done

# ---------------------------------------------------------------------------------------------
# Check 7 — extractor self-test, invoked for real.
#
# The function is defined earlier (immediately after extract_paths_keys) so the `--self-test`
# early exit near the top of this script can run it standalone for fast iteration. This is the
# unconditional invocation that actually makes the fixture table guard the parser on every real
# gate run — without it the whole table is dead code in CI.
# ---------------------------------------------------------------------------------------------
extractor_self_test

exit "$FAILED"
