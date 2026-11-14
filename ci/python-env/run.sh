#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# The one place that runs the Python package manager, pip and the proto install of the package
# manager. See README.md in this directory. Usage: bash ci/python-env/run.sh <operation> [args...]
# Exit codes: 2 usage error, 127 missing tool, every other code is the code of the program.
set -euo pipefail
export PROTO_REPORTER=text

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd -P)"

die() {
  printf 'python-env: %s\n' "$*" >&2
  exit 2
}

need_tool() {
  if ! command -v "$1" >/dev/null 2>&1; then
    printf 'python-env: %s not found\n' "$1" >&2
    exit 127
  fi
}

in_py() {
  case "$(pwd -P)/" in
    "$ROOT/py/"*) ;;
    *) die "$1 py needs a directory inside py/" ;;
  esac
}

[ "$#" -ge 1 ] || die "no operation given"
op=$1
shift

case "$op" in
  run)
    [ "$#" -ge 2 ] || die "run needs a target and a command"
    target=$1
    shift
    case "$target" in
      py)
        in_py run
        need_tool uv
        uv run --locked -- "$@"
        ;;
      ci/release-plan | ci/workflow-credentials | ci/wasm-lockstep | ci/helm-render)
        project=$target
        need_tool uv
        uv run --locked --project "$ROOT/$project" --python '>=3.12' -- "$@"
        ;;
      *) die "run: unknown target '$target'" ;;
    esac
    ;;
  run-frozen)
    [ "$#" -ge 2 ] || die "run-frozen needs a target and a command"
    [ "$1" = "py" ] || die "run-frozen: unknown target '$1'"
    shift
    in_py run-frozen
    need_tool uv
    uv run --frozen -- "$@"
    ;;
  protoc-plugin)
    [ "$#" -eq 0 ] || die "protoc-plugin takes no argument"
    need_tool uv
    uv run --locked --project "$ROOT/py" protoc-gen-python_betterproto2
    ;;
  kernel-test)
    [ "$#" -eq 0 ] || die "kernel-test takes no argument"
    need_tool uv
    cd "$ROOT/py/packages/paigasus-kernel"
    uv run --isolated --reinstall-package paigasus-py-bindings pytest tests
    ;;
  script)
    [ "$#" -ge 1 ] || die "script needs a file"
    file=$1
    shift
    case "$file" in
      -*) die "script: the file must not start with '-'" ;;
    esac
    need_tool uv
    uv run --isolated --no-project --python '>=3.12' python3 "$file" "$@"
    ;;
  build)
    case "$#" in
      0)
        need_tool uv
        uv build
        ;;
      2)
        package=$1
        outdir=$2
        case "$package" in
          paigasus-proto | paigasus-kernel) ;;
          *) die "build: unknown package '$package'" ;;
        esac
        case "$outdir" in
          '' | -*) die "build: the out dir must not be empty or start with '-'" ;;
        esac
        need_tool uv
        mkdir -p "$outdir"
        abs_outdir="$(cd "$outdir" && pwd -P)"
        cd "$ROOT/py"
        uv build --package "$package" --out-dir "$abs_outdir"
        ;;
      *) die "build takes no argument or a package and an out dir" ;;
    esac
    ;;
  lock)
    [ "$#" -eq 0 ] || die "lock takes no argument"
    [ -z "${MOON_PROJECT_ID+x}" ] || die "lock refuses to run inside a Moon task"
    need_tool uv
    cd "$ROOT/py"
    uv lock
    ;;
  cache-dir)
    [ "$#" -eq 0 ] || die "cache-dir takes no argument"
    need_tool uv
    uv cache dir
    ;;
  install)
    [ "$#" -eq 0 ] || die "install takes no argument"
    need_tool proto
    proto install uv
    ;;
  probe)
    [ "$#" -eq 0 ] || die "probe takes no argument"
    if command -v uv >/dev/null 2>&1; then
      exit 0
    fi
    exit 127
    ;;
  install-ziglang)
    [ "$#" -eq 1 ] || die "install-ziglang needs one python"
    python=$1
    case "$python" in
      -*) die "install-ziglang: the python must not start with '-'" ;;
    esac
    need_tool "$python"
    "$python" -m pip install ziglang
    ;;
  install-ziglang-system)
    [ "$#" -eq 1 ] || die "install-ziglang-system needs one python"
    python=$1
    case "$python" in
      -*) die "install-ziglang-system: the python must not start with '-'" ;;
    esac
    need_tool "$python"
    "$python" -m pip install --break-system-packages ziglang
    ;;
  wheel-smoke)
    [ "$#" -eq 2 ] || die "wheel-smoke needs a venv dir and a wheel"
    wheel=$2
    case "$wheel" in
      -*) die "wheel-smoke: the wheel must not start with '-'" ;;
    esac
    if [ -f "$1/bin/python" ]; then
      vpy="$1/bin/python"
    elif [ -f "$1/Scripts/python.exe" ]; then
      vpy="$1/Scripts/python.exe"
    else
      printf 'python-env: python not found in %s\n' "$1" >&2
      exit 127
    fi
    "$vpy" -m pip install --no-index --disable-pip-version-check "$wheel"
    ;;
  sdist-install)
    [ "$#" -eq 2 ] || die "sdist-install needs a venv dir and an sdist"
    sdist=$2
    case "$sdist" in
      -*) die "sdist-install: the sdist must not start with '-'" ;;
    esac
    if [ -f "$1/bin/pip" ]; then
      vpip="$1/bin/pip"
    elif [ -f "$1/Scripts/pip.exe" ]; then
      vpip="$1/Scripts/pip.exe"
    else
      printf 'python-env: pip not found in %s\n' "$1" >&2
      exit 127
    fi
    "$vpip" install "$sdist"
    ;;
  *) die "unknown operation '$op'" ;;
esac
