# SMA-693 wasm-bindgen Lockstep Updater Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A weekly scheduled workflow proposes the wasm-bindgen family bump as ONE pull request, with the regenerated wasm glue, so that no person runs the runbook for the normal case.

**Architecture:** One workflow, `.github/workflows/wasm-lockstep.yml`, with two jobs (spec approach A). Job `build` has no environment and no secret; it runs every third-party step inside `docker run` over a copy of the tree without `.git`, and uploads one artifact. Job `propose` has `environment: release-pr`; it executes no artifact content, checks the downloaded bytes with the stdlib checker `ci/wasm-lockstep/lockstep_check.py`, mints the App token, and pushes to `deps/wasm-bindgen-lockstep`. A new gate, `repo:wasm-lockstep`, runs that checker and a PyYAML pin check of the workflow (`ci/wasm-lockstep/pin_check.py`) on every PR that touches them.

**Tech Stack:** GitHub Actions; Docker (`docker.io/library/rust:1.95.0-bookworm`, pinned by digest); Python 3.12 stdlib (`tomllib`) and PyYAML 6 in a dedicated uv project; bash 3.2-compatible gate wrapper; Moon 2.5.3 task registration.

**Spec:** `docs/superpowers/specs/2026-09-27-sma-693-wasm-bindgen-lockstep-updater-design.md` (approved by Sven on 2026-10-02). Read it in full before Task 1.

## Global Constraints

Every task's requirements implicitly include this section. Values are copied from the spec.

- Approach A (Q1, decided): one workflow `.github/workflows/wasm-lockstep.yml`, jobs `build` + `propose`, triggers `schedule` and `workflow_dispatch`, top-level `permissions: contents: read`.
- `concurrency: { group: wasm-lockstep, cancel-in-progress: false }` on the workflow.
- `build`: `runs-on: ubuntu-latest`, `timeout-minutes: 60`, `permissions: contents: read`, no `environment`, no job-level `uses:` or `secrets:`, no `secrets` context. Outputs: only `changed`.
- `propose`: `needs: build`, `if: needs.build.result == 'success'`, `environment: release-pr` (Q2, decided), `permissions: contents: read`, `timeout-minutes: 15`.
- `propose` reads `needs.build.outputs.changed` ONLY in `if:` expressions.
- Schedule (Q3 default): Tuesday 06:17 UTC, cron `'17 6 * * 2'`.
- Bot branch (Q5 default): `deps/wasm-bindgen-lockstep`; push refspec exactly `HEAD:refs/heads/deps/wasm-bindgen-lockstep` with `--force-with-lease`.
- Cache-scope control (Q7, decided): the docker container; no `sudo` removal.
- Base check (Q8 default): a moved `main` is a GREEN notice, no push.
- Commit (Q9 default): with git, as `release.yml` does; identity `paigasusbot[bot]` / `285361405+paigasusbot[bot]@users.noreply.github.com`; message `build(deps): move wasm-bindgen to <version> and regenerate the wasm glue`, no body.
- Refusal (Q10, decided): a refusal stays red; Sven watches the runs; no tracking issue.
- `workflows` permission (Q11 default): measure at M3, then decide.
- Action pins: `actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1` (v7.0.1), `actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a` (v7.0.1), `actions/download-artifact@3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c` (v8.0.1), `actions/create-github-app-token@bcd2ba49218906704ab6c1aa796996da409d3eb1` (v3.2.0) with `client-id`, `private-key`, `permission-contents: write`, `permission-pull-requests: write`.
- Artifact: name `wasm-lockstep`, ONE upload path `$RUNNER_TEMP/stage/`, `if-no-files-found: error`, `retention-days: 7`.
- The family (F1): `wasm-bindgen`, `wasm-bindgen-macro`, `wasm-bindgen-macro-support`, `wasm-bindgen-shared`, `js-sys`, `web-sys`, `wasm-bindgen-futures`. The four `cargo update -p` roots: `wasm-bindgen`, `js-sys`, `web-sys`, `wasm-bindgen-futures`.
- Source: `registry+https://github.com/rust-lang/crates.io-index`, with a checksum. Version: strict semver `^[0-9]+\.[0-9]+\.[0-9]+$` (this plan also refuses a leading zero).
- Artifact files: `rs/Cargo.lock` plus a subset of `rs/crates/bindings/paigasus-wasm/{paigasus_wasm.js, paigasus_wasm_bg.js, paigasus_wasm.d.ts, paigasus_wasm_bg.wasm.d.ts, paigasus_wasm_bg.wasm}`; each file 8 MiB or less.
- Checker exit codes: 0 pass, **3 refusal**, 2 infrastructure, 4 no change. `run.sh` maps 3 -> 1 and every other non-zero code -> 2.
- `lockstep_check.py` is stdlib only. `pin_check.py` uses PyYAML from `ci/wasm-lockstep/`'s own uv project, called with `uv run --locked --project ci/wasm-lockstep --python '>=3.12'`.
- Gate `repo:wasm-lockstep`, script exactly `set -euo pipefail` / `bash ci/wasm-lockstep/run.sh --self-test` / `bash ci/wasm-lockstep/run.sh --negative-control` / `bash ci/wasm-lockstep/run.sh`. Inputs: `ci/wasm-lockstep/**/*`, `.github/workflows/wasm-lockstep.yml`, `rs/Cargo.lock`, `.prototools`. All seven registry obligations of `ci/CLAUDE.md`.
- `run.sh` exports `PROTO_REPORTER=text` at the top.
- Every new source file opens with an SPDX header (`# SPDX-License-Identifier: Apache-2.0`).
- Commits: conventional, with a workspace scope. No line in a commit BODY of the form `token: value`, and no `#` followed by digits. Every commit ends with the trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`, after a blank line. Never `--amend`, never `git reset`, never `--no-verify`, never a bare `git stash`.
- Do not install host software (no `brew install`). For GNU tools, use `docker run ubuntu:24.04`.

## Review Focus

The five failure modes below are the most likely to hurt a person who uses this workflow, and the spec names no test for them. Each one has its test in the owning task.

1. **A version string that passes a naive check.** Python's `$` also matches before a trailing newline, and `\d` also matches non-ASCII digits. A lock version `"0.2.129\n"`, `"0.2.１２９"` or `"0.2.0129"` must be refused, because the version reaches a commit title and a PR body. Test: three `R-SEMVER` rows in Task 1 Step 1.
2. **A symlinked PARENT directory.** A check of the six FILES with `test -L` does not see `rs/crates -> /tmp`; `cp` then reads a host path. Test: the row "a symlinked parent directory" (`R-SYMLINK`) in Task 1 Step 1, and the per-component `test -L` loop of the stage step in Task 3 Step 3.
3. **A step that still runs after a refusal.** `if: always()` on the push step, or `continue-on-error: "true"` (a STRING, which PyYAML does not coerce) on the verify step, lets the token or the push run after `lockstep_check.py` refused. Test: rows P14 (boolean and string) and P15 in Task 2 Step 1.
4. **A secret or an output read through an indirect form.** A `secrets` read hoisted to the workflow-level `env:`, injected by a YAML alias, or written as `Secrets.X` / `needs['build']` / `NEEDS.build` slips a check that walks only job keys or matches one case. Test: rows "a workflow-level env reads a secret", "a YAML alias injects a secrets read into build", "Secrets.X in another case in build", "needs['build'] in an env in propose" and "NEEDS in another case in a with: in propose" in Task 2 Step 1.
5. **A negation that `set -e` ignores.** `! test -L "$f"` under `set -e` never stops the step, because bash ignores the status of a `!` command. A symlink then passes the stage step. Test: the P5 row "a bare ! test -L on the build host" in Task 2 Step 1; the stage step in Task 3 Step 3 uses `if test -L ...; then exit 1; fi`.

## Spec facts that changed on HEAD (2026-10-02, base `4face4af`)

The spec cites line numbers from 2026-09-27. This plan uses the current ones:

| Spec citation | Now |
|---|---|
| `rs/CLAUDE.md` runbook bullet, lines 171-224; "ONE host" line 194; Monday run line 223 | lines 176-229; "ONE host" line 199; Monday run line 228 |
| `ts/CLAUDE.md:132` ("ONE host") | lines 134-136 |
| `rs/Cargo.lock:6122-6123` (`wasm-bindgen` 0.2.128) | lines 6120-6121, still 0.2.128 |
| `release.yml:262-272` (the mint) | lines 258-272 (the action at line 261) |
| `ci.yml:122-142` (setup) | `moon setup` at 124-125, the serial rustup step at 140-144; the `T=(…)` array at line 269 |
| `moon.yml:806-846` (`repo:workflow-credentials`) | lines 828-868 |

Plan decisions that refine the spec (Sven can object at plan review):

- The container runs as `--user 65534:65534` (`nobody`), not as the runner's uid. Node's `os.userInfo()` in `generate-wasm --post` throws for a uid without a passwd entry, and the image has no entry for uid 1001. Task 6 (M2) measures this.
- The image is `docker.io/library/rust:1.95.0-bookworm@sha256:6258907abe69656e41cd992e0b705cdcfabcbbe3db374f92ed2d47121282d4a1` (the multi-platform index digest, resolved on 2026-10-02). It carries rustup, gcc, git and curl.
- Container run 2 makes a git repository INSIDE the container (Moon's `vcs.client` is git, and the work copy has no `.git`), installs proto with its install script at the `.prototools` pin, and always runs `pnpm --dir ts install --frozen-lockfile`.
- The `propose` command-word allowlist adds `grep` (the 404 body check) and `sed` (the PR number) to the spec's list.
- The P6 rule pins each action NAME and a full 40-hex SHA, not one specific SHA, so a dependabot action bump does not turn the gate red.
- `lockstep_check.py` gets two more subcommands: `current` (the family invariant on HEAD's lock; the gate's real run) and `status` (the `git status --porcelain` check of `propose` step 4, out of shell).
- The new `ci/wasm-lockstep/uv.lock` joins `repo:osv` (`ci/osv/run.sh` LOCKFILES and the task inputs). The spec does not list this, but every other gate lockfile is scanned there.
- One bullet in `.github/CLAUDE.md` records that `release-pr` now has two consumers.

---

## The command prelude

The Bash tool does not keep variables between calls. Start EVERY command block in this plan with these three lines. Put your session scratchpad directory (the system prompt names it) into `SCRATCH`.

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-693-wasm-lockstep
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
export SCRATCH=/path/to/your/session/scratchpad
```

Two bash shims (Task 0 makes them) select the bash for a gate: `PATH="$SCRATCH/bash32:$PATH"` gives `/bin/bash` 3.2.57, and `PATH="$SCRATCH/bash5:$PATH"` gives Homebrew bash 5. Run a gate script by its path (`./ci/<gate>/run.sh`), so its `#!/usr/bin/env bash` picks the shim. The worktree sandbox can refuse a command whose program is literally `bash`.

---

### Task 0: Provision the worktree

**Files:**
- Commit: `docs/superpowers/specs/2026-09-27-sma-693-wasm-bindgen-lockstep-updater-design.md` (untracked now)
- Commit: `docs/superpowers/plans/2026-10-02-sma-693-wasm-bindgen-lockstep-updater.md` (this file)

**Interfaces:**
- Consumes: nothing.
- Produces: a provisioned worktree on branch `feature/sma-693-wasm-bindgen-lockstep-updater`; the shims `$SCRATCH/bash32/bash` and `$SCRATCH/bash5/bash`; the venv interpreter `ci/wasm-lockstep/.venv/bin/python3` comes in Task 2.

- [ ] **Step 1: Check the branch and the base**

```bash
git rev-parse --abbrev-ref HEAD
git log --oneline -1
git status --short
```

Expected: `feature/sma-693-wasm-bindgen-lockstep-updater`; HEAD `4face4af` or a later commit of this branch; only the spec and this plan untracked. If the branch is `main` or a `worktree-*` name, stop and ask.

- [ ] **Step 2: Install the dependencies**

```bash
proto install
pnpm -C ts install
( cd py && uv sync )
( cd rs && cargo fetch )
```

Expected: each command exits 0. `pnpm -C ts install` also installs commitlint and the lefthook hooks. Without it, the `commit-msg` hook fails with `commitlint not found`. Do not bypass the hook.

- [ ] **Step 3: Make the two bash shims**

```bash
mkdir -p "$SCRATCH/bash32" "$SCRATCH/bash5"
ln -sf /bin/bash "$SCRATCH/bash32/bash"
ln -sf /opt/homebrew/bin/bash "$SCRATCH/bash5/bash"
PATH="$SCRATCH/bash32:$PATH" bash --version | sed -n 1p
PATH="$SCRATCH/bash5:$PATH" bash --version | sed -n 1p
```

Expected: `GNU bash, version 3.2.57…` and `GNU bash, version 5.…`.

- [ ] **Step 4: Commit the spec and the plan**

```bash
git add docs/superpowers/specs/2026-09-27-sma-693-wasm-bindgen-lockstep-updater-design.md docs/superpowers/plans/2026-10-02-sma-693-wasm-bindgen-lockstep-updater.md
git commit -m "docs(ci): add the SMA-693 wasm-bindgen lockstep updater spec and plan" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

Expected: one commit. If the commit fails with "failed to fill whole buffer", 1Password is locked: stop and ask Sven to unlock it. Do not use `--no-gpg-sign`.

---

### Task 1: `lockstep_check.py` — the lock and artifact checker

**Files:**
- Create: `ci/wasm-lockstep/lockstep_check.py`

**Interfaces:**
- Consumes: nothing (stdlib only: `os`, `re`, `stat`, `sys`, `tempfile`, `tomllib`).
- Produces (later tasks call these exactly):
  - CLI `python3 ci/wasm-lockstep/lockstep_check.py lock --old <lock> --new <lock>` -> exit 0 and lines `family-moved <name> <old|-> <new|->`; exit 4 and lines `family-current <name> <version>`; exit 3 with `lockstep_check: REFUSED R-<CODE>: …` on stderr; exit 2 for infrastructure.
  - CLI `… artifact --dir <dir> --old <lock> [--body-file <f>] [--title-file <f>]` -> exit 0 (writes the two files) or 3/2.
  - CLI `… status --file <git-status-porcelain-file>` -> exit 0 or 3 (`R-STATUS`).
  - CLI `… current --lock <lock>` -> exit 0 and `family-current` lines, or 3.
  - CLI `… --self-test` -> exit 0 (all rows pass) or 3.
  - CLI `… --negative-control --lock <lock>` -> exit 0 or 3.
  - Python: `lock_verdict(old: dict, new: dict) -> list[tuple[str, str, str]]`, `current_verdict(lock: dict) -> list[tuple[str, str]]`, `run_artifact(directory: str, old_path: str, body_file: str | None, title_file: str | None) -> list[tuple[str, str, str]]`, `status_verdict(text: str) -> None`; exceptions `RefusalError(code, message)` (attribute `.code`), `InfraError`, `NoChangeError`.

- [ ] **Step 1: Write the file with the fixture table and stub verdicts**

Create `ci/wasm-lockstep/lockstep_check.py` with exactly this content. The section `# ---- the verdicts ----` holds stubs that Step 3 replaces.

```python
# SPDX-License-Identifier: Apache-2.0
"""The lock and artifact checker of the wasm-lockstep workflow (SMA-693 spec 5.3).

Stdlib only (tomllib, Python 3.11+). The `propose` job runs this file with the runner's own
`python3`, and installs nothing, so it must never import a third-party package.

  lockstep_check.py lock --old <lock> --new <lock>
  lockstep_check.py artifact --dir <dir> --old <lock> [--body-file <f>] [--title-file <f>]
  lockstep_check.py status --file <git-status-porcelain-file>
  lockstep_check.py current --lock <lock>
  lockstep_check.py --self-test
  lockstep_check.py --negative-control --lock <lock>

Exit codes: 0 pass | 3 refusal | 2 infrastructure | 4 no change. Not 1: `uv` and a Python
traceback both exit 1, so 1 cannot mean "the lock is wrong". ci/wasm-lockstep/run.sh maps
3 -> 1 and every other non-zero code -> 2. Do not "normalize" the refusal code to 1.

Every refusal carries a CODE (R-...). The self-test asserts the code of each row, so a row
cannot pass because a different check happened to refuse it.
"""

from __future__ import annotations

import os
import re
import stat
import sys
import tempfile
import tomllib

RC_OK = 0
RC_INFRA = 2
RC_REFUSE = 3
RC_NO_CHANGE = 4

# The seven packages the four-package `cargo update -p` moved at SMA-683 M0 (spec F1).
FAMILY = frozenset({
    "wasm-bindgen",
    "wasm-bindgen-macro",
    "wasm-bindgen-macro-support",
    "wasm-bindgen-shared",
    "js-sys",
    "web-sys",
    "wasm-bindgen-futures",
})
CRATES_IO = "registry+https://github.com/rust-lang/crates.io-index"
# Strict semver core: no pre-release, no build metadata, no leading zero. `[0-9]`, never `\d`:
# `\d` also matches non-ASCII digits such as U+FF11. Used with fullmatch(), never match(): `$`
# in a Python pattern also matches before a trailing newline.
SEMVER = re.compile(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)")
CHECKSUM = re.compile(r"[0-9a-f]{64}")
LOCK_PATH = "rs/Cargo.lock"
ARTIFACT_DIR = "rs/crates/bindings/paigasus-wasm"
ARTIFACT_FILES = (
    "paigasus_wasm.js",
    "paigasus_wasm_bg.js",
    "paigasus_wasm.d.ts",
    "paigasus_wasm_bg.wasm.d.ts",
    "paigasus_wasm_bg.wasm",
)
ALLOWED_FILES = frozenset({LOCK_PATH} | {f"{ARTIFACT_DIR}/{name}" for name in ARTIFACT_FILES})
ALLOWED_DIRS = frozenset({"rs", "rs/crates", "rs/crates/bindings", ARTIFACT_DIR})
# 8 MiB per file. The committed paigasus_wasm_bg.wasm is 50,950 bytes (2026-10-02).
SIZE_CAP = 8 * 1024 * 1024
TITLE_MAX = 100
RUNBOOK = 'rs/CLAUDE.md, "The wasm-bindgen family does not move through dependabot"'


class RefusalError(Exception):
    """The input is wrong. Maps to RC_REFUSE."""

    def __init__(self, code: str, message: str) -> None:
        super().__init__(f"{code}: {message}")
        self.code = code


class InfraError(Exception):
    """The check could not run. Maps to RC_INFRA."""


class NoChangeError(Exception):
    """The lock did not change. Maps to RC_NO_CHANGE."""


def parse_lock_text(text: str, label: str) -> dict:
    try:
        return tomllib.loads(text)
    except tomllib.TOMLDecodeError as exc:
        raise InfraError(f"{label} is not valid TOML: {exc}") from exc


def load_lock(path: str) -> dict:
    try:
        with open(path, encoding="utf-8") as handle:
            text = handle.read()
    except (OSError, UnicodeDecodeError) as exc:
        raise InfraError(f"cannot read {path}: {exc}") from exc
    return parse_lock_text(text, path)


def packages(lock: dict, label: str) -> list[dict]:
    raw = lock.get("package", [])
    if not isinstance(raw, list):
        raise RefusalError("R-SHAPE", f"{label}: `package` is not an array of tables")
    for entry in raw:
        if not isinstance(entry, dict) or not isinstance(entry.get("name"), str) \
                or not isinstance(entry.get("version"), str):
            raise RefusalError("R-SHAPE", f"{label}: a [[package]] entry has no string name and version: {entry!r}")
        deps = entry.get("dependencies", [])
        if not isinstance(deps, list) or not all(isinstance(d, str) for d in deps):
            raise RefusalError("R-SHAPE", f"{label}: {entry['name']} has a `dependencies` value that is not a list of strings")
    return raw


def key_of(entry: dict) -> tuple[str, str, str]:
    return (entry["name"], entry["version"], str(entry.get("source", "")))


def normalise_ref(ref: str) -> str:
    """Cargo writes a dependency reference in three forms: `name`, `name version` and
    `name version (source)`. A FAMILY reference becomes the bare name, so a family version move
    does not count as a change of the package that refers to it. Any other reference stays
    exactly as written."""
    name = ref.split(" ", 1)[0]
    return name if name in FAMILY else ref


def normalised(entry: dict) -> dict:
    out = dict(entry)
    if "dependencies" in out:
        out["dependencies"] = [normalise_ref(d) for d in out["dependencies"]]
    return out


def write_file(path: str, text: str) -> None:
    try:
        with open(path, "w", encoding="utf-8") as handle:
            handle.write(text)
    except OSError as exc:
        raise InfraError(f"cannot write {path}: {exc}") from exc


# ---- the verdicts ----------------------------------------------------------------------------

def _stub(name: str):
    raise InfraError(f"{name} is not implemented yet")


def check_semver(entry: dict, label: str) -> None:
    _stub("check_semver")


def lock_verdict(old: dict, new: dict) -> list[tuple[str, str, str]]:
    _stub("lock_verdict")


def current_verdict(lock: dict) -> list[tuple[str, str]]:
    _stub("current_verdict")


def artifact_tree(root: str) -> None:
    _stub("artifact_tree")


def title_for(changes: list[tuple[str, str, str]]) -> str:
    _stub("title_for")


def body_for(changes: list[tuple[str, str, str]]) -> str:
    _stub("body_for")


def status_verdict(text: str) -> None:
    _stub("status_verdict")


def run_artifact(directory: str, old_path: str, body_file: str | None, title_file: str | None) -> list[tuple[str, str, str]]:
    _stub("run_artifact")


# ---- the self-test --------------------------------------------------------------------------

def _pkg(name: str, version: str, source: str | None = CRATES_IO, checksum: str | None = "a" * 64,
         deps: tuple[str, ...] = ()) -> str:
    out = f'[[package]]\nname = "{name}"\nversion = "{version}"\n'
    if source is not None:
        out += f'source = "{source}"\n'
    if checksum is not None:
        out += f'checksum = "{checksum}"\n'
    if deps:
        out += "dependencies = [\n" + "".join(f' "{d}",\n' for d in deps) + "]\n"
    return out + "\n"


HEADER = "# This file is automatically @generated by Cargo.\n# It is not intended for manual editing.\nversion = 4\n\n"
OLD_FAMILY = (("js-sys", "0.3.105"), ("wasm-bindgen", "0.2.128"), ("wasm-bindgen-futures", "0.4.78"),
              ("wasm-bindgen-macro", "0.2.128"), ("wasm-bindgen-macro-support", "0.2.128"),
              ("wasm-bindgen-shared", "0.2.128"), ("web-sys", "0.3.105"))
NEW_FAMILY = (("js-sys", "0.3.106"), ("wasm-bindgen", "0.2.129"), ("wasm-bindgen-futures", "0.4.79"),
              ("wasm-bindgen-macro", "0.2.129"), ("wasm-bindgen-macro-support", "0.2.129"),
              ("wasm-bindgen-shared", "0.2.129"), ("web-sys", "0.3.106"))
REST = _pkg("anyhow", "1.0.98", checksum="b" * 64) + _pkg("paigasus-wasm", "0.1.0", source=None, checksum=None, deps=("wasm-bindgen",))


def _lock(family=OLD_FAMILY, rest: str = REST, header: str = HEADER, checksum: str = "a" * 64) -> str:
    return header + "".join(_pkg(n, v, checksum=checksum) for n, v in family) + rest


def _outcome(fn) -> str:
    try:
        fn()
    except RefusalError as exc:
        return exc.code
    except NoChangeError:
        return "NOCHANGE"
    except InfraError:
        return "INFRA"
    return "PASS"


def _verdict(old_text: str, new_text: str):
    return lambda: lock_verdict(parse_lock_text(old_text, "old"), parse_lock_text(new_text, "new"))


def _ref_rows() -> list[tuple[str, object, str]]:
    """The three Cargo reference forms, each in a non-family package's dependencies list. The
    family move must not count as a change of that package. The negative twins prove the
    normalisation is scoped to FAMILY names: the same forms on a non-family name still refuse."""
    rows = []
    forms = (
        ("name", "wasm-bindgen", "wasm-bindgen"),
        ("name version", "wasm-bindgen 0.2.128", "wasm-bindgen 0.2.129"),
        ("name version (source)", f"wasm-bindgen 0.2.128 ({CRATES_IO})", f"wasm-bindgen 0.2.129 ({CRATES_IO})"),
    )
    for label, old_ref, new_ref in forms:
        old = _lock(rest=REST + _pkg("consumer", "1.0.0", deps=(old_ref,)))
        new = _lock(NEW_FAMILY, rest=REST + _pkg("consumer", "1.0.0", deps=(new_ref,)))
        rows.append((f"reference form `{label}` to a family package", _verdict(old, new), "PASS"))
    twin_old = _lock(rest=REST + _pkg("consumer", "1.0.0", deps=("anyhow 1.0.98",)))
    twin_new = _lock(NEW_FAMILY, rest=REST + _pkg("consumer", "1.0.0", deps=("anyhow 1.0.99",)))
    rows.append(("reference form `name version` to a NON-family package", _verdict(twin_old, twin_new), "R-NONFAMILY"))
    return rows


def _write_tree(root: str, files: dict[str, bytes], links: dict[str, str] | None = None) -> None:
    for rel, data in files.items():
        path = os.path.join(root, rel)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "wb") as handle:
            handle.write(data)
    for rel, target in (links or {}).items():
        path = os.path.join(root, rel)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        os.symlink(target, path)


def _artifact_rows(tmp: str) -> list[tuple[str, object, str]]:
    old_path = os.path.join(tmp, "old.lock")
    write_file(old_path, _lock())
    good = {LOCK_PATH: _lock(NEW_FAMILY).encode()}
    good.update({f"{ARTIFACT_DIR}/{n}": b"x" for n in ARTIFACT_FILES})
    cases: list[tuple[str, dict[str, bytes], dict[str, str] | None, str]] = [
        ("artifact in the real downloaded layout", good, None, "PASS"),
        ("artifact with only the lock and two glue files", {k: v for k, v in good.items() if k == LOCK_PATH or k.endswith(".js")}, None, "PASS"),
        ("artifact under an extra rs/ level", {f"rs/{k}": v for k, v in good.items()}, None, "R-LAYOUT"),
        ("artifact without the rs/ level", {k[len("rs/"):]: v for k, v in good.items()}, None, "R-LAYOUT"),
        ("a sixth artifact file", {**good, f"{ARTIFACT_DIR}/extra.js": b"x"}, None, "R-LAYOUT"),
        ("a case variant of an artifact name", {LOCK_PATH: good[LOCK_PATH], f"{ARTIFACT_DIR}/Paigasus_wasm.js": b"x"}, None, "R-LAYOUT"),
        ("a missing rs/Cargo.lock", {k: v for k, v in good.items() if k != LOCK_PATH}, None, "R-LAYOUT"),
        ("a symlinked artifact file", {LOCK_PATH: good[LOCK_PATH]}, {f"{ARTIFACT_DIR}/paigasus_wasm.js": "/etc/hostname"}, "R-SYMLINK"),
        ("a symlinked parent directory", {LOCK_PATH: good[LOCK_PATH]}, {"rs/crates": "/tmp"}, "R-SYMLINK"),
        ("an artifact lock equal to the old lock", {**good, LOCK_PATH: _lock().encode()}, None, "R-NOCHANGE"),
        ("an artifact lock that does not parse", {**good, LOCK_PATH: b"version = = 4\n"}, None, "INFRA"),
    ]
    rows: list[tuple[str, object, str]] = []
    for index, (label, files, links, want) in enumerate(cases):
        root = os.path.join(tmp, f"case{index}")
        os.makedirs(root)
        _write_tree(root, files, links)
        rows.append((label, (lambda r=root: run_artifact(r, old_path, None, None)), want))
    # The size cap: a sparse file one byte over 8 MiB costs no disk.
    big = os.path.join(tmp, "big")
    _write_tree(big, {LOCK_PATH: good[LOCK_PATH], f"{ARTIFACT_DIR}/paigasus_wasm_bg.wasm": b""})
    os.truncate(os.path.join(big, ARTIFACT_DIR, "paigasus_wasm_bg.wasm"), SIZE_CAP + 1)
    rows.append(("an oversize artifact file", lambda: run_artifact(big, old_path, None, None), "R-SIZE"))
    return rows


def _files_rows(tmp: str) -> list[tuple[str, object, str]]:
    """The title and body files exist after a pass and do not exist after a refusal."""
    old_path = os.path.join(tmp, "files-old.lock")
    write_file(old_path, _lock())
    rows = []
    for label, new_text, want_files in (("pass", _lock(NEW_FAMILY), True),
                                        ("refusal", _lock(NEW_FAMILY, rest=REST + _pkg("evil", "1.0.0")), False)):
        root = os.path.join(tmp, f"files-{label}")
        _write_tree(root, {LOCK_PATH: new_text.encode()})
        title = os.path.join(tmp, f"title-{label}.txt")
        body = os.path.join(tmp, f"body-{label}.md")

        def check(root=root, title=title, body=body, want_files=want_files) -> None:
            outcome = _outcome(lambda: run_artifact(root, old_path, body, title))
            exists = os.path.exists(title) or os.path.exists(body)
            if exists != want_files:
                raise RefusalError("R-SELFTEST", f"the title/body files exist={exists} after a {outcome}")
            if want_files:
                with open(title, encoding="utf-8") as handle:
                    if handle.read() != "build(deps): move wasm-bindgen to 0.2.129 and regenerate the wasm glue\n":
                        raise RefusalError("R-SELFTEST", "the title file holds the wrong text")
        rows.append((f"the title and body files after a {label}", check, "PASS"))
    return rows


def self_test() -> int:
    eighth = REST + _pkg("bumpalo", "3.19.0")
    changed_rest = _pkg("anyhow", "1.0.99", checksum="b" * 64) + _pkg("paigasus-wasm", "0.1.0", source=None, checksum=None, deps=("wasm-bindgen",))
    dropped = tuple(p for p in NEW_FAMILY if p[0] != "wasm-bindgen-macro-support")
    git_source = tuple((n, v) for n, v in NEW_FAMILY if n != "web-sys")
    git_web_sys = _pkg("web-sys", "0.3.106", source="git+https://github.com/evil/web-sys#abc", checksum=None)
    no_checksum = _pkg("web-sys", "0.3.106", checksum=None)
    rows: list[tuple[str, object, str]] = [
        ("the M0 bump of all seven", _verdict(_lock(), _lock(NEW_FAMILY)), "PASS"),
        ("a bump that drops one family package", _verdict(_lock(), _lock(dropped)), "PASS"),
        ("an eighth package added", _verdict(_lock(), _lock(NEW_FAMILY, rest=eighth)), "R-NONFAMILY"),
        ("a non-family package changed", _verdict(_lock(), _lock(NEW_FAMILY, rest=changed_rest)), "R-NONFAMILY"),
        ("a git source on a family entry", _verdict(_lock(), _lock(git_source, rest=REST + git_web_sys)), "R-SOURCE"),
        ("a family entry without a checksum", _verdict(_lock(), _lock(git_source, rest=REST + no_checksum)), "R-CHECKSUM"),
        ("a family name moved twice", _verdict(_lock((*OLD_FAMILY, ("wasm-bindgen", "0.2.127"))), _lock(NEW_FAMILY)), "R-TWICE"),
        ("two versions of one family name in the new lock", _verdict(_lock(), _lock((*OLD_FAMILY, ("wasm-bindgen", "0.2.129")))), "R-DUPLICATE"),
        ("a shell payload in a version string", _verdict(_lock(), _lock((("wasm-bindgen", '0.2.129\\"; rm -rf /'),))), "R-SEMVER"),
        ("a pre-release version", _verdict(_lock(), _lock((("wasm-bindgen", "0.2.129-rc.1"),))), "R-SEMVER"),
        ("a version with a trailing newline", _verdict(_lock(), _lock((("wasm-bindgen", "0.2.129\\n"),))), "R-SEMVER"),
        ("a version with non-ASCII digits", _verdict(_lock(), _lock((("wasm-bindgen", "0.2." + chr(0xFF11) + chr(0xFF12) + chr(0xFF19)),))), "R-SEMVER"),
        ("a version with a leading zero", _verdict(_lock(), _lock((("wasm-bindgen", "0.2.0129"),))), "R-SEMVER"),
        ("no change", _verdict(_lock(), _lock()), "NOCHANGE"),
        ("the lock format version changed", _verdict(_lock(), _lock(NEW_FAMILY, header=HEADER.replace("version = 4", "version = 3"))), "R-FORMAT"),
        ("a [patch.unused] table added", _verdict(_lock(), _lock(NEW_FAMILY) + '[[patch.unused]]\nname = "x"\nversion = "1.0.0"\n'), "R-FORMAT"),
        ("a family checksum changed in place", _verdict(_lock(), _lock(checksum="c" * 64)), "R-INPLACE"),
        ("a lock that does not parse", _verdict(_lock(), "version = = 4\n"), "INFRA"),
        ("current: the real shape", lambda: current_verdict(parse_lock_text(_lock(), "lock")), "PASS"),
        ("current: two versions side by side", lambda: current_verdict(parse_lock_text(_lock((*OLD_FAMILY, ("wasm-bindgen", "0.2.129"))), "lock")), "R-DUPLICATE"),
        ("current: no wasm-bindgen", lambda: current_verdict(parse_lock_text(_lock(OLD_FAMILY[:1]), "lock")), "R-ABSENT"),
        ("status: the lock and two glue files", lambda: status_verdict(f" M {LOCK_PATH}\n M {ARTIFACT_DIR}/paigasus_wasm.js\n M {ARTIFACT_DIR}/paigasus_wasm_bg.wasm\n"), "PASS"),
        ("status: an untracked file", lambda: status_verdict(f" M {LOCK_PATH}\n?? {ARTIFACT_DIR}/extra.js\n"), "R-STATUS"),
        ("status: another tracked file", lambda: status_verdict(f" M {LOCK_PATH}\n M rs/Cargo.toml\n"), "R-STATUS"),
        ("status: the lock is not modified", lambda: status_verdict(f" M {ARTIFACT_DIR}/paigasus_wasm.js\n"), "R-STATUS"),
        ("title: an over-long version", lambda: title_for([("wasm-bindgen", "0.2.128", "0." + "9" * 80 + ".0")]), "R-TITLE"),
    ]
    rows += _ref_rows()
    failures = 0
    with tempfile.TemporaryDirectory() as tmp:
        rows += _artifact_rows(tmp)
        rows += _files_rows(tmp)
        for label, fn, want in rows:
            got = _outcome(fn)
            if got == want:
                print(f"  ok    {label}: {got}")
            else:
                print(f"  FAIL  {label}: want {want}, got {got}", file=sys.stderr)
                failures += 1
    print(f"lockstep_check self-test: {len(rows)} rows, {failures} failed")
    return RC_REFUSE if failures else RC_OK


def negative_control(lock_path: str) -> int:
    """The real `lock` verdict on HEAD's rs/Cargo.lock (the caller writes it with `git show`, F15):
    the lock against itself is NOCHANGE; one non-family version changed is R-NONFAMILY; a
    synthetic move of the real wasm-bindgen entries passes."""
    try:
        with open(lock_path, encoding="utf-8") as handle:
            text = handle.read()
    except OSError as exc:
        raise InfraError(f"cannot read {lock_path}: {exc}") from exc
    real = parse_lock_text(text, lock_path)
    family = current_verdict(real)
    failures = 0

    def expect(label: str, fn, want: str) -> None:
        nonlocal failures
        got = _outcome(fn)
        if got == want:
            print(f"  ok    {label}: {got}")
        else:
            print(f"  FAIL  {label}: want {want}, got {got}", file=sys.stderr)
            failures += 1

    expect("HEAD's lock against itself", lambda: lock_verdict(real, real), "NOCHANGE")
    first = next(p for p in packages(real, "lock") if p["name"] not in FAMILY and "source" in p)
    mutated = parse_lock_text(text, lock_path)
    for entry in mutated["package"]:
        if key_of(entry) == key_of(first):
            entry["version"] = "999.0.0"
    expect(f"HEAD's lock with {first['name']} moved", lambda: lock_verdict(real, mutated), "R-NONFAMILY")
    bumped = parse_lock_text(text, lock_path)
    for entry in bumped["package"]:
        if entry["name"] in FAMILY:
            major, minor, patch = entry["version"].split(".")
            entry["version"] = f"{major}.{minor}.{int(patch) + 1}"
            entry["checksum"] = "d" * 64
    expect("HEAD's lock with every family patch version moved", lambda: lock_verdict(real, bumped), "PASS")
    print(f"lockstep_check negative control: wasm-bindgen {dict(family)['wasm-bindgen']} on HEAD, {failures} failed")
    return RC_REFUSE if failures else RC_OK


# ---- the command line -----------------------------------------------------------------------

def _options(argv: list[str], names: tuple[str, ...], required: tuple[str, ...]) -> dict[str, str]:
    out: dict[str, str] = {}
    rest = list(argv)
    while rest:
        flag = rest.pop(0)
        if flag not in names or not rest:
            raise InfraError(f"usage: unknown flag or missing value at {flag!r}")
        out[flag] = rest.pop(0)
    for name in required:
        if name not in out:
            raise InfraError(f"usage: {name} is required")
    return out


def main(argv: list[str]) -> int:
    if argv == ["--self-test"]:
        return self_test()
    if argv[:1] == ["--negative-control"]:
        opts = _options(argv[1:], ("--lock",), ("--lock",))
        return negative_control(opts["--lock"])
    if not argv:
        raise InfraError("usage: lockstep_check.py lock|artifact|status|current ... | --self-test | --negative-control --lock <path>")
    command, rest = argv[0], argv[1:]
    if command == "lock":
        opts = _options(rest, ("--old", "--new"), ("--old", "--new"))
        try:
            changes = lock_verdict(load_lock(opts["--old"]), load_lock(opts["--new"]))
        except NoChangeError:
            for name, version in current_verdict(load_lock(opts["--new"])):
                print(f"family-current {name} {version}")
            return RC_NO_CHANGE
        for name, old, new in changes:
            print(f"family-moved {name} {old} {new}")
        return RC_OK
    if command == "artifact":
        opts = _options(rest, ("--dir", "--old", "--body-file", "--title-file"), ("--dir", "--old"))
        changes = run_artifact(opts["--dir"], opts["--old"], opts.get("--body-file"), opts.get("--title-file"))
        for name, old, new in changes:
            print(f"family-moved {name} {old} {new}")
        return RC_OK
    if command == "status":
        opts = _options(rest, ("--file",), ("--file",))
        try:
            with open(opts["--file"], encoding="utf-8") as handle:
                text = handle.read()
        except OSError as exc:
            raise InfraError(f"cannot read {opts['--file']}: {exc}") from exc
        status_verdict(text)
        print("status: only allowed files changed")
        return RC_OK
    if command == "current":
        opts = _options(rest, ("--lock",), ("--lock",))
        for name, version in current_verdict(load_lock(opts["--lock"])):
            print(f"family-current {name} {version}")
        return RC_OK
    raise InfraError(f"unknown command {command!r}")


if __name__ == "__main__":
    try:
        sys.exit(main(sys.argv[1:]))
    except RefusalError as exc:
        print(f"lockstep_check: REFUSED {exc}. Nothing was pushed. The manual runbook is in {RUNBOOK}.", file=sys.stderr)
        sys.exit(RC_REFUSE)
    except InfraError as exc:
        print(f"lockstep_check: infrastructure error: {exc}", file=sys.stderr)
        sys.exit(RC_INFRA)
```

- [ ] **Step 2: Run the self-test and see it fail**

```bash
git -C . show HEAD:rs/Cargo.lock > "$SCRATCH/head.lock"
PY="$(uv run --locked --project ci/workflow-credentials --python '>=3.12' python3 -c 'import sys; print(sys.executable)')"
"$PY" ci/wasm-lockstep/lockstep_check.py --self-test; echo "rc=$?"
```

(The `ci/workflow-credentials` venv is only a Python 3.12 for this task. Task 2 makes this gate's own venv.)

Expected: `lockstep_check self-test: 44 rows, 41 failed` and `rc=3`. Only the three rows that want `INFRA` pass.

- [ ] **Step 3: Replace the stub section with the verdicts**

Replace everything from the line `# ---- the verdicts ---…` up to (not including) the line `# ---- the self-test ---…`. Keep two blank lines before the self-test line. The new section is exactly this:

```python
# ---- the verdicts ----------------------------------------------------------------------------

def check_semver(entry: dict, label: str) -> None:
    version = entry["version"]
    if not SEMVER.fullmatch(version):
        raise RefusalError("R-SEMVER", f"{label}: {entry['name']} has the version {version!r}, which is not a strict X.Y.Z")


def lock_verdict(old: dict, new: dict) -> list[tuple[str, str, str]]:
    """The AC4.2 verdict. Returns (name, old version or '-', new version or '-') per moved family
    package, sorted. Raises RefusalError or NoChangeError."""
    old_top = {k: v for k, v in old.items() if k != "package"}
    new_top = {k: v for k, v in new.items() if k != "package"}
    if old_top != new_top:
        raise RefusalError("R-FORMAT", f"a top-level lock table or the lock format version changed: {sorted(set(old_top) | set(new_top))}")
    old_pkgs = packages(old, "old lock")
    new_pkgs = packages(new, "new lock")
    old_by_key: dict[tuple[str, str, str], dict] = {}
    new_by_key: dict[tuple[str, str, str], dict] = {}
    for by_key, pkgs, label in ((old_by_key, old_pkgs, "old lock"), (new_by_key, new_pkgs, "new lock")):
        for entry in pkgs:
            if key_of(entry) in by_key:
                raise RefusalError("R-SHAPE", f"{label}: {key_of(entry)} appears twice")
            by_key[key_of(entry)] = entry

    # Non-family packages must be identical after the family references are normalised. This
    # also refuses an ADDED or REMOVED non-family package: a new transitive dependency.
    old_rest = {k: normalised(v) for k, v in old_by_key.items() if k[0] not in FAMILY}
    new_rest = {k: normalised(v) for k, v in new_by_key.items() if k[0] not in FAMILY}
    if old_rest != new_rest:
        changed = sorted({k[0] for k in set(old_rest) ^ set(new_rest)}
                         | {k[0] for k in set(old_rest) & set(new_rest) if old_rest[k] != new_rest[k]})
        raise RefusalError("R-NONFAMILY", f"a package outside the wasm-bindgen family changed, was added or was removed: {changed}")

    for label, by_key in (("old lock", old_by_key), ("new lock", new_by_key)):
        for k, entry in by_key.items():
            if k[0] in FAMILY:
                check_semver(entry, label)

    removed = [k for k in old_by_key if k not in new_by_key]
    added = [k for k in new_by_key if k not in old_by_key]
    for name in FAMILY:
        if sum(1 for k in removed if k[0] == name) > 1 or sum(1 for k in added if k[0] == name) > 1:
            raise RefusalError("R-TWICE", f"{name} has more than one removed or more than one added entry")
    new_names = [k[0] for k in new_by_key if k[0] in FAMILY]
    for name in sorted(set(new_names)):
        if new_names.count(name) > 1:
            raise RefusalError("R-DUPLICATE", f"the new lock holds two versions of {name} side by side")
    for k in added:
        added_entry = new_by_key[k]
        if added_entry.get("source") != CRATES_IO:
            raise RefusalError("R-SOURCE", f"{k[0]} {k[1]} does not come from crates.io (source {added_entry.get('source')!r})")
        checksum = added_entry.get("checksum")
        if not isinstance(checksum, str) or not CHECKSUM.fullmatch(checksum):
            raise RefusalError("R-CHECKSUM", f"{k[0]} {k[1]} has no valid checksum")
    for k in set(old_by_key) & set(new_by_key):
        if k[0] in FAMILY and old_by_key[k].get("checksum") != new_by_key[k].get("checksum"):
            raise RefusalError("R-INPLACE", f"{k[0]} {k[1]} kept its version but changed its checksum")

    if not removed and not added:
        raise NoChangeError("the wasm-bindgen family did not move")
    rows: dict[str, list[str]] = {}
    for k in removed:
        rows.setdefault(k[0], ["-", "-"])[0] = k[1]
    for k in added:
        rows.setdefault(k[0], ["-", "-"])[1] = k[1]
    return sorted((name, pair[0], pair[1]) for name, pair in rows.items())


def current_verdict(lock: dict) -> list[tuple[str, str]]:
    """The family invariant on ONE lock, the precondition of every weekly `lock` verdict:
    wasm-bindgen is present, no family name twice, crates.io source, a checksum, strict semver."""
    found: dict[str, str] = {}
    for entry in packages(lock, "lock"):
        name = entry["name"]
        if name not in FAMILY:
            continue
        if name in found:
            raise RefusalError("R-DUPLICATE", f"the lock holds two versions of {name} side by side")
        check_semver(entry, "lock")
        if entry.get("source") != CRATES_IO:
            raise RefusalError("R-SOURCE", f"{name} does not come from crates.io")
        checksum = entry.get("checksum")
        if not isinstance(checksum, str) or not CHECKSUM.fullmatch(checksum):
            raise RefusalError("R-CHECKSUM", f"{name} has no valid checksum")
        found[name] = entry["version"]
    if "wasm-bindgen" not in found:
        raise RefusalError("R-ABSENT", "the lock holds no wasm-bindgen entry")
    return sorted(found.items())


def artifact_tree(root: str) -> None:
    """AC4.1: rs/Cargo.lock plus a subset of the five artifact files, all regular files under the
    size cap, no symlink, no other entry, no directory other than the path prefixes."""
    if not os.path.isdir(root) or os.path.islink(root):
        raise RefusalError("R-LAYOUT", f"{root} is not a directory")
    seen_files: set[str] = set()
    pending = [""]
    while pending:
        rel_dir = pending.pop()
        with os.scandir(os.path.join(root, rel_dir) if rel_dir else root) as it:
            entries = sorted(it, key=lambda e: e.name)
        for entry in entries:
            rel = f"{rel_dir}/{entry.name}" if rel_dir else entry.name
            info = entry.stat(follow_symlinks=False)
            if stat.S_ISLNK(info.st_mode):
                raise RefusalError("R-SYMLINK", f"{rel} is a symlink")
            if stat.S_ISDIR(info.st_mode):
                if rel not in ALLOWED_DIRS:
                    raise RefusalError("R-LAYOUT", f"{rel}/ is not an allowed directory")
                pending.append(rel)
            elif stat.S_ISREG(info.st_mode):
                if rel not in ALLOWED_FILES:
                    raise RefusalError("R-LAYOUT", f"{rel} is not one of the six allowed files")
                if info.st_size > SIZE_CAP:
                    raise RefusalError("R-SIZE", f"{rel} is {info.st_size} bytes, over the {SIZE_CAP}-byte cap")
                seen_files.add(rel)
            else:
                raise RefusalError("R-LAYOUT", f"{rel} is not a regular file or a directory")
    if LOCK_PATH not in seen_files:
        raise RefusalError("R-LAYOUT", f"the artifact has no {LOCK_PATH}")


def title_for(changes: list[tuple[str, str, str]]) -> str:
    moved = {name: new for name, _old, new in changes}
    if moved.get("wasm-bindgen", "-") != "-":
        title = f"build(deps): move wasm-bindgen to {moved['wasm-bindgen']} and regenerate the wasm glue"
    else:
        title = "build(deps): move the wasm-bindgen family and regenerate the wasm glue"
    if len(title) > TITLE_MAX:
        raise RefusalError("R-TITLE", f"the commit title is {len(title)} characters, over the commitlint maximum of {TITLE_MAX}")
    return title


def body_for(changes: list[tuple[str, str, str]]) -> str:
    lines = [
        "This pull request moves the wasm-bindgen family in lockstep and regenerates the committed wasm artifacts.",
        "The scheduled `wasm-lockstep` workflow made it (SMA-693).",
        "",
        "| Package | Old | New |",
        "|---|---|---|",
    ]
    lines += [f"| `{name}` | `{old}` | `{new}` |" for name, old, new in changes]
    lines += [
        "",
        "**The artifacts come from a build that ran third-party code** (the new proc-macros, build scripts,",
        "`wasm-pack` and `wasm-bindgen-cli`). The workflow checked the file paths and the lock shape. Those checks",
        "do not make the content of the five files safe.",
        "",
        "Reviewer checklist:",
        "",
        "- [ ] Read the `rs/Cargo.lock` diff. Every changed entry is a family package from crates.io.",
        "- [ ] Confirm that `CI` is green, `committed-wasm.test.ts` included.",
        "- [ ] Expect a binary diff in `paigasus_wasm_bg.wasm`: a Linux build makes different bytes than a macOS build (SMA-634 F12).",
        "- [ ] Do not push to this branch. The next run refuses a branch that a person changed.",
        "",
        f"A refusal of a later run points to the manual runbook in {RUNBOOK}.",
    ]
    return "\n".join(lines) + "\n"


def status_verdict(text: str) -> None:
    """`git status --porcelain --untracked-files=all` after the copy: rs/Cargo.lock modified, a
    subset of the five artifact paths modified, and nothing else."""
    paths = []
    for line in text.splitlines():
        if not line:
            continue
        if len(line) < 4 or line[:3] != " M ":
            raise RefusalError("R-STATUS", f"git status shows {line!r}; only an unstaged modification of an allowed file is expected")
        paths.append(line[3:])
    if LOCK_PATH not in paths:
        raise RefusalError("R-STATUS", f"git status does not show {LOCK_PATH} as modified")
    for path in paths:
        if path not in ALLOWED_FILES:
            raise RefusalError("R-STATUS", f"git status shows {path!r}, which is not one of the six allowed files")


def run_artifact(directory: str, old_path: str, body_file: str | None, title_file: str | None) -> list[tuple[str, str, str]]:
    artifact_tree(directory)
    try:
        changes = lock_verdict(load_lock(old_path), load_lock(os.path.join(directory, LOCK_PATH)))
    except NoChangeError as exc:
        raise RefusalError("R-NOCHANGE", "the artifact lock equals the checked-out lock, so there is nothing to propose") from exc
    title = title_for(changes)
    body = body_for(changes)
    # Written ONLY here, after every version string passed check_semver and every refusal had
    # its chance. A refusal above leaves both files absent.
    if title_file:
        write_file(title_file, title + "\n")
    if body_file:
        write_file(body_file, body)
    return changes
```

- [ ] **Step 4: Run the self-test, the control and ruff, and see them pass**

```bash
PY="$(uv run --locked --project ci/workflow-credentials --python '>=3.12' python3 -c 'import sys; print(sys.executable)')"
git show HEAD:rs/Cargo.lock > "$SCRATCH/head.lock"
"$PY" ci/wasm-lockstep/lockstep_check.py --self-test; echo "rc=$?"
"$PY" ci/wasm-lockstep/lockstep_check.py --negative-control --lock "$SCRATCH/head.lock"; echo "rc=$?"
"$PY" ci/wasm-lockstep/lockstep_check.py current --lock "$SCRATCH/head.lock"; echo "rc=$?"
"$PY" ci/wasm-lockstep/lockstep_check.py lock --old "$SCRATCH/head.lock" --new "$SCRATCH/head.lock"; echo "rc=$?"
uv run --locked --project py ruff check --config py/pyproject.toml ci/wasm-lockstep/lockstep_check.py; echo "ruff=$?"
```

Expected:
- `lockstep_check self-test: 44 rows, 0 failed`, `rc=0`.
- Three `ok` lines (`NOCHANGE`, `R-NONFAMILY`, `PASS`), then `lockstep_check negative control: wasm-bindgen 0.2.128 on HEAD, 0 failed`, `rc=0`.
- Seven `family-current …` lines (`wasm-bindgen 0.2.128`), `rc=0`.
- The same seven lines and `rc=4`.
- `All checks passed!`, `ruff=0`.

- [ ] **Step 5: Prove three checks by mutation (spec T4, part 1)**

Save this tool as `$SCRATCH/t4_mutations.py`. It is not committed. It mutates a COPY under `$SCRATCH`, so the tree stays clean and no restore can discard work.

```python
# SMA-693 T4 — the mutation proof. NOT committed: run it from the worktree root with the venv python.
#   python t4_mutations.py <python> <out-dir> lockstep_check.py|pin_check.py
# Each mutation deletes ONE check by replacing its line with an always-false form that still
# compiles. The mutated COPY runs its own --self-test. The proof holds only when the copy exits 3
# (it RAN and a row failed; a SyntaxError would exit 1) and the named row is among the failures.
import os
import subprocess
import sys

PY, OUT, ONLY = sys.argv[1], sys.argv[2], sys.argv[3]
MUTATIONS = (
    ("lockstep_check.py", '        if added_entry.get("source") != CRATES_IO:\n', '        if False:\n',
     "a git source on a family entry"),
    ("lockstep_check.py", "    if not SEMVER.fullmatch(version):\n", "    if False:\n",
     "a shell payload in a version string"),
    ("lockstep_check.py", "                if rel not in ALLOWED_FILES:\n", "                if False:\n",
     "a sixth artifact file"),
    ("pin_check.py",
     '    out += [f"P9 {w} reads the needs context outside an if:" for w in _reads(doc, NEEDS_CTX, skip_if=True)]\n',
     "    out += []\n", "needs.build.outputs.x in a run in propose"),
    ("pin_check.py", "    if set(jobs) != EXPECTED_JOBS:\n", "    if False:\n",
     "a third job with environment release-pr"),
)
os.makedirs(OUT, exist_ok=True)
bad = 0
for name, old, new, row in MUTATIONS:
    if name != ONLY:
        continue
    source = open(os.path.join("ci", "wasm-lockstep", name), encoding="utf-8").read()
    count = source.count(old)
    target = os.path.join(OUT, name)
    open(target, "w", encoding="utf-8").write(source.replace(old, new))
    run = subprocess.run([PY, target, "--self-test"], capture_output=True, text=True, check=False)
    red = f"FAIL  {row}:" in run.stderr
    ok = count == 1 and run.returncode == 3 and red
    print(f"{'PROVED' if ok else 'NOT PROVED'}  {name}: {old.strip()!r} -> rc {run.returncode}, row {row!r} red={red}, line count {count}")
    bad += 0 if ok else 1
print(f"t4: {bad} mutation(s) not proved")
sys.exit(1 if bad else 0)
```

```bash
PY="$(uv run --locked --project ci/workflow-credentials --python '>=3.12' python3 -c 'import sys; print(sys.executable)')"
"$PY" "$SCRATCH/t4_mutations.py" "$PY" "$SCRATCH/t4" lockstep_check.py; echo "rc=$?"
git status --short ci/
```

Expected: three `PROVED` lines (the source check, the semver check, the extra-file check), each with `rc 3`, `red=True`, `line count 1`; `t4: 0 mutation(s) not proved`; `rc=0`. `git status` shows only `?? ci/wasm-lockstep/`. An `rc 1` in a line means the mutated copy did not run (a syntax error), which proves nothing.

- [ ] **Step 6: Commit**

```bash
git add ci/wasm-lockstep/lockstep_check.py
git commit -m "feat(ci): add the wasm-lockstep lock and artifact checker" -m "The stdlib checker that the wasm-lockstep workflow runs twice, in build as an early exit
and in propose on the downloaded bytes. Each refusal carries a code, and the self-test
asserts the code of each row." -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: `pin_check.py`, its uv project, and the supply-chain watch of its lock

**Files:**
- Create: `ci/wasm-lockstep/pyproject.toml`
- Create: `ci/wasm-lockstep/uv.lock` (generated by `uv lock`)
- Create: `ci/wasm-lockstep/pin_check.py`
- Modify: `ci/osv/run.sh:33-40` (LOCKFILES)
- Modify: `moon.yml:50-52` (the `repo:osv` inputs)
- Modify: `.github/dependabot.yml:102-123` (a fourth `uv` entry after `/ci/helm-render`)

**Interfaces:**
- Consumes: nothing from Task 1.
- Produces:
  - CLI `uv run --locked --project ci/wasm-lockstep --python '>=3.12' python3 ci/wasm-lockstep/pin_check.py <workflow.yml>` -> exit 0 (`pin_check: <file> satisfies P0-P17`) or 3 (one `pin_check: P<n> …` line per violation on stderr) or 2.
  - `… pin_check.py --self-test` -> 0 or 3. `… pin_check.py --negative-control <workflow.yml>` -> 0 or 3.
  - Python: `violations(doc: dict) -> list[str]` (each string starts with `P<n> `), `commands(script: str) -> tuple[list[list[str]], list[str]]`.
  - The venv interpreter `ci/wasm-lockstep/.venv/bin/python3` (Python 3.12, PyYAML 6.0.3).

- [ ] **Step 1: Write the uv project, lock it, and write the file with the fixture table and a stub rule set**

Create `ci/wasm-lockstep/pyproject.toml`:

```toml
# SPDX-License-Identifier: Apache-2.0
# A DEDICATED one-dependency project, the layout of ci/workflow-credentials/ and ci/helm-render/.
# Only pin_check.py needs it (PyYAML). lockstep_check.py stays stdlib-only, because the propose job
# of .github/workflows/wasm-lockstep.yml runs it with the runner's own python3. A separate project,
# not the workflow-credentials one, so the two gates do not share a lock (SMA-693 spec 5.4).
[project]
name = "paigasus-wasm-lockstep"
version = "0.0.0"
description = "The pin check of the wasm-lockstep workflow (repo:wasm-lockstep)"
requires-python = ">=3.12"
dependencies = ["pyyaml>=6.0.3,<7"]
```

```bash
uv lock --project ci/wasm-lockstep
sed -n 1,12p ci/wasm-lockstep/uv.lock
```

Expected: `Resolved 2 packages`; the lock names `pyyaml` 6.0.3.

Create `ci/wasm-lockstep/pin_check.py` with exactly this content. The section `# ---- the rules ----` holds a stub that Step 3 replaces.

```python
# SPDX-License-Identifier: Apache-2.0
"""The pin check of .github/workflows/wasm-lockstep.yml (SMA-693 spec 5.4).

A PyYAML parse, never a text scan: SMA-593 measured fourteen bypasses of a text scan, a YAML
alias among them. The rules (P0-P17) are the trust model of spec 5.1 and 5.2 in checkable form.
ci/wasm-lockstep/README.md lists each rule and what it does not prove.

  pin_check.py <workflow.yml>                       the rules on one workflow
  pin_check.py --self-test                          the in-process fixture table (T3)
  pin_check.py --negative-control <workflow.yml>    mutations of the real workflow must red

Exit codes: 0 pass | 3 a rule failed | 2 infrastructure. Not 1: `uv` exits 1 on a failed
resolution. ci/wasm-lockstep/run.sh maps 3 -> 1 and every other non-zero code -> 2.

The five PyYAML coercions of ci/CLAUDE.md are handled: a bare `on:` key is the boolean True;
`if: false` and `continue-on-error: false` are booleans; a quoted "false" is a string; `needs:`
can be a scalar. Aliases resolve in the parse, so an alias that injects a `secrets` read is seen.
A merge key (`<<:`) is refused: GitHub Actions does not merge it, so a merged parse would check a
workflow that GitHub does not run.
"""

from __future__ import annotations

import copy
import re
import shlex
import sys

import yaml

RC_OK = 0
RC_INFRA = 2
RC_ASSERT = 3


class InfraError(Exception):
    """The check could not run. Maps to RC_INFRA."""


class AssertionFailureError(Exception):
    """The workflow is wrong. Maps to RC_ASSERT."""


# The expression grammar of ci/workflow-credentials/workflow_credentials.py, copied (the two gates
# have separate uv projects on purpose). See that file for the measured reasons of each part.
EXPR_SPAN = re.compile(r"\$\{\{((?:'[^']*'|\"[^\"]*\"|(?!\}\}).)*+)\}\}", re.S)
STRING_LITERAL = re.compile(r"'[^']*'|\"[^\"]*\"")
SECRETS_CTX = re.compile(r"(?<![\w.-])secrets(?![\w-])", re.IGNORECASE)
NEEDS_CTX = re.compile(r"(?<![\w.-])needs(?![\w-])", re.IGNORECASE)
STATUS_FN = re.compile(r"(?<![\w.-])(always|failure|cancelled)\s*\(", re.IGNORECASE)
SHA_PIN = re.compile(r"[0-9a-f]{40}")
ASSIGNMENT = re.compile(r"[A-Za-z_][A-Za-z0-9_]*=.*", re.S)

EXPECTED_JOBS = frozenset({"build", "propose"})
EXPECTED_TRIGGERS = frozenset({"schedule", "workflow_dispatch"})
PERMISSIONS = {"contents": "read"}
ALLOWED_ACTIONS = {
    "build": frozenset({"actions/checkout", "actions/upload-artifact"}),
    "propose": frozenset({"actions/checkout", "actions/download-artifact", "actions/create-github-app-token"}),
}
PROPOSE_ORDER = ("checkout", "download", "verify", "apply", "token", "commit", "base", "push", "pr", "close")
BUILD_OUTPUTS = {"changed": "${{ steps.lock.outputs.changed }}"}
TOKEN_WITH = {
    "client-id": "${{ secrets.PAIGASUS_BOT_APP_ID }}",
    "private-key": "${{ secrets.PAIGASUS_BOT_PRIVATE_KEY }}",
    "permission-contents": "write",
    "permission-pull-requests": "write",
}
REFSPEC = "HEAD:refs/heads/deps/wasm-bindgen-lockstep"
LEASE_PREFIX = "--force-with-lease=refs/heads/deps/wasm-bindgen-lockstep:"
PUSH_REMOTE = "https://x-access-token:${PUSH_TOKEN}@github.com/${GITHUB_REPOSITORY}.git"
CHECKER = "ci/wasm-lockstep/lockstep_check.py"
CONTAINER_SCRIPT = "ci/wasm-lockstep/container.sh"
IMAGE_TOKEN = "$LOCKSTEP_IMAGE"
IMAGE_PIN = re.compile(r"docker\.io/library/rust:[0-9][0-9.]*-bookworm@sha256:[0-9a-f]{64}")
DOCKER_FLAGS = frozenset({"--rm"})
DOCKER_VALUES = {"--user": "65534:65534", "--volume": "$RUNNER_TEMP/work:/work", "--workdir": "/work"}

# Command words. A word is the first word of a simple command, after a separator, a keyword
# that starts a command, or `sudo`. `case` is NOT allowed: its patterns read as command words.
COMMON_WORDS = frozenset({"if", "then", "else", "elif", "fi", "for", "do", "done", "while", "until",
                          "!", "{", "}", "exit", "true", "set", "read", "[", "test", "echo", "printf", "cat", "sed"})
JOB_WORDS = {
    "build": COMMON_WORDS | {"df", "sudo", "rm", "git", "tar", "mkdir", "cp", "chmod", "python3", "docker"},
    "propose": COMMON_WORDS | {"python3", "git", "gh", "cp", "grep"},
}
SUDO_WORDS = frozenset({"rm", "docker"})
STARTS_COMMAND = frozenset({"if", "then", "else", "elif", "do", "while", "until", "!", "{"})
SEPARATOR_CHARS = frozenset(";&|()")
# set -e ignores the status of a command negated with `!`, so `! test -L f` never stops a step.
# A `!` is allowed only where its status is tested: right after if, elif, while or until.
NEGATION_OK = frozenset({"if", "elif", "while", "until"})
SUBST = "__SUBST__"


class _StrictLoader(yaml.SafeLoader):
    """SafeLoader that refuses duplicate keys (PyYAML's default is last-wins) and merge keys."""


def _construct_mapping(loader, node, deep=False):
    for key_node, _ in node.value:
        if key_node.tag == "tag:yaml.org,2002:merge":
            raise yaml.constructor.ConstructorError(
                "while constructing a mapping", node.start_mark,
                "a merge key (<<:), which GitHub Actions does not merge", key_node.start_mark)
    seen = set()
    for key_node, _ in node.value:
        key = loader.construct_object(key_node, deep=deep)
        if key in seen:
            raise yaml.constructor.ConstructorError(
                "while constructing a mapping", node.start_mark, f"duplicate key {key!r}", key_node.start_mark)
        seen.add(key)
    return yaml.SafeLoader.construct_mapping(loader, node, deep=deep)


_StrictLoader.add_constructor(yaml.resolver.BaseResolver.DEFAULT_MAPPING_TAG, _construct_mapping)


def parse_text(text: str, label: str) -> dict:
    try:
        docs = list(yaml.load_all(text, Loader=_StrictLoader))
    except yaml.YAMLError as exc:
        raise AssertionFailureError(f"P0 {label} is not valid YAML for this check: {exc}") from exc
    if len(docs) != 1 or not isinstance(docs[0], dict):
        raise AssertionFailureError(f"P0 {label} must hold exactly one YAML mapping")
    return docs[0]


def load(path: str) -> dict:
    try:
        with open(path, encoding="utf-8") as handle:
            text = handle.read()
    except (OSError, UnicodeDecodeError) as exc:
        raise InfraError(f"cannot read {path}: {exc}") from exc
    return parse_text(text, path)


def _strings(node, path="$"):
    """(path, last key, string) for every string key and value."""
    if isinstance(node, dict):
        for key, value in node.items():
            here = f"{path}.{key}"
            if isinstance(key, str):
                yield here, None, key
            if isinstance(value, str):
                yield here, key, value
            else:
                yield from _strings(value, here)
    elif isinstance(node, list):
        for index, value in enumerate(node):
            if isinstance(value, str):
                yield f"{path}[{index}]", None, value
            else:
                yield from _strings(value, f"{path}[{index}]")


def _reads(node, context: re.Pattern, path: str = "$", skip_if: bool = False) -> list[str]:
    """Paths whose string reads `context` in an expression span, or in a bare `if:` value."""
    out = []
    for where, key, text in _strings(node, path):
        if skip_if and key == "if":
            continue
        spans = [STRING_LITERAL.sub("", s) for s in EXPR_SPAN.findall(text)]
        if key == "if":
            spans.append(STRING_LITERAL.sub("", EXPR_SPAN.sub("", text)))
        if any(context.search(span) for span in spans):
            out.append(where)
    return out


def _needs_list(value) -> list:
    # PyYAML coercion: `needs: build` is a scalar, and iterating a string yields its characters.
    if isinstance(value, str):
        return [value]
    return list(value) if isinstance(value, list) else [value]


def triggers(doc: dict) -> set[str]:
    """`on:` parses as the boolean True. Read both keys and UNION them (SMA-593 F3)."""
    out: set[str] = set()
    for key in ("on", True):
        if key not in doc:
            continue
        value = doc[key]
        if isinstance(value, str):
            out.add(value)
        elif isinstance(value, (list, dict)):
            out |= {str(x) for x in value}
        else:
            out.add(repr(value))
    return out


# ---- the shell reader -----------------------------------------------------------------------

def _substitutions(text: str) -> tuple[str, list[str], list[str]]:
    """Replace each `$(...)` with a placeholder and return (outer, inner texts, problems). A
    backtick or a here-document is a problem: this reader does not follow them, so it refuses."""
    out, inner, problems = [], [], []
    i = 0
    while i < len(text):
        if text.startswith("$((", i):
            problems.append("an arithmetic expansion $((...)); write the value another way")
            i += 3
            continue
        if text.startswith("$(", i):
            depth, j = 1, i + 2
            while j < len(text) and depth:
                depth += {"(": 1, ")": -1}.get(text[j], 0)
                j += 1
            if depth:
                problems.append("an unbalanced $(")
                return "".join(out), inner, problems
            inner.append(text[i + 2:j - 1])
            out.append(SUBST)
            i = j
            continue
        if text[i] == "`":
            problems.append("a backtick command substitution")
        if text.startswith("<<", i):
            problems.append("a here-document")
        out.append(text[i])
        i += 1
    return "".join(out), inner, problems


def commands(script: str) -> tuple[list[list[str]], list[str]]:
    """Every simple command of a `run:` script as [word, args...], and the problems found."""
    outer, inner, problems = _substitutions(script.replace("\\\n", " "))
    found: list[list[str]] = []
    for text in inner:
        sub, sub_problems = commands(text)
        found += sub
        problems += sub_problems
    for line in outer.splitlines():
        lexer = shlex.shlex(line, posix=True, punctuation_chars=True)
        lexer.whitespace_split = True
        try:
            tokens = list(lexer)
        except ValueError as exc:
            problems.append(f"a line the shell reader cannot split ({exc}): {line.strip()!r}")
            continue
        expect, skip, keyword = True, False, None
        for token in tokens:
            if skip:
                skip = False
                continue
            if token and set(token) <= SEPARATOR_CHARS | {"<", ">"}:
                if "<" in token or ">" in token:
                    skip = True
                else:
                    expect, keyword = True, None
                continue
            if expect:
                if ASSIGNMENT.fullmatch(token):
                    continue
                if token == "!" and keyword not in NEGATION_OK:
                    problems.append(f"a bare `!` negation, which set -e ignores; write `if ...; then exit 1; fi`: {line.strip()!r}")
                found.append([token])
                expect = token in STARTS_COMMAND
                keyword = token if expect else None
                continue
            found[-1].append(token)
    return found, problems


# ---- the rules ---------------------------------------------------------------------------------

def violations(doc: dict) -> list[str]:
    return []


# ---- the self-test (T3) -----------------------------------------------------------------------

FIXTURE = """\
name: wasm-lockstep
on:
  schedule:
    - cron: '17 6 * * 2'
  workflow_dispatch:
permissions:
  contents: read
env:
  LOCKSTEP_IMAGE: docker.io/library/rust:1.95.0-bookworm@sha256:""" + "a" * 64 + """
jobs:
  build:
    runs-on: ubuntu-latest
    permissions:
      contents: read
    outputs:
      changed: ${{ steps.lock.outputs.changed }}
    steps:
      - id: checkout
        uses: actions/checkout@""" + "1" * 40 + """
        with:
          persist-credentials: false
      - id: update
        run: |
          docker run --rm --user 65534:65534 --volume "$RUNNER_TEMP/work:/work" --workdir /work "$LOCKSTEP_IMAGE" bash ci/wasm-lockstep/container.sh update
      - id: lock
        run: |
          rc=0
          python3 ci/wasm-lockstep/lockstep_check.py lock --old a --new b > "$RUNNER_TEMP/v.txt" || rc=$?
          if [ "$rc" -eq 0 ]; then
            echo "changed=true" >> "$GITHUB_OUTPUT"
          fi
      - id: upload
        if: steps.lock.outputs.changed == 'true'
        uses: actions/upload-artifact@""" + "2" * 40 + """
  propose:
    needs: build
    if: needs.build.result == 'success'
    runs-on: ubuntu-latest
    environment: release-pr
    permissions:
      contents: read
    steps:
      - id: checkout
        if: needs.build.outputs.changed == 'true'
        uses: actions/checkout@""" + "1" * 40 + """
        with:
          persist-credentials: false
      - id: download
        if: needs.build.outputs.changed == 'true'
        uses: actions/download-artifact@""" + "3" * 40 + """
      - id: verify
        if: needs.build.outputs.changed == 'true'
        run: python3 ci/wasm-lockstep/lockstep_check.py artifact --dir d --old rs/Cargo.lock
      - id: apply
        if: needs.build.outputs.changed == 'true'
        run: cp a b
      - id: token
        uses: actions/create-github-app-token@""" + "4" * 40 + """
        with:
          client-id: ${{ secrets.PAIGASUS_BOT_APP_ID }}
          private-key: ${{ secrets.PAIGASUS_BOT_PRIVATE_KEY }}
          permission-contents: write
          permission-pull-requests: write
      - id: commit
        if: needs.build.outputs.changed == 'true'
        run: git commit -F "$RUNNER_TEMP/pr-title.txt"
      - id: base
        if: needs.build.outputs.changed == 'true'
        env:
          GH_TOKEN: ${{ github.token }}
        run: |
          now="$(gh api "repos/${GITHUB_REPOSITORY}/git/ref/heads/main" --jq .object.sha)"
          echo "moved=false" >> "$GITHUB_OUTPUT"
      - id: push
        if: needs.build.outputs.changed == 'true' && steps.base.outputs.moved == 'false'
        env:
          PUSH_TOKEN: ${{ steps.token.outputs.token }}
        run: |
          git push --force-with-lease="refs/heads/deps/wasm-bindgen-lockstep:${lease}" "https://x-access-token:${PUSH_TOKEN}@github.com/${GITHUB_REPOSITORY}.git" HEAD:refs/heads/deps/wasm-bindgen-lockstep
      - id: pr
        if: needs.build.outputs.changed == 'true' && steps.base.outputs.moved == 'false'
        run: gh pr create --title "$(cat "$RUNNER_TEMP/pr-title.txt")" --body "run cargo update"
      - id: close
        if: needs.build.outputs.changed == 'false'
        run: echo "cargo update is not needed"
"""

PUSH_LINE = 'git push --force-with-lease="refs/heads/deps/wasm-bindgen-lockstep:${lease}" "https://x-access-token:${PUSH_TOKEN}@github.com/${GITHUB_REPOSITORY}.git" HEAD:refs/heads/deps/wasm-bindgen-lockstep'
DOCKER_LINE = 'docker run --rm --user 65534:65534 --volume "$RUNNER_TEMP/work:/work" --workdir /work "$LOCKSTEP_IMAGE"'
TOKEN_STEP = "      - id: token\n        uses: actions/create-github-app-token@" + "4" * 40 + """
        with:
          client-id: ${{ secrets.PAIGASUS_BOT_APP_ID }}
          private-key: ${{ secrets.PAIGASUS_BOT_PRIVATE_KEY }}
          permission-contents: write
          permission-pull-requests: write
"""
VERIFY_STEP = """      - id: verify
        if: needs.build.outputs.changed == 'true'
        run: python3 ci/wasm-lockstep/lockstep_check.py artifact --dir d --old rs/Cargo.lock
"""
BUILD_STEP_ANCHOR = "      - id: update\n"
PROPOSE_RUN = "        run: cp a b\n"
BUILD_RUN = "          rc=0\n"

# (label, (old, new) replacements on FIXTURE, rule id that must appear or "PASS")
SELF_TEST_ROWS: tuple[tuple[str, tuple[tuple[str, str], ...], str], ...] = (
    ("the fixture passes, its PR body text says cargo update", (), "PASS"),
    ("needs as a list", (("    needs: build\n", "    needs: [build]\n"),), "PASS"),
    ("a boolean if: false on a step", (("      - id: close\n        if: needs.build.outputs.changed == 'false'\n", "      - id: close\n        if: false\n"),), "PASS"),
    ("secrets.X read in build", ((BUILD_STEP_ANCHOR, BUILD_STEP_ANCHOR + "        env:\n          X: ${{ secrets.X }}\n"),), "P3"),
    ("secrets['X'] read in build", ((BUILD_STEP_ANCHOR, BUILD_STEP_ANCHOR + "        env:\n          X: ${{ secrets['X'] }}\n"),), "P3"),
    ("toJSON(secrets) read in build", ((BUILD_STEP_ANCHOR, BUILD_STEP_ANCHOR + "        env:\n          X: ${{ toJSON(secrets) }}\n"),), "P3"),
    ("Secrets.X in another case in build", ((BUILD_STEP_ANCHOR, BUILD_STEP_ANCHOR + "        env:\n          X: ${{ Secrets.X }}\n"),), "P3"),
    ("a bare if: secrets.X in build", ((BUILD_STEP_ANCHOR, BUILD_STEP_ANCHOR + "        if: secrets.X != ''\n"),), "P3"),
    ("a workflow-level env reads a secret", (("env:\n", "env:\n  K: ${{ secrets.X }}\n"),), "P3"),
    ("a YAML alias injects a secrets read into build", (
        ("permissions:\n  contents: read\nenv:\n", "permissions:\n  contents: read\nx-leak: &leak ${{ secrets.X }}\nenv:\n"),
        (BUILD_STEP_ANCHOR, BUILD_STEP_ANCHOR + "        env:\n          X: *leak\n")), "P3"),
    ("a secrets read in propose outside the token step", ((PROPOSE_RUN, PROPOSE_RUN + "        env:\n          K: ${{ secrets.PAIGASUS_BOT_PRIVATE_KEY }}\n"),), "P3"),
    ("an environment on build", (("  build:\n    runs-on: ubuntu-latest\n", "  build:\n    runs-on: ubuntu-latest\n    environment: release-pr\n"),), "P3"),
    ("a third job with environment release-pr", (("jobs:\n", "jobs:\n  third:\n    runs-on: x\n    environment: release-pr\n    permissions:\n      contents: read\n    steps:\n      - run: echo\n"),), "P1"),
    ("a job-level uses with secrets: inherit in build", (("  build:\n    runs-on: ubuntu-latest\n", "  build:\n    uses: ./.github/workflows/x.yml\n    secrets: inherit\n    runs-on: ubuntu-latest\n"),), "P12"),
    ("a container: key on build", (("  build:\n    runs-on: ubuntu-latest\n", "  build:\n    container: ubuntu:24.04\n    runs-on: ubuntu-latest\n"),), "P12"),
    ("docker run -e", ((DOCKER_LINE, DOCKER_LINE.replace("--rm", "--rm -e TOKEN")),), "P4"),
    ("docker run --env=X", ((DOCKER_LINE, DOCKER_LINE.replace("--rm", "--rm --env=X")),), "P4"),
    ("docker run --env-file", ((DOCKER_LINE, DOCKER_LINE.replace("--rm", "--rm --env-file f")),), "P4"),
    ("docker run --privileged", ((DOCKER_LINE, DOCKER_LINE.replace("--rm", "--rm --privileged")),), "P4"),
    ("docker run with the docker socket", ((DOCKER_LINE, DOCKER_LINE.replace("--rm", "--rm -v /var/run/docker.sock:/var/run/docker.sock")),), "P4"),
    ("docker run --network=host", ((DOCKER_LINE, DOCKER_LINE.replace("--rm", "--rm --network=host")),), "P4"),
    ("docker run --pid=host", ((DOCKER_LINE, DOCKER_LINE.replace("--rm", "--rm --pid=host")),), "P4"),
    ("docker run --cap-add", ((DOCKER_LINE, DOCKER_LINE.replace("--rm", "--rm --cap-add SYS_PTRACE")),), "P4"),
    ("docker run without --user", ((DOCKER_LINE, DOCKER_LINE.replace(" --user 65534:65534", "")),), "P4"),
    ("docker run as root", ((DOCKER_LINE, DOCKER_LINE.replace("65534:65534", "0:0")),), "P4"),
    ("an image tag without a digest", (("@sha256:" + "a" * 64, ""),), "P4"),
    ("cargo on the build host", ((BUILD_RUN, BUILD_RUN + "          ( cd rs && cargo build )\n"),), "P5"),
    ("a bare ! test -L on the build host", ((BUILD_RUN, BUILD_RUN + '          ! test -L "$RUNNER_TEMP/work/rs"\n'),), "P5"),
    ("a work-copy script on the build host", ((BUILD_RUN, BUILD_RUN + '          bash "$RUNNER_TEMP/work/x.sh"\n'),), "P5"),
    ("the token step before the checker step", ((VERIFY_STEP + "      - id: apply\n        if: needs.build.outputs.changed == 'true'\n        run: cp a b\n" + TOKEN_STEP,
                                                  TOKEN_STEP + VERIFY_STEP + "      - id: apply\n        if: needs.build.outputs.changed == 'true'\n        run: cp a b\n"),), "P7"),
    ("a cargo command in propose", ((PROPOSE_RUN, "        run: cargo update -p wasm-bindgen\n"),), "P5"),
    ("cargo inside $(...) in propose", ((PROPOSE_RUN, '        run: echo "$(cargo metadata)"\n'),), "P5"),
    ("a backtick in propose", ((PROPOSE_RUN, "        run: echo `id`\n"),), "P5"),
    ("bash -c in propose", ((PROPOSE_RUN, "        run: bash -c 'cargo build'\n"),), "P5"),
    ("python3 -c in propose", ((PROPOSE_RUN, "        run: python3 -c 'print(1)'\n"),), "P5"),
    ("env as a command wrapper in propose", ((PROPOSE_RUN, "        run: env cargo build\n"),), "P5"),
    ("cargo after a pipe in propose", ((PROPOSE_RUN, "        run: cat a | cargo build\n"),), "P5"),
    ("a push refspec to refs/heads/main", ((PUSH_LINE, PUSH_LINE.replace("HEAD:refs/heads/deps/wasm-bindgen-lockstep", "HEAD:refs/heads/main")),), "P8"),
    ("a push with --force", ((PUSH_LINE, PUSH_LINE.replace('--force-with-lease="refs/heads/deps/wasm-bindgen-lockstep:${lease}"', "--force")),), "P8"),
    ("a second git push", ((PROPOSE_RUN, "        run: git push origin HEAD:refs/heads/x\n"),), "P8"),
    ("needs.build.outputs.x in a run in propose", ((PROPOSE_RUN, '        run: echo "${{ needs.build.outputs.changed }}"\n'),), "P9"),
    ("needs['build'] in an env in propose", ((PROPOSE_RUN, PROPOSE_RUN + "        env:\n          C: ${{ needs['build'].outputs.changed }}\n"),), "P9"),
    ("NEEDS in another case in a with: in propose", (("          persist-credentials: false\n      - id: download", "          persist-credentials: false\n          ref: ${{ NEEDS.build.outputs.changed }}\n      - id: download"),), "P9"),
    ("a second build output", (("      changed: ${{ steps.lock.outputs.changed }}\n", "      changed: ${{ steps.lock.outputs.changed }}\n      version: x\n"),), "P9"),
    ("the bare on: key replaced by a null value", (("on:\n  schedule:\n    - cron: '17 6 * * 2'\n  workflow_dispatch:\n", "on:\n"),), "P2"),
    ("a quoted on key adds push next to the bare one", (("permissions:\n  contents: read\nenv:", "'on': push\npermissions:\n  contents: read\nenv:"),), "P2"),
    ("a pull_request trigger", (("  workflow_dispatch:\n", "  workflow_dispatch:\n  pull_request:\n"),), "P2"),
    ("a job widens permissions", (("    environment: release-pr\n    permissions:\n      contents: read\n", "    environment: release-pr\n    permissions:\n      contents: write\n"),), "P10"),
    ("top-level write-all", (("permissions:\n  contents: read\nenv:", "permissions: write-all\nenv:"),), "P10"),
    ("propose without the release-pr environment", (("    environment: release-pr\n", ""),), "P11"),
    ("continue-on-error: true on verify", ((VERIFY_STEP, VERIFY_STEP + "        continue-on-error: true\n"),), "P14"),
    ('continue-on-error: "true" (a string) on verify', ((VERIFY_STEP, VERIFY_STEP + '        continue-on-error: "true"\n'),), "P14"),
    ("if: always() on the push step", (("      - id: push\n        if: needs.build.outputs.changed == 'true' && steps.base.outputs.moved == 'false'\n", "      - id: push\n        if: always()\n"),), "P15"),
    ("shell: python on a propose step", ((PROPOSE_RUN, PROPOSE_RUN + "        shell: python\n"),), "P13"),
    ("persist-credentials: true on a checkout", (("        with:\n          persist-credentials: false\n      - id: download", "        with:\n          persist-credentials: true\n      - id: download"),), "P17"),
    ("the token asks for workflows: write", (("          permission-pull-requests: write\n", "          permission-pull-requests: write\n          permission-workflows: write\n"),), "P16"),
    ("an unlisted action in propose", (("actions/download-artifact@" + "3" * 40, "evil/action@" + "3" * 40),), "P6"),
    ("an action pinned to a tag", (("actions/download-artifact@" + "3" * 40, "actions/download-artifact@v8"),), "P6"),
    ("a merge key", (("    outputs:\n", "    <<: {timeout-minutes: 5}\n    outputs:\n"),), "P0"),
    ("a duplicate key", (("    environment: release-pr\n", "    environment: release-pr\n    environment: other\n"),), "P0"),
)


def _rules_of(text: str) -> list[str]:
    try:
        return violations(parse_text(text, "fixture"))
    except AssertionFailureError as exc:
        return [str(exc)]


def self_test() -> int:
    failures = 0
    for label, replacements, want in SELF_TEST_ROWS:
        text = FIXTURE
        drift = [old for old, _new in replacements if old not in text]
        for old, new in replacements:
            text = text.replace(old, new, 1)
        if drift:
            print(f"  FAIL  {label}: the fixture no longer holds {drift[0]!r}, so the row tests nothing", file=sys.stderr)
            failures += 1
            continue
        got = _rules_of(text)
        ok = not got if want == "PASS" else any(v.startswith(want + " ") for v in got)
        if ok:
            print(f"  ok    {label}: {want}")
        else:
            print(f"  FAIL  {label}: want {want}, got {got}", file=sys.stderr)
            failures += 1
    print(f"pin_check self-test: {len(SELF_TEST_ROWS)} rows, {failures} failed")
    return RC_ASSERT if failures else RC_OK


def negative_control(path: str) -> int:
    """Mutations of the REAL workflow. The real one must pass, and each mutation must red."""
    real = load(path)
    failures = 0
    base = violations(real)
    if base:
        print(f"  FAIL  the real workflow does not pass, so no mutation can prove anything: {base}", file=sys.stderr)
        return RC_ASSERT

    def expect(label: str, mutated: dict, want: str) -> None:
        nonlocal failures
        got = violations(mutated)
        if any(v.startswith(want + " ") for v in got):
            print(f"  ok    {label}: {want}")
        else:
            print(f"  FAIL  {label}: want {want}, got {got}", file=sys.stderr)
            failures += 1

    leak = copy.deepcopy(real)
    leak["jobs"]["build"]["steps"][0].setdefault("env", {})["LEAK"] = "${{ secrets.PAIGASUS_BOT_PRIVATE_KEY }}"
    expect("a secrets read added to build", leak, "P3")
    order = copy.deepcopy(real)
    steps = order["jobs"]["propose"]["steps"]
    ids = [s.get("id") for s in steps]
    v, t = ids.index("verify"), ids.index("token")
    steps[v], steps[t] = steps[t], steps[v]
    expect("the token step moved before the verify step", order, "P7")
    target = copy.deepcopy(real)
    for step in target["jobs"]["propose"]["steps"]:
        if step.get("id") == "push":
            step["run"] = step["run"].replace(REFSPEC, "HEAD:refs/heads/main")
    expect("the push refspec moved to main", target, "P8")
    output = copy.deepcopy(real)
    for step in output["jobs"]["propose"]["steps"]:
        if step.get("id") == "apply":
            step.setdefault("env", {})["C"] = "${{ needs.build.outputs.changed }}"
    expect("needs.build.outputs read in an env of propose", output, "P9")
    print(f"pin_check negative control: 4 mutations, {failures} failed")
    return RC_ASSERT if failures else RC_OK


def main(argv: list[str]) -> int:
    if argv == ["--self-test"]:
        return self_test()
    if len(argv) == 2 and argv[0] == "--negative-control":
        return negative_control(argv[1])
    if len(argv) != 1 or argv[0].startswith("-"):
        raise InfraError("usage: pin_check.py <workflow.yml> | --self-test | --negative-control <workflow.yml>")
    found = violations(load(argv[0]))
    for line in found:
        print(f"pin_check: {line}", file=sys.stderr)
    if found:
        return RC_ASSERT
    print(f"pin_check: {argv[0]} satisfies P0-P17")
    return RC_OK


if __name__ == "__main__":
    try:
        sys.exit(main(sys.argv[1:]))
    except AssertionFailureError as exc:
        print(f"pin_check: {exc}", file=sys.stderr)
        sys.exit(RC_ASSERT)
    except InfraError as exc:
        print(f"pin_check: infrastructure error: {exc}", file=sys.stderr)
        sys.exit(RC_INFRA)
```

- [ ] **Step 2: Run the self-test and see it fail**

```bash
uv run --locked --project ci/wasm-lockstep --python '>=3.12' python3 ci/wasm-lockstep/pin_check.py --self-test; echo "rc=$?"
```

Expected: `pin_check self-test: 60 rows, 55 failed` and `rc=3`. Only the three `PASS` rows and the two `P0` rows (a merge key, a duplicate key, refused by the loader) pass.

- [ ] **Step 3: Replace the stub section with the rules**

Replace everything from the line `# ---- the rules ---…` up to (not including) the line `# ---- the self-test (T3) ---…`. Keep two blank lines before the self-test line. The new section is exactly this:

```python
# ---- the rules ---------------------------------------------------------------------------------

def _docker_run(args: list[str]) -> list[str]:
    out, seen, i = [], set(), 0
    while i < len(args) and args[i] != IMAGE_TOKEN:
        name, eq, value = args[i].partition("=")
        if name in DOCKER_FLAGS and not eq:
            seen.add(name)
            i += 1
            continue
        if name in DOCKER_VALUES:
            if not eq:
                value = args[i + 1] if i + 1 < len(args) else ""
                i += 1
            if value != DOCKER_VALUES[name]:
                out.append(f"docker run {name} {value!r}, expected {DOCKER_VALUES[name]!r}")
            seen.add(name)
            i += 1
            continue
        out.append(f"docker run option {args[i]!r} is not on the allowlist")
        i += 1
    if i >= len(args):
        out.append('docker run has no "$LOCKSTEP_IMAGE" image argument')
        return out
    missing = (DOCKER_FLAGS | set(DOCKER_VALUES)) - seen
    if missing:
        out.append(f"docker run lacks {sorted(missing)}")
    rest = args[i + 1:]
    if len(rest) != 3 or rest[:2] != ["bash", CONTAINER_SCRIPT] or rest[2] not in ("update", "build"):
        out.append(f"docker run must run `bash {CONTAINER_SCRIPT} update|build`, not {rest!r}")
    return out


def command_violations(job: str, script: str, where: str) -> list[str]:
    out = []
    found, problems = commands(script)
    out += [f"P5 {where}: {p}" for p in problems]
    allowed = JOB_WORDS[job]
    for cmd in found:
        word = cmd[0]
        if word not in allowed:
            out.append(f"P5 {where}: the command word {word!r} is not on the {job} allowlist")
        elif word == "python3" and cmd[1:2] != [CHECKER]:
            out.append(f"P5 {where}: python3 may run {CHECKER} only, not {cmd[1:2]!r}")
        elif word == "sudo" and (cmd[1:2] == [] or cmd[1] not in SUDO_WORDS or (cmd[1] == "docker" and cmd[2:3] != ["image"])):
            out.append(f"P5 {where}: sudo may run rm and `docker image` only, not {cmd[1:3]!r}")
        elif word == "docker":
            if cmd[1:2] != ["run"]:
                out.append(f"P4 {where}: docker may run `docker run` only, not {cmd[1:2]!r}")
            else:
                out += [f"P4 {where}: {p}" for p in _docker_run(cmd[2:])]
    return out


def _steps(job: dict, name: str) -> list[dict]:
    steps = job.get("steps")
    if not isinstance(steps, list) or not all(isinstance(s, dict) for s in steps):
        raise AssertionFailureError(f"P0 job {name} has no list of step mappings")
    return steps


def _uses_violation(uses: str, job: str, where: str) -> str | None:
    action, at, ref = uses.partition("@")
    if action not in ALLOWED_ACTIONS[job]:
        return f"P6 {where}: {action!r} is not on the {job} action allowlist"
    if not at or not SHA_PIN.fullmatch(ref):
        return f"P6 {where}: {uses!r} is not pinned to a full commit SHA"
    return None


def step_violations(job: str, steps: list[dict]) -> list[str]:
    out = []
    for index, step in enumerate(steps):
        where = f"jobs.{job}.steps[{index}] ({step.get('id', '?')})"
        if "uses" in step:
            problem = _uses_violation(str(step["uses"]), job, where)
            if problem:
                out.append(problem)
            if str(step["uses"]).startswith("actions/checkout@"):
                with_block = step.get("with") if isinstance(step.get("with"), dict) else {}
                if with_block.get("persist-credentials") not in (False, "false"):
                    out.append(f"P17 {where}: a checkout must set persist-credentials: false")
        if "run" in step:
            out += command_violations(job, str(step["run"]), where)
        if step.get("shell", "bash") != "bash":
            out.append(f"P13 {where}: shell must be bash, not {step.get('shell')!r}")
        if step.get("continue-on-error", False) not in (False, "false"):
            out.append(f"P14 {where}: continue-on-error must be absent or false")
        if STATUS_FN.search(str(step.get("if", ""))):
            out.append(f"P15 {where}: an if: with always(), failure() or cancelled() runs after a refusal")
    return out


def violations(doc: dict) -> list[str]:
    out: list[str] = []
    if triggers(doc) != EXPECTED_TRIGGERS:
        out.append(f"P2 the triggers are {sorted(triggers(doc))}, expected {sorted(EXPECTED_TRIGGERS)}")
    if doc.get("permissions") != PERMISSIONS:
        out.append(f"P10 the top-level permissions are {doc.get('permissions')!r}, expected {PERMISSIONS}")
    if "defaults" in doc:
        out.append("P13 a workflow-level defaults: block can change the shell of every run step")
    top = {k: v for k, v in doc.items() if k != "jobs"}
    out += [f"P3 {w} reads the secrets context outside the jobs" for w in _reads(top, SECRETS_CTX)]
    env = doc.get("env") if isinstance(doc.get("env"), dict) else {}
    if not IMAGE_PIN.fullmatch(str(env.get("LOCKSTEP_IMAGE", ""))):
        out.append("P4 env.LOCKSTEP_IMAGE must be docker.io/library/rust:<version>-bookworm@sha256:<64 hex>")
    jobs = doc.get("jobs")
    if not isinstance(jobs, dict):
        return [*out, "P1 the workflow has no jobs mapping"]
    if set(jobs) != EXPECTED_JOBS:
        out.append(f"P1 the jobs are {sorted(map(str, jobs))}, expected {sorted(EXPECTED_JOBS)}")
    out += [f"P9 {w} reads the needs context outside an if:" for w in _reads(doc, NEEDS_CTX, skip_if=True)]
    for name in sorted(EXPECTED_JOBS & set(jobs)):
        job = jobs[name]
        if not isinstance(job, dict):
            out.append(f"P1 job {name} is not a mapping")
            continue
        if job.get("permissions") != PERMISSIONS:
            out.append(f"P10 job {name} permissions are {job.get('permissions')!r}, expected {PERMISSIONS}")
        for key in ("container", "services", "uses", "secrets", "defaults"):
            if key in job:
                out.append(f"P12 job {name} declares {key}:")
        out += step_violations(name, _steps(job, name))
    build, propose = jobs.get("build"), jobs.get("propose")
    if isinstance(build, dict):
        if "environment" in build:
            out.append("P3 build declares an environment")
        out += [f"P3 {w} reads the secrets context in build" for w in _reads(build, SECRETS_CTX, "$.jobs.build")]
        if build.get("outputs") != BUILD_OUTPUTS:
            out.append(f"P9 build outputs are {build.get('outputs')!r}, expected {BUILD_OUTPUTS}")
    if isinstance(propose, dict):
        out += _propose_violations(propose)
    out += _push_violations(jobs)
    return out


def _propose_violations(propose: dict) -> list[str]:
    out = []
    if propose.get("environment") != "release-pr":
        out.append(f"P11 propose environment is {propose.get('environment')!r}, expected 'release-pr'")
    if _needs_list(propose.get("needs")) != ["build"]:
        out.append(f"P11 propose needs is {propose.get('needs')!r}, expected build")
    if str(propose.get("if", "")).strip() != "needs.build.result == 'success'":
        out.append("P11 propose if: must be exactly needs.build.result == 'success'")
    steps = _steps(propose, "propose")
    ids = tuple(str(s.get("id")) for s in steps)
    if ids != PROPOSE_ORDER:
        out.append(f"P7 the propose step order is {list(ids)}, expected {list(PROPOSE_ORDER)}")
    by_id = {str(s.get("id")): s for s in steps}
    verify = by_id.get("verify", {})
    verify_cmds = commands(str(verify.get("run", "")))[0]
    if not any(c[:3] == ["python3", CHECKER, "artifact"] for c in verify_cmds):
        out.append(f"P7 the verify step does not run `python3 {CHECKER} artifact`")
    token = by_id.get("token", {})
    if not str(token.get("uses", "")).startswith("actions/create-github-app-token@") or token.get("with") != TOKEN_WITH:
        out.append(f"P16 the token step must use actions/create-github-app-token with exactly {sorted(TOKEN_WITH)}")
    for index, step in enumerate(steps):
        if step.get("id") == "token":
            continue
        reads = _reads(step, SECRETS_CTX, f"$.jobs.propose.steps[{index}]")
        out += [f"P3 {w} reads the secrets context outside the token step" for w in reads]
    rest = {k: v for k, v in propose.items() if k != "steps"}
    out += [f"P3 {w} reads the secrets context in propose" for w in _reads(rest, SECRETS_CTX, "$.jobs.propose")]
    return out


def _push_violations(jobs: dict) -> list[str]:
    pushes = []
    for name, job in jobs.items():
        if not isinstance(job, dict) or not isinstance(job.get("steps"), list):
            continue
        for step in job["steps"]:
            if not isinstance(step, dict):
                continue
            for cmd in commands(str(step.get("run", "")))[0]:
                if cmd[0] == "git" and "push" in cmd[1:]:
                    pushes.append((name, step.get("id"), cmd))
    if len(pushes) != 1 or pushes[0][:2] != ("propose", "push"):
        return [f"P8 expected exactly one git push, in propose step push; found {[(p[0], p[1]) for p in pushes]}"]
    cmd = pushes[0][2]
    args = cmd[cmd.index("push") + 1:]
    out = []
    leases = [a for a in args if a.startswith(LEASE_PREFIX)]
    other_options = [a for a in args if a.startswith("-") and not a.startswith(LEASE_PREFIX)]
    positional = [a for a in args if not a.startswith("-")]
    if len(leases) != 1:
        out.append(f"P8 the push needs exactly one {LEASE_PREFIX}<sha> option")
    if other_options:
        out.append(f"P8 the push carries other options {other_options}")
    if positional != [PUSH_REMOTE, REFSPEC]:
        out.append(f"P8 the push target is {positional}, expected {[PUSH_REMOTE, REFSPEC]}")
    return out
```

- [ ] **Step 4: Run the self-test and ruff, and see them pass**

```bash
uv run --locked --project ci/wasm-lockstep --python '>=3.12' python3 ci/wasm-lockstep/pin_check.py --self-test; echo "rc=$?"
uv run --locked --project py ruff check --config py/pyproject.toml ci/wasm-lockstep/; echo "ruff=$?"
```

Expected: `pin_check self-test: 60 rows, 0 failed`, `rc=0`; `All checks passed!`, `ruff=0`. The negative control needs the real workflow, so Task 3 runs it.

- [ ] **Step 5: Prove two checks by mutation (spec T4, part 2)**

```bash
PY="$(uv run --locked --project ci/wasm-lockstep --python '>=3.12' python3 -c 'import sys; print(sys.executable)')"
"$PY" "$SCRATCH/t4_mutations.py" "$PY" "$SCRATCH/t4" pin_check.py; echo "rc=$?"
```

Expected: two `PROVED` lines (the output-rule check P9, the job-set check P1), each `rc 3`, `red=True`, `line count 1`; `t4: 0 mutation(s) not proved`; `rc=0`.

- [ ] **Step 6: Watch the new lockfile (repo:osv and dependabot)**

In `ci/osv/run.sh`, replace:

```bash
  'ci/helm-render/uv.lock'
)
```

with:

```bash
  'ci/helm-render/uv.lock'
  # SMA-693 — repo:wasm-lockstep resolves pyyaml through its own uv project, so its lockfile is a
  # fifth pip-ecosystem manifest. moon.yml's repo:osv inputs carry the same path.
  'ci/wasm-lockstep/uv.lock'
)
```

In `moon.yml` (the `repo:osv` inputs), replace:

```yaml
      - 'ci/helm-render/uv.lock'
      - '.prototools'
      - '.proto/plugins/osv-scanner.toml'
```

with:

```yaml
      - 'ci/helm-render/uv.lock'
      # SMA-693 — the same for repo:wasm-lockstep's own uv project; it is in ci/osv/run.sh's
      # LOCKFILES array too. This entry re-keys the gate when that lockfile moves.
      - 'ci/wasm-lockstep/uv.lock'
      - '.prototools'
      - '.proto/plugins/osv-scanner.toml'
```

In `.github/dependabot.yml`, after the `/ci/helm-render` entry (its last line is `          - patch`, directly above `  # ---- GitHub Actions: workflows at repo root ----`), insert:

```yaml

  # ---- Python: standalone uv project for repo:wasm-lockstep (SMA-693) ----
  # A FOURTH uv entry, for the same reason as the /ci/workflow-credentials entry above: this
  # gate's project sits outside the py/ workspace on purpose, so without this entry its pinned
  # pyyaml is in a lockfile that nothing watches. Settings mirror that block exactly.
  - package-ecosystem: uv
    directory: /ci/wasm-lockstep
    schedule:
      interval: weekly
      day: monday
      time: "06:00"
      timezone: Etc/UTC
    commit-message:
      prefix: "build(deps)"
      prefix-development: "build(deps)"
    groups:
      uv-minor-patch:
        applies-to: version-updates
        update-types:
          - minor
          - patch
```

- [ ] **Step 7: Run the watch checks**

```bash
git add ci/wasm-lockstep/pyproject.toml ci/wasm-lockstep/uv.lock
./ci/osv/run.sh; echo "rc=$?"
uv run --locked --project ci/wasm-lockstep python3 -c 'import yaml; d=yaml.safe_load(open(".github/dependabot.yml")); print([u["directory"] for u in d["updates"] if u["package-ecosystem"]=="uv"])'
```

Expected: an `osv gate:` line for `ci/wasm-lockstep/uv.lock` with a count of at least 1 package, `rc=0` (osv-scanner needs network access); `['/py', '/ci/workflow-credentials', '/ci/helm-render', '/ci/wasm-lockstep']`.

- [ ] **Step 8: Commit**

```bash
git add ci/wasm-lockstep/pin_check.py ci/wasm-lockstep/pyproject.toml ci/wasm-lockstep/uv.lock ci/osv/run.sh moon.yml .github/dependabot.yml
git commit -m "feat(ci): add the wasm-lockstep workflow pin check" -m "A PyYAML parse of the workflow, in its own uv project, with seventeen rules for the trust
model of the lockstep updater. repo:osv and dependabot watch the new lockfile." -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: The workflow and the container script

**Files:**
- Create: `.github/workflows/wasm-lockstep.yml`
- Create: `ci/wasm-lockstep/container.sh` (mode 100755)

**Interfaces:**
- Consumes: Task 1 CLI (`lock`, `artifact`, `status`); Task 2 CLI (`pin_check.py <workflow>`, `--negative-control <workflow>`).
- Produces: the workflow `wasm-lockstep` (job ids `build`, `propose`; `build` output `changed`; `propose` step ids `checkout, download, verify, apply, token, commit, base, push, pr, close`); `container.sh update|build`, run only inside the build container.

- [ ] **Step 1: Run the pin check on the absent workflow and see it fail**

```bash
uv run --locked --project ci/wasm-lockstep --python '>=3.12' python3 ci/wasm-lockstep/pin_check.py .github/workflows/wasm-lockstep.yml; echo "rc=$?"
```

Expected: `pin_check: infrastructure error: cannot read .github/workflows/wasm-lockstep.yml: …`, `rc=2`.

- [ ] **Step 2: Write the container script**

Create `ci/wasm-lockstep/container.sh` with exactly this content, then make it executable:

```bash
#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# The two container runs of .github/workflows/wasm-lockstep.yml (SMA-693 spec 5.1, steps 4 and 6).
#
#   container.sh update   cargo update of the four family roots (cargo runs no build script)
#   container.sh build    proto, moon setup, the rustup components, pnpm install, generate-wasm
#
# This script runs INSIDE the build container only: `docker run --rm --user 65534:65534` over the
# work copy at /work, with no runner environment (no CI, no GITHUB_ACTIONS, no
# ACTIONS_RUNTIME_TOKEN). Do not run it on a host: it installs proto into $HOME, makes a git
# repository in /work and writes rs/target and ts/node_modules there.
#
# WHY THE USER nobody (65534) AND NOT THE RUNNER'S UID. generate-wasm --post calls Node's
# os.userInfo(), which throws when the uid has no passwd entry, and the image has no entry for the
# runner's uid 1001. `nobody` has one. The identity markers that --post then rejects are `nobody`
# and the last segment of HOME, `lockstep-home` (generate-wasm.mjs post()).
set -euo pipefail

export HOME=/tmp/lockstep-home
mkdir -p "$HOME"
cd /work

# proto prints NDJSON on stdout in an agent environment (SMA-609). Harmless here, and the task
# script of generate-wasm captures a node call, so keep the same rule.
export PROTO_REPORTER=text

case "${1:-}" in
  update)
    cd rs
    cargo update -p wasm-bindgen -p js-sys -p web-sys -p wasm-bindgen-futures
    ;;
  build)
    # Moon's vcs client is git, and the work copy has no .git (spec 5.1: the host made the copy
    # with git archive). The repository is made HERE, inside the container, so the host never
    # runs git on the work copy. safe.directory: /work belongs to the runner's uid, not to nobody.
    cat > "$HOME/.gitconfig" <<'EOF'
[safe]
	directory = /work
[user]
	name = wasm-lockstep
	email = wasm-lockstep@invalid
[commit]
	gpgsign = false
[maintenance]
	auto = false
[gc]
	auto = 0
EOF
    git init -q
    git add -A
    git commit -q -m "wasm-lockstep work copy"

    proto_version="$(sed -n 's/^proto = "\([0-9][0-9.]*\)"$/\1/p' .prototools)"
    case "$proto_version" in
      ''|*[!0-9.]*) echo "container.sh: no proto pin in .prototools (got '${proto_version}')" >&2; exit 2 ;;
    esac
    curl -fsSL https://moonrepo.dev/install/proto.sh -o "$HOME/proto-install.sh"
    bash "$HOME/proto-install.sh" "$proto_version" --yes --no-profile
    export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"

    proto install
    moon setup
    # The serial pre-install of ci.yml: concurrent first cargo calls race on ~/.rustup/downloads.
    ( cd rs && rustup component add rustfmt clippy && rustup target add wasm32-unknown-unknown )
    pnpm --dir ts install --frozen-lockfile
    moon run paigasus-kernel-ts:generate-wasm
    ;;
  *)
    echo "container.sh: usage: container.sh update|build" >&2
    exit 2
    ;;
esac
```

```bash
chmod +x ci/wasm-lockstep/container.sh
```

- [ ] **Step 3: Write the workflow**

Create `.github/workflows/wasm-lockstep.yml` with exactly this content:

```yaml
# SPDX-License-Identifier: Apache-2.0
#
# wasm-lockstep — propose the wasm-bindgen family bump as ONE pull request (SMA-693).
#
# js-sys, web-sys and wasm-bindgen-futures pin wasm-bindgen with `=`, so dependabot cannot move the
# family (SMA-683). This workflow runs the four-package `cargo update -p` on `main`, regenerates the
# five committed wasm artifacts on Linux, and opens or updates a pull request on the bot branch
# `deps/wasm-bindgen-lockstep`. A person reviews the lock diff before the merge (spec section 9:
# nothing enforces that review).
#
# THE TRUST MODEL (spec 5.1, 5.2). Read ci/wasm-lockstep/README.md before you change this file.
#   * `build` has no environment and reads no secret. Every step that runs third-party code runs
#     inside `docker run`, over a copy of the tree without `.git`, with no runner environment, so
#     that code cannot read ACTIONS_RUNTIME_TOKEN, write a cache entry, or write GITHUB_OUTPUT.
#   * `propose` holds the App key (environment `release-pr`, main-only branch policy). It executes
#     no artifact content: no toolchain, no cargo, no pnpm, no moon, no script from the artifact.
#   * `needs.build.outputs` appears in `propose` only in `if:` values. Every value that reaches a
#     command comes from lockstep_check.py's output on the downloaded bytes.
# repo:wasm-lockstep (ci/wasm-lockstep/pin_check.py) asserts these rules on every PR that edits
# this file. A refusal of the checker stays red until a person runs the manual runbook in
# rs/CLAUDE.md (SMA-693 Q10).
name: wasm-lockstep

on:
  schedule:
    # Tuesday 06:17 UTC (SMA-693 Q3): after the Monday 06:00 UTC dependabot run, and not on a round
    # minute, where GitHub delays or drops scheduled runs. GitHub sends the failure mail of a
    # scheduled run to the person who last changed this cron line (spec section 9).
    - cron: '17 6 * * 2'
  workflow_dispatch:

permissions:
  contents: read

concurrency:
  group: wasm-lockstep
  cancel-in-progress: false

env:
  # The build container: the rust image that matches rs/rust-toolchain.toml, pinned by the digest
  # of its multi-platform index (resolved 2026-10-02 with `docker buildx imagetools inspect`).
  # Bump it together with rs/rust-toolchain.toml.
  LOCKSTEP_IMAGE: docker.io/library/rust:1.95.0-bookworm@sha256:6258907abe69656e41cd992e0b705cdcfabcbbe3db374f92ed2d47121282d4a1

jobs:
  build:
    name: update the family and regenerate the wasm artifacts
    runs-on: ubuntu-latest
    timeout-minutes: 60
    permissions:
      contents: read
    outputs:
      changed: ${{ steps.lock.outputs.changed }}
    steps:
      - name: Reclaim runner disk
        id: reclaim
        run: |
          df -h /
          sudo rm -rf /usr/local/lib/android /usr/share/dotnet /opt/ghc /usr/local/.ghcup /opt/hostedtoolcache/CodeQL || true
          sudo docker image prune --all --force > /dev/null 2>&1 || true
          df -h /

      - name: Checkout
        id: checkout
        uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1  # v7.0.1
        with:
          fetch-depth: 1
          persist-credentials: false

      # A convenience check, not the boundary. The boundary is the main-only branch policy of the
      # release-pr environment, which refuses the propose job on any other ref.
      - name: Refuse a ref other than main
        id: ref
        env:
          REF: ${{ github.ref }}
        run: |
          if [ "$REF" != "refs/heads/main" ]; then
            echo "::error::wasm-lockstep runs on refs/heads/main only, not on $REF."
            exit 1
          fi

      # The container gets this copy and nothing else. It has no .git, and the host runs no git
      # command and no file from it after this step: a git call can run a hook or an fsmonitor
      # command from .git/config. The old lock comes from git show, not from the copy.
      - name: Make the work copy without .git
        id: copy
        run: |
          set -euo pipefail
          mkdir -p "$RUNNER_TEMP/work" "$RUNNER_TEMP/stage"
          git archive HEAD | tar -x -C "$RUNNER_TEMP/work"
          git show HEAD:rs/Cargo.lock > "$RUNNER_TEMP/old.lock"
          chmod -R a+rwX "$RUNNER_TEMP/work"

      # Container run 1: cargo resolves the index and runs no build script.
      - name: Update the four family roots in the container
        id: update
        run: |
          set -euo pipefail
          docker run --rm --user 65534:65534 --volume "$RUNNER_TEMP/work:/work" --workdir /work "$LOCKSTEP_IMAGE" bash ci/wasm-lockstep/container.sh update

      # The early exit, in the runbook's order: a refusal stops the job before any compile. This
      # is not the control: propose runs the same checker again on the downloaded bytes. The host
      # writes `changed` here, before the compile, so third-party code cannot set it.
      - name: Check the lock change on the host
        id: lock
        run: |
          set -euo pipefail
          rc=0
          python3 ci/wasm-lockstep/lockstep_check.py lock --old "$RUNNER_TEMP/old.lock" --new "$RUNNER_TEMP/work/rs/Cargo.lock" > "$RUNNER_TEMP/lock-verdict.txt" || rc=$?
          cat "$RUNNER_TEMP/lock-verdict.txt"
          if [ "$rc" -eq 0 ]; then
            echo "changed=true" >> "$GITHUB_OUTPUT"
          elif [ "$rc" -eq 4 ]; then
            echo "changed=false" >> "$GITHUB_OUTPUT"
            version="$(sed -n 's/^family-current wasm-bindgen \([0-9.]*\)$/\1/p' "$RUNNER_TEMP/lock-verdict.txt")"
            echo "::notice::The wasm-bindgen family is current at ${version}. No pull request is needed."
            echo "The wasm-bindgen family is current at ${version}." >> "$GITHUB_STEP_SUMMARY"
          elif [ "$rc" -eq 3 ]; then
            echo "::error::lockstep_check refused the lock change (see the line above). Follow the manual runbook in rs/CLAUDE.md, \"The wasm-bindgen family does not move through dependabot\"."
            exit 1
          else
            echo "::error::lockstep_check could not run (exit ${rc}). This is an infrastructure error, not a verdict."
            exit 1
          fi

      # Container run 2: proto, moon setup, pnpm install, then generate-wasm. The container has no
      # CI variable, so Moon does not drop the runInCI: false task (spec F2).
      - name: Regenerate the wasm artifacts in the container
        id: build
        if: steps.lock.outputs.changed == 'true'
        run: |
          set -euo pipefail
          docker run --rm --user 65534:65534 --volume "$RUNNER_TEMP/work:/work" --workdir /work "$LOCKSTEP_IMAGE" bash ci/wasm-lockstep/container.sh build

      # Copy only. The host executes no file of the work copy. Every path component is checked for
      # a symlink, so a symlinked file or parent directory cannot make cp read a host file.
      - name: Stage the six files
        id: stage
        if: steps.lock.outputs.changed == 'true'
        run: |
          set -euo pipefail
          src="$RUNNER_TEMP/work"
          dst="$RUNNER_TEMP/stage"
          for d in rs rs/crates rs/crates/bindings rs/crates/bindings/paigasus-wasm; do
            if test -L "$src/$d"; then
              echo "::error::$d in the work copy is a symlink."
              exit 1
            fi
          done
          mkdir -p "$dst/rs/crates/bindings/paigasus-wasm"
          for f in rs/Cargo.lock rs/crates/bindings/paigasus-wasm/paigasus_wasm.js rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.js rs/crates/bindings/paigasus-wasm/paigasus_wasm.d.ts rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm.d.ts rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm; do
            if test -L "$src/$f"; then
              echo "::error::$f in the work copy is a symlink."
              exit 1
            fi
            test -f "$src/$f"
            cp "$src/$f" "$dst/$f"
          done

      # ONE path, so the artifact root is the stage directory and the download holds rs/... (F13).
      - name: Upload the artifact
        id: upload
        if: steps.lock.outputs.changed == 'true'
        uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a  # v7.0.1
        with:
          name: wasm-lockstep
          path: ${{ runner.temp }}/stage/
          if-no-files-found: error
          retention-days: 7

  propose:
    name: check the artifact and propose the pull request
    needs: build
    if: needs.build.result == 'success'
    runs-on: ubuntu-latest
    timeout-minutes: 15
    # The credential boundary, the same one release.yml uses (SMA-580): PAIGASUS_BOT_* are
    # environment secrets on release-pr, whose deployment branch policy is main-only.
    environment: release-pr
    permissions:
      contents: read
    steps:
      - name: Checkout the commit the build ran on
        id: checkout
        if: needs.build.outputs.changed == 'true'
        uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1  # v7.0.1
        with:
          ref: ${{ github.sha }}
          persist-credentials: false

      - name: Download the artifact
        id: download
        if: needs.build.outputs.changed == 'true'
        uses: actions/download-artifact@3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c  # v8.0.1
        with:
          name: wasm-lockstep
          path: ${{ runner.temp }}/lockstep

      # AC4.1 and AC4.2, from the checked-out github.sha (trusted), on the downloaded bytes. A
      # refusal exits 3 and fails this step, so no later step runs and no token is minted.
      - name: Check the artifact and the lock
        id: verify
        if: needs.build.outputs.changed == 'true'
        run: |
          set -euo pipefail
          python3 ci/wasm-lockstep/lockstep_check.py artifact --dir "$RUNNER_TEMP/lockstep" --old rs/Cargo.lock --body-file "$RUNNER_TEMP/pr-body.md" --title-file "$RUNNER_TEMP/pr-title.txt"

      - name: Apply the files to the checkout
        id: apply
        if: needs.build.outputs.changed == 'true'
        run: |
          set -euo pipefail
          for f in rs/Cargo.lock rs/crates/bindings/paigasus-wasm/paigasus_wasm.js rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.js rs/crates/bindings/paigasus-wasm/paigasus_wasm.d.ts rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm.d.ts rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm; do
            if test -f "$RUNNER_TEMP/lockstep/$f"; then
              cp "$RUNNER_TEMP/lockstep/$f" "$f"
            fi
          done
          git status --porcelain --untracked-files=all > "$RUNNER_TEMP/status.txt"
          cat "$RUNNER_TEMP/status.txt"
          python3 ci/wasm-lockstep/lockstep_check.py status --file "$RUNNER_TEMP/status.txt"

      # Runs on both paths: changed == 'true' needs it to push, changed == 'false' to close an
      # obsolete pull request. The two permissions are explicit, as in release.yml (zizmor
      # github-app): without them the token carries every permission of the installation.
      - name: Mint the App installation token
        id: token
        uses: actions/create-github-app-token@bcd2ba49218906704ab6c1aa796996da409d3eb1  # v3.2.0
        with:
          client-id: ${{ secrets.PAIGASUS_BOT_APP_ID }}
          private-key: ${{ secrets.PAIGASUS_BOT_PRIVATE_KEY }}
          permission-contents: write
          permission-pull-requests: write

      # The identity of release.yml: the App's bot user in the resolvable <id>+<login>@ form. The
      # message is the checker's title only, with no body (the commitlint footer trap).
      - name: Commit as the bot
        id: commit
        if: needs.build.outputs.changed == 'true'
        run: |
          set -euo pipefail
          git config user.name "paigasusbot[bot]"
          git config user.email "285361405+paigasusbot[bot]@users.noreply.github.com"
          git add -- rs/Cargo.lock rs/crates/bindings/paigasus-wasm
          git commit -F "$RUNNER_TEMP/pr-title.txt"

      # AC4.3, immediately before the push. It branches on the exit status of gh api, never on an
      # `|| echo` fallback: gh api prints a 404 body on stdout (.github/CLAUDE.md). A moved main is
      # a GREEN notice (SMA-693 Q8): the next run proposes on the new base.
      - name: Check that main did not move
        id: base
        if: needs.build.outputs.changed == 'true'
        env:
          GH_TOKEN: ${{ github.token }}
          BUILT_ON: ${{ github.sha }}
        run: |
          set -euo pipefail
          rc=0
          gh api "repos/${GITHUB_REPOSITORY}/git/ref/heads/main" --jq .object.sha > "$RUNNER_TEMP/main-sha.txt" || rc=$?
          if [ "$rc" -ne 0 ]; then
            cat "$RUNNER_TEMP/main-sha.txt"
            echo "::error::gh api could not read refs/heads/main (exit ${rc})."
            exit 1
          fi
          now="$(cat "$RUNNER_TEMP/main-sha.txt")"
          if [ "${#now}" -ne 40 ]; then
            echo "::error::gh api returned '${now}', not a commit SHA."
            exit 1
          fi
          if [ "$now" != "$BUILT_ON" ]; then
            echo "moved=true" >> "$GITHUB_OUTPUT"
            echo "::notice::main moved from ${BUILT_ON} to ${now}. Nothing was pushed; the next run proposes on the new base."
          else
            echo "moved=false" >> "$GITHUB_OUTPUT"
          fi

      # The branch-owner check, then ONE push with a lease on the SHA read here. The refspec is
      # fixed text; repo:wasm-lockstep pins it. An absent branch leases on the empty value, so the
      # push fails if the branch appears in between.
      - name: Check the bot branch owner and push
        id: push
        if: needs.build.outputs.changed == 'true' && steps.base.outputs.moved == 'false'
        env:
          GH_TOKEN: ${{ steps.token.outputs.token }}
          PUSH_TOKEN: ${{ steps.token.outputs.token }}
        run: |
          set -euo pipefail
          rc=0
          gh api "repos/${GITHUB_REPOSITORY}/git/ref/heads/deps/wasm-bindgen-lockstep" --jq .object.sha > "$RUNNER_TEMP/branch-sha.txt" || rc=$?
          if [ "$rc" -eq 0 ]; then
            lease="$(cat "$RUNNER_TEMP/branch-sha.txt")"
            author="$(gh api "repos/${GITHUB_REPOSITORY}/commits/${lease}" --jq .author.login)"
            if [ "$author" != "paigasusbot[bot]" ]; then
              echo "::error::A person pushed to the bot branch (head ${lease} by '${author}'). Merge or close that pull request first."
              exit 1
            fi
          elif grep -qF '"status":"404"' "$RUNNER_TEMP/branch-sha.txt"; then
            lease=""
          else
            cat "$RUNNER_TEMP/branch-sha.txt"
            echo "::error::gh api could not read the bot branch (exit ${rc})."
            exit 1
          fi
          git push --force-with-lease="refs/heads/deps/wasm-bindgen-lockstep:${lease}" "https://x-access-token:${PUSH_TOKEN}@github.com/${GITHUB_REPOSITORY}.git" HEAD:refs/heads/deps/wasm-bindgen-lockstep

      # The App token opens the pull request, so its CI runs (a GITHUB_TOKEN event starts no
      # workflow).
      - name: Open or update the pull request
        id: pr
        if: needs.build.outputs.changed == 'true' && steps.base.outputs.moved == 'false'
        env:
          GH_TOKEN: ${{ steps.token.outputs.token }}
        run: |
          set -euo pipefail
          gh pr list --repo "$GITHUB_REPOSITORY" --head deps/wasm-bindgen-lockstep --state open --json number --jq '.[].number' > "$RUNNER_TEMP/open-prs.txt"
          number="$(sed -n 1p "$RUNNER_TEMP/open-prs.txt")"
          if [ -z "$number" ]; then
            gh pr create --repo "$GITHUB_REPOSITORY" --base main --head deps/wasm-bindgen-lockstep --title "$(cat "$RUNNER_TEMP/pr-title.txt")" --body-file "$RUNNER_TEMP/pr-body.md"
          else
            gh pr edit "$number" --repo "$GITHUB_REPOSITORY" --title "$(cat "$RUNNER_TEMP/pr-title.txt")" --body-file "$RUNNER_TEMP/pr-body.md"
          fi

      - name: Close an obsolete pull request
        id: close
        if: needs.build.outputs.changed == 'false'
        env:
          GH_TOKEN: ${{ steps.token.outputs.token }}
        run: |
          set -euo pipefail
          gh pr list --repo "$GITHUB_REPOSITORY" --head deps/wasm-bindgen-lockstep --state open --json number --jq '.[].number' > "$RUNNER_TEMP/open-prs.txt"
          while read -r number; do
            gh pr close "$number" --repo "$GITHUB_REPOSITORY" --comment "The wasm-bindgen family is current on main. This proposal is obsolete."
          done < "$RUNNER_TEMP/open-prs.txt"
```

- [ ] **Step 4: Run the pin check, its control, actionlint and shellcheck, and see them pass**

```bash
uv run --locked --project ci/wasm-lockstep --python '>=3.12' python3 ci/wasm-lockstep/pin_check.py .github/workflows/wasm-lockstep.yml; echo "rc=$?"
uv run --locked --project ci/wasm-lockstep --python '>=3.12' python3 ci/wasm-lockstep/pin_check.py --negative-control .github/workflows/wasm-lockstep.yml; echo "rc=$?"
AL="$(proto --reporter text bin actionlint | tail -n1)"; [ -x "$AL" ] || echo "actionlint did not resolve"
SC="$(uv run --locked --project py python3 -c 'import shutil; print(shutil.which("shellcheck"))')"; [ -x "$SC" ] || echo "shellcheck did not resolve"
"$AL" -shellcheck "$SC" .github/workflows/wasm-lockstep.yml; echo "actionlint=$?"
"$SC" ci/wasm-lockstep/container.sh; echo "shellcheck=$?"
```

Expected: `pin_check: .github/workflows/wasm-lockstep.yml satisfies P0-P17`, `rc=0`; four `ok` lines and `pin_check negative control: 4 mutations, 0 failed`, `rc=0`; `actionlint=0`; `shellcheck=0`. If actionlint hangs at 0% CPU, the host pipe holds 512 bytes (SMA-612): stop it, and run this one command in a `docker run --rm -v "$PWD:/w" -w /w ubuntu:24.04` container instead, or leave the verdict to CI and say so.

- [ ] **Step 5: Confirm that repo:workflow-credentials does not change (spec T7)**

```bash
PATH="$SCRATCH/bash32:$PATH" ./ci/workflow-credentials/run.sh; echo "rc=$?"
git diff --quiet origin/main -- ci/workflow-credentials/ && echo "workflow-credentials unchanged"
```

Expected: a `workflow-credentials: subjects:` line that does NOT name `wasm-lockstep.yml`, `rc=0`; `workflow-credentials unchanged`. The workflow has no `pull_request`, `pull_request_target`, `issue_comment` or `workflow_run` trigger (AC7).

- [ ] **Step 6: Commit**

```bash
git add .github/workflows/wasm-lockstep.yml ci/wasm-lockstep/container.sh
git commit -m "feat(ci): add the weekly wasm-bindgen lockstep workflow" -m "Job build updates the four family roots and regenerates the wasm artifacts inside a docker
container with no runner environment. Job propose checks the downloaded bytes, mints the
App token in the release-pr environment and opens one pull request on deps/wasm-bindgen-
lockstep." -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: `run.sh`, the README, and the `repo:wasm-lockstep` Moon task

**Files:**
- Create: `ci/wasm-lockstep/run.sh` (mode 100755)
- Create: `ci/wasm-lockstep/README.md`
- Modify: `moon.yml` (append a task after `moon-diagnosis-exec`, which ends at line 1072)

**Interfaces:**
- Consumes: Task 1 CLI (`--self-test`, `--negative-control --lock`, `current --lock`, `lock --old --new`); Task 2 CLI (`--self-test`, `--negative-control <wf>`, `<wf>`); Task 3 workflow.
- Produces: `./ci/wasm-lockstep/run.sh [--self-test|--negative-control]` -> 0 / 1 / 2. Its pinned lines are listed in Task 5 (`WASM_LOCKSTEP_SH_CALL_SITES`); keep them byte-identical. The Moon target `repo:wasm-lockstep`.

- [ ] **Step 1: Run the Moon target before it exists and see it fail**

```bash
moon run repo:wasm-lockstep; echo "rc=$?"
```

Expected: a non-zero rc and a Moon error that no task `wasm-lockstep` exists in project `repo`.

- [ ] **Step 2: Write `run.sh`**

Create `ci/wasm-lockstep/run.sh` with exactly this content, then make it executable:

```bash
#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# repo:wasm-lockstep — the gate over .github/workflows/wasm-lockstep.yml and its checker (SMA-693).
#
#   run.sh                     pin_check.py on the real workflow, and lockstep_check.py current on
#                              HEAD's rs/Cargo.lock (the family invariant every weekly run needs)
#   run.sh --self-test         both checkers' in-process fixture tables, plus this wrapper's rc rows
#   run.sh --negative-control  both checkers' controls on the real inputs, plus the 3 -> 1 mapping
#                              on a real refusal
#
# Exit codes: 0 pass | 1 an assertion failed | 2 infrastructure error. Both checkers exit 3 for an
# assertion (and lockstep_check.py 4 for "no change"). This wrapper maps 3 to 1 and every other
# non-zero code to 2, because `uv` and a Python traceback both exit 1 (ci/CLAUDE.md). Do not
# "normalize" a checker to 1.
#
# The lock comes from `git show HEAD:rs/Cargo.lock`, never from the working tree: an unlocked
# cargo call inside `moon ci` can rewrite the working-tree lock during the run (spec F15).
#
# Runs under /bin/bash 3.2.57 and bash 5: no mapfile, no declare -A, no here-string, no pipe into
# an early-exit reader (ci/actionlint check 13). A function called in `|| rc=$?` position runs
# with errexit OFF, so every function below routes each status by hand and never calls
# die_infra from inside a command substitution (the errexit-swallows-nested-exit trap).
set -euo pipefail

# proto prints NDJSON on stdout in an agent environment, which poisons every $(...) capture of a
# proto or shim call. Exported once here, so every later capture inherits it (SMA-609).
export PROTO_REPORTER=text

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
HERE="$REPO_ROOT/ci/wasm-lockstep"
WORKFLOW="$REPO_ROOT/.github/workflows/wasm-lockstep.yml"

die_infra() { printf 'wasm-lockstep: infrastructure error (rc=2): %s\n' "$*" >&2; exit 2; }

MODE=check
while [ $# -gt 0 ]; do
  case "$1" in
    --self-test)        MODE=selftest; shift ;;
    --negative-control) MODE=negctl;   shift ;;
    *) die_infra "unknown flag: $1" ;;
  esac
done

TMP="$(mktemp -d)" || die_infra "mktemp -d failed"
trap 'rm -rf "$TMP"' EXIT

# The venv interpreter, resolved ONCE, at the top level. --locked stops uv from rewriting
# uv.lock. lockstep_check.py needs only the stdlib, but runs through the same >=3.12 interpreter,
# because the host python3 of the development Mac can be older than tomllib (3.11).
command -v uv >/dev/null 2>&1 || die_infra "uv is not on PATH; run 'proto install', or add ~/.proto/shims to PATH"
PY="$(uv run --locked --project "$HERE" --python '>=3.12' python3 -c 'import sys, tomllib, yaml; print(sys.executable)')" \
  || die_infra "uv could not provide the locked PyYAML environment for $HERE"
case "$PY" in
  "$HERE/.venv/"*) ;;
  *) die_infra "the interpreter is not under $HERE/.venv. Got: ${PY:-<empty>}" ;;
esac
git -C "$REPO_ROOT" show HEAD:rs/Cargo.lock >"$TMP/head.lock" || die_infra "git show HEAD:rs/Cargo.lock failed"

# A checker's raw rc -> the gate's rc. Echo only; never exits.
gate_rc() {
  case "$1" in 0) echo 0 ;; 3) echo 1 ;; *) echo 2 ;; esac
}

# Runs "$PY" "$@" and RETURNS the mapped rc. Never exits.
run_checker() {
  local raw=0
  "$PY" "$@" || raw=$?
  return "$(gate_rc "$raw")"
}

report() {  # $1 label, $2 gate rc
  case "$2" in
    0) printf 'PASS  [%s]\n' "$1" ;;
    1) printf 'FAIL  [%s]: assertion failure\n' "$1" >&2 ;;
    *) printf 'FAIL  [%s]: infrastructure error (rc=2)\n' "$1" >&2 ;;
  esac
}

real_run() {
  local worst=0 pin_rc lock_rc
  pin_rc=0; run_checker "$HERE/pin_check.py" "$WORKFLOW" || pin_rc=$?
  report "pin_check $WORKFLOW" "$pin_rc"
  if [ "$pin_rc" -gt "$worst" ]; then worst="$pin_rc"; fi
  lock_rc=0; run_checker "$HERE/lockstep_check.py" current --lock "$TMP/head.lock" || lock_rc=$?
  report "lockstep_check current on HEAD:rs/Cargo.lock" "$lock_rc"
  if [ "$lock_rc" -gt "$worst" ]; then worst="$lock_rc"; fi
  exit "$worst"
}

self_test() {
  local st_worst=0 lst_rc pst_rc pair raw want got map_rc
  lst_rc=0; run_checker "$HERE/lockstep_check.py" --self-test || lst_rc=$?
  report "lockstep_check --self-test" "$lst_rc"
  if [ "$lst_rc" -gt "$st_worst" ]; then st_worst="$lst_rc"; fi
  pst_rc=0; run_checker "$HERE/pin_check.py" --self-test || pst_rc=$?
  report "pin_check --self-test" "$pst_rc"
  if [ "$pst_rc" -gt "$st_worst" ]; then st_worst="$pst_rc"; fi
  # The mapping table, and the mapping of a REAL child status (a table alone does not prove that
  # run_checker routes the status it gets).
  for pair in 0:0 3:1 1:2 2:2 4:2 127:2; do
    raw="${pair%%:*}"; want="${pair#*:}"
    got="$(gate_rc "$raw")"
    if [ "$got" != "$want" ]; then
      printf 'FAIL  [gate_rc %s]: got %s, want %s\n' "$raw" "$got" "$want" >&2
      [ "$st_worst" -ge 1 ] || st_worst=1
    fi
    map_rc=0; run_checker -c "raise SystemExit($raw)" || map_rc=$?
    if [ "$map_rc" != "$want" ]; then
      printf 'FAIL  [run_checker exit %s]: got %s, want %s\n' "$raw" "$map_rc" "$want" >&2
      [ "$st_worst" -ge 1 ] || st_worst=1
    fi
  done
  if [ "$st_worst" -ne 0 ]; then
    printf 'wasm-lockstep self-test: FAILED (rc=%s)\n' "$st_worst" >&2
    exit "$st_worst"
  fi
  printf '== wasm-lockstep self-test passed ==\n'
}

negative_control() {
  local nc_worst=0 lnc_rc pnc_rc nc_map_rc
  lnc_rc=0; run_checker "$HERE/lockstep_check.py" --negative-control --lock "$TMP/head.lock" || lnc_rc=$?
  report "lockstep_check --negative-control" "$lnc_rc"
  if [ "$lnc_rc" -gt "$nc_worst" ]; then nc_worst="$lnc_rc"; fi
  pnc_rc=0; run_checker "$HERE/pin_check.py" --negative-control "$WORKFLOW" || pnc_rc=$?
  report "pin_check --negative-control" "$pnc_rc"
  if [ "$pnc_rc" -gt "$nc_worst" ]; then nc_worst="$pnc_rc"; fi
  # The 3 -> 1 mapping on a REAL refusal: HEAD's lock with the version of its first [[package]]
  # changed. awk is the same on BSD and GNU for this script. The refusal must reach this wrapper
  # as 1 and name its code; an infrastructure exit or a pass is a failure of the control.
  awk 'BEGIN { done = 0 } /^version = "/ && !done && seen { print "version = \"999.0.0\""; done = 1; next } /^\[\[package\]\]/ { seen = 1 } { print }' \
    "$TMP/head.lock" >"$TMP/mutated.lock" || die_infra "awk could not write the mutated lock"
  if cmp -s "$TMP/head.lock" "$TMP/mutated.lock"; then
    die_infra "the mutation changed nothing in HEAD's rs/Cargo.lock"
  fi
  nc_map_rc=0; run_checker "$HERE/lockstep_check.py" lock --old "$TMP/head.lock" --new "$TMP/mutated.lock" >"$TMP/map.out" 2>&1 || nc_map_rc=$?
  if [ "$nc_map_rc" -ne 1 ]; then
    printf 'FAIL  [3 -> 1 on a real refusal]: the wrapper returned %s, want 1\n' "$nc_map_rc" >&2
    cat "$TMP/map.out" >&2
    [ "$nc_worst" -ge 1 ] || nc_worst=1
  fi
  if ! grep -qF -- 'R-NONFAMILY' "$TMP/map.out"; then
    printf 'FAIL  [3 -> 1 on a real refusal]: the refusal does not name R-NONFAMILY\n' >&2
    [ "$nc_worst" -ge 1 ] || nc_worst=1
  fi
  if [ "$nc_worst" -ne 0 ]; then
    printf 'wasm-lockstep negative control: FAILED (rc=%s)\n' "$nc_worst" >&2
    exit "$nc_worst"
  fi
  printf '== wasm-lockstep negative control passed ==\n'
}

case "$MODE" in
  selftest) self_test ;;
  negctl)   negative_control ;;
  check)    real_run ;;
esac
```

```bash
chmod +x ci/wasm-lockstep/run.sh
```

- [ ] **Step 3: Write the README**

Create `ci/wasm-lockstep/README.md` with exactly this content:

````markdown
# `ci/wasm-lockstep/` — the lockstep updater and its gate (SMA-693)

`.github/workflows/wasm-lockstep.yml` moves the wasm-bindgen family in lockstep and regenerates the
five committed wasm artifacts. It opens or updates ONE pull request on the bot branch
`deps/wasm-bindgen-lockstep`. A person reviews the lock diff before the merge.

`repo:wasm-lockstep` is the gate over that workflow and its checker. Spec:
`docs/superpowers/specs/2026-09-27-sma-693-wasm-bindgen-lockstep-updater-design.md`.

## Files

| File | Job |
|---|---|
| `lockstep_check.py` | The lock and artifact checker. Stdlib only: the `propose` job runs it with the runner's own `python3`. |
| `pin_check.py` | The pin check of the workflow. PyYAML, from this directory's own uv project. |
| `container.sh` | The two container runs of the `build` job. It runs inside the build container only. |
| `run.sh` | The gate wrapper: `--self-test`, `--negative-control`, and the real run. |
| `pyproject.toml`, `uv.lock` | PyYAML for `pin_check.py`. Dependabot and `repo:osv` watch the lock. |

## Exit codes

| Code | `lockstep_check.py` | `pin_check.py` | `run.sh` |
|---|---|---|---|
| 0 | pass | pass | pass |
| 1 | (a Python traceback) | (a Python traceback) | an assertion failed |
| 2 | infrastructure | infrastructure | infrastructure |
| 3 | refusal | a rule failed | (not used) |
| 4 | no change | (not used) | (not used) |

`run.sh` maps 3 to 1 and every other non-zero code to 2. `uv` and a Python traceback both exit 1,
so a checker that exits 1 for a refusal would make a PyPI outage read as "the lock is wrong". Do not
"normalize" a checker to 1.

## The lock verdict (`lockstep_check.py lock`)

The family is the seven packages the four-package `cargo update -p` moved at SMA-683 M0:
`wasm-bindgen`, `wasm-bindgen-macro`, `wasm-bindgen-macro-support`, `wasm-bindgen-shared`,
`js-sys`, `web-sys` and `wasm-bindgen-futures`. Each refusal has a code. The self-test asserts the
code of each row, so a row cannot pass because a different check refused it.

| Code | The refusal |
|---|---|
| `R-FORMAT` | The lock format version or a top-level table (`[patch]`, `[metadata]`) changed. |
| `R-SHAPE` | A `[[package]]` entry has no string name and version, or a key appears twice. |
| `R-NONFAMILY` | A package outside the family changed, was added or was removed. Cargo writes a dependency reference in three forms (`name`, `name version`, `name version (source)`); a reference to a family package becomes the bare name before the comparison. |
| `R-SEMVER` | A family version is not a strict `X.Y.Z`: no pre-release, no build metadata, no leading zero, ASCII digits only, no trailing newline. |
| `R-TWICE` | A family name has more than one removed or more than one added entry. |
| `R-DUPLICATE` | The new lock holds two versions of one family name side by side. |
| `R-SOURCE` | An added family entry does not come from crates.io. |
| `R-CHECKSUM` | An added family entry has no 64-hex checksum. |
| `R-INPLACE` | A family entry kept its version and changed its checksum. |
| `R-ABSENT` | (`current` only) The lock holds no `wasm-bindgen`. |

No change exits 4. The count of seven is NOT asserted: a family release can drop a package (pass) or
bring a new transitive dependency (`R-NONFAMILY`, so the manual runbook applies).

`artifact` adds `R-LAYOUT`, `R-SYMLINK` and `R-SIZE` (the tree holds `rs/Cargo.lock` and a subset of
the five artifact files, all regular files of 8 MiB or less), `R-NOCHANGE` (the artifact lock equals
the checked-out lock) and `R-TITLE` (the commit title is over 100 characters). It writes the PR title
and body files only after every check passed. `status` (`R-STATUS`) checks `git status --porcelain`
after the copy. `current` checks the family invariant on one lock; the real run of the gate runs it
on `HEAD:rs/Cargo.lock`, so a PR that puts a second `wasm-bindgen` into the lock goes red at once,
not on the next Tuesday.

## The pin rules (`pin_check.py`)

| Rule | The assertion |
|---|---|
| P0 | The file is one YAML mapping, with no duplicate key and no merge key (`<<:`). GitHub Actions does not merge a merge key. |
| P1 | The jobs are exactly `build` and `propose`. |
| P2 | The triggers are exactly `schedule` and `workflow_dispatch` (a bare `on:` parses as `True`; both keys are read). |
| P3 | `build` declares no environment and reads no `secrets` context. The workflow level reads none. `propose` reads it only in the `with:` of the step `token`. |
| P4 | Every `docker run` uses exactly `--rm --user 65534:65534 --volume "$RUNNER_TEMP/work:/work" --workdir /work "$LOCKSTEP_IMAGE"` and runs `bash ci/wasm-lockstep/container.sh update` or `build`. `LOCKSTEP_IMAGE` is a rust image pinned by a sha256 digest. |
| P5 | Every command word of a `run:` script is on the job's allowlist. `python3` runs `lockstep_check.py` only. A `$(...)` is read too. A backtick, a here-document, an arithmetic expansion, `case` and a bare `!` negation are refused. |
| P6 | Every `uses:` is an allowlisted action, pinned to a full 40-hex commit SHA. |
| P7 | The `propose` step ids are exactly `checkout, download, verify, apply, token, commit, base, push, pr, close`, and `verify` runs `lockstep_check.py artifact`. |
| P8 | The workflow holds ONE `git push`, in step `push`, with one `--force-with-lease=refs/heads/deps/wasm-bindgen-lockstep:<sha>`, the App-token remote and the refspec `HEAD:refs/heads/deps/wasm-bindgen-lockstep`. |
| P9 | The `needs` context appears only in `if:` values. `build` outputs only `changed`. |
| P10 | The top-level and each job's `permissions` are exactly `contents: read`. |
| P11 | `propose` has `environment: release-pr`, `needs: build` and `if: needs.build.result == 'success'`. |
| P12 | No job declares `container`, `services`, `uses`, `secrets` or `defaults`. |
| P13 | No step sets a shell other than `bash`, and the workflow has no `defaults:`. |
| P14 | No step sets `continue-on-error` to anything but false. |
| P15 | No step `if:` calls `always()`, `failure()` or `cancelled()`. |
| P16 | The token step uses `actions/create-github-app-token` with exactly `client-id`, `private-key`, `permission-contents: write` and `permission-pull-requests: write`. |
| P17 | Every checkout sets `persist-credentials: false`. |

The P6 rule pins the action NAME and a full SHA, not one specific SHA, so a dependabot action bump
does not turn this gate red. A new action, or a tag in place of a SHA, does.

## The trust model

- **`build`** has no environment and reads no secret. Its `GITHUB_TOKEN` is `contents: read`. Every
  step that runs third-party code (cargo, build scripts, proc-macros, `wasm-pack`,
  `wasm-bindgen-cli`, pnpm packages) runs inside `docker run`, as the user `nobody`, over a copy of
  the tree without `.git`. The container gets no runner environment: no `CI`, no `GITHUB_ACTIONS`,
  no `ACTIONS_RUNTIME_TOKEN`, no `GITHUB_TOKEN`, no `GITHUB_OUTPUT`. So that code cannot write a
  cache entry in the `refs/heads/main` scope or an artifact (spec F11, AC3).
- After the compile, the host runs no file of the work copy and no `git` command on it. It checks
  each path component for a symlink, then copies the six files with `cp`.
- The host writes `changed` BEFORE the compile, so third-party code cannot set it.
- **`propose`** holds the App key for the whole job (environment `release-pr`, main-only branch
  policy). The control is that no step executes artifact content: no toolchain, no cargo, no pnpm,
  no moon, and no script from the artifact. A refusal fails the `verify` step, so the token step
  never runs.
- **The output rule.** `propose` reads `needs.build.outputs.changed` only in `if:` values. Every
  value that reaches a command (the versions, the PR title and body) comes from `lockstep_check.py`
  on the downloaded bytes, through a file or `env:`.
- **Why the user `nobody`.** `generate-wasm --post` calls Node's `os.userInfo()`, which throws when
  the uid has no passwd entry. The image has no entry for the runner's uid 1001. `nobody` (65534)
  has one. The identity markers that `--post` then rejects are `nobody` and `lockstep-home`.

## What the checks do not prove

- The artifacts come from a build that ran third-party code. The checks prove paths and the lock
  shape, not content. A person reads the lock diff, not the five files.
- Nothing enforces the review. The `main` ruleset has no `pull_request` rule. The PR's own `CI`
  runs the unreviewed code before any review, in the PR cache scope, with no secrets: the same
  exposure as a dependabot PR.
- A container escape on the hosted runner can still reach the `main` cache scope. Sven accepted
  this residual (SMA-693 Q7).
- `pin_check.py` reads command words, not shell semantics. It does not follow `git -c` aliases, and
  it reads a quoted `$(...)` by bracket depth only.
- A refusal stays red every week until a person runs the manual runbook in `rs/CLAUDE.md`
  ("The wasm-bindgen family does not move through dependabot"). Sven watches the runs (Q10).

## Local runs

`run.sh` runs under `/bin/bash` 3.2.57 and Homebrew bash 5. It has no `mapfile`, no `declare -A`, no
here-string and no pipe into an early-exit reader. It needs `uv` on `PATH` (the proto shims).

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
./ci/wasm-lockstep/run.sh --self-test
./ci/wasm-lockstep/run.sh --negative-control
./ci/wasm-lockstep/run.sh
```

## Measurements

### M2 — the local container pre-check

Not measured yet.

### M0 — the scratch-branch run on a GitHub runner

Not measured yet.

### M3 — the first `workflow_dispatch` run on `main`

Not measured yet. This happens after the merge.
````

- [ ] **Step 4: Add the Moon task**

Append to `moon.yml`, after the last line of `moon-diagnosis-exec` (`      - 'ci/moon-diagnosis/**/*'`):

```yaml

  wasm-lockstep:
    description: 'Check the wasm-lockstep workflow against its trust model, and the family invariant on HEAD rs/Cargo.lock (SMA-693).'
    # WHY THIS EXISTS — .github/workflows/wasm-lockstep.yml runs third-party build code in one job
    # and holds the App key in the other. pin_check.py asserts the rules that keep the two apart
    # (no secret in build, the docker form, the command-word allowlists, the step order, the push
    # refspec, the output rule). lockstep_check.py is the checker both jobs run; its self-test and
    # control run here, and its `current` verdict on HEAD's lock reds a PR that breaks the family
    # invariant before the next scheduled run does (ci/wasm-lockstep/README.md).
    #
    # INPUTS. The workflow is what pin_check.py reads. rs/Cargo.lock is what the real run reads
    # through `git show HEAD:` (spec F15: never the working tree). .prototools pins the `uv` the
    # gate shells out to. SELF_TASK_EXPECTED_GLOBS pins these four.
    #
    # `--self-test` and `--negative-control` run FIRST and in the SAME block. `set -euo pipefail`
    # is REQUIRED: Moon takes a `script:` block's status from its LAST command, so without it a
    # failing control is masked by the passing real run. SELF_SCHEDULED_GATES pins these four lines.
    script: |
      set -euo pipefail
      bash ci/wasm-lockstep/run.sh --self-test
      bash ci/wasm-lockstep/run.sh --negative-control
      bash ci/wasm-lockstep/run.sh
    toolchain: 'system'
    inputs:
      - 'ci/wasm-lockstep/**/*'
      - '.github/workflows/wasm-lockstep.yml'
      - 'rs/Cargo.lock'
      - '.prototools'
```

- [ ] **Step 5: Run the gate under both bash versions, and through Moon**

```bash
git add ci/wasm-lockstep/run.sh ci/wasm-lockstep/README.md moon.yml
for shim in bash32 bash5; do
  for mode in --self-test --negative-control ""; do
    rc=0; PATH="$SCRATCH/$shim:$PATH" ./ci/wasm-lockstep/run.sh $mode > "$SCRATCH/run-$shim$mode.log" 2>&1 || rc=$?
    echo "$shim ${mode:-real} rc=$rc"
  done
done
PATH="$SCRATCH/bash32:$PATH" moon run repo:wasm-lockstep --force; echo "moon=$?"
PATH="$SCRATCH/bash32:$PATH" ./ci/wasm-lockstep/run.sh --bogus; echo "bogus=$?"
```

Expected: six lines `rc=0` (bash32 and bash5, three modes each); `moon=0`; `wasm-lockstep: infrastructure error (rc=2): unknown flag: --bogus` and `bogus=2`. The self-test log holds `== wasm-lockstep self-test passed ==`; the control log holds `== wasm-lockstep negative control passed ==`; the real log holds `PASS  [pin_check …]` and `PASS  [lockstep_check current on HEAD:rs/Cargo.lock]`.

- [ ] **Step 6: Commit**

```bash
git commit -m "feat(ci): add the repo:wasm-lockstep gate" -m "run.sh runs both checkers in the self-test, the negative control and the real run, and
maps a checker refusal of 3 to 1 and every other non-zero status to 2. It runs under bash
3.2 and bash 5." -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

Note: until Task 5, `repo:affected-smoke` is red on this branch (a `repo:*` task with a `--self-test` and no registry entry). Task 5 Step 1 uses that red as its failing test.

- [ ] **Step 7: Prove that the wrapper's mapping can fail (red-first, on a mutant copy)**

The mutant sits next to the real script, so its `REPO_ROOT` is right. `rm` removes it; the committed `run.sh` is never edited.

```bash
sed 's/case "$1" in 0) echo 0 ;; 3) echo 1 ;; \*) echo 2 ;; esac/case "$1" in 0) echo 0 ;; *) echo 2 ;; esac/' ci/wasm-lockstep/run.sh > ci/wasm-lockstep/run-mutant.sh
chmod +x ci/wasm-lockstep/run-mutant.sh
PATH="$SCRATCH/bash32:$PATH" ./ci/wasm-lockstep/run-mutant.sh --self-test > "$SCRATCH/mutant.log" 2>&1; echo "rc=$?"
grep -F 'FAIL  [gate_rc 3]' "$SCRATCH/mutant.log"
rm ci/wasm-lockstep/run-mutant.sh
git status --short ci/
```

Expected: `rc=1`; the line `FAIL  [gate_rc 3]: got 2, want 1`; `git status` prints nothing for `ci/`.

---

### Task 5: Register the gate (the seven obligations)

**Files:**
- Modify: `.github/workflows/ci.yml:269` (the `T=(…)` array)
- Modify: `CLAUDE.md:146` (the `ci-targets` marker block)
- Modify: `ci/affected-graph/ci_targets.py` (lines 186-223 `REQUIRED_REPO_TASKS`; 244-440 `SELF_TASK_EXPECTED_GLOBS`; 537-727 `SELF_SCHEDULED_GATES`; after 1594-1624 `NEXT_ENV_SH_CALL_SITES` a new `WASM_LOCKSTEP_SH_CALL_SITES`; 1994-2127 `check_self_invocation`; `self_test` at 2353 (fixtures 2485-2508, haystacks 2975-2977, the 70 calls, the parameter loop 3299-3303, the battery after 3740); 4160-4165 `_CALL_SITE_SOURCE_KEYS`; 4191-4195 the call in `collect_findings`; the report text near 4255-4277; `main` near 4445-4475)
- Modify: `ci/actionlint/run.sh:2168-2171` (`T_AFFECTED_SMOKE_REQUIRED_INPUTS`)
- Modify: `moon.yml:272-277` (`repo:affected-smoke` inputs)

**Interfaces:**
- Consumes: Task 4's `run.sh` lines (byte-identical) and the Moon task `repo:wasm-lockstep`.
- Produces: `WASM_LOCKSTEP_SH_CALL_SITES: tuple[str, ...]` (31 lines); `check_self_invocation(…, next_env_sh_text, wasm_lockstep_sh_text)` (12 required positional parameters); `_CALL_SITE_SOURCE_KEYS` gains `"wasm_lockstep"`.

- [ ] **Step 1: Run the coverage check and see it fail**

The Moon task from Task 4 runs `--self-test` and `--negative-control`, but no registry names it yet.

```bash
python3 ci/affected-graph/ci_targets.py; echo "rc=$?"
```

Expected: `rc=1`, and `:wasm-lockstep` appears in at least two findings: "A CI-eligible `repo:*` task is NOT in ci.yml's `T=(...)` array" and "A `repo:*` task's own resolved script runs `--self-test` or `--negative-control`, but it has no SELF_SCHEDULED_GATES entry". If neither row names it, the premise is wrong: stop and report.

- [ ] **Step 2: Add `:wasm-lockstep` to `T` and to the CLAUDE.md mirror**

In `.github/workflows/ci.yml` line 269, replace ` :moon-diagnosis-exec :test-e2e)` with ` :moon-diagnosis-exec :wasm-lockstep :test-e2e)`.

In `CLAUDE.md` line 146, replace `  :next-public-free :helm-render :moon-diagnosis-exec :test-e2e` with `  :next-public-free :helm-render :moon-diagnosis-exec :wasm-lockstep :test-e2e`. Do not touch the two marker lines, and do not copy them anywhere.

- [ ] **Step 3: Floor the reachability pair**

In `moon.yml` (`repo:affected-smoke` inputs), replace:

```yaml
      - 'ci/next-env/**/*'
      # SMA-541 — this task now asserts that CLAUDE.md's documented full-graph command mirrors
```

with:

```yaml
      - 'ci/next-env/**/*'
      # SMA-693 — this task pins load-bearing lines inside ci/wasm-lockstep/run.sh
      # (WASM_LOCKSTEP_SH_CALL_SITES), so a change under ci/wasm-lockstep/ MUST re-key it. The
      # broad 'ci/**/*' entry already covers it for SCHEDULING; the narrow entry is kept because
      # check 8e in ci/actionlint/run.sh floors this array's length and the file's own policy is to
      # keep narrow globs rather than collapse them into the broad one.
      - 'ci/wasm-lockstep/**/*'
      # SMA-541 — this task now asserts that CLAUDE.md's documented full-graph command mirrors
```

In `ci/actionlint/run.sh`, replace:

```bash
  'ci/next-env/**/*'
  'CLAUDE.md'
  '.prototools'
)
```

with:

```bash
  'ci/next-env/**/*'
  # SMA-693 — floors the input that makes WASM_LOCKSTEP_SH_CALL_SITES reachable. Without it, a PR
  # editing ci/wasm-lockstep/** does not schedule repo:affected-smoke, and neither the
  # SELF_SCHEDULED_GATES nor the SELF_TASK_EXPECTED_GLOBS pin for that gate can fire.
  'ci/wasm-lockstep/**/*'
  'CLAUDE.md'
  '.prototools'
)
```

- [ ] **Step 4: Edit `ci_targets.py` — the registries, the table, the signature and the fixtures**

Save this tool as `$SCRATCH/ci_targets_edits_1.py` (not committed). Each replacement asserts that its old text occurs exactly once, so a drifted file stops the tool instead of a silent half-edit.

```python
import sys
p = sys.argv[1]
s = open(p, encoding="utf-8").read()
def rep(old, new):
    global s
    assert s.count(old) == 1, (s.count(old), old[:80])
    s = s.replace(old, new)

# 1 REQUIRED_REPO_TASKS
rep('''    "next-env-drift",
)

# SMA-553 D13''', '''    "next-env-drift",
    # SMA-693. Same reasoning as the entries above: repo:wasm-lockstep carries a
    # --negative-control, and check_forward's `want`/`got` shrink CONSISTENTLY when a task is
    # dropped from `T` and made CI-ineligible in the same edit — so without a floor entry the whole
    # gate, control included, could be switched off with every check green.
    "wasm-lockstep",
)

# SMA-553 D13''')
# 2 SELF_TASK_EXPECTED_GLOBS
rep('''        "ts/apps/*/tsconfig.json",
        "ts/pnpm-lock.yaml",
    ),
}''', '''        "ts/apps/*/tsconfig.json",
        "ts/pnpm-lock.yaml",
    ),
    # SMA-693. One glob then three literals, in check_gate_inputs' comparison order (globs sorted,
    # then files sorted). The workflow is what pin_check.py reads, rs/Cargo.lock is what the real
    # run reads through `git show HEAD:`, and .prototools pins the `uv` the gate shells out to.
    # Drop any one and the gate serves a cached PASS on the exact edit it exists to catch.
    "wasm-lockstep": (
        "ci/wasm-lockstep/**/*",
        ".github/workflows/wasm-lockstep.yml",
        ".prototools",
        "rs/Cargo.lock",
    ),
}''')
# 3 SELF_SCHEDULED_GATES
rep('''    "next-env-drift": (
        "set -euo pipefail",
        "bash ci/next-env/run.sh --negative-control",
        "bash ci/next-env/run.sh",
    ),
}''', '''    "next-env-drift": (
        "set -euo pipefail",
        "bash ci/next-env/run.sh --negative-control",
        "bash ci/next-env/run.sh",
    ),
    # SMA-693. Four lines, like the other self-scheduled gates. Whole-line matched:
    # `bash ci/wasm-lockstep/run.sh` is a strict PREFIX of both flagged lines.
    "wasm-lockstep": (
        "set -euo pipefail",
        "bash ci/wasm-lockstep/run.sh --self-test",
        "bash ci/wasm-lockstep/run.sh --negative-control",
        "bash ci/wasm-lockstep/run.sh",
    ),
}''')
# 4 the table
rep('''    'exit "$rc"',
)


def read_input(path, label):''', '''    'exit "$rc"',
)

# SMA-693 — ci/wasm-lockstep/run.sh's load-bearing lines. REACHABILITY IS NOT AUTOMATIC: this check
# only runs when repo:affected-smoke is scheduled, so moon.yml lists `ci/wasm-lockstep/**/*` among
# its inputs and ci/actionlint/run.sh's T_AFFECTED_SMOKE_REQUIRED_INPUTS floors that entry.
#
# Matched as stripped WHOLE LINES, like the next-env and moon-diagnosis haystacks and for both of
# their reasons. Every entry occurs EXACTLY ONCE in run.sh.
#
# The pins hold the proto reporter export, the flag parse and the dispatch arms, the HEAD lock
# read (spec F15), the 3 -> 1 mapping and the line that routes a real child status, and for each
# mode every checker run, its rc aggregation, the failure guard and the exit. The pins hold text;
# the self-test and the control are what prove behaviour.
WASM_LOCKSTEP_SH_CALL_SITES = (
    # The proto reporter, the flag parse and the dispatch arms.
    'export PROTO_REPORTER=text',
    '--self-test)        MODE=selftest; shift ;;',
    '--negative-control) MODE=negctl;   shift ;;',
    'selftest) self_test ;;',
    'negctl)   negative_control ;;',
    'check)    real_run ;;',
    # The lock comes from HEAD, never from the working tree.
    'git -C "$REPO_ROOT" show HEAD:rs/Cargo.lock >"$TMP/head.lock" || die_infra "git show HEAD:rs/Cargo.lock failed"',
    # The 3 -> 1 mapping, and the routing of the real child status.
    'case "$1" in 0) echo 0 ;; 3) echo 1 ;; *) echo 2 ;; esac',
    '"$PY" "$@" || raw=$?',
    'return "$(gate_rc "$raw")"',
    # The real run: both checkers, the aggregation and the exit.
    'pin_rc=0; run_checker "$HERE/pin_check.py" "$WORKFLOW" || pin_rc=$?',
    'if [ "$pin_rc" -gt "$worst" ]; then worst="$pin_rc"; fi',
    'lock_rc=0; run_checker "$HERE/lockstep_check.py" current --lock "$TMP/head.lock" || lock_rc=$?',
    'if [ "$lock_rc" -gt "$worst" ]; then worst="$lock_rc"; fi',
    'exit "$worst"',
    # The self-test: both fixture tables, the real child-status row, the guard and the exit.
    'lst_rc=0; run_checker "$HERE/lockstep_check.py" --self-test || lst_rc=$?',
    'if [ "$lst_rc" -gt "$st_worst" ]; then st_worst="$lst_rc"; fi',
    'pst_rc=0; run_checker "$HERE/pin_check.py" --self-test || pst_rc=$?',
    'if [ "$pst_rc" -gt "$st_worst" ]; then st_worst="$pst_rc"; fi',
    'map_rc=0; run_checker -c "raise SystemExit($raw)" || map_rc=$?',
    'if [ "$st_worst" -ne 0 ]; then',
    'exit "$st_worst"',
    # The control: both controls, the real refusal and its two guards, the guard and the exit.
    'lnc_rc=0; run_checker "$HERE/lockstep_check.py" --negative-control --lock "$TMP/head.lock" || lnc_rc=$?',
    'if [ "$lnc_rc" -gt "$nc_worst" ]; then nc_worst="$lnc_rc"; fi',
    'pnc_rc=0; run_checker "$HERE/pin_check.py" --negative-control "$WORKFLOW" || pnc_rc=$?',
    'if [ "$pnc_rc" -gt "$nc_worst" ]; then nc_worst="$pnc_rc"; fi',
    'nc_map_rc=0; run_checker "$HERE/lockstep_check.py" lock --old "$TMP/head.lock" --new "$TMP/mutated.lock" >"$TMP/map.out" 2>&1 || nc_map_rc=$?',
    'if [ "$nc_map_rc" -ne 1 ]; then',
    "if ! grep -qF -- 'R-NONFAMILY' \\"$TMP/map.out\\"; then",
    'if [ "$nc_worst" -ne 0 ]; then',
    'exit "$nc_worst"',
)


def read_input(path, label):''')
# 5 signature + docstring
rep('''    next_public_free_sh_text, helm_render_sh_text, moon_diagnosis_sh_text, next_env_sh_text,
):''', '''    next_public_free_sh_text, helm_render_sh_text, moon_diagnosis_sh_text, next_env_sh_text,
    wasm_lockstep_sh_text,
):''')
rep('''    release-plan, ruff, next-public-free, helm-render, moon-diagnosis-exec and next-env-drift gates missing from''',
    '''    release-plan, ruff, next-public-free, helm-render, moon-diagnosis-exec, next-env-drift and wasm-lockstep gates missing from''')
rep('''`moon_diagnosis_sh_text` and `next_env_sh_text` are REQUIRED''', '''`moon_diagnosis_sh_text`, `next_env_sh_text` and `wasm_lockstep_sh_text` are REQUIRED''')
rep('''        for site in NEXT_ENV_SH_CALL_SITES
        if site not in next_env_lines
    )
    return missing''', '''        for site in NEXT_ENV_SH_CALL_SITES
        if site not in next_env_lines
    )
    # SMA-693 — stripped whole lines, like the next-env haystack and for the same two reasons.
    wasm_lockstep_lines = {line.strip() for line in wasm_lockstep_sh_text.splitlines()}
    missing.extend(
        f"ci/wasm-lockstep/run.sh: {site}"
        for site in WASM_LOCKSTEP_SH_CALL_SITES
        if site not in wasm_lockstep_lines
    )
    return missing''')
# 6 self_test fixtures
rep('''                 # SMA-637 — a floor member too, for the same reason.
                 "next-env-drift": True},''', '''                 # SMA-637 — a floor member too, for the same reason.
                 "next-env-drift": True,
                 # SMA-693 — a floor member too, for the same reason.
                 "wasm-lockstep": True},''')
rep('''                 "moon-diagnosis-exec", "next-env-drift"]''', '''                 "moon-diagnosis-exec", "next-env-drift", "wasm-lockstep"]''')
rep('''    wired_next_env = "".join(
        f"    {site}\\n" for site in NEXT_ENV_SH_CALL_SITES
    )
''', '''    wired_next_env = "".join(
        f"    {site}\\n" for site in NEXT_ENV_SH_CALL_SITES
    )
    # SMA-693 — the same shape for ci/wasm-lockstep/run.sh, derived from the registry.
    wired_wasm_lockstep = "".join(
        f"    {site}\\n" for site in WASM_LOCKSTEP_SH_CALL_SITES
    )
''')
open(p, "w", encoding="utf-8").write(s)
print("edits applied")
```

```bash
python3 "$SCRATCH/ci_targets_edits_1.py" ci/affected-graph/ci_targets.py
```

Expected: `edits applied`.

- [ ] **Step 5: Thread the new haystack through every `check_self_invocation` call in `self_test`**

`self_test` holds 70 calls with 11 positional arguments each. Save this tool as `$SCRATCH/thread_wasm_lockstep.py` (not committed). It appends `, wired_wasm_lockstep` after the LAST argument of each call, found by the AST, and refuses to run twice.

```python
# Appends `, wired_wasm_lockstep` as the LAST positional argument of every check_self_invocation
# call inside self_test(). A one-shot edit tool for SMA-693 Task 6; not committed.
import ast
import sys

path = sys.argv[1]
text = open(path, encoding="utf-8").read()
tree = ast.parse(text)
self_test = next(n for n in tree.body if isinstance(n, ast.FunctionDef) and n.name == "self_test")
calls = [n for n in ast.walk(self_test)
         if isinstance(n, ast.Call) and isinstance(n.func, ast.Name) and n.func.id == "check_self_invocation"]
bad = [n.lineno for n in calls if len(n.args) != 11 or n.keywords]
if bad:
    sys.exit(f"calls without exactly 11 positional arguments at lines {bad}; was the script run twice?")
lines = text.splitlines(keepends=True)
offsets = [0]
for line in lines:
    offsets.append(offsets[-1] + len(line.encode("utf-8")))
data = text.encode("utf-8")
points = sorted((offsets[c.args[-1].end_lineno - 1] + c.args[-1].end_col_offset for c in calls), reverse=True)
for point in points:
    data = data[:point] + b", wired_wasm_lockstep" + data[point:]
open(path, "w", encoding="utf-8").write(data.decode("utf-8"))
print(f"threaded {len(calls)} calls")
```

```bash
python3 "$SCRATCH/thread_wasm_lockstep.py" ci/affected-graph/ci_targets.py
grep -c "wired_wasm_lockstep" ci/affected-graph/ci_targets.py
```

Expected: `threaded 70 calls`; the count `71` (70 arguments plus the definition from Step 4). Python 3.12 or later is required (the f-string positions of PEP 701); the proto `python3` is 3.14.

- [ ] **Step 6: Edit `ci_targets.py` — the battery, the source keys, the report and `main`**

Save this tool as `$SCRATCH/ci_targets_edits_2.py` (not committed):

```python
import sys
p = sys.argv[1]
s = open(p, encoding="utf-8").read()
def rep(old, new):
    global s
    assert s.count(old) == 1, (s.count(old), old[:80])
    s = s.replace(old, new)

rep('''        "helm_render_sh_text", "moon_diagnosis_sh_text", "next_env_sh_text",
    ):''', '''        "helm_render_sh_text", "moon_diagnosis_sh_text", "next_env_sh_text",
        "wasm_lockstep_sh_text",
    ):''')
rep('''        failures.append(
            "check_self_invocation: a NEUTERED next-env comparison (always-false guard) "
            "satisfied the pin"
        )
''', '''        failures.append(
            "check_self_invocation: a NEUTERED next-env comparison (always-false guard) "
            "satisfied the pin"
        )

    # SMA-693 — the next-env battery above, repeated for the wasm-lockstep haystack: a deletion
    # row per pinned line, a contamination row, a commented-out row and a disabled-guard row.
    for _wl_site in WASM_LOCKSTEP_SH_CALL_SITES:
        _wl_broken = "".join(
            line for line in wired_wasm_lockstep.splitlines(keepends=True)
            if line.strip() != _wl_site
        )
        if not check_self_invocation(
            wired, scripts, wired_actionlint, wired_release_parity, wired_workflow_credentials,
            wired_release_plan, wired_ruff, wired_next_public_free, wired_helm_render,
            wired_moon_diagnosis, wired_next_env, _wl_broken,
        ):
            failures.append(
                f"check_self_invocation: missed {_wl_site!r} deleted from ci/wasm-lockstep/run.sh"
            )
    # Contamination: a wasm-lockstep site must not be satisfiable from another haystack.
    if not check_self_invocation(
        wired + wired_wasm_lockstep, scripts, wired_actionlint, wired_release_parity,
        wired_workflow_credentials, wired_release_plan, wired_ruff, wired_next_public_free,
        wired_helm_render, wired_moon_diagnosis, wired_next_env + wired_wasm_lockstep,
        "".join(line for line in wired_wasm_lockstep.splitlines(keepends=True)
                if line.strip() != WASM_LOCKSTEP_SH_CALL_SITES[0]),
    ):
        failures.append(
            "check_self_invocation: a wasm-lockstep site was satisfied by another haystack's text"
        )
    # Whole-LINE, not substring: a commented-out copy of a pinned line must report missing.
    _wl_commented = wired_wasm_lockstep.replace(
        "negctl)   negative_control ;;\\n", "# negctl)   negative_control ;;\\n"
    )
    if not check_self_invocation(
        wired, scripts, wired_actionlint, wired_release_parity, wired_workflow_credentials,
        wired_release_plan, wired_ruff, wired_next_public_free, wired_helm_render,
        wired_moon_diagnosis, wired_next_env, _wl_commented,
    ):
        failures.append(
            "check_self_invocation: a COMMENTED-OUT wasm-lockstep line satisfied the pin "
            "(widened to substring matching)"
        )
    # The disabled guard. The control's rc comparison is what makes the real refusal prove the
    # 3 -> 1 mapping; comparing nc_map_rc with ITSELF leaves every other pinned line
    # byte-identical, so the control would pass whatever rc the wrapper gave.
    _wl_guard_neutered = wired_wasm_lockstep.replace(
        'if [ "$nc_map_rc" -ne 1 ]; then\\n', 'if [ "$nc_map_rc" -ne "$nc_map_rc" ]; then\\n'
    )
    if not check_self_invocation(
        wired, scripts, wired_actionlint, wired_release_parity, wired_workflow_credentials,
        wired_release_plan, wired_ruff, wired_next_public_free, wired_helm_render,
        wired_moon_diagnosis, wired_next_env, _wl_guard_neutered,
    ):
        failures.append(
            "check_self_invocation: a NEUTERED wasm-lockstep comparison (always-false guard) "
            "satisfied the pin"
        )
''')
rep('''# The ten shell sources check_self_invocation reads, keyed so collect_findings' signature does
# not grow eight positional parameters that a caller could silently transpose.
_CALL_SITE_SOURCE_KEYS = (
    "run", "actionlint", "release_parity", "workflow_credentials", "release_plan", "ruff",
    "next_public_free", "helm_render", "moon_diagnosis", "next_env",
)''', '''# The eleven shell sources check_self_invocation reads, keyed so collect_findings' signature does
# not grow positional parameters that a caller could silently transpose.
_CALL_SITE_SOURCE_KEYS = (
    "run", "actionlint", "release_parity", "workflow_credentials", "release_plan", "ruff",
    "next_public_free", "helm_render", "moon_diagnosis", "next_env", "wasm_lockstep",
)''')
rep('''        sh["moon_diagnosis"], sh["next_env"],
    )''', '''        sh["moon_diagnosis"], sh["next_env"], sh["wasm_lockstep"],
    )''')
rep('''         "    RUFF_SH_CALL_SITES, HELM_RENDER_SH_CALL_SITES, MOON_DIAGNOSIS_SH_CALL_SITES and\\n"
         "    NEXT_ENV_SH_CALL_SITES in\\n"''', '''         "    RUFF_SH_CALL_SITES, HELM_RENDER_SH_CALL_SITES, MOON_DIAGNOSIS_SH_CALL_SITES,\\n"
         "    NEXT_ENV_SH_CALL_SITES and WASM_LOCKSTEP_SH_CALL_SITES in\\n"''')
rep('''         "    the aggregation or the final exit). The control can then pass while it proves\\n"
         "    nothing, or the real run can exit 0 on drift.\\n"''', '''         "    the aggregation or the final exit). The control can then pass while it proves\\n"
         "    nothing, or the real run can exit 0 on drift.\\n"
         "    A row prefixed `ci/wasm-lockstep/run.sh:` means one of the pinned lines of that\\n"
         "    gate is gone: the flag parse or a dispatch arm, the HEAD lock read, the 3 -> 1\\n"
         "    mapping, a checker run or its rc aggregation, the real refusal of the control or\\n"
         "    one of its guards, or an exit. A mode can then exit 0 while it asserts nothing.\\n"''')
rep('''        next_env_sh = read_input(
            root / "ci" / "next-env" / "run.sh", "ci/next-env/run.sh"
        )
''', '''        next_env_sh = read_input(
            root / "ci" / "next-env" / "run.sh", "ci/next-env/run.sh"
        )
        wasm_lockstep_sh = read_input(
            root / "ci" / "wasm-lockstep" / "run.sh", "ci/wasm-lockstep/run.sh"
        )
''')
rep('''                "next_env": next_env_sh,
            },''', '''                "next_env": next_env_sh,
                "wasm_lockstep": wasm_lockstep_sh,
            },''')
open(p, "w", encoding="utf-8").write(s)
print("edits2 applied")
```

```bash
python3 "$SCRATCH/ci_targets_edits_2.py" ci/affected-graph/ci_targets.py
```

Expected: `edits2 applied`.

- [ ] **Step 7: Run the registry checks and see them pass**

```bash
python3 ci/affected-graph/ci_targets.py --self-test; echo "selftest=$?"
python3 ci/affected-graph/ci_targets.py; echo "real=$?"
python3 ci/affected-graph/task_inputs.py; echo "liveness=$?"
uv run --locked --project py ruff check --config py/pyproject.toml ci/affected-graph/ci_targets.py; echo "ruff=$?"
PATH="$SCRATCH/bash32:$PATH" moon run repo:affected-smoke --force; echo "smoke=$?"
```

Expected: `ci-targets self-test OK`, `selftest=0`; `real=0`; `liveness=0`; `ruff=0`; `smoke=0` (bash 3.2 through the shim, because `repo:affected-smoke` deadlocks under Homebrew bash on this Mac). If `smoke` fails with `JSONDecodeError: Extra data` or a `proto-shim` EACCES line, it is a known host flake: keep the output, then re-run with `--force` once.

- [ ] **Step 8: Prove that the pins bite (red-first on the real run.sh)**

`run.sh` is committed (Task 4), so a mutation of the real file is safe to restore with `git checkout`.

```bash
grep -n 'negctl)   negative_control ;;' ci/wasm-lockstep/run.sh
sed -i '' '/negctl)   negative_control ;;/d' ci/wasm-lockstep/run.sh
python3 ci/affected-graph/ci_targets.py > "$SCRATCH/pin-red.log" 2>&1; echo "rc=$?"
grep -F 'ci/wasm-lockstep/run.sh: negctl)   negative_control ;;' "$SCRATCH/pin-red.log"
git checkout -- ci/wasm-lockstep/run.sh
git diff --quiet -- ci/wasm-lockstep/run.sh && echo "restored"
```

Expected: one line number; `rc=1`; the missing-site row; `restored`.

- [ ] **Step 9: Run the actionlint half of the floor**

```bash
PATH="$SCRATCH/bash5:$PATH" ./ci/actionlint/run.sh > "$SCRATCH/actionlint.log" 2>&1; echo "rc=$?"
grep -F 'pipe capacity' "$SCRATCH/actionlint.log"
```

Expected: the preflight line `pipe capacity 65536 bytes (floor 8192)` and `rc=0`. If the preflight reports 512 bytes, the gate exits 2 in seconds: that is a host condition (SMA-612), not a verdict. Then run the gate in a Linux container (memory note "Small pipe: run gates in a Linux container") or leave the verdict to CI, and say which one you did.

- [ ] **Step 10: Commit**

```bash
git add .github/workflows/ci.yml CLAUDE.md ci/affected-graph/ci_targets.py ci/actionlint/run.sh moon.yml
git commit -m "feat(ci): register repo:wasm-lockstep with its seven obligations" -m "The gate joins the T array and its CLAUDE.md mirror, SELF_SCHEDULED_GATES,
SELF_TASK_EXPECTED_GLOBS, REQUIRED_REPO_TASKS, a WASM_LOCKSTEP_SH_CALL_SITES pin of 31
run.sh lines, and the affected-smoke input with its actionlint floor." -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: M2 — the local container pre-check

M2 runs container runs 1 and 2 locally, in the same image, as the same user, with the same `HOME` as the workflow. It finds a `--post` failure before M0. M0 is the proof; M2 is a fast pre-check (spec section 8).

**Files:**
- Modify: `ci/wasm-lockstep/README.md` (the `### M2` section)
- Modify (only if M2 fails): `ci/wasm-lockstep/container.sh`

**Interfaces:**
- Consumes: `container.sh update|build` (Task 3), `lockstep_check.py lock|artifact` (Task 1), the committed tree (`git archive HEAD` copies only committed files).
- Produces: the M2 table in the README.

- [ ] **Step 1: Make the work copy and run container run 1**

```bash
IMG="docker.io/library/rust:1.95.0-bookworm@sha256:6258907abe69656e41cd992e0b705cdcfabcbbe3db374f92ed2d47121282d4a1"
rm -rf "$SCRATCH/m2" && mkdir -p "$SCRATCH/m2/work" "$SCRATCH/m2/stage"
git archive HEAD | tar -x -C "$SCRATCH/m2/work"
git show HEAD:rs/Cargo.lock > "$SCRATCH/m2/old.lock"
chmod -R a+rwX "$SCRATCH/m2/work"
docker run --rm --platform linux/amd64 --user 65534:65534 --volume "$SCRATCH/m2/work:/work" --workdir /work "$IMG" bash ci/wasm-lockstep/container.sh update; echo "update=$?"
```

Expected: cargo prints `Locking N packages to latest compatible versions` with the family names; `update=0`. `--platform linux/amd64` matches the runner; if Docker Desktop has no amd64 emulation, drop the flag and record "arm64" in the table.

- [ ] **Step 2: Run the lock verdict on the host**

```bash
PY=ci/wasm-lockstep/.venv/bin/python3
"$PY" ci/wasm-lockstep/lockstep_check.py lock --old "$SCRATCH/m2/old.lock" --new "$SCRATCH/m2/work/rs/Cargo.lock"; echo "lock=$?"
```

Expected: `lock=0` with `family-moved wasm-bindgen 0.2.128 0.2.<z>` lines (F16: 0.2.129 is published). Record the lines. A `lock=3` is a real refusal (for example a new transitive dependency): record the code and the message, and go on with Step 3, because M2 measures the build, not the lock.

- [ ] **Step 3: Run container run 2 (longer than 10 minutes)**

Start this with the Bash tool's `run_in_background: true`. Then wait until the log holds `M2-EXIT`. Do not end your turn while it runs.

```bash
IMG="docker.io/library/rust:1.95.0-bookworm@sha256:6258907abe69656e41cd992e0b705cdcfabcbbe3db374f92ed2d47121282d4a1"
start="$(date -u +%s)"
rc=0; docker run --rm --platform linux/amd64 --user 65534:65534 --volume "$SCRATCH/m2/work:/work" --workdir /work "$IMG" bash ci/wasm-lockstep/container.sh build > "$SCRATCH/m2/build.log" 2>&1 || rc=$?
echo "M2-EXIT $rc seconds=$(( $(date -u +%s) - start ))" >> "$SCRATCH/m2/build.log"
```

Expected: the log ends with `generate-wasm: wrote 5 files into rs/crates/bindings/paigasus-wasm/` and `M2-EXIT 0 seconds=<n>`.

If it fails, read the log. Known candidates: the flags of the proto install script (`--yes`, `--no-profile`); Moon refusing a repository without a commit; the `--post` identity guard. Fix `container.sh`, commit the fix, and run M2 again from Step 1. NEVER weaken the `--post` guard and NEVER add `-e`, `--env` or a second volume to `docker run`.

- [ ] **Step 4: Probe the passwd entries and the container environment**

```bash
IMG="docker.io/library/rust:1.95.0-bookworm@sha256:6258907abe69656e41cd992e0b705cdcfabcbbe3db374f92ed2d47121282d4a1"
docker run --rm --platform linux/amd64 "$IMG" getent passwd 1001; echo "uid1001=$?"
docker run --rm --platform linux/amd64 "$IMG" getent passwd 65534; echo "uid65534=$?"
docker run --rm --platform linux/amd64 --user 65534:65534 "$IMG" sh -c 'env | sort'
```

Expected: `uid1001=2` (no entry: the reason the container does not run as the runner's uid); a `nobody:x:65534:65534:…` line and `uid65534=0`; an environment with only the image's variables (`PATH`, `RUSTUP_HOME`, `CARGO_HOME`, `RUST_VERSION`, `HOSTNAME`, `HOME`), and no `CI`, no `GITHUB_*`, no `ACTIONS_*`.

- [ ] **Step 5: Stage the six files and run the propose checker on them**

```bash
PY=ci/wasm-lockstep/.venv/bin/python3
mkdir -p "$SCRATCH/m2/stage/rs/crates/bindings/paigasus-wasm"
for f in rs/Cargo.lock rs/crates/bindings/paigasus-wasm/paigasus_wasm.js rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.js rs/crates/bindings/paigasus-wasm/paigasus_wasm.d.ts rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm.d.ts rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm; do cp "$SCRATCH/m2/work/$f" "$SCRATCH/m2/stage/$f"; done
"$PY" ci/wasm-lockstep/lockstep_check.py artifact --dir "$SCRATCH/m2/stage" --old rs/Cargo.lock --title-file "$SCRATCH/m2/title.txt" --body-file "$SCRATCH/m2/body.md"; echo "artifact=$?"
cat "$SCRATCH/m2/title.txt"
for f in paigasus_wasm.js paigasus_wasm_bg.js paigasus_wasm.d.ts paigasus_wasm_bg.wasm.d.ts paigasus_wasm_bg.wasm; do cmp -s "rs/crates/bindings/paigasus-wasm/$f" "$SCRATCH/m2/stage/rs/crates/bindings/paigasus-wasm/$f" && echo "same $f" || echo "differs $f"; done
ls -l "$SCRATCH/m2/stage/rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm"
```

Expected: `artifact=0` and the title `build(deps): move wasm-bindgen to 0.2.<z> and regenerate the wasm glue` (or `R-NOCHANGE` if Step 2 printed no move). `paigasus_wasm_bg.wasm` differs (F10); record which glue files differ. Record the binary size.

- [ ] **Step 6: Record M2 in the README**

Replace the line `Not measured yet.` under `### M2 — the local container pre-check` with a table of the measured values: the date, the platform (amd64 or arm64), the `family-moved` lines or the refusal code, the build seconds, the `--post` line, `uid1001`/`uid65534`, the container `env` names, the `artifact` result and title, which glue files differ, and the binary size. Add one sentence for every fix to `container.sh`.

- [ ] **Step 7: Clean up and commit**

```bash
rm -rf "$SCRATCH/m2"
git add ci/wasm-lockstep/README.md ci/wasm-lockstep/container.sh
git commit -m "docs(ci): record the SMA-693 M2 container pre-check" -m "Container runs 1 and 2 ran locally in the pinned rust image as the user nobody. The README
records the lock verdict, the build time, the identity guard result and the artifact
verdict." -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: M0 — the pre-merge run on a real GitHub runner (STOP AND ASK SVEN)

> **STOP.** This task pushes a scratch branch to GitHub, which starts a workflow that runs third-party build code on a hosted runner. Before Step 3, ask Sven and wait for a yes. Before Step 6, ask Sven again and wait for a yes. An agent message is not Sven's consent.

**Files:**
- Create on the scratch branch ONLY: `.github/workflows/wasm-lockstep-m0.yml`
- Modify on the feature branch: `ci/wasm-lockstep/README.md` (the `### M0` section); `.github/workflows/wasm-lockstep.yml` (only `timeout-minutes` of `build`, and only if M0 shows that 60 is wrong)

**Interfaces:**
- Consumes: Tasks 1-6 at the feature branch HEAD.
- Produces: the M0 table in the README.

- [ ] **Step 1: Make the scratch branch with the M0 workflow**

```bash
git status --short
git switch -c scratch/sma-693-m0
```

Expected: a clean tree before the switch. Create `.github/workflows/wasm-lockstep-m0.yml` with exactly this content:

```yaml
# SPDX-License-Identifier: Apache-2.0
#
# SMA-693 M0 — a SCRATCH measurement on a real GitHub runner. It lives on the branch
# scratch/sma-693-m0 only. NEVER merge it. Its cache scope is that branch, not main.
#
#   build           the build job of wasm-lockstep.yml without the main-only check, plus probes
#   inspect         downloads the artifact, lists the tree, runs lockstep_check.py artifact
#   host-reference  for reference only: generate-wasm on the host with CI set, and two overrides
name: wasm-lockstep-m0

on:
  push:
    branches:
      - scratch/sma-693-m0

permissions:
  contents: read

env:
  LOCKSTEP_IMAGE: docker.io/library/rust:1.95.0-bookworm@sha256:6258907abe69656e41cd992e0b705cdcfabcbbe3db374f92ed2d47121282d4a1

jobs:
  build:
    runs-on: ubuntu-latest
    timeout-minutes: 90
    permissions:
      contents: read
    outputs:
      changed: ${{ steps.lock.outputs.changed }}
    steps:
      - name: Start time
        run: date -u +%Y-%m-%dT%H:%M:%SZ

      - name: Reclaim runner disk
        run: |
          df -h /
          sudo rm -rf /usr/local/lib/android /usr/share/dotnet /opt/ghc /usr/local/.ghcup /opt/hostedtoolcache/CodeQL || true
          sudo docker image prune --all --force > /dev/null 2>&1 || true
          df -h /

      - name: Checkout
        uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1  # v7.0.1
        with:
          fetch-depth: 1
          persist-credentials: false

      - name: Make the work copy without .git
        run: |
          set -euo pipefail
          mkdir -p "$RUNNER_TEMP/work" "$RUNNER_TEMP/stage"
          git archive HEAD | tar -x -C "$RUNNER_TEMP/work"
          git show HEAD:rs/Cargo.lock > "$RUNNER_TEMP/old.lock"
          chmod -R a+rwX "$RUNNER_TEMP/work"

      - name: Probe the container environment
        run: |
          set -euo pipefail
          docker run --rm --user 65534:65534 --volume "$RUNNER_TEMP/work:/work" --workdir /work "$LOCKSTEP_IMAGE" bash -c 'echo "--- env"; env | sort; echo "--- pids"; ls /proc; echo "--- passwd"; getent passwd 1001 || echo "no passwd entry for uid 1001"; getent passwd 65534; echo "--- runtime token in env: ${ACTIONS_RUNTIME_TOKEN:-absent}"'

      - name: Update the four family roots in the container
        run: |
          set -euo pipefail
          docker run --rm --user 65534:65534 --volume "$RUNNER_TEMP/work:/work" --workdir /work "$LOCKSTEP_IMAGE" bash ci/wasm-lockstep/container.sh update

      - name: Check the lock change on the host
        id: lock
        run: |
          set -euo pipefail
          rc=0
          python3 ci/wasm-lockstep/lockstep_check.py lock --old "$RUNNER_TEMP/old.lock" --new "$RUNNER_TEMP/work/rs/Cargo.lock" > "$RUNNER_TEMP/lock-verdict.txt" || rc=$?
          cat "$RUNNER_TEMP/lock-verdict.txt"
          echo "lockstep_check rc=${rc}"
          if [ "$rc" -eq 0 ]; then
            echo "changed=true" >> "$GITHUB_OUTPUT"
          else
            echo "changed=false" >> "$GITHUB_OUTPUT"
          fi

      - name: Regenerate the wasm artifacts in the container
        if: steps.lock.outputs.changed == 'true'
        run: |
          set -euo pipefail
          date -u +%Y-%m-%dT%H:%M:%SZ
          docker run --rm --user 65534:65534 --volume "$RUNNER_TEMP/work:/work" --workdir /work "$LOCKSTEP_IMAGE" bash ci/wasm-lockstep/container.sh build
          date -u +%Y-%m-%dT%H:%M:%SZ

      - name: Stage the six files
        if: steps.lock.outputs.changed == 'true'
        run: |
          set -euo pipefail
          src="$RUNNER_TEMP/work"
          dst="$RUNNER_TEMP/stage"
          mkdir -p "$dst/rs/crates/bindings/paigasus-wasm"
          for f in rs/Cargo.lock rs/crates/bindings/paigasus-wasm/paigasus_wasm.js rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.js rs/crates/bindings/paigasus-wasm/paigasus_wasm.d.ts rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm.d.ts rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm; do
            if test -L "$src/$f"; then
              echo "::error::$f is a symlink"
              exit 1
            fi
            cp "$src/$f" "$dst/$f"
          done
          ls -l "$dst/rs/Cargo.lock" "$dst/rs/crates/bindings/paigasus-wasm"

      - name: Upload the artifact
        if: steps.lock.outputs.changed == 'true'
        uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a  # v7.0.1
        with:
          name: wasm-lockstep
          path: ${{ runner.temp }}/stage/
          if-no-files-found: error
          retention-days: 7

      - name: End time
        if: always()
        run: date -u +%Y-%m-%dT%H:%M:%SZ

  inspect:
    needs: build
    if: needs.build.outputs.changed == 'true'
    runs-on: ubuntu-latest
    timeout-minutes: 15
    permissions:
      contents: read
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1  # v7.0.1
        with:
          persist-credentials: false
      - uses: actions/download-artifact@3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c  # v8.0.1
        with:
          name: wasm-lockstep
          path: ${{ runner.temp }}/lockstep
      - name: List the downloaded tree and run the propose checker on it
        run: |
          set -euo pipefail
          find "$RUNNER_TEMP/lockstep" -exec ls -ld {} +
          python3 ci/wasm-lockstep/lockstep_check.py artifact --dir "$RUNNER_TEMP/lockstep" --old rs/Cargo.lock --body-file "$RUNNER_TEMP/pr-body.md" --title-file "$RUNNER_TEMP/pr-title.txt"
          cat "$RUNNER_TEMP/pr-title.txt" "$RUNNER_TEMP/pr-body.md"

  host-reference:
    runs-on: ubuntu-latest
    timeout-minutes: 60
    permissions:
      contents: read
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1  # v7.0.1
        with:
          persist-credentials: false
      - uses: moonrepo/setup-toolchain@261c62cb5b0f580c7be7c8cd0f023a2e96756095  # v0
        with:
          cache: false
      - run: proto install
      - run: moon setup
      - working-directory: rs
        run: |
          rustup component add rustfmt clippy
          rustup target add wasm32-unknown-unknown
      - run: pnpm --dir ts install --frozen-lockfile
      - name: generate-wasm with CI and GITHUB_ACTIONS set
        run: |
          rc=0
          moon run paigasus-kernel-ts:generate-wasm || rc=$?
          echo "CI=${CI} GITHUB_ACTIONS=${GITHUB_ACTIONS} rc=${rc}"
      - name: generate-wasm with env -u CI -u GITHUB_ACTIONS
        run: |
          rc=0
          env -u CI -u GITHUB_ACTIONS moon run paigasus-kernel-ts:generate-wasm || rc=$?
          echo "rc=${rc}"
          git status --porcelain
      - name: generate-wasm with CI=false
        run: |
          git checkout -- rs/crates/bindings/paigasus-wasm
          rc=0
          CI=false moon run paigasus-kernel-ts:generate-wasm || rc=$?
          echo "rc=${rc}"
          git status --porcelain
```

```bash
git add .github/workflows/wasm-lockstep-m0.yml
git commit -m "test(ci): add the SMA-693 M0 scratch measurement workflow" -m "A scratch branch only. It never merges." -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

- [ ] **Step 2: Ask Sven before the push**

Tell Sven: the branch `scratch/sma-693-m0`; the one extra file; that it runs the `build` job (with probes), an `inspect` job and a `host-reference` job on hosted runners, with `contents: read`, no environment and no secrets; that its cache scope is the scratch branch. Wait for his yes.

- [ ] **Step 3: Push and wait for the run**

```bash
git push -u origin scratch/sma-693-m0
REPO="$(gh repo view --json nameWithOwner --jq .nameWithOwner)"; echo "$REPO"
gh run list --branch scratch/sma-693-m0 --workflow wasm-lockstep-m0.yml --json databaseId,status --jq '.[0]'
```

Expected: one run. Note its `databaseId` as `RUN`. Wait with `gh run watch "$RUN" --exit-status` started with `run_in_background: true`; do not end your turn while it runs.

- [ ] **Step 4: Collect the measurements**

```bash
RUN=<the databaseId from Step 3>
REPO="$(gh repo view --json nameWithOwner --jq .nameWithOwner)"
gh run view "$RUN" --json jobs --jq '.jobs[] | [.name, .conclusion, .startedAt, .completedAt] | @tsv'
gh run view "$RUN" --log > "$SCRATCH/m0.log"
gh api "repos/$REPO/actions/runs/$RUN/artifacts" --jq '.artifacts[] | [.id, .name, .size_in_bytes] | @tsv'
grep -nE -- '--- env|--- pids|--- passwd|runtime token in env|no passwd entry|family-moved|family-current|lockstep_check rc=|generate-wasm: wrote|generate-wasm: the new|rc=|R-[A-Z]+' "$SCRATCH/m0.log"
```

Record, from the log:
- `build` inside the container: the `env` listing has no `CI`, `GITHUB_ACTIONS`, `ACTIONS_RUNTIME_TOKEN` or `GITHUB_TOKEN`; the `pids` listing shows only the container's own processes; `runtime token in env: absent`.
- The lock verdict, the `generate-wasm --post` line, and which `^:build` tasks Moon ran (the `▪▪▪▪ … (… )` lines of the build step).
- `inspect`: the downloaded tree (it must hold `rs/Cargo.lock` and `rs/crates/bindings/paigasus-wasm/…`, with no extra `rs/` level and no `stage/` level) and the `artifact` verdict.
- `host-reference`: the rc of each of the three `generate-wasm` steps (with `CI` set, with `env -u CI -u GITHUB_ACTIONS`, with `CI=false`), and whether `git status --porcelain` showed the five files.
- The job times and the artifact size.

- [ ] **Step 5: Return to the feature branch and record M0**

```bash
git switch feature/sma-693-wasm-bindgen-lockstep-updater
```

Replace the line `Not measured yet.` under `### M0 — the scratch-branch run on a GitHub runner` with a table of the Step 4 values, the run URL and the date. If the `build` job took more than 30 minutes, set its `timeout-minutes` in `.github/workflows/wasm-lockstep.yml` to twice the measured time, rounded up to 10 minutes; else keep 60.

```bash
git add ci/wasm-lockstep/README.md .github/workflows/wasm-lockstep.yml
git commit -m "docs(ci): record the SMA-693 M0 runner measurement" -m "A scratch-branch run on a hosted runner. The README records the container environment, the
build time, the downloaded artifact tree and the host-reference result for CI and
GITHUB_ACTIONS." -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

- [ ] **Step 6: Ask Sven, then delete the scratch branch**

Ask Sven for a yes to delete `scratch/sma-693-m0` on GitHub and locally. Then:

```bash
git push origin --delete scratch/sma-693-m0
git branch -D scratch/sma-693-m0
git ls-remote --heads origin scratch/sma-693-m0
```

Expected: the last command prints nothing. The M0 artifact expires after 7 days; to delete it now, run `gh api -X DELETE "repos/$REPO/actions/artifacts/<id>"` with the id from Step 4.

---

### Task 8: Documentation (AC9)

**Files:**
- Modify: `rs/CLAUDE.md:176-201`
- Modify: `ts/CLAUDE.md:134-136`
- Modify: `ts/packages/paigasus-kernel/moon.yml:186-191, 215-216`
- Modify: `rs/Cargo.toml:150-151`
- Modify: `.github/dependabot.yml:6-13`
- Modify: `ts/packages/paigasus-kernel/tests/committed-wasm.test.ts:75-76`
- Modify: `.github/CLAUDE.md` (one bullet after the first bullet of `## Workflow credentials and release guards`, line 261)

**Interfaces:**
- Consumes: the workflow name `wasm-lockstep.yml` and the branch `deps/wasm-bindgen-lockstep` (Task 3).
- Produces: text only. The bullet title `The wasm-bindgen family does not move through dependabot` stays EXACTLY as it is: three files quote it.

- [ ] **Step 1: Find the old statements**

```bash
git grep -n "stays frozen until a person\|ONE host regenerates\|Only ONE host regenerates\|CI never regenerates" -- rs/CLAUDE.md ts/CLAUDE.md ts/packages/paigasus-kernel/moon.yml .github/dependabot.yml rs/Cargo.toml
```

Expected: hits in `rs/CLAUDE.md:179`, `ts/CLAUDE.md:134`, `ts/packages/paigasus-kernel/moon.yml:189` and `:215`, `.github/dependabot.yml:9`. These are the statements AC9 changes.

- [ ] **Step 2: `rs/CLAUDE.md` — the normal path and the one-host rule**

Replace:

```markdown
  time. `cargo update -p wasm-bindgen` then locks 0 packages (MEASURED, spec M0). Since SMA-680
  the release PR does not move it either. So `wasm-bindgen` stays frozen until a person moves
  the whole family. Do that at a `rust-toolchain.toml` or `wasm-pack` bump, at a `repo:deny`
  advisory for the family, or when a newer `wasm-bindgen` is needed. Use a normal
  `feature/sma-NNN-<slug>` PR.
```

with:

```markdown
  time. `cargo update -p wasm-bindgen` then locks 0 packages (MEASURED, spec M0). Since SMA-680
  the release PR does not move it either.
  **The normal path is `.github/workflows/wasm-lockstep.yml` (SMA-693).** Every Tuesday at
  06:17 UTC it runs the four-package `cargo update -p` on `main`. When the lock changes, it
  regenerates the five artifacts on Linux in a build container, and opens or updates ONE pull
  request on the bot branch `deps/wasm-bindgen-lockstep`. Read the `rs/Cargo.lock` diff of that
  pull request before the merge: nothing enforces this review. Do not push to the bot branch;
  the next run refuses a branch that a person changed. To start a run now:
  `gh workflow run wasm-lockstep.yml --ref main`. `ci/wasm-lockstep/README.md` holds the trust
  model and the refusal codes.
  Use the manual runbook below only when the workflow cannot help: it refuses the lock change (a
  new transitive dependency, or a newer `syn`), its build fails because the pinned `wasm-pack`
  does not support the new 0.2.z, or the `reqwest` case below. Use a normal
  `feature/sma-NNN-<slug>` PR.
```

Replace:

```markdown
  scripts on your machine, where your `gh` token and signing agent are available. Run
  `generate-wasm` on ONE host (SMA-634 F12). If the pinned `wasm-pack` does not support the new
  0.2.z, bump it in `.prototools` in the same PR (the invariant above `wasm-bindgen` in
```

with:

```markdown
  scripts on your machine, where your `gh` token and signing agent are available. ONE host per
  PR regenerates the artifacts. A family bump from the `wasm-lockstep` workflow regenerates on
  Linux inside its build container; a kernel or binding edit regenerates on the author's host.
  So the binary changes host between PRs (SMA-634 F12). `CI` never regenerates; it only
  compares. If the pinned `wasm-pack` does not support the new
  0.2.z, bump it in `.prototools` in the same PR (the invariant above `wasm-bindgen` in
```

- [ ] **Step 3: `ts/CLAUDE.md` — the one-host rule**

Replace:

```markdown
  broken link, not even with `--force`. Only ONE host regenerates the artifacts; a second host makes
  different bytes and a diff that says nothing. A conflict in the five files is resolved by taking
  either side and running `generate-wasm` again.
```

with:

```markdown
  broken link, not even with `--force`. ONE host per PR regenerates the artifacts. A family bump
  from the `wasm-lockstep` workflow (SMA-693) regenerates on Linux inside its build container; a
  kernel or binding edit regenerates on the author's host. So the binary changes host between PRs,
  and a second host in one PR makes different bytes and a diff that says nothing. `CI` never
  regenerates; it only compares. A conflict in the five files is resolved by taking either side and
  running `generate-wasm` again.
```

- [ ] **Step 4: `ts/packages/paigasus-kernel/moon.yml` — the two comments**

Replace:

```yaml
  # Run it after an edit to the Rust kernel or the wasm binding, then commit all five files.
  # tests/committed-wasm.test.ts reds until the committed files agree with the source.
  #
  # ONE host regenerates them. The binary bytes differ per host (spec F12), so a second host
  # produces a diff that says nothing. The four glue files are byte-identical on macOS, Linux arm64
  # and Linux amd64 (spec F15), which is what makes the gate valid in CI.
```

with:

```yaml
  # Run it after an edit to the Rust kernel or the wasm binding, then commit all five files. A
  # wasm-bindgen family bump runs it in the build container of .github/workflows/wasm-lockstep.yml
  # (SMA-693). tests/committed-wasm.test.ts reds until the committed files agree with the source.
  #
  # ONE host per PR regenerates them. A family bump from the wasm-lockstep workflow regenerates on
  # Linux; a kernel or binding edit regenerates on the author's host. The binary bytes differ per
  # host (spec F12), so the binary changes host between PRs, and a second host in one PR produces
  # a diff that says nothing. The four glue files are byte-identical on macOS, Linux arm64 and
  # Linux amd64 (spec F15), which is what makes the gate valid in CI.
```

Replace:

```yaml
  # runInCI: false — CI never regenerates; it only compares. cache: false — the task writes tracked
  # files, and a cached replay would write nothing.
```

with:

```yaml
  # runInCI: false — the CI workflow never regenerates; it only compares. The wasm-lockstep
  # workflow runs this task inside a container that has no CI variable, so Moon does not drop it
  # there (SMA-693 spec F2). cache: false — the task writes tracked files, and a cached replay
  # would write nothing.
```

- [ ] **Step 5: `rs/Cargo.toml`, `.github/dependabot.yml`, `committed-wasm.test.ts`, `.github/CLAUDE.md`**

In `rs/Cargo.toml`, replace:

```toml
# (runbook: rs/CLAUDE.md, "The wasm-bindgen family does not move through dependabot"), or
# this re-introduces the schema mismatch the proto pin was meant to avoid.
```

with:

```toml
# (.github/workflows/wasm-lockstep.yml moves the family every week and fails its build when the
# pinned wasm-pack is too old; the manual runbook is in rs/CLAUDE.md, "The wasm-bindgen family
# does not move through dependabot"), or this re-introduces the schema mismatch the proto pin
# was meant to avoid.
```

In `.github/dependabot.yml`, replace:

```yaml
  # locks 0 packages (MEASURED, spec M0). Since SMA-680 the release PR does not move it either.
  # So wasm-bindgen stays frozen until a person moves the whole family. The runbook is in
  # rs/CLAUDE.md, "The wasm-bindgen family does not move through dependabot".
```

with:

```yaml
  # locks 0 packages (MEASURED, spec M0). Since SMA-680 the release PR does not move it either.
  # SMA-693: .github/workflows/wasm-lockstep.yml moves the whole family every Tuesday and opens
  # ONE pull request. The manual runbook for its refusals is in rs/CLAUDE.md, "The wasm-bindgen
  # family does not move through dependabot".
```

In `ts/packages/paigasus-kernel/tests/committed-wasm.test.ts`, replace:

```ts
  'Run `moon run paigasus-kernel-ts:generate-wasm` and commit all five files under rs/crates/bindings/paigasus-wasm/ (paigasus_wasm_bg.wasm and the four glue files). If wasm-bindgen moved, follow the wasm-bindgen runbook in rs/CLAUDE.md ("The wasm-bindgen family does not move through dependabot").';
```

with:

```ts
  'Run `moon run paigasus-kernel-ts:generate-wasm` and commit all five files under rs/crates/bindings/paigasus-wasm/ (paigasus_wasm_bg.wasm and the four glue files). If wasm-bindgen moved, run the wasm-lockstep workflow (`gh workflow run wasm-lockstep.yml --ref main`), or follow the wasm-bindgen runbook in rs/CLAUDE.md ("The wasm-bindgen family does not move through dependabot").';
```

In `.github/CLAUDE.md`, after the first bullet of `## Workflow credentials and release guards` (it ends with `the only proof is a real run on \`main\`.`), insert:

```markdown
- The `release-pr` environment has TWO consumers since SMA-693: the `release-pr` job of
  `release.yml` and the `propose` job of `wasm-lockstep.yml`. Both read `PAIGASUS_BOT_*` from it,
  and both rely on its main-only deployment branch policy as the credential boundary. A change to
  that policy or to those secrets changes both workflows. `release_guard.py` does not check
  `wasm-lockstep.yml`; `repo:wasm-lockstep` (`ci/wasm-lockstep/pin_check.py`) does.
```

- [ ] **Step 6: Check the text and the formatters**

```bash
git grep -n "stays frozen until a person\|Only ONE host regenerates\|ONE host regenerates them" -- rs/CLAUDE.md ts/CLAUDE.md ts/packages/paigasus-kernel/moon.yml .github/dependabot.yml; echo "old=$?"
git grep -c "The wasm-bindgen family does not move through dependabot" -- rs/CLAUDE.md rs/Cargo.toml .github/dependabot.yml ts/packages/paigasus-kernel/tests/committed-wasm.test.ts
moon run ts:fmt; echo "fmt=$?"
( cd rs && cargo fmt --check ); echo "cargo-fmt=$?"
moon run paigasus-kernel-ts:test --force; echo "kernel-test=$?"
```

Expected: no old hit and `old=1`; at least one hit in each of the four files; `fmt=0`; `cargo-fmt=0`; `kernel-test=0` (the REGENERATE text is a message, not an assertion).

- [ ] **Step 7: Commit**

```bash
git add rs/CLAUDE.md ts/CLAUDE.md ts/packages/paigasus-kernel/moon.yml rs/Cargo.toml .github/dependabot.yml ts/packages/paigasus-kernel/tests/committed-wasm.test.ts .github/CLAUDE.md
git commit -m "docs(repo): name the wasm-lockstep workflow as the normal path for the family bump" -m "The runbook stays for the refusal cases, the reqwest case and a wasm-pack bump. The one-
host rule now says one host per PR, with the workflow on Linux for a family bump." -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 9: The full gate graph before the push (spec T5-T8)

**Files:** none (verification only).

**Interfaces:**
- Consumes: every earlier task.
- Produces: a recorded local verdict per gate, for the PR description.

- [ ] **Step 1: Fetch the base and run the CI graph under bash 3.2**

```bash
git fetch origin main
PATH="$SCRATCH/bash32:$PATH" moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site :http-extractor-envelope :input-liveness :promtool :observability-drift :nats-permissions :release-parity :release-parity-py :release-parity-ts :publish-metadata :version-lockstep :workflow-credentials :pyo3-stub-drift :ruff-ci :next-public-free :helm-render :moon-diagnosis-exec :wasm-lockstep :test-e2e --base origin/main --include-relations; echo "ci=$?"
```

This list is the root `CLAUDE.md` marker block after Task 5. Expected: `repo:wasm-lockstep`, `repo:affected-smoke`, `repo:input-liveness`, `repo:workflow-credentials` and `repo:osv` pass. Under bash 3.2 the bash-4+ gates (`repo:ruff-ci`, `repo:next-public-free`, `repo:publish-metadata`, `repo:version-lockstep`, `repo:nats-permissions`) can fail with `mapfile: command not found` or `declare: -A: invalid option`: that is the wrong bash, not a finding. Before you re-run anything after a failure, copy the moon cache report and the task state directory out of the repo (root `CLAUDE.md`, the diagnosis procedure, Step 0).

- [ ] **Step 2: Re-run the bash-4+ gates under bash 5**

```bash
for g in ruff next-public publish-metadata version-lockstep; do rc=0; PATH="$SCRATCH/bash5:$PATH" ./ci/$g/run.sh > "$SCRATCH/gate-$g.log" 2>&1 || rc=$?; echo "$g rc=$rc"; done
PATH="$SCRATCH/bash5:$PATH" moon run repo:nats-permissions --force; echo "nats=$?"
PATH="$SCRATCH/bash5:$PATH" ./ci/actionlint/run.sh > "$SCRATCH/actionlint.log" 2>&1; echo "actionlint=$?"; grep -F 'pipe capacity' "$SCRATCH/actionlint.log"
```

Expected: each `rc=0`; `nats=0`; `actionlint=0` with a pipe preflight of 65536 bytes. A 512-byte preflight means rc 2 and no local verdict for actionlint (SMA-612): record that and leave it to CI. A local `version-lockstep --negative-control` `tar: Write error` is a known host artifact (SMA-686).

- [ ] **Step 3: Record the verdicts**

Write one line per gate (pass, fail, or "no local verdict, reason") into `$SCRATCH/sma-693-verdicts.txt`. The PR description (the `feature-factory:open-pr` stage) quotes it. Do not push in this plan: the pipeline's open-pr stage pushes and opens the PR.

---

## After the merge (not an implementation task)

### M3 — the first `workflow_dispatch` run on `main`

After the PR merges, ask Sven before the dispatch. Then:

```bash
gh workflow run wasm-lockstep.yml --ref main
gh run list --workflow wasm-lockstep.yml --branch main --json databaseId,status,conclusion --jq '.[0]'
```

Record in `ci/wasm-lockstep/README.md` (`### M3`), in a follow-up PR:
- the `build` result (`rs/Cargo.lock` holds 0.2.128 and 0.2.129 is published, so a change is expected unless a person ran the runbook first; spec F16);
- the `propose` result: the base check, the branch-owner check, the push, and the PR number;
- whether GitHub refused the force push for the missing `workflows` permission (Q11). If it did, ask Sven to choose: a new branch per run, or the `workflows` permission;
- that the PR's own `CI` started (the App token, not `GITHUB_TOKEN`, opened it).

Then the human review of the lock diff, and the merge, are Sven's.

---

## Self-review notes

- Spec coverage: AC1 (Task 3 `build` lock step and `close` step); AC2 (Task 3 stage, upload, apply and status check); AC3 (Task 3 container, Task 2 P3/P4/P12); AC4.1/AC4.2 (Task 1 `artifact` and `lock`); AC4.3 (Task 3 `base` step, green notice); AC5 (Task 1 exit 3 and the runbook message; Task 3 `verify` fails before the token); AC6 (PR body from `body_for`; M3); AC7 (Task 3 Step 5); AC8 (Tasks 1, 2, 4, 5); AC9 (Task 8). T1 Task 1; T2 Task 1 and Task 4; T3 Task 2; T4 Tasks 1, 2 and 4; T5 Task 3 Step 4 and Task 9; T6 Task 5 and Task 9; T7 Task 3 Step 5; T8 Task 9 and the PR's CI. M2 Task 6, M0 Task 7, M3 after the merge.
- Not mappable to a task: "Sven watches the red runs" (Q10) and "the cron line must be last changed by Sven" (section 9). The second holds when Sven authors the merge, because GitHub mails the person who last changed the cron line.
