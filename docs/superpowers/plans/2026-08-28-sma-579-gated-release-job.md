# SMA-579 — Gated release job, npm activation, release guard: Implementation Plan

> **Historical record.** This plan describes what was planned, not what shipped. Several
> embedded code snippets contained defects — the `lstrip("./")` path bug, a process-substitution
> exit-2 swallow, missing `read_text` error handling, and the `plan`-job fallback's dangling
> outputs — that were found in review and corrected during execution. The code on this branch
> does NOT match the code shown below. The spec and the shipped files are authoritative.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship the complete-but-inert irreversible half of the release path — tags, crates.io, PyPI and npm — behind `vars.PAIGASUS_RELEASE_ENABLED`, plus a CI gate that asserts every registry-reaching job is gated.

**Architecture:** `release.yml` gains a `plan → {wheels, prebuild, proto-dist} → release → {publish-pypi, publish-npm}` graph. Everything reversible runs before the first irreversible step, and **no job downstream of `release` builds anything**. A new `ci/actionlint/release_guard.py`, driven by real PyYAML obtained through the pinned uv, asserts the gating structurally rather than by grep.

**Tech Stack:** GitHub Actions, release-plz 0.3.158 (core 0.36.14), maturin, napi-rs 3.x, wasm-pack, uv, Moon 2.3.2, bash + Python 3.12.

**Spec:** `docs/superpowers/specs/2026-08-28-sma-579-gated-release-job-design.md` (revision 2, approved 2026-08-28)

## Global Constraints

- **Every source file opens with an SPDX header:** `# SPDX-License-Identifier: Apache-2.0` (Python, YAML, shell).
- **`release.yml` must NEVER gain a `pull_request` or `pull_request_target` trigger.** It reads secrets; a PR trigger makes same-repo PRs receive them.
- **`wheels.yml` and `prebuild.yml` must NEVER declare `secrets:` or `id-token: write`.** They carry `pull_request` triggers.
- **No job downstream of `release` may build anything** — download, assert, upload only.
- **No `always()`, `!cancelled()`, `success()` or `failure()`** anywhere on a gated `needs:` path. This is banned by the guard this plan writes.
- **`napi prepublish` always carries `--no-gh-release`.** Its `--help` does not list this flag; `prebuild.yml:241-243` records that it exists and is required. Do not "correct" it.
- **Exact equality, never a substring**, for every assertion (`wheels.yml:15-18`).
- **Pin every GitHub Action by SHA** with a trailing `# vX.Y.Z` comment, matching every existing `uses:` in this repo.
- **Branch/path trigger filters are block sequences**, never inline `[main]` — `repo:actionlint` fails all four keys loudly on inline flow.
- **Commit scopes** must be one of: `rs, py, ts, contracts, ci, docs, deps, release, repo, claude, workspace`. Subject starts lowercase, ≤100 chars. No `#NNN` in the body (breaks `footer-leading-blank`).
- **Bash PATH:** prefix commands with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- **Measure exit status UNPIPED.** `cmd | tail` reports `tail`'s status. This already cost one wrong measurement on this issue.
- **Worktree:** `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-579`, branch `feature/sma-579-release-activation-d-the-gated-release-job-npm-activation`.

---

## File Structure

| File | Responsibility |
|---|---|
| `ci/actionlint/release_guard.py` | **Create.** The verdict: parse a workflow, decide gating. Owns `UNGATED_JOBS`, the accepted gate expressions, the fixture table, and `--self-test` / `--fixture-count`. |
| `ci/actionlint/run.sh` | **Modify.** Adds `release_guard_self_test` (11th table), `SELF_TEST_COUNT` 10→11, and the production call site (check 10). |
| `ci/affected-graph/ci_targets.py` | **Modify.** Adds three `ACTIONLINT_SH_CALL_SITES` entries: the production call, the `--self-test` invocation, and the fixture arity floor. |
| `.github/workflows/release.yml` | **Modify.** Adds `plan`, `wheels`, `prebuild`, `proto-dist`, `approve-release`, `release`, `publish-pypi`, `publish-npm`. Moves `concurrency` to the `release-pr` job. |
| `.github/workflows/prebuild.yml` | **Modify.** Adds `workflow_call`, the wasm build, the `npm-dirs`/`wasm-dist` artifacts, an SPDX header. |
| `rs/release-plz.toml` | **Modify.** Per-package `git_release_enable`. |
| `rs/crates/bindings/paigasus-wasm/package.json` | **Modify.** Drop `private`, add `publishConfig.access` + metadata. |
| `rs/crates/bindings/paigasus-node-bindings/package.json` | **Modify.** Drop `private`. |
| `py/pyproject.toml` | **Modify.** Add `pyyaml` to the dev group. |
| `py/packages/paigasus-proto/pyproject.toml` | **Modify.** Add `[tool.paigasus] pypi = true`. |
| `ci/publish-metadata/run.sh` | **Modify.** One line: `EXPECTED_PYPI_PUBLISHABLE` gains `paigasus-proto`. |
| `CLAUDE.md`, `ci/actionlint/README.md` | **Modify.** Gotchas and the re-measured cost table. |

---

## Task 1: Settle the measurements that can still reshape the design

**No production code.** Four of these can change §1's job graph, §4's credentials, or §5's assertion target. Doing them first is why the rest of the plan can be written as fact.

**Files:**
- Create: `docs/superpowers/specs/2026-08-28-sma-579-measurements.md`
- Modify: `docs/superpowers/specs/2026-08-28-sma-579-gated-release-job-design.md` (fold results into §1.3b, §4.4, §5.3, §2.1, §7.6, §12)

**Interfaces:**
- Produces: for every later task — whether `plan` exists (M1), whether `NPM_TOKEN` exists (M4), which package the version assertion binds to (M2), the `git_release_enable` key spelling (M5).

- [ ] **Step 1: M1 — does `release-plz release --dry-run` succeed, and what does it cost?**

The umbrella §14 Q6 recorded a measured `exit 101, no matching package named 'paigasus-proto-derive'` for a per-package dry-run before the derive crate exists on crates.io. If `release-plz release --dry-run` inherits that, `plan` fails on the first push after SMA-580.

Run from the worktree, **status unpiped**:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs
start=$(date +%s)
release-plz release --dry-run --git-token "$(gh auth token)" > /tmp/m1.out 2> /tmp/m1.err
rc=$?
end=$(date +%s)
echo "exit=$rc  wall=$((end-start))s"
tail -40 /tmp/m1.err
```

`--git-token` is required even for a dry-run (spec §1.3c, measured). A read-scoped `gh auth token` is enough — the dry-run creates nothing.

Record: exit status, wall-clock, and whether `paigasus-proto-derive` appears in any error.

- [ ] **Step 2: M1 decision**

- `exit 0` → `plan` stands as specified.
- non-zero for the derive-crate reason → **delete `plan`**; gate `wheels`/`prebuild`/`proto-dist` on `vars.PAIGASUS_RELEASE_ENABLED == 'true'` directly (spec §1.3b fallback). Every later task's `needs: plan` becomes the direct gate, and `UNGATED_JOBS` in Task 3 stays `{"release-pr"}` either way.
- wall-clock > ~5 min → record it; `plan` still stands (it replaces a 12-leg matrix) but note the cost in `release.yml`.

- [ ] **Step 3: M2 — does `--output json` list Cargo `publish = false` packages?**

`paigasus-py-bindings` is `publish = false`. Spec §5.3 binds the version assertion to `paigasus-kernel` because of this; confirm whether binding to both is possible.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs
release-plz release --dry-run --output json --git-token "$(gh auth token)" > /tmp/m2.json 2>/tmp/m2.err
echo "exit=$?"
cat /tmp/m2.json
```

Record which `package_name` values appear. Expected shape (measured from `release_plz_core` 0.36.14):
`{"releases":[{"package_name":…,"prs":[…],"tag":…,"version":…}]}`.

- [ ] **Step 4: M3 — how does the CLI serialize `release()`'s `None`?**

`release()` returns `Option<Release>`. `release-pr`'s precedent wraps `None` as `[]`. Determine whether a no-op run prints `{"releases":[]}`, `null`, or nothing — Task 6 keys `has_releases` on it.

If Step 3 produced a non-empty run, re-run after confirming nothing is releasable, or read `main.rs` at the pinned tag as `release-pr` did.

- [ ] **Step 5: M4 — does npm Trusted Publishing remove `NPM_TOKEN`?**

Two sub-questions, both from spec §4.4:
1. Does npm Trusted Publishing cover a **scoped org package** (`@paigasus/*`)?
2. Does `napi prepublish` publish through the npm CLI in a way that picks it up?

Check npm's current Trusted Publishing docs for (1). For (2), inspect the installed CLI:

```bash
cd ts/packages/paigasus-kernel
grep -rn "npm publish\|execSync\|spawn" ../../node_modules/@napi-rs/cli/dist/*.js | grep -i publish | head -20
```

Record the finding. If both hold → no `NPM_TOKEN` anywhere. If either fails → `NPM_TOKEN` stays, **and the reason it was rejected is recorded**, per §4.4.

- [ ] **Step 6: M5 — the `git_release_enable` TOML key**

Spec §2.1 declines to assert this from memory. The config parser is in the `release-plz` CLI crate, not `release_plz_core`.

```bash
D=/tmp/rp-cli && rm -rf $D && mkdir -p $D && cd $D
curl -sSL "https://static.crates.io/crates/release-plz/release-plz-0.3.158.crate" \
  -H 'User-Agent: paigasus-sma579' -o cli.crate
tar xzf cli.crate
grep -rn 'git_release_enable\|git_tag_enable' release-plz-0.3.158/src/ | head
```

Record the exact key name and whether it is valid at `[[package]]` scope.

- [ ] **Step 7: M6 — `NPM_CONFIG_PROVENANCE` and the generated `repository` field**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/packages/paigasus-kernel
CRATE=../../../rs/crates/bindings/paigasus-node-bindings
pnpm exec napi create-npm-dirs --cwd "$CRATE"
cat "$CRATE"/npm/*/package.json | head -40
```

Record whether the generated per-platform `package.json` files carry `repository` (required by `npm publish --provenance`). Clean up the generated `npm/` dir afterwards: `rm -rf "$CRATE/npm"`.

- [ ] **Step 8: Write the measurements record**

Create `docs/superpowers/specs/2026-08-28-sma-579-measurements.md` with one section per measurement: the exact command, the raw output, and the decision it settles. State scope limits (one host, one version) the way SMA-578's spec does.

- [ ] **Step 9: Fold results into the spec**

Amend the design doc: §1.3b (M1), §2.1 (M5), §5.3 (M2/M3), §4.4 + §4.5 (M4), §7.6 (M6), and strike the answered entries from §12.

- [ ] **Step 10: Commit**

```bash
git add docs/superpowers/specs/
git commit -m "docs(docs): measure the SMA-579 release-path preconditions"
```

---

## Task 2: `pyyaml` as a locked, scanned dev dependency

Spec §9.1. The `uv run --with` form leaves pyyaml unlocked, unscanned, and — because `ci.yml` keys the uv cache on `hashFiles('py/uv.lock')` — refetched from PyPI on **every** run, since an exact primary-key hit makes `actions/cache` skip its save.

**Files:**
- Modify: `py/pyproject.toml`
- Modify: `py/uv.lock` (regenerated)

**Interfaces:**
- Produces: `uv run --project py python3 …` can `import yaml`. Task 3's guard and Task 4's invocation depend on it.

- [ ] **Step 1: Add the dependency**

In `py/pyproject.toml`, add to the dev dependency group (match the file's existing group syntax exactly — read it first):

```toml
    "pyyaml==6.0.3",
```

Pin exactly, not a range: this is a required-check gate's parser, and a silent minor bump changes parse behaviour.

- [ ] **Step 2: Regenerate the lock**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd py && uv sync
```

- [ ] **Step 3: Verify it is importable through the project**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
uv run --project py python3 -c "import yaml; print(yaml.__version__)"
```

Expected: `6.0.3`.

- [ ] **Step 4: Verify the cache key actually rotates**

The whole point. `py/uv.lock` must have changed:

```bash
git diff --stat py/uv.lock
```

Expected: non-empty. An empty diff means pyyaml did not reach the lock and Step 1 targeted the wrong group.

- [ ] **Step 5: Verify version-lockstep still passes**

`py/uv.lock` is one of its eighteen sites.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
bash ci/version-lockstep/run.sh --check
```

Expected: exit 0.

- [ ] **Step 6: Commit**

```bash
git add py/pyproject.toml py/uv.lock
git commit -m "deps(py): add pyyaml for the release guard's workflow parse"
```

---

## Task 3: `ci/actionlint/release_guard.py` — the verdict

Spec §8.2–8.6. Written test-first: the fixture table IS the test, and it lives beside the verdict so `--self-test` can drive it.

**Files:**
- Create: `ci/actionlint/release_guard.py`

**Interfaces:**
- Produces, for Task 4:
  - `release_guard.py <workflow.yml> [<workflow.yml> …]` → exit 0 clean, 1 violations (printed to stdout, one per line), 2 infra.
  - `release_guard.py --self-test` → exit 0 all fixtures behave, 1 otherwise.
  - `release_guard.py --fixture-count` → prints an integer to stdout, exit 0.
- Consumes: `pyyaml` from Task 2.

- [ ] **Step 1: Write the file with its fixture table first**

Create `ci/actionlint/release_guard.py`:

```python
# SPDX-License-Identifier: Apache-2.0
"""Release guard (SMA-579) — assert every job that can reach a registry is gated.

WHY A REAL PARSER, NOT grep. The verdict must tell a JOB-level `if:` from the EIGHT identical
STEP-level ones release.yml already carries, and must walk `needs:` chains. Neither is a
line-oriented question. SMA-593 exists because ci/publish-metadata hand-rolled a partial YAML
scanner and 14 spellings evaded it; a second hand-rolled scanner would recreate that defect class
in a guard whose whole job is structural.

PyYAML is a YAML 1.1 parser and GitHub's schema collides with it in five measured places — see
COERCIONS in the fixture table. The `on:` key parsing as the boolean True is the one that bites
first.

FAIL-CLOSED. Every abnormal condition exits 2 (infra). Never a skip, never a pass.
"""

from __future__ import annotations

import sys
from pathlib import Path

try:
    import yaml
except ImportError:  # pragma: no cover - exercised by the missing-interpreter path
    print("release-guard: PyYAML is not importable. This gate needs the pinned pyyaml from "
          "py/pyproject.toml — invoke via `uv run --project py`.", file=sys.stderr)
    raise SystemExit(2)

# --- The pinned vocabulary ---------------------------------------------------------------------

# V2: the gate expression is pinned as a LITERAL, accepted in exactly two forms. A substring test
# would admit `!= 'disabled'` and `== 'true' || github.actor == 'x'`, both of which always run.
GATE_EXPR = "vars.PAIGASUS_RELEASE_ENABLED == 'true'"
ACCEPTED_GATE_FORMS = frozenset({GATE_EXPR, "${{ " + GATE_EXPR + " }}"})

# V1: the subject is EVERY job; the exemption is the pin. Inverted from a detection-derived set,
# which could not see a publish step using an unrecognised mechanism (SMA-579 review).
UNGATED_JOBS = frozenset({"release-pr"})

# V3: the real bypass class is any status-check function, not two literal spellings.
# `success() || failure()`, `!failure()` and `${{ ! cancelled() }}` all evade a two-string test.
STATUS_FUNCS = ("always", "cancelled", "success", "failure")

# V6: detection, retained ONLY for called workflows where UNGATED_JOBS has no meaning.
PUBLISH_MARKERS = (
    "release-plz release",
    "npm publish",
    "napi prepublish",
    "twine upload",
    "gh-action-pypi-publish",
    "cargo publish",
)


def infra(msg: str) -> "NoReturn":  # type: ignore[valid-type]
    print(f"release-guard: {msg}", file=sys.stderr)
    raise SystemExit(2)


# --- Parsing -----------------------------------------------------------------------------------

def load_workflow(path: Path) -> dict:
    """Parse one workflow. Fail-closed on every abnormal condition."""
    if not path.is_file():
        infra(f"{path}: not a readable file")
    try:
        docs = [d for d in yaml.safe_load_all(path.read_text(encoding="utf-8")) if d is not None]
    except yaml.YAMLError as exc:
        infra(f"{path}: unparseable YAML: {exc}")
    if len(docs) != 1:
        infra(f"{path}: expected exactly 1 YAML document, found {len(docs)}")
    doc = docs[0]
    if not isinstance(doc, dict):
        infra(f"{path}: top level is {type(doc).__name__}, expected a mapping")
    if not isinstance(doc.get("jobs"), dict):
        infra(f"{path}: 'jobs:' is missing or not a mapping")
    return doc


def triggers_of(doc: dict) -> dict:
    """`on:` parses as the BOOLEAN True under YAML 1.1. Measured on release.yml: top-level keys
    come back as ['name', True, 'concurrency', 'permissions', 'jobs']."""
    raw = doc.get("on", doc.get(True))
    if isinstance(raw, str):
        return {raw: None}
    if isinstance(raw, list):
        return {k: None for k in raw}
    if isinstance(raw, dict):
        return raw
    infra("'on:' is missing or of an unexpected type")


def needs_of(job: dict) -> list[str]:
    """`needs:` may be a scalar string. Iterating a str yields CHARACTERS and silently walks
    nothing — which would make the transitive half of V1 vacuous, and that half carries every job
    but `plan`."""
    raw = job.get("needs")
    if raw is None:
        return []
    if isinstance(raw, str):
        return [raw]
    if isinstance(raw, list):
        return [str(x) for x in raw]
    infra(f"'needs:' is {type(raw).__name__}, expected a string or list")


def if_text(job: dict) -> str | None:
    """`if: false` parses to the BOOLEAN False, not the string 'false'."""
    raw = job.get("if")
    if raw is None:
        return None
    if isinstance(raw, bool):
        return "true" if raw else "false"
    return str(raw).strip()


def coe_is_false(job_or_step: dict) -> bool:
    """V4. `continue-on-error: false` parses to the BOOLEAN False; `"false"` stays a STRING and
    GitHub treats it as false too. Accept both; reject everything else."""
    raw = job_or_step.get("continue-on-error")
    if raw is None:
        return True
    if isinstance(raw, bool):
        return raw is False
    return str(raw).strip() == "false"


# --- The verdict -------------------------------------------------------------------------------

def is_gated(job_id: str, jobs: dict, seen: frozenset[str] = frozenset()) -> bool:
    """V1. Gated directly, or through an unbroken `needs:` chain from a gated job."""
    if job_id in seen:          # a needs: cycle is not a gate
        return False
    job = jobs.get(job_id)
    if not isinstance(job, dict):
        return False
    if if_text(job) in ACCEPTED_GATE_FORMS:
        return True
    deps = needs_of(job)
    if not deps:
        return False
    # EVERY dependency must be gated: one ungated path is an ungated job.
    return all(is_gated(d, jobs, seen | {job_id}) for d in deps)


def gated_path_jobs(job_id: str, jobs: dict, seen: frozenset[str] = frozenset()) -> set[str]:
    """The job plus every job on its needs: path — the set V3/V4 apply to."""
    if job_id in seen or job_id not in jobs:
        return set()
    out = {job_id}
    for d in needs_of(jobs[job_id]):
        out |= gated_path_jobs(d, jobs, seen | {job_id})
    return out


def job_publishes(job: dict) -> bool:
    """V6 detection. Used ONLY for called workflows."""
    for step in job.get("steps") or []:
        if not isinstance(step, dict):
            continue
        blob = f"{step.get('run', '')}\n{step.get('uses', '')}"
        if any(m in blob for m in PUBLISH_MARKERS):
            return True
    return False


def check_main(doc: dict, name: str) -> list[str]:
    """V1-V5 over the release workflow."""
    out: list[str] = []
    jobs = doc["jobs"]

    for job_id, job in jobs.items():
        if not isinstance(job, dict):
            infra(f"{name}: job '{job_id}' is not a mapping")
        if job_id in UNGATED_JOBS:
            continue

        if not is_gated(job_id, jobs):
            out.append(
                f"{name}: job '{job_id}' is not gated on PAIGASUS_RELEASE_ENABLED, directly or "
                f"through an unbroken needs: chain. Add the gate, extend the chain, or add it to "
                f"UNGATED_JOBS with a stated reason."
            )
            continue

        # V3/V4 apply to the job AND every job on its needs: path — an always() upstream
        # un-gates everything downstream of it.
        for pid in gated_path_jobs(job_id, jobs):
            pjob = jobs[pid]
            txt = if_text(pjob) or ""
            for fn in STATUS_FUNCS:
                if f"{fn}(" in txt.replace(" ", ""):
                    out.append(
                        f"{name}: job '{pid}' (on '{job_id}'s gated path) uses the status "
                        f"function {fn}() in its if:. That defeats the gate for every job "
                        f"downstream of it."
                    )
            if not coe_is_false(pjob):
                out.append(
                    f"{name}: job '{pid}' carries continue-on-error: "
                    f"{pjob.get('continue-on-error')!r}. A failed job then counts as success for "
                    f"needs:, so a failed publish still releases downstream."
                )
            for step in pjob.get("steps") or []:
                if isinstance(step, dict) and not coe_is_false(step):
                    out.append(
                        f"{name}: job '{pid}' has a step with continue-on-error: "
                        f"{step.get('continue-on-error')!r}. That hides a failed publish inside a "
                        f"job that still reports success."
                    )

        # V5: the tagging boundary (spec §2), enforced rather than documented.
        for step in job.get("steps") or []:
            if not isinstance(step, dict):
                continue
            run = str(step.get("run") or "")
            if "napi prepublish" in run and "--no-gh-release" not in run:
                out.append(
                    f"{name}: job '{job_id}' runs `napi prepublish` without --no-gh-release. "
                    f"release-plz owns every tag (ADR-0011 S3); napi must never cut one."
                )
    return out


def check_called(doc: dict, name: str) -> list[str]:
    """V6. A workflow the release path CALLS may publish only if it is workflow_call-ONLY.

    Revision 1 of the spec claimed such a workflow inherits the caller's gate. It does not:
    wheels.yml and prebuild.yml carry their own push: and pull_request: triggers, so a publish
    step added to one would run ungated on every PR while the caller's gate stayed green.
    """
    out: list[str] = []
    publishing = [jid for jid, j in doc["jobs"].items() if isinstance(j, dict) and job_publishes(j)]
    if not publishing:
        return out
    trigs = set(triggers_of(doc))
    if trigs != {"workflow_call"}:
        out.append(
            f"{name}: jobs {sorted(publishing)} can reach a registry, but this workflow's "
            f"triggers are {sorted(str(t) for t in trigs)}. A workflow called from the release "
            f"path may publish only if it is workflow_call-ONLY — otherwise the publish runs "
            f"ungated on its own triggers while the caller's gate stays green."
        )
    return out


# --- Fixtures ----------------------------------------------------------------------------------
# Each row: (name, kind, yaml, expected_substring | None). None means "must be clean".
# The arity floor in ci/actionlint/run.sh pins len(FIXTURES) — emptying this table would
# otherwise be invisible to check 7's bash-only definition counter.

_OK_MAIN = """
on:
  push:
    branches:
      - main
jobs:
  release-pr:
    runs-on: ubuntu-latest
    steps: [{run: echo hi}]
  plan:
    if: vars.PAIGASUS_RELEASE_ENABLED == 'true'
    runs-on: ubuntu-latest
    steps: [{run: echo plan}]
  release:
    needs: [plan]
    runs-on: ubuntu-latest
    steps: [{run: release-plz release}]
"""

FIXTURES: list[tuple[str, str, str, str | None]] = [
    ("healthy control", "main", _OK_MAIN, None),
    ("ungated job", "main", _OK_MAIN.replace("    if: vars.PAIGASUS_RELEASE_ENABLED == 'true'\n", ""),
     "is not gated"),
    ("gate expression weakened to !=", "main",
     _OK_MAIN.replace("== 'true'", "!= 'disabled'"), "is not gated"),
    ("gate expression widened with ||", "main",
     _OK_MAIN.replace("== 'true'", "== 'true' || github.actor == 'x'"), "is not gated"),
    ("wrapped gate form is accepted", "main",
     _OK_MAIN.replace("if: vars.PAIGASUS_RELEASE_ENABLED == 'true'",
                      "if: ${{ vars.PAIGASUS_RELEASE_ENABLED == 'true' }}"), None),
    ("always() on the gated job", "main",
     _OK_MAIN.replace("    needs: [plan]", "    needs: [plan]\n    if: always()"), "status function"),
    ("!cancelled() with spacing", "main",
     _OK_MAIN.replace("    needs: [plan]", "    needs: [plan]\n    if: ${{ ! cancelled() }}"),
     "status function"),
    ("success() || failure()", "main",
     _OK_MAIN.replace("    needs: [plan]", "    needs: [plan]\n    if: success() || failure()"),
     "status function"),
    ("job-level continue-on-error: true", "main",
     _OK_MAIN.replace("    needs: [plan]", "    needs: [plan]\n    continue-on-error: true"),
     "continue-on-error"),
    ("step-level continue-on-error: true", "main",
     _OK_MAIN.replace("steps: [{run: release-plz release}]",
                      "steps: [{run: release-plz release, continue-on-error: true}]"),
     "step with continue-on-error"),
    ("continue-on-error: false (bool) is accepted", "main",
     _OK_MAIN.replace("    needs: [plan]", "    needs: [plan]\n    continue-on-error: false"), None),
    ('continue-on-error: "false" (str) is accepted', "main",
     _OK_MAIN.replace("    needs: [plan]", '    needs: [plan]\n    continue-on-error: "false"'), None),
    ("needs: as a SCALAR string still walks", "main",
     _OK_MAIN.replace("    needs: [plan]", "    needs: plan"), None),
    ("needs: scalar pointing at an ungated job reds", "main",
     _OK_MAIN.replace("    needs: [plan]", "    needs: release-pr"), "is not gated"),
    ("napi prepublish without --no-gh-release", "main",
     _OK_MAIN.replace("run: release-plz release", "run: napi prepublish --npm-dir npm"),
     "without --no-gh-release"),
    ("napi prepublish with --no-gh-release is clean", "main",
     _OK_MAIN.replace("run: release-plz release",
                      "run: napi prepublish --no-gh-release --npm-dir npm"), None),
    ("job-level if: false is MORE restrictive, so clean", "main",
     _OK_MAIN.replace("    needs: [plan]", "    needs: [plan]\n    if: false"), None),
    ("called workflow that is workflow_call-only may publish", "called",
     "on:\n  workflow_call:\njobs:\n  build:\n    steps: [{run: twine upload dist/*}]\n", None),
    ("called workflow with pull_request may NOT publish", "called",
     "on:\n  workflow_call:\n  pull_request:\n    branches:\n      - main\n"
     "jobs:\n  build:\n    steps: [{run: twine upload dist/*}]\n", "workflow_call-ONLY"),
    ("called workflow with no publish step is clean", "called",
     "on:\n  workflow_call:\n  pull_request:\n    branches:\n      - main\n"
     "jobs:\n  build:\n    steps: [{run: maturin build}]\n", None),
]


def self_test() -> int:
    rc = 0
    for name, kind, text, want in FIXTURES:
        docs = [d for d in yaml.safe_load_all(text) if d is not None]
        doc = docs[0]
        if not isinstance(doc.get("jobs"), dict):
            print(f"FIXTURE BROKEN '{name}': no jobs mapping", file=sys.stderr)
            rc = 1
            continue
        found = (check_main if kind == "main" else check_called)(doc, "fixture")
        blob = " | ".join(found)
        if want is None and found:
            print(f"FAIL '{name}': expected clean, got: {blob}", file=sys.stderr)
            rc = 1
        elif want is not None and want not in blob:
            print(f"FAIL '{name}': expected a violation containing {want!r}, got: "
                  f"{blob or '(clean)'}", file=sys.stderr)
            rc = 1
    return rc


def main(argv: list[str]) -> int:
    if argv == ["--fixture-count"]:
        print(len(FIXTURES))
        return 0
    if argv == ["--self-test"]:
        return self_test()
    if not argv:
        infra("usage: release_guard.py <workflow.yml> [...] | --self-test | --fixture-count")

    violations: list[str] = []
    main_path = Path(argv[0])
    main_doc = load_workflow(main_path)
    violations += check_main(main_doc, main_path.name)

    # Follow local reusable-workflow calls out of the MAIN workflow only (one level; a called
    # workflow cannot itself call another local one in this repo, and V6 keeps the callees honest).
    for job in main_doc["jobs"].values():
        uses = str(job.get("uses") or "") if isinstance(job, dict) else ""
        if uses.startswith("./"):
            p = Path(uses.lstrip("./"))
            violations += check_called(load_workflow(p), p.name)

    for v in violations:
        print(v)
    return 1 if violations else 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
```

- [ ] **Step 2: Run the self-test and verify it PASSES**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
uv run --project py python3 ci/actionlint/release_guard.py --self-test
echo "exit=$?"
```

Expected: exit 0, no output. If any fixture fails, the verdict is wrong — fix the verdict, not the fixture.

- [ ] **Step 3: Prove the guard REDS on a real broken workflow**

A guard never observed reporting red is the control-that-lies failure. Build a temporary broken `release.yml` and confirm exit 1:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cp .github/workflows/release.yml /tmp/rg-broken.yml
python3 - <<'PY'
import pathlib
p = pathlib.Path('/tmp/rg-broken.yml')
t = p.read_text()
t = t.replace('jobs:\n', "jobs:\n  rogue:\n    runs-on: ubuntu-latest\n    steps:\n      - run: npm publish\n", 1)
p.write_text(t)
PY
uv run --project py python3 ci/actionlint/release_guard.py /tmp/rg-broken.yml
echo "exit=$?  (expect 1)"
```

Expected: exit 1 and a line naming `rogue` as ungated.

- [ ] **Step 4: Verify it is CLEAN on the current real workflow**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
uv run --project py python3 ci/actionlint/release_guard.py .github/workflows/release.yml
echo "exit=$?  (expect 0)"
```

Today `release.yml` has only `release-pr`, which is in `UNGATED_JOBS`, so this must be 0.

- [ ] **Step 5: Verify the fail-closed contract**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
uv run --project py python3 ci/actionlint/release_guard.py /nonexistent.yml; echo "missing file: $? (expect 2)"
printf 'jobs: [not, a, mapping]\n' > /tmp/rg-bad.yml
uv run --project py python3 ci/actionlint/release_guard.py /tmp/rg-bad.yml; echo "bad jobs: $? (expect 2)"
printf 'a: 1\n---\nb: 2\n' > /tmp/rg-multi.yml
uv run --project py python3 ci/actionlint/release_guard.py /tmp/rg-multi.yml; echo "multi-doc: $? (expect 2)"
```

All three must print 2. Never 0.

- [ ] **Step 6: Commit**

```bash
git add ci/actionlint/release_guard.py
git commit -m "ci(repo): add the release guard's workflow-structure verdict (SMA-579)"
```

---

## Task 4: Wire the guard into `ci/actionlint/run.sh`

Spec §8.7. The eleventh self-test table plus the production call site.

**Files:**
- Modify: `ci/actionlint/run.sh` (SELF_TEST_COUNT at :40; new function before check 7; `run_self_tests` at :4015; production call near :4740)

**Interfaces:**
- Consumes: Task 3's three CLI modes.
- Produces, for Task 5: the exact call-site lines to pin — `release_guard_self_test`, the `--self-test` invocation, and the arity-floor line.

- [ ] **Step 1: Add a helper and the eleventh fixture table**

Insert immediately before the `# Check 7` banner comment (just above `assert_self_tests_ran`):

```bash
# ---------------------------------------------------------------------------------------------
# Check 10 (SMA-579) — the release guard. The VERDICT lives in ci/actionlint/release_guard.py
# because it needs YAML STRUCTURE: a job-level `if:` must be told from the eight identical
# step-level ones release.yml carries, and `needs:` chains must be walked. Neither is a
# line-oriented question, and SMA-593 is the standing evidence that a hand-rolled scanner for
# this class rots into a control that lies.
#
# WHY A BASH WRAPPER AT ALL. Check 7 counts bash `*_self_test` DEFINITIONS and check 9 mutates
# lines inside run_self_tests — both see bash only. A Python fixture table is invisible to them,
# so EMPTYING it would leave this gate passing having asserted nothing. The arity floor below is
# what closes that, exactly as check 8e's two floors do for its own tables.
release_guard_py() {
  uv run --project py python3 ci/actionlint/release_guard.py "$@"
}

release_guard_self_test() {
  local rc=0 n
  SELF_TESTS_RAN=$((SELF_TESTS_RAN + 1))

  n="$(release_guard_py --fixture-count)" || infra "check 10: release_guard.py --fixture-count failed"
  case "$n" in ''|*[!0-9]*) infra "check 10: --fixture-count printed '$n', expected an integer" ;; esac
  [ "$n" -ge 20 ] || infra "check 10: release_guard.py reports $n fixtures, expected at least 20"

  release_guard_py --self-test || { fail "check 10: release_guard.py --self-test reported a broken
      verdict. The release guard is not deciding what it is documented to decide."; rc=1; }

  return $rc
}
```

- [ ] **Step 2: Bump `SELF_TEST_COUNT` 10 → 11**

At `ci/actionlint/run.sh:40`, change the value and extend the trailing comment:

```bash
SELF_TEST_COUNT=11  # extractor, path-filter, branch-filter, config, ci-target-floor,
                    # invocation-allowlist, affected-graph-wiring, block-execution,
                    # kill-predicate, affected-smoke-block, release-guard
```

- [ ] **Step 3: Add the invocation to `run_self_tests`**

After `affected_smoke_block_self_test`:

```bash
  release_guard_self_test
```

- [ ] **Step 4: Run `--self-test` and verify it passes with the new count**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
bash ci/actionlint/run.sh --self-test
echo "exit=$?  (expect 0)"
```

Expected: 0. A mismatch between definitions and `SELF_TEST_COUNT` reds here with the counter message.

- [ ] **Step 5: Prove the counter bites**

Temporarily comment out the `release_guard_self_test` line, re-run, confirm it reds, then restore:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
sed -i.bak 's/^  release_guard_self_test$/  # release_guard_self_test/' ci/actionlint/run.sh
bash ci/actionlint/run.sh --self-test; echo "exit=$? (expect 1)"
mv ci/actionlint/run.sh.bak ci/actionlint/run.sh
bash ci/actionlint/run.sh --self-test; echo "restored exit=$? (expect 0)"
```

**`mv` restores the ORIGINAL mtime, which is older than the edit — Moon and cargo both key on mtime. Run `touch ci/actionlint/run.sh` after restoring** so no later task reads a stale cache.

- [ ] **Step 6: Add the production call site**

Immediately before the `selftest_mutation_battery` line at the end of the file:

```bash
# ---------------------------------------------------------------------------------------------
# Check 10 — the release guard, over the real workflow. Runs here (not in --self-test) because it
# reads the actual .github/workflows tree, like checks 5/6.
# ---------------------------------------------------------------------------------------------
while IFS= read -r v; do
  [ -n "$v" ] && fail "check 10: $v"
done < <(release_guard_py .github/workflows/release.yml)
```

- [ ] **Step 7: Run the full gate**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
bash ci/actionlint/run.sh
echo "exit=$?  (expect 0)"
```

- [ ] **Step 8: Commit**

```bash
git add ci/actionlint/run.sh
git commit -m "ci(repo): wire the release guard into the actionlint gate (SMA-579)"
```

---

## Task 5: Pin the guard's call sites in `ci_targets.py`

Spec §8.7.3 and §8.7.5. Without these, deleting the call site or emptying the fixture table is a one-file edit that no gate notices.

**Files:**
- Modify: `ci/affected-graph/ci_targets.py` (`ACTIONLINT_SH_CALL_SITES`)

**Interfaces:**
- Consumes: Task 4's exact line text.

- [ ] **Step 1: Add three entries**

Append to `ACTIONLINT_SH_CALL_SITES`, after the two check-8e floors:

```python
    # SMA-579 — check 10's production call site. Column 0, like every other entry here: that
    # haystack matches at column 0 deliberately, so a call nested inside a function or an `if`
    # cannot satisfy it (review N5).
    "done < <(release_guard_py .github/workflows/release.yml)",
    # ...and the SELF-TEST invocation, pinned separately. Deleting it leaves the production call
    # running against a verdict nothing proved correct.
    "release_guard_py --self-test || { fail \"check 10: release_guard.py --self-test reported a broken",
    # ...and the FIXTURE TABLE's arity floor. This one exists because check 10's verdict is
    # PYTHON and check 7's definition counter is BASH-only: emptying FIXTURES in
    # release_guard.py is invisible to every other check, so the gate would pass having asserted
    # nothing. Same hole the two check-8e floors close, one language over.
    '[ "$n" -ge 20 ] || infra "check 10: release_guard.py reports $n fixtures, expected at least 20"',
```

- [ ] **Step 2: Run the pin check**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 ci/affected-graph/ci_targets.py
echo "exit=$?  (expect 0)"
```

If an entry does not match, print the offending line from `run.sh` and reconcile the text **exactly** — these are substring matches against real lines.

- [ ] **Step 3: Prove each pin bites**

For each of the three, delete the line from `run.sh`, confirm `ci_targets.py` reds, restore, `touch`:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cp ci/actionlint/run.sh /tmp/rs.keep
grep -v 'done < <(release_guard_py .github/workflows/release.yml)' /tmp/rs.keep > ci/actionlint/run.sh
python3 ci/affected-graph/ci_targets.py; echo "call-site deleted: $? (expect non-zero)"
cp /tmp/rs.keep ci/actionlint/run.sh && touch ci/actionlint/run.sh
python3 ci/affected-graph/ci_targets.py; echo "restored: $? (expect 0)"
```

Repeat for the `--self-test` line and the arity-floor line.

- [ ] **Step 4: Run the self-test suite**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 ci/affected-graph/ci_targets.py --self-test
echo "exit=$?  (expect 0)"
```

- [ ] **Step 5: Commit**

```bash
git add ci/affected-graph/ci_targets.py
git commit -m "ci(repo): pin the release guard's call sites and fixture floor (SMA-579)"
```

---

## Task 6: `release.yml` — the `plan` and `release` jobs

Spec §1, §3, §4. The guard from Tasks 3-5 now gets a real subject.

**Files:**
- Modify: `.github/workflows/release.yml`

**Interfaces:**
- Produces, for Tasks 7-9: `needs.plan.outputs.has_releases`, `.kernel_release`, `.proto_release`, `.kernel_version`, `.proto_version`; and `needs.release.outputs.released` (the raw JSON).

- [ ] **Step 1: Move `concurrency` from workflow level to the `release-pr` job**

Delete the workflow-level `concurrency:` block (currently lines 13-15). Add to the `release-pr` job, keeping the existing rationale comment:

```yaml
    concurrency:
      group: release-pr
      cancel-in-progress: false
```

Add a second group on the release path's entry point (`plan`, Step 3).

- [ ] **Step 2: Add the ordering rationale comment**

Above `jobs:`, add — spec §1.1 requires this comment exist, because the order looks arbitrary otherwise:

```yaml
# JOB ORDER IS LOAD-BEARING, and it is the reverse of the obvious one (SMA-578 review B3).
# Everything REVERSIBLE runs before the first IRREVERSIBLE step:
#
#   plan -> {wheels, prebuild, proto-dist} -> release -> {publish-pypi, publish-npm}
#
# The draft this replaced ran `release` FIRST, so release-plz published to crates.io and cut six
# tags before a single wheel was built. A zig regression or a runner-image change in the 12-leg
# matrix then left crates.io permanently published, tags permanently cut, and paigasus-kernel
# missing from PyPI while pinning paigasus-py-bindings==X.Y.Z. Nothing forces that order: the
# release commit on main already carries the bumped versions, so the artifacts can be built first.
#
# COROLLARY, equally load-bearing: NO JOB DOWNSTREAM OF `release` MAY BUILD ANYTHING. publish-pypi
# and publish-npm only download, assert and upload. A wasm-pack or uv build after the crates.io
# upload reintroduces exactly the half-published state this order exists to prevent.
```

- [ ] **Step 3: Add the `plan` job**

```yaml
  # The ONLY job carrying the gate directly. Everything below is gated TRANSITIVELY through an
  # unbroken needs: chain rooted here — which is the topology ci/actionlint/release_guard.py's V1
  # is written to walk, so the guard's needs:-walking is exercised in production, not only by
  # fixtures.
  #
  # It exists to avoid building a 12-leg matrix on every push to main just to learn there is
  # nothing to release. It does NOT gate the build jobs per family: a skipped needs: dependency
  # skips its dependents, and reviving them needs always()/!cancelled() — which this repo's own
  # release guard bans. Correctness over cost; a proto-only release builds kernel wheels nobody
  # uploads.
  plan:
    name: plan the release
    if: vars.PAIGASUS_RELEASE_ENABLED == 'true'
    runs-on: ubuntu-latest
    timeout-minutes: 30
    concurrency:
      group: release-path
      cancel-in-progress: false
    outputs:
      has_releases:   ${{ steps.dry.outputs.has_releases }}
      kernel_release: ${{ steps.dry.outputs.kernel_release }}
      proto_release:  ${{ steps.dry.outputs.proto_release }}
      kernel_version: ${{ steps.dry.outputs.kernel_version }}
      proto_version:  ${{ steps.dry.outputs.proto_version }}
    steps:
      # release-plz requires a git token EVEN under --dry-run: release() calls get_git_client()
      # unconditionally (release_plz_core 0.36.14, src/command/release.rs:543) and hard-errors
      # without one. `git_release_enable = false` does NOT suppress it — measured at both
      # [workspace] and [[package]] scope.
      - name: Mint the App installation token
        id: app_token
        uses: actions/create-github-app-token@bcd2ba49218906704ab6c1aa796996da409d3eb1  # v3.2.0
        with:
          client-id: ${{ secrets.PAIGASUS_BOT_APP_ID }}
          private-key: ${{ secrets.PAIGASUS_BOT_PRIVATE_KEY }}
          permission-contents: read

      - name: Checkout
        uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1  # v7.0.1
        with:
          fetch-depth: 0
          persist-credentials: false

      - name: Set up proto + Moon
        uses: moonrepo/setup-toolchain@261c62cb5b0f580c7be7c8cd0f023a2e96756095  # v0
        with:
          cache: false

      - name: Install pinned release-plz CLI
        run: proto install release-plz

      # --dry-run creates NO tags: create_git_tag_and_release() is reachable only from the `else`
      # arm of `if input.dry_run`, at both call sites (release_plz_core 0.36.14, release.rs:888
      # and :959). Verified by reading the pinned source, not inferred from --help.
      - name: Dry-run the release
        id: dry
        working-directory: rs
        env:
          GIT_TOKEN: ${{ steps.app_token.outputs.token }}
        run: |
          set -euo pipefail
          OUT="$(release-plz release --dry-run --output json)"
          echo "$OUT"
          NAMES="$(printf '%s' "$OUT" | jq -r '.releases[]?.package_name // empty')"
          if [ -z "$NAMES" ]; then
            echo "has_releases=false" >> "$GITHUB_OUTPUT"
          else
            echo "has_releases=true" >> "$GITHUB_OUTPUT"
          fi
          printf '%s\n' "$NAMES" | grep -qx 'paigasus-kernel' \
            && echo "kernel_release=true"  >> "$GITHUB_OUTPUT" \
            || echo "kernel_release=false" >> "$GITHUB_OUTPUT"
          printf '%s\n' "$NAMES" | grep -qx 'paigasus-proto' \
            && echo "proto_release=true"  >> "$GITHUB_OUTPUT" \
            || echo "proto_release=false" >> "$GITHUB_OUTPUT"
          echo "kernel_version=$(printf '%s' "$OUT" | jq -r \
            '.releases[]? | select(.package_name=="paigasus-kernel") | .version // empty')" >> "$GITHUB_OUTPUT"
          echo "proto_version=$(printf '%s' "$OUT" | jq -r \
            '.releases[]? | select(.package_name=="paigasus-proto") | .version // empty')" >> "$GITHUB_OUTPUT"
```

If Task 1's M1 showed the dry-run fails, apply §1.3b's fallback instead: delete `plan` and put `if: vars.PAIGASUS_RELEASE_ENABLED == 'true'` directly on `wheels`, `prebuild`, `proto-dist` and `release`.

- [ ] **Step 4: Add `approve-release` and `release`**

```yaml
  # The ONE place a human approval can be inserted, by adding required reviewers to the
  # `release-approval` environment in repo settings. It must NOT go on the publishing jobs:
  # GitHub pauses EACH job entering an environment, so reviewers on release-publish would stop
  # the run again between crates.io and PyPI — leaving crates.io published and PyPI empty if the
  # second approval is rejected or times out (30 days). That is the split state the job order
  # above exists to prevent. SMA-580: add reviewers HERE, never to release-publish.
  approve-release:
    name: approve the release
    needs: [plan, wheels, prebuild, proto-dist]
    if: needs.plan.outputs.has_releases == 'true'
    runs-on: ubuntu-latest
    environment: release-approval
    steps:
      - run: echo "Approved. Everything below this point is irreversible."

  release:
    name: publish to crates.io and cut tags
    needs: [plan, wheels, prebuild, proto-dist, approve-release]
    runs-on: ubuntu-latest
    timeout-minutes: 45
    environment: release-publish
    permissions:
      id-token: write    # crates.io OIDC exchange
      contents: read     # the App token below does the writing, not GITHUB_TOKEN
    outputs:
      released: ${{ steps.rel.outputs.json }}
    steps:
      - name: Authenticate with crates.io
        id: cratesio
        uses: rust-lang/crates-io-auth-action@c6f97d42243bad5fab37ca0427f495c86d5b1a18  # v1.0.5

      # A SECOND mint: tokens are per-job and live one hour. Every checkout in this repo sets
      # persist-credentials: false, so there are no ambient git credentials and release-plz's
      # tag push needs this explicitly.
      - name: Mint the App installation token
        id: app_token
        uses: actions/create-github-app-token@bcd2ba49218906704ab6c1aa796996da409d3eb1  # v3.2.0
        with:
          client-id: ${{ secrets.PAIGASUS_BOT_APP_ID }}
          private-key: ${{ secrets.PAIGASUS_BOT_PRIVATE_KEY }}
          permission-contents: write

      - name: Checkout
        uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1  # v7.0.1
        with:
          fetch-depth: 0
          persist-credentials: false

      - name: Set up proto + Moon
        uses: moonrepo/setup-toolchain@261c62cb5b0f580c7be7c8cd0f023a2e96756095  # v0
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
          OUT="$(release-plz release --output json)"
          echo "$OUT"
          echo "json=$OUT" >> "$GITHUB_OUTPUT"
```

- [ ] **Step 5: Verify the guard is GREEN on the new file**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
uv run --project py python3 ci/actionlint/release_guard.py .github/workflows/release.yml
echo "exit=$?  (expect 0)"
```

Every job but `release-pr` must resolve as gated through `plan`.

- [ ] **Step 6: Prove the guard reds on each real bypass**

Not fixtures — the actual file. For each mutation, confirm exit 1, then restore:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cp .github/workflows/release.yml /tmp/rel.keep
G() { uv run --project py python3 ci/actionlint/release_guard.py .github/workflows/release.yml; echo "  -> $?"; }

sed -i.b "s/    if: vars.PAIGASUS_RELEASE_ENABLED == 'true'/    if: always()/" .github/workflows/release.yml
echo "always() on plan:"; G
cp /tmp/rel.keep .github/workflows/release.yml

sed -i.b "s/^  release:$/  release:\n    continue-on-error: true/" .github/workflows/release.yml
echo "continue-on-error on release:"; G
cp /tmp/rel.keep .github/workflows/release.yml && touch .github/workflows/release.yml
rm -f .github/workflows/release.yml.b
```

Both must print `-> 1`.

- [ ] **Step 7: actionlint the file**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
actionlint .github/workflows/release.yml
echo "exit=$?  (expect 0)"
```

- [ ] **Step 8: Commit**

```bash
git add .github/workflows/release.yml
git commit -m "ci(release): add the gated plan and release jobs (SMA-579)"
```

---

## Task 7: The reversible build stage

Spec §1.1, §7.2, §7.3. `prebuild.yml` becomes reusable and takes over the wasm build; a `proto-dist` job builds the Python proto distribution. **All of this runs before `release`.**

**Files:**
- Modify: `.github/workflows/prebuild.yml`
- Modify: `.github/workflows/release.yml` (add `wheels`, `prebuild`, `proto-dist`)

**Interfaces:**
- Produces: artifacts `prebuild-<platform>` (existing), `npm-dirs`, `wasm-dist`, `proto-dist-py`; plus `wheels.yml`'s existing `wheel-*`, `sdist`, `face-paigasus-kernel`.

- [ ] **Step 1: Add the SPDX header and `workflow_call` to `prebuild.yml`**

Line 1 becomes:

```yaml
# SPDX-License-Identifier: Apache-2.0
name: prebuild
```

Add to the `on:` block, above `workflow_dispatch:`:

```yaml
on:
  # Consumed by release.yml's reversible stage (SMA-579). This workflow must NEVER declare
  # `secrets:` or `id-token: write` — it keeps a pull_request trigger, and same-repo PRs receive
  # repository secrets. Publishing happens in release.yml, which downloads these artifacts.
  workflow_call:

  workflow_dispatch:
```

- [ ] **Step 2: Correct the stale credentials comment**

Replace `# SMA-407 adds publish creds at activation.` with:

```yaml
# No publish, and no GitHub release (the assemble job omits napi's opt-in --gh-release flag).
# Credentials are NOT added here and never will be: this workflow carries a pull_request trigger.
# release.yml owns every credential and downloads these artifacts (SMA-579 §4).
```

- [ ] **Step 3: Add the wasm build to the `assemble` job**

`wasm-pack build` CLEANS its `--out-dir`: it overwrites `.gitignore` with a bare `*` and **DELETES `package.json`** even with `--no-pack` (`rs/crates/bindings/paigasus-wasm/.gitignore:4-10`). Building into the crate root would destroy the very metadata being published, and could regenerate a `package.json` **without** `publishConfig.access: public` — publishing the scoped package **restricted and irreversibly**.

Use a third scratch dir (`build` owns `.wasmpack-out`, `test` owns `.wasmpack-test-out`):

```yaml
      # A THIRD scratch dir. wasm-pack CLEANS --out-dir and deletes package.json, so it must
      # never run in the crate root (see that crate's .gitignore). ts/packages/paigasus-kernel's
      # build task owns .wasmpack-out and its test task owns .wasmpack-test-out; this needs its
      # own name so a concurrent moon ci cannot race it.
      - name: Build the wasm distribution
        run: |
          set -euo pipefail
          cd rs/crates/bindings/paigasus-wasm
          rustup target add wasm32-unknown-unknown
          wasm-pack build . --target bundler --release --no-pack \
            --out-dir .wasmpack-release-out --out-name paigasus_wasm
          test -s .wasmpack-release-out/paigasus_wasm_bg.wasm \
            || { echo "::error::wasm-pack produced no binary"; exit 1; }
          cp package.json .wasmpack-release-out/package.json

      - name: Upload the wasm distribution
        uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a  # v7.0.1
        with:
          name: wasm-dist
          path: rs/crates/bindings/paigasus-wasm/.wasmpack-release-out/
          if-no-files-found: error
```

Add `.wasmpack-release-out/` to `rs/crates/bindings/paigasus-wasm/.gitignore`.

- [ ] **Step 4: Upload the assembled npm dirs**

After the existing `napi artifacts` step:

```yaml
      - name: Upload the assembled npm dirs
        uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a  # v7.0.1
        with:
          name: npm-dirs
          path: rs/crates/bindings/paigasus-node-bindings/npm/
          if-no-files-found: error
```

- [ ] **Step 5: Add the three caller jobs to `release.yml`**

```yaml
  # THE REVERSIBLE STAGE. Everything below `release` only downloads what these produce.
  wheels:
    name: build wheels
    needs: plan
    uses: ./.github/workflows/wheels.yml

  prebuild:
    name: build node addons and wasm
    needs: plan
    uses: ./.github/workflows/prebuild.yml

  proto-dist:
    name: build the Python proto distribution
    needs: plan
    runs-on: ubuntu-latest
    timeout-minutes: 15
    steps:
      - name: Checkout
        uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1  # v7.0.1
        with:
          persist-credentials: false

      - name: Set up proto + Moon
        uses: moonrepo/setup-toolchain@261c62cb5b0f580c7be7c8cd0f023a2e96756095  # v0
        with:
          cache: false

      - name: Install Moon-managed toolchains
        run: moon setup

      - name: Build the distribution
        working-directory: py
        run: uv build --package paigasus-proto --out-dir ../dist

      - name: Upload
        uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a  # v7.0.1
        with:
          name: proto-dist-py
          path: |
            dist/paigasus_proto-*.whl
            dist/paigasus_proto-*.tar.gz
          if-no-files-found: error
```

- [ ] **Step 6: Verify the guard still passes, including V6**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
uv run --project py python3 ci/actionlint/release_guard.py .github/workflows/release.yml
echo "exit=$?  (expect 0)"
```

`wheels.yml` and `prebuild.yml` are now followed by V6. Neither has a publish step, so both are clean — the assertion is that they stay that way.

- [ ] **Step 7: Prove V6 bites on a real called workflow**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cp .github/workflows/wheels.yml /tmp/wh.keep
printf '\n      - name: rogue\n        run: twine upload dist/*\n' >> .github/workflows/wheels.yml
uv run --project py python3 ci/actionlint/release_guard.py .github/workflows/release.yml
echo "exit=$?  (expect 1, naming workflow_call-ONLY)"
cp /tmp/wh.keep .github/workflows/wheels.yml && touch .github/workflows/wheels.yml
```

- [ ] **Step 8: actionlint both workflows**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
actionlint .github/workflows/release.yml .github/workflows/prebuild.yml
echo "exit=$?  (expect 0)"
```

- [ ] **Step 9: Commit**

```bash
git add .github/workflows/release.yml .github/workflows/prebuild.yml rs/crates/bindings/paigasus-wasm/.gitignore
git commit -m "ci(release): build every artifact before the first irreversible step (SMA-579)"
```

---

## Task 8: `publish-pypi`

Spec §5. Downloads, asserts, uploads. Builds nothing.

**Files:**
- Modify: `.github/workflows/release.yml`

- [ ] **Step 1: Add the job**

```yaml
  publish-pypi:
    name: publish to PyPI
    needs: [plan, wheels, proto-dist, release]
    runs-on: ubuntu-latest
    timeout-minutes: 20
    environment: release-publish
    permissions:
      id-token: write    # PyPI trusted publishing; the claim binds to THIS file's name
    steps:
      # THREE downloads, not one. wheels.yml deliberately keeps face-paigasus-kernel OUTSIDE the
      # wheel-* namespace precisely so a single `pattern: wheel-*` + merge-multiple cannot
      # collapse them: paigasus-py-bindings MUST reach PyPI before paigasus-kernel, because the
      # face pins `==` and the reverse order leaves it uninstallable in between.
      - name: Download the bindings wheels
        uses: actions/download-artifact@3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c  # v8.0.1
        with:
          pattern: wheel-*
          merge-multiple: true
          path: dist-bindings

      - name: Download the bindings sdist
        uses: actions/download-artifact@3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c  # v8.0.1
        with:
          name: sdist
          path: dist-bindings

      - name: Download the kernel face
        uses: actions/download-artifact@3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c  # v8.0.1
        with:
          name: face-paigasus-kernel
          path: dist-face

      - name: Download the proto distribution
        uses: actions/download-artifact@3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c  # v8.0.1
        with:
          name: proto-dist-py
          path: dist-proto

      # Binds the artifact to the release. paigasus-py-bindings is Cargo `publish = false`, so
      # release-plz may not report it at all — bind against paigasus-kernel, which it definitely
      # reports and which version_group holds to the same number (repo:version-lockstep asserts
      # that). Without this a re-run against a moved main could publish a stale wheel.
      - name: Assert the wheels match the released version
        env:
          KERNEL_VERSION: ${{ needs.plan.outputs.kernel_version }}
        run: |
          set -euo pipefail
          [ -n "$KERNEL_VERSION" ] || { echo "::error::no kernel version from plan"; exit 1; }
          norm="${KERNEL_VERSION//./_}"
          found=0
          for w in dist-bindings/*.whl; do
            case "$(basename "$w")" in
              paigasus_py_bindings-"$KERNEL_VERSION"-*) found=$((found+1)) ;;
              *) echo "::error::unexpected wheel $(basename "$w") — expected version $KERNEL_VERSION"; exit 1 ;;
            esac
          done
          [ "$found" -ge 7 ] || { echo "::error::found $found wheels, expected at least 7"; exit 1; }
          echo "OK: $found wheels at $KERNEL_VERSION"

      # ORDER IS LOAD-BEARING: bindings, then the face. skip-existing makes a partial failure
      # re-runnable — PyPI returns 400 "file already exists" otherwise, so an un-skipped retry can
      # never succeed unaided. PyPI is delete-but-never-reuse.
      - name: Publish paigasus-py-bindings
        uses: pypa/gh-action-pypi-publish@ed0c53931b1dc9bd32cbe73a98c7f6766f8a527e  # v1.13.0
        with:
          packages-dir: dist-bindings
          skip-existing: true

      - name: Publish paigasus-kernel
        uses: pypa/gh-action-pypi-publish@ed0c53931b1dc9bd32cbe73a98c7f6766f8a527e  # v1.13.0
        with:
          packages-dir: dist-face
          skip-existing: true

      # Independent cadence: release-plz sees the RUST crate `paigasus-proto`; what uploads here
      # is the PYTHON distribution built from py/packages/paigasus-proto. Same name, two things.
      - name: Publish paigasus-proto
        if: needs.plan.outputs.proto_release == 'true'
        uses: pypa/gh-action-pypi-publish@ed0c53931b1dc9bd32cbe73a98c7f6766f8a527e  # v1.13.0
        with:
          packages-dir: dist-proto
          skip-existing: true
```

Verify the `pypa/gh-action-pypi-publish` SHA against its current release before committing:
`gh api repos/pypa/gh-action-pypi-publish/releases --jq '.[0].tag_name'` then resolve the tag.

- [ ] **Step 2: Verify the guard passes**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
uv run --project py python3 ci/actionlint/release_guard.py .github/workflows/release.yml
echo "exit=$?  (expect 0)"
```

- [ ] **Step 3: actionlint**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
actionlint .github/workflows/release.yml
echo "exit=$?  (expect 0)"
```

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/release.yml
git commit -m "ci(release): add the PyPI publish path (SMA-579)"
```

---

## Task 9: npm packaging and `publish-npm`

Spec §6, §7.

**Files:**
- Modify: `rs/crates/bindings/paigasus-wasm/package.json`
- Modify: `rs/crates/bindings/paigasus-node-bindings/package.json`
- Modify: `.github/workflows/release.yml`

- [ ] **Step 1: De-privatize `@paigasus/node-bindings`**

Remove the `"private": true,` line. Everything else is already correct.

- [ ] **Step 2: De-privatize and complete `@paigasus/wasm`**

Remove `"private": true,` and add the missing metadata. The file becomes:

```json
{
  "name": "@paigasus/wasm",
  "version": "0.1.0",
  "type": "module",
  "license": "Apache-2.0",
  "description": "WebAssembly binding for the Paigasus Rust kernel.",
  "keywords": ["paigasus", "wasm", "webassembly", "wasm-bindgen", "rust"],
  "homepage": "https://github.com/SMK1085/paigasus-core#readme",
  "repository": {
    "type": "git",
    "url": "git+https://github.com/SMK1085/paigasus-core.git",
    "directory": "rs/crates/bindings/paigasus-wasm"
  },
  "main": "paigasus_wasm.js",
  "module": "paigasus_wasm.js",
  "types": "paigasus_wasm.d.ts",
  "sideEffects": ["./paigasus_wasm.js", "./snippets/*"],
  "files": [
    "paigasus_wasm.js",
    "paigasus_wasm_bg.js",
    "paigasus_wasm_bg.wasm",
    "paigasus_wasm.d.ts",
    "paigasus_wasm_bg.wasm.d.ts"
  ],
  "publishConfig": {
    "access": "public"
  }
}
```

**`publishConfig.access: public` is not optional** — a scoped package without it publishes *restricted*, and that is irreversible at that version.

- [ ] **Step 3: Verify the workspace still installs and version-lockstep passes**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts install --frozen-lockfile
bash ci/version-lockstep/run.sh --check
echo "exit=$?  (expect 0)"
```

- [ ] **Step 4: Add `publish-npm`**

```yaml
  publish-npm:
    name: publish to npm
    needs: [plan, prebuild, release]
    runs-on: ubuntu-latest
    timeout-minutes: 20
    environment: release-publish
    permissions:
      id-token: write    # npm provenance
    steps:
      - name: Checkout
        uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1  # v7.0.1
        with:
          persist-credentials: false

      - name: Set up proto + Moon
        uses: moonrepo/setup-toolchain@261c62cb5b0f580c7be7c8cd0f023a2e96756095  # v0
        with:
          cache: false

      - name: Install JS workspace deps
        run: pnpm --dir ts install --frozen-lockfile

      - name: Download the node addons
        uses: actions/download-artifact@3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c  # v8.0.1
        with:
          pattern: prebuild-*
          merge-multiple: true
          path: rs/crates/bindings/paigasus-node-bindings/artifacts

      - name: Download the assembled npm dirs
        uses: actions/download-artifact@3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c  # v8.0.1
        with:
          name: npm-dirs
          path: rs/crates/bindings/paigasus-node-bindings/npm

      - name: Download the wasm distribution
        uses: actions/download-artifact@3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c  # v8.0.1
        with:
          name: wasm-dist
          path: wasm-dist

      # npm has no --skip-existing, and napi prepublish publishes EIGHT packages. A re-run after
      # a partial success would hit 403 "cannot publish over the previously published versions"
      # on the ones that landed, so the retry could never succeed unaided — the same argument
      # skip-existing answers for PyPI. Give npm those semantics by hand.
      - name: Skip packages already at this version
        id: npmstate
        env:
          VERSION: ${{ needs.plan.outputs.kernel_version }}
        run: |
          set -euo pipefail
          published () { npm view "$1@$2" version >/dev/null 2>&1; }
          if published "@paigasus/wasm" "$VERSION"; then
            echo "wasm=skip" >> "$GITHUB_OUTPUT"
          else
            echo "wasm=publish" >> "$GITHUB_OUTPUT"
          fi
          if published "@paigasus/node-bindings" "$VERSION"; then
            echo "node=skip" >> "$GITHUB_OUTPUT"
          else
            echo "node=publish" >> "$GITHUB_OUTPUT"
          fi

      # --no-gh-release IS REQUIRED and is NOT in --help (prebuild.yml:241-243 records this;
      # ghRelease defaults ON). release-plz owns every tag — ADR-0011 S3, "the tool owns every
      # tag", singular. ci/actionlint/release_guard.py's V5 fails if this flag is ever dropped.
      - name: Publish the node addon and its platform packages
        if: steps.npmstate.outputs.node == 'publish'
        working-directory: ts/packages/paigasus-kernel
        env:
          NPM_CONFIG_PROVENANCE: 'true'
        run: |
          set -euo pipefail
          pnpm exec napi prepublish --no-gh-release --npm-dir npm \
            --cwd ../../../rs/crates/bindings/paigasus-node-bindings

      # Assert the TARBALL, not the working tree: `files` membership is what npm actually ships.
      # A wasm-less package installs cleanly and fails at import.
      - name: Assert the wasm tarball carries its binary
        if: steps.npmstate.outputs.wasm == 'publish'
        working-directory: wasm-dist
        run: |
          set -euo pipefail
          npm pack --dry-run --json > /tmp/pack.json
          python3 - <<'PY'
          import json, sys
          files = {f["path"] for f in json.load(open("/tmp/pack.json"))[0]["files"]}
          if "paigasus_wasm_bg.wasm" not in files:
              print("::error::the wasm tarball ships no binary:", sorted(files))
              sys.exit(1)
          print("OK: tarball carries paigasus_wasm_bg.wasm")
          PY

      - name: Publish @paigasus/wasm
        if: steps.npmstate.outputs.wasm == 'publish'
        working-directory: wasm-dist
        run: npm publish --provenance --access public
```

If Task 1's M4 showed npm Trusted Publishing does not apply, add `NODE_AUTH_TOKEN: ${{ secrets.NPM_TOKEN }}` plus an `actions/setup-node` registry step to the two publishing steps, and record in §4.5 that it was rejected for a measured reason.

- [ ] **Step 5: Verify the guard passes — including V5**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
uv run --project py python3 ci/actionlint/release_guard.py .github/workflows/release.yml
echo "exit=$?  (expect 0)"
```

- [ ] **Step 6: Prove V5 bites**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cp .github/workflows/release.yml /tmp/rel2.keep
sed -i.b 's/napi prepublish --no-gh-release/napi prepublish/' .github/workflows/release.yml
uv run --project py python3 ci/actionlint/release_guard.py .github/workflows/release.yml
echo "exit=$?  (expect 1, naming --no-gh-release)"
cp /tmp/rel2.keep .github/workflows/release.yml && touch .github/workflows/release.yml
rm -f .github/workflows/release.yml.b
```

- [ ] **Step 7: Run Prettier over the changed JSON**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run ts:fmt
```

- [ ] **Step 8: Commit**

```bash
git add rs/crates/bindings/paigasus-wasm/package.json \
        rs/crates/bindings/paigasus-node-bindings/package.json \
        .github/workflows/release.yml
git commit -m "ci(release): activate npm publishing for the binding packages (SMA-579)"
```

---

## Task 10: `paigasus-proto` PyPI ownership — marker and expected set, **one commit**

Spec §5.4, §9. Check P0 compares a runtime discovery scan against `EXPECTED_PYPI_PUBLISHABLE` by strict equality, so **either edit alone reds the gate**. They must land together or whoever merges first reds `main`.

**Files:**
- Modify: `py/packages/paigasus-proto/pyproject.toml`
- Modify: `ci/publish-metadata/run.sh:119`

- [ ] **Step 1: Add the marker**

Append to `py/packages/paigasus-proto/pyproject.toml`:

```toml
# SMA-579 §5.4 — this distribution is uploaded by release.yml's publish-pypi job, conditioned on
# release-plz reporting a release for the RUST crate `paigasus-proto`. Without this marker the
# proto family would burn a PyPI version on every release that nothing ever uploaded, so the
# Python package could never be published at a matching version.
[tool.paigasus]
pypi = true
```

- [ ] **Step 2: Confirm the gate reds with the marker alone**

This proves the same-commit requirement is real, not assumed:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
bash ci/publish-metadata/run.sh
echo "exit=$?  (expect non-zero — P0 strict equality)"
```

- [ ] **Step 3: Add it to the expected set**

At `ci/publish-metadata/run.sh:119`:

```bash
EXPECTED_PYPI_PUBLISHABLE=("paigasus-kernel" "paigasus-proto" "paigasus-py-bindings")
```

Update the comment at lines 116-118, which currently says the package is deliberately absent and that SMA-579 owns the decision — it is now present, and this is that decision.

- [ ] **Step 4: Confirm the gate is green again**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
bash ci/publish-metadata/run.sh
echo "exit=$?  (expect 0)"
```

If Check P1 reds on metadata, `py/packages/paigasus-proto/pyproject.toml` already carries description, readme, license-files, authors and classifiers — read the failure before changing anything.

- [ ] **Step 5: Commit BOTH files together**

```bash
git add py/packages/paigasus-proto/pyproject.toml ci/publish-metadata/run.sh
git commit -m "ci(py): own paigasus-proto's PyPI publication (SMA-579)"
```

---

## Task 11: `release-plz.toml` — GitHub Releases

Spec §2.1. Two release pages per release commit, not six.

**Files:**
- Modify: `rs/release-plz.toml`

- [ ] **Step 1: Apply Task 1 M5's measured key**

Using the key name and scope M5 established, disable GitHub releases for every package except the two family heads. For each non-head package block add the disabling key, and add a comment above the family heads:

```toml
# GITHUB RELEASES (SMA-579 §2.1). release-plz creates one per released package by default, which
# would mean SIX release pages per release commit for two lockstep families. Only the two family
# HEADS carry one, each with its family's changelog. TAGS are unaffected — every package still
# gets `<package>-v<version>`, because release-plz owns every tag (ADR-0011 S3) and napi is
# barred from cutting any (`--no-gh-release`, asserted by ci/actionlint/release_guard.py V5).
```

- [ ] **Step 2: Verify the parity fixture still derives**

`ci/release-parity` derives its fixture config from this file's classification keys.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
bash ci/release-parity/run.sh --negative-control
echo "control exit=$?  (expect 0 — it reports red as expected)"
bash ci/release-parity/run.sh
echo "real exit=$?  (expect 0)"
```

- [ ] **Step 3: Verify release-plz still parses the config**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && release-plz release --dry-run --output json --git-token "$(gh auth token)" >/dev/null 2>/tmp/rp.err
echo "exit=$?"; tail -5 /tmp/rp.err
```

An unknown key errors here rather than being ignored — which is the point of measuring M5.

- [ ] **Step 4: Commit**

```bash
git add rs/release-plz.toml
git commit -m "release(rs): cut one GitHub release per family, not per package (SMA-579)"
```

---

## Task 12: Re-measure the gate cost, document, run the full graph

Spec §8.7.4, §9, §10.

**Files:**
- Modify: `ci/actionlint/README.md`
- Modify: `moon.yml` (the `actionlint` task's cost comment only — **not** its `inputs`)
- Modify: `CLAUDE.md`

- [ ] **Step 1: Re-measure `repo:actionlint`**

The battery grows from eleven concurrent subprocesses to twelve. **Sequential min-of-N is invalid on this shared host** — use interleaved A/B sweeps, as `ci/actionlint/README.md` already prescribes. Measure both `--self-test` alone and the full gate, at least 5 interleaved pairs.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
for i in 1 2 3 4 5; do
  s=$(python3 -c 'import time;print(time.time())')
  bash ci/actionlint/run.sh --self-test >/dev/null 2>&1
  e=$(python3 -c 'import time;print(time.time())')
  python3 -c "print(f'self-test $i: {($e-$s):.2f}s')"
  s=$(python3 -c 'import time;print(time.time())')
  bash ci/actionlint/run.sh >/dev/null 2>&1
  e=$(python3 -c 'import time;print(time.time())')
  python3 -c "print(f'full     $i: {($e-$s):.2f}s')"
done
```

- [ ] **Step 2: Update both cost tables**

Write the measured figures into `ci/actionlint/README.md`'s table and `moon.yml`'s `actionlint` comment, replacing the `~17-19s` figure. State the method (interleaved A/B) and that this wave added check 10. **Do not adjust by estimate.**

Also update the "ten fixture tables" enumeration in `moon.yml` to eleven, and the "eleven concurrent subprocesses" figure to twelve.

- [ ] **Step 3: Add the CLAUDE.md gotchas**

Append to the Gotchas section — each entry states what was measured, not what was assumed:

- release-plz owns every tag (`<package>-v<version>`, default); `napi prepublish` always carries `--no-gh-release`, a flag its own `--help` does not list; `ci/actionlint/release_guard.py` V5 asserts it.
- `release-plz release --dry-run` creates **no** tags (`create_git_tag_and_release` is reachable only from the non-dry-run arm, `release_plz_core` 0.36.14 `release.rs:888`/`:959`) — **but it still requires a git token**, because `get_git_client()` runs unconditionally at `release.rs:543`. `git_release_enable = false` does not suppress it.
- `release --output json` is `{"releases":[{package_name,prs,tag,version}]}` — `releases`/`package_name`, **not** `release-pr`'s `prs`/`package` shape.
- PyYAML parses `on:` as the boolean `True`, `if: false` and `continue-on-error: false` as `False`, and `continue-on-error: "false"` as a **string**; `needs:` may be a scalar string, and iterating it yields characters. Every workflow parser in `ci/` must handle all five.
- `wasm-pack build` **deletes `package.json`** in its `--out-dir` even with `--no-pack`. Never run it in the crate root; the release path uses `.wasmpack-release-out`, a third scratch dir beside `build`'s and `test`'s.
- `release.yml` must never gain a `pull_request`/`pull_request_target` trigger.
- GitHub Actions supports YAML **anchors and aliases** (since 2025-09-18) but **not merge keys**.

- [ ] **Step 4: Run the full gate graph**

Per-project tasks do not run `repo:*` gates. Use the marker-delimited command from CLAUDE.md:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :http-extractor-envelope :input-liveness :promtool :observability-drift \
  :nats-permissions :release-parity :release-parity-py :release-parity-ts \
  :publish-metadata :version-lockstep --base origin/main --include-relations
```

Diagnose any unattributed failure via `.moon/cache/ciReport.json`.

- [ ] **Step 5: Force-run the two cache-sensitive gates**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:actionlint --force
moon run repo:input-liveness --force
```

- [ ] **Step 6: Commit**

```bash
git add ci/actionlint/README.md moon.yml CLAUDE.md
git commit -m "docs(claude): record the release-path gotchas and re-measure the gate (SMA-579)"
```

---

## Self-Review

**Spec coverage:**

| Spec section | Task |
|---|---|
| §1 job graph, ordering, `plan` | 6, 7 |
| §1.3 the three premises | 1 |
| §1.4 concurrency | 6 (Step 1), 1 (Q4 measurement) |
| §2 tagging boundary | 9 (V5 in use), 11 |
| §2.1 GitHub Releases | 11 |
| §3 release job + JSON schema | 6 |
| §3.1 tag-push credential | 6 (second mint) |
| §4 credential isolation | 7 (Step 2 comment), 6, 8, 9 |
| §4.1 two environments | 6 (`approve-release`), 8, 9 |
| §4.2 crates.io OIDC | 6 |
| §4.3 PyPI OIDC | 8 |
| §4.4 npm token measurement | 1 (M4), 9 |
| §4.5 pre-flight table | 1 (Step 9 folds M4's result in) |
| §5.1-5.3 PyPI order, idempotency, binding | 8 |
| §5.4 paigasus-proto | 10, 8 (conditional upload) |
| §6 npm non-idempotency | 9 (Step 4 `npmstate`) |
| §7.1-7.6 npm packaging, wasm, prebuild | 9, 7 |
| §8.2-8.6 the verdict | 3 |
| §8.7 guard-the-guard | 4, 5 |
| §9 CI bookkeeping | 10, 12 |
| §9.1 pyyaml locked | 2 |
| §10 testing | every task's verification steps; 12 Step 4 |
| §11 rollback | documented in the spec; §6/§8 implement `skip-existing` and `npm view` |

**Placeholder scan:** no "TBD", no "add error handling", no "similar to Task N". Every code step carries the actual content. The two conditional branches (M1's fallback in Task 6, M4's in Task 9) both state the exact alternative rather than deferring it.

**Type consistency:** `release_guard_py` is the bash helper in Tasks 4 and 5; `check_main`/`check_called`/`is_gated`/`needs_of`/`if_text`/`coe_is_false`/`FIXTURES` are Task 3's names, used unchanged in Task 4's floor (`--fixture-count`) and Task 5's pins. `plan`'s five outputs are declared in Task 6 and consumed by name in Tasks 8 and 9 (`kernel_version`, `proto_release`). Artifact names are consistent across Tasks 7-9: `wheel-*`, `sdist`, `face-paigasus-kernel`, `proto-dist-py`, `prebuild-*`, `npm-dirs`, `wasm-dist`.

**One known gap, stated rather than hidden:** Task 1's M1 can delete the `plan` job, which changes Task 6 Step 3 and every `needs: plan` in Tasks 7-9 to a direct `if:` gate. The fallback is specified in spec §1.3b and repeated at Task 6 Step 3, so no re-design is required — but Tasks 6-9 must be read after M1 is known, not before.
