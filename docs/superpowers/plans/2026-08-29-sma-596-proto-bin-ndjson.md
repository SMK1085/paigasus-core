<!-- SPDX-License-Identifier: Apache-2.0 -->

# SMA-596 proto NDJSON resolution Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the release-parity harness resolve its tool binaries deterministically and fail loudly at the point of resolution, so the `repo:release-parity` gate returns a real verdict in an agent session instead of aborting rc=2.

**Architecture:** Two bash ecosystem modules each resolve one tool binary into a variable at file top level. Each gains (a) a deterministic primary resolution with no fallback, and (b) an `[ -x ]` assertion that exits 2 with a message carrying the harness's own `infrastructure error (rc=2)` classifier. No shared library is introduced — the spec (D4, and Sven's scope choice) rejected a `ci/lib/` layer, so each module carries its own small fatal helper with a module-specific name.

**Tech Stack:** bash (`set -euo pipefail`), proto 0.61.1, Moon 2.5.3, release-plz 0.3.158, python-semantic-release via uv.

**Spec:** `docs/superpowers/specs/2026-08-29-sma-596-proto-bin-ndjson-design.md` (revision 3)

## Global Constraints

- Every source file opens with an SPDX header: `# SPDX-License-Identifier: Apache-2.0`. Both target files already have one — do not add a second.
- Conventional commits with a workspace scope. These files are repo tooling, so the scope is `repo` for code and `docs` for documentation: `fix(repo): …`, `docs(docs): …`. Subject starts lowercase, max 100 chars. Never put a `#NNN` reference or a bare `token: value` line in the commit **body** — it fails `footer-leading-blank` in CI even when the local hook passes.
- Bash tool PATH lacks the proto-managed CLIs. **Every** command in this plan must be preceded in the same shell invocation by:
  `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`
- All work happens in the worktree `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-596` on branch `feature/sma-596-proto-bin-ndjson`. Do not touch the main checkout.
- Error messages must **not** cite line numbers. They move as comment blocks grow.
- The literal classifier string is `infrastructure error (rc=2)`. It must appear verbatim, because `run.sh:67` and `run.sh:81` use that exact vocabulary and CLAUDE.md tells readers to grep for it. This is the one string in this plan that must not be paraphrased.
- Do **not** unset `AI_AGENT`, `CLAUDECODE` or `CLAUDE_CODE_ENTRYPOINT` at any point. The whole issue is that the gates must work with them set. Steps that need the non-agent path use `env -u`, which scopes the removal to one command.
- Do **not** restore a mutated file by moving a `.bak` copy back. A backwards mtime makes cargo serve a stale artifact and the re-run then fails for an unrelated reason. Restore with `git checkout -- <file>`.

---

## File Structure

| File | Change | Responsibility |
| ---- | ------ | -------------- |
| `ci/release-parity/ecosystems/release-plz.sh` | Modify lines 10–20 | Resolve `RELEASE_PLZ_BIN` via `proto --reporter text`, assert it and `CARGO_BIN` |
| `ci/release-parity/ecosystems/python-semantic-release.sh` | Modify lines 20–29 | Resolve `PSR_BIN` via `uv run` only, assert it |
| `ci/release-parity/README.md` | Modify | State the harness-wide resolution policy once |
| `CLAUDE.md` | Modify lines 52–59 | Rewrite the NDJSON gotcha: corrected scope, the fix, the residual |

There is no unit-test layer under `ci/release-parity/`. The harness verifies by running, so each task's "test" is a mutation that must produce a specific failure, followed by a restore and a real run.

---

### Task 1: `release-plz.sh` — deterministic resolution and assertions

**Files:**
- Modify: `ci/release-parity/ecosystems/release-plz.sh:10-20`
- Test: none (bash module; verified by running the harness)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: `_rp_fatal <line>...` — writes the classifier line then each argument indented, to stderr, and exits 2. `RELEASE_PLZ_BIN` and `CARGO_BIN` keep their existing names and meanings; every later use in the file is unchanged.

- [ ] **Step 1: Record the current (broken) failure, so the fix is provably the cause**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-596
bash ci/release-parity/run.sh --negative-control; echo "rc=$?"
```

Expected: `rc=2`, with output containing `line 103:` and `{"type":"message"` and
`negative-control INCONCLUSIVE: infrastructure error (rc=2)`.
Save that output — it is AC2's "before" evidence for the record.

- [ ] **Step 2: Replace lines 10–20 with the new block**

Replace exactly this existing text:

```bash
# release-plz and the `cargo metadata` it spawns run inside a temp fixture OUTSIDE
# this repo. The proto `release-plz` shim resolves its version by walking up from
# CWD to find .prototools — from /tmp that fails (CI: proto::tool::unknown_id). So
# resolve the absolute tool binaries once, from the repo, and invoke those directly.
_RP_SELF="${BASH_SOURCE[0]:-$0}"
_RP_REPO_ROOT="$(cd "$(dirname "$_RP_SELF")/../../.." && pwd)"
RELEASE_PLZ_BIN="$( (cd "$_RP_REPO_ROOT" && proto bin release-plz) 2>/dev/null || command -v release-plz || echo release-plz )"
# release-plz shells out to `cargo metadata`; pass an explicit, CWD-independent
# cargo (rustup proxy / real binary, not a CWD-sensitive shim).
CARGO_BIN="$( command -v cargo 2>/dev/null || true )"
[ -n "$CARGO_BIN" ] || CARGO_BIN="$HOME/.cargo/bin/cargo"
```

with:

```bash
# Abort with the harness's OWN vocabulary. run.sh prints "infrastructure error
# (rc=2)" at :67 and :81, but this module is sourced at run.sh:21 — so an exit from
# here fires DURING the source and run.sh never reaches either line. Without this
# string the abort would be unclassifiable, and CLAUDE.md tells readers to grep for
# it. Deliberately duplicated in python-semantic-release.sh rather than shared: one
# module is sourced per run, and a ci/lib/ layer was considered and rejected
# (SMA-596 D4).
_rp_fatal() { # line...
  echo "FATAL: release-parity ABORTED: infrastructure error (rc=2)" >&2
  printf '       %s\n' "$@" >&2
  exit 2
}

# release-plz and the `cargo metadata` it spawns run inside a temp fixture OUTSIDE
# this repo. The proto `release-plz` shim resolves its version by walking up from
# CWD to find .prototools — from /tmp that fails (CI: proto::tool::unknown_id;
# recorded on that CI run, NOT reproduced locally — SMA-596 L2). So resolve the
# absolute tool binaries once, from the repo, and invoke those directly. Do not
# delete this dance as redundant; it is what stops the SMA-398 bug returning.
#
# `--reporter text` is REQUIRED, not cosmetic. proto prints NDJSON on stdout when it
# detects an agent environment (AI_AGENT / CLAUDECODE / CLAUDE_CODE_ENTRYPOINT), and
# `proto bin` still exits 0 while doing it — so an unflagged capture silently yields
# a JSON blob instead of a path, and every `||` fallback is skipped because nothing
# failed. That is SMA-596.
#
# There is deliberately NO fallback. This harness exists to compare one pinned
# release-plz's classification behaviour; a silently substituted binary produces a
# verdict about the wrong tool. Failing loudly is correct here.
#
# `exit` from a module that run.sh sources at top level is intentional — do NOT
# "fix" it to `return`. run.sh parses its arguments (:12) and handles --help (:15)
# before the source (:21), so this cannot pre-empt either.
_RP_SELF="${BASH_SOURCE[0]:-$0}"
_RP_REPO_ROOT="$(cd "$(dirname "$_RP_SELF")/../../.." && pwd)"
RELEASE_PLZ_BIN="$(cd "$_RP_REPO_ROOT" && proto --reporter text bin release-plz)" || _rp_fatal \
  "release-plz.sh: 'proto --reporter text bin release-plz' failed." \
  "Run 'proto install' from the repo root." \
  "An older proto without --reporter also lands here (SMA-596 D1)."
[ -x "$RELEASE_PLZ_BIN" ] || _rp_fatal \
  "release-plz.sh: release-plz did not resolve to an executable file." \
  "Got: ${RELEASE_PLZ_BIN:-<empty>}" \
  "If that looks like JSON, proto's agent-mode NDJSON leaked past --reporter text (SMA-596)."

# release-plz shells out to `cargo metadata`; pass an explicit, CWD-independent
# cargo (rustup proxy / real binary, not a CWD-sensitive shim). This fallback is
# KEPT, unlike release-plz's: it is a real reachable default, and cargo is not the
# tool under test. The assertion below is what stops a bad value surfacing later as
# a confusing cargo error instead of a resolution error (SMA-596 D2.1).
CARGO_BIN="$( command -v cargo 2>/dev/null || true )"
[ -n "$CARGO_BIN" ] || CARGO_BIN="$HOME/.cargo/bin/cargo"
[ -x "$CARGO_BIN" ] || _rp_fatal \
  "release-plz.sh: cargo did not resolve to an executable file." \
  "Got: ${CARGO_BIN:-<empty>}" \
  "Install Rust, or put cargo on PATH."
```

- [ ] **Step 3: Run the gate and verify it now reaches a real verdict**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-596
bash ci/release-parity/run.sh --negative-control; echo "rc=$?"
```

Expected: `rc=0`, output ends `negative-control OK: harness reported red as expected`.
No `{"type":"message"` anywhere in the output.

- [ ] **Step 4: Run the real suite, not only the control**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-596
bash ci/release-parity/run.sh; echo "rc=$?"
```

Expected: `rc=0`, output ends `== all parity cases passed ==`.
A control proves the harness can report red; only this proves the parity assertion still holds.

- [ ] **Step 5: Prove the assertion bites — mutate the resolution**

Temporarily change the `RELEASE_PLZ_BIN=` line to:

```bash
RELEASE_PLZ_BIN='{"type":"message","message":"/nope"}'
```

then run:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-596
bash ci/release-parity/run.sh --negative-control; echo "rc=$?"
```

Expected: `rc=2`. Output contains `FATAL: release-parity ABORTED: infrastructure error (rc=2)`,
`release-plz did not resolve to an executable file`, and the JSON blob after `Got:`.
It must **not** contain `line 103` or `No such file or directory`.

- [ ] **Step 6: Restore, and prove the flag is what fixes it**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-596
git checkout -- ci/release-parity/ecosystems/release-plz.sh
```

That reverts Step 2 as well, so re-apply Step 2, then delete **only** `--reporter text`
from the `proto` call and run:

```bash
bash ci/release-parity/run.sh --negative-control; echo "rc=$?"
```

Expected: `rc=2` with the same `did not resolve to an executable file` message and a JSON
blob after `Got:`. This is what distinguishes "the assertion fires" from "the flag works" —
without it, Step 5 alone cannot tell them apart. Then restore `--reporter text`.

- [ ] **Step 7: Prove the `CARGO_BIN` assertion bites**

Temporarily append after the `CARGO_BIN` fallback line:

```bash
CARGO_BIN=/nonexistent/cargo
```

Run the control, expect `rc=2` with `cargo did not resolve to an executable file` and
`Got: /nonexistent/cargo`. Then remove that line.

- [ ] **Step 8: Confirm the file is back to the intended state**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-596
git diff ci/release-parity/ecosystems/release-plz.sh
bash ci/release-parity/run.sh --negative-control; echo "rc=$?"
```

Expected: the diff shows only the Step 2 change — no leftover mutation lines. `rc=0`.

- [ ] **Step 9: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-596
git add ci/release-parity/ecosystems/release-plz.sh
git commit -m "fix(repo): resolve release-plz with proto's text reporter and assert it

proto prints NDJSON on stdout in an agent environment and proto bin
still exits 0, so the capture yielded a JSON blob and both fallbacks
were skipped. The gate aborted rc=2 rather than reporting a verdict.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 2: `python-semantic-release.sh` — drop the live fallback and assert

**Files:**
- Modify: `ci/release-parity/ecosystems/python-semantic-release.sh:20-29`
- Test: none (bash module; verified by running the harness)

**Interfaces:**
- Consumes: the message shape and classifier string established in Task 1. The two modules' assertions must read alike; a reader comparing them must not wonder whether a difference is meaningful.
- Produces: `_psr_fatal <line>...` — same contract as `_rp_fatal`, different name so nobody reads them as one shared helper. `PSR_BIN` keeps its name and meaning.

- [ ] **Step 1: Record which arm carries the gate today**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-596
( cd py && uv run --frozen python -c 'import shutil,sys; sys.stdout.write(shutil.which("semantic-release") or "")' ); echo " rc=$?"
command -v semantic-release || echo "(not on PATH)"
```

Expected: arm 1 prints a path ending `py/.venv/bin/semantic-release` with rc=0, and
`semantic-release` is **not** on PATH. That is what makes removing the fallback safe —
it is not what is carrying the gate. If arm 1 fails here, **stop and report**: the
premise of D3.1 does not hold on this machine and the plan needs revisiting.

- [ ] **Step 2: Replace lines 20–29 with the new block**

Replace exactly this existing text:

```bash
# PSR is a Python package in py/'s uv dev-deps. The fixture lives in /tmp, outside
# the repo, where `uv run` can't resolve the project — so resolve the absolute
# semantic-release binary once, from py/ (mirrors release-plz.sh's RELEASE_PLZ_BIN).
# `uv run --frozen` also bootstraps py/.venv from uv.lock (there is no separate
# `uv sync` CI step).
_PSR_SELF="${BASH_SOURCE[0]:-$0}"
_PSR_REPO_ROOT="$(cd "$(dirname "$_PSR_SELF")/../../.." && pwd)"
PSR_BIN="$( (cd "$_PSR_REPO_ROOT/py" && uv run --frozen python -c 'import shutil,sys; sys.stdout.write(shutil.which("semantic-release") or "")') 2>/dev/null || true )"
[ -n "$PSR_BIN" ] || PSR_BIN="$( command -v semantic-release 2>/dev/null || echo semantic-release )"
```

with:

```bash
# Same contract as release-plz.sh's _rp_fatal, deliberately duplicated rather than
# shared: run.sh sources exactly one module per run, and a ci/lib/ layer was
# considered and rejected (SMA-596 D4). The distinct name keeps that honest.
_psr_fatal() { # line...
  echo "FATAL: release-parity ABORTED: infrastructure error (rc=2)" >&2
  printf '       %s\n' "$@" >&2
  exit 2
}

# PSR is a Python package in py/'s uv dev-deps. The fixture lives in /tmp, outside
# the repo, where `uv run` can't resolve the project — so resolve the absolute
# semantic-release binary once, from py/ (mirrors release-plz.sh's RELEASE_PLZ_BIN).
# `uv run --frozen` also bootstraps py/.venv from uv.lock (there is no separate
# `uv sync` CI step).
#
# The `|| true` is KEPT so a failure lands as an empty value on the assertion below
# rather than killing the script under `set -e` with no explanation.
#
# The old `|| command -v semantic-release || echo semantic-release` fallback is GONE
# (SMA-596 D3.1). Unlike release-plz's dead fallbacks it was genuinely reachable, so
# on a machine with a global semantic-release installed it could silently make an
# unpinned build the tool under test. That matters more here than anywhere else in
# this harness: this module is the REFERENCE implementation for the 0.x expectation
# the other ecosystems are compared against, so substituting it corrupts the whole
# comparison rather than one side of it.
_PSR_SELF="${BASH_SOURCE[0]:-$0}"
_PSR_REPO_ROOT="$(cd "$(dirname "$_PSR_SELF")/../../.." && pwd)"
PSR_BIN="$( (cd "$_PSR_REPO_ROOT/py" && uv run --frozen python -c 'import shutil,sys; sys.stdout.write(shutil.which("semantic-release") or "")') 2>/dev/null || true )"
[ -x "$PSR_BIN" ] || _psr_fatal \
  "python-semantic-release.sh: semantic-release did not resolve to an executable file." \
  "Got: ${PSR_BIN:-<empty>}" \
  "Run 'uv sync' in py/, and check py/uv.lock still carries python-semantic-release." \
  "There is deliberately no PATH fallback (SMA-596 D3.1)."
```

- [ ] **Step 3: Run the py gate's control and real suite**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-596
bash ci/release-parity/run.sh --ecosystem python-semantic-release --negative-control; echo "rc=$?"
bash ci/release-parity/run.sh --ecosystem python-semantic-release; echo "rc=$?"
```

Expected: `rc=0` for both. The control ends `negative-control OK: harness reported red as
expected`; the real run ends `== all parity cases passed ==`. A green result here proves
arm 1 carried the gate, because Step 1 established the fallback resolves to nothing.

- [ ] **Step 4: Prove the assertion bites**

Temporarily append after the `PSR_BIN=` line:

```bash
PSR_BIN=/nonexistent/semantic-release
```

Run:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-596
bash ci/release-parity/run.sh --ecosystem python-semantic-release --negative-control; echo "rc=$?"
```

Expected: `rc=2`, output contains `FATAL: release-parity ABORTED: infrastructure error (rc=2)`,
`semantic-release did not resolve to an executable file` and `Got: /nonexistent/semantic-release`.
Then delete that line and re-run Step 3 to confirm `rc=0`.

- [ ] **Step 5: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-596
git add ci/release-parity/ecosystems/python-semantic-release.sh
git commit -m "fix(repo): drop the semantic-release PATH fallback and assert the resolution

The fallback was reachable, unlike release-plz's, so a globally
installed unpinned build could silently become the tool under test in
the module that defines the reference expectation.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 3: Documentation — CLAUDE.md and the harness README

**Files:**
- Modify: `CLAUDE.md:52-59`
- Modify: `ci/release-parity/README.md`

**Interfaces:**
- Consumes: the corrected scope from the spec §2 and the resolution policy from Tasks 1–2.
- Produces: nothing code-facing.

- [ ] **Step 1: Replace the CLAUDE.md NDJSON bullet**

Replace exactly this existing bullet (`CLAUDE.md:52-59`):

```markdown
- `proto` prints **NDJSON on stdout** when it detects an agent environment (`AI_AGENT`,
  `CLAUDECODE`, `CLAUDE_CODE_ENTRYPOINT`), including a `Detected an AI agent environment…`
  preamble line. That breaks every `$(proto bin <tool>)` capture: the variable becomes a JSON
  blob, not a path. `ci/release-parity/ecosystems/release-plz.sh` does exactly this, so all three
  `repo:release-parity*` gates abort `INCONCLUSIVE (rc=2)` — **not red** — in any agent-driven
  local run. CI has no agent detection, so it never shows there. This is NOT new in proto 0.61.1;
  0.58.1 behaves identically (measured both, SMA-595). To verify those gates locally, `unset
  AI_AGENT CLAUDECODE CLAUDE_CODE_ENTRYPOINT` first — with that, all three pass.
```

with:

```markdown
- `proto` prints **NDJSON on stdout** when it detects an agent environment (`AI_AGENT`,
  `CLAUDECODE`, `CLAUDE_CODE_ENTRYPOINT`), including a `Detected an AI agent environment…`
  preamble line. That breaks any captured `$(proto <subcommand> …)`: the variable becomes a JSON
  blob, not a path, and the `||` fallbacks never fire because proto **exits 0** — it succeeded, it
  just answered in a different language. It is a property of proto's reporter, not of `proto bin`.
  **Pass `--reporter text`** (or `PROTO_REPORTER=text`) on any proto call whose stdout you capture;
  `ci/release-parity/ecosystems/release-plz.sh` is the worked example, and it also asserts
  `[ -x ]` on the result so a future regression fails at the assignment rather than 87 lines later
  (SMA-596). This is NOT new in proto 0.61.1; 0.58.1 behaves identically (measured both, SMA-595).
  A proto-**shimmed** tool (`uv`, `node`, `release-plz`) is a different case: the shim execs the
  tool, so captured stdout is the tool's — that is measured for the two cases below, not proven
  generally.
  **Scope, corrected.** Only `repo:release-parity` was ever affected — NOT all three.
  `ci/release-parity/run.sh` sources exactly ONE ecosystem module per invocation, and only
  `release-plz.sh` invoked the proto CLI; `-py` resolves through `uv run` and `-ts` through `node`,
  and both were measured green in the same agent session that showed `release-parity` aborting.
  The earlier "all three" claim here was a regression against SMA-530's own spec, which had it
  right. The `unset AI_AGENT CLAUDECODE CLAUDE_CODE_ENTRYPOINT` workaround is **no longer needed
  for these gates**; it may still matter for other proto oddities (see the entry above).
  **Residual:** nothing gates this. A new captured proto call written the broken way reds nothing
  (SMA-596 D4) — this bullet is the only control.
```

- [ ] **Step 2: Verify no "all three" claim survives**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-596
grep -n "all three" CLAUDE.md
```

Expected: the only surviving hits are the `release-parity*` **task-wiring** sentence
("All three `repo:release-parity*` tasks run … `--negative-control` before their real run")
and any unrelated use. There must be **no** remaining claim that all three gates abort.
Read each hit and confirm.

- [ ] **Step 3: Confirm the ci-targets markers are untouched**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-596
grep -c "ci-targets:begin" CLAUDE.md; grep -c "ci-targets:end" CLAUDE.md
```

Expected: `1` and `1`. A second copy of either marker anywhere in the file — even inside
backticks in prose — reds `repo:affected-smoke` (SMA-541). This step exists because Task 3
edits CLAUDE.md and that is the failure it could cause.

- [ ] **Step 4: Add the resolution policy to the harness README**

In `ci/release-parity/README.md`, add a short subsection (place it near the ecosystem-module
description, **not** in the Limitations section, which is reserved for the negative-control
pins):

```markdown
### Tool resolution policy

Both `release-plz.sh` and `python-semantic-release.sh` resolve their tool binary once, at
module top level, with **no fallback**, and assert `[ -x ]` on the result before use. A
failure exits 2 with the harness's `infrastructure error (rc=2)` classifier — the module is
sourced by `run.sh`, so an exit here fires during the source and `run.sh` never reaches its
own abort lines. The classifier in the module's message is what keeps such a failure
greppable.

The reason there is no fallback: this harness compares one specific pinned tool's
classification behaviour. A silently substituted binary — a different version, or a shim
that resolves differently in CI than locally — produces a verdict about the wrong tool
(SMA-596 D3, D3.1).

`semantic-release.sh` is not part of this policy. It invokes `node` against a runner script
rather than resolving a tool binary into a variable, so there is no obvious site for the
assertion, and it has not been reviewed for an equivalent hazard (SMA-596 L6).
```

- [ ] **Step 5: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-596
git add CLAUDE.md ci/release-parity/README.md
git commit -m "docs(docs): correct the proto NDJSON gotcha and record the resolution policy

One gate was affected, not three. The earlier claim was a regression
against SMA-530's spec, which had the scope right.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 4: Full verification sweep

**Files:** none modified. This task produces evidence, not code.

**Interfaces:**
- Consumes: Tasks 1–3 complete and committed.
- Produces: the recorded before/after evidence AC2 requires, and a green full graph.

- [ ] **Step 1: All three gates at gate level, in this agent session**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-596
env | grep -E '^(AI_AGENT|CLAUDECODE|CLAUDE_CODE_ENTRYPOINT)=' ; echo "--- markers above must be non-empty ---"
env | grep -E '^PROTO_REPORTER=' || echo "PROTO_REPORTER unset (required)"
moon run repo:release-parity --force; echo "rc=$?"
moon run repo:release-parity-py --force; echo "rc=$?"
moon run repo:release-parity-ts --force; echo "rc=$?"
```

Expected: all three `rc=0`. `--force` is required — a restored file re-hits a cached PASS.
The three detection variables must be present, and `PROTO_REPORTER` must be unset, or the
run passes for the wrong reason.

- [ ] **Step 2: Exercise the non-agent code path**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-596
env -u AI_AGENT -u CLAUDECODE -u CLAUDE_CODE_ENTRYPOINT proto bin release-plz; echo "rc=$?"
env -u AI_AGENT -u CLAUDECODE -u CLAUDE_CODE_ENTRYPOINT bash ci/release-parity/run.sh; echo "rc=$?"
env -u AI_AGENT -u CLAUDECODE -u CLAUDE_CODE_ENTRYPOINT bash ci/release-parity/run.sh --ecosystem python-semantic-release; echo "rc=$?"
```

Expected: the `proto bin` call prints a bare path with rc=0, and both suites end
`== all parity cases passed ==` with rc=0. CI runs this shape and has never run the new one.
This does not prove CI behaviour (spec L3/L4) — it removes the case where the change is
broken for every non-agent caller.

- [ ] **Step 3: Full graph**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-596
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site :http-extractor-envelope :input-liveness :promtool :observability-drift :nats-permissions :release-parity :release-parity-py :release-parity-ts :publish-metadata :version-lockstep :workflow-credentials --base origin/main --include-relations
echo "rc=$?"
```

Expected: `rc=0`. `ci/release-parity/**/*` is an input to all three parity gates **and** to
`repo:affected-smoke`, so this change selects the gate holding the `RELEASE_PARITY_SH_CALL_SITES`
pins. Those pins target `run.sh` and `moon.yml`, neither of which this change touches, so no
re-baseline is expected. If `affected-smoke` reds, read the task log — not `ciReport.json`,
which carries no stdout and a null exitCode (SMA-597).

- [ ] **Step 4: If `repo:affected-smoke` aborts in under 3 seconds**

Capture the full task output **before** re-running — a re-run passes and destroys the evidence.
Grep it for `proto-shim`. If that line is present, this is the known intermittent EACCES abort
(CLAUDE.md), unrelated to this change; re-run `moon run repo:affected-smoke --force`. If absent,
diagnose on its own terms and report.

- [ ] **Step 5: Report the AC evidence**

Produce a table mapping each acceptance criterion to the step that discharged it and the
observed output:

| AC | Discharged by | Evidence |
| -- | ------------- | -------- |
| 1 | Task 4 Step 1 | three gates rc=0, variables set |
| 2 | Task 1 Step 1 (before) + Task 4 Step 1 (after) | rc=2 → rc=0 |
| 3 | Task 1 Steps 5–7, Task 2 Step 4 | assertion messages, no `line 103` |
| 4 | spec §3 | one call site, `CARGO_BIN` unaffected but asserted |
| 5 | Task 3 Steps 1–2 | rewritten bullet, no "all three" claim |

---

## Self-Review

**Spec coverage.** D1 → Task 1 Step 2. D2 → Task 1 Step 2 (both assertions) and Task 2
Step 2. D2.1 → Task 1 Step 2 and Step 7. D2.2 → no code; the `[ -x ]` choice is what Task 1
implements, and the reasoning stays in the spec. D3 → Task 1 Step 2 (fallback deleted).
D3.1 → Task 2. D4 → nothing to implement; the residual is documented in Task 3 Step 1.
D5 → Task 3 Steps 1–3. §5's README change → Task 3 Step 4. §6's ten steps map onto Task 1
Steps 1/3/4/5/6/7, Task 2 Steps 1/3/4, and Task 4 Steps 1/2/3. §9's pin statement → Task 4
Step 3. No spec section is unimplemented.

**Placeholder scan.** Every code step carries the literal replacement text. No "add error
handling", no "similar to Task N" — Task 2's helper is written out in full even though it
mirrors Task 1's, because the implementer may read the tasks out of order.

**Type consistency.** `_rp_fatal` and `_psr_fatal` have identical signatures (`line...`) and
identical output shape, and each is used only in its own file. `RELEASE_PLZ_BIN`, `CARGO_BIN`
and `PSR_BIN` keep their existing names, so no downstream use in either file changes. The
classifier string `infrastructure error (rc=2)` is byte-identical in both helpers and in the
README text.

**One risk the plan cannot remove.** Task 4 Step 2 exercises the non-agent code shape on this
machine, not on a CI runner. The changed line has never executed on CI (spec L4), and which
proto binary a runner executes is unmeasured (spec L3). The first CI run of this branch is the
first execution of that path anywhere.
