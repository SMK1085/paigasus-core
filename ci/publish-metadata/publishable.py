# SPDX-License-Identifier: Apache-2.0
"""The publishable set of the Cargo workspace, and Check 0 (SMA-376; one copy since SMA-735).

Two callers use this one copy: the metadata_checks heredoc in run.sh imports
publishable_packages, and run.sh --verify-publish-groups runs this file as a script. So the
verify build in release.yml's verify-crates job and the PR gate agree on which crates are
publishable.

Exit-code contract, inherited from run.sh: 0 pass / 1 the repository is wrong / 2
infrastructure. An empty set is 2 (cargo metadata is broken, or every crate is
publish = false). A set that is not EXPECTED_PUBLISHABLE is 1.
"""

from __future__ import annotations

import json
import os
import sys


def is_publishable(pkg: dict) -> bool:
    # cargo metadata: null => publishable anywhere; [] => publish = false;
    # non-empty list => publishable to those named registries.
    value = pkg.get("publish")
    return value is None or (isinstance(value, list) and len(value) > 0)


def publishable_packages(meta: dict, expected: list[str]) -> dict[str, dict]:
    """The publishable packages of `meta` by name. Check 0: exits 2 on an empty set and 1 when
    the set is not `expected`."""
    pkgs = {p["name"]: p for p in meta.get("packages", []) if is_publishable(p)}
    found = sorted(pkgs)
    want = sorted(expected)
    if not found:
        print(
            "FATAL: no publishable crate found. Either cargo metadata is broken or every "
            "crate is publish = false. This gate must never pass over an empty set.",
            file=sys.stderr,
        )
        sys.exit(2)
    if found != want:
        print(
            f"Check 0 FAILED: publishable set {found} != expected {want}.\n"
            "  Add the crate to EXPECTED_PUBLISHABLE in ci/publish-metadata/run.sh — "
            "or you have just silently disabled this gate.",
            file=sys.stderr,
        )
        sys.exit(1)
    return pkgs


def main(argv: list[str]) -> int:
    if len(argv) != 2:
        print("usage: publishable.py <metadata.json> <expected-csv>", file=sys.stderr)
        return 2
    try:
        with open(argv[0], encoding="utf-8") as fh:
            meta = json.load(fh)
    except Exception as exc:
        print(f"FATAL: cannot read cargo metadata JSON: {exc}", file=sys.stderr)
        return 2
    expected = [x for x in argv[1].split(",") if x]
    for name, pkg in sorted(publishable_packages(meta, expected).items()):
        print(f"{name}\t{os.path.dirname(pkg['manifest_path'])}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
