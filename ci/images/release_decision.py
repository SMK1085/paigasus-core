#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""SMA-658: the decisions behind a service image release, kept out of shell and YAML so that a
self-test can prove them.

Subcommands. Each one prints `key=value` lines on stdout, in a fixed order:

  oci-digests ARCHIVE
      manifest=, config=, platform= of a single-image buildx OCI archive.
  adopt --new-digest D --ghcr D|none --dockerhub D|none --git-tag present|absent
      action=already-released | push-new | adopt, then digest= and copy_to=.
      The FIRST digest published under :<version> is final (spec D10). A new build never
      overwrites it; it adopts it.
  floating --service S --version X.Y.Z --tags-file FILE
      move=true|false, minor_tag=, major_tag= (empty while the major version is 0).
      FILE holds bare tag names or `git ls-remote --tags` lines.
  sbom-summary SPDX_JSON
      packages=, libc6=true|false, cargo= (a measurement for spec M8, not an assertion).

Exit codes: 0 decided | 2 usage or unreadable input | 3 a conflict, or a failed self-test row.
It never exits 1: `uv` exits 1 on its own failures, so 1 would be ambiguous.

Standard library only, so it runs with `uv run --no-project --python '>=3.12' python3`.
"""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import re
import sys
import tarfile
from pathlib import Path
from typing import TYPE_CHECKING, Any

if TYPE_CHECKING:
    from collections.abc import Callable

DIGEST_RE = re.compile(r"sha256:[0-9a-f]{64}")
VERSION_RE = re.compile(r"(\d+)\.(\d+)\.(\d+)")
SERVICE_RE = re.compile(r"[a-z][a-z0-9-]*")
NONE = "none"
IMAGE_MANIFEST_TYPES = frozenset(
    {
        "application/vnd.oci.image.manifest.v1+json",
        "application/vnd.docker.distribution.manifest.v2+json",
    }
)

Version = tuple[int, int, int]


class UsageError(Exception):
    """Bad arguments or an unreadable input: exit 2."""


class ConflictError(Exception):
    """The two registries hold different digests under one version: exit 3."""


def parse_version(text: str) -> Version:
    match = VERSION_RE.fullmatch(text)
    if match is None:
        raise UsageError(f"not a MAJOR.MINOR.PATCH version: {text!r}")
    return (int(match[1]), int(match[2]), int(match[3]))


def require_digest(text: str) -> str:
    if DIGEST_RE.fullmatch(text) is None:
        raise UsageError(f"not a sha256 digest: {text!r}")
    return text


def optional_digest(text: str) -> str | None:
    return None if text == NONE else require_digest(text)


def adopt(new_digest: str, ghcr: str | None, dockerhub: str | None, *, git_tag_present: bool) -> dict[str, str]:
    """Spec D10: the first digest published under :<version> is final."""
    if git_tag_present:
        return {"action": "already-released", "digest": "", "copy_to": "none"}
    if ghcr is None and dockerhub is None:
        return {"action": "push-new", "digest": new_digest, "copy_to": "none"}
    if ghcr is not None and dockerhub is not None:
        if ghcr != dockerhub:
            raise ConflictError(f":<version> points at two digests: ghcr={ghcr} dockerhub={dockerhub}")
        return {"action": "adopt", "digest": ghcr, "copy_to": "none"}
    if ghcr is not None:
        return {"action": "adopt", "digest": ghcr, "copy_to": "dockerhub"}
    assert dockerhub is not None
    return {"action": "adopt", "digest": dockerhub, "copy_to": "ghcr"}


def tag_names(lines: list[str]) -> list[str]:
    """Accept bare tag names and `git ls-remote --tags` lines (`<sha>\\trefs/tags/<name>`)."""
    names: list[str] = []
    for line in lines:
        fields = line.split()
        if fields:
            names.append(fields[-1].removeprefix("refs/tags/"))
    return names


def floating(service: str, version: str, lines: list[str]) -> dict[str, str]:
    """Spec § 4.3 step 7: move :<major>.<minor> and :latest only forward, compared as numbers."""
    if SERVICE_RE.fullmatch(service) is None:
        raise UsageError(f"not a service name: {service!r}")
    mine = parse_version(version)
    pattern = re.compile(rf"paigasus-{re.escape(service)}-v(\d+\.\d+\.\d+)")
    existing = [parse_version(m[1]) for name in tag_names(lines) if (m := pattern.fullmatch(name))]
    move = not existing or mine >= max(existing)
    return {
        "move": "true" if move else "false",
        "minor_tag": f"{mine[0]}.{mine[1]}",
        "major_tag": "" if mine[0] == 0 else str(mine[0]),
    }


def _member(tar: tarfile.TarFile, name: str) -> bytes:
    try:
        handle = tar.extractfile(name)
    except KeyError as exc:
        raise UsageError(f"the archive has no {name}") from exc
    if handle is None:
        raise UsageError(f"{name} in the archive is not a regular file")
    return handle.read()


def _blob(tar: tarfile.TarFile, digest: str) -> bytes:
    data = _member(tar, "blobs/sha256/" + digest.removeprefix("sha256:"))
    if "sha256:" + hashlib.sha256(data).hexdigest() != digest:
        raise UsageError(f"blob {digest} does not match its own digest")
    return data


def oci_digests_from(tar: tarfile.TarFile) -> dict[str, str]:
    """The manifest and config digests of a single-image buildx OCI export."""
    index = json.loads(_member(tar, "index.json"))
    manifests = index.get("manifests") or []
    if len(manifests) != 1:
        raise UsageError(f"index.json lists {len(manifests)} manifests; expected one image (build with --provenance=false --sbom=false)")
    entry = manifests[0]
    if entry.get("mediaType") not in IMAGE_MANIFEST_TYPES:
        raise UsageError(f"index.json names {entry.get('mediaType')!r}, not one image manifest")
    manifest_digest = require_digest(str(entry.get("digest", "")))
    manifest = json.loads(_blob(tar, manifest_digest))
    config_digest = require_digest(str((manifest.get("config") or {}).get("digest", "")))
    config = json.loads(_blob(tar, config_digest))
    platform = f"{config.get('os', 'unknown')}/{config.get('architecture', 'unknown')}"
    return {"manifest": manifest_digest, "config": config_digest, "platform": platform}


def oci_digests(path: Path) -> dict[str, str]:
    try:
        with tarfile.open(path) as tar:
            return oci_digests_from(tar)
    except (OSError, tarfile.TarError, json.JSONDecodeError) as exc:
        raise UsageError(f"cannot read {path}: {exc}") from exc


def sbom_summary(doc: dict[str, Any]) -> dict[str, str]:
    """Spec M8: does the SBOM see the Ubuntu packages and the Rust crates at all?"""
    packages = doc.get("packages") or []
    names = {str(p.get("name", "")) for p in packages}
    cargo = sum(
        1 for p in packages if any(str(ref.get("referenceLocator", "")).startswith("pkg:cargo/") for ref in p.get("externalRefs") or [])
    )
    return {"packages": str(len(packages)), "libc6": "true" if "libc6" in names else "false", "cargo": str(cargo)}


# --- self-test -------------------------------------------------------------------------------

D1 = "sha256:" + "1" * 64
D2 = "sha256:" + "2" * 64
D3 = "sha256:" + "3" * 64


def _fixture_archive(
    *, media_type: str = "application/vnd.oci.image.manifest.v1+json", manifests: int = 1
) -> tuple[tarfile.TarFile, str, str]:
    """An in-memory OCI archive shaped like a buildx single-image export."""
    config = json.dumps({"os": "linux", "architecture": "arm64"}).encode()
    config_digest = "sha256:" + hashlib.sha256(config).hexdigest()
    manifest = json.dumps({"config": {"digest": config_digest}}).encode()
    manifest_digest = "sha256:" + hashlib.sha256(manifest).hexdigest()
    index = json.dumps({"manifests": [{"mediaType": media_type, "digest": manifest_digest}] * manifests}).encode()
    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode="w") as tar:
        for name, data in (
            ("index.json", index),
            ("blobs/sha256/" + manifest_digest.removeprefix("sha256:"), manifest),
            ("blobs/sha256/" + config_digest.removeprefix("sha256:"), config),
        ):
            info = tarfile.TarInfo(name)
            info.size = len(data)
            tar.addfile(info, io.BytesIO(data))
    buf.seek(0)
    return tarfile.open(fileobj=buf, mode="r"), manifest_digest, config_digest


def _outcome(fn: Callable[[], object]) -> object:
    try:
        return fn()
    except UsageError:
        return "UsageError"
    except ConflictError:
        return "ConflictError"


def self_test() -> int:
    tar, manifest, config = _fixture_archive()
    gw = "gateway"
    rows: list[tuple[str, Callable[[], object], object]] = [
        # adopt (spec D10, § 4.3 step 3)
        ("adopt: the git tag exists", lambda: adopt(D1, D2, D2, git_tag_present=True)["action"], "already-released"),
        ("adopt: nothing published", lambda: adopt(D1, None, None, git_tag_present=False), {"action": "push-new", "digest": D1, "copy_to": "none"}),
        ("adopt: both registries agree", lambda: adopt(D1, D2, D2, git_tag_present=False), {"action": "adopt", "digest": D2, "copy_to": "none"}),
        ("adopt: only GHCR has it", lambda: adopt(D1, D2, None, git_tag_present=False), {"action": "adopt", "digest": D2, "copy_to": "dockerhub"}),
        ("adopt: only Docker Hub has it", lambda: adopt(D1, None, D3, git_tag_present=False), {"action": "adopt", "digest": D3, "copy_to": "ghcr"}),
        ("adopt: the registries disagree", lambda: adopt(D1, D2, D3, git_tag_present=False), "ConflictError"),
        ("digest: 'none' where a digest is required", lambda: require_digest(NONE), "UsageError"),
        ("digest: a short digest", lambda: require_digest("sha256:abc"), "UsageError"),
        ("digest: 'none' is absent where allowed", lambda: optional_digest(NONE), None),
        # floating (spec § 4.3 step 7, § 7.3)
        ("floating: no tag yet", lambda: floating(gw, "0.1.0", [])["move"], "true"),
        ("floating: an equal version moves", lambda: floating(gw, "0.2.0", ["paigasus-gateway-v0.2.0"])["move"], "true"),
        ("floating: an older version holds", lambda: floating(gw, "0.1.1", ["paigasus-gateway-v0.2.0"])["move"], "false"),
        ("floating: 0.10.0 beats 0.9.0 (numeric order)", lambda: floating(gw, "0.10.0", ["paigasus-gateway-v0.9.0"])["move"], "true"),
        ("floating: 0.2.0 holds against 0.10.0", lambda: floating(gw, "0.2.0", ["paigasus-gateway-v0.10.0"])["move"], "false"),
        ("floating: other services' tags are ignored", lambda: floating(gw, "0.1.0", ["paigasus-iam-v9.0.0"])["move"], "true"),
        (
            "floating: ls-remote lines and peeled refs",
            lambda: floating(gw, "0.1.0", ["abc\trefs/tags/paigasus-gateway-v0.3.0", "abc\trefs/tags/paigasus-gateway-v0.3.0^{}"])["move"],
            "false",
        ),
        ("floating: no :major tag while 0.x", lambda: floating(gw, "0.4.2", []), {"move": "true", "minor_tag": "0.4", "major_tag": ""}),
        ("floating: a :major tag from 1.0", lambda: floating(gw, "1.2.3", [])["major_tag"], "1"),
        ("floating: a malformed version", lambda: floating(gw, "v1.2", []), "UsageError"),
        ("floating: a malformed service", lambda: floating("Gate way", "0.1.0", []), "UsageError"),
        # oci-digests (spec § 4.2 identity check)
        ("oci: a single-image archive", lambda: oci_digests_from(tar), {"manifest": manifest, "config": config, "platform": "linux/arm64"}),
        ("oci: an index is refused", lambda: oci_digests_from(_fixture_archive(media_type="application/vnd.oci.image.index.v1+json")[0]), "UsageError"),
        ("oci: two manifests are refused", lambda: oci_digests_from(_fixture_archive(manifests=2)[0]), "UsageError"),
        # sbom-summary (spec M8)
        (
            "sbom: counts libc6 and cargo packages",
            lambda: sbom_summary({"packages": [{"name": "libc6"}, {"name": "serde", "externalRefs": [{"referenceLocator": "pkg:cargo/serde@1.0.228"}]}]}),
            {"packages": "2", "libc6": "true", "cargo": "1"},
        ),
        ("sbom: an empty document", lambda: sbom_summary({}), {"packages": "0", "libc6": "false", "cargo": "0"}),
    ]
    failed = 0
    for label, fn, want in rows:
        got = _outcome(fn)
        if got != want:
            failed += 1
            print(f"FAIL {label}: expected {want!r}, got {got!r}", file=sys.stderr)
    if failed:
        print(f"release_decision self-test: {failed} of {len(rows)} rows failed", file=sys.stderr)
        return 3
    print(f"release_decision self-test: {len(rows)} rows OK")
    return 0


# --- CLI ---------------------------------------------------------------------------------------


def _emit(result: dict[str, str]) -> None:
    for key, value in result.items():
        print(f"{key}={value}")


def _read_text(path: Path) -> str:
    try:
        return path.read_text(encoding="utf-8")
    except OSError as exc:
        raise UsageError(f"cannot read {path}: {exc}") from exc


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(prog="release_decision.py", description="SMA-658 image release decisions")
    parser.add_argument("--self-test", action="store_true", help="run the fixture table and exit")
    sub = parser.add_subparsers(dest="command")
    p_oci = sub.add_parser("oci-digests")
    p_oci.add_argument("archive", type=Path)
    p_adopt = sub.add_parser("adopt")
    p_adopt.add_argument("--new-digest", required=True)
    p_adopt.add_argument("--ghcr", required=True)
    p_adopt.add_argument("--dockerhub", required=True)
    p_adopt.add_argument("--git-tag", required=True, choices=("present", "absent"))
    p_float = sub.add_parser("floating")
    p_float.add_argument("--service", required=True)
    p_float.add_argument("--version", required=True)
    p_float.add_argument("--tags-file", required=True, type=Path)
    p_sbom = sub.add_parser("sbom-summary")
    p_sbom.add_argument("sbom", type=Path)
    try:
        args = parser.parse_args(argv)
    except SystemExit as exc:  # argparse exits 2 on a usage error and 0 on --help
        return int(exc.code or 0)

    if args.self_test:
        return self_test()
    try:
        if args.command == "oci-digests":
            _emit(oci_digests(args.archive))
        elif args.command == "adopt":
            _emit(
                adopt(
                    require_digest(args.new_digest),
                    optional_digest(args.ghcr),
                    optional_digest(args.dockerhub),
                    git_tag_present=args.git_tag == "present",
                )
            )
        elif args.command == "floating":
            _emit(floating(args.service, args.version, _read_text(args.tags_file).splitlines()))
        elif args.command == "sbom-summary":
            try:
                doc = json.loads(_read_text(args.sbom))
            except json.JSONDecodeError as exc:
                raise UsageError(f"{args.sbom} is not JSON: {exc}") from exc
            if not isinstance(doc, dict):
                raise UsageError(f"{args.sbom} is not an SPDX JSON object")
            _emit(sbom_summary(doc))
        else:
            parser.print_usage(sys.stderr)
            return 2
    except UsageError as exc:
        print(f"release_decision: {exc}", file=sys.stderr)
        return 2
    except ConflictError as exc:
        print("action=conflict")
        print(f"release_decision: {exc}", file=sys.stderr)
        return 3
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
