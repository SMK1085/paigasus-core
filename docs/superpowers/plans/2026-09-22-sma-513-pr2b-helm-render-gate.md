# SMA-513 PR 2b — repo:helm-render gate Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the `repo:helm-render` gate. It renders `charts/paigasus` with the pinned `helm`, asserts checks 1, 1a, 2, 3 and 4 in one Python module, runs the six chart scripts (checks 5 and 6), and proves with six overlay fixtures that each check can fail.

**Architecture:** `ci/helm-render/run.sh` is the three-mode entry point (`--self-test`, `--negative-control`, real run). It resolves `helm` through proto and the Python interpreter through a dedicated uv project ONCE, then runs `ci/helm-render/helm_render.py` and every `charts/paigasus/tests/*.sh` with those two first on `PATH`. The module exits 0, 3 or 2. The wrapper maps 3 to 1 and every other non-zero code to 2. The gate registers with the ten obligations of spec § 7.

**Tech Stack:** bash (3.2.57 and 5.x), Python 3.12, PyYAML 6.0.3 through uv 0.11.16, Helm 3.22.0 through proto 0.61.1, Moon 2.5.3.

**Spec:** `docs/superpowers/specs/2026-09-22-sma-513-pr2b-helm-render-gate-design.md` (revision 2). Parent: `docs/superpowers/specs/2026-09-19-sma-513-multi-zone-ingress-helm-design.md` § 8.

**Measured before this plan was written.** Every code block in Tasks 1–8 ran in a scratch copy of the repository on 2026-09-22: the module self-test, the real run (20 rows), the negative control (6 of 6 OK), all three modes under `/bin/bash` 3.2.57 and `/opt/homebrew/bin/bash` 5.3.15 (host pipe capacity 65536 bytes), the delete-the-feature mutations, and `ci_targets.py --self-test` with every registration edit applied. Where a step gives an expected output, that output is the measured one.

## Global Constraints

- Work only in `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-513-helm-render-gate`. Before the first commit, run `git branch --show-current` and confirm `feature/sma-513-helm-render-gate`.
- `SCRATCH=/private/tmp/claude-501/sma-513-pr2b` is the scratch directory for helper scripts. Create it with `mkdir -p`. Never write helper scripts into the repository.
- The worktree sandbox refuses compound shell commands. Put a multi-command step into a script file under `$SCRATCH` and run it with `/bin/bash <file>`.
- Prefix tool commands with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- Every new source file starts with an SPDX header: `# SPDX-License-Identifier: Apache-2.0` for shell, Python and TOML; `<!-- SPDX-License-Identifier: Apache-2.0 -->` for Markdown; chart templates keep the `{{/* SPDX-License-Identifier: Apache-2.0 */}}` line they copy from the live chart.
- `run.sh` starts with `set -euo pipefail`, then `export PROTO_REPORTER=text` (SMA-609 standing rule).
- bash 3.2 and bash 5 compatibility: no `mapfile`, no `declare -A`, no here-string (`<<<`), and a `"${A[@]+"${A[@]}"}"` guard on every array that can be empty.
- No pipe into an early-exit reader (`grep -q`, `grep -m`, `head`, `awk … exit`). `ci/actionlint/run.sh` check 13 bans it in every tracked `*.sh` and in `moon.yml`. Grep a file, or use process substitution.
- Exit codes: the gate returns 0 pass, 1 assertion failure, 2 infrastructure error. `helm_render.py` returns 0, **3** for an assertion failure and 2 for an infrastructure error. `run.sh` maps module 3 to 1 and any other non-zero module code to 2. A chart script's 1 is 1; any other non-zero chart-script code is 2.
- Capture a step that can `exit 2` with `x=0; step || x=$?`, never inside `$( )` (errexit swallows nested exit codes).
- Read-only rule: the gate writes only under one `mktemp -d` directory that a `trap` removes, plus `ci/helm-render/.venv` (ignored by `.gitignore:32`). It never writes into `charts/`, never calls `render.sh --update`, and never runs `uv` inside a fixture directory.
- Every render uses `--kube-version 1.31.0`.
- `ci/helm-render/pyproject.toml` pins `pyyaml==6.0.3`.
- `helm` is the version pinned in `.prototools` (`helm = "3.22.0"`, which `helm version --short` prints as `v3.22.0+g144ca65`).
- Do NOT change any file under `charts/paigasus/templates/`, `charts/paigasus/tests/*.sh` or `charts/paigasus/tests/golden/`. If a check finds a real chart defect, stop and report it.
- Conventional commits. commitlint REQUIRES a scope from `rs, py, ts, contracts, ci, docs, deps, release, repo, claude, workspace`. Use `feat(ci): …`, `test(ci): …` or `docs(ci): …`.
- A commit body must not contain a line that starts with `#NNN` or with `token: value`. Use a subject line and the trailer only.
- Every commit ends with the trailer `Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>`. Commit with `git commit -m "<subject>" -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"`.
- Add, don't amend. Never `git commit --amend`, never `--no-verify`.
- Do not write the two ci-targets marker comments, or quote them, anywhere except where they already are in the root `CLAUDE.md`.
- Do not name Moon's CI report JSON file (the `.moon/cache` report that `moon ci` writes) in any new file. `ci/actionlint/run.sh` check 12 reds a file that names it without a marker.

---

## File Structure

| File | Action | Responsibility |
| -- | -- | -- |
| `ci/helm-render/pyproject.toml` | Create | The one-dependency uv project (`pyyaml==6.0.3`) |
| `ci/helm-render/uv.lock` | Create (generated) | Locks PyYAML 6.0.3 |
| `ci/helm-render/helm_render.py` | Create | Checks 1, 1a, 2, 3, 4; the in-process self-test; exit 0/3/2 |
| `ci/helm-render/run.sh` | Create | The three modes, `helm` and venv resolution, the chart-script loop (checks 5 and 6), the negative control, the rc maps |
| `ci/helm-render/fixtures/literal-ingress/templates/ingress.yaml` | Create | Overlay: both ingress rules written literally |
| `ci/helm-render/fixtures/zones-omits-enabled/templates/_helpers.tpl` | Create | Overlay: `PAIGASUS_ZONES` skips an enabled `gateway` |
| `ci/helm-render/fixtures/leaked-value/templates/console-env-configmap.yaml` | Create | Overlay: a data entry carries `zones.gateway.backend.url` |
| `ci/helm-render/fixtures/template-only-diff/templates/console-deployment.yaml` | Create | Overlay: every console reads `zones.iam`'s image tag |
| `ci/helm-render/fixtures/slug-mirror/templates/_helpers.tpl` | Create | Overlay: `paigasus.serviceSlugs` lists `billing` too |
| `ci/helm-render/fixtures/security-context/templates/console-deployment.yaml` | Create | Overlay: the console pod drops `runAsNonRoot` |
| `ci/helm-render/README.md` | Create | Checks, fixture table with measured rows, exit codes, read-only rule, spec § 10 hazards |
| `moon.yml` | Modify | New `repo:helm-render` task (append after line 967); `repo:affected-smoke` input (after line 248); `repo:osv` input (after line 49) |
| `.github/workflows/ci.yml` | Modify | Line 268: `:helm-render` in `T` |
| `CLAUDE.md` | Modify | Line 145: `:helm-render` in the ci-targets block |
| `ci/affected-graph/ci_targets.py` | Modify | `REQUIRED_REPO_TASKS` (186-208), `SELF_TASK_EXPECTED_GLOBS` (373-380), `SELF_SCHEDULED_GATES` (634-640), new `HELM_RENDER_SH_CALL_SITES` (after 1321), `check_self_invocation` (1691-1801), self-test fixtures and rows (2159-2175, 2628-2630, 2631-3230, 2952-2955, before 3233), `_CALL_SITE_SOURCE_KEYS` (3653-3656), `collect_findings` (3682-3685), `main()` (3914-3941) |
| `ci/actionlint/run.sh` | Modify | `T_AFFECTED_SMOKE_REQUIRED_INPUTS` (2127-2162): one entry after line 2159 |
| `ci/osv/run.sh` | Modify | `LOCKFILES` (33-37) |
| `.github/dependabot.yml` | Modify | New uv block after line 94 |
| `charts/paigasus/README.md` | Modify | "Running the tests" (112-126): name all six scripts |

---

### Task 1: The uv project and the `helm_render.py` skeleton

**Files:**
- Create: `ci/helm-render/pyproject.toml`
- Create: `ci/helm-render/uv.lock` (generated by `uv lock`)
- Create: `ci/helm-render/helm_render.py`
- Create (scratch, not in the repo): `$SCRATCH/hr.sh`

**Interfaces:**
- Produces: `class InfraError(Exception)`, `class ShapeError(Exception)`, `@dataclass(frozen=True) class Row(row: str, ok: bool, detail: str = "")`, `_get(obj, *path)`, `_row(row: str, fn) -> Row`, `_of_kind(docs, kind) -> list`, `_one(items, what)`, `_name(doc) -> str`, `_configmap_with(docs, key) -> dict`, `_json_map(docs, key) -> dict`, `_app_label(dep) -> str`, `_deployments(docs, label) -> list`, `_containers(dep) -> list`, `_env(container) -> dict`, `_service(docs, name) -> dict`, `_selects(service, dep) -> bool`, `_selected_deployment(docs, service) -> dict`, `_service_port(service, name) -> dict`, `helm_template(chart, enabled, extra=()) -> str`, `parse_docs(text) -> list[dict]`, `_chart_values(chart) -> dict`, `base_paths(chart) -> dict[str, str]`, `run_checks(chart) -> list[Row]`, `report(rows, out=None) -> int`, `synthetic(zones=("gateway", "iam"), tags=None, backend_tag="0.0.0") -> list[dict]`, `_find(docs, kind, name) -> dict`, `self_test() -> int`, `main(argv=None) -> int`; constants `REPO_ROOT`, `KUBE_VERSION`, `RELEASE`, `SENTINEL_URL`, `SENTINEL_HOST`, `STUB_VALUES`, `SUBSETS`, `_SYN_IAM`, `_SYN_CONSOLE`, `_SYN_INGRESS`, `_SYN_PATH`, `_SYN_PATHS`, `_SYN_PROTO`, `_SYN_HELPERS`.
- Produces: two insertion anchors that Tasks 2–5 use. In the module: the line `# --------------------------------------------------------------------------- run`. In `self_test()`: the line `    # ---- the exit-code contract and the parser's infrastructure errors`.

- [ ] **Step 1: Write the uv project and lock it**

`ci/helm-render/pyproject.toml`:

```toml
# SPDX-License-Identifier: Apache-2.0
# A DEDICATED one-dependency project, the same layout as ci/workflow-credentials/ (SMA-593). py/
# is a uv workspace whose members build a PyO3 cdylib, so `uv run --project py` would put a
# maturin build on every chart-edit PR. run.sh resolves this project's interpreter ONCE, with
# --locked, and runs everything else through it (spec § 4).
[project]
name = "paigasus-helm-render"
version = "0.0.0"
description = "Static render checks for charts/paigasus (repo:helm-render)"
requires-python = ">=3.12"
dependencies = ["pyyaml==6.0.3"]
```

Run:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-513-helm-render-gate/ci/helm-render
uv lock
```

Expected: `Resolved 2 packages`. `uv.lock` holds `name = "pyyaml"`, `version = "6.0.3"`, and `requires-dist = [{ name = "pyyaml", specifier = "==6.0.3" }]`. Its `pyyaml` section is byte-identical to the one in `ci/workflow-credentials/uv.lock` (measured).

- [ ] **Step 2: Write the scratch runner `$SCRATCH/hr.sh`**

```bash
#!/bin/bash
# Runs ci/helm-render/helm_render.py with the locked venv python and the pinned helm first on PATH.
# usage: /bin/bash /private/tmp/claude-501/sma-513-pr2b/hr.sh <helm_render.py args>
set -uo pipefail
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
WT=/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-513-helm-render-gate
cd "$WT" || exit 2
PY="$(uv run --locked --project ci/helm-render python3 -c 'import sys, yaml; print(sys.executable)')" || exit 2
HELM="$(proto --reporter text bin helm)" || exit 2
PATH="$(dirname "$PY"):$(dirname "$HELM"):$PATH" "$PY" ci/helm-render/helm_render.py "$@"
echo "rc=$?"
```

- [ ] **Step 3: Write the failing self-test first**

Create `ci/helm-render/helm_render.py` with ONLY this content (the header, the imports and the self-test; the functions it calls do not exist yet):

```python
#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""repo:helm-render — checks 1, 1a, 2, 3 and 4 over `helm template` renders of a chart.

Usage:
    helm_render.py --chart <dir>     run every check against the chart in <dir>
    helm_render.py --self-test       run the in-process rows over inline YAML and proto text

Exit codes: 0 pass | 3 an assertion failed | 2 infrastructure error (a render that fails, a
source file this module cannot parse). ci/helm-render/run.sh maps 3 to 1 and every other
non-zero code to 2. A Python traceback exits 1, so it can never read as an assertion failure.

`helm` is taken from PATH. run.sh puts the proto-pinned helm first on PATH; do not call this
module directly with another helm and read its verdict as the gate's.
"""

from __future__ import annotations

import argparse
import io
import json
import shutil
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

import yaml


def self_test():
    failures = []

    def expect(label, rows, fail=(), passing=()):
        got = {r.row: r for r in rows}
        for row in fail:
            if row not in got or got[row].ok:
                failures.append(f"{label}: row [{row}] should FAIL, got {got.get(row)}")
        for row in passing:
            if row not in got or not got[row].ok:
                failures.append(f"{label}: row [{row}] should PASS, got {got.get(row)}")

    def expect_infra(label, fn):
        try:
            fn()
        except InfraError:
            return
        failures.append(f"{label}: expected an infrastructure error (rc 2), got none")

    both, iam_only = ("gateway", "iam"), ("iam",)

    # ---- the synthetic render: every later row builds on it
    want_both = ["ConfigMap", "ConfigMap", "Deployment", "Deployment", "Deployment", "Ingress", "Service", "Service", "Service"]
    got_both = sorted(d["kind"] for d in synthetic(both))
    if got_both != want_both:
        failures.append(f"synthetic(both): kinds are {got_both}, want {want_both}")
    want_iam = ["ConfigMap", "ConfigMap", "Deployment", "Deployment", "Ingress", "Service", "Service"]
    got_iam = sorted(d["kind"] for d in synthetic(iam_only))
    if got_iam != want_iam:
        failures.append(f"synthetic(iam only): kinds are {got_iam}, want {want_iam}")

    # ---- the exit-code contract and the parser's infrastructure errors
    if report([Row("x", True)], io.StringIO()) != 0 or report([Row("x", True), Row("y", False, "bad")], io.StringIO()) != 3:
        failures.append("report: the exit code does not follow the rows (want 0 and 3)")
    expect_infra("parse_docs invalid YAML", lambda: parse_docs("a: [\n"))
    expect_infra("parse_docs a non-mapping document", lambda: parse_docs("- a\n- b\n"))

    for f in failures:
        print(f"  FAIL {f}")
    if failures:
        print(f"helm_render.py self-test: {len(failures)} row(s) failed")
        return 3
    print("== helm_render.py self-test passed ==")
    return 0


def main(argv=None):
    parser = argparse.ArgumentParser(description="repo:helm-render checks 1, 1a, 2, 3 and 4")
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--chart", type=Path, help="the chart directory to render and check")
    mode.add_argument("--self-test", action="store_true", help="run the in-process rows")
    args = parser.parse_args(argv)
    try:
        if args.self_test:
            return self_test()
        return report(run_checks(args.chart))
    except InfraError as exc:
        print(f"INFRA  helm-render: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
```

- [ ] **Step 4: Run it and see it fail**

Run: `/bin/bash /private/tmp/claude-501/sma-513-pr2b/hr.sh --self-test`

Expected: a traceback ending in `NameError: name 'InfraError' is not defined` (or `synthetic`), and `rc=1`.

- [ ] **Step 5: Add the implementation**

Insert this block between the `import yaml` line and `def self_test():`:

```python
REPO_ROOT = Path(__file__).resolve().parents[2]

KUBE_VERSION = "1.31.0"
RELEASE = "paigasus"
SENTINEL_URL = "http://gateway-sentinel.example.test:8088"
SENTINEL_HOST = "gateway-sentinel.example.test"

# The required values, as ONE constant. A seventh copy of the list the six chart scripts hold
# (spec § 10 risk 4); it differs from theirs only in zones.gateway.backend.url, which is the
# sentinel here so check 2 can find it. A missing value makes every render fail, which is rc 2.
STUB_VALUES = (
    ("ingress.host", "console.example.test"),
    ("ingress.tlsSecretName", "console-tls"),
    ("oidc.issuer", "https://idp.example.test/realms/paigasus"),
    ("oidc.clientId", "paigasus-console"),
    ("oidc.existingSecret", "paigasus-console-secret"),
    ("postgres.existingSecret", "paigasus-postgres-secret"),
    ("zones.iam.backend.apiKeysPepperSecret", "paigasus-iam-pepper"),
    ("zones.gateway.backend.url", SENTINEL_URL),
)

# The valid zone subsets: (row label, enabled zone ids).
SUBSETS = (("iam", ("iam",)), ("iam+gateway", ("gateway", "iam")))


class InfraError(Exception):
    """Exit 2: the module could not obtain or parse its input."""


class ShapeError(Exception):
    """The rendered chart lacks a field a row reads. The row FAILS; the run goes on."""


@dataclass(frozen=True)
class Row:
    row: str
    ok: bool
    detail: str = ""


# --------------------------------------------------------------------------- helpers


def _get(obj, *path):
    """obj[path[0]][path[1]]..., or ShapeError naming the first missing step."""
    cur = obj
    for i, key in enumerate(path):
        where = ".".join(str(p) for p in path[: i + 1])
        if isinstance(key, int):
            if not isinstance(cur, list) or key >= len(cur):
                raise ShapeError(f"missing {where}")
        elif not isinstance(cur, dict) or key not in cur:
            raise ShapeError(f"missing {where}")
        cur = cur[key]
    return cur


def _row(row, fn):
    """Run one row. fn returns a list of problems; an empty list is a pass."""
    try:
        problems = fn()
    except ShapeError as exc:
        problems = [f"rendered shape: {exc}"]
    return Row(row, not problems, "; ".join(problems))


def _of_kind(docs, kind):
    return [d for d in docs if d.get("kind") == kind]


def _one(items, what):
    if len(items) != 1:
        raise ShapeError(f"expected exactly one {what}, found {len(items)}")
    return items[0]


def _name(doc):
    return _get(doc, "metadata", "name")


def _configmap_with(docs, key):
    return _one([d for d in _of_kind(docs, "ConfigMap") if key in (d.get("data") or {})], f"ConfigMap carrying {key}")


def _json_map(docs, key):
    raw = _get(_configmap_with(docs, key), "data", key)
    try:
        value = json.loads(raw)
    except (TypeError, ValueError) as exc:
        raise ShapeError(f"{key} is not JSON: {exc}") from exc
    if not isinstance(value, dict):
        raise ShapeError(f"{key} is not a JSON object")
    return value


def _app_label(dep):
    return _get(dep, "spec", "template", "metadata", "labels", "app.kubernetes.io/name")


def _deployments(docs, label):
    return [d for d in _of_kind(docs, "Deployment") if _app_label(d) == label]


def _containers(dep):
    return _get(dep, "spec", "template", "spec", "containers")


def _env(container):
    return {e.get("name"): e["value"] for e in container.get("env") or [] if "value" in e}


def _service(docs, name):
    return _one([s for s in _of_kind(docs, "Service") if _name(s) == name], f"Service named {name}")


def _selects(service, dep):
    selector = _get(service, "spec", "selector")
    labels = _get(dep, "spec", "template", "metadata", "labels")
    return bool(selector) and all(labels.get(k) == v for k, v in selector.items())


def _selected_deployment(docs, service):
    return _one([d for d in _of_kind(docs, "Deployment") if _selects(service, d)], f"Deployment selected by Service {_name(service)}")


def _service_port(service, name):
    ports = [p for p in _get(service, "spec", "ports") if p.get("name") == name]
    return _one(ports, f"port named {name} on Service {_name(service)}")


# --------------------------------------------------------------------------- render


def helm_template(chart, enabled, extra=()):
    """One `helm template` of `chart` with exactly the zones in `enabled` switched on."""
    helm = shutil.which("helm")
    if helm is None:
        raise InfraError("helm is not on PATH; run this module through ci/helm-render/run.sh")
    values = _chart_values(chart)
    cmd = [helm, "template", RELEASE, str(chart), "--kube-version", KUBE_VERSION]
    for key, value in STUB_VALUES:
        cmd += ["--set", f"{key}={value}"]
    for zone in sorted(values["zones"]):
        cmd += ["--set", f"zones.{zone}.enabled={'true' if zone in enabled else 'false'}"]
    cmd += list(extra)
    proc = subprocess.run(cmd, capture_output=True, text=True, check=False)
    if proc.returncode != 0:
        raise InfraError(f"helm template failed (rc={proc.returncode}) for {chart} {' '.join(extra)}: {proc.stderr.strip()}")
    return proc.stdout


def parse_docs(text):
    try:
        docs = [d for d in yaml.safe_load_all(text) if d is not None]
    except yaml.YAMLError as exc:
        raise InfraError(f"the render is not valid YAML: {exc}") from exc
    for d in docs:
        if not isinstance(d, dict):
            raise InfraError(f"a rendered document is not a mapping: {d!r}")
    return docs


def _chart_values(chart):
    try:
        values = yaml.safe_load((Path(chart) / "values.yaml").read_text())
    except (OSError, yaml.YAMLError) as exc:
        raise InfraError(f"cannot read {chart}/values.yaml: {exc}") from exc
    if not isinstance(values, dict) or not isinstance(values.get("zones"), dict):
        raise InfraError(f"{chart}/values.yaml has no zones mapping")
    return values


def base_paths(chart):
    """basePath -> zone id, from the chart's own values.yaml."""
    out = {}
    for zone, spec in _chart_values(chart)["zones"].items():
        if not isinstance(spec, dict) or not isinstance(spec.get("basePath"), str):
            raise InfraError(f"zones.{zone}.basePath is not a string in {chart}/values.yaml")
        out[spec["basePath"]] = zone
    return out


# --------------------------------------------------------------------------- run


def run_checks(chart):
    """Every row for the chart in `chart`. Each render must succeed, or this raises InfraError."""
    chart = Path(chart).resolve()
    rows = []
    for _label, enabled in SUBSETS:
        parse_docs(helm_template(chart, enabled))
    return rows


def report(rows, out=None):
    """Print one line per row, then the verdict. Returns 3 if any row failed, else 0."""
    for r in rows:
        print(f"{'PASS' if r.ok else 'FAIL'}  [{r.row}]" + (f": {r.detail}" if r.detail else ""), file=out)
    failed = [r for r in rows if not r.ok]
    if failed:
        print(f"== helm-render: {len(failed)} of {len(rows)} row(s) FAILED ==", file=out)
        return 3
    print(f"== helm-render: all {len(rows)} rows passed ==", file=out)
    return 0


# --------------------------------------------------------------------------- self-test

_SYN_IAM = """\
---
apiVersion: v1
kind: ConfigMap
metadata: {{name: r-console-env}}
data:
  PAIGASUS_IAM_GRPC_URL: "http://r-iam-backend:9090"
---
apiVersion: v1
kind: ConfigMap
metadata: {{name: r-zonemap}}
data:
  PAIGASUS_ZONES: '{zones}'
  PAIGASUS_SERVICES: '{services}'
---
apiVersion: v1
kind: Service
metadata: {{name: r-iam-backend}}
spec:
  selector: {{app.kubernetes.io/name: iam-backend}}
  ports:
    - {{name: http, port: 8080, targetPort: http}}
    - {{name: grpc, port: 9090, targetPort: grpc}}
---
apiVersion: apps/v1
kind: Deployment
metadata: {{name: r-iam-backend}}
spec:
  template:
    metadata:
      labels: {{app.kubernetes.io/name: iam-backend}}
    spec:
      securityContext: {{runAsUser: 65532, runAsGroup: 65532, runAsNonRoot: true}}
      containers:
        - name: backend
          image: "repo/iam:{backend_tag}"
          securityContext: {{allowPrivilegeEscalation: false}}
          ports:
            - {{name: http, containerPort: 8080}}
            - {{name: grpc, containerPort: 9090}}
          env:
            - {{name: IAM_HTTP_ADDR, value: "0.0.0.0:8080"}}
            - {{name: IAM_GRPC_ADDR, value: "0.0.0.0:9090"}}
"""

_SYN_CONSOLE = """\
---
apiVersion: v1
kind: Service
metadata: {{name: r-{zone}-console}}
spec:
  selector: {{app.kubernetes.io/name: {zone}-console}}
  ports:
    - {{name: http, port: 3000, targetPort: http}}
---
apiVersion: apps/v1
kind: Deployment
metadata: {{name: r-{zone}-console}}
spec:
  template:
    metadata:
      labels: {{app.kubernetes.io/name: {zone}-console}}
      annotations: {{checksum/zonemap: "{zonemap_sum}"}}
    spec:
      securityContext: {{runAsUser: 65532, runAsGroup: 65532, runAsNonRoot: true}}
      containers:
        - name: console
          image: "repo/{zone}-console:{tag}"
          securityContext: {{allowPrivilegeEscalation: false}}
          ports:
            - {{name: http, containerPort: 3000}}
          env:
            - {{name: PAIGASUS_ZONE, value: {zone}}}
            - {{name: PORT, value: "3000"}}
"""

_SYN_INGRESS = """\
---
apiVersion: networking.k8s.io/v1
kind: Ingress
metadata: {{name: r}}
spec:
  rules:
    - host: console.example.test
      http:
        paths:
{paths}"""

_SYN_PATH = """\
          - path: /{zone}
            pathType: Prefix
            backend: {{service: {{name: r-{zone}-console, port: {{name: http}}}}}}
"""

_SYN_PATHS = {"/iam": "iam", "/gateway": "gateway"}

_SYN_PROTO = """\
// SPDX-License-Identifier: Apache-2.0
//     CAPABILITY_IAM_AUTHZ_CEDAR  ->  "iam.authz.cedar"
enum Capability {
  // CAPABILITY_BILLING_LEDGER = 99; a comment naming a value does not count
  CAPABILITY_UNSPECIFIED = 0;
  CAPABILITY_IAM_AUTHZ_CEDAR = 1;
  CAPABILITY_IAM_APIKEYS = 2 [deprecated = true];
  CAPABILITY_GATEWAY_CHAT_STREAM = 4; // trailing comment
  reserved 5;
}
"""

_SYN_HELPERS = '{{/* SPDX */}}\n{{- define "paigasus.serviceSlugs" -}}\niam gateway\n{{- end -}}\n'


def synthetic(zones=("gateway", "iam"), tags=None, backend_tag="0.0.0"):
    """A minimal render as parsed docs: the IAM backend, one console per zone, the two maps."""
    tags = tags or {}
    zonemap = {z: f"/{z}" for z in sorted(zones)}
    services = {z: ("http://r-iam-backend:8080" if z == "iam" else "http://gw.example.test:8088") for z in sorted(zones)}
    text = _SYN_IAM.format(zones=json.dumps(zonemap), services=json.dumps(services), backend_tag=backend_tag)
    for zone in sorted(zones):
        text += _SYN_CONSOLE.format(zone=zone, tag=tags.get(zone, "0.0.0"), zonemap_sum="-".join(sorted(zones)))
    text += _SYN_INGRESS.format(paths="".join(_SYN_PATH.format(zone=z) for z in sorted(zones)))
    return parse_docs(text)


def _find(docs, kind, name):
    return _one([d for d in docs if d.get("kind") == kind and _name(d) == name], f"{kind} {name}")
```

- [ ] **Step 6: Run the self-test, the real run and the infra rows**

Run:

```bash
/bin/bash /private/tmp/claude-501/sma-513-pr2b/hr.sh --self-test
/bin/bash /private/tmp/claude-501/sma-513-pr2b/hr.sh --chart charts/paigasus
/bin/bash /private/tmp/claude-501/sma-513-pr2b/hr.sh --chart /nonexistent
```

Expected, in order:
- `== helm_render.py self-test passed ==`, `rc=0`.
- `== helm-render: all 0 rows passed ==`, `rc=0`. (Tasks 2–5 add the rows.)
- `INFRA  helm-render: cannot read /nonexistent/values.yaml: …`, `rc=2`.

Then prove the no-helm row. Write `$SCRATCH/nohelm.sh`:

```bash
#!/bin/bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-513-helm-render-gate || exit 2
PY="$(uv run --locked --project ci/helm-render python3 -c 'import sys; print(sys.executable)')"
env PATH=/usr/bin:/bin "$PY" ci/helm-render/helm_render.py --chart charts/paigasus
echo "rc=$?"
```

Run `/bin/bash $SCRATCH/nohelm.sh`. Expected: `INFRA  helm-render: helm is not on PATH; …`, `rc=2`.

- [ ] **Step 7: Lint**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; uv run --locked --project py ruff check --config py/pyproject.toml -- ci/helm-render/helm_render.py`

Expected: `All checks passed!` (`repo:ruff-ci` lints `ci/**/*.py`, `moon.yml:916`). `ruff format` is not gated over `ci/`; do not run it.

- [ ] **Step 8: Commit**

```bash
git add ci/helm-render/pyproject.toml ci/helm-render/uv.lock ci/helm-render/helm_render.py
git commit -m "feat(ci): add the helm-render uv project and module skeleton (SMA-513)" -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

Confirm `git status --porcelain` does not list `ci/helm-render/.venv` (it is ignored).

---

### Task 2: Check 1a (the slug mirror) and check 1 (coupling, routing, membership)

**Files:**
- Modify: `ci/helm-render/helm_render.py` (import block, constants after `REPO_ROOT`, a new section before the `run` anchor, `run_checks`, `self_test`)

**Interfaces:**
- Consumes: Task 1's helpers, `synthetic`, `_find`, `_SYN_PATHS`, `_SYN_PROTO`, `_SYN_HELPERS`, the two anchors.
- Produces: `PROTO`, `STATE_TS`, `CAPABILITY_TS`, `STATE_TS_LINE`, `CAPABILITY_TS_LINE`; `proto_slugs(text) -> set[str]` (raises `InfraError`), `chart_slugs(helpers_text) -> set[str]` (raises `InfraError`), `check1a(chart_set, proto_set, state_ts, capability_ts) -> Row` (row `1a`), `check1(label, docs, enabled, paths, slugs) -> list[Row]` (rows `1.1 <label>`, `1.2 <label>`, `1.3 <label>`).

- [ ] **Step 1: Write the failing rows**

Insert this block in `self_test()` immediately BEFORE the line `    # ---- the exit-code contract and the parser's infrastructure errors`:

```python
    # ---- check 1
    slugs = {"iam", "gateway"}
    rows1 = ("1.1 t", "1.2 t", "1.3 t")
    expect("check1 good, both zones", check1("t", synthetic(both), both, _SYN_PATHS, slugs), passing=rows1)
    expect("check1 good, iam only", check1("t", synthetic(iam_only), iam_only, _SYN_PATHS, slugs), passing=rows1)
    # A zone missing from ALL THREE sets: they agree with each other, so only "equal to E" sees it.
    expect("check1 zone absent everywhere", check1("t", synthetic(iam_only), both, _SYN_PATHS, slugs), fail=("1.1 t",))
    literal = synthetic(both)
    zonemap = _configmap_with(literal, "PAIGASUS_ZONES")
    zonemap["data"]["PAIGASUS_ZONES"] = json.dumps({"iam": "/iam"})
    zonemap["data"]["PAIGASUS_SERVICES"] = json.dumps({"iam": "http://r-iam-backend:8080"})
    expect("check1 literal ingress, iam only", check1("t", literal, iam_only, _SYN_PATHS, slugs), fail=("1.1 t",))
    omits = synthetic(both)
    _configmap_with(omits, "PAIGASUS_ZONES")["data"]["PAIGASUS_ZONES"] = json.dumps({"iam": "/iam"})
    expect("check1 PAIGASUS_ZONES omits a zone", check1("t", omits, both, _SYN_PATHS, slugs), fail=("1.1 t",))
    # Routing: /gateway sent to the IAM console. The key sets all stay equal, so 1.1 passes.
    swapped = synthetic(both)
    for p in _one(_of_kind(swapped, "Ingress"), "Ingress")["spec"]["rules"][0]["http"]["paths"]:
        p["backend"]["service"]["name"] = "r-iam-console"
    expect("check1 /gateway routes to the IAM console", check1("t", swapped, both, _SYN_PATHS, slugs), fail=("1.2 t",), passing=("1.1 t",))
    wrong_zone = synthetic(both)
    _containers(_find(wrong_zone, "Deployment", "r-gateway-console"))[0]["env"][0]["value"] = "iam"
    expect("check1 a console serves the wrong PAIGASUS_ZONE", check1("t", wrong_zone, both, _SYN_PATHS, slugs), fail=("1.2 t",))
    expect("check1 enabled id outside the slug set", check1("t", synthetic(both), both, _SYN_PATHS, {"iam"}), fail=("1.3 t",))

    # ---- check 1a
    if proto_slugs(_SYN_PROTO) != {"iam", "gateway"}:
        failures.append(f"proto_slugs: got {sorted(proto_slugs(_SYN_PROTO))}, want ['gateway', 'iam']")
    expect_infra("proto_slugs no-dot key", lambda: proto_slugs(_SYN_PROTO.replace("CAPABILITY_IAM_APIKEYS", "CAPABILITY_SOLO")))
    expect_infra("proto_slugs no enum block", lambda: proto_slugs("enum Other {\n  X = 0;\n}\n"))
    expect_infra("proto_slugs unparseable line", lambda: proto_slugs(_SYN_PROTO.replace("reserved 5;", "CAPABILITY_IAM_X = ;")))
    expect_infra("proto_slugs only UNSPECIFIED", lambda: proto_slugs("enum Capability {\n  CAPABILITY_UNSPECIFIED = 0;\n}\n"))
    if chart_slugs(_SYN_HELPERS) != {"iam", "gateway"}:
        failures.append(f"chart_slugs: got {sorted(chart_slugs(_SYN_HELPERS))}, want ['gateway', 'iam']")
    state_ok, cap_ok = f"  {STATE_TS_LINE}\n", f"  {CAPABILITY_TS_LINE}\n"
    expect("check1a equal sets", [check1a(chart_slugs(_SYN_HELPERS), slugs, state_ok, cap_ok)], passing=("1a",))
    extra = chart_slugs(_SYN_HELPERS.replace("iam gateway", "iam gateway billing"))
    expect("check1a extra chart slug", [check1a(extra, slugs, state_ok, cap_ok)], fail=("1a",))
    double = chart_slugs(_SYN_HELPERS.replace("iam gateway", "iam  gateway"))
    expect("check1a double space", [check1a(double, slugs, state_ok, cap_ok)], fail=("1a",))
    expect("check1a proto has a slug the chart lacks", [check1a({"iam"}, slugs, state_ok, cap_ok)], fail=("1a",))
    expect("check1a state.ts line changed", [check1a(slugs, slugs, state_ok.replace("indexOf", "lastIndexOf"), cap_ok)], fail=("1a",))
    expect("check1a capability.ts line changed", [check1a(slugs, slugs, state_ok, cap_ok.replace("'.'", "'-'"))], fail=("1a",))

```

- [ ] **Step 2: Run it and see it fail**

Run: `/bin/bash /private/tmp/claude-501/sma-513-pr2b/hr.sh --self-test`

Expected: `NameError: name 'check1' is not defined`, `rc=1`.

- [ ] **Step 3: Add the implementation**

(a) Replace the import block with:

```python
import argparse
import io
import json
import re
import shutil
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

import yaml
```

(b) Replace the line `REPO_ROOT = Path(__file__).resolve().parents[2]` with:

```python
REPO_ROOT = Path(__file__).resolve().parents[2]
PROTO = REPO_ROOT / "contracts" / "proto" / "paigasus" / "common" / "v1" / "service_info.proto"
STATE_TS = REPO_ROOT / "ts" / "packages" / "paigasus-discovery" / "src" / "core" / "state.ts"
CAPABILITY_TS = REPO_ROOT / "ts" / "packages" / "paigasus-proto" / "src" / "capability.ts"

# The two TypeScript lines that derive SERVICE_SLUGS. Check 1a re-implements them in Python, so
# a change to either line must stop a human (spec § 5, check 1a). Compared as stripped lines.
STATE_TS_LINE = (
    "export const SERVICE_SLUGS: readonly string[] = "
    "[...new Set(CAPABILITY_KEYS.map((k) => k.slice(0, k.indexOf('.'))))];"
)
CAPABILITY_TS_LINE = "return name.slice(PREFIX.length).toLowerCase().replace(/_/g, '.');"
```

The two pinned lines are `ts/packages/paigasus-discovery/src/core/state.ts:21` and `ts/packages/paigasus-proto/src/capability.ts:27` (measured).

(c) Insert this section immediately BEFORE the line `# --------------------------------------------------------------------------- run`:

```python
# --------------------------------------------------------------------------- check 1a


def proto_slugs(text):
    """The service-slug set derived from the proto's `enum Capability` block (spec check 1a)."""
    blocks = re.findall(r"^enum Capability \{\n(.*?)^\}", text, re.MULTILINE | re.DOTALL)
    if len(blocks) != 1:
        raise InfraError(f"expected one `enum Capability {{ ... }}` block in {PROTO.name}, found {len(blocks)}")
    slugs = set()
    for raw in blocks[0].splitlines():
        line = raw.split("//", 1)[0].strip()
        if not line or line.startswith(("option ", "reserved ")):
            continue
        m = re.fullmatch(r"([A-Z][A-Z0-9_]*)\s*=\s*-?\d+\s*(\[[^\]]*\])?\s*;", line)
        if m is None:
            raise InfraError(f"cannot parse enum Capability line {raw.strip()!r}")
        name = m.group(1)
        if name == "CAPABILITY_UNSPECIFIED":
            continue
        if not name.startswith("CAPABILITY_"):
            raise InfraError(f"enum value {name} does not carry the CAPABILITY_ prefix capabilityWireKey strips")
        key = name[len("CAPABILITY_"):].lower().replace("_", ".")
        if "." not in key:
            raise InfraError(
                f"capability key {key!r} ({name}) has no '.'; state.ts's k.slice(0, k.indexOf('.')) would drop its last "
                "character, so Python and TypeScript would disagree on the slug. Re-confirm the rule before registering it."
            )
        slugs.add(key.split(".", 1)[0])
    if not slugs:
        raise InfraError("enum Capability registers no capability, so the slug set is empty")
    return slugs


def chart_slugs(helpers_text):
    """The literal of `paigasus.serviceSlugs`, split with split(" ") as paigasus.validate splits it."""
    blocks = re.findall(r'\{\{-?\s*define\s+"paigasus\.serviceSlugs"\s*-?\}\}(.*?)\{\{-?\s*end\s*-?\}\}', helpers_text, re.DOTALL)
    if len(blocks) != 1:
        raise InfraError(f"expected one paigasus.serviceSlugs define in _helpers.tpl, found {len(blocks)}")
    return set(blocks[0].strip().split(" "))


def check1a(chart_set, proto_set, state_ts, capability_ts):
    def body():
        problems = []
        if chart_set != proto_set:
            problems.append(f"paigasus.serviceSlugs is {sorted(chart_set)}, the proto registry derives {sorted(proto_set)}; they must be EQUAL (spec A2)")
        for label, text, line in (("state.ts", state_ts, STATE_TS_LINE), ("capability.ts", capability_ts, CAPABILITY_TS_LINE)):
            if line not in {raw.strip() for raw in text.splitlines()}:
                problems.append(f"{label} no longer carries the derivation line this check mirrors; re-confirm the rule, then update it here: {line}")
        return problems

    return _row("1a", body)


# --------------------------------------------------------------------------- check 1


def check1(label, docs, enabled, paths, slugs):
    want = sorted(enabled)

    def coupling():
        ingress_paths = [
            _get(p, "path") for ing in _of_kind(docs, "Ingress") for rule in _get(ing, "spec", "rules") for p in _get(rule, "http", "paths")
        ]
        ingress_ids = sorted(paths.get(p, f"<unmapped path {p}>") for p in ingress_paths)
        zones = sorted(_json_map(docs, "PAIGASUS_ZONES"))
        services = sorted(_json_map(docs, "PAIGASUS_SERVICES"))
        problems = []
        for what, got in (("Ingress paths", ingress_ids), ("PAIGASUS_ZONES keys", zones), ("PAIGASUS_SERVICES keys", services)):
            if got != want:
                problems.append(f"{what} give {got}, enabled zones are {want}")
        return problems

    def routing():
        problems = []
        for ing in _of_kind(docs, "Ingress"):
            for rule in _get(ing, "spec", "rules"):
                for p in _get(rule, "http", "paths"):
                    path = _get(p, "path")
                    zone = paths.get(path)
                    svc = _service(docs, _get(p, "backend", "service", "name"))
                    dep = _selected_deployment(docs, svc)
                    got = _env(_get(_containers(dep), 0)).get("PAIGASUS_ZONE")
                    if zone is None or got != zone:
                        problems.append(f"{path} routes to Service {_name(svc)}, whose Deployment serves PAIGASUS_ZONE={got!r}, not {zone!r}")
                    _service_port(svc, _get(p, "backend", "service", "port", "name"))
        return problems

    def membership():
        unknown = sorted(set(enabled) - slugs)
        return [f"enabled zone(s) {unknown} are not in the proto-derived slug set {sorted(slugs)}"] if unknown else []

    return [_row(f"1.1 {label}", coupling), _row(f"1.2 {label}", routing), _row(f"1.3 {label}", membership)]


```

(d) Replace the whole `run_checks` function with:

```python
def run_checks(chart):
    """Every row for the chart in `chart`. Each render must succeed, or this raises InfraError."""
    chart = Path(chart).resolve()
    try:
        slugs = proto_slugs(PROTO.read_text())
        state_ts = STATE_TS.read_text()
        capability_ts = CAPABILITY_TS.read_text()
        helpers = (chart / "templates" / "_helpers.tpl").read_text()
    except OSError as exc:
        raise InfraError(f"cannot read a source file: {exc}") from exc
    paths = base_paths(chart)
    rows = [check1a(chart_slugs(helpers), slugs, state_ts, capability_ts)]
    for label, enabled in SUBSETS:
        raw = helm_template(chart, enabled)
        docs = parse_docs(raw)
        rows += check1(label, docs, enabled, paths, slugs)
    return rows
```

- [ ] **Step 4: Run it and see it pass**

Run the self-test and the real run through `hr.sh`.

Expected: `== helm_render.py self-test passed ==` with `rc=0`; then, for the chart:

```
PASS  [1a]
PASS  [1.1 iam]
PASS  [1.2 iam]
PASS  [1.3 iam]
PASS  [1.1 iam+gateway]
PASS  [1.2 iam+gateway]
PASS  [1.3 iam+gateway]
== helm-render: all 7 rows passed ==
rc=0
```

- [ ] **Step 5: Lint and commit**

Run ruff as in Task 1 Step 7. Expected: `All checks passed!`

```bash
git add ci/helm-render/helm_render.py
git commit -m "feat(ci): helm-render checks 1 and 1a, coupling, routing and the slug mirror (SMA-513)" -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 3: Check 2 (a disabled zone leaves no trace)

**Files:**
- Modify: `ci/helm-render/helm_render.py` (new section before the `run` anchor, `run_checks`, `self_test`)

**Interfaces:**
- Consumes: `STUB_VALUES`, `SENTINEL_HOST`, `SENTINEL_URL`, `synthetic`, `_configmap_with`.
- Produces: `_strings(node, path="$")` (a generator of `(where, text)`), `check2(docs, raw, disabled="gateway") -> Row` (row `2`).

- [ ] **Step 1: Write the failing rows**

Insert immediately BEFORE the exit-code anchor in `self_test()`:

```python
    # ---- check 2
    clean = synthetic(iam_only)
    expect("check2 clean iam-only render", [check2(clean, "kind: ConfigMap\n")], passing=("2",))
    leaked = synthetic(iam_only)
    _configmap_with(leaked, "PAIGASUS_IAM_GRPC_URL")["data"]["PAIGASUS_UPSTREAM_URL"] = SENTINEL_URL
    expect("check2 sentinel value leaked", [check2(leaked, "")], fail=("2",))
    keyed = synthetic(iam_only)
    _configmap_with(keyed, "PAIGASUS_IAM_GRPC_URL")["data"]["X_GATEWAY_Y"] = "1"
    expect("check2 zone id in a KEY, upper case", [check2(keyed, "")], fail=("2",))
    expect("check2 zone id in a comment only", [check2(clean, "# serves /gateway too\n")], fail=("2",))

```

- [ ] **Step 2: Run it and see it fail**

Expected: `NameError: name 'check2' is not defined`, `rc=1`.

- [ ] **Step 3: Add the implementation**

Insert immediately BEFORE the `run` anchor:

```python
# --------------------------------------------------------------------------- check 2


def _strings(node, path="$"):
    if isinstance(node, dict):
        for key, value in node.items():
            if isinstance(key, str):
                yield f"{path} (key)", key
            yield from _strings(value, f"{path}.{key}")
    elif isinstance(node, list):
        for i, value in enumerate(node):
            yield from _strings(value, f"{path}[{i}]")
    elif isinstance(node, str):
        yield path, node


def check2(docs, raw, disabled="gateway"):
    needles = {disabled, SENTINEL_HOST.lower()} | {v.lower() for k, v in STUB_VALUES if k.startswith(f"zones.{disabled}.")}

    def body():
        problems = []
        for i, doc in enumerate(docs):
            for where, text in _strings(doc, f"doc[{i}]"):
                hits = sorted(n for n in needles if n in text.lower())
                if hits:
                    problems.append(f"{where} = {text!r} contains {hits}")
        # The raw text too: a comment is not a YAML string, and a zone id leaked into one is still
        # a trace (ingress.yaml writes its comments per zone for this reason).
        raw_hits = sorted(n for n in needles if n in raw.lower())
        if raw_hits and not problems:
            problems.append(f"the raw render contains {raw_hits} outside any YAML string (a comment?)")
        return problems

    return _row("2", body)


```

In `run_checks`, replace:

```python
        rows += check1(label, docs, enabled, paths, slugs)
    return rows
```

with:

```python
        rows += check1(label, docs, enabled, paths, slugs)
        if enabled == ("iam",):
            rows.append(check2(docs, raw))
    return rows
```

- [ ] **Step 4: Run it and see it pass**

Expected: self-test passes. The real run prints `PASS  [2]` after `PASS  [1.3 iam]` and ends `== helm-render: all 8 rows passed ==`, `rc=0`. The iam-only render has 0 case-insensitive hits for `gateway` (measured on the raw text, 13568 bytes).

- [ ] **Step 5: Lint and commit**

```bash
git add ci/helm-render/helm_render.py
git commit -m "feat(ci): helm-render check 2, no trace of a disabled zone (SMA-513)" -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 4: Check 3 (independent rollout)

**Files:**
- Modify: `ci/helm-render/helm_render.py` (import block, a constant after `SUBSETS`, a new section before the `run` anchor, `run_checks`, `self_test`)

**Interfaces:**
- Consumes: `helm_template`, `parse_docs`, `_deployments`, `_one`, `_get`, `synthetic`, `_find`.
- Produces: `CHECK3_EXPECT: dict[str, dict[str, str]]`, `compare_templates(row, before, after) -> Row`, `_bumped_app_version(chart, dest) -> Path`, `check3(chart) -> list[Row]` (rows `3a`, `3a-prime`, `3b`, `3c`).

- [ ] **Step 1: Write the failing rows**

Insert immediately BEFORE the exit-code anchor in `self_test()`:

```python
    # ---- check 3
    def tagged(**tags):
        return synthetic(both, tags=tags)

    expect("check3 a good", [compare_templates("3a", tagged(iam="t1"), tagged(iam="t2"))], passing=("3a",))
    expect("check3 a IAM tag moves both consoles", [compare_templates("3a", tagged(iam="t1", gateway="t1"), tagged(iam="t2", gateway="t2"))], fail=("3a",))
    expect("check3 a IAM tag moves nothing", [compare_templates("3a", tagged(iam="t1"), tagged(iam="t1"))], fail=("3a",))
    expect("check3 a-prime good", [compare_templates("3a-prime", tagged(gateway="t1"), tagged(gateway="t2"))], passing=("3a-prime",))
    expect("check3 a-prime gateway tag moves nothing", [compare_templates("3a-prime", tagged(gateway="t1"), tagged(gateway="t1"))], fail=("3a-prime",))
    bumped = synthetic(both, tags={"iam": "0.0.1", "gateway": "0.0.1"}, backend_tag="0.0.1")
    expect("check3 b good", [compare_templates("3b", synthetic(both), bumped)], passing=("3b",))
    backend_same = synthetic(both, tags={"iam": "0.0.1", "gateway": "0.0.1"})
    expect("check3 b backend unchanged", [compare_templates("3b", synthetic(both), backend_same)], fail=("3b",))
    expect("check3 c good", [compare_templates("3c", synthetic(both), synthetic(iam_only))], passing=("3c",))
    expect("check3 c backend restarted", [compare_templates("3c", synthetic(both), synthetic(iam_only, backend_tag="0.0.1"))], fail=("3c",))
    expect("check3 c gateway console survives", [compare_templates("3c", synthetic(both), synthetic(both))], fail=("3c",))
    console_same = synthetic(iam_only)
    _find(console_same, "Deployment", "r-iam-console")["spec"]["template"] = copy.deepcopy(_find(synthetic(both), "Deployment", "r-iam-console")["spec"]["template"])
    expect("check3 c IAM console unchanged", [compare_templates("3c", synthetic(both), console_same)], fail=("3c",))

```

- [ ] **Step 2: Run it and see it fail**

Expected: `NameError: name 'compare_templates' is not defined` (or `copy`), `rc=1`.

- [ ] **Step 3: Add the implementation**

(a) Replace the import block with:

```python
import argparse
import copy
import io
import json
import re
import shutil
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path

import yaml
```

(b) Insert after the `SUBSETS = …` line:

```python

# Check 3: row -> the required result per pod template, keyed by the `app.kubernetes.io/name`
# label of spec.template. "differs" is a POSITIVE assertion (spec § 5, check 3).
CHECK3_EXPECT = {
    "3a": {"iam-console": "differs", "gateway-console": "equal", "iam-backend": "equal"},
    "3a-prime": {"iam-console": "equal", "gateway-console": "differs", "iam-backend": "equal"},
    "3b": {"iam-console": "differs", "gateway-console": "differs", "iam-backend": "differs"},
    "3c": {"iam-console": "differs", "gateway-console": "absent", "iam-backend": "equal"},
}
```

(c) Insert immediately BEFORE the `run` anchor:

```python
# --------------------------------------------------------------------------- check 3


def compare_templates(row, before, after):
    """Compare spec.template of each CHECK3_EXPECT Deployment, as parsed YAML."""

    def body():
        problems = []
        for name, want in CHECK3_EXPECT[row].items():
            b = _get(_one(_deployments(before, name), f"{name} Deployment before the change"), "spec", "template")
            found = _deployments(after, name)
            if want == "absent":
                if found:
                    problems.append(f"{name}: the Deployment is still rendered; it must be absent")
                continue
            a = _get(_one(found, f"{name} Deployment after the change"), "spec", "template")
            if want == "differs" and a == b:
                problems.append(f"{name}: spec.template is equal; it must differ")
            if want == "equal" and a != b:
                problems.append(f"{name}: spec.template differs; it must be equal")
        return problems

    return _row(row, body)


def _bumped_app_version(chart, dest):
    shutil.copytree(chart, dest)
    chart_yaml = Path(dest) / "Chart.yaml"
    text, n = re.subn(r"(?m)^appVersion:.*$", 'appVersion: "0.0.0-helm-render-bump"', chart_yaml.read_text())
    if n != 1:
        raise InfraError(f"expected one appVersion line in {chart}/Chart.yaml, found {n}")
    chart_yaml.write_text(text)
    return Path(dest)


def check3(chart):
    both = ("gateway", "iam")
    base = parse_docs(helm_template(chart, both))
    rows = []
    for row, key in (("3a", "zones.iam.console.image.tag"), ("3a-prime", "zones.gateway.console.image.tag")):
        before = parse_docs(helm_template(chart, both, ("--set", f"{key}=t1")))
        after = parse_docs(helm_template(chart, both, ("--set", f"{key}=t2")))
        rows.append(compare_templates(row, before, after))
    # Case b edits Chart.yaml in a temp COPY only. run.sh exports TMPDIR, so the copy lands under
    # the gate's own mktemp directory.
    with tempfile.TemporaryDirectory(prefix="helm-render-3b-") as tmp:
        bumped = _bumped_app_version(chart, Path(tmp) / "chart")
        rows.append(compare_templates("3b", base, parse_docs(helm_template(bumped, both))))
    rows.append(compare_templates("3c", base, parse_docs(helm_template(chart, ("iam",)))))
    return rows


```

(d) In `run_checks`, replace:

```python
        if enabled == ("iam",):
            rows.append(check2(docs, raw))
    return rows
```

with:

```python
        if enabled == ("iam",):
            rows.append(check2(docs, raw))
    rows += check3(chart)
    return rows
```

- [ ] **Step 4: Run it and see it pass**

Expected: self-test passes. The real run adds `PASS  [3a]`, `PASS  [3a-prime]`, `PASS  [3b]`, `PASS  [3c]` and ends `== helm-render: all 12 rows passed ==`, `rc=0`. Then confirm the temp copy did not touch the chart: `git status --porcelain charts/` prints nothing.

- [ ] **Step 5: Lint and commit**

```bash
git add ci/helm-render/helm_render.py
git commit -m "feat(ci): helm-render check 3, independent rollout per pod template (SMA-513)" -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 5: Check 4 (the chart agrees with itself)

**Files:**
- Modify: `ci/helm-render/helm_render.py` (import block, a constant after `CHECK3_EXPECT`, a new section before the `run` anchor, `run_checks`, `self_test`)

**Interfaces:**
- Consumes: Task 1's helpers.
- Produces: `POD_IDENTITY`, `_as_int(value, what) -> int`, `_container_port(container, name) -> int`, `_addr_port(value, what) -> int`, `_url_port(value, what) -> int`, `_same(got, want) -> bool`, `_resolve_target(service_port, container) -> int`, `check4(label, docs) -> list[Row]` (rows `4 iam-http <label>`, `4 iam-grpc <label>`, `4 console-port <label>`, `4 security-context <label>`).

- [ ] **Step 1: Write the failing rows**

Insert immediately BEFORE the exit-code anchor in `self_test()`:

```python
    # ---- check 4
    four = ("4 iam-http t", "4 iam-grpc t", "4 console-port t", "4 security-context t")
    expect("check4 good", check4("t", synthetic(both)), passing=four)

    def mutated(fn):
        docs = synthetic(both)
        fn(docs)
        return check4("t", docs)

    def backend_env(docs):
        return _containers(_find(docs, "Deployment", "r-iam-backend"))[0]["env"]

    def pod_sc(docs, name):
        return _find(docs, "Deployment", name)["spec"]["template"]["spec"]["securityContext"]

    def container_sc(docs, name):
        return _containers(_find(docs, "Deployment", name))[0]["securityContext"]

    def services_port(docs):
        _configmap_with(docs, "PAIGASUS_SERVICES")["data"]["PAIGASUS_SERVICES"] = json.dumps({"gateway": "http://gw:1", "iam": "http://r-iam-backend:18080"})

    expect("check4 IAM_HTTP_ADDR drifts", mutated(lambda d: backend_env(d)[0].update(value="0.0.0.0:8081")), fail=("4 iam-http t",), passing=("4 iam-grpc t",))
    expect("check4 Service http port drifts", mutated(lambda d: _find(d, "Service", "r-iam-backend")["spec"]["ports"][0].update(port=80)), fail=("4 iam-http t",))
    expect("check4 PAIGASUS_SERVICES.iam port drifts", mutated(services_port), fail=("4 iam-http t",))
    expect("check4 IAM_GRPC_ADDR drifts", mutated(lambda d: backend_env(d)[1].update(value="0.0.0.0:9091")), fail=("4 iam-grpc t",), passing=("4 iam-http t",))
    expect(
        "check4 PAIGASUS_IAM_GRPC_URL drifts",
        mutated(lambda d: _configmap_with(d, "PAIGASUS_IAM_GRPC_URL")["data"].update(PAIGASUS_IAM_GRPC_URL="http://r-iam-backend:9999")),
        fail=("4 iam-grpc t",),
    )
    expect("check4 console PORT drifts", mutated(lambda d: _containers(_find(d, "Deployment", "r-iam-console"))[0]["env"][1].update(value="8080")), fail=("4 console-port t",))
    expect("check4 console PORT is not a number", mutated(lambda d: _containers(_find(d, "Deployment", "r-iam-console"))[0]["env"][1].update(value="http")), fail=("4 console-port t",))
    expect("check4 console Service targets another port", mutated(lambda d: _find(d, "Service", "r-gateway-console")["spec"]["ports"][0].update(targetPort=8080)), fail=("4 console-port t",))
    expect("check4 pod drops runAsNonRoot", mutated(lambda d: pod_sc(d, "r-iam-console").pop("runAsNonRoot")), fail=("4 security-context t",))
    expect("check4 runAsNonRoot: 1 is not true", mutated(lambda d: pod_sc(d, "r-iam-backend").update(runAsNonRoot=1)), fail=("4 security-context t",))
    expect("check4 container runs as root", mutated(lambda d: container_sc(d, "r-gateway-console").update(runAsUser=0)), fail=("4 security-context t",))
    expect("check4 privilege escalation not denied", mutated(lambda d: container_sc(d, "r-iam-backend").pop("allowPrivilegeEscalation")), fail=("4 security-context t",))
    expect("check4 readOnlyRootFilesystem on the pod", mutated(lambda d: pod_sc(d, "r-iam-console").update(readOnlyRootFilesystem=True)), fail=("4 security-context t",))
    expect("check4 readOnlyRootFilesystem on a container", mutated(lambda d: container_sc(d, "r-iam-console").update(readOnlyRootFilesystem=True)), fail=("4 security-context t",))

```

- [ ] **Step 2: Run it and see it fail**

Expected: `NameError: name 'check4' is not defined`, `rc=1`.

- [ ] **Step 3: Add the implementation**

(a) Replace the import block with the final one:

```python
import argparse
import copy
import io
import json
import re
import shutil
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path
from urllib.parse import urlsplit

import yaml
```

(b) Insert after the closing `}` of `CHECK3_EXPECT`:

```python

# Check 4's pod-level identity (spec § 5, check 4). A container may omit these keys, but may not
# set one to another value.
POD_IDENTITY = {"runAsUser": 65532, "runAsGroup": 65532, "runAsNonRoot": True}
```

(c) Insert immediately BEFORE the `run` anchor:

```python
# --------------------------------------------------------------------------- check 4


def _as_int(value, what):
    try:
        return int(value)
    except (TypeError, ValueError) as exc:
        raise ShapeError(f"{what} {value!r} is not an integer") from exc


def _container_port(container, name):
    ports = [p for p in container.get("ports") or [] if p.get("name") == name]
    return _as_int(_get(_one(ports, f"container port named {name}"), "containerPort"), f"containerPort {name}")


def _addr_port(value, what):
    try:
        return int(str(value).rsplit(":", 1)[1])
    except (IndexError, ValueError) as exc:
        raise ShapeError(f"{what} {value!r} carries no port") from exc


def _url_port(value, what):
    try:
        port = urlsplit(str(value)).port
    except ValueError as exc:
        raise ShapeError(f"{what} {value!r} carries no valid port") from exc
    if port is None:
        raise ShapeError(f"{what} {value!r} carries no port")
    return port


def _same(got, want):
    """Equal AND the same type: True == 1 in Python, and runAsNonRoot: 1 is not runAsNonRoot: true."""
    return type(got) is type(want) and got == want


def _resolve_target(service_port, container):
    """A Service targetPort as a number: a named port resolves through the container's ports."""
    target = _get(service_port, "targetPort")
    if isinstance(target, int):
        return target
    return _container_port(container, str(target))


def check4(label, docs):
    def iam_backend():
        dep = _one(_deployments(docs, "iam-backend"), "iam-backend Deployment")
        container = _get(_containers(dep), 0)
        svc = _one([s for s in _of_kind(docs, "Service") if _selects(s, dep)], "Service selecting the iam-backend Deployment")
        return dep, container, svc

    def iam_http():
        _dep, container, svc = iam_backend()
        port = _service_port(svc, "http")
        got = {
            "containerPort http": _container_port(container, "http"),
            "IAM_HTTP_ADDR": _addr_port(_get(_env(container), "IAM_HTTP_ADDR"), "IAM_HTTP_ADDR"),
            "Service port http": _as_int(_get(port, "port"), "Service port http"),
            "Service targetPort http": _resolve_target(port, container),
            "PAIGASUS_SERVICES.iam": _url_port(_get(_json_map(docs, "PAIGASUS_SERVICES"), "iam"), "PAIGASUS_SERVICES.iam"),
        }
        return [] if len(set(got.values())) == 1 else [f"IAM HTTP ports disagree: {got}"]

    def iam_grpc():
        _dep, container, svc = iam_backend()
        port = _service_port(svc, "grpc")
        env_cm = _configmap_with(docs, "PAIGASUS_IAM_GRPC_URL")
        got = {
            "containerPort grpc": _container_port(container, "grpc"),
            "IAM_GRPC_ADDR": _addr_port(_get(_env(container), "IAM_GRPC_ADDR"), "IAM_GRPC_ADDR"),
            "Service port grpc": _as_int(_get(port, "port"), "Service port grpc"),
            "Service targetPort grpc": _resolve_target(port, container),
            "PAIGASUS_IAM_GRPC_URL": _url_port(_get(env_cm, "data", "PAIGASUS_IAM_GRPC_URL"), "PAIGASUS_IAM_GRPC_URL"),
        }
        return [] if len(set(got.values())) == 1 else [f"IAM gRPC ports disagree: {got}"]

    def console_port():
        problems = []
        consoles = [d for d in _of_kind(docs, "Deployment") if str(_app_label(d)).endswith("-console")]
        if not consoles:
            raise ShapeError("no console Deployment rendered")
        for dep in consoles:
            container = _get(_containers(dep), 0)
            svc = _one([s for s in _of_kind(docs, "Service") if _selects(s, dep)], f"Service selecting {_name(dep)}")
            got = {
                "containerPort http": _container_port(container, "http"),
                "PORT": _as_int(_get(_env(container), "PORT"), "PORT"),
                "Service targetPort http": _resolve_target(_service_port(svc, "http"), container),
            }
            if len(set(got.values())) != 1:
                problems.append(f"{_name(dep)} ports disagree: {got}")
        return problems

    def security_context():
        problems = []
        deps = _of_kind(docs, "Deployment")
        if not deps:
            raise ShapeError("no Deployment rendered")
        for dep in deps:
            pod = _get(dep, "spec", "template", "spec")
            psc = pod.get("securityContext") or {}
            for key, want in POD_IDENTITY.items():
                if not _same(psc.get(key), want):
                    problems.append(f"{_name(dep)}: pod securityContext.{key} is {psc.get(key)!r}, must be {want!r}")
            if "readOnlyRootFilesystem" in psc:
                problems.append(f"{_name(dep)}: pod securityContext carries readOnlyRootFilesystem (RUNBOOK-containers.md § 7: untested)")
            for c in (pod.get("containers") or []) + (pod.get("initContainers") or []):
                csc = c.get("securityContext") or {}
                if csc.get("allowPrivilegeEscalation") is not False:
                    problems.append(f"{_name(dep)}/{c.get('name')}: allowPrivilegeEscalation must be false")
                for key, want in POD_IDENTITY.items():
                    if key in csc and not _same(csc[key], want):
                        problems.append(f"{_name(dep)}/{c.get('name')}: container securityContext.{key} is {csc[key]!r}, must be absent or {want!r}")
                if "readOnlyRootFilesystem" in csc:
                    problems.append(f"{_name(dep)}/{c.get('name')}: container securityContext carries readOnlyRootFilesystem (RUNBOOK-containers.md § 7: untested)")
        return problems

    return [
        _row(f"4 iam-http {label}", iam_http),
        _row(f"4 iam-grpc {label}", iam_grpc),
        _row(f"4 console-port {label}", console_port),
        _row(f"4 security-context {label}", security_context),
    ]


```

(d) In `run_checks`, replace:

```python
        if enabled == ("iam",):
            rows.append(check2(docs, raw))
    rows += check3(chart)
```

with:

```python
        if enabled == ("iam",):
            rows.append(check2(docs, raw))
        rows += check4(label, docs)
    rows += check3(chart)
```

- [ ] **Step 4: Run it and see it pass**

Expected: self-test passes. The real run prints exactly these 20 rows (measured order) and `rc=0`:

```
PASS  [1a]
PASS  [1.1 iam]
PASS  [1.2 iam]
PASS  [1.3 iam]
PASS  [2]
PASS  [4 iam-http iam]
PASS  [4 iam-grpc iam]
PASS  [4 console-port iam]
PASS  [4 security-context iam]
PASS  [1.1 iam+gateway]
PASS  [1.2 iam+gateway]
PASS  [1.3 iam+gateway]
PASS  [4 iam-http iam+gateway]
PASS  [4 iam-grpc iam+gateway]
PASS  [4 console-port iam+gateway]
PASS  [4 security-context iam+gateway]
PASS  [3a]
PASS  [3a-prime]
PASS  [3b]
PASS  [3c]
== helm-render: all 20 rows passed ==
```

- [ ] **Step 5: Lint and commit**

```bash
git add ci/helm-render/helm_render.py
git commit -m "feat(ci): helm-render check 4, ports and security context agree (SMA-513)" -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

- [ ] **Step 6: Delete-the-feature proof for the rows no fixture covers (spec § 8)**

Run this AFTER the commit, so a restore cannot lose work. Write `$SCRATCH/dtf_module.py`:

```python
# Delete each assertion body in turn; the module self-test must go red for every one.
import pathlib
import subprocess
import sys

WT = pathlib.Path("/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-513-helm-render-gate")
M = WT / "ci/helm-render/helm_render.py"
PY = WT / "ci/helm-render/.venv/bin/python3"
orig = M.read_text()
MUTS = {
    "1.2 routing": ("    def routing():\n", "    def routing():\n        return []\n"),
    "3b": ('    "3b": {"iam-console": "differs", "gateway-console": "differs", "iam-backend": "differs"},\n', '    "3b": {},\n'),
    "3c": ('    "3c": {"iam-console": "differs", "gateway-console": "absent", "iam-backend": "equal"},\n', '    "3c": {},\n'),
    "4 iam-http": ("    def iam_http():\n", "    def iam_http():\n        return []\n"),
    "4 iam-grpc": ("    def iam_grpc():\n", "    def iam_grpc():\n        return []\n"),
    "4 console-port": ("    def console_port():\n", "    def console_port():\n        return []\n"),
}
bad = 0
try:
    for label, (old, new) in MUTS.items():
        assert orig.count(old) == 1, label
        M.write_text(orig.replace(old, new))
        p = subprocess.run([str(PY), str(M), "--self-test"], capture_output=True, text=True, check=False)
        n = sum(1 for line in p.stdout.splitlines() if line.startswith("  FAIL"))
        print(f"{label}: rc={p.returncode}, {n} self-test row(s) red")
        bad += p.returncode != 3 or n == 0
finally:
    M.write_text(orig)
print("restored:", M.read_text() == orig)
sys.exit(1 if bad else 0)
```

Run: `python3 /private/tmp/claude-501/sma-513-pr2b/dtf_module.py`

Expected (measured): `1.2 routing: rc=3, 2 …`, `3b: rc=3, 1 …`, `3c: rc=3, 3 …`, `4 iam-http: rc=3, 3 …`, `4 iam-grpc: rc=3, 2 …`, `4 console-port: rc=3, 3 …`, `restored: True`, exit 0. Then `git status --porcelain` prints nothing. Keep the six counts for the README (Task 10).

---

### Task 6: `run.sh` — the three modes, tool resolution, the chart-script loop and the wrapper rows

**Files:**
- Create: `ci/helm-render/run.sh` (mode 755)

**Interfaces:**
- Consumes: `helm_render.py --self-test` and `--chart <dir>` with the 0/3/2 contract.
- Produces: the gate contract (0/1/2) for three invocations — `bash ci/helm-render/run.sh --self-test`, `bash ci/helm-render/run.sh --negative-control`, `bash ci/helm-render/run.sh`. Shell functions: `die_infra`, `resolve_helm` (sets `HELM_BIN`), `resolve_python` (sets `PY`), `module_rc <raw>`, `script_rc <raw>`, `mapped_run_module <cmd…>`, `run_chart_script <path>`, `run_chart_scripts`, `real_run`, `self_test`, `negative_control`. Globals `REPO_ROOT`, `HERE`, `CHART`, `FIXTURES`, `CHART_SCRIPT_FLOOR=6`, `FIXTURE_TABLE`, `TMP`. Task 8 pins 35 of its lines verbatim; do not reformat them.

- [ ] **Step 1: Write `ci/helm-render/run.sh`**

```bash
#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# repo:helm-render — the static gate over charts/paigasus (SMA-513 PR 2b).
#
#   run.sh                     checks 1-4 (helm_render.py) against charts/paigasus, then every
#                              charts/paigasus/tests/*.sh (checks 5 and 6)
#   run.sh --self-test         helm_render.py's in-process rows, plus this wrapper's rc rows
#   run.sh --negative-control  each fixture under fixtures/ must fail its own named row
#
# Exit codes: 0 pass | 1 an assertion failed | 2 infrastructure error. They never collapse:
# helm_render.py exits 3 for an assertion and this wrapper maps 3 to 1 and every other non-zero
# code to 2, because a Python traceback exits 1. A chart script's 1 is an assertion; any other
# non-zero code from it (127, 141) is 2. Every row runs after a failure, and the gate's rc is the
# worst one seen: 2 before 1 before 0.
#
# Read-only: everything this gate writes lands under one mktemp -d directory, removed on EXIT,
# except ci/helm-render/.venv, which uv creates and .gitignore ignores. It never writes into
# charts/ and never calls render.sh --update. See README.md.
#
# Runs under /bin/bash 3.2.57 and bash 5: no mapfile, no declare -A, no here-string, no pipe into
# an early-exit reader (ci/actionlint check 13), and a "+" guard on every possibly-empty array.
set -euo pipefail

# proto prints NDJSON on stdout in an agent environment, which poisons every $(...) capture of a
# proto or shim call. Exported once here, so every later capture inherits it (SMA-609).
export PROTO_REPORTER=text

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
HERE="$REPO_ROOT/ci/helm-render"
CHART="$REPO_ROOT/charts/paigasus"
FIXTURES="$HERE/fixtures"
CHART_SCRIPT_FLOOR=6

# name|rows that must FAIL (";"-separated, exact)|row prefixes that must stay green (";"-separated)
FIXTURE_TABLE=(
  'literal-ingress|1.1 iam|'
  'zones-omits-enabled|1.1 iam+gateway|'
  'leaked-value|2|1.1 '
  'template-only-diff|3a;3a-prime|'
  'slug-mirror|1a|'
  'security-context|4 security-context iam;4 security-context iam+gateway|'
)

die_infra() { printf 'helm-render: infrastructure error (rc=2): %s\n' "$*" >&2; exit 2; }

SELFTEST=0
NEGATIVE=0
while [ $# -gt 0 ]; do
  case "$1" in
    --self-test)        SELFTEST=1; shift ;;
    --negative-control) NEGATIVE=1; shift ;;
    *) die_infra "unknown flag: $1" ;;
  esac
done

TMP="$(mktemp -d)" || die_infra "mktemp -d failed"
trap 'rm -rf "$TMP"' EXIT
# helm_render.py's tempfile, and helm's own cache and config, all land under $TMP.
export TMPDIR="$TMP"
export HELM_CACHE_HOME="$TMP/helm-cache" HELM_CONFIG_HOME="$TMP/helm-config" HELM_DATA_HOME="$TMP/helm-data"

# helm, resolved ONCE to an absolute path through proto from the repo root, where the shim finds
# .prototools. No fallback: the golden files are a byte pin of the pinned helm, so another helm
# would give a verdict about the wrong binary. Model: ci/release-parity/ecosystems/release-plz.sh.
resolve_helm() {
  local want got bin
  # Digits and dots only: the [plugins] table also has a `helm = "file://..."` line.
  want="$(sed -n 's/^helm = "\([0-9][0-9.]*\)"$/\1/p' "$REPO_ROOT/.prototools")"
  case "$want" in
    ''|*[!0-9.]*) die_infra "expected one 'helm = \"X.Y.Z\"' pin in .prototools, got: ${want:-<none>}" ;;
  esac
  bin="$(cd "$REPO_ROOT" && proto --reporter text bin helm)" \
    || die_infra "'proto --reporter text bin helm' failed; run 'proto install' from the repo root"
  # -f AND -x: -x alone is true for a searchable directory.
  if [ ! -f "$bin" ] || [ ! -x "$bin" ]; then
    die_infra "helm did not resolve to an executable file. Got: ${bin:-<empty>}"
  fi
  got="$("$bin" version --short)" || die_infra "'$bin version --short' failed"
  case "$got" in
    "v$want"|"v$want+"*) ;;
    *) die_infra "helm is $got but .prototools pins $want; the golden files are a byte pin of the pinned helm" ;;
  esac
  HELM_BIN="$bin"
}

# The venv interpreter, resolved ONCE. --locked stops uv from rewriting uv.lock. After this no uv
# runs, and never from inside a fixture directory (ci/ruff/README.md, the PR 206 lesson).
resolve_python() {
  local py
  command -v uv >/dev/null 2>&1 || die_infra "uv is not on PATH; run 'proto install', or add ~/.proto/shims to PATH"
  py="$(uv run --locked --project "$HERE" python3 -c 'import sys, yaml; print(sys.executable)')" \
    || die_infra "uv could not provide the locked PyYAML environment for $HERE"
  case "$py" in
    "$HERE/.venv/"*) ;;
    *) die_infra "the interpreter is not under $HERE/.venv. Got: ${py:-<empty>}" ;;
  esac
  if [ ! -f "$py" ] || [ ! -x "$py" ]; then
    die_infra "the venv interpreter is not an executable file: $py"
  fi
  PY="$py"
}

module_rc() {  # $1: helm_render.py's raw rc -> the gate's rc
  case "$1" in 0) echo 0 ;; 3) echo 1 ;; *) echo 2 ;; esac
}

script_rc() {  # $1: a chart script's raw rc -> the gate's rc
  case "$1" in 0) echo 0 ;; 1) echo 1 ;; *) echo 2 ;; esac
}

# Runs "$@" (the module, or a stand-in in the self-test) and RETURNS the mapped rc.
mapped_run_module() {
  local raw=0
  "$@" || raw=$?
  return "$(module_rc "$raw")"
}

# Runs one chart script under THIS bash, not its shebang, so the bash 3.2 run covers it.
run_chart_script() {
  local raw=0
  "$BASH" "$1" --set ingress.host=console.example.test || raw=$?
  return "$(script_rc "$raw")"
}

run_chart_scripts() {
  local s worst=0 cs_rc
  local scripts=()
  for s in "$CHART"/tests/*.sh; do
    [ -f "$s" ] && scripts+=("$s")
  done
  if [ "${#scripts[@]}" -lt "$CHART_SCRIPT_FLOOR" ]; then
    printf 'FAIL  [6 floor]: %s chart script(s) under %s, expected at least %s\n' \
      "${#scripts[@]}" "$CHART/tests" "$CHART_SCRIPT_FLOOR" >&2
    return 2
  fi
  for s in "${scripts[@]+"${scripts[@]}"}"; do
    printf -- '--- [6 %s]\n' "${s##*/}"
    cs_rc=0; run_chart_script "$s" || cs_rc=$?
    case "$cs_rc" in
      0) printf 'PASS  [6 %s]\n' "${s##*/}" ;;
      1) printf 'FAIL  [6 %s]: assertion failure\n' "${s##*/}" >&2 ;;
      *) printf 'FAIL  [6 %s]: infrastructure error (rc=2)\n' "${s##*/}" >&2 ;;
    esac
    if [ "$cs_rc" -gt "$worst" ]; then worst="$cs_rc"; fi
  done
  return "$worst"
}

real_run() {
  local worst=0 mod_rc=0 all_rc=0
  mod_rc=0; mapped_run_module "$PY" "$HERE/helm_render.py" --chart "$CHART" || mod_rc=$?
  if [ "$mod_rc" -gt "$worst" ]; then worst="$mod_rc"; fi
  all_rc=0; run_chart_scripts || all_rc=$?
  if [ "$all_rc" -gt "$worst" ]; then worst="$all_rc"; fi
  return "$worst"
}

self_test() {
  local failures=0 got stub st_mod_rc
  _st_row() {  # $1 label, $2 want, $3 got
    if [ "$2" = "$3" ]; then
      printf '  ok %s\n' "$1"
    else
      printf '  FAIL %s: expected %s, got %s\n' "$1" "$2" "$3" >&2
      failures=$((failures + 1))
    fi
  }
  st_mod_rc=0; mapped_run_module "$PY" "$HERE/helm_render.py" --self-test || st_mod_rc=$?
  _st_row "helm_render.py --self-test passes" 0 "$st_mod_rc"
  _st_row "module rc 0 maps to 0" 0 "$(module_rc 0)"
  _st_row "module rc 3 maps to 1" 1 "$(module_rc 3)"
  _st_row "module rc 1 (a traceback) maps to 2" 2 "$(module_rc 1)"
  _st_row "module rc 2 maps to 2" 2 "$(module_rc 2)"
  _st_row "chart-script rc 1 maps to 1" 1 "$(script_rc 1)"
  _st_row "chart-script rc 127 maps to 2" 2 "$(script_rc 127)"
  _st_row "chart-script rc 141 maps to 2" 2 "$(script_rc 141)"
  # The same mappings through the real call paths, with live processes.
  got=0; mapped_run_module "$PY" -c 'import sys; sys.exit(3)' || got=$?
  _st_row "a live module exit 3 reaches the gate as 1" 1 "$got"
  got=0; mapped_run_module "$PY" -c 'raise SystemExit("traceback stand-in")' || got=$?
  _st_row "a live module exit 1 reaches the gate as 2" 2 "$got"
  stub="$TMP/stub-chart-script.sh"
  printf 'exit 127\n' >"$stub"
  got=0; run_chart_script "$stub" || got=$?
  _st_row "a live chart-script rc 127 reaches the gate as 2" 2 "$got"
  printf 'exit 1\n' >"$stub"
  got=0; run_chart_script "$stub" || got=$?
  _st_row "a live chart-script rc 1 reaches the gate as 1" 1 "$got"
  # The worst rc seen: the module's own 2 must reach the caller as 2, not be folded into 1.
  if [ "$st_mod_rc" -eq 2 ]; then return 2; fi
  if [ "$failures" -gt 0 ]; then return 1; fi
  return 0
}

negative_control() {
  local entry name rest fails greens row out fx_rc verdict total
  local nc_failed=0 nc_inconclusive=0 d known
  # The table and the fixture directories must agree: a directory with no row is never run.
  for d in "$FIXTURES"/*/; do
    [ -d "$d" ] || continue
    known=0
    for entry in "${FIXTURE_TABLE[@]}"; do
      if [ "${entry%%|*}" = "$(basename "$d")" ]; then known=1; fi
    done
    if [ "$known" -eq 0 ]; then
      printf 'negative-control FAILED [%s]: fixture directory has no FIXTURE_TABLE row\n' "$(basename "$d")" >&2
      nc_failed=$((nc_failed + 1))
    fi
  done
  for entry in "${FIXTURE_TABLE[@]}"; do
    name="${entry%%|*}"; rest="${entry#*|}"; fails="${rest%%|*}"; greens="${rest#*|}"
    out="$TMP/$name.out"
    rm -rf "$TMP/paigasus"
    if [ ! -d "$FIXTURES/$name" ] || ! cp -R "$CHART" "$TMP/paigasus" || ! cp -R "$FIXTURES/$name/." "$TMP/paigasus/"; then
      printf 'negative-control INCONCLUSIVE: infrastructure error (rc=2) [%s]: cannot build the fixture chart\n' "$name" >&2
      nc_inconclusive=$((nc_inconclusive + 1)); continue
    fi
    fx_rc=0; "$PY" "$HERE/helm_render.py" --chart "$TMP/paigasus" >"$out" 2>&1 || fx_rc=$?
    verdict=OK
    if [ "$fx_rc" != 0 ] && [ "$fx_rc" != 3 ]; then
      verdict="INCONCLUSIVE: infrastructure error (rc=$fx_rc)"
    elif [ "$fx_rc" != 3 ]; then
      verdict="FAILED: the module passed a mutated chart (rc=0)"
    else
      while [ -n "$fails" ]; do
        row="${fails%%;*}"
        if [ "$row" = "$fails" ]; then fails=""; else fails="${fails#*;}"; fi
        if ! grep -qF -- "FAIL  [$row]" "$out"; then verdict="FAILED: named row [$row] did not fail"; fi
      done
      while [ -n "$greens" ]; do
        row="${greens%%;*}"
        if [ "$row" = "$greens" ]; then greens=""; else greens="${greens#*;}"; fi
        if grep -qF -- "FAIL  [$row" "$out"; then verdict="FAILED: a row starting [$row must stay green"; fi
      done
    fi
    total="$(grep -c '^FAIL  \[' "$out" || true)"
    case "$verdict" in
      OK) printf 'negative-control OK [%s]: rc 3, %s row(s) failed, the named row(s) among them\n' "$name" "$total" ;;
      INCONCLUSIVE*) nc_inconclusive=$((nc_inconclusive + 1))
                     printf 'negative-control %s [%s]\n' "$verdict" "$name" >&2; sed 's/^/    /' "$out" >&2 ;;
      *) nc_failed=$((nc_failed + 1))
         printf 'negative-control %s [%s]\n' "$verdict" "$name" >&2; sed 's/^/    /' "$out" >&2 ;;
    esac
  done
  if [ "$nc_failed" -gt 0 ]; then
    printf 'helm-render negative control: %d fixture(s) FAILED\n' "$nc_failed" >&2
    return 1
  elif [ "$nc_inconclusive" -gt 0 ]; then
    printf 'helm-render negative control: %d fixture(s) INCONCLUSIVE\n' "$nc_inconclusive" >&2
    return 2
  fi
  printf '== helm-render negative control passed (%d fixtures) ==\n' "${#FIXTURE_TABLE[@]}"
}

resolve_helm
resolve_python
# The venv first, so the chart scripts' bare python3 gets PyYAML; then the pinned helm.
PATH="$(dirname "$PY"):$(dirname "$HELM_BIN"):$PATH"
export PATH

if [ "$SELFTEST" = 1 ]; then
  st_rc=0; self_test || st_rc=$?
  if [ "$st_rc" -ne 0 ]; then
    printf 'helm-render self-test: FAILED (rc=%s)\n' "$st_rc" >&2
    exit "$st_rc"
  fi
  printf '== helm-render self-test passed ==\n'
  exit 0
fi

if [ "$NEGATIVE" = 1 ]; then
  nc_rc=0; negative_control || nc_rc=$?
  exit "$nc_rc"
fi

real_rc=0; real_run || real_rc=$?
if [ "$real_rc" -eq 0 ]; then
  printf '== helm-render: all checks passed ==\n'
else
  printf '== helm-render: FAILED (rc=%s) ==\n' "$real_rc" >&2
fi
exit "$real_rc"
```

Run `chmod 755 ci/helm-render/run.sh`.

- [ ] **Step 2: Run the three modes under `/bin/bash` 3.2.57**

Write `$SCRATCH/modes.sh`:

```bash
#!/bin/bash
# usage: /bin/bash modes.sh <bash-binary> <mode>...   (mode: --self-test | --negative-control | real)
set -uo pipefail
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
B="$1"; shift
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-513-helm-render-gate || exit 2
for mode in "$@"; do
  echo "===== $B run.sh $mode"
  if [ "$mode" = real ]; then "$B" ci/helm-render/run.sh; else "$B" ci/helm-render/run.sh "$mode"; fi
  echo "rc=$?"
done
git status --porcelain
```

Run: `/bin/bash $SCRATCH/modes.sh /bin/bash --self-test real --negative-control`

Expected:
- `--self-test`: `== helm_render.py self-test passed ==`, then twelve `  ok …` rows (`traceback stand-in` on stderr between two of them is expected), `== helm-render self-test passed ==`, `rc=0`.
- real: the 20 module rows, then `--- [6 env.sh]` … `PASS  [6 render.sh]` for the six scripts in glob order (`env.sh`, `ingress.sh`, `maps.sh`, `names.sh`, `refusals.sh`, `render.sh`), `== helm-render: all checks passed ==`, `rc=0`.
- `--negative-control` (no fixtures yet — this is Task 7's failing test): six lines `negative-control INCONCLUSIVE: infrastructure error (rc=2) [<name>]: cannot build the fixture chart`, then `helm-render negative control: 6 fixture(s) INCONCLUSIVE`, `rc=2`.
- `git status --porcelain` prints only `?? ci/helm-render/run.sh` (the gate wrote nothing else).

- [ ] **Step 3: Delete-the-feature proof for the wrapper rows**

Write `$SCRATCH/dtf_wrapper.py`:

```python
# Break each rc map in turn; the wrapper self-test must exit 1 for each.
import pathlib
import subprocess

WT = pathlib.Path("/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-513-helm-render-gate")
R = WT / "ci/helm-render/run.sh"
orig = R.read_text()
MUTS = {
    "module 3 -> 1": ('case "$1" in 0) echo 0 ;; 3) echo 1 ;; *) echo 2 ;; esac', 'case "$1" in 0) echo 0 ;; 3) echo 3 ;; *) echo 2 ;; esac'),
    "module * -> 2": ('case "$1" in 0) echo 0 ;; 3) echo 1 ;; *) echo 2 ;; esac', 'case "$1" in 0) echo 0 ;; 3) echo 1 ;; *) echo 1 ;; esac'),
    "script * -> 2": ('case "$1" in 0) echo 0 ;; 1) echo 1 ;; *) echo 2 ;; esac', 'case "$1" in 0) echo 0 ;; 1) echo 1 ;; *) echo 0 ;; esac'),
}
env = {"PATH": f"{pathlib.Path.home()}/.proto/shims:{pathlib.Path.home()}/.proto/bin:/usr/bin:/bin", "HOME": str(pathlib.Path.home())}
try:
    for label, (old, new) in MUTS.items():
        assert orig.count(old) == 1, label
        R.write_text(orig.replace(old, new))
        p = subprocess.run(["/bin/bash", str(R), "--self-test"], capture_output=True, text=True, env=env, check=False)
        print(f"{label}: rc={p.returncode}; " + "; ".join(x for x in p.stderr.splitlines() if x.startswith("  FAIL")))
finally:
    R.write_text(orig)
print("restored:", R.read_text() == orig)
```

Run it after Step 5's commit: `python3 $SCRATCH/dtf_wrapper.py`. Expected: each of the three lines shows `rc=1` and at least one `  FAIL … maps to …` or `  FAIL a live … reaches the gate as …` row; then `restored: True`.

- [ ] **Step 4: Check the shell rules**

Run: `grep -nE 'mapfile|declare -A|<<<' ci/helm-render/run.sh` — expected: no output.
Run: `grep -nE '\| *(grep -q|grep -m|head|awk)' ci/helm-render/run.sh` — expected: no output.

- [ ] **Step 5: Commit**

```bash
git add ci/helm-render/run.sh
git commit -m "feat(ci): helm-render run.sh with three modes, tool resolution and rc mapping (SMA-513)" -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

Then run Step 3.

---

### Task 7: The six overlay fixtures and the negative control

**Files:**
- Create: `ci/helm-render/fixtures/literal-ingress/templates/ingress.yaml`
- Create: `ci/helm-render/fixtures/zones-omits-enabled/templates/_helpers.tpl`
- Create: `ci/helm-render/fixtures/leaked-value/templates/console-env-configmap.yaml`
- Create: `ci/helm-render/fixtures/template-only-diff/templates/console-deployment.yaml`
- Create: `ci/helm-render/fixtures/slug-mirror/templates/_helpers.tpl`
- Create: `ci/helm-render/fixtures/security-context/templates/console-deployment.yaml`
- Create (scratch): `$SCRATCH/make_fixtures.py`, `$SCRATCH/fixture_rows.sh`

**Interfaces:**
- Consumes: `FIXTURE_TABLE` and `negative_control` from Task 6; the live chart files, which each fixture copies whole and changes in one block.
- Produces: six fixtures, each a whole-file copy of one live template (two for none) with ONE mutation. The negative control reports `OK` for all six.

- [ ] **Step 1: The failing test is Task 6 Step 2's control run**

It returned `rc=2` with six INCONCLUSIVE rows. Keep that output as the red state.

- [ ] **Step 2: Generate the fixtures from the live chart**

Each fixture is the live file with one exact block replaced. The script asserts that each block occurs exactly once, so a stale anchor fails loudly. Write `$SCRATCH/make_fixtures.py`:

```python
# Build ci/helm-render/fixtures/* from the LIVE chart: copy one file, replace one exact block.
import pathlib

WT = pathlib.Path("/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-513-helm-render-gate")
CHART = WT / "charts/paigasus"
FIX = WT / "ci/helm-render/fixtures"


def put(fixture, rel, old, new):
    src = (CHART / rel).read_text()
    assert src.count(old) == 1, (fixture, rel, src.count(old))
    out = FIX / fixture / rel
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(src.replace(old, new))
    print("wrote", out.relative_to(WT))


put(
    "literal-ingress",
    "templates/ingress.yaml",
    """          {{- range $id, $z := .Values.zones }}
          {{- if $z.enabled }}
          # This zone's own {{ $z.basePath }}/auth/* routes are IN-APP route handlers and fall
          # under its own prefix already, so they need no rule of their own. Written per zone,
          # not as a single static comment naming both prefixes: a disabled zone must leave no
          # trace anywhere in the rendered output (see tests/ingress.sh), and a comment naming
          # the OTHER zone's prefix here would leak it into an iam-only render.
          - path: {{ $z.basePath }}
            pathType: Prefix
            backend:
              service:
                name: {{ include "paigasus.name" (list $root (printf "%s-console" $id)) }}
                port:
                  name: http
          {{- end }}
          {{- end }}
""",
    """          # NEGATIVE-CONTROL FIXTURE (ci/helm-render): both rules written literally, NOT ranged
          # over the enabled zones. Check 1.1 must fail for the iam-only subset.
          - path: /iam
            pathType: Prefix
            backend:
              service:
                name: {{ include "paigasus.name" (list $root "iam-console") }}
                port:
                  name: http
          - path: /gateway
            pathType: Prefix
            backend:
              service:
                name: {{ include "paigasus.name" (list $root "gateway-console") }}
                port:
                  name: http
""",
)

put(
    "zones-omits-enabled",
    "templates/_helpers.tpl",
    """{{- define "paigasus.zoneMapJson" -}}
{{- $m := dict -}}
{{- range $id, $z := .Values.zones -}}
{{- if $z.enabled -}}{{- $_ := set $m $id $z.basePath -}}{{- end -}}""",
    """{{- define "paigasus.zoneMapJson" -}}
{{- /* NEGATIVE-CONTROL FIXTURE (ci/helm-render): skips an ENABLED gateway zone. */ -}}
{{- $m := dict -}}
{{- range $id, $z := .Values.zones -}}
{{- if and $z.enabled (ne $id "gateway") -}}{{- $_ := set $m $id $z.basePath -}}{{- end -}}""",
)

put(
    "leaked-value",
    "templates/console-env-configmap.yaml",
    """  PAIGASUS_SESSION_STORE: "redis"
""",
    """  PAIGASUS_SESSION_STORE: "redis"
  # NEGATIVE-CONTROL FIXTURE (ci/helm-render): written outside any range, so it leaks
  # zones.gateway.backend.url even when the gateway zone is disabled. Check 2 must fail and
  # check 1.1 must stay green.
  PAIGASUS_UPSTREAM_URL: {{ .Values.zones.gateway.backend.url | quote }}
""",
)

put(
    "template-only-diff",
    "templates/console-deployment.yaml",
    '''          image: "{{ $z.console.image.repository }}:{{ $z.console.image.tag | default $root.Chart.AppVersion }}"''',
    '''          # NEGATIVE-CONTROL FIXTURE (ci/helm-render): every console reads zones.iam's tag.
          image: "{{ $z.console.image.repository }}:{{ $root.Values.zones.iam.console.image.tag | default $root.Chart.AppVersion }}"''',
)

put(
    "slug-mirror",
    "templates/_helpers.tpl",
    """{{- define "paigasus.serviceSlugs" -}}
iam gateway
{{- end -}}""",
    """{{- define "paigasus.serviceSlugs" -}}
iam gateway billing
{{- end -}}""",
)

put(
    "security-context",
    "templates/console-deployment.yaml",
    """        runAsGroup: 65532
        runAsNonRoot: true
""",
    """        runAsGroup: 65532
        # NEGATIVE-CONTROL FIXTURE (ci/helm-render): runAsNonRoot dropped from the console pod.
""",
)
```

The `security-context` anchor occurs once in `console-deployment.yaml` (the backend's copy is in another file). The `template-only-diff` fixture reads `zones.iam`, not the first zone in range order: Helm ranges alphabetically, so the first zone is `gateway` (spec § 6).

Run: `python3 $SCRATCH/make_fixtures.py`. Expected: six `wrote ci/helm-render/fixtures/…` lines. Then confirm the chart is unchanged: `git status --porcelain charts/` prints nothing.

- [ ] **Step 3: Run the negative control and see it pass**

Run: `/bin/bash $SCRATCH/modes.sh /bin/bash --negative-control`

Expected (measured):

```
negative-control OK [literal-ingress]: rc 3, 3 row(s) failed, the named row(s) among them
negative-control OK [zones-omits-enabled]: rc 3, 1 row(s) failed, the named row(s) among them
negative-control OK [leaked-value]: rc 3, 1 row(s) failed, the named row(s) among them
negative-control OK [template-only-diff]: rc 3, 2 row(s) failed, the named row(s) among them
negative-control OK [slug-mirror]: rc 3, 1 row(s) failed, the named row(s) among them
negative-control OK [security-context]: rc 3, 2 row(s) failed, the named row(s) among them
== helm-render negative control passed (6 fixtures) ==
rc=0
```

- [ ] **Step 4: Measure each fixture's failing rows, one at a time**

Write `$SCRATCH/fixture_rows.sh`:

```bash
#!/bin/bash
# For each fixture: build the fixture chart in a temp dir and list the module's FAIL rows.
set -uo pipefail
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
WT=/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-513-helm-render-gate
cd "$WT" || exit 2
PY="$(uv run --locked --project ci/helm-render python3 -c 'import sys; print(sys.executable)')"
HELM="$(proto --reporter text bin helm)"
PATH="$(dirname "$PY"):$(dirname "$HELM"):$PATH"
T="$(mktemp -d)"
for d in ci/helm-render/fixtures/*/; do
  n="$(basename "$d")"
  rm -rf "$T/paigasus"
  cp -R charts/paigasus "$T/paigasus"
  cp -R "$d." "$T/paigasus/"
  "$PY" ci/helm-render/helm_render.py --chart "$T/paigasus" > "$T/$n.out" 2>&1
  echo "== $n rc=$?"
  grep '^FAIL\|^INFRA' "$T/$n.out" | cut -c1-160
done
rm -rf "$T"
```

Run: `/bin/bash $SCRATCH/fixture_rows.sh`

Expected (measured; record it in the README in Task 10):

| Fixture | rc | FAIL rows |
| -- | -- | -- |
| `leaked-value` | 3 | `[2]` only (both `1.1` rows stay green) |
| `literal-ingress` | 3 | `[1.1 iam]` (named), `[1.2 iam]` (no Service `…-gateway-console` exists), `[2]` (the path `/gateway` and the Service name) |
| `security-context` | 3 | `[4 security-context iam]`, `[4 security-context iam+gateway]` (both named) |
| `slug-mirror` | 3 | `[1a]` only |
| `template-only-diff` | 3 | `[3a]` (`gateway-console` differs), `[3a-prime]` (`gateway-console` is equal) — both named |
| `zones-omits-enabled` | 3 | `[1.1 iam+gateway]` only |

If a row set differs, stop and read the output. Do not change the chart to make it match.

- [ ] **Step 5: Commit**

```bash
git add ci/helm-render/fixtures
git commit -m "test(ci): six helm-render negative-control fixtures (SMA-513)" -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

- [ ] **Step 6: Prove the control can report FAILED and INCONCLUSIVE**

After the commit, write `$SCRATCH/control_bites.py`:

```python
# (1) A check with its body deleted must make the control report FAILED (rc 1).
# (2) A table row with no fixture directory must make it report INCONCLUSIVE (rc 2).
import pathlib
import subprocess

WT = pathlib.Path("/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-513-helm-render-gate")
M = WT / "ci/helm-render/helm_render.py"
R = WT / "ci/helm-render/run.sh"
orig_m, orig_r = M.read_text(), R.read_text()
env = {"PATH": f"{pathlib.Path.home()}/.proto/shims:{pathlib.Path.home()}/.proto/bin:/usr/bin:/bin", "HOME": str(pathlib.Path.home())}


def run(label):
    p = subprocess.run(["/bin/bash", str(R), "--negative-control"], capture_output=True, text=True, env=env, check=False)
    print(f"{label}: rc={p.returncode}")
    for line in (p.stdout + p.stderr).splitlines():
        if line.startswith(("negative-control FAILED", "negative-control INCONCLUSIVE", "helm-render negative control")):
            print("   ", line[:140])


try:
    old = "    def body():\n        problems = []\n        for i, doc in enumerate(docs):\n"
    assert orig_m.count(old) == 1
    M.write_text(orig_m.replace(old, "    def body():\n        return []\n        problems = []\n        for i, doc in enumerate(docs):\n"))
    run("check 2 body deleted")
    M.write_text(orig_m)
    R.write_text(orig_r.replace("  'slug-mirror|1a|'\n", "  'slug-mirror|1a|'\n  'no-such-fixture|1a|'\n"))
    run("a table row with no fixture directory")
finally:
    M.write_text(orig_m)
    R.write_text(orig_r)
print("restored:", M.read_text() == orig_m and R.read_text() == orig_r)
```

Run: `python3 $SCRATCH/control_bites.py`. Expected (measured): `check 2 body deleted: rc=1` with `negative-control FAILED: the module passed a mutated chart (rc=0) [leaked-value]`; `a table row with no fixture directory: rc=2` with `negative-control INCONCLUSIVE: infrastructure error (rc=2) [no-such-fixture]: …`; `restored: True`. Then `git status --porcelain` prints nothing.

---

### Task 8: Registration obligations 1–7 and the Moon task

Re-read `ci/CLAUDE.md` "Registering a new `repo:*` gate" before you start. If it holds a newer rule than the seven below, follow it and record the difference in the commit subject's PR notes.

**Files:**
- Modify: `moon.yml` — append the `helm-render` task after line 967; add one `repo:affected-smoke` input after line 248
- Modify: `.github/workflows/ci.yml:268`
- Modify: `CLAUDE.md:145`
- Modify: `ci/affected-graph/ci_targets.py` (line ranges in the File Structure table)
- Modify: `ci/actionlint/run.sh` — one entry after line 2159
- Create (scratch): `$SCRATCH/add_helm_arg.py`

**Interfaces:**
- Consumes: the four task-script lines and the 35 `run.sh` lines Task 6 wrote.
- Produces: `HELM_RENDER_SH_CALL_SITES: tuple[str, ...]` (35 entries); `check_self_invocation(run_sh_text, scripts, actionlint_sh_text, release_parity_sh_text, workflow_credentials_sh_text, release_plan_sh_text, ruff_sh_text, next_public_free_sh_text, helm_render_sh_text)`; `_CALL_SITE_SOURCE_KEYS` gains `"helm_render"`; registry keys `"helm-render"` in `REQUIRED_REPO_TASKS`, `SELF_TASK_EXPECTED_GLOBS`, `SELF_SCHEDULED_GATES`.

- [ ] **Step 1: Add the Moon task (append to `moon.yml`)**

Append after the last line (967, `      - 'ts/**/*'`), with one blank line before it:

```yaml

  helm-render:
    description: 'Render charts/paigasus and assert zone coupling and routing, no trace of a disabled zone, independent rollout, port and security-context agreement, and run every chart script (SMA-513).'
    # WHY THIS EXISTS — charts/paigasus/tests/ holds six scripts (lint, golden files, coupling,
    # maps, names, env, refusals) and until this task nothing ran them in CI. helm_render.py adds
    # the properties they do not assert: an independent expected zone set, the route target, a
    # sentinel-value leak search, spec.template-only rollout diffs, and the chart's own port and
    # security-context agreement (spec 2026-09-22-sma-513-pr2b-helm-render-gate-design.md § 5).
    #
    # INPUTS. `.prototools` and `.proto/plugins/helm.toml` are the helm pin: the golden files are
    # a byte pin of that helm, so without them a helm bump serves a cached PASS. The proto and the
    # two TypeScript files are what check 1a reads. There are no `deps`: nothing here reads a
    # build output.
    #
    # `--self-test` and `--negative-control` run FIRST and in the SAME block. `set -euo pipefail`
    # is REQUIRED: Moon does not enable errexit for `script:` blocks and takes the block's status
    # from its LAST command, so without it a failing control is masked by the passing real run.
    # These four lines are pinned by SELF_SCHEDULED_GATES.
    script: |
      set -euo pipefail
      bash ci/helm-render/run.sh --self-test
      bash ci/helm-render/run.sh --negative-control
      bash ci/helm-render/run.sh
    toolchain: 'system'
    inputs:
      - '.prototools'
      - '.proto/plugins/helm.toml'
      - 'charts/**/*'
      - 'ci/helm-render/**/*'
      - 'contracts/proto/paigasus/common/v1/service_info.proto'
      - 'ts/packages/paigasus-discovery/src/core/state.ts'
      - 'ts/packages/paigasus-proto/src/capability.ts'
```

- [ ] **Step 2: Obligations 1 and 2 — `T` and the CLAUDE.md mirror**

In `.github/workflows/ci.yml:268`, replace `:next-public-free :test-e2e)` with `:next-public-free :helm-render :test-e2e)`. The array stays on one line.

In `CLAUDE.md:145`, replace the line `  :next-public-free :test-e2e` with `  :next-public-free :helm-render :test-e2e`. Change nothing else in that block, and do not touch the two marker lines around it. `check_docs` compares the two lists in order, so the position must match `T`.

- [ ] **Step 3: Write the failing test — run the ci-targets guard now**

Run:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 ci/affected-graph/ci_targets.py; echo "rc=$?"
```

Expected: `rc=1`. The report names `helm-render` under the SELF_SCHEDULED_GATES coverage row (`check_self_scheduled_coverage`: a `repo:*` task whose script runs `--self-test` has no `SELF_SCHEDULED_GATES` entry). This is the red state.

Also measure how Moon reports the new task's inputs, for Step 5:

```bash
moon query tasks > "$SCRATCH/tasks.json"
python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); d=d.get("tasks", d); t=d["repo"]["helm-render"]; print(sorted(g for g in t.get("inputGlobs",{}) if not g.startswith(".moon/"))); print(sorted(t.get("inputFiles",{})))' "$SCRATCH/tasks.json"
```

Expected: `['charts/**/*', 'ci/helm-render/**/*']` and `['.proto/plugins/helm.toml', '.prototools', 'contracts/proto/paigasus/common/v1/service_info.proto', 'ts/packages/paigasus-discovery/src/core/state.ts', 'ts/packages/paigasus-proto/src/capability.ts']`. If Moon reports another split, use its split in Step 5 (globs sorted, then files sorted).

- [ ] **Step 4: Add the ninth argument at all 59 existing `check_self_invocation` calls**

`check_self_invocation` takes one positional text per gate (`ci_targets.py:1691-1695`), and 59 calls pass eight today (measured with `ast`). Write `$SCRATCH/add_helm_arg.py`:

```python
# Append the ninth positional argument to every check_self_invocation(...) call. The production
# call inside collect_findings gets sh["helm_render"]; every self-test call gets
# wired_helm_render. Refuses to run twice (a call with nine arguments stops it).
#   python3 add_helm_arg.py ci/affected-graph/ci_targets.py
import ast
import sys

path = sys.argv[1]
src = open(path).read()
tree = ast.parse(src)
lines = src.splitlines(keepends=True)
offsets = [0]
for line in lines:
    offsets.append(offsets[-1] + len(line))
parents = {child: node for node in ast.walk(tree) for child in ast.iter_child_nodes(node)}


def enclosing_function(node):
    while node in parents:
        node = parents[node]
        if isinstance(node, ast.FunctionDef):
            return node.name
    return None


inserts = []
for node in ast.walk(tree):
    if isinstance(node, ast.Call) and getattr(node.func, "id", None) == "check_self_invocation":
        if len(node.args) != 8 or node.keywords:
            sys.exit(f"line {node.lineno}: expected 8 positional args, found {len(node.args)}")
        last = node.args[7]
        pos = offsets[last.end_lineno - 1] + last.end_col_offset
        value = 'sh["helm_render"]' if enclosing_function(node) == "collect_findings" else "wired_helm_render"
        inserts.append((pos, f", {value}"))
for pos, text in sorted(inserts, reverse=True):
    src = src[:pos] + text + src[pos:]
open(path, "w").write(src)
print(f"rewrote {len(inserts)} calls")
```

Run: `python3 $SCRATCH/add_helm_arg.py ci/affected-graph/ci_targets.py`. Expected: `rewrote 59 calls`.

- [ ] **Step 5: Obligations 3–6 and the plumbing in `ci/affected-graph/ci_targets.py`**

Make these edits with the Edit tool. Each `old` block occurs exactly once.

(a) `REQUIRED_REPO_TASKS` (obligation 6). Replace:

```python
    # entry the whole gate, control included, could be switched off with every check green.
    "next-public-free",
)
```

with:

```python
    # entry the whole gate, control included, could be switched off with every check green.
    "next-public-free",
    # SMA-513 PR 2b. Same reasoning as the four entries above: repo:helm-render carries a
    # --negative-control, and check_forward's `want`/`got` shrink CONSISTENTLY when a task is
    # dropped from `T` and made CI-ineligible in the same edit — so without a floor entry the whole
    # gate, control included, could be switched off with every check green.
    "helm-render",
)
```

(b) `SELF_TASK_EXPECTED_GLOBS` (obligation 4). Replace:

```python
        "ci/next-public/**/*",
        "ts/**/*",
    ),
}
```

with:

```python
        "ci/next-public/**/*",
        "ts/**/*",
    ),
    # SMA-513 PR 2b. Two globs then five literals, in check_gate_inputs' comparison order (globs
    # sorted, then files sorted — `.proto/plugins/helm.toml` sorts before `.prototools` because
    # `/` < `t`). The two helm files are the byte pin the golden files depend on: drop them and a
    # helm bump serves a cached PASS. The proto and the two TypeScript files are what check 1a
    # reads; drop one and a new capability slug or a changed derivation line no longer re-keys it.
    "helm-render": (
        "charts/**/*",
        "ci/helm-render/**/*",
        ".proto/plugins/helm.toml",
        ".prototools",
        "contracts/proto/paigasus/common/v1/service_info.proto",
        "ts/packages/paigasus-discovery/src/core/state.ts",
        "ts/packages/paigasus-proto/src/capability.ts",
    ),
}
```

(c) `SELF_SCHEDULED_GATES` (obligation 3). Replace:

```python
        "bash ci/next-public/run.sh --negative-control",
        "bash ci/next-public/run.sh",
    ),
}
```

with:

```python
        "bash ci/next-public/run.sh --negative-control",
        "bash ci/next-public/run.sh",
    ),
    # SMA-513 PR 2b. Four lines, like the other self-scheduled gates: `set -euo pipefail` is what
    # makes a failing control propagate, since Moon takes a `script:` block's status from its LAST
    # command. Whole-line matched: `bash ci/helm-render/run.sh` is a strict PREFIX of both flagged
    # lines, so a substring test would report the gate wired after the REAL RUN was deleted.
    "helm-render": (
        "set -euo pipefail",
        "bash ci/helm-render/run.sh --self-test",
        "bash ci/helm-render/run.sh --negative-control",
        "bash ci/helm-render/run.sh",
    ),
}
```

(d) `HELM_RENDER_SH_CALL_SITES` (obligation 5). Replace:

```python
    'if [ "$failures" -gt 0 ]; then',
)


def read_input(path, label):
```

with:

```python
    'if [ "$failures" -gt 0 ]; then',
)


# SMA-513 PR 2b — ci/helm-render/run.sh's load-bearing lines. REACHABILITY IS NOT AUTOMATIC: this
# check only runs when repo:affected-smoke is scheduled, so moon.yml lists `ci/helm-render/**/*`
# among its inputs and ci/actionlint/run.sh's T_AFFECTED_SMOKE_REQUIRED_INPUTS floors that entry.
#
# Matched as stripped WHOLE LINES, like the ruff and next-public-free haystacks and for both of
# their reasons: the `case` arms and `if` bodies are indented, so a column-0 rule would reject the
# real executing lines, while a substring rule would let a COMMENTED-OUT copy satisfy the pin.
# Every entry was verified to occur EXACTLY ONCE in run.sh before this tuple was written.
#
# What the groups close, in order:
#   - the flag parse, the self-test dispatch and its failure guard: a neutered parse or dispatch
#     falls through to a mode that proves nothing;
#   - the NEGATIVE guard, the control's dispatch and exit, and its assertion body (the fixture
#     invocation, the rc arms, the named-row and stay-green greps): with only the structure
#     pinned, deleting every assertion left a control that printed "passed" (the
#     WORKFLOW_CREDENTIALS_SH_CALL_SITES measurement);
#   - both report arms, so a control that counted failures cannot swallow them;
#   - the six FIXTURE_TABLE rows, so a fixture cannot be dropped with its directory in silence;
#   - the two rc maps, so 3 -> 1 and "anything else -> 2" cannot be rewritten to 0;
#   - the real run: the module call and the chart-script loop (glob, floor, floor guard and
#     invocation). Without these a deleted loop leaves the gate green with no `helm lint` and no
#     golden diff at all (spec § 7 obligation 5).
HELM_RENDER_SH_CALL_SITES = (
    "--self-test)        SELFTEST=1; shift ;;",
    "--negative-control) NEGATIVE=1; shift ;;",
    'if [ "$SELFTEST" = 1 ]; then',
    "st_rc=0; self_test || st_rc=$?",
    'if [ "$st_rc" -ne 0 ]; then',
    'exit "$st_rc"',
    'if [ "$NEGATIVE" = 1 ]; then',
    "nc_rc=0; negative_control || nc_rc=$?",
    'exit "$nc_rc"',
    'fx_rc=0; "$PY" "$HERE/helm_render.py" --chart "$TMP/paigasus" >"$out" 2>&1 || fx_rc=$?',
    'if [ "$fx_rc" != 0 ] && [ "$fx_rc" != 3 ]; then',
    'elif [ "$fx_rc" != 3 ]; then',
    'if ! grep -qF -- "FAIL  [$row]" "$out"; then verdict="FAILED: named row [$row] did not fail"; fi',
    'if grep -qF -- "FAIL  [$row" "$out"; then verdict="FAILED: a row starting [$row must stay green"; fi',
    'if [ "$nc_failed" -gt 0 ]; then',
    "printf 'helm-render negative control: %d fixture(s) FAILED\\n' \"$nc_failed\" >&2",
    'elif [ "$nc_inconclusive" -gt 0 ]; then',
    "printf 'helm-render negative control: %d fixture(s) INCONCLUSIVE\\n' \"$nc_inconclusive\" >&2",
    "'literal-ingress|1.1 iam|'",
    "'zones-omits-enabled|1.1 iam+gateway|'",
    "'leaked-value|2|1.1 '",
    "'template-only-diff|3a;3a-prime|'",
    "'slug-mirror|1a|'",
    "'security-context|4 security-context iam;4 security-context iam+gateway|'",
    'case "$1" in 0) echo 0 ;; 3) echo 1 ;; *) echo 2 ;; esac',
    'case "$1" in 0) echo 0 ;; 1) echo 1 ;; *) echo 2 ;; esac',
    'mod_rc=0; mapped_run_module "$PY" "$HERE/helm_render.py" --chart "$CHART" || mod_rc=$?',
    "all_rc=0; run_chart_scripts || all_rc=$?",
    "real_rc=0; real_run || real_rc=$?",
    'exit "$real_rc"',
    'for s in "$CHART"/tests/*.sh; do',
    "CHART_SCRIPT_FLOOR=6",
    'if [ "${#scripts[@]}" -lt "$CHART_SCRIPT_FLOOR" ]; then',
    'cs_rc=0; run_chart_script "$s" || cs_rc=$?',
    '"$BASH" "$1" --set ingress.host=console.example.test || raw=$?',
)


def read_input(path, label):
```

(e) `check_self_invocation`. Replace:

```python
    workflow_credentials_sh_text, release_plan_sh_text, ruff_sh_text,
    next_public_free_sh_text,
):
    """Call sites of the affected-graph, actionlint, release-parity, workflow-credentials,
    release-plan, ruff and next-public-free gates missing from where they must appear.
```

with:

```python
    workflow_credentials_sh_text, release_plan_sh_text, ruff_sh_text,
    next_public_free_sh_text, helm_render_sh_text,
):
    """Call sites of the affected-graph, actionlint, release-parity, workflow-credentials,
    release-plan, ruff, next-public-free and helm-render gates missing from where they must appear.
```

Replace:

```python
    `release_plan_sh_text`, `ruff_sh_text` and `next_public_free_sh_text` are REQUIRED positional parameters, deliberately.
```

with:

```python
    `release_plan_sh_text`, `ruff_sh_text`, `next_public_free_sh_text` and `helm_render_sh_text` are REQUIRED positional parameters, deliberately.
```

Replace:

```python
        f"ci/next-public/run.sh: {site}"
        for site in NEXT_PUBLIC_FREE_SH_CALL_SITES
        if site not in next_public_free_lines
    )
    return missing
```

with:

```python
        f"ci/next-public/run.sh: {site}"
        for site in NEXT_PUBLIC_FREE_SH_CALL_SITES
        if site not in next_public_free_lines
    )
    # SMA-513 PR 2b — stripped whole lines, like the ruff haystack and for the same two reasons.
    helm_render_lines = {line.strip() for line in helm_render_sh_text.splitlines()}
    missing.extend(
        f"ci/helm-render/run.sh: {site}"
        for site in HELM_RENDER_SH_CALL_SITES
        if site not in helm_render_lines
    )
    return missing
```

(f) The self-test's floor fixture. Obligation 6 has a second site the spec does not name: `check_floor(tasks_fixture)` must stay empty, so the fixture must hold every floor member (measured: without this edit the self-test reports `check_floor: fired on a fixture containing every floor member`). Replace:

```python
                 # SMA-502 — a floor member too, for the same reason.
                 "next-public-free": True},
```

with:

```python
                 # SMA-502 — a floor member too, for the same reason.
                 "next-public-free": True,
                 # SMA-513 PR 2b — a floor member too, for the same reason.
                 "helm-render": True},
```

and replace:

```python
                 "workflow-credentials", "ruff-ci", "next-public-free"]
```

with:

```python
                 "workflow-credentials", "ruff-ci", "next-public-free", "helm-render"]
```

(g) The wired fixture. Replace:

```python
    wired_next_public_free = "".join(
        f"    {site}\n" for site in NEXT_PUBLIC_FREE_SH_CALL_SITES
    )
```

with:

```python
    wired_next_public_free = "".join(
        f"    {site}\n" for site in NEXT_PUBLIC_FREE_SH_CALL_SITES
    )
    # SMA-513 PR 2b — the same shape for ci/helm-render/run.sh, derived from the registry.
    wired_helm_render = "".join(
        f"    {site}\n" for site in HELM_RENDER_SH_CALL_SITES
    )
```

(h) The REQUIRED-parameter introspection. Replace:

```python
        "release_plan_sh_text", "ruff_sh_text", "next_public_free_sh_text",
    ):
```

with:

```python
        "release_plan_sh_text", "ruff_sh_text", "next_public_free_sh_text",
        "helm_render_sh_text",
    ):
```

(i) The helm-render battery: a deletion row per pinned line, a contamination row, a commented-out row and a disabled-guard row (the shape of `ci_targets.py:3132-3230`). Insert immediately BEFORE the line `    # _scripts (SMA-553 D10) — a second pure extractor, so _eligibility's shape is untouched.`:

```python
    # SMA-513 PR 2b — the next-public-free battery above, repeated for the helm-render haystack:
    # a deletion row per pinned line, a contamination row, a commented-out row and a
    # disabled-guard row.
    for _hr_site in HELM_RENDER_SH_CALL_SITES:
        _hr_broken = "".join(
            line for line in wired_helm_render.splitlines(keepends=True)
            if line.strip() != _hr_site
        )
        if not check_self_invocation(
            wired, scripts, wired_actionlint, wired_release_parity, wired_workflow_credentials,
            wired_release_plan, wired_ruff, wired_next_public_free, _hr_broken,
        ):
            failures.append(
                f"check_self_invocation: missed {_hr_site!r} deleted from ci/helm-render/run.sh"
            )
    # Contamination: a helm-render site must not be satisfiable from another haystack.
    if not check_self_invocation(
        wired + wired_helm_render, scripts, wired_actionlint, wired_release_parity,
        wired_workflow_credentials, wired_release_plan, wired_ruff, wired_next_public_free,
        "".join(line for line in wired_helm_render.splitlines(keepends=True)
                if line.strip() != HELM_RENDER_SH_CALL_SITES[0]),
    ):
        failures.append(
            "check_self_invocation: a helm-render site was satisfied by run.sh text"
        )
    # Whole-LINE, not substring: a commented-out copy of a pinned line must report missing.
    _hr_commented = wired_helm_render.replace(
        'if [ "$NEGATIVE" = 1 ]; then\n', '# if [ "$NEGATIVE" = 1 ]; then\n'
    )
    if not check_self_invocation(
        wired, scripts, wired_actionlint, wired_release_parity, wired_workflow_credentials,
        wired_release_plan, wired_ruff, wired_next_public_free, _hr_commented,
    ):
        failures.append(
            "check_self_invocation: a COMMENTED-OUT helm-render line satisfied the pin "
            "(widened to substring matching)"
        )
    # The disabled guard. A mutated chart that makes the module exit 0 must be FAILED; neutering
    # that comparison to something always-false leaves every OTHER pinned line byte-identical, so
    # the control would report OK for a fixture the module passed.
    _hr_guard_neutered = wired_helm_render.replace(
        'elif [ "$fx_rc" != 3 ]; then\n', 'elif [ "$fx_rc" != "$fx_rc" ]; then\n'
    )
    if not check_self_invocation(
        wired, scripts, wired_actionlint, wired_release_parity, wired_workflow_credentials,
        wired_release_plan, wired_ruff, wired_next_public_free, _hr_guard_neutered,
    ):
        failures.append(
            "check_self_invocation: a NEUTERED helm-render comparison (always-false guard) "
            "satisfied the pin"
        )

```

(j) The source keys. Replace:

```python
_CALL_SITE_SOURCE_KEYS = (
    "run", "actionlint", "release_parity", "workflow_credentials", "release_plan", "ruff",
    "next_public_free",
)
```

with:

```python
_CALL_SITE_SOURCE_KEYS = (
    "run", "actionlint", "release_parity", "workflow_credentials", "release_plan", "ruff",
    "next_public_free", "helm_render",
)
```

(k) `main()`. Replace:

```python
        next_public_free_sh = read_input(
            root / "ci" / "next-public" / "run.sh", "ci/next-public/run.sh"
        )
```

with:

```python
        next_public_free_sh = read_input(
            root / "ci" / "next-public" / "run.sh", "ci/next-public/run.sh"
        )
        helm_render_sh = read_input(
            root / "ci" / "helm-render" / "run.sh", "ci/helm-render/run.sh"
        )
```

and replace:

```python
                "next_public_free": next_public_free_sh,
            },
```

with:

```python
                "next_public_free": next_public_free_sh,
                "helm_render": helm_render_sh,
            },
```

Step 4 already changed the `collect_findings` call to pass `sh["helm_render"]`. `EXPECTED_FINDING_KEYS` does not change: this PR adds no key to `collect_findings`.

- [ ] **Step 6: Obligation 7 — reachability**

In `moon.yml`, in `repo:affected-smoke`'s `inputs`, insert after line 248 (`      - 'ci/next-public/**/*'`):

```yaml
      # SMA-513 PR 2b — this task pins load-bearing lines inside ci/helm-render/run.sh
      # (HELM_RENDER_SH_CALL_SITES), so a change under ci/helm-render/ MUST re-key it. The broad
      # 'ci/**/*' entry already covers it for SCHEDULING; the narrow entry is kept because check
      # 8e in ci/actionlint/run.sh floors this array's length and the file's own policy is to keep
      # narrow globs rather than collapse them into the broad one.
      - 'ci/helm-render/**/*'
```

In `ci/actionlint/run.sh`, insert after line 2159 (`  'ci/next-public/**/*'`), before `  'CLAUDE.md'`:

```bash
  # SMA-513 PR 2b — floors the input that makes HELM_RENDER_SH_CALL_SITES reachable. Without it, a
  # PR editing ci/helm-render/** does not schedule repo:affected-smoke, and neither the
  # SELF_SCHEDULED_GATES nor the SELF_TASK_EXPECTED_GLOBS pin for that gate can fire.
  'ci/helm-render/**/*'
```

The floor at `ci/actionlint/run.sh:5774` is `-ge 20`; the array grows from 25 to 26 entries, so the floor needs no edit.

- [ ] **Step 7: Run and see it pass**

Run each, in order:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 ci/affected-graph/ci_targets.py --self-test; echo "rc=$?"
python3 ci/affected-graph/ci_targets.py; echo "rc=$?"
python3 ci/affected-graph/task_inputs.py; echo "rc=$?"
uv run --locked --project py ruff check --config py/pyproject.toml -- ci/affected-graph/ci_targets.py ci/helm-render/helm_render.py
```

Expected: `ci-targets self-test OK` and `rc=0`; a `PASS  ci-targets …` line and `rc=0`; `repo:input-liveness` passes (every new input matches a tracked file); `All checks passed!`.

Then prove exactly-once and the real-text deletion sweep. Write `$SCRATCH/sites.py`:

```python
import pathlib
import sys

WT = pathlib.Path("/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-513-helm-render-gate")
sys.path.insert(0, str(WT / "ci/affected-graph"))
import ci_targets as c  # noqa: E402

hr = (WT / "ci/helm-render/run.sh").read_text()
lines = [line.strip() for line in hr.splitlines()]
bad = [s for s in c.HELM_RENDER_SH_CALL_SITES if lines.count(s) != 1]
print("sites:", len(c.HELM_RENDER_SH_CALL_SITES), "not exactly once:", bad)
unseen = []
for site in c.HELM_RENDER_SH_CALL_SITES:
    cut = "".join(line for line in hr.splitlines(keepends=True) if line.strip() != site)
    got = c.check_self_invocation("", {}, "", "", "", "", "", "", cut)
    if f"ci/helm-render/run.sh: {site}" not in got:
        unseen.append(site)
print("deletions not reported:", unseen)
```

Run: `python3 $SCRATCH/sites.py`. Expected (measured): `sites: 35 not exactly once: []` and `deletions not reported: []`.

- [ ] **Step 8: Run the affected-graph guard and the actionlint gate under their bashes**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
/bin/bash ci/affected-graph/run.sh --negative-control; echo "rc=$?"
/bin/bash ci/affected-graph/run.sh; echo "rc=$?"
/opt/homebrew/bin/bash ci/actionlint/run.sh; echo "rc=$?"
```

Expected: rc 0 for all three. `repo:affected-smoke` needs system bash 3.2 (root CLAUDE.md). For actionlint, read its preflight line first: it needs `pipe capacity 65536 bytes`. If the preflight reports a 512-byte pipe and exits 2, record that and rely on CI for this gate; it is a host condition, not a finding.

- [ ] **Step 9: Commit**

```bash
git add moon.yml .github/workflows/ci.yml CLAUDE.md ci/affected-graph/ci_targets.py ci/actionlint/run.sh
git commit -m "feat(ci): register repo:helm-render with its seven gate obligations (SMA-513)" -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 9: Obligations 8–10 — the new uv lockfile in osv and Dependabot

**Files:**
- Modify: `ci/osv/run.sh:33-37`
- Modify: `moon.yml` — `repo:osv` `inputs`, after line 49
- Modify: `.github/dependabot.yml` — a new block after line 94

**Interfaces:**
- Consumes: `ci/helm-render/uv.lock` from Task 1.
- Produces: `ci/helm-render/uv.lock` scanned by `repo:osv` and watched by Dependabot.

- [ ] **Step 1: The failing test**

Run: `grep -c 'ci/helm-render/uv.lock' ci/osv/run.sh moon.yml .github/dependabot.yml`. Expected: `0` for each file. The lockfile is in no scan today.

- [ ] **Step 2: Obligation 8 — `LOCKFILES`**

In `ci/osv/run.sh`, replace:

```bash
LOCKFILES=(
  'ts/pnpm-lock.yaml'
  'py/uv.lock'
  'ci/workflow-credentials/uv.lock'
)
```

with:

```bash
LOCKFILES=(
  'ts/pnpm-lock.yaml'
  'py/uv.lock'
  'ci/workflow-credentials/uv.lock'
  # SMA-513 PR 2b — repo:helm-render resolves pyyaml through its own uv project, so its lockfile
  # is a fourth pip-ecosystem manifest. moon.yml's repo:osv inputs carry the same path.
  'ci/helm-render/uv.lock'
)
```

- [ ] **Step 3: Obligation 9 — `repo:osv` inputs**

In `moon.yml`, insert after line 49 (`      - 'ci/workflow-credentials/uv.lock'`):

```yaml
      # SMA-513 PR 2b — the same for repo:helm-render's own uv project; it is in
      # ci/osv/run.sh's LOCKFILES array too. This entry re-keys the gate when that lockfile moves.
      - 'ci/helm-render/uv.lock'
```

- [ ] **Step 4: Obligation 10 — Dependabot**

In `.github/dependabot.yml`, insert after line 94 (the last `- patch` of the `/ci/workflow-credentials` block), before the blank line and `# ---- GitHub Actions: workflows at repo root ----`:

```yaml

  # ---- Python: standalone uv project for repo:helm-render (SMA-513 PR 2b) ----
  # A THIRD uv entry, for the same reason as the /ci/workflow-credentials entry above: this
  # gate's project sits outside the py/ workspace on purpose, so without this entry its pinned
  # pyyaml is in a lockfile that nothing watches. Settings mirror that block exactly.
  - package-ecosystem: uv
    directory: /ci/helm-render
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

- [ ] **Step 5: Run and see it pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
bash ci/osv/run.sh; echo "rc=$?"
python3 -c 'import yaml; d=yaml.safe_load(open(".github/dependabot.yml")); print([u["directory"] for u in d["updates"] if u["package-ecosystem"]=="uv"])'
python3 ci/affected-graph/task_inputs.py; echo "rc=$?"
```

Expected: an `osv gate: ci/helm-render/uv.lock … packages scanned` line with a count of at least 1, and `rc=0` (osv-scanner needs network access); `['/py', '/ci/workflow-credentials', '/ci/helm-render']`; input-liveness `rc=0`.

- [ ] **Step 6: Commit**

```bash
git add ci/osv/run.sh moon.yml .github/dependabot.yml
git commit -m "build(ci): scan and watch the helm-render uv lockfile (SMA-513)" -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 10: The READMEs, the wiring proofs and full verification

**Files:**
- Create: `ci/helm-render/README.md`
- Modify: `charts/paigasus/README.md:112-126`

**Interfaces:**
- Consumes: the measurements of Task 5 Step 6, Task 6 Step 3, Task 7 Steps 3, 4 and 6, and Task 8 Step 7.
- Produces: the documentation spec § 3 and § 8 require, and the verification record.

- [ ] **Step 1: Write `ci/helm-render/README.md`**

Use the numbers you measured. The values below are the ones measured on 2026-09-22; replace any that differ.

````markdown
<!-- SPDX-License-Identifier: Apache-2.0 -->

# `repo:helm-render`

The static gate over `charts/paigasus` (SMA-513 PR 2b). It renders the chart with the pinned
`helm` and asserts properties that `helm lint` and the golden files cannot see. It also runs
every script under `charts/paigasus/tests/`, which nothing else runs in CI.

Spec: `docs/superpowers/specs/2026-09-22-sma-513-pr2b-helm-render-gate-design.md`, which
replaces § 8 of `docs/superpowers/specs/2026-09-19-sma-513-multi-zone-ingress-helm-design.md`.

## Modes

| Command | Does |
| -- | -- |
| `bash ci/helm-render/run.sh` | Checks 1–4 (`helm_render.py --chart charts/paigasus`), then every `charts/paigasus/tests/*.sh` (checks 5 and 6) |
| `bash ci/helm-render/run.sh --self-test` | `helm_render.py --self-test` (in-process rows over inline YAML and proto text), then the wrapper's rc rows |
| `bash ci/helm-render/run.sh --negative-control` | Each fixture under `fixtures/` must make the module exit 3 with its own named row failed |

`moon run repo:helm-render` runs all three, the two controls first, under `set -euo pipefail`.

## Checks

Every render is `helm template paigasus <chart> --kube-version 1.31.0` with the stub values in
`helm_render.py`'s `STUB_VALUES`. The valid subsets are `iam` and `iam+gateway`.

| Row | Check | It fails when |
| -- | -- | -- |
| `1a` | The slug mirror | `paigasus.serviceSlugs` in `_helpers.tpl` is not EQUAL to the slug set derived from `enum Capability` in `service_info.proto`, or a TypeScript derivation line changed |
| `1.1 <subset>` | Coupling (AC 2) | The Ingress zone ids, the `PAIGASUS_ZONES` keys or the `PAIGASUS_SERVICES` keys are not each EQUAL to the enabled set |
| `1.2 <subset>` | Routing | An Ingress path routes to a Service whose Deployment serves another `PAIGASUS_ZONE` |
| `1.3 <subset>` | Membership | An enabled zone id is not in the proto-derived slug set |
| `2` | No trace of a disabled zone | Any string key or value of the iam-only render, or its raw text, contains `gateway`, the sentinel host or a value set under `zones.gateway` (case-insensitive) |
| `3a`, `3a-prime`, `3b`, `3c` | Independent rollout (AC 5) | `spec.template` of a Deployment differs, stays equal or survives against the table in spec § 5 check 3 |
| `4 iam-http <subset>` | Ports agree | The IAM `http` containerPort, `IAM_HTTP_ADDR`, the Service's `http` port and target, and `PAIGASUS_SERVICES.iam` disagree |
| `4 iam-grpc <subset>` | Ports agree | The IAM `grpc` containerPort, `IAM_GRPC_ADDR`, the Service's `grpc` port and target, and `PAIGASUS_IAM_GRPC_URL` disagree |
| `4 console-port <subset>` | Ports agree | A console's containerPort, `PORT` and its Service's resolved target port disagree |
| `4 security-context <subset>` | Security context | A pod lacks `runAsUser: 65532`, `runAsGroup: 65532`, `runAsNonRoot: true`; a container lacks `allowPrivilegeEscalation: false` or sets an identity key to another value; either level carries `readOnlyRootFilesystem` |
| `6 <script>` | Every chart script | A `charts/paigasus/tests/*.sh` exits non-zero. `render.sh` is check 5 (lint and golden files) |

**Why equality for the slug mirror (spec A2).** The safety property is "chart ⊆ proto".
Equality is stronger on purpose: the contracts PR that registers the first capability of a new
service must also edit `charts/`. That is the only moment anyone thinks about a new zone.

**The `readOnlyRootFilesystem` pin.** `docs/ops/RUNBOOK-containers.md` § 7 says that posture is
untested for every image in this repo. A hardening PR must change this row and that runbook
section together.

## Exit codes

| Code | Meaning |
| -- | -- |
| 0 | pass |
| 1 | an assertion failed (the module's 3, or a chart script's 1) |
| 2 | infrastructure error (a failed render, an unparseable source, a missing tool, a chart script's 127 or 141, fewer than six chart scripts) |

`helm_render.py` exits 3, not 1, for an assertion, because a Python traceback exits 1. Do not
"normalize" it. Every row runs after a failure; the gate's rc is the worst one seen.

## The negative control

Each fixture holds only the files it changes. `run.sh` builds the fixture chart as
`cp -R charts/paigasus <tmp>/paigasus`, then `cp -R <fixture>/. <tmp>/paigasus/`. Per fixture,
the module must exit 3 with the named row failed. Other rows may also fail. Measured one fixture
at a time on 2026-09-22:

| Fixture | Mutation | Named row | Must stay green | Rows that failed |
| -- | -- | -- | -- | -- |
| `literal-ingress` | both Ingress rules written literally | `1.1 iam` | — | `1.1 iam`, `1.2 iam`, `2` |
| `zones-omits-enabled` | `PAIGASUS_ZONES` skips an enabled `gateway` | `1.1 iam+gateway` | — | `1.1 iam+gateway` |
| `leaked-value` | a `console-env` entry carries `zones.gateway.backend.url` | `2` | `1.1 …` | `2` |
| `template-only-diff` | every console reads `zones.iam`'s image tag | `3a`, `3a-prime` | — | `3a`, `3a-prime` |
| `slug-mirror` | `paigasus.serviceSlugs` lists `billing` too | `1a` | — | `1a` |
| `security-context` | the console pod drops `runAsNonRoot` | `4 security-context iam`, `4 security-context iam+gateway` | — | the two named rows |

`literal-ingress` also fails `1.2 iam` and `2` by construction: a literal `/gateway` rule both
names the disabled zone and routes to a Service that does not exist.

Verdicts: the expected result is `OK`; rc 0, or rc 3 without a named row, is
`negative-control FAILED`; any other rc is `negative-control INCONCLUSIVE: infrastructure error
(rc=N)`. Any FAILED gives 1, else any INCONCLUSIVE gives 2, else 0. Measured: deleting check 2's
body makes `leaked-value` report FAILED (rc 1); a table row with no fixture directory reports
INCONCLUSIVE (rc 2).

**Refreshing a stale fixture.** A fixture is a whole-file copy of one live template. A template
refactor can make it stale, and the control then reports FAILED or INCONCLUSIVE. Rebuild it from
the live file: copy the file, then re-apply the one mutation in the table above.

## Delete-the-feature record (spec § 8)

For the rows no fixture covers, each assertion body was removed in turn and the module
self-test went red (rc 3): check 1.2 routing (2 rows red), check 3 case b (1), check 3 case c
(3), `4 iam-http` (3), `4 iam-grpc` (2), `4 console-port` (3). The wrapper's rc maps were broken
in turn (3 to 3, any to 1, any to 0) and `--self-test` exited 1 each time. Deleting any one of
the 35 `HELM_RENDER_SH_CALL_SITES` lines from `run.sh` makes `ci_targets.py` report it.

## Tool resolution

- `helm` resolves once through `proto --reporter text bin helm` from the repo root, and must be
  an executable file whose `helm version --short` starts with `v` plus the `.prototools` pin.
  No fallback: the golden files are a byte pin of that helm.
- Python resolves once through `uv run --locked --project ci/helm-render`, and must be a file
  under `ci/helm-render/.venv`. After that no `uv` runs.
- The module and the chart scripts run with the venv's `bin` and the helm directory first on
  `PATH`. Chart scripts run as `"$BASH" <script>`, so the bash that runs `run.sh` runs them too.

## Read-only rule

The gate writes only under one `mktemp -d` directory, removed by a `trap`. `TMPDIR` and the
`HELM_*_HOME` variables point into it, so check 3 case b's `Chart.yaml` copy lands there. The one
exception is `ci/helm-render/.venv`, which `.gitignore` ignores. The gate never writes into
`charts/`, never calls `render.sh --update`, and never runs `uv` inside a fixture directory.
Nothing in the repository enforces this rule; review does.

## Residual risks (spec § 10)

1. Checks 1 and 2 exist twice. `charts/paigasus/tests/ingress.sh` keeps its weaker copy.
2. Check 1a reads the proto text, not the generated TypeScript. The text pin on the two
   derivation lines catches a change to those lines, not a change elsewhere (for example to
   `PREFIX` in `capability.ts`).
3. Overlay fixtures copy whole files, so a template refactor can make one stale (above).
4. `STUB_VALUES` is a seventh copy of the required values. A new required value must go into all
   seven; a missing one makes every render fail, which is rc 2.
5. `maps.sh` reads only part of its input in a pipe under `pipefail` (`maps.sh:37-46`). On a
   host whose new pipe holds 512 bytes, its `printf` can get SIGPIPE (141), which this gate
   reports as rc 2. A Linux runner's 64 KiB pipe holds the whole render. Not measured. This PR
   does not change the chart scripts.

## Running it locally

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
bash ci/helm-render/run.sh --self-test
bash ci/helm-render/run.sh --negative-control
bash ci/helm-render/run.sh
```

It runs under `/bin/bash` 3.2.57 and under bash 5: it uses no `mapfile`, no `declare -A`, no
here-string and no pipe into an early-exit reader. Measured 2026-09-22 under both, with a host
pipe capacity of 65536 bytes: about 2 s per mode.
````

- [ ] **Step 2: Update `charts/paigasus/README.md`**

Replace lines 112-126 (the "Running the tests" section) with:

````markdown
## Running the tests

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
export PROTO_REPORTER=text
charts/paigasus/tests/refusals.sh --set ingress.host=console.example.test
charts/paigasus/tests/maps.sh
charts/paigasus/tests/ingress.sh
charts/paigasus/tests/render.sh
charts/paigasus/tests/names.sh
charts/paigasus/tests/env.sh
```

`refusals.sh` needs `--set ingress.host=…` from its caller, since its own valid-render rows have
no default host; it holds the other seven REQUIRED values valid by default in its own `FIXED`
array, so each refusal row can set only the one value it names to `""`. `maps.sh`, `ingress.sh`,
`render.sh`, `names.sh` and `env.sh` set every REQUIRED value themselves, `ingress.host` included.

`repo:helm-render` runs all six in CI, each with `--set ingress.host=console.example.test`, and
adds checks the scripts do not make. See `ci/helm-render/README.md`.
````

- [ ] **Step 3: Delete-the-feature proof for the real-run wiring**

A deleted chart-script loop must red `repo:affected-smoke`. Write `$SCRATCH/dtf_wiring.py`:

```python
# Delete one real-run line from run.sh; ci_targets.py must report it (rc 1), then restore.
import pathlib
import subprocess

WT = pathlib.Path("/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-513-helm-render-gate")
R = WT / "ci/helm-render/run.sh"
orig = R.read_text()
line = '    cs_rc=0; run_chart_script "$s" || cs_rc=$?\n'
try:
    assert orig.count(line) == 1
    R.write_text(orig.replace(line, ""))
    p = subprocess.run(["python3", str(WT / "ci/affected-graph/ci_targets.py")], capture_output=True, text=True, check=False, cwd=WT)
    print("rc:", p.returncode)
    print("\n".join(x for x in p.stderr.splitlines() if "helm-render" in x))
finally:
    R.write_text(orig)
print("restored:", R.read_text() == orig)
```

Run it with the proto PATH exported. Expected: `rc: 1`, a row `ci/helm-render/run.sh: cs_rc=0; run_chart_script "$s" || cs_rc=$?`, `restored: True`.

- [ ] **Step 4: Verify all three modes under both bashes**

Run:

```bash
/bin/bash $SCRATCH/modes.sh /bin/bash --self-test --negative-control real
/bin/bash $SCRATCH/modes.sh /opt/homebrew/bin/bash --self-test --negative-control real
```

Expected: `rc=0` six times. If the Homebrew bash run hangs, measure the pipe capacity (root CLAUDE.md, SMA-612) and record it; do not read a hang as a gate failure. Then `git status --porcelain` prints only the two README changes, and `git status --porcelain --ignored ci/helm-render charts` lists nothing under `charts/` and only `ci/helm-render/.venv/` as ignored.

- [ ] **Step 5: Confirm the chart is unchanged**

Run: `git diff --stat origin/main -- charts/paigasus/templates charts/paigasus/tests`. Expected: no output.

- [ ] **Step 6: `moon run` and the full gate graph**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:helm-render
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site :http-extractor-envelope :input-liveness :promtool :observability-drift :nats-permissions :release-parity :release-parity-py :release-parity-ts :publish-metadata :version-lockstep :workflow-credentials :pyo3-stub-drift :ruff-ci :next-public-free :helm-render :test-e2e --base origin/main --include-relations
```

This target list must equal the command in the root `CLAUDE.md` ci-targets block after Task 8.

Expected: `repo:helm-render` passes. Apply the root CLAUDE.md bash split to the full graph: Moon resolves `bash` through `PATH` (Homebrew 5.3.15 on this Mac), so re-run the gates that need the other bash directly and read those results instead: `/bin/bash ci/affected-graph/run.sh --negative-control && /bin/bash ci/affected-graph/run.sh` for `repo:affected-smoke`; `/opt/homebrew/bin/bash ci/ruff/run.sh`, `ci/next-public/run.sh` and `ci/publish-metadata/run.sh` for the bash-4+ gates; `repo:actionlint` only when its pipe preflight passes. If a task fails, capture its Moon state directory before any re-run (root CLAUDE.md diagnosis procedure, step 0).

- [ ] **Step 7: Commit**

```bash
git add ci/helm-render/README.md charts/paigasus/README.md
git commit -m "docs(ci): document repo:helm-render and name all six chart scripts (SMA-513)" -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

Then run `git log --oneline origin/main..HEAD` and confirm ten commits after the spec commit `cfa271aa`, with no amend.

---

## Spec deviations and clarifications

Each item gives the evidence. None changes a decision in the spec. Items 1–4 add work the spec does not name; items 5–10 state how the plan reads a spec sentence; items 11–13 correct a citation.

1. **Obligation 6 has a second site.** `ci_targets.py`'s self-test holds `tasks_fixture` and `aligned_t` (2159-2175), and asserts that `check_floor(tasks_fixture)` is empty. With only `REQUIRED_REPO_TASKS` edited, the self-test measured `check_floor: fired on a fixture containing every floor member`. Task 8 Step 5 (f) adds `helm-render` to both.
2. **Obligation 5 plumbing has two more sites than spec § 7 lists.** Besides `read_input` in `main()`, the ninth parameter and the self-test rows, the `sh` dict key needs `"helm_render"` in `_CALL_SITE_SOURCE_KEYS` (3653-3656, used by the `EXPECTED_FINDING_KEYS` self-test at 3267-3268), and the REQUIRED-parameter introspection loop (2952-2955) must name `helm_render_sh_text`. The ninth argument goes into 59 calls (measured with `ast`); Task 8 Step 4 does it with a script.
3. **More pinned lines than obligation 5 lists.** The plan pins 35 lines. Beyond the spec's list it pins the three `exit` lines, the six `FIXTURE_TABLE` rows and the two rc maps, so a dropped fixture or a rewritten map cannot pass in silence.
4. **Check 2 also searches the raw render text.** Spec § 5 says "walk every string key and every string value". A parsed walk cannot see a YAML comment, yet spec § 5 justifies the check by `ingress.yaml`'s per-zone comments. The plan adds a raw-text search (a superset). Measured: the iam-only raw render has 0 hits.
5. **"Each required to occur exactly once in run.sh" (obligation 5).** `check_self_invocation` tests whole-line membership, like every other haystack. The plan verifies exactly-once at authoring time (Task 8 Step 7, `sites.py`), as the existing tuples' comments record. The gate itself does not count occurrences.
6. **Check 4 "the console Service's target port".** The rendered `targetPort` is the NAME `http`, not a number (`console-service.yaml:19`). The plan resolves the name through the container's ports. It also adds each IAM Service's resolved `targetPort` to the two IAM rows.
7. **Negative-control named rows.** The plan names subset rows exactly: `zones-omits-enabled` names `1.1 iam+gateway` (the only subset where `gateway` is enabled), `security-context` names both subsets' rows, and `leaked-value`'s stay-green entry is the prefix `1.1 ` (both subsets).
8. **"Fails ONLY its named row" is not true for `literal-ingress`.** Measured: it also fails `1.2 iam` and `2`, by construction. Spec § 4 allows other failing rows; the README records them.
9. **The `.prototools` helm pin parse.** `.prototools` has two `helm = "…"` lines: the version and the `[plugins]` `file://` URL. The first measured run of a naive `sed` parse read both and exited 2. `resolve_helm` accepts digits and dots only.
10. **`pyyaml==6.0.3`, not the model's range.** Spec § 3 says "same layout as `ci/workflow-credentials/`", whose `pyproject.toml` pins `pyyaml>=6.0.3,<7`. The plan follows the spec's explicit `==6.0.3`. The locked `pyyaml` section is byte-identical to the model's.
11. Spec § 4 cites `ci/release-parity/run.sh:66-75` for the report arms. They are at 68-76.
12. Spec § 5 check 1a cites `capability.ts:21-27`. The function is at 22-28; the pinned `return` line is 27.
13. Spec § 4 cites `release-plz.sh:43-56`; correct as measured. `moon.yml:916`, `ci/osv/run.sh:33-37`, `.github/dependabot.yml:73-94`, `.gitignore:32` and `ci_targets.py:1691-1695` are also correct.

---

## Self-Review

**Spec coverage.**

| Spec § | Requirement | Task |
| -- | -- | -- |
| 1, 2 A1 | Run all six chart scripts unchanged | 6 (`run_chart_scripts`), 10 Step 5 |
| 2 A2 | Slug set EQUAL to the proto set; README states why | 2, 10 |
| 2 A3, § 6 | Overlay fixtures, `cp -R …/.` | 6 (`negative_control`), 7 |
| 2 A4 | One module, `--chart <dir>` | 1–5 |
| 2 A5, § 9 | Obligation 8 moves to PR 3 | not in this plan (out of scope) |
| 3 | Files, including `pyproject.toml` pin and `uv.lock` | 1, 6, 7, 10 |
| 3 | `helm_render.py` ruff-clean | 1–5 Step "Lint", 8 Step 7 |
| 4 Setup | `set -euo pipefail`, `PROTO_REPORTER`, `REPO_ROOT` from `BASH_SOURCE`, helm via proto with `[ -f ]`/`[ -x ]` and version assert, venv once with `--locked`, `PATH` order, `"$BASH" <script>` | 6 |
| 4 Exit codes | 0/3/2 module, 3 to 1, other to 2, chart 1 vs other, no `$( )` around exit 2, worst rc, rows continue | 1, 6 |
| 4 Shell | no `mapfile`/`declare -A`/unguarded arrays/early-exit pipes; bash 3.2 and 5 | 6 Step 4, 10 Step 4 |
| 4 Read-only | `mktemp -d` + `trap`, `.venv` exception, never `render.sh --update` | 6, 10 Step 4 |
| 4 Modes | `--self-test` with wrapper rows (3 to 1, 127 to 2); `--negative-control` rule and total; real run | 6, 7 |
| 5 Renders | `--kube-version 1.31.0`, own stub list, sentinel URL | 1 |
| 5 Check 1 | 1.1 each set equal to E; 1.2 routing; 1.3 membership | 2 |
| 5 Check 1a | enum-block parse, skip UNSPECIFIED, no-dot fails closed, `split(" ")`, TS text pins | 2 |
| 5 Check 2 | iam-only, keys and values, case-insensitive, id + zones.gateway values + sentinel host | 3 |
| 5 Check 3 | cases a, a′, b (temp `Chart.yaml`), c; `spec.template` only; positive "differs" | 4 |
| 5 Check 4 | IAM HTTP, IAM gRPC, console ports; security context exact rule | 5 |
| 5 Checks 5, 6 | `render.sh` without `--update`; glob, sorted, floor 6, one row per script, `--set ingress.host=…` | 6 |
| 6 | Six fixtures; named rows; `leaked-value` keeps 1.1 green; `template-only-diff` reads `zones.iam`; README records measurements | 7, 10 |
| 7 obligations 1–7 | `T`, CLAUDE.md, `SELF_SCHEDULED_GATES`, `SELF_TASK_EXPECTED_GLOBS`, `HELM_RENDER_SH_CALL_SITES` + plumbing + battery, `REQUIRED_REPO_TASKS`, affected-smoke input + floor | 8 |
| 7 obligations 8–10 | `LOCKFILES`, `repo:osv` inputs, Dependabot | 9 |
| 7 Moon task | `toolchain: 'system'`, four-line script, seven inputs, no `deps` | 8 Step 1 |
| 7 | `EXPECTED_FINDING_KEYS` unchanged | 8 Step 5 note |
| 8 | Both bashes, pipe state; fixtures one at a time; delete-the-feature for 1.2, 3b, 3c, three port rows; wrapper rows; `moon run`; full graph | 5 Step 6, 6 Step 3, 7 Steps 4 and 6, 10 Steps 3–6 |
| 9 | No chart template, chart script or golden change | Global Constraints, 10 Step 5 |
| 10 | Residual risks in the README | 10 Step 1 |

**Placeholder scan.** No "TBD", "TODO", "similar to Task N" or "fill in". Every code step holds the full code. The README in Task 10 Step 1 carries measured values and tells the implementer to replace any that differ; that is a verification instruction, not a placeholder.

**Name consistency.** Checked across tasks: `helm_template`, `parse_docs`, `base_paths`, `proto_slugs`, `chart_slugs`, `check1a`, `check1`, `check2`, `compare_templates`, `check3`, `check4`, `run_checks`, `report`, `synthetic`, `_find`, `self_test`, `main`; row ids `1a`, `1.1|1.2|1.3 <subset>`, `2`, `3a`, `3a-prime`, `3b`, `3c`, `4 iam-http|iam-grpc|console-port|security-context <subset>`, `6 <script>`; shell names `resolve_helm`, `resolve_python`, `HELM_BIN`, `PY`, `module_rc`, `script_rc`, `mapped_run_module`, `run_chart_script`, `run_chart_scripts`, `real_run`, `self_test`, `negative_control`, `FIXTURE_TABLE`, `CHART_SCRIPT_FLOOR`, `st_rc`, `nc_rc`, `fx_rc`, `mod_rc`, `all_rc`, `real_rc`, `cs_rc`; registry names `HELM_RENDER_SH_CALL_SITES`, `wired_helm_render`, `helm_render_sh_text`, `helm_render_sh`, `"helm_render"`, `"helm-render"`. The 35 pinned strings in Task 8 Step 5 (d) are copied from Task 6's `run.sh`, and `sites.py` proves each occurs exactly once.
