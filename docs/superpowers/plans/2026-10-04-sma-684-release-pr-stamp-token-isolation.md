# SMA-684 Release-PR Token Isolation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The `release-pr` job in `.github/workflows/release.yml` compiles nothing and runs no install script, because every step of that job can read the App private key. The release PR is still stamped, and the stamped `index.js` is byte-identical to `napi build` output.

**Architecture:** `ci/version-lockstep/run.sh --write` gets a text writer for the napi glue version literals (`write_site`'s new `napi-glue` arm, backed by `napi_glue_py`) and loses its `napi build` call. `--check` gets a static uv metadata assertion, so `uv lock` can never need a build backend. `--write` gets a changed-path boundary. `release.yml` loses its `pnpm install` step. `rs/release-plz.toml` sets `semver_check = false`. `ci/actionlint/release_guard.py` gets V18, an allowlist of actions, command words and command prefixes for every `UNGATED_JOBS` member.

**Tech Stack:** bash 4+ (`ci/version-lockstep/run.sh`, `declare -A`), python3 stdlib heredocs (`re`, `tomllib`, `subprocess`), Python 3.12 + PyYAML through `uv run --locked --project py` (`release_guard.py`), GitHub Actions YAML, release-plz 0.3.158, @napi-rs/cli 3.10.4 (`ts/pnpm-lock.yaml:1711`), Moon 2.5.3.

**Spec:** `docs/superpowers/specs/2026-10-04-sma-684-release-pr-stamp-token-isolation-design.md` (approved by Sven on 2026-10-04, GATE 1).

## Global Constraints

- Work only in the worktree `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-684-stamp-token`, on branch `feature/sma-684-stamp-step-token-isolation`. Check `git branch --show-current` before the first commit of each task.
- Do not edit or stage the spec file. `git add` only the paths that a step names.
- Never use `git commit --amend`, `git reset`, `git rebase`, `git stash`, `--no-verify` or `--no-gpg-sign`. Make one new commit per task.
- If a commit fails with "failed to fill whole buffer", 1Password is locked. Stop and ask the user to unlock it.
- If a commit fails with "commitlint not found", run `pnpm -C ts install`. Do not bypass the hook.
- Conventional commits with a workspace scope (`ci`, `rs`, `docs`), header at most 100 characters, ending in `(SMA-684)`. No body line may start with `#NNN` or look like `token: value` (for example `V18: ...` or `Spec: ...`). End every message with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Do not install host software (no `brew install`, no `pip install`). Run every command in the foreground.
- Before a proto tool (`moon`, `uv`, `pnpm`, `release-plz`, `actionlint`): `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text`.
- If the worktree sandbox refuses a command (it refuses many commands that name `.github` or use `git` in a compound form), write the commands into a script under the session scratchpad and run `/bin/bash <script>`.
- Every new shell or Python source file opens with an SPDX header. This plan adds no new source file.
- `ci/version-lockstep/run.sh` needs bash 4+: run it with `/opt/homebrew/bin/bash`, never with `/bin/bash` 3.2 (it uses `declare -A`).
- `ci/affected-graph/run.sh` (`repo:affected-smoke`) needs `/bin/bash` 3.2. `repo:ruff-ci`, `repo:next-public-free`, `repo:publish-metadata`, `repo:version-lockstep` and `repo:nats-permissions` need `/opt/homebrew/bin/bash`. `repo:actionlint` needs `/opt/homebrew/bin/bash` and a pipe preflight that passes.
- Probe the host pipe before a bash-5 run (Task 1 Step 1). If a new pipe holds only 512 bytes, a heredoc over 512 bytes and a `tar` pipe hang or fail under Homebrew bash. Then run `ci/version-lockstep/run.sh` in Docker: `docker run --rm -v "$PWD":/w -w /w python:3.12-bookworm bash ci/version-lockstep/run.sh <flag>` (one docker command per Bash call). Never read a hang or `tar: Write error` as a gate result.
- `ci/actionlint/run.sh` check 13 bans a pipe into `grep -q`, `grep -m`, `head` or `awk … exit` in every tracked `*.sh`, workflow and `moon.yml`. Use `grep -q … < <(printf '%s\n' "$var")` instead. Do not use a here-string (`<<<`) on text that can exceed 512 bytes.
- In `ci/version-lockstep/run.sh`, a new non-comment line must not contain `cargo` followed by a space and a word. `cargo_moon_parity.py`'s A8 scan reads every such line, and only the three existing `cargo update -w` lines carry a waiver (`ci/affected-graph/cargo_moon_parity.py:456-490`). Do not change those three lines.
- `write_site`'s exit contract stays 0 (wrote or current), 3 (the repo is wrong), 2 (any other failure). `stamp_sites` maps 3 to 1 and every other non-zero status to 2 (`run.sh:749-755`).
- Restore a mutation with the Edit tool. Never with `git checkout --`, which also discards the uncommitted work under test.
- Do not write the file name of Moon's CI report JSON (the file that the root `CLAUDE.md` diagnosis procedure reads) in any file of this change. `repo:actionlint` check 12 then requires a marker on that file.

## Review Focus

1. **A version-length change compared as a string.** `"0.10.0" < "0.9.9"` as strings, so a string compare refuses a real bump and accepts a lowering. The L check must compare integer tuples. Pinned in Task 1 (`napi_glue_writer_self_test` cases N2 and L2: `0.9.9 -> 0.10.0` writes, `0.10.0 -> 0.9.9` exits 3).
2. **A shell keyword or a command substitution hides a forbidden command behind an allowed word.** The spec lists `if`, `then`, `else`, `fi` as allowed command words, so `if cargo build; then` and `if [ -n "$X" ]; then cargo build; fi` would pass V18. `echo "$(cargo build)"`, `` echo `cargo build` `` and `X="a$(cargo build)"` start with an allowed word. V18 strips the keywords and checks the next word, and rejects a substitution after the command word or inside an assignment value. Pinned in Task 7 (`_SMA684_V18_CASES`).
3. **V18 reds the real job because `command_segments` does not read quotes.** `jq '.prs | length'` (`release.yml:367`) splits into a segment whose first word is `length')"`. The continuation `echo "…" \` (`release.yml:369-370`) puts `"at` first on the next line. A comment line inside a `run:` block that holds `|` or `;` splits the same way. Pinned in Task 5 (two `jq` calls) and Task 7 (logical-line join, full-line comment skip, the clean controls, and the real job as a clean control).
4. **The changed-path check does not see a file in a new directory.** `git status --porcelain` without `--untracked-files=all` reports a new directory as `dir/`, and a file in an untracked directory that existed before is not new at all. A rename entry carries a second path. Pinned in Task 4 (the fixture creates `extra/new/file.txt` and requires that exact path in the report; a path that was dirty before stays unreported).
5. **The uv check reads only the files that SITES names, or the staged trees lack its files.** `paigasus-ml` and `paigasus-workflows` are uv members but not SITES rows. `stage_pristine_tree` stages only SITES files, so `run_check` on a staged tree would exit 2 once it reads `py/pyproject.toml`. Pinned in Task 3 (U1 puts the dynamic field in a member that SITES does not name; U8 runs the real `run_check` on a staged tree with a dynamic field in the staged `paigasus-ml` and requires rc 1, not 2; `--negative-control` must still pass).

---

## File map

| File | Task | Change |
|---|---|---|
| `ci/version-lockstep/run.sh` | 1, 2, 3, 4 | `napi_glue_py`, the `napi-glue` arm and filter, `_fn_body`, `napi_glue_writer_self_test`; `run_write` without `napi build`; `uv_static_metadata_check`, its staging and its self-test; `dirty_paths`, `write_set_violations` |
| `ci/version-lockstep/README.md` | 1, 2, 3, 4 | the sections that each task's code changes |
| `ts/CLAUDE.md`, `ts/packages/paigasus-kernel/moon.yml`, `.github/CLAUDE.md` | 2, 3 | "`--write` runs `napi build`" text; the input count |
| `moon.yml` (root), `ci/affected-graph/ci_targets.py` | 3, 7 | `repo:version-lockstep` inputs and their pin; the release-guard fixture floor pins |
| `.github/workflows/release.yml` | 5 | remove the `pnpm install` step, the `jq` rewrite, comments |
| `rs/release-plz.toml` | 6 | `semver_check = false`, the `dependencies_update` comment |
| `ci/actionlint/release_guard.py` | 7 | V18, its fixture rows, its case table and helper |
| `ci/actionlint/run.sh`, `ci/actionlint/README.md` | 7 | the fixture floor; check 10 row, L22, L43 |
| `ci/affected-graph/cargo_moon_parity.py` | 8 | waiver prose ("nine" sites becomes "ten") |

---

## Task 1: The napi-glue text writer and its self-test

**Files:**
- Modify: `ci/version-lockstep/run.sh` (line 80 `SELF_TEST_COUNT`; lines 222-225 the `napi-glue` read comment; lines 455-466 and 510-516 the two `case` lines in `stamp_sites_self_test`; after line 542 new functions; lines 550-553 `run_self_tests`; after line 740 `napi_glue_py`; lines 742-747 the `stamp_sites` comment; line 762 the filter; before line 984 the new arm; line 984 the `*)` comment)
- Modify: `ci/version-lockstep/README.md` (lines 21-22, 50, 114-118, 147-152)

**Interfaces:**
- Produces `napi_glue_py write <file> <version>`: prints `1` (wrote) or `0` (already current) on stdout; exit 0, 3 (E3 or L), 2 (V, E1, E2, E4, E5, E6, unreadable file, bad usage).
- Produces `napi_glue_py verify <old-file> <new-file> <version>`: no stdout; exit 0 or 2 (E4, E5).
- Produces `write_site napi-glue <path> <version>` → delegates to `napi_glue_py write "$REPO_ROOT/<path>" <version>`.
- Produces `_fn_body <function-name>`: prints that function's lines from `run.sh`, comment lines removed; exit 2 when the function is not found.
- Produces `napi_glue_writer_self_test` (self-test table 5). `SELF_TEST_COUNT=5`.
- Consumes `stamp_sites` (passes `napi-glue` rows to `write_site` after this task), `read_version napi-glue`, `stage_pristine_tree`.

- [ ] **Step 1: Probe the host pipe**

Run:
```bash
python3 -c 'import os
r, w = os.pipe(); os.set_blocking(w, False); n = 0
try:
    while True: n += os.write(w, b"x")
except BlockingIOError: print(n)'
```
Expected: `65536` (or another value of 8192 or more). If it prints `512`, use the Docker command from Global Constraints for every `ci/version-lockstep/run.sh` run in this plan.

- [ ] **Step 2: Write the failing tests**

In `ci/version-lockstep/run.sh`, change line 80:
```bash
SELF_TEST_COUNT=5   # site_verdict, lock_reader, cargo_package_writer, stamp_sites, napi_glue_writer
```

In `stamp_sites_self_test`, the line
```bash
    case "$kind" in cargo-package|pyproject|pyproject-dep|packagejson) ;; *) continue ;; esac
```
occurs twice (the sentinel loop and the kernel readback loop). Replace both (Edit with `replace_all: true`) with:
```bash
    case "$kind" in cargo-package|pyproject|pyproject-dep|packagejson|napi-glue) ;; *) continue ;; esac
```

In `stamp_sites_self_test`, directly before the proto readback loop (the `for entry in "${SITES[@]}"; do` whose body starts with `[ "$group" = proto ] || continue`), insert:
```bash
  # SMA-684: read_version's napi-glue arm reads the G1 literals only. Count both literal forms in
  # the stamped file, so a writer that moved G1 and left G2 behind cannot pass this table.
  got="$(python3 - "$tmp/rs/crates/bindings/paigasus-node-bindings/index.js" <<'PY'
import sys
with open(sys.argv[1], encoding="utf-8") as f:
    s = f.read()
print(s.count("bindingPackageVersion !== '9.9.9'"), s.count("expected 9.9.9 but got"))
PY
)" || return 2
  [ "${got% *}" -gt 0 ] && [ "${got% *}" = "${got#* }" ] \
    || { fail "self-test: stamp_sites left the napi glue G1/G2 counts at '$got', expected two equal non-zero counts at 9.9.9"; return 1; }

```

After the closing `}` of `stamp_sites_self_test` (line 542) and before `run_self_tests() {`, insert:
```bash
# SMA-684: print the body of the shell function $1 in this file, with comment lines removed. The
# call-site pins below read a body this way. A process substitution feeds grep, because check 13
# bans a pipe into `grep -q`, and a here-string over 512 bytes deadlocks Homebrew bash 5.3.15 on a
# host whose new pipes hold 512 bytes (ci/CLAUDE.md).
_fn_body() { # $1 function name
  local all
  all="$(sed -n "/^$1() {/,/^}\$/p" "${BASH_SOURCE[0]}")" || return 2
  [ -n "$all" ] || return 2
  grep -Ev '^[[:space:]]*#' < <(printf '%s\n' "$all") || return 2
}

# SMA-684: fixture table for the napi-glue text writer (spec §5.1, §5.7), plus the call-site pins
# that keep it on the write path. The generator writes the two guard shapes that napi 3.10.4
# writes: 26 native guards and one WASI guard, as in the real index.js, so 54 version literals. A
# correct edit must equal the generator's output at the new version, byte for byte.
napi_glue_writer_self_test() {
  local tmp rc got before after entry f tgt want
  tmp="$(mktemp -d)" || die_infra "cannot create a scratch dir"
  # shellcheck disable=SC2064
  trap "rm -rf '$tmp'" RETURN

  _ngw_gen() { # $1 file  $2 version  $3 native guard count  [$4 one extra line]
    python3 - "$tmp/$1" "$2" "$3" "${4:-}" <<'PY'
import sys
p, v, n, extra = sys.argv[1], sys.argv[2], int(sys.argv[3]), sys.argv[4]
out = ["// prettier-ignore", "/* auto-generated by NAPI-RS */", ""]
for i in range(n):
    out += [
        "      try {",
        f"        const binding = require('@paigasus/node-bindings-t{i}')",
        f"        const bindingPackageVersion = require('@paigasus/node-bindings-t{i}/package.json').version",
        f"        if (bindingPackageVersion !== '{v}' && process.env.NAPI_RS_ENFORCE_VERSION_CHECK && process.env.NAPI_RS_ENFORCE_VERSION_CHECK !== '0') {{",
        f"          throw new Error(`Native binding package version mismatch, expected {v} but got ${{bindingPackageVersion}}. You can reinstall dependencies to fix this issue.`)",
        "        }",
        "        return binding",
        "      } catch (e) {",
        "        loadErrors.push(e)",
        "      }",
    ]
out += [
    "        if (process.env.NAPI_RS_ENFORCE_VERSION_CHECK && process.env.NAPI_RS_ENFORCE_VERSION_CHECK !== '0') {",
    "          const bindingPackageVersion = require('@paigasus/node-bindings-wasm32-wasi/package.json').version",
    f"          if (bindingPackageVersion !== '{v}') {{",
    f"            throw new Error(`WASI binding package version mismatch, expected {v} but got ${{bindingPackageVersion}}. You can reinstall dependencies to fix this issue.`)",
    "          }",
    "        }",
    "module.exports.parsePrn = nativeBinding.parsePrn",
]
if extra:
    out.append(extra)
with open(p, "w", encoding="utf-8") as f:
    f.write("\n".join(out) + "\n")
PY
  }
  _ngw() { # $1 fixture  $2 version -> sets rc and got
    rc=0
    got="$(REPO_ROOT="$tmp" write_site napi-glue "$1" "$2" 2>/dev/null)" || rc=$?
  }
  _ngw_count() { # $1 file  $2 literal -> prints the number of occurrences
    python3 -c 'import sys; print(open(sys.argv[1], encoding="utf-8").read().count(sys.argv[2]))' "$tmp/$1" "$2"
  }
  _ngw_sub() { # $1 file  $2 literal  $3 replacement -> replaces the FIRST occurrence only
    python3 - "$tmp/$1" "$2" "$3" <<'PY'
import sys
p, a, b = sys.argv[1:4]
with open(p, encoding="utf-8") as f:
    s = f.read()
if a not in s:
    raise SystemExit(2)
with open(p, "w", encoding="utf-8") as f:
    f.write(s.replace(a, b, 1))
PY
  }

  # N1: a normal bump. All 54 literals move, and the file equals the generator's 0.3.0 output.
  _ngw_gen n1.js 0.2.0 26; _ngw_gen n1.want 0.3.0 26
  _ngw n1.js 0.3.0
  [ "$rc" -eq 0 ] && [ "$got" = 1 ] || { fail "self-test: napi-glue N1 rc=$rc got='$got', expected rc 0 and 1"; return 1; }
  cmp -s "$tmp/n1.js" "$tmp/n1.want" || { fail "self-test: napi-glue N1 is not the generator's 0.3.0 output"; return 1; }
  [ "$(_ngw_count n1.js 0.3.0)" = 54 ] || { fail "self-test: napi-glue N1 does not hold 54 literals at 0.3.0"; return 1; }

  # N2: a bump that changes the length, 0.9.9 -> 0.10.0. A string compare would refuse it.
  _ngw_gen n2.js 0.9.9 26; _ngw_gen n2.want 0.10.0 26
  _ngw n2.js 0.10.0
  [ "$rc" -eq 0 ] && [ "$got" = 1 ] || { fail "self-test: napi-glue N2 rc=$rc got='$got', expected rc 0 and 1"; return 1; }
  cmp -s "$tmp/n2.js" "$tmp/n2.want" || { fail "self-test: napi-glue N2 is not the generator's 0.10.0 output"; return 1; }

  # N3: already current. Prints 0, changes no byte and keeps the mtime.
  touch -t 200001010000 "$tmp/n2.js"
  before="$(python3 -c 'import os,sys;print(os.stat(sys.argv[1]).st_mtime_ns)' "$tmp/n2.js")"
  _ngw n2.js 0.10.0
  after="$(python3 -c 'import os,sys;print(os.stat(sys.argv[1]).st_mtime_ns)' "$tmp/n2.js")"
  [ "$rc" -eq 0 ] && [ "$got" = 0 ] && [ "$before" = "$after" ] \
    || { fail "self-test: napi-glue N3 rc=$rc got='$got' mtime $before -> $after"; return 1; }
  cmp -s "$tmp/n2.js" "$tmp/n2.want" || { fail "self-test: napi-glue N3 changed a byte"; return 1; }

  # N4: the NEW version string already occurs outside the two forms. It is left alone.
  _ngw_gen n4.js 0.2.0 26 "// release notes for 0.3.0"; _ngw_gen n4.want 0.3.0 26 "// release notes for 0.3.0"
  _ngw n4.js 0.3.0
  [ "$rc" -eq 0 ] && [ "$got" = 1 ] || { fail "self-test: napi-glue N4 rc=$rc got='$got'"; return 1; }
  cmp -s "$tmp/n4.js" "$tmp/n4.want" || { fail "self-test: napi-glue N4 touched the text outside the guards"; return 1; }

  # N6: the WASI guard shape alone (no env condition on the comparison line).
  _ngw_gen n6.js 0.2.0 0; _ngw_gen n6.want 0.3.0 0
  _ngw n6.js 0.3.0
  [ "$rc" -eq 0 ] && [ "$got" = 1 ] || { fail "self-test: napi-glue N6 rc=$rc got='$got'"; return 1; }
  cmp -s "$tmp/n6.js" "$tmp/n6.want" || { fail "self-test: napi-glue N6 is not the generator's WASI output"; return 1; }

  # N5 and N7: shapes the writer must refuse, each with its exit code, leaving the file unchanged.
  _ngw_gen n5.js 0.2.0 26 "// pinned at 0.2.0"         # E6: a decoy <old> outside a guard
  printf 'module.exports = {}\n' >"$tmp/e1.js"           # E1: no G1 guard at all
  _ngw_gen e2.js 0.2.0 26                                 # E2: one G2 literal lost
  _ngw_sub e2.js "expected 0.2.0 but got" "expected-0.2.0-but got" || die_infra "cannot build fixture e2"
  _ngw_gen e2b.js 0.2.0 26                                # E2: a G1 literal in a form G1 does not read
  _ngw_sub e2b.js "!== '0.2.0'" "!== '0.2.0-rc.1'" || die_infra "cannot build fixture e2b"
  _ngw_gen e3.js 0.2.0 26                                 # E3: one guard holds another version
  _ngw_sub e3.js "!== '0.2.0'" "!== '0.1.0'" || die_infra "cannot build fixture e3"
  _ngw_sub e3.js "expected 0.2.0 but got" "expected 0.1.0 but got" || die_infra "cannot build fixture e3"
  _ngw_gen l1.js 0.3.0 26                                 # L: lowering
  _ngw_gen l2.js 0.10.0 26                                # L: lowering across a length change
  _ngw_gen v1.js 0.2.0 26                                 # V: a non-plain target
  for entry in n5:0.3.0:2 e1:0.3.0:2 e2:0.3.0:2 e2b:0.3.0:2 e3:0.3.0:3 l1:0.2.0:3 l2:0.9.9:3 v1:0.3.0-rc.1:2; do
    IFS=':' read -r f tgt want <<<"$entry"
    cp "$tmp/$f.js" "$tmp/$f.before"
    _ngw "$f.js" "$tgt"
    [ "$rc" -eq "$want" ] || { fail "self-test: napi-glue $f must exit $want, got rc=$rc"; return 1; }
    cmp -s "$tmp/$f.js" "$tmp/$f.before" || { fail "self-test: napi-glue $f changed the file although refused"; return 1; }
  done

  # N8: E4 and E5 cannot fire on a correct writer, so the verify mode gets an edit that no correct
  # writer makes. The positive control first: a correct edit verifies.
  _ngw_gen x_old.js 0.2.0 26; _ngw_gen x_ok.js 0.3.0 26
  napi_glue_py verify "$tmp/x_old.js" "$tmp/x_ok.js" 0.3.0 2>/dev/null \
    || { fail "self-test: napi-glue verify refused a correct edit"; return 1; }
  cp "$tmp/x_ok.js" "$tmp/x_e4.js"
  _ngw_sub x_e4.js "!== '0.3.0'" "!== '0.2.0'" || die_infra "cannot build fixture x_e4"
  rc=0; napi_glue_py verify "$tmp/x_old.js" "$tmp/x_e4.js" 0.3.0 2>/dev/null || rc=$?
  [ "$rc" -eq 2 ] || { fail "self-test: napi-glue E4 (one guard left behind) rc=$rc, expected 2"; return 1; }
  cp "$tmp/x_ok.js" "$tmp/x_e5.js"; printf 'x' >>"$tmp/x_e5.js"
  rc=0; napi_glue_py verify "$tmp/x_old.js" "$tmp/x_e5.js" 0.3.0 2>/dev/null || rc=$?
  [ "$rc" -eq 2 ] || { fail "self-test: napi-glue E5 (a byte outside the literals) rc=$rc, expected 2"; return 1; }

  # Call-site pins (spec §5.7). write_site keeps its arm, and stamp_sites passes the kind to it.
  grep -Eq '^    napi-glue\)$' < <(_fn_body write_site) \
    || { fail "self-test: write_site has no napi-glue) arm"; return 1; }
  grep -Eq '^      ([a-z-]+[|])*napi-glue([|][a-z-]+)*\) ;;$' < <(_fn_body stamp_sites) \
    || { fail "self-test: the stamp_sites filter does not name napi-glue"; return 1; }

  SELF_TESTS_RAN=$((SELF_TESTS_RAN + 1))
}

```

In `run_self_tests`, after the line `  stamp_sites_self_test`, add:
```bash
  napi_glue_writer_self_test
```

- [ ] **Step 3: Run the tests and see them fail**

Run: `/opt/homebrew/bin/bash ci/version-lockstep/run.sh --self-test; echo "rc=$?"`

Expected: stderr shows `FAIL: self-test: stamp_sites left napi-glue rs/crates/bindings/paigasus-node-bindings/index.js at '0.2.0', expected 9.9.9`, and `rc=1`. (`stamp_sites_self_test` runs before the new table, and `stamp_sites` still skips the kind.)

- [ ] **Step 4: Implement the writer, the arm and the filter**

After the closing `}` of `cargo_publish_false` (line 740) and before the comment `# SMA-685: the per-site loop of --write`, insert:
```bash
# SMA-684: the version literals of the committed napi glue, written as TEXT (spec §5.1). `napi
# build` wrote them before, and it compiled the binding crate, the kernel and every build script
# and proc macro in their graph, in the release-PR job, where every step can read the App private
# key. This writer compiles nothing.
#
# Two literal forms carry the version, and the version is the only capture group:
#   G1  bindingPackageVersion !== '<X.Y.Z>'
#   G2  expected <X.Y.Z> but got
# Modes:
#   write <file> <version>         every check in memory, then an in-place write (open for
#                                  writing, never a rename: a rename breaks the pnpm hard link,
#                                  ts/CLAUDE.md). Prints 1 if it wrote, 0 if already current.
#   verify <old> <new> <version>   E4 and E5 only. The self-test drives this mode with an edit
#                                  that no correct writer makes.
# Exit codes follow write_site's contract (see stamp_sites below): 0 wrote or already current;
# 3 the repo is wrong (E3: the guards disagree; L: the file is higher than the head), which
# stamp_sites maps to rc 1; 2 for every other failure (V, E1, E2, E4, E5, E6, an unreadable
# file). E3 exits 3 and not 1 here: a python traceback also exits 1, and stamp_sites must read
# that as an infrastructure failure.
napi_glue_py() { # write <file> <version> | verify <old-file> <new-file> <version>
  python3 - "$@" <<'PY'
import re, sys

VER = r"[0-9]+\.[0-9]+\.[0-9]+"
G1 = re.compile(r"bindingPackageVersion !== '(" + VER + r")'")
G2 = re.compile(r"expected (" + VER + r") but got")
G1_ANY = "bindingPackageVersion !== '"
G2_ANY = re.compile(r"expected \S+ but got")
MASK = "\x00V\x00"


def fatal(msg):
    print(f"FATAL: {msg}", file=sys.stderr)
    raise SystemExit(2)


def repo_wrong(msg):
    print(f"FAIL: {msg}", file=sys.stderr)
    raise SystemExit(3)


def plain(v, what):
    m = re.fullmatch(r"([0-9]+)\.([0-9]+)\.([0-9]+)", v)
    if m is None:
        fatal(f"{what} version '{v}' is not plain X.Y.Z")
    return tuple(int(x) for x in m.groups())


def read_text(p):
    try:
        with open(p, "rb") as f:
            return f.read().decode("utf-8")
    except (OSError, UnicodeDecodeError) as e:
        fatal(f"cannot read {p} as UTF-8: {e}")


def masked(s):
    s = G1.sub(lambda m: "bindingPackageVersion !== '" + MASK + "'", s)
    return G2.sub(lambda m: "expected " + MASK + " but got", s)


def verify(old, new, target):
    # E4: every literal in the new text holds the target, and no literal appeared or vanished.
    old1, old2 = G1.findall(old), G2.findall(old)
    new1, new2 = G1.findall(new), G2.findall(new)
    if len(new1) != len(old1) or len(new2) != len(old2) or any(v != target for v in new1 + new2):
        fatal(f"E4: after the edit, not every guard literal holds {target}; writer defect")
    # E5: masks, not a reverse edit on positions. A reverse edit shares code with the forward
    # edit, and its positions move when the version length changes (0.9.9 -> 0.10.0).
    if masked(old) != masked(new):
        fatal("E5: the edit changed a byte outside the guard literals; writer defect")


def write(path, target):
    # V: the target is plain X.Y.Z, the same rule as the cargo-package arm's plain().
    head = plain(target, "head")
    old = read_text(path)
    g1, g2 = G1.findall(old), G2.findall(old)
    # E1: at least one G1 literal.
    if not g1:
        fatal(f"E1: {path} has no `bindingPackageVersion !== '<X.Y.Z>'` guard; the napi format changed")
    # E2: G1 and G2 agree in count, and no literal of either form escapes the strict pattern.
    n1_any, n2_any = old.count(G1_ANY), len(G2_ANY.findall(old))
    if len(g1) != len(g2) or n1_any != len(g1) or n2_any != len(g2):
        fatal(f"E2: {path} has {len(g1)} G1 and {len(g2)} G2 literals, {n1_any} and {n2_any} in any form; the napi format changed")
    # E6: each version string the guards hold occurs exactly twice per guard in the whole file.
    values = sorted(set(g1 + g2))
    found = sum(old.count(v) for v in values)
    if found != 2 * len(g1):
        fatal(f"E6: {path} holds {values} {found} times, expected {2 * len(g1)} (two per guard); napi added or removed a version-bearing literal")
    # E3: one value across every literal.
    if len(values) != 1:
        repo_wrong(f"{path}: the guards hold more than one version: {values}")
    cur = values[0]
    # L: never lower. Integer tuples, not strings: "0.10.0" < "0.9.9" as strings.
    if plain(cur, "site") > head:
        repo_wrong(f"{path} is at {cur}, higher than the head {target}; not lowered")
    if cur == target:
        print(0)
        raise SystemExit(0)
    new = G1.sub(lambda m: "bindingPackageVersion !== '" + target + "'", old)
    new = G2.sub(lambda m: "expected " + target + " but got", new)
    verify(old, new, target)
    with open(path, "wb") as f:
        f.write(new.encode("utf-8"))
    print(1)


args = sys.argv[1:]
if len(args) == 3 and args[0] == "write":
    write(args[1], args[2])
elif len(args) == 4 and args[0] == "verify":
    verify(read_text(args[1]), read_text(args[2]), args[3])
else:
    fatal(f"usage: napi_glue_py write <file> <version> | verify <old> <new> <version>, got {args}")
PY
}

```

In the comment above `stamp_sites` (lines 742-747), change the sentence
```bash
# loop on a staged tree. It writes the pyproject, pyproject-dep and packagejson sites, and the
```
to
```bash
# loop on a staged tree. It writes the pyproject, pyproject-dep, packagejson and napi-glue sites
# (napi-glue since SMA-684), and the
```

In `stamp_sites`, change the filter line
```bash
      pyproject|pyproject-dep|packagejson) ;;
```
to
```bash
      pyproject|pyproject-dep|packagejson|napi-glue) ;;
```

In `write_site`, directly before the line `    *) printf '0' ;;   # regeneration-owned kinds (cargo-wsdep, the locks, the napi glue) are not written here`, insert:
```bash
    napi-glue)
      # SMA-684: the version literals of the committed napi glue, edited as text. napi_glue_py
      # holds the checks and the exit codes; this arm only names the file.
      [ -r "$abs" ] || die_infra "cannot read $target"
      napi_glue_py write "$abs" "$version"
      ;;
```
and change that `*)` line to:
```bash
    *) printf '0' ;;   # regeneration-owned kinds (cargo-wsdep, the two locks) are not written here
```

In `read_version`, replace lines 223-225:
```bash
      # napi regenerates 26 `bindingPackageVersion !== '<v>'` guards from package.json.
      # A non-uniform set prints "" and reads as MISMATCH; an unreadable or undecodable
      # file exits 2.
```
with:
```bash
      # The 27 `bindingPackageVersion !== '<v>'` guards (26 native, 1 WASI). --write's text writer
      # (write_site's napi-glue arm, SMA-684) writes them, and this arm reads them back. A
      # non-uniform set prints "" and reads as MISMATCH; an unreadable or undecodable file exits 2.
```

- [ ] **Step 5: Run the tests and see them pass**

Run: `/opt/homebrew/bin/bash ci/version-lockstep/run.sh --self-test; echo "rc=$?"`
Expected: `== version-lockstep self-tests passed (5 tables) ==` and `rc=0`.

Run: `/opt/homebrew/bin/bash ci/version-lockstep/run.sh --negative-control; echo "rc=$?"`
Expected: the three `== negative control: ... ==` lines and `rc=0`.

Run: `/opt/homebrew/bin/bash ci/version-lockstep/run.sh; echo "rc=$?"`
Expected: `== all 20 version-lockstep sites agree ==` and `rc=0`.

- [ ] **Step 6: Deletion check V-2 (a): remove the arm**

With Edit, delete the six-line `napi-glue)` arm from `write_site` (from `    napi-glue)` to its `      ;;`).
Run: `/opt/homebrew/bin/bash ci/version-lockstep/run.sh --self-test; echo "rc=$?"`
Expected: `FAIL: self-test: stamp_sites left napi-glue rs/crates/bindings/paigasus-node-bindings/index.js at '0.2.0', expected 9.9.9` and `rc=1`.
Restore the arm with Edit (paste back the exact six lines from Step 4). Re-run; expected `(5 tables)` and `rc=0`.

- [ ] **Step 7: Deletion check V-2 (b): remove the kind from the filter**

With Edit, change `      pyproject|pyproject-dep|packagejson|napi-glue) ;;` back to `      pyproject|pyproject-dep|packagejson) ;;`.
Run: `/opt/homebrew/bin/bash ci/version-lockstep/run.sh --self-test; echo "rc=$?"`
Expected: the same `stamp_sites left napi-glue … at '0.2.0'` failure and `rc=1`.
Restore with Edit. Re-run; expected `(5 tables)` and `rc=0`.

- [ ] **Step 8: Update the README**

In `ci/version-lockstep/README.md`, replace lines 21-22:
```markdown
- `rs/crates/bindings/paigasus-node-bindings/index.js`, whose 26 committed
  `bindingPackageVersion !== '<v>'` guards napi regenerates from `package.json`
```
with:
```markdown
- `rs/crates/bindings/paigasus-node-bindings/index.js`, whose 27 committed guards (26 native,
  1 WASI) each carry the version twice: `bindingPackageVersion !== '<v>'` and
  `expected <v> but got`
```
Replace the `--self-test` row (line 50):
```markdown
| `--self-test` | Fixture tables for the verdict function, the lock readers and the cargo-package writer, plus `stamp_sites` on a staged copy of the real tree. |
```
with:
```markdown
| `--self-test` | Fixture tables for the verdict function, the lock readers, the cargo-package writer and the napi-glue writer, plus `stamp_sites` on a staged copy of the real tree. |
```
Replace lines 114-118:
```markdown
**L2 — Fixture-table coverage now spans six of the eight `read_version` kinds, plus the
cargo-package writer and the production stamping call site.**
`--self-test` (`SELF_TEST_COUNT=4`) runs four tables: `site_verdict_self_test` (OK/MISMATCH
logic), `lock_reader_self_test`, `cargo_package_writer_self_test` (SMA-685), and
`stamp_sites_self_test` (SMA-685).
```
with:
```markdown
**L2 — Fixture-table coverage now spans six of the eight `read_version` kinds, plus the
cargo-package and napi-glue writers and the production stamping call site.**
`--self-test` (`SELF_TEST_COUNT=5`) runs five tables: `site_verdict_self_test` (OK/MISMATCH
logic), `lock_reader_self_test`, `cargo_package_writer_self_test` (SMA-685),
`stamp_sites_self_test` (SMA-685), and `napi_glue_writer_self_test` (SMA-684).

`napi_glue_writer_self_test` drives `write_site napi-glue` against generated glue: a normal bump,
a length-changing bump (`0.9.9` to `0.10.0`), an already-current file, a new version string that
already occurs outside the guards, a decoy old version string (E6), the WASI guard alone, and the
refusals E1, E2, E3, L (also across a length change) and a non-plain target. It drives E4 and E5
through `napi_glue_py verify`, because a correct writer cannot trip them. It also pins the
`napi-glue)` arm in `write_site` and the kind in the `stamp_sites` filter.
```
Replace lines 147-152:
```markdown
The remaining two kinds (`cargo-wsdep` and `napi-glue`) still have no fixture of their own, so
a broken parser inside one of them — the wrong TOML key, an off-by-one on the `[[package]]`
block split, a regex that matches the wrong table — is caught only if it happens to manifest on
the real repo's current files or on the one non-lock site (`packagejson`) the negative control
drifts. The `cargo-package` kind's READ side is also proven only on the real tree's own file
shapes, not on the varied layouts the write-side fixtures cover.
```
with:
```markdown
`cargo-wsdep` still has no fixture of its own, so a broken parser inside it — the wrong TOML key,
a regex that matches the wrong table — is caught only if it happens to manifest on the real
repo's current files. `napi-glue` has a WRITE fixture since SMA-684
(`napi_glue_writer_self_test`), and `stamp_sites_self_test` writes the real `index.js`, reads it
back and counts both literal forms, but the READ arm has no synthetic fixture. The
`cargo-package` kind's READ side is also proven only on the real tree's own file shapes, not on
the varied layouts the write-side fixtures cover.
```

- [ ] **Step 9: Commit**

```bash
git add ci/version-lockstep/run.sh ci/version-lockstep/README.md
git commit -m "feat(ci): write the napi glue version literals as text in version-lockstep (SMA-684)" \
  -m "write_site gets a napi-glue arm backed by napi_glue_py. It edits the two version literals of every guard in index.js in place and checks the file in memory before it writes. stamp_sites now passes the napi-glue row to it. A new self-test table covers the writer, and stamp_sites_self_test reads the stamped real index.js back." \
  -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Task 2: `run_write` compiles nothing

**Files:**
- Modify: `ci/version-lockstep/run.sh` (`napi_glue_writer_self_test`: new pin before its `SELF_TESTS_RAN` line; `run_write` lines 993-1007)
- Modify: `ci/version-lockstep/README.md` (line 48, lines 172-177)
- Modify: `ts/CLAUDE.md` (lines 141-143, 150-153)
- Modify: `ts/packages/paigasus-kernel/moon.yml` (lines 26-27, 269-272)
- Modify: `.github/CLAUDE.md` (lines 35-38, 183-190)

**Interfaces:**
- Consumes `_fn_body run_write` (Task 1).
- Produces `run_write` that runs only `stamp_sites`, the two lock regenerations and the summary line. Its non-comment lines name no `pnpm`, `npx`, `napi` or `node` command word.

- [ ] **Step 1: Write the failing pin**

In `napi_glue_writer_self_test`, directly before its line `  SELF_TESTS_RAN=$((SELF_TESTS_RAN + 1))`, insert:
```bash
  # run_write compiles nothing (spec A1, A3). Comment lines are removed first. A bare \bnode\b
  # would match paigasus-node-bindings, so a command word is matched by its neighbours instead.
  local body
  body="$(_fn_body run_write)" || { fail "self-test: cannot read the body of run_write"; return 1; }
  if grep -Eq '(^|[[:space:];&|(])(pnpm|npx|napi|node)([[:space:];&|)]|$)' < <(printf '%s\n' "$body"); then
    fail "self-test: run_write names pnpm, npx, napi or node as a command word; --write must compile nothing (SMA-684)"
    return 1
  fi

```

- [ ] **Step 2: Run it and see it fail**

Run: `/opt/homebrew/bin/bash ci/version-lockstep/run.sh --self-test; echo "rc=$?"`
Expected: `FAIL: self-test: run_write names pnpm, npx, napi or node as a command word; --write must compile nothing (SMA-684)` and `rc=1`.

- [ ] **Step 3: Remove the build from `run_write`**

Replace this block of `run_write`:
```bash
  # Regenerate the three derived files (SITES rows 16-20 — kernel's and proto's cargo-lock and
  # uv-lock rows each point at the same file, so five rows resolve to three files). Each file is
  # owned by a tool, not by this script.
  ( cd "$REPO_ROOT/rs" && cargo update -w --offline >/dev/null 2>&1 ) \
    || ( cd "$REPO_ROOT/rs" && cargo update -w >/dev/null ) \
    || die_infra "cargo update -w failed (site 16)"
  ( cd "$REPO_ROOT/py" && uv lock >/dev/null ) || die_infra "uv lock failed (site 17)"
  # @napi-rs/cli is a devDependency of @paigasus/kernel, not of the ts workspace root
  # (pnpm-workspace.yaml's catalog comment: a file:-linked dep's devDeps aren't installed
  # at the consumer's node_modules root) — a bare `pnpm exec` from ts/ cannot find `napi`
  # and pnpm treats it as a recursive exec across every workspace package instead, failing
  # on the first one that lacks it. Scope it with --filter to the package that has it.
  ( cd "$REPO_ROOT/ts" && pnpm --filter @paigasus/kernel exec napi build --platform \
      --cwd "$REPO_ROOT/rs/crates/bindings/paigasus-node-bindings" >/dev/null ) \
    || die_infra "napi build failed (site 18)"
```
with:
```bash
  # Regenerate the two derived lock files (SITES rows 16, 17, 19 and 20: kernel's and proto's
  # cargo-lock and uv-lock rows each point at the same file, so four rows resolve to two files).
  # Each lock file is owned by its tool. Neither command runs a build script or a build backend
  # (spec F6, F7; --check asserts the static uv metadata that F7 needs). Row 18, the napi glue,
  # is written by stamp_sites above through write_site's napi-glue arm (SMA-684). This function
  # compiles nothing: the release-PR job runs it, and every step of that job can read the App
  # private key. napi_glue_writer_self_test pins that no build command comes back here.
  ( cd "$REPO_ROOT/rs" && cargo update -w --offline >/dev/null 2>&1 ) \
    || ( cd "$REPO_ROOT/rs" && cargo update -w >/dev/null ) \
    || die_infra "cargo update -w failed (site 16)"
  ( cd "$REPO_ROOT/py" && uv lock >/dev/null ) || die_infra "uv lock failed (site 17)"
```
The three `cargo update -w` lines and the `uv lock` line stay byte-identical (the A8 waivers key on their text).

- [ ] **Step 4: Run and see it pass**

Run: `/opt/homebrew/bin/bash ci/version-lockstep/run.sh --self-test; echo "rc=$?"`
Expected: `== version-lockstep self-tests passed (5 tables) ==` and `rc=0`.

- [ ] **Step 5: Deletion check V-2 (c): put `napi build` back**

With Edit, insert directly after the `uv lock` line of `run_write`:
```bash
  ( cd "$REPO_ROOT/ts" && pnpm --filter @paigasus/kernel exec napi build --platform ) || die_infra "x"
```
Run: `/opt/homebrew/bin/bash ci/version-lockstep/run.sh --self-test; echo "rc=$?"`
Expected: the `run_write names pnpm, npx, napi or node` failure and `rc=1`.
Remove the line with Edit. Re-run; expected `(5 tables)` and `rc=0`.

- [ ] **Step 6: Update the docs that say `--write` runs `napi build`**

`ci/version-lockstep/README.md`, replace the `--write` row (line 48):
```markdown
| `--write` | Rewrite the nine sites release-plz cannot reach (the six non-Cargo sites and the three `publish = false` binding manifests) and regenerate the three derived files (five `SITES` rows: 16-20). |
```
with:
```markdown
| `--write` | Rewrite the ten sites release-plz cannot reach (the six non-Cargo sites, the three `publish = false` binding manifests, and the version literals of the napi glue `index.js`) and regenerate the two lock files (four `SITES` rows: 16, 17, 19 and 20). It compiles nothing (SMA-684). |
```
Replace lines 172-177 (the `@napi-rs/cli` bullet):
```markdown
- `@napi-rs/cli` is a devDependency of `@paigasus/kernel` (`ts/packages/paigasus-kernel`),
  not of the ts workspace root — a `file:`-linked consumer's own devDeps aren't installed at
  the root `node_modules`. A bare `pnpm exec napi …` from `ts/` finds no `napi` binary and
  pnpm treats it as a recursive exec across every workspace package, failing on the first
  one that lacks it. `run_write` scopes the call with `pnpm --filter @paigasus/kernel exec`
  instead.
```
with:
```markdown
- The `napi-glue` writer (SMA-684) edits the version literals of `index.js` as text. It
  replaced `napi build`, which compiled the binding crate and its whole build graph in the
  release-PR job, where every step can read the App private key. It checks the file in memory
  before it writes (spec §5.1: V, E1, E2, E6, E3, L, E4, E5) and writes in place, never by
  rename, because a rename breaks the pnpm hard link (`ts/CLAUDE.md`). A guard format change in
  a new `@napi-rs/cli` exits 2. `stamp_sites_self_test` runs the writer on the real `index.js`,
  and `index.js` is an input of this task, so a napi bump PR that regenerates the glue reds here
  before a release. `run_write` runs no `pnpm`, `npx`, `napi` or `node` command;
  `napi_glue_writer_self_test` pins that.
```

`ts/CLAUDE.md`, replace lines 141-143:
```markdown
  `moon run paigasus-kernel-ts:generate-napi-glue` is the local writer. `version-lockstep --write`
  also writes both files in a release. It runs a full `napi build --platform` in place, which stamps
  the version guards and replaces both files by rename.
```
with:
```markdown
  `moon run paigasus-kernel-ts:generate-napi-glue` is the local writer. In a release,
  `version-lockstep --write` writes only the version literals of `index.js`, as text and in place
  (SMA-684). It runs no `napi build` and does not touch `index.d.ts`, which holds no version.
```
Replace lines 150-153:
```markdown
  The `build` task runs `tsc`, and `tsc` reads the installed copy of `index.d.ts` through
  `ts/node_modules`. A rename-write breaks the pnpm hard link of that copy. After a checkout, a
  rebase or `version-lockstep --write` that replaces `index.d.ts`, run `rm -rf ts/node_modules &&
  pnpm -C ts install`, as in the wasm paragraph above.
```
with:
```markdown
  The `build` task runs `tsc`, and `tsc` reads the installed copy of `index.d.ts` through
  `ts/node_modules`. A rename-write breaks the pnpm hard link of that copy. After a checkout or a
  rebase that replaces `index.d.ts`, run `rm -rf ts/node_modules && pnpm -C ts install`, as in the
  wasm paragraph above.
```

`ts/packages/paigasus-kernel/moon.yml`, replace lines 26-27:
```yaml
  # the scratch files index.fresh.js and index.fresh.d.ts beside it (SMA-667): the committed
  # index.js and index.d.ts have two writers only, generate-napi-glue and version-lockstep --write.
```
with:
```yaml
  # the scratch files index.fresh.js and index.fresh.d.ts beside it (SMA-667): the committed
  # index.js and index.d.ts have two writers only. generate-napi-glue writes both files.
  # version-lockstep --write writes only the version literals of index.js, as text (SMA-684).
```
Replace lines 269-272:
```yaml
  # SMA-667. The sanctioned local writer of the two committed napi glue files
  # (rs/crates/bindings/paigasus-node-bindings/index.js and index.d.ts). `version-lockstep --write`
  # is the only other writer. In a release it runs a full `napi build --platform` in place. That
  # build stamps the version guards and rewrites both files by rename.
```
with:
```yaml
  # SMA-667. The sanctioned local writer of the two committed napi glue files
  # (rs/crates/bindings/paigasus-node-bindings/index.js and index.d.ts). `version-lockstep --write`
  # is the only other writer, and of index.js only. In a release it edits the version literals of
  # the guards as text, in place (SMA-684). It runs no `napi build` and does not write index.d.ts.
```

`.github/CLAUDE.md`, replace lines 35-38:
```markdown
- `dependencies_update` is `false` since SMA-680. `true` runs a full `cargo update` in the release
  PR. That made the committed wasm glue stale on v0.2.0, and it ran unreviewed third-party build
  scripts in the stamp step, which holds a write-capable token. `false` runs
  `cargo update --workspace`. The reasons and the source lines are in the key's comment.
```
with:
```markdown
- `dependencies_update` is `false` since SMA-680. `true` runs a full `cargo update` in the release
  PR. That made the committed wasm glue stale on v0.2.0. Before SMA-684 it also ran unreviewed
  third-party build scripts in the stamp step, which holds a write-capable token. Since SMA-684
  that step compiles nothing, so the wasm glue reason alone keeps the key `false`. `false` runs
  `cargo update --workspace`. The reasons and the source lines are in the key's comment.
```
Replace lines 183-190:
```markdown
- `--write` owns nine sites: five kernel non-Cargo sites, one proto pyproject site, and three
  binding manifests. The script checks all twenty sites, because a `version_group` fault could
  stop applying silently. Today that risk is real only for `paigasus-proto-derive`. Two sites
  drift silently without this check. `py/uv.lock` drifts because its `moon.yml` runs bare
  `uv sync`, not `--locked`. The 26 `bindingPackageVersion` guards in the committed napi glue
  drift because the codegen-drift gate covers only the three `**/generated` proto dirs.
  Since SMA-667, `paigasus-kernel-ts:test` (`tests/committed-napi-glue.test.ts`) also fails on stale
  guards. The generator reads the version from the crate `package.json`.
```
with:
```markdown
- `--write` owns ten sites: five kernel non-Cargo sites, one proto pyproject site, three binding
  manifests, and the version literals of the napi glue `index.js` (SMA-684). The script checks
  all twenty sites, because a `version_group` fault could stop applying silently. Today that risk
  is real only for `paigasus-proto-derive`. Two sites drift silently without this check.
  `py/uv.lock` drifts because its `moon.yml` runs bare `uv sync`, not `--locked`. The 27
  `bindingPackageVersion` guards in the committed napi glue (26 native, 1 WASI) drift because the
  codegen-drift gate covers only the three `**/generated` proto dirs. Since SMA-667,
  `paigasus-kernel-ts:test` (`tests/committed-napi-glue.test.ts`) also fails on stale guards.
- `--write` writes the napi guards with a text writer, not with `napi build` (SMA-684). The
  release-PR job compiles nothing, because every step of that job can read the App private key.
  The writer exits 2 when the napi guard format changes. `stamp_sites_self_test` runs it on the
  real `index.js`, so a napi bump PR that regenerates the glue reds `repo:version-lockstep`
  before a release. `release_guard.py` V18 holds the `release-pr` job to an allowlist of steps.
```

- [ ] **Step 7: Re-run the gate**

Run: `/opt/homebrew/bin/bash ci/version-lockstep/run.sh --self-test; echo "rc=$?"` — expected `(5 tables)`, `rc=0`.
Run: `/opt/homebrew/bin/bash ci/version-lockstep/run.sh; echo "rc=$?"` — expected `== all 20 version-lockstep sites agree ==`, `rc=0`.

- [ ] **Step 8: Commit**

```bash
git add ci/version-lockstep/run.sh ci/version-lockstep/README.md ts/CLAUDE.md ts/packages/paigasus-kernel/moon.yml .github/CLAUDE.md
git commit -m "fix(ci): stop running napi build in version-lockstep --write (SMA-684)" \
  -m "run_write no longer compiles the napi binding. The napi-glue text writer from the previous commit writes the version literals instead. A self-test pin fails if pnpm, npx, napi or node comes back into run_write. The docs that said --write runs napi build are corrected." \
  -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Task 3: The static uv metadata check in `--check`

**Files:**
- Modify: `ci/version-lockstep/run.sh` (line 80; lines 41-58 the `EXPECTED_SITE_COUNT` comment; `run_self_tests`; new self-test after `napi_glue_writer_self_test`; `run_check` lines 586-593; `stage_pristine_tree` lines 612-622; new function after `napi_glue_py`)
- Modify: `moon.yml` (root, `version-lockstep` task, lines 695-718)
- Modify: `ci/affected-graph/ci_targets.py` (lines 246-263 `SELF_TASK_EXPECTED_GLOBS["version-lockstep"]`; comments at lines 277, 2135-2136, 4041-4045, 4054-4055)
- Modify: `ci/version-lockstep/README.md` (lines 47, 50, 74-79, 114-118; new section)
- Modify: `.github/CLAUDE.md` (lines 191-197)

**Interfaces:**
- Produces `uv_static_metadata_check` (reads `$REPO_ROOT`): exit 0 clean; 1 a violation (prints `FAIL: <file>: …` lines on stderr, each naming the file and the field); 2 cannot read a file or the workspace declares no members.
- Produces `uv_static_metadata_check --list`: prints every pyproject path it reads, relative to `$REPO_ROOT`, sorted, one per line; exit 0 or 2.
- Produces `uv_static_metadata_self_test` (table 6). `SELF_TEST_COUNT=6`.
- `run_check` prints `uv workspace: every local package declares static metadata` on a clean tree; maps the check's rc 1 to its own rc 1 and any other non-zero to `return 2`.
- `stage_pristine_tree` also stages the `--list` output.

- [ ] **Step 1: Write the failing self-test**

Change line 80:
```bash
SELF_TEST_COUNT=6   # site_verdict, lock_reader, cargo_package_writer, stamp_sites, napi_glue_writer, uv_static_metadata
```

After the closing `}` of `napi_glue_writer_self_test`, insert:
```bash
# SMA-684 §5.2: fixture table for the static uv metadata check. Each case is a small uv workspace
# under its own scratch REPO_ROOT: one member with a path source (the maturin crate's shape), and
# one member that SITES does not name (the dormant shape of paigasus-ml and paigasus-workflows).
uv_static_metadata_self_test() {
  local tmp rc listed out
  tmp="$(mktemp -d)" || die_infra "cannot create a scratch dir"
  # shellcheck disable=SC2064
  trap "rm -rf '$tmp'" RETURN

  _usm_tree() { # $1 case dir -> a clean workspace
    local r="$tmp/$1"
    mkdir -p "$r/py/packages/a" "$r/py/packages/dormant" "$r/rs/b"
    printf '[tool.uv.workspace]\nmembers = ["packages/*"]\n' >"$r/py/pyproject.toml"
    printf '[project]\nname = "a"\nversion = "0.1.0"\ndependencies = ["b==0.1.0"]\n\n[tool.uv.sources]\nb = { path = "../../../rs/b" }\n' >"$r/py/packages/a/pyproject.toml"
    printf '[project]\nname = "dormant"\nversion = "0.0.0"\ndependencies = []\n' >"$r/py/packages/dormant/pyproject.toml"
    printf '[project]\nname = "b"\nversion = "0.1.0"\n\n[build-system]\nrequires = ["maturin>=1.9.6,<2"]\nbuild-backend = "maturin"\n' >"$r/rs/b/pyproject.toml"
  }
  _usm() { # $1 case dir -> sets rc
    rc=0
    REPO_ROOT="$tmp/$1" uv_static_metadata_check >/dev/null 2>&1 || rc=$?
  }

  # U0: the clean tree passes.
  _usm_tree u0; _usm u0
  [ "$rc" -eq 0 ] || { fail "self-test: uv-static U0 (clean) rc=$rc, expected 0"; return 1; }
  # U0b: --list names every file the check reads, the path source included. stage_pristine_tree
  # stages exactly this list, so a file missing here is a file the staged trees lack.
  listed="$(REPO_ROOT="$tmp/u0" uv_static_metadata_check --list)" \
    || { fail "self-test: uv-static --list failed on the clean tree"; return 1; }
  [ "$listed" = "$(printf 'py/packages/a/pyproject.toml\npy/packages/dormant/pyproject.toml\npy/pyproject.toml\nrs/b/pyproject.toml')" ] \
    || { fail "self-test: uv-static --list printed '$listed'"; return 1; }
  # U1: dynamic version in a member that SITES does not name.
  _usm_tree u1
  printf '[project]\nname = "dormant"\ndynamic = ["version"]\ndependencies = []\n' >"$tmp/u1/py/packages/dormant/pyproject.toml"
  _usm u1; [ "$rc" -eq 1 ] || { fail "self-test: uv-static U1 (dynamic version) rc=$rc, expected 1"; return 1; }
  # U2: dynamic dependencies in a member.
  _usm_tree u2
  printf '[project]\nname = "a"\nversion = "0.1.0"\ndynamic = ["dependencies"]\n\n[tool.uv.sources]\nb = { path = "../../../rs/b" }\n' >"$tmp/u2/py/packages/a/pyproject.toml"
  _usm u2; [ "$rc" -eq 1 ] || { fail "self-test: uv-static U2 (dynamic dependencies) rc=$rc, expected 1"; return 1; }
  # U3: dynamic optional-dependencies in the path source.
  _usm_tree u3
  printf '[project]\nname = "b"\nversion = "0.1.0"\ndynamic = ["optional-dependencies"]\n\n[build-system]\nrequires = ["maturin>=1.9.6,<2"]\nbuild-backend = "maturin"\n' >"$tmp/u3/rs/b/pyproject.toml"
  _usm u3; [ "$rc" -eq 1 ] || { fail "self-test: uv-static U3 (dynamic optional-dependencies) rc=$rc, expected 1"; return 1; }
  # U4: a member with no [project] table: uv would run its build backend to read its metadata.
  _usm_tree u4
  printf '[build-system]\nrequires = ["setuptools"]\n' >"$tmp/u4/py/packages/dormant/pyproject.toml"
  _usm u4; [ "$rc" -eq 1 ] || { fail "self-test: uv-static U4 (no [project]) rc=$rc, expected 1"; return 1; }
  # U5: a dynamic field outside the three named ones is allowed (the check is not over-broad).
  _usm_tree u5
  printf '[project]\nname = "dormant"\nversion = "0.0.0"\ndependencies = []\ndynamic = ["classifiers"]\n' >"$tmp/u5/py/packages/dormant/pyproject.toml"
  _usm u5; [ "$rc" -eq 0 ] || { fail "self-test: uv-static U5 (dynamic classifiers) rc=$rc, expected 0"; return 1; }
  # U6: the message names the file and the field.
  out="$(REPO_ROOT="$tmp/u1" uv_static_metadata_check 2>&1 >/dev/null)" || true
  grep -Fq "py/packages/dormant/pyproject.toml: [project].dynamic lists 'version'" < <(printf '%s\n' "$out") \
    || { fail "self-test: uv-static U6 message does not name the file and the field: '$out'"; return 1; }
  # U7: a malformed member is an infrastructure failure, not a verdict.
  _usm_tree u7
  printf '[project\n' >"$tmp/u7/py/packages/dormant/pyproject.toml"
  _usm u7; [ "$rc" -eq 2 ] || { fail "self-test: uv-static U7 (malformed) rc=$rc, expected 2"; return 1; }
  # U8: the production path. The real run_check, on a staged copy of the real tree, with a dynamic
  # version in the staged paigasus-ml (a uv member that SITES does not name). rc 2 here means the
  # staging lacks a uv file; rc 0 means run_check no longer calls the check.
  mkdir "$tmp/st"
  stage_pristine_tree "$tmp/st"
  mkdir -p "$tmp/st/py/packages/paigasus-ml"
  printf '[project]\nname = "paigasus-ml"\ndynamic = ["version"]\ndependencies = []\n' >"$tmp/st/py/packages/paigasus-ml/pyproject.toml"
  rc=0; ( REPO_ROOT="$tmp/st" run_check ) >/dev/null 2>&1 || rc=$?
  [ "$rc" -eq 1 ] || { fail "self-test: uv-static U8 run_check on a staged tree with a dynamic member rc=$rc, expected 1"; return 1; }

  SELF_TESTS_RAN=$((SELF_TESTS_RAN + 1))
}

```

In `run_self_tests`, after the line `  napi_glue_writer_self_test`, add:
```bash
  uv_static_metadata_self_test
```

- [ ] **Step 2: Run and see it fail**

Run: `/opt/homebrew/bin/bash ci/version-lockstep/run.sh --self-test; echo "rc=$?"`
Expected: `FAIL: self-test: uv-static U0 (clean) rc=127, expected 0` and `rc=1`.

- [ ] **Step 3: Implement the check**

After the closing `}` of `napi_glue_py`, insert:
```bash
# SMA-684 §5.2: `uv lock` runs no build backend only while uv can read every local package's
# metadata without a build (spec F7). A `dynamic` version, dependencies or optional-dependencies
# field makes `uv lock` run the build backend, and for paigasus-py-bindings that backend is
# maturin, which compiles Rust in the release-PR job. The pyproject reader above covers only the
# three SITES pyproject files; this check reads every uv workspace member (members glob of
# py/pyproject.toml, minus `exclude`) and every [tool.uv.sources] path source, transitively. A
# member with no [project] table and a path source that is not a directory are violations too:
# uv would build either to read its metadata. With --list it prints the files it reads instead,
# so stage_pristine_tree can stage them.
uv_static_metadata_check() { # [--list] -> rc 0 clean | 1 a violation | 2 cannot read
  python3 - "$REPO_ROOT" "$@" <<'PY'
import glob, os, sys, tomllib
from fnmatch import fnmatch

root, args = sys.argv[1], sys.argv[2:]
listing = args == ["--list"]
if args and not listing:
    print(f"INFRA: unknown argument(s) {args}", file=sys.stderr)
    raise SystemExit(2)
py = os.path.join(root, "py")
ws_file = os.path.normpath(os.path.join(py, "pyproject.toml"))


def rel(p):
    return os.path.relpath(p, root)


def load(p):
    try:
        with open(p, "rb") as f:
            return tomllib.load(f)
    except (OSError, tomllib.TOMLDecodeError) as e:
        print(f"INFRA: cannot read {rel(p)}: {e}", file=sys.stderr)
        raise SystemExit(2) from None


ws = load(ws_file)
uvws = ws.get("tool", {}).get("uv", {}).get("workspace")
if not isinstance(uvws, dict) or not uvws.get("members"):
    print("INFRA: py/pyproject.toml declares no [tool.uv.workspace] members", file=sys.stderr)
    raise SystemExit(2)
queue = [ws_file]
for pat in uvws["members"]:
    dirs = [d for d in sorted(glob.glob(os.path.join(py, pat))) if os.path.isdir(d)]
    if not dirs:
        print(f"INFRA: the uv workspace member glob {pat!r} matches no directory", file=sys.stderr)
        raise SystemExit(2)
    for d in dirs:
        if not any(fnmatch(os.path.relpath(d, py), e) for e in uvws.get("exclude", [])):
            queue.append(os.path.normpath(os.path.join(d, "pyproject.toml")))

seen, violations = set(), []
while queue:
    p = queue.pop(0)
    if p in seen:
        continue
    seen.add(p)
    doc = load(p)
    proj = doc.get("project")
    if p != ws_file and not isinstance(proj, dict):
        violations.append(f"{rel(p)}: no [project] table, so uv would run the build backend to read its metadata")
    if isinstance(proj, dict):
        dynamic = proj.get("dynamic", [])
        for field in ("version", "dependencies", "optional-dependencies"):
            if field in dynamic:
                violations.append(f"{rel(p)}: [project].dynamic lists '{field}', so uv lock would run the build backend")
    sources = doc.get("tool", {}).get("uv", {}).get("sources", {})
    for name, src in sources.items():
        for entry in src if isinstance(src, list) else [src]:
            if isinstance(entry, dict) and "path" in entry:
                target = os.path.normpath(os.path.join(os.path.dirname(p), entry["path"]))
                if os.path.isdir(target):
                    queue.append(os.path.join(target, "pyproject.toml"))
                else:
                    violations.append(f"{rel(p)}: [tool.uv.sources] {name} path {entry['path']!r} is not a directory, so uv would build it")

if listing:
    for p in sorted(seen):
        print(rel(p))
    raise SystemExit(0)
for v in violations:
    print(f"FAIL: {v}", file=sys.stderr)
raise SystemExit(1 if violations else 0)
PY
}

```

- [ ] **Step 4: Run the fixture cases U0-U7**

Run: `/opt/homebrew/bin/bash ci/version-lockstep/run.sh --self-test; echo "rc=$?"`
Expected: `FAIL: self-test: uv-static U8 run_check on a staged tree with a dynamic member rc=0, expected 1` and `rc=1` (U0-U7 pass; `run_check` does not call the check yet, so the dynamic member goes unseen).

- [ ] **Step 5: Wire the check into `run_check` and the staging**

In `run_check`, replace:
```bash
  # Non-vacuity: the loop must have covered every declared site.
  [ "$checked" -eq "${#SITES[@]}" ] \
    || die_infra "checked $checked sites but ${#SITES[@]} are declared"
```
with:
```bash
  # Non-vacuity: the loop must have covered every declared site.
  [ "$checked" -eq "${#SITES[@]}" ] \
    || die_infra "checked $checked sites but ${#SITES[@]} are declared"
  # SMA-684 §5.2: uv lock in --write must never need a build backend. Explicit status routing,
  # not errexit, for the same reason as read_version above.
  local uvrc=0
  uv_static_metadata_check || uvrc=$?
  case "$uvrc" in
    0) printf 'uv workspace: every local package declares static metadata\n' ;;
    1) rc=1 ;;
    *) return 2 ;;
  esac
```

Replace the whole `stage_pristine_tree` function:
```bash
stage_pristine_tree() { # $1 destination dir
  local dest="$1" entry kind target
  {
    printf 'rs/Cargo.toml\n'
    for entry in "${SITES[@]}"; do
      IFS='|' read -r _ kind target <<<"$entry"
      [ "$kind" = cargo-wsdep ] || printf '%s\n' "$target"
    done
  } | sort -u | ( cd "$REPO_ROOT" && tar -cf - -T - ) | ( cd "$dest" && tar -xf - ) \
    || { rm -rf "$dest"; die_infra "cannot stage a scratch copy of the version-carrying files"; }
}
```
with:
```bash
stage_pristine_tree() { # $1 destination dir
  local dest="$1" entry kind target uvfiles
  # SMA-684: run_check also reads the uv workspace pyproject files (uv_static_metadata_check), so
  # the staged tree carries the same list that check reads, derived from the check itself.
  uvfiles="$(uv_static_metadata_check --list)" \
    || { rm -rf "$dest"; die_infra "cannot list the uv workspace pyproject files to stage"; }
  {
    printf 'rs/Cargo.toml\n'
    printf '%s\n' "$uvfiles"
    for entry in "${SITES[@]}"; do
      IFS='|' read -r _ kind target <<<"$entry"
      [ "$kind" = cargo-wsdep ] || printf '%s\n' "$target"
    done
  } | sort -u | ( cd "$REPO_ROOT" && tar -cf - -T - ) | ( cd "$dest" && tar -xf - ) \
    || { rm -rf "$dest"; die_infra "cannot stage a scratch copy of the version-carrying files"; }
}
```

- [ ] **Step 6: Run and see everything pass**

Run: `/opt/homebrew/bin/bash ci/version-lockstep/run.sh --self-test; echo "rc=$?"` — expected `== version-lockstep self-tests passed (6 tables) ==`, `rc=0`.
Run: `/opt/homebrew/bin/bash ci/version-lockstep/run.sh --negative-control; echo "rc=$?"` — expected the three `== negative control: ... ==` lines, `rc=0` (not an exit 2 from the staging).
Run: `/opt/homebrew/bin/bash ci/version-lockstep/run.sh; echo "rc=$?"` — expected `uv workspace: every local package declares static metadata`, then `== all 20 version-lockstep sites agree ==`, `rc=0`.

- [ ] **Step 7: Deletion check: unwire the check from `run_check`**

With Edit, change `  uv_static_metadata_check || uvrc=$?` to `  true || uvrc=$?`.
Run: `/opt/homebrew/bin/bash ci/version-lockstep/run.sh --self-test; echo "rc=$?"`
Expected: `FAIL: self-test: uv-static U8 run_check on a staged tree with a dynamic member rc=0, expected 1`, `rc=1`.
Restore with Edit. Re-run; expected `(6 tables)`, `rc=0`.

- [ ] **Step 8: Write the failing input pin**

`repo:version-lockstep` now reads `py/pyproject.toml` and every `py/packages/*/pyproject.toml`, so a change to any of them must schedule it. In `ci/affected-graph/ci_targets.py`, replace the `"version-lockstep"` tuple of `SELF_TASK_EXPECTED_GLOBS` (globs first, sorted; then files, sorted):
```python
    "version-lockstep": (
        "ci/version-lockstep/run.sh",
        "py/packages/paigasus-kernel/pyproject.toml",
        "py/packages/paigasus-proto/pyproject.toml",
        "py/uv.lock",
```
with:
```python
    "version-lockstep": (
        "py/packages/*/pyproject.toml",
        "ci/version-lockstep/run.sh",
        "py/packages/paigasus-kernel/pyproject.toml",
        "py/packages/paigasus-proto/pyproject.toml",
        "py/pyproject.toml",
        "py/uv.lock",
```
(The remaining eleven `rs/…` lines of the tuple stay as they are.)

Run (from the worktree root): `python3 ci/affected-graph/ci_targets.py; echo "rc=$?"`
Expected: a row starting `repo:version-lockstep's authored inputs are [` … `expected exactly [` and `rc=1`.

- [ ] **Step 9: Add the inputs**

In `moon.yml`, in the `version-lockstep` task, replace:
```yaml
    # Every file that CARRIES a version in either family, plus the script itself. Narrower than
```
with:
```yaml
    # Every file that CARRIES a version in either family, plus the script itself, plus the uv
    # workspace pyproject files that --check's static uv metadata assertion reads (SMA-684): the
    # workspace root as a literal path, and every member as a glob, because that check discovers
    # members at runtime from the members glob. Narrower than
```
and replace the last two input lines:
```yaml
      - 'py/packages/paigasus-kernel/pyproject.toml'
      - 'py/packages/paigasus-proto/pyproject.toml'

  actionlint:
```
with:
```yaml
      - 'py/packages/paigasus-kernel/pyproject.toml'
      - 'py/packages/paigasus-proto/pyproject.toml'
      - 'py/pyproject.toml'
      - 'py/packages/*/pyproject.toml'

  actionlint:
```

Run: `python3 ci/affected-graph/ci_targets.py; echo "rc=$?"` — expected `rc=0`.
Run: `python3 ci/affected-graph/ci_targets.py --self-test; echo "rc=$?"` — expected `rc=0`.
Run: `moon run repo:input-liveness; echo "rc=$?"` — expected the task passes, `rc=0`.

- [ ] **Step 10: Correct the counts in comments and docs**

`ci/version-lockstep/run.sh`, in the `EXPECTED_SITE_COUNT` comment, replace:
```bash
# which independently asserts moon.yml's own `inputs:` list — the paths SITES reads (14
# distinct: py/packages/paigasus-kernel/pyproject.toml, rs/Cargo.lock, and py/uv.lock are each
# read by two rows) plus rs/Cargo.toml (read by the cargo-wsdep kind by name, not by a SITES
# path) plus this script itself — 16 total, matching moon.yml's inputs: list.
```
with:
```bash
# which independently asserts moon.yml's own `inputs:` list — the paths SITES reads (14
# distinct: py/packages/paigasus-kernel/pyproject.toml, rs/Cargo.lock, and py/uv.lock are each
# read by two rows) plus rs/Cargo.toml (read by the cargo-wsdep kind by name, not by a SITES
# path) plus this script itself, plus py/pyproject.toml and the glob py/packages/*/pyproject.toml
# (read by uv_static_metadata_check, SMA-684) — 18 entries, matching moon.yml's inputs: list.
```

`ci/affected-graph/ci_targets.py`:
- Line 277: replace `    # so exact match is affordable, exactly as for version-lockstep's sixteen.` with `    # so exact match is affordable, exactly as for version-lockstep's eighteen.`
- Lines 2135-2136: replace `    does today (repo:input-liveness is glob-only, repo:version-lockstep file-only) and which is` with `    did when this parameter was added (repo:input-liveness glob-only, repo:version-lockstep then file-only) and which is`
- Lines 4041-4042: replace
```python
    # ...and both directions on a FILES-ONLY gate (SMA-576). repo:version-lockstep declares
    # sixteen literal paths and no glob, so neither of these rows is visible to the glob tuple at
```
with
```python
    # ...and both directions on the FILE half of a gate (SMA-576). repo:version-lockstep declares
    # seventeen literal paths beside one glob (SMA-684), so neither of these rows is visible to the glob tuple at
```
- Lines 4054-4055: replace
```python
    # both registered gates declare exactly one kind of input, so "globs then files" and "files
    # then globs" agree on all of them, and a mutation reversing the two survived the whole battery
```
with
```python
    # when this pair was written, both registered gates declared one kind of input, so "globs then
    # files" and "files then globs" agreed on all of them, and a mutation reversing the two survived the whole battery
```

`ci/version-lockstep/README.md`:
- Line 47, replace `| `--check` (default) | Compare all 20 sites. Exit 1 on any drift. |` with `| `--check` (default) | Compare all 20 sites, and assert static uv metadata (SMA-684). Exit 1 on any drift or violation. |`
- Line 50, replace `the cargo-package writer and the napi-glue writer, plus` with `the cargo-package writer, the napi-glue writer and the static uv metadata check, plus`.
- Lines 74-79, replace the two bullets that say "sixteen":
```markdown
- `SELF_TASK_EXPECTED_GLOBS` pins the task's sixteen `inputs:` entries. Drop one and the gate
  stops re-keying on that version site — it then reports PASS from Moon's cache over a file it
  never read. All sixteen are literal paths, so moon resolves them into `inputFiles` rather than
  `inputGlobs`; that constant compares the whole authored set across both buckets (SMA-576).
- `repo:input-liveness` asserts each of those sixteen still names a TRACKED file, so moving one
  reds CI instead of silently switching part of this gate off.
```
with:
```markdown
- `SELF_TASK_EXPECTED_GLOBS` pins the task's eighteen `inputs:` entries. Drop one and the gate
  stops re-keying on that file — it then reports PASS from Moon's cache over a file it never
  read. Seventeen are literal paths, which moon resolves into `inputFiles`; one,
  `py/packages/*/pyproject.toml` (SMA-684), is a glob, which moon resolves into `inputGlobs`.
  That constant compares the whole authored set across both buckets (SMA-576).
- `repo:input-liveness` asserts each of those eighteen still matches a TRACKED file, so moving
  one reds CI instead of silently switching part of this gate off.
```
- In L2, replace `` `--self-test` (`SELF_TEST_COUNT=5`) runs five tables `` with `` `--self-test` (`SELF_TEST_COUNT=6`) runs six tables ``, and replace `` `stamp_sites_self_test` (SMA-685), and `napi_glue_writer_self_test` (SMA-684). `` with `` `stamp_sites_self_test` (SMA-685), `napi_glue_writer_self_test` (SMA-684), and `uv_static_metadata_self_test` (SMA-684). ``
- Directly before `## How it runs in CI`, insert:
```markdown
## The static uv metadata check (SMA-684)

`--write` runs `uv lock` in the release-PR job, where every step can read the App private key.
`uv lock` runs no build backend only while uv can read every local package's metadata without a
build (spec F7). `--check` therefore asserts that no uv workspace member, and no
`[tool.uv.sources]` path source, lists `version`, `dependencies` or `optional-dependencies` in
`[project].dynamic`. A package with no `[project]` table, and a path source that is not a
directory, are violations too. A violation exits 1 and names the file and the field. The check
reads the members from `py/pyproject.toml`'s members glob, not from `SITES`, so it covers
`paigasus-ml` and `paigasus-workflows`. `stage_pristine_tree` stages the same files
(`uv_static_metadata_check --list`), so the negative control and `stamp_sites_self_test` run the
check on a complete tree.

```

`.github/CLAUDE.md`, replace:
```markdown
  pairing rule above, listing all sixteen of its literal `inputs`, so it needs no
```
with:
```markdown
  pairing rule above, listing all eighteen of its `inputs` (seventeen literal paths and the glob
  `py/packages/*/pyproject.toml`, SMA-684), so it needs no
```

- [ ] **Step 11: Re-run**

Run: `/opt/homebrew/bin/bash ci/version-lockstep/run.sh --self-test; echo "rc=$?"` — expected `(6 tables)`, `rc=0`.
Run: `python3 ci/affected-graph/ci_targets.py --self-test; echo "rc=$?"` — expected `rc=0`.

- [ ] **Step 12: Commit**

```bash
git add ci/version-lockstep/run.sh ci/version-lockstep/README.md moon.yml ci/affected-graph/ci_targets.py .github/CLAUDE.md
git commit -m "feat(ci): assert static uv metadata in version-lockstep --check (SMA-684)" \
  -m "uv lock in the release-PR job runs no build backend only while every local package declares a static version and static dependencies. --check now asserts that for every uv workspace member and every path source, and exits 1 on a violation. The staged trees carry the files the check reads, and repo:version-lockstep is scheduled by a change to any of them." \
  -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Task 4: The changed-path check after `--write`

**Files:**
- Modify: `ci/version-lockstep/run.sh` (new `dirty_paths` and `write_set_violations` before `run_write() {`; `run_write`; `stamp_sites_self_test` before its final `SELF_TESTS_RAN` line)
- Modify: `ci/version-lockstep/README.md` (line 48; new section)

**Interfaces:**
- Produces `dirty_paths` (reads `$REPO_ROOT`): prints every path that `git status --porcelain=v1 -z --untracked-files=all` reports (both paths of a rename or copy), sorted, one per line; exit 0, or 2 when `git status` fails.
- Produces `write_set_violations <before>`: prints every path that is in `dirty_paths` now, not in `<before>`, and not a SITES path (cargo-wsdep rows excluded), sorted; exit 0, or 2 when `dirty_paths` fails.
- `run_write` exits 2 with `INFRA: --write changed paths outside its write set (the SITES paths): <p1>, <p2>` when the list is not empty.

- [ ] **Step 1: Write the failing test and pins**

In `stamp_sites_self_test`, directly before its final line `  SELF_TESTS_RAN=$((SELF_TESTS_RAN + 1))`, insert:
```bash
  # SMA-684 §5.3: the changed-path check, on a scratch git repository with the staged tree
  # committed. A path dirty before the snapshot stays unreported. A SITES path changed after it is
  # allowed. A file in a NEW directory must be reported by its full path, which only
  # --untracked-files=all gives (the default reports `extra/`). The fixture commits, so it sets
  # the keys ci/CLAUDE.md requires (no background maintenance, no signing, no global config).
  local g before viol wbody
  g="$(mktemp -d)" || { rm -rf "$tmp"; die_infra "cannot create a scratch dir"; }
  stage_pristine_tree "$g"
  ( export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
    git -C "$g" init -q \
      && git -C "$g" config maintenance.auto false && git -C "$g" config gc.auto 0 \
      && git -C "$g" config commit.gpgsign false && git -C "$g" config tag.gpgsign false \
      && git -C "$g" config user.name self-test && git -C "$g" config user.email self-test@invalid \
      && git -C "$g" add -A && git -C "$g" commit -q -m base ) >/dev/null 2>&1 \
    || { rm -rf "$tmp" "$g"; die_infra "cannot make the scratch git repository"; }
  printf 'x\n' >"$g/pre-existing.txt"
  before="$(REPO_ROOT="$g" dirty_paths)" \
    || { rm -rf "$tmp" "$g"; die_infra "dirty_paths failed on the scratch repository"; }
  printf 'y\n' >>"$g/pre-existing.txt"
  printf '{"version": "9.9.9"}\n' >"$g/rs/crates/bindings/paigasus-wasm/package.json"
  mkdir -p "$g/extra/new"
  printf 'z\n' >"$g/extra/new/file.txt"
  viol="$(REPO_ROOT="$g" write_set_violations "$before")" \
    || { rm -rf "$tmp" "$g"; die_infra "write_set_violations failed on the scratch repository"; }
  rm -rf "$g"
  [ "$viol" = "extra/new/file.txt" ] \
    || { fail "self-test: the changed-path check reported '$viol', expected exactly 'extra/new/file.txt'"; return 1; }
  # ...and run_write still takes the snapshot and still applies the check.
  wbody="$(_fn_body run_write)" || { fail "self-test: cannot read the body of run_write"; return 1; }
  grep -Fq 'before="$(dirty_paths)" || die_infra' < <(printf '%s\n' "$wbody") \
    || { fail "self-test: run_write no longer records the dirty paths before it writes"; return 1; }
  grep -Fq 'violations="$(write_set_violations "$before")" || die_infra' < <(printf '%s\n' "$wbody") \
    || { fail "self-test: run_write no longer computes the paths outside its write set"; return 1; }
  grep -Fq '|| die_infra "--write changed paths outside its write set' < <(printf '%s\n' "$wbody") \
    || { fail "self-test: run_write no longer refuses a path outside its write set"; return 1; }

```

- [ ] **Step 2: Run and see it fail**

Run: `/opt/homebrew/bin/bash ci/version-lockstep/run.sh --self-test; echo "rc=$?"`
Expected: `INFRA: dirty_paths failed on the scratch repository` and `rc=2`.

- [ ] **Step 3: Implement**

Directly before `run_write() {`, insert:
```bash
# SMA-684 §5.3: the dirty and untracked paths of the work tree at $REPO_ROOT, one per line,
# sorted. `-z` keeps a path with a space or a quote intact. `--untracked-files=all` names every
# untracked FILE: the default collapses a new directory to `dir/`, which would hide a file inside
# a directory that was already untracked. A rename or copy names both of its paths.
dirty_paths() {
  python3 - "$REPO_ROOT" <<'PY'
import subprocess, sys
try:
    out = subprocess.run(
        ["git", "-C", sys.argv[1], "status", "--porcelain=v1", "-z", "--untracked-files=all"],
        check=True, capture_output=True).stdout.decode("utf-8", "surrogateescape")
except (OSError, subprocess.CalledProcessError) as e:
    print(f"INFRA: git status failed in {sys.argv[1]}: {e}", file=sys.stderr)
    raise SystemExit(2) from None
parts, paths, i = out.split("\0"), set(), 0
while i < len(parts):
    entry = parts[i]
    i += 1
    if not entry:
        continue
    xy, path = entry[:2], entry[3:]
    paths.add(path)
    if "R" in xy or "C" in xy:
        paths.add(parts[i])
        i += 1
for p in sorted(paths):
    print(p)
PY
}

# SMA-684 §5.3: print every path that is dirty now, was not dirty in $1 (dirty_paths' output from
# before the write), and is not in the write set. The write set is the SITES paths, rs/Cargo.lock
# and py/uv.lock among them; a cargo-wsdep row names a crate, not a file, so it adds no path. The
# stamp step runs `git add -A`, so this set is the boundary of what the release-PR job commits.
write_set_violations() { # $1 the dirty_paths output from before the write
  local after entry kind target allowed=""
  after="$(dirty_paths)" || return 2
  for entry in "${SITES[@]}"; do
    IFS='|' read -r _ kind target <<<"$entry"
    [ "$kind" = cargo-wsdep ] || allowed="$allowed$target"$'\n'
  done
  python3 - "$1" "$after" "$allowed" <<'PY'
import sys
before, after, allowed = (set(filter(None, a.split("\n"))) for a in sys.argv[1:4])
for p in sorted(after - before - allowed):
    print(p)
PY
}

```

In `run_write`, replace:
```bash
run_write() {
  local wrote rc=0
  wrote="$(stamp_sites)" || rc=$?
```
with:
```bash
run_write() {
  local wrote rc=0 before violations
  # SMA-684 §5.3: the paths that were dirty before this run. Every path that is new after it must
  # be in the write set; write_set_violations below holds that.
  before="$(dirty_paths)" || die_infra "cannot list the dirty paths before --write"
  wrote="$(stamp_sites)" || rc=$?
```
and directly after the line `  ( cd "$REPO_ROOT/py" && uv lock >/dev/null ) || die_infra "uv lock failed (site 17)"`, insert:
```bash

  violations="$(write_set_violations "$before")" || die_infra "cannot list the changed paths after --write"
  [ -z "$violations" ] \
    || die_infra "--write changed paths outside its write set (the SITES paths): ${violations//$'\n'/, }"
```

- [ ] **Step 4: Run and see it pass**

Run: `/opt/homebrew/bin/bash ci/version-lockstep/run.sh --self-test; echo "rc=$?"`
Expected: `== version-lockstep self-tests passed (6 tables) ==` and `rc=0`.

- [ ] **Step 5: Deletion checks**

(a) With Edit, change `"--porcelain=v1", "-z", "--untracked-files=all"],` to `"--porcelain=v1", "-z"],` in `dirty_paths`.
Run the self-test. Expected: `FAIL: self-test: the changed-path check reported 'extra/', expected exactly 'extra/new/file.txt'`, `rc=1`. Restore with Edit.

(b) With Edit, delete the three-line `violations=…` / `[ -z "$violations" ] \` / `|| die_infra "--write changed paths…"` block from `run_write`.
Run the self-test. Expected: `FAIL: self-test: run_write no longer computes the paths outside its write set`, `rc=1`. Restore with Edit.

Re-run after both restores; expected `(6 tables)`, `rc=0`.

- [ ] **Step 6: Update the README**

In `ci/version-lockstep/README.md`, in the `--write` row, replace `It compiles nothing (SMA-684). |` with `It compiles nothing, and it exits 2 when a path outside the SITES paths changed (SMA-684). |`.
Directly before `## How it runs in CI`, insert:
```markdown
## The changed-path check (SMA-684)

`run_write` records the dirty and untracked paths before it starts (`dirty_paths`, which reads
`git status --porcelain=v1 -z --untracked-files=all`). After it finishes, every path that is new
in that list must be a `SITES` path; `rs/Cargo.lock` and `py/uv.lock` are `SITES` paths.
Otherwise it exits 2 and names the paths. The release-PR stamp step runs `git add -A`, so this
check is the boundary of what that job commits. A path that was already dirty before `--write`
is not new, so a local run in a dirty tree does not red on it. `stamp_sites_self_test` drives the
check on a scratch git repository and pins its two call sites in `run_write`.

```

- [ ] **Step 7: Commit**

```bash
git add ci/version-lockstep/run.sh ci/version-lockstep/README.md
git commit -m "feat(ci): bound the paths that version-lockstep --write may change (SMA-684)" \
  -m "run_write records the dirty paths before it starts. After it finishes, every new dirty path must be a SITES path, or it exits 2 and names the paths. The release-PR stamp step commits with git add -A, so this is the boundary of what that job commits." \
  -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Task 5: The `release-pr` job runs no install and no compile

**Files:**
- Modify: `.github/workflows/release.yml` (lines 286-290 the `Checkout` comment; lines 314-317 the install step; lines 318-322 the comment above `Open or update the release PR`; lines 345-353 the comment above the stamp step; line 367 the `jq` line)

**Interfaces:**
- Produces a `release-pr` job whose steps are: the preflight, the mint, `Checkout`, `Set up proto + Moon`, `proto install release-plz`, `moon setup`, `Open or update the release PR`, the stamp step. Task 7's V18 consumes it as its real clean control.

- [ ] **Step 1: Write the failing check**

Write this file to the session scratchpad as `release_pr_check.py` (it is a one-time check, not committed):
```python
import yaml

jobs = yaml.safe_load(open(".github/workflows/release.yml"))["jobs"]
steps = jobs["release-pr"]["steps"]
names = [s.get("name") for s in steps]
assert "Install JS workspace deps" not in names, f"the install step is still there: {names}"
runs = "\n".join(str(s.get("run", "")) for s in steps)
assert "pnpm" not in runs and "napi" not in runs, "release-pr still runs pnpm or napi"
assert "| length" not in runs, "a pipe inside the quoted jq program splits under command_segments (V18)"
print("release-pr: no pnpm, no napi, no quoted jq pipe")
```
Run (from the worktree root): `uv run --locked --project py python3 <scratchpad>/release_pr_check.py; echo "rc=$?"`
Expected: `AssertionError: the install step is still there: [...]` and `rc=1`.

- [ ] **Step 2: Edit the workflow**

Replace the `Checkout` comment lines:
```yaml
          # the local git config for the WHOLE job, which handed a write-capable token to
          # four steps that have no business with it (setup-toolchain, proto install,
          # moon setup, pnpm install). A compromised action or transitive dependency in any
          # of them could have used it before release work even began.
```
with:
```yaml
          # the local git config for the WHOLE job, which handed a write-capable token to
          # the steps that have no business with it (setup-toolchain, proto install,
          # moon setup; a `pnpm install` step also ran here until SMA-684 removed it). A
          # compromised action or transitive dependency in any of them could have used it
          # before release work even began.
```

Delete these four lines (the step and the blank line after it):
```yaml
      - name: Install JS workspace deps
        if: steps.preflight.outputs.configured == 'true'
        run: pnpm --dir ts install --frozen-lockfile

```

Replace:
```yaml
      # release-plz first, ALWAYS, then --write (next step). release-plz owns the publishable
      # crates' Cargo versions and the [workspace.dependencies] requirements. --write then
      # writes the nine sites release-plz cannot reach: the three publish = false binding
      # manifests, the pyproject/package.json sites, and the dependency pin. It also
      # regenerates three derived files (SMA-685).
```
with:
```yaml
      # release-plz first, ALWAYS, then --write (next step). release-plz owns the publishable
      # crates' Cargo versions and the [workspace.dependencies] requirements. --write then
      # writes the ten sites release-plz cannot reach: the three publish = false binding
      # manifests, the pyproject/package.json sites, the dependency pin, and the version
      # literals of the napi glue index.js. It also regenerates the two lock files (SMA-685).
      # It compiles nothing (SMA-684); see the comment on the stamp step.
```

Replace:
```yaml
      # never from `git rev-parse --abbrev-ref HEAD` (which would still read "main" here and
      # push straight to it).
      - name: Stamp the lockstep manifests onto the release PR branch
```
with:
```yaml
      # never from `git rev-parse --abbrev-ref HEAD` (which would still read "main" here and
      # push straight to it).
      #
      # This step compiles nothing (SMA-684). `--write` edits text with python3 and runs
      # `cargo update -w` and `uv lock`; neither runs a build script or a build backend (spec F6,
      # F7, and repo:version-lockstep's --check asserts the static uv metadata that F7 needs).
      # The isolation level is the JOB, not the step: every step of this job can read the App
      # private key from the runner, so no step may compile or run an install script.
      # ci/actionlint/release_guard.py V18 holds this job to an allowlist of actions, command
      # words and command prefixes. V18 splits a line on `|` without reading quotes, which is
      # why PR_COUNT below uses two jq calls and not `jq '.prs | length'`. The spec's §4.1 lists
      # the tools this job still trusts:
      # docs/superpowers/specs/2026-10-04-sma-684-release-pr-stamp-token-isolation-design.md
      - name: Stamp the lockstep manifests onto the release PR branch
```

Replace:
```yaml
          PR_COUNT="$(printf '%s' "$PR_JSON" | jq '.prs | length')"
```
with:
```yaml
          PR_COUNT="$(printf '%s' "$PR_JSON" | jq '.prs' | jq 'length')"
```
The stamp step keeps `GH_TOKEN_FOR_PUSH`.

- [ ] **Step 3: Run the checks and see them pass**

Run: `uv run --locked --project py python3 <scratchpad>/release_pr_check.py; echo "rc=$?"`
Expected: `release-pr: no pnpm, no napi, no quoted jq pipe` and `rc=0`.

Run: `actionlint -shellcheck "$PWD/py/.venv/bin/shellcheck" .github/workflows/release.yml; echo "rc=$?"` (run `uv sync --locked --project py` first if `py/.venv/bin/shellcheck` does not exist)
Expected: no output and `rc=0`.

Run: `uv run --locked --project py python3 ci/actionlint/release_guard.py .github/workflows/release.yml; echo "rc=$?"`
Expected: no output and `rc=0` (V1-V17 are still clean).

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/release.yml
git commit -m "fix(ci): remove the JS install from the release-pr job (SMA-684)" \
  -m "The release-pr job no longer runs pnpm install, and version-lockstep --write no longer compiles, so no step of the job that holds the App credentials runs a lifecycle script or a build script. PR_COUNT uses two jq calls so that the V18 allowlist reads every command word correctly." \
  -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Task 6: `semver_check = false` in `rs/release-plz.toml`

**Files:**
- Modify: `rs/release-plz.toml` (lines 74-76 the `dependencies_update` comment item 2; after line 84 `dependencies_update = false`)

**Interfaces:**
- Produces `[workspace] semver_check = false`. release-plz 0.3.158's schema declares `Workspace.semver_check` as `boolean | null` with `additionalProperties: false` (MEASURED with `release-plz generate-schema` while this plan was written).

- [ ] **Step 1: Write the failing check**

Write this file to the session scratchpad as `semver_check.py`:
```python
import json
import sys
import tomllib

schema = json.load(open(sys.argv[1]))
defs = schema.get("definitions") or schema.get("$defs")
allowed = set(defs["Workspace"]["properties"])
ws = tomllib.load(open("rs/release-plz.toml", "rb"))["workspace"]
unknown = sorted(set(ws) - allowed)
assert not unknown, f"[workspace] keys that release-plz 0.3.158 rejects: {unknown}"
assert ws.get("semver_check") is False, f"semver_check is {ws.get('semver_check')!r}, expected false"
print("rs/release-plz.toml: semver_check = false, every [workspace] key is in the schema")
```
Make the schema: `cd <scratchpad> && release-plz generate-schema` (it writes `<scratchpad>/.schema/latest.json`).
Run (from the worktree root): `python3 <scratchpad>/semver_check.py <scratchpad>/.schema/latest.json; echo "rc=$?"`
Expected: `AssertionError: semver_check is None, expected false` and `rc=1`.

- [ ] **Step 2: Add the key and correct the comment**

Replace lines 74-76:
```toml
#   2. The release-pr job's stamp step builds with a write-capable App token in its environment
#      (release.yml, GH_TOKEN_FOR_PUSH). With `true`, third-party build scripts that no person
#      reviewed ran there.
```
with:
```toml
#   2. Before SMA-684 the release-pr job's stamp step compiled with a write-capable App token in
#      its environment (release.yml, GH_TOKEN_FOR_PUSH), so with `true` third-party build scripts
#      that no person reviewed ran there. Since SMA-684 that job compiles nothing, so this reason
#      is historical; reason 1 alone keeps the key `false`.
```

Directly after the line `dependencies_update = false`, insert:
```toml

# --- No cargo-semver-checks in the release-PR job (SMA-684) --------------------------------
# release-plz runs cargo-semver-checks, which builds rustdoc, for every library crate when the
# binary is on PATH and this key is not set (READ, release_plz_core updater.rs:867-876 at
# 0.3.158). No repo file installs the binary, so today it does not run only because the runner
# does not have it. This key makes that a property of the config: the release-pr job holds the
# App private key and a write-capable token, and it compiles nothing (spec F5, §5.5). If a
# semver check is wanted later, it must run in a job without credentials. The release-parity
# harness copies only `features_always_increment_minor` from this file
# (ci/release-parity/ecosystems/release-plz.sh:84-97), so this key does not reach it.
semver_check = false
```

- [ ] **Step 3: Run and see it pass**

Run: `python3 <scratchpad>/semver_check.py <scratchpad>/.schema/latest.json; echo "rc=$?"`
Expected: `rs/release-plz.toml: semver_check = false, every [workspace] key is in the schema` and `rc=0`.

Run: `uv run --locked --project py python3 ci/release-plan/release_plan.py --self-test; echo "rc=$?"` (it parses this file)
Expected: `rc=0`.

- [ ] **Step 4: Commit**

```bash
git add rs/release-plz.toml
git commit -m "fix(rs): set semver_check = false in release-plz.toml (SMA-684)" \
  -m "release-plz runs cargo-semver-checks, which builds rustdoc, when the binary is on PATH. The release-pr job holds the App credentials and must compile nothing, so the config now says so instead of relying on the runner image." \
  -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Task 7: V18 in `release_guard.py`

**Files:**
- Modify: `ci/actionlint/release_guard.py` (lines 43-54 the `UNGATED_JOBS` comment; after line 1577 the V18 code; lines 1746-1747 the `check_main` docstring; after line 1862 the call; after line 3347 three `FIXTURES` rows; after line 4199 the case table and helper; line 4248 the registration)
- Modify: `ci/actionlint/run.sh` (line 4615 the fixture floor)
- Modify: `ci/affected-graph/ci_targets.py` (lines 1091, 2874, 3124-3125: the same floor, pinned three times)
- Modify: `ci/actionlint/README.md` (line 46 check 10 row; L22; new L43 after L42)

**Interfaces:**
- Produces `ungated_job_violations(doc: dict, name: str) -> list[str]`. Every row starts `"{name}: V18: "`. Called once from `check_main`, outside the per-job loop.
- Produces `v18_segment_verdict(segment: str) -> str | None` (None = allowed).
- Produces `_sma684_v18_allowlist_bites() -> str | None` (registered in `self_test`), `_SMA684_V18_CASES` (40 rows), `_SMA684_V18_CASE_COUNT = 40`.
- `release_guard.py --fixture-count` prints `171` (168 today plus three rows). The floor becomes `170`.

- [ ] **Step 1: Write the failing tests**

At the end of `FIXTURES`, after the row that ends with `"V17: job 'publish-images-iam-console' downloads artifacts without a `name:`"),` and before the closing `]`, insert:
```python
    # SMA-684 V18 (spec V-3): each shape, end to end through check_main, in the release-pr job.
    ("SMA-684 V18 pnpm install in release-pr", "main",
     _OK_MAIN.replace("steps: [{run: echo hi}]",
                      "steps: [{run: pnpm --dir ts install --frozen-lockfile}]", 1),
     "the command word 'pnpm' is not on the allowlist"),
    ("SMA-684 V18 napi build in release-pr", "main",
     _OK_MAIN.replace("steps: [{run: echo hi}]", "steps: [{run: napi build --platform}]", 1),
     "the command word 'napi' is not on the allowlist"),
    ("SMA-684 V18 actions/setup-node in release-pr", "main",
     _OK_MAIN.replace("steps: [{run: echo hi}]", "steps: [{uses: actions/setup-node@v4}]", 1),
     "uses the action 'actions/setup-node'"),
```

After the function `_sma658_new_publish_markers_bite` (it ends with `    return None`, line 4199), insert:
```python


# SMA-684 V18. One case per step shape, driven straight at ungated_job_violations(), the way
# _SMA658_MARKER_CASES drives job_publishes(): a FIXTURES row buries one rejected shape under every
# other violation the same file produces, where this table reds on that one shape alone. Rows are
# (label, step, want_red): first the spec's rejected shapes (§5.6), then the shapes the plan's
# Review Focus added, then the clean controls.
_SMA684_V18_CASES: tuple[tuple[str, dict, bool], ...] = (
    ("pnpm install", {"run": "pnpm --dir ts install --frozen-lockfile"}, True),
    ("napi build", {"run": "napi build"}, True),
    ("node", {"run": "node x.js"}, True),
    ("cargo build", {"run": "cargo build"}, True),
    ("cargo b", {"run": "cargo b"}, True),
    ("cargo with a toolchain", {"run": "cargo +1.95.0 build"}, True),
    ("cargo publish --dry-run", {"run": "cargo publish --dry-run"}, True),
    ("uvx", {"run": "uvx foo"}, True),
    ("make", {"run": "make"}, True),
    ("bash other.sh", {"run": "bash other.sh"}, True),
    ("a chain after the allowed write", {"run": "bash ci/version-lockstep/run.sh --write && bash other.sh"}, True),
    ("cargo update -w as a direct step", {"run": "cargo update -w"}, True),
    ("uses setup-node", {"uses": "actions/setup-node@v4"}, True),
    ("uses maturin-action", {"uses": "PyO3/maturin-action@v1"}, True),
    ("uses pnpm action-setup", {"uses": "pnpm/action-setup@v4"}, True),
    ("uses a local action", {"uses": "./local-action"}, True),
    ("if as a prefix", {"run": "if cargo build; then\n  echo ok\nfi"}, True),
    ("then as a prefix", {"run": 'if [ -n "$X" ]; then cargo build; fi'}, True),
    ("a substitution after an allowed word", {"run": 'echo "$(cargo build)"'}, True),
    ("a backtick substitution after an allowed word", {"run": "echo `cargo build`"}, True),
    ("a substitution inside an assignment value", {"run": 'X="a$(cargo build)"'}, True),
    ("an assignment and then a command", {"run": "X=1 cargo build"}, True),
    ("a shell override", {"run": "echo hi", "shell": "python {0}"}, True),
    ("neither uses nor run", {"name": "empty"}, True),
    ("a docker action", {"uses": "docker://rust:1.95"}, True),
    ("a longer flag than the allowed prefix", {"run": "bash ci/version-lockstep/run.sh --writex"}, True),
    ("a pipe inside a quoted jq program", {"run": "PR_COUNT=\"$(printf '%s' \"$PR_JSON\" | jq '.prs | length')\""}, True),
    ("moon setup", {"run": "moon setup"}, False),
    ("proto install release-plz", {"run": "proto install release-plz"}, False),
    ("release-plz release-pr", {"run": "release-plz release-pr --output json"}, False),
    ("the branch push", {"run": 'git push "$AUTH_REMOTE" "HEAD:$BRANCH"'}, False),
    ("a captured release-plz call", {"run": 'OUT="$(release-plz release-pr --output json)"'}, False),
    ("a continued echo", {"run": 'echo "a" \\\n     "b (c) d"'}, False),
    ("pinned checkout", {"uses": "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1"}, False),
    ("pinned setup-toolchain", {"uses": "moonrepo/setup-toolchain@261c62cb5b0f580c7be7c8cd0f023a2e96756095"}, False),
    ("pinned app token", {"uses": "actions/create-github-app-token@bcd2ba49218906704ab6c1aa796996da409d3eb1"}, False),
    ("two jq calls", {"run": "PR_COUNT=\"$(printf '%s' \"$PR_JSON\" | jq '.prs' | jq 'length')\""}, False),
    ("a plain assignment", {"run": 'AUTH_REMOTE="https://x-access-token:${GH_TOKEN_FOR_PUSH}@github.com/${GITHUB_REPOSITORY}.git"'}, False),
    ("if, exit and fi", {"run": 'if [ "$PR_COUNT" -eq 0 ]; then\n  exit 0\nfi'}, False),
    ("a comment line with separators", {"run": "# a | b; c && d\necho ok"}, False),
)
# Deleting a row must red: the table is the only pin on each shape.
_SMA684_V18_CASE_COUNT = 40


def _sma684_v18_allowlist_bites() -> str | None:
    if len(_SMA684_V18_CASES) != _SMA684_V18_CASE_COUNT:
        return f"_SMA684_V18_CASES holds {len(_SMA684_V18_CASES)} rows, expected {_SMA684_V18_CASE_COUNT}"
    for label, step, want_red in _SMA684_V18_CASES:
        found = ungated_job_violations({"jobs": {"release-pr": {"steps": [step]}}}, "fixture")
        if bool(found) != want_red:
            return f"{label}: expected {'a V18 violation' if want_red else 'clean'}, got {found or '(clean)'}"
        if not all(": V18: " in v for v in found):
            return f"{label}: a violation does not name V18: {found}"
    # The keys that run code or change how every step of the job runs.
    for key, value in (("container", "ubuntu:24.04"), ("services", {"db": {"image": "postgres"}}),
                       ("defaults", {"run": {"shell": "python {0}"}})):
        if not ungated_job_violations({"jobs": {"release-pr": {key: value, "steps": [{"run": "echo hi"}]}}}, "fixture"):
            return f"a job-level `{key}:` on release-pr read clean"
    if not ungated_job_violations({"defaults": {"run": {"shell": "python {0}"}},
                                   "jobs": {"release-pr": {"steps": [{"run": "echo hi"}]}}}, "fixture"):
        return "a workflow-level `defaults:` read clean"
    # Scope: V18 applies to UNGATED_JOBS members only.
    if ungated_job_violations({"jobs": {"build": {"steps": [{"run": "cargo build"}]}}}, "fixture"):
        return "V18 fired on a job outside UNGATED_JOBS"
    # The real job is the clean control the spec names (§5.6). check 10 runs this from the root.
    real = Path(".github/workflows/release.yml")
    if not real.is_file():
        return f"{real} is not readable from {Path.cwd()}; run the self-test from the repository root"
    found = ungated_job_violations(load_workflow(real), real.name)
    if found:
        return f"the real release-pr job fails V18: {found}"
    return None
```

In `self_test`, after the line `        ("pr2 review i4: UNGATED_JOBS is pinned by strict equality", _ungated_jobs_pinned),`, add:
```python
        ("sma-684 V18 allowlist: every rejected shape reds, every control is clean",
         _sma684_v18_allowlist_bites),
```

- [ ] **Step 2: Run and see them fail**

Run (from the worktree root): `uv run --locked --project py python3 ci/actionlint/release_guard.py --self-test; echo "rc=$?"`
Expected: three lines `FAIL 'SMA-684 V18 …': expected a violation containing …, got: (clean)`, then `NameError: name 'ungated_job_violations' is not defined`, and `rc=1`.

- [ ] **Step 3: Implement V18**

After the function `chain_download_violations` (it ends with `    return out`, line 1577) and before `def plan_run_segments`, insert:
```python


# V18 (SMA-684). Every UNGATED_JOBS member (today only release-pr) can read the App private key:
# the runner receives every secret a job references when the job starts, so code in ANY step of
# the job can read it, not only a step with a token in its env:. So no step of such a job may
# compile, or run a build script, a proc macro, or an npm or pip lifecycle or install script
# (spec A1). This is an ALLOWLIST (spec D4): a new tool reds until someone adds it here, with a
# reason, after the job's trust in it is reviewed. The spec's §4.1 lists the tools it trusts.
UNGATED_JOB_ACTIONS = frozenset({
    "actions/create-github-app-token",
    "actions/checkout",
    "moonrepo/setup-toolchain",
})
UNGATED_JOB_COMMANDS = frozenset({"set", "echo", "printf", "[", "exit", "jq", "git"})
# Shell keywords are NOT command words. They are stripped, and the word after them is checked:
# as allowed words, `if cargo build; then` and `then cargo build` passed (plan Review Focus 2).
UNGATED_JOB_KEYWORDS = frozenset({"if", "then", "else", "elif", "fi", "!"})
UNGATED_JOB_PREFIXES = (
    "proto install release-plz",
    "moon setup",
    "release-plz release-pr",
    "bash ci/version-lockstep/run.sh --write",
)
# Job keys that run code (container:, services:) or change how every run: step runs (defaults:).
UNGATED_JOB_BANNED_KEYS = ("container", "services", "defaults")
V18_HINT = ("The release-pr job can read the App private key, so it may run only the tools on "
            "V18's allowlist (docs/superpowers/specs/2026-10-04-sma-684-release-pr-stamp-token-"
            "isolation-design.md, section 5.6).")
_V18_SUBST_RE = re.compile(r"""(?:[A-Za-z_][A-Za-z0-9_]*=)?"?\$\(""")
_V18_ASSIGN_RE = re.compile(r"""[A-Za-z_][A-Za-z0-9_]*=("[^"]*"|'[^']*'|[^\s"']*)(?=\s|$)""")


def _v18_logical_lines(run_text: str) -> list[str]:
    """The run: text as the shell reads it: a line that ends in a backslash joins the next one,
    and a full-line comment is dropped. command_segments splits before it strips a `#` comment,
    so a comment line holding `|` or `;` would otherwise yield a segment that is not a command."""
    out: list[str] = []
    buf = ""
    for raw in run_text.splitlines():
        stripped = raw.rstrip()
        if stripped.endswith("\\"):
            buf += stripped[:-1] + " "
            continue
        line = buf + raw
        buf = ""
        if not line.lstrip().startswith("#"):
            out.append(line)
    if buf and not buf.lstrip().startswith("#"):
        out.append(buf)
    return out


def v18_segment_verdict(segment: str) -> str | None:
    """None when one command segment may run in an UNGATED_JOBS member, else the reason it may
    not. Leading variable assignments and a leading `$(` (with or without its opening quote) are
    removed; then shell keywords; then the rest must start with an allowed command prefix or an
    allowed command word. A command substitution anywhere after that point, or inside an
    assignment value, is refused: it would run a command V18 does not see. Not a shell parser:
    see command_segments and README L20/L43."""
    s = segment.strip()
    while True:
        m = _V18_SUBST_RE.match(s)
        if m:
            s = s[m.end():].lstrip()
            continue
        m = _V18_ASSIGN_RE.match(s)
        if m:
            if "$(" in m.group(1) or "`" in m.group(1):
                return "a command substitution inside an assignment value"
            s = s[m.end():].lstrip()
            continue
        break
    words = s.split()
    while words and words[0] in UNGATED_JOB_KEYWORDS:
        words = words[1:]
    if not words:
        return None
    rest = " ".join(words)
    if "$(" in rest or "`" in rest:
        return "a command substitution after the command word"
    for prefix in UNGATED_JOB_PREFIXES:
        if rest == prefix or rest.startswith((prefix + " ", prefix + ")")):
            return None
    if words[0] in UNGATED_JOB_COMMANDS:
        return None
    return f"the command word {words[0]!r} is not on the allowlist"


def ungated_job_violations(doc: dict, name: str) -> list[str]:
    """V18 (SMA-684). Every step of every UNGATED_JOBS member must match the allowlist above.
    Does not use the dry-run exemption (_dry_run_exempts): a dry run still compiles."""
    out: list[str] = []
    if "defaults" in doc:
        out.append(f"{name}: V18: the workflow sets `defaults:`, which changes how every `run:` step "
                   f"of an UNGATED_JOBS member runs. {V18_HINT}")
    jobs = doc["jobs"]
    for jid in sorted(UNGATED_JOBS):
        job = jobs.get(jid)
        if not isinstance(job, dict):
            continue
        for key in UNGATED_JOB_BANNED_KEYS:
            if key in job:
                out.append(f"{name}: V18: job '{jid}' sets `{key}:`, which runs code or changes how "
                           f"its steps run. {V18_HINT}")
        for i, step in enumerate(steps_of(job, f"{name}: job '{jid}'")):
            if not isinstance(step, dict):
                out.append(f"{name}: V18: job '{jid}' step #{i + 1} is not a mapping. {V18_HINT}")
                continue
            where = f"{name}: V18: job '{jid}' step '{step.get('name') or f'#{i + 1}'}'"
            if "shell" in step:
                out.append(f"{where} sets `shell:`, so its `run:` text is not read as bash. {V18_HINT}")
            uses, run = step.get("uses"), step.get("run")
            if uses is None and run is None:
                out.append(f"{where} has neither `uses:` nor `run:`. {V18_HINT}")
            if uses is not None:
                action = str(uses).split("@", 1)[0]
                if action not in UNGATED_JOB_ACTIONS:
                    out.append(f"{where} uses the action {action!r}, which is not on the allowlist "
                               f"{sorted(UNGATED_JOB_ACTIONS)}. {V18_HINT}")
            if run is not None:
                for line in _v18_logical_lines(str(run)):
                    for seg in command_segments(line):
                        why = v18_segment_verdict(seg)
                        if why:
                            out.append(f"{where}: the segment {seg.strip()!r} is not allowed: {why}. {V18_HINT}")
    return out
```

In `check_main`, replace:
```python
    out += chain_download_violations(jobs, name)
    return out
```
with:
```python
    out += chain_download_violations(jobs, name)
    # SMA-684. V18: once, outside the per-job loop, like V8 above. The loop's `continue` for an
    # UNGATED_JOBS member would otherwise skip it for exactly the job it exists for.
    out += ungated_job_violations(doc, name)
    return out
```
and replace the docstring lines:
```python
    """V1-V5, V7, V8a-c, V8e, V9 and V13-V17 over the release workflow (V16a-c runs from
    main()). V16e is one of the V13-V17 group. V6 applies to CALLED workflows (see
```
with:
```python
    """V1-V5, V7, V8a-c, V8e, V9 and V13-V18 over the release workflow (V16a-c runs from
    main()). V16e is one of the V13-V18 group. V6 applies to CALLED workflows (see
```

At the end of the `UNGATED_JOBS` comment, directly before `UNGATED_JOBS = frozenset({"release-pr"})`, insert:
```python
#
# SMA-684: V18 (ungated_job_violations) also holds every member to an allowlist of actions,
# command words and command prefixes, because such a job runs with the App private key on the
# runner. A new member inherits V18 too.
```

- [ ] **Step 4: Run and see it pass**

Run: `uv run --locked --project py python3 ci/actionlint/release_guard.py --self-test; echo "rc=$?"` — expected no output, `rc=0`.
Run: `uv run --locked --project py python3 ci/actionlint/release_guard.py .github/workflows/release.yml; echo "rc=$?"` — expected no output, `rc=0`.
Run: `uv run --locked --project py ruff check --config py/pyproject.toml ci/actionlint/release_guard.py; echo "rc=$?"` — expected `All checks passed!`, `rc=0`.

- [ ] **Step 5: Deletion check: remove the call site**

With Edit, delete the line `    out += ungated_job_violations(doc, name)` from `check_main`.
Run the self-test. Expected: three `FAIL 'SMA-684 V18 …': expected a violation containing …, got: (clean)` lines, `rc=1`. Restore with Edit; re-run; expected `rc=0`.

- [ ] **Step 6: V-3 on the real workflow**

For each of the three shapes below, one at a time: with Edit, insert the step into the `release-pr` job of `.github/workflows/release.yml` directly before `      - name: Install pinned release-plz CLI`; run `uv run --locked --project py python3 ci/actionlint/release_guard.py .github/workflows/release.yml; echo "rc=$?"`; check the output; delete the step with Edit.
```yaml
      - name: V3 probe
        run: pnpm --dir ts install
```
Expected: `release.yml: V18: job 'release-pr' step 'V3 probe': the segment 'pnpm --dir ts install' is not allowed: the command word 'pnpm' is not on the allowlist. …` and `rc=1`.
```yaml
      - name: V3 probe
        run: napi build
```
Expected: the same line with `'napi build'` and `'napi'`, and `rc=1`.
```yaml
      - name: V3 probe
        uses: actions/setup-node@v4
```
Expected: `release.yml: V18: job 'release-pr' step 'V3 probe' uses the action 'actions/setup-node', which is not on the allowlist …` and `rc=1`.
After the last removal, re-run; expected no output and `rc=0`. Confirm `git diff --stat .github/workflows/release.yml` shows no change.

- [ ] **Step 7: Re-set the fixture floor**

Run: `uv run --locked --project py python3 ci/actionlint/release_guard.py --fixture-count` — expected `171`.

In `ci/actionlint/run.sh`, replace line 4615:
```bash
  [ "$n" -ge 162 ] || infra "check 10: release_guard.py reports $n fixtures, expected at least 162"
```
with:
```bash
  [ "$n" -ge 170 ] || infra "check 10: release_guard.py reports $n fixtures, expected at least 170"
```
In `ci/affected-graph/ci_targets.py`, make the same change in all three pinned copies (its own comment at line 1089 says so):
- line 1091: `'[ "$n" -ge 162 ] || infra "check 10: release_guard.py reports $n fixtures, expected at least 162"',` becomes `'[ "$n" -ge 170 ] || infra "check 10: release_guard.py reports $n fixtures, expected at least 170"',`
- line 2874: `'  [ "$n" -ge 162 ] || infra "check 10: release_guard.py reports $n fixtures, expected at least 162"\n'` becomes the same text with `170` twice.
- lines 3124-3125: `'  [ "$n" -ge 162 ] || infra "check 10: release_guard.py reports $n fixtures, '` and `'expected at least 162"\n',` become the same text with `170`.

Run: `grep -n 'ge 16[0-9]\|least 16[0-9]' ci/actionlint/run.sh ci/affected-graph/ci_targets.py; echo "rc=$?"` — expected no output, `rc=1`.
Run: `python3 ci/affected-graph/ci_targets.py --self-test; echo "rc=$?"` — expected `rc=0`.
Run: `python3 ci/affected-graph/ci_targets.py; echo "rc=$?"` — expected `rc=0` (the whole-line pin matches the new `run.sh` line).

- [ ] **Step 8: Update the actionlint README**

In `ci/actionlint/README.md`, check 10 row (line 46): replace `reports at least 162 fixtures` with `reports at least 170 fixtures`, and after the sentence that ends `rather than line-oriented text scanning.` add: ` Since SMA-684 the verdict includes V18: every step of an `UNGATED_JOBS` member (`release-pr`) must match an allowlist of actions, command words and command prefixes, because that job can read the App private key (L43).`

In L22, replace `It is shared by ALL SIXTEEN registered helpers, not by two.` with `It is shared by ALL TWENTY-THREE registered helpers, not by two (the prose said sixteen while the tuple held twenty-two; SMA-684 counted it again when it added `_sma684_v18_allowlist_bites`).`

After the whole L42 entry (the last `**L42 …**` paragraph and its list) and before `## Cost`, insert:
```markdown
**L43 — V18 is an allowlist over command words, not a shell parser (SMA-684).** V18 holds every
`UNGATED_JOBS` member (today only `release-pr`) to three actions and a short list of command
words and exact command prefixes, because every step of that job can read the App private key
from the runner. Shell keywords (`if`, `then`, `else`, `elif`, `fi`, `!`) are stripped and the
next word is checked; a command substitution after the command word or inside an assignment
value is refused; `shell:`, `container:`, `services:` and `defaults:` are refused. Four residuals.
`command_segments` does not parse quotes (L20), so a command inside a quoted string is not seen,
and a separator inside quotes splits a segment: the real job's `jq '.prs | length'` read as a
command word `length')"`, so SMA-684 rewrote it as two `jq` calls. V18 cannot see inside an
allowed program: `bash ci/version-lockstep/run.sh --write` is guarded by that script's own
self-tests (the napi-glue writer, the `run_write` pins and the changed-path check), and
`moon setup` by one measurement (spec F8, run 37113088606), not by a gate. An allowed word can
still do harm that a person writes into the workflow itself, for example `echo` with a redirect
into `.git/hooks`; a reviewer sees that in the diff, V18 does not. And the three allowed actions
and the tools in the spec's §4.1 can read the key; V18 removes the compile and the install, not
that trust.

```

- [ ] **Step 9: Commit**

```bash
git add ci/actionlint/release_guard.py ci/actionlint/run.sh ci/affected-graph/ci_targets.py ci/actionlint/README.md
git commit -m "feat(ci): hold the release-pr job to a step allowlist in release_guard (SMA-684)" \
  -m "The new V18 check allows only three actions and a short list of command words and prefixes in every UNGATED_JOBS job, because that job can read the App private key. A case table drives each rejected shape and each clean control, three fixture rows drive it end to end, and the real job is a clean control. The fixture floor moves to 170 in run.sh and in its three ci_targets.py pins." \
  -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Task 8: Stale-text sweep

**Files:**
- Modify: `ci/affected-graph/cargo_moon_parity.py` (line 460, waiver prose)
- Modify: any other file that the Step 1 search finds, outside `docs/superpowers/`

**Interfaces:** none (prose only).

- [ ] **Step 1: Search for the stale phrases**

Run:
```bash
grep -rn -e 'nine version sites' -e 'nine sites' -e 'three derived files' -e '26 `bindingPackageVersion' -e '26 committed' -e 'napi build failed' -e 'SELF_TEST_COUNT=4' -e 'filter @paigasus/kernel exec' \
  --exclude-dir=node_modules --exclude-dir=target --exclude-dir=.venv --exclude-dir=superpowers . ; echo "rc=$?"
```
Expected before the edit: at least `ci/affected-graph/cargo_moon_parity.py:460:` (`"PURPOSE is to regenerate the lock after writing the nine version sites it owns "`), and `rc=0`.

- [ ] **Step 2: Fix every hit**

In `ci/affected-graph/cargo_moon_parity.py`, replace `"PURPOSE is to regenerate the lock after writing the nine version sites it owns "` with `"PURPOSE is to regenerate the lock after writing the ten version sites it owns "`. Do not change the dict KEY strings above it (`("ci/version-lockstep/run.sh", "cargo update -w --offline >/dev/null 2>")` and the two others); they must match `run.sh` byte for byte.
Fix any other hit in the same way: state the SMA-684 fact (ten sites, two lock files, 27 guards, the text writer, `SELF_TEST_COUNT=6`).

- [ ] **Step 3: Re-run the search**

Run the Step 1 command. Expected: no output, `rc=1`.
Run: `python3 ci/affected-graph/cargo_moon_parity.py --self-test; echo "rc=$?"` — expected `rc=0`.
Run: `uv run --locked --project py ruff check --config py/pyproject.toml ci/affected-graph/cargo_moon_parity.py ci/affected-graph/ci_targets.py; echo "rc=$?"` — expected `All checks passed!`, `rc=0`.

- [ ] **Step 4: Commit**

```bash
git add ci/affected-graph/cargo_moon_parity.py
git commit -m "docs(ci): remove stale napi build text from the version-lockstep notes (SMA-684)" \
  -m "The A8 waiver prose and the other notes now say that --write owns ten sites and regenerates two lock files." \
  -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```
(Add to `git add` every other file that Step 2 changed.)

---

## Task 9: Verification (spec V-1, V-4, V-5) and the follow-up

**Files:** none in the repo. Scratch files only, under the session scratchpad.

**Interfaces:**
- Consumes the committed branch head (Tasks 1-8).
- Produces the V-1 record for the PR body: the commands, the result and the `@napi-rs/cli` version.

- [ ] **Step 1: V-1, the equivalence proof**

Write this script to `<scratchpad>/v1.sh` (replace `<scratchpad>` with the real path) and run `/bin/bash <scratchpad>/v1.sh`. It clones the committed branch head into the scratchpad, so the worktree stays untouched.
```bash
set -euo pipefail
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
W=/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-684-stamp-token
S=<scratchpad>/v1
rm -rf "$S"
git clone -q --branch feature/sma-684-stamp-step-token-isolation "$W" "$S"
cd "$S"
git config maintenance.auto false; git config gc.auto 0
pnpm -C ts install --frozen-lockfile >/dev/null
# A length-changing bump, the shape release-plz writes: the head crate and the workspace requirement.
python3 - <<'PY'
import re
for p, pat, rep in (
    ("rs/crates/libs/paigasus-kernel/Cargo.toml", r'(?m)^version = "0\.2\.0"$', 'version = "0.10.0"'),
    ("rs/Cargo.toml", r'(?m)^(paigasus-kernel = \{[^\n]*version = ")0\.2\.0(")', r'\g<1>0.10.0\g<2>'),
):
    s = open(p).read()
    s, n = re.subn(pat, rep, s, count=1)
    assert n == 1, p
    open(p, "w").write(s)
PY
/opt/homebrew/bin/bash ci/version-lockstep/run.sh --write
/opt/homebrew/bin/bash ci/version-lockstep/run.sh
cd ts/packages/paigasus-kernel
echo "napi-rs cli: $(pnpm exec napi --version)"
pnpm exec napi build --platform --cwd ../../../rs/crates/bindings/paigasus-node-bindings --js index.fresh.js --dts index.fresh.d.ts --no-dts-cache >/dev/null
cd ../../../rs/crates/bindings/paigasus-node-bindings
cmp index.js index.fresh.js
cmp index.d.ts index.fresh.d.ts
echo "V-1 EQUAL: index.js == index.fresh.js and index.d.ts == index.fresh.d.ts at 0.10.0"
```
Expected output, in order: `version-lockstep: wrote 9 site(s)` (the three kernel binding manifests, the two kernel pyproject files, the two package.json files, the dependency pin and the napi glue; the proto sites are already current), `uv workspace: every local package declares static metadata`, `== all 20 version-lockstep sites agree ==`, `napi-rs cli: 3.10.4` (record the exact string), and `V-1 EQUAL: …`. Any `cmp` difference stops the script with a non-zero exit: then stop and report; spec §8 (fallback C) applies.
If the pipe probe (Task 1 Step 1) printed `512`, the `--write` and `--check` lines can fail with `tar: Write error` or hang; then re-take the probe later or run the two `run.sh` lines of the script in the `python:3.12-bookworm` container, and run the `napi build` part on the host.
Record in the PR body: the script, the printed lines, and the `@napi-rs/cli` version.

- [ ] **Step 2: V-4, the two gates under their own bash**

Run each command as its own Bash call, from the worktree root:
- `/opt/homebrew/bin/bash ci/version-lockstep/run.sh --self-test; echo "rc=$?"` — expected `(6 tables)`, `rc=0`.
- `/opt/homebrew/bin/bash ci/version-lockstep/run.sh --negative-control; echo "rc=$?"` — expected the three control lines, `rc=0`.
- `/opt/homebrew/bin/bash ci/version-lockstep/run.sh; echo "rc=$?"` — expected `== all 20 version-lockstep sites agree ==`, `rc=0`.
- `/opt/homebrew/bin/bash ci/actionlint/run.sh; echo "rc=$?"` — first read its preflight line `pipe capacity N bytes`. With `N` of 8192 or more, expected `rc=0` and no `FAIL` row. With `N` = 512 it exits rc 2 with a `small` message: that is a host condition, not a result. Then record "no local actionlint verdict" and rely on CI; the V18 part is already proven by Task 7 Steps 4-6, which do not use bash.

- [ ] **Step 3: V-5, the full gate graph**

Run the command between the `ci-targets` markers of the root `CLAUDE.md` (`moon ci :build :test … --base origin/main --include-relations`) after `git fetch origin main`. Moon resolves `bash` through `PATH`; follow the memory note "affected-smoke hang: shim bash, don't prepend /bin" for `repo:affected-smoke`.
Expected: every task passes. Then re-run directly the gates that need the other bash, and read those results instead of the `moon ci` verdict for them:
- `/bin/bash ci/affected-graph/run.sh; echo "rc=$?"` — expected `rc=0`.
- `/opt/homebrew/bin/bash ci/version-lockstep/run.sh` (Step 2).
- `/opt/homebrew/bin/bash ci/ruff/run.sh; echo "rc=$?"`, `/opt/homebrew/bin/bash ci/publish-metadata/run.sh; echo "rc=$?"`, `/opt/homebrew/bin/bash ci/next-public/run.sh; echo "rc=$?"` — expected `rc=0` each.
- `repo:actionlint` per Step 2.
If a task fails, capture its `.moon/cache/states/<project>/<task>/` directory before any re-run (root `CLAUDE.md`, the diagnosis procedure, Step 0).

- [ ] **Step 4: Note V-6 (after the merge)**

Write in the PR body: V-6 is the only full proof. The first release PR after the merge must be stamped, pushed and green, including `paigasus-kernel-ts:test` and `repo:version-lockstep`. The job skips green without the App secrets, so no PR run proves the job.

- [ ] **Step 5: The follow-up issue (coordinator, not code)**

The coordinator files one Linear issue (spec §5.9) with project, milestone, priority and labels (`area:*` plus Bug/Improvement/Feature) set at creation. Title: "release job: cargo publish compiles while CARGO_REGISTRY_TOKEN and the App credentials are on the runner". It also records spec §10 Q1 (every tool in §4.1 can read `PAIGASUS_BOT_PRIVATE_KEY`; check that the App installation is limited to this repository with the minimum permissions) if Sven did not answer it.
