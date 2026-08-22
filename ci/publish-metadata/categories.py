#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""crates.io category slug data for repo:publish-metadata (SMA-529).

WHY A MODULE AND NOT MORE HEREDOCS IN run.sh — the offline PR check and the two network
modes must share ONE implementation of "is this payload trustworthy" and "is this snapshot
intact". run.sh's Python heredocs are separate processes; sharing a function between them
means duplicating it, and a duplicated control rots independently of the copy the real run
uses. This is the same reason ci/affected-graph/ and ci/error-registry/ are modules.

Exit-code contract, inherited from run.sh: 0 pass / 1 the repo is wrong / 2 infrastructure.
SnapshotError maps to 1 (re-running fixes nothing); FetchError maps to 2 (re-running is the
right triage). Nothing here may return a value that reads as "validated" on failure.
"""
import datetime
import difflib
import json
import re
import sys
import urllib.error
import urllib.request

API_URL = "https://crates.io/api/v1/category_slugs"

# crates.io returns 403 to any request without a User-Agent (measured). Pinned here so the
# provenance header's quoted refresh command and the fetch cannot drift apart.
USER_AGENT = "paigasus-core-ci (+https://github.com/SMK1085/paigasus-core)"

# The staleness bound. The scheduled freshness job is the primary drift detector, but a cron
# is NOT a control it can rely on: GitHub delays or silently drops scheduled runs under load
# (security-scan.yml says so in its own header) and disables schedule: triggers entirely
# after 60 days of repository inactivity. Neither produces a red. This bound converts "the
# cron stopped" into a red on the PR path, which someone actually reads.
MAX_SNAPSHOT_AGE_DAYS = 90

# Truncation is caught by comparing the parsed count to the header's `count:`, which is
# exact and self-maintaining. This floor is only a backstop for a body AND header truncated
# together. It is deliberately NOT a "sensible fraction of 104": such a number tolerates
# precisely the 68% cut that the Dependabot lockfile truncation actually was.
ABSOLUTE_FLOOR = 2

MAX_SLUG_LEN = 64

# Escape hatch, mirroring SKIP_PATTERNS / BRANCH_SKIP / ALLOW_DEAD_INPUT / T_EXEMPT
# elsewhere in ci/. crates.io owns this vocabulary; if it ever ships a slug the corruption
# check rejects, `--refresh` would write a snapshot the offline check then rejects — and
# since :publish-metadata is in ci.yml's T=(...) array, that is EVERY PR red with no path
# forward except editing this gate under a red CI. Map slug -> non-blank reason.
ALLOW_SLUG_SHAPE: dict[str, str] = {}

# A CORRUPTION detector, not the slug grammar. It must reject truncation, an HTML error
# page, and CRLF damage without encoding assumptions about what crates.io may name a
# category next. See ALLOW_SLUG_SHAPE for why a strict grammar is a liability here.
_SLUG_CORRUPT_RE = re.compile(r"^[a-z0-9:-]+$")

_SOURCE_LINE = f"# source: {API_URL}"
_REFRESH_LINE = "# refresh: ci/publish-metadata/run.sh --refresh-categories"


class SnapshotError(Exception):
    """The committed snapshot is missing, stale, or corrupt. Maps to exit 1."""


class FetchError(Exception):
    """The live fetch failed or returned an untrustworthy payload. Maps to exit 2."""


def _header_value(lines: list[str], key: str) -> str | None:
    prefix = f"# {key}:"
    for line in lines:
        if line.startswith(prefix):
            return line[len(prefix):].strip()
    return None


def parse_snapshot(text: str, today: datetime.date) -> list[str]:
    """Parse and fully validate a snapshot's text. Raises SnapshotError."""
    # Strip \r BEFORE anything else. This repo ships no .gitattributes (stated as a known
    # fact in ci/affected-graph/ci_targets.py), so on a CRLF checkout every line would carry
    # a trailing \r, fail the corruption check, and red every PR with a message that is
    # wrong about what is broken.
    lines = [line.rstrip() for line in text.splitlines()]

    slugs: list[str] = []
    for line in lines:
        if not line or line.startswith("#"):
            continue
        if len(line) > MAX_SLUG_LEN:
            raise SnapshotError(
                f"slug {line[:MAX_SLUG_LEN]!r}… exceeds {MAX_SLUG_LEN} chars — "
                "the snapshot looks corrupt"
            )
        if not _SLUG_CORRUPT_RE.match(line) and line not in ALLOW_SLUG_SHAPE:
            raise SnapshotError(
                f"snapshot line {line!r} is not a plausible slug (expected only "
                "lowercase letters, digits, '-' and ':'). The snapshot looks corrupt — "
                "regenerate it with `ci/publish-metadata/run.sh --refresh-categories`. "
                "If crates.io genuinely introduced this slug, add it to ALLOW_SLUG_SHAPE "
                "in ci/publish-metadata/categories.py with a reason."
            )
        slugs.append(line)

    if not slugs:
        raise SnapshotError("snapshot contains no slugs — it cannot validate anything")
    if len(slugs) < ABSOLUTE_FLOOR:
        raise SnapshotError(
            f"snapshot contains {len(slugs)} slug(s), below the floor of {ABSOLUTE_FLOOR}"
        )

    declared = _header_value(lines, "count")
    if declared is None:
        raise SnapshotError(
            "snapshot has no `# count:` header, so truncation cannot be detected — "
            "regenerate it with `ci/publish-metadata/run.sh --refresh-categories`"
        )
    try:
        declared_count = int(declared)
    except ValueError as exc:
        raise SnapshotError(f"snapshot `# count:` is not an integer: {declared!r}") from exc
    if declared_count != len(slugs):
        raise SnapshotError(
            f"snapshot declares {declared_count} slugs but contains {len(slugs)} — "
            "the file is truncated or was hand-edited"
        )

    fetched_raw = _header_value(lines, "fetched")
    if fetched_raw is None:
        raise SnapshotError("snapshot has no `# fetched:` header, so staleness is unknowable")
    try:
        fetched = datetime.date.fromisoformat(fetched_raw)
    except ValueError as exc:
        raise SnapshotError(
            f"snapshot `# fetched:` is not an ISO date: {fetched_raw!r}"
        ) from exc
    age = (today - fetched).days
    if age > MAX_SNAPSHOT_AGE_DAYS:
        raise SnapshotError(
            f"snapshot was fetched {age} days ago (max {MAX_SNAPSHOT_AGE_DAYS}). The daily "
            "freshness job may have stopped running — GitHub disables schedule: triggers "
            "after 60 days of repository inactivity. Refresh it with "
            "`ci/publish-metadata/run.sh --refresh-categories`."
        )

    return slugs


def load_snapshot(path: str, today: datetime.date) -> list[str]:
    """Read and validate the snapshot at `path`. Raises SnapshotError."""
    try:
        with open(path, encoding="utf-8") as fh:
            text = fh.read()
    except OSError as exc:
        # Exit 1, not 2: a deleted tracked file is an authorial mistake, and re-running
        # fixes nothing. Same call the file already makes for a missing release-plz.toml.
        raise SnapshotError(
            f"cannot read {path}: {exc}. Restore it, or regenerate it with "
            "`ci/publish-metadata/run.sh --refresh-categories`."
        ) from exc
    return parse_snapshot(text, today)


def render_snapshot(slugs: list[str], fetched: datetime.date) -> str:
    """Render a snapshot file. Sorted in Python (codepoint order), never shell `sort`:
    collation is locale-dependent and these slugs are hyphen-rich, so a refresh on macOS
    would otherwise produce a different byte order than one on ubuntu-latest."""
    ordered = sorted(set(slugs))
    header = [
        "# SPDX-License-Identifier: Apache-2.0",
        "# crates.io category slugs — DO NOT EDIT BY HAND.",
        _SOURCE_LINE,
        f"# fetched: {fetched.isoformat()}",
        f"# count: {len(ordered)}",
        _REFRESH_LINE,
    ]
    return "\n".join(header + ordered) + "\n"


def nearest(slug: str, slugs: list[str]) -> str | None:
    """The 'did you mean' hint. Deterministic: stdlib difflib, fixed cutoff, at most one
    result, and None when nothing qualifies (the caller then omits the clause entirely)."""
    matches = difflib.get_close_matches(slug, slugs, n=1, cutoff=0.8)
    return matches[0] if matches else None


def _self_test() -> int:
    """Counted self-tests. A gate that cannot report red is worse than no gate, and a
    self-test that silently stops running takes the only proof-that-it-bites with it
    (SMA-526), so the count is asserted, not just the outcomes."""
    today = datetime.date(2026, 8, 22)
    fresh = today.isoformat()
    checks = 0
    failures = 0

    def expect_ok(label, fn):
        nonlocal checks, failures
        checks += 1
        try:
            fn()
            print(f"  ok — {label}")
        except Exception as exc:  # noqa: BLE001 - the point is to catch everything
            print(f"  FAILED — {label}: {exc}", file=sys.stderr)
            failures += 1

    def expect_snapshot_error(label, fn):
        nonlocal checks, failures
        checks += 1
        try:
            fn()
        except SnapshotError:
            print(f"  ok — {label} raises SnapshotError")
            return
        except Exception as exc:  # noqa: BLE001
            print(f"  FAILED — {label}: wrong exception {exc!r}", file=sys.stderr)
            failures += 1
            return
        print(f"  FAILED — {label}: did not raise", file=sys.stderr)
        failures += 1

    def body(slugs, count=None, fetched=fresh):
        n = len(slugs) if count is None else count
        head = [
            "# SPDX-License-Identifier: Apache-2.0",
            _SOURCE_LINE,
            f"# fetched: {fetched}",
            f"# count: {n}",
        ]
        return "\n".join(head + slugs) + "\n"

    good = ["data-structures", "parser-implementations", "aerospace::drones"]

    expect_ok(
        "a well-formed snapshot parses",
        lambda: parse_snapshot(body(good), today),
    )
    expect_ok(
        "CRLF line endings are tolerated",
        lambda: parse_snapshot(body(good).replace("\n", "\r\n"), today),
    )
    expect_ok(
        "a ::-nested slug is accepted",
        lambda: parse_snapshot(body(["aerospace::drones", "science::bioinformatics"]), today),
    )
    expect_snapshot_error(
        "an empty snapshot",
        lambda: parse_snapshot(body([]), today),
    )
    expect_snapshot_error(
        "a count that disagrees with the body",
        lambda: parse_snapshot(body(good, count=99), today),
    )
    expect_snapshot_error(
        "a missing count header",
        lambda: parse_snapshot("# fetched: " + fresh + "\n" + "\n".join(good) + "\n", today),
    )
    expect_snapshot_error(
        "a missing fetched header",
        lambda: parse_snapshot("# count: 3\n" + "\n".join(good) + "\n", today),
    )
    expect_snapshot_error(
        "a snapshot older than the staleness bound",
        lambda: parse_snapshot(
            body(good, fetched=(today - datetime.timedelta(days=91)).isoformat()), today
        ),
    )
    expect_ok(
        "a snapshot exactly at the staleness bound",
        lambda: parse_snapshot(
            body(good, fetched=(today - datetime.timedelta(days=90)).isoformat()), today
        ),
    )
    expect_snapshot_error(
        "an uppercase character in a slug",
        lambda: parse_snapshot(body(["Data-Structures"]), today),
    )
    expect_snapshot_error(
        "an HTML error page pasted into the snapshot",
        lambda: parse_snapshot(body(["<html><head><title>502</title>"]), today),
    )
    expect_snapshot_error(
        "a slug longer than MAX_SLUG_LEN",
        lambda: parse_snapshot(body(["a" * (MAX_SLUG_LEN + 1)]), today),
    )
    expect_snapshot_error(
        "a missing snapshot file",
        lambda: load_snapshot("/nonexistent/crates-io-categories.txt", today),
    )
    expect_ok(
        "render_snapshot round-trips through parse_snapshot",
        lambda: parse_snapshot(render_snapshot(good, today), today),
    )
    expect_ok(
        "render_snapshot sorts in codepoint order and dedupes",
        lambda: _assert(
            [ln for ln in render_snapshot(["b", "a", "a"], today).splitlines()
             if not ln.startswith("#")] == ["a", "b"],
            "render_snapshot ordering",
        ),
    )
    expect_ok(
        "nearest suggests a close slug",
        lambda: _assert(
            nearest("data-structure", good) == "data-structures", "nearest hit"
        ),
    )
    expect_ok(
        "nearest returns None when nothing is close",
        lambda: _assert(nearest("zzzzzzzz", good) is None, "nearest miss"),
    )

    expected_checks = 17
    if checks != expected_checks:
        print(
            f"SELF-TEST COUNT CHANGED: ran {checks}, expected {expected_checks}. Update "
            "expected_checks deliberately — a shrinking table is how a control stops "
            "proving anything.",
            file=sys.stderr,
        )
        failures += 1

    if failures:
        print(f"categories self-test: {failures} failure(s)", file=sys.stderr)
        return 1
    print(f"categories self-test: {checks} checks passed")
    return 0


def _assert(condition: bool, label: str) -> None:
    if not condition:
        raise AssertionError(label)


def main(argv: list[str]) -> int:
    if argv[1:] == ["--self-test"]:
        return _self_test()
    print(f"usage: {argv[0]} --self-test", file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv))
