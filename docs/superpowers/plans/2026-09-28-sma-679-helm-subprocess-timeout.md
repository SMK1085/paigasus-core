# SMA-679 helm subprocess timeout Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bound every `helm` subprocess and the one `git` subprocess in `ci/helm-render/helm_render.py` with a timeout, so that a hang becomes a named infrastructure error (rc 2), not a silent 30-minute CI job kill.

**Architecture:** One helper `_run_helm(cmd, label, run=subprocess.run)` runs every helm command with `timeout=HELM_TIMEOUT_S` and maps `TimeoutExpired` and `OSError` to `InfraError`. `helm_template()` and `check7()` call it. `release_tags()` keeps its own direct `run(...)` call with `timeout=GIT_TIMEOUT_S`. Self-test rows prove each mapping, each call site's keyword, and (through an `ast` walk) that no other subprocess call exists.

**Tech Stack:** Python 3 (stdlib `subprocess`, `ast`, `time`), PyYAML, helm 3.22.0 from proto, bash (`ci/helm-render/run.sh`), ruff 0.16.8.

**Spec:** `docs/superpowers/specs/2026-09-28-sma-679-helm-subprocess-timeout-design.md`

## Global Constraints

- `HELM_TIMEOUT_S = 30` and `GIT_TIMEOUT_S = 30` (decisions A1, A2). The sizing row pins `max(HELM_TIMEOUT_S, GIT_TIMEOUT_S) * 8 <= 5 * 60`.
- A timeout or an `OSError` at helm start is `InfraError` (module rc 2), never a failed row. A non-zero helm exit in row 7 stays an assertion (rc 3).
- Only `ci/helm-render/helm_render.py` and `ci/helm-render/README.md` change. `run.sh`, `moon.yml`, `ci/affected-graph/ci_targets.py`, the chart and the fixtures do NOT change.
- `EXPECTED_ROW_LABELS` stays at 24 labels and unchanged. `HELM_RENDER_SH_CALL_SITES` stays unchanged.
- Stub runners are `def`, not assigned lambdas (ruff `E731`; `py/pyproject.toml` selects `E, F, W, I, N, UP, B, A, C4, SIM, TCH, RUF`). Write `X > 0`, not `0 < X` (ruff `SIM300`).
- Self-test rows read `kw.get("timeout")`, never `kw["timeout"]`.
- The README must not name the moon report file outside a moon-diagnosis marker block (actionlint check 12).
- Every source file keeps its SPDX header. Commits: conventional, scope `ci`, end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. No `--amend`, no `git reset`, no `--no-verify`.
- Mutation runs (Task 5) are authorized. Restore each mutation by reverting the exact edit with the Edit tool, never `git checkout --`. Never commit a mutation.
- Do not install host software. Prefix every shell with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` and `export PROTO_REPORTER=text`.

## Measured facts for this plan (2026-09-28, worktree on 213bc99d + spec commit)

- `helm template paigasus charts/paigasus --kube-version 1.31.0 -f ci/kind/values/a.yaml -f ci/kind/values/b.yaml` with helm `v3.22.0+g144ca65` took 0.03, 0.02 and 0.02 s wall time. SMA-694 did not change the order of magnitude. 30 s stays about 1000 times the measured time.
- An `ast` walk over the current module finds exactly three subprocess calls: `helm_template` line 264, `body` (in `check7`) line 645, `release_tags` line 843.
- `/bin/bash ci/helm-render/run.sh --self-test` passes on the base. It creates `ci/helm-render/.venv`, so `ci/helm-render/.venv/bin/python` exists after the first run.

- Pre-validation of this plan (2026-09-28): the plan author applied every code block of Tasks 1-4 to a scratch copy of the module outside the repository. The copy's `--self-test` passed (rc 0) and `ruff check --config py/pyproject.toml` passed. On that copy, M1-M7 each gave rc 3 with no traceback, and the red rows were exactly the ones Task 5 predicts. This is not the Task 5 record: the implementer must re-measure on the real file.

## Decisions this plan makes where the spec is silent or conflicts

1. **The D6 walk skips the `self_test` function.** The spec's `fast_run` stub (D5) calls the real `subprocess.run` inside `self_test`. A walk over the whole file counts that call, and the exact multiset `{_run_helm: 1, release_tags: 1}` can never hold. `self_test` is test code, not a production call path. A synthetic-source case pins the exemption.
2. **The D6 verdict is a function, `_subprocess_call_problems(source)`.** The self-test runs it on the real module source and on eight synthetic sources (guard the guard). So the walk is proved to bite on every CI run, not only in the manual mutation battery.
3. **The message row uses the label `render-label-x` and the command `helm-stub template x`.** The spec's label `stub` is a substring of `helm-stub`, so a message without the label still passed. The distinct needles close that gap.
4. **`helm_template`'s `label` is keyword-only and required:** `def helm_template(chart, enabled, extra=(), *, label, run=subprocess.run, helm=None)`. A caller that forgets the label fails with a `TypeError` at once.
5. **Labels for the 11 `helm_template` calls:** `subset iam`, `subset iam+gateway`, `3 base`, `3a t1`, `3a t2`, `3a-prime t1`, `3a-prime t2`, `3b before`, `3b after`, `3c iam only`, `8c fallback`.

## Review Focus

1. A real `TimeoutExpired` on POSIX carries `stderr` as `bytes` even with `text=True`, or as `None`. The helper must decode bytes with `errors="replace"` and accept `None`. Task 1's message row uses `bytes`; Task 1 also adds a `None` case.
2. A timeout in row 7 must NOT become a failed row. `_row()` catches only `ShapeError`, so the `InfraError` must propagate. Task 2's `check7 raises InfraError, not a failed row` row pins it.
3. A future subprocess call that is not `subprocess.run` (for example `subprocess.Popen` or `check_output`) bypasses the bound. Task 4's synthetic `a new Popen` case pins it.
4. A timeout keyword that is a literal (`timeout=30`) or the wrong constant slips past a "has a timeout" check. Task 4's `a literal timeout` and `the git constant in _run_helm` cases pin it.
5. The live path: a real hung helm on `PATH` under `main(["--chart", …])` must give module rc 2 with the D4 text on stderr. Task 5 Step 6 proves it by hand and records it in the README and the PR.

## File Structure

- `ci/helm-render/helm_render.py` — the only code file. New: `HELM_TIMEOUT_S`, `GIT_TIMEOUT_S` (constants block); `_run_helm`, `_SUBPROCESS_FUNCS`, `_is_subprocess_call`, `_subprocess_calls`, `_subprocess_call_problems` (render section); `expect_infra_strict` and one `SMA-679` self-test block (in `self_test`). Changed: `helm_template` signature and body, its 11 call sites, `check7`'s `body`, `release_tags`.
- `ci/helm-render/README.md` — D8 text: the exit-code table, the row-7 note, the delete-the-feature record, the residual risks.

---

### Task 1: The constants, `_run_helm` and its self-test rows

**Files:**
- Modify: `ci/helm-render/helm_render.py` (imports at lines 17-34; constants after line 51 `SENTINEL_HOST = …`; new helper before `def helm_template` at line 252; `self_test` at line 1102: `expect_infra_strict` after `expect_infra` at line 1114-1119, a new block after the row-7 block that ends at line 1420)

**Interfaces:**
- Consumes: `InfraError` (line 144), `self_test()`'s `failures` list.
- Produces: `HELM_TIMEOUT_S: int = 30`, `GIT_TIMEOUT_S: int = 30`, `_run_helm(cmd: list, label: str, run=subprocess.run) -> subprocess.CompletedProcess` (raises `InfraError`). In `self_test`: `expect_infra_strict(label: str, fn) -> InfraError | None`, stubs `timeout_run(cmd, **_kw)`, `oserror_run(cmd, **_kw)`, `fast_run(cmd, **kw)`. Tasks 2 and 3 add rows to the same `SMA-679` block and reuse these stubs.

- [ ] **Step 1: Add the `time` import**

In the import block, add `import time` between `import tempfile` and `import tomllib`. Do NOT add `import ast` yet: Task 4 adds it, and an unused import reds ruff (`F401`).

```python
import tempfile
import time
import tomllib
```

- [ ] **Step 2: Write the failing self-test rows**

In `self_test()`, directly after the `expect_infra` helper (the block that ends `failures.append(f"{label}: expected an infrastructure error (rc 2), got none")`), add:

```python
    def expect_infra_strict(label, fn):
        """Like expect_infra, but any other exception is a named failure, not a traceback that
        stops every later row (SMA-679). Returns the InfraError, or None."""
        try:
            fn()
        except InfraError as exc:
            return exc
        except Exception as exc:
            failures.append(f"{label}: expected InfraError, got {type(exc).__name__}: {exc}")
            return None
        failures.append(f"{label}: expected InfraError, got none")
        return None
```

After the row-7 block (after the line `expect_infra("check7 a missing b.yaml raises InfraError", …)` and before the comment `# ---- row inventory floor (F1)`), add:

```python
    # ---- SMA-679: every helm subprocess is bounded by HELM_TIMEOUT_S, the git one by
    # GIT_TIMEOUT_S. A timeout, or an OSError when helm starts, is rc 2, never a failed row.
    def timeout_run(cmd, **_kw):
        raise subprocess.TimeoutExpired(cmd, HELM_TIMEOUT_S, stderr=b"partial")

    def timeout_run_no_stderr(cmd, **_kw):
        raise subprocess.TimeoutExpired(cmd, HELM_TIMEOUT_S)

    def oserror_run(cmd, **_kw):
        raise PermissionError("stub")

    def fast_run(cmd, **kw):
        # The real kill-and-reap path of subprocess.run, with a short limit. D6 exempts self_test.
        kw["timeout"] = 0.5
        return subprocess.run(cmd, **kw)

    helm_cmd = ["helm-stub", "template", "x"]
    expect_infra_strict("helm timeout: _run_helm maps TimeoutExpired to InfraError",
                        lambda: _run_helm(helm_cmd, "render-label-x", run=timeout_run))
    exc = expect_infra_strict("helm timeout: the InfraError names the render, the command, the limit and the stderr",
                              lambda: _run_helm(helm_cmd, "render-label-x", run=timeout_run))
    msg = str(exc) if exc is not None else ""
    for needle in ("for render-label-x:", "helm-stub template x", "HELM_TIMEOUT_S", f"{HELM_TIMEOUT_S} s", "partial stderr: partial"):
        if needle not in msg:
            failures.append(f"helm timeout: the InfraError names the render, the command, the limit and the stderr: {needle!r} is not in {msg!r}")
    exc = expect_infra_strict("helm timeout: a TimeoutExpired with no stderr still gives InfraError",
                              lambda: _run_helm(helm_cmd, "render-label-x", run=timeout_run_no_stderr))
    if exc is not None and "partial stderr" in str(exc):
        failures.append(f"helm timeout: a TimeoutExpired with no stderr must not print a partial stderr, got {exc}")
    expect_infra_strict("helm timeout: _run_helm maps OSError to InfraError",
                        lambda: _run_helm(helm_cmd, "render-label-x", run=oserror_run))
    started = time.monotonic()
    expect_infra_strict("helm timeout: a real subprocess over the limit is killed and gives InfraError",
                        lambda: _run_helm([sys.executable, "-c", "import time; time.sleep(30)"], "sleep", run=fast_run))
    elapsed = time.monotonic() - started
    if elapsed >= 10:
        failures.append(f"helm timeout: a real subprocess over the limit is killed and gives InfraError: took {elapsed:.1f} s, want < 10 s")
    if not (HELM_TIMEOUT_S > 0 and GIT_TIMEOUT_S > 0 and max(HELM_TIMEOUT_S, GIT_TIMEOUT_S) * 8 <= 5 * 60):
        failures.append(f"helm timeout: 8 module runs fit far inside the job limit: HELM_TIMEOUT_S={HELM_TIMEOUT_S}, "
                        f"GIT_TIMEOUT_S={GIT_TIMEOUT_S}; 8 * max must be <= 300 s (spec D1). A larger value needs a new decision")
```

- [ ] **Step 3: Run the self-test to verify it fails**

Run:
```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-679-helm-subprocess-timeout && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && ci/helm-render/.venv/bin/python ci/helm-render/helm_render.py --self-test; echo "rc=$?"
```
(If `.venv` is missing, run `/bin/bash ci/helm-render/run.sh --self-test` once first; it creates it.)
Expected: `rc=1` and a traceback that ends in `NameError: name 'HELM_TIMEOUT_S' is not defined` (the needle tuple reads the constant directly).

- [ ] **Step 4: Add the constants**

After the line `SENTINEL_HOST = "gateway-sentinel.example.test"`, add:

```python

# SMA-679: the limit for ONE helm subprocess, in seconds. A render takes well under a second
# (measured 0.02-0.03 s). The task starts the module 8 times (7 negative-control fixtures + the real
# run), and each module run can time out once, so the worst case is 8 * HELM_TIMEOUT_S. That sum
# must stay far below the 30-minute CI job limit, which the whole moon ci graph shares. A call
# over the limit is an infrastructure error (rc 2), never a row failure.
HELM_TIMEOUT_S = 30
# SMA-679: the limit for the one git subprocess (release_tags), in seconds. It reads local refs
# only. It shares the HELM_TIMEOUT_S sizing rule: one module run can time out at most once, of
# either kind, because the first InfraError ends the run.
GIT_TIMEOUT_S = 30
```

- [ ] **Step 5: Add `_run_helm`**

Directly before `def helm_template(chart, enabled, extra=()):` (after the `# ---- render` banner), add:

```python
def _run_helm(cmd, label, run=subprocess.run):
    """Run one helm command with HELM_TIMEOUT_S (SMA-679). A timeout, or an OSError when helm
    starts, is InfraError (rc 2); the message names the render (label), the limit and the command.
    Returns the CompletedProcess; the caller reads returncode. `run` is a parameter only so
    self_test() can drive this with no helm; production never passes it."""
    try:
        return run(cmd, capture_output=True, text=True, check=False, timeout=HELM_TIMEOUT_S)
    except subprocess.TimeoutExpired as exc:
        # On POSIX, subprocess.run gives the partial output as bytes even with text=True.
        tail = exc.stderr.decode(errors="replace") if isinstance(exc.stderr, bytes) else (exc.stderr or "")
        tail = tail.strip()[-500:]
        raise InfraError(
            f"helm did not finish in {HELM_TIMEOUT_S} s (HELM_TIMEOUT_S) for {label}: "
            f"{' '.join(map(str, cmd))}" + (f"; partial stderr: {tail}" if tail else "")
        ) from exc
    except OSError as exc:
        raise InfraError(f"helm could not start for {label}: {exc}") from exc


```

- [ ] **Step 6: Run the self-test and ruff to verify they pass**

Run:
```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-679-helm-subprocess-timeout && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && ci/helm-render/.venv/bin/python ci/helm-render/helm_render.py --self-test; echo "rc=$?"; uv run --locked --project py ruff check --config py/pyproject.toml ci/helm-render/helm_render.py; echo "ruff rc=$?"
```
Expected: `== helm_render.py self-test passed ==`, `rc=0`, `All checks passed!`, `ruff rc=0`. The run takes about 0.5 s longer than before (the real-subprocess row).

- [ ] **Step 7: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-679-helm-subprocess-timeout && test "$(git branch --show-current)" = feature/sma-679-helm-subprocess-timeout && git add ci/helm-render/helm_render.py && git commit -m "feat(ci): add a bounded helm runner to repo:helm-render (SMA-679)

_run_helm runs one helm command with HELM_TIMEOUT_S (30 s) and maps
TimeoutExpired and OSError to InfraError (rc 2). GIT_TIMEOUT_S is the
matching limit for the git tag listing.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Route `helm_template` and `check7` through `_run_helm`

**Files:**
- Modify: `ci/helm-render/helm_render.py` (`helm_template` at line 252 before Task 1, now about line 290; its call sites in `check3` (lines 477-490 before Task 1) and `run_checks` (lines 937 and 954 before Task 1); `check7`'s `body` (line 645 before Task 1); the `SMA-679` self-test block from Task 1)

**Interfaces:**
- Consumes: `_run_helm(cmd, label, run=subprocess.run)`, `HELM_TIMEOUT_S`, the self-test stubs `timeout_run` and `expect_infra_strict` from Task 1.
- Produces: `helm_template(chart, enabled, extra=(), *, label: str, run=subprocess.run, helm: str | None = None) -> str`. `check7(chart, run=subprocess.run, helm=None, values_dir=KIND_VALUES)` keeps its signature.

- [ ] **Step 1: Write the failing self-test rows**

At the end of the `SMA-679` self-test block (after the sizing row from Task 1), add:

```python
    class _RecProc:
        def __init__(self):
            self.returncode, self.stdout, self.stderr = 0, "", ""

    with tempfile.TemporaryDirectory(prefix="helm-render-679-") as tmp:
        tmp_chart = Path(tmp) / "chart"
        tmp_chart.mkdir()
        (tmp_chart / "values.yaml").write_text("zones:\n  iam: {}\n  gateway: {}\n")
        (Path(tmp) / "a.yaml").write_text("{}\n")
        (Path(tmp) / "b.yaml").write_text("{}\n")
        expect_infra_strict("helm timeout: helm_template raises InfraError",
                            lambda: helm_template(tmp_chart, ("iam",), label="t", run=timeout_run, helm="helm-stub"))
        expect_infra_strict("helm timeout: check7 raises InfraError, not a failed row",
                            lambda: check7(tmp_chart, run=timeout_run, helm="helm-stub", values_dir=tmp))
        seen = []

        def recording_run(cmd, **kw):
            seen.append(kw)
            return _RecProc()

        expect_no_error = []
        try:
            helm_template(tmp_chart, ("iam",), label="t", run=recording_run, helm="helm-stub")
        except Exception as exc:
            expect_no_error.append(f"{type(exc).__name__}: {exc}")
        got = [kw.get("timeout") for kw in seen]
        if expect_no_error or got != [HELM_TIMEOUT_S]:
            failures.append(f"helm timeout: helm_template passes timeout=HELM_TIMEOUT_S: timeouts {got}, errors {expect_no_error}")
        seen.clear()
        check7(tmp_chart, run=recording_run, helm="helm-stub", values_dir=tmp)
        got = [kw.get("timeout") for kw in seen]
        if got != [HELM_TIMEOUT_S, HELM_TIMEOUT_S]:
            failures.append(f"helm timeout: check7 passes timeout=HELM_TIMEOUT_S on both renders: got {got}")
```

- [ ] **Step 2: Run the self-test to verify it fails**

Run:
```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-679-helm-subprocess-timeout && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && ci/helm-render/.venv/bin/python ci/helm-render/helm_render.py --self-test; echo "rc=$?"
```
Expected: `rc=3` with these failed rows:
- `helm timeout: helm_template raises InfraError: expected InfraError, got TypeError: helm_template() got an unexpected keyword argument 'label'`
- `helm timeout: check7 raises InfraError, not a failed row: expected InfraError, got TimeoutExpired: …`
- `helm timeout: helm_template passes timeout=HELM_TIMEOUT_S: timeouts [], errors ['TypeError: …']`
- `helm timeout: check7 passes timeout=HELM_TIMEOUT_S on both renders: got [None, None]`

- [ ] **Step 3: Change `helm_template`**

Replace the whole function:

```python
def helm_template(chart, enabled, extra=(), *, label, run=subprocess.run, helm=None):
    """One `helm template` of `chart` with exactly the zones in `enabled` switched on. `label`
    names the render in a timeout message (SMA-679): three pairs of renders have byte-identical
    commands. `run` and `helm` are parameters only so self_test() can drive this with no helm;
    production never passes them."""
    helm = helm or shutil.which("helm")
    if helm is None:
        raise InfraError("helm is not on PATH; run this module through ci/helm-render/run.sh")
    values = _chart_values(chart)
    cmd = [helm, "template", RELEASE, str(chart), "--kube-version", KUBE_VERSION]
    for key, value in STUB_VALUES:
        cmd += ["--set", f"{key}={value}"]
    for zone in sorted(values["zones"]):
        cmd += ["--set", f"zones.{zone}.enabled={'true' if zone in enabled else 'false'}"]
    cmd += list(extra)
    proc = _run_helm(cmd, label, run=run)
    if proc.returncode != 0:
        raise InfraError(f"helm template failed (rc={proc.returncode}) for {chart} {' '.join(extra)}: {proc.stderr.strip()}")
    return proc.stdout
```

- [ ] **Step 4: Pass a label at every call site**

In `check3`, replace the body from `base = …` to the `3c` line with:

```python
    base = parse_docs(helm_template(chart, both, label="3 base"))
    rows = []
    for row, key in (("3a", "zones.iam.console.image.tag"), ("3a-prime", "zones.gateway.console.image.tag")):
        before = parse_docs(helm_template(chart, both, ("--set", f"{key}=t1"), label=f"{row} t1"))
        after = parse_docs(helm_template(chart, both, ("--set", f"{key}=t2"), label=f"{row} t2"))
        rows.append(compare_templates(row, before, after))
    # Case b edits Chart.yaml in a temp COPY only. run.sh exports TMPDIR, so the copy lands under
    # the gate's own mktemp directory. SMA-688: both sides render with every tag cleared, so the
    # row still proves the appVersion fallback now that values.yaml pins each tag.
    with tempfile.TemporaryDirectory(prefix="helm-render-3b-") as tmp:
        bumped = _bumped_app_version(chart, Path(tmp) / "chart")
        before = parse_docs(helm_template(chart, both, CLEARED_TAGS, label="3b before"))
        rows.append(compare_templates("3b", before, parse_docs(helm_template(bumped, both, CLEARED_TAGS, label="3b after"))))
    rows.append(compare_templates("3c", base, parse_docs(helm_template(chart, ("iam",), label="3c iam only"))))
    return rows
```

In `run_checks`, change `raw = helm_template(chart, enabled)` to:

```python
        raw = helm_template(chart, enabled, label=f"subset {label}")
```

and change `fallback_docs = parse_docs(helm_template(chart, ("gateway", "iam"), CLEARED_TAGS))` to:

```python
    fallback_docs = parse_docs(helm_template(chart, ("gateway", "iam"), CLEARED_TAGS, label="8c fallback"))
```

Then confirm that no call site lacks a label:
```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-679-helm-subprocess-timeout && grep -n "helm_template(" ci/helm-render/helm_render.py | grep -v "label="
```
Expected: only the `def helm_template(` line.

- [ ] **Step 5: Change `check7`'s body**

In `check7`, replace `proc = run(cmd, capture_output=True, text=True, check=False)` with:

```python
            proc = _run_helm(cmd, f"7 {label}", run=run)
```

Extend the `check7` docstring's second paragraph with one sentence, so it reads:

```python
    A render that fails is an ASSERTION (the row fails, rc 3): the chart and the kind values
    disagree, which is exactly what this row exists to catch. A missing values file is rc 2, and
    so is a render that runs longer than HELM_TIMEOUT_S (SMA-679): a hung helm is a tool fault.
```

- [ ] **Step 6: Run the self-test, a real run and ruff**

Run:
```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-679-helm-subprocess-timeout && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && export PROTO_REPORTER=text && ci/helm-render/.venv/bin/python ci/helm-render/helm_render.py --self-test; echo "rc=$?"; H="$(proto --reporter text bin helm | tail -n1)"; PATH="$(dirname "$H"):$PATH" ci/helm-render/.venv/bin/python ci/helm-render/helm_render.py --chart charts/paigasus | tail -2; echo "chart rc=$?"; uv run --locked --project py ruff check --config py/pyproject.toml ci/helm-render/helm_render.py; echo "ruff rc=$?"
```
Expected: `self-test passed`, `rc=0`; `== helm-render: all 24 rows passed ==`; ruff `All checks passed!`. (The `chart rc` line shows `tail`'s status; the "all 24 rows passed" line is the verdict.)

- [ ] **Step 7: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-679-helm-subprocess-timeout && test "$(git branch --show-current)" = feature/sma-679-helm-subprocess-timeout && git add ci/helm-render/helm_render.py && git commit -m "feat(ci): bound every helm render in repo:helm-render (SMA-679)

helm_template and check7 now run helm through _run_helm, so a render
over HELM_TIMEOUT_S is InfraError (rc 2) that names the render. Row 7
still fails as an assertion on a non-zero helm exit.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Bound the `git for-each-ref` call in `release_tags`

**Files:**
- Modify: `ci/helm-render/helm_render.py` (`release_tags`, line 836 before Task 1; the `SMA-679` self-test block)

**Interfaces:**
- Consumes: `GIT_TIMEOUT_S` (Task 1), `timeout_run` and `expect_infra_strict` (Task 1), `_GitProc` (existing, defined in `self_test` before the row-7 block).
- Produces: `release_tags(run=subprocess.run) -> frozenset[str]`, unchanged signature; its one `run(...)` call passes `timeout=GIT_TIMEOUT_S`.

- [ ] **Step 1: Write the failing self-test rows**

At the end of the `SMA-679` self-test block (after the `with tempfile.TemporaryDirectory(prefix="helm-render-679-")` block from Task 2, at the same indentation as `helm_cmd = …`), add:

```python
    exc = expect_infra_strict("git timeout: release_tags maps TimeoutExpired to InfraError", lambda: release_tags(run=timeout_run))
    msg = str(exc) if exc is not None else ""
    for needle in ("GIT_TIMEOUT_S", f"{GIT_TIMEOUT_S} s", "for-each-ref"):
        if needle not in msg:
            failures.append(f"git timeout: release_tags maps TimeoutExpired to InfraError: {needle!r} is not in {msg!r}")
    git_seen = []

    def git_recording_run(cmd, **kw):
        git_seen.append(kw)
        return _GitProc(0, "paigasus-iam-v0.1.0\n")

    release_tags(run=git_recording_run)
    got = [kw.get("timeout") for kw in git_seen]
    if got != [GIT_TIMEOUT_S]:
        failures.append(f"git timeout: release_tags passes timeout=GIT_TIMEOUT_S: got {got}")
```

- [ ] **Step 2: Run the self-test to verify it fails**

Run:
```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-679-helm-subprocess-timeout && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && ci/helm-render/.venv/bin/python ci/helm-render/helm_render.py --self-test; echo "rc=$?"
```
Expected: `rc=3` with `git timeout: release_tags maps TimeoutExpired to InfraError: expected InfraError, got TimeoutExpired: …`, three needle failures for that row, and `git timeout: release_tags passes timeout=GIT_TIMEOUT_S: got [None]`.

- [ ] **Step 3: Change `release_tags`**

Replace the `try` block in `release_tags` with:

```python
    try:
        proc = run(cmd, capture_output=True, text=True, check=False, timeout=GIT_TIMEOUT_S)
    except FileNotFoundError as exc:
        raise InfraError(f"git is not on PATH: {exc}") from exc
    except subprocess.TimeoutExpired as exc:
        raise InfraError(
            f"git for-each-ref did not finish in {GIT_TIMEOUT_S} s (GIT_TIMEOUT_S): {' '.join(map(str, cmd))}"
        ) from exc
```

Append one sentence to the `release_tags` docstring, before the closing `"""`: `A listing over GIT_TIMEOUT_S is rc 2 (SMA-679).`

- [ ] **Step 4: Run the self-test and ruff to verify they pass**

Run:
```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-679-helm-subprocess-timeout && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && ci/helm-render/.venv/bin/python ci/helm-render/helm_render.py --self-test; echo "rc=$?"; uv run --locked --project py ruff check --config py/pyproject.toml ci/helm-render/helm_render.py; echo "ruff rc=$?"
```
Expected: `self-test passed`, `rc=0`, ruff `All checks passed!`.

- [ ] **Step 5: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-679-helm-subprocess-timeout && test "$(git branch --show-current)" = feature/sma-679-helm-subprocess-timeout && git add ci/helm-render/helm_render.py && git commit -m "feat(ci): bound the git tag listing in repo:helm-render (SMA-679)

release_tags passes timeout=GIT_TIMEOUT_S (30 s) and maps TimeoutExpired
to InfraError (rc 2) that names the command and the limit.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: The structural pin (D6): an `ast` walk over the module

**Files:**
- Modify: `ci/helm-render/helm_render.py` (imports; new functions directly after `_run_helm`; the `SMA-679` self-test block)

**Interfaces:**
- Consumes: the module after Tasks 2 and 3: exactly one subprocess call in `_run_helm` (with `timeout=HELM_TIMEOUT_S`) and one in `release_tags` (with `timeout=GIT_TIMEOUT_S`).
- Produces: `_SUBPROCESS_FUNCS: frozenset[str]`, `_is_subprocess_call(func: ast.expr) -> bool`, `_subprocess_calls(source: str) -> list[tuple[str, ast.Call]]`, `_subprocess_call_problems(source: str) -> list[str]` (empty list is a pass).

- [ ] **Step 1: Write the failing self-test rows**

At the end of the `SMA-679` self-test block (after the git rows from Task 3), add:

```python
    # D6: every subprocess call in the module goes through _run_helm or release_tags, each with its
    # own constant. The synthetic sources prove that the walk bites (guard the guard); they are
    # string constants, not Call nodes, so the walk cannot match them.
    d6_ok = (
        "def _run_helm(cmd, label, run=subprocess.run):\n"
        "    return run(cmd, timeout=HELM_TIMEOUT_S)\n"
        "def release_tags(run=subprocess.run):\n"
        "    return run(['git'], timeout=GIT_TIMEOUT_S)\n"
    )
    d6_cases = (
        ("the real module", Path(__file__).read_text(encoding="utf-8"), False),
        ("only the two bounded calls", d6_ok, False),
        ("a call inside self_test is exempt", d6_ok + "def self_test():\n    subprocess.run(['x'])\n", False),
        ("a reverted helm_template", d6_ok + "def helm_template(cmd):\n    return subprocess.run(cmd)\n", True),
        ("a reverted check7 body", d6_ok + "def check7(run):\n    def body():\n        return run(['helm'])\n    return body\n", True),
        ("a new Popen", d6_ok + "def f():\n    subprocess.Popen(['helm'])\n", True),
        ("a module-level check_output", d6_ok + "subprocess.check_output(['helm'])\n", True),
        ("no timeout in _run_helm", d6_ok.replace("run(cmd, timeout=HELM_TIMEOUT_S)", "run(cmd)"), True),
        ("a literal timeout in release_tags", d6_ok.replace("timeout=GIT_TIMEOUT_S", "timeout=30"), True),
        ("the git constant in _run_helm", d6_ok.replace("timeout=HELM_TIMEOUT_S", "timeout=GIT_TIMEOUT_S"), True),
    )
    for case, source, want_problem in d6_cases:
        got = _subprocess_call_problems(source)
        if bool(got) != want_problem:
            failures.append(f"helm timeout: every subprocess call goes through _run_helm or release_tags [{case}]: "
                            + ("expected a problem, got none" if want_problem else "; ".join(got)))
```

- [ ] **Step 2: Run the self-test to verify it fails**

Run:
```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-679-helm-subprocess-timeout && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && ci/helm-render/.venv/bin/python ci/helm-render/helm_render.py --self-test; echo "rc=$?"
```
Expected: `rc=1`, a traceback that ends in `NameError: name '_subprocess_call_problems' is not defined`.

- [ ] **Step 3: Add the import and the walk**

Add `import ast` between `import argparse` and `import copy`.

Directly after `_run_helm` (before `def helm_template`), add:

```python
_SUBPROCESS_FUNCS = frozenset({"run", "Popen", "call", "check_call", "check_output"})
# D6 (SMA-679): the only functions that may start a subprocess, each with its timeout constant.
_BOUNDED_CALLERS = {"_run_helm": "HELM_TIMEOUT_S", "release_tags": "GIT_TIMEOUT_S"}


def _is_subprocess_call(func):
    """True for `run(…)` or `subprocess.<run|Popen|call|check_call|check_output>(…)`."""
    if isinstance(func, ast.Name):
        return func.id == "run"
    return (isinstance(func, ast.Attribute) and isinstance(func.value, ast.Name)
            and func.value.id == "subprocess" and func.attr in _SUBPROCESS_FUNCS)


def _subprocess_calls(source):
    """(innermost enclosing function name, call) for every subprocess call in `source`. The body
    of `self_test` is skipped: its fast_run stub calls the real subprocess.run on purpose."""
    found = []

    def visit(node, fn_name):
        for child in ast.iter_child_nodes(node):
            if isinstance(child, (ast.FunctionDef, ast.AsyncFunctionDef)):
                if child.name != "self_test":
                    visit(child, child.name)
                continue
            if isinstance(child, ast.Call) and _is_subprocess_call(child.func):
                found.append((fn_name, child))
            visit(child, fn_name)

    visit(ast.parse(source), "<module>")
    return found


def _subprocess_call_problems(source):
    """D6 (SMA-679): the problems with the subprocess calls in `source`; an empty list is a pass.
    Exactly one call in each of _BOUNDED_CALLERS, each with `timeout=<its constant>`. A call
    through an alias (`from subprocess import run as r`, `os.system`) is not seen."""
    calls = _subprocess_calls(source)
    names = sorted(name for name, _call in calls)
    problems = []
    if names != sorted(_BOUNDED_CALLERS):
        problems.append(f"expected exactly one subprocess call in each of {sorted(_BOUNDED_CALLERS)}, found calls in {names}. "
                        "Route every helm call through _run_helm")
    for name, call in calls:
        want = _BOUNDED_CALLERS.get(name)
        if want is None:
            continue
        timeout = next((k.value for k in call.keywords if k.arg == "timeout"), None)
        if not (isinstance(timeout, ast.Name) and timeout.id == want):
            problems.append(f"the subprocess call in {name} (line {call.lineno}) must pass timeout={want}")
    return problems
```

- [ ] **Step 4: Run the self-test and ruff to verify they pass**

Run:
```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-679-helm-subprocess-timeout && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && ci/helm-render/.venv/bin/python ci/helm-render/helm_render.py --self-test; echo "rc=$?"; ci/helm-render/.venv/bin/python -c 'import sys; sys.path.insert(0, "ci/helm-render"); import helm_render as h; print([(n, c.lineno) for n, c in h._subprocess_calls(open("ci/helm-render/helm_render.py").read())])'; uv run --locked --project py ruff check --config py/pyproject.toml ci/helm-render/helm_render.py; echo "ruff rc=$?"
```
Expected: `self-test passed`, `rc=0`; the list shows exactly two entries, `('_run_helm', <line>)` and `('release_tags', <line>)`; ruff `All checks passed!`.

- [ ] **Step 5: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-679-helm-subprocess-timeout && test "$(git branch --show-current)" = feature/sma-679-helm-subprocess-timeout && git add ci/helm-render/helm_render.py && git commit -m "test(ci): pin every repo:helm-render subprocess call to its timeout (SMA-679)

An ast walk asserts exactly one subprocess call in _run_helm and one in
release_tags, each with timeout= its own constant. Synthetic sources
prove the walk reds on a reverted call site, a new Popen and a literal
or wrong timeout.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Delete-the-feature battery, live proof and README

**Files:**
- Modify (temporarily, never committed): `ci/helm-render/helm_render.py`
- Modify: `ci/helm-render/README.md` (exit-code table line 60; row-7 table line 40; delete-the-feature record after line 128; residual risks after line 163)

**Interfaces:**
- Consumes: the finished module from Tasks 1-4.
- Produces: README text only.

For each mutation: make the ONE edit with the Edit tool, run the two commands below, record the `rc` and every `FAIL` line, then revert the exact edit with the Edit tool and run `git diff --stat ci/helm-render/helm_render.py` (expected: empty). Do not use `git checkout --`.

The commands for each mutation:
```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-679-helm-subprocess-timeout && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && export PROTO_REPORTER=text && ci/helm-render/.venv/bin/python -c 'import sys; sys.path.insert(0, "ci/helm-render"); import helm_render' && echo "imports ok"; ci/helm-render/.venv/bin/python ci/helm-render/helm_render.py --self-test; echo "module rc=$?"; /bin/bash ci/helm-render/run.sh --self-test > "${TMPDIR:-/tmp}"/sma679-st.log 2>&1; echo "run.sh rc=$?"; grep -E "FAIL|passed|FAILED" "${TMPDIR:-/tmp}"/sma679-st.log | tail -5
```
Every mutation must print `imports ok`, `module rc=3` (no traceback) and `run.sh rc=1`.

- [ ] **Step 1: M1 — drop the timeout in `_run_helm`**

Edit: in `_run_helm`, `check=False, timeout=HELM_TIMEOUT_S)` → `check=False)`.
Expected red rows: `helm timeout: helm_template passes timeout=HELM_TIMEOUT_S`, `helm timeout: check7 passes timeout=HELM_TIMEOUT_S on both renders`, `helm timeout: every subprocess call goes through _run_helm or release_tags [the real module]`. The real-subprocess row stays green (`fast_run` sets its own timeout; that row proves the kill path, not the keyword). Revert.

- [ ] **Step 2: M2 — `_run_helm` no longer catches `TimeoutExpired`**

Edit: in `_run_helm`, `except subprocess.TimeoutExpired as exc:` → `except subprocess.CalledProcessError as exc:`.
Expected red rows, each with `expected InfraError, got TimeoutExpired`: `_run_helm maps TimeoutExpired to InfraError`, `the InfraError names the render, …` (plus its needle rows), `a TimeoutExpired with no stderr still gives InfraError`, `a real subprocess over the limit is killed and gives InfraError`, `helm_template raises InfraError`, `check7 raises InfraError, not a failed row`. Revert.

- [ ] **Step 3: M3 and M4 — revert one call site each**

M3 edit: in `helm_template`, `proc = _run_helm(cmd, label, run=run)` → `proc = run(cmd, capture_output=True, text=True, check=False)`.
Expected red: `helm timeout: helm_template raises InfraError` (got `TimeoutExpired`), `helm timeout: helm_template passes timeout=HELM_TIMEOUT_S`, the D6 `[the real module]` row. Revert.

M4 edit: in `check7`'s `body`, `proc = _run_helm(cmd, f"7 {label}", run=run)` → `proc = run(cmd, capture_output=True, text=True, check=False)`.
Expected red: `helm timeout: check7 raises InfraError, not a failed row` (got `TimeoutExpired`), `helm timeout: check7 passes timeout=HELM_TIMEOUT_S on both renders`, the D6 `[the real module]` row. Revert.

- [ ] **Step 4: M5 — an oversized limit**

Edit: `HELM_TIMEOUT_S = 30` → `HELM_TIMEOUT_S = 120`.
Expected red: `helm timeout: 8 module runs fit far inside the job limit`. Revert.

- [ ] **Step 5: M6 and M7 — the git call**

M6 edit: in `release_tags`, `check=False, timeout=GIT_TIMEOUT_S)` → `check=False)`.
Expected red: `git timeout: release_tags passes timeout=GIT_TIMEOUT_S`, the D6 `[the real module]` row. Revert.

M7 edit: in `release_tags`, `except subprocess.TimeoutExpired as exc:` → `except subprocess.CalledProcessError as exc:`.
Expected red: `git timeout: release_tags maps TimeoutExpired to InfraError` (got `TimeoutExpired`), plus its needle rows. Revert.

After M7's revert, run `git diff --stat ci/helm-render/helm_render.py` (expected: empty) and the self-test once more (expected: `rc=0`). If the Edit tool or the permission system refuses a mutation, restore the file, write the exact edit and its predicted rows under "Mutation proof pending" in the task report, and go on.

If any mutation gives a result other than the prediction, stop and diagnose before the README step. If a fix follows, re-run the whole battery M1-M7, not only the failed one.

- [ ] **Step 6: Live timeout proof**

Run (the stub is created in the scratchpad and never committed; `exec` makes the stub itself the process that `subprocess.run` kills):
```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-679-helm-subprocess-timeout && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && STUB="$(mktemp -d)" && printf '#!/bin/sh\nexec sleep 5\n' > "$STUB/helm" && chmod +x "$STUB/helm" && PATH="$STUB:$PATH" ci/helm-render/.venv/bin/python -c 'import sys; sys.path.insert(0, "ci/helm-render"); import helm_render as h; h.HELM_TIMEOUT_S = 1; sys.exit(h.main(["--chart", "charts/paigasus"]))'; echo "rc=$?"; rm -rf "$STUB"
```
Expected: `rc=2` after about 1 s, and stderr `INFRA  helm-render: helm did not finish in 1 s (HELM_TIMEOUT_S) for subset iam: <stub dir>/helm template paigasus <repo>/charts/paigasus --kube-version 1.31.0 --set …`. Record the exact line for the README and the PR body.

- [ ] **Step 7: Update the README**

In `ci/helm-render/README.md`:

(a) Exit-code table, replace the row-2 line with:

```markdown
| 2 | infrastructure error (a failed render, an unparseable source, a missing tool, a chart script's 127 or 141, fewer than seven chart scripts; a `helm` call inside `helm_render.py` that runs longer than `HELM_TIMEOUT_S` (30 s) or cannot start (`OSError`); the `git for-each-ref` tag listing when it runs longer than `GIT_TIMEOUT_S` (30 s)) |
```

(b) Checks table, in the `7 kind-values` row, append to the last cell: ` A render that runs longer than HELM_TIMEOUT_S is rc 2, not a failed row (SMA-679)`. The cell then ends `… exits non-zero. A missing values file is rc 2. A render that runs longer than `HELM_TIMEOUT_S` is rc 2, not a failed row (SMA-679) |`.

(c) At the end of "Delete-the-feature record" (after the row-8c paragraph), add a paragraph with the measured results. Template (fill in the measured rc and rows from Steps 1-6; do not copy the predictions if a measurement differs):

```markdown
The helm and git timeouts (SMA-679) were measured on <date>. Each mutation imports, and makes
`helm_render.py --self-test` exit `rc=3` and `bash ci/helm-render/run.sh --self-test` exit
`rc=1`, with no traceback. M1 (no `timeout=` in `_run_helm`): <rows>. M2 (`_run_helm` catches
`CalledProcessError`, not `TimeoutExpired`): <rows>. M3 (`helm_template` calls `run` directly):
<rows>. M4 (`check7` calls `run` directly): <rows>. M5 (`HELM_TIMEOUT_S = 120`): <rows>. M6
(no `timeout=` in `release_tags`): <rows>. M7 (`release_tags` catches `CalledProcessError`):
<rows>. A stub `helm` that sleeps, with `HELM_TIMEOUT_S = 1` set in the process, made
`main(["--chart", "charts/paigasus"])` return 2 with `<the INFRA line>`.
```

(d) "Residual risks": add these items after item 5:

```markdown
6. Only the helm and git calls inside `helm_render.py` have a timeout (SMA-679). `run.sh`'s
   `resolve_helm` (`helm version --short`) and the helm calls in the seven chart scripts have
   none, and a systemic helm hang hangs there first. SMA-722 owns them and proposes a moon
   `options.timeout` on `repo:helm-render`.
7. The task starts the module 8 times (7 negative-control fixtures and the real run). Each module
   run can lose at most one timeout, so a hang in every render costs 8 × 30 s = 240 s of the
   30-minute CI job. The self-test pins `8 × max(HELM_TIMEOUT_S, GIT_TIMEOUT_S) <= 300 s`.
8. `subprocess.run` kills only the direct child, not a process group. `helm template` starts no
   child process with no plugin loaded (`run.sh` sets `HELM_DATA_HOME` to an empty directory).
9. The self-test's `ast` walk does not see a subprocess call through an alias
   (`from subprocess import run as r`) or through `os.system`, and it skips `self_test` itself.
10. 30 s is a judgment, not a measurement on a CI runner. A render took 0.02-0.03 s on the
    development Mac (helm 3.22.0, 2026-09-28).
```

Check that the README does not name the moon report file:
```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-679-helm-subprocess-timeout && grep -n 'ci[R]eport' ci/helm-render/README.md; echo "grep rc=$? (1 = not found, correct)"
```

- [ ] **Step 8: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-679-helm-subprocess-timeout && test "$(git branch --show-current)" = feature/sma-679-helm-subprocess-timeout && git diff --stat && git add ci/helm-render/README.md && git commit -m "docs(ci): record the repo:helm-render timeouts and their mutation proof (SMA-679)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```
Expected before the commit: `git diff --stat` shows only `ci/helm-render/README.md`.

---

### Task 6: Full gate verification

**Files:** none changed, unless a gate finds a defect (then fix in the file that owns it, re-run the Task 5 battery, and commit with a `fix(ci): …` message).

**Interfaces:**
- Consumes: the branch after Task 5.
- Produces: a verdict per gate for the PR body.

- [ ] **Step 1: Prove the pinned files did not change**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-679-helm-subprocess-timeout && git diff --stat origin/main -- ci/helm-render/run.sh moon.yml ci/affected-graph/ charts/ ci/helm-render/fixtures/; echo "---"; git diff --stat origin/main
```
Expected: the first diff is empty. The second lists only `ci/helm-render/helm_render.py`, `ci/helm-render/README.md`, the spec and this plan.

- [ ] **Step 2: The helm-render gate, all three modes, under bash 3.2**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-679-helm-subprocess-timeout && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && export PROTO_REPORTER=text && /bin/bash ci/helm-render/run.sh --self-test > "${TMPDIR:-/tmp}"/sma679-a.log 2>&1; echo "self-test rc=$?"; /bin/bash ci/helm-render/run.sh --negative-control > "${TMPDIR:-/tmp}"/sma679-b.log 2>&1; echo "negative-control rc=$?"; /bin/bash ci/helm-render/run.sh > "${TMPDIR:-/tmp}"/sma679-c.log 2>&1; echo "real rc=$?"; tail -3 "${TMPDIR:-/tmp}"/sma679-a.log "${TMPDIR:-/tmp}"/sma679-b.log "${TMPDIR:-/tmp}"/sma679-c.log
```
Expected: all three `rc=0`; `negative-control OK` for each of the 7 fixtures; the real run ends with `all 24 rows passed` and every chart script green. If the real run gives rc 2 from `maps.sh` with 141, the host is in the 512-byte small-pipe state (SMA-612). That is a host artifact, not a finding: record it, and rely on CI (or the Linux-container workaround in the memory note "Small pipe: run gates in a Linux container").

- [ ] **Step 3: The other gates that this edit selects**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-679-helm-subprocess-timeout && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && export PROTO_REPORTER=text && moon query tasks --affected 2>/dev/null | python3 -c 'import json,sys; d=json.load(sys.stdin); print(sorted(f"{p}:{t}" for p, ts in d["tasks"].items() for t in ts))'
```
Expected: the list holds `repo:helm-render`, `repo:affected-smoke`, `repo:ruff-ci` and `repo:actionlint` (other tasks may also appear; run each of them too).

Run each gate with the bash it needs (see the root `CLAUDE.md` "This development Mac only"):
```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-679-helm-subprocess-timeout && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && export PROTO_REPORTER=text && for m in --self-test --negative-control ""; do /bin/bash ci/affected-graph/run.sh $m >> "${TMPDIR:-/tmp}"/sma679-d.log 2>&1; echo "affected-smoke $m (bash 3.2) rc=$?"; done; for m in --self-test --negative-control ""; do /opt/homebrew/bin/bash ci/ruff/run.sh $m >> "${TMPDIR:-/tmp}"/sma679-e.log 2>&1; echo "ruff-ci $m (bash 5) rc=$?"; done; /opt/homebrew/bin/bash ci/actionlint/run.sh > "${TMPDIR:-/tmp}"/sma679-f.log 2>&1; echo "actionlint (bash 5) rc=$?"; grep -m1 "pipe capacity" "${TMPDIR:-/tmp}"/sma679-f.log; tail -2 "${TMPDIR:-/tmp}"/sma679-d.log "${TMPDIR:-/tmp}"/sma679-e.log "${TMPDIR:-/tmp}"/sma679-f.log
```
Expected: rc 0 on every line. These are the three modes that the `script:` blocks of `repo:affected-smoke` and `repo:ruff-ci` in `moon.yml` run (read them if a mode differs). Remove the two `sma679-d.log` and `sma679-e.log` files before a re-run, because the loops append. If actionlint exits 2 with a "pipe holds only 512 bytes" message, the host is in the small-pipe state: record it and rely on CI or a Linux container. A known local artifact: under `/bin/bash` 3.2 actionlint prints two false `cargo-lock-step` rows; do not run it under 3.2.

- [ ] **Step 4: Record the verdicts**

Write the rc of each gate, the pipe-capacity line and any host artifact into the task report, for the PR body. Do not commit a log file.

---

## Self-review record

- Spec coverage: D1 → Task 1 Step 4. D2 → Task 1 Step 5 and Task 2. D3 → Task 2 (row `check7 raises InfraError, not a failed row`; docstring). D4 → Task 1 message row, Task 3 git message row, Task 5 Step 6. D5 → Tasks 1-3 rows (all 11 labels, plus the no-stderr row). D6 → Task 4 (with decision 1: `self_test` skipped). D7 → Task 6 Step 1 (no change to `EXPECTED_ROW_LABELS`, `run.sh`, `ci_targets.py`). D8 → Task 5 Step 7. D9 → Task 3. AC 1 → Task 5 Step 6 plus Task 2. AC 2 → Task 4. AC 3 → Task 3 and Task 4. AC 4 → Task 6 Step 2. AC 5 → Task 5 Steps 1-5. AC 6 → Task 6.
- The follow-up issue for the unbounded paths already exists (SMA-722, decision A3). No task files it again.
- Names used across tasks: `_run_helm`, `HELM_TIMEOUT_S`, `GIT_TIMEOUT_S`, `expect_infra_strict`, `timeout_run`, `oserror_run`, `fast_run`, `_RecProc`, `recording_run`, `git_recording_run`, `_subprocess_calls`, `_subprocess_call_problems`, `_BOUNDED_CALLERS`. Each is defined in exactly one task before its first use.
