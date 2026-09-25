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

SMA-688. The image chains come from ci/images/chains.toml, not from a list in this file. A chain
that cannot be read RUNS (the fail-safe direction above). A registry that cannot be read names no
chain at all: `--keys` then exits 3. ci/release-plan/run.sh then reads the keys from the registry
with sed and writes every chain output fail-safe. It fails the plan job only when that read also
finds no key (spec § 4.3).
"""
from __future__ import annotations

import argparse
import contextlib
import io
import json
import re
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
    # (e.g. `"a".get` for `package = ["a"]`) — before this comprehension ever runs, so its own
    # `isinstance(p, dict)` clause never fires for that mutation; it is kept as defense-in-depth.
    # With the NAME check neutered, this comprehension's `isinstance(p.get("name"), str)` clause
    # filters the nameless entry OUT before `p["name"]` is evaluated: no exception here at all:
    # the entry is silently dropped, and releasable_packages falls through to an unrelated
    # InconclusiveError further down (crate_manifests can't find rs/Cargo.toml in the fixture
    # tree). So this comprehension's own typed-failure belt is real only for the name check; for
    # the element check the failure is raised inside config_sections before the comprehension
    # ever runs. Do not "simplify" either filter away because config_sections looks like it makes
    # them unreachable — that is the point.
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


# SMA-688. The chain registry (spec D10). It replaces SMA-658's EXPECTED_SERVICES pin: the keys,
# their kinds, their version files and their changelogs live in ONE file, and four readers use
# it. A registry that cannot be read is inconclusive for EVERY key and never yields an empty key
# list: an empty list writes no chain output, and a chain output that nobody writes runs that
# chain with an empty version.
CHAIN_REGISTRY = Path("ci") / "images" / "chains.toml"
CHAIN_KINDS = frozenset({"cargo", "npm"})
_CHAIN_KEY_RE = re.compile(r"[a-z][a-z0-9-]*")
_CHAIN_FIELDS = ("kind", "version_file", "changelog", "ghcr", "hub")

# SMA-688. The ONE copy in this file of the registry header shape that ci/release-plan/run.sh
# reads with sed when `--keys` fails (spec § 4.3):
#     sed -n 's/^\[chain\.\([a-z][a-z0-9-]*\)\]$/\1/p' "$REPO_ROOT/ci/images/chains.toml"
# That sed line in run.sh's github_output() is the TWIN of this pattern. Change one, and change
# the other in the same commit. _assert_repo compares the keys this pattern finds with the
# tomllib keys. So a header that tomllib reads and the sed read misses (trailing spaces, a
# trailing comment, a quoted key, a dotted key under a bare [chain]) fails --assert on the pull
# request, and the sed fallback can never name only part of the chains.
CHAIN_HEADER_SED_RE = re.compile(r"^\[chain\.([a-z][a-z0-9-]*)\]$")


def sed_chain_keys(text: str) -> list[str]:
    """The keys that run.sh's sed fallback reads from this registry text, in file order.

    Split on "\\n" only, like sed: a CRLF line keeps its "\\r", so neither reader matches it. This
    is only true of `text` if it was decoded from the raw bytes: `Path.read_text()` opens in
    universal-newline mode and silently turns "\\r\\n" into "\\n" before this function ever sees
    it, which would make a CRLF header match here while real sed still rejects it. Every caller of
    this function must pass `path.read_bytes().decode("utf-8")`, never `path.read_text(...)`.
    """
    return [m.group(1) for line in text.split("\n") if (m := CHAIN_HEADER_SED_RE.match(line))]


def release_name(key: str) -> str:
    """`paigasus-<key>`: the git tag prefix and the image title label of a chain (spec § 4.1)."""
    return f"paigasus-{key}"


def chain_registry(repo_root: Path) -> dict[str, dict[str, str]]:
    """key -> entry, in file order. Every failure is InconclusiveError, and nothing returns {}."""
    cfg = load_toml(repo_root / CHAIN_REGISTRY)
    chains = cfg.get("chain")
    if not isinstance(chains, dict) or not chains:
        raise InconclusiveError(f"{CHAIN_REGISTRY} has no [chain.<key>] table")
    out: dict[str, dict[str, str]] = {}
    for key, entry in chains.items():
        if _CHAIN_KEY_RE.fullmatch(key) is None:
            raise InconclusiveError(f"{CHAIN_REGISTRY}: the chain key {key!r} is not [a-z][a-z0-9-]*")
        if not isinstance(entry, dict):
            raise InconclusiveError(f"{CHAIN_REGISTRY}: [chain.{key}] is not a table")
        for field in _CHAIN_FIELDS:
            value = entry.get(field)
            if not isinstance(value, str) or not value:
                raise InconclusiveError(f"{CHAIN_REGISTRY}: [chain.{key}] has no string {field}")
        if entry["kind"] not in CHAIN_KINDS:
            raise InconclusiveError(
                f"{CHAIN_REGISTRY}: [chain.{key}] kind {entry['kind']!r} is not one of "
                f"{sorted(CHAIN_KINDS)}")
        out[key] = {field: entry[field] for field in _CHAIN_FIELDS}
    return out

# PR 2 review, minor: a service version must be exactly MAJOR.MINOR.PATCH. Without this,
# `0.1.0-rc1` was read as an ordinary version, ran the whole chain — GHCR push, both
# attestations, the Docker Hub copy, both signatures — and only failed at the very end, in
# `ci/images/release_decision.py`'s `floating` step, whose own VERSION_RE has the same shape but
# cannot help a release that has already written to two registries. Catching it here, at the one
# place the version is first read, makes the release-images `decide` step's label compare
# (`"${label}" != "${VERSION}"`) reject it BEFORE the first registry write, since an invalid
# version is treated the same as any other inconclusive read (see service_state's docstring): the
# chain still runs (fail-safe), but with an EMPTY plan version, which mismatches the archive's
# real label immediately.
_SERVICE_VERSION_RE = re.compile(r"\d+\.\d+\.\d+")

_CHANGELOG_HEADING = re.compile(r"^##\s+\[?(?P<version>[0-9][^\]\s]*)\]?", re.M)


def changelog_names_version(text: str, version: str) -> bool:
    """True when a `## [<version>]` heading names exactly this version.

    The comparison is on the captured version STRING, not a prefix test: `## [0.1.01]` must not
    satisfy a 0.1.0 release, and prose that merely contains the number must not either.
    """
    return any(m.group("version") == version for m in _CHANGELOG_HEADING.finditer(text))


def service_skips(version: str, key: str, tags: set[str]) -> bool:
    """Spec § 6.1 (SMA-658). Skip when there is no real version yet, or when the tag exists.

    SMA-688: the tag name comes from release_name(key), the one place that builds it. The test is
    set MEMBERSHIP of the whole tag name, never a prefix test: `iam` is a string prefix of
    `iam-console`, so a prefix test would read one chain's tag as the other's.
    """
    if version == "0.0.0":
        return True
    return tag_for(release_name(key), version) in tags


def cargo_version(repo_root: Path, key: str, entry: dict[str, str]) -> str:
    """The SMA-658 cargo reader: the crate comes from the workspace members, by its release name.

    SMA-688 adds one check. The registry's version_file must be that same manifest, because
    ci/helm-render reads version_file directly. Two readers that read two files would let the
    chart tag and the release plan disagree.
    """
    manifest = crate_manifests(repo_root / "rs")[release_name(key)]
    if manifest.resolve() != (repo_root / entry["version_file"]).resolve():
        raise InconclusiveError(
            f"{CHAIN_REGISTRY} names {entry['version_file']} for {key}, but the workspace "
            f"resolves {release_name(key)} to {manifest}")
    pkg = load_toml(manifest).get("package") or {}
    version = pkg.get("version")
    if not isinstance(version, str):
        raise InconclusiveError(f"{release_name(key)} has no literal [package] version in {manifest}")
    return version


def npm_version(repo_root: Path, key: str, entry: dict[str, str]) -> str:
    """SMA-688. The top-level `version` string of a console's package.json (spec D1)."""
    path = repo_root / entry["version_file"]
    try:
        doc = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise InconclusiveError(f"cannot read {path}: {exc}") from exc
    version = doc.get("version") if isinstance(doc, dict) else None
    if not isinstance(version, str):
        raise InconclusiveError(
            f"{path} has no top-level string version for {key} (got {type(version).__name__})")
    return version


def service_state(repo_root: Path, tags: set[str]) -> dict[str, tuple[bool, str]]:
    """chain key -> (skip, version). A SEPARATE FAILURE DOMAIN from the kernel verdict.

    Each key is read in its own try. A failure for one key writes skip=False for that key — the
    fail-safe direction, because spec § 4.3 step 3 (SMA-658) makes a run for an already released
    version a no-op — and leaves the other keys and the kernel verdict untouched.

    SMA-688: the keys come from ci/images/chains.toml, and `repo_root` is the repository root,
    no longer `rs/`. When the registry itself cannot be read, this returns {} and says so on
    stderr: it cannot name a key, so it writes none. ci/release-plan/run.sh asks `--keys` first.
    In that state it reads the keys from the registry with sed and writes every chain output
    fail-safe, and it fails the plan job only when the sed read finds no key either.
    """
    try:
        registry = chain_registry(repo_root)
    except Exception as exc:  # deliberately broad; see the docstring above.
        print(f"release-plan: the chain registry is inconclusive ({type(exc).__name__}: {exc}) "
              f"— no chain output can be named", file=sys.stderr)
        return {}
    readers = {"cargo": cargo_version, "npm": npm_version}
    out: dict[str, tuple[bool, str]] = {}
    for key, entry in registry.items():
        try:
            version = readers[entry["kind"]](repo_root, key, entry)
            if version != "0.0.0" and _SERVICE_VERSION_RE.fullmatch(version) is None:
                raise InconclusiveError(
                    f"{release_name(key)}'s version {version!r} in {entry['version_file']} "
                    f"is not MAJOR.MINOR.PATCH")
            out[key] = (service_skips(version, key, tags), version)
        except Exception as exc:  # deliberately broad; see the docstring above.
            print(f"release-plan: {key} is inconclusive ({type(exc).__name__}: {exc}) — run",
                  file=sys.stderr)
            out[key] = (False, "")
    return out


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

    SMA-688: the tree carries the chain registry. Collection fails first, so `_assert_repo`
    returns 3 before it reads the registry. Mutation this row proves: remove the `try` around
    collection in _assert_repo, and the helper sees a traceback, not 3.
    """
    tmp = tempfile.mkdtemp()
    try:
        rs_root = Path(tmp) / "rs"
        crate_dir = rs_root / "crates" / "libs" / "a"
        crate_dir.mkdir(parents=True)
        _write_chain_fixture(Path(tmp))
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
    function under test leaves the helper passing. Every marker and TOML text passed in below is
    drawn from SHAPE_FIXTURES — the same mapping _markers_are_mutually_exclusive iterates — so the
    two checks cannot drift apart the way a hand-typed second copy could (SMA-608 final fix wave,
    Important 2).
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


# Single source for every malformed-shape fixture below AND for _markers_are_mutually_exclusive's
# distinctness check (SMA-608 final fix wave, Important 2). Before this, the two only coincided:
# _markers_are_mutually_exclusive held its own hand-typed `cases` dict, so shortening a fixture's
# marker (e.g. "entry at index 0 is not a table" to "is not a table") left the fixture AND that
# check both green while the fixture became satisfiable by an unrelated [workspace] error. Keyed
# by marker; each value is the ordered tuple of (malformed TOML text, description) fixtures that
# marker covers. Two entries share the "[workspace] is not a table" marker on purpose — a
# non-table and an array-of-tables [workspace] both fail the same isinstance check with the same
# message, so config_sections()'s five ASSERTIONS map onto six shape FIXTURES below.
SHAPE_FIXTURES: dict[str, tuple[tuple[str, str], ...]] = {
    "[workspace] is not a table": (
        ("workspace = []\n", "a non-table [workspace]"),
        ("[[workspace]]\ngit_tag_name = 'v{{ version }}'\n", "an array-of-tables [workspace]"),
    ),
    "is not an array of tables": (
        ('package = { name = "a" }\n', "a table-valued package section"),
    ),
    "entry at index 0 is not a table": (
        ('package = ["a"]\n', "a non-table [[package]] entry"),
    ),
    "has no string name": (
        ('[[package]]\nrelease = false\n', "a [[package]] entry with no name"),
    ),
    "declares [[package]] name": (
        ('[[package]]\nname = "a"\nrelease = true\n[[package]]\nname = "a"\nrelease = false\n',
         "a duplicated [[package]] name"),
    ),
}


def _workspace_not_a_table_is_inconclusive() -> str | None:
    """`workspace = []` — the FALSY shape. `or {}` substituted a fresh dict and the membership
    test was vacuously false, so the guard passed having asserted nothing (SMA-608)."""
    marker = "[workspace] is not a table"
    toml_text, what = SHAPE_FIXTURES[marker][0]
    return _shape_fixture(toml_text, marker, what)


def _workspace_array_of_tables_is_inconclusive() -> str | None:
    """`[[workspace]]` — the TRUTHY wrong container. MEASURED: `[{'git_tag_name': 'x'}]` is
    truthy so `or {}` did not substitute, and `'git_tag_name' in [{...}]` compares against the
    dict as an ELEMENT and is False. The issue named two bypasses; this is the third."""
    marker = "[workspace] is not a table"
    toml_text, what = SHAPE_FIXTURES[marker][1]
    return _shape_fixture(toml_text, marker, what)


def _package_not_an_array_of_tables_is_inconclusive() -> str | None:
    """`package = { ... }` — a table, not an array of tables. Iterating a dict yields its KEYS
    as strings, so the old `isinstance(pkg, dict)` guard skipped every one (SMA-608)."""
    marker = "is not an array of tables"
    toml_text, what = SHAPE_FIXTURES[marker][0]
    return _shape_fixture(toml_text, marker, what)


def _package_entry_not_a_table_is_inconclusive() -> str | None:
    """An array of tables holding something that is not a table."""
    marker = "entry at index 0 is not a table"
    toml_text, what = SHAPE_FIXTURES[marker][0]
    return _shape_fixture(toml_text, marker, what)


def _nameless_package_entry_is_inconclusive() -> str | None:
    """A `[[package]]` entry with no `name` loses its author's intent SILENTLY.

    The old filter dropped it, so a block meaning `release = false` was discarded: the crate it
    meant to exempt stayed in `out`, was permanently demanded a tag release-plz will never cut,
    and the skip became unreachable without anybody being told. The direction is fail-safe (it
    BUILDS), which is why this was nearly carved out — but workspace_members refuses
    `[workspace] exclude` outright for the structurally identical reason, in this same file.
    Two shapes with one structure do not get two policies (SMA-608).
    """
    marker = "has no string name"
    toml_text, what = SHAPE_FIXTURES[marker][0]
    return _shape_fixture(toml_text, marker, what)


def _duplicate_package_name_is_inconclusive() -> str | None:
    """A repeated `[[package]] name` is the ONE shape found whose direction is a SKIP.

    MEASURED: `{p["name"]: p for p in entries}` keeps the LAST entry, so a duplicate carrying
    `release = false` drops that crate from `out`. No tag is ever demanded for it, and if the
    other packages' tags exist, decide() returns True — a real release skipped, silently.
    crate_manifests raises on duplicate MANIFESTS; nothing raised on duplicate release-plz
    ENTRIES, and the runtime path never consults EXPECTED_RELEASABLE (SMA-608).
    """
    marker = "declares [[package]] name"
    toml_text, what = SHAPE_FIXTURES[marker][0]
    return _shape_fixture(toml_text, marker, what)


def _broken_crate_manifest_tree(tmp: str) -> Path:
    """A tree whose collection fails with an UNTYPED exception.

    MEASURED: crate_manifests reads `load_toml(manifest).get("package") or {}`, which yields the
    int 3, then calls `3.get("name")` -> AttributeError: 'int' object has no attribute 'get'.
    Only a broad `except Exception` converts that. Everything else here is well-formed, so the
    failure is unambiguously the one this fixture names.

    SMA-688: the tree carries the chain registry, so the only untyped failure is the crate
    manifest's. Mutation proved by its two users: narrow the broad `except Exception` in
    _assert_repo or in run() to `except InconclusiveError`.
    """
    rs_root = Path(tmp) / "rs"
    crate_dir = rs_root / "crates" / "libs" / "a"
    crate_dir.mkdir(parents=True)
    (rs_root / "Cargo.toml").write_text('[workspace]\nmembers = ["crates/*/*"]\n')
    (rs_root / "release-plz.toml").write_text("")
    (crate_dir / "Cargo.toml").write_text("package = 3\n")
    _write_chain_fixture(Path(tmp))
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
    """Every marker in SHAPE_FIXTURES must match exactly ONE of the five malformed-shape
    messages config_sections raises.

    §3.2's distinctness is load-bearing and easy to break by rewording a message: matching the
    element row on "is not a table" would accept the [workspace] error, and matching it on
    "[[package]] entry" would accept the nameless-entry error. Asserted, not read (M10).

    Reads SHAPE_FIXTURES — the same mapping every _shape_fixture() call above draws its marker
    and TOML text from — rather than a second, hand-typed copy. A hand-typed copy is what let
    this check pass while a fixture's marker had already drifted (SMA-608 final fix wave,
    Important 2): the two vocabularies coincided by construction, not by assertion, until now.
    """
    messages: dict[str, str] = {}
    for marker, variants in SHAPE_FIXTURES.items():
        toml_text, _what = variants[0]
        try:
            config_sections(tomllib.loads(toml_text))
        except InconclusiveError as exc:
            messages[marker] = str(exc)
            continue
        return f"config_sections did not raise for the {marker!r} case"
    problems = []
    for marker in SHAPE_FIXTURES:
        hits = [m for m, msg in messages.items() if marker in msg]
        if hits != [marker]:
            problems.append(f"{marker!r} also matches {[h for h in hits if h != marker]}")
    return "; ".join(problems) or None


# SMA-658. A chain is SKIPPED when it has no real version yet, or when its tag already exists.
# Anything else RUNS, which is the fail-safe direction: spec § 4.3 step 3 makes a run for an
# already released version a no-op.
#
# SMA-688: each row names its chain key. `iam` is a string prefix of `iam-console`, so the last
# three rows prove that the tag test compares the WHOLE tag name. Mutation: replace the set
# membership in service_skips with `any(t.startswith(release_name(key)) for t in tags)`, and the
# row "iam at 0.1.0 does not read the iam-console tag" reds.
SERVICE_FIXTURES: list[tuple[str, str, str, set[str], bool]] = [
    ("a version with no tag -> run", "iam", "0.1.0", set(), False),
    ("the tag already exists -> skip", "iam", "0.1.0", {"paigasus-iam-v0.1.0"}, True),
    ("still 0.0.0 -> skip", "iam", "0.0.0", set(), True),
    ("a newer version than the tag -> run", "iam", "0.2.0", {"paigasus-iam-v0.1.0"}, False),
    ("a tag that only PREFIXES the wanted one -> run", "iam", "0.1.0", {"paigasus-iam-v0.1.0-rc1"}, False),
    ("iam at 0.1.0 does not read the iam-console tag -> run", "iam", "0.1.0",
     {"paigasus-iam-console-v0.1.0"}, False),
    ("iam-console does not read the iam tag -> run", "iam-console", "0.1.0",
     {"paigasus-iam-v0.1.0"}, False),
    ("iam-console with its own tag -> skip", "iam-console", "0.1.0",
     {"paigasus-iam-console-v0.1.0"}, True),
]


def _service_fixture_rows() -> str | None:
    for label, key, version, tags, want in SERVICE_FIXTURES:
        got = service_skips(version, key, tags)
        if got != want:
            return f"{label!r}: expected {want}, got {got}"
    return None


def _changelog_reader_rows() -> str | None:
    rows: list[tuple[str, str, str, bool]] = [
        ("a keep-a-changelog heading", "## [0.1.0] - 2026-09-20\n", "0.1.0", True),
        ("a heading with a compare link", "## [0.1.0](https://x/y) - 2026-09-20\n", "0.1.0", True),
        ("a bare heading", "## 0.1.0\n", "0.1.0", True),
        ("only the unreleased section", "## [Unreleased]\n", "0.1.0", False),
        ("another version only", "## [0.2.0] - 2026-09-20\n", "0.1.0", False),
        # A prefix must not read as a hit: 0.1.0 is not named by a 0.1.01 heading.
        ("a longer version that starts with it", "## [0.1.01] - 2026-09-20\n", "0.1.0", False),
        ("the version inside prose, not a heading", "see 0.1.0 below\n", "0.1.0", False),
    ]
    for label, text, version, want in rows:
        got = changelog_names_version(text, version)
        if got != want:
            return f"{label!r}: expected {want}, got {got}"
    return None


# SMA-688. The registry that every synthetic tree below carries: the same four keys and the same
# paths as the real ci/images/chains.toml. A tree that reaches _assert_repo or service_state must
# hold it and both console package.json files at 0.0.0 (spec § 4.2). Without them each console
# adds a "could not be read" problem, and a fixture that expects exactly one problem passes for
# the wrong reason.
_FIXTURE_CHAINS_TOML = """\
[chain.iam]
kind = "cargo"
version_file = "rs/crates/services/paigasus-iam/Cargo.toml"
changelog = "rs/crates/services/paigasus-iam/CHANGELOG.md"
ghcr = "ghcr.io/smk1085/paigasus-iam"
hub = "docker.io/smaschek/paigasus-iam"

[chain.gateway]
kind = "cargo"
version_file = "rs/crates/services/paigasus-gateway/Cargo.toml"
changelog = "rs/crates/services/paigasus-gateway/CHANGELOG.md"
ghcr = "ghcr.io/smk1085/paigasus-gateway"
hub = "docker.io/smaschek/paigasus-gateway"

[chain.iam-console]
kind = "npm"
version_file = "ts/apps/iam-console/package.json"
changelog = "ts/apps/iam-console/CHANGELOG.md"
ghcr = "ghcr.io/smk1085/paigasus-iam-console"
hub = "docker.io/smaschek/paigasus-iam-console"

[chain.gateway-console]
kind = "npm"
version_file = "ts/apps/gateway-console/package.json"
changelog = "ts/apps/gateway-console/CHANGELOG.md"
ghcr = "ghcr.io/smk1085/paigasus-gateway-console"
hub = "docker.io/smaschek/paigasus-gateway-console"
"""
_FIXTURE_CHAINS: dict[str, dict[str, str]] = tomllib.loads(_FIXTURE_CHAINS_TOML)["chain"]


def _write_chain_fixture(repo_root: Path, *, console_versions: dict[str, str] | None = None,
                         console_texts: dict[str, str] | None = None) -> None:
    """Write ci/images/chains.toml and both console package.json files under `repo_root`.

    A console is at 0.0.0 unless `console_versions` names it. `console_texts` replaces the whole
    text of a console's package.json, for the malformed-shape rows.
    """
    (repo_root / CHAIN_REGISTRY).parent.mkdir(parents=True, exist_ok=True)
    (repo_root / CHAIN_REGISTRY).write_text(_FIXTURE_CHAINS_TOML)
    for key in ("iam-console", "gateway-console"):
        path = repo_root / _FIXTURE_CHAINS[key]["version_file"]
        path.parent.mkdir(parents=True, exist_ok=True)
        version = (console_versions or {}).get(key, "0.0.0")
        default = json.dumps({"name": f"@paigasus/{key}", "version": version, "private": True},
                             indent=2) + "\n"
        path.write_text((console_texts or {}).get(key, default))


def _git_commit_and_tag(tmp: str, tags: tuple[str, ...]) -> None:
    """One commit, then each tag in `tags`. Moved here unchanged from
    _service_unparsable_version_asserts_three_tree, so that several trees can share it."""
    subprocess.run(["git", "init", "-q", tmp], check=True)
    subprocess.run(["git", "-C", tmp, "config", "user.email", "release-plan-self-test@example.com"],
                    check=True)
    subprocess.run(["git", "-C", tmp, "config", "user.name", "release-plan self-test"], check=True)
    subprocess.run(["git", "-C", tmp, "add", "-A"], check=True)
    # `-c commit.gpgsign=false` / `-c tag.gpgSign=false`: this repo's global git config signs
    # every commit and tag (1Password-backed SSH signing). A throwaway fixture tree must not
    # depend on that being unlocked, and an unsigned, unannotated tag is all `repo_tags` reads.
    subprocess.run(["git", "-C", tmp, "-c", "commit.gpgsign=false", "commit", "-q", "-m", "init"],
                    check=True)
    for tag in tags:
        subprocess.run(["git", "-C", tmp, "-c", "tag.gpgSign=false", "tag", tag], check=True)


def _complete_chain_tree(tmp: str, *, versions: dict[str, str] | None = None,
                         changelogs: dict[str, str] | None = None,
                         console_texts: dict[str, str] | None = None,
                         tags: tuple[str, ...] = ("unrelated-tag",)) -> Path:
    """A whole repository that `_assert_repo` reads as CLEAN while every chain is at 0.0.0.

    The derived releasable set equals EXPECTED_RELEASABLE, the tree carries a tag, and every chain
    of the fixture registry resolves. So a fixture built on it gets exactly the problem it names
    and no other. `versions` maps a chain key to its version (default 0.0.0). `changelogs` maps a
    chain key to the text of its CHANGELOG.md. `console_texts` replaces a console's package.json.
    """
    all_versions = {"iam": "0.0.0", "gateway": "0.0.0", "iam-console": "0.0.0",
                    "gateway-console": "0.0.0", **(versions or {})}
    repo_root = Path(tmp)
    rs_root = repo_root / "rs"
    for name in ("paigasus-kernel", "paigasus-proto", "paigasus-proto-derive"):
        d = rs_root / "crates" / "libs" / name
        d.mkdir(parents=True)
        (d / "Cargo.toml").write_text(f'[package]\nname = "{name}"\nversion = "1.0.0"\n')
    for key in ("iam", "gateway"):
        d = rs_root / "crates" / "services" / release_name(key)
        d.mkdir(parents=True)
        (d / "Cargo.toml").write_text(
            f'[package]\nname = "{release_name(key)}"\nversion = "{all_versions[key]}"\n'
            f'publish = false\n')
    (rs_root / "Cargo.toml").write_text(
        '[workspace]\nmembers = ["crates/libs/*", "crates/services/*"]\n')
    (rs_root / "release-plz.toml").write_text("")
    _write_chain_fixture(
        repo_root,
        console_versions={k: all_versions[k] for k in ("iam-console", "gateway-console")},
        console_texts=console_texts)
    for key, text in (changelogs or {}).items():
        (repo_root / _FIXTURE_CHAINS[key]["changelog"]).write_text(text)
    _git_commit_and_tag(tmp, tags)
    return repo_root


# S9 (SMA-658). Spec § 6.1's fail-safe direction as a fixture: a service that cannot be read is
# inconclusive for THAT SERVICE alone — never a silent skip, and never cross-talk into the other
# service or into run()'s own kernel verdict. This mirrors _broken_crate_manifest_tree's shape,
# but leaves `paigasus-iam` OUT of `[workspace] members` entirely (rather than malforming its
# manifest), so `crate_manifests(rs_root)["paigasus-iam"]` fails with a plain KeyError that never
# touches the scan for the healthy `paigasus-gateway` crate or for run()'s own kernel packages.
# SMA-688: the tree also carries the chain registry and both consoles at 0.0.0.
def _mixed_service_tree(tmp: str) -> Path:
    rs_root = Path(tmp) / "rs"
    kernel_dir = rs_root / "crates" / "libs" / "paigasus-kernel"
    gateway_dir = rs_root / "crates" / "services" / "paigasus-gateway"
    kernel_dir.mkdir(parents=True)
    gateway_dir.mkdir(parents=True)
    (rs_root / "Cargo.toml").write_text(
        '[workspace]\nmembers = ["crates/libs/*", "crates/services/paigasus-gateway"]\n')
    (rs_root / "release-plz.toml").write_text("")
    (kernel_dir / "Cargo.toml").write_text(
        '[package]\nname = "paigasus-kernel"\nversion = "0.1.0"\n')
    (gateway_dir / "Cargo.toml").write_text(
        '[package]\nname = "paigasus-gateway"\nversion = "0.1.0"\npublish = false\n')
    _write_chain_fixture(Path(tmp))
    return rs_root


def _malformed_service_version_tree(tmp: str) -> Path:
    """A service crate with a version that is not MAJOR.MINOR.PATCH.

    PR 2 review, minor. MEASURED before this check existed: `0.1.0-rc1` read as an ordinary
    version — it is not `0.0.0` and no tag named it yet, so `service_skips` returned False (run)
    — and the chain ran all the way through the GHCR push, both attestations, the Docker Hub copy
    and both signatures before `ci/images/release_decision.py`'s `floating` step finally rejected
    it. `paigasus-gateway` stays healthy, mirroring `_mixed_service_tree`'s shape, to prove the
    rejection stays scoped to `iam` alone.

    SMA-688: the tree carries the chain registry and both consoles at 0.0.0, so the consoles
    read as a clean skip and add nothing to the iam verdict under test.
    """
    rs_root = Path(tmp) / "rs"
    kernel_dir = rs_root / "crates" / "libs" / "paigasus-kernel"
    iam_dir = rs_root / "crates" / "services" / "paigasus-iam"
    gateway_dir = rs_root / "crates" / "services" / "paigasus-gateway"
    for d in (kernel_dir, iam_dir, gateway_dir):
        d.mkdir(parents=True)
    (rs_root / "Cargo.toml").write_text(
        '[workspace]\nmembers = ["crates/libs/*", "crates/services/*"]\n')
    (rs_root / "release-plz.toml").write_text("")
    (kernel_dir / "Cargo.toml").write_text(
        '[package]\nname = "paigasus-kernel"\nversion = "0.1.0"\n')
    (iam_dir / "Cargo.toml").write_text(
        '[package]\nname = "paigasus-iam"\nversion = "0.1.0-rc1"\npublish = false\n')
    (gateway_dir / "Cargo.toml").write_text(
        '[package]\nname = "paigasus-gateway"\nversion = "0.1.0"\npublish = false\n')
    _write_chain_fixture(Path(tmp))
    return rs_root


def _service_version_format_is_rejected() -> str | None:
    """Mutation this row proves: delete the `_SERVICE_VERSION_RE.fullmatch` test in
    service_state, and iam reads (True-or-False, '0.1.0-rc1') instead of (False, '')."""
    tmp = tempfile.mkdtemp()
    try:
        rs_root = _malformed_service_version_tree(tmp)
        state = service_state(rs_root.parent, set())
        if state.get("iam") != (False, ""):
            return (f"the malformed iam version did not read as (False, '') (run with an empty, "
                     f"never-matching plan version): {state.get('iam')!r}")
        if state.get("gateway") != (False, "0.1.0"):
            return (f"the healthy gateway crate was affected by iam's malformed version: "
                     f"{state.get('gateway')!r}")
        return None
    finally:
        shutil.rmtree(tmp)


def _service_unparsable_version_asserts_three_tree(tmp: str) -> Path:
    """A tree built so `_assert_repo` reports EXACTLY ONE problem if the fix under
    `_service_unparsable_version_asserts_three` is present, and ZERO if it is not.

    PR 2 review, finding 1. `_complete_chain_tree` derives exactly EXPECTED_RELEASABLE, carries a
    tag, and holds gateway and both consoles at the legitimate 0.0.0 skip (SMA-688: the chain
    registry and both console package.json files are part of it). That leaves iam's `0.1.0-rc1`
    — a version `_SERVICE_VERSION_RE` cannot parse — as the ONLY thing that can make
    `_assert_repo` report a problem, which is what proves the fix is load-bearing.
    """
    return _complete_chain_tree(tmp, versions={"iam": "0.1.0-rc1"})


def _service_unparsable_version_asserts_three() -> str | None:
    """PR 2 review, finding 1. Before the fix, `_assert_repo`'s service loop read

        if not version or version == "0.0.0":
            continue

    which treated `service_state`'s `(False, "")` — a version it could not parse at all — the
    same as the legitimate `0.0.0` "not released yet" skip. So an unparsable service version
    made `--assert` exit 0 on the very pull request that introduced it, and the fault surfaced
    only at runtime, after two architecture builds and two registry pushes.

    This row REDS without the fix: comment out the `problems.append` block added for this
    finding, re-run `--self-test`, and this row fails with "expected 3" because the fixture
    tree above manufactures no OTHER problem — `_assert_repo` would report rc 0.
    """
    tmp = tempfile.mkdtemp()
    try:
        repo_root = _service_unparsable_version_asserts_three_tree(tmp)
        with contextlib.redirect_stderr(io.StringIO()) as err:
            rc = _assert_repo(repo_root)
        if rc != 3:
            return (f"_assert_repo returned {rc} for an unparsable iam service version "
                     f"(0.1.0-rc1), expected 3 — an unreadable version must not read as the "
                     f"legitimate 0.0.0 skip")
        if "could not be read" not in err.getvalue():
            return (f"_assert_repo returned 3 but did not name the unparsable version as the "
                     f"cause: {err.getvalue()!r}")
        return None
    finally:
        shutil.rmtree(tmp)


def _service_state_is_a_separate_failure_domain() -> str | None:
    """Mutation this row proves: move service_state's per-key `try` outside the loop, and the
    healthy gateway crate reads (False, '') with iam."""
    tmp = tempfile.mkdtemp()
    try:
        rs_root = _mixed_service_tree(tmp)
        state = service_state(rs_root.parent, set())
        if state.get("iam") != (False, ""):
            return f"the missing iam crate did not read as (False, ''): {state.get('iam')!r}"
        if state.get("gateway") != (False, "0.1.0"):
            return (f"the healthy gateway crate was affected by iam's failure: "
                    f"{state.get('gateway')!r}")
        try:
            run(Path(tmp), "push")
        except Exception as exc:  # deliberately broad; run() must never raise (see its docstring)
            return f"run() raised {type(exc).__name__}: {exc} — a service failure must not reach it"
        return None
    finally:
        shutil.rmtree(tmp)


# SMA-658 fix round 1. `UnicodeDecodeError` is a `ValueError`, not an `OSError` — the original
# `except OSError` around the CHANGELOG.md read in `_assert_repo` did not catch it, so a
# non-UTF-8 file escaped past `main()` and crashed the interpreter at rc 1. That reproduces the
# exact class SMA-608 fixed for the collection layer (a bare exception mapped to `die_infra` = 2
# by `run.sh`, instead of "the repository is wrong" = 3), for a different read site. `load_toml`'s
# `except (OSError, tomllib.TOMLDecodeError)` is the pattern this follows: name every expected
# failure mode explicitly.
def _changelog_undecodable_asserts_three() -> str | None:
    """Mutation this row proves: narrow `except (OSError, UnicodeDecodeError)` in _assert_repo to
    `except OSError`, and the non-UTF-8 CHANGELOG.md escapes as a traceback.

    SMA-688: the tree carries the chain registry, and the row now also asserts the stderr names
    the changelog. This tree has other problems too (no tag, a derived releasable set that is
    not EXPECTED_RELEASABLE), so rc 3 alone does not prove that the decode error was reported.
    """
    tmp = tempfile.mkdtemp()
    try:
        rs_root = Path(tmp) / "rs"
        kernel_dir = rs_root / "crates" / "libs" / "paigasus-kernel"
        iam_dir = rs_root / "crates" / "services" / "paigasus-iam"
        kernel_dir.mkdir(parents=True)
        iam_dir.mkdir(parents=True)
        (rs_root / "Cargo.toml").write_text(
            '[workspace]\nmembers = ["crates/libs/paigasus-kernel", '
            '"crates/services/paigasus-iam"]\n')
        (rs_root / "release-plz.toml").write_text(
            '[[package]]\nname = "paigasus-iam"\nrelease = false\n')
        (kernel_dir / "Cargo.toml").write_text(
            '[package]\nname = "paigasus-kernel"\nversion = "1.0.0"\n')
        (iam_dir / "Cargo.toml").write_text(
            '[package]\nname = "paigasus-iam"\nversion = "0.1.0"\npublish = false\n')
        # An invalid UTF-8 byte (0xFF is never valid in any UTF-8 sequence position).
        (iam_dir / "CHANGELOG.md").write_bytes(b"## [0.1.0]\n\xff\n")
        _write_chain_fixture(Path(tmp))
        subprocess.run(["git", "init", "-q", tmp], check=True)
        try:
            with contextlib.redirect_stderr(io.StringIO()) as err:
                rc = _assert_repo(Path(tmp))
        except Exception as exc:  # deliberately broad; catching it IS the RED signal pre-fix
            return (f"_assert_repo raised {type(exc).__name__}: {exc} for a non-UTF-8 "
                    f"CHANGELOG.md instead of returning 3 — this is the rc=1 crash")
        if rc != 3:
            return f"_assert_repo returned {rc} for a non-UTF-8 CHANGELOG.md, expected 3"
        if "CHANGELOG.md cannot be read" not in err.getvalue():
            return (f"_assert_repo returned 3 but did not name the undecodable changelog: "
                    f"{err.getvalue()!r}")
        return None
    finally:
        shutil.rmtree(tmp)


# SMA-688. The malformed registry shapes. Each one must be InconclusiveError whose message holds
# the marker, and `--keys` must then print NO key and exit 3. An empty or partial key list would
# leave a chain output unwritten, and ci/release-plan/run.sh would run that chain with an empty
# version (spec § 4.2, last bullet).
_REGISTRY_SHAPE_CASES: tuple[tuple[str, str, str], ...] = (
    ("no chain table", "[other]\nx = 1\n", "has no [chain.<key>] table"),
    ("an empty chain table", "chain = {}\n", "has no [chain.<key>] table"),
    ("an entry that is not a table", "[chain]\niam = 3\n", "is not a table"),
    ("an entry with no version_file",
     '[chain.iam]\nkind = "cargo"\nchangelog = "c"\nghcr = "g"\nhub = "h"\n',
     "has no string version_file"),
    ("an unknown kind",
     '[chain.iam]\nkind = "pip"\nversion_file = "v"\nchangelog = "c"\nghcr = "g"\nhub = "h"\n',
     "kind 'pip' is not one of"),
    ("a key outside [a-z][a-z0-9-]*",
     '[chain.Iam_X]\nkind = "cargo"\nversion_file = "v"\nchangelog = "c"\nghcr = "g"\nhub = "h"\n',
     "is not [a-z][a-z0-9-]*"),
    ("invalid TOML", "[chain.iam\n", "cannot read"),
)


def _keys_of(repo_root: Path) -> tuple[int, str]:
    """`main(["--keys", repo_root])`: its return code and its stdout. Stderr is discarded."""
    out = io.StringIO()
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(io.StringIO()):
        rc = main(["--keys", str(repo_root)])
    return rc, out.getvalue()


def _keys_prints_the_registry() -> str | None:
    """`--keys` prints the registry keys, one on each line, in FILE order, and exits 0.

    ci/release-plan/run.sh reads this output to name the chain outputs it writes. Mutation: print
    `sorted(registry)` (gateway, gateway-console, iam, iam-console), or all keys on one line, and
    this row reds.
    """
    tmp = tempfile.mkdtemp()
    try:
        _write_chain_fixture(Path(tmp))
        rc, out = _keys_of(Path(tmp))
        want = "iam\ngateway\niam-console\ngateway-console\n"
        if rc != 0 or out != want:
            return f"--keys returned rc {rc} and printed {out!r}, want rc 0 and {want!r}"
        return None
    finally:
        shutil.rmtree(tmp)


def _missing_registry_is_inconclusive() -> str | None:
    """A tree with no ci/images/chains.toml. `--keys` exits 3 and prints nothing, and
    service_state names no key at all.

    Mutation: fall back to a hard-coded key list when the registry cannot be read, and this row
    reds on the non-empty stdout.
    """
    tmp = tempfile.mkdtemp()
    try:
        rc, out = _keys_of(Path(tmp))
        if rc != 3 or out:
            return f"--keys on a tree with no registry returned rc {rc} and printed {out!r}, want rc 3 and ''"
        with contextlib.redirect_stderr(io.StringIO()) as err:
            state = service_state(Path(tmp), set())
        if state != {}:
            return f"service_state named keys without a registry: {state!r}"
        if "the chain registry is inconclusive" not in err.getvalue():
            return f"service_state did not say why it named no key: {err.getvalue()!r}"
        return None
    finally:
        shutil.rmtree(tmp)


def _malformed_registry_is_inconclusive() -> str | None:
    """Each _REGISTRY_SHAPE_CASES entry: chain_registry raises with the case's own marker, and
    `--keys` exits 3 with no stdout.

    Mutation: delete any one validation in chain_registry, and its case reds with "did not raise"
    or with the wrong marker. The marker match, not a bare `except InconclusiveError`, is what
    makes a neutered check visible (the _tag_name_override_is_inconclusive lesson).
    """
    for label, text, marker in _REGISTRY_SHAPE_CASES:
        tmp = tempfile.mkdtemp()
        try:
            (Path(tmp) / CHAIN_REGISTRY).parent.mkdir(parents=True)
            (Path(tmp) / CHAIN_REGISTRY).write_text(text)
            try:
                chain_registry(Path(tmp))
            except InconclusiveError as exc:
                if marker not in str(exc):
                    return f"{label}: InconclusiveError for the wrong reason: {exc!r} (want {marker!r})"
            else:
                return f"{label}: chain_registry did not raise"
            rc, out = _keys_of(Path(tmp))
            if rc != 3 or out:
                return f"{label}: --keys returned rc {rc} and printed {out!r}, want rc 3 and ''"
        finally:
            shutil.rmtree(tmp)
    return None


def _only_iam_console_bumped() -> str | None:
    """AC 1: a release that bumps one console runs exactly that console's chain.

    iam and gateway are at 0.1.0 and tagged, iam-console is at 0.1.0 and untagged, and
    gateway-console is still 0.0.0. Mutation: read every console from one package.json, or look
    the iam-console tag up under the iam release name, and this row reds.
    """
    tmp = tempfile.mkdtemp()
    try:
        repo_root = _complete_chain_tree(
            tmp, versions={"iam": "0.1.0", "gateway": "0.1.0", "iam-console": "0.1.0"})
        got = service_state(repo_root, {"paigasus-iam-v0.1.0", "paigasus-gateway-v0.1.0"})
        want = {"iam": (True, "0.1.0"), "gateway": (True, "0.1.0"),
                "iam-console": (False, "0.1.0"), "gateway-console": (True, "0.0.0")}
        if got != want:
            return f"service_state returned {got!r}, want {want!r}"
        return None
    finally:
        shutil.rmtree(tmp)


def _console_package_inconclusive(text: str, what: str) -> str | None:
    """One malformed iam-console package.json: that key reads (False, ''), and no other key
    changes. Mutation: move service_state's per-key `try` outside the loop, and the other keys
    read (False, '') too."""
    tmp = tempfile.mkdtemp()
    try:
        repo_root = _complete_chain_tree(
            tmp, versions={"iam": "0.1.0", "gateway": "0.1.0", "gateway-console": "0.1.0"},
            console_texts={"iam-console": text})
        with contextlib.redirect_stderr(io.StringIO()) as err:
            got = service_state(repo_root, set())
        want = {"iam": (False, "0.1.0"), "gateway": (False, "0.1.0"),
                "iam-console": (False, ""), "gateway-console": (False, "0.1.0")}
        if got != want:
            return f"{what}: service_state returned {got!r}, want {want!r}"
        if "iam-console is inconclusive" not in err.getvalue():
            return f"{what}: no 'iam-console is inconclusive' line on stderr: {err.getvalue()!r}"
        return None
    finally:
        shutil.rmtree(tmp)


def _console_package_no_version() -> str | None:
    return _console_package_inconclusive(
        '{"name": "@paigasus/iam-console", "private": true}\n', "no version")


def _console_package_numeric_version() -> str | None:
    return _console_package_inconclusive(
        '{"name": "@paigasus/iam-console", "version": 1}\n', '"version": 1')


def _console_package_invalid_json() -> str | None:
    return _console_package_inconclusive(
        '{"name": "@paigasus/iam-console", "version": "0.1.0",\n', "invalid JSON")


def _console_assert_case(changelog: str) -> tuple[int, str]:
    """_assert_repo over a clean tree with iam-console at 0.1.0 and this CHANGELOG.md text."""
    tmp = tempfile.mkdtemp()
    try:
        repo_root = _complete_chain_tree(
            tmp, versions={"iam-console": "0.1.0"}, changelogs={"iam-console": changelog})
        with contextlib.redirect_stderr(io.StringIO()) as err:
            rc = _assert_repo(repo_root)
        return rc, err.getvalue()
    finally:
        shutil.rmtree(tmp)


def _console_changelog_missing_asserts_three() -> str | None:
    """A console at 0.1.0 with no `## [0.1.0]` section fails --assert (spec § 4.2).

    Mutation: restrict the changelog loop in _assert_repo to `kind == "cargo"`, and this row reds.
    Its twin below proves the tree has no OTHER problem, so rc 3 here is this cause alone.
    """
    rc, err = _console_assert_case("# Changelog\n\n## [Unreleased]\n")
    if rc != 3:
        return f"_assert_repo returned {rc} for iam-console 0.1.0 with no changelog section, expected 3"
    if "paigasus-iam-console is at 0.1.0 but" not in err or "`## [0.1.0]` heading" not in err:
        return f"_assert_repo returned 3 but did not name the missing console section: {err!r}"
    return None


def _console_changelog_present_asserts_zero() -> str | None:
    """The twin of the row above: the same tree WITH the section is clean."""
    rc, err = _console_assert_case("# Changelog\n\n## [Unreleased]\n\n## [0.1.0] - 2026-09-25\n")
    if rc != 0:
        return f"_assert_repo returned {rc} for a clean console tree, expected 0: {err!r}"
    return None


def _main_prints_every_chain_output() -> str | None:
    """The runtime path prints `skip_<key>` and `version_<key>` for EVERY registry key, in
    registry order, and the verdict line last.

    Mutation: loop over a hard-coded ("iam", "gateway") in main(), and the two console pairs are
    missing. ci/release-plan/run.sh would then take its fail-safe branch on every push.
    """
    tmp = tempfile.mkdtemp()
    try:
        _complete_chain_tree(tmp, versions={"iam": "0.1.0", "iam-console": "0.1.0"},
                             tags=("paigasus-iam-v0.1.0",))
        out = io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(io.StringIO()):
            rc = main(["--event-name", "push", tmp])
        lines = out.getvalue().splitlines()
        got = [ln for ln in lines if ln.startswith(("skip_", "version_"))]
        want = ["skip_iam=true", "version_iam=0.1.0", "skip_gateway=true", "version_gateway=0.0.0",
                "skip_iam-console=false", "version_iam-console=0.1.0",
                "skip_gateway-console=true", "version_gateway-console=0.0.0"]
        if rc != 0 or got != want:
            return f"main printed {got!r} (rc {rc}), want {want!r}"
        if not lines or not lines[-1].startswith("nothing_to_release="):
            return f"the verdict line is not last: {lines[-1:]!r}"
        return None
    finally:
        shutil.rmtree(tmp)


def _cargo_version_file_mismatch_is_inconclusive() -> str | None:
    """The cargo reader finds the crate through the workspace members. The registry's
    version_file must name that same manifest, because ci/helm-render reads version_file directly.

    Mutation: delete the resolve() comparison in cargo_version, and iam reads (False, '0.1.0').
    """
    tmp = tempfile.mkdtemp()
    try:
        repo_root = _complete_chain_tree(tmp, versions={"iam": "0.1.0"})
        registry = repo_root / CHAIN_REGISTRY
        registry.write_text(registry.read_text().replace(
            "rs/crates/services/paigasus-iam/Cargo.toml", "rs/crates/services/paigasus-iam/Other.toml"))
        with contextlib.redirect_stderr(io.StringIO()):
            state = service_state(repo_root, set())
        if state.get("iam") != (False, ""):
            return f"a version_file that names another file did not read as (False, ''): {state.get('iam')!r}"
        if state.get("gateway") != (True, "0.0.0"):
            return f"the gateway chain was affected: {state.get('gateway')!r}"
        return None
    finally:
        shutil.rmtree(tmp)


def _sed_parity_case(header: str) -> str | None:
    """_assert_repo over a clean tree whose `[chain.iam]` header line is replaced by `header`.

    tomllib still reads all four keys from each variant, and the row asserts that first. Without
    that guard, a variant that tomllib also rejects would fail --assert as an unreadable registry,
    and the row would pass for the wrong reason. _console_changelog_present_asserts_zero is the
    twin: the same tree with the bare header is clean, so rc 3 here is the parity check alone.
    """
    tmp = tempfile.mkdtemp()
    try:
        repo_root = _complete_chain_tree(tmp)
        registry = repo_root / CHAIN_REGISTRY
        text = registry.read_text()
        if "[chain.iam]\n" not in text:
            return "the fixture registry has no bare [chain.iam] line to replace"
        registry.write_text(text.replace("[chain.iam]\n", header + "\n", 1))
        keys = list(chain_registry(repo_root))
        if keys != ["iam", "gateway", "iam-console", "gateway-console"]:
            return f"tomllib does not read the four keys from {header!r}: {keys!r}"
        with contextlib.redirect_stderr(io.StringIO()) as err:
            rc = _assert_repo(repo_root)
        if rc != 3:
            return f"_assert_repo returned {rc} for the header {header!r}, expected 3"
        if "the sed read of" not in err.getvalue() or "['gateway', 'gateway-console', 'iam-console']" not in err.getvalue():
            return f"_assert_repo returned 3 but did not report the sed/tomllib mismatch: {err.getvalue()!r}"
        return None
    finally:
        shutil.rmtree(tmp)


def _sed_parity_trailing_spaces() -> str | None:
    """`[chain.iam]  `: tomllib reads iam, and run.sh's sed read does not (spec § 4.3).

    Mutation: delete the sed parity check in _assert_repo (the `if registry:` block), and this row
    reds with rc 0. Also: widen CHAIN_HEADER_SED_RE to allow trailing spaces without the same
    change to run.sh, and this row reds, which is the prompt to change the twin.
    """
    return _sed_parity_case("[chain.iam]  ")


def _sed_parity_trailing_comment() -> str | None:
    """`[chain.iam] # the IAM service`: tomllib reads iam, and the sed read does not.

    Mutation: delete the sed parity check in _assert_repo, and this row reds with rc 0.
    """
    return _sed_parity_case("[chain.iam] # the IAM service")


def _sed_parity_quoted_key() -> str | None:
    """`[chain."iam"]`: tomllib reads the key iam, and the sed read does not.

    Mutation: delete the sed parity check in _assert_repo, and this row reds with rc 0.
    """
    return _sed_parity_case('[chain."iam"]')


def _sed_parity_crlf_header() -> str | None:
    """`[chain.iam]\\r\\n`: tomllib reads iam (TOML accepts CRLF line endings), and real sed does
    not, because the trailing `\\r` sits before sed's own end-of-line, so `]$` never matches
    (MEASURED: `printf '[chain.iam]\\r\\n[chain.gw]\\n' | sed -n
    's/^\\[chain\\.\\([a-z][a-z0-9-]*\\)\\]$/\\1/p'` prints only `gw`).

    This is the row `_sed_parity_case` cannot cover: `_sed_parity_case` writes its variant with
    `Path.write_text`, which never inserts a `\\r`, so it must write the CRLF byte itself. This
    row's fixture is built by hand, not through `_sed_parity_case`, so it can control the exact
    bytes on disk.

    Mutation this row proves: in `_assert_repo`'s sed-parity block, read the registry with
    `.read_text(encoding="utf-8")` instead of `.read_bytes().decode("utf-8")`. `read_text` opens
    in universal-newline mode and silently turns the `\\r\\n` into a bare `\\n` before
    `sed_chain_keys` ever sees it, so the in-process sed read then also finds `iam` — the parity
    check reports no problem for what is a real mismatch against the sed run.sh actually runs.
    """
    tmp = tempfile.mkdtemp()
    try:
        repo_root = _complete_chain_tree(tmp)
        registry = repo_root / CHAIN_REGISTRY
        data = registry.read_bytes()
        marker = b"[chain.iam]\n"
        if marker not in data:
            return "the fixture registry has no bare [chain.iam] line to replace"
        registry.write_bytes(data.replace(marker, b"[chain.iam]\r\n", 1))
        keys = list(chain_registry(repo_root))
        if keys != ["iam", "gateway", "iam-console", "gateway-console"]:
            return f"tomllib does not read the four keys from a CRLF [chain.iam] header: {keys!r}"
        with contextlib.redirect_stderr(io.StringIO()) as err:
            rc = _assert_repo(repo_root)
        if rc != 3:
            return f"_assert_repo returned {rc} for a CRLF [chain.iam] header, expected 3"
        if "the sed read of" not in err.getvalue() or "['gateway', 'gateway-console', 'iam-console']" not in err.getvalue():
            return f"_assert_repo returned 3 but did not report the sed/tomllib mismatch: {err.getvalue()!r}"
        return None
    finally:
        shutil.rmtree(tmp)


def _sed_parity_fixture_registry() -> str | None:
    """The fixture registry, which has the same headers as the real ci/images/chains.toml, gives
    the same keys to both readers. The real file is checked by `--assert .` (Step 4) and by
    repo:actionlint check 11. This row does not read the real file through `__file__`, because
    run.sh rows 7 and 8 run --self-test on a COPY of this file in a temp directory.

    Mutation: change CHAIN_HEADER_SED_RE so that it drops a hyphenated key (for example
    `[a-z][a-z0-9]*`), and this row reds.
    """
    got = sed_chain_keys(_FIXTURE_CHAINS_TOML)
    if got != list(_FIXTURE_CHAINS):
        return f"sed_chain_keys read {got!r} from the fixture registry, want {list(_FIXTURE_CHAINS)!r}"
    return None


# The collection-layer rows: paths a pure-function fixture cannot reach. Fourteen of the fifteen
# need a filesystem (they build throwaway trees under tempfile.mkdtemp()); row 15
# (_markers_are_mutually_exclusive) needs none, but still cannot be expressed as a decide()-only
# FIXTURES row, since it asserts a property of config_sections' own error messages. Module-level
# so `--collection-count` can count them and so self_test()'s floor below has something to floor;
# the FIXTURES floor's own comment explains why a countable table matters.
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
    ("SMA-658 service skip rows", _service_fixture_rows),
    ("SMA-658 changelog reader rows", _changelog_reader_rows),
    ("SMA-658 service_state is a separate failure domain", _service_state_is_a_separate_failure_domain),
    ("SMA-658 fix round 1: a non-UTF-8 CHANGELOG.md makes --assert exit 3, not 1",
     _changelog_undecodable_asserts_three),
    ("PR 2 review: a non-MAJOR.MINOR.PATCH service version is rejected",
     _service_version_format_is_rejected),
    ("PR 2 review finding 1: an unparsable service version makes --assert exit 3, not 0",
     _service_unparsable_version_asserts_three),
    ("SMA-688 --keys prints the registry keys in file order", _keys_prints_the_registry),
    ("SMA-688 a missing chains.toml is inconclusive for every key", _missing_registry_is_inconclusive),
    ("SMA-688 a malformed chains.toml is inconclusive, never an empty key list",
     _malformed_registry_is_inconclusive),
    ("SMA-688 a tree that bumps only iam-console runs only that chain", _only_iam_console_bumped),
    ("SMA-688 a console package.json with no version is inconclusive for that key only",
     _console_package_no_version),
    ("SMA-688 a console package.json with a non-string version is inconclusive for that key only",
     _console_package_numeric_version),
    ("SMA-688 a console package.json with invalid JSON is inconclusive for that key only",
     _console_package_invalid_json),
    ("SMA-688 a console at 0.1.0 with no changelog section fails --assert",
     _console_changelog_missing_asserts_three),
    ("SMA-688 a console at 0.1.0 with its changelog section passes --assert",
     _console_changelog_present_asserts_zero),
    ("SMA-688 main prints skip_<key> and version_<key> for every key", _main_prints_every_chain_output),
    ("SMA-688 the cargo version file must be the manifest the workspace resolves",
     _cargo_version_file_mismatch_is_inconclusive),
    ("SMA-688 sed parity: a chain header with trailing spaces fails --assert",
     _sed_parity_trailing_spaces),
    ("SMA-688 sed parity: a chain header with a trailing comment fails --assert",
     _sed_parity_trailing_comment),
    ("SMA-688 sed parity: a quoted chain key fails --assert", _sed_parity_quoted_key),
    ("SMA-688 sed parity: a CRLF chain header fails --assert (real sed, not read_text)",
     _sed_parity_crlf_header),
    ("SMA-688 sed parity: the fixture registry gives both readers the same keys",
     _sed_parity_fixture_registry),
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
    if len(COLLECTION_ROWS) < 14:
        print(f"FAIL COLLECTION_ROWS has only {len(COLLECTION_ROWS)} row(s); the floor is 14 — "
              "something emptied or gutted the collection-layer table", file=sys.stderr)
        rc = 3
    for label, fn in COLLECTION_ROWS:
        try:
            err = fn()
        except Exception as exc:  # deliberately broad; see "EVERY call is wrapped" above
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
    # SMA-658 spec § 3.1, widened by SMA-688 to every chain in ci/images/chains.toml: a version is
    # bumped by hand, so nothing else can catch a forgotten changelog entry. `repo:actionlint`
    # check 11 runs --assert on every pull request, which is what makes this a gate rather than a
    # convention.
    try:
        registry = chain_registry(repo_root)
    except InconclusiveError as exc:
        problems.append(f"the chain registry cannot be read ({exc}). Every chain output of the "
                        f"release plan depends on it.")
        registry = {}
    # SMA-688. The sed twin (see CHAIN_HEADER_SED_RE). When `--keys` fails, run.sh names the chain
    # outputs from the sed read alone, so that read must find the SAME keys as tomllib. `--keys`
    # prints the tomllib keys, so this one check also proves that a valid `--keys` answer holds
    # every key the fallback would name.
    if registry:
        try:
            # read_bytes().decode(), NOT read_text(): read_text opens in universal-newline mode
            # and turns a CRLF header into a bare "\n" one, so a "[chain.iam]\r\n" line would
            # match CHAIN_HEADER_SED_RE here while real sed, which sees the raw "\r", does not
            # (MEASURED: `printf '[chain.iam]\r\n[chain.gw]\n' | sed -n
            # 's/^\[chain\.\([a-z][a-z0-9-]*\)\]$/\1/p'` prints only `gw`). read_bytes() gives
            # sed_chain_keys the same bytes real sed reads.
            sed_keys = sed_chain_keys((repo_root / CHAIN_REGISTRY).read_bytes().decode("utf-8"))
        except (OSError, UnicodeDecodeError) as exc:
            problems.append(f"{CHAIN_REGISTRY} cannot be read as text ({exc}).")
        else:
            if set(sed_keys) != set(registry):
                problems.append(
                    f"the sed read of {CHAIN_REGISTRY} finds {sorted(set(sed_keys))}, but tomllib "
                    f"finds {sorted(registry)}. Write each chain header as a bare "
                    f"`[chain.<key>]` line, with nothing after the `]`: ci/release-plan/run.sh "
                    f"reads the keys with sed when `--keys` fails.")
    for key, (_skip, version) in service_state(repo_root, tags).items():
        name = release_name(key)
        # PR 2 review finding 1: an empty version and "0.0.0" are NOT the same state. `""` means
        # service_state could not read a literal MAJOR.MINOR.PATCH version at all (a missing
        # file, a non-string version, or one that fails _SERVICE_VERSION_RE) — that is a
        # REPOSITORY PROBLEM this gate exists to catch. "0.0.0" means the version was read fine
        # and simply has not shipped yet, which is a legitimate skip.
        if not version:
            problems.append(
                f"{name}'s version could not be read (see the '{key} is inconclusive' line "
                f"above). A hand-bumped chain must carry a literal MAJOR.MINOR.PATCH version in "
                f"{registry[key]['version_file']}.")
            continue
        if version == "0.0.0":
            continue
        changelog = repo_root / registry[key]["changelog"]
        try:
            text = changelog.read_text(encoding="utf-8")
        # SMA-658 fix round 1: UnicodeDecodeError is a ValueError, not an OSError, so a
        # non-UTF-8 CHANGELOG.md escaped this except and crashed main() at rc 1 — the same class
        # SMA-608 fixed for the collection layer. Named explicitly, like load_toml's
        # `except (OSError, tomllib.TOMLDecodeError)` above.
        except (OSError, UnicodeDecodeError) as exc:
            problems.append(f"{name} is at {version} but {changelog} cannot be read "
                            f"({exc}). A hand-bumped chain needs a changelog section.")
            continue
        if not changelog_names_version(text, version):
            problems.append(f"{name} is at {version} but {changelog} has no "
                            f"`## [{version}]` heading. Add the section in the same PR as the "
                            f"bump: nothing else records what that release contains.")
    for p in problems:
        print(f"release-plan: {p}", file=sys.stderr)
    return 3 if problems else 0


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--self-test", action="store_true")
    ap.add_argument("--assert", dest="do_assert", action="store_true")
    ap.add_argument("--fixture-count", action="store_true")
    ap.add_argument("--collection-count", action="store_true")
    ap.add_argument("--keys", action="store_true")
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
    if args.keys:
        # SMA-688. ci/release-plan/run.sh reads this list to name the chain outputs it writes:
        # one key on each line, in file order. A registry that cannot be read prints NO key and
        # exits 3, because an empty or partial list would leave a chain output unwritten. The
        # catch is broad for the reason _assert_repo's is: this mode never exits 1.
        try:
            registry = chain_registry(root)
        except Exception as exc:  # deliberately broad; see the comment above.
            print(f"release-plan: {type(exc).__name__}: {exc}", file=sys.stderr)
            return 3
        for key in registry:
            print(key)
        return 0
    if args.do_assert:
        return _assert_repo(root)

    nothing, reason = run(root, args.event_name)
    print(f"release-plan: {reason}")
    # SMA-658. The chain lines are computed in their own failure domain, so a broken chain read
    # cannot change the kernel verdict above. `repo_tags` is read once more here on purpose:
    # `run()` swallows its own failure, and a second failure here must land on the per-chain
    # fail-safe rather than on the kernel one. SMA-688: one pair of lines for every registry key.
    try:
        tags = repo_tags(root)
    except Exception as exc:  # deliberately broad; the fail-safe direction is "run".
        print(f"release-plan: tags are inconclusive ({type(exc).__name__}: {exc}) — run",
              file=sys.stderr)
        tags = set()
    for key, (skip, version) in service_state(root, tags).items():
        print(f"skip_{key}={'true' if skip else 'false'}")
        print(f"version_{key}={version}")
    print(f"nothing_to_release={'true' if nothing else 'false'}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
