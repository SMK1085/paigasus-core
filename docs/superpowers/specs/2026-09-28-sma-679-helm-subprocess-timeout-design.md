# SMA-679: bound every `helm` subprocess in `repo:helm-render` with a timeout

- Linear: SMA-679 (related: SMA-513, PR 296; follow-up: SMA-722)
- Path: bounded (one Python module, its README; no gate wiring change)
- Date: 2026-09-27 (draft), 2026-09-28 (approved, rebased on `origin/main` 213bc99d)
- Status: approved by Sven on 2026-09-28 with the decisions below

## Approval decisions (Sven, 2026-09-28)

These decisions override the draft defaults. The sections below already include them.

| # | Decision | Effect on this spec |
|---|---|---|
| A1 | `HELM_TIMEOUT_S = 30` s. | D1 stays as written. Open question 3 is answered. |
| A2 | The `git for-each-ref` call in `release_tags()` also gets a timeout. | Scope is wider than the issue text. New D9, new self-test rows, new mutations M6-M7, D6 assertion 2 covers both calls. Open question 1 is answered. The Linear issue has a "Scope extended on 2026-09-28" section. |
| A3 | File ONE follow-up issue for the remaining unbounded helm paths (the chart scripts and `resolve_helm`'s `version --short`), with a proposal for a moon `options.timeout` on `repo:helm-render`. | Filed as SMA-722 (related to SMA-679). Open question 2 is answered. These paths stay out of scope here. |
| A4 | Issue AC 1 is met when the module exits 2 and `run.sh` exits 2 after the chart scripts finish. | AC 1 stays as written. Open question 4 is answered. |
| A5 | SMA-694 (HTTPRoute, merged) changed the chart and the `ci/helm-render` fixtures: rebase the spec on the current code. | See "Rebase on `origin/main` (2026-09-28)" at the end. |

## Problem

`ci/helm-render/helm_render.py` starts `helm template` in two places. Neither passes `timeout`:

| Call site | Line on `origin/main` | Callers |
|---|---|---|
| `helm_template()` — `subprocess.run(cmd, capture_output=True, text=True, check=False)` | 264 | `run_checks()` (2 subset renders, 1 row-8c render) and `check3()` (8 renders): 11 calls per real run |
| `check7()` body — `run(cmd, capture_output=True, text=True, check=False)`, `run` defaults to `subprocess.run` | 645 | row `7 kind-values`: 2 calls per real run |

The module also starts one `git` process with no `timeout` (decision A2):

| Call site | Line on `origin/main` | Callers |
|---|---|---|
| `release_tags()` — `run(cmd, capture_output=True, text=True, check=False)`, `run` defaults to `subprocess.run`; `cmd` is `git -C <repo> for-each-ref …` | 843 | `run_checks()` (line 932): 1 call per real run |

A `helm` that hangs (a blocked filesystem read, a future helm defect, a render that loops on some
chart content) blocks the gate until the `moon ci` job limit (`ci.yml:24`: `timeout-minutes: 30`).
The job then dies with no named cause, and the `buffer-only-failure` output style shows nothing
for the task. A stuck helm plugin cannot cause this in this gate: `run.sh:62` sets
`HELM_DATA_HOME` to an empty `mktemp` directory, so no plugin loads. A `git` that hangs (for
example on a locked or network-mounted `.git`) has the same effect.

CodeRabbit raised this on PR 296 (`helm_render.py:615`, row 7 only). It was pushed back because a
timeout only in row 7 makes the module inconsistent. This issue does it for the whole module.

The issue is not fixed on `origin/main`: `git log origin/main --grep=SMA-679` is empty, and the
module has no `timeout` and no `TimeoutExpired` anywhere (read 2026-09-27, re-read 2026-09-28 on
213bc99d).

### What this issue bounds, and what it does not

This change bounds the 13 helm calls and the 1 git call inside `helm_render.py`. It does NOT make
the whole gate unable to hang. These helm processes stay unbounded (see "Residual risk"). The
follow-up issue SMA-722 owns them (decision A3):

- `run.sh:80`, `resolve_helm`: `"$bin" version --short`. It is the first helm process in every
  `run.sh` mode, so a systemic hang (for example a helm that hangs on start) hangs there, before
  the module runs.
- The seven chart scripts under `charts/paigasus/tests/` (49 lines that mention `helm` in total:
  `env.sh` 25, `ingress.sh` 7, `ca-bundle.sh` 6, `names.sh` 4, `render.sh` 3, `refusals.sh` 3,
  `maps.sh` 1; the same count method as the draft, which counted 43 before SMA-694 added renders
  to `ingress.sh` and `refusals.sh`). `real_run()` (`run.sh:151-158`) runs them even after the
  module returns 2.

So the change converts to rc 2 only a hang that occurs first inside `helm_render.py`, typically
one that depends on chart content or on the flags of one render, or a hang of the tag listing.

## Measured facts (2026-09-27, this Mac, helm 3.22.0 from proto)

- `helm template paigasus charts/paigasus --kube-version 1.31.0 -f ci/kind/values/a.yaml -f
  ci/kind/values/b.yaml` took 0.02-0.03 s wall time, three runs, rc 0, 334 output lines. (This
  was measured before SMA-694. The HTTPRoute templates are off by default, so the order of
  magnitude does not change. The plan re-measures it on the current chart.)
- The other renders use the same chart and `--set` flags only, so they are the same order of
  magnitude. The slowest real call is below 0.1 s on this host. CI runners are slower, but not by
  three orders of magnitude.
- The `repo:helm-render` task (`moon.yml:1004-1008`) runs `--self-test`, then
  `--negative-control`, then the real run. The negative control runs the module once for each of
  the 7 fixtures under `ci/helm-render/fixtures/` and continues to the next fixture on rc 2
  (`run.sh:212-246`). So one task run starts the module 8 times with real helm (7 + 1).

## Design

### D1. One constant

```python
# SMA-679: the limit for ONE helm subprocess, in seconds. A render takes well under a second
# (measured 0.03 s). The task starts the module 8 times (7 negative-control fixtures + the real
# run), and each module run can time out once, so the worst case is 8 * HELM_TIMEOUT_S. That sum
# must stay far below the 30-minute CI job limit, which the whole moon ci graph shares. A call
# over the limit is an infrastructure error (rc 2), never a row failure.
HELM_TIMEOUT_S = 30
```

Placed next to `KUBE_VERSION`. 30 s is about 1000 times the measured time. Inside ONE module run,
only one call can time out, because the first `InfraError` ends that run. Across the task, a hang
in every render costs 8 × 30 s = 240 s, which is 4 of the 30 minutes. (The first draft chose
120 s. That gives 16 minutes in the worst case, more than half of the job budget, and a value of
600 s would exceed the job limit. The rule is the one `chart.yml:59-67` applies: size a limit to
the SUM of the parts.) Sven confirmed 30 s on 2026-09-28 (decision A1).

### D2. One helper for every helm call

```python
def _run_helm(cmd, label, run=subprocess.run):
    """Run one helm command with HELM_TIMEOUT_S. A timeout or an OSError when helm starts is
    InfraError (rc 2); the message names the render (label), the limit and the command. Returns
    the CompletedProcess; the caller reads returncode."""
    try:
        return run(cmd, capture_output=True, text=True, check=False, timeout=HELM_TIMEOUT_S)
    except subprocess.TimeoutExpired as exc:
        tail = exc.stderr.decode(errors="replace") if isinstance(exc.stderr, bytes) else (exc.stderr or "")
        tail = tail.strip()[-500:]
        raise InfraError(
            f"helm did not finish in {HELM_TIMEOUT_S} s (HELM_TIMEOUT_S) for {label}: "
            f"{' '.join(map(str, cmd))}" + (f"; partial stderr: {tail}" if tail else "")
        ) from exc
    except OSError as exc:
        raise InfraError(f"helm could not start for {label}: {exc}") from exc
```

- `label` names the render. The command alone does not identify it: three pairs of renders give
  byte-identical commands (SUBSETS `iam` and row 3c; SUBSETS `iam+gateway` and the check3 base; the
  3b "before" render and the 8c fallback; lines 477/488/490/937/954). `helm_template()` gains a
  `label` argument, and each caller passes a short name (for example `"3b before"`,
  `"8c fallback"`, `"subset iam"`). `check7()` passes `f"7 {label}"` (`"7 a.yaml"`,
  `"7 a.yaml + b.yaml"`).
- `exc.stderr` holds what helm printed before the kill. On POSIX `subprocess.run` gives it as
  bytes even with `text=True`, so the helper decodes with `errors="replace"`.
- `OSError` covers, for example, `PermissionError` on the helm path. `release_tags()` already maps
  `FileNotFoundError` for git. Today such an error is a traceback; after this change it is a named
  `INFRA` line. `TimeoutExpired` is not an `OSError`, so the order of the clauses has no effect.
- `helm_template()` (line 252, today `def helm_template(chart, enabled, extra=())`) calls
  `_run_helm(cmd, label, run=run)`. It gains the `label` argument and two keyword parameters,
  `run=subprocess.run` and `helm=None`, with the same contract as `check7()`: only `self_test()`
  passes them, production never does. `helm=None` keeps the `shutil.which("helm")` lookup and
  its `InfraError`.
- `check7()`'s body calls `_run_helm(cmd, f"7 {label}", run=run)` in place of `run(...)`.
- After the change, `helm_render.py` holds exactly two direct subprocess calls: the one in
  `_run_helm` and the git call in `release_tags()`. D6 pins this, and D9 bounds the git call.

`subprocess.run` with `timeout` sends SIGKILL to the direct child (helm) and reaps it before it
raises `TimeoutExpired`. It does not kill a process group, so a process that helm itself started
can survive. helm does not start child processes for `helm template` with no plugins loaded, so
this does not apply here in practice.

### D3. Row 7 semantics stay as they are

A non-zero helm exit in row 7 is still an ASSERTION (row fails, rc 3). A timeout in row 7 is an
`InfraError` (rc 2), like a timeout anywhere else. The `InfraError` propagates out of `_row()`
(it catches only `ShapeError`) to `main()`, which prints `INFRA  helm-render: …` and returns 2.
`run.sh`'s `module_rc` maps 2 to 2.

Rationale: a hung helm says nothing about whether the chart and the kind values agree. It is a
fault of the tool, which is the definition of rc 2 in the module docstring.

### D4. The message

The message names the render, the limit, the constant and the full command, for example:
`INFRA  helm-render: helm did not finish in 30 s (HELM_TIMEOUT_S) for 7 a.yaml + b.yaml:
/…/helm template paigasus /…/charts/paigasus --kube-version 1.31.0 -f … -f …; partial stderr: …`.

For git (D9): `INFRA  helm-render: git for-each-ref did not finish in 30 s (GIT_TIMEOUT_S):
git -C /… for-each-ref --format=%(refname:lstrip=2) refs/tags/paigasus-*`.

### D5. Self-test rows (in `self_test()`, beside the row-7 block)

Stub runners are written as `def`, not as assigned lambdas: ruff-ci applies
`py/pyproject.toml`'s rule set, which selects `E` (E731).

- `timeout_run(cmd, **kw)`: raises `subprocess.TimeoutExpired(cmd, HELM_TIMEOUT_S, stderr=b"partial")`.
- `oserror_run(cmd, **kw)`: raises `PermissionError("stub")`.
- `recording_run(cmd, **kw)`: appends `(cmd, kw)` to a list and returns a new `_RecProc` with
  `returncode = 0`, `stdout = ""` and `stderr = ""`. The existing `_Proc` (line 1397) has no
  `stdout`, and `helm_template` returns `proc.stdout`, so the new rows need their own class.
- `fast_run(cmd, **kw)`: sets `kw["timeout"] = 0.5` and calls the real `subprocess.run(cmd, **kw)`.

A new helper `expect_infra_strict(label, fn)` catches `InfraError` (pass) and every other
`Exception` (a named failure: `expected InfraError, got <type>: <message>`). With it, a mutation
that lets `TimeoutExpired` or `KeyError` escape gives a named failed row, not a traceback that
also stops every row after it. The existing `expect_infra` (line 1114) stays as it is.

The new rows use their own temporary directory: the row-7 block deletes `b.yaml` at line 1419.
It holds `a.yaml`, `b.yaml` and a minimal chart directory with a `values.yaml` that has a `zones`
mapping (for `_chart_values`).

| Row label | What it proves |
|---|---|
| `helm timeout: _run_helm maps TimeoutExpired to InfraError` | `expect_infra_strict` on `_run_helm(["helm-stub"], "stub", run=timeout_run)` |
| `helm timeout: the InfraError names the render, the command, the limit and the stderr` | The message contains `stub`, `helm-stub`, `HELM_TIMEOUT_S`, `str(HELM_TIMEOUT_S)` and `partial` |
| `helm timeout: _run_helm maps OSError to InfraError` | `expect_infra_strict` on `_run_helm(["helm-stub"], "stub", run=oserror_run)` |
| `helm timeout: a real subprocess over the limit is killed and gives InfraError` | `expect_infra_strict` on `_run_helm([sys.executable, "-c", "import time; time.sleep(30)"], "sleep", run=fast_run)`; also asserts it returns in less than 10 s. This proves on every CI run that the real kill and reap path and a real `TimeoutExpired` map to `InfraError`. |
| `helm timeout: helm_template raises InfraError` | `expect_infra_strict` on `helm_template(tmp_chart, ("iam",), label="t", run=timeout_run, helm="helm-stub")` |
| `helm timeout: check7 raises InfraError, not a failed row` | `expect_infra_strict` on `check7(tmp_chart, run=timeout_run, helm="helm-stub", values_dir=tmp)` |
| `helm timeout: helm_template passes timeout=HELM_TIMEOUT_S` | `recording_run` sees one call with `kw.get("timeout") == HELM_TIMEOUT_S` |
| `helm timeout: check7 passes timeout=HELM_TIMEOUT_S on both renders` | `recording_run` sees two calls, each with `kw.get("timeout") == HELM_TIMEOUT_S` |
| `git timeout: release_tags maps TimeoutExpired to InfraError` | `expect_infra_strict` on `release_tags(run=timeout_run)`; the message contains `GIT_TIMEOUT_S` and `for-each-ref` (D9) |
| `git timeout: release_tags passes timeout=GIT_TIMEOUT_S` | a recording runner that returns a `_GitProc(0, "paigasus-iam-v0.1.0\n")` sees one call with `kw.get("timeout") == GIT_TIMEOUT_S` (D9) |
| `helm timeout: 8 module runs fit far inside the job limit` | `0 < HELM_TIMEOUT_S`, `0 < GIT_TIMEOUT_S` and `max(HELM_TIMEOUT_S, GIT_TIMEOUT_S) * 8 <= 5 * 60` |

The rows read `kw.get("timeout")`, not `kw["timeout"]`, so a deleted keyword gives a named failure,
not a `KeyError`.

The three "passes timeout" rows exist because of the guard-the-guard lesson: the
`TimeoutExpired`-raising rows alone stay green if a call site stops passing `timeout`, since a
stub raises no matter what it receives.

The last row pins the D1 sizing rule. The bound allows up to 37 s for either constant; a larger
value needs a new decision, not a silent edit. It uses `max(…)` because the first `InfraError`
ends a module run, so one module run can lose at most one timeout of either kind (D9).

The existing `ok_run` and `lambda cmd, **_kw: _Proc(...)` stubs for row 7, and the
`lambda cmd, **_kw: _GitProc(...)` and `git_missing(cmd, **_kw)` (line 1361) stubs for
`release_tags` (lines 1361-1371), already accept `**kwargs`, so they keep working with the new
`timeout` keyword.

### D6. Structural pin (in `self_test()`): an `ast` walk, not a text match

One more row: `helm timeout: every subprocess call goes through _run_helm or release_tags`.

It parses the module's own source with `ast.parse(Path(__file__).read_text())`. It walks every
`FunctionDef` and records, with the name of the innermost enclosing function, every `ast.Call`
whose `func` is:

- `Name(id="run")`, or
- `Attribute(value=Name(id="subprocess"), attr=…)` with `attr` in
  `{"run", "Popen", "call", "check_call", "check_output"}`.

It asserts:

1. The multiset of enclosing function names is exactly `{"_run_helm": 1, "release_tags": 1}`.
2. The call in `_run_helm` has a `timeout` keyword whose value is `Name(id="HELM_TIMEOUT_S")`,
   and the call in `release_tags` has a `timeout` keyword whose value is
   `Name(id="GIT_TIMEOUT_S")` (decision A2).

On `origin/main` 213bc99d the same walk finds exactly three calls: line 264 in `helm_template`,
line 645 in `body` (inside `check7`) and line 843 in `release_tags` (measured 2026-09-28).

String constants are not `Call` nodes, so the row cannot match its own needle (the text match in
the first draft did, like the check-13 self-scan trap in `ci/actionlint/`). A `run=subprocess.run`
default is an `Attribute` in a default, not a `Call`, so it needs no exemption. The row catches:
a reverted `helm_template` (a `subprocess.run(` call in `helm_template`), a reverted `check7` (a
`run(` call in `body`), a new `subprocess.Popen` helm call anywhere, and a deleted `timeout=` in
`_run_helm` or in `release_tags`. Its failure message prints the found multiset and says to route
every helm call through `_run_helm`.

Limit: a call through an alias (`from subprocess import run as r`, `os.system`) is not caught.
No such import exists in the module, and adding one is a visible review item.

### D7. `EXPECTED_ROW_LABELS` and `ci/affected-graph/ci_targets.py`

- `EXPECTED_ROW_LABELS` (line 116) lists the rows of a real `run_checks()` call. This change adds
  no real row, so the constant and its arity pin (24, line 1424) do not change. The new rows are
  self-test rows only.
- `ci_targets.py` pins `HELM_RENDER_SH_CALL_SITES` (lines 1412-1449 on 213bc99d), which are lines
  of `run.sh`, not of `helm_render.py`. `run.sh` does not change, so no pin changes.
  `repo:affected-smoke` still re-keys on `ci/helm-render/**/*` (`moon.yml:257`) and must stay
  green.

### D8. README (`ci/helm-render/README.md`)

- "Exit codes" table (line 54), row 2: add "a `helm` call inside `helm_render.py` that runs longer
  than `HELM_TIMEOUT_S` (30 s), or that cannot start (`OSError`); the `git for-each-ref` tag listing
  when it runs longer than `GIT_TIMEOUT_S` (30 s)".
- Under the row-7 check (the `7 kind-values` row of the "Checks" table, line 40): one sentence. A
  timeout in row 7 is rc 2, not a failed row.
- "Residual risks" (line 151): add the unbounded helm paths (`resolve_helm`'s `version --short`,
  the chart scripts) with a pointer to SMA-722, the 8-module-runs multiplier with its worst case
  (240 s), and the process-group limit from D2.
- "Delete-the-feature record" (line 95): add the measured M1-M7 results (rc, failing row names,
  date).
- The README must not name the moon report file. If a new sentence needs it, it must sit inside
  a moon-diagnosis marker block (actionlint check 12).

### D9. `release_tags()` gets a timeout (decision A2)

```python
# SMA-679: the limit for the one git subprocess (release_tags), in seconds. It reads local refs
# only. It shares the D1 sizing rule: one module run can time out at most once, of either kind.
GIT_TIMEOUT_S = 30
```

Placed next to `HELM_TIMEOUT_S`. In `release_tags()`:

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

- The call stays a direct `run(...)` call in `release_tags`. It does not go through `_run_helm`,
  because that helper's message says "helm" and uses `HELM_TIMEOUT_S`. D6 pins the git call by
  its own constant.
- The existing `FileNotFoundError` clause stays as it is. A wider `OSError` mapping for git is not
  part of this issue.
- The `run` parameter keeps its contract: only `self_test()` passes it.
- Worst case: `release_tags()` runs once per module run (line 932), before the subset renders. The
  first `InfraError` ends the run, so a git timeout and a helm timeout cannot both occur in one
  module run. The task worst case stays 8 × 30 s = 240 s.

## Out of scope

- The chart scripts under `charts/paigasus/tests/*.sh` and `run.sh`'s `resolve_helm` call. They
  call helm from bash, macOS has no `timeout` binary, and the issue names `helm_render.py`. This
  is a real gap (see "What this issue bounds"). SMA-722 owns it (decision A3).
- A moon-level `options.timeout` on `repo:helm-render`. It is the one mechanism that bounds the
  whole task, including the paths above. It is left out of this issue for these reasons: it edits
  `moon.yml`, which re-keys many repo gates and sits next to the four `script:` lines that
  `SELF_SCHEDULED_GATES` pins; it needs its own sizing (the full task, not one helm call); and a
  moon timeout gives the `timed-out` status with no exit code, which is a different diagnosis
  path. SMA-722 proposes it (decision A3).
- A wider `OSError` mapping in `release_tags()` (see D9).

## Acceptance criteria

1. A `helm` call in `helm_render.py` that exceeds `HELM_TIMEOUT_S` makes the module exit 2 with a
   message that names the render, the command and the limit. In a real `run.sh` run, the result
   is exit 2 when the chart scripts then finish, because `real_run()` returns the worst rc. (Issue
   AC 1, as Sven confirmed in decision A4.)
2. Every helm subprocess in `helm_render.py` goes through `_run_helm`, which passes
   `timeout=HELM_TIMEOUT_S`. The D6 `ast` row enforces it.
3. The `git for-each-ref` call in `release_tags()` passes `timeout=GIT_TIMEOUT_S`, and a timeout
   there makes the module exit 2 with a message that names the command and the limit. The D5 git
   rows and the D6 row enforce it. (Decision A2.)
4. `run.sh --self-test` passes with the D5 and D6 rows. (Issue AC 2, first half.)
5. Delete-the-feature: each mutation M1-M7 below compiles, and makes `run.sh --self-test` exit
   non-zero with the predicted named rows failing (no traceback). (Issue AC 2, second half.)
6. `run.sh`, `run.sh --negative-control`, `repo:affected-smoke`, `repo:ruff-ci` and
   `repo:actionlint` stay green. `EXPECTED_ROW_LABELS` and `HELM_RENDER_SH_CALL_SITES` are
   unchanged.

## Test strategy

- **Unit (self-test):** the D5 and D6 rows, run through `bash ci/helm-render/run.sh --self-test`
  (it needs the proto helm and the uv venv; the gate runs under bash 3.2 and 5).
- **Delete-the-feature battery.** Apply each mutation alone, run `--self-test`, record the rc and
  the failing rows, then restore by reverting the single edit (not `git checkout --`, which also
  reverts the fix under test). Re-run the whole battery after any later fix. Each mutation is a
  literal edit that compiles:
  - M1: in `_run_helm`, change `check=False, timeout=HELM_TIMEOUT_S)` to `check=False)`. Expect
    the two helm "passes timeout" rows and the D6 row red. The real-subprocess row stays green,
    because `fast_run` sets its own `timeout`; that row proves the kill path, not the keyword.
  - M2: in `_run_helm`, change `except subprocess.TimeoutExpired as exc:` to
    `except subprocess.CalledProcessError as exc:`. Expect the `_run_helm maps TimeoutExpired`,
    message, real-subprocess, `helm_template raises` and `check7 raises` rows red, each with
    "expected InfraError, got TimeoutExpired".
  - M3: in `helm_template`, change `proc = _run_helm(cmd, label, run=run)` to
    `proc = run(cmd, capture_output=True, text=True, check=False)`. Expect the helm_template
    "passes timeout" row, the helm_template `raises InfraError` row and the D6 row red.
  - M4: in `check7`'s `body`, change `proc = _run_helm(cmd, f"7 {label}", run=run)` to
    `proc = run(cmd, capture_output=True, text=True, check=False)`. Expect the check7 "passes
    timeout" row, the check7 `raises InfraError` row and the D6 row red.
  - M5: change `HELM_TIMEOUT_S = 30` to `HELM_TIMEOUT_S = 120`. Expect the sizing row red.
  - M6: in `release_tags`, change `check=False, timeout=GIT_TIMEOUT_S)` to `check=False)`. Expect
    the `release_tags passes timeout` row and the D6 row red.
  - M7: in `release_tags`, change `except subprocess.TimeoutExpired as exc:` to
    `except subprocess.CalledProcessError as exc:`. Expect the `release_tags maps TimeoutExpired`
    row red with "expected InfraError, got TimeoutExpired".
  A mutation that fails at import proves nothing about a row; confirm each one imports.
- **Live timeout proof (manual, recorded in the PR):** in a Python process from the repo root,
  import the module, set `h.HELM_TIMEOUT_S = 1`, put a stub `helm` that runs `sleep 5` first on
  `PATH`, and call `h.main(["--chart", "charts/paigasus"])`. Expect return value 2 and the D4 text
  on stderr. Do not use a scratch copy of the module (its `REPO_ROOT = parents[2]` breaks, which
  gives a different rc 2), and do not edit the file in place. Do not commit the stub.
- **Full gate:** `moon run repo:helm-render repo:affected-smoke repo:ruff-ci repo:actionlint`
  (an edit to `ci/helm-render/helm_render.py` selects the last two too). Mind the bash split in
  `CLAUDE.md`: `repo:ruff-ci` needs bash 4+, `repo:affected-smoke` needs bash 3.2, and
  `repo:actionlint` needs bash 5 and a healthy pipe. `run.sh` itself is not edited.
- **Host note:** this Mac can be in the 512-byte small-pipe state (SMA-612). In that state
  `maps.sh` in `repo:helm-render` gets SIGPIPE locally. That is a host artifact, and CI is the
  verdict. Run bash-5 gates in a Linux container when the pipe preflight fails.

## Files expected to change

- `ci/helm-render/helm_render.py` — `HELM_TIMEOUT_S`, `GIT_TIMEOUT_S`, `_run_helm`,
  `helm_template(…, label, run=, helm=)` and its callers' labels, `check7` body, `release_tags`
  timeout (D9), `expect_infra_strict`, self-test rows D5 and D6.
- `ci/helm-render/README.md` — D8.

No change expected in `run.sh`, `moon.yml`, `ci/affected-graph/ci_targets.py`, the chart, or the
fixtures.

## Residual risk

- The gate can still hang the job. `resolve_helm`'s `version --short` (`run.sh:80`) and the helm
  lines in the chart scripts have no bound. A systemic helm hang hangs there, with no named
  cause. Only a moon `options.timeout` or a bash-side bound closes this. SMA-722 owns it.
- The limit is a judgment, not a measurement on a CI runner. A runner that is 1000 times slower
  than this Mac would red the gate with rc 2. That is unlikely, and the message names the cause.
- Worst case inside the task: 8 × 30 s = 240 s for a hang in every render of every module run.
- `subprocess.run` kills only the direct child, not a process group (D2).
- The D6 row does not see a subprocess call through an alias or through `os.system`.

## Open questions

All four are answered (Sven, 2026-09-28).

1. Should `release_tags()`'s `git for-each-ref` call also get a timeout? **Answered: yes
   (decision A2).** See D9. D6 now pins its `timeout=GIT_TIMEOUT_S`.
2. Should the unbounded helm paths (the chart scripts, `resolve_helm`'s `version --short`) get a
   bound, and should that bound be a moon `options.timeout` on `repo:helm-render`? **Answered:
   one follow-up issue, SMA-722, proposes a moon `options.timeout` on `repo:helm-render`
   (decision A3).** Not in this issue.
3. Is 30 s the limit Sven wants? **Answered: yes, `HELM_TIMEOUT_S = 30` (decision A1).**
   `GIT_TIMEOUT_S` uses the same value.
4. Is issue AC 1 ("run.sh exits 2") met by this spec's reading: the module exits 2, and `run.sh`
   exits 2 when the chart scripts finish? **Answered: yes (decision A4).** A hang in a chart
   script is not covered here; SMA-722 owns it.

## Challenge changelog

Verdict: **APPROVE WITH CHANGES** (spec-challenger, 2026-09-27). Each finding was checked against
the code on `main` (`helm_render.py`, `run.sh`, `moon.yml`, `ci.yml`, `chart.yml`, the README,
`py/pyproject.toml`).

Folded:

- BLOCKER, D6 matches its own needle and misses the `run(` shape: confirmed (lines 623, 645, 836,
  843). D6 is now an `ast` walk with an exact multiset `{_run_helm: 1, release_tags: 1}` and a
  `timeout=Name("HELM_TIMEOUT_S")` check.
- MAJOR, M2 is a syntax error and M1-M3 give tracebacks: confirmed (`expect_infra` at line 1114
  catches only `InfraError`; README lines 107-108 record the same trap). Mutations are now literal
  edits that compile; new `expect_infra_strict` records other exceptions as named failures; rows
  read `kw.get("timeout")`. M5 was added for the sizing row.
- MAJOR, the limit ignores the negative-control loop: confirmed (`moon.yml:1004-1008`,
  `run.sh:212-246`, 7 fixtures). The limit is now 30 s, sized to 8 module runs, and the sizing
  row is `HELM_TIMEOUT_S * 8 <= 5 * 60`. The D1 sentence is corrected.
- MAJOR, the motivating failure is only partly addressed: confirmed (`run.sh:80`, `run.sh:151-158`,
  the helm lines in the chart scripts). A "What this issue bounds" section, the residual-risk
  list, and a stated reason for leaving `options.timeout` out were added.
- MINOR, the command does not identify the row: confirmed. `_run_helm` and `helm_template` take a
  `label`.
- MINOR, add partial stderr: folded, decoded with `errors="replace"`.
- MINOR, wrong causes in the problem statement: confirmed (`run.sh:62`). The plugin cause is
  removed; the orphan sentence is corrected to the direct child only.
- MINOR, the live proof can give rc 2 for the wrong reason: folded. It now imports the module in
  a process. A permanent real-subprocess row was added; it uses a `fast_run` wrapper that sets
  `timeout=0.5`, so `_run_helm` needs no `timeout` parameter and D6 stays exact.
- MINOR, the D5 text does not match the code: confirmed (`_Proc` has no `stdout`; `b.yaml` is
  deleted at line 1419; ruff selects `E`). Fixed.
- MINOR, the verification list is incomplete: folded (`repo:ruff-ci`, `repo:actionlint`).
- MINOR, the D8 README edits are too small: folded (residual risks, delete-the-feature record).
- QUESTION, map `OSError` to `InfraError`: folded. It turns a traceback into a named `INFRA` line,
  consistent with `release_tags()`.
- QUESTION, AC 1 meaning: answered in AC 1 and kept as open question 4 for Sven (now answered,
  decision A4).

Rejected:

- Add `options.timeout` on `repo:helm-render` in this issue: rejected for this issue. It edits
  `moon.yml`, needs a separate whole-task sizing and a different diagnosis path, and the issue
  names `helm_render.py`. It is now the follow-up SMA-722.
- Give `_run_helm` a `timeout` parameter for the real-subprocess row: rejected. The `fast_run`
  wrapper gets the same proof and keeps the D6 check on `Name("HELM_TIMEOUT_S")` exact.

## Rebase on `origin/main` (2026-09-28)

The spec was checked against `origin/main` 213bc99d, which includes SMA-694 (023755e4, HTTPRoute)
and SMA-695 (4051df5e, `ingress.enabled`). Changes made:

- The chart-script helm line count changed from 43 to 49: `ingress.sh` 2 → 7 and `refusals.sh`
  2 → 3. The other five scripts are unchanged.
- The negative-control fixture loop is at `run.sh:212-246`, not `run.sh:220-245`.
- `HELM_RENDER_SH_CALL_SITES` is at `ci_targets.py:1412-1449`, not 1385-1419.
- The `chart.yml` job-sizing comment is at lines 59-67, not 59-66.
- Added line anchors that the draft did not give: `helm_template` line 252, `EXPECTED_ROW_LABELS`
  line 116 and its arity pin line 1424, the `release_tags` stubs lines 1364-1371, README section
  lines.
- Confirmed unchanged: `helm_render.py` lines 252/264/474-491/623/645/836/843/932/937/954/1114/
  1397/1419; the 7 fixtures; `EXPECTED_ROW_LABELS` still has 24 labels; `run.sh:62`, `:80`,
  `:151-158`; `moon.yml:1004-1008`; `ci.yml:24`; the `ast` walk finds exactly three subprocess
  calls. SMA-694 added no subprocess call to the module.
- The "Measured facts" render time predates SMA-694. The plan re-measures it.
- Scope changes from the approval decisions: D9, AC 3, rows for git, M6-M7, the sizing row with
  `max(…)`, the D6 assertion for `GIT_TIMEOUT_S`, and the SMA-722 pointers.
