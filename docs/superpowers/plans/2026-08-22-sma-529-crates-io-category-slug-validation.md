<!-- SPDX-License-Identifier: Apache-2.0 -->

# crates.io Category Slug Validation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `repo:publish-metadata` reject a `categories` entry that is not a real crates.io category slug, without introducing a path by which the check passes while asserting nothing.

**Architecture:** A committed snapshot of crates.io's 104 category slugs is validated offline on the PR path. All slug logic lives in a new Python module `ci/publish-metadata/categories.py` with counted self-tests — the repo's established pattern (`ci/affected-graph/ci_targets.py`, `ci/error-registry/check.py`) — because `run.sh`'s Python heredocs are separate processes and cannot share a function between the offline check and the network modes. A daily job in `security-scan.yml` refetches live and reds on drift; an offline staleness bound on the snapshot's `fetched:` date reds when that job stops running.

**Tech Stack:** Bash 3.2-compatible (macOS ships 3.2; CI is ubuntu-latest), Python 3 stdlib only (`json`, `urllib`, `difflib`, `datetime`, `re`), Moon 2.3.2, GitHub Actions.

**Spec:** `docs/superpowers/specs/2026-08-22-sma-529-crates-io-category-slug-validation-design.md`

## Global Constraints

- Every source file opens with an SPDX header: `// SPDX-License-Identifier: Apache-2.0`, `#` for Python/shell/text, `<!-- … -->` for Markdown.
- Exit-code contract, unchanged: `0` pass, `1` the repo is wrong, `2` infrastructure failed. A broken invocation must NEVER read as "all checks passed".
- Python: **stdlib only**. No new dependency may enter `ci/`.
- Sorting is done in Python `sorted()` (codepoint order), **never** shell `sort` — collation is locale-dependent and slugs are hyphen-rich.
- Snapshot comparison is **set-based**, never text-based.
- Slug comparison against `Cargo.toml` is **exact and case-sensitive** (crates.io's publish path uses `.filter(categories::slug.eq_any(slugs))` with no lowercasing).
- Pinned User-Agent, used in exactly one place: `paigasus-core-ci (+https://github.com/SMK1085/paigasus-core)`. crates.io returns 403 without one.
- API endpoint: `https://crates.io/api/v1/category_slugs` (104 slugs incl. `::`-nested). **Not** `/api/v1/categories`, which returns only the 58 top-level ones.
- `MAX_SNAPSHOT_AGE_DAYS = 90`, `ABSOLUTE_FLOOR = 2`, `MAX_SLUG_LEN = 64`.
- Declare and assign on separate lines in bash: `local x="$(cmd)"` masks the command's exit status and would swallow the 1-vs-2 distinction.
- Run every command with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` so `moon` resolves to the repo-pinned version.
- Commit messages: conventional commits with a workspace scope (`feat(ci):`, `fix(ci):`). Subject must start lowercase and be ≤100 chars. No `#NNN` in the body — it breaks commitlint's `footer-leading-blank`.

## Corrections applied during execution

This plan was written before execution; several of its code blocks were corrected while implementing it. This is the record of what was planned — the tasks below are left as originally written. Anyone re-running them verbatim should account for these corrections instead:

- Fixture snapshots in Task 4 need **≥2 slugs** with a matching `# count:` — `ABSOLUTE_FLOOR` is 2 and fires before the count, `fetched:` and staleness guards, so single-slug fixtures red for the wrong reason and the `crlf-snap` row (which expects rc 0) fails outright.
- Task 4 must use `import categories as categories_module` — a local `categories` variable shadows the module.
- `categories.py` carries **no shebang**; SPDX is line 1, matching the sibling `ci/*.py` files.
- `.rstrip()` in `parse_snapshot` guards **trailing whitespace**, not CRLF — `str.splitlines()` and universal-newline translation already handle CRLF, so the plan's CRLF rationale overclaims.
- `fetch_live_slugs` must catch `urllib.error.HTTPError` **before** `(URLError, OSError)`, or a definitive 403 is retried three times.
- Check 4's grep must be anchored to a non-comment `run:` line and also reject `if:` — an unanchored substring match is defeated by commenting the invocation out.
- The self-test count grew from the plan's arithmetic to **37** as review rounds added rows.
- `moon.yml` gained a fourth literal input, `ci/publish-metadata/categories.py`, which the plan's YAML block omitted.

## Measured baseline (do not re-derive)

Exit codes of the eleven existing negative-control fixtures plus the positive control, measured against the unmodified `ci/publish-metadata/run.sh`. Task 1 encodes exactly these:

| Fixture | rc |
|---|---|
| Check 0 (empty publishable set) | **2** |
| Check 0 (unexpected publishable crate) | 1 |
| Check 1 (empty description) | 1 |
| Check 1 (six keywords) | 1 |
| Check 1 (21-char keyword) | 1 |
| Check 1 (keyword with a leading hyphen) | 1 |
| Check 1 (no categories) | 1 |
| Check 3 (0.0.0 crate not release-blocked) | 1 |
| Check 3 (per-package release = true override) | 1 |
| Check 2b (LICENSE not packaged) | 1 |
| Check 2b (moon.yml packaged) | 1 |
| Positive control (clean fixture) | 0 |

Also measured: a Python heredoc reading `sys.argv[4]` with three arguments raises `IndexError` and exits **1** — which the current `_expect_red` reports as "ok — reports red". This is why Task 1 must land before Task 4.

## File Structure

| File | Responsibility |
|---|---|
| `ci/publish-metadata/categories.py` | **Create.** All slug logic: snapshot parse/validate/render, live fetch, payload validation, set diff, nearest-match. Counted self-tests. CLI: `--check-freshness`, `--refresh`, `--self-test`. |
| `ci/publish-metadata/crates-io-categories.txt` | **Create.** The committed snapshot, generated by `--refresh`. |
| `ci/publish-metadata/README.md` | **Create.** Refresh workflow, User-Agent requirement, case-sensitivity finding, Limitations. |
| `ci/publish-metadata/run.sh` | **Modify.** `_expect_rc` harness, Check 1b wiring, call-site pin, two dispatch arms, usage string. |
| `.github/workflows/security-scan.yml` | **Modify.** New `category-slug-freshness` job, `paths:` entry, header comment. |
| `moon.yml` | **Modify.** Two new literal `inputs` on `publish-metadata`. |

---

### Task 1: Migrate the negative control to exact exit-code assertions

`_expect_red` asserts only *non-zero*, so it cannot tell 1 from 2 — the file's headline invariant. It must be replaced before Task 4 adds a fourth argument, or every existing fixture silently becomes a vacuous pass.

**Files:**
- Modify: `ci/publish-metadata/run.sh:286-294` (`_expect_red` → `_expect_rc`), `:307-357` (eleven call sites), `:359-366` (positive control)

**Interfaces:**
- Consumes: nothing.
- Produces: `_expect_rc <want-rc> <label> <cmd…>` — a bash function local to `negative_control()`. Increments `failures` when the observed rc differs from `<want-rc>`; prints `  ok — <label> (rc N)` on match. Tasks 4 and 5 add rows using this signature.

- [ ] **Step 1: Replace the harness**

Replace lines 286-294 (the `_expect_red` definition) with:

```bash
  # Assert an EXACT exit code, not merely "non-zero". The file's headline contract is
  # 0 pass / 1 the repo is wrong / 2 infrastructure (see the header), and a harness that
  # cannot tell 1 from 2 leaves that contract unasserted for every row below. It also
  # silently absorbs a BROKEN INVOCATION: a Python heredoc reading an argument that was
  # never passed raises IndexError and exits 1, which a non-zero check reports as
  # "ok — reports red" while proving nothing about the rule the row names (measured).
  _expect_rc() { # $1 want-rc, $2 label, rest = command
    local want="$1" label="$2"; shift 2
    local got=0
    "$@" >/dev/null 2>&1 || got=$?
    if [ "$got" -ne "$want" ]; then
      echo "NEGATIVE CONTROL FAILED: $label — expected rc $want, got rc $got" >&2
      failures=$((failures + 1))
    else
      echo "  ok — $label (rc $got)"
    fi
  }
```

- [ ] **Step 2: Re-express all eleven rows with exact codes**

Change each `_expect_red "<label>" \` to `_expect_rc <rc> "<label>" \` using the measured baseline table above. Only the first row takes rc 2:

```bash
  _expect_rc 2 "Check 0 (empty publishable set)" \
    metadata_checks "$tmp/empty.json" "$good_rp" "paigasus-kernel"
```

All ten remaining rows take rc 1. Do not change any fixture content in this task.

- [ ] **Step 3: Convert the positive control to the same harness**

Replace lines 359-366 (the hand-rolled `if ! metadata_checks … ; then` block) with:

```bash
  # Positive control: a clean fixture must pass, or every "red" above is meaningless.
  _meta "$tmp/good.json" "$(printf '%s' "$base" | sed 's/"version":"0.0.0"/"version":"0.1.0"/')"
  _expect_rc 0 "clean fixture passes (checks are not vacuously red)" \
    metadata_checks "$tmp/good.json" "$bad_rp" "paigasus-kernel"
```

- [ ] **Step 4: Verify the control still passes**

```bash
bash ci/publish-metadata/run.sh --negative-control
```

Expected: twelve `ok — …` lines, each ending `(rc N)` matching the baseline table, then `negative control: every check reports red on a broken fixture`. Exit 0.

- [ ] **Step 5: Verify the harness itself bites**

Temporarily change the first row's expected rc from `2` to `1`, re-run, and confirm it now FAILS with `expected rc 1, got rc 2`. Revert the change and re-run to confirm it passes again.

This proves the new harness can distinguish 1 from 2 — the entire point of the task.

- [ ] **Step 6: Commit**

```bash
git add ci/publish-metadata/run.sh
git commit -m "test(ci): assert exact exit codes in the publish-metadata negative control"
```

---

### Task 2: Snapshot model in `categories.py`

**Files:**
- Create: `ci/publish-metadata/categories.py`
- Create: `ci/publish-metadata/crates-io-categories.txt`

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `SnapshotError(Exception)` — the committed data is wrong → caller exits **1**.
  - `parse_snapshot(text: str, today: datetime.date) -> list[str]` — raises `SnapshotError`.
  - `load_snapshot(path: str, today: datetime.date) -> list[str]` — raises `SnapshotError` when the file is missing.
  - `render_snapshot(slugs: list[str], fetched: datetime.date) -> str`
  - `nearest(slug: str, slugs: list[str]) -> str | None`
  - Constants `API_URL`, `USER_AGENT`, `MAX_SNAPSHOT_AGE_DAYS`, `ABSOLUTE_FLOOR`, `MAX_SLUG_LEN`, `ALLOW_SLUG_SHAPE`.

- [ ] **Step 1: Write the module with its self-tests**

Create `ci/publish-metadata/categories.py`:

```python
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
```

- [ ] **Step 2: Add the counted self-test block**

Append to `categories.py`:

```python
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
```

- [ ] **Step 3: Add a minimal CLI so the self-test is runnable**

Append:

```python
def main(argv: list[str]) -> int:
    if argv[1:] == ["--self-test"]:
        return _self_test()
    print(f"usage: {argv[0]} --self-test", file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv))
```

- [ ] **Step 4: Run the self-test and verify it passes**

```bash
python3 ci/publish-metadata/categories.py --self-test
```

Expected: 17 `ok — …` lines, then `categories self-test: 17 checks passed`. Exit 0.

- [ ] **Step 5: Verify the self-test bites**

Temporarily change `MAX_SNAPSHOT_AGE_DAYS` to `9999`, re-run, and confirm the row `a snapshot older than the staleness bound raises SnapshotError` FAILS with `did not raise`. Revert and confirm it passes again.

- [ ] **Step 6: Generate the real snapshot**

Write a throwaway one-liner (do NOT add a CLI arm yet — that is Task 3):

```bash
python3 -c "
import datetime, json, sys, urllib.request
sys.path.insert(0, 'ci/publish-metadata')
import categories as c
req = urllib.request.Request(c.API_URL, headers={'User-Agent': c.USER_AGENT})
with urllib.request.urlopen(req, timeout=30) as r:
    payload = json.loads(r.read().decode('utf-8'))
slugs = [e['slug'] for e in payload['category_slugs']]
open('ci/publish-metadata/crates-io-categories.txt', 'w', encoding='utf-8').write(
    c.render_snapshot(slugs, datetime.date.today()))
print('wrote', len(slugs), 'slugs')
"
```

Expected: `wrote 104 slugs` (the exact number may differ if crates.io has changed — that is fine, the header records it).

- [ ] **Step 7: Verify the generated snapshot validates**

```bash
python3 -c "
import datetime, sys
sys.path.insert(0, 'ci/publish-metadata')
import categories as c
s = c.load_snapshot('ci/publish-metadata/crates-io-categories.txt', datetime.date.today())
print('parsed', len(s), 'slugs')
assert 'data-structures' in s and 'parser-implementations' in s
assert any('::' in x for x in s), 'nested slugs missing — wrong endpoint?'
print('kernel slugs present, nested slugs present')
"
```

Expected: the parsed count, then `kernel slugs present, nested slugs present`.

- [ ] **Step 8: Commit**

```bash
git add ci/publish-metadata/categories.py ci/publish-metadata/crates-io-categories.txt
git commit -m "feat(ci): add crates.io category slug snapshot and its validator"
```

---

### Task 3: Live fetch, payload validation, and drift diff

**Files:**
- Modify: `ci/publish-metadata/categories.py`

**Interfaces:**
- Consumes: `SnapshotError`, `FetchError`, `load_snapshot`, `render_snapshot`, `API_URL`, `USER_AGENT` from Task 2.
- Produces:
  - `validate_live_payload(status: int, body: bytes | str) -> list[str]` — raises `FetchError`.
  - `fetch_live_slugs(url: str = API_URL) -> list[str]` — raises `FetchError`.
  - `diff_slug_sets(live: list[str], snapshot: list[str]) -> tuple[list[str], list[str]]` returning `(added, removed)`.
  - CLI arms `--check-freshness --snapshot PATH` (rc 0/1/2) and `--refresh --snapshot PATH` (rc 0/2).

- [ ] **Step 1: Write the validation and diff functions**

Insert into `categories.py` above `_self_test`:

```python
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
        except (urllib.error.URLError, OSError) as exc:
            last = exc
            if attempt < 2:
                time.sleep(2 ** attempt)
    raise FetchError(f"could not reach {url} after 3 attempts: {last}")


def diff_slug_sets(live: list[str], snapshot: list[str]):
    """Set-based, so file ordering can never cause a false red."""
    live_set, snap_set = set(live), set(snapshot)
    return sorted(live_set - snap_set), sorted(snap_set - live_set)
```

Add `import time` to the imports at the top of the file.

- [ ] **Step 2: Add the two CLI arms**

Replace the `main` function from Task 2 with:

```python
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
```

Add `import os` to the imports at the top of the file.

- [ ] **Step 3: Add self-test rows for the new functions**

Insert before the `expected_checks = 17` line in `_self_test`, and add an
`expect_fetch_error` helper next to `expect_snapshot_error`:

```python
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
```

Change `expected_checks = 17` to `expected_checks = 26`.

- [ ] **Step 4: Run the self-test**

```bash
python3 ci/publish-metadata/categories.py --self-test
```

Expected: 26 `ok — …` lines, then `categories self-test: 26 checks passed`. Exit 0.

- [ ] **Step 5: Verify the freshness arm against the live API**

```bash
python3 ci/publish-metadata/categories.py --check-freshness \
  --snapshot ci/publish-metadata/crates-io-categories.txt; echo "rc=$?"
```

Expected: `category snapshot: fresh (N slugs)` and `rc=0`.

- [ ] **Step 6: Verify the freshness arm reds on drift**

```bash
cp ci/publish-metadata/crates-io-categories.txt /tmp/snap-backup.txt
python3 -c "
p='ci/publish-metadata/crates-io-categories.txt'
lines=[l for l in open(p).read().splitlines()]
body=[l for l in lines if not l.startswith('#')]
head=[l for l in lines if l.startswith('#')]
body=body[:-1]
head=['# count: %d'%len(body) if l.startswith('# count:') else l for l in head]
open(p,'w').write('\n'.join(head+body)+'\n')
"
python3 ci/publish-metadata/categories.py --check-freshness \
  --snapshot ci/publish-metadata/crates-io-categories.txt; echo "rc=$?"
cp /tmp/snap-backup.txt ci/publish-metadata/crates-io-categories.txt
```

Expected: a `+ <slug> (crates.io added this)` line, the STALE message, and `rc=1`. The
final `cp` restores the snapshot — confirm with `git diff --stat` showing no change.

- [ ] **Step 7: Verify `--refresh` refuses under CI**

```bash
CI=1 python3 ci/publish-metadata/categories.py --refresh \
  --snapshot ci/publish-metadata/crates-io-categories.txt; echo "rc=$?"
```

Expected: `FATAL: --refresh mutates the tree and must not run in CI` and `rc=2`.

- [ ] **Step 8: Commit**

```bash
git add ci/publish-metadata/categories.py
git commit -m "feat(ci): add live crates.io category fetch, payload validation and drift diff"
```

---

### Task 4: Wire the slug check into `run.sh` Check 1b

This task is atomic by necessity: adding the fourth argument without simultaneously
repairing the eleven call sites and the `base` fixture would leave every existing control
vacuous (measured: `IndexError` → rc 1 → reported as "ok").

**Files:**
- Modify: `ci/publish-metadata/run.sh` — header comment block (`:1-27`), `PYTHONPATH` export near `:30`, `metadata_checks` (`:44-48`, `:115`), `negative_control` (`:296-357`), `main` (`:375-410`)

**Interfaces:**
- Consumes: `load_snapshot`, `nearest`, `SnapshotError` from Task 2.
- Produces: `metadata_checks <metadata.json> <release-plz.toml> <expected-csv> <snapshot-path>` — a fourth positional argument, required. Task 5 does not change this signature.

- [ ] **Step 1: Export `PYTHONPATH` so the heredoc can import the module**

After line 30 (`RS_DIR="$REPO_ROOT/rs"`), add:

```bash
SNAPSHOT="$REPO_ROOT/ci/publish-metadata/crates-io-categories.txt"

# The heredocs below `cd` into rs/, so an import cannot rely on CWD. Exported once rather
# than per-call so --negative-control's direct function calls see it too.
export PYTHONPATH="$REPO_ROOT/ci/publish-metadata${PYTHONPATH:+:$PYTHONPATH}"
```

- [ ] **Step 2: Take the snapshot path as a fourth argument**

Change line 44's comment and line 48's unpacking:

```bash
metadata_checks() { # $1 metadata.json  $2 release-plz.toml  $3 expected-csv  $4 snapshot
```

```python
# A MISSING 4th argument is a broken invocation, not a reason to skip Check 1b. Exit 2:
# silently skipping would make every fixture below pass while asserting nothing, and an
# IndexError here exits 1, which a non-zero-only harness reports as a successful red.
if len(sys.argv) < 5 or not sys.argv[4].strip():
    print("FATAL: the snapshot path was not passed to metadata_checks", file=sys.stderr)
    sys.exit(2)

meta_path, rp_path, expected_csv, snapshot_path = (
    sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
)

import categories

try:
    known_slugs = categories.load_snapshot(snapshot_path, datetime.date.today())
except categories.SnapshotError as exc:
    print(f"category snapshot: {exc}", file=sys.stderr)
    sys.exit(1)
```

Add `import datetime` to the heredoc's existing import line.

- [ ] **Step 3: Add the membership check**

Directly after line 115's `categories = pkg.get("categories") or []` block (after the
existing empty/max-5 rules), add:

```python
    for category in categories:
        if not isinstance(category, str):
            errors.append(f"{name}: category {category!r} is not a string")
            continue
        if category != category.strip():
            errors.append(
                f"{name}: category {category!r} has surrounding whitespace"
            )
            continue
        if category in known_slugs:
            continue
        hint = categories_module.nearest(category, known_slugs)
        suggestion = f" Did you mean {hint!r}?" if hint else ""
        errors.append(
            f"{name}: category {category!r} is not a crates.io category slug."
            f"{suggestion} crates.io DROPS unknown slugs — the publish succeeds and the "
            "crate appears uncategorized, and `cargo publish --dry-run` returns before "
            "the warning that would have told you. crates.io matches slugs EXACTLY and "
            "case-sensitively at publish time (its read API does not, which is why a "
            "browser check misleads). Valid slugs: "
            "ci/publish-metadata/crates-io-categories.txt"
        )
```

Note: the local variable `categories` shadows the module name inside this loop, so import
the module as `categories_module`:

```python
import categories as categories_module
```

and use `categories_module.load_snapshot` / `categories_module.SnapshotError` in Step 2.

- [ ] **Step 4: Repair the shared fixture and all eleven call sites**

In `negative_control`, change line 300-302's `base` so its category is a **real** slug —
otherwise every derived fixture reds on the bogus category rather than on its own mutation,
and the positive control fails outright:

```bash
  local base='{"name":"paigasus-kernel","version":"0.0.0","publish":null,
    "manifest_path":"/nowhere/Cargo.toml","description":"d","license":"Apache-2.0",
    "repository":"r","readme":"README.md","keywords":["k"],
    "categories":["data-structures"]}'
```

Add a fixture snapshot after line 296's `good_rp`/`bad_rp` declarations:

```bash
  # A REAL snapshot for the pre-existing rows. Without it every one of them would fail on
  # a missing 4th argument (rc 2) instead of on the rule it names.
  local fix_snap="$tmp/snapshot.txt"
  printf '# fetched: %s\n# count: 3\ndata-structures\nparser-implementations\naerospace::drones\n' \
    "$(date -u +%Y-%m-%d)" >"$fix_snap"
```

Then append `"$fix_snap"` to all eleven `metadata_checks` call sites and the positive
control. The `assert_package_list` rows are unaffected — they do not call `metadata_checks`.

Also update the Check 1 "no categories" fixture, which currently seds on `["c"]`:

```bash
  _meta "$tmp/no-cat.json" "$(printf '%s' "$base" | sed 's/"categories":\["data-structures"\]/"categories":[]/')"
```

- [ ] **Step 5: Add the new Check 1b fixtures**

After the "no categories" row, add:

```bash
  # --- Check 1b: category slugs must be real crates.io slugs ---------------------------
  _meta "$tmp/bad-cat.json" "$(printf '%s' "$base" | sed 's/"data-structures"/"data-structure"/')"
  _expect_rc 1 "Check 1b (category is not a crates.io slug)" \
    metadata_checks "$tmp/bad-cat.json" "$good_rp" "paigasus-kernel" "$fix_snap"

  # crates.io's PUBLISH path matches exactly (categories::slug.eq_any), while its READ api
  # lowercases. A differently-cased slug is therefore DROPPED at publish, and this row is
  # what stops a future "relax this to case-insensitive" edit reintroducing that false green.
  _meta "$tmp/case-cat.json" "$(printf '%s' "$base" | sed 's/"data-structures"/"Data-Structures"/')"
  _expect_rc 1 "Check 1b (category differs only in case)" \
    metadata_checks "$tmp/case-cat.json" "$good_rp" "paigasus-kernel" "$fix_snap"

  _meta "$tmp/ws-cat.json" "$(printf '%s' "$base" | sed 's/"data-structures"/" data-structures"/')"
  _expect_rc 1 "Check 1b (category has surrounding whitespace)" \
    metadata_checks "$tmp/ws-cat.json" "$good_rp" "paigasus-kernel" "$fix_snap"

  _meta "$tmp/nested-ok.json" "$(printf '%s' "$base" | sed 's/"version":"0.0.0"/"version":"0.1.0"/; s/"data-structures"/"aerospace::drones"/')"
  _expect_rc 0 "Check 1b (::-nested slug present in the snapshot)" \
    metadata_checks "$tmp/nested-ok.json" "$bad_rp" "paigasus-kernel" "$fix_snap"

  _meta "$tmp/nested-bad.json" "$(printf '%s' "$base" | sed 's/"data-structures"/"aerospace::drone"/')"
  _expect_rc 1 "Check 1b (::-nested slug absent from the snapshot)" \
    metadata_checks "$tmp/nested-bad.json" "$good_rp" "paigasus-kernel" "$fix_snap"

  # The snapshot guards, driven through the SAME entry point the real run uses.
  _expect_rc 1 "Check 1b (snapshot file missing)" \
    metadata_checks "$tmp/nested-ok.json" "$bad_rp" "paigasus-kernel" "$tmp/nope.txt"

  printf '# fetched: %s\n# count: 0\n' "$(date -u +%Y-%m-%d)" >"$tmp/empty-snap.txt"
  _expect_rc 1 "Check 1b (snapshot contains no slugs)" \
    metadata_checks "$tmp/nested-ok.json" "$bad_rp" "paigasus-kernel" "$tmp/empty-snap.txt"

  printf '# fetched: %s\n# count: 99\ndata-structures\n' "$(date -u +%Y-%m-%d)" >"$tmp/count-snap.txt"
  _expect_rc 1 "Check 1b (snapshot count disagrees with its body)" \
    metadata_checks "$tmp/nested-ok.json" "$bad_rp" "paigasus-kernel" "$tmp/count-snap.txt"

  printf '# count: 1\ndata-structures\n' >"$tmp/nodate-snap.txt"
  _expect_rc 1 "Check 1b (snapshot has no fetched: header)" \
    metadata_checks "$tmp/nested-ok.json" "$bad_rp" "paigasus-kernel" "$tmp/nodate-snap.txt"

  printf '# fetched: 2000-01-01\n# count: 1\ndata-structures\n' >"$tmp/old-snap.txt"
  _expect_rc 1 "Check 1b (snapshot older than the staleness bound)" \
    metadata_checks "$tmp/nested-ok.json" "$bad_rp" "paigasus-kernel" "$tmp/old-snap.txt"

  printf '# fetched: %s\n# count: 1\n<html><title>502</title>\n' "$(date -u +%Y-%m-%d)" >"$tmp/html-snap.txt"
  _expect_rc 1 "Check 1b (snapshot is an HTML error page)" \
    metadata_checks "$tmp/nested-ok.json" "$bad_rp" "paigasus-kernel" "$tmp/html-snap.txt"

  # CRLF must be TOLERATED — the repo ships no .gitattributes, and rejecting it would red
  # every PR on a CRLF checkout with a message that is wrong about what is broken.
  printf '# fetched: %s\r\n# count: 1\r\naerospace::drones\r\n' "$(date -u +%Y-%m-%d)" >"$tmp/crlf-snap.txt"
  _expect_rc 0 "Check 1b (CRLF snapshot is tolerated)" \
    metadata_checks "$tmp/nested-ok.json" "$bad_rp" "paigasus-kernel" "$tmp/crlf-snap.txt"

  # A broken INVOCATION must not read as "Check 1b skipped".
  _expect_rc 2 "Check 1b (snapshot argument not passed)" \
    metadata_checks "$tmp/nested-ok.json" "$bad_rp" "paigasus-kernel"
```

- [ ] **Step 6: Run the module self-test from within the negative control**

At the top of `negative_control()`, before the first fixture, add:

```bash
  # The module's own controls run here too, so `run.sh --negative-control` is the single
  # command that proves every layer of this gate can report red.
  if ! python3 "$REPO_ROOT/ci/publish-metadata/categories.py" --self-test; then
    echo "NEGATIVE CONTROL FAILED: categories.py self-test" >&2
    failures=$((failures + 1))
  fi
```

- [ ] **Step 7: Pass the real snapshot in `main`**

In `main()` (line ~397), add the fourth argument:

```bash
  publishable="$(metadata_checks "$meta_json" "$RS_DIR/release-plz.toml" "$expected_csv" "$SNAPSHOT")" \
    || status=$?
```

- [ ] **Step 8: Update the header comment block**

In the check list at lines 1-27, after the `Check 1` entry, add:

```bash
#   Check 1b each category is a REAL crates.io slug, validated against the committed
#            snapshot ci/publish-metadata/crates-io-categories.txt. crates.io DROPS unknown
#            slugs and publishes anyway, and `cargo publish --dry-run` returns before the
#            warning that would have said so — so Check 2 cannot catch this (SMA-529).
```

- [ ] **Step 9: Run the negative control**

```bash
bash ci/publish-metadata/run.sh --negative-control
```

Expected: the module self-test's 26 checks, then every `ok — …` row including the thirteen
new Check 1b rows, then `negative control: every check reports red on a broken fixture`.
Exit 0.

- [ ] **Step 10: Run the real gate**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
bash ci/publish-metadata/run.sh
```

Expected: `publish-metadata: paigasus-kernel OK` then `publish-metadata: all checks passed`.

- [ ] **Step 11: Prove Check 1b bites on the real crate**

```bash
cp rs/crates/libs/paigasus-kernel/Cargo.toml /tmp/kernel-backup.toml
sed -i.bak 's/"data-structures"/"data-structure"/' rs/crates/libs/paigasus-kernel/Cargo.toml
bash ci/publish-metadata/run.sh; echo "rc=$?"
cp /tmp/kernel-backup.toml rs/crates/libs/paigasus-kernel/Cargo.toml
rm -f rs/crates/libs/paigasus-kernel/Cargo.toml.bak
git diff --stat rs/
```

Expected: the failure names `data-structure`, suggests `data-structures`, and `rc=1`. The
final `git diff --stat rs/` must show **no** changes — if it shows any, the restore failed.

- [ ] **Step 12: Commit**

```bash
git add ci/publish-metadata/run.sh
git commit -m "feat(ci): validate crates.io category slugs in publish-metadata"
```

---

### Task 5: Pin the freshness job's call site

`--check-categories-freshness` is invoked in exactly one place, a workflow job scheduled by
GitHub rather than Moon. Nothing in the repo's existing guard machinery covers a workflow
job, so deleting it is silent and permanent.

**Files:**
- Modify: `ci/publish-metadata/run.sh` — new `assert_freshness_call_site`, `main` wiring, `negative_control` rows

**Interfaces:**
- Consumes: `_expect_rc` from Task 1.
- Produces: `assert_freshness_call_site <workflow-file>` — returns 0 when the workflow
  invokes the freshness check and carries no suppressing `continue-on-error`, else 1.

- [ ] **Step 1: Write the assertion**

Insert after `assert_package_list` (after line 210):

```bash
# Guard-the-guard (SMA-542): a new check's own CALL SITE is what goes unguarded. The
# fixture rows below exercise this function; only this assertion covers its INVOCATION in
# the workflow, and repo:actionlint's equivalent machinery is keyed on ci.yml alone.
# Takes the workflow path as a FILE so --negative-control drives the same code.
assert_freshness_call_site() { # $1 workflow file
  local wf="$1" rc=0

  if [ ! -f "$wf" ] || [ ! -r "$wf" ]; then
    echo "FATAL: cannot read $wf — the call-site pin cannot assert anything" >&2
    return 2
  fi

  if ! grep -qF -- '--check-categories-freshness' "$wf"; then
    echo "Check 4 FAILED: $wf no longer invokes --check-categories-freshness." >&2
    echo "  The category snapshot's ONLY drift detector would be silently disabled." >&2
    rc=1
  fi

  # Any continue-on-error other than the literal `false` can suppress that job's red. The
  # file carries none today, so a whole-file rule is both simple and strict; a legitimate
  # future use needs an explicit exemption added here with a reason.
  local offending
  offending="$(grep -n 'continue-on-error:' "$wf" | grep -v 'continue-on-error: *false' || true)"
  if [ -n "$offending" ]; then
    echo "Check 4 FAILED: $wf suppresses a failure with continue-on-error:" >&2
    printf '  %s\n' "$offending" >&2
    rc=1
  fi

  return "$rc"
}
```

- [ ] **Step 2: Call it from `main`**

In `main()`, immediately after `cd "$RS_DIR"`, add:

```bash
  # Before anything expensive: if the freshness job is gone, the snapshot is unmaintained
  # and every other check here is validating against data nothing keeps current.
  assert_freshness_call_site "$REPO_ROOT/.github/workflows/security-scan.yml" || exit $?
```

- [ ] **Step 3: Add the fixtures**

In `negative_control`, after the Check 2b rows, add:

```bash
  # --- Check 4: the freshness job's call site ------------------------------------------
  local wf_ok="$tmp/wf-ok.yml" wf_gone="$tmp/wf-gone.yml" wf_coe="$tmp/wf-coe.yml"
  printf 'jobs:\n  freshness:\n    steps:\n      - run: ci/publish-metadata/run.sh --check-categories-freshness\n' >"$wf_ok"
  printf 'jobs:\n  freshness:\n    steps:\n      - run: echo nothing\n' >"$wf_gone"
  printf 'jobs:\n  freshness:\n    steps:\n      - run: ci/publish-metadata/run.sh --check-categories-freshness\n        continue-on-error: true\n' >"$wf_coe"

  _expect_rc 0 "Check 4 (workflow invokes the freshness check)" \
    assert_freshness_call_site "$wf_ok"
  _expect_rc 1 "Check 4 (freshness invocation deleted)" \
    assert_freshness_call_site "$wf_gone"
  _expect_rc 1 "Check 4 (freshness step suppressed by continue-on-error)" \
    assert_freshness_call_site "$wf_coe"
  _expect_rc 2 "Check 4 (workflow file unreadable)" \
    assert_freshness_call_site "$tmp/no-such-workflow.yml"

  # The REAL workflow must satisfy the same assertion the fixtures do.
  _expect_rc 0 "Check 4 (the real security-scan.yml passes)" \
    assert_freshness_call_site "$REPO_ROOT/.github/workflows/security-scan.yml"
```

- [ ] **Step 4: Add the header comment entry**

After the `Check 3` entry in the header block, add:

```bash
#   Check 4  .github/workflows/security-scan.yml still INVOKES the freshness check and does
#            not suppress it with continue-on-error. Nothing else guards a workflow job:
#            repo:actionlint's call-site machinery is keyed on ci.yml only (SMA-529).
```

- [ ] **Step 5: Verify the fixtures fail before the workflow exists**

```bash
bash ci/publish-metadata/run.sh --negative-control; echo "rc=$?"
```

Expected: the four fixture rows pass, and `Check 4 (the real security-scan.yml passes)`
**FAILS** — the workflow job does not exist yet. This is correct: it proves the assertion
is live against the real file. Task 6 makes it green.

- [ ] **Step 6: Commit**

```bash
git add ci/publish-metadata/run.sh
git commit -m "feat(ci): pin the category freshness job's call site in the workflow"
```

---

### Task 6: Workflow job, Moon inputs, dispatch arms, and README

**Files:**
- Modify: `ci/publish-metadata/run.sh:412-421` (dispatch + usage)
- Modify: `.github/workflows/security-scan.yml`
- Modify: `moon.yml` (the `publish-metadata` task's `inputs`, ~line 479)
- Create: `ci/publish-metadata/README.md`

**Interfaces:**
- Consumes: `categories.py`'s `--check-freshness` / `--refresh` arms, `$SNAPSHOT`.
- Produces: `run.sh --check-categories-freshness` and `run.sh --refresh-categories`.

- [ ] **Step 1: Add the two dispatch arms and update the usage string**

Replace the `case` block at lines 412-421:

```bash
case "${1:-}" in
  '') main "$@" ;;
  --negative-control) negative_control ;;
  --check-categories-freshness)
    exec python3 "$REPO_ROOT/ci/publish-metadata/categories.py" \
      --check-freshness --snapshot "$SNAPSHOT" ;;
  --refresh-categories)
    exec python3 "$REPO_ROOT/ci/publish-metadata/categories.py" \
      --refresh --snapshot "$SNAPSHOT" ;;
  -h|--help)
    echo "usage: run.sh [--negative-control | --check-categories-freshness |"
    echo "               --refresh-categories | -h|--help]"
    exit 0 ;;
  *) echo "unknown arg: $1" >&2; exit 2 ;;
esac
```

- [ ] **Step 2: Add the workflow job**

In `.github/workflows/security-scan.yml`, add `ci/publish-metadata/**` to the existing
`pull_request.paths:` **block sequence** (never the inline flow form — `repo:actionlint`'s
extractor fails all four trigger keys loudly on inline flow):

```yaml
      - 'ci/publish-metadata/**'
```

Then append a second job after the `osv` job:

```yaml
  category-slugs:
    name: crates.io category snapshot freshness
    runs-on: ubuntu-latest
    timeout-minutes: 10
    steps:
      - name: Checkout
        uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1  # v7.0.1
        with:
          persist-credentials: false

      # Invoked directly, NOT through `moon run repo:publish-metadata`: Moon reports that
      # task cached whenever the tree is unchanged, which is the normal case here and
      # exactly the state this job exists to re-check against a changed crates.io. Needs no
      # toolchain — python3 is on the runner and the check is stdlib-only.
      - name: crates.io category snapshot freshness
        run: ci/publish-metadata/run.sh --check-categories-freshness
```

- [ ] **Step 3: Correct the workflow's header framing**

The header comment at lines 12-13 says "A red run here means an advisory landed on shipped
code". That is now only true of the `osv` job. Replace those two lines with:

```yaml
# Runs against the default branch. TWO different kinds of red live here, and they triage
# differently:
#   * `osv` red — an advisory landed on shipped code. Triage it, do not let it queue.
#   * `category-slugs` red — our vendored crates.io category snapshot drifted from the
#     live registry. Not a security event: run `ci/publish-metadata/run.sh
#     --refresh-categories` and commit the result (SMA-529).
```

- [ ] **Step 4: Add the Moon inputs**

In `moon.yml`, in the `publish-metadata` task's `inputs`, add two **literal** paths beside
the existing `ci/publish-metadata/run.sh` (do NOT replace it with a glob —
`ci/affected-graph/task_inputs.py` checks `inputFiles` for exact tracked membership but
checks `inputGlobs` only for "matches ≥ 1 tracked file", so a glob would stop redding
`repo:input-liveness` when the snapshot is deleted):

```yaml
      - 'ci/publish-metadata/categories.py'
      - 'ci/publish-metadata/crates-io-categories.txt'
      - '.github/workflows/security-scan.yml'
```

Add a comment above them:

```yaml
      # security-scan.yml is an input because Check 4 ASSERTS ON IT: without this, the
      # call-site pin would serve a cached pass on exactly the PR that deletes the job.
```

- [ ] **Step 5: Write the README**

Create `ci/publish-metadata/README.md`:

````markdown
<!-- SPDX-License-Identifier: Apache-2.0 -->

# `repo:publish-metadata`

Asserts every publishable crate is genuinely releasable (SMA-376), and that its
`categories` are real crates.io slugs (SMA-529).

## Checks

| Check | What it asserts | Failure |
|---|---|---|
| 0 | The publishable set equals `EXPECTED_PUBLISHABLE` | 1 (2 if empty) |
| 1 | Metadata crates.io accepts at upload time | 1 |
| 1b | Every category is a real crates.io slug | 1 |
| 2 | `cargo publish --dry-run` succeeds | 1 / 2 |
| 2b | The packaged file list ships README + LICENSE, not moon.yml | 1 |
| 3 | A 0.0.0 crate is release-blocked | 1 |
| 4 | The freshness job's call site still exists | 1 |

Exit codes: `0` pass, `1` the repo is wrong, `2` infrastructure failed.

## The category snapshot

`crates-io-categories.txt` is a committed snapshot of
`https://crates.io/api/v1/category_slugs`. Refresh it with:

```bash
ci/publish-metadata/run.sh --refresh-categories
```

Two things to know before touching it:

- **crates.io returns 403 without a `User-Agent`.** The pinned one lives in
  `categories.py`.
- **Use `/api/v1/category_slugs`, not `/api/v1/categories`.** The latter returns only the
  58 top-level categories and would falsely reject every `::`-nested subcategory.

### Case sensitivity — the trap

crates.io's **publish** path matches slugs exactly and case-sensitively
(`update_crate` uses `categories::slug.eq_any(slugs)` with no lowercasing). Its **read**
API lowercases (`with_slug` uses `lower(slug)`), so `GET /api/v1/categories/Data-Structures`
returns 200 while publishing `Data-Structures` silently drops the category.

Check 1b is therefore **exact**, and the negative control pins it. Do not "fix" a
case-mismatch red by relaxing the comparison.

### Why an unknown slug is invisible without this gate

crates.io drops unknown slugs and publishes anyway. cargo *does* print
`the following are not valid category slugs and were ignored: …` — but only after
`registry.publish()`, and `cargo publish --dry-run` returns before that call. So Check 2
provably cannot see it, and the only moment it appears is the irreversible upload.

## Limitations

- **L1 — a single combined edit defeats Check 4.** Removing the freshness job from
  `security-scan.yml` *and* the `assert_freshness_call_site` call from `run.sh` in one
  commit passes green. Same bounded shape as `ci/actionlint/README.md`'s L6.
- **L2 — removal detection is not in a required check.** `moon ci` is the only required
  status check; the freshness job is not. A slug retired upstream is caught on the PR path
  only after the 90-day staleness bound forces a refresh.
- **L3 — the staleness bound reds a PR unrelated to categories.** By design: the
  alternative is a freshness mechanism that can switch itself off silently.
````

- [ ] **Step 6: Verify the negative control is now fully green**

```bash
bash ci/publish-metadata/run.sh --negative-control; echo "rc=$?"
```

Expected: every row `ok`, including `Check 4 (the real security-scan.yml passes)`, and
`rc=0`. This is the row that failed at the end of Task 5.

- [ ] **Step 7: Verify the new dispatch arms**

```bash
bash ci/publish-metadata/run.sh --check-categories-freshness; echo "rc=$?"
bash ci/publish-metadata/run.sh --help; echo "rc=$?"
bash ci/publish-metadata/run.sh --typo; echo "rc=$?"
```

Expected: `category snapshot: fresh (N slugs)` / `rc=0`; the usage text / `rc=0`;
`unknown arg: --typo` / `rc=2`.

- [ ] **Step 8: Prove Check 4 bites against the real workflow**

```bash
cp .github/workflows/security-scan.yml /tmp/wf-backup.yml
sed -i.bak 's/--check-categories-freshness/--nothing-at-all/' .github/workflows/security-scan.yml
bash ci/publish-metadata/run.sh; echo "rc=$?"
cp /tmp/wf-backup.yml .github/workflows/security-scan.yml
rm -f .github/workflows/security-scan.yml.bak
git diff --stat .github/
```

Expected: `Check 4 FAILED: … no longer invokes --check-categories-freshness`, `rc=1`, and
`git diff --stat .github/` showing **no** changes after the restore.

- [ ] **Step 9: Commit**

```bash
git add ci/publish-metadata/run.sh ci/publish-metadata/README.md \
  .github/workflows/security-scan.yml moon.yml
git commit -m "feat(ci): schedule crates.io category snapshot freshness checks"
```

---

### Task 7: Full-graph verification

Per-project Moon tasks do not run the repo-level gates. This runs the graph the way CI does.

**Files:** none modified unless a gate reds.

- [ ] **Step 1: Run the affected graph exactly as CI does**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :input-liveness :promtool :observability-drift :nats-permissions :release-parity \
  :release-parity-py :release-parity-ts :publish-metadata --base origin/main \
  --include-relations
```

Expected: all green.

- [ ] **Step 2: If anything reds, diagnose via the CI report**

Moon reports failures unattributed. Get the actual failing task:

```bash
jq '.actions[] | select(.status=="failed") | {label, status}' .moon/cache/ciReport.json
```

Two gates are the likely reds and both have known fixes:
- `repo:input-liveness` — a declared input matches nothing. Confirm all three new literal
  paths are `git add`ed; an untracked declared file fails by design.
- `repo:actionlint` — a trigger-filter problem. Confirm `ci/publish-metadata/**` was added
  as a **block sequence** entry under `paths:`, not inline flow.

- [ ] **Step 3: Confirm the working tree is clean**

```bash
git status --porcelain
```

Expected: empty. Any leftover `.bak` or `.tmp` file from the bite-proving steps in Tasks 4
and 6 is a defect — remove it.

- [ ] **Step 4: Commit any fixes**

```bash
git add -A
git commit -m "fix(ci): satisfy repo gates for the category slug validation"
```

Skip this step if Step 1 was green and Step 3 was clean.

---

## Self-Review

**Spec coverage:**

| Spec section | Task |
|---|---|
| §1 snapshot file, Python sort, set-based compare | 2 (render_snapshot), 3 (diff_slug_sets) |
| §2 Check 1b, exact match, difflib hint, whitespace/non-string | 4 |
| §3 all six guards + exit codes, count-header truncation, corruption detector, `ALLOW_SLUG_SHAPE`, CRLF | 2 (parse_snapshot), 4 (fixtures) |
| §4 urllib, `validate_live_payload`, `fetch_live_slugs`, retries, atomic write, CI refusal, dispatch, usage | 3, 6 |
| §5 strict equality on drift | 3 (`_cmd_check_freshness`) |
| §6 workflow job, `timeout-minutes`, paths block sequence, header comment | 6 |
| §7 guard-the-guard call-site pin | 5 |
| §8 negative control incl. the four required repairs | 1 (`_expect_rc`), 4 (base fixture + eleven call sites + absent-arg row) |
| §9 Moon literal inputs, no `T=()` change | 6 |
| §10 README with Limitations | 6 |
| Success criteria 1-8 | 4 (1,2,3), 2 (4), 3 (5), 5+6 (6), 1+4+5+6 (7), 7 (all) |

**Type consistency:** `load_snapshot(path, today)`, `parse_snapshot(text, today)`,
`validate_live_payload(status, body)`, `diff_slug_sets(live, snapshot) -> (added, removed)`,
`nearest(slug, slugs) -> str | None`, `render_snapshot(slugs, fetched)` are used with these
exact signatures in Tasks 2, 3, 4 and 6. `_expect_rc <want> <label> <cmd…>` is defined in
Task 1 and used in Tasks 4 and 5. `metadata_checks` takes four arguments from Task 4 onward,
including every pre-existing call site.

**Ordering constraint:** Task 1 **must** precede Task 4 — adding the fourth argument while
`_expect_red` is still in place converts all eleven existing controls to vacuous passes, and
nothing in CI would notice. Task 5 leaves one negative-control row deliberately red until
Task 6 adds the workflow job; that is stated in Task 5 Step 5 and is the proof the pin is
live rather than fixture-only.

<!-- moon-diagnosis:superseded -->
> **Superseded (SMA-597).** The `ciReport.json` diagnosis advice above does not work as written:
> there is no action-level `exitCode` key, and the file carries no stdout/stderr at all. The
> measured procedure is in CLAUDE.md between the `moon-diagnosis` markers. This document is left
> otherwise unedited as a record of what was believed when it was written.
