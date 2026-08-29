# Cargo Lockfile Integrity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make a truncated `rs/Cargo.lock` red the required `moon ci` check, and stop any cargo invocation from silently repairing one mid-run.

**Architecture:** Three parts. Part 1 is an unconditional `ci.yml` step running `cargo metadata --locked` **before** the `moon ci` step, so nothing has repaired the lock yet — race-free by placement, needing no new Moon task and no registry obligations. Part 2 adds `--locked` to eight task declarations so cargo audits reflect the shipped resolution. Part 3 guards Part 2 with a check inside `cargo_moon_parity.py`, which `repo:affected-smoke` already runs, deriving its invocation set from `moon query projects` rather than from file text.

**Tech Stack:** Bash, Python 3.12, Moon 2.5.3, cargo 1.95, GitHub Actions.

**Spec:** `docs/superpowers/specs/2026-08-29-sma-601-cargo-lock-integrity-design.md`

## Global Constraints

- Every source file opens with an SPDX header: `# SPDX-License-Identifier: Apache-2.0` for bash and Python.
- Exit-code contract for the new gate: **0 = pass, 1 = the lock does not satisfy the manifests, 2 = infrastructure (the gate asserted nothing)**. rc 2 must be visibly distinct in the step output.
- Never use `--offline` or `--frozen` in the new gate: on a cold cargo cache `cargo metadata` needs the registry index, so `--offline` reports a false red.
- `set -euo pipefail` is required in every Moon `script:` block — Moon takes a block's status from its last command.
- **errexit does not propagate through `$( )`**: `foo || ec=$?` cannot observe an `exit 2` raised inside a command substitution that `foo` called, because POSIX suspends errexit for the left side of an AND-OR list. Use explicit `|| return 2` instead of relying on errexit.
- Prefix local commands with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` so the proto-managed tools — moon, uv, buf, nextest — resolve to the repo-pinned versions. `cargo` itself comes from rustup, which honours `rs/rust-toolchain.toml` whenever it is invoked inside the Rust workspace, so the export does not pin it.
- Branch: `feature/sma-601-cargo-lock-integrity`. Conventional commits with a workspace scope.
- Do NOT add a `repo:*` Moon task. Adding one would pull in eight registry obligations the spec deliberately avoids.

---

### Task 1: The `ci.yml` integrity step

**Files:**
- Create: `ci/cargo-lock-integrity/run.sh`
- Create: `ci/cargo-lock-integrity/README.md`
- Modify: `.github/workflows/ci.yml` (insert a step after "Drop any cached nextest JUnit reports (stale-artifact guard)" at `:205-206`, before "moon ci (affected graph)" at `:208`)

**Interfaces:**
- Consumes: nothing.
- Produces: `ci/cargo-lock-integrity/run.sh`, supporting three modes — no argument (real run), `--self-test`, `--negative-control`. Exit codes 0/1/2 per Global Constraints. Task 5 pins the `ci.yml` step's text, so the step's `name:` and `run:` lines are load-bearing and must be copied verbatim from Step 5 below.

- [ ] **Step 1: Write `ci/cargo-lock-integrity/run.sh` with a failing real run**

Create the file with mode 0755. Write it in full:

```bash
#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# SMA-601 — assert that rs/Cargo.lock satisfies every workspace manifest.
#
# WHY THIS IS A ci.yml STEP AND NOT A repo:* MOON TASK. Dependabot cargo PRs repeatedly ship a
# truncated lock (PRs 83, 96, 140, 149, 181). `moon ci` was green on all of them because an
# UNLOCKED cargo invocation re-resolves and rewrites an inconsistent lock in place, mid-run,
# before any --locked task reads it: measured on PR 181's 72c0ddb52, `cargo tree` and `cargo deny`
# each rewrote the lock from 176 packages to 548 and exited 0, both starting at 06:37:55, twelve
# seconds before the first --locked task. A Moon task would race those repairers. This step runs
# BEFORE the `moon ci` step, when nothing has run yet, so the working tree still holds the
# committed lock. That is the same argument CLAUDE.md records for the codegen-drift step.
#
# EXIT CODES. 0 pass; 1 the lock does not satisfy the manifests; 2 infrastructure — the gate
# asserted nothing. `cargo metadata` exits 101 for a broken lock, a malformed manifest and a
# registry outage alike, so a shared code would let a crates.io outage red a REQUIRED check.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
RS_DIR="$REPO_ROOT/rs"

# cargo has no distinct exit code for "the registry is down" vs "your lock is broken", so
# classify on stderr. Mirrors ci/publish-metadata/run.sh:589-604. Returns 1 for a real
# assertion failure, 2 for infrastructure.
#
# The --locked message wins FIRST and unconditionally. It is cargo's own wording for exactly
# the condition this gate exists to detect, and a truncated-lock run ALSO prints "Updating
# crates.io index" beforehand — so a network pattern evaluated first would misfile every real
# detection as infrastructure and the gate would never report red.
classify_cargo_failure() { # $1 captured-output file -> rc 1 assertion, rc 2 infrastructure
  if grep -qF 'because --locked was passed to prevent this' "$1"; then
    return 1
  fi
  if grep -qiE 'spurious network error|could not connect|connection timed out|network failure|rate limit|HTTP status 50[234]|failed to fetch|error sending request|failed to get response' "$1"; then
    return 2
  fi
  return 1
}

# Runs the assertion against the workspace at $1. Echoes cargo's captured output on failure.
# Returns 0, 1 or 2. Explicit `|| ...` rather than relying on errexit: errexit is suspended for
# the left side of an AND-OR list, so a nested failure would otherwise be swallowed.
assert_lock_satisfies_manifests() { # $1 workspace dir
  local dir="$1" out rc=0
  out="$(mktemp)" || return 2
  if ( cd "$dir" && cargo metadata --locked --format-version 1 >/dev/null ) 2>"$out"; then
    rm -f "$out"
    return 0
  fi
  classify_cargo_failure "$out" || rc=$?
  cat "$out" >&2
  rm -f "$out"
  return "$rc"
}

report() { # $1 rc
  case "$1" in
    0) echo "cargo-lock-integrity: rs/Cargo.lock satisfies every workspace manifest" ;;
    1) echo "::error::rs/Cargo.lock does not satisfy every workspace manifest. A dependency PR has probably shipped a TRUNCATED lock (see SMA-601). Repair it against the merge-base rather than force-pushing: compare package counts with 'grep -c ^.\\[\\[package\\]\\] rs/Cargo.lock'." >&2 ;;
    2) echo "::error::cargo-lock-integrity ABORTED: infrastructure error (rc=2). The gate asserted NOTHING — this is not a green result." >&2 ;;
  esac
}

# --self-test: drive classify_cargo_failure over a fixture table. Counted, never a bare pass.
self_test() {
  local failures=0 cases=0 tmp rc
  tmp="$(mktemp)"

  expect_class() { # $1 name  $2 expected-rc  $3 stderr-text
    cases=$((cases + 1))
    printf '%s\n' "$3" > "$tmp"
    rc=0
    classify_cargo_failure "$tmp" || rc=$?
    if [ "$rc" -ne "$2" ]; then
      echo "self-test '$1': classify_cargo_failure returned $rc, expected $2" >&2
      failures=$((failures + 1))
    fi
  }

  expect_class 'truncated lock is an assertion failure' 1 \
    'error: cannot update the lock file /src/rs/Cargo.lock because --locked was passed to prevent this'
  # The real red path prints BOTH lines. Proves the --locked test wins over the network test.
  expect_class 'index fetch preceding a lock error is still an assertion failure' 1 \
    '    Updating crates.io index
error: cannot update the lock file /src/rs/Cargo.lock because --locked was passed to prevent this'
  expect_class 'a registry outage is infrastructure' 2 \
    'error: failed to get response from https://index.crates.io/config.json
Caused by: spurious network error (3 tries remaining)'
  expect_class 'a rate limit is infrastructure' 2 \
    'error: failed to fetch https://github.com/rust-lang/crates.io-index: rate limit exceeded'
  expect_class 'a 503 is infrastructure' 2 \
    'error: download of config.json failed: HTTP status 503'
  expect_class 'an unrecognised cargo error is an assertion failure, never a silent skip' 1 \
    'error: failed to parse manifest at /src/rs/crates/libs/paigasus-kernel/Cargo.toml'

  rm -f "$tmp"

  if [ "$cases" -ne 6 ]; then
    echo "self-test: ran $cases cases, expected 6 — a fixture row was deleted" >&2
    failures=$((failures + 1))
  fi
  if [ "$failures" -ne 0 ]; then
    echo "cargo-lock-integrity --self-test: $failures failure(s)" >&2
    return 1
  fi
  echo "cargo-lock-integrity --self-test: $cases case(s), all correct"
}

# --negative-control: prove the gate reports RED, through the SAME function the real run calls.
# A control that skips that call is the SMA-530 "control that actively lies" shape.
negative_control() {
  local tmp rc=0
  tmp="$(mktemp -d)" || return 2
  # shellcheck disable=SC2064
  trap "rm -rf '$tmp'" RETURN
  git -C "$REPO_ROOT" archive HEAD rs | tar -x -C "$tmp" || return 2
  # Delete the FIRST [[package]] block. Name-free on purpose: a hard-coded crate name rots the
  # day that dependency is dropped, and any missing package is enough to make the lock
  # inconsistent with the manifests.
  python3 - "$tmp/rs/Cargo.lock" <<'PY' || return 2
import re, sys
p = sys.argv[1]
text = open(p).read()
blocks = text.split("\n[[package]]\n")
if len(blocks) < 3:
    sys.exit("negative control could not find two [[package]] blocks to mutate")
del blocks[1]
open(p, "w").write("\n[[package]]\n".join(blocks))
PY
  assert_lock_satisfies_manifests "$tmp/rs" || rc=$?
  case "$rc" in
    1) echo "cargo-lock-integrity --negative-control: reported red (rc=1) as expected" ;;
    0) echo "::error::negative control PASSED on a mutated lock — the gate cannot report red." >&2
       return 1 ;;
    *) echo "::error::negative control returned rc=$rc, not the expected 1. The control asserted NOTHING." >&2
       return 2 ;;
  esac
}

main() {
  local rc=0
  case "${1:-}" in
    --self-test)        self_test; return $? ;;
    --negative-control) negative_control; return $? ;;
    '') ;;
    *) echo "usage: run.sh [--self-test|--negative-control]" >&2; return 2 ;;
  esac
  assert_lock_satisfies_manifests "$RS_DIR" || rc=$?
  report "$rc"
  return "$rc"
}

main "$@"
```

- [ ] **Step 2: Run the self-test and the negative control to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
chmod +x ci/cargo-lock-integrity/run.sh
bash ci/cargo-lock-integrity/run.sh --self-test
bash ci/cargo-lock-integrity/run.sh --negative-control
bash ci/cargo-lock-integrity/run.sh
```

Expected: `--self-test` prints `6 case(s), all correct`; `--negative-control` prints `reported red (rc=1) as expected`; the real run prints `rs/Cargo.lock satisfies every workspace manifest`. All three exit 0.

- [ ] **Step 3: Prove the gate reds on the real PR 181 lock**

```bash
gh api "repos/SMK1085/paigasus-core/contents/rs/Cargo.lock?ref=72c0ddb52" --jq '.content' | base64 -d > /tmp/lock-181.toml
cp rs/Cargo.lock /tmp/lock-main.toml
cp /tmp/lock-181.toml rs/Cargo.lock
bash ci/cargo-lock-integrity/run.sh; echo "rc=$?"
git checkout -- rs/Cargo.lock
grep -c '^\[\[package\]\]' rs/Cargo.lock
```

Expected: `rc=1`, the `::error::` line naming a truncated lock, and `543` after the restore. If `rc=2`, `classify_cargo_failure`'s ordering is wrong — the `--locked` test must win over the network test.

- [ ] **Step 4: Write `ci/cargo-lock-integrity/README.md`**

Cover, in this order: what the gate asserts; the measured root cause from spec section 2 including the timestamp table; why it is a `ci.yml` step rather than a `repo:*` task; the 0/1/2 exit-code contract and why 2 exists; and a **Limitations** section listing verbatim, one bullet each, the seven residuals from spec section 7.

- [ ] **Step 5: Add the step to `ci.yml`**

Insert immediately before the `- name: moon ci (affected graph)` step. Copy verbatim — Task 5 pins both lines:

```yaml
      # SMA-601 — MUST stay BEFORE the `moon ci` step. Placement is the whole guarantee: an
      # unlocked cargo invocation inside the graph (measured: repo:deny and
      # repo:wasm-getrandom-free) re-resolves and rewrites an inconsistent lock in place, so a
      # check that ran inside `moon ci` would race the repair and pass. Carries no `if:`, for
      # the reason CLAUDE.md gives for the codegen-drift step below: it must run on EVERY CI run
      # and must not be deselectable. Pinned by check 8f in ci/actionlint/run.sh.
      - name: Cargo lockfile integrity (rs/Cargo.lock satisfies every manifest)
        run: bash ci/cargo-lock-integrity/run.sh
```

- [ ] **Step 6: Verify the workflow still lints**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:actionlint
```

Expected: PASS. If it reds on `continue-on-error` or the `T` array, the step was inserted in the wrong job.

- [ ] **Step 7: Commit**

```bash
git add ci/cargo-lock-integrity/ .github/workflows/ci.yml
git commit -m "ci(repo): assert rs/Cargo.lock satisfies every manifest before moon ci (SMA-601)"
```

---

### Task 2: Add `--locked` to the eight unlocked declarations

**Files:**
- Modify: `.moon/tasks/rust.yml` (`build` at `:35`, `build-release` at `:48`, `test` at `:54`, and the `lint` comment at `:69-76`)
- Modify: `moon.yml` (`repo:deny` at `:19`, `repo:parity-corpus-drift` at `:218`, `repo:observability-drift` at `:242`, `repo:nats-permissions` at `:270`, `repo:wasm-getrandom-free` at `:322`)
- Modify: `ts/packages/paigasus-kernel/moon.yml` (`build` at `:43`, `test` at `:131` — the `wasm-pack` halves only)

**Interfaces:**
- Consumes: nothing from Task 1.
- Produces: a graph in which every task whose resolved invocation contains a `cargo <verb>` also contains `--locked`, except the three wrapper tasks Task 3 allowlists. Task 3's check fails until this task lands, so this must be committed first.

- [ ] **Step 1: Confirm the current unlocked count**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon query projects > /tmp/projects-before.json
python3 - <<'PY'
import json, re
d = json.load(open('/tmp/projects-before.json'))
pat = re.compile(r'\bcargo\s+(?:\+\S+\s+)?(bench|build|check|clippy|deny|doc|fetch|metadata|nextest|package|publish|run|test|tree|update)\b')
n = u = 0
for p in d['projects']:
    for name, t in (p.get('tasks') or {}).items():
        blob = ' '.join([t.get('command') or '', t.get('script') or ''] + [str(a) for a in (t.get('args') or [])])
        if pat.search(blob):
            n += 1
            if '--locked' not in blob and '--frozen' not in blob:
                u += 1
print(f"cargo-resolving invocations: {n}, unlocked: {u}")
PY
```

Expected: `cargo-resolving invocations: 57, unlocked: 44`.

- [ ] **Step 2: Edit the three shared Rust task declarations**

In `.moon/tasks/rust.yml`:
- `build`: `command: 'cargo build'` becomes `command: 'cargo build --locked'`
- `build-release`: `command: 'cargo build --release'` becomes `command: 'cargo build --release --locked'`
- `test`: `command: 'cargo nextest run --no-tests=pass'` becomes `command: 'cargo nextest run --locked --no-tests=pass'`

Then rewrite the `lint` comment block at `:69-76`. It currently claims "Among the compile gates, only `lint` passes `--locked`", which this task makes false, and it cites `ci/publish-metadata/run.sh:243,258`, which is stale — the real `--locked` sites are `:742` and `:812`. Replace those two sentences with:

```
    # `--locked` so clippy lints the resolution the PR actually SHIPS. Without it cargo silently
    # re-resolves and rewrites an inconsistent Cargo.lock, and the thirteen lints below would
    # compile against newest-compatible versions instead — which would make the whole point of the
    # workspace inputs unprovable. Since SMA-601 EVERY cargo-resolving task in the graph passes
    # `--locked`, not `lint` alone: an unlocked invocation repairs a truncated lock in place before
    # any locked task reads it, which is how five Dependabot PRs merged a truncated lock through a
    # green `moon ci`. `repo:publish-metadata` also uses it, for its packaging checks
    # (`ci/publish-metadata/run.sh:742,812`). The flag set is asserted by
    # `repo:affected-smoke`'s A8 (ci/affected-graph/cargo_moon_parity.py).
```

- [ ] **Step 3: Edit the five `repo:*` declarations in `moon.yml`**

- `:19` becomes `command: 'cargo deny --locked --manifest-path rs/Cargo.toml check'`
- `:218`, inside the script, `cargo run -p paigasus-kernel-parity --bin gen-parity-vectors` becomes `cargo run --locked -p paigasus-kernel-parity --bin gen-parity-vectors`
- `:242` `cargo nextest run --no-tests=pass -p paigasus-observability --test drift` becomes `cargo nextest run --locked --no-tests=pass -p paigasus-observability --test drift`
- `:270` `cargo nextest run --no-tests=pass -p paigasus-iam --test nats_permissions --test docker_preflight --profile iam-nats` gains `--locked` immediately after `run`
- `:322` `cargo tree -p paigasus-wasm --target wasm32-unknown-unknown -e no-dev` becomes `cargo tree --locked -p paigasus-wasm --target wasm32-unknown-unknown -e no-dev`

- [ ] **Step 4: Add the `wasm-pack` cargo passthrough**

`wasm-pack build`'s `[EXTRA_OPTIONS]...` positional is documented as "List of extra options to pass to `cargo build`". In `ts/packages/paigasus-kernel/moon.yml`, append ` -- --locked` to **both** `wasm-pack build` invocations (`:43` and `:131`), after `--out-name paigasus_wasm`.

This does **not** make the task locked, and the plan originally claimed it did. The passthrough is real — a bogus flag there is rejected by cargo — but measured against a truncated 176-package lock, `wasm-pack build … -- --locked` still exits 0 and rewrites the lock to 548 packages: wasm-pack makes its own **unlocked** cargo call BEFORE the build it forwards to, and repairs the lock there. Keep the flag, because it does constrain the forwarded `cargo build`, and record the mechanism in a comment beside both invocations.

Do **not** attempt this for `napi build` or `uv sync` either. Measured against the pinned CLI, `napi build` exposes no `--locked` and no cargo passthrough, and cargo has no environment-variable equivalent; `uv sync` drives maturin with no flag path through either. All **three** wrapper tools are the residual, so `paigasus-kernel-ts:build`, `paigasus-kernel-ts:test` and `paigasus-kernel-py:test` all need Task 3 allowlist entries.

- [ ] **Step 5: Verify the unlocked count dropped and the graph still loads**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon query projects > /tmp/projects-after.json
# same python as Step 1, reading /tmp/projects-after.json
```

Expected: `cargo-resolving invocations: 57, unlocked: 0`. The count stays 57 because `--locked` changes no task's membership.

- [ ] **Step 6: Prove the flag bites, both directions**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cp rs/Cargo.lock /tmp/lock-main.toml
moon run repo:wasm-getrandom-free --force; echo "intact rc=$?"
cp /tmp/lock-181.toml rs/Cargo.lock
moon run repo:wasm-getrandom-free --force; echo "truncated rc=$?"
grep -c '^\[\[package\]\]' rs/Cargo.lock
git checkout -- rs/Cargo.lock
```

Expected: intact `rc=0`; truncated non-zero; and the package count still `176` after the truncated run — proving the task no longer repairs the lock. If it reads `548`, the flag did not reach cargo.

- [ ] **Step 7: Run the Rust graph to confirm nothing regressed**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:deny repo:wasm-getrandom-free repo:parity-corpus-drift paigasus-kernel-rs:build paigasus-kernel-rs:test
```

Expected: all PASS. A red here means the lock genuinely does not satisfy a manifest — fix the lock, do not remove the flag.

- [ ] **Step 8: Commit**

```bash
git add .moon/tasks/rust.yml moon.yml ts/packages/paigasus-kernel/moon.yml
git commit -m "ci(repo): pass --locked on every cargo-resolving task (SMA-601)"
```

---

### Task 3: A8 — assert every cargo-resolving task stays locked

**Files:**
- Modify: `ci/affected-graph/cargo_moon_parity.py` (constants near `FFI_MARKERS` at `:118`; a new check function beside `check_ffi_inputs` at `:322`; `collect_findings` at `:1391`; `EXPECTED_FINDING_KEYS`; `self_test`)

**Interfaces:**
- Consumes: `projects[pid]["invocations"]` — the resolved `command` + `script` + `args` joined, built at `cargo_moon_parity.py:626-633`, `None` when moon reported none of the three.
- Produces: `check_cargo_locked(projects, allow=ALLOW_UNLOCKED_CARGO, floor=REQUIRED_LOCKED_TASKS) -> list[str]`, and an `"a8"` entry in `collect_findings`' returned list.

- [ ] **Step 1: Write the failing self-test cases**

Add to `self_test()` in `cargo_moon_parity.py`, after the A7 block. Use the existing fixture style — a clean baseline plus controls that match a row **kind**, never mere non-emptiness:

```python
    # A8 (SMA-601): every task whose resolved invocation reaches cargo must pass --locked.
    # An unlocked one re-resolves and REWRITES an inconsistent lock in place, which is how five
    # Dependabot PRs merged a truncated lock through a green `moon ci`.
    locked_ok = {
        "a-rs": {"invocations": {"lint": "cargo clippy --locked --all-targets"}},
        "b-rs": {"invocations": {"build": "cargo build --locked"}},
        "k-ts": {"invocations": {"build": "pnpm exec napi build --platform"}},
    }
    if check_cargo_locked(locked_ok, allow={"k-ts:build": "napi has no --locked"},
                          floor=("a-rs:lint",)):
        failures.append("A8 reported violations on a clean fixture")

    # A8-a: an unlocked cargo invocation with no allowlist entry must fire, and name the task.
    broken = {"a-rs": {"invocations": {"lint": "cargo clippy --all-targets"}}}
    rows = check_cargo_locked(broken, allow={}, floor=("a-rs:lint",))
    if not any("a-rs:lint" in r for r in rows):
        failures.append("A8 did not fire on an unlocked cargo invocation")

    # A8-b: --frozen is NOT accepted. It implies --offline, which false-reds on a cold cargo
    # cache — the reason the gate itself refuses --offline.
    frozen = {"a-rs": {"invocations": {"lint": "cargo clippy --frozen"}}}
    if not check_cargo_locked(frozen, allow={}, floor=("a-rs:lint",)):
        failures.append("A8 accepted --frozen, which implies --offline")

    # A8-c: an allowlist entry with an empty reason must be rejected, like A6-d's.
    rows = check_cargo_locked(broken, allow={"a-rs:lint": ""}, floor=("a-rs:lint",))
    if not rows:
        failures.append("A8 accepted an allowlist entry with an empty reason")

    # A8-d: the FLOOR must fire when the derivation degrades to empty — a derived set that
    # matches nothing asserts nothing while still printing PASS (the A5 lesson).
    rows = check_cargo_locked({"a-rs": {"invocations": {"lint": "echo nothing"}}},
                              allow={}, floor=("a-rs:lint",))
    if not any("A8 examines" in r for r in rows):
        failures.append("A8 floor did not fire when a required task stopped matching")

    # A8-e: an absent invocation is infra-shaped, never a silent skip. Mirrors A5.
    try:
        check_cargo_locked({"a-rs": {"invocations": {"lint": None}}}, allow={},
                           floor=("a-rs:lint",))
        failures.append("A8 did not raise infra on a task with no command and no script")
    except MoonOutputError:
        pass
```

- [ ] **Step 2: Run the self-test to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 ci/affected-graph/cargo_moon_parity.py --self-test
```

Expected: FAIL with `NameError: name 'check_cargo_locked' is not defined`.

- [ ] **Step 3: Add the constants**

Insert immediately after `REQUIRED_FFI_TASKS` (`cargo_moon_parity.py:127-131`):

```python
# SMA-601 — the cargo subcommands that RESOLVE the dependency graph, and therefore rewrite an
# inconsistent Cargo.lock in place unless --locked is passed. `fmt` and `machete` are absent
# deliberately: neither reads the lock. Matched against the same resolved `command` + `args` +
# `script` blob A5 uses, NOT against file text — a text scan of moon.yml/.moon/tasks/*.yml/
# rs/Dockerfile/ci/**/*.sh was measured at 45 matches of which ~14 were real invocations, because
# `moon.yml:323` is `echo "cargo tree failed …"` on an EXECUTING line and
# `ci/publish-metadata/run.sh:179` is a Python f-string inside a heredoc. The resolved blob has
# no prose: measured at 57 matches, 0 false positives.
LOCK_RESOLVING_VERBS = (
    "bench", "build", "check", "clippy", "deny", "doc", "fetch", "metadata",
    "nextest", "package", "publish", "run", "test", "tree", "update",
)

CARGO_INVOCATION_RE = re.compile(
    r"\bcargo\s+(?:\+\S+\s+)?(?:" + "|".join(LOCK_RESOLVING_VERBS) + r")\b"
)

# A wrapper reaches cargo without the literal token, so A8 matches FFI_MARKERS too. Without this
# the three wrapper tasks would be silently OUT of scope rather than visibly allowlisted.
# `--locked` is accepted; `--frozen` is NOT — it implies `--offline`, which false-reds on a cold
# cargo cache.
LOCKED_FLAG = "--locked"

# Tasks that cannot pass the flag, each with the measured reason. Same idiom as T_EXEMPT and
# ALLOW_DEAD_INPUT: an exemption is allowed, a SILENT one is not.
ALLOW_UNLOCKED_CARGO = {
    "paigasus-kernel-ts:build": (
        "reaches cargo through TWO wrappers, neither of which can guarantee a locked resolution "
        "(both measured, SMA-601). `napi build` exposes no --locked and no cargo passthrough, and "
        "cargo has no env-var equivalent. `wasm-pack build ... -- --locked` DOES forward the flag "
        "to the cargo build it wraps, but wasm-pack makes its OWN unlocked cargo call BEFORE that "
        "build and repairs the lock there: measured against a truncated 176-package lock it exits "
        "0 and rewrites the lock to 548. The flag is kept anyway — it constrains the forwarded "
        "build — but it does not lock the task."
    ),
    "paigasus-kernel-ts:test": "as paigasus-kernel-ts:build",
    "paigasus-kernel-py:test": (
        "reaches cargo through `uv sync --reinstall-package`, which drives maturin, which drives "
        "cargo — no flag path through either (SMA-601)"
    ),
}

# The floor, for the reason REQUIRED_FFI_TASKS carries: a derived set that shrinks to EMPTY
# asserts nothing while still printing PASS. Every task named here MUST be in the derived set.
REQUIRED_LOCKED_TASKS = (
    "paigasus-kernel-rs:lint",
    "paigasus-iam-rs:test",
    "repo:deny",
    "repo:wasm-getrandom-free",
)
```

Confirm `import re` is already present at the top of the file; add it if not.

- [ ] **Step 4: Add the check function**

The body below tests `LOCKED_FLAG in blob` for every matched task. That is correct for a literal `cargo <verb>` match, and **vacuous for a wrapper match** — a consequence of the wasm-pack correction above. `paigasus-kernel-ts:build` runs an unlocked `napi build` beside a `wasm-pack build … -- --locked`, so a blob-level test greens a task whose own cargo call still repairs the lock. The implemented version therefore requires an `ALLOW_UNLOCKED_CARGO` entry for **every** FFI-marker match, whether or not `--locked` appears in its blob, and a task matching both kinds is governed by the wrapper rule. Read `check_cargo_locked`'s docstring for the shipped contract.

Insert after `derive_ffi_tasks` (`cargo_moon_parity.py:318`):

```python
def check_cargo_locked(projects, allow=ALLOW_UNLOCKED_CARGO, floor=REQUIRED_LOCKED_TASKS):
    """Return the A8 violation list: cargo-resolving tasks that do not pass --locked.

    Raises MoonOutputError if a task in the floor exposes none of a command, a script, or any
    args — the same absent-invocation contract A5 uses.
    """
    rows = []
    matched = set()
    for pid in sorted(projects):
        invocations = projects[pid].get("invocations") or {}
        for name in sorted(invocations):
            target = f"{pid}:{name}"
            blob = invocations[name]
            if blob is None:
                if target in floor:
                    raise MoonOutputError(
                        f"{target} reported none of a `command`, a `script`, or any `args` — "
                        f"moon's output shape changed, so A8 cannot be evaluated"
                    )
                continue
            is_wrapper = any(marker in blob for marker in FFI_MARKERS)
            if not (is_wrapper or CARGO_INVOCATION_RE.search(blob)):
                continue
            matched.add(target)
            # A wrapper is NEVER cleared by the flag — see the note above this block.
            if not is_wrapper and LOCKED_FLAG in blob:
                continue
            reason = allow.get(target)
            if reason is None:
                rows.append(
                    f"{target} reaches cargo without {LOCKED_FLAG} — it will re-resolve and "
                    f"REWRITE an inconsistent Cargo.lock in place: {blob[:120]}"
                )
            elif not reason.strip():
                rows.append(
                    f"{target} is in ALLOW_UNLOCKED_CARGO with an empty reason — an exemption "
                    f"is allowed, a silent one is not"
                )
    for target in floor:
        if target not in matched:
            rows.append(
                f"A8 examines {len(matched)} task(s) and {target} is not among them — the "
                f"derivation has degraded and would assert nothing"
            )
    return rows
```

- [ ] **Step 5: Wire it into `collect_findings` and `EXPECTED_FINDING_KEYS`**

In `collect_findings`, compute `a8 = check_cargo_locked(projects)` beside `a5 = check_ffi_inputs(projects)`, and append to the `findings` list, after the `a7` entry:

```python
        ("a8", a8,
             "Cargo-resolving task without --locked (it REPAIRS a truncated lock mid-run,\n"
             "    so every later --locked gate reads a lock the PR never shipped — SMA-601).\n"
             "    Fix: add `--locked` to the task's command, or add an ALLOW_UNLOCKED_CARGO\n"
             "    entry with the measured reason it cannot take one.\n"
             "    An `A8 examines` row means the opposite — the derivation stopped matching a\n"
             "    task it must cover; fix that first, every other A8 row is meaningless until\n"
             "    it passes."),
```

Append `"a8"` to `EXPECTED_FINDING_KEYS`. The `self_test` assertions at `:838-853` compare both the length and the exact key tuple, so both must be updated together.

- [ ] **Step 6: Run the self-test and the real gate to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 ci/affected-graph/cargo_moon_parity.py --self-test
moon run repo:affected-smoke --force
```

Expected: both PASS. A8 must report zero rows, because Task 2 already locked every non-wrapper invocation. If A8 reports rows naming crate tasks, Task 2 is incomplete — fix Task 2, do not widen the allowlist.

- [ ] **Step 7: Prove A8 bites**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
sed -i.bak "s/cargo deny --locked --manifest-path/cargo deny --manifest-path/" moon.yml
moon run repo:affected-smoke --force; echo "rc=$?"
mv moon.yml.bak moon.yml
touch moon.yml
moon run repo:affected-smoke --force; echo "restored rc=$?"
```

Expected: non-zero with a row naming `repo:deny`, then `restored rc=0`. The `touch` is required — restoring a file by `mv` rolls its mtime **backwards**, so the tool reuses a stale cached result.

- [ ] **Step 8: Commit**

```bash
git add ci/affected-graph/cargo_moon_parity.py
git commit -m "ci(repo): assert every cargo-resolving task passes --locked (SMA-601)"
```

---

### Task 4: The `rs/Dockerfile` assertion and its input wiring

**Files:**
- Modify: `ci/affected-graph/cargo_moon_parity.py` (extend the A8 rows)
- Modify: `moon.yml` (`repo:affected-smoke` `inputs`, `:169-204`)
- Modify: `ci/actionlint/run.sh` (`T_AFFECTED_SMOKE_REQUIRED_INPUTS`, `:2097-2118`)

**Interfaces:**
- Consumes: `check_cargo_locked` from Task 3.
- Produces: `check_dockerfile_locked(root) -> list[str]`, folded into the same `"a8"` finding rows.

- [ ] **Step 1: Write the failing self-test case**

Add to `self_test()`, after the A8-e case:

```python
    # A8-f: rs/Dockerfile is outside moon's view, so it takes a narrow text assertion of its own.
    # One RUN line, one verb — none of the prose-collision risk a general text scan carries.
    with tempfile.TemporaryDirectory() as tmp:
        rs = Path(tmp) / "rs"
        rs.mkdir()
        (rs / "Dockerfile").write_text("RUN cargo build --release --locked -p paigasus-iam\n")
        if check_dockerfile_locked(Path(tmp)):
            failures.append("A8 reported a violation on a locked Dockerfile")
        (rs / "Dockerfile").write_text("RUN cargo build --release -p paigasus-iam\n")
        if not check_dockerfile_locked(Path(tmp)):
            failures.append("A8 did not fire on an unlocked Dockerfile cargo build")
        (rs / "Dockerfile").unlink()
        try:
            check_dockerfile_locked(Path(tmp))
            failures.append("A8 did not raise infra on a missing rs/Dockerfile")
        except MoonOutputError:
            pass
```

- [ ] **Step 2: Run the self-test to verify it fails**

Run: `python3 ci/affected-graph/cargo_moon_parity.py --self-test`
Expected: FAIL with `NameError: name 'check_dockerfile_locked' is not defined`.

- [ ] **Step 3: Implement the function**

Insert after `check_cargo_locked`:

```python
def check_dockerfile_locked(root):
    """Return A8 rows for rs/Dockerfile, which moon's task graph cannot see.

    A narrow, line-oriented assertion rather than a general text scan: the file holds one cargo
    line and no prose that mentions a cargo verb, so the false-positive rate that killed the
    general scan does not apply. A missing file is infrastructure, never a silent pass.
    """
    path = root / "rs" / "Dockerfile"
    if not path.is_file():
        raise MoonOutputError(
            f"{path} is absent — A8's Dockerfile assertion cannot be evaluated. If the file "
            f"legitimately moved, update check_dockerfile_locked rather than deleting the check"
        )
    rows = []
    seen = 0
    for lineno, line in enumerate(path.read_text().splitlines(), 1):
        stripped = line.split("#", 1)[0]
        if not CARGO_INVOCATION_RE.search(stripped):
            continue
        seen += 1
        if LOCKED_FLAG not in stripped:
            rows.append(
                f"rs/Dockerfile:{lineno} reaches cargo without {LOCKED_FLAG}: {stripped.strip()}"
            )
    # The floor, for the reason REQUIRED_LOCKED_TASKS carries: zero matches asserts nothing while
    # still printing PASS.
    if seen == 0:
        rows.append(
            "A8 examines rs/Dockerfile and found no cargo invocation at all — the image build "
            "stopped compiling in this file, so this assertion now covers nothing"
        )
    return rows
```

In `collect_findings`, change the a8 computation to `a8 = check_cargo_locked(projects) + check_dockerfile_locked(root)`. `root` is already a parameter of `collect_findings`.

- [ ] **Step 4: Run the self-test to verify it passes**

Run: `python3 ci/affected-graph/cargo_moon_parity.py --self-test`
Expected: PASS.

- [ ] **Step 5: Add `rs/Dockerfile` to both input registries**

In `moon.yml`, inside `repo:affected-smoke`'s `inputs`, after the `'rs/**/Cargo.toml'` entry:

```yaml
      # SMA-601 — A8 ASSERTS on this file (moon's task graph cannot see a Dockerfile RUN line),
      # so without it the assertion is real but unreachable behind a cached PASS on exactly the
      # PR that drops `--locked` from the image build. Same reasoning as the ci/actionlint/**/*
      # entry below.
      - 'rs/Dockerfile'
```

In `ci/actionlint/run.sh`, add `'rs/Dockerfile'` to `T_AFFECTED_SMOKE_REQUIRED_INPUTS`. That check tests **containment**, not order or arity equality (`ci/actionlint/run.sh:2321-2331`; the comment at `:2087` says "CONTAINMENT, not equality"), so position in the array is free and the arity floor of 20 at `ci_targets.py:657` stays satisfied at 21.

- [ ] **Step 6: Verify both gates**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:affected-smoke --force
moon run repo:actionlint --force
```

Expected: both PASS. If `repo:actionlint` reports `missing-input rs/Dockerfile`, Step 5's `moon.yml` half was not applied.

- [ ] **Step 7: Commit**

```bash
git add ci/affected-graph/cargo_moon_parity.py moon.yml ci/actionlint/run.sh
git commit -m "ci(repo): assert rs/Dockerfile keeps --locked and key A8 on it (SMA-601)"
```

---

### Task 5: Pin the `ci.yml` step against deletion

**Files:**
- Modify: `ci/actionlint/run.sh` (a new constant, a new verdict function, a new `*_self_test` table, `SELF_TEST_COUNT` at `:40`, and a call in `run_self_tests` at `:4016`)

**Interfaces:**
- Consumes: the exact `name:` and `run:` lines written in Task 1 Step 5.
- Produces: check 8f. `SELF_TEST_COUNT` rises from 10 to 11; `selftest_mutation_battery` extends automatically from the `*_self_test` definition count, and asserts the two agree.

- [ ] **Step 1: Add the pinned constant**

Near `T_AFFECTED_SMOKE_REQUIRED_INPUTS`, add:

```bash
# SMA-601 — check 8f. The lockfile-integrity step is a plain ci.yml step, not a Moon task, so
# none of ci_targets.py's registries can see it: no T entry, no SELF_SCHEDULED_GATES row. The
# codegen-drift step has the same exposure and carries no pin; this one does. Whole lines,
# compared after stripping.
T_CARGO_LOCK_STEP_REQUIRED=(
  '- name: Cargo lockfile integrity (rs/Cargo.lock satisfies every manifest)'
  'run: bash ci/cargo-lock-integrity/run.sh'
)
```

- [ ] **Step 2: Write the failing self-test table**

Add `cargo_lock_step_self_test()` beside `affected_smoke_block_self_test` (`:2995`), mirroring its shape. Write it in full:

```bash
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

  local wired
  wired="jobs:
  ci:
    steps:
      - name: Cargo lockfile integrity (rs/Cargo.lock satisfies every manifest)
        run: bash ci/cargo-lock-integrity/run.sh

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
        run: bash ci/cargo-lock-integrity/run.sh
"
  expect_step 'the step after moon ci is out of order' 'out-of-order' "$reordered"

  expect_step 'a missing run: line is reported' \
    'missing-line run: bash ci/cargo-lock-integrity/run.sh' \
    "$(printf '%s' "$wired" | grep -vxF -e '        run: bash ci/cargo-lock-integrity/run.sh')"

  expect_step 'a missing name: line is reported' \
    'missing-line - name: Cargo lockfile integrity (rs/Cargo.lock satisfies every manifest)' \
    "$(printf '%s' "$wired" | grep -vxF \
        -e '      - name: Cargo lockfile integrity (rs/Cargo.lock satisfies every manifest)')"

  # continue-on-error: true would let the step red and the job stay green — a silent bypass.
  local coe_true coe_false
  coe_true="${wired/        run: bash ci\/cargo-lock-integrity\/run.sh/        run: bash ci\/cargo-lock-integrity\/run.sh
        continue-on-error: true}"
  expect_step 'continue-on-error: true is reported' 'continue-on-error true' "$coe_true"

  coe_false="${wired/        run: bash ci\/cargo-lock-integrity\/run.sh/        run: bash ci\/cargo-lock-integrity\/run.sh
        continue-on-error: false}"
  expect_step 'continue-on-error: false is clean' '' "$coe_false"

  return "$rc"
}
```

Six cases. If `fail` is not in scope at that point in the file, use the same reporting call `affected_smoke_block_self_test` uses.

- [ ] **Step 3: Run the self-test to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
bash ci/actionlint/run.sh --self-test
```

Expected: FAIL with `cargo_lock_step_verdict: command not found`.

- [ ] **Step 4: Implement `cargo_lock_step_verdict` and call the table**

Write it in full, beside the other verdict functions:

```bash
# Check 8f (SMA-601). Echoes one row per violation, nothing when clean. Row vocabulary:
#   missing-line <text>       a required line is absent
#   out-of-order              the step does not precede the `moon ci` step
#   continue-on-error <value> the step's continue-on-error is anything but the literal false
cargo_lock_step_verdict() { # $1 workflow file
  local f="$1" line stripped n_step n_moon coe

  for line in "${T_CARGO_LOCK_STEP_REQUIRED[@]}"; do
    if ! grep -qxF -e "$line" <(sed 's/^[[:space:]]*//' "$f"); then
      echo "missing-line $line"
    fi
  done

  # Placement is the guarantee, so ordering is asserted, not assumed. Both greps are anchored on
  # the stripped text so indentation changes do not defeat them.
  n_step="$(sed 's/^[[:space:]]*//' "$f" | grep -nxF \
    -e '- name: Cargo lockfile integrity (rs/Cargo.lock satisfies every manifest)' \
    | head -1 | cut -d: -f1)"
  n_moon="$(sed 's/^[[:space:]]*//' "$f" | grep -nxF \
    -e '- name: moon ci (affected graph)' | head -1 | cut -d: -f1)"
  if [ -n "$n_step" ] && [ -n "$n_moon" ] && [ "$n_step" -gt "$n_moon" ]; then
    echo "out-of-order"
  fi

  # Anything but the literal `false` suppresses the step's failure. Same rule check 8 applies to
  # the moon ci step.
  if [ -n "$n_step" ]; then
    coe="$(sed 's/^[[:space:]]*//' "$f" | sed -n "$((n_step + 1)),$((n_step + 4))p" \
      | grep -m1 '^continue-on-error:' | sed 's/^continue-on-error:[[:space:]]*//')"
    if [ -n "$coe" ] && [ "$coe" != "false" ]; then
      echo "continue-on-error $coe"
    fi
  fi
}
```

Then call it against the real workflow in the check-8 region, reporting each row through `fail`; add `cargo_lock_step_self_test` to `run_self_tests`; and bump `SELF_TEST_COUNT` from `10` to `11`, extending the trailing comment at `:40-41` with `cargo-lock-step`.

- [ ] **Step 5: Run the self-test and the real gate to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
bash ci/actionlint/run.sh --self-test
moon run repo:actionlint --force
```

Expected: both PASS, and the self-test reports 11 tables. If it reports a definitions/count mismatch, `SELF_TEST_COUNT` was not bumped.

- [ ] **Step 6: Prove check 8f bites**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cp .github/workflows/ci.yml /tmp/ci.yml.orig
grep -v 'run: bash ci/cargo-lock-integrity/run.sh' /tmp/ci.yml.orig > .github/workflows/ci.yml
moon run repo:actionlint --force; echo "rc=$?"
cp /tmp/ci.yml.orig .github/workflows/ci.yml
touch .github/workflows/ci.yml
moon run repo:actionlint --force; echo "restored rc=$?"
```

Expected: non-zero with a `missing-line` row, then `restored rc=0`.

- [ ] **Step 7: Commit**

```bash
git add ci/actionlint/run.sh
git commit -m "ci(repo): pin the cargo-lock-integrity step in ci.yml (SMA-601)"
```

---

### Task 6: Document the mechanism and verify the whole graph

**Files:**
- Modify: `CLAUDE.md` (a new Gotchas entry)

**Interfaces:**
- Consumes: everything above.
- Produces: nothing consumed by later tasks.

- [ ] **Step 1: Add the CLAUDE.md gotcha entry**

Add to the Gotchas list. Do **not** touch the marker-delimited `ci-targets` command — this issue adds no `repo:*` task, so `T` is unchanged, and a second copy of either marker reds `repo:affected-smoke`.

```markdown
- An **unlocked cargo invocation repairs a truncated `rs/Cargo.lock` in place, mid-run**, and that
  is why five Dependabot PRs (83, 96, 140, 149, 181) merged a truncated lock through a green
  `moon ci`. Measured on PR 181's `72c0ddb52` (176 packages against main's 543, holding 5 of 13
  workspace members): `cargo tree` and `cargo deny` each re-resolved the lock to **548 packages and
  exited 0**, both starting at 06:37:55 — twelve seconds before the first `--locked` task. So
  `paigasus-gateway-rs:lint` and `paigasus-iam-rs:lint` ran `cargo clippy --locked` for real, for
  24s and 72s, against a lock that had already been repaired, and passed. The repaired lock is
  never committed, so `main` keeps the truncated one. Two consequences. A gate that reads the lock
  from the WORKING TREE inside `moon ci` races the repair and is worthless — which is why
  `ci/cargo-lock-integrity/run.sh` is an unconditional **`ci.yml` step placed before the `moon ci`
  step** (pinned by check 8f in `ci/actionlint/run.sh`), not a `repo:*` task. And `cargo deny`
  audits a re-resolved graph whenever the lock does not already satisfy the manifests — not on
  every PR, since cargo rewrites nothing when the lock is consistent, but on exactly the PRs that
  matter. Since SMA-601 every cargo-resolving task passes `--locked`, asserted generically by A8
  (`ci/affected-graph/cargo_moon_parity.py`); the three FFI wrapper tasks cannot, because
  `napi build` exposes no `--locked` and no cargo passthrough, `uv sync` drives maturin with no
  flag path, and `wasm-pack` — which DOES forward `-- --locked` — makes its own unlocked cargo
  call before the forwarded build and repairs the lock there (measured: 176 -> 548 packages,
  exit 0). All three carry `ALLOW_UNLOCKED_CARGO` entries, and A8 demands one for every
  wrapper-matched task even when `--locked` appears elsewhere in its script. `--locked` proves the lock is
  CONSISTENT with the manifests, not that it is correct: a swapped-but-compatible version or a
  tampered checksum still passes.
```

- [ ] **Step 2: Verify the CLAUDE.md marker count is still 1**

```bash
grep -c 'ci-targets:begin' CLAUDE.md
grep -c 'ci-targets:end' CLAUDE.md
```

Expected: `1` and `1`. Any other value reds `repo:affected-smoke`.

- [ ] **Step 3: Run the full graph exactly as CI does**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
unset AI_AGENT CLAUDECODE CLAUDE_CODE_ENTRYPOINT
bash ci/cargo-lock-integrity/run.sh
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :http-extractor-envelope :input-liveness :promtool :observability-drift \
  :nats-permissions :release-parity :release-parity-py :release-parity-ts \
  :publish-metadata :version-lockstep :workflow-credentials --base origin/main --include-relations
```

The `unset` is required: `proto` emits NDJSON on stdout when it detects an agent environment, which aborts all three `repo:release-parity*` gates with rc 2. Diagnose any unattributed failure through `.moon/cache/ciReport.json`.

Expected: all green. `repo:affected-smoke` is the one most likely to red — read its A8 rows first.

- [ ] **Step 4: Commit**

```bash
git add CLAUDE.md
git commit -m "docs(repo): record the mid-run Cargo.lock repair mechanism (SMA-601)"
```
