# SMA-685 release-plz version_group — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Record the causes of the two SMA-680 release-plz behaviours by measurement, make
`version-lockstep --write` stamp the Cargo `publish = false` binding crates, and correct every
comment that states the old claim.

**Architecture:** `ci/version-lockstep/run.sh` gets a `cargo-package` arm in `write_site` and a
new `stamp_sites` function (the per-site loop, split out of `run_write`). `stamp_sites` writes a
`cargo-package` site only when it is not the group head and its Cargo manifest says
`publish = false`. Two new self-test tables cover the writer (fixtures) and `stamp_sites` (a
staged copy of the real tree). The negative control gets a third drift. Measurements run in a
throwaway full clone.

**Tech Stack:** bash (4+, `declare -A`), python3 `tomllib` (3.11+), release-plz 0.3.158
(proto-pinned), cargo, uv, pnpm, Moon 2.5.3.

**Spec:** `docs/superpowers/specs/2026-09-25-sma-685-release-plz-version-group-design.md`

## Global Constraints

- Every source file keeps its SPDX header. Commits: conventional, with a workspace scope
  (`fix(ci): …`, `docs(repo): …`), each ending with
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Commit messages are written to a file in the scratchpad and passed with `git commit -F <file>`.
  The worktree sandbox refuses complex inline git commands.
- Never `git commit --amend`, never `git reset HEAD~1`, never `--no-verify`. One new commit per
  task. After each task, the controller checks `git log --oneline -3` and `git reflog -3`.
- Scratchpad: `S=/private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/363e7d6e-c3d7-47de-a594-7ce354d3ce38/scratchpad`.
  Worktree: `W=/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-685-release-plz-version-group`.
- Shell variables do NOT persist between tool calls. Set `S`, `W`, `C=$S/sma685/clone` and the
  PATH again at the start of every command.
- Scratch commits in the throwaway clone use `git -c commit.gpgsign=false commit … --no-verify`
  (no 1Password prompt, no commitlint in the clone). Never use either flag in the worktree.
- Tool PATH for every shell: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` and
  `export PROTO_REPORTER=text`.
- `ci/version-lockstep/run.sh` needs bash 4+. Run it with `/opt/homebrew/bin/bash`. Before any run
  of it, confirm the host pipe holds at least 8192 bytes (Task 1 step 1). If it holds less, stop
  and report: its heredocs hang under bash 5 on a 512-byte pipe (SMA-612). Never read a hang as a
  result.
- No new line in `ci/version-lockstep/run.sh` may contain a cargo command shape such as
  `cargo package`, `cargo update`, `cargo build` — not even in a string or comment. A8
  (`ci/affected-graph/cargo_moon_parity.py:369-370`) does not strip quotes.
- No new `*.sh` line may pipe into an early-exit reader (`| grep -q`, `| grep -m`, `| head`,
  `| awk … exit`). `repo:actionlint` check 13 reds it. Do not use a here-string (`<<<`) on data
  that can exceed 512 bytes.
- The three regeneration lines in `run_write` (`cargo update -w --offline …`, `cargo update -w
  >/dev/null )`, `die_infra "cargo update -w failed (site 16)"`) stay byte-identical and stay
  inside `run_write`. `ALLOW_UNLOCKED_CARGO_SCRIPT` keys on that text.
- No new file may name Moon's CI report file (the JSON report under `.moon/cache/`) by its file
  name. `repo:actionlint` check 12 reds it.
- All prose (comments, docs, the measurements file) is ASD-STE100 Simplified Technical English,
  with an evidence label (READ / MEASURED / INFERRED) on each claim.
- `write_site cargo-package` contract: args `<kind> <target> <head-version>`; prints `1` (changed)
  or `0` (no change) on stdout; rc 0 on success, rc 1 when the site is higher than the head (with
  a `FAIL:` line on stderr, file unchanged), rc 2 on any shape it refuses.

## Review Focus

1. A real manifest shape the fixtures do not show (a comment line between `[package]` and
   `version`; the kernel's long comment block) — expected: the writer edits only the version
   value. Pinned by the `stamp_sites_self_test` on the staged real tree (Task 3).
2. A multi-line string in `[package]` (for example a `description = """…"""` that contains a line
   starting with `[`) — expected: fail closed (rc 2) or a correct edit, never a wrong edit. Pinned
   by fixture F12 in Task 2 (the `tomllib` diff assertion catches any wrong edit).
3. A `version` key in `[package.metadata.x]` or `[dependencies.foo]` placed BEFORE `[package]`
   in the file — expected: untouched. Pinned by fixture F7 in Task 2.
4. The head at a lower version than a binding (a hand edit gone wrong) — expected: `--write`
   exits 1 and names the site; nothing is written. Pinned by fixture F10 (Task 2) and by
   `stamp_sites` propagating rc 1 (Task 3).
5. A manifest without a trailing newline — expected: a correct edit and no added newline. Pinned
   by fixture F13 in Task 2.

---

### Task 1: Measure B1 and B2 in a throwaway clone (M1, M1b, M1c, M2)

No repo code changes. The deliverable is the new measurements file.

**Files:**
- Create: `docs/superpowers/specs/2026-09-25-sma-685-measurements.md`

**Interfaces:**
- Consumes: nothing.
- Produces: `$S/sma685/clone` (a full clone) with two tags: `m1-scratch` (the M1 scratch
  commit) and `m1-result` (M1's result committed as a scratch commit). Task 4 reuses both; record
  their shas in the measurements file.

- [ ] **Step 1: Record host preconditions**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
/opt/homebrew/bin/bash --version | sed -n 1p
python3 - <<'PY'
import os, fcntl
r, w = os.pipe()
fcntl.fcntl(w, fcntl.F_SETFL, os.O_NONBLOCK)
n = 0
try:
    while True:
        n += os.write(w, b"x" * 64)
except BlockingIOError:
    pass
print("pipe capacity", n)
PY
release-plz --version
```
Expected: bash 5.x; `pipe capacity` ≥ 8192; `release-plz 0.3.158`. If the capacity is below
8192, write that in the measurements file and STOP the whole plan (report BLOCKED).

- [ ] **Step 2: Make the full clone and record the baseline**

```bash
C=$S/sma685/clone
mkdir -p $S/sma685 && git clone -q /Users/smaschek/dev/paigasus/paigasus-core "$C"
cd "$C" && git switch -q -c m1 cb772393
git rev-parse --abbrev-ref --symbolic-full-name '@{upstream}' ; echo "upstream-rc=$?"
git rev-parse HEAD origin/main main
grep -n '^version' rs/crates/libs/paigasus-kernel/Cargo.toml rs/crates/bindings/*/Cargo.toml rs/crates/libs/paigasus-proto*/Cargo.toml
git log --oneline paigasus-kernel-v0.1.0..HEAD -- rs/crates/libs/paigasus-kernel
git log --oneline paigasus-proto-v0.2.0..HEAD -- rs/crates/libs/paigasus-proto
```
Expected: `upstream-rc` non-zero (no upstream); all three shas `cb772393…`; kernel and bindings
`0.1.0`, proto `0.2.0`; the kernel log is empty; the proto log lists `1fd87a15`. Never use
`git worktree add` for this clone: release-plz would move the worktree's HEAD
(`copy_dir.rs:50-93`).

- [ ] **Step 3: M1 — the B1 symptom**

```bash
cd "$C"
printf '\n' >> rs/crates/libs/paigasus-kernel/README.md
git -c commit.gpgsign=false commit -q -am 'fix(rs): scratch readme edit for the sma-685 m1 measurement' --no-verify
git tag m1-scratch && git rev-parse m1-scratch
git status --short
(cd rs && RUST_LOG=release_plz_core=debug,git_cmd=trace GIT_TRACE=$S/sma685/m1.gittrace \
  release-plz update >$S/sma685/m1.log 2>&1); echo "rc=$?"
grep -n '^version' rs/crates/libs/paigasus-kernel/Cargo.toml rs/crates/bindings/*/Cargo.toml rs/crates/libs/paigasus-proto*/Cargo.toml
grep -n 'no upstream configured\|already up to date\|next version' $S/sma685/m1.log
grep -n 'checkout' $S/sma685/m1.gittrace
```
Expected: rc 0; kernel `0.1.1`; the three bindings `0.1.0`; both proto crates `0.3.0`; a
`no upstream configured` warning; the git trace shows `checkout m1`. Record the verbatim lines.
Then keep the result as a scratch commit for Task 4:
```bash
git add -A && git -c commit.gpgsign=false commit -q -m 'chore: scratch m1 result' --no-verify
git tag m1-result && git rev-parse m1-result
cp rs/Cargo.lock $S/sma685/lock-m1-result
git show cb772393:rs/Cargo.lock > $S/sma685/lock-base
```

- [ ] **Step 4: M1b — the B1 cause (control)**

```bash
cd "$C"
curl -s -A 'sma-685-probe' -o /dev/null -w '%{http_code}\n' https://crates.io/api/v1/crates/paigasus-wasm
```
Expected: `404`. If it is `200`, repeat with `paigasus-node-bindings` and use the first name
that gives 404; write the choice down.
```bash
git reset -q --hard m1-scratch && git clean -fdq -e target -e node_modules && git status --short
sed -i '' '/^publish = false$/d' rs/crates/bindings/paigasus-wasm/Cargo.toml
git diff --stat
git -c commit.gpgsign=false commit -q -am 'chore: scratch m1b make paigasus-wasm cargo-publishable' --no-verify
(cd rs && RUST_LOG=release_plz_core=debug,git_cmd=trace release-plz update >$S/sma685/m1b.log 2>&1); echo "rc=$?"
grep -n '^version' rs/crates/libs/paigasus-kernel/Cargo.toml rs/crates/bindings/*/Cargo.toml
grep -n 'paigasus-wasm' $S/sma685/m1b.log
```
Expected (READ: `version.rs:14-15`, `updater.rs:814-820`): `paigasus-wasm` 0.1.1, the kernel
0.1.1, the other two bindings 0.1.0. If `git diff --stat` shows no change, the manifest has no
exact `publish = false` line: stop and report. If `paigasus-wasm` stays at 0.1.0, write "B1
cause: OPEN" and do not guess.

- [ ] **Step 5: M1c — the SMA-407 fixture cause (`git_only`)**

```bash
cd "$C"
git reset -q --hard m1-scratch && git clean -fdq -e target -e node_modules && git status --short
python3 - rs/release-plz.toml <<'PY'
import sys
p = sys.argv[1]; s = open(p).read()
old = '[[package]]\nname = "paigasus-wasm"\n'
assert s.count(old) == 1, "paigasus-wasm entry not found once"
s = s.replace(old, old + 'git_only = true\n')
open(p, "w").write(s)
PY
git -c commit.gpgsign=false commit -q -am 'chore: scratch m1c git_only on paigasus-wasm' --no-verify
(cd rs && RUST_LOG=release_plz_core=debug,git_cmd=trace release-plz update >$S/sma685/m1c.log 2>&1); echo "rc=$?"
grep -n '^version' rs/crates/libs/paigasus-kernel/Cargo.toml rs/crates/bindings/*/Cargo.toml
grep -n -i 'paigasus-wasm\|error' $S/sma685/m1c.log
```
Expected: either `paigasus-wasm` 0.1.1 (the §2.1 inference holds), or an error. Record an error
verbatim; a `cargo package` error also confirms the spec's first rejected alternative. Record
which case happened.

- [ ] **Step 6: M2 — the B2 cause**

```bash
cd "$C"
git reset -q --hard m1-scratch && git clean -fdq -e target -e node_modules && git status --short
git switch -q -c m2 m1-scratch
git rev-parse --abbrev-ref --symbolic-full-name '@{upstream}' ; echo "upstream-rc=$?"
(cd rs && RUST_LOG=release_plz_core=debug,git_cmd=trace GIT_TRACE=$S/sma685/m2a.gittrace \
  release-plz update >$S/sma685/m2a.log 2>&1); echo "run A rc=$?"
grep -n '^version' rs/crates/libs/paigasus-kernel/Cargo.toml
grep -n 'no upstream configured\|paigasus-kernel' $S/sma685/m2a.log
grep -n 'checkout' $S/sma685/m2a.gittrace
git reset -q --hard m1-scratch && git clean -fdq -e target -e node_modules && git status --short
git rev-parse main origin/main
git branch -u origin/main
git rev-parse --abbrev-ref --symbolic-full-name '@{upstream}'
(cd rs && RUST_LOG=release_plz_core=debug,git_cmd=trace GIT_TRACE=$S/sma685/m2b.gittrace \
  release-plz update >$S/sma685/m2b.log 2>&1); echo "run B rc=$?"
grep -n '^version' rs/crates/libs/paigasus-kernel/Cargo.toml
grep -n 'already up to date\|next version calculated\|diff: Diff' $S/sma685/m2b.log
grep -n 'checkout' $S/sma685/m2b.gittrace
git branch --unset-upstream
```
Expected: run A — kernel 0.1.1, the `no upstream configured` warning, checkout `m2`. Before run
B, `main` and `origin/main` are both `cb772393…`; the upstream reads `origin/main`. Run B —
`paigasus-kernel: already up to date`, `commits: []`, a checkout of `main`. If run B bumps the
kernel, write "B2 cause: OPEN" and stop M2; do not guess.

- [ ] **Step 7: Write the measurements file**

Create `docs/superpowers/specs/2026-09-25-sma-685-measurements.md` with the sections
`## Host`, `## Baseline`, `## M1`, `## M1b`, `## M1c`, `## M2` and a placeholder heading
`## M3 and mutations` that holds the single line `Recorded by Task 4.` Each section states:
the command (as run), the verbatim output lines, the result table where versions are compared,
and a one-line verdict with an evidence label. Record the shas of the tags `m1-scratch` and `m1-result`. Use STE.
Do not name Moon's CI report file.

- [ ] **Step 8: Commit**

```bash
cd $W && git add docs/superpowers/specs/2026-09-25-sma-685-measurements.md
git commit -F $S/msg-task1.txt
```
with `$S/msg-task1.txt`:
```
docs(repo): measure the SMA-685 release-plz behaviours

Record M1, M1b, M1c and M2 from a throwaway clone at cb772393.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
```

---

### Task 2: The `cargo-package` writer arm, with a fixture self-test

**Files:**
- Modify: `ci/version-lockstep/run.sh` (`write_site`, around `:431-565`; add
  `cargo_package_writer_self_test` after `lock_reader_self_test`, around `:286`;
  `run_self_tests`, around `:288`; `SELF_TEST_COUNT`, `:80`)

**Interfaces:**
- Consumes: `write_site`, `fail`, `SELF_TESTS_RAN`, `SELF_TEST_COUNT`, `REPO_ROOT` override.
- Produces: `write_site cargo-package <target> <head-version>` per the contract in Global
  Constraints; `cargo_package_writer_self_test` (a self-test table).

- [ ] **Step 1: Write the failing self-test**

Add this function after `lock_reader_self_test` (it uses `REPO_ROOT="$tmp"` so `write_site`
resolves fixture paths inside the temp dir):

```bash
# SMA-685: fixture table for the cargo-package writer. The fixtures deliberately vary the
# layout (spacing, comments, table order, CRLF, no trailing newline), because a writer tested
# on one layout only passes on the layout its author assumed.
cargo_package_writer_self_test() {
  local tmp rc got before after
  tmp="$(mktemp -d)" || die_infra "cannot create a scratch dir"
  # shellcheck disable=SC2064
  trap "rm -rf '$tmp'" RETURN

  _cpw() { # $1 fixture file (relative to $tmp)  $2 head version -> sets rc and got
    rc=0
    got="$(REPO_ROOT="$tmp" write_site cargo-package "$1" "$2" 2>/dev/null)" || rc=$?
  }
  _cpw_expect() { # $1 fixture  $2 expected file content (printf format)
    local want
    want="$(printf "$2")"
    [ "$(cat "$tmp/$1")" = "$want" ] \
      || { fail "self-test: cargo-package writer produced the wrong text for $1"; return 1; }
  }

  # F1: plain layout; rust-version, a dependency version and an inline table stay untouched.
  printf '[package]\nname = "a"\nversion = "0.1.0"\nrust-version = "1.95"\n\n[dependencies]\nfoo = { version = "0.1.0" }\n\n[dependencies.bar]\nversion = "0.1.0"\n' >"$tmp/f1.toml"
  _cpw f1.toml 0.2.0
  [ "$rc" -eq 0 ] && [ "$got" = 1 ] || { fail "self-test: F1 rc=$rc got='$got', expected rc 0 and 1"; return 1; }
  _cpw_expect f1.toml '[package]\nname = "a"\nversion = "0.2.0"\nrust-version = "1.95"\n\n[dependencies]\nfoo = { version = "0.1.0" }\n\n[dependencies.bar]\nversion = "0.1.0"' || return 1

  # F2: idempotent — same version prints 0, changes no byte and keeps the mtime.
  touch -t 200001010000 "$tmp/f1.toml"
  before="$(python3 -c 'import os,sys;print(os.stat(sys.argv[1]).st_mtime_ns)' "$tmp/f1.toml")"
  _cpw f1.toml 0.2.0
  after="$(python3 -c 'import os,sys;print(os.stat(sys.argv[1]).st_mtime_ns)' "$tmp/f1.toml")"
  [ "$rc" -eq 0 ] && [ "$got" = 0 ] && [ "$before" = "$after" ] \
    || { fail "self-test: F2 idempotence rc=$rc got='$got' mtime $before -> $after"; return 1; }

  # F3: spacing variants and a trailing comment on the version line.
  printf '[package]\nname="b"\nversion   =  "0.1.0"   # the floor\n' >"$tmp/f3.toml"
  _cpw f3.toml 0.1.1
  [ "$rc" -eq 0 ] && [ "$got" = 1 ] || { fail "self-test: F3 rc=$rc got='$got'"; return 1; }
  _cpw_expect f3.toml '[package]\nname="b"\nversion   =  "0.1.1"   # the floor' || return 1

  # F4: a commented header and comment lines between [package] and version.
  printf '# top\n[package] # the crate\nname = "c"\n# The floor. version = "9.9.9" in a comment.\n# more\nversion = "0.1.0"\n' >"$tmp/f4.toml"
  _cpw f4.toml 0.3.0
  [ "$rc" -eq 0 ] || { fail "self-test: F4 rc=$rc"; return 1; }
  _cpw_expect f4.toml '# top\n[package] # the crate\nname = "c"\n# The floor. version = "9.9.9" in a comment.\n# more\nversion = "0.3.0"' || return 1

  # F5: CRLF line endings are kept.
  printf '[package]\r\nname = "d"\r\nversion = "0.1.0"\r\n' >"$tmp/f5.toml"
  _cpw f5.toml 0.2.0
  [ "$rc" -eq 0 ] || { fail "self-test: F5 rc=$rc"; return 1; }
  [ "$(od -An -c "$tmp/f5.toml" | tr -d ' \n')" = "$(printf '[package]\r\nname = "d"\r\nversion = "0.2.0"\r\n' | od -An -c | tr -d ' \n')" ] \
    || { fail "self-test: F5 CRLF was not kept"; return 1; }

  # F6: [package] is not the first table, and [package.metadata.x] has its own version key.
  printf '[lib]\npath = "src/lib.rs"\n\n[package]\nname = "e"\nversion = "0.1.0"\n\n[package.metadata.x]\nversion = "7.7.7"\n' >"$tmp/f6.toml"
  _cpw f6.toml 0.2.0
  [ "$rc" -eq 0 ] || { fail "self-test: F6 rc=$rc"; return 1; }
  _cpw_expect f6.toml '[lib]\npath = "src/lib.rs"\n\n[package]\nname = "e"\nversion = "0.2.0"\n\n[package.metadata.x]\nversion = "7.7.7"' || return 1

  # F7: a version key in a table BEFORE [package] stays untouched.
  printf '[dependencies.foo]\nversion = "0.1.0"\n\n[package]\nname = "f"\nversion = "0.1.0"\n' >"$tmp/f7.toml"
  _cpw f7.toml 0.2.0
  [ "$rc" -eq 0 ] || { fail "self-test: F7 rc=$rc"; return 1; }
  _cpw_expect f7.toml '[dependencies.foo]\nversion = "0.1.0"\n\n[package]\nname = "f"\nversion = "0.2.0"' || return 1

  # F8-F9, F11: shapes the writer must refuse with rc 2, leaving the file unchanged.
  local f
  printf '[package]\nname = "g"\nversion.workspace = true\n' >"$tmp/f8a.toml"
  printf '[lib]\npath = "x"\n' >"$tmp/f8b.toml"
  printf '[package]\nname = "g"\n' >"$tmp/f8c.toml"
  printf '[package]\nname = "g"\nversion = "0.1.0"\nversion = "0.1.0"\n' >"$tmp/f9.toml"
  printf '[package]\nname = "g"\nversion = "0.1.0-rc.1"\n' >"$tmp/f11.toml"
  for f in f8a f8b f8c f9 f11; do
    before="$(cat "$tmp/$f.toml")"
    _cpw "$f.toml" 0.2.0
    [ "$rc" -eq 2 ] || { fail "self-test: $f must be refused with rc 2, got rc=$rc"; return 1; }
    [ "$(cat "$tmp/$f.toml")" = "$before" ] || { fail "self-test: $f was changed although refused"; return 1; }
  done

  # F10: refuses to lower a version (rc 1), file unchanged.
  printf '[package]\nname = "h"\nversion = "0.2.0"\n' >"$tmp/f10.toml"
  _cpw f10.toml 0.1.0
  [ "$rc" -eq 1 ] || { fail "self-test: F10 lowering must be rc 1, got rc=$rc"; return 1; }
  _cpw_expect f10.toml '[package]\nname = "h"\nversion = "0.2.0"' || return 1

  # F12: a multi-line description with a line that starts with "[" — never a wrong edit.
  printf '[package]\nname = "i"\ndescription = """\n[not a table]\nversion = "5.5.5"\n"""\nversion = "0.1.0"\n' >"$tmp/f12.toml"
  _cpw f12.toml 0.2.0
  if [ "$rc" -eq 0 ]; then
    _cpw_expect f12.toml '[package]\nname = "i"\ndescription = """\n[not a table]\nversion = "5.5.5"\n"""\nversion = "0.2.0"' || return 1
  else
    [ "$rc" -eq 2 ] || { fail "self-test: F12 must be a correct edit or rc 2, got rc=$rc"; return 1; }
  fi

  # F13: no trailing newline — none is added.
  printf '[package]\nname = "j"\nversion = "0.1.0"' >"$tmp/f13.toml"
  _cpw f13.toml 0.2.0
  [ "$rc" -eq 0 ] || { fail "self-test: F13 rc=$rc"; return 1; }
  [ "$(od -An -c "$tmp/f13.toml" | tr -d ' \n')" = "$(printf '[package]\nname = "j"\nversion = "0.2.0"' | od -An -c | tr -d ' \n')" ] \
    || { fail "self-test: F13 changed the file end"; return 1; }

  SELF_TESTS_RAN=$((SELF_TESTS_RAN + 1))
}
```

Note: `$(cat …)` strips trailing newlines, which is why `_cpw_expect` strings omit the final
`\n`; F5 and F13 compare bytes with `od` for that reason. `od … | tr` is a full reader, not an
early-exit one, so check 13 allows it.

Add the call to `run_self_tests` after `lock_reader_self_test`, and change `:80` to
`SELF_TEST_COUNT=3   # site_verdict, lock_reader, cargo_package_writer`.

- [ ] **Step 2: Run the self-test to see it fail**

Run: `cd $W && /opt/homebrew/bin/bash ci/version-lockstep/run.sh --self-test; echo rc=$?`
Expected: rc 1 with `FAIL: self-test: F1 rc=0 got='0', expected rc 0 and 1`. (`write_site`'s
catch-all arm `*) printf '0' ;;` answers `0` for the unknown kind.)

- [ ] **Step 3: Implement the writer arm**

In `write_site`, add this arm BEFORE the final `*) printf '0' ;;` arm:

```bash
    cargo-package)
      # SMA-685: the [package] version of a Cargo manifest, edited in place (no TOML
      # round-trip, so no unrelated churn). release-plz 0.3.158 never writes the version of a
      # crate whose Cargo manifest says `publish = false`, version_group or not (READ,
      # updater.rs:283-302), so --write stamps those. stamp_sites decides WHICH sites; this arm
      # only edits one file. It fails closed (rc 2) on any shape it does not understand, and it
      # refuses to lower a version (rc 1).
      python3 - "$abs" "$version" <<'PY'
import re, sys, tomllib

def fatal(msg):
    print(f"FATAL: {msg}", file=sys.stderr); raise SystemExit(2)

def plain(v, what):
    m = re.fullmatch(r"(\d+)\.(\d+)\.(\d+)", v)
    if m is None:
        fatal(f"{what} version '{v}' is not plain X.Y.Z")
    return tuple(int(x) for x in m.groups())

p, v = sys.argv[1], sys.argv[2]
head = plain(v, "head")
raw = open(p, "rb").read()
try:
    s = raw.decode("utf-8")
    old_doc = tomllib.loads(s)
except Exception as e:
    fatal(f"malformed {p}: {e}")
hdrs = list(re.finditer(r"(?m)^[ \t]*\[package\][ \t]*(?:#[^\r\n]*)?\r?$", s))
if len(hdrs) != 1:
    fatal(f"{p}: expected one [package] table header, found {len(hdrs)}")
start = hdrs[0].end()
nxt = re.search(r"(?m)^[ \t]*\[", s[start:])
end = start + nxt.start() if nxt else len(s)
body = s[start:end]
keys = list(re.finditer(r"(?m)^[ \t]*version[ \t]*[=.]", body))
if len(keys) != 1:
    fatal(f"{p}: expected one version key in [package], found {len(keys)}")
vm = re.search(r'(?m)^([ \t]*version[ \t]*=[ \t]*")([^"\r\n]*)(")', body)
if vm is None:
    fatal(f"{p}: the [package] version is not a literal string")
cur = plain(vm.group(2), "site")
if cur > head:
    print(f"FAIL: {p} is at {vm.group(2)}, higher than the head {v}; not lowered", file=sys.stderr)
    raise SystemExit(1)
if cur == head:
    print(0); raise SystemExit(0)
a, b = start + vm.start(2), start + vm.end(2)
new = s[:a] + v + s[b:]
try:
    new_doc = tomllib.loads(new)
except Exception as e:
    fatal(f"{p}: the edit made invalid TOML: {e}")
old_doc.setdefault("package", {})["version"] = v
if new_doc != old_doc:
    fatal(f"{p}: the edit changed more than package.version")
open(p, "wb").write(new.encode("utf-8"))
print(1)
PY
      ;;
```

Then update the comment on the catch-all arm to: `*) printf '0' ;;   # regeneration-owned
kinds (cargo-wsdep, the locks, the napi glue) are not written here`.

Note: `open(p, "rb")` and `.encode` keep CRLF and a missing final newline, because the text is
never translated. `tomllib.loads` accepts `\r\n`. F12 is caught by the header count or the
`tomllib` diff check.

- [ ] **Step 4: Run the self-test to see it pass**

Run: `cd $W && /opt/homebrew/bin/bash ci/version-lockstep/run.sh --self-test; echo rc=$?`
Expected: `== version-lockstep self-tests passed (3 tables) ==`, rc 0.
Also run: `/opt/homebrew/bin/bash ci/version-lockstep/run.sh; echo rc=$?` — expected
`== all 20 version-lockstep sites agree ==`, rc 0. And
`/opt/homebrew/bin/bash ci/version-lockstep/run.sh --negative-control; echo rc=$?` — rc 0.

- [ ] **Step 5: Prove the test bites (mutation, not committed)**

Insert this line as the FIRST arm inside `write_site`'s `case "$kind" in`:
`    cargo-package) printf '0' ;;  # MUTATION-SMA-685`
Run `--self-test`: expected rc 1 (`F1 … got='0'`). Then delete exactly that marked line (use the
Edit tool; never `git checkout --`). Run `--self-test` again: expected rc 0. Confirm with
`git diff | grep -c MUTATION-SMA-685` → `0`.

- [ ] **Step 6: Commit**

```bash
cd $W && git add ci/version-lockstep/run.sh && git commit -F $S/msg-task2.txt
```
with:
```
fix(ci): let version-lockstep write a Cargo [package] version

Add a cargo-package arm to write_site. It edits only the [package]
version value in place, fails closed on unknown shapes and refuses to
lower a version. A new fixture self-test covers the layouts.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
```

---

### Task 3: `stamp_sites` — stamp the `publish = false` non-head crates, with a real-tree self-test

**Files:**
- Modify: `ci/version-lockstep/run.sh` (`run_write`, around `:566-600`; new `stamp_sites` and
  `cargo_publish_false` above it; new `stamp_sites_self_test` after
  `cargo_package_writer_self_test`; `run_self_tests`; `SELF_TEST_COUNT`; `negative_control`)

**Interfaces:**
- Consumes: `write_site cargo-package` (Task 2), `read_version`, `stage_pristine_tree`,
  `run_check`, `SITES`, `SOURCE_OF_TRUTH`.
- Produces: `stamp_sites` — reads `$REPO_ROOT`, writes every site `--write` owns, prints the
  count of changed sites on stdout, returns 0 / 1 (a site is higher than its head) / 2 (infra).
  `cargo_publish_false <target>` — prints `1` if the manifest's Cargo `publish` is `false` or
  `[]`, else `0`; rc 2 on a malformed file.

- [ ] **Step 1: Write the failing self-test**

Add after `cargo_package_writer_self_test`:

```bash
# SMA-685: run the PRODUCTION loop (stamp_sites) on a staged copy of the real tree. The fixture
# table above tests write_site alone and cannot see a kind dropped from stamp_sites' filter; this
# table can, and it runs the writer on the real manifest shapes.
stamp_sites_self_test() {
  local tmp rc=0 entry group kind target got derive_before derive_after
  tmp="$(mktemp -d)" || die_infra "cannot create a scratch dir"
  # shellcheck disable=SC2064
  trap "rm -rf '$tmp'" RETURN
  stage_pristine_tree "$tmp"
  derive_before="$(REPO_ROOT="$tmp" read_version cargo-package rs/crates/libs/paigasus-proto-derive/Cargo.toml)" || return 2
  # Move the kernel head to a sentinel, independently of the writer under test.
  python3 - "$tmp/rs/crates/libs/paigasus-kernel/Cargo.toml" <<'PY'
import re, sys
p = sys.argv[1]; s = open(p, encoding="utf-8").read()
s, n = re.subn(r'(?m)^version = "[^"]*"$', 'version = "9.9.9"', s, count=1)
assert n == 1, "no kernel version line"
open(p, "w", encoding="utf-8").write(s)
PY
  REPO_ROOT="$tmp" stamp_sites >/dev/null || rc=$?
  [ "$rc" -eq 0 ] || { fail "self-test: stamp_sites on the staged tree returned $rc"; return 1; }
  for entry in "${SITES[@]}"; do
    IFS='|' read -r group kind target <<<"$entry"
    [ "$group" = kernel ] || continue
    case "$kind" in cargo-package|pyproject|pyproject-dep|packagejson) ;; *) continue ;; esac
    got="$(REPO_ROOT="$tmp" read_version "$kind" "$target" "$group")" || return 2
    [ "$got" = "9.9.9" ] || { fail "self-test: stamp_sites left $kind $target at '$got', expected 9.9.9"; return 1; }
  done
  derive_after="$(REPO_ROOT="$tmp" read_version cargo-package rs/crates/libs/paigasus-proto-derive/Cargo.toml)" || return 2
  [ "$derive_before" = "$derive_after" ] \
    || { fail "self-test: stamp_sites touched the publishable paigasus-proto-derive ($derive_before -> $derive_after)"; return 1; }
  SELF_TESTS_RAN=$((SELF_TESTS_RAN + 1))
}
```

Add the call to `run_self_tests`. Change `SELF_TEST_COUNT` to
`4   # site_verdict, lock_reader, cargo_package_writer, stamp_sites`. Also add a definitions
anchor in `run_self_tests`, before the first call, so a table that is defined but not called reds:

```bash
  local defs
  defs="$(grep -cE '^[a-z_]+_self_test\(\) \{$' "${BASH_SOURCE[0]}")" || die_infra "cannot count self-test definitions"
  [ "$defs" -eq "$SELF_TEST_COUNT" ] \
    || die_infra "found $defs *_self_test definitions, expected $SELF_TEST_COUNT"
```

- [ ] **Step 2: Run it to see it fail**

Run: `cd $W && /opt/homebrew/bin/bash ci/version-lockstep/run.sh --self-test; echo rc=$?`
Expected: rc 127 or a failure naming `stamp_sites` (the function does not exist yet). Record the
actual output.

- [ ] **Step 3: Implement `cargo_publish_false`, `stamp_sites`, and the new `run_write` head**

Add above `run_write`:

```bash
cargo_publish_false() { # $1 target -> prints 1 if Cargo `publish` is false or [], else 0
  local abs="$REPO_ROOT/$1"
  [ -r "$abs" ] || die_infra "cannot read $1"
  python3 - "$abs" <<'PY'
import sys, tomllib
p = sys.argv[1]
try:
    pub = tomllib.load(open(p, "rb"))["package"].get("publish", True)
except Exception as e:
    print(f"malformed {p}: {e}", file=sys.stderr); sys.exit(2)
print(1 if pub is False or pub == [] else 0)
PY
}

# SMA-685: the per-site loop of --write, split out of run_write so the self-test runs the SAME
# loop on a staged tree. It writes the pyproject, pyproject-dep and packagejson sites, and the
# cargo-package sites that are NOT the group head and whose Cargo manifest says
# `publish = false`. release-plz 0.3.158 never writes those (READ, updater.rs:283-302). A
# publishable non-head (paigasus-proto-derive) is left to release-plz, so --check still sees a
# version_group fault there. Prints the count of changed sites.
stamp_sites() {
  local wrote=0 group kind target head expected changed pf rc
  for entry in "${SITES[@]}"; do
    IFS='|' read -r group kind target <<<"$entry"
    head="${SOURCE_OF_TRUTH[$group]}"
    case "$kind" in
      pyproject|pyproject-dep|packagejson) ;;
      cargo-package)
        [ "$target" != "$head" ] || continue
        pf="$(cargo_publish_false "$target")" || return 2
        [ "$pf" = 1 ] || continue
        ;;
      *) continue ;;
    esac
    # Explicit status handling rather than errexit: stamp_sites may be called on the left of
    # `||`, which suspends errexit (same discipline as run_check, SMA-576).
    expected="$(read_version cargo-package "$head")" || return 2
    rc=0
    changed="$(write_site "$kind" "$target" "$expected")" || rc=$?
    [ "$rc" -eq 0 ] || return "$rc"
    wrote=$((wrote + changed))
  done
  printf '%d\n' "$wrote"
}
```

Replace the head of `run_write` (its `local` line and its `for … done` loop, up to but NOT
including the `# Regenerate the three derived files` comment) with:

```bash
run_write() {
  local wrote rc=0
  wrote="$(stamp_sites)" || rc=$?
  [ "$rc" -eq 0 ] || return "$rc"
```

Leave everything from `# Regenerate the three derived files` to the end of `run_write`
byte-identical. Update that comment's first line only if it says "six"; it must not gain any
cargo command text.

- [ ] **Step 4: Add the third negative-control drift**

In `negative_control`, add `tmp3` next to `tmp1`/`tmp2` (declare it, `mktemp -d` it, add it to
the `trap`). Before the final `printf '== negative control: … both …'` line, add:

```bash
  stage_pristine_tree "$tmp3"

  # Third drift (SMA-685): a cargo-package binding manifest. README L1 recorded that this kind had
  # no end-to-end drift, and a release-plz bump of the kernel alone produces exactly this shape.
  python3 - "$tmp3/rs/crates/bindings/paigasus-wasm/Cargo.toml" <<'PY'
import re, sys
p = sys.argv[1]; s = open(p, encoding="utf-8").read()
s, n = re.subn(r'(?m)^version = "[^"]*"$', 'version = "99.99.99"', s, count=1)
assert n == 1, "no version line"
open(p, "w", encoding="utf-8").write(s)
PY

  local ec3=0
  REPO_ROOT="$tmp3" run_check >/dev/null 2>&1 || ec3=$?
  if [ "$ec3" -eq 2 ]; then
    fail "negative control: run_check hit an infrastructure failure (exit 2) instead of
      reporting the cargo-package drift."
    return 1
  fi
  if [ "$ec3" -ne 1 ]; then
    fail "negative control: a drifted binding manifest was ACCEPTED (run_check exited $ec3, expected 1)."
    return 1
  fi
```
and change the final line to
`printf '== negative control: version-lockstep reported red on a packagejson, a lock and a cargo-package drift ==\n'`.

- [ ] **Step 5: Run everything**

```bash
cd $W
/opt/homebrew/bin/bash ci/version-lockstep/run.sh --self-test; echo rc=$?
/opt/homebrew/bin/bash ci/version-lockstep/run.sh --negative-control; echo rc=$?
/opt/homebrew/bin/bash ci/version-lockstep/run.sh; echo rc=$?
```
Expected: `self-tests passed (4 tables)` rc 0; the new three-drift line rc 0; `all 20 … agree`
rc 0.

- [ ] **Step 6: Prove the production call site is covered (mutation, not committed)**

In `stamp_sites`, insert this marked line directly after its `    case "$kind" in` line:
`      cargo-package) continue ;;  # MUTATION-SMA-685`.
Run `--self-test`: expected rc 1 with `stamp_sites left cargo-package rs/crates/bindings/…`.
Delete exactly the marked line with the Edit tool. Run `--self-test`: rc 0. Confirm
`git diff | grep -c MUTATION-SMA-685` → `0`.

- [ ] **Step 7: Run the A8 and actionlint scans that read this file**

```bash
cd $W && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
moon run repo:affected-smoke --force 2>&1 | tail -20
```
Expected: pass. (`repo:affected-smoke` needs system bash 3.2 for its here-strings; if Moon
resolves Homebrew bash and hangs, follow `affected-smoke-hang-fixed-by-system-bash` in memory:
use a bash-only shim dir, do not prepend `/bin`.) The actionlint check 13 run happens in Task 6.

- [ ] **Step 8: Commit**

```bash
cd $W && git add ci/version-lockstep/run.sh && git commit -F $S/msg-task3.txt
```
with:
```
fix(ci): stamp the publish = false kernel bindings in version-lockstep --write

release-plz 0.3.158 never writes the version of a Cargo publish = false
crate, so a kernel-only release PR failed repo:version-lockstep. Split
the stamp loop into stamp_sites, stamp the non-head publish = false
cargo-package sites, and cover the loop and a third drift in the tests.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
```

---

### Task 4: M3 and the mutations in the throwaway clone

**Files:**
- Modify: `docs/superpowers/specs/2026-09-25-sma-685-measurements.md` (replace the
  `Recorded by Task 4.` line)

**Interfaces:**
- Consumes: `$S/sma685/clone`, the tag `m1-result`, `$S/sma685/lock-m1-result`,
  `$S/sma685/lock-base` (Task 1); the new `run.sh` (Task 3, at the worktree HEAD).
- Produces: the M3 section.

- [ ] **Step 1: Provision the clone** (M3 runs `cargo update -w`, `uv lock`, `napi build` and a
  Moon test)

```bash
C=$S/sma685/clone; cd "$C"
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
git reset -q --hard m1-result && git clean -fdq -e target -e node_modules && git status --short
proto install >/dev/null 2>&1; echo proto=$?
pnpm -C ts install --frozen-lockfile >/dev/null 2>&1; echo pnpm=$?
(cd py && uv sync >/dev/null 2>&1); echo uv=$?
```
Expected: all `0`. `git status --short` empty after the reset. If `pnpm` or `uv` changed tracked
files, record it and `git reset -q --hard m1-result` again.

- [ ] **Step 2: M3.1 — the old `--write`, then the old `--check`** (the `run.sh` at
  `cb772393`, which is what the clone has)

```bash
cd "$C"
/opt/homebrew/bin/bash ci/version-lockstep/run.sh --write; echo write-rc=$?
/opt/homebrew/bin/bash ci/version-lockstep/run.sh 2>&1 | tee $S/sma685/m3-old-check.txt; echo "check-rc=${PIPESTATUS[0]}"
grep -c '^FAIL:' $S/sma685/m3-old-check.txt
```
Expected: write rc 0; check rc 1; exactly 4 `FAIL:` rows — the three binding `cargo-package`
rows and the kernel `cargo-lock` row. Record them verbatim. If the count differs, record the
real rows and mark the gap claim as corrected, not confirmed.

- [ ] **Step 3: M3.2 — the new `--write`, then the new `--check`**

```bash
cd "$C"
git reset -q --hard m1-result && git clean -fdq -e target -e node_modules && git status --short
cp $W/ci/version-lockstep/run.sh ci/version-lockstep/run.sh
/opt/homebrew/bin/bash ci/version-lockstep/run.sh --write; echo write-rc=$?
/opt/homebrew/bin/bash ci/version-lockstep/run.sh; echo check-rc=$?
grep -n '^version' rs/crates/libs/paigasus-kernel/Cargo.toml rs/crates/bindings/*/Cargo.toml rs/crates/libs/paigasus-proto*/Cargo.toml
git diff --stat
```
Expected: write rc 0 with `wrote N site(s)`; check rc 0, `all 20 … agree`; kernel and the three
bindings 0.1.1; both proto crates 0.3.0.

- [ ] **Step 4: M3.3 — lockdiff**

```bash
cat > $S/sma685/lockdiff.py <<'PY'
import sys, tomllib
def load(p):
    d = {}
    for pk in tomllib.load(open(p, "rb")).get("package", []):
        d.setdefault(pk["name"], []).append((pk["version"], "source" in pk))
    return d
a, b = load(sys.argv[1]), load(sys.argv[2])
for n in sorted(set(a) | set(b)):
    if a.get(n) != b.get(n):
        kind = "third-party" if any(s for _, s in (a.get(n) or b.get(n))) else "workspace"
        print(kind, n, sorted(v for v, _ in a.get(n, [])), "->", sorted(v for v, _ in b.get(n, [])), sep="\t")
PY
python3 $S/sma685/lockdiff.py $S/sma685/lock-m1-result "$C/rs/Cargo.lock"
python3 $S/sma685/lockdiff.py $S/sma685/lock-base "$C/rs/Cargo.lock"
```
Expected: first diff — the three binding workspace entries only, zero third-party lines. (The
spec §4 M3.3 says four; M1's own `cargo update --workspace` already moved the kernel entry, so
three is correct against `m1-result`. Record the real number.) Second diff, against
`cb772393` — four kernel-family and two proto workspace entries, zero third-party lines. Record
both verbatim.

- [ ] **Step 5: M3.4 — the wasm/napi test**

```bash
cd "$C" && moon run paigasus-kernel-ts:test --force 2>&1 | tail -8
```
Expected: pass (vitest summary `Test Files … passed`). Record the summary lines.

- [ ] **Step 6: Mutations against M3.2**

For each mutation: reset the clone to the tag `m1-result`, copy in the new `run.sh`, insert the
marked line with python (`assert` it matches exactly once), run `--write` then `--check`, record
the `--check` rc, then run `--self-test` and record its rc.
- Mutation 1: in `write_site`, insert `    cargo-package) printf '0' ;;  # MUTATION-SMA-685`
  directly after the line `  case "$kind" in` that follows
  `local kind="$1" target="$2" version="$3" abs="$REPO_ROOT/$2"`.
- Mutation 2: in `stamp_sites`, insert `      cargo-package) continue ;;  # MUTATION-SMA-685`
  directly after its `    case "$kind" in` line.
Expected for both: `--check` rc 1 with the three binding rows red; `--self-test` rc 1. Nothing in
the worktree changes: confirm `git -C $W status --short` is empty.

- [ ] **Step 7: Write the M3 section and commit**

Replace `Recorded by Task 4.` in the measurements file with sections `### M3.1` … `### Mutations`,
each with the command, verbatim lines and a verdict. Then:
```bash
cd $W && git add docs/superpowers/specs/2026-09-25-sma-685-measurements.md && git commit -F $S/msg-task4.txt
```
with:
```
docs(repo): measure the version-lockstep stamp fix for SMA-685

Record M3 (old and new --write on a release-plz kernel bump), the
lockdiff, the kernel-ts test and the two mutations.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
```

---

### Task 5: Correct every comment and doc site, and the release.yml step text

**Files:**
- Modify: `rs/release-plz.toml:56-64`, `:87-94`, `:144-150`
- Modify: `rs/crates/libs/paigasus-kernel/Cargo.toml:3-8`
- Modify: `.github/CLAUDE.md:8-27`, `:50-58`, `:146-156`, `:275-278` (+ one new bullet)
- Modify: `.github/workflows/release.yml:318-321`, `:354`, `:395`, `:632-634`
- Modify: `ci/version-lockstep/run.sh` (comments only), `ci/version-lockstep/README.md`
- Modify: `ci/affected-graph/cargo_moon_parity.py:451-456` (a waiver REASON string only)
- Modify: `docs/superpowers/specs/2026-09-24-sma-680-measurements.md` (two pointer lines)

**Interfaces:**
- Consumes: the Task 1 and Task 4 verdicts. If any measurement recorded a cause as OPEN, the
  text below that states the cause must say OPEN instead; ask the controller before writing.

- [ ] **Step 1: Check for a pin on the release.yml step name**

```bash
cd $W && git grep -n "non-Cargo" -- ci/ .github/ | cat
```
Expected: hits only in `.github/workflows/release.yml` (and none under `ci/`). If `ci/` has a
hit, keep the step name and change only the commit message.

- [ ] **Step 2: `rs/release-plz.toml`**

Replace lines 56-64 (from `# --- Releasable: the kernel family` to the line
`# \`repo:version-lockstep\` asserts the non-Cargo manifests follow.`) with:

```toml
# --- Releasable: the kernel family (ADR-0011 S1) ------------------------------------------
# One version across crates.io/PyPI/npm. release-plz 0.3.158 bumps ONLY `paigasus-kernel` here.
# It never writes the version of a crate whose Cargo manifest says `publish = false`, version
# group or not (READ: `packages_to_process()`, release_plz_core updater.rs:283-302, takes only
# Cargo-publishable and `git_only` crates; version groups are computed over that set only,
# updater.rs:162-189; MEASURED, SMA-685 M1 and M1b). So `version_group` and `release = true`
# on the three binding crates below are INERT today. They stay as the declaration, in case a
# later pin changes the filter; read the source again when .prototools moves the pin.
# The binding versions come from `ci/version-lockstep/run.sh --write`, which the release-PR
# job's stamp step runs (SMA-685). It writes each `publish = false` non-head crate's version
# from the head. `repo:version-lockstep` then checks every site.
# An earlier comment here said `version_group` applies to these crates "(measured)". It was
# wrong: the bindings reached 0.1.0 by hand (e2007d55), and the SMA-407 fixture most probably
# ran with `git_only = true` (INFERRED; SMA-685 M1c).
```

Replace lines 87-94 (the note above `paigasus-py-bindings`, from `# publish = false MIRRORS` to
`# ship as maturin/napi/wasm byproducts to PyPI and npm instead.`) with:

```toml
# publish = false MIRRORS each crate's Cargo manifest. release-plz reads an ABSENT publish key as
# `publish = true` and hard-errors on the contradiction ("has `publish = false` ... in the
# Cargo.toml, but it has `publish = true` in the release-plz configuration"), which blocked
# `release-plz release` entirely — measured on this repo before SMA-579 fixed it.
# `release = true` and `version_group` are inert on these crates (see the kernel-family note
# above): release-plz neither bumps nor tags them. Their versions come from
# `version-lockstep --write`. They ship as maturin/napi/wasm byproducts to PyPI and npm.
```

In lines 144-150, replace the sentences from `That is the scoped claim, not a blanket one:` to
`see the CONFLICT (SMA-685) note there.` with:

```toml
# The same filter applies to every Cargo `publish = false` crate, in a version group or not
# (READ, SMA-685; see the kernel-family note above).
```

Also change `(M7, measured on\n# 0.3.158)` context only if it now contradicts; it does not.
Confirm: `grep -n 'CONFLICT\|DOES\s*$\|DOES apply' rs/release-plz.toml` → no hits.

- [ ] **Step 3: The kernel manifest comment**

In `rs/crates/libs/paigasus-kernel/Cargo.toml`, replace lines 6-8 (from `# (SMA-385 trap). This
version is held` … to `# manifests say \`publish = false\` (measured against release-plz 0.3.158).`)
so that the block reads:

```toml
# The 0.1.0 floor (ADR-0011 S3). release-plz cuts every tag; never hand-place a `*-vX.Y.Z`
# tag — manual tags lack release-plz's tracking metadata and silently stop future bumps
# (the SMA-385 trap). This version is held in lockstep with the py/npm binding artifacts by
# `repo:version-lockstep`. release-plz bumps only this crate: the three binding crates are
# Cargo `publish = false`, which release-plz 0.3.158 never versions (SMA-685). The release-PR
# job's `version-lockstep --write` step writes their versions from this one.
```

(Decision recorded at GATE 1: this edit makes the next `release-pr` run propose a kernel 0.1.1
release PR. That is accepted as the first live test of the fix.)

- [ ] **Step 4: `.github/CLAUDE.md`**

- In the release-plz bullet (`:8-27`), replace the sentences from `Read that as the scoped claim
  it is` to `the other.` with: `The same filter applies inside a version group: release-plz
  0.3.158 never writes the version of a Cargo \`publish = false\` crate, even when the group head
  is publishable (READ, \`updater.rs:283-302\`; MEASURED, SMA-685 M1/M1b). The kernel binding
  crates get their versions from \`version-lockstep --write\` instead. The earlier claim that
  \`version_group\` writes them was wrong.`
- In the standing rule (`:50-58`), change the rule sentence to `**Standing rule: release-plz tags
  only what it PUBLISHES. \`release = true\` does not get a \`publish = false\` crate tagged, and
  (SMA-685) it does not keep it in the version group either.**` and replace `so \`release = true\`
  keeps a crate in the version group and does NOT get it tagged` with `so \`release = true\` does
  NOT get it tagged`, and `those crates' versions come from \`version_group\` +
  \`repo:version-lockstep\`` with `those crates' versions come from \`version-lockstep --write\`
  and are checked by \`repo:version-lockstep\``.
- In the kernel-family lockstep bullet (`:146-156`), replace `release-plz owns every Cargo
  \`[package] version\` — via per-package \`version_group\` — **and** the
  \`[workspace.dependencies]\` version requirements; both were measured against the pinned
  0.3.158, as was the fact that \`version_group\` applies to crates whose Cargo manifest says
  \`publish = false\`. The script owns the six sites Cargo cannot reach (\`--write\`)` with
  `release-plz owns the Cargo \`[package] version\` of each group's publishable crates and the
  \`[workspace.dependencies]\` version requirements (measured against 0.3.158). It does NOT write
  a Cargo \`publish = false\` crate (SMA-685). The script owns nine sites (\`--write\`): the six
  non-Cargo sites and the three \`publish = false\` binding manifests`.
- In the service-versions bullet (`:275-278`), replace `See the release-plz entry above for the
  bound on that claim: a \`publish = false\` crate inside a group with a publishable head IS
  version-written.` with `The same holds inside a version group (SMA-685).`
- Add one bullet after the release-plz bullet: `- **A local \`release-plz\` measurement needs a
  branch with no upstream, or an upstream with the same branch name.** release-plz checks out the
  upstream's branch NAME in its temporary copy (READ, \`git_cmd/src/lib.rs:44-60, 177-179\`). A
  scratch branch \`m2\` that tracks \`origin/feature/x\` makes it walk \`feature/x\`, which does
  not contain the scratch commit, and it reports \`already up to date\` (MEASURED, SMA-685 M2).
  \`git checkout -B m2 origin/…\` sets such an upstream. CI is not affected: \`main\` tracks
  \`origin/main\`.`

- [ ] **Step 5: `release.yml`**

- `:318-321`: `# release-plz first, ALWAYS, then --write (next step). release-plz owns the
  publishable crates' Cargo versions and the [workspace.dependencies] requirements; --write then
  brings the nine sites it cannot reach (the three publish = false binding manifests,
  pyproject/package.json/the dependency pin) plus the three regenerated sites into line
  (SMA-685).`
- `:354`: `- name: Stamp the lockstep manifests onto the release PR branch` (only if Step 1 found
  no pin).
- `:395`: `git commit -m "chore(rs): stamp the lockstep manifests to the release version"`.
- `:632-634`: replace `which version_group holds to the identical number (repo:version-lockstep
  asserts that).` with `which the stamp step's version-lockstep --write holds to the identical
  number (repo:version-lockstep asserts that).`

- [ ] **Step 6: `ci/version-lockstep/run.sh` comments and README**

- `git grep -n "six\|release-plz owns\|not written here" -- ci/version-lockstep/` and correct
  each hit that says `--write` skips Cargo manifests or owns six sites. Do not change code.
- README `:25-28`: keep the rationale; append: `\`--write\` writes only the \`publish = false\`
  non-head \`cargo-package\` sites (SMA-685), so it cannot hide a proto \`version_group\` fault:
  \`paigasus-proto-derive\` is publishable and stays release-plz's.`
- README Modes table `--write` row: `Rewrite the nine sites release-plz cannot reach (the six
  non-Cargo sites and the three \`publish = false\` binding manifests) and regenerate the three
  derived files (five \`SITES\` rows: 16-20).` `--self-test` row: `Fixture tables for the verdict
  function, the lock readers and the cargo-package writer, plus \`stamp_sites\` on a staged copy
  of the real tree.`
- README negative-control section: change "two drifts" to "three drifts" and add the
  `paigasus-wasm` `cargo-package` drift to the list.
- README limitations: `grep -n 'L1\|L2' ci/version-lockstep/README.md`; in L1 record that the
  `cargo-package` kind now has an end-to-end drift (SMA-685); correct L2 only if it states the
  old ownership claim.

- [ ] **Step 7: The A8 waiver reason and the SMA-680 pointers**

- `ci/affected-graph/cargo_moon_parity.py:453-454`: in the reason string, change `after writing
  the six non-Cargo version sites` to `after writing the nine version sites it owns (SMA-685)`.
  Do not change the dict KEY on `:450`.
- `docs/superpowers/specs/2026-09-24-sma-680-measurements.md`: after the M1 attempt 2
  `Cause: OPEN.` line (`:232`) and after the M2 attempt 1 verdict paragraph (`:247`), add one line
  each: `SMA-685 resolved this cause: see \`2026-09-25-sma-685-measurements.md\`.`

- [ ] **Step 8: Verify and commit**

```bash
cd $W
git grep -n "version_group.*publish = false\|DOES apply\|owns every Cargo\|six sites\|non-Cargo manifests" -- ':!docs/superpowers/plans' ':!docs/superpowers/specs/2026-0[8]*' | cat
/opt/homebrew/bin/bash ci/version-lockstep/run.sh --self-test; echo rc=$?
```
Expected: no hit that states the old claim as current (hits inside the SMA-685 spec, the SMA-685
measurements and quoted history are fine); self-test rc 0.
```bash
git add -A rs/release-plz.toml rs/crates/libs/paigasus-kernel/Cargo.toml .github/CLAUDE.md .github/workflows/release.yml ci/version-lockstep ci/affected-graph/cargo_moon_parity.py docs/superpowers/specs/2026-09-24-sma-680-measurements.md
git commit -F $S/msg-task5.txt
```
with:
```
docs(repo): correct the version_group claims for publish = false crates

release-plz 0.3.158 never versions a Cargo publish = false crate, so the
kernel bindings take their version from version-lockstep --write.
Correct every comment that said version_group writes them, and name the
stamp step for what it now writes.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
```

---

### Task 6: The full gate graph

**Files:** none (fix-ups only, if a gate reds).

- [ ] **Step 1: Run the CI target graph**

```bash
cd $W && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
git fetch -q origin main
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :http-extractor-envelope :input-liveness :promtool :observability-drift \
  :nats-permissions :release-parity :release-parity-py :release-parity-ts \
  :publish-metadata :version-lockstep :workflow-credentials :pyo3-stub-drift :ruff-ci \
  :next-public-free :helm-render :test-e2e --base origin/main --include-relations
```
Expected: every scheduled target passes. For a failure, follow the root `CLAUDE.md` diagnosis
procedure (capture first). Known host artifacts: a gate that needs bash 4+ fails with
`mapfile`/`declare -A` errors under bash 3.2, and `repo:affected-smoke` needs bash 3.2; re-run
such a gate directly with the right bash and record both results. `repo:actionlint` gives a local
verdict only when its pipe preflight passes.

- [ ] **Step 2: Record and commit fix-ups**

If a gate needed a fix, make it in its own commit (`fix(ci): …`). Record the gate table (target,
result, bash used) in the PR description, not in a repo file.
