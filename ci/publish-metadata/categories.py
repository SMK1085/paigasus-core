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
import os
import re
import sys
import time
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
    # ALLOW_SLUG_SHAPE is an escape hatch into a corruption detector, so — mirroring
    # SKIP_PATTERNS / BRANCH_SKIP / ALLOW_DEAD_INPUT / T_EXEMPT elsewhere in ci/ (see
    # ci/affected-graph/ci_targets.py's bad_exempt check) — its reason is REQUIRED, not
    # merely present as a dict value nobody reads. This must fire whether or not the
    # offending slug even appears in this particular snapshot: an unreviewable blank-reason
    # entry is a defect in the module itself, not a defect conditional on today's input.
    bad_shape_exempt = sorted(
        slug for slug, reason in ALLOW_SLUG_SHAPE.items() if not (reason or "").strip()
    )
    if bad_shape_exempt:
        raise SnapshotError(
            "ALLOW_SLUG_SHAPE entries with no reason: "
            f"{bad_shape_exempt!r}. An exemption from the corruption check is a recorded "
            "decision — give each entry a non-blank reason in "
            "ci/publish-metadata/categories.py, or delete it."
        )

    # NOTE: CRLF itself needs no help here — str.splitlines() already treats "\r\n" (and a
    # lone "\r") as a line boundary and leaves no residual "\r", and load_snapshot's open()
    # uses the default newline=None, which translates CRLF to "\n" before parse_snapshot
    # ever sees the text. Python handles CRLF twice over on its own. What .rstrip() actually
    # defends against is trailing whitespace (spaces/tabs) that a hand-edit can introduce,
    # which would otherwise fail the corruption check below.
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


def validate_live_payload(status: int, body) -> list[str]:
    """Decide whether a fetched payload is trustworthy. Raises FetchError.

    Split out from the request itself so --negative-control can drive the deciding half
    with fixtures. Every branch here is a way a fetch LOOKS successful while carrying
    nothing: a 403 body, a CDN HTML error page, a truncated response, an empty array. Any
    of them reaching --refresh unchecked would overwrite the committed snapshot with
    garbage, which is worse than the typo this gate exists to catch.
    """
    if status != 200:
        raise FetchError(f"crates.io returned HTTP {status} (expected 200)")
    if isinstance(body, bytes):
        try:
            body = body.decode("utf-8")
        except UnicodeDecodeError as exc:
            raise FetchError(f"response body is not valid UTF-8: {exc}") from exc
    try:
        payload = json.loads(body)
    except json.JSONDecodeError as exc:
        raise FetchError(
            f"response body is not valid JSON ({exc}); the first 80 bytes were "
            f"{body[:80]!r} — this is usually an HTML error page from a CDN"
        ) from exc
    if not isinstance(payload, dict) or "category_slugs" not in payload:
        raise FetchError("response JSON has no `category_slugs` key")
    entries = payload["category_slugs"]
    if not isinstance(entries, list):
        raise FetchError("`category_slugs` is not a list")
    slugs = []
    for entry in entries:
        if not isinstance(entry, dict) or not isinstance(entry.get("slug"), str):
            raise FetchError(f"malformed category entry: {entry!r}")
        slugs.append(entry["slug"])
    if not slugs:
        # Never "zero slugs, no error": that is the shape of a vacuous pass.
        raise FetchError("crates.io returned an empty category list")
    if len(slugs) < ABSOLUTE_FLOOR:
        raise FetchError(f"crates.io returned only {len(slugs)} slug(s)")
    return slugs


def fetch_live_slugs(url: str = API_URL) -> list[str]:
    """Thin request wrapper; all judgement lives in validate_live_payload."""
    last: Exception | None = None
    for attempt in range(3):
        try:
            req = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
            with urllib.request.urlopen(req, timeout=30) as resp:
                return validate_live_payload(resp.status, resp.read())
        except FetchError:
            raise
        except urllib.error.HTTPError as exc:
            # MUST precede the (URLError, OSError) arm: HTTPError subclasses URLError
            # subclasses OSError, so the broad arm would otherwise retry a DEFINITIVE
            # answer (403, 404) three times and report it as a connectivity failure.
            # Body is deliberately not read — validate_live_payload rejects on status
            # before it touches the body, and exc.fp can be None, making exc.read() throw.
            validate_live_payload(exc.code, b"")
            raise  # unreachable: validate_live_payload always raises on a non-200
        except (urllib.error.URLError, OSError) as exc:
            last = exc
            if attempt < 2:
                time.sleep(2 ** attempt)
    raise FetchError(f"could not reach {url} after 3 attempts: {last}")


def diff_slug_sets(live: list[str], snapshot: list[str]):
    """Set-based, so file ordering can never cause a false red."""
    live_set, snap_set = set(live), set(snapshot)
    return sorted(live_set - snap_set), sorted(snap_set - live_set)


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

    def expect_fetch_error(label, fn):
        nonlocal checks, failures
        checks += 1
        try:
            fn()
        except FetchError:
            print(f"  ok — {label} raises FetchError")
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
        "CRLF line endings parse (regression guard — Python's own splitlines()/universal "
        "newlines handle this, not .rstrip())",
        lambda: parse_snapshot(body(good).replace("\n", "\r\n"), today),
    )
    expect_ok(
        "a slug line with trailing spaces still parses (this is what .rstrip() defends)",
        lambda: parse_snapshot(body(good)[:-1] + "   \n", today),
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

    def _with_allow_slug_shape(entries, fn):
        # Manipulate the module-level dict inside try/finally so a row cannot leak its
        # mutation into any row that runs after it.
        saved = dict(ALLOW_SLUG_SHAPE)
        try:
            ALLOW_SLUG_SHAPE.clear()
            ALLOW_SLUG_SHAPE.update(entries)
            fn()
        finally:
            ALLOW_SLUG_SHAPE.clear()
            ALLOW_SLUG_SHAPE.update(saved)

    expect_snapshot_error(
        "an ALLOW_SLUG_SHAPE entry with a blank reason is rejected, even when the slug "
        "does not appear in this snapshot (F3 regression guard)",
        lambda: _with_allow_slug_shape(
            {"Weird-Slug": ""}, lambda: parse_snapshot(body(good), today)
        ),
    )
    expect_ok(
        "an ALLOW_SLUG_SHAPE entry with a real reason genuinely silences the corruption "
        "check for that slug (proves the hatch itself still works)",
        lambda: _with_allow_slug_shape(
            {"Weird-Slug": "crates.io shipped this mixed-case slug, see SMA-000"},
            lambda: parse_snapshot(body(good + ["Weird-Slug"]), today),
        ),
    )
    expect_snapshot_error(
        "an HTML error page pasted into the snapshot",
        lambda: parse_snapshot(body(["<html><head><title>502</title>"]), today),
    )
    expect_snapshot_error(
        "a slug longer than MAX_SLUG_LEN",
        lambda: parse_snapshot(body(["a" * (MAX_SLUG_LEN + 1)]), today),
    )
    expect_ok(
        "a slug of exactly MAX_SLUG_LEN chars parses",
        lambda: parse_snapshot(body(["a" * MAX_SLUG_LEN, "data-structures"]), today),
    )
    expect_snapshot_error(
        "a snapshot with exactly 1 slug is below ABSOLUTE_FLOOR",
        lambda: parse_snapshot(body(["data-structures"]), today),
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

    ok_payload = json.dumps({"category_slugs": [{"slug": s} for s in good]})

    expect_ok(
        "a valid payload validates",
        lambda: _assert(validate_live_payload(200, ok_payload) == good, "payload slugs"),
    )
    expect_fetch_error(
        "a 403 response",
        lambda: validate_live_payload(403, b"you must supply a User-Agent"),
    )
    expect_fetch_error(
        "an HTML error page",
        lambda: validate_live_payload(200, b"<html><head><title>502</title></head>"),
    )
    expect_fetch_error(
        "an empty category list",
        lambda: validate_live_payload(200, b'{"category_slugs":[]}'),
    )
    expect_fetch_error(
        "a truncated JSON body",
        lambda: validate_live_payload(200, ok_payload[: len(ok_payload) // 2]),
    )
    expect_fetch_error(
        "a payload with no category_slugs key",
        lambda: validate_live_payload(200, b'{"categories":[]}'),
    )
    expect_fetch_error(
        "category_slugs present but not a list",
        lambda: validate_live_payload(200, b'{"category_slugs":{}}'),
    )
    expect_fetch_error(
        "a malformed category entry (no string slug)",
        lambda: validate_live_payload(200, b'{"category_slugs":[{"name":"x"}]}'),
    )
    expect_fetch_error(
        "a non-empty list below ABSOLUTE_FLOOR",
        lambda: validate_live_payload(
            200, b'{"category_slugs":[{"slug":"data-structures"}]}'
        ),
    )

    def _check_no_retry_on_http_error():
        call_count = 0

        def fake_urlopen(*args, **kwargs):
            nonlocal call_count
            call_count += 1
            raise urllib.error.HTTPError(
                "https://crates.io/api/v1/category_slugs", 403, "Forbidden", {}, None
            )

        original_urlopen = urllib.request.urlopen
        urllib.request.urlopen = fake_urlopen
        try:
            try:
                fetch_live_slugs()
            except FetchError:
                pass
            else:
                raise AssertionError("fetch_live_slugs did not raise FetchError")
        finally:
            urllib.request.urlopen = original_urlopen
        _assert(
            call_count == 1,
            f"a definitive 403 must not be retried — expected 1 urlopen call, got {call_count}",
        )

    expect_ok(
        "a 403 HTTPError is reported without being retried "
        "(regression guard for the URLError/OSError catch ordering)",
        _check_no_retry_on_http_error,
    )

    expect_ok(
        "diff_slug_sets reports an upstream addition",
        lambda: _assert(
            diff_slug_sets(good + ["brand-new"], good) == (["brand-new"], []), "added"
        ),
    )
    expect_ok(
        "diff_slug_sets reports an upstream removal",
        lambda: _assert(
            diff_slug_sets(good[:-1], good) == ([], [good[-1]]), "removed"
        ),
    )
    expect_ok(
        "diff_slug_sets is order-insensitive",
        lambda: _assert(
            diff_slug_sets(list(reversed(good)), good) == ([], []), "order"
        ),
    )

    expected_checks = 35
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


def _cmd_check_freshness(snapshot_path: str) -> int:
    today = datetime.date.today()
    try:
        live = fetch_live_slugs()
    except FetchError as exc:
        print(f"FATAL: {exc}", file=sys.stderr)
        return 2
    try:
        snapshot = load_snapshot(snapshot_path, today)
    except SnapshotError as exc:
        print(f"category snapshot: {exc}", file=sys.stderr)
        return 1
    added, removed = diff_slug_sets(live, snapshot)
    if not added and not removed:
        print(f"category snapshot: fresh ({len(snapshot)} slugs)")
        return 0
    for slug in added:
        print(f"  + {slug} (crates.io added this)", file=sys.stderr)
    for slug in removed:
        print(f"  - {slug} (crates.io no longer has this)", file=sys.stderr)
    print(
        "category snapshot is STALE. This is a metadata-freshness failure, NOT a security "
        "advisory. Fix: run `ci/publish-metadata/run.sh --refresh-categories` and commit "
        f"{snapshot_path}.",
        file=sys.stderr,
    )
    return 1


def _cmd_refresh(snapshot_path: str) -> int:
    # Refuses under CI: this mutates the tree, and a gate that rewrites its own expected
    # data in CI would green itself against whatever it just fetched.
    if os.environ.get("CI"):
        print("FATAL: --refresh mutates the tree and must not run in CI", file=sys.stderr)
        return 2
    try:
        live = fetch_live_slugs()
    except FetchError as exc:
        print(f"FATAL: {exc}", file=sys.stderr)
        return 2
    text = render_snapshot(live, datetime.date.today())
    # Temp file + atomic rename: a partial write must never leave a truncated snapshot
    # behind, because the next run would validate against it.
    tmp_path = snapshot_path + ".tmp"
    with open(tmp_path, "w", encoding="utf-8") as fh:
        fh.write(text)
    os.replace(tmp_path, snapshot_path)
    print(f"category snapshot: wrote {len(live)} slugs to {snapshot_path}")
    return 0


def main(argv: list[str]) -> int:
    args = argv[1:]
    if args == ["--self-test"]:
        return _self_test()
    if len(args) == 3 and args[1] == "--snapshot":
        if args[0] == "--check-freshness":
            return _cmd_check_freshness(args[2])
        if args[0] == "--refresh":
            return _cmd_refresh(args[2])
    print(
        f"usage: {argv[0]} --self-test\n"
        f"       {argv[0]} --check-freshness --snapshot PATH\n"
        f"       {argv[0]} --refresh --snapshot PATH",
        file=sys.stderr,
    )
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv))
