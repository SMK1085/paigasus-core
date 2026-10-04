# SPDX-License-Identifier: Apache-2.0
"""Keep the 'no shipped package reaches it' reason of an osv waiver true (SMA-733).

osv-scanner applies an [[IgnoredVulns]] entry to an advisory ID for EVERY path in the lock
file. Some waivers in osv-scanner.toml are justified only because no shipped package reaches
the vulnerable package. This guard walks ts/pnpm-lock.yaml and fails when a shipped importer
reaches a package that SHIPPED_FREE_WAIVERS names. It uses only the Python standard library,
because the `osv advisory scan` job has no uv, node or pnpm (spec F10).

A shipped importer is an importer whose key starts with `apps/`, plus every importer that such
an importer reaches through a `link:` entry. Its roots are its dependencies,
optionalDependencies AND devDependencies: ts/Dockerfile installs dev dependencies, and a
bundler can inline anything that app code imports. The root importer `.` is never shipped.

Exit codes are DELIBERATELY not the repo's usual 0/1/2:
  0  every table entry is reached only from non-shipped importers
  2  infrastructure: the inputs are wrong or the walk cannot be trusted (reason token in the
     first output line)
  3  a shipped importer reaches a waived package (token `shipped-path`)
An uncaught traceback exits 1. ci/osv/run.sh maps 3 -> finding and EVERY other code -> 2, so
a crash can never read as a finding or as a pass.

usage: shipped_reachability.py [--lockfile PATH] [--config PATH] | --self-test
Spec: docs/superpowers/specs/2026-10-04-sma-733-braces-osv-waiver-design.md
"""

from __future__ import annotations

import argparse
import collections
import posixpath
import re
import sys
from pathlib import Path

RC_OK = 0
RC_INFRA = 2
RC_SHIPPED = 3

# Each osv waiver whose reason says "no shipped package reaches it", mapped to the npm package
# it waives. osv-scanner.toml's header requires an entry here for every such waiver.
SHIPPED_FREE_WAIVERS = {"GHSA-vfj7-8cjw-p6xm": "braces"}

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]
FIXTURES = HERE / "fixtures"
DEFAULT_FIXTURE_CONFIG = "waiver-config.toml"

IMPORTER_KINDS = ("dependencies", "optionalDependencies", "devDependencies")
SNAPSHOT_EDGE_KINDS = ("dependencies", "optionalDependencies")

# The exact fixture set --self-test must find. Deleting or adding a fixture reds the self-test
# until this tuple changes, so a lost fixture cannot make the self-test pass vacuously.
EXPECTED_FIXTURES = (
    "bad-version.yaml",
    "block-end.yaml",
    "catalogs.yaml",
    "clean.yaml",
    "dangling.yaml",
    "file-dep.yaml",
    "no-edges.yaml",
    "shipped-alias.yaml",
    "shipped-dev.yaml",
    "shipped-link.yaml",
    "shipped-optional.yaml",
    "shipped-prod.yaml",
    "table-stale.yaml",
)


class GuardError(Exception):
    """A check failed. `rc` is the exit code, `token` the reason token."""

    def __init__(self, rc: int, token: str, lines: list[str]) -> None:
        super().__init__(token)
        self.rc = rc
        self.token = token
        self.lines = lines


def infra(token: str, *detail: str) -> GuardError:
    return GuardError(RC_INFRA, token, [f"osv shipped-guard: FAIL {token}: {detail[0]}", *detail[1:]])


def unquote(text: str) -> str:
    """Remove ONE pair of matching ' or " quotes."""
    if len(text) >= 2 and text[0] == text[-1] and text[0] in "'\"":
        return text[1:-1]
    return text


def split_entry(body: str) -> tuple[str, str | None]:
    """Split `key: value` or `key:` (indent already removed). The value is None for `key:`."""
    if body[:1] in "'\"":
        end = body.find(body[0], 1)
        if end > 0 and body[end + 1 : end + 2] == ":":
            rest = body[end + 2 :].strip()
            return body[1:end], (unquote(rest) if rest else None)
    if body.endswith(":"):
        return body[:-1], None
    key, sep, value = body.partition(": ")
    if not sep:
        return body, None
    return unquote(key.strip()), unquote(value.strip())


def parse_lockfile(text: str) -> tuple[str | None, dict, dict]:
    """Return (lockfileVersion, importers, snapshots) from a pnpm 9.0 lock file.

    importers: {importer: {kind: {name: version}}}
    snapshots: {snapshot key: [(name, version), ...]}
    Every other top-level section (settings, catalogs, overrides, packages) is skipped.
    """
    version = None
    importers: dict[str, dict[str, dict[str, str | None]]] = {}
    snapshots: dict[str, list[tuple[str, str | None]]] = {}
    section = None
    importer = kind = dep = snap = None
    in_edges = False
    for raw in text.splitlines():
        body = raw.strip()
        if not body or body.startswith("#"):
            continue
        indent = len(raw) - len(raw.lstrip(" "))
        key, value = split_entry(body)
        if indent == 0:
            section = key
            importer = kind = dep = snap = None
            in_edges = False
            if key == "lockfileVersion":
                version = value
            continue
        if section == "importers":
            if indent == 2:
                importer, kind, dep = key, None, None
                importers[importer] = {k: {} for k in IMPORTER_KINDS}
            elif indent == 4:
                kind, dep = (key if key in IMPORTER_KINDS else None), None
            elif indent == 6 and kind is not None:
                dep = key
                importers[importer][kind][dep] = None
            elif indent == 8 and kind is not None and dep is not None and key == "version":
                importers[importer][kind][dep] = value
        elif section == "snapshots":
            if indent == 2:
                snap, in_edges = key, False
                snapshots[snap] = []
            elif indent == 4:
                # Only a dependencies/optionalDependencies BLOCK gives edges. Any other key at
                # indent 4 (transitivePeerDependencies:, optional: true) ends the edge block.
                in_edges = key in SNAPSHOT_EDGE_KINDS and value is None
            elif indent == 6 and in_edges:
                snapshots[snap].append((key, value))
    return version, importers, snapshots


def node_for(importer: str, name: str, value: str | None) -> tuple[str, str]:
    """Map one `name: version` entry to a node: ('importer', path) or ('snapshot', key)."""
    if value is None:
        return ("snapshot", f"{name}@<no version>")
    if value.startswith("link:"):
        return ("importer", posixpath.normpath(posixpath.join(importer, value[len("link:") :])))
    if value.startswith("file:"):
        return ("snapshot", f"{name}@{value}")
    head = value.split("(", 1)[0]
    if "@" in head[1:]:
        return ("snapshot", value)  # an alias: `name: realname@version`
    return ("snapshot", f"{name}@{value}")


def package_name(snapshot_key: str) -> str:
    head = snapshot_key.split("(", 1)[0]
    at = head.rfind("@")
    return head[:at] if at > 0 else head


def build_graph(importers: dict, snapshots: dict) -> dict:
    """Return {node: [child node, ...]}. Raise `dangling-edge` for any edge to a missing node."""
    graph: dict[tuple[str, str], list[tuple[str, str]]] = {}
    for imp, kinds in importers.items():
        children = []
        for kind in IMPORTER_KINDS:
            for name, value in kinds[kind].items():
                children.append(node_for(imp, name, value))
        graph[("importer", imp)] = children
    for key, edges in snapshots.items():
        graph[("snapshot", key)] = [node_for(".", name, value) for name, value in edges]
    for node, children in graph.items():
        for child in children:
            if child not in graph:
                raise infra(
                    "dangling-edge",
                    f"{label(node)} has an edge to {child[0]} '{child[1]}', which the lock file does not define",
                    "  The parser and the lock file disagree; a walk over it cannot be trusted.",
                )
    return graph


def label(node: tuple[str, str]) -> str:
    return node[1]


def walk(graph: dict, starts: list) -> dict:
    """Breadth-first walk. Return {reached node: parent node or None}."""
    parent: dict[tuple[str, str], tuple[str, str] | None] = {}
    queue = collections.deque()
    for start in starts:
        if start not in parent:
            parent[start] = None
            queue.append(start)
    while queue:
        node = queue.popleft()
        # .get: build_graph already refused a dangling edge. If that check is ever lost, the
        # walk stops at the missing node, and dangling.yaml reds as a plain mismatch.
        for child in graph.get(node, []):
            if child not in parent:
                parent[child] = node
                queue.append(child)
    return parent


def path_to(parent: dict, node: tuple[str, str]) -> list[str]:
    out = []
    cur = node
    while cur is not None:
        out.append(label(cur))
        cur = parent[cur]
    return list(reversed(out))


def check(lockfile: Path, config: Path) -> tuple[int, list[str]]:
    """Run every check. Return (exit code, output lines). The first line holds the token."""
    try:
        return RC_OK, _check(lockfile, config)
    except GuardError as exc:
        return exc.rc, exc.lines


def _check(lockfile: Path, config: Path) -> list[str]:
    if not lockfile.is_file():
        raise infra("no-lockfile", f"'{lockfile}' does not exist")
    if not config.is_file():
        raise infra("no-config", f"'{config}' does not exist")
    version, importers, snapshots = parse_lockfile(lockfile.read_text(encoding="utf-8"))
    if version != "9.0":
        raise infra("lockfile-version", f"lockfileVersion is {version!r}, not '9.0'; update the parser in {Path(__file__).name}")
    if not importers or not snapshots:
        raise infra("empty-section", f"parsed {len(importers)} importers and {len(snapshots)} snapshots; both must be non-zero")
    apps = sorted(i for i in importers if i.startswith("apps/"))
    if not apps:
        raise infra("no-shipped-importer", "no importer key starts with 'apps/'")
    graph = build_graph(importers, snapshots)
    config_text = config.read_text(encoding="utf-8")
    for ghsa in SHIPPED_FREE_WAIVERS:
        if not re.search(r'^[ \t]*id[ \t]*=[ \t]*"' + re.escape(ghsa) + r'"[ \t]*$', config_text, re.MULTILINE):
            raise infra(
                "table-stale",
                f"SHIPPED_FREE_WAIVERS names {ghsa}, but '{config}' has no `id = \"{ghsa}\"` line",
                f"  Remove {ghsa} from SHIPPED_FREE_WAIVERS in {Path(__file__).name}, or restore the waiver.",
            )

    shipped = walk(graph, [("importer", a) for a in apps])
    control = walk(graph, [("importer", i) for i in sorted(importers)])

    for ghsa, package in SHIPPED_FREE_WAIVERS.items():
        # Edge control: some snapshot the control walk reached must have an edge to the package,
        # so the package is reached through a path of at least two edges. A direct root alone
        # does not prove that the walk follows snapshot edges, quoted keys and peer suffixes.
        via = sorted(
            node
            for node in control
            if node[0] == "snapshot" and any(c[0] == "snapshot" and package_name(c[1]) == package for c in graph.get(node, []))
        )
        if not via:
            raise infra(
                "edge-control",
                f"the control walk reaches no snapshot with an edge to '{package}' ({ghsa})",
                "  Either the walk no longer follows snapshot edges (a parser regression), or the package",
                f"  left the lock file. In the second case the waiver is stale: remove {ghsa} from",
                "  osv-scanner.toml and from SHIPPED_FREE_WAIVERS.",
            )

    for ghsa, package in SHIPPED_FREE_WAIVERS.items():
        hits = sorted(n for n in shipped if n[0] == "snapshot" and package_name(n[1]) == package)
        if hits:
            lines = [f"osv shipped-guard: FAIL shipped-path: a shipped importer reaches '{package}' ({ghsa})"]
            lines += ["  " + " > ".join(path_to(shipped, hit)) for hit in hits]
            lines += [
                f"  The waiver for {ghsa} in osv-scanner.toml says no shipped package reaches '{package}'.",
                "  Remove the waiver and fix the path, or rewrite the waiver's reason.",
            ]
            raise GuardError(RC_SHIPPED, "shipped-path", lines)

    return [
        f"osv shipped-guard: {package} ({ghsa}) reached only from dev tooling, not from a shipped importer"
        for ghsa, package in SHIPPED_FREE_WAIVERS.items()
    ]


def fixture_expectation(path: Path) -> tuple[int, str | None, Path]:
    """Read `# expect: <rc> [token]` and the optional `# config: <file>` header lines."""
    expect = None
    config = FIXTURES / DEFAULT_FIXTURE_CONFIG
    for line in path.read_text(encoding="utf-8").splitlines():
        if line.startswith("# expect:"):
            expect = line[len("# expect:") :].split()
        elif line.startswith("# config:"):
            config = FIXTURES / line[len("# config:") :].strip()
    if not expect:
        raise SystemExit(f"osv shipped-guard self-test: {path.name} has no '# expect:' line")
    return int(expect[0]), (expect[1] if len(expect) > 1 else None), config


def self_test() -> int:
    found = tuple(sorted(p.name for p in FIXTURES.glob("*.yaml")))
    failures = []
    if found != EXPECTED_FIXTURES:
        failures.append(f"fixture set differs: found {list(found)}, expected {list(EXPECTED_FIXTURES)}")
    for name in found:
        want_rc, want_token, config = fixture_expectation(FIXTURES / name)
        try:
            got_rc, lines = check(FIXTURES / name, config)
        except Exception as exc:  # name the fixture; never let one crash hide the others
            got_rc, lines = 1, [f"{type(exc).__name__}: {exc}"]
        first = lines[0] if lines else ""
        token_ok = want_token is None or f"FAIL {want_token}:" in first
        if got_rc != want_rc or not token_ok:
            failures.append(f"{name}: expected rc {want_rc} {want_token or ''}, got rc {got_rc}: {first}")
    if failures:
        for failure in failures:
            print(f"  FAIL {failure}", file=sys.stderr)
        print(f"osv shipped-guard self-test: {len(failures)} failure(s)", file=sys.stderr)
        return RC_INFRA
    print(f"osv shipped-guard self-test: {len(found)} fixtures passed")
    return RC_OK


def main(argv: list[str]) -> int:
    if sys.version_info < (3, 9):  # noqa: UP036 - the scan job's python3 is not pinned (spec F10)
        print("osv shipped-guard: FAIL python-too-old: Python 3.9 or later is required", file=sys.stderr)
        return RC_INFRA
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--lockfile", default=str(REPO / "ts" / "pnpm-lock.yaml"))
    parser.add_argument("--config", default=str(REPO / "osv-scanner.toml"))
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args(argv[1:])
    if args.self_test:
        return self_test()
    rc, lines = check(Path(args.lockfile), Path(args.config))
    for line in lines:
        print(line, file=sys.stdout if rc == RC_OK else sys.stderr)
    return rc


if __name__ == "__main__":
    sys.exit(main(sys.argv))
