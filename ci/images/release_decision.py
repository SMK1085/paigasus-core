#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""SMA-658: the decisions behind a service image release, kept out of shell and YAML so that a
self-test can prove them.

Subcommands. Each one prints `key=value` lines on stdout, in a fixed order:

  oci-digests ARCHIVE
      manifest=, config=, platform= of a single-image buildx OCI archive.
  labels ARCHIVE
      version=, revision=, title= (the org.opencontainers.image.* labels) of a single-image
      buildx OCI archive. SMA-658 C1: `crane config` takes a REGISTRY reference only -- it cannot
      read a local `oci-archive:` path -- so this is what release.yml reads the labels with
      instead.
  adopt --new-digest D --ghcr D|none --dockerhub D|none --git-tag present|absent
      action=already-released | push-new | adopt, then digest= and copy_to=.
      The FIRST digest published under :<version> is final (spec D10). A new build never
      overwrites it; it adopts it.
  floating --service S --version X.Y.Z --tags-file FILE
      move=true|false, minor_tag=, major_tag= (empty while the major version is 0).
      FILE holds bare tag names or `git ls-remote --tags` lines.
  sbom-summary SPDX_JSON
      packages=, libc6=true|false, cargo=, npm=, next=true|false, deb= (a measurement, spec M8).
  sbom-floor --service KEY --arch ARCH [--chisel-dir DIR] SPDX_JSON
      SMA-688 D5: the sbom-summary lines, then kind= and floor=pass. One floor for each image
      kind, read from ci/images/chains.toml. SMA-665 D5: a cargo key also reads
      DIR/chisel-manifest-KEY-ARCH.txt (DIR is the working directory by default), and its SBOM's
      Debian entries must be the same set of (name, version, arch) as that chisel fetch list.
      An npm key reads no list and ignores --arch. A missing, empty or unparsable list is
      exit 2. A failed floor or an unknown key prints floor=fail and exits 3.

Exit codes: 0 decided | 2 usage or unreadable input | 3 a conflict, or a failed self-test row.
It never exits 1: `uv` exits 1 on its own failures, so 1 would be ambiguous.

Standard library only, so it runs with `uv run --no-project --python '>=3.12' python3`.
"""

from __future__ import annotations

import argparse
import contextlib
import hashlib
import io
import json
import re
import sys
import tarfile
import tempfile
import tomllib
from pathlib import Path
from typing import TYPE_CHECKING, Any
from urllib.parse import unquote

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

# SMA-665. One Debian package: (name, version, arch). The arch is "" when a purl has no arch
# qualifier.
Deb = tuple[str, str, str]

# SMA-665 D3/D5. A line of chisel-manifest-<key>-<arch>.txt, as extract_chisel_manifest in
# ci/images/run.sh writes it. MEASURED (spec M4, chisel v1.4.2):
#   Fetching pool/main/g/glibc/libc6_2.39-0ubuntu8.9_arm64.deb
# A pool file name carries no epoch, and `~` and `+` stay literal in it.
CHISEL_FETCH_RE = re.compile(r"Fetching pool/[^/\s]+/[^/\s]+/[^/\s]+/(?P<name>[^/_\s]+)_(?P<version>[^/_\s]+)_(?P<arch>[^/_.\s]+)\.deb")
ARCH_RE = re.compile(r"[a-z0-9]+")
EPOCH_RE = re.compile(r"^\d+:")


class UsageError(Exception):
    """Bad arguments or an unreadable input: exit 2."""


class ConflictError(Exception):
    """The two registries hold different digests under one version: exit 3."""


class FloorError(Exception):
    """SMA-688: an SBOM below its image kind's floor, or a key that names no chain: exit 3."""


# SMA-688 D5. The chain registry sits beside this file. Only `kind` is read here.
CHAINS_TOML = Path(__file__).resolve().with_name("chains.toml")


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


def _require_dict(value: object, what: str) -> dict[str, Any]:
    """Every JSON value in this file comes from a file on disk, not a trusted source. A wrong
    shape must raise UsageError (exit 2). It must never raise an unhandled AttributeError. The
    module docstring says this script never exits 1."""
    if not isinstance(value, dict):
        raise UsageError(f"{what} must be a JSON object, not {type(value).__name__!r}")
    return value


def _require_list(value: object, what: str) -> list[Any]:
    if not isinstance(value, list):
        raise UsageError(f"{what} must be a JSON list, not {type(value).__name__!r}")
    return value


def _image_config_from(tar: tarfile.TarFile) -> tuple[dict[str, Any], str, str]:
    """The parsed image config of a single-image buildx OCI export, plus the two digests that
    identify it. Shared by `oci_digests_from` (which only needs the digests) and `labels_from`
    (SMA-658 C1, which needs the config's own `Labels`)."""
    index = _require_dict(json.loads(_member(tar, "index.json")), "index.json")
    manifests_value = index.get("manifests")
    manifests = [] if manifests_value is None else _require_list(manifests_value, "index.json's manifests")
    if len(manifests) != 1:
        raise UsageError(f"index.json lists {len(manifests)} manifests; expected one image (build with --provenance=false --sbom=false)")
    entry = _require_dict(manifests[0], "index.json's manifest entry")
    if entry.get("mediaType") not in IMAGE_MANIFEST_TYPES:
        raise UsageError(f"index.json names {entry.get('mediaType')!r}, not one image manifest")
    manifest_digest = require_digest(str(entry.get("digest", "")))
    manifest = _require_dict(json.loads(_blob(tar, manifest_digest)), "the image manifest")
    config_ref_value = manifest.get("config")
    config_ref = {} if config_ref_value is None else config_ref_value
    config_digest = require_digest(str(_require_dict(config_ref, "the image manifest's config").get("digest", "")))
    config = _require_dict(json.loads(_blob(tar, config_digest)), "the image config")
    return config, manifest_digest, config_digest


def oci_digests_from(tar: tarfile.TarFile) -> dict[str, str]:
    """The manifest and config digests of a single-image buildx OCI export."""
    config, manifest_digest, config_digest = _image_config_from(tar)
    platform = f"{config.get('os', 'unknown')}/{config.get('architecture', 'unknown')}"
    return {"manifest": manifest_digest, "config": config_digest, "platform": platform}


def oci_digests(path: Path) -> dict[str, str]:
    try:
        with tarfile.open(path) as tar:
            return oci_digests_from(tar)
    except (OSError, tarfile.TarError, json.JSONDecodeError, UnicodeDecodeError) as exc:
        raise UsageError(f"cannot read {path}: {exc}") from exc


def labels_from(tar: tarfile.TarFile) -> dict[str, str]:
    """SMA-658 C1: `crane config "oci-archive:…"` cannot read a local archive. MEASURED against
    crane 0.22.1: `Error: fetching config: parsing reference … could not parse reference`, and
    with a missing file it tries to resolve a host called `oci-archive`. `oci-archive:` is a
    syft/skopeo transport; `crane config` takes a REGISTRY reference only. This reads the labels
    straight out of the archive `release_decision.py` already has on disk, so `release.yml` never
    shells out to `crane config` for a config it already extracted.

    SMA-688: it also returns the title. The publish job compares it with `paigasus-<key>`, so an
    archive of another chain stops before the first registry write, even when both chains share
    one version."""
    config, _manifest_digest, _config_digest = _image_config_from(tar)
    config_obj_value = config.get("config")
    config_obj = {} if config_obj_value is None else _require_dict(config_obj_value, "the image config's 'config' object")
    labels_value = config_obj.get("Labels")
    label_map = {} if labels_value is None else _require_dict(labels_value, "the image config's Labels")
    version = str(label_map.get("org.opencontainers.image.version", ""))
    revision = str(label_map.get("org.opencontainers.image.revision", ""))
    title = str(label_map.get("org.opencontainers.image.title", ""))
    if not version:
        raise UsageError("the image config carries no org.opencontainers.image.version label")
    if not revision:
        raise UsageError("the image config carries no org.opencontainers.image.revision label")
    if not title:
        raise UsageError("the image config carries no org.opencontainers.image.title label")
    return {"version": version, "revision": revision, "title": title}


def labels(path: Path) -> dict[str, str]:
    try:
        with tarfile.open(path) as tar:
            return labels_from(tar)
    except (OSError, tarfile.TarError, json.JSONDecodeError, UnicodeDecodeError) as exc:
        raise UsageError(f"cannot read {path}: {exc}") from exc


def _sbom_packages(doc: dict[str, Any]) -> list[tuple[str, list[str]]]:
    """(name, its referenceLocator strings) for each SPDX package. A wrong shape is exit 2."""
    # A missing "packages" key defaults to []; a present key must be a list (even if falsey).
    packages_value = doc.get("packages")
    packages = [] if packages_value is None else _require_list(packages_value, "packages")
    entries: list[tuple[str, list[str]]] = []
    for p in packages:
        if not isinstance(p, dict):
            raise UsageError(f"packages must be a list of objects, not {type(p).__name__!r}")
        # A missing "externalRefs" key defaults to []; a present key must be a list (even if falsey).
        refs_value = p.get("externalRefs")
        refs = [] if refs_value is None else _require_list(refs_value, "a package's externalRefs")
        locators: list[str] = []
        for ref in refs:
            if not isinstance(ref, dict):
                raise UsageError(f"an externalRefs entry must be a JSON object, not {type(ref).__name__!r}")
            locators.append(str(ref.get("referenceLocator", "")))
        entries.append((str(p.get("name", "")), locators))
    return entries


def sbom_summary(doc: dict[str, Any]) -> dict[str, str]:
    """Spec M8 (SMA-658): does the SBOM see the OS packages and the Rust crates at all?

    SMA-688 adds the npm count and whether `next` is there. A console image has no Rust crates,
    and its floor reads those two instead. SMA-665 adds `deb`, the count of pkg:deb purls."""
    entries = _sbom_packages(doc)
    names = {name for name, _locators in entries}
    locators = [locator for _name, refs in entries for locator in refs]
    return {
        "packages": str(len(entries)),
        "libc6": "true" if "libc6" in names else "false",
        "cargo": str(sum(1 for locator in locators if locator.startswith("pkg:cargo/"))),
        "npm": str(sum(1 for locator in locators if locator.startswith("pkg:npm/"))),
        # The purl of `next` itself. A scoped `@next/env` is pkg:npm/%40next/env@…, so it does
        # not match.
        "next": "true" if any(locator.startswith("pkg:npm/next@") for locator in locators) else "false",
        "deb": str(sum(1 for locator in locators if locator.startswith("pkg:deb/"))),
    }


def parse_deb_purl(locator: str) -> Deb:
    """SMA-665 D5: (name, version, arch) of a pkg:deb purl. The name, the version and the arch
    are percent-decoded. The namespace, the other qualifiers and a subpath are ignored. A leading
    `N:` epoch is dropped, because a pool file name has none. A purl with no arch qualifier gives
    arch "": that is a floor failure (exit 3), not a parse failure.

    MEASURED (spec M2, syft 1.52.0): syft writes `~` raw, `+` as %2B and the epoch colon as %3A,
    for example pkg:deb/ubuntu/zz-epoch@1%3A2.3%2Bdfsg-1~x?arch=arm64&distro=ubuntu-24.04. A
    purl writer may also leave `+` raw or write `~` as %7E; both decode to the same version."""
    if not locator.startswith("pkg:deb/"):
        raise UsageError(f"not a pkg:deb purl: {locator!r}")
    body = locator.removeprefix("pkg:deb/").partition("#")[0]
    body, _sep, query = body.partition("?")
    path, at, version_text = body.rpartition("@")
    segments = path.split("/")
    if not at or not version_text or not 1 <= len(segments) <= 2 or not all(segments):
        raise UsageError(f"a pkg:deb purl that does not parse: {locator!r}")
    arch = ""
    for pair in query.split("&") if query else []:
        key, eq, value = pair.partition("=")
        if not eq or not key:
            raise UsageError(f"a pkg:deb purl qualifier that does not parse: {locator!r}")
        if key == "arch":
            arch = unquote(value)
    name = unquote(segments[-1])
    version = EPOCH_RE.sub("", unquote(version_text), count=1)
    if not name or not version:
        raise UsageError(f"a pkg:deb purl with an empty name or version: {locator!r}")
    return (name, version, arch)


def sbom_debs(doc: dict[str, Any]) -> list[Deb]:
    """SMA-665 D5: (name, version, arch) for each pkg:deb purl in the SBOM, in document order.
    A pkg:deb purl that does not parse is exit 2."""
    return [parse_deb_purl(locator) for _name, refs in _sbom_packages(doc) for locator in refs if locator.startswith("pkg:deb/")]


def parse_chisel_list(text: str) -> list[Deb]:
    """SMA-665 D5: (name, version, arch) for each line of a chisel fetch list. An empty list, a
    line that does not parse, and one name on two lines are exit 2."""
    entries: list[Deb] = []
    seen: dict[str, str] = {}
    for line in text.splitlines():
        if not line.strip():
            continue
        match = CHISEL_FETCH_RE.fullmatch(line.strip())
        if match is None:
            raise UsageError(f"a chisel fetch line that does not parse: {line!r}")
        name, version, arch = match["name"], match["version"], match["arch"]
        if name in seen:
            raise UsageError(f"the chisel fetch list names {name} twice ({seen[name]} and {version})")
        seen[name] = version
        entries.append((name, version, arch))
    if not entries:
        raise UsageError("the chisel fetch list is empty")
    return entries


def _chisel_reasons(debs: list[Deb], chisel: list[Deb], arch: str) -> list[str]:
    """SMA-665 D5: the SBOM's Debian entries and the chisel fetch list must be the same set of
    (name, version, arch). Each list entry matches exactly one SBOM entry (forward), and the two
    counts are equal (reverse). Together the two rules are set equality."""
    reasons: list[str] = []
    for name, version, list_arch in chisel:
        if list_arch not in (arch, "all"):
            reasons.append(f"the chisel fetch list names {name} {version} for {list_arch}, not for {arch}")
    for name, version, deb_arch in debs:
        if not deb_arch:
            reasons.append(f"the SBOM lists {name} {version} with no arch qualifier")
    # Forward: each list entry is in the SBOM exactly once.
    for entry in chisel:
        found = debs.count(entry)
        if found != 1:
            reasons.append(f"the SBOM lists {' '.join(entry)} {found} times; expected exactly once")
    # Reverse: no Debian entry in the SBOM is outside the list.
    if len(debs) != len(chisel):
        extra = sorted(set(debs) - set(chisel))
        reasons.append(f"the SBOM has {len(debs)} Debian entries and the chisel fetch list has {len(chisel)}; not in the list: {extra}")
    return reasons


def chain_kinds(text: str) -> dict[str, str]:
    """SMA-688: key -> kind, from the text of ci/images/chains.toml. A wrong shape is exit 2."""
    try:
        data = tomllib.loads(text)
    except tomllib.TOMLDecodeError as exc:
        raise UsageError(f"the chain registry is not TOML: {exc}") from exc
    chains = _require_dict(data.get("chain"), "the chain registry's [chain] table")
    kinds: dict[str, str] = {}
    for key, entry in chains.items():
        kind = _require_dict(entry, f"[chain.{key}]").get("kind")
        if not isinstance(kind, str):
            raise UsageError(f"[chain.{key}] has no string kind")
        kinds[key] = kind
    return kinds


def sbom_floor(key: str, kinds: dict[str, str], summary: dict[str, str], *, debs: list[Deb], chisel: list[Deb] | None, arch: str) -> str:
    """SMA-688 D5. The kind of `key` when its SBOM reaches that kind's floor. Else FloorError.

    cargo: at least one Rust crate (the binary was built with cargo auditable; the SMA-658 rule),
           libc6, and (SMA-665 D5) the SBOM's Debian entries `debs` equal the chisel fetch list
           `chisel` as a set of (name, version, arch). `chisel` must not be None for this kind.
    npm:   at least one npm package, libc6 (the distroless base's dpkg data, spec M3) and `next`.
           `debs`, `chisel` and `arch` are not read.
    """
    if key not in kinds:
        raise FloorError(f"{key!r} names no chain in ci/images/chains.toml (known: {sorted(kinds)})")
    kind = kinds[key]
    reasons: list[str] = []
    if kind == "cargo":
        if chisel is None:
            raise UsageError(f"the cargo floor for {key!r} needs the chisel fetch list")
        if int(summary["cargo"]) < 1:
            reasons.append("the SBOM lists no Rust crates; the binary was not built with cargo auditable")
        if summary["libc6"] != "true":
            reasons.append("the SBOM does not list libc6; the chisel cut's generated dpkg status data is missing")
        reasons += _chisel_reasons(debs, chisel, arch)
    elif kind == "npm":
        if int(summary["npm"]) < 1:
            reasons.append("the SBOM lists no npm packages")
        if summary["libc6"] != "true":
            reasons.append("the SBOM does not list libc6; the base image's package data is gone")
        if summary["next"] != "true":
            reasons.append("the SBOM does not list next; the console's own framework is not seen")
    else:
        raise FloorError(f"{key!r} has kind {kind!r}, and no SBOM floor exists for that kind")
    if reasons:
        raise FloorError(f"the {kind} floor failed for {key}: " + "; ".join(reasons))
    return kind


# --- self-test -------------------------------------------------------------------------------

D1 = "sha256:" + "1" * 64
D2 = "sha256:" + "2" * 64
D3 = "sha256:" + "3" * 64


def _archive_from_files(files: dict[str, bytes]) -> tarfile.TarFile:
    """Builds an in-memory tar from a name-to-bytes map. The shape matches an OCI archive's
    members."""
    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode="w") as tar:
        for name, data in files.items():
            info = tarfile.TarInfo(name)
            info.size = len(data)
            tar.addfile(info, io.BytesIO(data))
    buf.seek(0)
    return tarfile.open(fileobj=buf, mode="r")


def _fixture_archive(
    *,
    media_type: str = "application/vnd.oci.image.manifest.v1+json",
    manifests: int = 1,
    config_extra: dict[str, Any] | None = None,
) -> tuple[tarfile.TarFile, str, str]:
    """An in-memory OCI archive shaped like a buildx single-image export. `config_extra` merges
    into the image config object -- SMA-658 C1's `labels` self-test rows use it to add or
    malform a `config.Labels` object without duplicating the whole fixture shape."""
    config_obj: dict[str, Any] = {"os": "linux", "architecture": "arm64"}
    if config_extra:
        config_obj.update(config_extra)
    config = json.dumps(config_obj).encode()
    config_digest = "sha256:" + hashlib.sha256(config).hexdigest()
    manifest = json.dumps({"config": {"digest": config_digest}}).encode()
    manifest_digest = "sha256:" + hashlib.sha256(manifest).hexdigest()
    index = json.dumps({"manifests": [{"mediaType": media_type, "digest": manifest_digest}] * manifests}).encode()
    tar = _archive_from_files(
        {
            "index.json": index,
            "blobs/sha256/" + manifest_digest.removeprefix("sha256:"): manifest,
            "blobs/sha256/" + config_digest.removeprefix("sha256:"): config,
        }
    )
    return tar, manifest_digest, config_digest


def _fixture_archive_bad_index() -> tarfile.TarFile:
    """`index.json` is valid JSON, but it is not an object."""
    return _archive_from_files({"index.json": json.dumps(["not", "an", "object"]).encode()})


def _fixture_archive_bad_manifest() -> tarfile.TarFile:
    """`index.json` is well-formed. The manifest blob it names is not an object."""
    manifest = json.dumps(["not", "an", "object"]).encode()
    manifest_digest = "sha256:" + hashlib.sha256(manifest).hexdigest()
    index = json.dumps(
        {"manifests": [{"mediaType": "application/vnd.oci.image.manifest.v1+json", "digest": manifest_digest}]}
    ).encode()
    return _archive_from_files(
        {
            "index.json": index,
            "blobs/sha256/" + manifest_digest.removeprefix("sha256:"): manifest,
        }
    )


def _outcome(fn: Callable[[], object]) -> object:
    try:
        return fn()
    except UsageError:
        return "UsageError"
    except ConflictError:
        return "ConflictError"
    except FloorError:
        return "FloorError"


# SMA-688. The kinds that the floor rows use: one key of each kind, the same as the registry.
FLOOR_KINDS = {"iam": "cargo", "iam-console": "npm"}


def _summary(**values: str) -> dict[str, str]:
    """An sbom_summary() result with every count at zero, then `values` on top."""
    return {"packages": "0", "libc6": "false", "cargo": "0", "npm": "0", "next": "false", "deb": "0", **values}


# SMA-665. A chisel fetch list of three packages, and the SBOM entries that match it. The texts
# are MEASURED (spec M2, M4) on the arm64 cut of 2026-10-01, with the arch set to amd64.
BASE_FILES: Deb = ("base-files", "13ubuntu10.5", "amd64")
CA_CERTS: Deb = ("ca-certificates", "20260601~24.04.1", "all")
LIBC6: Deb = ("libc6", "2.39-0ubuntu8.9", "amd64")
CHISEL = [BASE_FILES, CA_CERTS, LIBC6]
CHISEL_TEXT = (
    "Fetching pool/main/b/base-files/base-files_13ubuntu10.5_amd64.deb\n"
    "Fetching pool/main/c/ca-certificates/ca-certificates_20260601~24.04.1_all.deb\n"
    "Fetching pool/main/g/glibc/libc6_2.39-0ubuntu8.9_amd64.deb\n"
)


def _cargo_floor(debs: list[Deb], chisel: list[Deb] | None = CHISEL, arch: str = "amd64", **values: str) -> object:
    """The cargo floor of `iam` with one crate and libc6, unless `values` says otherwise."""
    return sbom_floor("iam", FLOOR_KINDS, _summary(**{"cargo": "1", "libc6": "true", **values}), debs=debs, chisel=chisel, arch=arch)


def _deb_doc(*locators: str) -> dict[str, Any]:
    """An SPDX document with one package for each purl."""
    return {"packages": [{"name": "p", "externalRefs": [{"referenceLocator": locator}]} for locator in locators]}


def _main_rc(argv: list[str]) -> int:
    """main()'s exit code, with its output kept off the self-test's own output."""
    with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
        return main(argv)


def _cli_rows(tmp: Path) -> list[tuple[str, Callable[[], object], object]]:
    """SMA-665: the sbom-floor CLI. The cargo key reads its list file; the npm key reads none."""
    cargo_sbom = tmp / "cargo.spdx.json"
    cargo_sbom.write_text(json.dumps({"packages": [
        {"name": "serde", "externalRefs": [{"referenceLocator": "pkg:cargo/serde@1.0.228"}]},
        {"name": "base-files", "externalRefs": [{"referenceLocator": "pkg:deb/ubuntu/base-files@13ubuntu10.5?arch=amd64&distro=ubuntu-24.04"}]},
        {"name": "ca-certificates", "externalRefs": [{"referenceLocator": "pkg:deb/ubuntu/ca-certificates@20260601~24.04.1?arch=all&distro=ubuntu-24.04"}]},
        {"name": "libc6", "externalRefs": [{"referenceLocator": "pkg:deb/ubuntu/libc6@2.39-0ubuntu8.9?arch=amd64&distro=ubuntu-24.04&upstream=glibc"}]},
    ]}))
    npm_sbom = tmp / "npm.spdx.json"
    npm_sbom.write_text(json.dumps({"packages": [
        {"name": "libc6", "externalRefs": [{"referenceLocator": "pkg:deb/debian/libc6@2.36-9+deb12u10?arch=amd64&distro=debian-12"}]},
        {"name": "next", "externalRefs": [{"referenceLocator": "pkg:npm/next@16.3.5"}]},
    ]}))
    with_list = tmp / "with-list"
    with_list.mkdir()
    (with_list / "chisel-manifest-iam-amd64.txt").write_text(CHISEL_TEXT)
    empty_list = tmp / "empty-list"
    empty_list.mkdir()
    (empty_list / "chisel-manifest-iam-amd64.txt").write_text("")
    no_list = tmp / "no-list"
    no_list.mkdir()

    def floor(*args: str) -> Callable[[], object]:
        return lambda: _main_rc(["sbom-floor", *args])

    return [
        ("cli: cargo with its chisel list passes", floor("--service", "iam", "--arch", "amd64", "--chisel-dir", str(with_list), str(cargo_sbom)), 0),
        ("cli: cargo with no list file is exit 2", floor("--service", "iam", "--arch", "amd64", "--chisel-dir", str(no_list), str(cargo_sbom)), 2),
        ("cli: cargo with an empty list file is exit 2", floor("--service", "iam", "--arch", "amd64", "--chisel-dir", str(empty_list), str(cargo_sbom)), 2),
        ("cli: cargo with the list of another arch is exit 2", floor("--service", "iam", "--arch", "arm64", "--chisel-dir", str(with_list), str(cargo_sbom)), 2),
        ("cli: no --arch is exit 2", floor("--service", "iam", "--chisel-dir", str(with_list), str(cargo_sbom)), 2),
        # The npm key reads no list file, so only the ARCH_RE guard can give exit 2 here. A cargo
        # key with "../amd64" gives exit 2 from the missing list file even without the guard.
        ("cli: an --arch that is not an architecture is exit 2", floor("--service", "iam-console", "--arch", "../amd64", "--chisel-dir", str(no_list), str(npm_sbom)), 2),
        ("cli: npm with the cargo arguments and no list file passes", floor("--service", "iam-console", "--arch", "amd64", "--chisel-dir", str(no_list), str(npm_sbom)), 0),
    ]


def self_test() -> int:
    with tempfile.TemporaryDirectory() as tmp:
        return _self_test(Path(tmp))


def _self_test(tmp: Path) -> int:
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
        ("oci: index.json is not an object", lambda: oci_digests_from(_fixture_archive_bad_index()), "UsageError"),
        ("oci: the manifest blob is not an object", lambda: oci_digests_from(_fixture_archive_bad_manifest()), "UsageError"),
        # labels (spec SMA-658 C1: crane config cannot read a local oci-archive)
        (
            "labels: a version, revision and title label",
            lambda: labels_from(
                _fixture_archive(
                    config_extra={
                        "config": {
                            "Labels": {
                                "org.opencontainers.image.version": "1.2.3",
                                "org.opencontainers.image.revision": "abc123",
                                "org.opencontainers.image.title": "paigasus-iam-console",
                            }
                        }
                    }
                )[0]
            ),
            {"version": "1.2.3", "revision": "abc123", "title": "paigasus-iam-console"},
        ),
        ("labels: no Labels at all (missing-label case)", lambda: labels_from(_fixture_archive()[0]), "UsageError"),
        (
            "labels: Labels present but missing the version key (missing-label case)",
            lambda: labels_from(_fixture_archive(config_extra={"config": {"Labels": {"org.opencontainers.image.revision": "abc123"}}})[0]),
            "UsageError",
        ),
        (
            "labels: Labels present but missing the revision key (missing-label case)",
            lambda: labels_from(_fixture_archive(config_extra={"config": {"Labels": {"org.opencontainers.image.version": "1.2.3"}}})[0]),
            "UsageError",
        ),
        (
            "labels: the 'config' object is not a dict (malformed-config case)",
            lambda: labels_from(_fixture_archive(config_extra={"config": "oops"})[0]),
            "UsageError",
        ),
        (
            "labels: Labels is not a dict (malformed-config case)",
            lambda: labels_from(_fixture_archive(config_extra={"config": {"Labels": "oops"}})[0]),
            "UsageError",
        ),
        # sbom-summary (spec M8)
        (
            "sbom: counts libc6 and cargo packages",
            lambda: sbom_summary({"packages": [{"name": "libc6"}, {"name": "serde", "externalRefs": [{"referenceLocator": "pkg:cargo/serde@1.0.228"}]}]}),
            {"packages": "2", "libc6": "true", "cargo": "1", "npm": "0", "next": "false", "deb": "0"},
        ),
        ("sbom: an empty document", lambda: sbom_summary({}), {"packages": "0", "libc6": "false", "cargo": "0", "npm": "0", "next": "false", "deb": "0"}),
        ("sbom: packages is not a list", lambda: sbom_summary({"packages": "oops"}), "UsageError"),
        ("sbom: packages list has non-dict elements", lambda: sbom_summary({"packages": [{"name": "libc6"}, "oops", 5]}), "UsageError"),
        (
            "sbom: externalRefs is not a list",
            lambda: sbom_summary({"packages": [{"name": "serde", "externalRefs": "oops"}]}),
            "UsageError",
        ),
        (
            "sbom: an externalRefs element is not an object",
            lambda: sbom_summary({"packages": [{"name": "serde", "externalRefs": ["oops"]}]}),
            "UsageError",
        ),
        # TDD: falsey-value cases that must raise UsageError
        ("sbom: packages is false (present but falsey)", lambda: sbom_summary({"packages": False}), "UsageError"),
        ("sbom: packages is 0 (present but falsey)", lambda: sbom_summary({"packages": 0}), "UsageError"),
        ("sbom: externalRefs is '' (present but falsey)", lambda: sbom_summary({"packages": [{"name": "test", "externalRefs": ""}]}), "UsageError"),
        ("sbom: missing packages key gives empty list (no error)", lambda: sbom_summary({}), {"packages": "0", "libc6": "false", "cargo": "0", "npm": "0", "next": "false", "deb": "0"}),
        # SMA-688: the console SBOM (spec M1: 67 npm, 10 deb, next 16.3.5).
        (
            "sbom: counts npm packages and sees next",
            lambda: sbom_summary({"packages": [
                {"name": "libc6"},
                {"name": "next", "externalRefs": [{"referenceLocator": "pkg:npm/next@16.3.5"}]},
                {"name": "react", "externalRefs": [{"referenceLocator": "pkg:npm/react@19.3.0"}]},
            ]}),
            {"packages": "3", "libc6": "true", "cargo": "0", "npm": "2", "next": "true", "deb": "0"},
        ),
        # `@next/env` is a DIFFERENT package. Its purl is pkg:npm/%40next/env@…, so it must not
        # read as `next`. Mutation: test `"next" in locator`, and this row reds.
        (
            "sbom: @next/env is not next",
            lambda: sbom_summary({"packages": [{"name": "@next/env", "externalRefs": [{"referenceLocator": "pkg:npm/%40next/env@16.3.5"}]}]})["next"],
            "false",
        ),
        # labels: the title is required (SMA-688, the decide step's identity check).
        (
            "labels: Labels present but missing the title key (missing-label case)",
            lambda: labels_from(_fixture_archive(config_extra={"config": {"Labels": {
                "org.opencontainers.image.version": "1.2.3",
                "org.opencontainers.image.revision": "abc123"}}})[0]),
            "UsageError",
        ),
        # floating: `iam` is a string prefix of `iam-console`. The pattern is a fullmatch, so the
        # console's tags never hold back the service's floating tags. Mutation: use re.match
        # with no `$`, and this row reds.
        ("floating: iam ignores iam-console tags", lambda: floating("iam", "0.1.0", ["paigasus-iam-console-v0.9.0"])["move"], "true"),
        # sbom-floor (SMA-688 D5): a pass and a fail for each kind, an unknown key, an unknown kind.
        ("floor: cargo with crates, libc6 and the chisel list", lambda: _cargo_floor(CHISEL), "cargo"),
        ("floor: cargo with no crate", lambda: _cargo_floor(CHISEL, cargo="0"), "FloorError"),
        ("floor: npm with npm, libc6 and next", lambda: sbom_floor("iam-console", FLOOR_KINDS, _summary(npm="67", libc6="true", next="true"), debs=[], chisel=None, arch="amd64"), "npm"),
        ("floor: npm with no npm package", lambda: sbom_floor("iam-console", FLOOR_KINDS, _summary(libc6="true", next="true"), debs=[], chisel=None, arch="amd64"), "FloorError"),
        ("floor: npm without libc6", lambda: sbom_floor("iam-console", FLOOR_KINDS, _summary(npm="67", next="true"), debs=[], chisel=None, arch="amd64"), "FloorError"),
        ("floor: npm without next", lambda: sbom_floor("iam-console", FLOOR_KINDS, _summary(npm="67", libc6="true"), debs=[], chisel=None, arch="amd64"), "FloorError"),
        # A cargo floor must not accept an npm image: crates are the cargo rule, npm is not.
        ("floor: a cargo key with only npm packages", lambda: _cargo_floor(CHISEL, cargo="0", npm="67", next="true"), "FloorError"),
        ("floor: an unknown key", lambda: sbom_floor("billing", FLOOR_KINDS, _summary(cargo="1"), debs=[], chisel=None, arch="amd64"), "FloorError"),
        ("floor: an unknown kind", lambda: sbom_floor("x", {"x": "pip"}, _summary(cargo="1"), debs=[], chisel=None, arch="amd64"), "FloorError"),
        # SMA-665 D5: the cargo floor's OS rule. Mutation: drop `reasons += _chisel_reasons(…)`,
        # and every FloorError row below reds. Mutation: drop the reverse (count) block in
        # _chisel_reasons, and "an SBOM entry not in the list" reds.
        ("floor: cargo without libc6", lambda: _cargo_floor(CHISEL, libc6="false"), "FloorError"),
        ("floor: a list entry the SBOM does not have", lambda: _cargo_floor([BASE_FILES, CA_CERTS]), "FloorError"),
        ("floor: a list entry the SBOM has twice", lambda: _cargo_floor([*CHISEL, LIBC6]), "FloorError"),
        ("floor: an SBOM entry not in the list", lambda: _cargo_floor([*CHISEL, ("openssl", "3.0.13-0ubuntu3.16", "amd64")]), "FloorError"),
        ("floor: a version mismatch", lambda: _cargo_floor([BASE_FILES, CA_CERTS, ("libc6", "2.39-0ubuntu8.8", "amd64")]), "FloorError"),
        ("floor: an arch mismatch", lambda: _cargo_floor([BASE_FILES, CA_CERTS, ("libc6", "2.39-0ubuntu8.9", "arm64")]), "FloorError"),
        ("floor: an SBOM entry with no arch qualifier", lambda: _cargo_floor([*CHISEL, ("gcc-14", "14.2.0-4ubuntu2~24.04.1", "")]), "FloorError"),
        ("floor: a list of another arch", lambda: _cargo_floor(CHISEL, arch="arm64"), "FloorError"),
        ("floor: cargo with no chisel list is exit 2", lambda: _cargo_floor(CHISEL, chisel=None), "UsageError"),
        # SMA-665: sbom-summary counts pkg:deb purls.
        ("sbom: counts deb purls", lambda: sbom_summary(_deb_doc("pkg:deb/ubuntu/libc6@2.39-0ubuntu8.9?arch=amd64", "pkg:cargo/serde@1.0.228"))["deb"], "1"),
        # SMA-665 D5: sbom_debs parses each pkg:deb purl. The first, fourth, sixth and eighth
        # texts are MEASURED syft 1.52.0 output (spec M2).
        (
            "debs: a plain purl",
            lambda: sbom_debs(_deb_doc("pkg:deb/ubuntu/libc6@2.39-0ubuntu8.9?arch=arm64&distro=ubuntu-24.04&upstream=glibc")),
            [("libc6", "2.39-0ubuntu8.9", "arm64")],
        ),
        ("debs: %2B decodes to +", lambda: sbom_debs(_deb_doc("pkg:deb/ubuntu/zz@2.3%2Bdfsg-1?arch=arm64")), [("zz", "2.3+dfsg-1", "arm64")]),
        ("debs: a raw + stays +", lambda: sbom_debs(_deb_doc("pkg:deb/ubuntu/zz@2.3+dfsg-1?arch=arm64")), [("zz", "2.3+dfsg-1", "arm64")]),
        (
            "debs: a raw ~ stays ~",
            lambda: sbom_debs(_deb_doc("pkg:deb/ubuntu/ca-certificates@20260601~24.04.1?arch=all&distro=ubuntu-24.04")),
            [CA_CERTS],
        ),
        ("debs: %7E decodes to ~", lambda: sbom_debs(_deb_doc("pkg:deb/ubuntu/ca-certificates@20260601%7E24.04.1?arch=all")), [CA_CERTS]),
        (
            "debs: a 1%3A epoch is dropped",
            lambda: sbom_debs(_deb_doc("pkg:deb/ubuntu/zz-epoch@1%3A2.3%2Bdfsg-1~x?arch=arm64&distro=ubuntu-24.04")),
            [("zz-epoch", "2.3+dfsg-1~x", "arm64")],
        ),
        ("debs: a raw 1: epoch is dropped", lambda: sbom_debs(_deb_doc("pkg:deb/ubuntu/zz@1:2.3?arch=arm64")), [("zz", "2.3", "arm64")]),
        (
            "debs: no arch qualifier gives an empty arch",
            lambda: sbom_debs(_deb_doc("pkg:deb/ubuntu/gcc-14@14.2.0-4ubuntu2~24.04.1?distro=ubuntu")),
            [("gcc-14", "14.2.0-4ubuntu2~24.04.1", "")],
        ),
        ("debs: no namespace", lambda: sbom_debs(_deb_doc("pkg:deb/libc6@2.39?arch=amd64")), [("libc6", "2.39", "amd64")]),
        ("debs: other purl types are ignored", lambda: sbom_debs(_deb_doc("pkg:cargo/serde@1.0.228", "pkg:npm/next@16.3.5")), []),
        ("debs: no version", lambda: sbom_debs(_deb_doc("pkg:deb/ubuntu/libc6?arch=amd64")), "UsageError"),
        ("debs: too many path segments", lambda: sbom_debs(_deb_doc("pkg:deb/ubuntu/x/libc6@2.39?arch=amd64")), "UsageError"),
        ("debs: a qualifier with no =", lambda: sbom_debs(_deb_doc("pkg:deb/ubuntu/libc6@2.39?arch")), "UsageError"),
        # SMA-665 D5: parse_chisel_list reads the fetch list. The text is MEASURED (spec M4).
        ("chisel: a real fetch list", lambda: parse_chisel_list(CHISEL_TEXT), CHISEL),
        ("chisel: an empty list", lambda: parse_chisel_list(""), "UsageError"),
        ("chisel: only blank lines", lambda: parse_chisel_list("\n\n"), "UsageError"),
        ("chisel: a line that does not parse", lambda: parse_chisel_list("Fetching pool/main/glibc/libc6_2.39_amd64.deb\n"), "UsageError"),
        ("chisel: the raw log form with ... does not parse", lambda: parse_chisel_list("Fetching pool/main/g/glibc/libc6_2.39_amd64.deb...\n"), "UsageError"),
        (
            "chisel: one name with two versions",
            lambda: parse_chisel_list("Fetching pool/main/g/glibc/libc6_2.39-1_amd64.deb\nFetching pool/main/g/glibc/libc6_2.39-2_amd64.deb\n"),
            "UsageError",
        ),
        # chain_kinds reads the registry text; a wrong shape is exit 2, not 3.
        ("kinds: a registry", lambda: chain_kinds('[chain.iam]\nkind = "cargo"\n[chain.iam-console]\nkind = "npm"\n'), {"iam": "cargo", "iam-console": "npm"}),
        ("kinds: no chain table", lambda: chain_kinds("[other]\nx = 1\n"), "UsageError"),
        ("kinds: an entry with no kind", lambda: chain_kinds("[chain.iam]\nversion_file = 'x'\n"), "UsageError"),
        ("kinds: not TOML", lambda: chain_kinds("[chain.iam\n"), "UsageError"),
        *_cli_rows(tmp),
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
    except (OSError, UnicodeDecodeError) as exc:
        raise UsageError(f"cannot read {path}: {exc}") from exc


def _read_sbom(path: Path) -> dict[str, Any]:
    try:
        doc = json.loads(_read_text(path))
    except json.JSONDecodeError as exc:
        raise UsageError(f"{path} is not JSON: {exc}") from exc
    if not isinstance(doc, dict):
        raise UsageError(f"{path} is not an SPDX JSON object")
    return doc


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(prog="release_decision.py", description="SMA-658 image release decisions")
    parser.add_argument("--self-test", action="store_true", help="run the fixture table and exit")
    sub = parser.add_subparsers(dest="command")
    p_oci = sub.add_parser("oci-digests")
    p_oci.add_argument("archive", type=Path)
    p_labels = sub.add_parser("labels")
    p_labels.add_argument("archive", type=Path)
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
    p_floor = sub.add_parser("sbom-floor")
    p_floor.add_argument("--service", required=True)
    p_floor.add_argument("--arch", required=True)
    p_floor.add_argument("--chisel-dir", type=Path, default=Path())
    p_floor.add_argument("sbom", type=Path)
    try:
        args = parser.parse_args(argv)
    except SystemExit as exc:  # argparse exits 2 on a usage error and 0 on --help
        return int(exc.code or 0)

    if args.self_test:
        return self_test()
    try:
        if args.command == "oci-digests":
            _emit(oci_digests(args.archive))
        elif args.command == "labels":
            _emit(labels(args.archive))
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
            _emit(sbom_summary(_read_sbom(args.sbom)))
        elif args.command == "sbom-floor":
            if ARCH_RE.fullmatch(args.arch) is None:
                raise UsageError(f"not an architecture: {args.arch!r}")
            doc = _read_sbom(args.sbom)
            summary = sbom_summary(doc)
            _emit(summary)
            kinds = chain_kinds(_read_text(CHAINS_TOML))
            # SMA-665 D5: the KIND decides whether a chisel list is read, never whether a file
            # exists. A missing list for a cargo key is exit 2 (fail closed).
            debs: list[Deb] = []
            chisel: list[Deb] | None = None
            if kinds.get(args.service) == "cargo":
                chisel = parse_chisel_list(_read_text(args.chisel_dir / f"chisel-manifest-{args.service}-{args.arch}.txt"))
                debs = sbom_debs(doc)
            kind = sbom_floor(args.service, kinds, summary, debs=debs, chisel=chisel, arch=args.arch)
            _emit({"kind": kind, "floor": "pass"})
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
    except FloorError as exc:
        print("floor=fail")
        print(f"release_decision: {exc}", file=sys.stderr)
        return 3
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
