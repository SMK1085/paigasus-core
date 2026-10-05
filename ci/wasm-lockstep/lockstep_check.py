# SPDX-License-Identifier: Apache-2.0
"""The lock and artifact checker of the wasm-lockstep workflow (SMA-693 spec 5.3).

Stdlib only (tomllib, Python 3.11+). The `propose` job runs this file with the runner's own
`python3`, and installs nothing, so it must never import a third-party package.

  lockstep_check.py lock --old <lock> --new <lock>
  lockstep_check.py artifact --dir <dir> --old <lock> [--body-file <f>] [--title-file <f>]
  lockstep_check.py status --file <git-status-porcelain-file>
  lockstep_check.py current --lock <lock>
  lockstep_check.py --self-test
  lockstep_check.py --negative-control --lock <lock>

Exit codes: 0 pass | 3 refusal | 2 infrastructure | 4 no change. Not 1: `uv` and a Python
traceback both exit 1, so 1 cannot mean "the lock is wrong". ci/wasm-lockstep/run.sh maps
3 -> 1 and every other non-zero code -> 2. Do not "normalize" the refusal code to 1.

Every refusal carries a CODE (R-...). The self-test asserts the code of each row, so a row
cannot pass because a different check happened to refuse it.
"""

from __future__ import annotations

import os
import re
import stat
import sys
import tempfile
import tomllib

RC_OK = 0
RC_INFRA = 2
RC_REFUSE = 3
RC_NO_CHANGE = 4

# The seven packages the four-package `cargo update -p` moved at SMA-683 M0 (spec F1).
FAMILY = frozenset({
    "wasm-bindgen",
    "wasm-bindgen-macro",
    "wasm-bindgen-macro-support",
    "wasm-bindgen-shared",
    "js-sys",
    "web-sys",
    "wasm-bindgen-futures",
})
CRATES_IO = "registry+https://github.com/rust-lang/crates.io-index"
# Strict semver core: no pre-release, no build metadata, no leading zero. `[0-9]`, never `\d`:
# `\d` also matches non-ASCII digits such as U+FF11. Used with fullmatch(), never match(): `$`
# in a Python pattern also matches before a trailing newline.
SEMVER = re.compile(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)")
CHECKSUM = re.compile(r"[0-9a-f]{64}")
LOCK_PATH = "rs/Cargo.lock"
ARTIFACT_DIR = "rs/crates/bindings/paigasus-wasm"
ARTIFACT_FILES = (
    "paigasus_wasm.js",
    "paigasus_wasm_bg.js",
    "paigasus_wasm.d.ts",
    "paigasus_wasm_bg.wasm.d.ts",
    "paigasus_wasm_bg.wasm",
)
ALLOWED_FILES = frozenset({LOCK_PATH} | {f"{ARTIFACT_DIR}/{name}" for name in ARTIFACT_FILES})
ALLOWED_DIRS = frozenset({"rs", "rs/crates", "rs/crates/bindings", ARTIFACT_DIR})
# 8 MiB per file. The committed paigasus_wasm_bg.wasm is 35,560 bytes after wasm-opt -O (SMA-435).
SIZE_CAP = 8 * 1024 * 1024
TITLE_MAX = 100
RUNBOOK = 'rs/CLAUDE.md, "The wasm-bindgen family does not move through dependabot"'


class RefusalError(Exception):
    """The input is wrong. Maps to RC_REFUSE."""

    def __init__(self, code: str, message: str) -> None:
        super().__init__(f"{code}: {message}")
        self.code = code


class InfraError(Exception):
    """The check could not run. Maps to RC_INFRA."""


class NoChangeError(Exception):
    """The lock did not change. Maps to RC_NO_CHANGE."""


def parse_lock_text(text: str, label: str) -> dict:
    try:
        return tomllib.loads(text)
    except tomllib.TOMLDecodeError as exc:
        raise InfraError(f"{label} is not valid TOML: {exc}") from exc


def load_lock(path: str) -> dict:
    try:
        with open(path, encoding="utf-8") as handle:
            text = handle.read()
    except (OSError, UnicodeDecodeError) as exc:
        raise InfraError(f"cannot read {path}: {exc}") from exc
    return parse_lock_text(text, path)


def packages(lock: dict, label: str) -> list[dict]:
    raw = lock.get("package", [])
    if not isinstance(raw, list):
        raise RefusalError("R-SHAPE", f"{label}: `package` is not an array of tables")
    for entry in raw:
        if not isinstance(entry, dict) or not isinstance(entry.get("name"), str) \
                or not isinstance(entry.get("version"), str):
            raise RefusalError("R-SHAPE", f"{label}: a [[package]] entry has no string name and version: {entry!r}")
        deps = entry.get("dependencies", [])
        if not isinstance(deps, list) or not all(isinstance(d, str) for d in deps):
            raise RefusalError("R-SHAPE", f"{label}: {entry['name']} has a `dependencies` value that is not a list of strings")
    return raw


def key_of(entry: dict) -> tuple[str, str, str]:
    return (entry["name"], entry["version"], str(entry.get("source", "")))


def normalise_ref(ref: str) -> str:
    """Cargo writes a dependency reference in three forms: `name`, `name version` and
    `name version (source)`. A FAMILY reference becomes the bare name, so a family version move
    does not count as a change of the package that refers to it. Any other reference stays
    exactly as written."""
    name = ref.split(" ", 1)[0]
    return name if name in FAMILY else ref


def normalised(entry: dict) -> dict:
    out = dict(entry)
    if "dependencies" in out:
        out["dependencies"] = [normalise_ref(d) for d in out["dependencies"]]
    return out


def write_file(path: str, text: str) -> None:
    try:
        with open(path, "w", encoding="utf-8") as handle:
            handle.write(text)
    except OSError as exc:
        raise InfraError(f"cannot write {path}: {exc}") from exc


# ---- the verdicts ----------------------------------------------------------------------------

def check_semver(entry: dict, label: str) -> None:
    version = entry["version"]
    if not SEMVER.fullmatch(version):
        raise RefusalError("R-SEMVER", f"{label}: {entry['name']} has the version {version!r}, which is not a strict X.Y.Z")


def lock_verdict(old: dict, new: dict) -> list[tuple[str, str, str]]:
    """The AC4.2 verdict. Returns (name, old version or '-', new version or '-') per moved family
    package, sorted. Raises RefusalError or NoChangeError."""
    old_top = {k: v for k, v in old.items() if k != "package"}
    new_top = {k: v for k, v in new.items() if k != "package"}
    if old_top != new_top:
        raise RefusalError("R-FORMAT", f"a top-level lock table or the lock format version changed: {sorted(set(old_top) | set(new_top))}")
    old_pkgs = packages(old, "old lock")
    new_pkgs = packages(new, "new lock")
    old_by_key: dict[tuple[str, str, str], dict] = {}
    new_by_key: dict[tuple[str, str, str], dict] = {}
    for by_key, pkgs, label in ((old_by_key, old_pkgs, "old lock"), (new_by_key, new_pkgs, "new lock")):
        for entry in pkgs:
            if key_of(entry) in by_key:
                raise RefusalError("R-SHAPE", f"{label}: {key_of(entry)} appears twice")
            by_key[key_of(entry)] = entry

    # Non-family packages must be identical after the family references are normalised. This
    # also refuses an ADDED or REMOVED non-family package: a new transitive dependency.
    old_rest = {k: normalised(v) for k, v in old_by_key.items() if k[0] not in FAMILY}
    new_rest = {k: normalised(v) for k, v in new_by_key.items() if k[0] not in FAMILY}
    if old_rest != new_rest:
        changed = sorted({k[0] for k in set(old_rest) ^ set(new_rest)}
                         | {k[0] for k in set(old_rest) & set(new_rest) if old_rest[k] != new_rest[k]})
        raise RefusalError("R-NONFAMILY", f"a package outside the wasm-bindgen family changed, was added or was removed: {changed}")

    for label, by_key in (("old lock", old_by_key), ("new lock", new_by_key)):
        for k, entry in by_key.items():
            if k[0] in FAMILY:
                check_semver(entry, label)

    removed = [k for k in old_by_key if k not in new_by_key]
    added = [k for k in new_by_key if k not in old_by_key]
    for name in FAMILY:
        if sum(1 for k in removed if k[0] == name) > 1 or sum(1 for k in added if k[0] == name) > 1:
            raise RefusalError("R-TWICE", f"{name} has more than one removed or more than one added entry")
    new_names = [k[0] for k in new_by_key if k[0] in FAMILY]
    for name in sorted(set(new_names)):
        if new_names.count(name) > 1:
            raise RefusalError("R-DUPLICATE", f"the new lock holds two versions of {name} side by side")
    if "wasm-bindgen" not in new_names:
        raise RefusalError("R-ABSENT", "the new lock holds no wasm-bindgen entry")
    held = set(new_names)
    for entry in new_pkgs:
        for ref in entry.get("dependencies", []):
            ref_name = ref.split(" ", 1)[0]
            if ref_name in FAMILY and ref_name not in held:
                raise RefusalError("R-DANGLING", f"{entry['name']} still refers to {ref_name}, but the new lock does not hold it")
    for k in added:
        added_entry = new_by_key[k]
        if added_entry.get("source") != CRATES_IO:
            raise RefusalError("R-SOURCE", f"{k[0]} {k[1]} does not come from crates.io (source {added_entry.get('source')!r})")
        checksum = added_entry.get("checksum")
        if not isinstance(checksum, str) or not CHECKSUM.fullmatch(checksum):
            raise RefusalError("R-CHECKSUM", f"{k[0]} {k[1]} has no valid checksum")
    for k in set(old_by_key) & set(new_by_key):
        if k[0] in FAMILY and old_by_key[k].get("checksum") != new_by_key[k].get("checksum"):
            raise RefusalError("R-INPLACE", f"{k[0]} {k[1]} kept its version but changed its checksum")

    if not removed and not added:
        raise NoChangeError("the wasm-bindgen family did not move")
    rows: dict[str, list[str]] = {}
    for k in removed:
        rows.setdefault(k[0], ["-", "-"])[0] = k[1]
    for k in added:
        rows.setdefault(k[0], ["-", "-"])[1] = k[1]
    return sorted((name, pair[0], pair[1]) for name, pair in rows.items())


def current_verdict(lock: dict) -> list[tuple[str, str]]:
    """The family invariant on ONE lock, the precondition of every weekly `lock` verdict:
    wasm-bindgen is present, no family name twice, crates.io source, a checksum, strict semver."""
    found: dict[str, str] = {}
    for entry in packages(lock, "lock"):
        name = entry["name"]
        if name not in FAMILY:
            continue
        if name in found:
            raise RefusalError("R-DUPLICATE", f"the lock holds two versions of {name} side by side")
        check_semver(entry, "lock")
        if entry.get("source") != CRATES_IO:
            raise RefusalError("R-SOURCE", f"{name} does not come from crates.io")
        checksum = entry.get("checksum")
        if not isinstance(checksum, str) or not CHECKSUM.fullmatch(checksum):
            raise RefusalError("R-CHECKSUM", f"{name} has no valid checksum")
        found[name] = entry["version"]
    if "wasm-bindgen" not in found:
        raise RefusalError("R-ABSENT", "the lock holds no wasm-bindgen entry")
    return sorted(found.items())


def artifact_tree(root: str) -> None:
    """AC4.1: rs/Cargo.lock plus a subset of the five artifact files, all regular files under the
    size cap, no symlink, no other entry, no directory other than the path prefixes."""
    if not os.path.isdir(root) or os.path.islink(root):
        raise RefusalError("R-LAYOUT", f"{root} is not a directory")
    seen_files: set[str] = set()
    pending = [""]
    while pending:
        rel_dir = pending.pop()
        try:
            with os.scandir(os.path.join(root, rel_dir) if rel_dir else root) as it:
                entries = sorted(it, key=lambda e: e.name)
        except OSError as exc:
            raise InfraError(f"cannot read the artifact directory {rel_dir or root!r}: {exc}") from exc
        for entry in entries:
            rel = f"{rel_dir}/{entry.name}" if rel_dir else entry.name
            info = entry.stat(follow_symlinks=False)
            if stat.S_ISLNK(info.st_mode):
                raise RefusalError("R-SYMLINK", f"{rel!r} is a symlink")
            if stat.S_ISDIR(info.st_mode):
                if rel not in ALLOWED_DIRS:
                    raise RefusalError("R-LAYOUT", f"{rel!r} is not an allowed directory")
                pending.append(rel)
            elif stat.S_ISREG(info.st_mode):
                if rel not in ALLOWED_FILES:
                    raise RefusalError("R-LAYOUT", f"{rel!r} is not one of the six allowed files")
                if info.st_size > SIZE_CAP:
                    raise RefusalError("R-SIZE", f"{rel} is {info.st_size} bytes, over the {SIZE_CAP}-byte cap")
                seen_files.add(rel)
            else:
                raise RefusalError("R-LAYOUT", f"{rel!r} is not a regular file or a directory")
    if LOCK_PATH not in seen_files:
        raise RefusalError("R-LAYOUT", f"the artifact has no {LOCK_PATH}")


def title_for(changes: list[tuple[str, str, str]]) -> str:
    moved = {name: new for name, _old, new in changes}
    if moved.get("wasm-bindgen", "-") != "-":
        title = f"build(deps): move wasm-bindgen to {moved['wasm-bindgen']} and regenerate the wasm glue"
    else:
        title = "build(deps): move the wasm-bindgen family and regenerate the wasm glue"
    if len(title) > TITLE_MAX:
        raise RefusalError("R-TITLE", f"the commit title is {len(title)} characters, over the commitlint maximum of {TITLE_MAX}")
    return title


def body_for(changes: list[tuple[str, str, str]]) -> str:
    lines = [
        "This pull request moves the wasm-bindgen family in lockstep and regenerates the committed wasm artifacts.",
        "The scheduled `wasm-lockstep` workflow made it (SMA-693).",
        "",
        "| Package | Old | New |",
        "|---|---|---|",
    ]
    lines += [f"| `{name}` | `{old}` | `{new}` |" for name, old, new in changes]
    lines += [
        "",
        "**The artifacts come from a build that ran third-party code** (the new proc-macros, build scripts,",
        "`wasm-pack` and `wasm-bindgen-cli`). The workflow checked the file paths and the lock shape. Those checks",
        "do not make the content of the five files safe.",
        "",
        "Reviewer checklist:",
        "",
        "- [ ] Read the `rs/Cargo.lock` diff. Every changed entry is a family package from crates.io.",
        "- [ ] Confirm that `CI` is green, `committed-wasm.test.ts` included.",
        "- [ ] Expect a binary diff in `paigasus_wasm_bg.wasm`: a Linux build makes different bytes than a macOS build (SMA-634 F12).",
        "- [ ] Do not push to this branch. The next run refuses a branch that a person changed.",
        "- [ ] To bring this PR up to date with main, run the workflow again (workflow_dispatch on main). Do not push to this branch or update it with a merge; the next run then refuses.",
        "",
        f"A refusal of a later run points to the manual runbook in {RUNBOOK}.",
    ]
    return "\n".join(lines) + "\n"


def status_verdict(text: str) -> None:
    """`git status --porcelain --untracked-files=all` after the copy: rs/Cargo.lock modified, a
    subset of the five artifact paths modified, and nothing else."""
    paths = []
    for line in text.splitlines():
        if not line:
            continue
        if len(line) < 4 or line[:3] != " M ":
            raise RefusalError("R-STATUS", f"git status shows {line!r}; only an unstaged modification of an allowed file is expected")
        paths.append(line[3:])
    if LOCK_PATH not in paths:
        raise RefusalError("R-STATUS", f"git status does not show {LOCK_PATH} as modified")
    for path in paths:
        if path not in ALLOWED_FILES:
            raise RefusalError("R-STATUS", f"git status shows {path!r}, which is not one of the six allowed files")


def run_artifact(directory: str, old_path: str, body_file: str | None, title_file: str | None) -> list[tuple[str, str, str]]:
    artifact_tree(directory)
    try:
        changes = lock_verdict(load_lock(old_path), load_lock(os.path.join(directory, LOCK_PATH)))
    except NoChangeError as exc:
        raise RefusalError("R-NOCHANGE", "the artifact lock equals the checked-out lock, so there is nothing to propose") from exc
    title = title_for(changes)
    body = body_for(changes)
    # Written ONLY here, after every version string passed check_semver and every refusal had
    # its chance. A refusal above leaves both files absent.
    if title_file:
        write_file(title_file, title + "\n")
    if body_file:
        write_file(body_file, body)
    return changes


# ---- the self-test --------------------------------------------------------------------------

def _pkg(name: str, version: str, source: str | None = CRATES_IO, checksum: str | None = "a" * 64,
         deps: tuple[str, ...] = ()) -> str:
    out = f'[[package]]\nname = "{name}"\nversion = "{version}"\n'
    if source is not None:
        out += f'source = "{source}"\n'
    if checksum is not None:
        out += f'checksum = "{checksum}"\n'
    if deps:
        out += "dependencies = [\n" + "".join(f' "{d}",\n' for d in deps) + "]\n"
    return out + "\n"


HEADER = "# This file is automatically @generated by Cargo.\n# It is not intended for manual editing.\nversion = 4\n\n"
OLD_FAMILY = (("js-sys", "0.3.105"), ("wasm-bindgen", "0.2.128"), ("wasm-bindgen-futures", "0.4.78"),
              ("wasm-bindgen-macro", "0.2.128"), ("wasm-bindgen-macro-support", "0.2.128"),
              ("wasm-bindgen-shared", "0.2.128"), ("web-sys", "0.3.105"))
NEW_FAMILY = (("js-sys", "0.3.106"), ("wasm-bindgen", "0.2.129"), ("wasm-bindgen-futures", "0.4.79"),
              ("wasm-bindgen-macro", "0.2.129"), ("wasm-bindgen-macro-support", "0.2.129"),
              ("wasm-bindgen-shared", "0.2.129"), ("web-sys", "0.3.106"))
REST = _pkg("anyhow", "1.0.98", checksum="b" * 64) + _pkg("paigasus-wasm", "0.1.0", source=None, checksum=None, deps=("wasm-bindgen",))


def _lock(family=OLD_FAMILY, rest: str = REST, header: str = HEADER, checksum: str = "a" * 64) -> str:
    return header + "".join(_pkg(n, v, checksum=checksum) for n, v in family) + rest


def _outcome(fn) -> str:
    try:
        fn()
    except RefusalError as exc:
        return exc.code
    except NoChangeError:
        return "NOCHANGE"
    except InfraError:
        return "INFRA"
    return "PASS"


def _verdict(old_text: str, new_text: str):
    return lambda: lock_verdict(parse_lock_text(old_text, "old"), parse_lock_text(new_text, "new"))


def _ref_rows() -> list[tuple[str, object, str]]:
    """The three Cargo reference forms, each in a non-family package's dependencies list. The
    family move must not count as a change of that package. The negative twins prove the
    normalisation is scoped to FAMILY names: the same forms on a non-family name still refuse."""
    rows = []
    forms = (
        ("name", "wasm-bindgen", "wasm-bindgen"),
        ("name version", "wasm-bindgen 0.2.128", "wasm-bindgen 0.2.129"),
        ("name version (source)", f"wasm-bindgen 0.2.128 ({CRATES_IO})", f"wasm-bindgen 0.2.129 ({CRATES_IO})"),
    )
    for label, old_ref, new_ref in forms:
        old = _lock(rest=REST + _pkg("consumer", "1.0.0", deps=(old_ref,)))
        new = _lock(NEW_FAMILY, rest=REST + _pkg("consumer", "1.0.0", deps=(new_ref,)))
        rows.append((f"reference form `{label}` to a family package", _verdict(old, new), "PASS"))
    twin_old = _lock(rest=REST + _pkg("consumer", "1.0.0", deps=("anyhow 1.0.98",)))
    twin_new = _lock(NEW_FAMILY, rest=REST + _pkg("consumer", "1.0.0", deps=("anyhow 1.0.99",)))
    rows.append(("reference form `name version` to a NON-family package", _verdict(twin_old, twin_new), "R-NONFAMILY"))
    return rows


def _write_tree(root: str, files: dict[str, bytes], links: dict[str, str] | None = None) -> None:
    for rel, data in files.items():
        path = os.path.join(root, rel)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "wb") as handle:
            handle.write(data)
    for rel, target in (links or {}).items():
        path = os.path.join(root, rel)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        os.symlink(target, path)


def _unreadable(root: str, old_path: str) -> None:
    target = os.path.join(root, "rs", "crates")
    os.chmod(target, 0)
    try:
        if os.access(target, os.R_OK):  # running as root: chmod does not bite, so force the outcome
            raise InfraError("the directory stayed readable (root); row skipped")
        run_artifact(root, old_path, None, None)
    finally:
        os.chmod(target, 0o755)


def _artifact_rows(tmp: str) -> list[tuple[str, object, str]]:
    old_path = os.path.join(tmp, "old.lock")
    write_file(old_path, _lock())
    good = {LOCK_PATH: _lock(NEW_FAMILY).encode()}
    good.update({f"{ARTIFACT_DIR}/{n}": b"x" for n in ARTIFACT_FILES})
    cases: list[tuple[str, dict[str, bytes], dict[str, str] | None, str]] = [
        ("artifact in the real downloaded layout", good, None, "PASS"),
        ("artifact with only the lock and two glue files", {k: v for k, v in good.items() if k == LOCK_PATH or k.endswith(".js")}, None, "PASS"),
        ("artifact under an extra rs/ level", {f"rs/{k}": v for k, v in good.items()}, None, "R-LAYOUT"),
        ("artifact without the rs/ level", {k[len("rs/"):]: v for k, v in good.items()}, None, "R-LAYOUT"),
        ("a sixth artifact file", {**good, f"{ARTIFACT_DIR}/extra.js": b"x"}, None, "R-LAYOUT"),
        ("a case variant of an artifact name", {LOCK_PATH: good[LOCK_PATH], f"{ARTIFACT_DIR}/Paigasus_wasm.js": b"x"}, None, "R-LAYOUT"),
        ("a missing rs/Cargo.lock", {k: v for k, v in good.items() if k != LOCK_PATH}, None, "R-LAYOUT"),
        ("a symlinked artifact file", {LOCK_PATH: good[LOCK_PATH]}, {f"{ARTIFACT_DIR}/paigasus_wasm.js": "/etc/hostname"}, "R-SYMLINK"),
        ("a symlinked parent directory", {LOCK_PATH: good[LOCK_PATH]}, {"rs/crates": "/tmp"}, "R-SYMLINK"),
        ("an artifact lock equal to the old lock", {**good, LOCK_PATH: _lock().encode()}, None, "R-NOCHANGE"),
        ("an artifact lock that does not parse", {**good, LOCK_PATH: b"version = = 4\n"}, None, "INFRA"),
    ]
    rows: list[tuple[str, object, str]] = []
    for index, (label, files, links, want) in enumerate(cases):
        root = os.path.join(tmp, f"case{index}")
        os.makedirs(root)
        _write_tree(root, files, links)
        rows.append((label, (lambda r=root: run_artifact(r, old_path, None, None)), want))
    # The size cap: a sparse file one byte over 8 MiB costs no disk.
    big = os.path.join(tmp, "big")
    _write_tree(big, {LOCK_PATH: good[LOCK_PATH], f"{ARTIFACT_DIR}/paigasus_wasm_bg.wasm": b""})
    os.truncate(os.path.join(big, ARTIFACT_DIR, "paigasus_wasm_bg.wasm"), SIZE_CAP + 1)
    locked = os.path.join(tmp, "locked")
    _write_tree(locked, good)
    rows.append(("an unreadable artifact directory", lambda: _unreadable(locked, old_path), "INFRA"))
    rows.append(("an oversize artifact file", lambda: run_artifact(big, old_path, None, None), "R-SIZE"))
    return rows


def _files_rows(tmp: str) -> list[tuple[str, object, str]]:
    """The title and body files exist after a pass and do not exist after a refusal."""
    old_path = os.path.join(tmp, "files-old.lock")
    write_file(old_path, _lock())
    rows = []
    for label, new_text, want_files, want_outcome in (
            ("pass", _lock(NEW_FAMILY), True, "PASS"),
            ("refusal", _lock(NEW_FAMILY, rest=REST + _pkg("evil", "1.0.0")), False, "R-NONFAMILY")):
        root = os.path.join(tmp, f"files-{label}")
        _write_tree(root, {LOCK_PATH: new_text.encode()})
        title = os.path.join(tmp, f"title-{label}.txt")
        body = os.path.join(tmp, f"body-{label}.md")

        def check(root=root, title=title, body=body, want_files=want_files, want_outcome=want_outcome) -> None:
            outcome = _outcome(lambda: run_artifact(root, old_path, body, title))
            if outcome != want_outcome:
                raise RefusalError("R-SELFTEST", f"the run ended with {outcome}, want {want_outcome}")
            exists = os.path.exists(title) or os.path.exists(body)
            if exists != want_files:
                raise RefusalError("R-SELFTEST", f"the title/body files exist={exists} after a {outcome}")
            if want_files:
                with open(title, encoding="utf-8") as handle:
                    if handle.read() != "build(deps): move wasm-bindgen to 0.2.129 and regenerate the wasm glue\n":
                        raise RefusalError("R-SELFTEST", "the title file holds the wrong text")
        rows.append((f"the title and body files after a {label}", check, "PASS"))
    return rows


def self_test() -> int:
    eighth = REST + _pkg("bumpalo", "3.19.0")
    changed_rest = _pkg("anyhow", "1.0.99", checksum="b" * 64) + _pkg("paigasus-wasm", "0.1.0", source=None, checksum=None, deps=("wasm-bindgen",))
    dropped = tuple(p for p in NEW_FAMILY if p[0] != "wasm-bindgen-macro-support")
    git_source = tuple((n, v) for n, v in NEW_FAMILY if n != "web-sys")
    git_web_sys = _pkg("web-sys", "0.3.106", source="git+https://github.com/evil/web-sys#abc", checksum=None)
    no_checksum = _pkg("web-sys", "0.3.106", checksum=None)
    rows: list[tuple[str, object, str]] = [
        ("the M0 bump of all seven", _verdict(_lock(), _lock(NEW_FAMILY)), "PASS"),
        ("a bump that drops one family package", _verdict(_lock(), _lock(dropped)), "PASS"),
        ("a new lock without wasm-bindgen", _verdict(_lock(), _lock(tuple(p for p in NEW_FAMILY if p[0] != "wasm-bindgen"), rest=REST)), "R-ABSENT"),
        ("a new lock with no family package at all", _verdict(_lock(), _lock((), rest=REST)), "R-ABSENT"),
        ("a dropped family package still referenced", _verdict(_lock(rest=REST + _pkg("consumer", "1.0.0", deps=("wasm-bindgen-macro-support 0.2.128",))), _lock(dropped, rest=REST + _pkg("consumer", "1.0.0", deps=("wasm-bindgen-macro-support 0.2.128",)))), "R-DANGLING"),
        ("an eighth package added", _verdict(_lock(), _lock(NEW_FAMILY, rest=eighth)), "R-NONFAMILY"),
        ("a non-family package changed", _verdict(_lock(), _lock(NEW_FAMILY, rest=changed_rest)), "R-NONFAMILY"),
        ("a git source on a family entry", _verdict(_lock(), _lock(git_source, rest=REST + git_web_sys)), "R-SOURCE"),
        ("a family entry without a checksum", _verdict(_lock(), _lock(git_source, rest=REST + no_checksum)), "R-CHECKSUM"),
        ("a family name moved twice", _verdict(_lock((*OLD_FAMILY, ("wasm-bindgen", "0.2.127"))), _lock(NEW_FAMILY)), "R-TWICE"),
        ("two versions of one family name in the new lock", _verdict(_lock(), _lock((*OLD_FAMILY, ("wasm-bindgen", "0.2.129")))), "R-DUPLICATE"),
        ("a shell payload in a version string", _verdict(_lock(), _lock((("wasm-bindgen", '0.2.129\\"; rm -rf /'),))), "R-SEMVER"),
        ("a pre-release version", _verdict(_lock(), _lock((("wasm-bindgen", "0.2.129-rc.1"),))), "R-SEMVER"),
        ("a version with a trailing newline", _verdict(_lock(), _lock((("wasm-bindgen", "0.2.129\\n"),))), "R-SEMVER"),
        ("a version with non-ASCII digits", _verdict(_lock(), _lock((("wasm-bindgen", "0.2." + chr(0xFF11) + chr(0xFF12) + chr(0xFF19)),))), "R-SEMVER"),
        ("a version with a leading zero", _verdict(_lock(), _lock((("wasm-bindgen", "0.2.0129"),))), "R-SEMVER"),
        ("no change", _verdict(_lock(), _lock()), "NOCHANGE"),
        ("the lock format version changed", _verdict(_lock(), _lock(NEW_FAMILY, header=HEADER.replace("version = 4", "version = 3"))), "R-FORMAT"),
        ("a [patch.unused] table added", _verdict(_lock(), _lock(NEW_FAMILY) + '[[patch.unused]]\nname = "x"\nversion = "1.0.0"\n'), "R-FORMAT"),
        ("a family checksum changed in place", _verdict(_lock(), _lock(checksum="c" * 64)), "R-INPLACE"),
        ("a lock that does not parse", _verdict(_lock(), "version = = 4\n"), "INFRA"),
        ("current: the real shape", lambda: current_verdict(parse_lock_text(_lock(), "lock")), "PASS"),
        ("current: two versions side by side", lambda: current_verdict(parse_lock_text(_lock((*OLD_FAMILY, ("wasm-bindgen", "0.2.129"))), "lock")), "R-DUPLICATE"),
        ("current: no wasm-bindgen", lambda: current_verdict(parse_lock_text(_lock(OLD_FAMILY[:1]), "lock")), "R-ABSENT"),
        ("status: the lock and two glue files", lambda: status_verdict(f" M {LOCK_PATH}\n M {ARTIFACT_DIR}/paigasus_wasm.js\n M {ARTIFACT_DIR}/paigasus_wasm_bg.wasm\n"), "PASS"),
        ("status: an untracked file", lambda: status_verdict(f" M {LOCK_PATH}\n?? {ARTIFACT_DIR}/extra.js\n"), "R-STATUS"),
        ("status: another tracked file", lambda: status_verdict(f" M {LOCK_PATH}\n M rs/Cargo.toml\n"), "R-STATUS"),
        ("status: the lock is not modified", lambda: status_verdict(f" M {ARTIFACT_DIR}/paigasus_wasm.js\n"), "R-STATUS"),
        ("title: an over-long version", lambda: title_for([("wasm-bindgen", "0.2.128", "0." + "9" * 80 + ".0")]), "R-TITLE"),
    ]
    rows += _ref_rows()
    failures = 0
    with tempfile.TemporaryDirectory() as tmp:
        rows += _artifact_rows(tmp)
        rows += _files_rows(tmp)
        for label, fn, want in rows:
            got = _outcome(fn)
            if got == want:
                print(f"  ok    {label}: {got}")
            else:
                print(f"  FAIL  {label}: want {want}, got {got}", file=sys.stderr)
                failures += 1
    print(f"lockstep_check self-test: {len(rows)} rows, {failures} failed")
    return RC_REFUSE if failures else RC_OK


def negative_control(lock_path: str) -> int:
    """The real `lock` verdict on HEAD's rs/Cargo.lock (the caller writes it with `git show`, F15):
    the lock against itself is NOCHANGE; one non-family version changed is R-NONFAMILY; a
    synthetic move of the real wasm-bindgen entries passes."""
    try:
        with open(lock_path, encoding="utf-8") as handle:
            text = handle.read()
    except (OSError, UnicodeDecodeError) as exc:
        raise InfraError(f"cannot read {lock_path}: {exc}") from exc
    real = parse_lock_text(text, lock_path)
    family = current_verdict(real)
    failures = 0

    def expect(label: str, fn, want: str) -> None:
        nonlocal failures
        got = _outcome(fn)
        if got == want:
            print(f"  ok    {label}: {got}")
        else:
            print(f"  FAIL  {label}: want {want}, got {got}", file=sys.stderr)
            failures += 1

    expect("HEAD's lock against itself", lambda: lock_verdict(real, real), "NOCHANGE")
    first = next((p for p in packages(real, "lock") if p["name"] not in FAMILY and "source" in p), None)
    if first is None:
        raise InfraError(f"{lock_path} holds no non-family package with a source to mutate")
    mutated = parse_lock_text(text, lock_path)
    for entry in mutated["package"]:
        if key_of(entry) == key_of(first):
            entry["version"] = "999.0.0"
    expect(f"HEAD's lock with {first['name']} moved", lambda: lock_verdict(real, mutated), "R-NONFAMILY")
    bumped = parse_lock_text(text, lock_path)
    for entry in bumped["package"]:
        if entry["name"] in FAMILY:
            major, minor, patch = entry["version"].split(".")
            entry["version"] = f"{major}.{minor}.{int(patch) + 1}"
            entry["checksum"] = "d" * 64
    expect("HEAD's lock with every family patch version moved", lambda: lock_verdict(real, bumped), "PASS")
    print(f"lockstep_check negative control: wasm-bindgen {dict(family)['wasm-bindgen']} on HEAD, {failures} failed")
    return RC_REFUSE if failures else RC_OK


# ---- the command line -----------------------------------------------------------------------

def _options(argv: list[str], names: tuple[str, ...], required: tuple[str, ...]) -> dict[str, str]:
    out: dict[str, str] = {}
    rest = list(argv)
    while rest:
        flag = rest.pop(0)
        if flag not in names or not rest:
            raise InfraError(f"usage: unknown flag or missing value at {flag!r}")
        out[flag] = rest.pop(0)
    for name in required:
        if name not in out:
            raise InfraError(f"usage: {name} is required")
    return out


def main(argv: list[str]) -> int:
    if argv == ["--self-test"]:
        return self_test()
    if argv[:1] == ["--negative-control"]:
        opts = _options(argv[1:], ("--lock",), ("--lock",))
        return negative_control(opts["--lock"])
    if not argv:
        raise InfraError("usage: lockstep_check.py lock|artifact|status|current ... | --self-test | --negative-control --lock <path>")
    command, rest = argv[0], argv[1:]
    if command == "lock":
        opts = _options(rest, ("--old", "--new"), ("--old", "--new"))
        try:
            changes = lock_verdict(load_lock(opts["--old"]), load_lock(opts["--new"]))
        except NoChangeError:
            for name, version in current_verdict(load_lock(opts["--new"])):
                print(f"family-current {name} {version}")
            return RC_NO_CHANGE
        for name, old, new in changes:
            print(f"family-moved {name} {old} {new}")
        return RC_OK
    if command == "artifact":
        opts = _options(rest, ("--dir", "--old", "--body-file", "--title-file"), ("--dir", "--old"))
        changes = run_artifact(opts["--dir"], opts["--old"], opts.get("--body-file"), opts.get("--title-file"))
        for name, old, new in changes:
            print(f"family-moved {name} {old} {new}")
        return RC_OK
    if command == "status":
        opts = _options(rest, ("--file",), ("--file",))
        try:
            with open(opts["--file"], encoding="utf-8") as handle:
                text = handle.read()
        except OSError as exc:
            raise InfraError(f"cannot read {opts['--file']}: {exc}") from exc
        status_verdict(text)
        print("status: only allowed files changed")
        return RC_OK
    if command == "current":
        opts = _options(rest, ("--lock",), ("--lock",))
        for name, version in current_verdict(load_lock(opts["--lock"])):
            print(f"family-current {name} {version}")
        return RC_OK
    raise InfraError(f"unknown command {command!r}")


if __name__ == "__main__":
    try:
        sys.exit(main(sys.argv[1:]))
    except RefusalError as exc:
        print(f"lockstep_check: REFUSED {exc}. Nothing was pushed. The manual runbook is in {RUNBOOK}.", file=sys.stderr)
        sys.exit(RC_REFUSE)
    except InfraError as exc:
        print(f"lockstep_check: infrastructure error: {exc}", file=sys.stderr)
        sys.exit(RC_INFRA)
