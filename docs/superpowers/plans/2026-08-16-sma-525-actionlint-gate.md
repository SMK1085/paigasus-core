# actionlint Workflow Gate Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `repo:actionlint` CI gate that lints `.github/workflows/**` with a proto-pinned actionlint and additionally proves every `paths:` filter glob actually matches the tree, so a broken filter can no longer silently disable a workflow forever.

**Architecture:** One vendored proto TOML plugin pins actionlint 1.7.12. One shell script, `ci/actionlint/run.sh`, runs seven checks: the linter itself, a control that no config file neuters it, four rule-tagged stdin self-tests plus a healthy control, and a path-filter existence check with its own extractor self-test. A `repo:actionlint` Moon task runs the script and `ci.yml` adds `:actionlint` to its target array.

**Tech Stack:** bash, awk, git pathspecs, proto TOML plugins, Moon 2.3.2, GitHub Actions.

**Spec:** `docs/superpowers/specs/2026-08-16-sma-525-actionlint-gate-design.md` — read it before starting. Every decision below traces to a D-number or §-number there.

> **Amended after the final whole-branch review.** The shell excerpts below are the code **as planned**; three findings changed it after they were written, and `ci/actionlint/run.sh` is the authority:
>
> - The extractor emits a `KEY` only for a `paths:`/`paths-ignore:` **two levels deep** inside `on:` (`on.<event>.paths`). A workflow input legitimately *named* `paths` sits one level deeper and is ignored; a flow-mapping event value (`push: { paths: [...] }`) emits a `KEY` with no items so check 6 fails loudly instead of skipping the event in silence.
> - Check 2 is an **allowlist**: `self-hosted-runner` is the only top-level key permitted in an actionlint config, and an `ignore` key is rejected in any style. The block-style-only `grep` shown below could be bypassed by a single flow-style line.
> - `check_pattern` was split into a pure `pattern_verdict` (echoes a stable token) plus a production call site that turns a non-`ok` verdict into `fail`, and the check-6 loop into `scan_workflow_records`. That is what makes `path_filter_self_test` possible — the standing control for checks 5 and 6, without which a mutation battery neutered both and the gate still exited 0.

## Global Constraints

- Every source file opens with an SPDX header: `# SPDX-License-Identifier: Apache-2.0` for shell (line 2, after the shebang — see `ci/osv/run.sh`).
- Branch is `feature/sma-525-repo-gate-workflow-yaml-with-actionlint`. Conventional commits with a workspace scope, subject **lowercase**, header **≤100 chars**. Never write `#NNN` in a commit body (breaks `footer-leading-blank`); write "owner/repo PR NNN".
- Do **not** use `--no-verify`. The worktree already has `ts/node_modules` installed, so commitlint runs.
- The Bash tool's PATH lacks proto CLIs. Prefix every command needing them with:
  `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`
- actionlint version is exactly `1.7.12`.
- `run.sh` uses `set -uo pipefail` — **never** `set -e` (spec §4.2: check 3 deliberately expects non-zero exits).
- Exit codes: **1** = assertion failure, **2** = infrastructure error. (`ci/` convention.)
- When reverting a mutated file during verification, use `git checkout -- <file>`. Never restore from a `.bak` copy — that rolls mtime backwards and produces confusing stale results.
- Never create a file whose base name is a Windows reserved device name (`CON`, `PRN`, `AUX`, `NUL`, `COM1`–`COM9`, `LPT1`–`LPT9`).

---

## File Structure

| File | Responsibility |
|---|---|
| `.proto/plugins/actionlint.toml` | Resolve + checksum official actionlint release assets per platform |
| `.prototools` | Pin `actionlint = "1.7.12"`, register the plugin |
| `ci/actionlint/run.sh` | All seven checks. Single script; the checks share `ARGS` and a failure flag |
| `ci/actionlint/README.md` | Why the gate exists, what each check proves, how to add a skip-list entry |
| `moon.yml` | `repo:actionlint` task |
| `.github/workflows/ci.yml` | `:actionlint` in `T=(…)`, step-name update |
| `CLAUDE.md` | Add `:actionlint` to the documented full-graph command |

---

### Task 1: Pin actionlint via a proto plugin

**Files:**
- Create: `.proto/plugins/actionlint.toml`
- Modify: `.prototools`

**Interfaces:**
- Consumes: nothing.
- Produces: an `actionlint` binary on the proto shim PATH, version `1.7.12`. Later tasks invoke it as bare `actionlint`.

- [ ] **Step 1: Write the plugin**

Create `.proto/plugins/actionlint.toml`. Facts verified against the real v1.7.12 release (spec §4.1): the binary sits at **archive root** so no `exe-path`; a combined `actionlint_{version}_checksums.txt` exists; asset names use Go's `amd64`/`arm64` on all three OSes so one global `[install.arch]` suffices; tags are `v`-prefixed while filenames carry the bare version.

```toml
# Vendored proto TOML plugin for actionlint (SMA-525).
#
# Resolves official, checksummed rhysd/actionlint GitHub release assets. Same vendoring
# rationale as buf/cargo-deny/promtool: a static schema over official release assets —
# nothing upstream to maintain.
#
# Assets are tarballs (zip on Windows) whose binary sits at the ARCHIVE ROOT, so unlike
# promtool and cargo-deny — whose binaries nest one directory deep — NO `exe-path` is needed
# on any platform. Closest shape here is osv-scanner (bare payload, global arch remap).
#
# actionlint names assets with Go's GOOS/GOARCH: actionlint_{version}_{linux,darwin,windows}_
# {amd64,arm64}. proto's default {arch} tokens are Rust triples (x86_64/aarch64), so BOTH
# arches need remapping. There is no per-OS naming divergence, so a single global
# [install.arch] covers every platform and no [platform.*.arch] override is required (which
# is what the SMA-411 proto floor exists for — irrelevant here).
#
# Tags are "v"-prefixed (v1.7.12) while asset filenames embed the BARE version — the promtool
# shape. The checksum filename interpolates {version} (precedent: cargo-deny, cargo-machete,
# cargo-nextest), unlike promtool/osv-scanner whose checksum assets have static names.

name = "actionlint"
type = "cli"

[platform.linux]
download-file = "actionlint_{version}_linux_{arch}.tar.gz"
checksum-file = "actionlint_{version}_checksums.txt"

[platform.macos]
download-file = "actionlint_{version}_darwin_{arch}.tar.gz"
checksum-file = "actionlint_{version}_checksums.txt"

[platform.windows]
download-file = "actionlint_{version}_windows_{arch}.zip"
checksum-file = "actionlint_{version}_checksums.txt"

[install]
download-url = "https://github.com/rhysd/actionlint/releases/download/v{version}/{download_file}"
checksum-url = "https://github.com/rhysd/actionlint/releases/download/v{version}/{checksum_file}"

# Go GOARCH naming; uniform across all three OSes.
[install.arch]
x86_64 = "amd64"
aarch64 = "arm64"

[resolve]
git-url = "https://github.com/rhysd/actionlint"
```

- [ ] **Step 2: Register the pin**

In `.prototools`, add `actionlint = "1.7.12"` to the alphabetical tool list (it goes **first**, before `buf = "1.70.0"`, but after the `proto = "0.58.1"` line and its comment block). Then add to the `[plugins]` table, also first:

```toml
actionlint = "file://./.proto/plugins/actionlint.toml"
```

- [ ] **Step 3: Verify the plugin resolves and the checksum is honoured**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
proto install actionlint
actionlint --version
```

Expected: installs without a checksum error, and `--version` prints `1.7.12` on its first line.

If it prints a *different* version, a globally-pinned actionlint is shadowing the repo pin — confirm shims come first on PATH.

- [ ] **Step 4: Commit**

```bash
git add .proto/plugins/actionlint.toml .prototools
git commit -m "build(repo): pin actionlint 1.7.12 via a vendored proto plugin (SMA-525)"
```

---

### Task 2: Script skeleton, check 1 (lint) and check 2 (config integrity)

**Files:**
- Create: `ci/actionlint/run.sh`

**Interfaces:**
- Consumes: the `actionlint` binary from Task 1.
- Produces: `ci/actionlint/run.sh`, executable, accepting no arguments (Task 4 adds `--self-test`). Exposes to later tasks: the `ARGS` array, the `fail()` helper, the `FAILED` flag, and the `WORKFLOW_FILES` array.

**Why `set -uo pipefail` and not `set -e`:** several checks deliberately expect and inspect non-zero exits — check 3 (Task 3) requires actionlint to *fail* on each fixture, and the verdict helpers of checks 5/6 signal through their status. Under `-e` those abort the script with an opaque status. Instead every check captures status explicitly and sets `FAILED`, so a failing check cannot be masked by a later passing one. This mirrors `ci/osv/run.sh`.

- [ ] **Step 1: Write the skeleton with checks 1 and 2**

```bash
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

# ---------------------------------------------------------------------------------------------
# Check 1 — lint every workflow.
#
# Invoked BARE, with no file arguments, relying on actionlint's repository auto-discovery. Two
# reasons: a `*.yml` argument list would silently miss a `.yaml`-suffixed workflow, and
# actionlint's exit-3-on-empty behaviour (which is what makes "the directory vanished" loud
# rather than a vacuous pass) applies ONLY to the auto-discovery path — an explicit glob that
# expands to nothing would exit 0 as "no errors found".
# ---------------------------------------------------------------------------------------------
# Capture the status BEFORE testing it. Inside `if ! cmd; then`, `$?` is the status of the
# negation (always 0), which would make the exit-3 branch below dead code and print the wrong
# code in the message. Verified: `if ! f; then rc=$?` yields 0 for a function returning 3.
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

exit "$FAILED"
```

- [ ] **Step 2: Make it executable and verify it passes on the clean tree**

```bash
chmod +x ci/actionlint/run.sh
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
ci/actionlint/run.sh; echo "exit=$?"
```

Expected: no output, `exit=0`.

- [ ] **Step 3: Prove check 1 bites (red test)**

```bash
sed -i '' 's/^    runs-on: ubuntu-latest$/    runs-on: ubunut-latest/' .github/workflows/ci.yml
ci/actionlint/run.sh; echo "exit=$?"
git checkout -- .github/workflows/ci.yml
```

Expected: prints a `[runner-label]` finding naming `ubunut-latest`, then `exit=1`.

- [ ] **Step 4: Prove check 2 bites (red test)**

```bash
cat > .github/actionlint.yaml <<'YAML'
paths:
  ".github/workflows/**":
    ignore:
      - ".*"
YAML
ci/actionlint/run.sh; echo "exit=$?"
rm -f .github/actionlint.yaml
```

Expected: the "contains an 'ignore:' key" message, `exit=1`.

Then confirm the allowed form does **not** trip it:

```bash
printf 'self-hosted-runner:\n  labels:\n    - my-runner\n' > .github/actionlint.yaml
ci/actionlint/run.sh; echo "exit=$?"
rm -f .github/actionlint.yaml
```

Expected: `exit=0`.

- [ ] **Step 5: Confirm the tree is clean, then commit**

```bash
git status --short   # must show ONLY the new ci/actionlint/run.sh
git add ci/actionlint/run.sh
git commit -m "feat(repo): add the actionlint gate script with lint + config-integrity checks (SMA-525)"
```

---

### Task 3: Checks 3 and 4 — rule-tagged self-tests

**Files:**
- Modify: `ci/actionlint/run.sh` (append before the final `exit "$FAILED"`)

**Interfaces:**
- Consumes: `ARGS`, `fail`, `infra` from Task 2.
- Produces: nothing consumed later.

**Why rule tags and not just exit status:** "a malformed workflow must fail" is satisfied by *any* non-zero exit, including a YAML parse error — which proves nothing about runner labels or expressions. A targeted `-ignore 'label .* is unknown'` added to check 1 would leave a status-only control green. Asserting the rule tag makes check 3 the standing version of the spec's §2 evidence table, one fixture per AC-1 class.

- [ ] **Step 1: Append checks 3 and 4**

```bash
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
```

- [ ] **Step 2: Verify the whole script still passes**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
ci/actionlint/run.sh; echo "exit=$?"
```

Expected: no output, `exit=0`.

- [ ] **Step 3: Prove check 3 bites (red test) — simulate a neutered linter**

Temporarily add a blanket ignore to the shared `ARGS` array to simulate someone silencing a rule:

```bash
sed -i '' "s/^ARGS=(-shellcheck= -pyflakes=)$/ARGS=(-shellcheck= -pyflakes= -ignore '.*')/" ci/actionlint/run.sh
ci/actionlint/run.sh; echo "exit=$?"
git checkout -- ci/actionlint/run.sh 2>/dev/null || sed -i '' "s/^ARGS=(-shellcheck= -pyflakes= -ignore '.*')$/ARGS=(-shellcheck= -pyflakes=)/" ci/actionlint/run.sh
```

Expected: four "ACCEPTED a deliberately broken workflow" failures, `exit=1`. This is the neutering scenario D3 exists for.

Note: `git checkout --` only works if the file is committed; on the first run it is not, so the `sed` fallback restores it. Confirm afterwards with `grep -n '^ARGS=' ci/actionlint/run.sh` that the line reads exactly `ARGS=(-shellcheck= -pyflakes=)`.

- [ ] **Step 4: Prove check 4 bites (red test)**

Corrupt the healthy fixture so the control has something to catch:

```bash
perl -0pi -e "s/healthy='name: selftest\non:/healthy='name: selftest\nonn:/" ci/actionlint/run.sh
ci/actionlint/run.sh; echo "exit=$?"
```

Expected: `actionlint REJECTED a known-good workflow`, `exit=1`.

Restore and confirm green again:

```bash
perl -0pi -e "s/healthy='name: selftest\nonn:/healthy='name: selftest\non:/" ci/actionlint/run.sh
ci/actionlint/run.sh; echo "exit=$?"
```

Expected: `exit=0`. Also confirm `grep -c "onn:" ci/actionlint/run.sh` prints `0`.

- [ ] **Step 5: Commit**

```bash
git status --short   # only ci/actionlint/run.sh modified
git add ci/actionlint/run.sh
git commit -m "feat(repo): add rule-tagged actionlint self-tests and a healthy control (SMA-525)"
```

---

### Task 4: The paths extractor and its self-test (check 7)

**Files:**
- Modify: `ci/actionlint/run.sh`

**Interfaces:**
- Consumes: `fail`, `infra`, `WORKFLOW_FILES` from Task 2.
- Produces: the function `extract_paths_keys <file>`, which prints TAB-separated records to stdout, one per line, in file order:
  - `KEY\t<kind>\t<lineno>` — a `paths:`/`paths-ignore:` key was seen, where `<kind>` is exactly `paths` or `paths-ignore`
  - `ITEM\t<kind>\t<pattern>` — one sequence entry belonging to the most recent `KEY`
  Task 5 consumes exactly this format.
- Produces: the `--self-test` flag, which runs the fixture table and exits.

**Why the contract is specified rather than left to judgement:** the naive implementation breaks on a file already committed here. `prebuild.yml`'s `pull_request.paths` block has three interior comment lines mid-sequence and trailing `#` comments on four entries. An extractor that closes the block at the first non-`- ` line extracts **7 of 9** globs — and a "did we get at least one?" control still passes, 7 being ≥ 1. The contract has to carry this, not the control.

**Contract:**
1. Only `paths:`/`paths-ignore:` keys **inside the top-level `on:` mapping** are considered. This makes a `paths:` line inside a `run:` block structurally impossible to mis-extract.
2. A key emits `KEY` **whether or not** a block follows. If the value after the colon is empty, a block sequence opens. If it is non-empty (the inline flow form `paths: [a, b]`), no block opens and zero `ITEM`s follow — which is exactly what makes check 6 fire loudly instead of silently skipping.
3. A block ends at the first **non-item** line whose indentation is **≤ the key's**, **not** at the first line that is not a `- ` item. The "non-item" qualifier is load-bearing: a *flush* block sequence, whose `- ` items sit at the same indentation as their key, is valid YAML, is what Prettier's YAML printer emits, and is accepted by GitHub and actionlint. An earlier draft of this clause said plain "dedent", which read a flush sequence as a closed block and produced a KEY with zero items — a false red inside the repo's only required check.
4. Blank lines and whole-line comments inside a block are skipped **without** closing it.
5. A trailing ` #` comment outside quotes is stripped; a `#` inside a quoted scalar is not.
6. Unquoted, single-quoted and double-quoted scalars are all accepted; surrounding quotes are stripped.

- [ ] **Step 1: Write the failing self-test first**

Append this to `run.sh`, immediately after the `WORKFLOW_FILES` block from Task 2 (it must come before the checks so `--self-test` can exit early):

```bash
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
```

- [ ] **Step 2: Run it to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
ci/actionlint/run.sh --self-test; echo "exit=$?"
```

Expected: FAIL — `extract_paths_keys: command not found` for every fixture (the function does not exist yet).

- [ ] **Step 3: Implement the extractor**

Insert `extract_paths_keys` immediately **above** `extractor_self_test`:

```bash
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
```

- [ ] **Step 4: Run the self-test to verify it passes**

```bash
ci/actionlint/run.sh --self-test; echo "exit=$?"
```

Expected: no output, `exit=0`. If a fixture mismatches, the diff is printed — fix the awk, not the fixture, unless the fixture's expectation contradicts the contract above.

- [ ] **Step 5: Verify against the real files, especially the hard one**

Add this **temporary** line just above the final `exit "$FAILED"` in `run.sh`:

```bash
for f in "${WORKFLOW_FILES[@]}"; do echo "### $f"; extract_paths_keys "$f"; done
```

Run it, check the counts below, then **delete the line again**:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
ci/actionlint/run.sh
```

Expected for `prebuild.yml`: **2** `KEY` records (the `push` and `pull_request` blocks) and **13** `ITEM` records total — 4 from `push` (`rs/**`, `.github/workflows/prebuild.yml`, `.prototools`, `.moon/**`) and 9 from `pull_request`. The 9 is the number that matters: an extractor that closes the block at the first interior comment yields 7. `security-scan.yml`: 1 `KEY`, 5 `ITEM`s. `ci.yml`: no records.

Delete the temporary line before committing.

- [ ] **Step 6: Commit**

```bash
git status --short
git add ci/actionlint/run.sh
git commit -m "feat(repo): extract workflow paths filters with a contract-tested awk parser (SMA-525)"
```

---

### Task 5: Checks 5 and 6 — glob vocabulary, tree matching, per-key control

**Files:**
- Modify: `ci/actionlint/run.sh`

**Interfaces:**
- Consumes: `extract_paths_keys`, `WORKFLOW_FILES`, `fail`, `infra`, `FAILED`.
- Produces: nothing consumed later.

**Why the vocabulary is restricted (spec D4) — this is the part that would have shipped broken:**

- `git ls-files -- ':(glob)P'` gives **wildcard-free patterns directory-prefix semantics**. `':(glob)rs'` matches 320 tracked files; GitHub matches **nothing**, because filter patterns match file paths and no file is named `rs`. Dropping a `/**` is among the likeliest hand-edits. So a wildcard-free pattern must be an **exact tracked file path**, never a prefix.
- `**` differs. GitHub defines it as "zero or more of any character", slash-crossing anywhere; git's wildmatch under `WM_PATHNAME` only crosses `/` when `**` is a whole path component. GitHub documents `'**.js'` as "all .js files in the repository"; under `:(glob)` it yields **0**. Accepting it silently would false-red the only required check.

So: literals must match exactly, `**` must be a whole path component, and `?`, `+`, `[`, `]` are rejected loudly with a message naming the divergence — never given a silently-wrong verdict.

- [ ] **Step 1: Append checks 5 and 6**

```bash
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
# Hence a restricted vocabulary where both matchers provably agree, and a LOUD rejection
# otherwise. Never a silently-wrong verdict in either direction.
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

check_pattern() {
  local file="$1" p="$2" n

  is_skipped "$p" && return

  # Negated entries are exclusions — requiring them to match a file would be wrong. They are
  # still COUNTED by check 6, which counts raw sequence items before any filtering, so an
  # all-negated block cannot hard-fail as "key with no items".
  case "$p" in '!'*) return ;; esac

  # Pathspec-injection guard: a pattern starting with ':' would be read by git as pathspec
  # magic. The '--' separator and quoting are necessary but not sufficient. Anything outside
  # this conservative class is rejected rather than passed to git.
  if ! printf '%s' "$p" | grep -qE '^[A-Za-z0-9._/*-]+$'; then
    fail "$file: pattern '$p' contains characters this gate will not pass to git.
      Supported: letters, digits, '.', '_', '/', '*', '-'. If GitHub accepts it, add it to
      SKIP_PATTERNS in $0 with a justification."
    return
  fi

  # '?' is "zero or one of the PRECEDING character" on GitHub but "any single character" in git;
  # '+' is "one or more of the preceding" on GitHub but a literal in git; '[]' is one alphanumeric
  # on GitHub but ranges/negation in git. All three would give a wrong verdict, so reject.
  case "$p" in
    *'?'*|*'+'*|*'['*|*']'*)
      fail "$file: pattern '$p' uses '?', '+' or '[]', whose meaning differs between GitHub
      filter patterns and git pathspecs, so this gate cannot verify it. Rewrite it, or add it to
      SKIP_PATTERNS in $0 with a justification."
      return ;;
  esac

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
# Check 6 — every extracted KEY must carry at least one sequence item, and at least one of those
# items must be a POSITIVE (non-'!') pattern.
#
# Counts RAW items, before the '!' filtering in the pattern check, so that an all-negated block
# cannot produce the WRONG failure: post-filter it has zero globs, and counting post-filter would
# report it as "no sequence entries this gate could read" — a claim about the extractor, sending
# the author after a YAML problem that is not there. A zero raw count is what converts an
# unsupported YAML form — the inline flow `paths: [a, b]`, and the flow-mapping event
# `push: { paths: … }`, neither of which the extractor parses — from a silent skip into a loud
# failure that names the file. The difference between a limitation and a hole.
#
# An all-negated block DOES hard-fail, under the second, more specific rule: GitHub includes a
# changed file only when it matches at least one POSITIVE pattern, so such a filter can never match
# anything and the trigger it guards is permanently dead. `paths-ignore:` is exempt — an
# all-negated paths-ignore is a no-op, not a dead trigger.
# ---------------------------------------------------------------------------------------------
for wf in "${WORKFLOW_FILES[@]}"; do
  records="$(extract_paths_keys "$wf")" || infra "extractor failed on $wf"
  [ -n "$records" ] || continue

  key_kind=""; key_line=""; key_items=0

  flush_key() {
    if [ -n "$key_kind" ] && [ "$key_items" -eq 0 ]; then
      fail "$wf:$key_line: '$key_kind:' has no sequence entries this gate could read. If it uses
      the inline form (paths: [a, b]), rewrite it as a block sequence — the extractor parses only
      block sequences, and skipping it silently is exactly the failure this gate exists to prevent."
    fi
  }

  while IFS=$'\t' read -r rec kind value; do
    case "$rec" in
      KEY)
        flush_key
        key_kind="$kind"; key_line="$value"; key_items=0 ;;
      ITEM)
        key_items=$((key_items + 1))
        [ "$kind" = "paths" ] && check_pattern "$wf" "$value" ;;
    esac
  done <<< "$records"

  flush_key
done
```

- [ ] **Step 2: Verify the clean tree still passes**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
ci/actionlint/run.sh; echo "exit=$?"
ci/actionlint/run.sh --self-test; echo "selftest exit=$?"
```

Expected: `exit=0` for both.

- [ ] **Step 3: Prove check 5 catches a typo'd glob (red test — AC-3)**

```bash
sed -i '' "s|      - 'rs/\*\*'|      - 'rz/**'|" .github/workflows/prebuild.yml
ci/actionlint/run.sh; echo "exit=$?"
git checkout -- .github/workflows/prebuild.yml
```

Expected: `paths glob 'rz/**' matches NO tracked file`, `exit=1`.

- [ ] **Step 4: Prove check 5 catches a dropped `/**` (red test — the D4 false-green)**

This is the case a naive `:(glob)` implementation passes with "320 files matched".

```bash
sed -i '' "s|      - 'rs/\*\*'|      - 'rs'|" .github/workflows/prebuild.yml
ci/actionlint/run.sh; echo "exit=$?"
git checkout -- .github/workflows/prebuild.yml
```

Expected: `paths entry 'rs' is not an exact tracked file path … Did you mean 'rs/**'?`, `exit=1`.

- [ ] **Step 5: Prove the vocabulary rejection and check 6 both bite (red tests)**

```bash
sed -i '' "s|      - 'rs/\*\*'|      - '**.rs'|" .github/workflows/prebuild.yml
ci/actionlint/run.sh; echo "exit=$?"
git checkout -- .github/workflows/prebuild.yml

perl -0pi -e "s/    paths:\n      - 'rs\/\*\*'.*?\n/    paths: ['rs\/**']\n/s" .github/workflows/prebuild.yml
ci/actionlint/run.sh; echo "exit=$?"
git checkout -- .github/workflows/prebuild.yml
```

Expected: first — `uses '**' inside a path segment`, `exit=1`. Second — `has no sequence entries this gate could read`, `exit=1`.

- [ ] **Step 6: Confirm the tree is clean and commit**

```bash
git status --short   # ONLY ci/actionlint/run.sh modified — no workflow file may remain mutated
git diff --stat
git add ci/actionlint/run.sh
git commit -m "feat(repo): verify workflow paths globs resolve against the tracked tree (SMA-525)"
```

---

### Task 6: Wire the gate into Moon and CI

**Files:**
- Create: `ci/actionlint/README.md`
- Modify: `moon.yml`, `.github/workflows/ci.yml`, `CLAUDE.md`

**Interfaces:**
- Consumes: `ci/actionlint/run.sh`.
- Produces: the `repo:actionlint` Moon target.

- [ ] **Step 1: Add the Moon task**

Append to `moon.yml`'s `tasks:` map, after `promtool:`:

```yaml
  actionlint:
    description: 'actionlint over .github/workflows/**, plus a control that every paths: filter glob still matches the tree (SMA-525).'
    # WHY `inputs: **/*` AND NOT THE NARROW LIST THE OTHER repo:* GATES USE — this gate makes an
    # assertion about the WHOLE FILE TREE, not about the workflow files. A directory rename is the
    # dominant real-world way a paths: glob comes to match nothing (a typo is at least made by
    # someone looking at the file), and keying on .github/workflows/** would let Moon serve a
    # cached pass indefinitely — reproducing the "silent and permanent" failure this gate exists
    # to prevent. Concretely: security-scan.yml filters on 'ci/osv/**', and this change adds a
    # sibling ci/actionlint/, so a future ci/ reshuffle is plausible.
    #
    # The narrow-inputs convention elsewhere in this file exists because those gates are expensive
    # (cargo nextest, next typegen). This one is ~1.0s standalone and warm (measured): six
    # actionlint invocations — one over the real workflows, five stdin fixtures — 26
    # `git ls-files` calls, and three fixture tables (extractor, path filters, config allowlist).
    # Through Moon it is ~11.6s, essentially all of which is Moon's own per-task floor; see the
    # measured table in ci/actionlint/README.md before concluding the inputs are the problem.
    script: 'ci/actionlint/run.sh'
    toolchain: 'system'
    inputs:
      - '**/*'
```

- [ ] **Step 2: Measure the hashing cost against the spec's threshold**

Broad `inputs:` makes Moon hash the whole tree for this task's cache key, and that cost — unlike the runtime — was never measured. The spec pre-commits to a threshold so this is not decided under pressure.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:actionlint --force
time moon run repo:actionlint --force
```

Compare against **Moon's own per-task floor**, not against an absolute wall-time number: run `repo:promtool` (an existing narrow-input task) the same way and use its time as the baseline. Measured on SMA-525: floor ~8.7s, this gate with a narrow input list ~10.4s, with `inputs: ['**/*']` ~11.6s, and with `inputs: ['**/*']` but no `hasher.ignorePatterns` ~98.6s.

**If broad `inputs:` costs materially more than a narrow list on the same floor:** stop and split the task per spec §4.2 — keep `repo:actionlint` with narrow inputs (`.github/workflows/**/*`, `.github/actionlint.*`, `ci/actionlint/**/*`, `.prototools`, `.proto/plugins/actionlint.toml`) running only checks 1–4, and add `repo:workflow-path-filters` with `inputs: ['**/*']` running checks 5–7 via a `--path-filters-only` flag. Both targets then go into `T=(…)` and into the CLAUDE.md command. On the numbers above the difference is ~1s, so it was **not** split; what the decision actually turns on is `hasher.ignorePatterns`, not the input glob. Report the measurement either way.

- [ ] **Step 3: Wire into CI**

In `.github/workflows/ci.yml`, add `:actionlint` to the `T=(…)` array (line ~184), placed after `:machete`:

```bash
T=(:build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free :redis-connect-single-site :promtool :observability-drift :nats-permissions :release-parity :release-parity-py :release-parity-ts)
```

And update the install step's name (line ~73) so the listed tools match reality:

```yaml
      - name: Install pinned CLIs from .prototools (actionlint, buf, lefthook, cargo-deny, cargo-machete, cargo-nextest, promtool)
```

- [ ] **Step 4: Update the documented full-graph command**

In `CLAUDE.md`'s Gotchas section, the "run the full graph like CI does" command enumerates every gate target. Add `:actionlint` in the same position as in `T=(…)`. Without this, the documented pre-push command silently omits the new gate — the exact class of omission this issue is about.

- [ ] **Step 5: Write the README**

Create `ci/actionlint/README.md`, matching the shape of `ci/affected-graph/README.md`:

```markdown
# actionlint gate

Lints `.github/workflows/**` and proves every `paths:` filter glob still matches the tree.

## Why

A `paths:` filter that comes to match nothing does not error. The workflow stops running,
forever, with no red check and no notification — `prebuild.yml` triggers only on
push-to-`main`, `workflow_dispatch` and a narrow `pull_request` filter, so its 7-platform
verification would silently cease. See SMA-525 and
`docs/superpowers/specs/2026-08-16-sma-525-actionlint-gate-design.md`.

actionlint alone is **not** sufficient: it validates syntax and has no view of the file tree,
so a valid-but-never-matching glob (`rz/**`) passes it cleanly. Checks 5–7 close that.

## The checks

| # | Check |
|---|---|
| 1 | `actionlint` over the auto-discovered workflow set |
| 2 | No `.github/actionlint.{yaml,yml}` carrying an `ignore:` key (it would neuter check 1 invisibly) |
| 3 | Four stdin fixtures, one per defect class, each must fail **with its expected rule tag** |
| 4 | A healthy stdin fixture must pass — the control for check 3 |
| 5 | Every `paths:` glob is in the supported vocabulary and matches the tracked tree |
| 6 | Every extracted `paths:` key carries at least one sequence entry |
| 7 | Extractor self-test against a fixture table (`run.sh --self-test`) |

## Supported glob vocabulary

`git ls-files ':(glob)P'` is not a sound model of GitHub filter patterns, so check 5 accepts
only the subset where both provably agree:

- **literals** — must be an *exact* tracked file path. A bare directory name (`rs`) matches
  nothing on GitHub, though git's pathspec would match everything beneath it.
- **`dir/**`**, **`**/name`** — `**` as a whole path component.
- **`*`** within a single segment.

Rejected loudly, never guessed at: `?`, `+`, `[]`, and `**` embedded in a segment (`**.js`).

## Escape hatches

- A **new GitHub runner label** the pinned actionlint does not know: add it to
  `self-hosted-runner.labels` in `.github/actionlint.yaml`. Check 2 permits that file; it bans
  only `ignore:`.
- A **GitHub-valid pattern outside the vocabulary**: add it to `SKIP_PATTERNS` in `run.sh` with
  a comment justifying it and saying what verifies it instead.
- **Anything worse**: drop `:actionlint` from `T=(…)` in `.github/workflows/ci.yml`. One line.

## Running it

```bash
moon run repo:actionlint      # via Moon, as CI does
ci/actionlint/run.sh          # directly, bypassing the Moon cache
ci/actionlint/run.sh --self-test   # extractor fixtures only, for fast iteration
```

Exit codes: `1` = assertion failure, `2` = infrastructure error.
```

- [ ] **Step 6: Verify AC-1 end-to-end through Moon, not just the binary**

The spec's §2 evidence proves the *binary* catches these; that is not the same as the *gate*
catching them. Note that `moon ci :actionlint` exits 0 having run nothing when the task is
unaffected relative to the base, so the mutation must be committed for the check to mean anything.

For each of the three AC-1 classes, run:

**Safety:** capture the pre-mutation SHA and reset to *that literal SHA*, never `HEAD~1`. A
miscount with `--hard` destroys real work, and this branch's commits are the only copy.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
SAFE=$(git rev-parse HEAD); echo "SAFE=$SAFE"
git status --porcelain   # MUST be empty before starting; stop if it is not

# (a) invalid syntax
perl -0pi -e 's/^on:\n/on:\n  workflow_dispatch:\n    paths:\n      - "rs\/**"\n/' .github/workflows/ci.yml
git add -A && git commit -q -m "test(repo): temporary ac-1 syntax mutation (SMA-525)"
moon run repo:actionlint --force; echo "exit=$?"
git reset --hard "$SAFE"

# (b) unknown runner label
sed -i '' 's/^    runs-on: ubuntu-latest$/    runs-on: ubunut-latest/' .github/workflows/ci.yml
git add -A && git commit -q -m "test(repo): temporary ac-1 runner-label mutation (SMA-525)"
moon run repo:actionlint --force; echo "exit=$?"
git reset --hard "$SAFE"

# (c) bad expression
perl -0pi -e 's/      - name: Checkout \(full history/      - run: echo \$\{\{ steps.nope.outputs.x \}\}\n      - name: Checkout (full history/' .github/workflows/ci.yml
git add -A && git commit -q -m "test(repo): temporary ac-1 expression mutation (SMA-525)"
moon run repo:actionlint --force; echo "exit=$?"
git reset --hard "$SAFE"
```

Expected for each: the task fails, `exit=1`, and the output names the rule (`[syntax-check]`,
`[runner-label]`, `[expression]`). Confirm with `git log --oneline -1` and `git status` that the
tree is back to the pre-mutation commit after each `reset --hard`.

Record the three outputs — they go in the PR body.

- [ ] **Step 7: Run the full graph the way CI does**

Per-project Moon tasks do **not** run repo-level gates, and this target is new to the array.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :promtool :observability-drift :nats-permissions \
  :release-parity :release-parity-py :release-parity-ts \
  --base origin/main --include-relations
```

Expected: all green. If Moon reports an unattributed "N failed", read
`.moon/cache/ciReport.json` and filter with
`jq '.actions[] | select(.status == "failed")'`.

Note: `:affected-smoke` will run, because this change edits `.github/workflows/ci.yml`, which is
one of its inputs. That is expected and it should pass — `ci/affected-graph/run.sh` asserts only
that every `moon ci` invocation carries `--include-relations`, not the contents of `T`.

- [ ] **Step 8: Commit**

```bash
git status --short
git add moon.yml .github/workflows/ci.yml CLAUDE.md ci/actionlint/README.md
git commit -m "ci(repo): run the actionlint gate in moon ci (SMA-525)"
```

---

## Self-Review

**Spec coverage:**

| Spec item | Task |
|---|---|
| §4.1 proto plugin (no exe-path, checksum, global arch, resolve) | 1 |
| §4.2 Moon task, broad inputs, hashing threshold + fallback | 6 (steps 1–2) |
| §4.2 `set -uo pipefail`, exit-code contract | 2 (step 1) |
| §4.3 check 1 (bare invocation) | 2 |
| §4.3 check 2 (config integrity) | 2 |
| §4.3 checks 3–4 (rule-tagged self-tests, shared ARGS) | 3 |
| §4.3 check 5 (vocabulary + matching, `paths-ignore` excluded, `!` handling, skip list, injection guard) | 5 |
| §4.3 check 6 (per-key control, raw counts) | 5 |
| §4.3.1 extractor contract + check 7 | 4 |
| §4.4 CI wiring, `:affected-smoke` note, AC-2 | 6 |
| §5 verification incl. AC-1 through Moon and AC-3 both ways | 5 (steps 3–5), 6 (steps 6–7) |
| §6 rollout/rollback escape hatches | 6 (step 5, README) |
| CLAUDE.md full-graph command | 6 (step 4) |
| README | 6 (step 5) |

**Follow-ups to file with the PR** (spec §7 L3, L5, L6): inline-bash linting via a pinned
shellcheck; `branches:` filter existence checking; a control asserting every `repo:*` gate is
present in `ci.yml`'s `T=(…)`.

**Type consistency:** `extract_paths_keys` emits `KEY\t<kind>\t<lineno>` and
`ITEM\t<kind>\t<pattern>` — defined in Task 4, consumed with the same field order and the same
`<kind>` spellings (`paths`, `paths-ignore`) in Task 5's read loop and in Task 4's fixtures.
`ARGS`, `fail`, `infra`, `FAILED` and `WORKFLOW_FILES` are defined in Task 2 and used unchanged
in Tasks 3–5. `SKIP_PATTERNS`, `is_skipped`, `globstars_are_components`, `tracked_exact` and
`check_pattern` are all defined and used within Task 5.

<!-- moon-diagnosis:superseded -->
> **Superseded (SMA-597).** The `ciReport.json` diagnosis advice above does not work as written:
> there is no action-level `exitCode` key, and the file carries no stdout/stderr at all. The
> measured procedure is in CLAUDE.md between the `moon-diagnosis` markers. This document is left
> otherwise unedited as a record of what was believed when it was written.
