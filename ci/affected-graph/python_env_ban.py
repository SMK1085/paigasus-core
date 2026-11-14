# SPDX-License-Identifier: Apache-2.0
# SMA-770 -- the python-env token ban (the `a14` key of cargo_moon_parity.py) and the behaviour
# self-test of the ci/python-env wrapper. Spec: docs/superpowers/specs/2026-11-14-sma-770-python-env-
# wrapper-token-ban-design.md, sections 5.3 to 5.5.
#
# The ban. Only ci/python-env/run.sh may run the Python package manager. So every tracked file,
# except the EXCLUSIONS, is scanned line by line for the TOKENS, and each hit is a row unless the
# line is in PINNED_LINES. The gate reads no shell text: it does not parse a command. Rules:
#   G-ban        a token on a scanned line is a row (`ban_rows`);
#   G-wrapper    the sha256 of the wrapper equals WRAPPER_SHA256 (`wrapper_rows`);
#   G-floor      the command of FLOOR_TASK holds FLOOR_TEXT (`floor_rows`);
#   G-toolchain  .moon/toolchains.yml sets no syncArgs, venvArgs or installArgs (`toolchain_rows`);
#   TOML keys    no pyproject.toml or uv.toml sets a key that reinstalls in place (`uv_config_rows`);
#   G4           a file that is not UTF-8 raises, unless git marks it `binary`;
#   G5           every file the ban reads matches an input of repo:affected-smoke.
# `check_python_env` runs all of them. cargo_moon_parity.py passes in its `_read_utf8` and
# `smoke_unreached`, so this module does not import that file (no import cycle).
#
# `wrapper_behaviour_failures` copies `ci/python-env/run.sh` into a temporary root, puts fake tools
# on PATH, runs the wrapper once per case, and compares the working directory and the arguments
# that the fake tool logged. Each failure is the string `python-env wrapper, <name>: <what differs>`.
#
# The bash binary comes from the PYTHON_ENV_TEST_BASH environment variable (default /bin/bash), so a
# re-run under Homebrew bash is real.
from __future__ import annotations

import hashlib
import os
import re
import shutil
import subprocess
import tempfile
import tomllib
from pathlib import Path
from typing import TYPE_CHECKING, NamedTuple

if TYPE_CHECKING:
    from collections.abc import Callable

WRAPPER_PATH = "ci/python-env/run.sh"


class MoonOutputError(RuntimeError):
    """The ban cannot read its input: a tracked file is missing on disk, a TOML file does not parse,
    or `git check-attr` failed. cargo_moon_parity.py lists this class in INFRA_ERRORS, so main()
    exits 2, the same as for its own MoonOutputError. A file that is not UTF-8 raises the error of
    the `read_utf8` that the caller passes in (cargo_moon_parity.MoonOutputError).
    """


# The token boundary of the spec (5.3): no word character, `.` or `-` on either side. `.venv` has
# only `(?<![\w-])` on its left, so a path part (`py/.venv`) matches and `a.venv` does not.
_L = r"(?<![\w.-])"
_R = r"(?![\w.-])"
TOKENS: tuple[tuple[str, re.Pattern[str]], ...] = (
    ("uv", re.compile(_L + r"uv" + _R)),
    ("uvx", re.compile(_L + r"uvx" + _R)),
    ("pip", re.compile(_L + r"pip" + _R)),
    ("pip3", re.compile(_L + r"pip3" + _R)),
    ("pip3.<n>", re.compile(_L + r"pip3\.[0-9]+" + _R)),
    ("ensurepip", re.compile(_L + r"ensurepip" + _R)),
    ("virtualenv", re.compile(_L + r"virtualenv" + _R)),
    (".venv", re.compile(r"(?<![\w-])\.venv" + _R)),
    ("UV_*", re.compile(_L + r"UV_[A-Z0-9_]+" + _R)),
    ("VIRTUAL_ENV", re.compile(_L + r"VIRTUAL_ENV" + _R)),
    ("--active", re.compile(_L + r"--active" + _R)),
)
# Known non-matches (spec 5.3). The last one is a false green that the spec accepts: prose.
NON_MATCHES: tuple[str, ...] = ("uv.lock", "uv.toml", "uv-cache", "pip.exe", "pipefail", "pipeline", "uvicorn", "the uv.")

EXEMPT_PATHS = ("ci/affected-graph/python_env_ban.py", "ci/python-env/run.sh")
_LOCKFILES = ("uv.lock", "Cargo.lock", "pnpm-lock.yaml")


def _base(path: str) -> str:
    return path.rsplit("/", 1)[-1]


# (name, predicate(path, binary paths)). A path that one predicate accepts is not read.
EXCLUSIONS: tuple[tuple[str, Callable[[str, frozenset[str]], bool]], ...] = (
    ("markdown", lambda p, _b: p.endswith(".md")),
    ("docs", lambda p, _b: p.startswith("docs/")),
    ("uv.lock", lambda p, _b: _base(p) == "uv.lock"),
    ("Cargo.lock", lambda p, _b: _base(p) == "Cargo.lock"),
    ("pnpm-lock.yaml", lambda p, _b: _base(p) == "pnpm-lock.yaml"),
    # G4: git marks the file `binary` in .gitattributes. Such a path is skipped before it is read.
    ("binary", lambda p, b: p in b),
    # Dependabot's ecosystem names (`package-ecosystem: uv`); the file runs nothing.
    ("dependabot", lambda p, _b: p == ".github/dependabot.yml"),
    ("exempt", lambda p, _b: p in EXEMPT_PATHS),
)

# G-wrapper: `shasum -a 256 ci/python-env/run.sh`. A wrapper change needs a change here in the same diff.
WRAPPER_SHA256 = "d96bd6cd5dc4ba2261fc070952fa5833a04fc35db803101a0606e99ea3d8c082"
# G-floor: the SMA-765 kernel task must run the isolated reinstall through the wrapper.
FLOOR_TASK = "paigasus-kernel-py:test"
FLOOR_TEXT = "ci/python-env/run.sh kernel-test"
MOON_TOOLCHAINS = ".moon/toolchains.yml"
SMOKE_PROJECT = "repo"
SMOKE_TASK = "affected-smoke"

# (path, exact stripped line) -> the reason the text cannot change (spec 5.4). The count must stay
# lower than 35. Tasks 4 to 6 add the entries.
PINNED_LINES: dict[tuple[str, str], str] = {
    (".gitignore", ".venv/"): "ignore pattern: the name of the project environment directory that git must ignore",
    (".moon/toolchains.yml", "packageManager: 'uv'"): "Moon toolchain schema value: the schema accepts only the tool name here",
    (".moon/workspace.yml", "- '**/.venv/**'"): "Moon hasher ignore glob: it names the project environment directory",
    (".prototools", 'uv = "0.11.16"'): "proto tool version pin: proto reads the tool name as the key",
    (
        "py/pyproject.toml",
        'exclude = ["**/__pycache__", "**/node_modules", "**/.venv", "**/dist", "**/build", "**/generated/**"]',
    ): "tool exclude glob: it names the project environment directory",
    (
        "py/scripts/assert_typecheck_coverage.py",
        'EXCLUDE_PARTS = frozenset({"generated", "__pycache__", "node_modules", ".venv", "dist", "build"})',
    ): "path parts to skip: one part is the name of the project environment directory",
    (
        "ci/actionlint/release_guard.py",
        'r"uv\\s+publish",',
    ): "detection data: the regex that finds a package-manager publish in a workflow cannot name the tool in other words",
    (
        "ci/actionlint/release_guard.py",
        '"jobs:\\n  build:\\n    steps: [{run: uv publish --trusted-publishing always}]\\n"),',
    ): "fixture data: a workflow with a package-manager publish step, which the release guard must refuse",
    (
        "ci/actionlint/release_guard.py",
        '("uvx", {"run": "uvx foo"}, True),',
    ): "fixture data: the command name is the input of the row that tests the allow list of run commands",
    (
        "ci/images/console-selftest.sh",
        """printf '%s\\n' '#!/usr/bin/env bash' 'echo "packages=1"' 'exit 0' > "$T/rs-bin/uv\"""",
    ): "fake tool: the stub must have the real tool name so that the body under test finds it on PATH",
    (
        "ci/images/console-selftest.sh",
        'chmod +x "$T/rs-work/ci/images/run.sh" "$T/rs-bin/syft" "$T/rs-bin/uv"',
    ): "fake tool: makes the stub of the real tool name executable",
    (
        "ci/helm-render/run.sh",
        '"$HERE/.venv/"*) ;;',
    ): "case guard: the interpreter must lie under the environment directory that the project creates, which has this name",
    (
        "ci/wasm-lockstep/run.sh",
        '"$HERE/.venv/"*) ;;',
    ): "case guard: the interpreter must lie under the environment directory that the project creates, which has this name",
    (
        "ci/version-lockstep/run.sh",
        'uvws = ws.get("tool", {}).get("uv", {}).get("workspace")',
    ): "pyproject table key: the code reads the tool table whose key is the tool name",
    (
        "ci/version-lockstep/run.sh",
        'sources = doc.get("tool", {}).get("uv", {}).get("sources", {})',
    ): "pyproject table key: the code reads the tool table whose key is the tool name",
}

# TEMPORARY (SMA-770 Tasks 3 to 7). The files that still hold hits that a later task migrates
# (plan Appendix A.6). A hit in a listed file is not a row; a listed file with no hit is a row, so
# the list shrinks as the migration moves. Tasks 4 to 6 empty it, and Task 7 deletes it.
MIGRATION_FILES: frozenset[str] = frozenset((
    "ci/affected-graph/cargo_moon_parity.py",
))

_BAN_FIX = "route the command through ci/python-env/run.sh, give the text new words, or pin the line in PINNED_LINES"


def token_hits(line: str) -> list[str]:
    """The names of the TOKENS on `line`, in TOKENS order."""
    return [name for name, pattern in TOKENS if pattern.search(line)]


def _excluded_by(path: str, binary: frozenset[str]) -> str | None:
    for name, pred in EXCLUSIONS:
        if pred(path, binary):
            return name
    return None


def binary_paths(root: Path, paths: list[str]) -> frozenset[str]:
    """The `paths` that git marks `binary` (`git check-attr -z binary --stdin`), read in `root`."""
    if not paths:
        return frozenset()
    # Imported here, not at the top: `python3 -I python_env_ban.py` (the standalone wrapper harness of
    # ci/python-env/README.md) has no script directory on sys.path. cargo_moon_parity.py has it.
    import task_inputs

    env = {k: v for k, v in os.environ.items() if k not in task_inputs._GIT_LOCAL_ENV}
    # The path list goes in through a temporary FILE, not a pipe. With `input=`, Python writes into
    # git's stdin while git writes its answers into stdout. On a host whose new pipe holds 512 bytes
    # (SMA-612) both writes blocked: measured, the real run hung at 0.01 s of CPU in git.
    with tempfile.TemporaryFile() as paths_file:
        paths_file.write("".join(f"{p}\0" for p in paths).encode("utf-8"))
        paths_file.seek(0)
        proc = subprocess.run(
            ["git", "-c", "core.quotePath=false", "check-attr", "-z", "binary", "--stdin"],
            cwd=root, stdin=paths_file, capture_output=True, env=env, check=False,
        )
    if proc.returncode != 0:
        raise MoonOutputError(
            f"`git check-attr binary` failed with rc {proc.returncode}: {proc.stderr.decode('utf-8', 'replace').strip()}"
        )
    return _parse_check_attr(proc.stdout)


def _parse_check_attr(stdout: bytes) -> frozenset[str]:
    """The paths whose `binary` value is `set` in the `git check-attr -z` output `stdout`."""
    try:
        text = stdout.decode("utf-8")
    except UnicodeDecodeError as exc:
        raise MoonOutputError(f"`git check-attr -z binary` printed output that is not UTF-8: {exc}") from exc
    parts = text.split("\0")
    if len(parts) % 3 != 1 or parts[-1]:
        raise MoonOutputError("`git check-attr -z binary` printed output that is not NUL-separated triples")
    return frozenset(path for path, _attr, value in zip(parts[0:-1:3], parts[1:-1:3], parts[2:-1:3], strict=True) if value == "set")


def ban_rows(
    root: Path,
    tracked,
    *,
    read_text: Callable[[Path], str],
    binary: frozenset[str],
    pinned: dict[tuple[str, str], str] = PINNED_LINES,
    migration: frozenset[str] = MIGRATION_FILES,
) -> tuple[list[str], set[str]]:
    """G-ban rows for the `tracked` paths, and the set of paths that were read.

    A line is split on `\\n`; its text for a pin is `line.rstrip("\\r").strip()`. A tracked path that
    is missing on disk raises MoonOutputError; it is never skipped. `read_text` raises on a file that
    is not UTF-8, so only a path in `binary` (skipped before the read) escapes G4.
    """
    root = Path(root)
    rows: list[str] = []
    reads: set[str] = set()
    pin_hits: dict[tuple[str, str], list[int]] = {key: [] for key in pinned}
    pinned_paths = {path for path, _ in pinned}
    hit_files: set[str] = set()
    for key, reason in sorted(pinned.items()):
        if not str(reason).strip():
            rows.append(f"PINNED_LINES entry {key[0]}: {key[1]!r} has an empty reason")
    for path in sorted(tracked):
        if _excluded_by(path, binary) is not None:
            continue
        full = root / path
        if not full.is_file():
            raise MoonOutputError(f"{path}: tracked by git but not a file on disk - the ban cannot read it")
        reads.add(path)
        text = read_text(full)
        for lineno, raw in enumerate(text.split("\n"), 1):
            names = token_hits(raw)
            if not names:
                continue
            stripped = raw.rstrip("\r").strip()
            # A pin counts only on a line with a token, so a pinned line that lost its token is stale.
            if path in pinned_paths and (path, stripped) in pin_hits:
                pin_hits[(path, stripped)].append(lineno)
                continue
            hit_files.add(path)
            if path in migration:
                continue
            rows.append(f"{path}:{lineno}: {', '.join(names)} in `{stripped}` - {_BAN_FIX}")
    for key, linenos in sorted(pin_hits.items()):
        if not linenos:
            rows.append(f"PINNED_LINES entry {key[0]}: {key[1]!r} matches no line of a scanned file (stale)")
        elif len(linenos) > 1:
            rows.append(
                f"PINNED_LINES entry {key[0]}: {key[1]!r} matches {len(linenos)} lines "
                f"({', '.join(map(str, linenos))}), so it is ambiguous; give one copy new words"
            )
    for path in sorted(migration - hit_files):
        rows.append(f"MIGRATION_FILES: {path} holds no hit any more (stale migration entry); remove it")
    return rows, reads


def wrapper_rows(root: Path, expected: str = WRAPPER_SHA256) -> list[str]:
    """G-wrapper: the sha256 of the wrapper must equal `expected`. The row prints the real hash."""
    path = Path(root) / WRAPPER_PATH
    if not path.is_file():
        return [f"FLOOR: {WRAPPER_PATH} is missing, so no command can run the Python package manager"]
    real = hashlib.sha256(path.read_bytes()).hexdigest()
    if real == expected:
        return []
    return [
        f"{WRAPPER_PATH} has sha256 {real}, but WRAPPER_SHA256 in ci/affected-graph/python_env_ban.py "
        f"is {expected}. Review the wrapper change and its self-test rows, then set the new hash"
    ]


def floor_rows(projects, task: str = FLOOR_TASK, text: str = FLOOR_TEXT) -> list[str]:
    """G-floor: the resolved command of `task` must contain `text` (a substring check)."""
    pid, _, name = task.partition(":")
    blob = ((projects.get(pid) or {}).get("invocations") or {}).get(name)
    if not blob:
        return [f"FLOOR: {task} is missing or has no command, so the floor asserts nothing"]
    if text not in blob:
        return [f"{task}: its command does not contain `{text}`, so the SMA-765 isolated bindings reinstall is gone"]
    return []


# The Moon Python toolchains append these to their own environment sync (InstallDependencies). Any
# value is a row. The key anywhere outside a `#` comment, quoted or not, so a flow mapping and a
# quoted key are rows too. (A copy of SMA-765's MOON_UV_ARGS_RE; Task 7 deletes the old one.)
MOON_ARGS_RE = re.compile(r"""(?<![\w-])["']?(syncArgs|venvArgs|installArgs)["']?[ \t]*:""")


def _yaml_code(line: str) -> str:
    """`line` without its YAML comment. A comment starts at a `#` that begins the line or follows
    white space, OUTSIDE a quote: a `#` inside `' … '` or `" … "` is part of the value. In a
    double-quoted value `\\` escapes the next character. A single-quoted `''` closes and opens
    the quote again, which gives the same state, so it needs no own branch.
    """
    quote, i = None, 0
    while i < len(line):
        c = line[i]
        if quote == "'":
            if c == "'":
                quote = None
        elif quote == '"':
            if c == "\\":
                i += 2
                continue
            if c == '"':
                quote = None
        elif c in "'\"":
            quote = c
        elif c == "#" and (i == 0 or line[i - 1] in " \t"):
            return line[:i]
        i += 1
    return line


def toolchain_rows(rel_path: str, text: str) -> list[str]:
    """G-toolchain rows for an argument that Moon appends to its own environment sync (SMA-765 R-toolchain)."""
    rows = []
    for lineno, line in enumerate(text.split("\n"), 1):
        code = _yaml_code(line)
        rows.extend(
            f"{rel_path}:{lineno} sets {m.group(1)}, which Moon appends to the environment sync of "
            f"InstallDependencies; the ban approves no argument there (SMA-765 R-toolchain)"
            for m in MOON_ARGS_RE.finditer(code)
        )
    return rows


# R7 (SMA-765 follow-up sweep 2): settings that make a plain project run reinstall, upgrade or
# rebuild a package in place, in `[tool.uv]` (and `[tool.uv.pip]`) of a pyproject.toml and at the
# top level (and `[pip]`) of a uv.toml. The floor: the list must hold the py workspace root.
UV_CONFIG_MUTATING_KEYS = ("reinstall", "reinstall-package", "upgrade", "upgrade-package", "cache-keys")
REQUIRED_UV_CONFIG = "py/pyproject.toml"


def uv_configs_of(tracked) -> list[str]:
    """The tracked `pyproject.toml` and `uv.toml` paths in `tracked`, sorted (R7)."""
    return sorted(p for p in tracked if p.rsplit("/", 1)[-1] in ("pyproject.toml", "uv.toml"))


def uv_config_rows(rel_path: str, text: str) -> list[str]:
    """Rows for the mutating keys in one pyproject.toml or uv.toml (R7)."""
    try:
        doc = tomllib.loads(text)
    except tomllib.TOMLDecodeError as exc:
        raise MoonOutputError(f"{rel_path}: not valid TOML, so the ban cannot read its settings: {exc}") from exc
    if rel_path == "uv.toml" or rel_path.endswith("/uv.toml"):
        tables = [("", doc), ("pip", doc.get("pip"))]
    else:
        uv = (doc.get("tool") or {}).get("uv")
        tables = [("tool.uv", uv), ("tool.uv.pip", uv.get("pip") if isinstance(uv, dict) else None)]
    rows = []
    for name, table in tables:
        if table is None:
            continue
        if not isinstance(table, dict):
            rows.append(f"{rel_path}: [{name}] is not a table, so the ban cannot read it")
            continue
        for key in UV_CONFIG_MUTATING_KEYS:
            if key in table:
                where = f"[{name}] " if name else ""
                rows.append(f"{rel_path}: {where}{key} is set, so a plain project run can change the environment in place (R7)")
    return rows


def check_python_env(
    projects,
    root: Path,
    tracked,
    *,
    read_utf8: Callable[[Path], str],
    smoke_unreached: Callable[..., list[str] | None],
    binary: frozenset[str] | None = None,
    pinned: dict[tuple[str, str], str] = PINNED_LINES,
    migration: frozenset[str] = MIGRATION_FILES,
) -> list[str]:
    """All rows of the `a14` key. `binary` None asks git; the self-test passes a fixed set."""
    root = Path(root)
    tracked = sorted(tracked)
    if binary is None:
        binary = binary_paths(root, tracked)
    rows, reads = ban_rows(root, tracked, read_text=read_utf8, binary=binary, pinned=pinned, migration=migration)
    rows.extend(wrapper_rows(root))
    rows.extend(floor_rows(projects))
    if not (root / MOON_TOOLCHAINS).is_file():
        rows.append(f"FLOOR: {MOON_TOOLCHAINS} is missing, so the ban cannot check the arguments of Moon's environment sync")
    else:
        reads.add(MOON_TOOLCHAINS)
        rows.extend(toolchain_rows(MOON_TOOLCHAINS, read_utf8(root / MOON_TOOLCHAINS)))
    configs = uv_configs_of(tracked)
    if REQUIRED_UV_CONFIG not in configs:
        rows.append(f"FLOOR: {REQUIRED_UV_CONFIG} is not in the TOML configuration list, so the R7 check read the wrong set")
    for rel_path in configs:
        reads.add(rel_path)
        rows.extend(uv_config_rows(rel_path, read_utf8(root / rel_path)))
    # G5: an edit to a file the ban reads must schedule repo:affected-smoke.
    unreached = smoke_unreached(projects, reads)
    if unreached is None:
        rows.append(
            f"{SMOKE_PROJECT}:{SMOKE_TASK} reported no `inputGlobs`, so the ban cannot check that an edit "
            f"to a file it reads schedules it"
        )
    else:
        rows.extend(
            f"{rel_path} is read by the ban but matches no input of {SMOKE_PROJECT}:{SMOKE_TASK}, so an "
            f"edit to it alone does not schedule the gate that reads it"
            for rel_path in unreached
        )
    return list(dict.fromkeys(rows))


class WrapperCase(NamedTuple):
    name: str
    cwd: str  # relative to the fixture root
    args: tuple[str, ...]
    env: dict[str, str]
    fake_rc: int
    want_rc: int
    want_tool: str | None  # the fake that must have run, as its log key; None = no fake may run
    want_cwd: str | None  # relative to the fixture root
    want_argv: tuple[str, ...] | None  # the arguments the fake must have received


WRAPPER_CASES: tuple[WrapperCase, ...] = (
    WrapperCase("run py", "py", ("run", "py", "pytest", "-q"), {}, 0, 0, "uv", "py", ("run", "--locked", "--", "pytest", "-q")),
    WrapperCase("run py member dir", "py/packages/paigasus-kernel", ("run", "py", "x"), {}, 0, 0, "uv", "py/packages/paigasus-kernel", ("run", "--locked", "--", "x")),
    WrapperCase("run py outside py", "sub", ("run", "py", "x"), {}, 0, 2, None, None, None),
    WrapperCase("run py sibling", "py-x", ("run", "py", "x"), {}, 0, 2, None, None, None),
    WrapperCase("run py root", ".", ("run", "py", "x"), {}, 0, 2, None, None, None),
    WrapperCase(
        "run ci", "sub", ("run", "ci/helm-render", "python3", "a.py"), {}, 0, 0, "uv", "sub",
        ("run", "--locked", "--project", "<ROOT>/ci/helm-render", "--python", ">=3.12", "--", "python3", "a.py"),
    ),
    WrapperCase("run unknown project", "sub", ("run", "ci/osv", "x"), {}, 0, 2, None, None, None),
    WrapperCase("run-frozen py", "py", ("run-frozen", "py", "x"), {}, 0, 0, "uv", "py", ("run", "--frozen", "--", "x")),
    WrapperCase("run-frozen outside py", "sub", ("run-frozen", "py", "x"), {}, 0, 2, None, None, None),
    WrapperCase(
        "protoc-plugin", "sub", ("protoc-plugin",), {}, 0, 0, "uv", "sub",
        ("run", "--locked", "--project", "<ROOT>/py", "protoc-gen-python_betterproto2"),
    ),
    WrapperCase(
        "kernel-test", "sub", ("kernel-test",), {}, 0, 0, "uv", "py/packages/paigasus-kernel",
        ("run", "--isolated", "--reinstall-package", "paigasus-py-bindings", "pytest", "tests"),
    ),
    WrapperCase("script rc 2", "sub", ("script", "f.py", "a"), {}, 2, 2, "uv", "sub", ("run", "--isolated", "--no-project", "--python", ">=3.12", "python3", "f.py", "a")),
    WrapperCase("script rc 3", "sub", ("script", "f.py", "a"), {}, 3, 3, "uv", "sub", ("run", "--isolated", "--no-project", "--python", ">=3.12", "python3", "f.py", "a")),
    WrapperCase("script rc 4", "sub", ("script", "f.py", "a"), {}, 4, 4, "uv", "sub", ("run", "--isolated", "--no-project", "--python", ">=3.12", "python3", "f.py", "a")),
    WrapperCase("script dash file", "sub", ("script", "-c", "x"), {}, 0, 2, None, None, None),
    WrapperCase("build", "py/packages/paigasus-kernel", ("build",), {}, 0, 0, "uv", "py/packages/paigasus-kernel", ("build",)),
    WrapperCase(
        "build package rel outdir", "sub", ("build", "paigasus-proto", "dist"), {}, 0, 0, "uv", "py",
        ("build", "--package", "paigasus-proto", "--out-dir", "<ROOT>/sub/dist"),
    ),
    WrapperCase(
        "build package dotdot outdir", "sub", ("build", "paigasus-kernel", "../out"), {}, 0, 0, "uv", "py",
        ("build", "--package", "paigasus-kernel", "--out-dir", "<ROOT>/out"),
    ),
    WrapperCase("build unknown package", "sub", ("build", "paigasus-x", "d"), {}, 0, 2, None, None, None),
    WrapperCase("build dash outdir", "sub", ("build", "paigasus-proto", "-o"), {}, 0, 2, None, None, None),
    WrapperCase("lock", "sub", ("lock",), {}, 0, 0, "uv", "py", ("lock",)),
    WrapperCase("lock under moon", "sub", ("lock",), {"MOON_PROJECT_ID": "repo"}, 0, 2, None, None, None),
    WrapperCase("lock under empty moon id", "sub", ("lock",), {"MOON_PROJECT_ID": ""}, 0, 2, None, None, None),
    WrapperCase("cache-dir", "sub", ("cache-dir",), {}, 0, 0, "uv", "sub", ("cache", "dir")),
    WrapperCase("install", "sub", ("install",), {}, 0, 0, "proto", "sub", ("install", "uv")),
    WrapperCase("install-ziglang", "sub", ("install-ziglang", "<FAKE>/python"), {}, 0, 0, "python", "sub", ("-m", "pip", "install", "ziglang")),
    WrapperCase(
        "install-ziglang-system", "sub", ("install-ziglang-system", "<FAKE>/python3"), {}, 0, 0, "python3", "sub",
        ("-m", "pip", "install", "--break-system-packages", "ziglang"),
    ),
    WrapperCase(
        "wheel-smoke", "sub", ("wheel-smoke", "<ROOT>/v", "x.whl"), {}, 0, 0, "v/bin/python", "sub",
        ("-m", "pip", "install", "--no-index", "--disable-pip-version-check", "x.whl"),
    ),
    WrapperCase(
        "wheel-smoke windows", "sub", ("wheel-smoke", "<ROOT>/w", "x.whl"), {}, 0, 0, "w/Scripts/python.exe", "sub",
        ("-m", "pip", "install", "--no-index", "--disable-pip-version-check", "x.whl"),
    ),
    WrapperCase("wheel-smoke extra argument", "sub", ("wheel-smoke", "<ROOT>/v", "x.whl", "y"), {}, 0, 2, None, None, None),
    WrapperCase("sdist-install", "sub", ("sdist-install", "<ROOT>/v", "p.tar.gz"), {}, 0, 0, "v/bin/pip", "sub", ("install", "p.tar.gz")),
    WrapperCase("sdist-install extra argument", "sub", ("sdist-install", "<ROOT>/v", "p.tar.gz", "y"), {}, 0, 2, None, None, None),
    WrapperCase("sdist-install windows", "sub", ("sdist-install", "<ROOT>/w", "p.tar.gz"), {}, 0, 0, "w/Scripts/pip.exe", "sub", ("install", "p.tar.gz")),
    WrapperCase("wheel-smoke dash wheel", "sub", ("wheel-smoke", "<ROOT>/v", "-x"), {}, 0, 2, None, None, None),
    WrapperCase("sdist-install dash sdist", "sub", ("sdist-install", "<ROOT>/v", "-x"), {}, 0, 2, None, None, None),
    WrapperCase("run-frozen unknown target", "py", ("run-frozen", "ci/helm-render", "x"), {}, 0, 2, None, None, None),
    WrapperCase("install-ziglang missing python", "sub", ("install-ziglang", "<ROOT>/nov/python"), {}, 0, 127, None, None, None),
    WrapperCase("install-ziglang extra argument", "sub", ("install-ziglang", "<FAKE>/python", "y"), {}, 0, 2, None, None, None),
    WrapperCase("install-ziglang dash python", "sub", ("install-ziglang", "-x"), {}, 0, 2, None, None, None),
    WrapperCase("install-ziglang-system missing python", "sub", ("install-ziglang-system", "<ROOT>/nov/python3"), {}, 0, 127, None, None, None),
    WrapperCase("install-ziglang-system extra argument", "sub", ("install-ziglang-system", "<FAKE>/python3", "y"), {}, 0, 2, None, None, None),
    WrapperCase("install-ziglang-system dash python", "sub", ("install-ziglang-system", "-x"), {}, 0, 2, None, None, None),
    WrapperCase("build empty outdir", "sub", ("build", "paigasus-proto", ""), {}, 0, 2, None, None, None),
    WrapperCase("argv transport", "py", ("run", "py", "p", "", "a b", "--locked"), {}, 0, 0, "uv", "py", ("run", "--locked", "--", "p", "", "a b", "--locked")),
    WrapperCase("unknown operation", "sub", ("sync",), {}, 0, 2, None, None, None),
    WrapperCase("no operation", "sub", (), {}, 0, 2, None, None, None),
)

# (name, args, want_rc, fake dir is on PATH). With the tool absent the wrapper must exit 127.
_ABSENT_CASES: tuple[tuple[str, tuple[str, ...], int, bool], ...] = (
    ("probe, uv absent", ("probe",), 127, False),
    ("run py, uv absent", ("run", "py", "x"), 127, False),
    ("install, proto absent", ("install",), 127, False),
    ("sdist-install, pip absent", ("sdist-install", "<ROOT>/nov", "p.tar.gz"), 127, True),
    ("probe, uv present", ("probe",), 0, True),
)

_FAKE = """#!/bin/sh
self="$0"
here="$(pwd -P)"
key="${self#"$FIXROOT"/}"
key="${key#fake/bin/}"
name="$(printf '%s' "$key" | tr '/' '_')"
{
  printf '%s\\0' "$here"
  for a in "$@"; do printf '%s\\0' "$a"; done
} >"$LOG/$name"
exit "${FAKE_RC:-0}"
"""


def _subst(text: str, root: Path, fake: Path) -> str:
    return text.replace("<ROOT>", str(root)).replace("<FAKE>", str(fake))


def _write_fake(path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(_FAKE, encoding="utf-8")
    path.chmod(0o755)


def _build_fixture(root: Path, repo: Path) -> None:
    wrapper = root / WRAPPER_PATH
    wrapper.parent.mkdir(parents=True)
    shutil.copyfile(repo / WRAPPER_PATH, wrapper)
    for d in ("py/packages/paigasus-kernel", "py-x", "sub", "nov", "ci/release-plan", "ci/workflow-credentials", "ci/wasm-lockstep", "ci/helm-render"):
        (root / d).mkdir(parents=True, exist_ok=True)
    for tool in ("uv", "pip", "python", "python3", "proto"):
        _write_fake(root / "fake" / "bin" / tool)
    for venv_tool in ("v/bin/python", "v/bin/pip", "w/Scripts/python.exe", "w/Scripts/pip.exe"):
        _write_fake(root / venv_tool)
    (root / "empty" / "bin").mkdir(parents=True)


def _read_log(path: Path) -> list[str]:
    return path.read_bytes().decode("utf-8").split("\0")[:-1]


def _run(bash: str, root: Path, cwd: Path, args: tuple[str, ...], env: dict[str, str], path_dir: str, log: Path, fake_rc: int) -> subprocess.CompletedProcess[str]:
    run_env = {"PATH": f"{path_dir}:/usr/bin:/bin", "LOG": str(log), "FIXROOT": str(root), "FAKE_RC": str(fake_rc)}
    run_env.update(env)
    return subprocess.run([bash, str(root / WRAPPER_PATH), *args], cwd=cwd, env=run_env, capture_output=True, text=True, check=False)


def wrapper_behaviour_failures(root: Path) -> list[str]:
    """Run every WRAPPER_CASES row and the tool-absent rows. `root` is a repository root."""
    failures: list[str] = []
    repo = Path(root).resolve()
    if not (repo / WRAPPER_PATH).is_file():
        return [f"python-env wrapper, setup: {WRAPPER_PATH} does not exist"]
    bash = os.environ.get("PYTHON_ENV_TEST_BASH", "/bin/bash")
    with tempfile.TemporaryDirectory() as tmp:
        fix = Path(tmp).resolve() / "fix"
        fix.mkdir()
        _build_fixture(fix, repo)
        fake = fix / "fake" / "bin"
        count = 0

        def fresh_log() -> Path:
            nonlocal count
            count += 1
            log = Path(tmp).resolve() / f"log{count}"
            log.mkdir()
            return log

        for case in WRAPPER_CASES:
            log = fresh_log()
            args = tuple(_subst(a, fix, fake) for a in case.args)
            proc = _run(bash, fix, fix / case.cwd, args, case.env, str(fake), log, case.fake_rc)
            tag = f"python-env wrapper, {case.name}"
            if proc.returncode != case.want_rc:
                failures.append(f"{tag}: exit {proc.returncode}, want {case.want_rc} (stderr: {proc.stderr.strip()})")
                continue
            logs = sorted(p.name for p in log.iterdir())
            if case.want_tool is None:
                if logs:
                    failures.append(f"{tag}: a tool ran ({logs}), want none")
                if not proc.stderr.startswith("python-env:"):
                    failures.append(f"{tag}: stderr does not start with 'python-env:' ({proc.stderr.strip()!r})")
                continue
            want_name = case.want_tool.replace("/", "_")
            if logs != [want_name]:
                failures.append(f"{tag}: tools run {logs}, want [{want_name!r}]")
                continue
            record = _read_log(log / want_name)
            want_cwd = str((fix / (case.want_cwd or ".")).resolve())
            if record[0] != want_cwd:
                failures.append(f"{tag}: cwd {record[0]!r}, want {want_cwd!r}")
            want_argv = [_subst(a, fix, fake) for a in (case.want_argv or ())]
            if record[1:] != want_argv:
                failures.append(f"{tag}: argv {record[1:]!r}, want {want_argv!r}")

        for name, raw_args, want_rc, with_fake in _ABSENT_CASES:
            log = fresh_log()
            args = tuple(_subst(a, fix, fake) for a in raw_args)
            path_dir = str(fake) if with_fake else str(fix / "empty" / "bin")
            proc = _run(bash, fix, fix / "py", args, {}, path_dir, log, 0)
            tag = f"python-env wrapper, {name}"
            if proc.returncode != want_rc:
                failures.append(f"{tag}: exit {proc.returncode}, want {want_rc}")
            if proc.stdout:
                failures.append(f"{tag}: stdout is not empty ({proc.stdout!r})")
            if proc.stderr and name.startswith("probe"):
                failures.append(f"{tag}: probe wrote to stderr ({proc.stderr!r})")
            if want_rc == 127 and not name.startswith("probe") and not proc.stderr.startswith("python-env:"):
                failures.append(f"{tag}: stderr does not start with 'python-env:' ({proc.stderr.strip()!r})")
            if any(log.iterdir()):
                failures.append(f"{tag}: a tool ran, want none")
    return failures


def _put(root: Path, rel: str, data: str | bytes) -> None:
    path = root / rel
    path.parent.mkdir(parents=True, exist_ok=True)
    if isinstance(data, str):
        data = data.encode("utf-8")
    path.write_bytes(data)


def self_test_failures(read_utf8: Callable[[Path], str], smoke_unreached: Callable[..., list[str] | None]) -> list[str]:
    """Every self-test row of the ban (spec 5.5). `read_utf8` and `smoke_unreached` come from cargo_moon_parity.py."""
    failures: list[str] = []

    # Tokens: one red row per token, one green row per known non-match (Review Focus 1).
    red = {  # token name -> lines that must be red
        "uv": ["uv sync", "$(uv run x)", "`uv`", "'uv'", '"uv"', "(uv)", "uv;", "uv:", "bin/uv", "[uv]", "a=uv"],
        "uvx": ["uvx ruff"], "pip": ["pip install x"], "pip3": ["pip3 install x"],
        "pip3.<n>": ["pip3.12 install x"], "ensurepip": ["python -m ensurepip"],
        "virtualenv": ["virtualenv v"], ".venv": ["py/.venv/bin/x", ".venv"],
        "UV_*": ["UV_CACHE_DIR=x", "env: { UV_PYTHON: 3 }"], "VIRTUAL_ENV": ["VIRTUAL_ENV=x"],
        "--active": ["x --active"],
    }
    green = [*NON_MATCHES, "-uv", "uv-x", "x.uv", "the uv.", "pip.exe", "uvicorn", "a.venv"]
    for name, lines in red.items():
        for line in lines:
            if name not in token_hits(line):
                failures.append(f"python-env ban: {line!r} should be red for {name}")
    for line in green:
        if token_hits(line):
            failures.append(f"python-env ban: {line!r} should be green, got {token_hits(line)}")
    # Guard the guard: a token without a red row, or a known non-match without a green row, reds.
    if [n for n, _ in TOKENS] != list(red):
        failures.append("python-env ban: a token has no red row, or a red row names no token (or the order differs)")
    if set(NON_MATCHES) - set(green):
        failures.append("python-env ban: a known non-match has no green row")
    if len(NON_MATCHES) != 8:
        failures.append(f"python-env ban: NON_MATCHES has {len(NON_MATCHES)} entries, the spec lists 8")

    # The live tables.
    if [n for n, _ in EXCLUSIONS] != ["markdown", "docs", "uv.lock", "Cargo.lock", "pnpm-lock.yaml", "binary", "dependabot", "exempt"]:
        failures.append(f"python-env ban: EXCLUSIONS names are {[n for n, _ in EXCLUSIONS]}")
    if not re.fullmatch(r"[0-9a-f]{64}", WRAPPER_SHA256):
        failures.append("python-env ban: WRAPPER_SHA256 is not 64 lowercase hex characters")
    if len(PINNED_LINES) >= 35:
        failures.append(f"python-env ban: PINNED_LINES has {len(PINNED_LINES)} entries, the limit is lower than 35")
    if set(EXEMPT_PATHS) != {"ci/affected-graph/python_env_ban.py", WRAPPER_PATH}:
        failures.append(f"python-env ban: EXEMPT_PATHS is {EXEMPT_PATHS}")

    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp).resolve()
        excl_fixtures = {
            "markdown": ["a/b.md"],
            "docs": ["docs/x.sh"],
            "uv.lock": ["py/uv.lock"],
            "Cargo.lock": ["rs/Cargo.lock"],
            "pnpm-lock.yaml": ["ts/pnpm-lock.yaml"],
            "binary": ["bin/blob.dat"],
            "dependabot": [".github/dependabot.yml"],
            "exempt": list(EXEMPT_PATHS),
        }
        fixture_binary = frozenset({"bin/blob.dat"})
        for paths in excl_fixtures.values():
            for rel in paths:
                _put(root, rel, b"uv \xff\n" if rel in fixture_binary else "#\nuv sync\n")
        _put(root, "ci/x/run.sh", "#!/bin/bash\nuv sync\n")
        _put(root, "ci/a b.sh", "uv\n")
        _put(root, "ci/latin.sh", b"uv \xff\n")
        _put(root, "ci/crlf.sh", b"uv sync\r\necho ok\r\n")
        _put(root, "ci/nl.sh", "x\nuv")
        _put(root, "ci/two.sh", "uv sync\nuv sync\n")
        _put(root, "ci/sp.sh", "  uv sync  \n")
        _put(root, "ci/sp2.sh", "uv  sync\n")
        _put(root, "ci/clean.sh", "echo ok\n")

        def run(tracked, binary=frozenset(), pinned=None, migration=frozenset()):
            return ban_rows(root, tracked, read_text=read_utf8, binary=binary, pinned={} if pinned is None else pinned, migration=migration)

        rows, reads = run(["ci/x/run.sh"])
        if len(rows) != 1 or "ci/x/run.sh:2" not in rows[0]:
            failures.append(f"python-env ban: a hit in ci/x/run.sh gave {rows}, want one row that names ci/x/run.sh:2")
        if reads != {"ci/x/run.sh"}:
            failures.append(f"python-env ban: the read set is {reads}, want {{'ci/x/run.sh'}}")
        # Guard the guard: one fixture per exclusion, and each fixture is excluded by its own entry only.
        if set(excl_fixtures) != {n for n, _ in EXCLUSIONS}:
            failures.append("python-env ban: an exclusion has no fixture, or a fixture names no exclusion")
        for name, paths in excl_fixtures.items():
            for rel in paths:
                try:
                    rows, reads = run([rel], binary=fixture_binary)
                except Exception as exc:  # a read of an excluded file raises on the non-UTF-8 fixture
                    failures.append(f"python-env ban: the {name} exclusion did not skip {rel}: {type(exc).__name__}: {exc}")
                    continue
                if rows or reads:
                    failures.append(f"python-env ban: the {name} exclusion did not skip {rel}: {rows}")
                by = [n for n, pred in EXCLUSIONS if pred(rel, fixture_binary)]
                if by != [name]:
                    failures.append(f"python-env ban: {rel} is excluded by {by}, want only [{name!r}]")
        # Review Focus 3: non-UTF-8 without `binary` raises; a path with a space is scanned; a missing path raises.
        try:
            run(["ci/latin.sh"])
            failures.append("python-env ban: a non-UTF-8 file that is not `binary` did not raise")
        except Exception as exc:  # the class comes from cargo_moon_parity.py
            if type(exc).__name__ != "MoonOutputError" or "ci/latin.sh" not in str(exc):
                failures.append(f"python-env ban: a non-UTF-8 file raised {type(exc).__name__}: {exc}")
        rows, _ = run(["ci/a b.sh"])
        if not any("ci/a b.sh:1" in r for r in rows):
            failures.append(f"python-env ban: the path with a space was not red: {rows}")
        try:
            run(["ci/gone.sh"])
            failures.append("python-env ban: a tracked path that is missing on disk did not raise")
        except Exception as exc:  # the wrong class is a row, not a crash
            if not isinstance(exc, MoonOutputError) or "ci/gone.sh" not in str(exc):
                failures.append(f"python-env ban: a missing tracked path raised {type(exc).__name__}, want MoonOutputError: {exc}")
        # Review Focus 2: CRLF, a pinned line in a CRLF file, and a last line with no newline.
        rows, _ = run(["ci/crlf.sh"])
        if len(rows) != 1 or "ci/crlf.sh:1" not in rows[0]:
            failures.append(f"python-env ban: the CRLF line was not red once: {rows}")
        rows, _ = run(["ci/crlf.sh"], pinned={("ci/crlf.sh", "uv sync"): "fixture"})
        if rows:
            failures.append(f"python-env ban: a pinned line in a CRLF file was red: {rows}")
        rows, _ = run(["ci/nl.sh"])
        if len(rows) != 1 or "ci/nl.sh:2" not in rows[0]:
            failures.append(f"python-env ban: the last line with no newline was not red: {rows}")
        # Pinned lines: empty reason, stale, ambiguous, exact stripped text only.
        rows, _ = run(["ci/x/run.sh"], pinned={("ci/x/run.sh", "uv sync"): " "})
        if not any("empty reason" in r for r in rows):
            failures.append(f"python-env ban: a pinned line with an empty reason gave no row: {rows}")
        rows, _ = run(["ci/x/run.sh"], pinned={("ci/x/run.sh", "uv lock"): "fixture"})
        if not any("stale" in r and "uv lock" in r for r in rows):
            failures.append(f"python-env ban: a pinned line that matches no line gave no stale row: {rows}")
        rows, _ = run(["ci/clean.sh"], pinned={("ci/clean.sh", "echo ok"): "fixture"})
        if not any("stale" in r and "echo ok" in r for r in rows):
            failures.append(f"python-env ban: a pinned line with no token gave no stale row: {rows}")
        rows, _ = run(["ci/two.sh"], pinned={("ci/two.sh", "uv sync"): "fixture"})
        if not any("ambiguous" in r for r in rows):
            failures.append(f"python-env ban: a pinned line that matches two lines gave no ambiguous row: {rows}")
        rows, _ = run(["ci/sp.sh"], pinned={("ci/sp.sh", "uv sync"): "fixture"})
        if rows:
            failures.append(f"python-env ban: a pinned line with outer white space was red: {rows}")
        rows, _ = run(["ci/sp2.sh"], pinned={("ci/sp2.sh", "uv sync"): "fixture"})
        if not any("ci/sp2.sh:1" in r for r in rows):
            failures.append(f"python-env ban: `uv  sync` matched the pin `uv sync`: {rows}")
        # MIGRATION_FILES: a listed file is not red; a listed file with no hit, or not tracked, is stale.
        rows, _ = run(["ci/x/run.sh"], migration=frozenset({"ci/x/run.sh"}))
        if rows:
            failures.append(f"python-env ban: a MIGRATION_FILES path was red: {rows}")
        rows, _ = run(["ci/clean.sh"], migration=frozenset({"ci/clean.sh"}))
        if not any("stale migration entry" in r and "ci/clean.sh" in r for r in rows):
            failures.append(f"python-env ban: a MIGRATION_FILES path with no hit gave no stale row: {rows}")
        rows, _ = run([], migration=frozenset({"ci/x/run.sh"}))
        if not any("stale migration entry" in r for r in rows):
            failures.append(f"python-env ban: an untracked MIGRATION_FILES path gave no stale row: {rows}")

    # binary_paths reads the real .gitattributes of this repository.
    repo = _repo_root()
    wasm = "rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm"
    got = binary_paths(repo, [wasm, WRAPPER_PATH, "ci/a b.sh"])
    if got != frozenset({wasm}):
        failures.append(f"python-env ban: binary_paths returned {sorted(got)}, want [{wasm!r}]")
    try:
        _parse_check_attr(b"x\0binary\0\xff\0")
        failures.append("python-env ban: check-attr output that is not UTF-8 did not raise")
    except Exception as exc:  # the wrong class is a row, not a crash
        if not isinstance(exc, MoonOutputError):
            failures.append(f"python-env ban: check-attr output that is not UTF-8 raised {type(exc).__name__}, want MoonOutputError")
    if binary_paths(repo, []) != frozenset():
        failures.append("python-env ban: binary_paths of no path is not empty")
    # A list far over 512 bytes, as on the real tree (the SMA-612 small-pipe hang).
    if binary_paths(repo, [WRAPPER_PATH] * 400 + [wasm]) != frozenset({wasm}):
        failures.append("python-env ban: binary_paths of a long path list is wrong")

    # G-wrapper.
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp).resolve()
        _put(root, WRAPPER_PATH, "x\n")
        real = hashlib.sha256(b"x\n").hexdigest()
        rows = wrapper_rows(root, expected="0" * 64)
        if len(rows) != 1 or real not in rows[0]:
            failures.append(f"python-env ban: a wrong wrapper hash gave {rows}, want one row with {real}")
        if wrapper_rows(root, expected=real):
            failures.append("python-env ban: the right wrapper hash gave a row")
        if not wrapper_rows(root / "nowhere"):
            failures.append("python-env ban: a missing wrapper gave no row")

    # G-floor.
    floor_ok = {"paigasus-kernel-py": {"invocations": {"test": "touch a && bash ../../../ci/python-env/run.sh kernel-test"}}}
    if floor_rows(floor_ok):
        failures.append(f"python-env ban: the floor task with the wrapper call gave {floor_rows(floor_ok)}")
    floor_bad = {"paigasus-kernel-py": {"invocations": {"test": "touch a && bash ../../../ci/python-env/run.sh run py pytest"}}}
    rows = floor_rows(floor_bad)
    if not any(FLOOR_TASK in r and not r.startswith("FLOOR:") for r in rows):
        failures.append(f"python-env ban: the floor task without {FLOOR_TEXT!r} gave {rows}")
    if not any(r.startswith("FLOOR:") and FLOOR_TASK in r for r in floor_rows({})):
        failures.append("python-env ban: a missing floor task gave no FLOOR: row")

    # G-toolchain (the SMA-765 R-toolchain fixtures, unchanged, and one `installArgs` row).
    for tc, want in (
        ("unstable_uv:\n  version: '0.11.16'\n  syncArgs: ['--reinstall']\n", True),
        ("unstable_python:\n  venvArgs:\n    - '--clear'\n", True),
        ("unstable_uv:\n  # syncArgs: ['--reinstall']\n  version: '0.11.16'\n", False),
        ("unstable_uv: {version: '0.11.16', syncArgs: ['--reinstall']}\n", True),
        ("unstable_uv:\n  \"syncArgs\": ['--reinstall']\n", True),
        ("unstable_uv:\n  version: '0.11.16' # syncArgs: x\n", False),
        ("{$schema: ' # local', unstable_uv: {syncArgs: ['--reinstall']}}\n", True),
        ('{$schema: " # local", unstable_uv: {syncArgs: [x]}}\n', True),
        ("{a: 'it''s # x', unstable_uv: {syncArgs: [x]}}\n", True),
        ('{a: "x\\" # y", unstable_uv: {syncArgs: [x]}}\n', True),
        ('"venvArgs": [x]\n', True),
        ("x: {installArgs: [x]}\n", True),
    ):
        if bool(toolchain_rows(MOON_TOOLCHAINS, tc)) is not want:
            failures.append(f"python-env ban: the toolchain rule reads {tc!r} as {'green' if want else 'red'}")

    # The TOML key check (the SMA-765 R7 fixtures, unchanged).
    for cfg_path, cfg, want in (
        ("py/pyproject.toml", "[tool.uv]\nreinstall = true\n", True),
        ("py/pyproject.toml", "[tool.uv]\ncache-keys = [{ file = 'x' }]\n", True),
        ("py/pyproject.toml", "[tool.uv.pip]\nupgrade = true\n", True),
        ("ci/x/uv.toml", "upgrade-package = ['x']\n", True),
        ("ci/x/uv.toml", "[pip]\nreinstall-package = ['x']\n", True),
        ("py/pyproject.toml", "[tool.uv]\nconstraint-dependencies = ['x>1']\n", False),
        ("py/pyproject.toml", "[project]\nname = 'x'\n# reinstall = true\n", False),
        ("ci/x/uv.toml", "[tool.uv]\nreinstall = true\n", False),
    ):
        if bool(uv_config_rows(cfg_path, cfg)) is not want:
            failures.append(f"python-env ban: the TOML key check reads {cfg_path} {cfg!r} as {'green' if want else 'red'}")
    try:
        uv_config_rows("py/pyproject.toml", "[tool.uv\n")
        failures.append("python-env ban: the TOML key check did not raise on a file that is not valid TOML")
    except MoonOutputError:
        pass
    if uv_configs_of({"a/pyproject.toml", "uv.toml", "b/x.toml", "c/uv.lock"}) != ["a/pyproject.toml", "uv.toml"]:
        failures.append("python-env ban: uv_configs_of selected the wrong paths")

    # Integration (ruling F2): the function that collect_findings calls, on one fixture tree. Each
    # rule must give its row through check_python_env, so a deleted call inside it reds.
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp).resolve()
        _put(root, WRAPPER_PATH, (repo / WRAPPER_PATH).read_bytes())
        _put(root, MOON_TOOLCHAINS, "unstable_uv:\n  version: '1'\n")
        _put(root, REQUIRED_UV_CONFIG, "[project]\nname = 'x'\n")
        _put(root, "ci/x/run.sh", "#!/bin/bash\nuv sync\n")
        smoke = {"task_inputs": {SMOKE_TASK: []}, "task_input_globs": {SMOKE_TASK: ["**/*"]}}
        projects = {SMOKE_PROJECT: smoke, **floor_ok}
        clean = [WRAPPER_PATH, MOON_TOOLCHAINS, REQUIRED_UV_CONFIG]

        def full(projects=projects, tracked=clean):
            return check_python_env(projects, root, tracked, read_utf8=read_utf8, smoke_unreached=smoke_unreached,
                                    binary=frozenset(), pinned={}, migration=frozenset())

        if full():
            failures.append(f"python-env ban, integration: the clean fixture tree gave {full()}")
        if not any("ci/x/run.sh:2" in r for r in full(tracked=[*clean, "ci/x/run.sh"])):
            failures.append("python-env ban, integration: check_python_env did not run the token ban")
        if not any(FLOOR_TASK in r for r in full(projects={SMOKE_PROJECT: smoke, **floor_bad})):
            failures.append("python-env ban, integration: check_python_env did not run the floor rule")
        if not any(r.startswith("FLOOR:") and REQUIRED_UV_CONFIG in r for r in full(tracked=[WRAPPER_PATH, MOON_TOOLCHAINS])):
            failures.append("python-env ban, integration: check_python_env did not floor the TOML key check")
        unscheduled = {SMOKE_PROJECT: {"task_inputs": {SMOKE_TASK: []}, "task_input_globs": {SMOKE_TASK: ["ci/**/*"]}}, **floor_ok}
        if not any(MOON_TOOLCHAINS in r and "matches no input" in r for r in full(projects=unscheduled)):
            failures.append("python-env ban, integration: G5 gave no row for a read file that no smoke glob matches")
        if not any("inputGlobs" in r for r in full(projects={SMOKE_PROJECT: {}, **floor_ok})):
            failures.append("python-env ban, integration: G5 gave no row when the smoke task reports no inputGlobs")
        _put(root, MOON_TOOLCHAINS, "unstable_uv:\n  syncArgs: ['--reinstall']\n")
        if not any(MOON_TOOLCHAINS in r and "syncArgs" in r for r in full()):
            failures.append("python-env ban, integration: check_python_env did not run the toolchain rule")
        (root / MOON_TOOLCHAINS).unlink()
        if not any(r.startswith("FLOOR:") and MOON_TOOLCHAINS in r for r in full(tracked=[WRAPPER_PATH, REQUIRED_UV_CONFIG])):
            failures.append("python-env ban, integration: a missing Moon toolchain file gave no FLOOR: row")
        _put(root, MOON_TOOLCHAINS, "unstable_uv:\n  version: '1'\n")
        _put(root, REQUIRED_UV_CONFIG, "[tool.uv]\nreinstall = true\n")
        if not any(REQUIRED_UV_CONFIG in r and "reinstall" in r for r in full()):
            failures.append("python-env ban, integration: check_python_env did not run the TOML key check")
        _put(root, REQUIRED_UV_CONFIG, "[project]\nname = 'x'\n")
        _put(root, WRAPPER_PATH, "changed\n")
        if not any(WRAPPER_PATH in r and "sha256" in r for r in full()):
            failures.append("python-env ban, integration: check_python_env did not run the wrapper hash rule")

    failures.extend(wrapper_behaviour_failures(repo))
    return failures


def _repo_root() -> Path:
    return Path(__file__).resolve().parents[2]


if __name__ == "__main__":
    import sys

    found = wrapper_behaviour_failures(_repo_root())
    for line in found:
        print(line)
    print(len(found))
    sys.exit(1 if found else 0)
