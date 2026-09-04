# SMA-608 — `rs/release-plz.toml` shape validation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `ci/release-plan/release_plan.py` refuse a malformed `rs/release-plz.toml` with `InconclusiveError` (which BUILDS) instead of passing a guard that asserts nothing.

**Architecture:** One new pure validator, `config_sections(cfg)`, asserts five properties of the two `release-plz.toml` sections and returns them; `assert_default_tag_format` and `releasable_packages` consume its output instead of re-deriving it. Coverage is added as `--self-test` fixtures (6 -> 15 rows), the fixture loop is hardened so no helper can exit 1, and an eighth `--negative-control` row proves the new validation reds under mutation.

**Tech Stack:** Python 3.12+ (`tomllib`, stdlib only — `ci/release-plan/pyproject.toml` is zero-dependency), bash, `uv run --locked`, Moon 2.5.3.

**Spec:** `docs/superpowers/specs/2026-09-04-sma-608-release-plz-toml-shape-validation-design.md`

## Global Constraints

- **FAIL-SAFE DIRECTION IS ABSOLUTE.** Every inconclusive outcome must BUILD. `run()` returns `(False, reason)`; `_assert_repo()` returns 3. No change may introduce a path that SKIPs. This outranks every other consideration in this plan.
- **The checker exits 0, 2, or 3 — never 1.** `README.md:139-141` documents it; `run.sh`'s `run_checker` maps 3 -> 1 and everything else -> `die_infra` (2). A traceback out of the interpreter is exit 1 and breaks the contract.
- Every file opens with an SPDX header: `# SPDX-License-Identifier: Apache-2.0`.
- Bash tool PATH lacks the proto CLIs. **Every command in this plan must be preceded by** `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- `ci/**/*.py` is linted by `repo:ruff-ci` against `py/pyproject.toml`'s rule set. `TC003` is selected: a `collections.abc` import used only in an annotation must sit under `if TYPE_CHECKING:`.
- Commits: conventional, workspace-scoped, subject must NOT start with the Linear key — put `(SMA-608)` at the end. Every commit ends with the `Co-Authored-By:` and `Claude-Session:` trailers used on this branch.
- **There is no `repo:release-plan` Moon target.** This suite runs as check 11 of `repo:actionlint`.
- Do not hand-edit `.github/CODEOWNERS` (Moon-generated).

## File Structure

| File | Responsibility | Change |
|---|---|---|
| `ci/release-plan/release_plan.py` | the checker | new `config_sections`; `assert_default_tag_format` signature; `releasable_packages` wiring; 9 new fixtures; hardened `self_test()`; `--collection-count` |
| `ci/release-plan/run.sh` | mode wrapper + negative control | new row 8 |
| `ci/release-plan/README.md` | suite documentation | row counts, new behaviour, stale `TypeError` citation |
| `ci/actionlint/run.sh` | check 11 schedules this suite | `--collection-count` floor; stale "nine lines" prose |
| `ci/actionlint/README.md` | check 11 cost record | subprocess count + timings (M7) |
| `ci/affected-graph/ci_targets.py` | pins `run.sh` lines | tenth `RELEASE_PLAN_SH_CALL_SITES` entry; items 1-9 prose |
| `moon.yml` | `repo:affected-smoke` inputs | "nine load-bearing lines" prose |

---

### Task 1: Harden `self_test()` so no fixture can exit 1

The collection loop calls `err = fn()` bare. Any helper raising anything escapes `main()` and the interpreter exits **1 with a traceback**, violating the never-1 contract. Tasks 2-4 add fixtures whose mutations raise `AttributeError` and `KeyError`, so this must land first. This task also extracts the row tuple to a module-level constant (needed by Task 6's floor) and adds `--collection-count`.

**Files:**
- Modify: `ci/release-plan/release_plan.py` (imports ~`:20-30`; `self_test()` `:433-470`; `main()` `:505-530`)

**Interfaces:**
- Consumes: nothing.
- Produces: `COLLECTION_ROWS: tuple[tuple[str, Callable[[], str | None]], ...]` — the collection-layer row table, defined immediately before `self_test()`. `--collection-count` prints `len(COLLECTION_ROWS)`.

- [ ] **Step 1: Write the failing test** — a temporary throwaway helper that raises, appended to the existing inline tuple in `self_test()` so it runs.

```python
def _tmp_raises_for_task1() -> str | None:
    raise RuntimeError("task 1 probe")
```

Add `("task 1 probe", _tmp_raises_for_task1),` as the last entry of `self_test()`'s existing inline `for label, fn in (...)` tuple.

- [ ] **Step 2: Run it to confirm the contract is broken today**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/sven/dev/paigasus/paigasus-core
uv run --locked --project ci/release-plan --python '>=3.12' python3 \
  ci/release-plan/release_plan.py --self-test; echo "rc=$?"
```

Expected: a `RuntimeError` traceback and `rc=1`. Record the output — this is the defect.

- [ ] **Step 3: Add the `TYPE_CHECKING` import**

In the import block, after `from pathlib import Path`:

```python
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from collections.abc import Callable
```

`from __future__ import annotations` is already at `:21`, so the annotation is a string at runtime. A plain `from collections.abc import Callable` trips ruff `TC003`.

- [ ] **Step 4: Extract the row tuple to a module-level constant**

Insert **immediately before** `def self_test()` — not beside `FIXTURES` at `:228`, which would raise `NameError` at import because the helpers are defined at `:252-430`:

```python
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
)
```

- [ ] **Step 5: Wrap the loop and consume the constant**

Replace `self_test()`'s inline `for label, fn in (...)` block (including the temporary probe row) with:

```python
    # EVERY call is wrapped. A helper that raises anything other than a returned error string
    # would otherwise escape main() and exit the interpreter at 1 — which README.md's "0, 2 or 3,
    # never 1" contract forbids, and which run_checker would then map onto die_infra (2),
    # reporting "uv or the interpreter failed" for a broken repository file. Tasks 2-4 add
    # fixtures whose mutations raise AttributeError and KeyError, so this is load-bearing, not
    # defensive decoration.
    for label, fn in COLLECTION_ROWS:
        try:
            err = fn()
        except Exception as exc:  # noqa: BLE001 - see the comment above; nothing may exit 1 here
            err = f"raised {type(exc).__name__}: {exc}"
        if err:
            print(f"FAIL {label!r}: {err}", file=sys.stderr)
            rc = 3
```

- [ ] **Step 6: Re-run with the probe still present to verify the wrap works**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/sven/dev/paigasus/paigasus-core
uv run --locked --project ci/release-plan --python '>=3.12' python3 \
  ci/release-plan/release_plan.py --self-test; echo "rc=$?"
```

Expected: `FAIL 'task 1 probe': raised RuntimeError: task 1 probe` on stderr and **`rc=3`**, no traceback.

- [ ] **Step 7: Remove the probe**

Delete `_tmp_raises_for_task1` and its `COLLECTION_ROWS` entry. Re-run Step 6's command; expected `rc=0` and no output.

- [ ] **Step 8: Add `--collection-count`**

In `main()`, beside the existing `--fixture-count` handling:

```python
    ap.add_argument("--collection-count", action="store_true")
```

and, immediately after the `--fixture-count` block:

```python
    if args.collection_count:
        print(len(COLLECTION_ROWS))
        return 0
```

**Do not widen `--fixture-count`.** Its consumer at `ci/actionlint/run.sh:4578` validates the output is a single integer with `case "$n" in ''|*[!0-9]*)`.

- [ ] **Step 9: Verify both counters and the full suite**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/sven/dev/paigasus/paigasus-core
R="uv run --locked --project ci/release-plan --python '>=3.12'"
uv run --locked --project ci/release-plan --python '>=3.12' python3 ci/release-plan/release_plan.py --fixture-count
uv run --locked --project ci/release-plan --python '>=3.12' python3 ci/release-plan/release_plan.py --collection-count
bash ci/release-plan/run.sh --self-test; echo "self-test rc=$?"
bash ci/release-plan/run.sh --negative-control; echo "negctl rc=$?"
```

Expected: `9`, `6`, `self-test rc=0`, `negctl rc=0`.

- [ ] **Step 10: Lint**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/sven/dev/paigasus/paigasus-core
bash ci/ruff/run.sh 2>&1 | tail -5
```

Expected: no findings for `ci/release-plan/release_plan.py`.

- [ ] **Step 11: Commit**

```bash
git add ci/release-plan/release_plan.py
git commit -F - <<'MSG'
ci(repo): stop a collection fixture from exiting the interpreter at 1 (SMA-608)

self_test() called each collection-layer helper bare, so anything a helper
raised escaped main() and exited 1 with a traceback -- against README.md's
"0, 2 or 3, never 1" contract, and mapped by run_checker onto die_infra (2).
Measured with a probe helper before the fix.

The row table moves to a module-level constant so --collection-count can
report it, which a later task floors from check 11.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_014mASjm4Xwz4fyKunCz2BSZ
MSG
```

---

### Task 2: `config_sections` — the three shape assertions

Implements AC1 and AC2. Fixtures 7-10.

**Files:**
- Modify: `ci/release-plan/release_plan.py` (`assert_default_tag_format` `:95-102`; `releasable_packages` `:178-183`; new helpers before `COLLECTION_ROWS`)

**Interfaces:**
- Consumes: `InconclusiveError`, `load_toml`, `COLLECTION_ROWS` (Task 1).
- Produces:
  - `config_sections(cfg: dict) -> tuple[dict, list[dict]]`
  - `assert_default_tag_format(workspace: dict, packages: list[dict]) -> None` — **signature change**; the only caller is `releasable_packages`.

- [ ] **Step 1: Write the four failing fixtures**

Insert before `COLLECTION_ROWS`. Each tree has **no `rs/Cargo.toml` and no `rs/crates/`**, so a neutered validator falls through to `load_toml`'s `cannot read …/rs/Cargo.toml` — a *different* message, which is what makes a wrong-reason report possible.

```python
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
```

Append to `COLLECTION_ROWS`:

```python
    ("a non-table [workspace] is inconclusive", _workspace_not_a_table_is_inconclusive),
    ("an array-of-tables [workspace] is inconclusive",
     _workspace_array_of_tables_is_inconclusive),
    ("a table-valued package section is inconclusive",
     _package_not_an_array_of_tables_is_inconclusive),
    ("a non-table [[package]] entry is inconclusive",
     _package_entry_not_a_table_is_inconclusive),
```

- [ ] **Step 2: Run to verify all four fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/sven/dev/paigasus/paigasus-core
uv run --locked --project ci/release-plan --python '>=3.12' python3 \
  ci/release-plan/release_plan.py --self-test; echo "rc=$?"
```

Expected: `rc=3` with four `FAIL` lines. The two `[workspace]` rows and the table-valued `package` row report *"did not raise InconclusiveError"* (today's bypasses — this is the bug reproduced). The `package = ["a"]` row reports *wrong reason* or *raised AttributeError*: record which, verbatim.

- [ ] **Step 3: Write `config_sections`**

Insert immediately after `load_toml`:

```python
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

    return workspace, packages
```

The name and duplicate assertions are Task 3; this task stops at shape.

- [ ] **Step 4: Add the element check**

Append inside `config_sections`, before `return`:

```python
    for i, entry in enumerate(packages):
        if not isinstance(entry, dict):
            raise InconclusiveError(
                f"rs/release-plz.toml's [[package]] entry at index {i} is not a table "
                f"(got {type(entry).__name__})")
```

- [ ] **Step 5: Change `assert_default_tag_format`'s signature**

```python
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
```

- [ ] **Step 6: Wire `releasable_packages`**

Replace its first three statements:

```python
    cfg = load_toml(rs_root / "release-plz.toml")
    workspace, package_entries = config_sections(cfg)
    assert_default_tag_format(workspace, package_entries)
    # BOTH filters are retained verbatim even though config_sections now asserts both
    # properties. They are typed-failure BELTS, not validation: MEASURED, with the element
    # check neutered, `"a".get` raises AttributeError, and with the name check neutered
    # `p["name"]` raises KeyError. Keeping the mutated failure typed is what lets a fixture
    # report its designed wrong-reason string instead of a traceback. Do not "simplify" these
    # away because config_sections looks like it makes them unreachable — that is the point.
    entries = {p["name"]: p for p in package_entries
               if isinstance(p, dict) and isinstance(p.get("name"), str)}
```

- [ ] **Step 7: Run to verify all four pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/sven/dev/paigasus/paigasus-core
uv run --locked --project ci/release-plan --python '>=3.12' python3 \
  ci/release-plan/release_plan.py --self-test; echo "rc=$?"
```

Expected: `rc=0`, no output.

- [ ] **Step 8: M4 — the real repository still passes (AC4)**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/sven/dev/paigasus/paigasus-core
bash ci/release-plan/run.sh --assert; echo "assert rc=$?"
```

Expected: `assert rc=0`.

- [ ] **Step 9: M1, M2, M3 — prove each fixture reds by mutation**

Run each mutation against a **copy**, never the real file. Record each result verbatim; the plan's expectations below are predictions, and where measurement disagrees the finding goes into the commit message.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/sven/dev/paigasus/paigasus-core
T=$(mktemp -d)
run_mut() { # $1 sed expr, $2 label
  sed "$1" ci/release-plan/release_plan.py > "$T/m.py"
  cmp -s ci/release-plan/release_plan.py "$T/m.py" && { echo "$2: VACUOUS - sed matched nothing"; return; }
  echo "--- $2 ---"
  uv run --locked --project ci/release-plan --python '>=3.12' python3 "$T/m.py" --self-test 2>&1 | head -6
  echo "rc=${PIPESTATUS[0]}"
}
run_mut 's/if not isinstance(workspace, dict):/if False and not isinstance(workspace, dict):/' M1
run_mut 's/if not isinstance(packages, list):/if False and not isinstance(packages, list):/'   M2
run_mut 's/if not isinstance(entry, dict):/if False and not isinstance(entry, dict):/'         M3
rm -rf "$T"
```

Expected: each prints `rc=3`. M1 reds rows 7 and 8; M2 reds row 9; M3 reds row 10. **M3's mechanism is expected to differ from the spec's §3.1 prediction** — with the element check neutered, `entry.get("name")` (added in Task 3) or the comprehension raises `AttributeError`, so the row reports `raised AttributeError` via Task 1's wrapper rather than its marker's wrong-reason string. Either is a red; record which occurred.

- [ ] **Step 10: Lint and commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/sven/dev/paigasus/paigasus-core
bash ci/ruff/run.sh 2>&1 | tail -5
git add ci/release-plan/release_plan.py
git commit -F - <<'MSG'
ci(repo): refuse a malformed release-plz.toml section shape (SMA-608)

assert_default_tag_format read [workspace] and [[package]] without checking
their TOML shape, so three malformed-but-valid configs bypassed it rather than
tripping it: workspace = [] substituted {} through a falsy `or`; [[workspace]]
is truthy so the membership test compared against the dict as an element; and
package = { ... } iterated to KEYS, which the isinstance guard skipped.

config_sections now asserts the shapes once and both readers consume it.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_014mASjm4Xwz4fyKunCz2BSZ
MSG
```

---

### Task 3: The two identity assertions — nameless and duplicate `name`

E7's duplicate-`name` path is the only shape in this review whose direction is a **SKIP**. E10's nameless entry is refused on the precedent of `workspace_members`' loud refusal of `[workspace] exclude`.

**Files:**
- Modify: `ci/release-plan/release_plan.py` (`config_sections`; new fixtures; `COLLECTION_ROWS`)

**Interfaces:**
- Consumes: `config_sections` (Task 2), `_shape_fixture` (Task 2).
- Produces: two more `InconclusiveError` markers — `has no string name`, `declares [[package]] name`.

- [ ] **Step 1: Write the two failing fixtures**

```python
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
```

Append to `COLLECTION_ROWS`:

```python
    ("a nameless [[package]] entry is inconclusive", _nameless_package_entry_is_inconclusive),
    ("a duplicated [[package]] name is inconclusive", _duplicate_package_name_is_inconclusive),
```

- [ ] **Step 2: Run to verify both fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/sven/dev/paigasus/paigasus-core
uv run --locked --project ci/release-plan --python '>=3.12' python3 \
  ci/release-plan/release_plan.py --self-test; echo "rc=$?"
```

Expected: `rc=3`, both rows reporting *"did not raise InconclusiveError"*.

- [ ] **Step 3: Add both assertions to `config_sections`**

Replace the element loop written in Task 2 Step 4 with:

```python
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
```

- [ ] **Step 4: Run to verify both pass, and the real repo still does (AC4)**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/sven/dev/paigasus/paigasus-core
uv run --locked --project ci/release-plan --python '>=3.12' python3 \
  ci/release-plan/release_plan.py --self-test; echo "self-test rc=$?"
bash ci/release-plan/run.sh --assert; echo "assert rc=$?"
```

Expected: both `rc=0`. The real config has 13 uniquely-named table entries (verified 2026-09-04).

- [ ] **Step 5: M9 — prove the name fixture reds by mutation**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/sven/dev/paigasus/paigasus-core
T=$(mktemp -d)
sed 's/if not isinstance(name, str) or not name:/if False:/' \
  ci/release-plan/release_plan.py > "$T/m.py"
cmp -s ci/release-plan/release_plan.py "$T/m.py" && echo "VACUOUS"
uv run --locked --project ci/release-plan --python '>=3.12' python3 "$T/m.py" --self-test 2>&1 | head -4
echo "rc=${PIPESTATUS[0]}"
sed 's/if name in seen:/if False:/' ci/release-plan/release_plan.py > "$T/d.py"
uv run --locked --project ci/release-plan --python '>=3.12' python3 "$T/d.py" --self-test 2>&1 | head -4
echo "rc=${PIPESTATUS[0]}"
rm -rf "$T"
```

Expected: both `rc=3`. Confirm the nameless mutation reports a **typed** failure (the retained `isinstance(p.get("name"), str)` belt keeps `p["name"]` from raising `KeyError`) — record which of the two shapes occurred.

- [ ] **Step 6: Lint and commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/sven/dev/paigasus/paigasus-core
bash ci/ruff/run.sh 2>&1 | tail -5
git add ci/release-plan/release_plan.py
git commit -F - <<'MSG'
ci(repo): refuse duplicate and nameless release-plz package entries (SMA-608)

A repeated [[package]] name is the one shape here whose direction is a SKIP.
Measured: the entry map keeps the LAST entry, so a duplicate carrying
release = false drops that crate from the demanded-tag set, and if the other
tags exist the decision reports nothing to release. Nothing caught it at
runtime, since that path never consults EXPECTED_RELEASABLE.

A nameless entry is refused on the precedent of workspace_members' loud
refusal of [workspace] exclude, which rejects the same intent-silently-lost
shape ten lines away.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_014mASjm4Xwz4fyKunCz2BSZ
MSG
```

---

### Task 4: Restore broad-catch coverage, and the marker-distinctness assertion

After Task 2, `workspace = 3` is typed, so `_malformed_config_asserts_three` no longer exercises either broad `except Exception`. E8: narrowing `_assert_repo`'s catch would leave `--self-test` green. `run()`'s catch never had coverage at all.

**Files:**
- Modify: `ci/release-plan/release_plan.py` (new fixtures; `COLLECTION_ROWS`)

**Interfaces:**
- Consumes: `_assert_repo`, `run`, `config_sections`.
- Produces: `_untyped_collection_failure_asserts_three`, `_untyped_collection_failure_builds`, `_markers_are_mutually_exclusive`.

- [ ] **Step 1: Write the three failing fixtures**

```python
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
        except Exception as exc:  # noqa: BLE001 - the point of the fixture
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
```

Append to `COLLECTION_ROWS`:

```python
    ("an untyped collection failure makes --assert exit 3",
     _untyped_collection_failure_asserts_three),
    ("an untyped collection failure makes run() build", _untyped_collection_failure_builds),
    ("the five shape markers are mutually exclusive", _markers_are_mutually_exclusive),
```

- [ ] **Step 2: Run to verify**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/sven/dev/paigasus/paigasus-core
uv run --locked --project ci/release-plan --python '>=3.12' python3 \
  ci/release-plan/release_plan.py --self-test; echo "rc=$?"
```

Expected: `rc=0`. These three assert behaviour that already holds — they are *regression floors*, not bug reproductions, so they pass on first run. Step 3 is what proves they are not vacuous.

- [ ] **Step 3: M8 — narrow each broad catch and confirm the matching fixture reds**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/sven/dev/paigasus/paigasus-core
T=$(mktemp -d)
# _assert_repo's catch is the SECOND `except Exception` in the file; run()'s is the first.
awk '/except Exception as exc:  # deliberately broad/{n++; if(n==1){sub(/except Exception/,"except InconclusiveError")}} {print}' \
  ci/release-plan/release_plan.py > "$T/run.py"
awk '/except Exception as exc:  # deliberately broad/{n++; if(n==2){sub(/except Exception/,"except InconclusiveError")}} {print}' \
  ci/release-plan/release_plan.py > "$T/assert.py"
for f in run assert; do
  cmp -s ci/release-plan/release_plan.py "$T/$f.py" && { echo "$f: VACUOUS"; continue; }
  echo "--- narrowed $f() catch ---"
  uv run --locked --project ci/release-plan --python '>=3.12' python3 "$T/$f.py" --self-test 2>&1 | head -4
  echo "rc=${PIPESTATUS[0]}"
done
rm -rf "$T"
```

Expected: both `rc=3`. Narrowing `run()`'s reds `_untyped_collection_failure_builds`; narrowing `_assert_repo`'s reds `_untyped_collection_failure_asserts_three`. **If either passes, the fixture is vacuous — stop and fix it before continuing.**

- [ ] **Step 4: M10 — confirm the marker assertion is not vacuous**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/sven/dev/paigasus/paigasus-core
T=$(mktemp -d)
sed 's/f"rs\/release-plz.toml.s \[\[package\]\] entry at index {i} has no string name")/f"rs\/release-plz.toml is not a table {i}")/' \
  ci/release-plan/release_plan.py > "$T/m.py"
cmp -s ci/release-plan/release_plan.py "$T/m.py" && echo "VACUOUS - adjust the sed to match the real line"
uv run --locked --project ci/release-plan --python '>=3.12' python3 "$T/m.py" --self-test 2>&1 | head -4
echo "rc=${PIPESTATUS[0]}"
rm -rf "$T"
```

Expected: `rc=3`, with `_markers_are_mutually_exclusive` reporting the collision. If the `sed` reports VACUOUS, adjust it to match the emitted line exactly — do not skip this step.

- [ ] **Step 5: Lint and commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/sven/dev/paigasus/paigasus-core
bash ci/ruff/run.sh 2>&1 | tail -5
git add ci/release-plan/release_plan.py
git commit -F - <<'MSG'
ci(repo): restore coverage of both broad except Exception catches (SMA-608)

Typing the workspace = 3 shape retired the only fixture that produced a
non-InconclusiveError through collection, so _assert_repo's broad catch could
have been narrowed with --self-test still green. run()'s catch never had
coverage at all. Both now have a fixture, measured by narrowing each catch and
confirming the matching row reds.

A third row asserts the five shape markers are mutually exclusive, since
rewording a message could otherwise make one fixture accept another's error.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_014mASjm4Xwz4fyKunCz2BSZ
MSG
```

---

### Task 5: Row 8 of the negative control, and its pin

E5: the collection-layer loop is deletable in silence, exactly as row 7's comment says of the `FIXTURES` loop.

**Files:**
- Modify: `ci/release-plan/run.sh` (after row 7, before `rm -rf "$tmp"`)
- Modify: `ci/affected-graph/ci_targets.py` (`RELEASE_PLAN_SH_CALL_SITES` and its items 1-9 comment)
- Modify: `moon.yml:216-217`, `ci/actionlint/run.sh:2140-2141` (prose counts)

**Interfaces:**
- Consumes: `_workspace_not_a_table_is_inconclusive`'s row label from Task 2.
- Produces: `run.sh` line `if [ "$mut8_rc" != "3" ]; then`, pinned as the tenth call site.

- [ ] **Step 1: Add row 8 to `negative_control()`**

Insert after row 7's block, before `rm -rf "$tmp"`:

```bash
  # Row 8 — the COLLECTION-LAYER loop and the shape validation, the same class row 7 closes for
  # the FIXTURES loop. Delete the collection loop, delete the new fixtures, or neuter
  # config_sections, and --self-test still returns 0 with every other row here passing.
  #
  # The mutation is a CONDITION neutering, not a line deletion. Every `raise InconclusiveError(...)`
  # in release_plan.py spans two physical lines, so a sed that deletes the raise leaves an empty
  # `if` body -> IndentationError -> the mutant exits 1, and this row would red with a diagnostic
  # pointing at the wrong thing while `cmp -s` passed.
  #
  # It asserts on STDERR as well as rc. rc 3 alone is satisfiable by the arity floor in
  # self_test(): delete two rows from COLLECTION_ROWS and the floor fires, self_test() returns 3
  # FOR THE FLOOR, and an rc-only assertion goes green while the neutered check went undetected —
  # the two controls covering for each other's absence. Rows 3/4 grep for a specific verdict line
  # for the same reason.
  local mut8_dir mut8_rc=0 mut8_out
  mut8_dir="$tmp/shape-mutant"
  mkdir -p "$mut8_dir"
  sed 's/if not isinstance(workspace, dict):/if False and not isinstance(workspace, dict):/' \
    "$HERE/release_plan.py" > "$mut8_dir/release_plan.py"
  if cmp -s "$HERE/release_plan.py" "$mut8_dir/release_plan.py"; then
    printf '  FAIL row 8 is vacuous: the shape mutation matched nothing in release_plan.py\n' >&2
    failures=$((failures + 1))
  fi
  mut8_out="$(uv run --locked --project "$HERE" --python '>=3.12' python3 \
    "$mut8_dir/release_plan.py" --self-test 2>&1)" || mut8_rc=$?
  if [ "$mut8_rc" != "3" ]; then
    printf '  FAIL a neutered [workspace] shape check exited %s, expected 3 — the collection\n' \
      "$mut8_rc" >&2
    printf '       loop in self_test() no longer evaluates its rows\n' >&2
    failures=$((failures + 1))
  fi
  if ! printf '%s\n' "$mut8_out" | grep -q "a non-table \[workspace\] is inconclusive"; then
    printf '  FAIL the mutant exited 3 without reporting the non-table [workspace] row — the\n' >&2
    printf '       exit code came from somewhere else (the arity floor, most likely)\n' >&2
    printf '  --- mutant output ---\n%s\n' "$mut8_out" >&2
    failures=$((failures + 1))
  fi
```

- [ ] **Step 2: Verify the pinned line is unique**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/sven/dev/paigasus/paigasus-core
grep -c 'if \[ "\$mut8_rc" != "3" \]; then' ci/release-plan/run.sh
```

Expected: `1`. If not, rename the local until it is — a pin satisfiable by an unrelated identical line asserts nothing.

- [ ] **Step 3: Run the negative control**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/sven/dev/paigasus/paigasus-core
bash ci/release-plan/run.sh --negative-control; echo "rc=$?"
```

Expected: `== release-plan negative control passed ==`, `rc=0`.

- [ ] **Step 4: Prove row 8 is not vacuous — delete the collection loop and confirm it reds**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/sven/dev/paigasus/paigasus-core
cp ci/release-plan/release_plan.py /tmp/rp.bak
python3 - <<'PY'
from pathlib import Path
p = Path("ci/release-plan/release_plan.py"); s = p.read_text()
i = s.index("    for label, fn in COLLECTION_ROWS:")
j = s.index("    return rc", i)
p.write_text(s[:i] + s[j:])
PY
bash ci/release-plan/run.sh --negative-control; echo "rc=$?"
cp /tmp/rp.bak ci/release-plan/release_plan.py && rm /tmp/rp.bak
bash ci/release-plan/run.sh --negative-control; echo "restored rc=$?"
```

Expected: the deleted-loop run prints row 8's FAIL and `rc=1`; the restored run prints `rc=0`. **Restore the file before continuing.**

- [ ] **Step 5: Add the tenth pin entry**

In `ci/affected-graph/ci_targets.py`, append to `RELEASE_PLAN_SH_CALL_SITES`:

```python
    'if [ "$mut8_rc" != "3" ]; then',
```

and extend the enumeration comment above it with:

```
#  10. Row 8's assertion (the collection-layer loop + shape-validation mutant), an ASSERTION
#      line for the same reason as 7 and 8: WORKFLOW_CREDENTIALS_SH_CALL_SITES measured that
#      deleting every assertion left its structural pins byte-identical and the control exited 0
#      having asserted nothing.
```

- [ ] **Step 6: Update the three prose counts**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/sven/dev/paigasus/paigasus-core
grep -n "nine load-bearing lines" moon.yml
grep -n "those nine lines" ci/actionlint/run.sh
grep -n "pins nine" ci/affected-graph/ci_targets.py
```

Change each "nine" to "ten" in: `moon.yml:216-217`, `ci/actionlint/run.sh:2140-2141`, and the `ci_targets.py` header comment. Re-run the greps to confirm zero remaining.

- [ ] **Step 7: Verify the pin gate**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/sven/dev/paigasus/paigasus-core
moon run repo:affected-smoke --force 2>&1 | tail -20
```

Expected: pass. A sub-3s failure mentioning `proto-shim` is the documented infrastructure abort — capture the output, then re-run.

- [ ] **Step 8: Commit**

```bash
git add ci/release-plan/run.sh ci/affected-graph/ci_targets.py moon.yml ci/actionlint/run.sh
git commit -F - <<'MSG'
ci(repo): add a negative-control row for the collection-layer loop (SMA-608)

Row 7's own comment says the FIXTURES loop is deletable in silence; the
collection-layer loop had the identical property and no such row. Row 8
neuters the workspace shape check in a copy and asserts the mutant reds.

It asserts on stderr as well as rc, because rc 3 alone is satisfiable by the
arity floor -- deleting two rows makes self_test return 3 for the floor while
the neutered check goes undetected, the two controls covering for each other.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_014mASjm4Xwz4fyKunCz2BSZ
MSG
```

---

### Task 6: The arity floor and its twin

E4. `FIXTURES`' floor is deliberately duplicated in a second, independently-scheduled file so one edit cannot remove both. `COLLECTION_ROWS` gets the same treatment.

**Files:**
- Modify: `ci/release-plan/release_plan.py` (`self_test()`)
- Modify: `ci/actionlint/run.sh` (`release_plan_self_test`, ~`:4566-4592`)

**Interfaces:**
- Consumes: `COLLECTION_ROWS` (Task 1), `--collection-count` (Task 1).
- Produces: an in-process floor and a check-11 floor, both at 12.

- [ ] **Step 1: Add the in-process floor**

In `self_test()`, immediately before the `for label, fn in COLLECTION_ROWS:` loop:

```python
    # The same reasoning as the FIXTURES floor above, for the collection rows. Deleting a helper
    # from COLLECTION_ROWS otherwise reds nothing: check 11's --fixture-count floor counts
    # FIXTURES only. Floored below the actual count so a legitimate row removal does not abort
    # the gate as infra. Twinned by check 11's --collection-count floor in ci/actionlint/run.sh,
    # in a separately scheduled file, so one edit cannot remove both.
    if len(COLLECTION_ROWS) < 12:
        print(f"FAIL COLLECTION_ROWS has only {len(COLLECTION_ROWS)} row(s); the floor is 12 — "
              "something emptied or gutted the collection-layer table", file=sys.stderr)
        rc = 3
```

- [ ] **Step 2: Verify the count and the floor**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/sven/dev/paigasus/paigasus-core
uv run --locked --project ci/release-plan --python '>=3.12' python3 \
  ci/release-plan/release_plan.py --collection-count
bash ci/release-plan/run.sh --self-test; echo "rc=$?"
```

Expected: `15` and `rc=0`.

- [ ] **Step 3: Correct the `FIXTURES` floor's stale twin pointer (E9a)**

`release_plan.py:437-439` says the twin lives "in ci/release-plan/run.sh's own negative control". It does not — `run.sh` has no count floor. Replace that clause with:

```python
    # that silently stops testing anything still reads as a pass. This floor is IN-PROCESS and
    # deliberately duplicated by a second, independent floor in ci/actionlint/run.sh's check 11
    # (`--fixture-count`), which is scheduled separately from this file — this repo's usual idiom
    # for a self-scheduled gate: two copies in two files, not one shared helper, so deleting
    # either one leaves the other standing.
```

- [ ] **Step 4: Add the check-11 twin**

In `ci/actionlint/run.sh`'s `release_plan_self_test`, after the existing `--fixture-count` floor:

```bash
  # The COLLECTION_ROWS twin. Separate flag, not a widened --fixture-count: that flag's consumer
  # above validates a single integer, and one number cannot floor two tables.
  c="$(uv run --locked --project ci/release-plan --python '>=3.12' python3 \
    ci/release-plan/release_plan.py --collection-count)" \
    || infra "check 11: release_plan.py --collection-count failed"
  case "$c" in ''|*[!0-9]*) infra "check 11: --collection-count printed '$c', expected an integer" ;; esac
  [ "$c" -ge 12 ] || infra "check 11: release_plan.py reports $c collection rows, expected at least 12"
```

Declare `c` in the function's `local` line alongside `n`.

- [ ] **Step 5: Verify check 11**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/sven/dev/paigasus/paigasus-core
bash ci/actionlint/run.sh --self-test 2>&1 | tail -20; echo "rc=$?"
```

Expected: pass.

- [ ] **Step 6: Commit**

```bash
git add ci/release-plan/release_plan.py ci/actionlint/run.sh
git commit -F - <<'MSG'
ci(repo): floor the collection-layer row table, twinned from check 11 (SMA-608)

Deleting a helper from the collection table redded nothing: check 11's
--fixture-count floor counts FIXTURES only. The new floor gets the twin the
FIXTURES floor has, in a separately scheduled file, so one edit cannot remove
both -- a single-copy floor is the same defect one level up.

Also corrects the FIXTURES floor's own comment, which named run.sh as its twin;
run.sh has no count floor anywhere. The twin is check 11.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_014mASjm4Xwz4fyKunCz2BSZ
MSG
```

---

### Task 7: Documentation corrections

E3 (four stale `TypeError` citations), E9b (a quoted message the code cannot emit), and the README's row counts and new behaviour.

**Files:**
- Modify: `ci/release-plan/release_plan.py:212`, `:409`, `:479`, `:319-326`
- Modify: `ci/release-plan/README.md:76`, `:86`, `:141-142`, the `--negative-control` bullet

- [ ] **Step 1: Correct the three code docstrings**

At `:212` (`run()`) and `:479` (`_assert_repo`), replace the `workspace = 3` clause with wording of this shape — keep each docstring's surrounding argument intact:

> `workspace = 3` in `rs/release-plz.toml` USED TO raise a bare `TypeError` from inside
> `assert_default_tag_format`'s membership test. SMA-608 types that shape — `config_sections`
> now raises `InconclusiveError` for it — so this catch exists for the RESIDUAL: shapes the
> validator does not model. It is not decoration; `_untyped_collection_failure_builds` (for this
> catch) and `_untyped_collection_failure_asserts_three` (for `_assert_repo`'s) are fixtures
> that red if either is narrowed, MEASURED against a crate manifest holding `package = 3`.

At `:409` (`_malformed_config_asserts_three`), note that the shape it uses is now typed, that the
row still asserts the 3 contract, and that the untyped-failure coverage moved to the two new rows.

- [ ] **Step 2: Correct `_tag_name_override_is_inconclusive`'s docstring (E9b)**

`:319-326` quotes `"no crate manifests under .../crates — the tree moved"`. The emitted string has
no `/crates` segment, and with this fixture's Cargo.toml-less tree that branch is unreachable
anyway — `crate_manifests` calls `workspace_members` -> `load_toml(rs_root / "Cargo.toml")` and
dies at `cannot read …/rs/Cargo.toml`. Replace the quoted message with the real one and correct
the claimed fall-through. Keep the MEASURED anti-pattern lesson — only the message is wrong.

- [ ] **Step 3: Update `README.md`**

- `:76` — `assert_default_tag_format()` no longer reads the file. State that `config_sections()`
  validates both sections and that a malformed `[workspace]` / `[[package]]`, a nameless entry, or
  a duplicated `name` is now **refused** with `InconclusiveError` rather than ignored. Name the
  duplicate-`name` case as the one whose old direction was a SKIP.
- `:86` — "six collection-layer rows" -> fifteen; enumerate the nine new ones.
- `:141-142` — the `TypeError` citation, same correction as Step 1.
- The `--negative-control` bullet — "seven rows" -> eight; describe row 8 (mutation, `cmp -s`
  vacuity guard, stderr assertion, and why rc alone is insufficient).

- [ ] **Step 4: Verify no stale citations remain**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/sven/dev/paigasus/paigasus-core
grep -rn "TypeError" ci/release-plan/
grep -rn "six collection-layer\|seven rows\|no crate manifests under \.\.\./crates" ci/release-plan/
```

Expected: every `TypeError` hit sits in a corrected passage that says the shape is now typed; the second grep returns nothing.

- [ ] **Step 5: Full suite, then commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/sven/dev/paigasus/paigasus-core
bash ci/release-plan/run.sh --self-test && bash ci/release-plan/run.sh --negative-control \
  && bash ci/release-plan/run.sh --assert && echo "ALL THREE PASS"
git add ci/release-plan/release_plan.py ci/release-plan/README.md
git commit -F - <<'MSG'
docs(repo): correct the release-plan citations SMA-608 falsifies (SMA-608)

Four sites justified a broad except Exception by citing workspace = 3 raising
a bare TypeError. That shape is typed now, so the rationale is restated: the
catch is for the residual, and two fixtures red if either catch is narrowed.

Also corrects _tag_name_override_is_inconclusive, which quoted a message the
code cannot emit and named a fall-through its own tree cannot reach.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_014mASjm4Xwz4fyKunCz2BSZ
MSG
```

---

### Task 8: Full-gate verification and the cost re-measurement

M4, M6, M7.

**Files:**
- Modify: `ci/actionlint/README.md:698-719` (subprocess count and timings)

- [ ] **Step 1: M7 — measure the new cost**

Row 8 adds a seventh `uv run` to `negative_control()`, which check 9's battery runs across 14 mutants plus the unmutated control.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/sven/dev/paigasus/paigasus-core
time bash ci/release-plan/run.sh --self-test
time bash ci/release-plan/run.sh --negative-control
```

Record both. Then update `ci/actionlint/README.md:698-704`'s recorded timings and `:714-719`'s subprocess count, which that section states is load-bearing.

- [ ] **Step 2: M6 — the two gates that schedule this suite**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/sven/dev/paigasus/paigasus-core
moon run repo:actionlint --force 2>&1 | tail -30; echo "actionlint rc=$?"
moon run repo:affected-smoke --force 2>&1 | tail -20; echo "affected-smoke rc=$?"
```

Expected: both pass. On an `affected-smoke` failure under 3s, grep the output for `proto-shim` — that is the documented infrastructure abort, not a real red; capture the output before re-running.

- [ ] **Step 3: Ruff and the wider graph**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/sven/dev/paigasus/paigasus-core
moon run repo:ruff-ci --force 2>&1 | tail -10
moon run repo:input-liveness --force 2>&1 | tail -10
```

Expected: both pass.

- [ ] **Step 4: Confirm every measurement is recorded**

Check that M1-M10 each have a recorded result — in a commit message, a docstring, or the plan's own checkboxes. Any measurement whose observed behaviour differed from this plan's prediction must be written down where the next reader will find it, not silently accepted. In particular: **M3's mechanism is predicted to differ from spec §3.1.** If it did, amend the spec.

- [ ] **Step 5: Commit**

```bash
git add ci/actionlint/README.md docs/superpowers/specs/
git commit -F - <<'MSG'
docs(repo): re-measure check 11's cost after the eighth control row (SMA-608)

Row 8 adds a seventh uv run to negative_control(), which check 9's battery
runs across 14 mutants plus the unmutated control. ci/actionlint/README.md
states the subprocess count is load-bearing, so it is re-measured rather than
estimated.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_014mASjm4Xwz4fyKunCz2BSZ
MSG
```

---

## Spec coverage check

| Spec section | Task |
|---|---|
| §3.1 `config_sections`, signature change, retained belts | 2, 3 |
| §3.2 markers and match substrings | 2, 3; asserted in 4 |
| §3.3 fail-safe direction | enforced throughout; asserted by 4's `run()` row |
| §3.4 fixtures 7-14 | 2, 3, 4 |
| §3.5 `self_test()` cannot exit 1 | 1 |
| §3.6 arity floor + twin | 1 (constant, flag), 6 (floors) |
| §3.7 row 8 | 5 |
| §3.8 registry obligations (pin + 3 prose sites) | 5 |
| §3.9 documentation corrections | 6 (E9a), 7 (E3, E9b, README) |
| M1-M3 | 2 Step 9 |
| M4 | 2 Step 8, 3 Step 4 |
| M5 | 5 Step 3, 7 Step 5 |
| M6 | 8 Step 2 |
| M7 | 8 Step 1 |
| M8 | 4 Step 3 |
| M9 | 3 Step 5 |
| M10 | 4 Steps 1 and 4 |

**Deviation from the spec, deliberate:** M10 is implemented as a permanent fixture
(`_markers_are_mutually_exclusive`, row 15) rather than a one-time measurement. The spec says
"by assertion rather than by reading"; a fixture is the stronger reading and costs ~20 lines.
This makes the final count **15** collection rows, not 14, with the floor at 12.
