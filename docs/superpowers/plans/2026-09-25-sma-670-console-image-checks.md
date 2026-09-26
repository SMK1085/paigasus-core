# SMA-670 Console Image Checks Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the four gaps that the PR 279 final review found in the console image checks of `ci/images/run.sh`, and prove each new check with a committed self-test.

**Architecture:** `assert_console_pins` gets six new static checks over `ts/Dockerfile`. `smoke_consoles` gets three new row functions: the runtime Node version, a baked-configuration scan, and a bounded `HEALTHCHECK` row. `ts/Dockerfile` gives the healthcheck `fetch` a 2500 ms signal. A new script, `ci/images/console-selftest.sh`, copies the functions out of `run.sh` with `awk`, calls them in their production shape against mutated fixture files and a stub `docker`, and runs the real healthcheck program in the real runtime image. `images.yml` runs the self-test before the console build.

**Tech Stack:** bash (3.2 and 5), POSIX `awk`/`grep`/`sed` (BSD and GNU), Docker, Node 24 (distroless runtime), GitHub Actions.

**Spec:** `docs/superpowers/specs/2026-09-25-sma-670-console-image-checks-design.md` (revision 2). Read it before you start. This plan does not re-open its decisions. The section "Deviations from the spec" at the end lists each place where a measurement forced a change.

## Global Constraints

- Work only in the worktree `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-670-console-image-checks`, on branch `feature/sma-670-harden-console-image-checks`. Run `git branch --show-current` before each commit.
- The sandbox refuses complex inline shell (loops, compound commands) in this worktree session. Write such a command to a script file in your scratchpad directory and run `/bin/bash <file>`.
- Every new shell line runs under `/bin/bash` 3.2.57 and under `/opt/homebrew/bin/bash` 5.x. Do not use `mapfile`, `readarray`, `declare -A`, `${v,,}`, `[[ -v ]]`, a here-string (`<<<`), a heredoc (`<<`), or an expansion of an array that can be empty.
- Do not pipe into an early-exit reader: no `| grep -q`, `| grep -m`, `| head`, and no `| awk … exit`. Use `sed -n 1p` in place of `head -1`. `repo:actionlint` check 13 scans every tracked `*.sh` for these.
- Do not use `sed -i` in committed code. BSD sed and GNU sed read its argument differently.
- Every `$( )` capture is guarded (`|| var=""` or `|| rc=$?`). A `grep` rc greater than 1 gives its own "could not run" `::error::`.
- Never capture `with_deadline` in `$( )`. MEASURED (spec M7): `x="$(with_deadline 6 true)"` takes 6 s under bash 5. Redirect its output to a file.
- `crate_for`, `assert_pins`, `build_one`, `smoke`, `assert_base_intact` and `with_deadline` in `ci/images/run.sh` do not change (spec A2).
- `::error::` and `::warning::` lines go to stderr. Green lines go to stdout and start with two spaces.
- Values: fetch signal `2500` ms; `CONSOLE_HC_DEADLINE=20`; walk floor `100` files; self-test deadlines 15 s (F0), 6 s (F1).
- `ci/images/console-selftest.sh` starts with `#!/usr/bin/env bash`, then `# SPDX-License-Identifier: Apache-2.0`, then `set -euo pipefail` and `export PROTO_REPORTER=text`. Its git mode is `100755`.
- Do not name the JSON report file that `moon ci` writes into `.moon/cache` in any new text. `repo:actionlint` check 12 then requires a marker on that file.
- Commits: Conventional Commits. The scope must be one of `rs`, `py`, `ts`, `contracts`, `ci`, `docs`, `deps`, `release`, `repo`, `claude`, `workspace` (commitlint `scope-enum`). `docs(ops)` is NOT allowed; use `docs(ci)`. Header at most 100 characters. Body lines at most 100 characters. No body line may start with `#NNN` or `word: value`, because commitlint then reads it as a footer.
- End every commit message with the trailer line `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`, after a blank line.
- Never use `git commit --amend`, `git reset` or `--no-verify`. Commit only at the end of a task.
- Commits are SSH-signed through 1Password. If a commit fails with `1Password: failed to fill whole buffer`, STOP and report. Do not bypass signing.
- Write all prose (comments, RUNBOOK text, commit bodies) in ASD-STE100 Simplified Technical English: short sentences, active voice, one idea per sentence.

## Review Focus

These five inputs follow from the spec, but no spec row tests them. Each one has a test in the task that owns the code.

1. A `pnpm install` inside a quoted `sh -c` string or inside parentheses must count as an install. A plain whitespace token split sees the token `install"` and misses it; the old check caught it. Test: row S2e (Task 3).
2. When an amd64 image runs on an arm64 host, `docker run` prints a platform `WARNING` on stderr. R-NODE must parse stdout only and stay green. Test: row N6 (Task 6).
3. A `HEALTHCHECK --timeout` in a unit other than whole seconds (`3000ms`) must red with a message that says what the check found, not fail on shell arithmetic. Test: row S6c (Task 5).
4. `CONSOLE_HC_DEADLINE` set to `0` must be refused before `docker` runs, like a non-number. Test: row H5 (Task 8), beside H4.
5. A parser directive in upper case or with blanks around `=` (`#  SYNTAX = docker/dockerfile:1`) must red. Test: row S1g (Task 2).

---

## File map

| File | Change | Tasks |
|---|---|---|
| `ci/images/console-selftest.sh` | New. The self-test harness. Mode 100755. | 1–8 |
| `ci/images/run.sh` | `assert_console_pins`: S-DIRECTIVE, S-FROM, S-COPYFROM, S-INSTALL, S-PAIGASUS, S-HCTIMEOUT. Three new row functions. `CONSOLE_HC_DEADLINE`. Three call lines and the AC 3 text in `smoke_consoles`. | 2–9 |
| `ts/Dockerfile` | The healthcheck `printf` gets the 2500 ms signal and a comment. | 5 |
| `docs/ops/RUNBOOK-containers.md` | Section 6 bullets. | 9 |
| `ts/apps/iam-console/tests/cluster/phase-a/cross-zone.spec.ts` | The RUNBOOK citation in the header comment. | 9 |
| `ts/CLAUDE.md` | The RUNBOOK citations (lines 147–150). | 9 |
| `.github/workflows/images.yml` | The step "Console image self-test". | 10 |

Harness layout. Every task after Task 1 inserts its harness block directly above the line `# --- summary ---`. The summary block stays last. A task that adds a function to `run.sh` also adds the function name to the `FUNCS=` line.

---

### Task 1: Harness skeleton with S0 and C2

**Files:**
- Create: `ci/images/console-selftest.sh`

**Interfaces:**
- Consumes: `assert_console_pins` and `with_deadline` in `ci/images/run.sh` (unchanged in this task).
- Produces (later tasks use these names exactly):
  - globals `HERE`, `REPO`, `RUN_SH`, `FUNCS`, `T` (temp dir), `HC_CTR` (F-row container name), `N_PASS`, `N_FAIL`, `N_SKIP`
  - `infra <msg>` — prints `console-selftest: infrastructure error (rc=2): <msg>` on stderr, exits 2
  - `say_pass <row>`, `say_skip <row> <reason>`, `say_fail <row> <reason> [<file>...]`
  - `check_row <row> <rc> <want_rc> <present> <absent> <out-file> <err-file>` — `<present>` is a `|`-separated list of fixed substrings that stderr must hold; `<absent>` is one fixed substring that stderr must not hold; an empty string means "no check"
  - `$T/fn-<name>.sh` — the copied text of each function in `FUNCS`
  - `run_pins <row> <root> <want_rc> <present>`, `mk_tree <row>`, `append_df <row> <line>...`, `sed_df <row> <sed-script>`, `pins_mut <row> <want_rc> <present>`
  - the line `# --- summary ---` as the insertion anchor

- [ ] **Step 1: Write the harness**

Create `ci/images/console-selftest.sh` with exactly this content:

```bash
#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# SMA-670 — the self-test for the console image checks in ci/images/run.sh.
#
# Each check that SMA-670 added to assert_console_pins and smoke_consoles gets a surgical mutation
# here that must red with that check's own named ::error:: substring, and the clean baseline must
# stay green. .github/workflows/images.yml runs it directly before "Build + smoke both consoles".
# It is NOT a Moon task, for the reason in the header of ci/images/run.sh.
#
# How it loads the code under test: awk copies each function named in FUNCS out of run.sh, from
# its `<name>() {` line to the next line that is exactly `}`. The copy goes through `bash -n`, is
# sourced, and each name must then be a defined function. run.sh itself is never sourced: its
# dispatch code at the end exits before any function could run. ROOT is the only global that a
# copied function reads, and the harness sets it inside each row's subshell.
#
# How it calls the code under test: in its production shape. assert_console_pins runs as a plain
# command under `set -euo pipefail`, as the run.sh dispatch runs it. A smoke-row function runs as
# `fn … || rc=$?`, as smoke_consoles runs it, so errexit is off inside it. Each row runs in a
# subshell, so an `exit` or a `set -u` failure is a FAIL row and does not stop the harness.
#
# Bash 3.2 (macOS /bin/bash) and bash 5 both run it: no mapfile, no declare -A, no here-string, no
# pipe into an early-exit reader, no expansion of an array that can be empty, no ${v,,}, no [[ -v ]].
#
# Output: one PASS, FAIL or SKIP line for each row. On a FAIL the captured output follows, every
# line prefixed with `    | `, so no output line starts with `::` and the Actions runner makes no
# false annotation. Exit 1 on any FAIL, 2 on an infrastructure error in the loader.
set -euo pipefail
export PROTO_REPORTER=text

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
RUN_SH="$HERE/run.sh"

# The functions copied out of run.sh. A task that adds a function to run.sh adds its name here.
FUNCS="assert_console_pins with_deadline"

T="$(mktemp -d "${TMPDIR:-/tmp}/paigasus-console-selftest.XXXXXX")"
HC_CTR="selftest-hc-$$"
cleanup() {
  if command -v docker >/dev/null 2>&1; then
    docker rm -f "$HC_CTR" >/dev/null 2>&1 || true
  fi
  rm -rf "$T"
}
trap cleanup EXIT

N_PASS=0
N_FAIL=0
N_SKIP=0

infra() {
  echo "console-selftest: infrastructure error (rc=2): $*" >&2
  exit 2
}

say_pass() {
  echo "PASS $1"
  N_PASS=$((N_PASS + 1))
}

say_skip() {
  echo "SKIP $1 — $2"
  N_SKIP=$((N_SKIP + 1))
}

# say_fail <row> <reason> [<file>...] — every line of each non-empty file follows, prefixed.
say_fail() {
  local row="$1" why="$2" f
  shift 2
  echo "FAIL ${row} — ${why}"
  for f in "$@"; do
    [ -s "$f" ] || continue
    echo "    | --- ${f##*/}"
    sed 's/^/    | /' "$f"
  done
  N_FAIL=$((N_FAIL + 1))
}

# check_row <row> <rc> <want_rc> <present> <absent> <out> <err> — a row passes only when the rc is
# the expected one, every `|`-separated substring of <present> is in stderr, and <absent> (when
# not empty) is not. `case` matches the fixed substrings: no grep, so no regex and no early-exit
# reader.
check_row() {
  local row="$1" rc="$2" want_rc="$3" present="$4" absent="$5" o="$6" e="$7" err one rest
  err="$(cat "$e")"
  if [ "$rc" -ne "$want_rc" ]; then
    say_fail "$row" "rc ${rc}, expected ${want_rc}" "$o" "$e"
    return 0
  fi
  rest="$present"
  while [ -n "$rest" ]; do
    one="${rest%%|*}"
    if [ "$one" = "$rest" ]; then rest=""; else rest="${rest#*|}"; fi
    case "$err" in
      *"$one"*) ;;
      *) say_fail "$row" "stderr does not hold '${one}'" "$o" "$e"; return 0 ;;
    esac
  done
  if [ -n "$absent" ]; then
    case "$err" in
      *"$absent"*) say_fail "$row" "stderr holds '${absent}', which it must not" "$o" "$e"; return 0 ;;
    esac
  fi
  say_pass "$row"
}

# --- loader ------------------------------------------------------------------------------------
extract_fn() {
  awk -v want="$1() {" '$0 == want { on = 1 } on { print } on && $0 == "}" { on = 0 }' "$RUN_SH"
}

: > "$T/funcs.sh"
for fn in $FUNCS; do
  extract_fn "$fn" > "$T/fn-$fn.sh" || infra "awk could not read ${RUN_SH}"
  [ -s "$T/fn-$fn.sh" ] || infra "no '${fn}() {' line in ci/images/run.sh; the loader copies from that line to the next line that is exactly '}'"
  cat "$T/fn-$fn.sh" >> "$T/funcs.sh"
done
"$BASH" -n "$T/funcs.sh" || infra "bash -n rejected the functions copied out of ci/images/run.sh"
# shellcheck source=/dev/null
. "$T/funcs.sh"
for fn in $FUNCS; do
  declare -F "$fn" >/dev/null || infra "${fn} is not a defined function after the copy was sourced"
done

# --- static rows (assert_console_pins) ---------------------------------------------------------
# run_pins <row> <root> <want_rc> <present> — the production shape: a plain command under
# `set -euo pipefail`, with errexit ON inside the function.
run_pins() {
  local row="$1" root="$2" want_rc="$3" present="$4" rc o e
  o="$T/$row.out"
  e="$T/$row.err"
  set +e
  # shellcheck disable=SC2034 # ROOT is read by the function under test.
  ( ROOT="$root"; set -euo pipefail; assert_console_pins ) >"$o" 2>"$e"
  rc=$?
  set -e
  check_row "$row" "$rc" "$want_rc" "$present" "" "$o" "$e"
}

# mk_tree <row> — a fixture ROOT that holds a copy of the real ts/Dockerfile and .prototools.
mk_tree() {
  mkdir -p "$T/$1/ts"
  cp "$REPO/ts/Dockerfile" "$T/$1/ts/Dockerfile"
  cp "$REPO/.prototools" "$T/$1/.prototools"
}

# append_df <row> <line>... — one append of one or more lines to the fixture Dockerfile.
append_df() {
  local row="$1"
  shift
  printf '%s\n' "$@" >> "$T/$row/ts/Dockerfile"
}

# sed_df <row> <sed-script> — one sed over the fixture Dockerfile. No `sed -i`: BSD and GNU sed
# take its argument differently.
sed_df() {
  sed "$2" "$T/$1/ts/Dockerfile" > "$T/$1/ts/Dockerfile.new"
  mv "$T/$1/ts/Dockerfile.new" "$T/$1/ts/Dockerfile"
}

# pins_mut <row> <want_rc> <present> — a mutation row. The mutation must have changed the copy,
# or the row is a FAIL named "mutation did not apply".
pins_mut() {
  if cmp -s "$REPO/ts/Dockerfile" "$T/$1/ts/Dockerfile"; then
    say_fail "$1" "mutation did not apply"
    return 0
  fi
  run_pins "$1" "$T/$1" "$2" "$3"
}

run_pins S0 "$REPO" 0 ""

mk_tree C2
append_df C2 '# PAIGASUS_X in a comment'
pins_mut C2 0 ""

# --- summary -----------------------------------------------------------------------------------
echo "console-selftest: ${N_PASS} passed, ${N_FAIL} failed, ${N_SKIP} skipped"
if [ "$N_FAIL" -ne 0 ]; then
  exit 1
fi
if [ "$N_SKIP" -ne 0 ] && [ "${CI:-}" = "true" ]; then
  echo "console-selftest: rows were skipped with CI=true; in CI a skipped row is a failure"
  exit 1
fi
exit 0
```

Then make it executable:

```bash
chmod +x ci/images/console-selftest.sh
```

- [ ] **Step 2: Prove that the harness can fail, for the right reasons**

Write this to `<scratchpad>/t1-neg.sh` and run `/bin/bash <scratchpad>/t1-neg.sh` from the worktree root:

```bash
set -u
W="$(pwd)"
S="$(mktemp -d)"
mkdir -p "$S/ci/images" "$S/ts"
cp "$W/ci/images/console-selftest.sh" "$S/ci/images/"
cp "$W/.prototools" "$S/"
echo "== negative 1: the loader cannot find assert_console_pins"
sed 's/^assert_console_pins() {$/assert_console_pins_gone() {/' "$W/ci/images/run.sh" > "$S/ci/images/run.sh"
cp "$W/ts/Dockerfile" "$S/ts/Dockerfile"
/bin/bash "$S/ci/images/console-selftest.sh"; echo "rc=$?"
echo "== negative 2: the real Dockerfile loses --frozen-lockfile"
cp "$W/ci/images/run.sh" "$S/ci/images/run.sh"
sed 's/ --frozen-lockfile//' "$W/ts/Dockerfile" > "$S/ts/Dockerfile"
/bin/bash "$S/ci/images/console-selftest.sh"; echo "rc=$?"
rm -rf "$S"
```

Expected for negative 1: the line `console-selftest: infrastructure error (rc=2): no 'assert_console_pins() {' line in ci/images/run.sh; …` and `rc=2`.
Expected for negative 2: `FAIL S0 — rc 1, expected 0` and `FAIL C2 — rc 1, expected 0`, each followed by lines that start with `    | ` (one of them holds `carry --frozen-lockfile`), then `console-selftest: 0 passed, 2 failed, 0 skipped` and `rc=1`. No output line starts with `::`.

- [ ] **Step 3: Run the harness on the real tree, under both bashes**

Run: `/bin/bash ci/images/console-selftest.sh; echo "rc=$?"`
Run: `/opt/homebrew/bin/bash ci/images/console-selftest.sh; echo "rc=$?"`
Expected for each: `PASS S0`, `PASS C2`, `console-selftest: 2 passed, 0 failed, 0 skipped`, `rc=0`.

- [ ] **Step 4: Commit**

```bash
git add ci/images/console-selftest.sh
git commit -m "test(ci): add the console image self-test harness (SMA-670)" \
  -m "The harness copies assert_console_pins out of ci/images/run.sh with awk and runs it in its
production shape against the real ts/Dockerfile (S0) and a comment-only mutation (C2)." \
  -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git ls-files -s ci/images/console-selftest.sh
```

Expected: the last command prints a line that starts with `100755`.

---

### Task 2: S-DIRECTIVE, S-FROM and S-COPYFROM

**Files:**
- Modify: `ci/images/run.sh` (`assert_console_pins`, lines 207–257 before this task)
- Test: `ci/images/console-selftest.sh`

**Interfaces:**
- Consumes: the Task 1 harness helpers `mk_tree`, `append_df`, `pins_mut`.
- Produces: locals `rt_pin_re` and `bd_pin_re` in `assert_console_pins` (the two pin regexes, used by the old pin checks and by S-FROM). Error substrings: `parser directive`, `FROM instruction(s)`, `which is not the builder stage`.

- [ ] **Step 1: Add the failing rows**

Insert this block directly above the line `# --- summary ---` in `ci/images/console-selftest.sh`:

```bash
mk_tree S1
append_df S1 'FROM node:latest AS extra'
pins_mut S1 1 'FROM instruction(s)'

mk_tree S1b
append_df S1b 'from node:latest as extra'
pins_mut S1b 1 'FROM instruction(s)'

mk_tree S1c
awk '/^FROM node:/ && !done { print "RUN true \\"; done = 1 } { print }' "$T/S1c/ts/Dockerfile" > "$T/S1c/ts/Dockerfile.new"
mv "$T/S1c/ts/Dockerfile.new" "$T/S1c/ts/Dockerfile"
pins_mut S1c 1 'FROM instruction(s)'

mk_tree S1d
{ printf '%s\n' '# syntax=docker/dockerfile:1'; cat "$REPO/ts/Dockerfile"; } > "$T/S1d/ts/Dockerfile"
pins_mut S1d 1 'parser directive'

mk_tree S1g
{ printf '%s\n' '#  SYNTAX = docker/dockerfile:1'; cat "$REPO/ts/Dockerfile"; } > "$T/S1g/ts/Dockerfile"
pins_mut S1g 1 'parser directive'

mk_tree S1e
append_df S1e 'COPY --from=alpine:latest /x /y'
pins_mut S1e 1 'which is not the builder stage'

mk_tree S1f
append_df S1f 'RUN --mount=type=bind,from=alpine:latest,target=/x true'
pins_mut S1f 1 'which is not the builder stage'

```

Note on S1c: the `RUN true \` line joins the builder `FROM` into a `RUN` in the normalised view, so the normalised file holds one `FROM`. The builder line keeps its digest. The spec words S1c with the digest removed too; MEASURED, the existing builder-pin check then reds first with its own message, so the row could never hold `FROM instruction(s)`.

- [ ] **Step 2: Run the rows to see them fail**

Run: `/bin/bash ci/images/console-selftest.sh; echo "rc=$?"`
Expected: `PASS S0`, `PASS C2`, then `FAIL S1 — rc 0, expected 1` and the same line for S1b, S1c, S1d, S1g, S1e and S1f. Summary `2 passed, 7 failed`, `rc=1`.

- [ ] **Step 3: Make the two pin regexes locals**

In `ci/images/run.sh`, replace:

```bash
  local n_runtime_pin n_builder_pin
  n_runtime_pin="$(grep -cE '^FROM gcr\.io/distroless/nodejs[0-9]+-debian12:nonroot@sha256:[0-9a-f]{64}([[:space:]]|$)' "$df")" || n_runtime_pin=0
```

with:

```bash
  # The two pin regexes are locals because S-FROM below reads them a second time, on the
  # normalised file, so the count of FROM lines and the pins read the same view (SMA-670 D6).
  local n_runtime_pin n_builder_pin rt_pin_re bd_pin_re
  rt_pin_re='^FROM gcr\.io/distroless/nodejs[0-9]+-debian12:nonroot@sha256:[0-9a-f]{64}([[:space:]]|$)'
  bd_pin_re='^FROM node:[0-9]+\.[0-9]+\.[0-9]+-bookworm@sha256:[0-9a-f]{64}[[:space:]]+AS[[:space:]]+builder([[:space:]]|$)'
  n_runtime_pin="$(grep -cE "$rt_pin_re" "$df")" || n_runtime_pin=0
```

and replace:

```bash
  n_builder_pin="$(grep -cE '^FROM node:[0-9]+\.[0-9]+\.[0-9]+-bookworm@sha256:[0-9a-f]{64}[[:space:]]+AS[[:space:]]+builder([[:space:]]|$)' "$df")" || n_builder_pin=0
```

with:

```bash
  n_builder_pin="$(grep -cE "$bd_pin_re" "$df")" || n_builder_pin=0
```

- [ ] **Step 4: Add S-DIRECTIVE before the normaliser**

In `ci/images/run.sh`, replace:

```bash
    echo "::error::ts/Dockerfile: the builder FROM line must be exactly one node:X.Y.Z-bookworm@sha256:<64 hex> AS builder — without the digest, the code compiled into /app comes from an unpinned image." >&2
    return 1
  fi

  # The next two checks read a NORMALISED copy of ts/Dockerfile, written to a temp file, never a
```

with:

```bash
    echo "::error::ts/Dockerfile: the builder FROM line must be exactly one node:X.Y.Z-bookworm@sha256:<64 hex> AS builder — without the digest, the code compiled into /app comes from an unpinned image." >&2
    return 1
  fi

  # S-DIRECTIVE (SMA-670 D6). On the RAW file, because a parser directive decides how the
  # normaliser below must read the file. Only the comment lines before the first line that is not a
  # comment can be directives. A `# syntax=` makes BuildKit pull an unpinned frontend image, and an
  # `# escape=` changes the continuation character that the normaliser joins on. The match is
  # case-insensitive, because Docker reads directives case-insensitively. awk reads the FILE, not a
  # pipe, and it has no `exit`, so it reads its whole input.
  local directive directive_rc=0
  directive="$(awk 'done { next } !/^[[:space:]]*#/ { done = 1; next } { l = tolower($0) } l ~ /^#[[:space:]]*(syntax|escape|check)[[:space:]]*=/ { print; done = 1 }' "$df")" || directive_rc=$?
  if [ "$directive_rc" -ne 0 ]; then
    echo "::error::assert_console_pins: awk exited ${directive_rc} while reading ts/Dockerfile's parser directives; the directive check could not run." >&2
    return 1
  fi
  if [ -n "$directive" ]; then
    echo "::error::ts/Dockerfile starts with a parser directive (${directive}); a '# syntax=' pulls an unpinned BuildKit frontend and '# escape=' changes how this check reads the file — remove it." >&2
    return 1
  fi

  # The checks below read a NORMALISED copy of ts/Dockerfile, written to a temp file, never a
```

- [ ] **Step 5: Add S-FROM and S-COPYFROM after the ENV/ARG check**

In `ci/images/run.sh`, replace:

```bash
    grep -nE '^[[:space:]]*([Ee][Nn][Vv]|[Aa][Rr][Gg])[[:space:]]+.*PAIGASUS_' "$norm" >&2 || true
    rm -f "$norm"
    return 1
  fi

  # EVERY `pnpm install` invocation carries --frozen-lockfile, not only the first one found. Each
```

with:

```bash
    grep -nE '^[[:space:]]*([Ee][Nn][Vv]|[Aa][Rr][Gg])[[:space:]]+.*PAIGASUS_' "$norm" >&2 || true
    rm -f "$norm"
    return 1
  fi

  # S-FROM (SMA-670 gap 2a, D6). Every image that the build reads must be one of the two
  # digest-pinned FROM images. So the normalised file holds exactly two FROM instructions, and each
  # one matches one of the two pin regexes above. An extra stage, an unpinned stage, or a `from`
  # instruction in lower case all red here. The count and the pins read the SAME normalised view.
  local from_lines from_rc=0 n_from n_from_bad
  from_lines="$(grep -E '^[[:space:]]*[Ff][Rr][Oo][Mm][[:space:]]' "$norm")" || from_rc=$?
  if [ "$from_rc" -gt 1 ]; then
    rm -f "$norm"
    echo "::error::assert_console_pins: grep exited ${from_rc} on the normalised ts/Dockerfile; the FROM check could not run." >&2
    return 1
  fi
  # printf into grep -c reads the whole input: grep -c is not an early-exit reader.
  n_from="$(printf '%s\n' "$from_lines" | grep -c .)" || n_from=0
  n_from_bad="$(printf '%s\n' "$from_lines" | grep -vE "$rt_pin_re|$bd_pin_re" | grep -c .)" || n_from_bad=0
  if [ "$n_from" -ne 2 ] || [ "$n_from_bad" -ne 0 ]; then
    echo "::error::ts/Dockerfile has ${n_from} FROM instruction(s), or a FROM that is not one of the two digest-pinned stages (the node builder and the distroless runtime); an extra or unpinned stage pulls an unpinned image into the build. The FROM lines of the normalised file follow." >&2
    printf '%s\n' "$from_lines" >&2
    rm -f "$norm"
    return 1
  fi

  # S-COPYFROM (SMA-670 D6). A `--from=<image>` pulls an image that no FROM line pins. So every
  # `--from=` value (COPY --from=) and every `from=` inside a `--mount=` argument (RUN --mount=…)
  # must name the builder stage or the named build context `bindings`. Each extraction is guarded:
  # grep rc 1 is "none found", and only rc > 1 is a failure.
  local cf_from cf_from_rc=0 cf_mount cf_mount_rc=0 cf_values cf_bad cf_v
  cf_from="$(grep -oE -- '--from=[^[:space:],]+' "$norm")" || cf_from_rc=$?
  cf_mount="$(grep -oE -- '--mount=[^[:space:]]+' "$norm")" || cf_mount_rc=$?
  if [ "$cf_from_rc" -gt 1 ] || [ "$cf_mount_rc" -gt 1 ]; then
    rm -f "$norm"
    echo "::error::assert_console_pins: grep exited ${cf_from_rc}/${cf_mount_rc} on the normalised ts/Dockerfile; the --from= check could not run." >&2
    return 1
  fi
  # grep rc 1 below means "no value at all" or "no bad value", and both leave the variable empty,
  # which is the correct reading. `from=` also finds the value inside each `--from=` match.
  cf_values="$(printf '%s\n' "$cf_from" "$cf_mount" | grep -oE 'from=[^[:space:],]+' | sed 's/^from=//')" || cf_values=""
  cf_bad="$(printf '%s\n' "$cf_values" | grep -vxE 'builder|bindings')" || cf_bad=""
  if [ -n "$cf_bad" ]; then
    while IFS= read -r cf_v; do
      echo "::error::ts/Dockerfile reads from '${cf_v}', which is not the builder stage or the bindings context; a --from=<image> pulls an image that no FROM line pins." >&2
    done < <(printf '%s\n' "$cf_bad")
    rm -f "$norm"
    return 1
  fi

  # EVERY `pnpm install` invocation carries --frozen-lockfile, not only the first one found. Each
```

- [ ] **Step 6: Run the rows to see them pass, under both bashes**

Run: `/bin/bash ci/images/console-selftest.sh; echo "rc=$?"`
Run: `/opt/homebrew/bin/bash ci/images/console-selftest.sh; echo "rc=$?"`
Expected for each: nine `PASS` lines (S0, C2, S1, S1b, S1c, S1d, S1g, S1e, S1f), `console-selftest: 9 passed, 0 failed, 0 skipped`, `rc=0`.

- [ ] **Step 7: Commit**

```bash
git add ci/images/run.sh ci/images/console-selftest.sh
git commit -m "feat(ci): hold ts/Dockerfile to its two pinned images (SMA-670)" \
  -m "assert_console_pins now rejects a parser directive, a third or unpinned FROM stage, and a
--from= value that is not the builder stage or the bindings context. Self-test rows S1 to S1g." \
  -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: S-INSTALL (`pnpm i` and friends)

**Files:**
- Modify: `ci/images/run.sh` (`assert_console_pins`, the `--frozen-lockfile` block)
- Test: `ci/images/console-selftest.sh`

**Interfaces:**
- Consumes: Task 1 helpers.
- Produces: the error texts `no 'pnpm install'/'pnpm i' instruction found` and `'pnpm install'/'pnpm i' invocation(s) carry --frozen-lockfile`. The local `inst` still holds one install invocation per line.

- [ ] **Step 1: Add the rows**

Insert this block directly above the line `# --- summary ---`:

```bash
mk_tree S2
append_df S2 'RUN pnpm i --filter x'
pins_mut S2 1 'carry --frozen-lockfile'

mk_tree S2b
append_df S2b 'RUN pnpm i'
pins_mut S2b 1 'carry --frozen-lockfile'

mk_tree S2c
append_df S2c 'RUN pnpm --filter x install'
pins_mut S2c 1 'carry --frozen-lockfile'

mk_tree S2d
append_df S2d 'RUN pnpm i --frozen-lockfile && pnpm i'
pins_mut S2d 1 'carry --frozen-lockfile'

mk_tree S2e
append_df S2e 'RUN sh -c "pnpm install"'
pins_mut S2e 1 'carry --frozen-lockfile'

mk_tree C1
append_df C1 'RUN pnpm info x && pnpm import && pnpm init'
pins_mut C1 0 ""

```

- [ ] **Step 2: Run the rows to see which fail**

Run: `/bin/bash ci/images/console-selftest.sh; echo "rc=$?"`
Expected: `FAIL S2 — rc 0, expected 1`, and the same for S2b, S2c and S2d. `PASS S2e` and `PASS C1`: the old `pnpm[[:space:]]+install` match already catches `sh -c "pnpm install"`. S2e is a regression guard (Review Focus 1): it must stay green after Step 3. Summary `11 passed, 4 failed`, `rc=1`.

- [ ] **Step 3: Replace the install extraction**

In `ci/images/run.sh`, replace:

```bash
  local inst inst_rc=0 n_inst n_frozen n_unfrozen
  inst="$(grep -oE 'pnpm[[:space:]]+install([[:space:]][^&;|]*)?' "$norm")" || inst_rc=$?
  rm -f "$norm"
  if [ "$inst_rc" -gt 1 ]; then
    echo "::error::assert_console_pins: grep exited ${inst_rc} on the normalised ts/Dockerfile; the --frozen-lockfile check could not run." >&2
    return 1
  fi
  if [ -z "$inst" ]; then
    echo "::error::ts/Dockerfile: no 'pnpm install' instruction found; the image would not be built from the committed lockfile." >&2
    return 1
  fi
```

with:

```bash
  #
  # S-INSTALL (SMA-670 gap 2b). The extraction takes EVERY pnpm invocation first, from the word
  # `pnpm` to the next `&&`, `;` or `|`. awk then splits each one into whitespace-separated tokens
  # and keeps it when a token is exactly `install`, `i`, `install-test` or `it`. So
  # `pnpm --filter x install`, `pnpm -C ts i` and a bare `pnpm i` are installs, and `pnpm info`,
  # `pnpm import`, `pnpm init` and `pnpm exec next build` are not. awk tokens, not `\b`, `\<` or
  # `\>`: those are not POSIX ERE, and BSD grep and GNU grep read them differently. Each token
  # loses its quote, backtick and parenthesis characters before the compare, so
  # `sh -c "pnpm install"` stays an install, as it was under the old `pnpm[[:space:]]+install`
  # match (MEASURED: without the strip the token is `install"` and the install is missed). The awk
  # has no `exit`, so it reads its whole input. `\047` is the single quote.
  local inv inv_rc=0 inst n_inst n_frozen n_unfrozen
  inv="$(grep -oE '(^|[^[:alnum:]_./-])pnpm([[:space:]][^&;|]*)?' "$norm")" || inv_rc=$?
  rm -f "$norm"
  if [ "$inv_rc" -gt 1 ]; then
    echo "::error::assert_console_pins: grep exited ${inv_rc} on the normalised ts/Dockerfile; the --frozen-lockfile check could not run." >&2
    return 1
  fi
  inst="$(printf '%s\n' "$inv" | awk '{ for (i = 1; i <= NF; i++) { t = $i; gsub(/["\047()`]/, "", t); if (t == "install" || t == "i" || t == "install-test" || t == "it") { print; next } } }')" || inst=""
  if [ -z "$inst" ]; then
    echo "::error::ts/Dockerfile: no 'pnpm install'/'pnpm i' instruction found; the image would not be built from the committed lockfile." >&2
    return 1
  fi
```

Then replace:

```bash
    echo "::error::ts/Dockerfile: ${n_frozen} of ${n_inst} 'pnpm install' invocation(s) carry --frozen-lockfile, and ${n_unfrozen} switch it off; every install must be frozen, or the image would not be built from the committed lockfile. The invocations follow." >&2
```

with:

```bash
    echo "::error::ts/Dockerfile: ${n_frozen} of ${n_inst} 'pnpm install'/'pnpm i' invocation(s) carry --frozen-lockfile, and ${n_unfrozen} switch it off; every install must be frozen, or the image would not be built from the committed lockfile. The invocations follow." >&2
```

- [ ] **Step 4: Run the rows to see them pass, under both bashes**

Run: `/bin/bash ci/images/console-selftest.sh; echo "rc=$?"` and the same with `/opt/homebrew/bin/bash`.
Expected for each: `console-selftest: 15 passed, 0 failed, 0 skipped`, `rc=0`.

- [ ] **Step 5: Prove that S2e guards the token strip**

Write this to `<scratchpad>/t3-mut.sh` and run `/bin/bash <scratchpad>/t3-mut.sh` from the worktree root:

```bash
set -u
W="$(pwd)"
S="$(mktemp -d)"
mkdir -p "$S/ci/images" "$S/ts"
cp "$W/ci/images/console-selftest.sh" "$S/ci/images/console-selftest.sh"
cp "$W/.prototools" "$S/.prototools"
cp "$W/ts/Dockerfile" "$S/ts/Dockerfile"
# Delete the gsub: each token is then compared with its quotes still on it.
sed 's#gsub(/\["\\047()`\]/, "", t); ##' "$W/ci/images/run.sh" > "$S/ci/images/run.sh"
if cmp -s "$W/ci/images/run.sh" "$S/ci/images/run.sh"; then echo "MUTATION DID NOT APPLY"; fi
/bin/bash "$S/ci/images/console-selftest.sh"; echo "rc=$?"
rm -rf "$S"
```

Expected: no `MUTATION DID NOT APPLY` line, then `FAIL S2e — rc 0, expected 1`, `14 passed, 1 failed`, `rc=1`.

- [ ] **Step 6: Commit**

```bash
git add ci/images/run.sh ci/images/console-selftest.sh
git commit -m "feat(ci): see pnpm i and option-first installs in ts/Dockerfile (SMA-670)" \
  -m "The --frozen-lockfile check now finds every pnpm invocation and splits it into awk tokens,
so pnpm i, pnpm --filter x install and pnpm -C ts i are installs. Self-test rows S2 to S2e, C1." \
  -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: S-PAIGASUS (a `PAIGASUS_` outside `ENV`/`ARG`)

**Files:**
- Modify: `ci/images/run.sh` (`assert_console_pins`)
- Test: `ci/images/console-selftest.sh`

**Interfaces:**
- Consumes: Task 2's S-COPYFROM block (the insertion anchor), Task 1 helpers.
- Produces: the error substring `names PAIGASUS_ outside an ENV/ARG`.

- [ ] **Step 1: Add the failing rows**

Insert this block directly above the line `# --- summary ---`:

```bash
mk_tree S3
append_df S3 'RUN echo PAIGASUS_X=1 > /app/.env'
pins_mut S3 1 'names PAIGASUS_ outside an ENV/ARG'

mk_tree S4
append_df S4 'COPY --from=builder /x /app/PAIGASUS_X'
pins_mut S4 1 'names PAIGASUS_ outside an ENV/ARG'

mk_tree S5
append_df S5 'RUN <<EOF' 'PAIGASUS_X=1' 'EOF'
pins_mut S5 1 'names PAIGASUS_ outside an ENV/ARG'

```

- [ ] **Step 2: Run the rows to see them fail**

Run: `/bin/bash ci/images/console-selftest.sh; echo "rc=$?"`
Expected: `FAIL S3 — rc 0, expected 1`, and the same for S4 and S5. `15 passed, 3 failed`, `rc=1`.

- [ ] **Step 3: Add S-PAIGASUS after S-COPYFROM**

In `ci/images/run.sh`, replace:

```bash
    done < <(printf '%s\n' "$cf_bad")
    rm -f "$norm"
    return 1
  fi

```

with:

```bash
    done < <(printf '%s\n' "$cf_bad")
    rm -f "$norm"
    return 1
  fi

  # S-PAIGASUS (SMA-670 gap 2c, the text half). The ENV/ARG check above has passed, so every
  # PAIGASUS_ that is left is outside an ENV or ARG instruction: a RUN, COPY, ADD or ONBUILD step,
  # a heredoc body line, or a continuation that the normaliser does not join (a blank line inside
  # an ENV value). None may name PAIGASUS_ at all. There is no PAIGASUS_COMPILED_* exemption:
  # ts/Dockerfile never writes those values, createNextConfig does (SMA-670 D7). Comment lines
  # cannot trigger this, because the normaliser dropped them. The lines are printed without line
  # numbers, because a normalised line number is not a ts/Dockerfile line number.
  local n_named named_rc=0
  n_named="$(grep -c 'PAIGASUS_' "$norm")" || named_rc=$?
  if [ "$named_rc" -gt 1 ]; then
    rm -f "$norm"
    echo "::error::assert_console_pins: grep exited ${named_rc} on the normalised ts/Dockerfile; the PAIGASUS_ outside ENV/ARG check could not run." >&2
    return 1
  fi
  if [ "${n_named:-0}" -ne 0 ]; then
    echo "::error::ts/Dockerfile names PAIGASUS_ outside an ENV/ARG instruction (a RUN, COPY, ADD or ONBUILD step, or a heredoc body); console config is deployment-varying and must stay runtime-only. The line(s) of the normalised file follow." >&2
    grep 'PAIGASUS_' "$norm" >&2 || true
    rm -f "$norm"
    return 1
  fi

```

- [ ] **Step 4: Correct the comment on the ENV/ARG check**

In `ci/images/run.sh`, replace:

```bash
  # case-sensitive, as IAM_/GATEWAY_ do in assert_pins. What this does NOT see: a PAIGASUS_ value
  # written by a RUN step into a file, or passed in through a COPY — it reads ENV and ARG only.
```

with:

```bash
  # case-sensitive, as IAM_/GATEWAY_ do in assert_pins. This check reads ENV and ARG only. A
  # PAIGASUS_ in a RUN, COPY, ADD or ONBUILD step or in a heredoc body is S-PAIGASUS's job, below.
```

- [ ] **Step 5: Run the rows to see them pass, under both bashes**

Run: `/bin/bash ci/images/console-selftest.sh; echo "rc=$?"` and the same with `/opt/homebrew/bin/bash`.
Expected for each: `console-selftest: 18 passed, 0 failed, 0 skipped`, `rc=0`.

- [ ] **Step 6: Commit**

```bash
git add ci/images/run.sh ci/images/console-selftest.sh
git commit -m "feat(ci): reject PAIGASUS_ outside ENV/ARG in ts/Dockerfile (SMA-670)" \
  -m "After the ENV/ARG check, no line of the normalised ts/Dockerfile may name PAIGASUS_: a RUN,
COPY, ADD or ONBUILD step or a heredoc body line reds. Self-test rows S3 to S5." \
  -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: The healthcheck fetch signal, S-HCTIMEOUT and the real fetch-timeout rows

**Files:**
- Modify: `ts/Dockerfile:69` (the healthcheck `printf`)
- Modify: `ci/images/run.sh` (`assert_console_pins`, the end of the function)
- Test: `ci/images/console-selftest.sh`

**Interfaces:**
- Consumes: `with_deadline` (copied by the loader since Task 1), Task 1 helpers, `HC_CTR`.
- Produces: the error substring `no AbortSignal.timeout` and, for an unreadable timeout, `HEALTHCHECK --timeout <none> s`. Harness globals `HC_DRIVER_JS`, `HC_RC`, `HC_RUNTIME_REF`, function `hc_run <src-file> <deadline> <out-file>`.

- [ ] **Step 1: Add the rows**

Insert this block directly above the line `# --- summary ---`:

```bash
mk_tree S6
sed_df S6 's/, { signal: AbortSignal\.timeout(2500) }//'
pins_mut S6 1 'no AbortSignal.timeout'

mk_tree S6b
sed_df S6b 's/AbortSignal\.timeout(2500)/AbortSignal.timeout(3000)/'
pins_mut S6b 1 'no AbortSignal.timeout'

mk_tree S6c
sed_df S6c 's/--timeout=3s/--timeout=3000ms/'
pins_mut S6c 1 'HEALTHCHECK --timeout <none> s'

# --- the real fetch timeout (F0, F1) -----------------------------------------------------------
# The rendered healthcheck.mjs runs in the RUNTIME image that the runtime FROM line names, digest
# included: that is the node that runs it in production. A driver starts a server on
# 127.0.0.1:3000 that accepts connections and never answers, and then imports the program from a
# data: URL. F0: with the 2500 ms signal the program must end by itself, rc exactly 1, with
# TimeoutError in its output. F1: with the signal removed it must hang until the watchdog kills
# it (rc 143 or 137). Together they prove that the signal is what ends the hang. with_deadline
# writes to a FILE, never to a `$( )` capture (SMA-670 M7).
HC_DRIVER_JS='
const net = require("net");
const server = net.createServer(() => {});
server.listen(3000, "127.0.0.1", () => {
  import("data:text/javascript," + encodeURIComponent(process.env.HC_SRC));
});
'
HC_RC=0
# hc_run <src-file> <deadline> <out-file> — sets HC_RC.
hc_run() {
  local src
  src="$(cat "$1")"
  docker rm -f "$HC_CTR" >/dev/null 2>&1 || true
  HC_RC=0
  with_deadline "$2" docker run --rm --init --network none --name "$HC_CTR" \
    -e "HC_SRC=${src}" --entrypoint /nodejs/bin/node "$HC_RUNTIME_REF" -e "$HC_DRIVER_JS" >"$3" 2>&1 || HC_RC=$?
  docker rm -f "$HC_CTR" >/dev/null 2>&1 || true
}

HC_RUNTIME_REF=""
if ! docker info >/dev/null 2>&1; then
  say_skip F0 "docker info failed, so no daemon can run the runtime image"
  say_skip F1 "docker info failed, so no daemon can run the runtime image"
else
  hc_line="$(grep -E '^RUN printf .*> /app/healthcheck\.mjs$' "$REPO/ts/Dockerfile")" || hc_line=""
  hc_fmt="${hc_line#*\'}"
  hc_fmt="${hc_fmt%%\'*}"
  HC_RUNTIME_REF="$(grep -oE '^FROM gcr\.io/distroless/nodejs[0-9]+-debian12:nonroot@sha256:[0-9a-f]{64}' "$REPO/ts/Dockerfile" | sed 's/^FROM //')" || HC_RUNTIME_REF=""
  if [ -z "$hc_line" ] || [ "$hc_fmt" = "$hc_line" ]; then
    say_fail F0 "healthcheck printf not found in ts/Dockerfile"
    say_fail F1 "healthcheck printf not found in ts/Dockerfile"
  elif [ -z "$HC_RUNTIME_REF" ]; then
    say_fail F0 "no digest-pinned runtime FROM line in ts/Dockerfile"
    say_fail F1 "no digest-pinned runtime FROM line in ts/Dockerfile"
  elif ! docker pull "$HC_RUNTIME_REF" >"$T/pull.log" 2>&1; then
    say_fail F0 "could not pull the runtime image ${HC_RUNTIME_REF}" "$T/pull.log"
    say_fail F1 "could not pull the runtime image ${HC_RUNTIME_REF}" "$T/pull.log"
  else
    # The format string holds only `\n` and one `%s`, which every printf renders the same way.
    # shellcheck disable=SC2059
    printf "$hc_fmt" /iam > "$T/hc.mjs"
    hc_run "$T/hc.mjs" 15 "$T/F0.out"
    case "$HC_RC" in
      1)
        case "$(cat "$T/F0.out")" in
          *TimeoutError*) say_pass F0 ;;
          *) say_fail F0 "rc 1, but the output holds no TimeoutError" "$T/F0.out" ;;
        esac
        ;;
      143|137) say_fail F0 "the healthcheck hung until the watchdog killed it (rc ${HC_RC})" "$T/F0.out" ;;
      *) say_fail F0 "rc ${HC_RC}, expected exactly 1 with TimeoutError" "$T/F0.out" ;;
    esac
    sed 's/, { signal: AbortSignal\.timeout(2500) }//' "$T/hc.mjs" > "$T/hc-nosignal.mjs"
    if cmp -s "$T/hc.mjs" "$T/hc-nosignal.mjs"; then
      say_fail F1 "mutation did not apply" "$T/hc.mjs"
    else
      hc_run "$T/hc-nosignal.mjs" 6 "$T/F1.out"
      case "$HC_RC" in
        143|137) say_pass F1 ;;
        *) say_fail F1 "rc ${HC_RC}; without the signal the program must hang until the watchdog kills it (143 or 137)" "$T/F1.out" ;;
      esac
    fi
    docker rmi "$HC_RUNTIME_REF" >/dev/null 2>&1 || true
  fi
fi

```

- [ ] **Step 2: Run the rows to see them fail**

Docker must be running. Run: `/bin/bash ci/images/console-selftest.sh; echo "rc=$?"`
Expected (MEASURED on the old Dockerfile): `FAIL S6 — mutation did not apply`, `FAIL S6b — mutation did not apply`, `FAIL S6c — rc 0, expected 1`, `FAIL F0 — the healthcheck hung until the watchdog killed it (rc 143)`, `FAIL F1 — mutation did not apply`. `18 passed, 5 failed`, `rc=1`. The F0 row takes about 16 s here, because it waits for the watchdog.

- [ ] **Step 3: Give the healthcheck fetch its signal**

In `ts/Dockerfile`, replace:

```dockerfile
RUN printf '// SPDX-License-Identifier: Apache-2.0\nconst r = await fetch(`http://127.0.0.1:${process.env.PORT ?? 3000}%s/healthz`);\nprocess.exit(r.ok ? 0 : 1);\n' "$BASE_PATH" > /app/healthcheck.mjs
```

with:

```dockerfile
# The fetch carries its own 2500 ms signal (SMA-670). Docker kills the probe at --timeout=3s, and
# without the signal only undici's 300 s headersTimeout bounds a server that accepts the
# connection and never answers. assert_console_pins holds the signal below the --timeout.
RUN printf '// SPDX-License-Identifier: Apache-2.0\nconst r = await fetch(`http://127.0.0.1:${process.env.PORT ?? 3000}%s/healthz`, { signal: AbortSignal.timeout(2500) });\nprocess.exit(r.ok ? 0 : 1);\n' "$BASE_PATH" > /app/healthcheck.mjs
```

- [ ] **Step 4: Run the rows again**

Run: `/bin/bash ci/images/console-selftest.sh; echo "rc=$?"`
Expected: `PASS F0` and `PASS F1` now. `FAIL S6 — rc 0, expected 1`, and the same for S6b and S6c, because no check reads the signal yet. `20 passed, 3 failed`, `rc=1`.

- [ ] **Step 5: Add S-HCTIMEOUT and extend the success line**

In `ci/images/run.sh`, replace:

```bash
    printf '%s\n' "$inst" >&2
    return 1
  fi
  echo "  ts/Dockerfile: distroless Node ${base_major} and builder Node ${builder_node}/pnpm ${builder_pnpm} match .prototools, both FROM lines digest-pinned, no baked PAIGASUS_* ENV/ARG, all ${n_inst} pnpm install(s) --frozen-lockfile"
```

with:

```bash
    printf '%s\n' "$inst" >&2
    return 1
  fi

  # S-HCTIMEOUT (SMA-670 D2). On the RAW file. The healthcheck printf line must hold exactly one
  # AbortSignal.timeout(<ms>), the HEALTHCHECK instruction must hold --timeout=<s>s, and the signal
  # must fire before Docker kills the probe. Without the signal, a server that accepts the
  # connection and never answers hangs the probe until Docker kills it, and only undici's 300 s
  # headersTimeout bounds a `docker exec` of the same program.
  local hc_line hc_line_rc=0 n_hc_line n_sig sig_ms hc_to_s
  hc_line="$(grep -E '^RUN printf .*> /app/healthcheck\.mjs$' "$df")" || hc_line_rc=$?
  if [ "$hc_line_rc" -gt 1 ]; then
    echo "::error::assert_console_pins: grep exited ${hc_line_rc} on ts/Dockerfile; the healthcheck timeout check could not run." >&2
    return 1
  fi
  n_hc_line="$(printf '%s\n' "$hc_line" | grep -c .)" || n_hc_line=0
  n_sig="$(printf '%s\n' "$hc_line" | grep -oE 'AbortSignal\.timeout\([0-9]+\)' | grep -c .)" || n_sig=0
  sig_ms="$(printf '%s\n' "$hc_line" | sed -n 's/.*AbortSignal\.timeout(\([0-9][0-9]*\)).*/\1/p' | sed -n 1p)" || sig_ms=""
  hc_to_s="$(grep -E '^[[:space:]]*HEALTHCHECK[[:space:]]' "$df" | grep -oE -- '--timeout=[0-9]+s' | sed -n 's/^--timeout=\([0-9]*\)s$/\1/p' | sed -n 1p)" || hc_to_s=""
  if [ "$n_hc_line" -ne 1 ] || [ "$n_sig" -ne 1 ] || [ -z "$sig_ms" ] || [ -z "$hc_to_s" ] \
    || [ "$sig_ms" -ge $((hc_to_s * 1000)) ]; then
    echo "::error::ts/Dockerfile's healthcheck.mjs fetch has no AbortSignal.timeout(<ms>) below the HEALTHCHECK --timeout (found: ${n_hc_line} healthcheck printf line(s), ${n_sig} signal(s), signal ${sig_ms:-<none>} ms, HEALTHCHECK --timeout ${hc_to_s:-<none>} s; only a --timeout=<N>s value in whole seconds is read); without it, a server that stops answering hangs the probe until Docker kills it." >&2
    return 1
  fi
  echo "  ts/Dockerfile: distroless Node ${base_major} and builder Node ${builder_node}/pnpm ${builder_pnpm} match .prototools, both FROM lines digest-pinned, no parser directive, exactly 2 FROM stages, every --from= is builder/bindings, no PAIGASUS_* anywhere, all ${n_inst} pnpm install(s) --frozen-lockfile, healthcheck signal ${sig_ms} ms < --timeout=${hc_to_s}s"
```

- [ ] **Step 6: Run everything, under both bashes**

Run: `/bin/bash ci/images/console-selftest.sh; echo "rc=$?"` and the same with `/opt/homebrew/bin/bash`.
Expected for each: `console-selftest: 23 passed, 0 failed, 0 skipped`, `rc=0`, in about 15 s. MEASURED: F0's output holds `DOMException [TimeoutError]: The operation was aborted due to timeout` from `Node.js v24.14.0`.

- [ ] **Step 7: Commit**

```bash
git add ts/Dockerfile ci/images/run.sh ci/images/console-selftest.sh
git commit -m "fix(ci): bound the console healthcheck fetch at 2500 ms (SMA-670)" \
  -m "healthcheck.mjs now passes AbortSignal.timeout(2500) to fetch, and assert_console_pins holds
the signal below the HEALTHCHECK --timeout. Self-test rows S6 to S6c, and F0/F1, which run the
rendered program in the pinned runtime image against a server that never answers." \
  -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: R-NODE, the stub `docker`, and the call-site pins

**Files:**
- Modify: `ci/images/run.sh` (new function after `CONSOLE_STAGED_TREE_JS`; one call line in `smoke_consoles`)
- Test: `ci/images/console-selftest.sh`

**Interfaces:**
- Consumes: Task 1 helpers and loader.
- Produces:
  - `console_node_version_row <app>` in `run.sh`. Returns 0 on a match or a minor/patch gap (with a `::warning::`), 1 otherwise. Reads `$ROOT/.prototools`. Calls `docker run --rm --entrypoint /nodejs/bin/node <app>:dev --version`.
  - Harness: `stub_docker_main` and the stub file `$T/stub/docker`; `stub_reset` (sets `STUB_VERSION_OUT`, `STUB_VERSION_RC`, `STUB_VERSION_ERR`, `STUB_ENV_OUT`, `STUB_ENV_RC`, `STUB_WALK_OUT`, `STUB_WALK_RC`, `STUB_EXEC_SLEEP`, `STUB_EXEC_RC`, `FX_ROOT`); fixtures `$T/fx-node/.prototools` and `$T/fx-nopin/.prototools`; `$T/argv-node`; `run_fn <row> <want_rc> <present> <absent> <want-argv-file|none> <fn> [<arg>...]`; `count_line <file> <line>`, `drop_line <file> <line>`, `pin_rows <row> <fn-file> <line>`.

- [ ] **Step 1: Add the rows and the stub**

Change the `FUNCS=` line in `ci/images/console-selftest.sh` to:

```bash
FUNCS="assert_console_pins with_deadline console_node_version_row smoke_consoles"
```

Insert this block directly above the line `# --- summary ---`:

```bash
# --- smoke-row rows (a stub docker) ------------------------------------------------------------
# The stub records its argv (one argument per line, a multi-line argument as <multi-line>, and a
# `--` line after each call) and answers from STUB_* variables. Each row puts the stub first on
# PATH inside its own subshell only, so the F rows above use the real docker. `exec sleep` makes
# the sleep the process that with_deadline kills.
stub_docker_main() {
  local a
  for a in "$@"; do
    case "$a" in *$'\n'*) a="<multi-line>" ;; esac
    printf '%s\n' "$a" >> "$STUB_ARGV"
  done
  printf '%s\n' "--" >> "$STUB_ARGV"
  case "${1:-}" in
    run)
      for a in "$@"; do
        if [ "$a" = "--version" ]; then
          if [ -n "$STUB_VERSION_ERR" ]; then printf '%s\n' "$STUB_VERSION_ERR" >&2; fi
          if [ -n "$STUB_VERSION_OUT" ]; then printf '%s\n' "$STUB_VERSION_OUT"; fi
          exit "$STUB_VERSION_RC"
        fi
      done
      if [ -n "$STUB_WALK_OUT" ]; then printf '%s\n' "$STUB_WALK_OUT"; fi
      exit "$STUB_WALK_RC"
      ;;
    image)
      if [ -n "$STUB_ENV_OUT" ]; then printf '%s\n' "$STUB_ENV_OUT"; fi
      exit "$STUB_ENV_RC"
      ;;
    exec)
      if [ "$STUB_EXEC_SLEEP" -gt 0 ]; then exec sleep "$STUB_EXEC_SLEEP"; fi
      exit "$STUB_EXEC_RC"
      ;;
  esac
  echo "stub docker: unexpected argv: $*" >&2
  exit 99
}
# `declare -f`, not a heredoc: bash 5 writes a heredoc into a pipe before its reader starts, and
# on a host whose new pipe holds only 512 bytes that write can hang (CLAUDE.md, SMA-612).
mkdir -p "$T/stub"
{
  printf '%s\n' '#!/usr/bin/env bash'
  declare -f stub_docker_main
  # shellcheck disable=SC2016 # the line is written into the stub file literally
  printf '%s\n' 'stub_docker_main "$@"'
} > "$T/stub/docker"
chmod +x "$T/stub/docker"

stub_reset() {
  STUB_VERSION_OUT="v24.16.0"; STUB_VERSION_RC=0; STUB_VERSION_ERR=""
  STUB_ENV_OUT="PATH=/usr/bin"; STUB_ENV_RC=0
  STUB_WALK_OUT="walked=1300"; STUB_WALK_RC=0
  STUB_EXEC_SLEEP=0; STUB_EXEC_RC=0
  FX_ROOT="$T/fx-node"
}

# The fixture .prototools for the N rows: a fixed node pin, so a Node bump in the real
# .prototools does not red this self-test.
mkdir -p "$T/fx-node" "$T/fx-nopin"
printf '%s\n' 'node = "24.16.0"' 'pnpm = "11.3.0"' > "$T/fx-node/.prototools"
printf '%s\n' 'pnpm = "11.3.0"' > "$T/fx-nopin/.prototools"

# The argv that console_node_version_row must hand to docker.
printf '%s\n' run --rm --entrypoint /nodejs/bin/node iam-console:dev --version -- > "$T/argv-node"

# run_fn <row> <want_rc> <present> <absent> <want-argv-file|none> <fn> [<arg>...] — the
# production shape: `fn … || rc=$?`, so errexit is off inside the function.
run_fn() {
  local row="$1" want_rc="$2" present="$3" absent="$4" want_argv="$5" rc=0 o e
  shift 5
  o="$T/$row.out"
  e="$T/$row.err"
  rm -f "$T/argv"
  (
    PATH="$T/stub:$PATH"
    STUB_ARGV="$T/argv"
    export PATH STUB_ARGV STUB_VERSION_OUT STUB_VERSION_RC STUB_VERSION_ERR STUB_ENV_OUT STUB_ENV_RC \
      STUB_WALK_OUT STUB_WALK_RC STUB_EXEC_SLEEP STUB_EXEC_RC
    # shellcheck disable=SC2034 # ROOT is read by the function under test.
    ROOT="$FX_ROOT"
    set -uo pipefail
    "$@"
  ) >"$o" 2>"$e" || rc=$?
  if [ "$want_argv" = "none" ]; then
    if [ -e "$T/argv" ]; then
      say_fail "$row" "the stub docker was called, and it must not be" "$T/argv" "$o" "$e"
      return 0
    fi
  elif ! cmp -s "$want_argv" "$T/argv"; then
    diff "$want_argv" "$T/argv" > "$T/$row.argv-diff" 2>&1 || true
    say_fail "$row" "the stub docker got a different argv (< expected, > got)" "$T/$row.argv-diff" "$o" "$e"
    return 0
  fi
  check_row "$row" "$rc" "$want_rc" "$present" "$absent" "$o" "$e"
}

stub_reset
run_fn N0 0 "" "::warning::" "$T/argv-node" console_node_version_row iam-console
stub_reset; STUB_VERSION_OUT="v24.14.0"
run_fn N1 0 "::warning::iam-console: the runtime image runs Node 24.14.0|refresh closes the gap" "::error::" "$T/argv-node" console_node_version_row iam-console
stub_reset; STUB_VERSION_OUT="v24.18.0"
run_fn N1b 0 "::warning::iam-console: the runtime image runs Node 24.18.0|bump .prototools" "::error::" "$T/argv-node" console_node_version_row iam-console
stub_reset; STUB_VERSION_OUT="v23.1.0"
run_fn N2 1 "a different major" "" "$T/argv-node" console_node_version_row iam-console
stub_reset; STUB_VERSION_OUT="garbage"
run_fn N3 1 "could not parse" "" "$T/argv-node" console_node_version_row iam-console
stub_reset; STUB_VERSION_OUT=""; STUB_VERSION_RC=125
run_fn N4 1 "NOT checked — docker exited 125" "" "$T/argv-node" console_node_version_row iam-console
stub_reset; FX_ROOT="$T/fx-nopin"
run_fn N5 1 'no node = "X.Y.Z" pin' "" none console_node_version_row iam-console
# docker prints a platform-mismatch WARNING on stderr when an amd64 image runs on an arm64 host.
# Only stdout is parsed, so the row stays green.
stub_reset; STUB_VERSION_ERR="WARNING: The requested image's platform (linux/amd64) does not match the detected host platform (linux/arm64/v8)"
run_fn N6 0 "" "could not parse" "$T/argv-node" console_node_version_row iam-console

# --- call-site rows (P1, P2) -------------------------------------------------------------------
# A row function that smoke_consoles no longer calls proves nothing, and every row above stays
# green. So the call lines are pinned as WHOLE lines (leading blanks allowed), `|| ec=1` included.
# Each pin has a mutation that deletes its line, and the pin must then red.
count_line() {
  awk -v want="$2" '{ l = $0; sub(/^[[:space:]]+/, "", l); if (l == want) n++ } END { print n + 0 }' "$1"
}

drop_line() {
  awk -v want="$2" '{ l = $0; sub(/^[[:space:]]+/, "", l); if (l != want) print }' "$1"
}

# pin_rows <row> <fn-file> <line> — the pin on the real text, then on a copy without the line.
pin_rows() {
  local row="$1" file="$2" line="$3" n
  n="$(count_line "$file" "$line")"
  if [ "$n" -eq 1 ]; then
    say_pass "$row"
  else
    say_fail "$row" "found ${n} copies of the line '${line}' in ${file##*/}, expected exactly 1"
  fi
  drop_line "$file" "$line" > "$T/$row-mut.sh"
  if cmp -s "$file" "$T/$row-mut.sh"; then
    say_fail "$row-mut" "mutation did not apply"
    return 0
  fi
  n="$(count_line "$T/$row-mut.sh" "$line")"
  if [ "$n" -eq 0 ]; then
    say_pass "$row-mut"
  else
    say_fail "$row-mut" "the pin still found the line after the mutation deleted it"
  fi
}

# shellcheck disable=SC2016 # the pinned call line is literal text
pin_rows P1a "$T/fn-smoke_consoles.sh" 'console_node_version_row "$app" || ec=1'

```

- [ ] **Step 2: Run to see the loader fail**

Run: `/bin/bash ci/images/console-selftest.sh; echo "rc=$?"`
Expected: `console-selftest: infrastructure error (rc=2): no 'console_node_version_row() {' line in ci/images/run.sh; …` and `rc=2`.

- [ ] **Step 3: Add `console_node_version_row`**

In `ci/images/run.sh`, replace:

```bash
console.log(["public=" + (fs.existsSync(root + "/public") ? "1" : "0")].concat(rel).join("\n"));
'
```

with:

```bash
console.log(["public=" + (fs.existsSync(root + "/public") ? "1" : "0")].concat(rel).join("\n"));
'

# --- SMA-670 smoke rows ----------------------------------------------------------------------------
# smoke_consoles calls each function in this section as `<fn> … || ec=1`. Because of the `||`,
# errexit is OFF inside them: each one checks the rc of every command itself and must not depend on
# `set -e`. Each one keeps its JS program in a `local`, so it needs no global except ROOT (and
# with_deadline, for the HEALTHCHECK row). ci/images/console-selftest.sh copies them out of this
# file with awk and calls them in this same `|| rc=$?` shape against a stub `docker`.

# R-NODE (SMA-670 gap 1). The runtime base pins only the Node MAJOR (distroless publishes no
# patch-level tags), so nothing else records which Node the image runs. This row prints it. A
# different major, an unparseable version or an unreadable .prototools pin is an error. A different
# minor or patch is a WARNING and the row stays green (SMA-670 D1): no change in this repository can
# make the patch equal, because the runtime tag `nonroot` holds no version and the digest is the only
# pin. It needs only the image, not a running container. stdout only is parsed; docker's own stderr
# passes through.
console_node_version_row() {
  local app="$1" pin ver_out ver_rc=0 line maj min pat pmaj pmin ppat advice
  local pin_re='^([0-9]+)\.([0-9]+)\.([0-9]+)$' ver_re='^v([0-9]+)\.([0-9]+)\.([0-9]+)$'
  pin="$(sed -n 's/^node = "\([0-9.]*\)"$/\1/p' "$ROOT/.prototools")" || pin=""
  if ! [[ $pin =~ $pin_re ]]; then
    echo "::error::${app}: runtime Node version NOT checked — no node = \"X.Y.Z\" pin in .prototools." >&2
    return 1
  fi
  pmaj="${BASH_REMATCH[1]}"; pmin="${BASH_REMATCH[2]}"; ppat="${BASH_REMATCH[3]}"
  ver_out="$(docker run --rm --entrypoint /nodejs/bin/node "${app}:dev" --version)" || ver_rc=$?
  if [ "$ver_rc" -ne 0 ]; then
    echo "::error::${app}: runtime Node version NOT checked — docker exited ${ver_rc} on ${app}:dev before node printed a version, so the image is missing or unreadable." >&2
    return 1
  fi
  line="$(printf '%s\n' "$ver_out" | sed -n 1p)" || line=""
  if ! [[ $line =~ $ver_re ]]; then
    echo "::error::${app}: could not parse the runtime Node version from '${line}' — /nodejs/bin/node --version must print vX.Y.Z." >&2
    return 1
  fi
  maj="${BASH_REMATCH[1]}"; min="${BASH_REMATCH[2]}"; pat="${BASH_REMATCH[3]}"
  if [ "$maj" -ne "$pmaj" ]; then
    echo "::error::${app}: the runtime image runs Node ${line#v}, but .prototools pins ${pin} — a different major; the runtime FROM line in ts/Dockerfile and .prototools disagree." >&2
    return 1
  fi
  if [ "$min" -ne "$pmin" ] || [ "$pat" -ne "$ppat" ]; then
    if [ "$min" -lt "$pmin" ] || { [ "$min" -eq "$pmin" ] && [ "$pat" -lt "$ppat" ]; }; then
      advice="The runtime is older: a later runtime digest refresh closes the gap."
    else
      advice="The runtime is newer: bump .prototools and the builder FROM line together."
    fi
    echo "::warning::${app}: the runtime image runs Node ${line#v}, .prototools pins ${pin}. distroless publishes no patch-level tags, so only the major is held. ${advice}" >&2
    echo "  ${app}: runtime Node ${line#v}, same major as .prototools ${pin} (minor/patch differ, see the warning)"
    return 0
  fi
  echo "  ${app}: runtime Node ${line#v} matches .prototools"
}
```

- [ ] **Step 4: Run to see the call-site pin fail**

Run: `/bin/bash ci/images/console-selftest.sh; echo "rc=$?"`
Expected: `PASS N0` to `PASS N6` (eight rows), then `FAIL P1a — found 0 copies of the line 'console_node_version_row "$app" || ec=1' in fn-smoke_consoles.sh, expected exactly 1` and `FAIL P1a-mut — mutation did not apply`. `rc=1`.

- [ ] **Step 5: Call the row from `smoke_consoles`**

In `ci/images/run.sh`, replace:

```bash
      echo "::error::${app}: shell absence NOT checked — docker exited ${sh_rc} on ${app}:dev before reaching an entrypoint, so the image is missing or unreadable and nothing was proved about the runtime base." >&2
      ec=1
    fi
```

with:

```bash
      echo "::error::${app}: shell absence NOT checked — docker exited ${sh_rc} on ${app}:dev before reaching an entrypoint, so the image is missing or unreadable and nothing was proved about the runtime base." >&2
      ec=1
    fi

    # SMA-670: image-only rows. They need only the image, so they run whether or not the
    # container started. ci/images/console-selftest.sh pins each call line, `|| ec=1` included.
    console_node_version_row "$app" || ec=1
```

- [ ] **Step 6: Run everything, under both bashes**

Run: `/bin/bash ci/images/console-selftest.sh; echo "rc=$?"` and the same with `/opt/homebrew/bin/bash`.
Expected for each: `console-selftest: 33 passed, 0 failed, 0 skipped`, `rc=0`.

- [ ] **Step 7: Commit**

```bash
git add ci/images/run.sh ci/images/console-selftest.sh
git commit -m "feat(ci): print the Node version that each console image runs (SMA-670)" \
  -m "console_node_version_row reds on a different major and warns on a minor or patch gap, since
distroless publishes no patch tags. Self-test rows N0 to N6 run it against a stub docker, and
P1a pins its call line in smoke_consoles." \
  -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: R-CONFIG (baked `PAIGASUS_*` and `.env` files)

**Files:**
- Modify: `ci/images/run.sh` (new function after `console_node_version_row`; one call line)
- Test: `ci/images/console-selftest.sh`

**Interfaces:**
- Consumes: Task 6's stub, `stub_reset`, `run_fn`, `pin_rows`.
- Produces: `console_image_config_row <app>` in `run.sh`. It calls `docker image inspect --format '{{range .Config.Env}}{{println .}}{{end}}' <app>:dev`, then `docker run --rm --entrypoint /nodejs/bin/node <app>:dev -e <walk JS> /app`. The walk prints `walked=<N>` and then every `.env*` path. The function drops paths that hold `/node_modules/`.

- [ ] **Step 1: Add the rows**

Change the `FUNCS=` line to:

```bash
FUNCS="assert_console_pins with_deadline console_node_version_row smoke_consoles console_image_config_row"
```

Insert this block directly above the line `# --- summary ---`:

```bash
# --- console_image_config_row (E rows) ---------------------------------------------------------
# The argv that console_image_config_row must hand to docker: the Config.Env read, then the walk.
printf '%s\n' image inspect --format '{{range .Config.Env}}{{println .}}{{end}}' iam-console:dev -- \
  run --rm --entrypoint /nodejs/bin/node iam-console:dev -e '<multi-line>' /app -- > "$T/argv-config"

stub_reset
run_fn E0 0 "" "::error::" "$T/argv-config" console_image_config_row iam-console
# The error names the key and never the value.
stub_reset; STUB_ENV_OUT="$(printf '%s\n' 'PATH=/usr/bin' 'PAIGASUS_X=secret-value')"
run_fn E1 1 "bakes PAIGASUS_X" "secret-value" "$T/argv-config" console_image_config_row iam-console
stub_reset; STUB_WALK_OUT="$(printf '%s\n' 'walked=1300' '/app/apps/x/.env')"
run_fn E2 1 "holds .env file" "" "$T/argv-config" console_image_config_row iam-console
stub_reset; STUB_WALK_OUT="$(printf '%s\n' 'walked=1300' '/app/node_modules/p/.env.example')"
run_fn E2b 0 "" "::error::" "$T/argv-config" console_image_config_row iam-console
stub_reset; STUB_ENV_OUT=""; STUB_ENV_RC=1
run_fn E3 1 "image config NOT checked" "" "$T/argv-config" console_image_config_row iam-console
stub_reset; STUB_WALK_OUT=""; STUB_WALK_RC=125
run_fn E4 1 ".env scan NOT checked" "" "$T/argv-config" console_image_config_row iam-console
stub_reset; STUB_WALK_OUT="walked=0"
run_fn E5 1 "too few to prove anything" "" "$T/argv-config" console_image_config_row iam-console

# shellcheck disable=SC2016 # the pinned call line is literal text
pin_rows P1b "$T/fn-smoke_consoles.sh" 'console_image_config_row "$app" || ec=1'

```

- [ ] **Step 2: Run to see the loader fail**

Run: `/bin/bash ci/images/console-selftest.sh; echo "rc=$?"`
Expected: `console-selftest: infrastructure error (rc=2): no 'console_image_config_row() {' line in ci/images/run.sh; …`, `rc=2`.

- [ ] **Step 3: Add `console_image_config_row`**

In `ci/images/run.sh`, replace:

```bash
  echo "  ${app}: runtime Node ${line#v} matches .prototools"
}
```

with:

```bash
  echo "  ${app}: runtime Node ${line#v} matches .prototools"
}

# R-CONFIG (SMA-670 gap 2c, the behavioural half). Two reads of the image, and both need only the
# image. (1) Config.Env must hold no PAIGASUS_* key; the error names the key, never the value.
# (2) The image's own node walks /app without following symlinks, and no file whose base name
# starts with `.env` may be there outside node_modules: Next loads `.env*` from the server's own
# directory at runtime, so a value in such a file is baked configuration. A dependency can ship an
# `.env.example`, so a path with a node_modules directory in it is not reported. The walk still
# counts those files, and a count under 100 means that it read the wrong tree (measured: 1367 files
# in iam-console, 1324 in gateway-console).
console_image_config_row() {
  local app="$1" rc=0 env_out env_rc=0 keys key walk_out walk_rc=0 first n paths
  local walk_js='
const fs = require("fs");
let walked = 0;
const found = [];
const walk = (d) => {
  for (const e of fs.readdirSync(d, { withFileTypes: true })) {
    const p = d + "/" + e.name;
    if (e.isDirectory()) { walk(p); continue; }
    walked++;
    if (e.name.startsWith(".env")) found.push(p);
  }
};
walk(process.argv[1]);
console.log(["walked=" + walked].concat(found).join("\n"));
'
  env_out="$(docker image inspect --format '{{range .Config.Env}}{{println .}}{{end}}' "${app}:dev")" || env_rc=$?
  if [ "$env_rc" -ne 0 ]; then
    echo "::error::${app}: image config NOT checked — docker image inspect exited ${env_rc} on ${app}:dev, so the image is missing or unreadable." >&2
    rc=1
  else
    keys="$(printf '%s\n' "$env_out" | sed -n 's/^\(PAIGASUS_[^=]*\)=.*$/\1/p')" || keys=""
    if [ -n "$keys" ]; then
      while IFS= read -r key; do
        echo "::error::${app}:dev bakes ${key} into Config.Env — console config is deployment-varying and must stay runtime-only (the value is not printed)." >&2
      done < <(printf '%s\n' "$keys")
      rc=1
    fi
  fi
  walk_out="$(docker run --rm --entrypoint /nodejs/bin/node "${app}:dev" -e "$walk_js" /app)" || walk_rc=$?
  if [ "$walk_rc" -ne 0 ]; then
    echo "::error::${app}: .env scan NOT checked — the walk exited ${walk_rc} on ${app}:dev, so the image is missing or unreadable." >&2
    return 1
  fi
  first="$(printf '%s\n' "$walk_out" | sed -n 1p)" || first=""
  n=""
  case "$first" in walked=*) n="${first#walked=}" ;; esac
  case "$n" in ''|*[!0-9]*) n="" ;; esac
  if [ -z "$n" ] || [ "$n" -lt 100 ]; then
    echo "::error::${app}: .env scan walked ${n:-an unreadable number of ('${first}')} files under /app — too few to prove anything; the walk read the wrong tree." >&2
    return 1
  fi
  # The node_modules filter is here, not in the JS, so the self-test's stub rows exercise it.
  # grep -v rc 1 means "every path was under node_modules", which leaves `paths` empty.
  paths="$(printf '%s\n' "$walk_out" | sed -n '2,$p' | grep -v '/node_modules/')" || paths=""
  if [ -n "$paths" ]; then
    echo "::error::${app}: the image holds .env file(s) under /app outside node_modules; Next loads them at runtime, so a value in them is baked configuration. The paths follow." >&2
    printf '%s\n' "$paths" >&2
    return 1
  fi
  if [ "$rc" -ne 0 ]; then return 1; fi
  echo "  ${app}: no PAIGASUS_* in Config.Env and no .env file in /app (${n} files walked)"
}
```

- [ ] **Step 4: Run to see the call-site pin fail**

Run: `/bin/bash ci/images/console-selftest.sh; echo "rc=$?"`
Expected: `PASS E0` to `PASS E5` (seven rows), `FAIL P1b — found 0 copies …` and `FAIL P1b-mut — mutation did not apply`. `rc=1`.

- [ ] **Step 5: Call the row from `smoke_consoles`**

In `ci/images/run.sh`, replace:

```bash
    console_node_version_row "$app" || ec=1
```

with:

```bash
    console_node_version_row "$app" || ec=1
    console_image_config_row "$app" || ec=1
```

- [ ] **Step 6: Run everything, under both bashes**

Run: `/bin/bash ci/images/console-selftest.sh; echo "rc=$?"` and the same with `/opt/homebrew/bin/bash`.
Expected for each: `console-selftest: 42 passed, 0 failed, 0 skipped`, `rc=0`.

- [ ] **Step 7: Commit**

```bash
git add ci/images/run.sh ci/images/console-selftest.sh
git commit -m "feat(ci): scan each console image for baked PAIGASUS_* config (SMA-670)" \
  -m "console_image_config_row reds on a PAIGASUS_* key in Config.Env (key only, never the value)
and on a .env file under /app outside node_modules. Self-test rows E0 to E5 and pin P1b." \
  -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: R-HEALTH (the bounded `HEALTHCHECK` row)

**Files:**
- Modify: `ci/images/run.sh` (`CONSOLE_HC_DEADLINE` after `CONSOLE_SMOKE_ENV`; new function after `console_image_config_row`; the `HEALTHCHECK` block and the `local` list in `smoke_consoles`)
- Test: `ci/images/console-selftest.sh`

**Interfaces:**
- Consumes: `with_deadline` (unchanged), Task 6's stub and helpers.
- Produces: global `CONSOLE_HC_DEADLINE=20`; `console_healthcheck_row <name> <app> <base_path> <deadline>` in `run.sh`. It calls `with_deadline "$deadline" docker exec "$name" /nodejs/bin/node /app/healthcheck.mjs >"$out" 2>&1 || hc_rc=$?`.

- [ ] **Step 1: Add the rows**

Change the `FUNCS=` line to:

```bash
FUNCS="assert_console_pins with_deadline console_node_version_row smoke_consoles console_image_config_row console_healthcheck_row"
```

Insert this block directly above the line `# --- summary ---`:

```bash
# --- console_healthcheck_row (H rows) ----------------------------------------------------------
# The argv that console_healthcheck_row must hand to docker (through with_deadline).
printf '%s\n' exec smoke-selftest /nodejs/bin/node /app/healthcheck.mjs -- > "$T/argv-health"

stub_reset
run_fn H0 0 "" "::error::" "$T/argv-health" console_healthcheck_row smoke-selftest iam-console /iam 5
stub_reset; STUB_EXEC_SLEEP=10
run_fn H1 1 "did not finish within 2s" "" "$T/argv-health" console_healthcheck_row smoke-selftest iam-console /iam 2
stub_reset; STUB_EXEC_RC=1
run_fn H2 1 "exited 1" "did not finish" "$T/argv-health" console_healthcheck_row smoke-selftest iam-console /iam 5
stub_reset; STUB_EXEC_RC=137
run_fn H3 1 "exited 137" "did not finish" "$T/argv-health" console_healthcheck_row smoke-selftest iam-console /iam 5
stub_reset
run_fn H4 1 "positive integer" "" none console_healthcheck_row smoke-selftest iam-console /iam abc
stub_reset
run_fn H5 1 "positive integer" "" none console_healthcheck_row smoke-selftest iam-console /iam 0

# shellcheck disable=SC2016 # the pinned call lines are literal text
pin_rows P1c "$T/fn-smoke_consoles.sh" 'console_healthcheck_row "$name" "$app" "$base_path" "$CONSOLE_HC_DEADLINE" || ec=1'
# shellcheck disable=SC2016 # the pinned call lines are literal text
pin_rows P2 "$T/fn-console_healthcheck_row.sh" 'with_deadline "$deadline" docker exec "$name" /nodejs/bin/node /app/healthcheck.mjs >"$out" 2>&1 || hc_rc=$?'

```

- [ ] **Step 2: Run to see the loader fail**

Run: `/bin/bash ci/images/console-selftest.sh; echo "rc=$?"`
Expected: `console-selftest: infrastructure error (rc=2): no 'console_healthcheck_row() {' line in ci/images/run.sh; …`, `rc=2`.

- [ ] **Step 3: Add the global and `console_healthcheck_row`**

In `ci/images/run.sh`, replace:

```bash
  -e "PAIGASUS_IAM_GRPC_URL=http://iam:9090"
)
```

with:

```bash
  -e "PAIGASUS_IAM_GRPC_URL=http://iam:9090"
)

# SMA-670 gap 3: the wall-clock bound, in seconds, on the smoke row that runs the image's own
# HEALTHCHECK program with `docker exec`. The program's own fetch signal (2500 ms, ts/Dockerfile)
# ends a hang first; this bound is for a hang that the signal does not end.
CONSOLE_HC_DEADLINE=20
```

Then replace:

```bash
  if [ "$rc" -ne 0 ]; then return 1; fi
  echo "  ${app}: no PAIGASUS_* in Config.Env and no .env file in /app (${n} files walked)"
}
```

with:

```bash
  if [ "$rc" -ne 0 ]; then return 1; fi
  echo "  ${app}: no PAIGASUS_* in Config.Env and no .env file in /app (${n} files walked)"
}

# R-HEALTH (SMA-670 gap 3). The image's OWN HEALTHCHECK program, run inside the running container.
# Nothing else in this suite executes /app/healthcheck.mjs: ts/Dockerfile writes it with a `printf`
# that carries a backtick template literal and a `%s` substitution, so an escaping or path
# regression would otherwise ship with every other row green. `docker exec` of the image's node
# needs no shell. The call runs under with_deadline, so a probe that hangs cannot hang this suite.
# Its output goes to a mktemp FILE, never to a `$( )` capture: MEASURED (SMA-670 M7), a captured
# with_deadline waits for its full deadline under bash 5, because the watchdog's orphan `sleep`
# holds the capture pipe open. A timeout is reported only when the rc is 143 or 137 AND the elapsed
# time reached the deadline; any other 137 (for example from the OOM killer) takes the "exited"
# message. A killed `docker exec` client can leave node running in the container;
# console_smoke_cleanup and the EXIT trap remove the container.
console_healthcheck_row() {
  local name="$1" app="$2" base_path="$3" deadline="$4" out hc_rc=0 start elapsed deadline_ok
  case "$deadline" in
    ''|*[!0-9]*) deadline_ok=0 ;;
    *) deadline_ok=1 ;;
  esac
  if [ "$deadline_ok" -eq 0 ] || [ "$deadline" -lt 1 ]; then
    echo "::error::${app}: HEALTHCHECK program NOT checked — the deadline '${deadline}' is not a positive integer number of seconds." >&2
    return 1
  fi
  out="$(mktemp "${TMPDIR:-/tmp}/paigasus-console-hc.XXXXXX")" || out=""
  if [ -z "$out" ]; then
    echo "::error::${app}: HEALTHCHECK program NOT checked — mktemp failed." >&2
    return 1
  fi
  start=$SECONDS
  with_deadline "$deadline" docker exec "$name" /nodejs/bin/node /app/healthcheck.mjs >"$out" 2>&1 || hc_rc=$?
  elapsed=$((SECONDS - start))
  if [ "$hc_rc" -eq 0 ]; then
    rm -f "$out"
    echo "  ${app}: HEALTHCHECK program /app/healthcheck.mjs exits 0"
    return 0
  fi
  if { [ "$hc_rc" -eq 143 ] || [ "$hc_rc" -eq 137 ]; } && [ "$elapsed" -ge "$deadline" ]; then
    echo "::error::${app}: the image's HEALTHCHECK program (/app/healthcheck.mjs) did not finish within ${deadline}s against a server that renders ${base_path} — the probe hangs; check the fetch timeout in ts/Dockerfile's healthcheck printf. Its output follows." >&2
  else
    echo "::error::${app}: the image's HEALTHCHECK program (/app/healthcheck.mjs) exited ${hc_rc} against a server that renders ${base_path} — check the healthcheck printf in ts/Dockerfile. Its output follows." >&2
  fi
  cat "$out" >&2 || true
  rm -f "$out"
  return 1
}
```

- [ ] **Step 4: Run to see the call-site pin fail**

Run: `/bin/bash ci/images/console-selftest.sh; echo "rc=$?"`
Expected: `PASS H0` to `PASS H5` (six rows; H1 takes about 2 s), `FAIL P1c — found 0 copies …`, `FAIL P1c-mut — mutation did not apply`, `PASS P2`, `PASS P2-mut`. `rc=1`.

- [ ] **Step 5: Move the old `HEALTHCHECK` block into the call**

In `ci/images/run.sh`, replace:

```bash
    # The image's OWN HEALTHCHECK program, run inside the container. Nothing else in this suite
    # executes /app/healthcheck.mjs: ts/Dockerfile writes it with a `printf` that carries a
    # backtick template literal and a `%s` substitution, so an escaping or path regression would
    # otherwise ship with every other row green. `docker exec` of the image's node needs no shell.
    # It runs only once the page rendered, because the probe it makes is the same server's
    # <basePath>/healthz. It does not set `bad`: the chunk rows below do not depend on it.
    # GUARDED: a non-zero exit is the finding, and must reach the named message, not abort.
    if [ "$bad" -eq 0 ]; then
      hc_rc=0
      hc_out="$(docker exec "$name" /nodejs/bin/node /app/healthcheck.mjs 2>&1)" || hc_rc=$?
      if [ "$hc_rc" -ne 0 ]; then
        echo "::error::${app}: the image's HEALTHCHECK program (/app/healthcheck.mjs) exited ${hc_rc} against a server that renders ${base_path} — check the healthcheck printf in ts/Dockerfile. Its output follows." >&2
        printf '%s\n' "$hc_out" >&2
        ec=1
      else
        echo "  ${app}: HEALTHCHECK program /app/healthcheck.mjs exits 0"
      fi
    fi
```

with:

```bash
    # The image's OWN HEALTHCHECK program (console_healthcheck_row above, SMA-670 gap 3). It runs
    # only once the page rendered, because the probe it makes is the same server's
    # <basePath>/healthz. It does not set `bad`: the chunk rows below do not depend on it.
    if [ "$bad" -eq 0 ]; then
      console_healthcheck_row "$name" "$app" "$base_path" "$CONSOLE_HC_DEADLINE" || ec=1
    fi
```

Then replace:

```bash
  local host_std host_static host_id run_rc sh_rc img_rc cstate hc_rc hc_out
```

with:

```bash
  local host_std host_static host_id run_rc sh_rc img_rc cstate
```

- [ ] **Step 6: Run everything, under both bashes**

Run: `/bin/bash ci/images/console-selftest.sh; echo "rc=$?"` and the same with `/opt/homebrew/bin/bash`.
Expected for each: `console-selftest: 52 passed, 0 failed, 0 skipped`, `rc=0`, in about 15 s.

- [ ] **Step 7: Commit**

```bash
git add ci/images/run.sh ci/images/console-selftest.sh
git commit -m "feat(ci): run the console HEALTHCHECK smoke row under a deadline (SMA-670)" \
  -m "console_healthcheck_row runs docker exec under with_deadline (CONSOLE_HC_DEADLINE=20),
writes to a temp file, and names a timeout only when the elapsed time reached the deadline.
Self-test rows H0 to H5, and pins P1c and P2." \
  -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 9: The AC 3 text, RUNBOOK section 6 and the citations

**Files:**
- Modify: `ci/images/run.sh` (`smoke_consoles` step 4 comment and success line)
- Modify: `docs/ops/RUNBOOK-containers.md` (section 6, lines 312–406)
- Modify: `ts/apps/iam-console/tests/cluster/phase-a/cross-zone.spec.ts:7`
- Modify: `ts/CLAUDE.md:147-150`

**Interfaces:**
- Consumes: the names from Tasks 2–8 (for the RUNBOOK text).
- Produces: none for code. `cross-zone.spec.ts` and `ts/CLAUDE.md` cite the RUNBOOK by section and bullet name, not by line numbers.

- [ ] **Step 1: Correct the step-4 comment and success line in `run.sh`**

In `ci/images/run.sh`, replace:

```bash
    # Step 4: the other zone's prefix does not serve it. This proves ONLY that a basePath is in
    # effect in this one container. It does NOT prove the two zones' assets do not collide:
    # MEASURED on iam-console:dev, the real chunk also 404s under /zzz/… and with no prefix at all,
    # so any unknown prefix gives this result. The real acceptance-criterion-3 proof needs both
    # zones behind one ingress, and belongs to that ingress (SMA-513 PR 2a), not to this row.
```

with:

```bash
    # Step 4: the other zone's prefix does not serve it. SMA-513 acceptance criterion 3 has two
    # halves (SMA-670 spec § 4.4). Steps 2 to 4 of this smoke test prove the PER-IMAGE half
    # (SMA-513 spec D4): each zone emits its asset URLs under its own basePath and serves them
    # there. This step alone proves only that a basePath is in effect in this one container:
    # MEASURED on iam-console:dev, the real chunk also 404s under /zzz/… and with no prefix at all,
    # so any unknown prefix gives this result, and this step does not prove that the two zones'
    # assets do not collide. The ONE-ORIGIN half is kind row R2
    # (ts/apps/iam-console/tests/cluster/phase-a/cross-zone.spec.ts): both zones hydrate through
    # one Traefik ingress.
```

Then replace:

```bash
        echo "  ${app}: serves ${chunk} (${bytes} bytes), 404 under ${other} (a basePath is in effect; cross-zone collision is NOT checked here — that needs the ingress)"
```

with:

```bash
        echo "  ${app}: serves ${chunk} (${bytes} bytes), 404 under ${other} (a basePath is in effect: the per-image half of AC 3; the one-origin half is kind row R2, cross-zone.spec.ts)"
```

- [ ] **Step 2: RUNBOOK — the image-pin bullet**

In `docs/ops/RUNBOOK-containers.md`, replace:

```markdown
  `/app`, so this pin controls what the image runs. `assert_console_pins` in `ci/images/run.sh`
  fails if either `FROM` line has no `@sha256:` digest, or if the runtime tag is not `nonroot`.
```

with:

```markdown
  `/app`, so this pin controls what the image runs. `assert_console_pins` in `ci/images/run.sh`
  fails if either `FROM` line has no `@sha256:` digest, or if the runtime tag is not `nonroot`.
- **Every image that the build reads is one of the two digest-pinned `FROM` images.**
  `assert_console_pins` reads `ts/Dockerfile` after it drops comment lines and joins continuation
  lines. It fails if the file has a number of `FROM` instructions that is not 2, or a `FROM` line
  that does not match one of the two pin patterns. It fails if a `--from=` value, or a `from=`
  value in a `RUN --mount=` argument, is not `builder` or `bindings`. A `--from=<image>` pulls an
  image that no `FROM` line pins. It also fails if the file starts with a parser directive
  (`# syntax=`, `# escape=` or `# check=`, in any letter case). A `# syntax=` directive pulls an
  unpinned BuildKit frontend image, and an `# escape=` directive changes how the check reads the
  file. A heredoc body line that starts with `from` counts as a `FROM` line, so it gives a false
  failure.
```

- [ ] **Step 3: RUNBOOK — the healthcheck bullets**

Replace:

```markdown
- **The healthcheck file fetches `<BASE_PATH>/healthz` on `127.0.0.1:$PORT`.** `smoke_consoles`
  runs this file in the running container with `docker exec` and the image's own node. It fails if
  the file exits with a code that is not 0.
```

with:

```markdown
- **The healthcheck file fetches `<BASE_PATH>/healthz` on `127.0.0.1:$PORT`, with a 2500 ms
  signal.** The `fetch` call passes `signal: AbortSignal.timeout(2500)`. Docker kills the probe
  after `--timeout=3s`. Without the signal, a server that accepts the connection and never answers
  holds the probe until Docker kills it. `assert_console_pins` fails if the `printf` line that
  writes the file does not hold exactly one `AbortSignal.timeout(<ms>)`, or if that value is not
  less than the `HEALTHCHECK --timeout`. It reads only a `--timeout=<N>s` value in whole seconds.
- **`smoke_consoles` runs the healthcheck file in the running container, with a deadline.** It
  uses `docker exec` and the image's own node, under `with_deadline` with `CONSOLE_HC_DEADLINE`
  (20 s). It fails if the file exits with a code that is not 0. It reports a timeout only when the
  exit code is 143 or 137 and the elapsed time reached the deadline.
```

- [ ] **Step 4: RUNBOOK — the runtime Node version bullet**

Replace:

```markdown
  does not copy them. The runtime base can hold only the major, because distroless publishes no
  patch-level tags.
```

with:

```markdown
  does not copy them. The runtime base can hold only the major, because distroless publishes no
  patch-level tags.
- **`smoke_consoles` prints the Node version that the runtime image runs.** The runtime base pins
  only the Node major, so no file records the full version. A different major, a version that the
  row cannot parse, or a missing `node` pin in `.prototools` is an error. A different minor or
  patch version is a `::warning::`, and the row stays green. On 2026-09-25 the runtime image ran
  Node 24.14.0 and `.prototools` pinned 24.16.0. When the runtime is older, a later digest refresh
  of the runtime base closes the gap. When the runtime is newer, change `.prototools` and the
  builder `FROM` line together.
```

- [ ] **Step 5: RUNBOOK — the runtime-configuration bullets**

Replace:

```markdown
  variable. `assert_console_pins` reads `ENV` and `ARG` instructions in `ts/Dockerfile` to enforce
  this. It joins continuation lines first and matches `ENV` and `ARG` in any letter case. It does
  not see a value that a `RUN` step writes into a file. There is one exception:
  `PAIGASUS_COMPILED_*` (`PAIGASUS_COMPILED_ZONE`, `PAIGASUS_COMPILED_BASE_PATH`).
  `createNextConfig` in `ts/packages/paigasus-next-config` writes these at build time on purpose.
  They record the zone that the artifact was built for, so `runtime.ts` can compare them with the
  `PAIGASUS_ZONE` that a deployment supplies. They are not deployment-varying configuration.
```

with:

```markdown
  variable. `assert_console_pins` reads `ENV` and `ARG` instructions in `ts/Dockerfile` to enforce
  this. It joins continuation lines first and matches `ENV` and `ARG` in any letter case. After
  that check, no other line of `ts/Dockerfile` can hold the text `PAIGASUS_`: a `RUN`, `COPY`,
  `ADD` or `ONBUILD` step and a heredoc body line are errors. Comment lines are not read. The text
  check needs the literal text `PAIGASUS_`, so a name that a step builds from parts, or a name from
  `--build-arg`, gets past it. There is one exception to the rule:
  `PAIGASUS_COMPILED_*` (`PAIGASUS_COMPILED_ZONE`, `PAIGASUS_COMPILED_BASE_PATH`).
  `createNextConfig` in `ts/packages/paigasus-next-config` writes these at build time on purpose.
  They record the zone that the artifact was built for, so `runtime.ts` can compare them with the
  `PAIGASUS_ZONE` that a deployment supplies. They are not deployment-varying configuration.
  `ts/Dockerfile` never writes them, so the text check has no exemption for them.
- **`smoke_consoles` reads the built image for baked configuration.** It fails if `Config.Env` of
  the image holds a `PAIGASUS_*` key. The error names the key and never the value. It also walks
  `/app` with the image's own node, and it fails if a file whose name starts with `.env` is there
  outside `node_modules`. The walk must count at least 100 files, or it read the wrong tree. It
  does not see a `PAIGASUS_*` value in a file with a different name, or an `.env*` file under
  `node_modules`.
```

- [ ] **Step 6: RUNBOOK — the `pnpm` bullet**

Replace:

```markdown
- **Every `pnpm install` in `ts/Dockerfile` uses `--frozen-lockfile`.** `assert_console_pins`
  fails if one `pnpm install` has no bare `--frozen-lockfile` flag. It also fails on any
  `--frozen-lockfile=<value>` form and on `--no-frozen-lockfile`.
```

with:

```markdown
- **Every `pnpm install` and `pnpm i` in `ts/Dockerfile` uses `--frozen-lockfile`.**
  `assert_console_pins` finds each `pnpm` invocation and splits it into words. It treats the
  invocation as an install when a word is `install`, `i`, `install-test` or `it`. So
  `pnpm --filter x install`, `pnpm -C ts i` and `sh -c "pnpm install"` are installs, and
  `pnpm info` and `pnpm exec` are not. It fails if one install has no bare `--frozen-lockfile`
  flag. It also fails on any `--frozen-lockfile=<value>` form and on `--no-frozen-lockfile`. It
  does not check `pnpm add`, `pnpm update` or `npm install`.
```

- [ ] **Step 7: RUNBOOK — the AC 3 bullet**

Replace:

```markdown
  unknown prefix and under no prefix (measured on `iam-console:dev`). So the row does not prove
  that the assets of the two zones do not collide. That proof needs both zones behind one ingress,
  and it belongs to the ingress work (SMA-513 PR 2a).
```

with:

```markdown
  unknown prefix and under no prefix (measured on `iam-console:dev`). So the row alone does not
  prove that the assets of the two zones do not collide. Acceptance criterion 3 of SMA-513 has two
  halves. The container smoke test (steps 2 to 4 of `smoke_consoles`) proves the per-image half
  (SMA-513 spec D4): each zone emits its asset URLs under its own basePath and serves them there.
  Kind row R2 (`ts/apps/iam-console/tests/cluster/phase-a/cross-zone.spec.ts`) proves the
  one-origin half: both zones hydrate through one Traefik ingress.
```

- [ ] **Step 8: RUNBOOK — the self-test and the kind rc 2 note**

Replace:

```markdown
  When the assumption fails, it fails on every run. The failure goes to the parity error that
  already states that the mismatch is not a drift between `ts/Dockerfile` and `moon.yml`.

## 7. What the first Deployment needs
```

with:

```markdown
  When the assumption fails, it fails on every run. The failure goes to the parity error that
  already states that the mismatch is not a drift between `ts/Dockerfile` and `moon.yml`.

`ci/images/console-selftest.sh` proves the checks of this section. For each check it applies one
mutation and requires the check's own error text, and the unchanged files must stay green. It runs
the rendered healthcheck file in the pinned runtime image against a server that never answers:
with the signal the file exits 1 with a `TimeoutError`, and without it the file hangs until a
watchdog kills it. `images.yml` runs the self-test directly before the console build. Run it
locally with `/bin/bash ci/images/console-selftest.sh`. `ci/kind/run.sh` reports every
`build-console` failure as an infrastructure error (rc 2), so a failure of a static check in
`assert_console_pins` shows there as rc 2, not as a test failure.

## 7. What the first Deployment needs
```

- [ ] **Step 9: The `cross-zone.spec.ts` citation**

In `ts/apps/iam-console/tests/cluster/phase-a/cross-zone.spec.ts`, replace:

```ts
// base path through Traefik (docs/ops/RUNBOOK-containers.md:370-374 left that proof open).
```

with:

```ts
// base path through Traefik: the one-origin half of AC 3 (docs/ops/RUNBOOK-containers.md, section 6,
// the bullet on the zone row of smoke_consoles).
```

- [ ] **Step 10: The `ts/CLAUDE.md` citations**

In `ts/CLAUDE.md`, replace:

```markdown
- The console image rules live in two places already. Do not copy them here. The static-asset
  staging rule is at `docs/ops/RUNBOOK-containers.md:364-369`. The exec-form
  `ENTRYPOINT`/`HEALTHCHECK` rule (no `ARG`/`ENV` expansion) is at `rs/CLAUDE.md:207` and
  `docs/ops/RUNBOOK-containers.md:317-322`.
```

with:

```markdown
- The console image rules live in two places already. Do not copy them here. The static-asset
  staging rule is in `docs/ops/RUNBOOK-containers.md` section 6, the bullet "The standalone output
  has no static assets". The exec-form `ENTRYPOINT`/`HEALTHCHECK` rule (no `ARG`/`ENV` expansion)
  is at `rs/CLAUDE.md:207` and in the same RUNBOOK section, the bullet "The image holds two
  fixed-path `.mjs` files".
```

- [ ] **Step 11: Check the results**

Run: `grep -n 'RUNBOOK-containers.md:[0-9]' ts/CLAUDE.md ts/apps/iam-console/tests/cluster/phase-a/cross-zone.spec.ts`
Expected: no output (exit 1).
Run: `grep -c 'The standalone output has no static assets\|The image holds two fixed-path' docs/ops/RUNBOOK-containers.md`
Expected: `2` (both cited bullet names exist).
Run: `pnpm -C ts exec prettier --check apps/iam-console/tests/cluster/phase-a/cross-zone.spec.ts`
Expected: `All matched files use Prettier code style!`
Run: `/bin/bash -n ci/images/run.sh && /bin/bash ci/images/console-selftest.sh; echo "rc=$?"`
Expected: `console-selftest: 52 passed, 0 failed, 0 skipped`, `rc=0`.

- [ ] **Step 12: Commit**

```bash
git add ci/images/run.sh docs/ops/RUNBOOK-containers.md ts/CLAUDE.md \
  ts/apps/iam-console/tests/cluster/phase-a/cross-zone.spec.ts
git commit -m "docs(ci): record the console image checks and correct the AC 3 text (SMA-670)" \
  -m "RUNBOOK section 6 gets one bullet per new check. The step-4 text in smoke_consoles and the
RUNBOOK now split AC 3 into the per-image half (container smoke) and the one-origin half (kind
row R2). The ts citations name the RUNBOOK section, not stale line numbers." \
  -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 10: The `images.yml` step and the final verification

**Files:**
- Modify: `.github/workflows/images.yml` (before the step "Build + smoke both consoles", line 163)

**Interfaces:**
- Consumes: `ci/images/console-selftest.sh` (exit 0 on a pass, 1 on a FAIL or on a SKIP with `CI=true`, 2 on a loader error).
- Produces: the workflow step "Console image self-test".

- [ ] **Step 1: Add the workflow step**

In `.github/workflows/images.yml`, replace:

```yaml
      # Placed AFTER the rs release-path evidence (build, load, smoke, rehearse, SBOM) and BEFORE
```

with:

```yaml
      # SMA-670: the self-test of the console image checks in ci/images/run.sh. Each static check
      # and each console smoke row must red on its own mutation, against a stub docker, and the
      # healthcheck's fetch signal must end a hang in the pinned runtime image. It sits directly
      # before the console build, so all rs evidence above already exists when it runs. With
      # CI=true a skipped row is a failure.
      - name: Console image self-test
        run: ci/images/console-selftest.sh

      # Placed AFTER the rs release-path evidence (build, load, smoke, rehearse, SBOM) and BEFORE
```

- [ ] **Step 2: The self-test under both bashes**

Run: `/bin/bash ci/images/console-selftest.sh; echo "rc=$?"`
Run: `/opt/homebrew/bin/bash ci/images/console-selftest.sh; echo "rc=$?"`
Expected for each: `console-selftest: 52 passed, 0 failed, 0 skipped`, `rc=0`.

- [ ] **Step 3: The six unchanged functions**

Write this to `<scratchpad>/t10-frozen.sh` and run `/bin/bash <scratchpad>/t10-frozen.sh` from the worktree root:

```bash
set -u
git show main:ci/images/run.sh > "${TMPDIR:-/tmp}/run.sh.main"
for fn in crate_for assert_pins build_one smoke assert_base_intact with_deadline; do
  a="$(awk -v want="$fn() {" '$0 == want { on = 1 } on { print } on && $0 == "}" { on = 0 }' "${TMPDIR:-/tmp}/run.sh.main")"
  b="$(awk -v want="$fn() {" '$0 == want { on = 1 } on { print } on && $0 == "}" { on = 0 }' ci/images/run.sh)"
  if [ -n "$a" ] && [ "$a" = "$b" ]; then echo "unchanged: $fn"; else echo "CHANGED OR MISSING: $fn"; fi
done
rm -f "${TMPDIR:-/tmp}/run.sh.main"
```

Expected: six `unchanged:` lines, no `CHANGED OR MISSING` line.

- [ ] **Step 4: The real console build and smoke**

Docker must be running. Run:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
ci/images/run.sh all-consoles; echo "rc=$?"
```

Expected, among the other lines: the `assert_console_pins` success line that ends with `healthcheck signal 2500 ms < --timeout=3s`; for each zone a `::warning::<app>: the runtime image runs Node 24.14.0, .prototools pins 24.16.0. … The runtime is older: a later runtime digest refresh closes the gap.` line (or a `matches .prototools` line after a runtime digest refresh), `<app>: no PAIGASUS_* in Config.Env and no .env file in /app (… files walked)`, `<app>: HEALTHCHECK program /app/healthcheck.mjs exits 0`; then `== CONSOLE SMOKE OK ==` and `rc=0`. This takes several minutes (two Next builds). If the staged-tree parity row reports "built from DIFFERENT sources", that is not from this change: report it, and do not delete `.next`.

- [ ] **Step 5: The workflow gate**

Run:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:actionlint; echo "rc=$?"
```

Expected: `rc=0`. Read the gate's pipe preflight line first. If it reports a pipe that holds only 512 bytes and exits rc 2, this host is in the small-pipe state (CLAUDE.md, SMA-612): record that, and rely on CI for this gate. Do not read that rc 2 as a finding.

- [ ] **Step 6: Commit**

```bash
git add .github/workflows/images.yml
git commit -m "ci(ci): run the console image self-test in images.yml (SMA-670)" \
  -m "The step runs ci/images/console-selftest.sh directly before the console build." \
  -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git log --oneline main..HEAD
```

Expected: the last command lists the spec commit and the ten task commits, and no other commit.

---

## After the PR is open (controller step, not an implementer task)

Spec § 4.4 and A5 ask for a comment on PR 279. The comment links to this PR, so it can only be posted after the open-pr stage. It is visible to other people: ask the user before you post it. Do not edit the PR 279 body. Put this text in a file, replace `<PR>` with this PR's number, and run `gh pr comment 279 --body-file <file>`:

```markdown
Correction to the AC 3 statement in this PR's description.

AC 3 has two halves. The container smoke test in `ci/images/run.sh` (steps 2 to 4 of
`smoke_consoles`) proves the per-image half (SMA-513 spec D4): each zone emits its asset URLs
under its own basePath and serves them there. Step 4 alone proves only that a basePath is in
effect. Kind row R2 (`ts/apps/iam-console/tests/cluster/phase-a/cross-zone.spec.ts`, PR 296)
proves the one-origin half: both zones hydrate through one Traefik ingress.

The repository text now says this (SMA-670, #<PR>). The description above stays as it was.
```

---

## Deviations from the spec (measured 2026-09-25)

1. **Row S1c.** The spec removes the builder digest too. MEASURED: the existing builder-pin check then reds first with "the builder FROM line must be exactly one node:X.Y.Z-bookworm@sha256:<64 hex> AS builder", so the row can never hold `FROM instruction(s)`. The plan keeps the digest. The `RUN true \` line alone makes the normalised file hold one `FROM`, and S-FROM reds.
2. **The `node_modules` filter (R-CONFIG).** The spec puts it in the walk, but row E2b feeds a `node_modules` path through the stub and expects rc 0. So the filter must be in the shell. The walk prints every `.env*` path, and the function drops paths that hold `/node_modules/`.
3. **S-INSTALL token strip.** Each awk token loses `"`, `'`, `` ` ``, `(` and `)` before the compare. MEASURED: without it, `RUN sh -c "pnpm install"` gives the token `install"` and the install is missed, but the old `pnpm[[:space:]]+install` match caught it. Checked under BSD awk 20200816, mawk and gawk. Row S2e guards it.
4. **Two substrings in one row.** Rows N1 and N1b need `::warning::` and the advice sentence. `check_row` takes a `|`-separated list for the present substrings.
5. **Stub file.** The harness writes the stub with `declare -f`, not with a heredoc, because bash 5 writes a heredoc into a pipe before the reader starts (the SMA-612 small-pipe hang). This is reasoned, not measured: the host pipe was healthy during these runs.
6. **P1 split.** P1 became P1a, P1b and P1c, so each task pins the call line it adds.
7. **S-HCTIMEOUT placement.** It reads the raw file, so it runs last, after the temp file is removed. S-DIRECTIVE runs before the normaliser.
8. **Extra rows** S1g, S2e, S6c, N6 and H5 (Review Focus). E1 also requires that the value is absent.
9. **Commit scope.** The task brief suggested `docs(ops)`. commitlint's `scope-enum` has no `ops`, so the plan uses `docs(ci)`.

Measurements taken on the prototype in the scratchpad (the full plan code, applied to copies of the three files):
- 52 of 52 rows pass under `/bin/bash` 3.2.57 (14 s) and under Homebrew bash 5.3.20 (13 s).
- In `ubuntu:24.04` (bash 5.2.21, GNU grep 3.11, GNU sed 4.9, mawk), 50 rows pass and F0/F1 print SKIP (no Docker in the container), rc 0. With `CI=true`, the harness prints the skip failure line.
- Against the unchanged `run.sh`, all 15 new static mutation rows fail with `rc 0, expected 1`, except S2e (the old check catches it), and S0, C1, C2 pass.
- F0 on the unchanged Dockerfile: rc 143 (the watchdog). With the signal: rc 1 and `DOMException [TimeoutError]` from Node v24.14.0 in the runtime image.
- On the local `iam-console:dev` and `gateway-console:dev` images: R-NODE warns (24.14.0 against 24.16.0), R-CONFIG walks 1367 and 1324 files and finds nothing, R-HEALTH exits 0 in 0 s under bash 5 (the file redirect has no M7 wait).
- `shellcheck -S warning` reports nothing on the harness. The `repo:actionlint` check-13 regex finds no early-exit reader in either file.
- `git diff`-equivalent check: the six frozen functions are byte-identical.

## Self-review

- **Spec coverage.** § 4.1: S-DIRECTIVE, S-FROM, S-COPYFROM (Task 2), S-INSTALL (Task 3), S-PAIGASUS (Task 4), S-HCTIMEOUT (Task 5). § 4.2: R-NODE (Task 6), R-CONFIG (Task 7), R-HEALTH and `CONSOLE_HC_DEADLINE` (Task 8). § 4.3: Task 5. § 4.4: Task 9, and the PR 279 comment after the PR opens. § 4.5: Task 9. § 5.1–5.5: Tasks 1–8. § 5.6: Task 10. § 6 A1: every row; A2: Task 10 Step 3; A3: Task 10 Step 4; A4: Task 10 Steps 2 and 5 plus CI; A5: Task 9 and the controller step.
- **Placeholders.** None. `<scratchpad>` in the throwaway scripts is the implementer's own scratchpad directory. `<PR>` in the controller comment is the number that the open-pr stage gives.
- **Names.** `run_pins`, `pins_mut`, `mk_tree`, `append_df`, `sed_df`, `run_fn`, `stub_reset`, `pin_rows`, `hc_run` and the three row functions have the same names and arguments in every task. The row counts per task (2, 9, 15, 18, 23, 33, 42, 52) follow from the rows each task adds.
- **Review Focus.** Each of the five lines has its row in the owning task: S2e (Task 3), N6 (Task 6), S6c (Task 5), H5 (Task 8), S1g (Task 2).
