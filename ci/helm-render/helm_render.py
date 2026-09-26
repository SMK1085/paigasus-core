#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""repo:helm-render — checks 1, 1a, 2, 3, 4, 7 and 8 over `helm template` renders of a chart.

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
import copy
import io
import json
import re
import shutil
import subprocess
import sys
import tempfile
import tomllib
from dataclasses import dataclass
from pathlib import Path
from urllib.parse import urlsplit

import yaml

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

KUBE_VERSION = "1.31.0"
RELEASE = "paigasus"
SENTINEL_URL = "http://gateway-sentinel.example.test:8088"
SENTINEL_HOST = "gateway-sentinel.example.test"

# Row 7 (SMA-513 PR 3 spec § 5.5): the kind job's values files. The kind job is not a required
# check; this row is, so a chart change that breaks those values reds before merge.
KIND_VALUES = REPO_ROOT / "ci" / "kind" / "values"

# Row 8 (SMA-688 D7). ci/images/chains.toml names each chain's GHCR repository and version file
# (spec D10). values.yaml pins one tag on each image block. A version bump that forgets the chart
# would otherwise make a default tag name an image that does not exist: ImagePullBackOff.
CHAINS_TOML = REPO_ROOT / "ci" / "images" / "chains.toml"
# How many distinct images the iam+gateway render holds: the iam console, the iam backend and the
# gateway console. The gateway backend is never rendered (spec § 1).
RENDERED_IMAGES = 3

# The required values, as ONE constant. One of eight copies of the list the seven chart scripts
# hold (spec § 10 risk 4); it differs from theirs only in zones.gateway.backend.url, which is the
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

# Check 3: row -> the required result per pod template, keyed by the `app.kubernetes.io/name`
# label of spec.template. "differs" is a POSITIVE assertion (spec § 5, check 3).
CHECK3_EXPECT = {
    "3a": {"iam-console": "differs", "gateway-console": "equal", "iam-backend": "equal"},
    "3a-prime": {"iam-console": "equal", "gateway-console": "differs", "iam-backend": "equal"},
    "3b": {"iam-console": "differs", "gateway-console": "differs", "iam-backend": "differs"},
    "3c": {"iam-console": "differs", "gateway-console": "absent", "iam-backend": "equal"},
}

# Row 3b (SMA-688 § 7.3). With explicit tags an appVersion bump changes no pod template, so row 3b
# renders BOTH sides of the bump with every image tag cleared. `--set <key>=` sets the empty
# string, and the templates' `default .Chart.AppVersion` then applies (measured, Task 9 Step 1).
CLEARED_TAGS = tuple(
    arg
    for key in ("zones.iam.console.image.tag", "zones.iam.backend.image.tag",
                "zones.gateway.console.image.tag", "zones.gateway.backend.image.tag")
    for arg in ("--set", f"{key}=")
)

# Check 4's pod-level identity (spec § 5, check 4). A container may omit these keys, but may not
# set one to another value.
POD_IDENTITY = {"runAsUser": 65532, "runAsGroup": 65532, "runAsNonRoot": True}

# The row labels a real run_checks() call produces, IN ORDER. This is the floor: a check whose
# production call is deleted (or reordered, or duplicated) makes the produced list diverge from
# this constant, and run_checks() then raises InfraError instead of silently reporting fewer rows
# as "all N rows passed" (F1). Re-baseline this deliberately when a row is genuinely added,
# removed or reordered — never to make a red run_checks() call green again.
EXPECTED_ROW_LABELS = (
    "1a",
    "1.1 iam",
    "1.2 iam",
    "1.3 iam",
    "2",
    "4 iam-http iam",
    "4 iam-grpc iam",
    "4 console-port iam",
    "4 security-context iam",
    "1.1 iam+gateway",
    "1.2 iam+gateway",
    "1.3 iam+gateway",
    "4 iam-http iam+gateway",
    "4 iam-grpc iam+gateway",
    "4 console-port iam+gateway",
    "4 security-context iam+gateway",
    "3a",
    "3a-prime",
    "3b",
    "3c",
    "7 kind-values",
    "8a default-image-tags",
    "8b default-image-render",
)


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
                    # An assertion, not a lookup: the return value is discarded, but a missing
                    # port named after the Ingress backend raises ShapeError, which fails this row.
                    _service_port(svc, _get(p, "backend", "service", "port", "name"))
        return problems

    def membership():
        unknown = sorted(set(enabled) - slugs)
        return [f"enabled zone(s) {unknown} are not in the proto-derived slug set {sorted(slugs)}"] if unknown else []

    return [_row(f"1.1 {label}", coupling), _row(f"1.2 {label}", routing), _row(f"1.3 {label}", membership)]


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
        # This structured walk is redundant with the raw-text search below for DETECTION — the raw
        # render also contains every string this walk visits. It runs first anyway because it
        # names the exact document and field a leak sits in, which is a better message than "the
        # raw render contains X somewhere".
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
    """Copy `chart` to `dest` and bump the copy's Chart.yaml appVersion, for case 3b's restart check."""
    shutil.copytree(chart, dest)
    chart_yaml = Path(dest) / "Chart.yaml"
    text, n = re.subn(r"(?m)^appVersion:.*$", 'appVersion: "0.0.0-helm-render-bump"', chart_yaml.read_text())
    if n != 1:
        raise InfraError(f"expected one appVersion line in {chart}/Chart.yaml, found {n}")
    chart_yaml.write_text(text)
    return Path(dest)


def check3(chart):
    """Rows 3a, 3a-prime, 3b and 3c; EXPECTED_ROW_LABELS is the inventory that floors this set."""
    both = ("gateway", "iam")
    base = parse_docs(helm_template(chart, both))
    rows = []
    for row, key in (("3a", "zones.iam.console.image.tag"), ("3a-prime", "zones.gateway.console.image.tag")):
        before = parse_docs(helm_template(chart, both, ("--set", f"{key}=t1")))
        after = parse_docs(helm_template(chart, both, ("--set", f"{key}=t2")))
        rows.append(compare_templates(row, before, after))
    # Case b edits Chart.yaml in a temp COPY only. run.sh exports TMPDIR, so the copy lands under
    # the gate's own mktemp directory. SMA-688: both sides render with every tag cleared, so the
    # row still proves the appVersion fallback now that values.yaml pins each tag.
    with tempfile.TemporaryDirectory(prefix="helm-render-3b-") as tmp:
        bumped = _bumped_app_version(chart, Path(tmp) / "chart")
        before = parse_docs(helm_template(chart, both, CLEARED_TAGS))
        rows.append(compare_templates("3b", before, parse_docs(helm_template(bumped, both, CLEARED_TAGS))))
    rows.append(compare_templates("3c", base, parse_docs(helm_template(chart, ("iam",)))))
    return rows


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


# --------------------------------------------------------------------------- check 7


def check7(chart, run=subprocess.run, helm=None, values_dir=KIND_VALUES):
    """Row 7: ci/kind/values/a.yaml, and a.yaml plus b.yaml, render against `chart` with rc 0.

    A render that fails is an ASSERTION (the row fails, rc 3): the chart and the kind values
    disagree, which is exactly what this row exists to catch. A missing values file is rc 2.
    `run`, `helm` and `values_dir` are parameters only so self_test() can drive the row without
    helm; production never passes them.
    """
    helm = helm or shutil.which("helm")
    if helm is None:
        raise InfraError("helm is not on PATH; run this module through ci/helm-render/run.sh")
    a, b = Path(values_dir) / "a.yaml", Path(values_dir) / "b.yaml"
    for f in (a, b):
        if not f.is_file():
            raise InfraError(f"the kind values file {f} does not exist")

    def body():
        problems = []
        for label, files in (("a.yaml", (a,)), ("a.yaml + b.yaml", (a, b))):
            cmd = [helm, "template", RELEASE, str(chart), "--kube-version", KUBE_VERSION]
            for f in files:
                cmd += ["-f", str(f)]
            proc = run(cmd, capture_output=True, text=True, check=False)
            if proc.returncode != 0:
                problems.append(f"{label}: helm template exited {proc.returncode}: {proc.stderr.strip()}")
        return problems

    return _row("7 kind-values", body)


# --------------------------------------------------------------------------- check 8


def chain_registry(path=CHAINS_TOML):
    """key -> entry, from the chain registry. An unreadable registry is rc 2."""
    try:
        data = tomllib.loads(Path(path).read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, tomllib.TOMLDecodeError) as exc:
        raise InfraError(f"cannot read the chain registry {path}: {exc}") from exc
    chains = data.get("chain")
    if not isinstance(chains, dict) or not chains:
        raise InfraError(f"{path} has no [chain.<key>] table")
    for key, entry in chains.items():
        if not isinstance(entry, dict) or not all(
                isinstance(entry.get(f), str) and entry.get(f) for f in ("kind", "version_file", "ghcr")):
            raise InfraError(f"{path}: [chain.{key}] needs string kind, version_file and ghcr values")
    return chains


def chain_version(entry, root=REPO_ROOT):
    """The version in a chain's version file: `[package] version` (cargo) or the top-level
    "version" (npm). An unreadable file or a missing version is rc 2: a source file this module
    cannot parse."""
    path = Path(root) / entry["version_file"]
    try:
        text = path.read_text(encoding="utf-8")
        if entry["kind"] == "cargo":
            version = (tomllib.loads(text).get("package") or {}).get("version")
        elif entry["kind"] == "npm":
            doc = json.loads(text)
            version = doc.get("version") if isinstance(doc, dict) else None
        else:
            raise InfraError(f"{path}: unknown chain kind {entry['kind']!r}")
    except (OSError, UnicodeDecodeError, tomllib.TOMLDecodeError, ValueError) as exc:
        raise InfraError(f"cannot read the version in {path}: {exc}") from exc
    if not isinstance(version, str):
        raise InfraError(f"{path} carries no string version")
    return version


def image_blocks(values, path="$"):
    """Every mapping under an `image` key of a values tree: (dotted path, repository, tag)."""
    out = []
    if isinstance(values, dict):
        for key, value in values.items():
            here = f"{path}.{key}"
            if key == "image" and isinstance(value, dict):
                out.append((here, value.get("repository"), value.get("tag")))
            else:
                out += image_blocks(value, here)
    elif isinstance(values, list):
        for i, value in enumerate(values):
            out += image_blocks(value, f"{path}[{i}]")
    return out


def check8a(values, registry, versions):
    """Row 8a: each chain has exactly ONE image block, and its tag equals the chain's version."""

    def body():
        problems = []
        blocks = image_blocks(values)
        known = {entry["ghcr"] for entry in registry.values()}
        for where, repo, _tag in blocks:
            if repo not in known:
                problems.append(f"{where}: repository {repo!r} is named by no chain in ci/images/chains.toml")
        for key, entry in registry.items():
            # Whole-string equality. `repo/iam` is a prefix of `repo/iam-console`.
            mine = [b for b in blocks if b[1] == entry["ghcr"]]
            if len(mine) != 1:
                problems.append(f"chain {key}: {len(mine)} image blocks name {entry['ghcr']}, expected exactly one")
                continue
            where, _repo, tag = mine[0]
            version = versions[key]
            if version == "0.0.0":
                problems.append(f"chain {key}: {entry['version_file']} is at 0.0.0, which is never released, so no image carries that tag")
            elif not tag:
                problems.append(f"{where}.tag is empty. It falls back to the chart appVersion, not to {version}")
            elif tag != version:
                problems.append(f"{where}.tag is {tag!r}, but {entry['version_file']} is at {version!r}. Update the tag in the same PR as the version bump")
        return problems

    return _row("8a default-image-tags", body)


def check8b(values, docs, stub_values=STUB_VALUES):
    """Row 8b: the iam+gateway render holds RENDERED_IMAGES images, and each one equals
    <repository>:<tag> of its values.yaml block. STUB_VALUES must set no image key, or the row
    would read the stub, not the default."""

    def body():
        problems = []
        leaked = [k for k, _v in stub_values if ".image." in f".{k}."]
        if leaked:
            problems.append(f"STUB_VALUES sets {leaked}; row 8b must render the values.yaml defaults")
        tags = {repo: tag for _where, repo, tag in image_blocks(values)}
        images = set()
        for dep in _of_kind(docs, "Deployment"):
            pod = _get(dep, "spec", "template", "spec")
            for container in (pod.get("containers") or []) + (pod.get("initContainers") or []):
                images.add(str(container.get("image")))
        if len(images) != RENDERED_IMAGES:
            problems.append(f"the render holds {len(images)} distinct images {sorted(images)}, expected {RENDERED_IMAGES}")
        for image in sorted(images):
            repo, _sep, tag = image.rpartition(":")
            if repo not in tags:
                problems.append(f"{image}: no values.yaml image block names {repo}")
            elif tag != tags[repo]:
                problems.append(f"{image}: the rendered tag is {tag!r}, but the values.yaml default is {tags[repo]!r}")
        return problems

    return _row("8b default-image-render", body)


# --------------------------------------------------------------------------- run


def _row_inventory_diff(got, want):
    """A human-readable difference between two row-label tuples: missing, extra, or reordered."""
    missing = [r for r in want if r not in got]
    extra = [r for r in got if r not in want]
    parts = []
    if missing:
        parts.append(f"missing {missing}")
    if extra:
        parts.append(f"extra {extra}")
    if not parts and list(got) != list(want):
        parts.append(f"reordered: got {list(got)}, want {list(want)}")
    return "; ".join(parts) if parts else "(no difference)"


def _check_row_inventory(got_labels, want_labels=EXPECTED_ROW_LABELS):
    """Raise InfraError unless `got_labels` equals `want_labels` exactly, in order (F1).

    Without this floor, a deleted check-3 call (or any other production call) makes run_checks()
    return fewer rows, and the gate still prints "all N rows passed" at the smaller N — a green
    run over a silently smaller check set. A mismatch here is an infrastructure error (rc 2), not
    an assertion failure: the gate itself is malformed, not the chart under test.
    """
    got_labels = tuple(got_labels)
    if got_labels == tuple(want_labels):
        return
    diff = _row_inventory_diff(got_labels, tuple(want_labels))
    raise InfraError(f"helm-render row inventory does not match EXPECTED_ROW_LABELS: {diff}")


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
    both_docs = None
    for label, enabled in SUBSETS:
        raw = helm_template(chart, enabled)
        docs = parse_docs(raw)
        rows += check1(label, docs, enabled, paths, slugs)
        if enabled == ("iam",):
            rows.append(check2(docs, raw))
        rows += check4(label, docs)
        if label == "iam+gateway":
            both_docs = docs
    rows += check3(chart)
    rows.append(check7(chart))
    # Row 8 (SMA-688 D7): the chart's default image tags against the chain registry.
    if both_docs is None:
        raise InfraError("SUBSETS holds no iam+gateway render; row 8b needs it")
    values = _chart_values(chart)
    registry = chain_registry()
    rows.append(check8a(values, registry, {key: chain_version(entry) for key, entry in registry.items()}))
    rows.append(check8b(values, both_docs))
    _check_row_inventory([r.row for r in rows])
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

    # ---- row 8 (SMA-688 D7): the default image tags track the image versions
    reg = {
        "iam": {"kind": "cargo", "version_file": "rs/x/Cargo.toml", "ghcr": "repo/iam"},
        "iam-console": {"kind": "npm", "version_file": "ts/x/package.json", "ghcr": "repo/iam-console"},
        "gateway-console": {"kind": "npm", "version_file": "ts/y/package.json", "ghcr": "repo/gateway-console"},
    }
    vers = {"iam": "0.1.0", "iam-console": "0.1.0", "gateway-console": "0.2.0"}

    def vals(iam="0.1.0", iam_console="0.1.0", gateway_console="0.2.0", extra=None):
        v = {"zones": {
            "iam": {"console": {"image": {"repository": "repo/iam-console", "tag": iam_console}},
                    "backend": {"image": {"repository": "repo/iam", "tag": iam}}},
            "gateway": {"console": {"image": {"repository": "repo/gateway-console", "tag": gateway_console}}},
        }}
        if extra:
            v["zones"]["gateway"]["backend"] = {"image": {"repository": extra, "tag": "0.1.0"}}
        return v

    r8a, r8b = ("8a default-image-tags",), ("8b default-image-render",)
    expect("check8a good", [check8a(vals(), reg, vers)], passing=r8a)
    expect("check8a a wrong tag", [check8a(vals(iam_console="0.0.9"), reg, vers)], fail=r8a)
    expect("check8a an empty tag", [check8a(vals(iam=""), reg, vers)], fail=r8a)
    # The tag EQUALS the version here, so only the 0.0.0 rule can red this row.
    expect("check8a a 0.0.0 version", [check8a(vals(iam="0.0.0"), reg, {**vers, "iam": "0.0.0"})], fail=r8a)
    expect("check8a an unknown repository", [check8a(vals(extra="repo/billing"), reg, vers)], fail=r8a)
    expect("check8a a chain with no image block",
           [check8a(vals(), {**reg, "gateway": {"kind": "cargo", "version_file": "g", "ghcr": "repo/gateway"}},
                    {**vers, "gateway": "0.1.0"})], fail=r8a)
    # `repo/iam` is a string prefix of `repo/iam-console`. A second block for the SAME repository
    # must red; a prefix match would also count iam-console's block as iam's.
    expect("check8a two image blocks for one chain", [check8a(vals(extra="repo/iam"), reg, vers)], fail=r8a)
    rendered = synthetic(both, tags={"iam": "0.1.0", "gateway": "0.2.0"}, backend_tag="0.1.0")
    expect("check8b good", [check8b(vals(), rendered)], passing=r8b)
    expect("check8b a rendered image with the wrong tag",
           [check8b(vals(), synthetic(both, tags={"iam": "0.0.0", "gateway": "0.2.0"}, backend_tag="0.1.0"))], fail=r8b)
    expect("check8b STUB_VALUES sets an image key",
           [check8b(vals(), rendered, stub_values=(*STUB_VALUES, ("zones.iam.console.image.tag", "x")))], fail=r8b)
    expect("check8b only two images rendered",
           [check8b(vals(), synthetic(iam_only, tags={"iam": "0.1.0"}, backend_tag="0.1.0"))], fail=r8b)
    with tempfile.TemporaryDirectory(prefix="helm-render-8-") as tmp:
        root = Path(tmp)
        (root / "Cargo.toml").write_text('[package]\nname = "x"\nversion = "0.3.0"\n')
        (root / "package.json").write_text('{"name": "x", "version": "0.4.0"}\n')
        (root / "bad.json").write_text('{"name": "x"}\n')
        (root / "chains.toml").write_text("[other]\nx = 1\n")
        if chain_version({"kind": "cargo", "version_file": "Cargo.toml"}, root) != "0.3.0":
            failures.append("chain_version: the cargo reader did not return 0.3.0")
        if chain_version({"kind": "npm", "version_file": "package.json"}, root) != "0.4.0":
            failures.append("chain_version: the npm reader did not return 0.4.0")
        expect_infra("chain_version: a package.json with no version",
                     lambda: chain_version({"kind": "npm", "version_file": "bad.json"}, root))
        expect_infra("chain_version: a missing version file",
                     lambda: chain_version({"kind": "cargo", "version_file": "none.toml"}, root))
        expect_infra("chain_registry: no [chain] table", lambda: chain_registry(root / "chains.toml"))

    # ---- the exit-code contract and the parser's infrastructure errors
    if report([Row("x", True)], io.StringIO()) != 0 or report([Row("x", True), Row("y", False, "bad")], io.StringIO()) != 3:
        failures.append("report: the exit code does not follow the rows (want 0 and 3)")
    expect_infra("parse_docs invalid YAML", lambda: parse_docs("a: [\n"))
    expect_infra("parse_docs a non-mapping document", lambda: parse_docs("- a\n- b\n"))

    # ---- row 7 (kind values): the row follows helm's exit status; a missing file is rc 2
    class _Proc:
        def __init__(self, rc):
            self.returncode, self.stderr = rc, "stub stderr"

    with tempfile.TemporaryDirectory(prefix="helm-render-7-") as tmp:
        (Path(tmp) / "a.yaml").write_text("{}\n")
        (Path(tmp) / "b.yaml").write_text("{}\n")
        calls = []

        def ok_run(cmd, **_kw):
            calls.append(cmd)
            return _Proc(0)

        expect("check7 both renders exit 0", [check7(Path(tmp), run=ok_run, helm="helm-stub", values_dir=tmp)], passing=("7 kind-values",))
        if [c.count("-f") for c in calls] != [1, 2]:
            failures.append(f"check7: expected a render with a.yaml and one with a.yaml + b.yaml, got {calls}")
        expect("check7 a.yaml fails to render", [check7(Path(tmp), run=lambda cmd, **_kw: _Proc(1), helm="helm-stub", values_dir=tmp)], fail=("7 kind-values",))
        expect(
            "check7 only the overlay fails to render",
            [check7(Path(tmp), run=lambda cmd, **_kw: _Proc(1 if cmd.count("-f") == 2 else 0), helm="helm-stub", values_dir=tmp)],
            fail=("7 kind-values",),
        )
        (Path(tmp) / "b.yaml").unlink()
        expect_infra("check7 a missing b.yaml raises InfraError", lambda: check7(Path(tmp), run=ok_run, helm="helm-stub", values_dir=tmp))

    # ---- row inventory floor (F1): EXPECTED_ROW_LABELS' own arity and content, plus
    # _check_row_inventory's behaviour on a missing, an extra and a reordered row.
    if len(EXPECTED_ROW_LABELS) != 23:
        failures.append(f"EXPECTED_ROW_LABELS: expected 23 labels, got {len(EXPECTED_ROW_LABELS)}")
    if len(set(EXPECTED_ROW_LABELS)) != len(EXPECTED_ROW_LABELS):
        failures.append("EXPECTED_ROW_LABELS: contains a duplicate label")
    _check_row_inventory(EXPECTED_ROW_LABELS)  # the constant against itself: must not raise
    expect_infra("row inventory: a truncated row list raises InfraError", lambda: _check_row_inventory(EXPECTED_ROW_LABELS[:-2]))
    expect_infra("row inventory: an extra row raises InfraError", lambda: _check_row_inventory((*EXPECTED_ROW_LABELS, "extra")))
    expect_infra(
        "row inventory: a reordered row list raises InfraError",
        lambda: _check_row_inventory((EXPECTED_ROW_LABELS[1], EXPECTED_ROW_LABELS[0], *EXPECTED_ROW_LABELS[2:])),
    )

    for f in failures:
        print(f"  FAIL {f}")
    if failures:
        print(f"helm_render.py self-test: {len(failures)} row(s) failed")
        return 3
    print("== helm_render.py self-test passed ==")
    return 0


def main(argv=None):
    parser = argparse.ArgumentParser(description="repo:helm-render checks 1, 1a, 2, 3, 4, 7 and 8")
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
