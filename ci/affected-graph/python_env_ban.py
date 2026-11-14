# SPDX-License-Identifier: Apache-2.0
# SMA-770 -- the behaviour self-test of the ci/python-env wrapper.
#
# `wrapper_behaviour_failures` copies `ci/python-env/run.sh` into a temporary root, puts fake tools
# on PATH, runs the wrapper once per case, and compares the working directory and the arguments
# that the fake tool logged. Each failure is the string `python-env wrapper, <name>: <what differs>`.
#
# The bash binary comes from the PYTHON_ENV_TEST_BASH environment variable (default /bin/bash), so a
# re-run under Homebrew bash is real.
from __future__ import annotations

import os
import shutil
import subprocess
import tempfile
from pathlib import Path
from typing import NamedTuple

WRAPPER_PATH = "ci/python-env/run.sh"


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


def _repo_root() -> Path:
    return Path(__file__).resolve().parents[2]


if __name__ == "__main__":
    import sys

    found = wrapper_behaviour_failures(_repo_root())
    for line in found:
        print(line)
    print(len(found))
    sys.exit(1 if found else 0)
