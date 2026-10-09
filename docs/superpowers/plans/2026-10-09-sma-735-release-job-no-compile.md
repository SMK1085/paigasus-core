# SMA-735 No Compile in the Release Job Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The `release` job in `.github/workflows/release.yml` compiles nothing while it holds the crates.io token and the App credentials. A new job without credentials, `verify-crates`, builds every publishable crate before `approve-release`, and the release guard reds when a change undoes either half.

**Architecture:** `ci/publish-metadata/run.sh` gets a mode `--verify-publish-groups` that runs only Check 0 and Check 2, with the publishable-set logic moved into a new module `ci/publish-metadata/publishable.py` so that the mode and `metadata_checks` share one copy. `ci/actionlint/release_guard.py` turns the V18 engine into a table of `StepAllowlist` rows (V18 unchanged), then adds a V20 row plus job rules for `verify-crates` and a V19 row plus liveness for `release`. `release.yml` gets the `verify-crates` job, `approve-release` and `release` need it, and the `Release` step runs `release-plz release --output json --no-verify`.

**Tech Stack:** bash 4+ (`ci/publish-metadata/run.sh`, `declare -A`), python3 stdlib (`publishable.py`, the heredocs), Python 3.12 + PyYAML through `uv run --locked --project py` (`release_guard.py`), GitHub Actions YAML, release-plz 0.3.158 (`.prototools:18`), cargo 1.95.0 (`rs/rust-toolchain.toml`), Moon 2.5.3, Docker `ubuntu:24.04` for the runner measurement.

**Spec:** `docs/superpowers/specs/2026-10-09-sma-735-release-job-no-compile-design.md` (GATE 1 approved by Sven on 2026-10-09). Read it before each task. Decisions on its open questions: Q1 stays open and gets a manual check in the runbook (Task 8). Q2: the residuals stay as §7 writes them. Q3: V-6 does not run without a separate consent from Sven (Task 10).

## Global Constraints

- Work only in `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-735-release-job-isolation`, branch `feature/sma-735-release-job-isolation`. Check `git branch --show-current` before each commit. Do not `cd` to the main checkout.
- `$SCRATCH` in this plan is the session scratchpad directory that the coordinator gives you. Put every temporary file there, never in the worktree.
- Every new source file opens with an SPDX header: `# SPDX-License-Identifier: Apache-2.0` (Python and bash), after the shebang if there is one.
- `ci/publish-metadata/run.sh` needs bash 4+ (`declare -A`): run it with `/opt/homebrew/bin/bash`, never `/bin/bash` 3.2.
- `repo:actionlint`'s full gate (`ci/actionlint/run.sh`) needs `/opt/homebrew/bin/bash` (bash 5) and a pipe preflight that passes (8192 bytes or more).
- `repo:affected-smoke` (`ci/affected-graph/run.sh`) needs `/bin/bash` 3.2. Through Moon, use a bash-only shim directory: `mkdir -p "$SCRATCH/bashshim" && ln -sf /bin/bash "$SCRATCH/bashshim/bash"`, then put `$SCRATCH/bashshim` first on `PATH`.
- Before any proto tool (`moon`, `uv`, `release-plz`, `cargo` through a shim): `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- Every gate script or measurement script that captures `proto` output exports `PROTO_REPORTER=text` at its top.
- Conventional commits with scope `ci`: `feat(ci)`, `fix(ci)`, `test(ci)`, `docs(ci)`. Header at most 100 characters, ending in `(SMA-735)`. Body: only the line `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. No body line that starts with `#NNN` or looks like `token: value` (for example `V19: ...`).
- Never use `git commit --no-verify`, `--no-gpg-sign`, `git commit --amend`, `git reset`, `git rebase` or a bare `git stash`. One new commit per task. If commitlint fails, report it; do not bypass it. If a commit fails with "failed to fill whole buffer", 1Password is locked: stop and ask.
- Do not install host software (no `brew`, no `pip install --user`, no `proto upgrade`). Installs inside a Docker container are allowed. Run every command in the foreground.
- If the worktree sandbox refuses a command (it refuses many compound `git` or `.github` commands), write the commands into a script under `$SCRATCH` and run `/bin/bash <script>`.
- The `FIXTURES` floor stays `[ "$n" -ge 170 ]` in `ci/actionlint/run.sh:4619` and in `ci/affected-graph/ci_targets.py:1116, 2961, 3215`. Do not touch those four lines (spec D7). V19 and V20 add no `FIXTURES` row.
- `_SMA684_V18_CASES` and `_SMA684_V18_CASE_COUNT = 147` stay unchanged, and `_sma684_v18_allowlist_bites` must keep passing.
- Before the push, run the full gate graph command from the root `CLAUDE.md` section "Before you push: the full gate graph" (Task 9). Copy the command from that file at run time. Do not copy or quote the two HTML marker comments around it anywhere.
- Do not write the file name of Moon's CI report JSON (the file that the root `CLAUDE.md` diagnosis procedure reads) in any file of this change: `repo:actionlint` check 12 then requires a marker on that file.
- `ci/actionlint/run.sh` check 13 bans a pipe into `grep -q`, `grep -m`, `head` or `awk … exit` in every tracked `*.sh`. Read a variable with `grep -q … < <(printf '%s\n' "$var")`. Do not add a here-string (`<<<`).
- All prose that this plan adds to a document, a comment or a message is in ASD-STE100 Simplified Technical English: short sentences, active voice, one idea per sentence, no idiom.
- Prove each new check by mutation (see "Mutation helper" below): commit first, then mutate with the helper, watch the test red, restore with the helper, and confirm `git diff --exit-code <file>`. Never restore with `git checkout --`.

## Review Focus

1. **`--no-verify` is a word in the text but not an argument that release-plz gets.** A quoted string (`--output 'json --no-verify x'`), a word after `--`, a `$VAR` next to it, or a comment line. A person expects V19 to red, because cargo then builds the crate with the credentials on the runner. Pinned in Task 6 (`_SMA735_V19_CASES` rows "--no-verify inside a single-quoted string", "… double-quoted string", "--no-verify after --", "a variable after --no-verify", "--no-verify only in a comment").
2. **A step key on the `Release` step changes what runs or hides a failure:** `if:`, `continue-on-error:`, `shell:`, or an env name such as `RUSTC_WRAPPER`. The spec lists these for `verify-crates` only. Pinned in Task 6 (V19 rows "a step if", "a step continue-on-error", "a step shell", "an env name outside the set") and Task 5 (V20 rows "a step if false" to "a step continue-on-error").
3. **A second publish job that V19 does not see.** GitHub reads an action path without case, so `Rust-Lang/crates-io-auth-action` is the same action. A person expects V19 liveness to red for a renamed job and for any other job that uses that action or runs `release-plz release`. Pinned in Task 6 (rows "the job renamed", "a second job with crates-io-auth-action", "a second job with an upper-case crates-io-auth-action path", "a second job that runs release-plz release").
4. **A publishable crate that is not in `EXPECTED_PUBLISHABLE`.** `publish_groups` reads only the listed names, so such a crate would skip the verify build and still be published by release-plz. A person expects the mode to exit 1 before any `cargo publish`. Pinned in Task 2 (stub mode `extra-crate`: rc 1 and no `publish` line in the stub log; mutation M4).
5. **A stale category snapshot stops a release.** The snapshot load fails after 90 days. The mode must never load it. Pinned in Task 2 (the structural row "--verify-publish-groups does not read the category snapshot").

---

## File Structure

| File | Task | Change |
|---|---|---|
| `docs/superpowers/specs/2026-10-09-sma-735-release-job-no-compile-design.md` | 1, 3 | append §14 "Measurement record" with the M2 and M1 results |
| `ci/publish-metadata/publishable.py` (new) | 2 | `is_publishable`, `publishable_packages` (Check 0), and a CLI that prints `<name>\t<dir>` |
| `ci/publish-metadata/run.sh` | 2 | `metadata_checks` imports the module; new `publishable_set`, `verify_publish_groups`; dispatch arm and usage; negative-control rows with a `cargo` stub |
| `moon.yml` (root) | 2 | `repo:publish-metadata` input `ci/publish-metadata/publishable.py` |
| `ci/affected-graph/ci_targets.py` | 2 | the same input in `SELF_TASK_EXPECTED_GLOBS["publish-metadata"]` |
| `ci/actionlint/release_guard.py` | 4, 5, 6, 7 | `StepAllowlist`, `segment_verdict`, `step_config_violations`, `allowlist_job_violations`, `V18_ROW`; V20 (`V20_ROW`, `verify_job_violations`); V19 (`V19_ROW`, `release_job_violations`); test tables and helpers; `check_main` wiring and docstring |
| `.github/workflows/release.yml` | 7 | new job `verify-crates`; `needs:` of `approve-release` and `release`; `--no-verify`; comments |
| `.github/workflows/prebuild.yml` | 8 | the comment that quotes `release`'s `needs:` |
| `.github/CLAUDE.md`, `ci/actionlint/README.md`, `ci/publish-metadata/README.md`, `docs/ops/RUNBOOK-release-activation.md` | 8 | spec §5.7 |

Not changed: `ci/actionlint/run.sh`, the fixture floor lines in `ci_targets.py`, `rs/release-plz.toml` (spec D3: the CLI flag, not the `publish_no_verify` key).

## Mutation helper

Create this file once, before the first mutation step (Task 2 Step 9). Every later task that mutates checks `[ -f "$SCRATCH/mutate.py" ]` and creates it from this section if it is absent.

`$SCRATCH/mutate.py`:
```python
# SPDX-License-Identifier: Apache-2.0
"""SMA-735 plan helper. `replace` swaps ONE anchored line for a marked mutation that carries the
original line in base64. `restore` puts every original line back. No git command is involved."""
import base64
import sys

TAG = "SMA735-MUTATION-WAS:"


def main(argv: list[str]) -> int:
    if len(argv) == 2 and argv[0] == "restore":
        path = argv[1]
        lines = open(path, encoding="utf-8").read().split("\n")
        out = [base64.b64decode(l.split(TAG, 1)[1].strip()).decode() if TAG in l else l for l in lines]
        open(path, "w", encoding="utf-8").write("\n".join(out))
        return 0
    if len(argv) == 4 and argv[0] == "replace":
        _, path, anchor, new = argv
        lines = open(path, encoding="utf-8").read().split("\n")
        hits = [i for i, l in enumerate(lines) if anchor in l]
        if len(hits) != 1:
            print(f"anchor {anchor!r} matched {len(hits)} lines in {path}, expected 1", file=sys.stderr)
            return 2
        i = hits[0]
        indent = lines[i][: len(lines[i]) - len(lines[i].lstrip())]
        lines[i] = f"{indent}{new}  # {TAG} {base64.b64encode(lines[i].encode()).decode()}"
        open(path, "w", encoding="utf-8").write("\n".join(lines))
        return 0
    print("usage: mutate.py replace <file> <anchor> <new-line> | restore <file>", file=sys.stderr)
    return 2


raise SystemExit(main(sys.argv[1:]))
```

Use: `python3 "$SCRATCH/mutate.py" replace <file> '<anchor>' '<new line without indentation>'`, then the test, then `python3 "$SCRATCH/mutate.py" restore <file> && git diff --exit-code <file>`.

---

### Task 1: M2 — measure that `release-plz release --no-verify` passes the flag to cargo and runs no build script

This is measurement M2 of spec §10. Nothing in the repository changes except the record in the spec.

**Files:**
- Create (scratch only): `$SCRATCH/m2/measure.sh`
- Modify: `docs/superpowers/specs/2026-10-09-sma-735-release-job-no-compile-design.md` (append §14)

**Interfaces:**
- Consumes: the proto-pinned release-plz 0.3.158; the release-plz source facts F1, F3 and F12 (spec §4). The run uses `--forge gitea --repo-url http://127.0.0.1:18735/o/r`: `associated_prs` gets a 404 from the local stub, returns an empty list (`release_plz_core/src/git/forge.rs:795-815`), and `release_always` (default `true`, `release.rs:71`) makes release-plz go on to `cargo publish`.
- Produces: the M2 result in spec §14. Task 8 quotes it ("MEASURED, spec M2" or "READ, spec F1").

**Stop condition.** STOP the plan and report to the coordinator when the `--no-verify` run writes the sentinel, or when its debug log shows a `cargo publish` argv without `--no-verify` while the control run works. Then spec D3 is false and the design must change. If the control run does not write the sentinel, the fixture is broken: fix the fixture and measure again (this is not a stop). If release-plz cannot reach `cargo publish` in any run (for example the sandbox refuses the local port or the crates.io lookup), record the exact error, fall back to F1 (READ) as spec §10 allows, and tell the coordinator.

- [ ] **Step 1: Write the measurement script**

`$SCRATCH/m2/measure.sh`:
```bash
#!/bin/bash
# SPDX-License-Identifier: Apache-2.0
# SMA-735 M2: does `release-plz release --no-verify` pass --no-verify to cargo publish and run no
# build.rs? Control: the same run without --no-verify must run build.rs. Run C: a package key
# publish_no_verify = false must not beat the CLI flag (spec F1, config.rs:100-138).
set -u
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
export PROTO_REPORTER=text
D="$SCRATCH/m2"
RP="$(proto bin release-plz | tail -n1)"
[ -x "$RP" ] || { echo "FATAL: no release-plz binary ($RP)"; exit 2; }
"$RP" --version
cargo --version

rm -rf "$D/ws" "$D/sentinels" "$D/target"
mkdir -p "$D/ws/src" "$D/sentinels"
NAME="sma735-m2-probe-$(date +%s)"
cd "$D/ws" || exit 2
git init -q .
git config maintenance.auto false
git config gc.auto 0
git config commit.gpgsign false
git config tag.gpgsign false
cat > Cargo.toml <<EOF
[package]
name = "$NAME"
version = "0.1.0"
edition = "2024"
license = "Apache-2.0"
description = "SMA-735 M2 probe, never published"
build = "build.rs"
EOF
cat > build.rs <<EOF
fn main() { std::fs::write("$D/sentinels/build-rs-ran", "x").unwrap(); }
EOF
echo 'pub fn f() {}' > src/lib.rs
printf 'target/\n' > .gitignore
CARGO_TARGET_DIR="$D/target" cargo generate-lockfile
rm -rf "$D/target" "$D/sentinels/"*
GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 git add -A
GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 \
  git -c user.email=m2@example.invalid -c user.name=m2 commit -qm "feat: probe"

# A local forge stub: every API path is a 404, which release-plz reads as "no associated PR".
python3 -m http.server 18735 --bind 127.0.0.1 --directory "$D/sentinels" >"$D/http.log" 2>&1 &
HTTP_PID=$!
trap 'kill "$HTTP_PID" 2>/dev/null' EXIT
sleep 1

run() { # $1 label, rest = extra release-plz flags
  local label="$1"; shift
  rm -rf "$D/target"; rm -f "$D/sentinels/build-rs-ran"
  ( cd "$D/ws" && env -i HOME="$HOME" PATH="$PATH" TMPDIR="${TMPDIR:-/tmp}" \
      CARGO_TARGET_DIR="$D/target" RELEASE_PLZ_LOG=debug \
      GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 \
      "$RP" release --dry-run --forge gitea --repo-url "http://127.0.0.1:18735/o/r" \
        --git-token not-a-real-token "$@" ) >"$D/$label.log" 2>&1
  echo "== $label rc=$?"
  if [ -e "$D/sentinels/build-rs-ran" ]; then echo "sentinel: build.rs RAN"; else echo "sentinel: absent"; fi
  grep -o 'Run `cargo publish[^`]*`' "$D/$label.log" || echo "no cargo publish argv in $D/$label.log"
}

run A-no-verify --no-verify
run B-control
printf '[[package]]\nname = "%s"\npublish_no_verify = false\n' "$NAME" > release-plz.toml
GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 git add release-plz.toml
GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 \
  git -c user.email=m2@example.invalid -c user.name=m2 commit -qm "chore: config"
run C-key-false-cli-flag --no-verify
```

- [ ] **Step 2: Run it**

Run: `SCRATCH="$SCRATCH" /bin/bash "$SCRATCH/m2/measure.sh"`
Expected:
```
== A-no-verify rc=0
sentinel: absent
Run `cargo publish --color always --manifest-path <…>/Cargo.toml --package sma735-m2-probe-<n> --dry-run --no-verify`
== B-control rc=0
sentinel: build.rs RAN
Run `cargo publish --color always --manifest-path <…>/Cargo.toml --package sma735-m2-probe-<n> --dry-run`
== C-key-false-cli-flag rc=0
sentinel: absent
Run `cargo publish … --dry-run --no-verify`
```
Read `$D/A-no-verify.log` when a line differs. Apply the stop condition above.

- [ ] **Step 3: Record the result in the spec**

Append to the end of the spec file (after §13). Write the measured values in place of the bracketed words, copied from Step 2's output:
```markdown
## 14. Measurement record

### M2 — release-plz 0.3.158 with `--no-verify` (2026-10-09)

A one-crate fixture with a sentinel in `build.rs`, a local forge stub that answers 404, and
`RELEASE_PLZ_LOG=debug release-plz release --dry-run`. Script: the plan, Task 1.

| Run | rc | `build.rs` ran | `cargo publish` argv in the debug log |
|---|---|---|---|
| A, `--no-verify` | [rc] | [no/yes] | [argv] |
| B, control | [rc] | [no/yes] | [argv] |
| C, `publish_no_verify = false` key and `--no-verify` flag | [rc] | [no/yes] | [argv] |

Result: [one sentence: the flag reaches cargo and no build script runs, or what failed].
```
If the fallback applied, write the exact error and the sentence "M2 fell back to F1 (READ)".

- [ ] **Step 4: Commit**

```bash
git add docs/superpowers/specs/2026-10-09-sma-735-release-job-no-compile-design.md
git commit -m "docs(ci): record the M2 release-plz measurement (SMA-735)" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: `run.sh --verify-publish-groups`, one publishable-set copy, and a `cargo` stub control

**Files:**
- Create: `ci/publish-metadata/publishable.py`
- Modify: `ci/publish-metadata/run.sh` (lines 186-212 Check 0 in `metadata_checks`; new functions after line 929, after `assert_check2_covered_everything`; negative-control rows before line 1211 and before line 1477; dispatch lines 1903-1917)
- Modify: `moon.yml` (`repo:publish-metadata` `inputs`, after `'ci/publish-metadata/categories.py'`)
- Modify: `ci/affected-graph/ci_targets.py` (`SELF_TASK_EXPECTED_GLOBS["publish-metadata"]`, after `"ci/publish-metadata/categories.py",`)

**Interfaces:**
- Consumes: `publish_groups`, `check_publish_group`, `assert_check2_covered_everything`, `PKG_DIR`, `CHECK2_INVOKED`, `EXPECTED_PUBLISHABLE`, `RS_DIR`, `REPO_ROOT` (all in `run.sh`).
- Produces: `ci/publish-metadata/publishable.py` with `is_publishable(pkg: dict) -> bool`, `publishable_packages(meta: dict, expected: list[str]) -> dict[str, dict]` (exits 2 on an empty set, 1 on a set that is not `expected`), and the CLI `python3 publishable.py <metadata.json> <expected-csv>` (prints `<name>\t<manifest-dir>` per crate, sorted; exits 2 on a bad invocation or unreadable JSON).
- Produces: bash `publishable_set <metadata.json> <expected-csv>` (the CLI above) and `verify_publish_groups` (no argument; 0 / 1 / 2).
- Produces: the dispatch `run.sh --verify-publish-groups`. Task 3 and Task 7 call exactly `bash ci/publish-metadata/run.sh --verify-publish-groups`.

- [ ] **Step 1: Probe the host pipe**

Run:
```bash
python3 -c 'import os
r, w = os.pipe(); os.set_blocking(w, False); n = 0
try:
    while True: n += os.write(w, b"x")
except BlockingIOError: print(n)'
```
Expected: `65536` or another value of 8192 or more. If it prints `512`, run every `run.sh` command of this task in Docker instead (one `docker` command per Bash call; the worktree and the main `.git` must be mounted at their own absolute paths, because the worktree's `.git` file names that path):
```bash
docker run --rm \
  -v /Users/smaschek/dev/paigasus/paigasus-core/.git:/Users/smaschek/dev/paigasus/paigasus-core/.git \
  -v "$PWD":"$PWD" -v "$PWD/rs/target" -w "$PWD" rust:1.95.0-bookworm \
  bash -c 'git config --global --add safe.directory "*"; command -v python3 >/dev/null || { apt-get update -qq && apt-get install -y -qq python3 >/dev/null; }; bash ci/publish-metadata/run.sh --negative-control'
```

- [ ] **Step 2: Write the failing negative-control rows**

In `ci/publish-metadata/run.sh`, insert before the line `  # Check 1 — each rule, one fixture apiece.` (line 1211):
```bash
  # publishable_set (SMA-735) — the one copy of the publishable set and Check 0, which
  # metadata_checks and --verify-publish-groups share. The same fixtures as the two Check 0
  # rows above, through the function the mode calls.
  _expect_rc 2 "publishable_set (empty publishable set)" \
    publishable_set "$tmp/empty.json" "paigasus-kernel"
  _expect_rc 1 "publishable_set (unexpected publishable crate)" \
    publishable_set "$tmp/wrong-name.json" "paigasus-kernel"
  _expect_rc 2 "publishable_set (unreadable metadata JSON)" \
    publishable_set "$tmp/does-not-exist.json" "paigasus-kernel"
  _expect_rc 2 "publishable_set (a missing argument)" \
    publishable_set "$tmp/empty.json"
  _meta "$tmp/ps-good.json" "$base"
  local ps_out ps_rc=0
  ps_out="$(publishable_set "$tmp/ps-good.json" "paigasus-kernel")" || ps_rc=$?
  if [ "$ps_rc" -ne 0 ] || [ "$ps_out" != "$(printf 'paigasus-kernel\t/nowhere')" ]; then
    echo "NEGATIVE CONTROL FAILED: publishable_set (clean fixture) — rc $ps_rc, output: $ps_out" >&2
    failures=$((failures + 1))
  else
    echo "  ok — publishable_set prints <name><TAB><manifest-dir> for a clean fixture"
  fi

```

Insert before the line `  # Check 2b — a listing missing LICENSE, and one containing moon.yml.` (line 1477):
```bash
  # --- SMA-735: --verify-publish-groups through the REAL dispatch, with a cargo stub ----
  # The stub records every argv and scripts the answer of `cargo publish`. Every other
  # subcommand (`cargo metadata`) goes to the real cargo, so the publishable set and the
  # groups come from the real workspace. CI=true keeps --allow-dirty out of the argv. The
  # stub lives in $tmp, so the case does not change the tree.
  local real_cargo="" stub_dir="$tmp/stub-bin" stub_log="$tmp/stub-cargo.log"
  real_cargo="$(command -v cargo)" || real_cargo=""
  if [ -z "$real_cargo" ]; then
    echo "NEGATIVE CONTROL FAILED: --verify-publish-groups — no cargo on PATH for the stub to pass through to" >&2
    failures=$((failures + 1))
  else
    mkdir -p "$stub_dir"
    cat >"$stub_dir/cargo" <<'STUB'
#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# A cargo stub for run.sh --negative-control (SMA-735). It records every argv, scripts the
# answer of `cargo publish`, and passes every other subcommand to the real cargo.
printf '%s\n' "$*" >>"$STUB_CARGO_LOG"
if [ "${1:-}" = "metadata" ] && [ "$STUB_CARGO_MODE" = "extra-crate" ]; then
  "$STUB_REAL_CARGO" "$@" | python3 -c 'import json, sys; m = json.load(sys.stdin); m["packages"].append({"name": "sma735-unlisted", "publish": None, "manifest_path": "/nowhere/Cargo.toml", "dependencies": []}); json.dump(m, sys.stdout)'
  rcs=("${PIPESTATUS[@]}")
  [ "${rcs[0]}" -eq 0 ] && [ "${rcs[1]}" -eq 0 ] || exit 1
  exit 0
fi
if [ "${1:-}" = "publish" ]; then
  case "$STUB_CARGO_MODE" in
    ok) echo "   Packaging (stub)"; exit 0 ;;
    compile) echo "error: could not compile \`stub\` (lib) due to 1 previous error" >&2; exit 101 ;;
    network) echo "warning: spurious network error (3 tries remaining): [28] Timeout was reached" >&2; exit 101 ;;
    *) echo "stub cargo: no scripted publish answer for mode '$STUB_CARGO_MODE'" >&2; exit 99 ;;
  esac
fi
exec "$STUB_REAL_CARGO" "$@"
STUB
    chmod +x "$stub_dir/cargo"

    _verify_with_stub() { # $1 stub mode, rest = extra arguments to run.sh after the mode
      local mode="$1"; shift
      : >"$stub_log"
      CI=true PATH="$stub_dir:$PATH" STUB_CARGO_LOG="$stub_log" STUB_CARGO_MODE="$mode" \
        STUB_REAL_CARGO="$real_cargo" \
        "$BASH" "$REPO_ROOT/ci/publish-metadata/run.sh" --verify-publish-groups "$@"
    }
    _no_publish_in_stub_log() { # $1 label
      if grep -q '^publish ' "$stub_log"; then
        echo "NEGATIVE CONTROL FAILED: $1 — the mode ran cargo publish. Recorded:" >&2
        cat "$stub_log" >&2
        failures=$((failures + 1))
      else
        echo "  ok — $1: no cargo publish ran"
      fi
    }

    _expect_rc 0 "--verify-publish-groups (stub: every group passes)" _verify_with_stub ok
    local n_publish
    n_publish="$(grep -c '^publish ' "$stub_log" || true)"
    if ! grep -qxF -- "publish --dry-run --locked -p paigasus-kernel" "$stub_log" \
       || ! grep -qxF -- "publish --dry-run --locked -p paigasus-proto -p paigasus-proto-derive" "$stub_log" \
       || [ "$n_publish" != "2" ]; then
      echo "NEGATIVE CONTROL FAILED: --verify-publish-groups did not run exactly the two publish groups. Recorded:" >&2
      cat "$stub_log" >&2
      failures=$((failures + 1))
    else
      echo "  ok — --verify-publish-groups runs both publish groups with --dry-run --locked"
    fi
    _expect_rc 1 "--verify-publish-groups (stub: could not compile is a defect)" _verify_with_stub compile
    _expect_rc 2 "--verify-publish-groups (stub: spurious network error is infrastructure)" _verify_with_stub network
    _expect_rc 2 "--verify-publish-groups (an extra argument)" _verify_with_stub ok extra
    _no_publish_in_stub_log "--verify-publish-groups with an extra argument"
    _expect_rc 1 "--verify-publish-groups (stub: a publishable crate outside EXPECTED_PUBLISHABLE)" \
      _verify_with_stub extra-crate
    _no_publish_in_stub_log "--verify-publish-groups with a crate outside EXPECTED_PUBLISHABLE"
  fi

  # Review Focus 5: the mode must never load the category snapshot, so a stale snapshot can
  # never stop a release. Structural, because the snapshot path is fixed in this file.
  if grep -qE 'metadata_checks|SNAPSHOT|categories' < <(declare -f verify_publish_groups); then
    echo "NEGATIVE CONTROL FAILED: --verify-publish-groups reads the category snapshot or metadata_checks" >&2
    failures=$((failures + 1))
  else
    echo "  ok — --verify-publish-groups does not read the category snapshot"
  fi

```

- [ ] **Step 3: Run the control to see it fail**

Run: `/opt/homebrew/bin/bash ci/publish-metadata/run.sh --negative-control; echo rc=$?`
Expected: `rc=1` and the last line `negative control: 9 check(s) failed to bite`. The nine `FAILED` lines are the five `publishable_set` rows (`got rc 127`: the function does not exist yet), `--verify-publish-groups (stub: every group passes) — expected rc 0, got rc 2` (the dispatch prints `unknown arg`), `did not run exactly the two publish groups`, `(stub: could not compile is a defect) — expected rc 1, got rc 2` and `(stub: a publishable crate outside EXPECTED_PUBLISHABLE) — expected rc 1, got rc 2`. The network row, the extra-argument rows and the snapshot row pass already, because `unknown arg` also exits 2 and `declare -f` of a missing function prints nothing. Task 2 Step 10 (M1, M3) proves those rows bite against the real mode. (Measured on a scratch clone while this plan was written.)

- [ ] **Step 4: Create the module**

`ci/publish-metadata/publishable.py`:
```python
# SPDX-License-Identifier: Apache-2.0
"""The publishable set of the Cargo workspace, and Check 0 (SMA-376; one copy since SMA-735).

Two callers use this one copy: the metadata_checks heredoc in run.sh imports
publishable_packages, and run.sh --verify-publish-groups runs this file as a script. So the
verify build in release.yml's verify-crates job and the PR gate agree on which crates are
publishable.

Exit-code contract, inherited from run.sh: 0 pass / 1 the repository is wrong / 2
infrastructure. An empty set is 2 (cargo metadata is broken, or every crate is
publish = false). A set that is not EXPECTED_PUBLISHABLE is 1.
"""

from __future__ import annotations

import json
import os
import sys


def is_publishable(pkg: dict) -> bool:
    # cargo metadata: null => publishable anywhere; [] => publish = false;
    # non-empty list => publishable to those named registries.
    value = pkg.get("publish")
    return value is None or (isinstance(value, list) and len(value) > 0)


def publishable_packages(meta: dict, expected: list[str]) -> dict[str, dict]:
    """The publishable packages of `meta` by name. Check 0: exits 2 on an empty set and 1 when
    the set is not `expected`."""
    pkgs = {p["name"]: p for p in meta.get("packages", []) if is_publishable(p)}
    found = sorted(pkgs)
    want = sorted(expected)
    if not found:
        print(
            "FATAL: no publishable crate found. Either cargo metadata is broken or every "
            "crate is publish = false. This gate must never pass over an empty set.",
            file=sys.stderr,
        )
        sys.exit(2)
    if found != want:
        print(
            f"Check 0 FAILED: publishable set {found} != expected {want}.\n"
            "  Add the crate to EXPECTED_PUBLISHABLE in ci/publish-metadata/run.sh — "
            "or you have just silently disabled this gate.",
            file=sys.stderr,
        )
        sys.exit(1)
    return pkgs


def main(argv: list[str]) -> int:
    if len(argv) != 2:
        print("usage: publishable.py <metadata.json> <expected-csv>", file=sys.stderr)
        return 2
    try:
        with open(argv[0], encoding="utf-8") as fh:
            meta = json.load(fh)
    except Exception as exc:
        print(f"FATAL: cannot read cargo metadata JSON: {exc}", file=sys.stderr)
        return 2
    expected = [x for x in argv[1].split(",") if x]
    for name, pkg in sorted(publishable_packages(meta, expected).items()):
        print(f"{name}\t{os.path.dirname(pkg['manifest_path'])}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
```

- [ ] **Step 5: Use the module in `metadata_checks`**

In `ci/publish-metadata/run.sh`, replace lines 186-212, from `def is_publishable(pkg):` to the `sys.exit(1)` of the Check 0 block (the line before `errors = []`), with:
```python
# --- Check 0: non-vacuity control -------------------------------------------------
# One copy in ci/publish-metadata/publishable.py, shared with --verify-publish-groups
# (SMA-735). It exits 2 on an empty set and 1 when the set is not EXPECTED_PUBLISHABLE.
# is_publishable is also what Check 5 below reads.
from publishable import is_publishable, publishable_packages

pkgs = publishable_packages(meta, expected)
found = sorted(pkgs)
```
The heredoc already finds the module: `run.sh` exports `PYTHONPATH="$REPO_ROOT/ci/publish-metadata…"` (line 100).

- [ ] **Step 6: Add the bash functions, the dispatch arm and the usage**

Insert after the closing `}` of `assert_check2_covered_everything` (before the comment line `# --negative-control — drive the SAME check code…`):
```bash
# The publishable set and Check 0, from ci/publish-metadata/publishable.py: one copy for
# metadata_checks and for --verify-publish-groups (SMA-735). Prints "<name>\t<manifest-dir>"
# per publishable crate. Exit codes: 0, 1 (the set is not EXPECTED_PUBLISHABLE), 2.
publishable_set() { # $1 metadata.json  $2 expected-csv
  python3 "$REPO_ROOT/ci/publish-metadata/publishable.py" "$@"
}

# --verify-publish-groups (SMA-735) — Check 0 and Check 2 alone, for release.yml's
# verify-crates job. That job holds no credential and runs before approve-release. The
# release job publishes with `release-plz release --no-verify`, so this mode is the verify
# build. It uses the same functions as main() and nothing else: it does not load the
# category snapshot, so a stale snapshot can never stop a release.
# Exit codes: 0 every group passed, 1 a defect, 2 infrastructure or a bad invocation.
verify_publish_groups() { # takes no argument
  if [ "$#" -ne 0 ]; then  # the mode takes no argument
    echo "usage: run.sh --verify-publish-groups (it takes no further argument; got: $*)" >&2
    return 2
  fi
  cd "$RS_DIR"

  local meta_json expected_csv publishable groups
  local status=0
  meta_json="$(mktemp)"
  if ! cargo metadata --format-version 1 --no-deps >"$meta_json" 2>/dev/null; then
    rm -f "$meta_json"
    echo "FATAL: \`cargo metadata\` failed in $RS_DIR — nothing could be verified." >&2
    return 2
  fi
  expected_csv="$(IFS=,; printf '%s' "${EXPECTED_PUBLISHABLE[*]}")"

  publishable="$(publishable_set "$meta_json" "$expected_csv")" || status=$?
  if [ "$status" -ne 0 ]; then
    rm -f "$meta_json"
    return "$status"
  fi
  groups="$(publish_groups "$meta_json" "$expected_csv")" || status=$?
  rm -f "$meta_json"
  [ "$status" -eq 0 ] || return "$status"

  local name dir enumerated=()
  while IFS=$'\t' read -r -u 3 name dir; do
    [ -n "$name" ] || continue
    enumerated+=("$name")
    PKG_DIR[$name]="$dir"
  done 3< <(printf '%s\n' "$publishable")

  local group_line group_pkgs
  while IFS= read -r -u 3 group_line; do
    [ -n "$group_line" ] || continue
    IFS=$'\t' read -r -a group_pkgs < <(printf '%s\n' "$group_line")
    status=0; check_publish_group "${group_pkgs[@]}" || status=$?
    [ "$status" -eq 0 ] || return "$status"  # a failed publish group stops the mode
  done 3< <(printf '%s\n' "$groups")

  assert_check2_covered_everything "${enumerated[@]}" || return $?
  echo "publish-metadata: --verify-publish-groups: every publish group passed (${#enumerated[@]} crates)"
}

```

Replace the dispatch (lines 1903-1917) with:
```bash
case "${1:-}" in
  '') main "$@" ;;
  --negative-control) negative_control ;;
  --verify-publish-groups) shift; verify_publish_groups "$@" ;;
  --check-categories-freshness)
    exec python3 "$REPO_ROOT/ci/publish-metadata/categories.py" \
      --check-freshness --snapshot "$SNAPSHOT" ;;
  --refresh-categories)
    exec python3 "$REPO_ROOT/ci/publish-metadata/categories.py" \
      --refresh --snapshot "$SNAPSHOT" ;;
  -h|--help)
    echo "usage: run.sh [--negative-control | --verify-publish-groups |"
    echo "               --check-categories-freshness | --refresh-categories | -h|--help]"
    exit 0 ;;
  *) echo "unknown arg: $1" >&2; exit 2 ;;
esac
```

- [ ] **Step 7: Run the control and the real mode to see them pass**

Run: `/opt/homebrew/bin/bash ci/publish-metadata/run.sh --negative-control; echo rc=$?`
Expected: `rc=0`, the last line `negative control: every check reports red on a broken fixture`, and these ok lines:
```
  ok — publishable_set (empty publishable set) (rc 2)
  ok — publishable_set (unexpected publishable crate) (rc 1)
  ok — publishable_set prints <name><TAB><manifest-dir> for a clean fixture
  ok — --verify-publish-groups (stub: every group passes) (rc 0)
  ok — --verify-publish-groups runs both publish groups with --dry-run --locked
  ok — --verify-publish-groups (stub: could not compile is a defect) (rc 1)
  ok — --verify-publish-groups (stub: spurious network error is infrastructure) (rc 2)
  ok — --verify-publish-groups (an extra argument) (rc 2)
  ok — --verify-publish-groups with an extra argument: no cargo publish ran
  ok — --verify-publish-groups (stub: a publishable crate outside EXPECTED_PUBLISHABLE) (rc 1)
  ok — --verify-publish-groups with a crate outside EXPECTED_PUBLISHABLE: no cargo publish ran
  ok — --verify-publish-groups does not read the category snapshot
```

Run (V-1, it compiles three crates and takes some minutes): `/opt/homebrew/bin/bash ci/publish-metadata/run.sh --verify-publish-groups; echo rc=$?`
Expected:
```
publish-metadata: group [paigasus-kernel] OK
publish-metadata: group [paigasus-proto paigasus-proto-derive] OK
publish-metadata: --verify-publish-groups: every publish group passed (3 crates)
rc=0
```

Run: `/opt/homebrew/bin/bash ci/publish-metadata/run.sh; echo rc=$?`
Expected: `publish-metadata: all checks passed` and `rc=0` (the normal run is unchanged).

Run: `grep -c "def is_publishable" ci/publish-metadata/run.sh`
Expected: `0` (one copy only).

- [ ] **Step 8: Register the new input, then lint**

In `moon.yml`, in the `publish-metadata` task `inputs`, after the line `      - 'ci/publish-metadata/categories.py'`, add:
```yaml
      - 'ci/publish-metadata/publishable.py'
```
In `ci/affected-graph/ci_targets.py`, in `SELF_TASK_EXPECTED_GLOBS["publish-metadata"]`, after `        "ci/publish-metadata/categories.py",` add:
```python
        "ci/publish-metadata/publishable.py",
```
Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
uv run --locked --project py ruff check --config py/pyproject.toml ci/publish-metadata/publishable.py
mkdir -p "$SCRATCH/bashshim" && ln -sf /bin/bash "$SCRATCH/bashshim/bash"
PATH="$SCRATCH/bashshim:$PATH" moon run repo:affected-smoke repo:input-liveness --force
```
Expected: `All checks passed!` from ruff; both Moon tasks pass. If `repo:affected-smoke` reports `repo:publish-metadata` input drift, the two edits above disagree: fix them.

- [ ] **Step 9: Commit**

```bash
git add ci/publish-metadata/publishable.py ci/publish-metadata/run.sh moon.yml ci/affected-graph/ci_targets.py
git commit -m "feat(ci): add the --verify-publish-groups mode to publish-metadata (SMA-735)" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

- [ ] **Step 10: Prove each assertion bites (mutation)**

Create `$SCRATCH/mutate.py` from the "Mutation helper" section if it is absent. Define the two helpers once in the shell:
```bash
R=ci/publish-metadata/run.sh
check() { /opt/homebrew/bin/bash "$R" --negative-control > "$SCRATCH/mut.out" 2>&1; echo "rc=$?"; grep "FAILED" "$SCRATCH/mut.out"; }
undo() { python3 "$SCRATCH/mutate.py" restore "$R" && git diff --exit-code "$R" && echo restored; }
```

M1, the dispatch arm does nothing:
```bash
python3 "$SCRATCH/mutate.py" replace "$R" '--verify-publish-groups) shift' '--verify-publish-groups) exit 0 ;;'
check; undo
```
Expected: `rc=1`, `NEGATIVE CONTROL FAILED: --verify-publish-groups did not run exactly the two publish groups`, the compile, network, extra-argument and outside-crate rows with `got rc 0`; then `restored`.

M2, a failed group does not stop the mode:
```bash
python3 "$SCRATCH/mutate.py" replace "$R" '|| return "$status"  # a failed publish group stops the mode' ':'
check; undo
```
Expected: `rc=1`, `(stub: could not compile is a defect) — expected rc 1, got rc 0` and `(stub: spurious network error is infrastructure) — expected rc 2, got rc 0`; then `restored`.

M3, the argument guard is gone:
```bash
python3 "$SCRATCH/mutate.py" replace "$R" 'if [ "$#" -ne 0 ]; then  # the mode takes no argument' 'if false; then'
check; undo
```
Expected: `rc=1`, `(an extra argument) — expected rc 2, got rc 0` and `with an extra argument — the mode ran cargo publish`; then `restored`.

M4, the mode ignores Check 0:
```bash
python3 "$SCRATCH/mutate.py" replace "$R" 'publishable="$(publishable_set "$meta_json" "$expected_csv")" || status=$?' 'publishable="$(publishable_set "$meta_json" "$expected_csv")" || true'
check; undo
```
Expected: `rc=1`, `with a crate outside EXPECTED_PUBLISHABLE — the mode ran cargo publish`; then `restored`.

Expected after the four restores: `git status --porcelain` prints nothing. (All four were measured on a scratch clone while this plan was written.)

---

### Task 3: M1 — measure what `verify-crates` needs on a plain runner image, and the cold-run time

This is measurement M1 of spec §5.2 and §10. The earlier scratch script `m1-noverify/measure.sh` measured fact F2 (cargo with and without `--no-verify`), not M1. This task measures M1.

**Files:**
- Create (scratch only): `$SCRATCH/m1-runner/inner.sh`, `$SCRATCH/m1-runner/clone.sh`
- Modify: the spec (§14, append the M1 part)

**Interfaces:**
- Consumes: `bash ci/publish-metadata/run.sh --verify-publish-groups` from Task 2 (committed).
- Produces: the decision for Task 5 and Task 7: the job has exactly two steps, `Checkout` and `Verify every publish group`, with no `moonrepo/setup-toolchain` and no `moon setup`, and `timeout-minutes: 30`.

**Stop condition.** STOP and report to the coordinator if the mode fails in the container because a tool is missing (rc 127, `command not found`, or a Python import error), or if the cold run takes more than 1200 seconds. Then the V20 row (Task 5) and the job (Task 7) need a plan change. A network rc 2: run the container once more; two network failures in a row are a stop too.

- [ ] **Step 1: Write the scripts**

`$SCRATCH/m1-runner/clone.sh`:
```bash
#!/bin/bash
# SPDX-License-Identifier: Apache-2.0
# SMA-735 M1: a clean clone of the committed branch, so the container never writes the worktree.
set -eu
W=/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-735-release-job-isolation
rm -rf "$SCRATCH/m1-runner/src"
git clone --quiet --no-hardlinks "$W" "$SCRATCH/m1-runner/src"
git -C "$SCRATCH/m1-runner/src" log --oneline -1
```

`$SCRATCH/m1-runner/inner.sh`:
```bash
#!/bin/bash
# SPDX-License-Identifier: Apache-2.0
# SMA-735 M1, inside ubuntu:24.04: only what the hosted runner image also has (bash 5, python3,
# git, a C toolchain, rustup with no toolchain). No proto, no Moon. rustup installs 1.95.0 from
# rs/rust-toolchain.toml on the first cargo call, as it does on the runner (spec F10).
set -u
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq >/dev/null
apt-get install -y -qq --no-install-recommends ca-certificates curl git python3 build-essential >/dev/null
curl -sSf https://sh.rustup.rs -o /tmp/rustup-init.sh
sh /tmp/rustup-init.sh -y --profile minimal --default-toolchain none >/dev/null 2>&1
. "$HOME/.cargo/env"
echo "bash: $BASH_VERSION"
python3 --version
command -v moon || echo "moon: absent"
command -v proto || echo "proto: absent"
export CI=true
start=$(date +%s); rc=0
bash ci/publish-metadata/run.sh --verify-publish-groups || rc=$?
echo "M1 rc=$rc seconds=$(( $(date +%s) - start ))"
cargo --version
uname -m
```

- [ ] **Step 2: Clone, then run the container**

Run: `SCRATCH="$SCRATCH" /bin/bash "$SCRATCH/m1-runner/clone.sh"`
Expected: one line, the Task 2 commit.

Run (one docker command in this Bash call):
`docker run --rm -v "$SCRATCH/m1-runner":/m1 -w /m1/src ubuntu:24.04 bash /m1/inner.sh`
Expected:
```
moon: absent
proto: absent
publish-metadata: group [paigasus-kernel] OK
publish-metadata: group [paigasus-proto paigasus-proto-derive] OK
publish-metadata: --verify-publish-groups: every publish group passed (3 crates)
M1 rc=0 seconds=<n>
cargo 1.95.0 (…)
```
with `<n>` under 1200. Apply the stop condition.

- [ ] **Step 3: Record M1 in the spec**

Append under §14 of the spec. Write the measured values in place of the bracketed words:
```markdown
### M1 — what `verify-crates` needs, and a cold run (2026-10-09)

Docker `ubuntu:24.04` ([uname -m]) with bash [version], python3 [version], git, build-essential
and rustup with no toolchain. No proto and no Moon. `CI=true bash ci/publish-metadata/run.sh
--verify-publish-groups` exited [rc] after [n] seconds, including the rustup install of 1.95.0
from `rs/rust-toolchain.toml`. Both groups passed.

Decision (spec §5.2): `verify-crates` keeps only `Checkout` and the verify step. It drops
`moonrepo/setup-toolchain` and `moon setup`. `timeout-minutes: 30` stays. The container is a
proxy: the hosted runner is x86_64 and has rustup and python3 in its image. V-6 (Sven's consent)
or V-7 (the first release after the merge) measures the hosted runner.
```

- [ ] **Step 4: Commit**

```bash
git add docs/superpowers/specs/2026-10-09-sma-735-release-job-no-compile-design.md
git commit -m "docs(ci): record the M1 verify-crates runner measurement (SMA-735)" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: The table-driven step-allowlist engine, with V18 unchanged

**Files:**
- Modify: `ci/actionlint/release_guard.py` (imports at lines 17-27; the V18 block at lines 1584-1918)

**Interfaces:**
- Consumes: `UNGATED_JOBS`, `_v18_logical_lines`, `v18_line_segments`, `v18_split`, `_V18_ASSIGN_RE`, `_V18_SET_WORD_RE`, `_V18_WORD_SPLIT_RE`, `_V18_BLANKS`, `steps_of`.
- Produces: `class StepAllowlist` (frozen dataclass; fields `rule: str`, `subject: str`, `jobs: tuple[str, ...]`, `actions: frozenset[str]`, `commands: frozenset[str]`, `keywords: frozenset[str]`, `prefixes: tuple[str, ...]`, `step_keys: frozenset[str]`, `env_names: frozenset[str]`, `workdirs: frozenset[str]`, `with_keys: dict[str, frozenset[str]]`, `hint: str`).
- Produces: `segment_verdict(segment: str, row: StepAllowlist) -> str | None`; `step_config_violations(step: dict, action: str | None, row: StepAllowlist) -> list[str]`; `allowlist_job_violations(doc: dict, name: str, row: StepAllowlist) -> list[str]`; `V18_ROW: StepAllowlist`.
- Keeps: `ungated_job_violations(doc: dict, name: str) -> list[str]` (now `allowlist_job_violations(doc, name, V18_ROW)`), `v18_split`, `v18_line_segments`. Removes `v18_segment_verdict` and `_v18_step_config_violations` (no other caller; `grep -rn` showed only the SMA-684 plan text).

- [ ] **Step 1: Write the V18 snapshot script**

`$SCRATCH/v18_snapshot.py`:
```python
# SPDX-License-Identifier: Apache-2.0
"""SMA-735 Task 4: every V18 message for the SMA-684 case table, the job and workflow cases and
the real file, as JSON. The engine refactor must not change one byte of it."""
import json
import sys
from pathlib import Path

sys.path.insert(0, "ci/actionlint")
import release_guard as rg  # noqa: E402

out = []
for label, step, _ in rg._SMA684_V18_CASES:
    out.append([label, rg.ungated_job_violations({"jobs": {"release-pr": {"steps": [step]}}}, "fixture")])
for key, value in (("container", "ubuntu:24.04"), ("services", {"db": {"image": "postgres"}}),
                   ("defaults", {"run": {"shell": "python {0}"}}), ("env", {"BASH_ENV": "evil.sh"}),
                   ("uses", "./.github/workflows/x.yml")):
    out.append([f"job:{key}", rg.ungated_job_violations(
        {"jobs": {"release-pr": {key: value, "steps": [{"run": "echo hi"}]}}}, "fixture")])
out.append(["workflow:defaults", rg.ungated_job_violations(
    {"defaults": {"run": {"shell": "python {0}"}}, "jobs": {"release-pr": {"steps": [{"run": "echo hi"}]}}}, "fixture")])
out.append(["workflow:env", rg.ungated_job_violations(
    {"env": {"BASH_ENV": "evil.sh"}, "jobs": {"release-pr": {"steps": [{"run": "echo hi"}]}}}, "fixture")])
out.append(["job-uses-no-steps", rg.ungated_job_violations(
    {"jobs": {"release-pr": {"uses": "./.github/workflows/x.yml"}}}, "fixture")])
out.append(["other-job", rg.ungated_job_violations({"jobs": {"build": {"steps": [{"run": "cargo build"}]}}}, "fixture")])
out.append(["real", rg.ungated_job_violations(rg.load_workflow(Path(".github/workflows/release.yml")), "release.yml")])
json.dump(out, sys.stdout, indent=1)
print()
```

- [ ] **Step 2: Take the snapshot before the change**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
uv run --locked --project py python3 "$SCRATCH/v18_snapshot.py" > "$SCRATCH/v18-before.json"
python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); print(len(d), sum(len(m) for _, m in d))' "$SCRATCH/v18-before.json"
```
Expected: `157 122` (147 case rows plus 10 extra rows, and 122 messages; measured on a scratch clone while this plan was written). If the numbers differ, the base commit changed: read the diff before you go on.

- [ ] **Step 3: Add the dataclass import**

In `ci/actionlint/release_guard.py`, after the line `import tomllib`, add:
```python
from dataclasses import dataclass
```

- [ ] **Step 4: Replace the engine**

Insert directly before the comment block that starts `# V18 (SMA-684). Every UNGATED_JOBS member` (line 1584):
```python
@dataclass(frozen=True)
class StepAllowlist:
    """One row of the step-allowlist engine (SMA-735 D5). A row names its jobs and the actions,
    command words, shell keywords, command prefixes, step keys, env names, working directories
    and `with:` keys that those jobs may use. The engine checks each row on its own and never
    merges the sets of two rows. Every message starts with `<file>: <rule>: ` and ends with
    `hint`. The rules for every row: no workflow-level `defaults:` or `env:`, no job key in
    UNGATED_JOB_BANNED_KEYS, no step `shell:`. See README L43 and L44."""

    rule: str
    subject: str
    jobs: tuple[str, ...]
    actions: frozenset[str]
    commands: frozenset[str]
    keywords: frozenset[str]
    prefixes: tuple[str, ...]
    step_keys: frozenset[str]
    env_names: frozenset[str]
    workdirs: frozenset[str]
    with_keys: dict[str, frozenset[str]]
    hint: str


```

Replace the whole function `v18_segment_verdict` (from `def v18_segment_verdict(segment: str) -> str | None:` to its last line `    return f"the command word {words[0]!r} is not on the allowlist"`) with:
```python
def segment_verdict(segment: str, row: StepAllowlist) -> str | None:
    """None when one command segment may run in a job of `row`, else the reason it may not.
    Leading variable assignments are removed; then the row's shell keywords; then the rest must
    start with one of the row's command prefixes or command words. Substitutions never reach
    here: v18_line_segments refuses them. Not a shell parser: see README L43.

    An assignment before a command word puts the variable into that command's environment, so
    `BASH_ENV=./x.sh bash ci/version-lockstep/run.sh --write` and `GIT_EXTERNAL_DIFF=./x git diff`
    run code. Such a segment is refused; a segment of assignments only stays allowed. `set` may
    use only the flags in _V18_SET_WORD_RE: `-a` (allexport) and `-k` export later assignments."""
    s = segment.strip(_V18_BLANKS)
    assigned = False
    while True:
        m = _V18_ASSIGN_RE.match(s)
        if not m:
            break
        assigned = True
        s = s[m.end():].lstrip(_V18_BLANKS)
    words = [w for w in _V18_WORD_SPLIT_RE.split(s) if w]
    if assigned and words:
        return (f"an assignment before the command word {words[0]!r} puts the variable into that "
                f"command's environment")
    while words and words[0] in row.keywords:
        words = words[1:]
    if not words:
        return None
    if words[0] == "set":
        for w in words[1:]:
            if not _V18_SET_WORD_RE.fullmatch(w):
                return (f"`set` with {w!r}, which is not on the allowlist (`-a`, `-o allexport` and "
                        f"`-k` export later assignments)")
    rest = " ".join(words)
    for prefix in row.prefixes:
        if rest == prefix or rest.startswith(prefix + " "):
            return None
    if words[0] in row.commands:
        return None
    return f"the command word {words[0]!r} is not on the allowlist"
```

After the `UNGATED_JOB_WITH_KEYS = {…}` block (it ends with `}` after the `"moonrepo/setup-toolchain": frozenset({"cache"}),` line), insert:
```python


# V18 as a row of the engine (SMA-735 D5). What V18 accepts does not change: the
# _SMA684_V18_CASES table and its count of 147 pin it.
V18_ROW = StepAllowlist(
    rule="V18",
    subject="an UNGATED_JOBS member",
    jobs=tuple(sorted(UNGATED_JOBS)),
    actions=UNGATED_JOB_ACTIONS,
    commands=UNGATED_JOB_COMMANDS,
    keywords=UNGATED_JOB_KEYWORDS,
    prefixes=UNGATED_JOB_PREFIXES,
    step_keys=UNGATED_JOB_STEP_KEYS,
    env_names=UNGATED_JOB_ENV_NAMES,
    workdirs=UNGATED_JOB_WORKDIRS,
    with_keys=UNGATED_JOB_WITH_KEYS,
    hint=V18_HINT,
)
```

Replace the whole function `_v18_step_config_violations` and the whole function `ungated_job_violations` (from `def _v18_step_config_violations(step: dict, action: str | None) -> list[str]:` to the final `    return out` of `ungated_job_violations`) with:
```python
def step_config_violations(step: dict, action: str | None, row: StepAllowlist) -> list[str]:
    """Reasons (without the location prefix) that a step's keys, env, workdir or `with:` are not
    allowed by `row`. `action` is the step's `uses:` path, or None."""
    out: list[str] = []
    for key in step:
        if key not in row.step_keys and key != "shell":
            out.append(f"sets the step key `{key}:`, which is not on the allowlist")
    env = step.get("env")
    if "env" in step:
        if not isinstance(env, dict):
            out.append("sets an `env:` that is not a mapping")
        else:
            for k in env:
                if k not in row.env_names:
                    out.append(f"sets the env name {k!r}, which is not on the allowlist "
                               f"{sorted(row.env_names)}")
    if "working-directory" in step and step["working-directory"] not in row.workdirs:
        out.append(f"sets `working-directory:` to {step['working-directory']!r}, "
                   f"not one of {sorted(row.workdirs)}")
    if "with" in step:
        with_ = step["with"]
        allowed = row.with_keys.get(action or "")
        if not isinstance(with_, dict):
            out.append("sets a `with:` that is not a mapping")
        elif allowed is None:
            out.append("sets `with:` on a step that is not an allowed action")
        else:
            for k in with_:
                if k not in allowed:
                    out.append(f"sets the `with:` key {k!r} of {action!r}, which is not on the allowlist "
                               f"{sorted(allowed)}")
    if action == "actions/checkout":
        with_ = step.get("with")
        persist = with_.get("persist-credentials") if isinstance(with_, dict) else None
        if str(persist).lower() != "false":
            out.append("does not set `persist-credentials: false` on the checkout")
    return out


def allowlist_job_violations(doc: dict, name: str, row: StepAllowlist) -> list[str]:
    """Every step of every job of `row` must match the row's allowlist, plus the rules for every
    row (see StepAllowlist). Does not use the dry-run exemption (_dry_run_exempts): a dry run
    still compiles."""
    out: list[str] = []
    jobs = doc["jobs"]
    for key in ("defaults", "env"):
        if key in doc and any(isinstance(jobs.get(jid), dict) for jid in row.jobs):
            out.append(f"{name}: {row.rule}: the workflow sets `{key}:`, which reaches every step of "
                       f"{row.subject}. {row.hint}")
    for jid in row.jobs:
        job = jobs.get(jid)
        if not isinstance(job, dict):
            continue
        for key in UNGATED_JOB_BANNED_KEYS:
            if key in job:
                out.append(f"{name}: {row.rule}: job '{jid}' sets `{key}:`, which runs code or changes how "
                           f"its steps run. {row.hint}")
        for i, step in enumerate(steps_of(job, f"{name}: job '{jid}'")):
            if not isinstance(step, dict):
                out.append(f"{name}: {row.rule}: job '{jid}' step #{i + 1} is not a mapping. {row.hint}")
                continue
            where = f"{name}: {row.rule}: job '{jid}' step '{step.get('name') or f'#{i + 1}'}'"
            if "shell" in step:
                out.append(f"{where} sets `shell:`, so its `run:` text is not read as bash. {row.hint}")
            uses, run = step.get("uses"), step.get("run")
            if uses is None and run is None:
                out.append(f"{where} has neither `uses:` nor `run:`. {row.hint}")
            action = None
            if uses is not None:
                action = str(uses).split("@", 1)[0]
                if action not in row.actions:
                    out.append(f"{where} uses the action {action!r}, which is not on the allowlist "
                               f"{sorted(row.actions)}. {row.hint}")
            for why in step_config_violations(step, action, row):
                out.append(f"{where} {why}. {row.hint}")
            if run is not None:
                lines, refused = _v18_logical_lines(str(run))
                if refused:
                    out.append(f"{where}: {refused}. {row.hint}")
                for line in lines:
                    segs, refused = v18_line_segments(line)
                    if refused:
                        out.append(f"{where}: the line {line.strip(_V18_BLANKS)!r} is not "
                                   f"allowed: {refused}. {row.hint}")
                        continue
                    for seg in segs:
                        why = segment_verdict(seg, row)
                        if why:
                            out.append(f"{where}: the segment {seg.strip(_V18_BLANKS)!r} is "
                                       f"not allowed: {why}. {row.hint}")
    return out


def ungated_job_violations(doc: dict, name: str) -> list[str]:
    """V18 (SMA-684). Every step of every UNGATED_JOBS member must match the allowlist above.
    Does not use the dry-run exemption (_dry_run_exempts): a dry run still compiles. Since
    SMA-735 this is the V18_ROW of the table-driven engine."""
    return allowlist_job_violations(doc, name, V18_ROW)
```

- [ ] **Step 5: Run the snapshot and the self-test to see them pass**

Run:
```bash
uv run --locked --project py python3 "$SCRATCH/v18_snapshot.py" > "$SCRATCH/v18-after.json"
diff "$SCRATCH/v18-before.json" "$SCRATCH/v18-after.json" && echo SNAPSHOT-IDENTICAL
uv run --locked --project py python3 ci/actionlint/release_guard.py --self-test; echo rc=$?
uv run --locked --project py python3 ci/actionlint/release_guard.py .github/workflows/release.yml; echo rc=$?
uv run --locked --project py python3 ci/actionlint/release_guard.py --fixture-count
uv run --locked --project py ruff check --config py/pyproject.toml ci/actionlint/release_guard.py
grep -n "v18_segment_verdict\|_v18_step_config_violations" ci/actionlint/release_guard.py
```
Expected: `SNAPSHOT-IDENTICAL`; `rc=0` twice with no output from the guard; the fixture count unchanged from before the task (170 or more); `All checks passed!`; the `grep` prints nothing.

- [ ] **Step 6: Commit**

```bash
git add ci/actionlint/release_guard.py
git commit -m "feat(ci): drive the release guard step allowlist from a table of rows (SMA-735)" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

- [ ] **Step 7: Prove the snapshot bites (mutation)**

Run:
```bash
python3 "$SCRATCH/mutate.py" replace ci/actionlint/release_guard.py '    subject="an UNGATED_JOBS member",' 'subject="an UNGATED_JOBS job",'
uv run --locked --project py python3 "$SCRATCH/v18_snapshot.py" > "$SCRATCH/v18-mutated.json"
diff -q "$SCRATCH/v18-before.json" "$SCRATCH/v18-mutated.json"; echo rc=$?
python3 "$SCRATCH/mutate.py" restore ci/actionlint/release_guard.py && git diff --exit-code ci/actionlint/release_guard.py
```
Expected: `Files … differ` and `rc=1`; then no diff after the restore.

---

### Task 5: V20 — the `verify-crates` row and its job rules

**Files:**
- Modify: `ci/actionlint/release_guard.py` (`StepAllowlist` gets `exact`; `segment_verdict`; new V20 block after `ungated_job_violations`; new test code after `_sma684_v18_allowlist_bites`; one row in `self_test`'s tuple)

**Interfaces:**
- Consumes: `StepAllowlist`, `allowlist_job_violations`, `segment_verdict` (Task 4); `gated_path_jobs`, `secret_refs`, `steps_of`, `APPROVAL_JOB`, `RELEASE_WORKFLOW_NAME`, `_APP_TOKEN_ACTION`, `_STRING_LITERAL`, `_V18_WORD_SPLIT_RE`, `_V18_BLANKS`, `SHA_CO`.
- Produces: `StepAllowlist.exact: tuple[tuple[str, ...], ...] = ()`; `VERIFY_JOB = "verify-crates"`; `VERIFY_COMMAND = ("bash", "ci/publish-metadata/run.sh", "--verify-publish-groups")`; `V20_HINT: str`; `V20_ROW: StepAllowlist`; `verify_job_violations(doc: dict, name: str) -> list[str]` (empty unless `name == RELEASE_WORKFLOW_NAME`).
- Produces (tests): `_SMA735_DOC_YAML: str` (the target shape of `plan`, `release-pr`, `verify-crates`, `approve-release`, `release`); `_sma735_doc() -> dict`; `_sma735_apply(doc: dict, job_id: str, op: str, arg: object) -> dict` with ops `none`, `doc-key`, `add-job`, `drop`, `rename`, `job-key`, `step`, `steps`, `run`, `step-key`; `_sma735_cases_bite(cases, count, fn, rule) -> str | None`; `_SMA735_V20_CASES`, `_SMA735_V20_CASE_COUNT = 37`, `_sma735_v20_bites() -> str | None`. Task 6 reuses all of these.
- Not wired into `check_main` yet: Task 7 does that together with the workflow change, so every commit stays green.

- [ ] **Step 1: Write the failing tests**

In `ci/actionlint/release_guard.py`, insert after the end of `_sma684_v18_allowlist_bites` (its last line `    return None`, before `def self_test() -> int:`):
```python
# SMA-735. The target shape of the release path, used by V19, V20 and the cross-row tests. It
# copies the real `release` job and the new `verify-crates` job (spec §5.1, §5.2, measured M1).
_SMA735_DOC_YAML = """
on:
  push:
    branches:
      - main
permissions:
  contents: read
jobs:
  plan:
    if: vars.PAIGASUS_RELEASE_ENABLED == 'true'
    runs-on: ubuntu-latest
    steps:
      - run: echo plan
  release-pr:
    if: github.event_name == 'push'
    runs-on: ubuntu-latest
    steps:
      - run: echo propose
  verify-crates:
    needs: [plan]
    if: needs.plan.outputs.nothing_to_release != 'true'
    runs-on: ubuntu-latest
    timeout-minutes: 30
    steps:
      - name: Checkout
        uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1
        with:
          persist-credentials: false
      - name: Verify every publish group
        run: bash ci/publish-metadata/run.sh --verify-publish-groups
  approve-release:
    needs: [verify-crates]
    runs-on: ubuntu-latest
    environment: release-approval
    steps:
      - run: echo approved
  release:
    needs: [verify-crates, approve-release]
    runs-on: ubuntu-latest
    environment: release-publish
    permissions:
      id-token: write
      contents: read
    steps:
      - name: Authenticate with crates.io
        id: cratesio
        uses: rust-lang/crates-io-auth-action@c6f97d42243bad5fab37ca0427f495c86d5b1a18
      - name: Mint the App installation token
        id: app_token
        uses: actions/create-github-app-token@bcd2ba49218906704ab6c1aa796996da409d3eb1
        with:
          client-id: ${{ secrets.PAIGASUS_BOT_APP_ID }}
          private-key: ${{ secrets.PAIGASUS_BOT_PRIVATE_KEY }}
          permission-contents: write
      - name: Checkout
        uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1
        with:
          fetch-depth: 0
          persist-credentials: false
      - name: Set up proto + Moon
        uses: moonrepo/setup-toolchain@261c62cb5b0f580c7be7c8cd0f023a2e96756095
        with:
          cache: false
      - name: Install pinned release-plz CLI
        run: proto install release-plz
      - name: Release
        id: rel
        working-directory: rs
        env:
          CARGO_REGISTRY_TOKEN: ${{ steps.cratesio.outputs.token }}
          GIT_TOKEN: ${{ steps.app_token.outputs.token }}
        run: |
          set -euo pipefail
          OUT="$(release-plz release --output json --no-verify)"
          echo "$OUT"
          echo "json=$OUT" >> "$GITHUB_OUTPUT"
"""


def _sma735_doc() -> dict:
    return yaml.safe_load(_SMA735_DOC_YAML)


def _sma735_step(job: dict, step_name: str) -> dict:
    hits = [s for s in job["steps"] if isinstance(s, dict) and s.get("name") == step_name]
    if len(hits) != 1:
        raise AssertionError(f"the SMA-735 fixture has {len(hits)} steps named {step_name!r}")
    return hits[0]


def _sma735_apply(doc: dict, job_id: str, op: str, arg: object) -> dict:
    """Apply one SMA-735 test change to `doc` and return it. `job_id` names the job the change
    is about (for `add-job`, the new job's id)."""
    jobs = doc["jobs"]
    if op == "none":
        pass
    elif op == "doc-key":
        key, value = arg
        doc[key] = value
    elif op == "add-job":
        jobs[job_id] = arg
    elif op in ("drop", "rename"):
        job = jobs.pop(job_id)
        if op == "rename":
            jobs[arg] = job
        for other in jobs.values():
            if isinstance(other.get("needs"), list):
                other["needs"] = [arg if (op == "rename" and n == job_id) else n
                                  for n in other["needs"] if op == "rename" or n != job_id]
    elif op == "job-key":
        key, value = arg
        if value is None:
            jobs[job_id].pop(key, None)
        else:
            jobs[job_id][key] = value
    elif op == "step":
        jobs[job_id]["steps"].append(arg)
    elif op == "steps":
        jobs[job_id]["steps"] = arg
    elif op == "run":
        step_name, text = arg
        _sma735_step(jobs[job_id], step_name)["run"] = text
    elif op == "step-key":
        step_name, key, value = arg
        step = _sma735_step(jobs[job_id], step_name)
        if value is None:
            step.pop(key, None)
        else:
            step[key] = value
    else:
        raise AssertionError(f"unknown SMA-735 test op {op!r}")
    return doc


def _sma735_cases_bite(cases: tuple, count: int, fn, rule: str) -> str | None:
    """Every row of `cases` reds `fn` or reads clean as its last field says, and every message
    names `rule`. Deleting a row must red: the strict count is the only pin on each shape."""
    if len(cases) != count:
        return f"the {rule} case table holds {len(cases)} rows, expected {count}"
    for label, job_id, op, arg, want_red in cases:
        found = fn(_sma735_apply(_sma735_doc(), job_id, op, arg), RELEASE_WORKFLOW_NAME)
        if bool(found) != want_red:
            return f"{label}: expected {'a ' + rule + ' violation' if want_red else 'clean'}, got {found or '(clean)'}"
        if not all(f": {rule}: " in v for v in found):
            return f"{label}: a violation does not name {rule}: {found}"
    return None


_V20_STEP = "Verify every publish group"
_SMA735_V20_CASES: tuple[tuple[str, str, str, object, bool], ...] = (
    ("the job is missing", "verify-crates", "drop", None, True),
    ("the path to approve-release is removed", "approve-release", "job-key", ("needs", ["plan"]), True),
    ("permissions id-token write", "verify-crates", "job-key", ("permissions", {"id-token": "write"}), True),
    ("permissions contents write", "verify-crates", "job-key", ("permissions", {"contents": "write"}), True),
    ("an empty permissions mapping", "verify-crates", "job-key", ("permissions", {}), True),
    ("environment release-publish", "verify-crates", "job-key", ("environment", "release-publish"), True),
    ("an App mint with no permission input", "verify-crates", "step",
     {"uses": "actions/create-github-app-token@bcd2ba49218906704ab6c1aa796996da409d3eb1",
      "with": {"client-id": "${{ secrets.PAIGASUS_BOT_APP_ID }}",
               "private-key": "${{ secrets.PAIGASUS_BOT_PRIVATE_KEY }}"}}, True),
    ("a step env that names secrets.X", "verify-crates", "step-key", (_V20_STEP, "env", {"T": "${{ secrets.X }}"}), True),
    ("a github.token reference", "verify-crates", "step-key", (_V20_STEP, "env", {"T": "${{ github.token }}"}), True),
    ("toJSON(github) in a step name", "verify-crates", "step-key", ("Checkout", "name", "${{ toJSON(github) }}"), True),
    ("github.token in a bare job if", "verify-crates", "job-key", ("if", "github.token != ''"), True),
    ("a checkout without with", "verify-crates", "step-key", ("Checkout", "with", None), True),
    ("a checkout with persist-credentials true", "verify-crates", "step-key", ("Checkout", "with", {"persist-credentials": True}), True),
    ("a checkout of another ref", "verify-crates", "step-key", ("Checkout", "with", {"persist-credentials": False, "ref": "other"}), True),
    ("the verify step with &", "verify-crates", "run", (_V20_STEP, "bash ci/publish-metadata/run.sh --verify-publish-groups &"), True),
    ("the verify step with a second command", "verify-crates", "run", (_V20_STEP, "bash ci/publish-metadata/run.sh --verify-publish-groups; echo done"), True),
    ("the verify step with a second line", "verify-crates", "run", (_V20_STEP, "bash ci/publish-metadata/run.sh --verify-publish-groups\necho done"), True),
    ("the verify command twice", "verify-crates", "run", (_V20_STEP, "bash ci/publish-metadata/run.sh --verify-publish-groups; bash ci/publish-metadata/run.sh --verify-publish-groups"), True),
    ("the verify step with an extra argument", "verify-crates", "run", (_V20_STEP, "bash ci/publish-metadata/run.sh --verify-publish-groups --negative-control"), True),
    ("the negative control before the mode", "verify-crates", "run", (_V20_STEP, "bash ci/publish-metadata/run.sh --negative-control --verify-publish-groups"), True),
    ("a quoted script path", "verify-crates", "run", (_V20_STEP, 'bash "ci/publish-metadata/run.sh" --verify-publish-groups'), True),
    ("sh in place of bash", "verify-crates", "run", (_V20_STEP, "sh ci/publish-metadata/run.sh --verify-publish-groups"), True),
    ("the verify step replaced by echo", "verify-crates", "run", (_V20_STEP, "echo skipped"), True),
    ("a step if false", "verify-crates", "step-key", (_V20_STEP, "if", False), True),
    ("a step shell", "verify-crates", "step-key", (_V20_STEP, "shell", "bash"), True),
    ("a step env BASH_ENV", "verify-crates", "step-key", (_V20_STEP, "env", {"BASH_ENV": "evil.sh"}), True),
    ("a step working-directory", "verify-crates", "step-key", (_V20_STEP, "working-directory", "rs"), True),
    ("a step continue-on-error", "verify-crates", "step-key", (_V20_STEP, "continue-on-error", True), True),
    ("the verify step is removed", "verify-crates", "steps",
     [{"name": "Checkout", "uses": "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1",
       "with": {"persist-credentials": False}}], True),
    ("a cargo build step", "verify-crates", "step", {"run": "cargo build"}, True),
    ("a setup-toolchain step", "verify-crates", "step",
     {"uses": "moonrepo/setup-toolchain@261c62cb5b0f580c7be7c8cd0f023a2e96756095", "with": {"cache": False}}, True),
    ("a moon setup step", "verify-crates", "step", {"run": "moon setup"}, True),
    ("a job container", "verify-crates", "job-key", ("container", "ubuntu:24.04"), True),
    ("a job env", "verify-crates", "job-key", ("env", {"BASH_ENV": "evil.sh"}), True),
    ("a workflow env", "verify-crates", "doc-key", ("env", {"BASH_ENV": "evil.sh"}), True),
    ("the target shape", "verify-crates", "none", None, False),
    ("the verify step with extra blanks", "verify-crates", "run", (_V20_STEP, "  bash ci/publish-metadata/run.sh   --verify-publish-groups  "), False),
)
_SMA735_V20_CASE_COUNT = 37


def _sma735_v20_bites() -> str | None:
    if VERIFY_JOB != "verify-crates" or V20_ROW.jobs != (VERIFY_JOB,):
        return f"V20 checks {V20_ROW.jobs!r} (VERIFY_JOB {VERIFY_JOB!r}), expected exactly ('verify-crates',)"
    return _sma735_cases_bite(_SMA735_V20_CASES, _SMA735_V20_CASE_COUNT, verify_job_violations, "V20")
```

In `self_test`, in the tuple of `(check_name, fn)` pairs, after the row `("sma-684 V18 allowlist: every rejected shape reds, every control is clean", _sma684_v18_allowlist_bites),` add:
```python
        ("sma-735 V20 verify-crates: every rejected shape reds, the target shape is clean",
         _sma735_v20_bites),
```

- [ ] **Step 2: Run the self-test to see it fail**

Run: `uv run --locked --project py python3 ci/actionlint/release_guard.py --self-test; echo rc=$?`
Expected: a traceback ending in `NameError: name 'VERIFY_JOB' is not defined`, `rc=1`.

- [ ] **Step 3: Add `exact` to the engine**

In `class StepAllowlist`, after the field `hint: str`, add:
```python
    # SMA-735 V20. A segment whose words equal one of these tuples exactly is allowed.
    exact: tuple[tuple[str, ...], ...] = ()
```
In `segment_verdict`, insert directly before the line `    rest = " ".join(words)`:
```python
    if tuple(words) in row.exact:
        return None
```

- [ ] **Step 4: Add V20**

Insert after the end of `ungated_job_violations`:
```python


# V20 (SMA-735). release.yml's `verify-crates` job builds every publishable crate with
# `cargo publish --dry-run` before `approve-release`, because the `release` job runs
# `release-plz release --no-verify` and so builds nothing. The build runs third-party build
# scripts and proc macros, so the job may hold no credential, and it may run only the verify
# command. Two parts: the engine row below, and the job rules in verify_job_violations.
# Scoped to RELEASE_WORKFLOW_NAME, the same as V11 (spec D7).
VERIFY_JOB = "verify-crates"
VERIFY_COMMAND = ("bash", "ci/publish-metadata/run.sh", "--verify-publish-groups")
V20_HINT = ("The verify-crates job builds third-party code before the approval, so it may hold no "
            "credential and may run only the verify command "
            "(docs/superpowers/specs/2026-10-09-sma-735-release-job-no-compile-design.md, "
            "section 5.5).")
V20_ROW = StepAllowlist(
    rule="V20",
    subject="the verify-crates job",
    jobs=(VERIFY_JOB,),
    actions=frozenset({"actions/checkout"}),
    commands=frozenset(),
    keywords=frozenset(),
    prefixes=(),
    step_keys=frozenset({"name", "id", "uses", "with", "run"}),
    env_names=frozenset(),
    workdirs=frozenset(),
    with_keys={"actions/checkout": frozenset({"persist-credentials"})},
    hint=V20_HINT,
    exact=(VERIFY_COMMAND,),
)
# A bare `if:` is an expression without the `${{ }}` wrapper, so `if: github.token != ''`
# reads the github context with no span to find.
_GITHUB_CTX = re.compile(r"(?<![\w.-])github(?![\w-])", re.IGNORECASE)


def _job_strings(node: object, key: object = None) -> list[tuple[str, bool]]:
    """Every string in `node` (keys and values, at any depth), with True when it is the value
    of an `if:` key, which GitHub reads as an expression."""
    out: list[tuple[str, bool]] = []
    if isinstance(node, dict):
        for k, v in node.items():
            out += _job_strings(k)
            out += _job_strings(v, k)
    elif isinstance(node, list):
        for v in node:
            out += _job_strings(v)
    elif isinstance(node, str):
        out.append((node, key == "if"))
    return out


def verify_job_violations(doc: dict, name: str) -> list[str]:
    """V20 (SMA-735): the `verify-crates` job exists, is on the needs: path of the approval job,
    holds no credential, and runs the verify command exactly once and nothing else."""
    if name != RELEASE_WORKFLOW_NAME:
        return []
    jobs = doc["jobs"]
    job = jobs.get(VERIFY_JOB)
    if not isinstance(job, dict):
        return [f"{name}: V20: no job named '{VERIFY_JOB}' exists, so nothing builds the crates "
                f"before the approval. {V20_HINT}"]
    out = allowlist_job_violations(doc, name, V20_ROW)
    where = f"{name}: V20: job '{VERIFY_JOB}'"
    if VERIFY_JOB not in gated_path_jobs(APPROVAL_JOB, jobs):
        out.append(f"{where} is not on the needs: path of '{APPROVAL_JOB}', so a person can "
                   f"approve the release before the verify build passed. {V20_HINT}")
    for key in ("permissions", "environment"):
        if key in job:
            out.append(f"{where} sets `{key}:`. The job must hold no credential and use the "
                       f"workflow default `contents: read`. {V20_HINT}")
    for text, bare in _job_strings(job):
        names, unresolved = secret_refs(text, bare_expression=bare)
        if names or unresolved:
            out.append(f"{where} reads the secrets context in {text!r}. {V20_HINT}")
        elif "${{" in text or (bare and _GITHUB_CTX.search(_STRING_LITERAL.sub("", text))):
            out.append(f"{where} holds the expression {text!r}. The job may hold none: "
                       f"`github.token` or `toJSON(github)` gives it a token. {V20_HINT}")
    steps = [s for s in steps_of(job, where) if isinstance(s, dict)]
    for step in steps:
        if str(step.get("uses") or "").split("@", 1)[0].lower() == _APP_TOKEN_ACTION:
            out.append(f"{where} mints an App token. {V20_HINT}")
    verify_runs = [s for s in steps if tuple(w for w in _V18_WORD_SPLIT_RE.split(
        str(s.get("run") or "").strip(_V18_BLANKS)) if w) == VERIFY_COMMAND]
    if len(verify_runs) != 1:
        out.append(f"{where} runs `{' '.join(VERIFY_COMMAND)}` in {len(verify_runs)} steps; "
                   f"exactly one step must run it. {V20_HINT}")
    return out
```

- [ ] **Step 5: Run the self-test to see it pass**

Run:
```bash
uv run --locked --project py python3 ci/actionlint/release_guard.py --self-test; echo rc=$?
uv run --locked --project py python3 ci/actionlint/release_guard.py .github/workflows/release.yml; echo rc=$?
uv run --locked --project py python3 "$SCRATCH/v18_snapshot.py" > "$SCRATCH/v18-after.json" && diff "$SCRATCH/v18-before.json" "$SCRATCH/v18-after.json" && echo SNAPSHOT-IDENTICAL
uv run --locked --project py ruff check --config py/pyproject.toml ci/actionlint/release_guard.py
```
Expected: `rc=0` and no output for both guard runs (V20 is not wired into `check_main` yet, so the real file is still clean); `SNAPSHOT-IDENTICAL`; `All checks passed!`.

- [ ] **Step 6: Commit**

```bash
git add ci/actionlint/release_guard.py
git commit -m "feat(ci): add release guard V20 for the verify-crates job (SMA-735)" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

- [ ] **Step 7: Prove each V20 rule bites (mutation)**

For each row: mutate, run `uv run --locked --project py python3 ci/actionlint/release_guard.py --self-test; echo rc=$?`, check `rc=1` and the expected `FAIL 'sma-735 V20 …'` line, then `python3 "$SCRATCH/mutate.py" restore ci/actionlint/release_guard.py && git diff --exit-code ci/actionlint/release_guard.py`.

| Anchor | New line | Expected `FAIL` names |
|---|---|---|
| `if VERIFY_JOB not in gated_path_jobs(APPROVAL_JOB, jobs):` | `if False:` | `the path to approve-release is removed: expected a V20 violation` |
| `for key in ("permissions", "environment"):` | `for key in ():` | `permissions id-token write: expected a V20 violation` |
| `elif "${{" in text or` | `elif False:` | `toJSON(github) in a step name: expected a V20 violation` (the `github.token` row stays red: its env name `T` is not on the allowlist) |
| `if len(verify_runs) != 1:` | `if False:` | `the verify step with &: expected a V20 violation` (in Task 5; after Task 6 the `&` rule also reds it, and the first failing row is `the verify command twice`) |
| `    if tuple(words) in row.exact:` | `if False:` | `the target shape: expected clean` |
| `out.append(f"{where} mints an App token. {V20_HINT}")` | `pass` | none: the allowlist also refuses the action, so the case stays red. Record this as expected defence in depth and restore. |

---

### Task 6: V19 — the `release` row, the `--no-verify` rule and liveness

**Files:**
- Modify: `ci/actionlint/release_guard.py` (`StepAllowlist` gets `required_flags` and `refuse_background`; `v18_split` and `v18_line_segments` get `seps`; `segment_verdict`; `allowlist_job_violations`; `V20_ROW` gets `refuse_background=True`; new V19 block after `verify_job_violations`; tests; two rows in `self_test`)

**Interfaces:**
- Consumes: everything Task 4 and Task 5 produce.
- Produces: `StepAllowlist.required_flags: tuple[tuple[tuple[str, ...], str], ...] = ()`; `StepAllowlist.refuse_background: bool = False`; `v18_split(text: str, seps: list[str] | None = None)`; `v18_line_segments(line: str, seps: list[str] | None = None)`; `_required_flag_verdict(text: str, tail: list[str], lead: tuple[str, ...], flag: str) -> str | None`; `RELEASE_JOBS = frozenset({"release"})`; `V19_HINT: str`; `V19_ROW: StepAllowlist`; `release_job_violations(doc: dict, name: str) -> list[str]` (empty unless `name == RELEASE_WORKFLOW_NAME`).
- Produces (tests): `_SMA735_V19_CASES`, `_SMA735_V19_CASE_COUNT = 40`, `_sma735_v19_bites`, `_sma735_cross_row_and_scope`.

- [ ] **Step 1: Write the failing tests**

Insert after `_sma735_v20_bites`:
```python
_V19_STEP = "Release"
_SMA735_V19_CASES: tuple[tuple[str, str, str, object, bool], ...] = (
    ("no --no-verify", "release", "run", (_V19_STEP, 'set -euo pipefail\nOUT="$(release-plz release --output json)"\necho "$OUT"'), True),
    ("--no-verify=false", "release", "run", (_V19_STEP, 'OUT="$(release-plz release --output json --no-verify=false)"'), True),
    ("--no-verify=true", "release", "run", (_V19_STEP, 'OUT="$(release-plz release --output json --no-verify=true)"'), True),
    ("--no-verify then --no-verify=false", "release", "run", (_V19_STEP, "release-plz release --no-verify --no-verify=false"), True),
    ("--no-verify only in a comment", "release", "run", (_V19_STEP, "# release-plz release --no-verify\nrelease-plz release --output json"), True),
    ("--no-verify inside a single-quoted string", "release", "run", (_V19_STEP, "release-plz release --output 'json --no-verify x'"), True),
    ("--no-verify inside a double-quoted string", "release", "run", (_V19_STEP, 'release-plz release --output "json --no-verify x"'), True),
    ("--no-verify after --", "release", "run", (_V19_STEP, "release-plz release --output json -- --no-verify"), True),
    ("a variable after --no-verify", "release", "run", (_V19_STEP, "release-plz release --no-verify $EXTRA"), True),
    ("a second release-plz release without the flag", "release", "run", (_V19_STEP, "release-plz release --no-verify\nrelease-plz release"), True),
    ("cargo publish", "release", "step", {"run": "cargo publish"}, True),
    ("cargo build", "release", "step", {"run": "cargo build"}, True),
    ("cargo package", "release", "step", {"run": "cargo package"}, True),
    ("moon setup", "release", "step", {"run": "moon setup"}, True),
    ("pnpm install", "release", "step", {"run": "pnpm --dir ts install --frozen-lockfile"}, True),
    ("release-plz release --no-verify && cargo build", "release", "run", (_V19_STEP, "release-plz release --no-verify && cargo build"), True),
    ("release-plz release --no-verify &", "release", "run", (_V19_STEP, "release-plz release --no-verify &"), True),
    ("release-plz release-pr in the release job", "release", "run", (_V19_STEP, "release-plz release-pr --output json"), True),
    ("release-plz update", "release", "run", (_V19_STEP, "release-plz update"), True),
    ("an if keyword", "release", "run", (_V19_STEP, "if true; then release-plz release --no-verify; fi"), True),
    ("set -a", "release", "run", (_V19_STEP, "set -a\nrelease-plz release --no-verify"), True),
    ("uses setup-node", "release", "step", {"uses": "actions/setup-node@v4"}, True),
    ("uses a local action", "release", "step", {"uses": "./local-action"}, True),
    ("an env name outside the set", "release", "step-key", (_V19_STEP, "env", {"CARGO_REGISTRY_TOKEN": "x", "RUSTC_WRAPPER": "./x"}), True),
    ("a working-directory outside the set", "release", "step-key", (_V19_STEP, "working-directory", "ts"), True),
    ("a step if", "release", "step-key", (_V19_STEP, "if", "always()"), True),
    ("a step continue-on-error", "release", "step-key", (_V19_STEP, "continue-on-error", True), True),
    ("a step shell", "release", "step-key", (_V19_STEP, "shell", "bash"), True),
    ("a with key on crates-io-auth-action", "release", "step-key", ("Authenticate with crates.io", "with", {"audience": "x"}), True),
    ("a checkout without persist-credentials false", "release", "step-key", ("Checkout", "with", {"fetch-depth": 0}), True),
    ("a job container", "release", "job-key", ("container", "ubuntu:24.04"), True),
    ("a job env", "release", "job-key", ("env", {"RUSTC_WRAPPER": "./x"}), True),
    ("a workflow defaults", "release", "doc-key", ("defaults", {"run": {"shell": "python {0}"}}), True),
    ("the job renamed", "release", "rename", "publish-crates", True),
    ("a second job with crates-io-auth-action", "extra", "add-job",
     {"runs-on": "ubuntu-latest", "steps": [{"uses": "rust-lang/crates-io-auth-action@c6f97d42243bad5fab37ca0427f495c86d5b1a18"}]}, True),
    ("a second job with an upper-case crates-io-auth-action path", "extra", "add-job",
     {"runs-on": "ubuntu-latest", "steps": [{"uses": "Rust-Lang/crates-io-auth-action@c6f97d42243bad5fab37ca0427f495c86d5b1a18"}]}, True),
    ("a second job that runs release-plz release", "extra", "add-job",
     {"runs-on": "ubuntu-latest", "steps": [{"run": "release-plz release --no-verify"}]}, True),
    ("the target shape", "release", "none", None, False),
    ("the flags in another order", "release", "run",
     (_V19_STEP, 'set -euo pipefail\nOUT="$(release-plz release --no-verify --output json)"\necho "$OUT"\necho "json=$OUT" >> "$GITHUB_OUTPUT"'), False),
    ("release-plz release-pr in the release-pr job", "release-pr", "step", {"run": "release-plz release-pr --output json"}, False),
)
_SMA735_V19_CASE_COUNT = 40


def _sma735_v19_bites() -> str | None:
    # RELEASE_JOBS is pinned by strict equality, the same style as _ungated_jobs_pinned: adding
    # a job id would switch the V19 allowlist on for it, removing one switches V19 off.
    if {"release"} != RELEASE_JOBS or V19_ROW.jobs != ("release",):
        return f"RELEASE_JOBS is {sorted(RELEASE_JOBS)!r} (V19 checks {V19_ROW.jobs!r}), expected exactly ['release']"
    return _sma735_cases_bite(_SMA735_V19_CASES, _SMA735_V19_CASE_COUNT, release_job_violations, "V19")


def _sma735_cross_row_and_scope() -> str | None:
    """Each row is silent outside its own jobs (spec §5.6 cross-row cases), and V19 and V20 are
    silent outside the release workflow (D7)."""
    checks = {"V18": ungated_job_violations, "V19": release_job_violations, "V20": verify_job_violations}
    owner = {"release-pr": "V18", "release": "V19", "verify-crates": "V20"}
    for job_id, rule in owner.items():
        doc = _sma735_apply(_sma735_doc(), job_id, "step", {"run": "cargo build"})
        for other, fn in checks.items():
            found = fn(doc, RELEASE_WORKFLOW_NAME)
            if other == rule and not found:
                return f"{rule} stayed silent on `cargo build` in '{job_id}'"
            if other != rule and found:
                return f"{other} fired on '{job_id}', which only {rule} owns: {found}"
    for rule, fn in (("V18", ungated_job_violations), ("V19", release_job_violations), ("V20", verify_job_violations)):
        if fn(_sma735_doc(), RELEASE_WORKFLOW_NAME):
            return f"{rule} fired on the clean target shape"
    broken = _sma735_apply(_sma735_apply(_sma735_doc(), "release", "drop", None), "verify-crates", "drop", None)
    for rule, fn in (("V19", release_job_violations), ("V20", verify_job_violations)):
        if fn(broken, "fixture"):
            return f"{rule} fired on a file that is not {RELEASE_WORKFLOW_NAME}"
        if not fn(broken, RELEASE_WORKFLOW_NAME):
            return f"{rule} stayed silent on {RELEASE_WORKFLOW_NAME} with its job dropped"
    return None
```

In `self_test`'s tuple, after the V20 row from Task 5, add:
```python
        ("sma-735 V19 release: every rejected shape reds, the target shape is clean",
         _sma735_v19_bites),
        ("sma-735 cross-row and scope: each row is silent outside its own jobs and files",
         _sma735_cross_row_and_scope),
```

- [ ] **Step 2: Run the self-test to see it fail**

Run: `uv run --locked --project py python3 ci/actionlint/release_guard.py --self-test; echo rc=$?`
Expected: a traceback ending in `NameError: name 'RELEASE_JOBS' is not defined`, `rc=1`.

- [ ] **Step 3: Add the separator capture**

In `v18_split`, change the signature line to:
```python
def v18_split(text: str, seps: list[str] | None = None) -> tuple[list[str], str | None]:
```
add this sentence at the end of its docstring (before the closing `"""`): `When seps is a list, each separator found is appended to it (SMA-735 V19 and V20 refuse a lone \`&\`).` and replace:
```python
            width = 2 if text[i:i + 2] in ("&&", "||", "|&") else 1
            segs.append("".join(cur))
```
with:
```python
            width = 2 if text[i:i + 2] in ("&&", "||", "|&") else 1
            if seps is not None:
                seps.append(text[i:i + width])
            segs.append("".join(cur))
```
In `v18_line_segments`, change the signature line to:
```python
def v18_line_segments(line: str, seps: list[str] | None = None) -> tuple[list[str], str | None]:
```
and its last line `    return v18_split(text)` to:
```python
    return v18_split(text, seps)
```

- [ ] **Step 4: Add the two fields and their engine code**

In `class StepAllowlist`, after the field `exact`, add:
```python
    # SMA-735 V19. ((leading words, flag), ...): a segment that starts with the leading words
    # must carry the flag as one plain word (see _required_flag_verdict).
    required_flags: tuple[tuple[tuple[str, ...], str], ...] = ()
    # SMA-735. Refuse a lone `&`: the step can end before the background command does.
    refuse_background: bool = False
```

Insert directly before `def segment_verdict(`:
```python
def _required_flag_verdict(text: str, tail: list[str], lead: tuple[str, ...], flag: str) -> str | None:
    """SMA-735 V19. The reason a segment that runs `lead` does not pass `flag` as one plain word,
    or None. Fail closed: a quote, a backslash or a `$` anywhere in such a segment can change
    the words that bash passes, so the segment is refused rather than read. A word after `--`
    is not a flag."""
    cmd = " ".join(lead)
    if any(c in text for c in "'\"\\$"):
        return (f"`{cmd}` with a quote, a backslash or a `$` in the same command; write its flags "
                f"as plain words, so the guard can read {flag}")
    if "--" in tail:
        tail = tail[:tail.index("--")]
    if any(w.startswith(flag + "=") for w in tail):
        return f"`{cmd}` with `{flag}=…`; only the plain word {flag} is accepted"
    if flag not in tail:
        return f"`{cmd}` without {flag}"
    return None


```

In `segment_verdict`, insert directly before `    if tuple(words) in row.exact:`:
```python
    for lead, flag in row.required_flags:
        if tuple(words[:len(lead)]) == lead:
            why = _required_flag_verdict(s, words[len(lead):], lead, flag)
            if why:
                return why
```

In `allowlist_job_violations`, replace:
```python
                for line in lines:
                    segs, refused = v18_line_segments(line)
                    if refused:
                        out.append(f"{where}: the line {line.strip(_V18_BLANKS)!r} is not "
                                   f"allowed: {refused}. {row.hint}")
                        continue
```
with:
```python
                for line in lines:
                    seps: list[str] = []
                    segs, refused = v18_line_segments(line, seps)
                    if refused:
                        out.append(f"{where}: the line {line.strip(_V18_BLANKS)!r} is not "
                                   f"allowed: {refused}. {row.hint}")
                        continue
                    if row.refuse_background and "&" in seps:
                        out.append(f"{where}: the line {line.strip(_V18_BLANKS)!r} is not allowed: "
                                   f"a lone `&` runs the command in the background, so the step "
                                   f"can end before the command does. {row.hint}")
```

In `V20_ROW = StepAllowlist(`, after `    exact=(VERIFY_COMMAND,),` add:
```python
    refuse_background=True,
```

- [ ] **Step 5: Add V19**

Insert after the end of `verify_job_violations`:
```python


# V19 (SMA-735). The `release` job holds CARGO_REGISTRY_TOKEN, the App installation token and the
# App private key. So it may compile nothing (spec A1): `release-plz release` must carry
# --no-verify, which release-plz passes to every `cargo publish` (spec F1, M2), and every step
# must match the allowlist below. The verify build runs in `verify-crates` (V20) instead.
# Liveness: the job must exist, and no other job may authenticate with crates.io or run
# `release-plz release`, so a rename cannot switch V19 off. Scoped to RELEASE_WORKFLOW_NAME (D7).
RELEASE_JOBS = frozenset({"release"})
V19_HINT = ("The release job holds the crates.io token and the App credentials, so it may run only "
            "the tools on V19's allowlist, and `release-plz release` must carry --no-verify "
            "(docs/superpowers/specs/2026-10-09-sma-735-release-job-no-compile-design.md, "
            "section 5.4).")
V19_ROW = StepAllowlist(
    rule="V19",
    subject="the release job",
    jobs=tuple(sorted(RELEASE_JOBS)),
    actions=frozenset({"rust-lang/crates-io-auth-action", "actions/create-github-app-token",
                       "actions/checkout", "moonrepo/setup-toolchain"}),
    commands=frozenset({"set", "echo"}),
    keywords=frozenset(),
    prefixes=("proto install release-plz", "release-plz release"),
    step_keys=frozenset({"name", "id", "uses", "with", "env", "run", "working-directory"}),
    env_names=frozenset({"CARGO_REGISTRY_TOKEN", "GIT_TOKEN"}),
    workdirs=frozenset({"rs"}),
    with_keys={
        "rust-lang/crates-io-auth-action": frozenset(),
        "actions/create-github-app-token": frozenset({"client-id", "private-key", "permission-contents"}),
        "actions/checkout": frozenset({"fetch-depth", "persist-credentials"}),
        "moonrepo/setup-toolchain": frozenset({"cache"}),
    },
    hint=V19_HINT,
    required_flags=((("release-plz", "release"), "--no-verify"),),
    refuse_background=True,
)
_CRATES_IO_AUTH_ACTION = "rust-lang/crates-io-auth-action"
_RELEASE_PLZ_RELEASE_RE = re.compile(r"release-plz\s+release(?![-\w])")


def release_job_violations(doc: dict, name: str) -> list[str]:
    """V19 (SMA-735): liveness, then the V19 row of the engine."""
    if name != RELEASE_WORKFLOW_NAME:
        return []
    jobs = doc["jobs"]
    out: list[str] = []
    for jid in sorted(RELEASE_JOBS):
        if not isinstance(jobs.get(jid), dict):
            out.append(f"{name}: V19: no job named '{jid}' exists. V19 keys on that literal name, "
                       f"so a rename switches it off. {V19_HINT}")
    for jid, job in jobs.items():
        if jid in RELEASE_JOBS or not isinstance(job, dict):
            continue
        for step in steps_of(job, f"{name}: job '{jid}'"):
            if not isinstance(step, dict):
                continue
            live_action = str(step.get("uses") or "").split("@", 1)[0].lower()
            if live_action == _CRATES_IO_AUTH_ACTION or _RELEASE_PLZ_RELEASE_RE.search(str(step.get("run") or "")):
                out.append(f"{name}: V19: job '{jid}' authenticates with crates.io or runs "
                           f"`release-plz release`, but it is not in RELEASE_JOBS "
                           f"{sorted(RELEASE_JOBS)}. Only a V19 job may publish to crates.io. {V19_HINT}")
                break
    out += allowlist_job_violations(doc, name, V19_ROW)
    return out
```

- [ ] **Step 6: Run the self-test to see it pass**

Run:
```bash
uv run --locked --project py python3 ci/actionlint/release_guard.py --self-test; echo rc=$?
uv run --locked --project py python3 ci/actionlint/release_guard.py .github/workflows/release.yml; echo rc=$?
uv run --locked --project py python3 "$SCRATCH/v18_snapshot.py" > "$SCRATCH/v18-after.json" && diff "$SCRATCH/v18-before.json" "$SCRATCH/v18-after.json" && echo SNAPSHOT-IDENTICAL
uv run --locked --project py ruff check --config py/pyproject.toml ci/actionlint/release_guard.py
```
Expected: `rc=0` twice with no guard output; `SNAPSHOT-IDENTICAL` (the `seps` argument and the new fields do not change V18); `All checks passed!`.

- [ ] **Step 7: Commit**

```bash
git add ci/actionlint/release_guard.py
git commit -m "feat(ci): add release guard V19 for the release job (SMA-735)" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

- [ ] **Step 8: Prove each V19 rule bites (mutation)**

Same procedure as Task 5 Step 7 (mutate, `--self-test`, expect `rc=1` and the named `FAIL`, restore, `git diff --exit-code`).

| Anchor | New line | Expected `FAIL` names |
|---|---|---|
| `    if flag not in tail:` | `if False:` | `no --no-verify: expected a V19 violation` |
| `    if any(w.startswith(flag + "=") for w in tail):` | `if False:` | `--no-verify then --no-verify=false: expected a V19 violation` |
| `if any(c in text for c in` | `if False:` | `--no-verify inside a single-quoted string: expected a V19 violation` |
| `    if "--" in tail:` | `if False:` | `--no-verify after --: expected a V19 violation` |
| `if row.refuse_background and "&" in seps:` | `if False:` | `release-plz release --no-verify &: expected a V19 violation` |
| `if live_action == _CRATES_IO_AUTH_ACTION or _RELEASE_PLZ_RELEASE_RE.search(` | `if False:` | `a second job with crates-io-auth-action: expected a V19 violation` |
| `live_action = str(step.get("uses") or "").split("@", 1)[0].lower()` | `live_action = str(step.get("uses") or "").split("@", 1)[0]` | `a second job with an upper-case crates-io-auth-action path: expected a V19 violation` |
| `    required_flags=((("release-plz", "release"), "--no-verify"),),` | `required_flags=(),` | `no --no-verify: expected a V19 violation` |

---

### Task 7: The workflow change, and V19 and V20 in `check_main`

**Files:**
- Modify: `.github/workflows/release.yml` (header lines 12-17 and 94-97 and 168-175; `plan` comment lines 418-420; new job before `approve-release` at line 549; `approve-release` `needs:` line 557; `release` `needs:` line 565; `Release` step lines 619-628; comment line 933)
- Modify: `ci/actionlint/release_guard.py` (`check_main` body and docstring; new `_sma735_real_workflow`; one row in `self_test`)

**Interfaces:**
- Consumes: `release_job_violations`, `verify_job_violations`, `VERIFY_JOB` (Tasks 5, 6); the M1 decision (Task 3): two steps, `timeout-minutes: 30`.
- Produces: `check_main(doc, name)` runs V19 and V20; `_sma735_real_workflow() -> str | None`.

- [ ] **Step 1: Wire V19 and V20 into `check_main` and add the real-file test (red first)**

In `check_main`, replace:
```python
    out += ungated_job_violations(doc, name)
    return out
```
with:
```python
    out += ungated_job_violations(doc, name)
    # SMA-735. V19 and V20: once each, outside the per-job loop, like V18. Both are scoped to
    # RELEASE_WORKFLOW_NAME inside the functions (spec D7), the same as V11.
    out += release_job_violations(doc, name)
    out += verify_job_violations(doc, name)
    return out
```
Replace the first two lines of its docstring:
```python
    """V1-V5, V7, V8a-c, V8e, V9 and V13-V18 over the release workflow (V16a-c runs from
    main()). V16e is one of the V13-V18 group. V6 applies to CALLED workflows (see
```
with:
```python
    """V1-V5, V7, V8a-c, V8e, V9, V13-V18, V19 and V20 over the release workflow (V16a-c runs
    from main()). V16e is one of the V13-V18 group. V19 (the `release` job) and V20 (the
    `verify-crates` job) run only when `name` is RELEASE_WORKFLOW_NAME. V6 applies to CALLED workflows (see
```

Insert after `_sma735_cross_row_and_scope`:
```python
def _sma735_real_workflow() -> str | None:
    """The real release.yml is the clean control for V19 and V20 (spec §5.6), and check_main is
    their production call site: a fixture table proves the verdict functions, not that
    check_main calls them. Run from the repository root, like check 10."""
    real = Path(".github/workflows/release.yml")
    if not real.is_file():
        return f"{real} is not readable from {Path.cwd()}; run the self-test from the repository root"
    for rule, fn in (("V19", release_job_violations), ("V20", verify_job_violations)):
        found = fn(load_workflow(real), real.name)
        if found:
            return f"the real release.yml fails {rule}: {found}"
    bad = [v for v in check_main(load_workflow(real), real.name) if ": V19: " in v or ": V20: " in v]
    if bad:
        return f"check_main reports V19 or V20 on the real release.yml: {bad}"
    doc = load_workflow(real)
    rel = [s for s in doc["jobs"]["release"]["steps"] if isinstance(s, dict) and s.get("name") == "Release"]
    if len(rel) != 1 or " --no-verify" not in str(rel[0].get("run")):
        return "the real release job has no single `Release` step that runs --no-verify"
    rel[0]["run"] = str(rel[0]["run"]).replace(" --no-verify", "")
    if not any(": V19: " in v for v in check_main(doc, real.name)):
        return "check_main did not report V19 when the real Release step lost --no-verify"
    doc = load_workflow(real)
    doc["jobs"][APPROVAL_JOB]["needs"] = [n for n in needs_of(doc["jobs"][APPROVAL_JOB]) if n != VERIFY_JOB]
    if not any(": V20: " in v for v in check_main(doc, real.name)):
        return "check_main did not report V20 when approve-release lost verify-crates"
    return None
```
In `self_test`'s tuple, after the cross-row row, add:
```python
        ("sma-735 V19 and V20 run on the real release.yml through check_main",
         _sma735_real_workflow),
```

Run:
```bash
uv run --locked --project py python3 ci/actionlint/release_guard.py --self-test; echo rc=$?
uv run --locked --project py python3 ci/actionlint/release_guard.py .github/workflows/release.yml; echo rc=$?
```
Expected: `FAIL 'sma-735 V19 and V20 run on the real release.yml through check_main': the real release.yml fails V19: [...without --no-verify...]` and `rc=1`; the guard run prints `release.yml: V20: no job named 'verify-crates' exists…` and a `release.yml: V19: … without --no-verify` line, `rc=1`.

- [ ] **Step 2: Add the `verify-crates` job**

In `.github/workflows/release.yml`, insert directly before the comment block that starts `  # The ONE place a human approval can be inserted` (line 549):
```yaml
  # THE VERIFY BUILD (SMA-735). The `release` job below runs `release-plz release --no-verify`, so
  # it compiles nothing while it holds the crates.io token and the App credentials. This job does
  # the build instead: one `cargo publish --dry-run --locked` for each publish group, through
  # `ci/publish-metadata/run.sh --verify-publish-groups` (Check 0 and Check 2 only). It runs before
  # `approve-release`, so a crate that does not build stops the run in the reversible stage.
  #
  # The build runs third-party build scripts and proc macros, so this job holds NO credential:
  # no `environment:`, no `permissions:` key (the workflow default is `contents: read`), no
  # `secrets` and no `${{ }}` expression at all. M1 measured that cargo through the runner's
  # rustup, python3 and bash are enough, so the job has no proto and no Moon step.
  # release_guard.py V20 pins all of this. A transient network fault exits 2: re-run the job.
  verify-crates:
    name: verify the crates.io packages
    needs: [plan]
    # FAIL-SAFE POLARITY, the same as `wheels`: anything but the literal 'true' builds.
    # release_guard.py V9b pins the accepted forms.
    if: needs.plan.outputs.nothing_to_release != 'true'
    runs-on: ubuntu-latest
    timeout-minutes: 30
    steps:
      - name: Checkout
        uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1  # v7.0.1
        with:
          persist-credentials: false

      - name: Verify every publish group
        run: bash ci/publish-metadata/run.sh --verify-publish-groups

```

- [ ] **Step 3: Change `needs:` and the `Release` step**

Replace the `approve-release` line `    needs: [wheels, prebuild, proto-dist]` with:
```yaml
    needs: [wheels, prebuild, proto-dist, verify-crates]
```
Replace the `release` line `    needs: [wheels, prebuild, proto-dist, approve-release]` with:
```yaml
    needs: [wheels, prebuild, proto-dist, verify-crates, approve-release]
```
Replace:
```yaml
      - name: Release
        id: rel
        working-directory: rs
        env:
          CARGO_REGISTRY_TOKEN: ${{ steps.cratesio.outputs.token }}
          GIT_TOKEN: ${{ steps.app_token.outputs.token }}
        run: |
          set -euo pipefail
          OUT="$(release-plz release --output json)"
```
with:
```yaml
      # THIS JOB COMPILES NOTHING (SMA-735). Every step can read the crates.io token and the App
      # credentials, and release-plz passes CARGO_REGISTRY_TOKEN to `cargo publish`. So
      # `--no-verify` is LOAD-BEARING: release-plz passes it to every `cargo publish`, and cargo
      # then packages and uploads without a build, a build script or a proc macro. The verify
      # build runs in `verify-crates`, before the approval. release_guard.py V19 holds the steps
      # of this job to an allowlist and reds when `--no-verify` is gone. The tools this job still
      # trusts are listed in docs/superpowers/specs/2026-10-09-sma-735-release-job-no-compile-design.md
      # section 4.1.
      - name: Release
        id: rel
        working-directory: rs
        env:
          CARGO_REGISTRY_TOKEN: ${{ steps.cratesio.outputs.token }}
          GIT_TOKEN: ${{ steps.app_token.outputs.token }}
        run: |
          set -euo pipefail
          OUT="$(release-plz release --output json --no-verify)"
```

- [ ] **Step 4: Update the comments that list the job order**

In `.github/workflows/release.yml`:

Replace lines 13-17:
```yaml
#   * Every other gated job — `wheels`, `prebuild`, `proto-dist`, `approve-release`, `release`,
#     `publish-pypi` and `publish-npm` — carries NO `if:` on the flag itself. They are gated
#     TRANSITIVELY, through an unbroken `needs:` chain back to `plan`: `wheels`, `prebuild` and
#     `proto-dist` depend on `plan` directly (and also skip on `plan`'s `nothing_to_release`
#     output), and the rest depend on those.
```
with:
```yaml
#   * Every other gated job — `wheels`, `prebuild`, `proto-dist`, `verify-crates`,
#     `approve-release`, `release`, `publish-pypi` and `publish-npm` — carries NO `if:` on the
#     flag itself. They are gated TRANSITIVELY, through an unbroken `needs:` chain back to
#     `plan`: `wheels`, `prebuild`, `proto-dist` and `verify-crates` depend on `plan` directly
#     (and also skip on `plan`'s `nothing_to_release` output), and the rest depend on those.
```

Replace line 97:
```yaml
#   {wheels, prebuild, proto-dist} -> release -> {publish-pypi, publish-npm}
```
with:
```yaml
#   {wheels, prebuild, proto-dist, verify-crates} -> approve-release -> release
#       -> {publish-pypi, publish-npm}
#
# SMA-735: `release` COMPILES NOTHING. It publishes with `release-plz release --no-verify`, and
# `verify-crates` does the verify build before the approval, in a job with no credential.
```

Replace lines 171-172:
```yaml
# below is the ONLY holder of the literal flag gate; `wheels`, `prebuild` and `proto-dist` gate on
# it transitively through `needs: [plan]` AND on its `nothing_to_release` output, and
```
with:
```yaml
# below is the ONLY holder of the literal flag gate; `wheels`, `prebuild`, `proto-dist` and
# `verify-crates` gate on it transitively through `needs: [plan]` AND on its
# `nothing_to_release` output, and
```

Replace lines 419-420:
```yaml
  # This job is the ONLY holder of the literal flag gate now. `wheels`, `prebuild` and
  # `proto-dist` gate transitively through `needs: [plan]` — with the flag off this job skips,
```
with:
```yaml
  # This job is the ONLY holder of the literal flag gate now. `wheels`, `prebuild`, `proto-dist`
  # and `verify-crates` gate transitively through `needs: [plan]` — with the flag off this job skips,
```

Replace (the comment in `publish-npm`, line 933):
```yaml
      # BEFORE `release`: `release` needs [wheels, prebuild, proto-dist, approve-release], so a
```
with:
```yaml
      # BEFORE `release`: `release` needs [wheels, prebuild, proto-dist, verify-crates,
      # approve-release], so a
```

- [ ] **Step 5: Run the guard, the self-test and actionlint to see them pass**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
uv run --locked --project py python3 ci/actionlint/release_guard.py --self-test; echo rc=$?
uv run --locked --project py python3 ci/actionlint/release_guard.py .github/workflows/release.yml; echo rc=$?
uv run --locked --project py python3 ci/actionlint/release_guard.py --fixture-count
actionlint .github/workflows/release.yml; echo rc=$?
uv run --locked --project py ruff check --config py/pyproject.toml ci/actionlint/release_guard.py
```
Expected: `rc=0` and no output for the self-test and for the guard on the real file; the fixture count unchanged (170 or more); `actionlint` prints nothing and `rc=0` (if actionlint hangs, the host pipe is small: stop it and leave the verdict to Task 9); `All checks passed!`.

- [ ] **Step 6: Commit**

```bash
git add .github/workflows/release.yml ci/actionlint/release_guard.py
git commit -m "fix(ci): verify the crates before the approval and publish with --no-verify (SMA-735)" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

- [ ] **Step 7: Prove the production call sites (mutation)**

| Anchor | New line | Expected |
|---|---|---|
| `    out += release_job_violations(doc, name)` | `out += []` | `--self-test` `rc=1`, `FAIL 'sma-735 V19 and V20 run on the real release.yml through check_main': check_main did not report V19 when the real Release step lost --no-verify` |
| `    out += verify_job_violations(doc, name)` | `out += []` | `--self-test` `rc=1`, `… check_main did not report V20 when approve-release lost verify-crates` |

Restore after each with `python3 "$SCRATCH/mutate.py" restore ci/actionlint/release_guard.py && git diff --exit-code ci/actionlint/release_guard.py`.

---

### Task 8: Documentation (spec §5.7)

**Files:**
- Modify: `.github/workflows/prebuild.yml` (lines 239-242)
- Modify: `.github/CLAUDE.md` (section "Workflow credentials and release guards", after the `UNGATED_JOBS` bullet)
- Modify: `ci/actionlint/README.md` (check 10 row, line 46; new L44 after L43, before `## Cost`)
- Modify: `ci/publish-metadata/README.md` (new section after "Check 2 — one dry-run per publish group", before "Check 5")
- Modify: `docs/ops/RUNBOOK-release-activation.md` (line 659; new §3.2 after line 204)

**Interfaces:**
- Consumes: the M2 result (Task 1) and the M1 result (Task 3) from spec §14; the names `verify-crates`, `--verify-publish-groups`, V19, V20, `StepAllowlist`, `publishable.py`.
- Produces: no code.

- [ ] **Step 1: `prebuild.yml`**

Replace:
```yaml
      # is upstream of the irreversible one: release.yml's `release` job declares
      # `needs: [wheels, prebuild, proto-dist, approve-release]`, and `prebuild` is this
```
with:
```yaml
      # is upstream of the irreversible one: release.yml's `release` job declares
      # `needs: [wheels, prebuild, proto-dist, verify-crates, approve-release]`, and `prebuild` is this
```

- [ ] **Step 2: `.github/CLAUDE.md`**

In the section `## Workflow credentials and release guards`, after the bullet that starts `` - `release_guard.py`'s `UNGATED_JOBS` exempts a job from the GATING rule (V1) ``, add this bullet. Write `MEASURED, spec M2` when Task 1 measured the flag, or `READ, spec F1` when M2 fell back:
```markdown
- **The `release` job compiles nothing (SMA-735).** Its `Release` step runs
  `release-plz release --output json --no-verify`. release-plz passes the flag to every
  `cargo publish`, so cargo packages and uploads without a build (MEASURED, spec M2). The build
  moved to the `verify-crates` job. That job runs before `approve-release` and holds no
  credential. It runs `bash ci/publish-metadata/run.sh --verify-publish-groups`, which is Check 0
  and Check 2 only. `release_guard.py` V19 holds the `release` job to an allowlist and requires
  `--no-verify`. V20 holds `verify-crates` to the verify command and refuses every credential and
  every `${{ }}` expression in it. V18, V19 and V20 are rows of one engine (`StepAllowlist`).
  Residuals (spec §7): the registry index can change between the verify build and the upload. In
  the merge-commit case release-plz publishes the release PR head, which `verify-crates` did not
  build. `publish-npm` still runs `pnpm install` while it holds `id-token: write`. No gate pins
  `rs/.cargo/config.toml` or `rs/rust-toolchain.toml`. Do not add `publish_no_verify` to
  `rs/release-plz.toml`: the CLI flag is the one V19 can see (spec D3).
```

- [ ] **Step 3: `ci/actionlint/README.md`**

In the check 10 row (line 46), replace:
```markdown
must match an allowlist of actions, command words and command prefixes, because that job can read the App private key (L43). Two parts:
```
with:
```markdown
must match an allowlist of actions, command words and command prefixes, because that job can read the App private key (L43). Since SMA-735 the verdict also includes V19 (the `release` job: an allowlist, and `release-plz release` must carry `--no-verify`) and V20 (the `verify-crates` job: the verify command only, on the `needs:` path of `approve-release`, and no credential). V18, V19 and V20 are rows of one table-driven engine (L44). Two parts:
```

Insert before the line `## Cost`:
```markdown
**L44 — V19 and V20 share V18's engine and its limits (SMA-735).** V18, V19 and V20 are rows of
one table, `StepAllowlist`. Each row holds the allowlist of its own jobs. The engine never merges
two rows. So what L43 says about V18 applies to V19 and V20 too: the engine reads a `run:` block
as command words, and it is not a shell parser. It does not read a command inside a quoted
string. V19 closes one case of this for its own rule: a `release-plz release` command that holds
a quote, a backslash or a `$` reds, so `--no-verify` cannot hide in a string or a variable. V19
and V20 run only when the checked file is `release.yml` (spec D7), the same scope as V11. Their
tests are direct calls (`_SMA735_V19_CASES`, `_SMA735_V20_CASES`), not `FIXTURES` rows, so the
fixture floor did not change. These residuals have no gate: a change to `rs/.cargo/config.toml`
or `rs/rust-toolchain.toml` can change what cargo runs in the `release` job; the `publish-npm`
job runs `pnpm install` while it can request an OIDC token; and nothing pins the rule "no build
downstream of `release`" in the `release.yml` header.

```

- [ ] **Step 4: `ci/publish-metadata/README.md`**

Insert before the line `### Check 5 — Rule R1, symmetric \`changelog_include\` in each version group`:
```markdown
### `--verify-publish-groups` — Check 0 and Check 2 only, for `release.yml` (SMA-735)

`bash ci/publish-metadata/run.sh --verify-publish-groups` runs Check 0 and Check 2 and nothing
else. The `verify-crates` job in `.github/workflows/release.yml` calls it. That job holds no
credential and runs before `approve-release`. The `release` job then publishes with
`release-plz release --no-verify`, so this mode is the last build of the crates before the upload.

- The publishable set comes from `ci/publish-metadata/publishable.py`, the same code that
  `metadata_checks` uses. A set that is not `EXPECTED_PUBLISHABLE` exits 1, so a crate outside
  the list cannot skip the verify build.
- The mode runs `check_publish_group` once for each group from `publish_groups`, then
  `assert_check2_covered_everything`.
- The mode does not load the category snapshot. A stale snapshot cannot stop a release.
- Exit codes: 0 every group passed; 1 a defect (for example "could not compile"); 2 an
  infrastructure fault (for example a network error) or a bad invocation. The mode takes no
  further argument and exits 2 when it gets one.
- When `verify-crates` fails with rc 2, a re-run of the job is safe: nothing irreversible ran.
- `--negative-control` runs the real dispatch with a `cargo` stub first on `PATH`. The stub
  records each argv and scripts the answer of `cargo publish`. `cargo metadata` goes to the real
  cargo. The rows assert both groups with `--dry-run --locked`, rc 1 for "could not compile", rc 2
  for "spurious network error", rc 2 for an extra argument, and rc 1 with no `cargo publish` for a
  publishable crate outside `EXPECTED_PUBLISHABLE`.

```

- [ ] **Step 5: `docs/ops/RUNBOOK-release-activation.md`**

Replace (line 659):
```markdown
`wheels`, `prebuild` and `proto-dist` build every artifact. Then `approve-release` enters the
```
with:
```markdown
`wheels`, `prebuild` and `proto-dist` build every artifact, and `verify-crates` builds every
crate with `cargo publish --dry-run` (SMA-735). Then `approve-release` enters the
```

Insert after line 204 (the paragraph that ends `Open the run and confirm the job executed.`), before the `---` line:
```markdown

### 3.2 The App installation scope — a manual check, OPEN (SMA-735)

Nobody has measured the scope of the Paigasus bot App installation. A user token cannot read it
(`GET /user/installations` returns 403). Before each release, do this check by hand:

1. Open GitHub **Settings → Applications → Installed GitHub Apps**, then **Configure** on the
   Paigasus bot App.
2. Confirm that **Repository access** is "Only select repositories" and lists only
   `SMK1085/paigasus-core`.
3. Confirm that the permissions are **Contents: Read and write**, **Pull requests: Read and
   write** and the mandatory **Metadata: Read-only**, and no other permission.
4. Write the date and the result below.

If the access is "All repositories", or if there is another permission, stop and tell the
maintainer. A wider scope gives more power to a stolen private key.

| Date | Repository access | Permissions | Checked by |
| --- | --- | --- | --- |
```

- [ ] **Step 6: Check the text**

Run:
```bash
git diff --stat
grep -n "verify-crates" .github/workflows/prebuild.yml .github/CLAUDE.md ci/actionlint/README.md ci/publish-metadata/README.md docs/ops/RUNBOOK-release-activation.md
```
Expected: five files changed; each file shows at least one `verify-crates` line. Read each added paragraph once more for ASD-STE100 style: sentences of 25 words or fewer, active voice, no idiom.

- [ ] **Step 7: Commit**

```bash
git add .github/workflows/prebuild.yml .github/CLAUDE.md ci/actionlint/README.md ci/publish-metadata/README.md docs/ops/RUNBOOK-release-activation.md
git commit -m "docs(ci): document verify-crates, V19 and V20 (SMA-735)" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 9: Full local gate run and the V-2 deletion battery

**Files:**
- Create (scratch only): `$SCRATCH/v2/make_cases.py`
- No repository change, unless a gate finds a defect. Then fix it in a new commit with the correct type and scope, and run this whole task again.

**Interfaces:**
- Consumes: every commit of Tasks 1-8.
- Produces: the V-2, V-3, V-4 and V-5 results for the coordinator.

- [ ] **Step 1: Probe the host pipe**

Run the probe from Task 2 Step 1. Write down the number.

- [ ] **Step 2: V-2 — each deletion must red a gate**

`$SCRATCH/v2/make_cases.py`:
```python
# SPDX-License-Identifier: Apache-2.0
"""SMA-735 V-2: one mutated copy of release.yml per deletion. Each copy must red the guard."""
import pathlib
import sys

import yaml

src = pathlib.Path(".github/workflows/release.yml").read_text(encoding="utf-8")
out = pathlib.Path(sys.argv[1])


def text_case(name: str, old: str, new: str) -> None:
    if src.count(old) != 1:
        sys.exit(f"{name}: expected exactly one {old!r}, found {src.count(old)}")
    d = out / name
    d.mkdir(parents=True, exist_ok=True)
    (d / "release.yml").write_text(src.replace(old, new), encoding="utf-8")


text_case("no-verify-removed", 'OUT="$(release-plz release --output json --no-verify)"',
          'OUT="$(release-plz release --output json)"')
text_case("approve-needs", "needs: [wheels, prebuild, proto-dist, verify-crates]\n",
          "needs: [wheels, prebuild, proto-dist]\n")
text_case("command-changed", "run: bash ci/publish-metadata/run.sh --verify-publish-groups",
          "run: bash ci/publish-metadata/run.sh --negative-control")
doc = yaml.safe_load(src)
del doc["jobs"]["verify-crates"]
for job in doc["jobs"].values():
    if isinstance(job.get("needs"), list):
        job["needs"] = [n for n in job["needs"] if n != "verify-crates"]
d = out / "job-deleted"
d.mkdir(parents=True, exist_ok=True)
(d / "release.yml").write_text(yaml.safe_dump(doc, sort_keys=False), encoding="utf-8")
print("cases written")
```
Run from the repository root:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
uv run --locked --project py python3 "$SCRATCH/v2/make_cases.py" "$SCRATCH/v2"
for c in no-verify-removed approve-needs command-changed job-deleted; do
  uv run --locked --project py python3 ci/actionlint/release_guard.py "$SCRATCH/v2/$c/release.yml" > "$SCRATCH/v2/$c.out" 2>&1
  echo "$c rc=$? V19=$(grep -c ': V19: ' "$SCRATCH/v2/$c.out") V20=$(grep -c ': V20: ' "$SCRATCH/v2/$c.out")"
done
```
Expected:
```
cases written
no-verify-removed rc=1 V19=1 V20=0
approve-needs rc=1 V19=0 V20=1
command-changed rc=1 V19=0 V20=2
job-deleted rc=1 V19=0 V20=1
```
(`command-changed` gives two V20 lines: the engine refuses the words, and the exactly-one rule finds zero steps. Any count of 1 or more in the expected column is a pass; a 0 there is a failure.)

Then the dispatch deletion:
```bash
python3 "$SCRATCH/mutate.py" replace ci/publish-metadata/run.sh '--verify-publish-groups) shift' '# the dispatch arm is deleted'
/opt/homebrew/bin/bash ci/publish-metadata/run.sh --negative-control > "$SCRATCH/v2/dispatch.out" 2>&1; echo rc=$?
grep -c "NEGATIVE CONTROL FAILED: --verify-publish-groups" "$SCRATCH/v2/dispatch.out"
python3 "$SCRATCH/mutate.py" restore ci/publish-metadata/run.sh && git diff --exit-code ci/publish-metadata/run.sh
```
Expected: `rc=1`, a count of 1 or more, then no diff.

- [ ] **Step 3: The full gate graph through Moon, with bash 3.2**

Copy the full gate graph command from the root `CLAUDE.md` section "Before you push: the full gate graph" (the `moon ci …` command between the two marker comments; do not copy the markers). Run it with the bash shim:
```bash
mkdir -p "$SCRATCH/bashshim" && ln -sf /bin/bash "$SCRATCH/bashshim/bash"
export PATH="$SCRATCH/bashshim:$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
git fetch origin main
<the moon ci command, copied verbatim>
```
Expected: every target passes, except the gates that need bash 4 or 5. Under bash 3.2 these can fail with `mapfile: command not found` or `declare: -A: invalid option`: `repo:ruff-ci`, `repo:next-public-free`, `repo:publish-metadata`, and `repo:actionlint` (which can also exit rc 2 on its pipe preflight). Such a failure is a bash-version artifact, not a finding. Any other failure is a finding: diagnose it with the root `CLAUDE.md` procedure (capture the evidence before a re-run).

- [ ] **Step 4: Re-run the bash-4+ gates directly**

Run each in its own Bash call:
```bash
/opt/homebrew/bin/bash ci/ruff/run.sh; echo rc=$?
/opt/homebrew/bin/bash ci/next-public/run.sh; echo rc=$?
/opt/homebrew/bin/bash ci/publish-metadata/run.sh --negative-control; echo rc=$?
/opt/homebrew/bin/bash ci/publish-metadata/run.sh; echo rc=$?
/opt/homebrew/bin/bash ci/publish-metadata/run.sh --verify-publish-groups; echo rc=$?
```
Expected: `rc=0` for each. If the pipe probe printed 512, run the three `ci/publish-metadata/run.sh` lines in Docker with the command from Task 2 Step 1 (change the last argument per line).

- [ ] **Step 5: `repo:actionlint` (V-3, V-4)**

Run (always; it does not need the pipe):
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
uv run --locked --project py python3 ci/actionlint/release_guard.py --self-test; echo rc=$?
uv run --locked --project py python3 ci/actionlint/release_guard.py .github/workflows/release.yml; echo rc=$?
```
Expected: `rc=0` twice, no output.

If the pipe probe printed 8192 or more, run: `/opt/homebrew/bin/bash ci/actionlint/run.sh; echo rc=$?`
Expected: the preflight line `pipe capacity <n> bytes (floor 8192)`, no `FAIL` row, `rc=0`.

If the probe printed 512, run the gate in a Linux container instead (memory "small-pipe-linux-container-workaround"): build FROM `rust:1.95.0-bookworm` plus uv, node and pnpm, with anonymous volumes over `ts/node_modules`, `py/.venv` and `rs/target`. If that container route is not available in this session, write "repo:actionlint: no local verdict (512-byte pipe); CI is the verdict" in the report. Never read a hang or the rc 2 pipe message as a pass or a fail.

- [ ] **Step 6: Report**

Give the coordinator one line per gate: the bash used, the rc, and the reason for any gate without a local verdict. Do not push.

---

### Task 10: Final checklist (coordinator only)

**Files:** none.

**Interfaces:**
- Consumes: Task 9's report.
- Produces: the push decision, the follow-up issue, and the post-merge check.

- [ ] **Step 1: Coverage check against the spec**

Confirm each item has a commit: §5.1 and §5.2 (Task 7), §5.3 (Task 2), §5.4 (Tasks 4 and 6), §5.5 (Task 5), §5.6 (Tasks 2, 4, 5, 6, 7), §5.7 (Tasks 7 and 8), M2 (Task 1), M1 (Task 3), V-1 (Task 2 Step 7), V-2 (Task 9 Step 2), V-3 (Task 9 Step 5), V-4 and V-5 (Task 9 Steps 3-5).

- [ ] **Step 2: V-6 — OPTIONAL, GATED: needs Sven's consent; do not run**

Only after a separate, explicit consent from Sven for this run and its cost (a full build matrix):
```bash
gh workflow run release.yml --ref feature/sma-735-release-job-isolation --repo SMK1085/paigasus-core
```
Then `verify-crates` must pass, and `approve-release` must fail to deploy from the branch (spec F15), so nothing irreversible runs. Without the consent, skip this step and say so in the PR description.

- [ ] **Step 3: Push and open the PR (the normal pipeline)**

Push and open the PR through the feature-factory open-pr stage. The PR description states: what changed, the M1 and M2 results, the residuals of spec §7, and the rollback (spec §11: revert the PR; the old job shape works again and no data format changes).

- [ ] **Step 4: Open the follow-up Linear issue (spec §9)**

Read SMA-735's project, milestone, priority and labels first, and set the same four fields at creation. Title: `publish-npm: pnpm install runs lifecycle scripts while the job holds id-token: write`. Body:
- The risk row of spec §7 for `publish-npm`, and the INFERRED claim to measure: crates.io and PyPI trusted publishing bind the repository, the workflow file and the environment, not the job.
- The option `pnpm install --ignore-scripts` for that job, and what `napi prepublish` then needs.
- F7: no gate holds the rule "no build downstream of `release`" in the `release.yml` header.
- F16: pnpm 11.3.0 runs a dependency build script only when `allowBuilds` allows it; only `sharp` is allowed.

- [ ] **Step 5: V-7 — the first release after the merge (read-only)**

On the first `release.yml` run after the merge that publishes:
```bash
gh run list --workflow release.yml --repo SMK1085/paigasus-core --limit 5
gh run view <run-id> --repo SMK1085/paigasus-core --json jobs --jq '.jobs[] | {name, startedAt, completedAt, conclusion}'
```
Confirm: `verify the crates.io packages` completed before `approve the release` started; the `publish to crates.io and cut tags` job is clearly shorter than in the last release before the merge (the verify build of three crates is gone); the `<crate>-v<version>` tags and the two GitHub releases exist as before. The release-plz log shows no `Compiling` line with or without `--no-verify` (spec F12), so do not look for one.

If `verify-crates` fails, the run stops before `approve-release` and nothing irreversible ran: fix forward on `main`, or re-run the job for a transient fault (rc 2). If `release` fails, the failure is in packaging or upload; release-plz is idempotent per package (spec §11).
