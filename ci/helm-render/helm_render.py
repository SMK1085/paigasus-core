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
import re
import shutil
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

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


# --------------------------------------------------------------------------- run


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
        if enabled == ("iam",):
            rows.append(check2(docs, raw))
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
