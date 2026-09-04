#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Decide whether a push to `main` has anything to release (SMA-603).

WHY THIS IS NOT A DRY RUN. The obvious design reads
`release-plz release --dry-run --output json` and skips on an empty `releases` array. It is
WRONG, and measurement M6 in the spec is why: with only the `kernel` version group bumped,
release-plz logs that it WOULD publish paigasus-kernel and cut `paigasus-kernel-v0.1.1`, and
still prints `{"releases":[]}` at exit 0. That array records PERFORMED releases, and a dry run
performs none, so it cannot tell "nothing to release" from "a release is pending". Reading it
would have silently, greenly and permanently skipped every kernel-group release.

WHAT THIS READS INSTEAD. Measurements M2 and M6 both show release-plz short-circuiting on TAG
EXISTENCE, before any registry or cargo work: `Already published - Tag <pkg>-v<version> already
exists`. That predicate is a pure function of local state, so it needs no token, no network and
no cargo — and it can be fixture-tested, which the dry-run reading could not be.

FAIL-SAFE DIRECTION. Every inconclusive outcome returns False, which BUILDS. A false build costs
runner time; a false skip silently drops a release. Nothing here may invert that.
"""
from __future__ import annotations

import argparse
import contextlib
import io
import shutil
import subprocess
import sys
import tempfile
import tomllib
from pathlib import Path
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from collections.abc import Callable

# --- The pinned vocabulary ---------------------------------------------------------------------

# What release-plz TAGS. CLAUDE.md records the measurement from the first live release: it only
# tags what it PUBLISHES — three tags, not six, and the three `publish = false` kernel-family
# binding crates were never mentioned in the release job log at all.
#
# STRICT EQUALITY, asserted by --assert against the DERIVED set. This is the EXPECTED_PR_SUBJECTS
# idiom: a newly publishable crate reds this gate until someone re-baselines deliberately. The
# RUNTIME path does NOT use this set — it derives, so a new crate is honoured immediately even if
# the re-baseline was forgotten. The pin exists to force the re-baseline to be conscious, never to
# drive the decision.
EXPECTED_RELEASABLE = frozenset({
    "paigasus-kernel",
    "paigasus-proto",
    "paigasus-proto-derive",
})

# release-plz's default tag format. --assert refuses to run if `git_tag_name` is configured
# anywhere, because `tag_for` below assumes this shape.
def tag_for(name: str, version: str) -> str:
    return f"{name}-v{version}"


class InconclusiveError(Exception):
    """Collection failed. Every raise site must end in nothing_to_release=false."""


# --- The decision, as a pure function ----------------------------------------------------------

def decide(event_name: str, packages: dict[str, str], tags: set[str]) -> tuple[bool, str]:
    """True means "nothing to release; skip the build matrix". Fixture-tested below."""
    if event_name != "push":
        # A workflow_dispatch is a deliberate act meaning "release now", so it ALWAYS builds.
        # That is the lever for the state where tags are cut but a registry is missing
        # (SMA-580's npm half). Spec §3.2 step 1.
        return False, f"event is {event_name!r}, not 'push' — build"
    if not packages:
        return False, "no releasable package resolved — build"
    if not tags:
        # THE SHALLOW-CHECKOUT FLOOR, and it is REDUNDANT FOR SAFETY — say so rather than
        # implying otherwise. With no tags every wanted tag is absent, so `missing` below is
        # non-empty and we would build anyway. It is kept for one reason: it names the
        # misconfiguration in the log, instead of reporting a list of "not yet cut" tags that
        # were in fact never looked for. A reader debugging a surprise build needs that
        # distinction.
        return False, "the repository reports no tags at all — build"
    missing = sorted(tag_for(n, v) for n, v in packages.items() if tag_for(n, v) not in tags)
    if missing:
        return False, f"tags not yet cut: {', '.join(missing)} — build"
    return True, "every releasable package is already tagged — nothing to release"


# --- Collection --------------------------------------------------------------------------------

def load_toml(path: Path) -> dict:
    try:
        with path.open("rb") as fh:
            return tomllib.load(fh)
    except (OSError, tomllib.TOMLDecodeError) as exc:
        raise InconclusiveError(f"cannot read {path}: {exc}") from exc


def config_sections(cfg: dict) -> tuple[dict, list[dict]]:
    """Validate rs/release-plz.toml's two sections and return them as ([workspace], [[package]]).

    `cfg.get(key, default)`, NOT `cfg.get(key) or default`. TOML has no null, so a present key
    always carries a non-None value; the explicit default is what routes `workspace = []` to the
    isinstance check below instead of silently substituting `{}` past it. That substitution was
    the whole defect (SMA-608).

    The list check and the element loop are DELIBERATELY separate statements. Fused as
    `isinstance(packages, list) and all(...)`, neutering the list half would also disable the
    element half, and the negative control's mutation would land on a different failure than the
    one it names.

    Every raise is InconclusiveError, which BUILDS. Nothing here may produce a skip.
    """
    workspace = cfg.get("workspace", {})
    if not isinstance(workspace, dict):
        raise InconclusiveError(
            f"rs/release-plz.toml's [workspace] is not a table "
            f"(got {type(workspace).__name__})")

    packages = cfg.get("package", [])
    if not isinstance(packages, list):
        raise InconclusiveError(
            f"rs/release-plz.toml's [[package]] is not an array of tables "
            f"(got {type(packages).__name__})")

    seen: dict[str, int] = {}
    for i, entry in enumerate(packages):
        if not isinstance(entry, dict):
            raise InconclusiveError(
                f"rs/release-plz.toml's [[package]] entry at index {i} is not a table "
                f"(got {type(entry).__name__})")
        name = entry.get("name")
        if not isinstance(name, str) or not name:
            raise InconclusiveError(
                f"rs/release-plz.toml's [[package]] entry at index {i} has no string name")
        if name in seen:
            raise InconclusiveError(
                f"rs/release-plz.toml declares [[package]] name {name!r} twice "
                f"(entries {seen[name]} and {i}); the entry map keeps the LAST, so a duplicate "
                f"carrying release = false silently drops that crate and SKIPS its release")
        seen[name] = i

    return workspace, packages


def assert_default_tag_format(workspace: dict, packages: list[dict]) -> None:
    """Refuse a `git_tag_name` override anywhere: tag_for() assumes release-plz's default.

    Takes ALREADY-VALIDATED sections from config_sections(). It carries no `or {}` and no
    isinstance guard of its own, because those were the bypasses — a guard that substitutes a
    default for a malformed value cannot tell "absent" from "wrong shape" (SMA-608).
    """
    if "git_tag_name" in workspace:
        raise InconclusiveError("rs/release-plz.toml sets [workspace] git_tag_name; tag_for() assumes "
                           "release-plz's default <package>-v<version>")
    for pkg in packages:
        if "git_tag_name" in pkg:
            raise InconclusiveError(f"rs/release-plz.toml sets git_tag_name on "
                               f"{pkg.get('name')!r}; tag_for() assumes the default format")


def workspace_members(rs_root: Path) -> list[str]:
    """`[workspace] members` from rs/Cargo.toml, verbatim.

    THE MEMBER SET IS DERIVED, NOT GUESSED. This used to be a hardcoded `crates/*/*/Cargo.toml`
    glob, which is what the workspace happens to declare today. A publishable member outside that
    exact shape — `tools/x`, or one directory deeper — was invisible: no tag was ever demanded for
    it, so a release with its tag still uncut read as "every releasable package is already tagged"
    and SKIPPED. `--assert`'s strict-equality pin could not catch it either, because both sides of
    that comparison derive from this one function.

    Every failure here is `InconclusiveError`, which BUILDS.
    """
    cfg = load_toml(rs_root / "Cargo.toml")
    ws = cfg.get("workspace")
    if not isinstance(ws, dict):
        raise InconclusiveError(f"{rs_root}/Cargo.toml declares no [workspace] table")
    # `exclude` SHRINKS the member set, and this function does not model it. Reading it as absent
    # would over-derive — demanding a tag for a non-member, which only ever BUILDS, so it is
    # fail-safe at runtime — but it would also make the skip permanently unreachable, silently.
    # Refusing loudly is the conscious-re-baseline direction this repo prefers: at runtime it
    # builds, and under --assert it exits 3 and reds check 11 until somebody teaches this
    # function about exclusion.
    excluded = ws.get("exclude")
    if isinstance(excluded, list) and excluded:
        raise InconclusiveError(f"{rs_root}/Cargo.toml sets [workspace] exclude={excluded!r}; "
                           "workspace_members() does not model member exclusion")
    members = ws.get("members")
    if not isinstance(members, list) or not members or \
            not all(isinstance(m, str) and m for m in members):
        raise InconclusiveError(f"{rs_root}/Cargo.toml has no usable [workspace] members list "
                           f"(got {members!r})")
    return members


def crate_manifests(rs_root: Path) -> dict[str, Path]:
    """Map package name -> Cargo.toml, over every `[workspace] members` entry.

    Needs no cargo and no network: Cargo's member patterns are plain path globs, so `Path.glob`
    expands them. A pattern that matches no manifest, or a literal member with no manifest on
    disk, is InconclusiveError — the tree moved, and guessing would under-derive.
    """
    found: dict[str, Path] = {}
    for pattern in workspace_members(rs_root):
        if any(ch in pattern for ch in "*?["):
            hits = sorted(rs_root.glob(f"{pattern}/Cargo.toml"))
        else:
            literal = rs_root / pattern / "Cargo.toml"
            hits = [literal] if literal.is_file() else []
        if not hits:
            raise InconclusiveError(f"workspace member {pattern!r} matched no Cargo.toml under "
                               f"{rs_root} — the tree moved")
        for manifest in hits:
            pkg = load_toml(manifest).get("package") or {}
            name = pkg.get("name")
            if not isinstance(name, str) or not name:
                continue
            if name in found and found[name] != manifest:
                raise InconclusiveError(
                    f"two manifests declare package {name!r}: {found[name]}, {manifest}")
            found[name] = manifest
    if not found:
        raise InconclusiveError(f"no crate manifests under {rs_root} — the tree moved")
    return found


def releasable_packages(rs_root: Path) -> dict[str, str]:
    """Package -> literal version, for every package release-plz would TAG.

    A package is tagged when Cargo does not say `publish = false` AND rs/release-plz.toml says
    neither `release = false` nor `publish = false`. An ABSENT release-plz entry reads as
    release = true / publish = true, which is release-plz's own default — so an unlisted crate
    counts as releasable and its missing tag makes us BUILD. That is the fail-safe direction.
    """
    cfg = load_toml(rs_root / "release-plz.toml")
    workspace, package_entries = config_sections(cfg)
    assert_default_tag_format(workspace, package_entries)
    # BOTH filters are retained verbatim even though config_sections now asserts both
    # properties, but MEASURED (SMA-608 Task 3), the two belts do not fail the same way when
    # their config_sections check is neutered. With the ELEMENT check neutered, a non-dict entry
    # reaches config_sections's own `entry.get("name")` first and raises AttributeError there
    # (e.g. `"a".get` for `package = ["a"]`) — before this comprehension ever runs. With the NAME
    # check neutered, this comprehension's `isinstance(p.get("name"), str)` clause filters the
    # nameless/duplicate entry OUT before `p["name"]` is evaluated: no exception here at all: the
    # entry is silently dropped, and releasable_packages falls through to an unrelated
    # InconclusiveError further down (crate_manifests can't find rs/Cargo.toml in the fixture
    # tree). So this comprehension's own typed-failure belt is real only for the element check;
    # for the name check it is a silent filter, not a raise. Do not "simplify" either filter away
    # because config_sections looks like it makes them unreachable — that is the point.
    entries = {p["name"]: p for p in package_entries
               if isinstance(p, dict) and isinstance(p.get("name"), str)}

    out: dict[str, str] = {}
    for name, manifest in crate_manifests(rs_root).items():
        pkg = load_toml(manifest).get("package") or {}
        if pkg.get("publish") is False:
            continue
        entry = entries.get(name, {})
        if entry.get("release") is False or entry.get("publish") is False:
            continue
        version = pkg.get("version")
        if not isinstance(version, str):
            # `version.workspace = true` parses as a dict. There is no literal to tag against.
            raise InconclusiveError(f"{name} has no literal [package] version in {manifest}")
        out[name] = version
    return out


def repo_tags(repo_root: Path) -> set[str]:
    try:
        proc = subprocess.run(["git", "-C", str(repo_root), "tag", "-l"],
                              capture_output=True, text=True, check=True)
    except (OSError, subprocess.CalledProcessError) as exc:
        raise InconclusiveError(f"git tag -l failed: {exc}") from exc
    return {line.strip() for line in proc.stdout.splitlines() if line.strip()}


def run(repo_root: Path, event_name: str) -> tuple[bool, str]:
    """The runtime path. It must NEVER traceback: `--github-output` wraps this call and its
    fail-safe is "warn and build", not "crash and let the caller decide". `InconclusiveError` is the
    expected collection failure. `workspace = 3` in `rs/release-plz.toml` USED TO raise a bare
    `TypeError` from inside `assert_default_tag_format`'s `"git_tag_name" in (...)` membership
    test; SMA-608 types that shape — `config_sections`'s `isinstance(workspace, dict)` check now
    raises `InconclusiveError` for it before `assert_default_tag_format` is ever reached. Catching
    `Exception` here, and ONLY here, is what still stands as the floor for the RESIDUAL: shapes
    this module does not model. It is not decoration — `_untyped_collection_failure_builds` is a
    fixture, MEASURED against a crate manifest holding `package = 3`, that reds if this catch is
    narrowed to `except InconclusiveError`. `--assert` and `--self-test` deliberately do not call
    `run()` and must keep surfacing errors loudly.
    """
    try:
        packages = releasable_packages(repo_root / "rs")
        tags = repo_tags(repo_root)
    except Exception as exc:  # deliberately broad; see the docstring above.
        return False, f"inconclusive ({type(exc).__name__}: {exc}) — build"
    return decide(event_name, packages, tags)


# --- The fixture table -------------------------------------------------------------------------

# (label, event_name, packages, tags, expected verdict)
FIXTURES: list[tuple[str, str, dict[str, str], set[str], bool]] = [
    ("every releasable package is tagged -> skip", "push",
     {"a": "1.0.0", "b": "1.0.0"}, {"a-v1.0.0", "b-v1.0.0"}, True),
    ("one tag missing -> build", "push",
     {"a": "1.0.0", "b": "1.0.0"}, {"a-v1.0.0"}, False),
    ("every tag missing -> build", "push",
     {"a": "1.0.1"}, {"a-v1.0.0"}, False),
    # M6's exact shape: the kernel group bumped, the proto group already tagged.
    ("a kernel-only bump -> build (M6)", "push",
     {"paigasus-kernel": "0.1.1", "paigasus-proto": "0.1.0", "paigasus-proto-derive": "0.1.0"},
     {"paigasus-kernel-v0.1.0", "paigasus-proto-v0.1.0", "paigasus-proto-derive-v0.1.0"}, False),
    ("the repo has no tags at all -> build", "push", {"a": "1.0.0"}, set(), False),
    ("no releasable package resolved -> build", "push", {}, {"a-v1.0.0"}, False),
    # A dispatch ALWAYS builds, even in the state that would otherwise skip.
    ("workflow_dispatch with every tag present -> build", "workflow_dispatch",
     {"a": "1.0.0"}, {"a-v1.0.0"}, False),
    ("schedule with every tag present -> build", "schedule",
     {"a": "1.0.0"}, {"a-v1.0.0"}, False),
    # A prefix collision must not read as a hit.
    ("a tag that only PREFIXES the wanted one -> build", "push",
     {"a": "1.0.0"}, {"a-v1.0.0-rc1"}, False),
]


def _missing_config_is_inconclusive() -> str | None:
    """A tree with crate manifests but no rs/release-plz.toml must be InconclusiveError.

    `load_toml` is the first thing `releasable_packages` calls, and a missing file raises
    `FileNotFoundError`, an `OSError` subclass, which `load_toml` already converts.

    The `except` below matches the SPECIFIC message `load_toml` raises for an unreadable file
    ("cannot read ... release-plz.toml"), not any `InconclusiveError` whatsoever. A bare
    `except InconclusiveError: return None` would also accept `crate_manifests`' unrelated "no crate
    manifests" InconclusiveError, which this tree cannot even reach — this crate dir exists — but a
    future refactor could make it reachable, and a helper that accepts any cause proves nothing
    about the ONE cause it claims to test (MEASURED — see I2 in the fix-round report: this is
    exactly the shape that let a neutered `assert_default_tag_format` pass unnoticed in the
    sibling helper below).
    """
    tmp = tempfile.mkdtemp()
    try:
        rs_root = Path(tmp) / "rs"
        crate_dir = rs_root / "crates" / "libs" / "a"
        crate_dir.mkdir(parents=True)
        (crate_dir / "Cargo.toml").write_text('[package]\nname = "a"\nversion = "1.0.0"\n')
        try:
            releasable_packages(rs_root)
        except InconclusiveError as exc:
            if "cannot read" in str(exc) and "release-plz.toml" in str(exc):
                return None
            return (f"releasable_packages raised InconclusiveError for the wrong reason: {exc!r} "
                    f"(expected a 'cannot read ... release-plz.toml' message)")
        return "releasable_packages did not raise InconclusiveError for a missing release-plz.toml"
    finally:
        shutil.rmtree(tmp)


def _workspace_version_is_inconclusive() -> str | None:
    """`version.workspace = true` parses as a dict, not a literal string, and must be
    InconclusiveError rather than silently treated as absent.

    Matches on the specific "no literal [package] version" message `releasable_packages` raises
    for this exact cause, for the same reason `_missing_config_is_inconclusive` does: a bare
    `except InconclusiveError` proves only that SOMETHING failed, not that THIS check fired.
    """
    tmp = tempfile.mkdtemp()
    try:
        rs_root = Path(tmp) / "rs"
        (rs_root).mkdir()
        (rs_root / "release-plz.toml").write_text("")
        (rs_root / "Cargo.toml").write_text('[workspace]\nmembers = ["crates/*/*"]\n')
        crate_dir = rs_root / "crates" / "libs" / "a"
        crate_dir.mkdir(parents=True)
        (crate_dir / "Cargo.toml").write_text(
            '[package]\nname = "a"\nversion.workspace = true\npublish = true\n')
        try:
            releasable_packages(rs_root)
        except InconclusiveError as exc:
            if "no literal [package] version" in str(exc):
                return None
            return (f"releasable_packages raised InconclusiveError for the wrong reason: {exc!r} "
                    f"(expected a message naming 'no literal [package] version')")
        return "releasable_packages did not raise InconclusiveError for a workspace-inherited version"
    finally:
        shutil.rmtree(tmp)


def _tag_name_override_is_inconclusive() -> str | None:
    """A `[workspace] git_tag_name` override invalidates `tag_for`'s default-format assumption,
    and must be InconclusiveError rather than silently tagged the usual way.

    This tree deliberately has NO `rs/Cargo.toml` at all — `assert_default_tag_format`
    must raise before `releasable_packages` ever reaches `crate_manifests`. MEASURED: with a
    bare `except InconclusiveError: return None`, neutering `assert_default_tag_format`'s body to a
    no-op `return` made THIS helper keep passing, because `crate_manifests` then calls
    `workspace_members` -> `load_toml(rs_root / "Cargo.toml")` on a tree with no such file, and
    that raises its own, unrelated "cannot read .../rs/Cargo.toml: ..." InconclusiveError — which
    the bare except accepted too. Matching on "git_tag_name" specifically is what makes that
    mutation visible: neutering the function under test now removes the ONLY source of a
    "git_tag_name" message, so this helper reports the wrong-reason string instead of None.
    """
    tmp = tempfile.mkdtemp()
    try:
        rs_root = Path(tmp) / "rs"
        rs_root.mkdir()
        (rs_root / "release-plz.toml").write_text(
            '[workspace]\ngit_tag_name = "v{{ version }}"\n')
        try:
            releasable_packages(rs_root)
        except InconclusiveError as exc:
            if "git_tag_name" in str(exc):
                return None
            return (f"releasable_packages raised InconclusiveError for the wrong reason: {exc!r} "
                    f"(expected a message naming git_tag_name)")
        return "releasable_packages did not raise InconclusiveError for a git_tag_name override"
    finally:
        shutil.rmtree(tmp)


def _member_outside_crates_is_seen() -> str | None:
    """A publishable workspace member OUTSIDE `crates/*/*` must still be demanded a tag.

    This is the SMA-603 fix-wave finding 2e, as a fixture. `crate_manifests` used to glob a
    hardcoded `crates/*/*/Cargo.toml`; a member declared anywhere else was invisible, its tag was
    never demanded, and a real release read as "everything is tagged" — a SILENT SKIP, the one
    failure direction this checker exists to prevent. `--assert`'s strict-equality pin cannot
    catch that, because both sides of its comparison come from this same function.
    """
    tmp = tempfile.mkdtemp()
    try:
        rs_root = Path(tmp) / "rs"
        rs_root.mkdir()
        (rs_root / "release-plz.toml").write_text("")
        (rs_root / "Cargo.toml").write_text(
            '[workspace]\nmembers = ["crates/*/*", "tools/*"]\n')
        for rel, name in (("crates/libs/a", "a"), ("tools/t", "t")):
            d = rs_root / rel
            d.mkdir(parents=True)
            (d / "Cargo.toml").write_text(
                f'[package]\nname = "{name}"\nversion = "1.0.0"\npublish = true\n')
        got = releasable_packages(rs_root)
        if got != {"a": "1.0.0", "t": "1.0.0"}:
            return (f"releasable_packages returned {got!r}; the `tools/*` member is missing, so "
                    f"its tag would never be demanded and a release would read as complete")
        return None
    except InconclusiveError as exc:  # pragma: no cover - a regression would surface here
        return f"releasable_packages raised InconclusiveError for a valid tree: {exc!r}"
    finally:
        shutil.rmtree(tmp)


def _unresolvable_member_is_inconclusive() -> str | None:
    """A `[workspace] members` entry that matches no Cargo.toml must be InconclusiveError.

    InconclusiveError BUILDS, which is the fail-safe direction. The alternative — quietly deriving a
    smaller package set — is exactly the silent skip `_member_outside_crates_is_seen` describes.
    """
    tmp = tempfile.mkdtemp()
    try:
        rs_root = Path(tmp) / "rs"
        rs_root.mkdir()
        (rs_root / "release-plz.toml").write_text("")
        (rs_root / "Cargo.toml").write_text(
            '[workspace]\nmembers = ["crates/*/*", "tools/*"]\n')
        d = rs_root / "crates" / "libs" / "a"
        d.mkdir(parents=True)
        (d / "Cargo.toml").write_text('[package]\nname = "a"\nversion = "1.0.0"\n')
        try:
            releasable_packages(rs_root)
        except InconclusiveError as exc:
            if "matched no Cargo.toml" in str(exc):
                return None
            return (f"releasable_packages raised InconclusiveError for the wrong reason: {exc!r} "
                    f"(expected a message naming 'matched no Cargo.toml')")
        return "releasable_packages did not raise InconclusiveError for an unresolvable member"
    finally:
        shutil.rmtree(tmp)


def _malformed_config_asserts_three() -> str | None:
    """A malformed `rs/release-plz.toml` must make `--assert` exit 3, not 1.

    MEASURED before the SMA-603 fix wave: `workspace = 3` raised a bare `TypeError` out of
    `assert_default_tag_format`, `_assert_repo`'s `except InconclusiveError` did not catch it, the
    interpreter exited 1 with a traceback, and `run.sh`'s `run_checker` mapped that 1 onto
    `die_infra` (2). Check 11 then reported "uv or the interpreter failed" for what is plainly a
    broken repository file, and README.md's "never 1" claim was false.

    SMA-608 types the `workspace = 3` shape itself: `config_sections` now raises
    `InconclusiveError` for it before `assert_default_tag_format` runs, so this row's own fixture
    no longer exercises the untyped path it was written against. It still asserts the 3 contract
    end to end (a malformed config -> `_assert_repo` -> exit 3) and is kept for that. The
    untyped-failure coverage this row used to be the only source of moved to two new rows,
    `_untyped_collection_failure_asserts_three` and `_untyped_collection_failure_builds`.
    """
    tmp = tempfile.mkdtemp()
    try:
        rs_root = Path(tmp) / "rs"
        crate_dir = rs_root / "crates" / "libs" / "a"
        crate_dir.mkdir(parents=True)
        (rs_root / "Cargo.toml").write_text('[workspace]\nmembers = ["crates/*/*"]\n')
        (rs_root / "release-plz.toml").write_text("workspace = 3\n")
        (crate_dir / "Cargo.toml").write_text('[package]\nname = "a"\nversion = "1.0.0"\n')
        # _assert_repo prints its diagnosis to stderr; a passing self-test must stay quiet.
        with contextlib.redirect_stderr(io.StringIO()):
            rc = _assert_repo(Path(tmp))
        if rc != 3:
            return f"_assert_repo returned {rc} for a malformed release-plz.toml, expected 3"
        return None
    finally:
        shutil.rmtree(tmp)


def _shape_fixture(toml_text: str, marker: str, what: str) -> str | None:
    """Assert releasable_packages raises InconclusiveError whose message carries `marker`.

    Matching the SPECIFIC marker, never a bare `except InconclusiveError`, is the lesson
    _tag_name_override_is_inconclusive's docstring records as MEASURED: a bare except also
    accepts an unrelated InconclusiveError raised further down the call chain, so neutering the
    function under test leaves the helper passing. Each marker below is verified mutually
    non-overlapping by _markers_are_mutually_exclusive.
    """
    tmp = tempfile.mkdtemp()
    try:
        rs_root = Path(tmp) / "rs"
        rs_root.mkdir()
        (rs_root / "release-plz.toml").write_text(toml_text)
        try:
            releasable_packages(rs_root)
        except InconclusiveError as exc:
            if marker in str(exc):
                return None
            return (f"releasable_packages raised InconclusiveError for the wrong reason: {exc!r} "
                    f"(expected a message naming {marker!r})")
        return f"releasable_packages did not raise InconclusiveError for {what}"
    finally:
        shutil.rmtree(tmp)


def _workspace_not_a_table_is_inconclusive() -> str | None:
    """`workspace = []` — the FALSY shape. `or {}` substituted a fresh dict and the membership
    test was vacuously false, so the guard passed having asserted nothing (SMA-608)."""
    return _shape_fixture("workspace = []\n", "[workspace] is not a table",
                          "a non-table [workspace]")


def _workspace_array_of_tables_is_inconclusive() -> str | None:
    """`[[workspace]]` — the TRUTHY wrong container. MEASURED: `[{'git_tag_name': 'x'}]` is
    truthy so `or {}` did not substitute, and `'git_tag_name' in [{...}]` compares against the
    dict as an ELEMENT and is False. The issue named two bypasses; this is the third."""
    return _shape_fixture("[[workspace]]\ngit_tag_name = 'v{{ version }}'\n",
                          "[workspace] is not a table", "an array-of-tables [workspace]")


def _package_not_an_array_of_tables_is_inconclusive() -> str | None:
    """`package = { ... }` — a table, not an array of tables. Iterating a dict yields its KEYS
    as strings, so the old `isinstance(pkg, dict)` guard skipped every one (SMA-608)."""
    return _shape_fixture('package = { name = "a" }\n', "is not an array of tables",
                          "a table-valued package section")


def _package_entry_not_a_table_is_inconclusive() -> str | None:
    """An array of tables holding something that is not a table."""
    return _shape_fixture('package = ["a"]\n', "entry at index 0 is not a table",
                          "a non-table [[package]] entry")


def _nameless_package_entry_is_inconclusive() -> str | None:
    """A `[[package]]` entry with no `name` loses its author's intent SILENTLY.

    The old filter dropped it, so a block meaning `release = false` was discarded: the crate it
    meant to exempt stayed in `out`, was permanently demanded a tag release-plz will never cut,
    and the skip became unreachable without anybody being told. The direction is fail-safe (it
    BUILDS), which is why this was nearly carved out — but workspace_members refuses
    `[workspace] exclude` outright for the structurally identical reason, in this same file.
    Two shapes with one structure do not get two policies (SMA-608).
    """
    return _shape_fixture('[[package]]\nrelease = false\n', "has no string name",
                          "a [[package]] entry with no name")


def _duplicate_package_name_is_inconclusive() -> str | None:
    """A repeated `[[package]] name` is the ONE shape found whose direction is a SKIP.

    MEASURED: `{p["name"]: p for p in entries}` keeps the LAST entry, so a duplicate carrying
    `release = false` drops that crate from `out`. No tag is ever demanded for it, and if the
    other packages' tags exist, decide() returns True — a real release skipped, silently.
    crate_manifests raises on duplicate MANIFESTS; nothing raised on duplicate release-plz
    ENTRIES, and the runtime path never consults EXPECTED_RELEASABLE (SMA-608).
    """
    return _shape_fixture(
        '[[package]]\nname = "a"\nrelease = true\n[[package]]\nname = "a"\nrelease = false\n',
        "declares [[package]] name", "a duplicated [[package]] name")


def _broken_crate_manifest_tree(tmp: str) -> Path:
    """A tree whose collection fails with an UNTYPED exception.

    MEASURED: crate_manifests reads `load_toml(manifest).get("package") or {}`, which yields the
    int 3, then calls `3.get("name")` -> AttributeError: 'int' object has no attribute 'get'.
    Only a broad `except Exception` converts that. Everything else here is well-formed, so the
    failure is unambiguously the one this fixture names.
    """
    rs_root = Path(tmp) / "rs"
    crate_dir = rs_root / "crates" / "libs" / "a"
    crate_dir.mkdir(parents=True)
    (rs_root / "Cargo.toml").write_text('[workspace]\nmembers = ["crates/*/*"]\n')
    (rs_root / "release-plz.toml").write_text("")
    (crate_dir / "Cargo.toml").write_text("package = 3\n")
    return rs_root


def _untyped_collection_failure_asserts_three() -> str | None:
    """_assert_repo's broad `except Exception` must convert an untyped collection failure to 3.

    This REPLACES the coverage _malformed_config_asserts_three used to provide. That fixture
    exists because `workspace = 3` raised a bare TypeError; SMA-608 types that shape, so after
    the fix NO fixture produced a non-InconclusiveError through collection and the broad catch
    could have been narrowed with --self-test still green.
    """
    tmp = tempfile.mkdtemp()
    try:
        _broken_crate_manifest_tree(tmp)
        with contextlib.redirect_stderr(io.StringIO()) as err:
            rc = _assert_repo(Path(tmp))
        if rc != 3:
            return f"_assert_repo returned {rc} for an untyped collection failure, expected 3"
        if "AttributeError" not in err.getvalue():
            return (f"_assert_repo returned 3 but did not name AttributeError: "
                    f"{err.getvalue()!r} — the broad catch may not be what produced this")
        return None
    finally:
        shutil.rmtree(tmp)


def _untyped_collection_failure_builds() -> str | None:
    """run()'s broad `except Exception` must BUILD rather than raise.

    E8: this catch had NO fixture coverage before or after SMA-608 — no helper called run()
    against a broken tree, and run.sh rows 3/4 point it at well-formed synthetic trees. It is
    the runtime path, so an escape here is a traceback in the release workflow's plan job.
    """
    tmp = tempfile.mkdtemp()
    try:
        _broken_crate_manifest_tree(tmp)
        try:
            nothing, reason = run(Path(tmp), "push")
        except Exception as exc:  # deliberately broad; catching it IS the fixture
            return f"run() raised {type(exc).__name__}: {exc} instead of returning a build verdict"
        if nothing:
            return f"run() reported nothing_to_release for a broken tree: {reason!r} — THIS IS A SKIP"
        if "AttributeError" not in reason:
            return (f"run() built, but its reason {reason!r} does not name AttributeError — "
                    f"the broad catch may not be what produced this")
        return None
    finally:
        shutil.rmtree(tmp)


def _markers_are_mutually_exclusive() -> str | None:
    """Every fixture marker must match exactly ONE of the five malformed-shape messages.

    §3.2's distinctness is load-bearing and easy to break by rewording a message: matching the
    element row on "is not a table" would accept the [workspace] error, and matching it on
    "[[package]] entry" would accept the nameless-entry error. Asserted, not read (M10).
    """
    cases = {
        "[workspace] is not a table": "workspace = []\n",
        "is not an array of tables": 'package = { name = "a" }\n',
        "entry at index 0 is not a table": 'package = ["a"]\n',
        "has no string name": "[[package]]\nrelease = false\n",
        "declares [[package]] name":
            '[[package]]\nname = "a"\n[[package]]\nname = "a"\n',
    }
    messages: dict[str, str] = {}
    for marker, text in cases.items():
        try:
            config_sections(tomllib.loads(text))
        except InconclusiveError as exc:
            messages[marker] = str(exc)
            continue
        return f"config_sections did not raise for the {marker!r} case"
    problems = []
    for marker in cases:
        hits = [m for m, msg in messages.items() if marker in msg]
        if hits != [marker]:
            problems.append(f"{marker!r} also matches {[h for h in hits if h != marker]}")
    return "; ".join(problems) or None


# The collection-layer rows: paths a pure-function fixture cannot reach, because they need a
# filesystem. Module-level so `--collection-count` can count them and so self_test()'s floor
# below has something to floor; the FIXTURES floor's own comment explains why a countable
# table matters.
COLLECTION_ROWS: tuple[tuple[str, Callable[[], str | None]], ...] = (
    ("a missing release-plz.toml is inconclusive", _missing_config_is_inconclusive),
    ("a workspace-inherited version is inconclusive", _workspace_version_is_inconclusive),
    ("a git_tag_name override is inconclusive", _tag_name_override_is_inconclusive),
    ("a member outside crates/*/* is still demanded a tag", _member_outside_crates_is_seen),
    ("an unresolvable workspace member is inconclusive", _unresolvable_member_is_inconclusive),
    ("a malformed release-plz.toml makes --assert exit 3, not 1",
     _malformed_config_asserts_three),
    ("a non-table [workspace] is inconclusive", _workspace_not_a_table_is_inconclusive),
    ("an array-of-tables [workspace] is inconclusive",
     _workspace_array_of_tables_is_inconclusive),
    ("a table-valued package section is inconclusive",
     _package_not_an_array_of_tables_is_inconclusive),
    ("a non-table [[package]] entry is inconclusive",
     _package_entry_not_a_table_is_inconclusive),
    ("a nameless [[package]] entry is inconclusive", _nameless_package_entry_is_inconclusive),
    ("a duplicated [[package]] name is inconclusive", _duplicate_package_name_is_inconclusive),
    ("an untyped collection failure makes --assert exit 3",
     _untyped_collection_failure_asserts_three),
    ("an untyped collection failure makes run() build", _untyped_collection_failure_builds),
    ("the five shape markers are mutually exclusive", _markers_are_mutually_exclusive),
)


def self_test() -> int:
    rc = 0
    # An emptied FIXTURES list makes the loop below run zero times and return 0 — a self-test
    # that silently stops testing anything still reads as a pass. This floor is IN-PROCESS and
    # deliberately duplicated by a second, independent floor in ci/actionlint/run.sh's check 11
    # (`--fixture-count`), which is scheduled separately from this file — this repo's usual idiom
    # for a self-scheduled gate: two copies in two files, not one shared helper, so deleting
    # either one leaves the other standing.
    if len(FIXTURES) < 8:
        print(f"FAIL FIXTURES has only {len(FIXTURES)} row(s); the floor is 8 — "
              "something emptied or gutted the fixture table", file=sys.stderr)
        rc = 3
    for label, event, packages, tags, want in FIXTURES:
        got, reason = decide(event, packages, tags)
        if got != want:
            print(f"FAIL {label!r}: expected {want}, got {got} ({reason})", file=sys.stderr)
            rc = 3

    # EVERY call is wrapped. A helper that raises anything other than a returned error string
    # would otherwise escape main() and exit the interpreter at 1 — which README.md's "0, 2 or 3,
    # never 1" contract forbids, and which run_checker would then map onto die_infra (2),
    # reporting "uv or the interpreter failed" for a broken repository file. MEASURED (SMA-608
    # Task 3, M3 re-run): with config_sections's element-not-a-table check neutered, its own
    # `entry.get("name")` raises AttributeError on a non-dict entry — this wrapper is what turns
    # that into a reported FAIL instead of an interpreter exit at 1. Not every neutered check
    # raises, though: the same task's M9 measurement found neutering the name check raises
    # nothing — releasable_packages's own belt filters the bad entry out instead — so this
    # wrapper is load-bearing for the checks that DO raise, not a blanket guarantee that every
    # mutation does.
    # The same reasoning as the FIXTURES floor above, for the collection rows. Deleting a helper
    # from COLLECTION_ROWS otherwise reds nothing: check 11's --fixture-count floor counts
    # FIXTURES only. Floored below the actual count so a legitimate row removal does not abort
    # the gate as infra. Twinned by check 11's --collection-count floor in ci/actionlint/run.sh,
    # in a separately scheduled file, so one edit cannot remove both.
    if len(COLLECTION_ROWS) < 12:
        print(f"FAIL COLLECTION_ROWS has only {len(COLLECTION_ROWS)} row(s); the floor is 12 — "
              "something emptied or gutted the collection-layer table", file=sys.stderr)
        rc = 3
    for label, fn in COLLECTION_ROWS:
        try:
            err = fn()
        except Exception as exc:  # deliberately broad; see the comment above
            err = f"raised {type(exc).__name__}: {exc}"
        if err:
            print(f"FAIL {label!r}: {err}", file=sys.stderr)
            rc = 3
    return rc


def _assert_repo(repo_root: Path) -> int:
    """--assert. The CI-side assertions; the runtime path uses none of them.

    Both collection calls sit inside the ONE try below, on purpose: `README.md` documents that
    this checker exits 0, 2, or 3 and never 1. `repo_tags()` can raise `InconclusiveError` too (a
    failed `git tag -l`), and if that call sat outside the try, that InconclusiveError would escape
    uncaught, the interpreter would exit 1 with a traceback, and `run_checker` would then map
    that 1 onto its `die_infra` branch (2) — silently breaking the documented contract.

    SMA-603 fix wave, Group 3: the `except` is BROAD for the same reason `run()`'s is, and the
    documented contract is why. MEASURED before that fix: `workspace = 3` in `rs/release-plz.toml`
    raised a bare `TypeError` from inside `assert_default_tag_format`'s membership test, which an
    `except InconclusiveError` did not catch — so `--assert` exited 1 with a traceback, `run_checker`
    mapped that onto `die_infra` (2), and a malformed repository file was reported as
    "infrastructure failed" rather than "the repository is wrong". The README claimed the checker
    could never exit 1; the code, not the doc, was wrong.

    `workspace = 3` no longer raises `TypeError`: SMA-608 types that shape, and `config_sections`
    now raises `InconclusiveError` for it before `assert_default_tag_format` is ever reached. This
    catch stays broad regardless — it is the floor for the RESIDUAL, shapes the validator does not
    model — and it is covered, not decorative: `_untyped_collection_failure_asserts_three`
    (MEASURED against a crate manifest holding `package = 3`) reds if it is narrowed to
    `except InconclusiveError`. Collection reads only repository files, so any failure of it IS a
    statement about the repository. `--self-test` deliberately keeps no such catch: it tests this
    module, not the tree.
    """
    problems: list[str] = []
    try:
        packages = releasable_packages(repo_root / "rs")
        tags = repo_tags(repo_root)
    except Exception as exc:  # deliberately broad; see the docstring above.
        print(f"release-plan: {type(exc).__name__}: {exc}", file=sys.stderr)
        return 3
    derived = frozenset(packages)
    if derived != EXPECTED_RELEASABLE:
        problems.append(
            f"the derived releasable set {sorted(derived)} does not equal the pinned "
            f"EXPECTED_RELEASABLE {sorted(EXPECTED_RELEASABLE)}. If a crate legitimately became "
            f"publishable, re-baseline the pin deliberately — do not loosen the comparison.")
    if not tags:
        problems.append("the repository reports no tags; --assert needs a full checkout")
    for p in problems:
        print(f"release-plan: {p}", file=sys.stderr)
    return 3 if problems else 0


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--self-test", action="store_true")
    ap.add_argument("--assert", dest="do_assert", action="store_true")
    ap.add_argument("--fixture-count", action="store_true")
    ap.add_argument("--collection-count", action="store_true")
    ap.add_argument("--event-name", default="")
    ap.add_argument("repo_root", nargs="?", default=".")
    args = ap.parse_args(argv)

    if args.fixture_count:
        print(len(FIXTURES))
        return 0
    if args.collection_count:
        print(len(COLLECTION_ROWS))
        return 0
    if args.self_test:
        return self_test()
    root = Path(args.repo_root)
    if args.do_assert:
        return _assert_repo(root)

    nothing, reason = run(root, args.event_name)
    print(f"release-plan: {reason}")
    print(f"nothing_to_release={'true' if nothing else 'false'}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
