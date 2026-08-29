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
import shutil
import subprocess
import sys
import tempfile
import tomllib
from pathlib import Path

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


class Inconclusive(Exception):
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
        raise Inconclusive(f"cannot read {path}: {exc}") from exc


def assert_default_tag_format(cfg: dict) -> None:
    if "git_tag_name" in (cfg.get("workspace") or {}):
        raise Inconclusive("rs/release-plz.toml sets [workspace] git_tag_name; tag_for() assumes "
                           "release-plz's default <package>-v<version>")
    for pkg in cfg.get("package") or []:
        if isinstance(pkg, dict) and "git_tag_name" in pkg:
            raise Inconclusive(f"rs/release-plz.toml sets git_tag_name on "
                               f"{pkg.get('name')!r}; tag_for() assumes the default format")


def crate_manifests(rs_root: Path) -> dict[str, Path]:
    """Map package name -> Cargo.toml. Walks rs/crates/**, so it needs no cargo and no network."""
    found: dict[str, Path] = {}
    for manifest in sorted(rs_root.glob("crates/*/*/Cargo.toml")):
        pkg = load_toml(manifest).get("package") or {}
        name = pkg.get("name")
        if not isinstance(name, str) or not name:
            continue
        if name in found:
            raise Inconclusive(f"two manifests declare package {name!r}: {found[name]}, {manifest}")
        found[name] = manifest
    if not found:
        raise Inconclusive(f"no crate manifests under {rs_root}/crates — the tree moved")
    return found


def releasable_packages(rs_root: Path) -> dict[str, str]:
    """Package -> literal version, for every package release-plz would TAG.

    A package is tagged when Cargo does not say `publish = false` AND rs/release-plz.toml says
    neither `release = false` nor `publish = false`. An ABSENT release-plz entry reads as
    release = true / publish = true, which is release-plz's own default — so an unlisted crate
    counts as releasable and its missing tag makes us BUILD. That is the fail-safe direction.
    """
    cfg = load_toml(rs_root / "release-plz.toml")
    assert_default_tag_format(cfg)
    entries = {p["name"]: p for p in (cfg.get("package") or [])
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
            raise Inconclusive(f"{name} has no literal [package] version in {manifest}")
        out[name] = version
    return out


def repo_tags(repo_root: Path) -> set[str]:
    try:
        proc = subprocess.run(["git", "-C", str(repo_root), "tag", "-l"],
                              capture_output=True, text=True, check=True)
    except (OSError, subprocess.CalledProcessError) as exc:
        raise Inconclusive(f"git tag -l failed: {exc}") from exc
    return {line.strip() for line in proc.stdout.splitlines() if line.strip()}


def run(repo_root: Path, event_name: str) -> tuple[bool, str]:
    try:
        packages = releasable_packages(repo_root / "rs")
        tags = repo_tags(repo_root)
    except Inconclusive as exc:
        return False, f"inconclusive ({exc}) — build"
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
    """A tree with crate manifests but no rs/release-plz.toml must be Inconclusive.

    `load_toml` is the first thing `releasable_packages` calls, and a missing file raises
    `FileNotFoundError`, an `OSError` subclass, which `load_toml` already converts.
    """
    tmp = tempfile.mkdtemp()
    try:
        rs_root = Path(tmp) / "rs"
        crate_dir = rs_root / "crates" / "libs" / "a"
        crate_dir.mkdir(parents=True)
        (crate_dir / "Cargo.toml").write_text('[package]\nname = "a"\nversion = "1.0.0"\n')
        try:
            releasable_packages(rs_root)
        except Inconclusive:
            return None
        return "releasable_packages did not raise Inconclusive for a missing release-plz.toml"
    finally:
        shutil.rmtree(tmp)


def _workspace_version_is_inconclusive() -> str | None:
    """`version.workspace = true` parses as a dict, not a literal string, and must be
    Inconclusive rather than silently treated as absent.
    """
    tmp = tempfile.mkdtemp()
    try:
        rs_root = Path(tmp) / "rs"
        (rs_root).mkdir()
        (rs_root / "release-plz.toml").write_text("")
        crate_dir = rs_root / "crates" / "libs" / "a"
        crate_dir.mkdir(parents=True)
        (crate_dir / "Cargo.toml").write_text(
            '[package]\nname = "a"\nversion.workspace = true\npublish = true\n')
        try:
            releasable_packages(rs_root)
        except Inconclusive:
            return None
        return "releasable_packages did not raise Inconclusive for a workspace-inherited version"
    finally:
        shutil.rmtree(tmp)


def _tag_name_override_is_inconclusive() -> str | None:
    """A `[workspace] git_tag_name` override invalidates `tag_for`'s default-format assumption,
    and must be Inconclusive rather than silently tagged the usual way.
    """
    tmp = tempfile.mkdtemp()
    try:
        rs_root = Path(tmp) / "rs"
        rs_root.mkdir()
        (rs_root / "release-plz.toml").write_text(
            '[workspace]\ngit_tag_name = "v{{ version }}"\n')
        try:
            releasable_packages(rs_root)
        except Inconclusive:
            return None
        return "releasable_packages did not raise Inconclusive for a git_tag_name override"
    finally:
        shutil.rmtree(tmp)


def self_test() -> int:
    rc = 0
    for label, event, packages, tags, want in FIXTURES:
        got, reason = decide(event, packages, tags)
        if got != want:
            print(f"FAIL {label!r}: expected {want}, got {got} ({reason})", file=sys.stderr)
            rc = 3

    # Collection-layer rows, which need the filesystem rather than the pure function.
    for label, fn in (
        ("a missing release-plz.toml is inconclusive", _missing_config_is_inconclusive),
        ("a workspace-inherited version is inconclusive", _workspace_version_is_inconclusive),
        ("a git_tag_name override is inconclusive", _tag_name_override_is_inconclusive),
    ):
        err = fn()
        if err:
            print(f"FAIL {label!r}: {err}", file=sys.stderr)
            rc = 3
    return rc


def _assert_repo(repo_root: Path) -> int:
    """--assert. The CI-side assertions; the runtime path uses none of them."""
    problems: list[str] = []
    try:
        packages = releasable_packages(repo_root / "rs")
    except Inconclusive as exc:
        print(f"release-plan: {exc}", file=sys.stderr)
        return 3
    derived = frozenset(packages)
    if derived != EXPECTED_RELEASABLE:
        problems.append(
            f"the derived releasable set {sorted(derived)} does not equal the pinned "
            f"EXPECTED_RELEASABLE {sorted(EXPECTED_RELEASABLE)}. If a crate legitimately became "
            f"publishable, re-baseline the pin deliberately — do not loosen the comparison.")
    if not repo_tags(repo_root):
        problems.append("the repository reports no tags; --assert needs a full checkout")
    for p in problems:
        print(f"release-plan: {p}", file=sys.stderr)
    return 3 if problems else 0


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--self-test", action="store_true")
    ap.add_argument("--assert", dest="do_assert", action="store_true")
    ap.add_argument("--fixture-count", action="store_true")
    ap.add_argument("--event-name", default="")
    ap.add_argument("repo_root", nargs="?", default=".")
    args = ap.parse_args(argv)

    if args.fixture_count:
        print(len(FIXTURES))
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
