# SMA-738 wasm-lockstep edge flips Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The `wasm-lockstep` checker accepts a dependency edge that cargo re-points between two versions that are already in the lock, reports each moved edge, and the `build` job refuses a lock that container run 2 changed.

**Architecture:** `lockstep_check.py` compares the dependency references of a non-family package by bare name (R-NONFAMILY) and checks each moved reference against cargo's canonical reference table (new refusal R-EDGE). The `lock` command prints the SHA-256 of the lock bytes it judged. The workflow writes that value as a step output before container run 2, and the `stage` step compares the staged lock with it through a new `same` subcommand (new refusal R-RUN2). `pin_check.py` pins the new workflow structure with P18 (now for every job) and a new rule P25.

**Tech Stack:** Python 3.12 stdlib (`tomllib`, `hashlib`) for `lockstep_check.py`; PyYAML (uv project `ci/wasm-lockstep`) for `pin_check.py`; GitHub Actions YAML; bash; cargo 1.95.0 for the reproductions.

**Spec:** `docs/superpowers/specs/2026-10-05-sma-738-wasm-lockstep-edge-flips-design.md` (approved; two adversarial challenges). Read it with this plan. Read the section "Spec questions" at the end of this plan before Task 1.

## Global Constraints

- Work only in the worktree `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-738-wasm-lockstep`, branch `feature/sma-738-wasm-lockstep-edge-flips`. Use absolute paths. Do not touch the main checkout.
- Do not install software (no `brew`, no `pip install`, no `cargo install`). Do not push and do not start a workflow. Only the coordinator does the steps that are marked COORDINATOR, after Sven approved them.
- Every new source file opens with an SPDX header (`# SPDX-License-Identifier: Apache-2.0`).
- Commits: conventional commits with the workspace scope `ci`, for example `fix(ci): ...`. The header is 100 characters or less. Every commit message ends with the line `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Never use `--no-verify`. Do not put a line that starts with `#NNN` in a commit body (commitlint `footer-leading-blank`).
- If `commitlint` is not found at commit time, provision the worktree (`proto install`, then `pnpm -C ts install`). If signing fails with "failed to fill whole buffer", 1Password is locked: ask the coordinator to ask Sven to unlock it.
- Exactly three commits on the feature branch: Task 1 (spec 4.1-4.4), Task 2 (spec 4.5, as ONE commit for a targeted rollback), Task 4 (the README M5 section). Task 3 and Task 5 make no commit.
- `ci/wasm-lockstep/lockstep_check.py` stays stdlib only. The `propose` job runs it with the runner's `python3`.
- The exit codes of `lockstep_check.py` do not change: 0 pass, 3 refusal, 2 infrastructure, 4 no change. `pin_check.py`: 0, 3, 2. `run.sh`: 0, 1, 2.
- Every refusal has an `R-` code. Every self-test row asserts its exact code. A row that must prove a value (not only an outcome) uses a check function that raises `RefusalError("R-SELFTEST", ...)` on a wrong value.
- The new refusal codes are `R-EDGE` (spec 4.2) and `R-RUN2` (spec 4.5). The new output lines are `edge-moved <package> <package version> <dependency> <old> <new>`, `lock-sha256 <64 hex>` and `same: rs/Cargo.lock sha256 <hex> did not change after the lock verdict`.
- `repo:ruff-ci` lints `ci/**/*.py` with the rules of `py/pyproject.toml` (line-length 200, rules E F W I N UP B A C4 SIM TCH RUF, E501 ignored). `ruff format` is NOT gated over `ci/`. Do not reformat existing code.
- PATH for tools, at the top of every shell session: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` and `export PROTO_REPORTER=text`.
- Bash versions on this Mac: `ci/wasm-lockstep/run.sh` works under `/bin/bash` 3.2 and Homebrew bash 5. `repo:actionlint`'s full gate needs Homebrew bash 5 AND a pipe preflight that passes; an rc 2 with a "small" pipe message is a host artifact, not a finding. `repo:ruff-ci` needs bash 4+. `repo:affected-smoke` needs `/bin/bash` 3.2.
- New shell code (workflow `run:` lines, scripts in the repo): never pipe into an early-exit reader (`grep -q`, `head`, `grep -m`, `awk ... exit`) under `pipefail`. Use `grep -qF -- "$lit" < <(printf '%s' "$x")` or `sed -n 1p`. No here-strings. For local one-off scripts, write the script to a file in the scratchpad with the Write tool and run `python3 <file>`; do not use a large here-document (Homebrew bash 5.3.15 can deadlock on one over 512 bytes on this host).
- The workflow `run:` scripts are linted by `pin_check.py` P5. I checked `COMMON_WORDS` in `ci/wasm-lockstep/pin_check.py:94-95`: `if then else elif fi for do done while until ! { } exit true set read [ test echo printf cat sed`. The `build` job adds `df sudo rm git tar mkdir cp chmod python3 docker` (`pin_check.py:97`). This plan adds only the words `sed`, `echo` and `python3` to `build` steps. All three are allowed. Do not add another command word.
- Do not write the literal file name of moon's CI report JSON (the CI report file that moon writes into `.moon/cache`) in any file of this change. `repo:actionlint` check 12 reds a file that names it without a marker.
- Scratch files go to `SCRATCH=/private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/31ebe72c-c15a-47ff-9c97-71e29e9e5eb6/scratchpad`.
- Prose in the repo files of this change (README, CLAUDE.md, comments, docstrings) uses ASD-STE100 Simplified Technical English: short sentences, active voice, no idiom.

## Review Focus

1. A FIFO or a directory at `same --file`: a person expects a fast `R-RUN2`, not a hang and not an infrastructure error. Test: Task 2, Step 1, the rows "same: a FIFO returns at once" and "same: a directory".
2. The canonical-form rule against what cargo really wrote: every reference in HEAD's real `rs/Cargo.lock` must be a key of `ref_table`. If it is not, every weekly run refuses with `R-EDGE`. Test: Task 1, Step 1, the negative-control row "every dependency reference of HEAD's lock is a canonical reference".
3. A package that depends on two versions of one name, and cargo moves only one of them: a person expects a pass with one edge row, not `R-EDGE`. Test: Task 1, Step 1, the row "one of two references to one name moves".
4. The workflow's `sed` extraction of `lock-sha256` and the checker's print format must agree. If they differ, `lock_sha256` is empty and every bump goes red with exit 2. Test: Task 2, Step 12 (the real `sed` line on real checker output) and Task 4 (M5 positive run).
5. The `edge-moved` lines on exit 4 must not change what the workflow reads from the `lock` output: the `family-current wasm-bindgen` line. Test: Task 3, Step 4 (the workflow's `sed` on the real exit-4 output).

---

## File map

| File | Task | Change |
|---|---|---|
| `ci/wasm-lockstep/lockstep_check.py` | 1, 2 | Bare-name compare, `ref_table`, `edge_verdict`, `LockResult`, `NoChangeError.edges`, `edge-moved` output, body table (1). `read_file`, `read_lock`, `run_same`, `same` subcommand, `lock-sha256` output, repeated-flag refusal (2). Self-test and negative-control rows for both. |
| `ci/wasm-lockstep/pin_check.py` | 2 | P18 for every job, `same` in `CHECKER_SUBS`, P25, `RUNNER_TEMP` rule, new `FIXTURE`, 13 mutation rows, 3 real-workflow mutations, "P0-P25". |
| `.github/workflows/wasm-lockstep.yml` | 2 | The `lock` step writes `lock_sha256`; the `stage` step gets `env:` and the final `same` command; three comments. |
| `ci/wasm-lockstep/README.md` | 1, 2, 4 | R-NONFAMILY, R-EDGE, edge output, R-NOCHANGE, a gap (1). R-RUN2, `same`, P18, P25, trust model, removed gap (2). M5 (4). |
| `rs/CLAUDE.md` | 1, 2 | Runbook: diff comment, R-EDGE (1); R-RUN2 (2). |
| `docs/superpowers/specs/2026-09-27-sma-693-wasm-bindgen-lockstep-updater-design.md` | 1 | A note under the 5.3 invariant. |
| `.github/workflows/wasm-lockstep-m5.yml`, scratch `rs/Cargo.lock`, scratch `ci/wasm-lockstep/container.sh` | 4 | On the scratch branch `feature/sma-738-m5-scratch` ONLY. Never merged. |

---

### Task 1: The edge rule (spec 4.1-4.4) in `lockstep_check.py`, its tests and its documents

**Files:**
- Modify: `ci/wasm-lockstep/lockstep_check.py` (imports `:22-29`, classes `:69-82`, `normalise_ref` `:119-125`, `lock_verdict` `:151-217`, `body_for` `:288-313`, `run_artifact` `:333-347`, `_ref_rows` `:394-411`, `self_test` `:500-551`, `negative_control` `:554-593`, `main` `:621-637`)
- Modify: `ci/wasm-lockstep/README.md:43-66` and the list "What the checks do not prove" (`:124-158`)
- Modify: `rs/CLAUDE.md:195-198` and `:207`
- Modify: `docs/superpowers/specs/2026-09-27-sma-693-wasm-bindgen-lockstep-updater-design.md:252-254`
- Test: the in-file self-test (`--self-test`) and negative control (`--negative-control`)

**Interfaces:**
- Consumes: nothing from other tasks.
- Produces (Task 2 relies on these exact names):
  - `Key = tuple[str, str, str]`, `FamilyRow = tuple[str, str, str]`, `EdgeRow = tuple[str, str, str, str, str]` (module-level type aliases).
  - `class LockResult(NamedTuple)` with fields `family: list[FamilyRow]` and `edges: list[EdgeRow]`.
  - `class NoChangeError(Exception)` with `__init__(self, message: str, edges: list[EdgeRow] | None = None)` and the attribute `edges: list[EdgeRow]`.
  - `def ref_table(by_key: dict[Key, dict]) -> dict[str, Key]`.
  - `def edge_verdict(old_by_key: dict[Key, dict], new_by_key: dict[Key, dict]) -> list[EdgeRow]`.
  - `def lock_verdict(old: dict, new: dict) -> LockResult`.
  - `def body_for(changes: list[FamilyRow], edges: list[EdgeRow] | None = None) -> str`.
  - `def run_artifact(directory: str, old_path: str, body_file: str | None, title_file: str | None) -> LockResult` (Task 2 changes the return type to `tuple[LockResult, str]`).
  - `def _print_family(rows: list[FamilyRow]) -> None`, `def _print_edges(rows: list[EdgeRow]) -> None`.
  - Test helpers: `_flip_rest`, `_expect_edges`, `_edges_check`, `_edge_rows`, `_edge_body_rows`, `_bump_family`, `_edge_target`, `_all_refs_canonical`; constants `WINDOWS_SYS`, `FLIP`, `FLIP_EDGES`, `CHANGED_REST`, `FOUR_WINDOWS_SYS`, `EDGE_HEADING`.

- [ ] **Step 1: Write the failing self-test and negative-control rows**

The two family-form rows of `_ref_rows` (`name version` and `name version (source)`) change from PASS to R-EDGE (spec 5.2). The bare-name family row stays PASS.

1a. Add the constant `EDGE_HEADING` next to `RUNBOOK` (`:66`), because `body_for` (Step 3) uses it too:

```python
# The heading of the second table of the PR body (SMA-738 spec 4.3).
EDGE_HEADING = "### Dependency edges that moved"
```

1b. Replace the whole function `_ref_rows` (`:394-411`) with:

```python
def _ref_rows() -> list[tuple[str, object, str]]:
    """A family reference in a non-family package. In a lock that passes R-DUPLICATE, a family
    name has one version, so cargo writes the bare name only. A family move then does not move the
    reference, and the bare-name row stays PASS. The `name version` and `name version (source)`
    forms are not what cargo writes for such a lock, so a move of them refuses with R-EDGE
    (SMA-738 spec 4.2 and 5.2). The last row proves that an unlocked NON-family reference refuses
    with R-EDGE: 1.0.99 is not in the lock (SMA-738 spec 5.2)."""
    rows = []
    forms = (
        ("name", "wasm-bindgen", "wasm-bindgen", "PASS"),
        ("name version", "wasm-bindgen 0.2.128", "wasm-bindgen 0.2.129", "R-EDGE"),
        ("name version (source)", f"wasm-bindgen 0.2.128 ({CRATES_IO})", f"wasm-bindgen 0.2.129 ({CRATES_IO})", "R-EDGE"),
    )
    for label, old_ref, new_ref, want in forms:
        old = _lock(rest=REST + _pkg("consumer", "1.0.0", deps=(old_ref,)))
        new = _lock(NEW_FAMILY, rest=REST + _pkg("consumer", "1.0.0", deps=(new_ref,)))
        rows.append((f"reference form `{label}` to a family package", _verdict(old, new), want))
    twin_old = _lock(rest=REST + _pkg("consumer", "1.0.0", deps=("anyhow 1.0.98",)))
    twin_new = _lock(NEW_FAMILY, rest=REST + _pkg("consumer", "1.0.0", deps=("anyhow 1.0.99",)))
    rows.append(("an unlocked `name version` reference to a NON-family package", _verdict(twin_old, twin_new), "R-EDGE"))
    return rows
```

1c. Directly after `_ref_rows`, add the fixtures and the row builders of spec 5.1. The rows that check only a code come first, so that Step 2 shows them as FAIL lines before a value row stops the run:

```python
# ---- SMA-738: the edge rows (spec 5.1) ---------------------------------------------------------

# The real case of SMA-738: two windows-sys versions, and five packages whose one reference to
# windows-sys moved from 0.61.2 to 0.52.0. Two versions of the name are in the lock, so cargo
# writes each reference in the `name version` form.
WINDOWS_SYS = _pkg("windows-sys", "0.52.0", checksum="c" * 64) + _pkg("windows-sys", "0.61.2", checksum="d" * 64)
FOUR_WINDOWS_SYS = _pkg("windows-sys", "0.45.0", checksum="1" * 64) + _pkg("windows-sys", "0.48.0", checksum="2" * 64) + WINDOWS_SYS
FLIP = (("errno", "0.3.14"), ("quinn-udp", "0.5.15"), ("rustix", "1.1.5"), ("tempfile", "3.27.0"), ("winapi-util", "0.1.11"))
FLIP_EDGES = [(name, version, "windows-sys", "0.61.2", "0.52.0") for name, version in FLIP]
CHANGED_REST = REST.replace('version = "1.0.98"', 'version = "1.0.99"')


def _flip_rest(target: str = "0.61.2", errno_deps: tuple[str, ...] | None = None, base: str = REST, extra: str = WINDOWS_SYS) -> str:
    """`base`, the windows-sys entries in `extra`, and the five packages of the real case. Each of
    the five refers to `windows-sys <target>`. `errno_deps` replaces the dependencies of errno."""
    out = base + extra
    for name, version in FLIP:
        deps = errno_deps if name == "errno" and errno_deps is not None else (f"windows-sys {target}",)
        out += _pkg(name, version, checksum="e" * 64, deps=deps)
    return out


def _expect_edges(old: dict, new: dict, want: list[EdgeRow]):
    """A row that proves the edge rows, not only the outcome. A wrong list raises R-SELFTEST. A
    NoChangeError with the right rows is raised again, so the row outcome is NOCHANGE."""
    def check() -> None:
        try:
            result = lock_verdict(old, new)
        except NoChangeError as exc:
            if exc.edges != want:
                raise RefusalError("R-SELFTEST", f"the NoChangeError carries {exc.edges}, want {want}") from exc
            raise
        if result.edges != want:
            raise RefusalError("R-SELFTEST", f"the verdict holds the edge rows {result.edges}, want {want}")
    return check


def _edges_check(old_text: str, new_text: str, want: list[EdgeRow]):
    return _expect_edges(parse_lock_text(old_text, "old"), parse_lock_text(new_text, "new"), want)


def _edge_rows() -> list[tuple[str, object, str]]:
    old = _flip_rest()
    flipped = _flip_rest("0.52.0")
    unlocked = _flip_rest(errno_deps=("windows-sys 0.60.0",))
    git_source = "git+https://github.com/example/windows-sys#" + "f" * 40
    same_pair = WINDOWS_SYS + _pkg("windows-sys", "0.52.0", source=git_source, checksum=None)
    toml_datetime = _pkg("toml_datetime", "0.6.11", checksum="7" * 64) + _pkg("toml_datetime", "1.1.1+spec-1.1.0", checksum="8" * 64)
    return [
        # The code rows.
        ("an edge flip and a version change of a non-family package", _verdict(_lock(rest=old), _lock(NEW_FAMILY, rest=_flip_rest("0.52.0", base=CHANGED_REST))), "R-NONFAMILY"),
        ("an edge flip and a new dependency name", _verdict(_lock(rest=old), _lock(NEW_FAMILY, rest=_flip_rest("0.52.0", errno_deps=("anyhow", "windows-sys 0.52.0")))), "R-NONFAMILY"),
        ("an added reference to an unlocked version, with a family move", _verdict(_lock(rest=old), _lock(NEW_FAMILY, rest=unlocked)), "R-EDGE"),
        ("an added reference to an unlocked version, no family move", _verdict(_lock(rest=old), _lock(rest=unlocked)), "R-EDGE"),
        ("an added reference to an unlocked version, and a family version that fails R-SEMVER", _verdict(_lock(rest=old), _lock((("wasm-bindgen", "0.2.129-rc.1"),), rest=unlocked)), "R-EDGE"),
        ("an added reference with a Markdown payload after a valid reference", _verdict(_lock(rest=old), _lock(NEW_FAMILY, rest=_flip_rest(errno_deps=("windows-sys 0.52.0 [x](http://evil)",)))), "R-EDGE"),
        ("an added bare-name reference when two versions of the name are in the lock", _verdict(_lock(rest=old), _lock(NEW_FAMILY, rest=_flip_rest(errno_deps=("windows-sys",)))), "R-EDGE"),
        ("a `name version (source)` reference with a source that is not in the lock", _verdict(_lock(rest=_flip_rest(extra=same_pair)), _lock(NEW_FAMILY, rest=_flip_rest(errno_deps=("windows-sys 0.52.0 (registry+https://evil.example/index)",), extra=same_pair))), "R-EDGE"),
        ("a removed reference that is not in the old lock's table", _verdict(_lock(rest=_flip_rest(errno_deps=("windows-sys",))), _lock(NEW_FAMILY, rest=_flip_rest())), "R-EDGE"),
        ("a new dependencies list with the same reference two times", _verdict(_lock(rest=_flip_rest(errno_deps=("windows-sys 0.52.0", "windows-sys 0.52.0"))), _lock(NEW_FAMILY, rest=_flip_rest(errno_deps=("windows-sys 0.52.0", "windows-sys 0.52.0")))), "R-EDGE"),
        ("two moved references of one dependency name on one side of one package", _verdict(_lock(rest=_flip_rest(errno_deps=("windows-sys 0.45.0", "windows-sys 0.48.0"), extra=FOUR_WINDOWS_SYS)), _lock(NEW_FAMILY, rest=_flip_rest(errno_deps=("windows-sys 0.52.0", "windows-sys 0.61.2"), extra=FOUR_WINDOWS_SYS))), "R-EDGE"),
        ("an added family reference with a payload after the family name", _verdict(_lock(rest=REST + _pkg("consumer", "1.0.0", deps=("wasm-bindgen",))), _lock(NEW_FAMILY, rest=REST + _pkg("consumer", "1.0.0", deps=("wasm-bindgen [x](http://evil)",)))), "R-EDGE"),
        # The value rows.
        ("the real five-package edge flip, with a family move", _edges_check(_lock(rest=old), _lock(NEW_FAMILY, rest=flipped), FLIP_EDGES), "PASS"),
        ("the real five-package edge flip, no family move", _edges_check(_lock(rest=old), _lock(rest=flipped), FLIP_EDGES), "NOCHANGE"),
        ("a `name version (source)` reference when two packages share the name and version: the correct source", _edges_check(
            _lock(rest=_flip_rest(extra=same_pair)),
            _lock(NEW_FAMILY, rest=_flip_rest(errno_deps=(f"windows-sys 0.52.0 ({CRATES_IO})",), extra=same_pair)),
            [("errno", "0.3.14", "windows-sys", "0.61.2", "0.52.0")]), "PASS"),
        ("a reference with build metadata moved between two locked versions", _edges_check(
            _lock(rest=REST + toml_datetime + _pkg("toml_edit", "0.22.27", checksum="9" * 64, deps=("toml_datetime 0.6.11",))),
            _lock(NEW_FAMILY, rest=REST + toml_datetime + _pkg("toml_edit", "0.22.27", checksum="9" * 64, deps=("toml_datetime 1.1.1+spec-1.1.0",))),
            [("toml_edit", "0.22.27", "toml_datetime", "0.6.11", "1.1.1+spec-1.1.0")]), "PASS"),
        # Review Focus 3: a package that depends on two versions of one name, and one of them moves.
        ("one of two references to one name moves", _edges_check(
            _lock(rest=_flip_rest(errno_deps=("windows-sys 0.52.0", "windows-sys 0.61.2"), extra=FOUR_WINDOWS_SYS)),
            _lock(NEW_FAMILY, rest=_flip_rest(errno_deps=("windows-sys 0.48.0", "windows-sys 0.52.0"), extra=FOUR_WINDOWS_SYS)),
            [("errno", "0.3.14", "windows-sys", "0.61.2", "0.48.0")]), "PASS"),
    ]


def _edge_body_rows(tmp: str) -> list[tuple[str, object, str]]:
    """The PR body after a pass: the second table with exactly the edge rows, or no second table."""
    old_path = os.path.join(tmp, "edge-old.lock")
    write_file(old_path, _lock(rest=_flip_rest()))
    rows = []
    for label, new_rest, want_rows in (("with the five-package flip", _flip_rest("0.52.0"), 5), ("with no edge flip", _flip_rest(), 0)):
        root = os.path.join(tmp, f"edge-body-{want_rows}")
        _write_tree(root, {LOCK_PATH: _lock(NEW_FAMILY, rest=new_rest).encode()})
        body = os.path.join(tmp, f"edge-body-{want_rows}.md")

        def check(root=root, body=body, want_rows=want_rows) -> None:
            run_artifact(root, old_path, body, None)
            with open(body, encoding="utf-8") as handle:
                text = handle.read()
            # An edge row has five pipes; a family row has four.
            found = [ln for ln in text.splitlines() if ln.startswith("| `") and ln.count("|") == 5]
            if (EDGE_HEADING in text) != bool(want_rows) or len(found) != want_rows:
                raise RefusalError("R-SELFTEST", f"the body holds heading={EDGE_HEADING in text} and {len(found)} edge rows, want {want_rows}")
            if want_rows and "| `errno 0.3.14` | `windows-sys` | `0.61.2` | `0.52.0` |" not in found:
                raise RefusalError("R-SELFTEST", f"the body has no errno edge row: {found}")
        rows.append((f"the PR body after a pass {label}", check, "PASS"))
    return rows
```

1d. In `self_test()` (`:538-542`), add the two row builders. Replace:

```python
    rows += _ref_rows()
    failures = 0
    with tempfile.TemporaryDirectory() as tmp:
        rows += _artifact_rows(tmp)
        rows += _files_rows(tmp)
```

with:

```python
    rows += _ref_rows()
    rows += _edge_rows()
    failures = 0
    with tempfile.TemporaryDirectory() as tmp:
        rows += _artifact_rows(tmp)
        rows += _files_rows(tmp)
        rows += _edge_body_rows(tmp)
```

1e. Add the negative-control helpers directly before `def negative_control` (`:554`):

```python
def _bump_family(lock: dict) -> None:
    """Every family patch version +1, with a new checksum: a synthetic family move."""
    for entry in lock["package"]:
        if entry["name"] in FAMILY:
            major, minor, patch = entry["version"].split(".")
            entry["version"] = f"{major}.{minor}.{int(patch) + 1}"
            entry["checksum"] = "d" * 64


def _all_refs_canonical(lock: dict) -> None:
    """Every dependency reference that cargo wrote into the lock is a key of ref_table (Review
    Focus 2). If not, the canonical-form rule differs from cargo, and every run refuses."""
    entries = packages(lock, "lock")
    table = ref_table({key_of(e): e for e in entries})
    bad = [(e["name"], ref) for e in entries for ref in e.get("dependencies", []) if ref not in table]
    if bad:
        raise RefusalError("R-EDGE", f"{len(bad)} references of the lock are not canonical, for example {bad[:5]}")


def _edge_target(lock: dict) -> tuple[Key, str, str, Key, Key]:
    """SMA-738 spec 5.3: the first non-family package in lock order with a reference to a name that
    has two or more versions in the lock, where the package does not already refer to the other
    version. The other version is the first one in lock order that the package does not refer to.
    Returns (package key, old reference, new reference, old dependency key, new dependency key)."""
    entries = packages(lock, "lock")
    by_key = {key_of(e): e for e in entries}
    table = ref_table(by_key)
    canonical = {key: ref for ref, key in table.items()}
    versions: dict[str, list[Key]] = {}
    for key in by_key:
        versions.setdefault(key[0], []).append(key)
    for entry in entries:
        if entry["name"] in FAMILY:
            continue
        deps = entry.get("dependencies", [])
        for ref in deps:
            dep_key = table.get(ref)
            if dep_key is None or len(versions[dep_key[0]]) < 2:
                continue
            for other in versions[dep_key[0]]:
                if other != dep_key and canonical[other] not in deps:
                    return key_of(entry), ref, canonical[other], dep_key, other
    raise InfraError("the lock holds no non-family package with a reference to a name with two or more versions; the target finder is broken")
```

1f. Replace the whole function `negative_control` (`:554-593`) with:

```python
def negative_control(lock_path: str) -> int:
    """The real `lock` verdict on HEAD's rs/Cargo.lock (the caller writes it with `git show`, F15):
    the lock against itself is NOCHANGE with no edge row; every reference is canonical; one
    non-family version changed is R-NONFAMILY; a synthetic move of the real wasm-bindgen entries
    passes. SMA-738 spec 5.3: one real reference re-pointed to another locked version is NOCHANGE,
    or PASS with a family move, each with exactly one edge row; re-pointed to an unlocked version
    it is R-EDGE."""
    try:
        with open(lock_path, encoding="utf-8") as handle:
            text = handle.read()
    except (OSError, UnicodeDecodeError) as exc:
        raise InfraError(f"cannot read {lock_path}: {exc}") from exc
    real = parse_lock_text(text, lock_path)
    family = current_verdict(real)
    failures = 0

    def expect(label: str, fn, want: str) -> None:
        nonlocal failures
        got = _outcome(fn)
        if got == want:
            print(f"  ok    {label}: {got}")
        else:
            print(f"  FAIL  {label}: want {want}, got {got}", file=sys.stderr)
            failures += 1

    expect("HEAD's lock against itself, with no edge row", _expect_edges(real, real, []), "NOCHANGE")
    expect("every dependency reference of HEAD's lock is a canonical reference", lambda: _all_refs_canonical(real), "PASS")
    first = next((p for p in packages(real, "lock") if p["name"] not in FAMILY and "source" in p), None)
    if first is None:
        raise InfraError(f"{lock_path} holds no non-family package with a source to mutate")
    mutated = parse_lock_text(text, lock_path)
    for entry in mutated["package"]:
        if key_of(entry) == key_of(first):
            entry["version"] = "999.0.0"
    expect(f"HEAD's lock with {first['name']} moved", lambda: lock_verdict(real, mutated), "R-NONFAMILY")
    bumped = parse_lock_text(text, lock_path)
    _bump_family(bumped)
    expect("HEAD's lock with every family patch version moved", lambda: lock_verdict(real, bumped), "PASS")

    package, old_ref, new_ref, dep_key, other_key = _edge_target(real)
    want = [(package[0], package[1], dep_key[0], dep_key[1], other_key[1])]

    def repointed(ref: str, bump: bool = False) -> dict:
        out = parse_lock_text(text, lock_path)
        for entry in out["package"]:
            if key_of(entry) == package:
                deps = entry["dependencies"]
                deps[deps.index(old_ref)] = ref
        if bump:
            _bump_family(out)
        return out

    moved = f"{package[0]} {package[1]}: {old_ref!r} re-pointed to {new_ref!r}"
    expect(f"HEAD's lock with {moved}", _expect_edges(real, repointed(new_ref), want), "NOCHANGE")
    expect(f"HEAD's lock with {moved} and every family patch version moved", _expect_edges(real, repointed(new_ref, bump=True), want), "PASS")
    unlocked = f"{dep_key[0]} 999.0.0"
    expect(f"HEAD's lock with {package[0]} {package[1]}: {old_ref!r} re-pointed to {unlocked!r}", lambda: lock_verdict(real, repointed(unlocked)), "R-EDGE")
    print(f"lockstep_check negative control: wasm-bindgen {dict(family)['wasm-bindgen']} on HEAD, {failures} failed")
    return RC_REFUSE if failures else RC_OK
```

- [ ] **Step 2: Run the tests and see them fail**

Run:

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-738-wasm-lockstep
SCRATCH=/private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/31ebe72c-c15a-47ff-9c97-71e29e9e5eb6/scratchpad
python3 ci/wasm-lockstep/lockstep_check.py --self-test; echo "self-test rc=$?"
git show HEAD:rs/Cargo.lock > "$SCRATCH/head.lock"
python3 ci/wasm-lockstep/lockstep_check.py --negative-control --lock "$SCRATCH/head.lock"; echo "negative-control rc=$?"
```

Expected (self-test): `FAIL` lines on stderr. The old code keeps a non-family reference exactly, so most R-EDGE code rows show `want R-EDGE, got R-NONFAMILY`. The family-payload row and the two family-form rows show `got PASS`, because the old code reduces a family reference to its bare name. The NON-family twin shows `want R-EDGE, got R-NONFAMILY`. Then the run stops with `AttributeError: 'list' object has no attribute 'edges'` at the first value row. rc is not 0.
Expected (negative control): the run stops at its first row with `AttributeError: 'NoChangeError' object has no attribute 'edges'`. rc is not 0. (The type aliases of Step 3 are not needed yet: under `from __future__ import annotations` an annotation is a string.)

- [ ] **Step 3: Implement the edge rule**

3a. Imports (`:22-29`). Replace:

```python
from __future__ import annotations

import os
import re
import stat
import sys
import tempfile
import tomllib
```

with:

```python
from __future__ import annotations

import os
import re
import stat
import sys
import tempfile
import tomllib
from collections import Counter
from typing import NamedTuple
```

3b. Type aliases. Directly after `RC_NO_CHANGE = 4` (`:34`), add:

```python

# A package key (name, version, source); a family row (name, old or '-', new or '-'); an edge row
# (package, package version, dependency, old version, new version). SMA-738 spec 4.3.
Key = tuple[str, str, str]
FamilyRow = tuple[str, str, str]
EdgeRow = tuple[str, str, str, str, str]
```

3c. Replace the class `NoChangeError` (`:81-82`) with:

```python
class NoChangeError(Exception):
    """The wasm-bindgen family did not move. Maps to RC_NO_CHANGE. It carries the edge rows,
    because cargo can re-point a dependency edge also when no family package moves (SMA-738)."""

    def __init__(self, message: str, edges: list[EdgeRow] | None = None) -> None:
        super().__init__(message)
        self.edges = list(edges or [])


class LockResult(NamedTuple):
    """The verdict when the family moved. `family`: one row per moved family package. `edges`: one
    row per re-pointed non-family edge, with every value from the old lock. Both sorted."""

    family: list[FamilyRow]
    edges: list[EdgeRow]
```

3d. Replace `normalise_ref` (`:119-125`) with:

```python
def normalise_ref(ref: str) -> str:
    """Cargo writes a dependency reference in three forms: `name`, `name version` and
    `name version (source)`. Every reference becomes its bare name (the text before the first
    space), so R-NONFAMILY compares only which names a package depends on, in their order, with
    their count. Which version a reference points to is checked by edge_verdict (R-EDGE). Cargo
    sorts the references by the full string, and a space sorts before every character that a crate
    name can hold, so a version change never changes the order of the bare names (SMA-738 4.1)."""
    return ref.split(" ", 1)[0]
```

`normalised` (`:128-132`) does not change: it already maps `normalise_ref` over `dependencies`.

3e. Directly after `check_semver` (`:145-148`), add:

```python
def ref_table(by_key: dict[Key, dict]) -> dict[str, Key]:
    """Cargo's canonical (minimal) reference for every package of one lock (SMA-738 spec 4.2):
    `name` when the lock holds one package of that name, `name version` when it holds one package
    with that name and version, else `name version (source)`. The version is the text of the
    `version` field, so build metadata and a pre-release need no parser."""
    names = Counter(key[0] for key in by_key)
    pairs = Counter((key[0], key[1]) for key in by_key)
    table: dict[str, Key] = {}
    for key in by_key:
        name, version, source = key
        if names[name] == 1:
            table[name] = key
        elif pairs[(name, version)] == 1:
            table[f"{name} {version}"] = key
        else:
            table[f"{name} {version} ({source})"] = key
    return table


def edge_verdict(old_by_key: dict[Key, dict], new_by_key: dict[Key, dict]) -> list[EdgeRow]:
    """R-EDGE (SMA-738 spec 4.2). It runs after R-NONFAMILY passed, so the non-family key sets of
    the two locks are equal, and each non-family package has the same bare-name list in both.
    Four checks, each R-EDGE: (1) a non-family dependencies list of the new lock holds one
    reference two times; (2) an added reference is not a key of the new lock's ref_table; (3) a
    removed reference is not a key of the old lock's ref_table; (4) one (package, dependency name)
    has more than one removed or more than one added reference. A reference to a family name gets
    checks 1 to 3 and gives no edge row. Returns the edge rows, sorted, with every value from the
    old lock (the new dependency key is also in the old lock, because the key sets are equal)."""
    old_table, new_table = ref_table(old_by_key), ref_table(new_by_key)
    for key, entry in new_by_key.items():
        if key[0] in FAMILY:
            continue
        for ref, count in Counter(entry.get("dependencies", [])).items():
            if count > 1:
                raise RefusalError("R-EDGE", f"{key[0]} {key[1]} lists the dependency reference {ref!r} {count} times; cargo never writes this")
    rows: list[EdgeRow] = []
    for key in sorted(k for k in old_by_key if k[0] not in FAMILY):
        old_deps = Counter(old_by_key[key].get("dependencies", []))
        new_deps = Counter(new_by_key[key].get("dependencies", []))
        removed = sorted((old_deps - new_deps).elements())
        added = sorted((new_deps - old_deps).elements())
        label = f"{key[0]} {key[1]}"
        for ref in added:
            if ref not in new_table:
                raise RefusalError("R-EDGE", f"{label}: the added dependency reference {ref!r} is not the reference that cargo writes for a package of the new lock")
        for ref in removed:
            if ref not in old_table:
                raise RefusalError("R-EDGE", f"{label}: the removed dependency reference {ref!r} is not the reference that cargo writes for a package of the old lock")
        by_name: dict[str, tuple[list[str], list[str]]] = {}
        for ref in removed:
            by_name.setdefault(normalise_ref(ref), ([], []))[0].append(ref)
        for ref in added:
            by_name.setdefault(normalise_ref(ref), ([], []))[1].append(ref)
        for dep, (gone, came) in sorted(by_name.items()):
            if dep in FAMILY:
                continue
            if len(gone) != 1 or len(came) != 1:
                raise RefusalError("R-EDGE", f"{label}: {dep!r} has {len(gone)} removed and {len(came)} added references, so the old and the new reference cannot be paired")
            rows.append((key[0], key[1], dep, old_table[gone[0]][1], new_table[came[0]][1]))
    return sorted(rows)
```

3f. In `lock_verdict` (`:151-217`):
- Change the signature line to `def lock_verdict(old: dict, new: dict) -> LockResult:`.
- Replace its docstring with:

```python
    """The AC4.2 verdict, with the SMA-738 edge rule. Returns a LockResult: `family` holds
    (name, old version or '-', new version or '-') per moved family package, `edges` holds
    (package, package version, dependency, old version, new version) per re-pointed non-family
    edge, both sorted. Raises RefusalError, or NoChangeError (with the edge rows) when no family
    package moved. Order: R-FORMAT, R-SHAPE, R-NONFAMILY, R-EDGE, then the family checks."""
```

- Replace the comment at `:168-169` with:

```python
    # Non-family packages must be identical after every dependency reference is reduced to its bare
    # name. This also refuses an ADDED or REMOVED non-family package (a new transitive dependency)
    # and a new or removed dependency NAME. Which version a reference points to is R-EDGE's check,
    # directly below, before the family checks and before the no-change decision.
```

- Directly after the `R-NONFAMILY` `raise` block (after `:175`), add:

```python
    edges = edge_verdict(old_by_key, new_by_key)
```

- Replace the end of the function (`:210-217`):

```python
    if not removed and not added:
        raise NoChangeError("the wasm-bindgen family did not move")
    rows: dict[str, list[str]] = {}
    for k in removed:
        rows.setdefault(k[0], ["-", "-"])[0] = k[1]
    for k in added:
        rows.setdefault(k[0], ["-", "-"])[1] = k[1]
    return sorted((name, pair[0], pair[1]) for name, pair in rows.items())
```

with:

```python
    if not removed and not added:
        raise NoChangeError("the wasm-bindgen family did not move", edges)
    rows: dict[str, list[str]] = {}
    for k in removed:
        rows.setdefault(k[0], ["-", "-"])[0] = k[1]
    for k in added:
        rows.setdefault(k[0], ["-", "-"])[1] = k[1]
    return LockResult(sorted((name, pair[0], pair[1]) for name, pair in rows.items()), edges)
```

3g. Replace `body_for` (`:288-313`) with:

```python
def body_for(changes: list[FamilyRow], edges: list[EdgeRow] | None = None) -> str:
    lines = [
        "This pull request moves the wasm-bindgen family in lockstep and regenerates the committed wasm artifacts.",
        "The scheduled `wasm-lockstep` workflow made it (SMA-693).",
        "",
        "| Package | Old | New |",
        "|---|---|---|",
    ]
    lines += [f"| `{name}` | `{old}` | `{new}` |" for name, old, new in changes]
    if edges:
        # SMA-738 spec 4.3. Every value is text from the old lock (the checked-out main).
        lines += [
            "",
            EDGE_HEADING,
            "",
            "Each package below now refers to another version of the dependency. Both versions were already in the lock (SMA-738).",
            "",
            "| Package | Dependency | Old | New |",
            "|---|---|---|---|",
        ]
        lines += [f"| `{package} {version}` | `{dep}` | `{old}` | `{new}` |" for package, version, dep, old, new in edges]
    lines += [
        "",
        "**The artifacts come from a build that ran third-party code** (the new proc-macros, build scripts,",
        "`wasm-pack` and `wasm-bindgen-cli`). The workflow checked the file paths and the lock shape. Those checks",
        "do not make the content of the five files safe.",
        "",
        "Reviewer checklist:",
        "",
        "- [ ] Read the `rs/Cargo.lock` diff. Every changed package entry is a family package from crates.io. Another entry may change only a dependency edge, to a version that is already in the lock (the second table). The checker does not read the manifests: `cargo-lock-integrity` in `CI` refuses an edge that a manifest does not allow.",
        "- [ ] Confirm that `CI` is green, `committed-wasm.test.ts` included.",
        "- [ ] Expect a binary diff in `paigasus_wasm_bg.wasm`: a Linux build makes different bytes than a macOS build (SMA-634 F12).",
        "- [ ] Do not push to this branch. The next run refuses a branch that a person changed.",
        "- [ ] To bring this PR up to date with main, run the workflow again (workflow_dispatch on main). Do not push to this branch or update it with a merge; the next run then refuses.",
        "",
        f"A refusal of a later run points to the manual runbook in {RUNBOOK}.",
    ]
    return "\n".join(lines) + "\n"
```

3h. Replace `run_artifact` (`:333-347`) with:

```python
def run_artifact(directory: str, old_path: str, body_file: str | None, title_file: str | None) -> LockResult:
    artifact_tree(directory)
    try:
        result = lock_verdict(load_lock(old_path), load_lock(os.path.join(directory, LOCK_PATH)))
    except NoChangeError as exc:
        # SMA-738: the two locks can differ in dependency edges only, so "equal" is no longer true.
        raise RefusalError("R-NOCHANGE", "the wasm-bindgen family did not move, so there is nothing to propose") from exc
    title = title_for(result.family)
    body = body_for(result.family, result.edges)
    # Written ONLY here, after every version string passed check_semver and every refusal had
    # its chance. A refusal above leaves both files absent.
    if title_file:
        write_file(title_file, title + "\n")
    if body_file:
        write_file(body_file, body)
    return result
```

3i. Directly before `def _options` (`:598`), add:

```python
def _print_family(rows: list[FamilyRow]) -> None:
    for name, old, new in rows:
        print(f"family-moved {name} {old} {new}")


def _print_edges(rows: list[EdgeRow]) -> None:
    """One line per edge row, after the family lines, also on exit 4 (SMA-738 spec 4.3)."""
    for package, version, dep, old, new in rows:
        print(f"edge-moved {package} {version} {dep} {old} {new}")
```

3j. In `main`, replace the `lock` and `artifact` branches (`:621-637`) with:

```python
    if command == "lock":
        opts = _options(rest, ("--old", "--new"), ("--old", "--new"))
        try:
            result = lock_verdict(load_lock(opts["--old"]), load_lock(opts["--new"]))
        except NoChangeError as exc:
            for name, version in current_verdict(load_lock(opts["--new"])):
                print(f"family-current {name} {version}")
            _print_edges(exc.edges)
            return RC_NO_CHANGE
        _print_family(result.family)
        _print_edges(result.edges)
        return RC_OK
    if command == "artifact":
        opts = _options(rest, ("--dir", "--old", "--body-file", "--title-file"), ("--dir", "--old"))
        result = run_artifact(opts["--dir"], opts["--old"], opts.get("--body-file"), opts.get("--title-file"))
        _print_family(result.family)
        _print_edges(result.edges)
        return RC_OK
```

The `title_for` signature stays `title_for(changes: list[tuple[str, str, str]])`. The title does not change (spec 4.3).

- [ ] **Step 4: Run the tests and see them pass**

Run:

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-738-wasm-lockstep
SCRATCH=/private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/31ebe72c-c15a-47ff-9c97-71e29e9e5eb6/scratchpad
python3 ci/wasm-lockstep/lockstep_check.py --self-test; echo "self-test rc=$?"
python3 ci/wasm-lockstep/lockstep_check.py --negative-control --lock "$SCRATCH/head.lock"; echo "negative-control rc=$?"
```

Expected: the last self-test line is `lockstep_check self-test: N rows, 0 failed` and `self-test rc=0`. Every new row prints `ok`. The negative control prints `ok` for its nine rows, among them `every dependency reference of HEAD's lock is a canonical reference: PASS` and `HEAD's lock with ahash 0.8.12: 'getrandom 0.3.4' re-pointed to '...': NOCHANGE` (the target that `_edge_target` finds on the current lock is `ahash 0.8.12`, reference `getrandom 0.3.4`), and ends with `0 failed`, `negative-control rc=0`.

If the canonical row fails, stop. It means that cargo writes a form that `ref_table` does not make. Report the five example references to the coordinator.

- [ ] **Step 5: Lint**

Run:

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-738-wasm-lockstep
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
uv run --locked --project py ruff check ci/wasm-lockstep/
```

Expected: `All checks passed!`. Fix a finding in the new code only. Do not run `ruff format`.

- [ ] **Step 6: Update the documents for 4.1-4.4**

6a. `ci/wasm-lockstep/README.md:47`. Replace the `R-NONFAMILY` row with these two rows:

```markdown
| `R-NONFAMILY` | A package outside the family was added or removed, or changed its version, source, checksum or the names it depends on. The dependency references of a non-family package are compared by bare name, in order, with their count (SMA-738). Which version a reference points to is the check of `R-EDGE`. |
| `R-EDGE` | A dependency reference moved in a way that cargo does not write (SMA-738): one `dependencies` list of the new lock holds a reference two times; an added reference is not the exact form that cargo writes for a package of the new lock (`name` when the lock holds one package of that name, `name version` when it holds one with that name and version, else `name version (source)`); a removed reference is not that form in the old lock; or one package has more than one removed or more than one added reference to one name. A moved reference to a family name gets the same checks. `R-EDGE` runs before the family checks and before the no-change decision. |
```

6b. `ci/wasm-lockstep/README.md:57-58`. After the paragraph that starts `No change exits 4.`, add:

```markdown
**Moved edges (SMA-738).** `cargo update` can re-point a dependency edge of a non-family package
to another version that is already in the lock. Each such edge is an edge row: the package, its
version, the dependency, and the old and the new version, all from the old lock. `lock` prints one
`edge-moved <package> <package version> <dependency> <old> <new>` line per row, after the
`family-moved` lines. On exit 4 it prints them after the `family-current` lines, so the weekly log
shows when cargo keeps flipping edges. `artifact` prints the same lines, and the PR body then has a
second table, "Dependency edges that moved".
```

6c. `ci/wasm-lockstep/README.md:61`. Replace `` `R-NOCHANGE` (the artifact lock equals the checked-out lock) `` with `` `R-NOCHANGE` (the family did not move; the two locks can differ in dependency edges only) ``.

6d. `ci/wasm-lockstep/README.md`, list "What the checks do not prove". After the first bullet (`The artifacts come from a build ...`), add:

```markdown
- `cargo update` in container run 1 can re-point a non-family dependency edge to another version
  that is already in the lock. This can change which code a target compiles. No new package enters
  the lock. The reviewer reads the edge table of the PR body. `cargo-lock-integrity` in `CI`
  catches only an edge outside the range that a manifest allows (SMA-738).
```

6e. `rs/CLAUDE.md:195-198`. Replace:

```markdown
  Use the manual runbook below only when the workflow cannot help: it refuses the lock change (a
  new transitive dependency, or a newer `syn`), its build fails because the pinned `wasm-pack`
  does not support the new 0.2.z, or the `reqwest` case below. Use a normal
  `feature/sma-NNN-<slug>` PR.
```

with:

```markdown
  Use the manual runbook below only when the workflow cannot help: it refuses the lock change (a
  new transitive dependency, a newer `syn`, or `R-EDGE`, see below), its build fails because the
  pinned `wasm-pack` does not support the new 0.2.z, or the `reqwest` case below. Use a normal
  `feature/sma-NNN-<slug>` PR.
  **`R-EDGE` with no family move (SMA-738).** The lock on `main` holds a reference that cargo now
  writes in another form, or the run made a reference that is not in the lock. Run the
  four-package `cargo update -p` locally and read the edge diff. If it only re-points edges
  between versions that are already in the lock, commit the lock in a normal
  `feature/sma-NNN-<slug>` PR. Otherwise open an issue.
```

6f. `rs/CLAUDE.md:207`. Replace:

```
  git diff -- rs/Cargo.lock   # the family entries (seven at M0) plus any new dep, crates.io only
```

with:

```
  git diff -- rs/Cargo.lock   # the family entries (seven at M0) plus any new dep, crates.io only, and dependency edges re-pointed between locked versions
```

6g. `docs/superpowers/specs/2026-09-27-sma-693-wasm-bindgen-lockstep-updater-design.md`. After line 254 (`` `"name version"` and `"name version (source)"`. The checker handles all three. ``), add:

```markdown
    SMA-738 note (2026-10-05): this invariant is replaced. `R-NONFAMILY` now compares the
    dependency references of a non-family package by bare name, in order. The new refusal `R-EDGE`
    accepts a moved reference only when the old and the new reference are each the exact form that
    cargo writes for a package of its lock. See
    `docs/superpowers/specs/2026-10-05-sma-738-wasm-lockstep-edge-flips-design.md`.
```

- [ ] **Step 7: Run the gate wrapper**

Run:

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-738-wasm-lockstep
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
./ci/wasm-lockstep/run.sh --self-test; echo "rc=$?"
./ci/wasm-lockstep/run.sh --negative-control; echo "rc=$?"
./ci/wasm-lockstep/run.sh; echo "rc=$?"
```

Expected: `== wasm-lockstep self-test passed ==` rc=0; `== wasm-lockstep negative control passed ==` rc=0 (its 3 -> 1 row still names `R-NONFAMILY`, because R-NONFAMILY runs before R-EDGE); `PASS  [pin_check ...]` and `PASS  [lockstep_check current on HEAD:rs/Cargo.lock]` rc=0.

- [ ] **Step 8: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-738-wasm-lockstep
git add ci/wasm-lockstep/lockstep_check.py ci/wasm-lockstep/README.md rs/CLAUDE.md docs/superpowers/specs/2026-09-27-sma-693-wasm-bindgen-lockstep-updater-design.md
git status --short
git commit -m "fix(ci): accept a dependency-edge flip between locked versions in wasm-lockstep (SMA-738)" -m "lockstep_check.py compares the dependency references of a non-family package by bare name.
The new refusal R-EDGE accepts a moved reference only when the old and the new reference are the
exact forms that cargo writes for a package of the lock. lock and artifact print one edge-moved
line per moved edge, also on exit 4, and the PR body gets a second table." -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

Expected: `git status --short` shows only the four files, all staged. The commit succeeds.

---

### Task 2: The run-1 lock compare (spec 4.5), as ONE commit

**Files:**
- Modify: `ci/wasm-lockstep/lockstep_check.py` (docstring `:7-12`, imports, `load_lock` `:92-98`, `run_artifact`, `negative_control`, `_options` `:598-609`, `main`, self-test)
- Modify: `ci/wasm-lockstep/pin_check.py` (docstring `:5`, constants `:57-58`, `_checker_tail` `:360-379`, `command_violations` `:382-420`, `_env_violations` `:423-436`, `step_violations` `:467-470`, `violations` `:517-523`, `FIXTURE` `:595-684`, `SELF_TEST_ROWS` `:706-836`, `negative_control` `:868-906`, `main` `:921`)
- Modify: `.github/workflows/wasm-lockstep.yml` (`:11-21`, `:106-129`, `:140-163`)
- Modify: `ci/wasm-lockstep/README.md`, `rs/CLAUDE.md`

**Interfaces:**
- Consumes (from Task 1): `LockResult`, `NoChangeError.edges`, `lock_verdict`, `run_artifact`, `_print_family`, `_print_edges`, `current_verdict`, `SIZE_CAP`, `LOCK_PATH`.
- Produces:
  - `def read_file(path: str, *, regular_only: bool = False) -> tuple[bytes, str]` (bytes, SHA-256 hex).
  - `def read_lock(path: str) -> tuple[dict, str]` (parsed lock, SHA-256 hex of the exact bytes).
  - `def load_lock(path: str) -> dict` (now `read_lock(path)[0]`).
  - `def run_same(expected: str, path: str) -> str` (returns the hex).
  - `run_artifact(...) -> tuple[LockResult, str]`.
  - `SHA256_HEX = re.compile(r"[0-9a-f]{64}")`.
  - CLI: `lockstep_check.py same --sha256 <64 hex> --file <path>`; output lines `lock-sha256 <hex>` and `same: rs/Cargo.lock sha256 <hex> did not change after the lock verdict`.
  - `pin_check.py`: `BUILD_ORDER`, `BUILD_IF`, `LOCK_CMD`, `LOCK_SHA_LINE`, `SAME_CMD`, `STAGE_ENV`, `UPLOAD_PATH`, `RUNNER_TEMP_ASSIGN`, `def _build_violations(build: dict) -> list[str]`; the rule id `P25`.
  - Workflow: the step output `steps.lock.outputs.lock_sha256`; the `stage` env `LOCK_SHA256`.

- [ ] **Step 1: Write the failing `lockstep_check.py` rows (spec 5.5)**

1a. Imports. Add `contextlib`, `errno`, `hashlib` and `io` to the import block, in sorted order:

```python
from __future__ import annotations

import contextlib
import errno
import hashlib
import io
import os
import re
import stat
import sys
import tempfile
import tomllib
from collections import Counter
from typing import NamedTuple
```

1b. Directly after `_edge_body_rows` (Task 1), add:

```python
# ---- SMA-738: the run-1 lock compare (spec 5.5) -------------------------------------------------

def _main_capture(argv: list[str]) -> tuple[int, str]:
    """main(argv) with its stdout captured. main() RETURNS 0 or 4, and _outcome maps any return
    to PASS, so a row that needs the return code or a printed line reads them here."""
    buffer = io.StringIO()
    with contextlib.redirect_stdout(buffer):
        rc = main(argv)
    return rc, buffer.getvalue()


def _hash_rows(tmp: str) -> list[tuple[str, object, str]]:
    def put(name: str, data: bytes) -> str:
        path = os.path.join(tmp, name)
        with open(path, "wb") as handle:
            handle.write(data)
        return path

    old = put("hash-old.lock", _lock().encode())
    new_bytes = _lock(NEW_FAMILY).encode()
    crlf_bytes = _lock(NEW_FAMILY).replace("\n", "\r\n").encode()
    new = put("hash-new.lock", new_bytes)
    crlf = put("hash-crlf.lock", crlf_bytes)
    unchanged = put("hash-unchanged.lock", _lock().encode())
    refused = put("hash-refused.lock", _lock(NEW_FAMILY, rest=REST + _pkg("evil", "1.0.0")).encode())
    sha = hashlib.sha256(new_bytes).hexdigest()
    one_byte = put("same-one-byte.lock", new_bytes.replace(b"0.2.129", b"0.2.130", 1))
    trailing = put("same-trailing.lock", new_bytes + b"\n")
    link = os.path.join(tmp, "same-link.lock")
    os.symlink(new, link)
    big = put("same-big.lock", b"")
    os.truncate(big, SIZE_CAP + 1)  # sparse: one byte over 8 MiB costs no disk
    big_sha = hashlib.sha256(bytes(SIZE_CAP + 1)).hexdigest()
    fifo = os.path.join(tmp, "same-fifo")
    os.mkfifo(fifo)
    directory = os.path.join(tmp, "same-dir")
    os.makedirs(directory)
    missing = os.path.join(tmp, "same-missing.lock")
    artifact = os.path.join(tmp, "hash-artifact")
    _write_tree(artifact, {LOCK_PATH: new_bytes})

    def printed(argv: list[str], want_rc: int, want_lines: list[str], prefix: str):
        def check() -> None:
            rc, out = _main_capture(argv)
            got = [ln for ln in out.splitlines() if ln.startswith(prefix)]
            if rc != want_rc or got != want_lines:
                raise RefusalError("R-SELFTEST", f"{argv[0]} returned {rc} and printed {got}, want {want_rc} and {want_lines}")
        return check

    def refused_without_hash() -> None:
        buffer = io.StringIO()
        try:
            with contextlib.redirect_stdout(buffer):
                main(["lock", "--old", old, "--new", refused])
        except RefusalError:
            if "lock-sha256" in buffer.getvalue():
                raise RefusalError("R-SELFTEST", "lock printed a lock-sha256 line before its refusal") from None
            raise
        raise RefusalError("R-SELFTEST", "lock did not refuse a lock with an added package")

    def same(sha_value: str, path: str) -> list[str]:
        return ["same", "--sha256", sha_value, "--file", path]

    same_line = f"same: {LOCK_PATH} sha256 {sha} did not change after the lock verdict"
    return [
        ("lock: a pass prints the SHA-256 of the raw file bytes", printed(["lock", "--old", old, "--new", new], RC_OK, [f"lock-sha256 {sha}"], "lock-sha256 "), "PASS"),
        ("lock: a pass on a lock with CRLF line ends hashes the raw bytes", printed(["lock", "--old", old, "--new", crlf], RC_OK, [f"lock-sha256 {hashlib.sha256(crlf_bytes).hexdigest()}"], "lock-sha256 "), "PASS"),
        ("lock: no change returns 4 and prints no lock-sha256 line", printed(["lock", "--old", old, "--new", unchanged], RC_NO_CHANGE, [], "lock-sha256 "), "PASS"),
        ("lock: a refusal prints no lock-sha256 line", refused_without_hash, "R-NONFAMILY"),
        ("same: the correct hash", printed(same(sha, new), RC_OK, [same_line], "same: "), "PASS"),
        ("same: a file that differs by one byte", lambda: main(same(sha, one_byte)), "R-RUN2"),
        ("same: a file that differs only in a trailing newline", lambda: main(same(sha, trailing)), "R-RUN2"),
        ("same: a symlink to a file with the correct hash", lambda: main(same(sha, link)), "R-RUN2"),
        ("same: a sparse file over SIZE_CAP, with its own hash", lambda: main(same(big_sha, big)), "R-RUN2"),
        ("same: a FIFO returns at once", lambda: main(same(sha, fifo)), "R-RUN2"),
        ("same: a directory", lambda: main(same(sha, directory)), "R-RUN2"),
        ("same: a missing file", lambda: main(same(sha, missing)), "INFRA"),
        ("same: an empty --sha256", lambda: main(same("", new)), "INFRA"),
        ("same: an upper-case --sha256", lambda: main(same(sha.upper(), new)), "INFRA"),
        ("same: a 63-character --sha256", lambda: main(same(sha[:63], new)), "INFRA"),
        ("same: --file given two times", lambda: main([*same(sha, new), "--file", new]), "INFRA"),
        ("same: no --sha256", lambda: main(["same", "--file", new]), "INFRA"),
        ("same: no --file", lambda: main(["same", "--sha256", sha]), "INFRA"),
        ("artifact: a pass prints the SHA-256 of the artifact lock", printed(["artifact", "--dir", artifact, "--old", old], RC_OK, [f"lock-sha256 {sha}"], "lock-sha256 "), "PASS"),
    ]
```

1c. In `self_test()`, after `rows += _edge_body_rows(tmp)`, add `rows += _hash_rows(tmp)`.

- [ ] **Step 2: Run the self-test and see it fail**

Run: `python3 ci/wasm-lockstep/lockstep_check.py --self-test; echo "rc=$?"` (from the worktree root).

Expected: `FAIL` lines for the three `lock:` value rows (no `lock-sha256` line, so `R-SELFTEST`), for `same: the correct hash` and every `R-RUN2` row (`got INFRA`: `unknown command 'same'`), and for `artifact: a pass ...` (the Task 1 `artifact` prints no `lock-sha256` line). The row `lock: a refusal prints no lock-sha256 line` shows `ok` already, because the old `lock` prints no hash at all. The `INFRA` rows of `same` show `ok` before the change too, because an unknown command is also INFRA; Step 4 proves them again after the change. rc is not 0.

- [ ] **Step 3: Implement the reader, `same`, the hash output and the repeated-flag refusal**

3a. Module docstring (`:7-12`). Replace the usage block with:

```python
  lockstep_check.py lock --old <lock> --new <lock>
  lockstep_check.py artifact --dir <dir> --old <lock> [--body-file <f>] [--title-file <f>]
  lockstep_check.py same --sha256 <64 hex> --file <path>
  lockstep_check.py status --file <git-status-porcelain-file>
  lockstep_check.py current --lock <lock>
  lockstep_check.py --self-test
  lockstep_check.py --negative-control --lock <lock>
```

3b. Directly after `CHECKSUM = re.compile(...)` (`:51`), add:

```python
# The `same --sha256` value (SMA-738 spec 4.5): lower-case only, used with fullmatch().
SHA256_HEX = re.compile(r"[0-9a-f]{64}")
```

3c. Replace `load_lock` (`:92-98`) with:

```python
def read_file(path: str, *, regular_only: bool = False) -> tuple[bytes, str]:
    """The ONE reader of a lock file (SMA-738 spec 4.5): `lock`, `artifact` and `same` all read
    through it. It reads the raw bytes in chunks, with no newline translation, and hashes each
    chunk as it reads. Returns (the bytes, their SHA-256 in lower-case hex).

    regular_only (the `same` subcommand): the file is opened with O_NOFOLLOW and O_NONBLOCK, so a
    symlink and a FIFO do not get followed or block. A symlink, a file that is not a regular file,
    or a file over SIZE_CAP refuses with R-RUN2. Every other OSError is InfraError (exit 2)."""
    flags = os.O_RDONLY
    if regular_only:
        flags |= os.O_NOFOLLOW | os.O_NONBLOCK
    try:
        fd = os.open(path, flags)
    except OSError as exc:
        if regular_only and exc.errno == errno.ELOOP:
            raise RefusalError("R-RUN2", f"{path} is a symlink") from exc
        raise InfraError(f"cannot read {path}: {exc}") from exc
    digest = hashlib.sha256()
    chunks: list[bytes] = []
    try:
        info = os.fstat(fd)
        if regular_only and not stat.S_ISREG(info.st_mode):
            raise RefusalError("R-RUN2", f"{path} is not a regular file")
        if regular_only and info.st_size > SIZE_CAP:
            raise RefusalError("R-RUN2", f"{path} is {info.st_size} bytes, over the {SIZE_CAP}-byte cap")
        size = 0
        while chunk := os.read(fd, 1 << 16):
            size += len(chunk)
            if regular_only and size > SIZE_CAP:
                raise RefusalError("R-RUN2", f"{path} grew over the {SIZE_CAP}-byte cap while it was read")
            digest.update(chunk)
            chunks.append(chunk)
    except OSError as exc:
        raise InfraError(f"cannot read {path}: {exc}") from exc
    finally:
        os.close(fd)
    return b"".join(chunks), digest.hexdigest()


def read_lock(path: str) -> tuple[dict, str]:
    """A lock read ONCE: the parsed dict and the SHA-256 of exactly the bytes that were parsed.
    Strict UTF-8, no newline translation (SMA-738 spec 4.5)."""
    data, digest = read_file(path)
    try:
        text = data.decode("utf-8")
    except UnicodeDecodeError as exc:
        raise InfraError(f"cannot read {path}: {exc}") from exc
    return parse_lock_text(text, path), digest


def load_lock(path: str) -> dict:
    return read_lock(path)[0]
```

3d. Replace `run_artifact` (the Task 1 version) with:

```python
def run_artifact(directory: str, old_path: str, body_file: str | None, title_file: str | None) -> tuple[LockResult, str]:
    """Returns the verdict and the SHA-256 of the artifact lock bytes that it judged."""
    artifact_tree(directory)
    new, digest = read_lock(os.path.join(directory, LOCK_PATH))
    try:
        result = lock_verdict(load_lock(old_path), new)
    except NoChangeError as exc:
        # SMA-738: the two locks can differ in dependency edges only, so "equal" is no longer true.
        raise RefusalError("R-NOCHANGE", "the wasm-bindgen family did not move, so there is nothing to propose") from exc
    title = title_for(result.family)
    body = body_for(result.family, result.edges)
    # Written ONLY here, after every version string passed check_semver and every refusal had
    # its chance. A refusal above leaves both files absent.
    if title_file:
        write_file(title_file, title + "\n")
    if body_file:
        write_file(body_file, body)
    return result, digest


def run_same(expected: str, path: str) -> str:
    """`same` (SMA-738 spec 4.5): the bytes of `path` must have the SHA-256 that `lock` printed
    before container run 2. It compares bytes, not parsed TOML, so a whitespace or comment change
    refuses too. No byte of the file goes to the log."""
    if not SHA256_HEX.fullmatch(expected):
        raise InfraError(f"usage: --sha256 must be 64 lower-case hex characters, got {expected!r}")
    data, actual = read_file(path, regular_only=True)
    if actual != expected:
        raise RefusalError("R-RUN2", f"{LOCK_PATH} changed after the lock verdict: the lock step judged sha256 {expected}, {path} has sha256 {actual} ({len(data)} bytes)")
    return actual
```

3e. Replace `_options` (`:598-609`) with:

```python
def _options(argv: list[str], names: tuple[str, ...], required: tuple[str, ...]) -> dict[str, str]:
    out: dict[str, str] = {}
    rest = list(argv)
    while rest:
        flag = rest.pop(0)
        if flag not in names or not rest:
            raise InfraError(f"usage: unknown flag or missing value at {flag!r}")
        if flag in out:
            # SMA-738: `same --file A --file B` must not compare another file than the one P25 pins.
            raise InfraError(f"usage: {flag} is given more than once")
        out[flag] = rest.pop(0)
    for name in required:
        if name not in out:
            raise InfraError(f"usage: {name} is required")
    return out
```

3f. In `main`, replace the usage message line:

```python
        raise InfraError("usage: lockstep_check.py lock|artifact|status|current ... | --self-test | --negative-control --lock <path>")
```

with:

```python
        raise InfraError("usage: lockstep_check.py lock|artifact|same|status|current ... | --self-test | --negative-control --lock <path>")
```

and replace the `lock` and `artifact` branches (the Task 1 version) with:

```python
    if command == "lock":
        opts = _options(rest, ("--old", "--new"), ("--old", "--new"))
        # The new lock is read ONCE (SMA-738 spec 4.5). The verdict, the no-change path and the
        # printed hash all use these bytes.
        new, digest = read_lock(opts["--new"])
        try:
            result = lock_verdict(load_lock(opts["--old"]), new)
        except NoChangeError as exc:
            for name, version in current_verdict(new):
                print(f"family-current {name} {version}")
            _print_edges(exc.edges)
            return RC_NO_CHANGE
        _print_family(result.family)
        _print_edges(result.edges)
        print(f"lock-sha256 {digest}")
        return RC_OK
    if command == "artifact":
        opts = _options(rest, ("--dir", "--old", "--body-file", "--title-file"), ("--dir", "--old"))
        result, digest = run_artifact(opts["--dir"], opts["--old"], opts.get("--body-file"), opts.get("--title-file"))
        _print_family(result.family)
        _print_edges(result.edges)
        print(f"lock-sha256 {digest}")
        return RC_OK
    if command == "same":
        opts = _options(rest, ("--sha256", "--file"), ("--sha256", "--file"))
        digest = run_same(opts["--sha256"], opts["--file"])
        print(f"same: {LOCK_PATH} sha256 {digest} did not change after the lock verdict")
        return RC_OK
```

Note: the old `lock` order read `--old` first. Now `--new` is read first. Both errors are InfraError (exit 2), so the exit code does not change.

3g. In `_edge_body_rows` (Task 1) nothing changes: it calls `run_artifact(...)` and does not use the return value. In `_files_rows` and `_artifact_rows` nothing changes for the same reason.

- [ ] **Step 4: Run the self-test and see it pass**

Run: `python3 ci/wasm-lockstep/lockstep_check.py --self-test; echo "rc=$?"`

Expected: every row `ok`, the last line `lockstep_check self-test: N rows, 0 failed`, rc=0. The FIFO row returns at once (the whole self-test ends in a few seconds).

- [ ] **Step 5: Add the `same` rows of the negative control, see them fail, then pass**

5a. In `negative_control` (the Task 1 version), replace the read block:

```python
    try:
        with open(lock_path, encoding="utf-8") as handle:
            text = handle.read()
    except (OSError, UnicodeDecodeError) as exc:
        raise InfraError(f"cannot read {lock_path}: {exc}") from exc
```

with:

```python
    data, digest = read_file(lock_path)
    try:
        text = data.decode("utf-8")
    except UnicodeDecodeError as exc:
        raise InfraError(f"cannot read {lock_path}: {exc}") from exc
```

5b. Directly before the final `print(f"lockstep_check negative control: ...")` line, add:

```python
    # SMA-738 spec 5.5: `same` on HEAD's real lock bytes, through main(), the production path.
    with tempfile.TemporaryDirectory() as tmp:
        copy_path = os.path.join(tmp, "Cargo.lock")
        with open(copy_path, "wb") as handle:
            handle.write(data)
        changed_text = re.sub(r'^version = "[^"]*"', 'version = "999.0.0"', text, count=1, flags=re.M)
        if changed_text == text:
            raise InfraError(f"{lock_path} holds no package version line to change")
        changed_path = os.path.join(tmp, "changed.lock")
        with open(changed_path, "wb") as handle:
            handle.write(changed_text.encode("utf-8"))

        def same_rc(path: str) -> None:
            rc, _out = _main_capture(["same", "--sha256", digest, "--file", path])
            if rc != RC_OK:
                raise RefusalError("R-SELFTEST", f"same returned {rc}, want 0")

        expect("same: HEAD's lock against its own sha256", lambda: same_rc(copy_path), "PASS")
        expect("same: a copy of HEAD's lock with its first package version changed", lambda: same_rc(changed_path), "R-RUN2")
```

5c. Run the negative control once with the line `expect("same: a copy of HEAD's lock ..."` changed to want `"PASS"`, to see that row FAIL (`want PASS, got R-RUN2`). Then set it back to `"R-RUN2"` and run again:

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-738-wasm-lockstep
SCRATCH=/private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/31ebe72c-c15a-47ff-9c97-71e29e9e5eb6/scratchpad
git show HEAD:rs/Cargo.lock > "$SCRATCH/head.lock"
python3 ci/wasm-lockstep/lockstep_check.py --negative-control --lock "$SCRATCH/head.lock"; echo "rc=$?"
```

Expected (final): eleven `ok` rows, `0 failed`, rc=0.

- [ ] **Step 6: Write the failing `pin_check.py` fixture and rows**

6a. Directly before `FIXTURE = """\` (`:595`), add the build-step pieces:

```python
# SMA-738 P25: the build steps of the fixture, as pieces, so that the mutation rows can name them.
LOCK_SED = r'''sha="$(sed -n 's/^lock-sha256 \([0-9a-f]\{64\}\)$/\1/p' "$RUNNER_TEMP/v.txt")"'''
LOCK_SHA_LINE = 'echo "lock_sha256=${sha}" >> "$GITHUB_OUTPUT"'
SAME_LINE = 'python3 ci/wasm-lockstep/lockstep_check.py same --sha256 "$LOCK_SHA256" --file "$RUNNER_TEMP/stage/rs/Cargo.lock"'
LOCK_STEP = "".join((
    "      - id: lock\n",
    "        run: |\n",
    "          rc=0\n",
    '          python3 ci/wasm-lockstep/lockstep_check.py lock --old "$RUNNER_TEMP/old.lock" --new "$RUNNER_TEMP/work/rs/Cargo.lock" > "$RUNNER_TEMP/v.txt" || rc=$?\n',
    '          if [ "$rc" -eq 0 ]; then\n',
    f"            {LOCK_SED}\n",
    '            echo "changed=true" >> "$GITHUB_OUTPUT"\n',
    f"            {LOCK_SHA_LINE}\n",
    "          fi\n",
))
BUILD_JOB_STEP = "".join((
    "      - id: build\n",
    "        if: steps.lock.outputs.changed == 'true'\n",
    "        run: |\n",
    '          docker run --rm --cap-drop=ALL --security-opt=no-new-privileges --user 65534:65534 --volume "$RUNNER_TEMP/work:/work" --workdir /work "$LOCKSTEP_IMAGE" bash ci/wasm-lockstep/container.sh build\n',
))
STAGE_IF = "      - id: stage\n        if: steps.lock.outputs.changed == 'true'\n"
STAGE_ENV_LINE = "          LOCK_SHA256: ${{ steps.lock.outputs.lock_sha256 }}\n"
STAGE_STEP = "".join((
    STAGE_IF,
    "        env:\n",
    STAGE_ENV_LINE,
    "        run: |\n",
    '          cp "$RUNNER_TEMP/work/rs/Cargo.lock" "$RUNNER_TEMP/stage/rs/Cargo.lock"\n',
    f"          {SAME_LINE}\n",
))
UPLOAD_HEAD = "      - id: upload\n"
UPLOAD_PATH_LINE = "          path: ${{ runner.temp }}/stage/\n"
UPLOAD_STEP = "".join((
    UPLOAD_HEAD,
    "        if: steps.lock.outputs.changed == 'true'\n",
    "        uses: actions/upload-artifact@" + "2" * 40 + "\n",
    "        with:\n",
    UPLOAD_PATH_LINE,
))
```

6b. In `FIXTURE`, replace the `build` steps (`:612-629`, from `    steps:\n      - id: checkout` up to and including the `upload` step) so that the text from `    outputs:` to `  propose:` reads:

```python
    outputs:
      changed: ${{ steps.lock.outputs.changed }}
    steps:
      - id: reclaim
        run: df -h /
      - id: checkout
        uses: actions/checkout@""" + "1" * 40 + """
        with:
          persist-credentials: false
      - id: ref
        run: test "$GITHUB_REF" = refs/heads/main
      - id: copy
        run: mkdir -p "$RUNNER_TEMP/work" "$RUNNER_TEMP/stage"
      - id: update
        run: |
          docker run --rm --cap-drop=ALL --security-opt=no-new-privileges --user 65534:65534 --volume "$RUNNER_TEMP/work:/work" --workdir /work "$LOCKSTEP_IMAGE" bash ci/wasm-lockstep/container.sh update
""" + LOCK_STEP + BUILD_JOB_STEP + STAGE_STEP + UPLOAD_STEP + """\
  propose:
```

The rest of `FIXTURE` (the `propose` job) does not change. The anchors `BUILD_STEP_ANCHOR` (`"      - id: update\n"`) and `BUILD_RUN` (`"          rc=0\n"`) each still occur once.

6c. At the end of `SELF_TEST_ROWS` (before the closing `)` at `:836`), add:

```python
    # ---- SMA-738: P18 for every job, and P25 ----
    ("the stage step without the same command", ((f"          {SAME_LINE}\n", ""),), "P25"),
    ("the same command joined with || true", ((SAME_LINE, SAME_LINE + " || true"),), "P18"),
    ("the same command not the last command of the stage step", ((f"          {SAME_LINE}\n", f"          {SAME_LINE}\n          echo done\n"),), "P18"),
    ("a second --file on the same command", ((SAME_LINE, SAME_LINE + ' --file "$RUNNER_TEMP/work/rs/Cargo.lock"'),), "P25"),
    ("--file names the work copy", ((SAME_LINE, SAME_LINE.replace("/stage/rs/", "/work/rs/")),), "P25"),
    ("the stage env reads another step output", ((STAGE_ENV_LINE, STAGE_ENV_LINE.replace("outputs.lock_sha256", "outputs.changed")),), "P25"),
    ("the lock step without the lock_sha256 line", ((f"            {LOCK_SHA_LINE}\n", ""),), "P25"),
    ("lock after build", ((LOCK_STEP + BUILD_JOB_STEP, BUILD_JOB_STEP + LOCK_STEP),), "P25"),
    ("a new step between stage and upload", ((UPLOAD_HEAD, "      - id: extra\n        run: echo extra\n" + UPLOAD_HEAD),), "P25"),
    ("the stage if: changed to != 'false'", ((STAGE_IF, STAGE_IF.replace("== 'true'", "!= 'false'")),), "P25"),
    ("upload.with.path names the work copy", ((UPLOAD_PATH_LINE, UPLOAD_PATH_LINE.replace("/stage/", "/work/")),), "P25"),
    ("a step env sets RUNNER_TEMP", ((BUILD_STEP_ANCHOR, BUILD_STEP_ANCHOR + "        env:\n          RUNNER_TEMP: /tmp\n"),), "P25"),
    ("a shell assignment RUNNER_TEMP=/tmp in the stage step", ((f"          {SAME_LINE}\n", f"          RUNNER_TEMP=/tmp\n          {SAME_LINE}\n"),), "P25"),
```

- [ ] **Step 7: Run the pin_check self-test and see it fail**

Run:

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-738-wasm-lockstep
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
uv run --locked --project ci/wasm-lockstep --python '>=3.12' python3 ci/wasm-lockstep/pin_check.py --self-test; echo "rc=$?"
```

Expected: the first row `the fixture passes ...: PASS` prints `ok` (the new fixture breaks no old rule). Every new P25 row prints `FAIL ... want P25, got [...]`, and the two P18 rows print `FAIL ... want P18`, because P18 runs for `propose` only. rc=3.

- [ ] **Step 8: Implement P18 for every job, P25 and the `RUNNER_TEMP` rule**

8a. Docstring line 5: replace `The rules (P0-P24)` with `The rules (P0-P25)`.

8b. Replace `CHECKER_SUBS = frozenset({"artifact", "status"})` (`:57`) with:

```python
CHECKER_SUBS = frozenset({"artifact", "status", "same"})
```

8c. Directly after `CONTAINER_STEP_OUTPUTS = ...` (`:65`), add:

```python
# P25 (SMA-738): the build job's structure around the run-1 lock compare.
BUILD_ORDER = ("reclaim", "checkout", "ref", "copy", "update", "lock", "build", "stage", "upload")
BUILD_IF = "steps.lock.outputs.changed == 'true'"
LOCK_CMD = ["python3", "ci/wasm-lockstep/lockstep_check.py", "lock", "--old", "$RUNNER_TEMP/old.lock", "--new", "$RUNNER_TEMP/work/rs/Cargo.lock"]
LOCK_OUTPUT_LINE = 'echo "lock_sha256=${sha}" >> "$GITHUB_OUTPUT"'
SAME_CMD = ["python3", "ci/wasm-lockstep/lockstep_check.py", "same", "--sha256", "$LOCK_SHA256", "--file", "$RUNNER_TEMP/stage/rs/Cargo.lock"]
STAGE_ENV = {"LOCK_SHA256": "${{ steps.lock.outputs.lock_sha256 }}"}
UPLOAD_PATH = "${{ runner.temp }}/stage/"
RUNNER_TEMP_ASSIGN = re.compile(r"\bRUNNER_TEMP(?:[:+])?=")
```

(`LOCK_OUTPUT_LINE` is the rule constant. `LOCK_SHA_LINE` in Step 6 is the fixture constant with the same text. Keep both names: the fixture must not import the rule's value, or a typo in the rule could pass its own test.)

8d. Replace the docstring of `_checker_tail` (`:361-363`) with:

```python
    """A `lockstep_check.py artifact|status|same` command must be the LAST command of its step,
    whole, with no `||`, `&&`, `;`, `|` or `&` joined to it and no open if/for/while around it. Else
    a refusal (exit 3) can be ignored: `... || true`, `... || rc=$?`, `if false; then ...; fi`. This
    applies to every job (SMA-738): `same` is the last command of the build step `stage`. The `lock`
    subcommand is not in CHECKER_SUBS, so the `lock` step keeps its `|| rc=$?`."""
```

8e. In `command_violations` (`:382-420`), directly after the `RUNNER_FILES` check (`:388-389`), add:

```python
    if RUNNER_TEMP_ASSIGN.search(script):
        out.append(f"P25 {where}: a run script assigns RUNNER_TEMP, which would move the paths that the stage compare and the upload use")
```

and inside the loop `for cmd in found:`, directly after `word = cmd[0]`, add:

```python
        if "RUNNER_TEMP" in cmd[1:]:
            out.append(f"P25 {where}: {word!r} names RUNNER_TEMP as an argument, so it can set the variable")
```

8f. In `_env_violations` (`:429-435`), inside `for key in env:`, add:

```python
        if key == "RUNNER_TEMP":
            out.append(f"P25 {where}: env sets RUNNER_TEMP, which would move the paths that the stage compare and the upload use")
```

8g. In `step_violations` (`:467-470`), replace:

```python
        if "run" in step:
            out += command_violations(job, str(step["run"]), where)
            if job == "propose":
                out += _checker_tail(str(step["run"]), where)
```

with:

```python
        if "run" in step:
            out += command_violations(job, str(step["run"]), where)
            out += _checker_tail(str(step["run"]), where)
```

8h. Directly before `def _propose_violations` (`:535`), add:

```python
def _build_violations(build: dict) -> list[str]:
    """P25 (SMA-738 spec 4.5). The lock step writes the SHA-256 of the lock bytes it judged as a
    step output before container run 2; the stage step compares the STAGED lock with it; upload
    sends exactly the stage directory. The exact token lists also refuse a repeated flag."""
    out = []
    steps = _steps(build, "build")
    ids = tuple(str(s.get("id")) for s in steps)
    if ids != BUILD_ORDER:
        out.append(f"P25 the build step order is {list(ids)}, expected {list(BUILD_ORDER)}")
    by_id = {str(s.get("id")): s for s in steps}
    for step_id in ("build", "stage", "upload"):
        if by_id.get(step_id, {}).get("if") != BUILD_IF:
            out.append(f"P25 the if: of build step {step_id} must be exactly {BUILD_IF}")
    lock_run = str(by_id.get("lock", {}).get("run", ""))
    lock_cmds = [c for c in commands(lock_run)[0] if c[:2] == ["python3", CHECKER]]
    if lock_cmds != [LOCK_CMD]:
        out.append(f"P25 the lock step must run exactly one checker command, {' '.join(LOCK_CMD)}, not {lock_cmds}")
    if LOCK_OUTPUT_LINE not in [line.strip() for line in lock_run.splitlines()]:
        out.append(f"P25 the lock step must hold the line {LOCK_OUTPUT_LINE}")
    stage = by_id.get("stage", {})
    stage_cmds = [c for c in commands(str(stage.get("run", "")))[0] if c[:2] == ["python3", CHECKER]]
    if stage_cmds != [SAME_CMD]:
        out.append(f"P25 the stage step must run exactly one checker command, {' '.join(SAME_CMD)}, not {stage_cmds}")
    if stage.get("env") != STAGE_ENV:
        out.append(f"P25 the stage env must be exactly {STAGE_ENV}, not {stage.get('env')!r}")
    upload = by_id.get("upload", {})
    with_block = upload.get("with") if isinstance(upload.get("with"), dict) else {}
    if with_block.get("path") != UPLOAD_PATH:
        out.append(f"P25 upload.with.path must be exactly {UPLOAD_PATH}, not {with_block.get('path')!r}")
    return out
```

8i. In `violations` (`:518-523`), inside `if isinstance(build, dict):`, after the `P9` outputs check, add:

```python
        out += _build_violations(build)
```

8j. In `main` (`:921`), replace `satisfies P0-P24` with `satisfies P0-P25`.

- [ ] **Step 9: Run the pin_check self-test and see it pass**

Run: `uv run --locked --project ci/wasm-lockstep --python '>=3.12' python3 ci/wasm-lockstep/pin_check.py --self-test; echo "rc=$?"`

Expected: every row `ok`, `pin_check self-test: N rows, 0 failed`, rc=0.

- [ ] **Step 10: Edit the workflow**

10a. `.github/workflows/wasm-lockstep.yml:17-18`. After the bullet that ends `...comes from lockstep_check.py's output on the downloaded bytes.`, add:

```yaml
#   * The `lock` step writes `lock_sha256`, the SHA-256 of the lock bytes it judged, as a STEP
#     output before container run 2. The `stage` step refuses a staged lock with other bytes
#     (R-RUN2). This control is in `build` only: `propose` cannot check it again (SMA-738).
```

10b. Replace the comment of the `lock` step (`:106-108`) with:

```yaml
      # The early exit, in the runbook's order: a refusal stops the job before any compile. For the
      # lock VERDICT this is not the control: propose runs the same checker again on the downloaded
      # bytes. It IS the only control that run 2 did not change the lock (SMA-738): on exit 0 it
      # writes lock_sha256, the SHA-256 of the bytes it judged, and the stage step compares the
      # staged lock with it. The host writes `changed` and `lock_sha256` here, before the compile,
      # so third-party code cannot set them.
```

10c. In the `lock` step script, replace:

```yaml
          if [ "$rc" -eq 0 ]; then
            echo "changed=true" >> "$GITHUB_OUTPUT"
```

with:

```yaml
          if [ "$rc" -eq 0 ]; then
            sha="$(sed -n 's/^lock-sha256 \([0-9a-f]\{64\}\)$/\1/p' "$RUNNER_TEMP/lock-verdict.txt")"
            echo "changed=true" >> "$GITHUB_OUTPUT"
            echo "lock_sha256=${sha}" >> "$GITHUB_OUTPUT"
```

An empty `sha` (a broken extraction) makes `same` exit 2 in the `stage` step. The bump then goes red; it is not passed (spec 4.5).

10d. Replace the comment of the `stage` step (`:140-141`) with:

```yaml
      # Copy only. The host executes no file of the work copy. Every path component is checked for
      # a symlink, so a symlinked file or parent directory cannot make cp read a host file. The last
      # command compares the STAGED lock with the lock_sha256 of the lock step: a lock that run 2
      # changed refuses with R-RUN2, and upload does not run (SMA-738). P18 and P25 pin it.
```

10e. In the `stage` step, add an `env:` after `if: steps.lock.outputs.changed == 'true'` (`:144`):

```yaml
        env:
          LOCK_SHA256: ${{ steps.lock.outputs.lock_sha256 }}
```

and add the final command after the `done` of the copy loop (`:163`), as the LAST line of the script:

```yaml
          python3 ci/wasm-lockstep/lockstep_check.py same --sha256 "$LOCK_SHA256" --file "$RUNNER_TEMP/stage/rs/Cargo.lock"
```

- [ ] **Step 11: Add the real-workflow mutations and run pin_check on the real file**

11a. In `pin_check.py` `negative_control` (`:868-906`), directly before the final `print(...)`, add:

```python
    # SMA-738: three mutations of the REAL build job. The fixture rows prove the rule; only these
    # prove that it bites on the real structure.
    def build_run(step_id: str, change) -> dict:
        mutated = copy.deepcopy(real)
        for step in mutated["jobs"]["build"]["steps"]:
            if step.get("id") == step_id:
                step["run"] = "".join(change(line) for line in str(step["run"]).splitlines(keepends=True))
        return mutated

    expect("the same line deleted from the stage step", build_run("stage", lambda ln: "" if " same --sha256 " in ln else ln), "P25")
    expect("|| true appended to the same line", build_run("stage", lambda ln: ln.rstrip("\n") + " || true\n" if " same --sha256 " in ln else ln), "P18")
    expect("the lock_sha256 line deleted from the lock step", build_run("lock", lambda ln: "" if "lock_sha256=" in ln else ln), "P25")
```

and change the final line to:

```python
    print(f"pin_check negative control: 7 mutations, {failures} failed")
```

11b. Run:

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-738-wasm-lockstep
uv run --locked --project ci/wasm-lockstep --python '>=3.12' python3 ci/wasm-lockstep/pin_check.py .github/workflows/wasm-lockstep.yml; echo "rc=$?"
uv run --locked --project ci/wasm-lockstep --python '>=3.12' python3 ci/wasm-lockstep/pin_check.py --negative-control .github/workflows/wasm-lockstep.yml; echo "rc=$?"
```

Expected: `pin_check: .github/workflows/wasm-lockstep.yml satisfies P0-P25`, rc=0. Then seven `ok` lines and `pin_check negative control: 7 mutations, 0 failed`, rc=0.

To see the new mutations bite, change `"P25"` of the first new `expect` to `"P24"`, run the negative control, and see `FAIL  the same line deleted from the stage step: want P24, got [...P25 ...]`. Set it back.

- [ ] **Step 12: Check that the workflow `sed` and the printed hash agree (Review Focus 4)**

`c9df6f09` holds the 0.2.128 family and `8c93b3d3` the merged 0.2.129 bump (no lock change in between), so this pair gives a real exit 0.

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-738-wasm-lockstep
SCRATCH=/private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/31ebe72c-c15a-47ff-9c97-71e29e9e5eb6/scratchpad
git show c9df6f09:rs/Cargo.lock > "$SCRATCH/rf-old.lock"
git show 8c93b3d3:rs/Cargo.lock > "$SCRATCH/rf-new.lock"
rc=0; python3 ci/wasm-lockstep/lockstep_check.py lock --old "$SCRATCH/rf-old.lock" --new "$SCRATCH/rf-new.lock" > "$SCRATCH/rf-verdict.txt" || rc=$?
echo "lock rc=$rc"; cat "$SCRATCH/rf-verdict.txt"
sha="$(sed -n 's/^lock-sha256 \([0-9a-f]\{64\}\)$/\1/p' "$SCRATCH/rf-verdict.txt")"
echo "sed: ${sha}"
shasum -a 256 "$SCRATCH/rf-new.lock"
python3 ci/wasm-lockstep/lockstep_check.py same --sha256 "$sha" --file "$SCRATCH/rf-new.lock"; echo "same rc=$?"
```

Expected: `lock rc=0`, seven `family-moved` lines, one `lock-sha256` line; `sed:` prints 64 hex characters equal to the first field of `shasum`; `same: rs/Cargo.lock sha256 <hex> did not change after the lock verdict` and `same rc=0`. If `lock rc` is 3, stop and report the refusal line: the pair is not the clean bump this step assumes.

- [ ] **Step 13: Update the documents for 4.5**

13a. `ci/wasm-lockstep/README.md`, refusal table: after the `R-DANGLING` row, add:

```markdown
| `R-RUN2` | (`same` only) The staged `rs/Cargo.lock` does not have the SHA-256 that the `lock` step printed, or it is a symlink, not a regular file, or over 8 MiB. Container run 2 changed the lock (SMA-738). |
```

13b. After the paragraph that starts `` `artifact` adds `R-LAYOUT` ``, add:

```markdown
`lock` prints `lock-sha256 <64 hex>` on exit 0 only: the SHA-256 of exactly the bytes that the
verdict judged (one read, binary mode, no newline translation). `same --sha256 <64 hex> --file
<path>` compares the bytes of one file with that value and refuses with `R-RUN2`. A malformed or
empty `--sha256`, a missing or unreadable file, and a flag given two times exit 2. `artifact` also
prints `lock-sha256` of the downloaded lock, so a reviewer can match the three values (`lock`,
`same`, `artifact`) across the job logs.
```

13c. Replace the P18 row with:

```markdown
| P18 | The `artifact`, `status` and `same` commands of `lockstep_check.py` are whole commands, in every job: the last command of the step, with no `\|\|`, `;`, pipe, `&&` guard, `if` or later `exit` joined to them. A refusal then fails the step. The `lock` command is not one of them: its step keeps `\|\| rc=$?`. |
```

13d. After the P24 row, add:

```markdown
| P25 | The `build` step ids are exactly `reclaim, checkout, ref, copy, update, lock, build, stage, upload`. The `if:` of `build`, `stage` and `upload` is exactly `steps.lock.outputs.changed == 'true'`. The `lock` step runs exactly one checker command, `python3 ci/wasm-lockstep/lockstep_check.py lock --old "$RUNNER_TEMP/old.lock" --new "$RUNNER_TEMP/work/rs/Cargo.lock"`, and holds the line `echo "lock_sha256=${sha}" >> "$GITHUB_OUTPUT"`. The `stage` step runs exactly one checker command, `python3 ci/wasm-lockstep/lockstep_check.py same --sha256 "$LOCK_SHA256" --file "$RUNNER_TEMP/stage/rs/Cargo.lock"`, and its `env:` is exactly `LOCK_SHA256: ${{ steps.lock.outputs.lock_sha256 }}`. `upload.with.path` is exactly `${{ runner.temp }}/stage/`. No script and no `env:` key at any level sets `RUNNER_TEMP` (SMA-738). |
```

13e. Trust model. Replace the bullet `- The host writes `changed` BEFORE the compile, so third-party code cannot set it.` with:

```markdown
- The host writes `changed` and `lock_sha256` BEFORE the compile, so third-party code cannot set
  them.
- **The run-1 lock compare (SMA-738).** On exit 0 the `lock` step writes `lock_sha256`, the SHA-256
  of the exact bytes that the verdict judged, as a STEP output, before container run 2. A step
  output cannot change after its step ends, and no file holds it. The last command of the `stage`
  step, `lockstep_check.py same`, refuses a staged `rs/Cargo.lock` with other bytes (`R-RUN2`), so
  `upload` and `propose` do not run. An edge move or a family entry that reaches `propose` can then
  come only from `cargo update` in run 1, which runs no build script. This control is in `build`
  only: `propose` cannot check it again, because that needs a second job output (P9). It depends
  on `pin_check.py` (P18, P25) and on the build runner host. A container escape in run 2 can reach
  the runner and defeat it: this is the same residual as the cache scope (Q7).
```

13f. In "What the checks do not prove", replace:

```markdown
- `pin_check.py` does not detect the removal of the per-component symlink loop of the stage step.
  It does not detect an upload of `$RUNNER_TEMP/work/` in place of the stage directory. The
  `propose` re-check of the layout (`R-LAYOUT`, `R-SYMLINK`) catches a wrong layout, not the
  removal of the loop.
```

with:

```markdown
- `pin_check.py` does not detect the removal of the per-component symlink loop of the stage step.
  The `propose` re-check of the layout (`R-LAYOUT`, `R-SYMLINK`) catches a wrong layout, not the
  removal of the loop.
```

13g. `rs/CLAUDE.md`. In the text that Task 1 wrote, replace `a new transitive dependency, a newer `syn`, or `R-EDGE`, see below)` with `a new transitive dependency, a newer `syn`, `R-EDGE` or `R-RUN2`, see below)`. After the `**`R-EDGE` with no family move (SMA-738).**` paragraph, add:

```markdown
  **`R-RUN2` (SMA-738).** Container run 2 changed `rs/Cargo.lock` after the `lock` step judged
  it: the `stage` step compares the staged lock with the `lock_sha256` output of the `lock` step.
  Reproduce run 2 locally in the pinned image, as in M2 (`ci/wasm-lockstep/README.md`), and diff
  the lock before and after it. If the change is benign (for example a lock repair by
  `wasm-pack`), open an issue and use the manual runbook below for the bump. If it is not benign,
  treat the family release as hostile and do not merge it.
```

- [ ] **Step 14: Lint and run the gate wrapper**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-738-wasm-lockstep
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
uv run --locked --project py ruff check ci/wasm-lockstep/
./ci/wasm-lockstep/run.sh --self-test; echo "rc=$?"
./ci/wasm-lockstep/run.sh --negative-control; echo "rc=$?"
./ci/wasm-lockstep/run.sh; echo "rc=$?"
```

Expected: `All checks passed!`; the three `run.sh` modes end with rc=0 (`== wasm-lockstep self-test passed ==`, `== wasm-lockstep negative control passed ==`, two `PASS` lines).

- [ ] **Step 15: Commit (the ONE commit of spec 4.5)**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-738-wasm-lockstep
git add ci/wasm-lockstep/lockstep_check.py ci/wasm-lockstep/pin_check.py .github/workflows/wasm-lockstep.yml ci/wasm-lockstep/README.md rs/CLAUDE.md
git status --short
git commit -m "feat(ci): refuse a lock that container run 2 changed in wasm-lockstep (SMA-738)" -m "The lock step writes lock_sha256, the SHA-256 of the lock bytes that the verdict judged, as a
step output before container run 2. The stage step ends with lockstep_check.py same, which
refuses a staged lock with other bytes (R-RUN2). pin_check P18 now runs for every job, and the
new rule P25 pins the build steps, the exact commands, the stage env and the upload path. This
commit holds all of spec 4.5, so one revert removes it and keeps the edge rule." -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

Expected: `git status --short` shows the five files staged and nothing else. The commit succeeds.

---

### Task 3: The real local reproduction (spec 5.4). No commit.

**Files:**
- Create (scratch only, not in the repo): `$SCRATCH/sma-738-repro/` and `$SCRATCH/sma-738-pr-notes.md`

**Interfaces:**
- Consumes: Tasks 1 and 2 at the feature-branch HEAD; `main:ci/wasm-lockstep/lockstep_check.py` as the old checker.
- Produces: `$SCRATCH/sma-738-pr-notes.md` (the text for the PR body: `cargo --version`, both checker outputs and exit codes).

- [ ] **Step 1: Make the scratch copy**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
WT=/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-738-wasm-lockstep
SCRATCH=/private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/31ebe72c-c15a-47ff-9c97-71e29e9e5eb6/scratchpad
R="$SCRATCH/sma-738-repro"
rm -rf "$R" && mkdir -p "$R/tree"
git -C "$WT" archive HEAD | tar -x -C "$R/tree"
git -C "$WT" show HEAD:rs/Cargo.lock > "$R/head.lock"
git -C "$WT" show main:ci/wasm-lockstep/lockstep_check.py > "$R/old_checker.py"
ls "$R/tree/rs/Cargo.lock" "$R/head.lock" "$R/old_checker.py"
```

Expected: the three paths print. No error.

- [ ] **Step 2: Run cargo update with cargo 1.95.0**

```bash
R=/private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/31ebe72c-c15a-47ff-9c97-71e29e9e5eb6/scratchpad/sma-738-repro
( cd "$R/tree/rs" && cargo --version && cargo update -p wasm-bindgen -p js-sys -p web-sys -p wasm-bindgen-futures ) > "$R/cargo.out" 2>&1; echo "cargo rc=$?"
cat "$R/cargo.out"
diff "$R/head.lock" "$R/tree/rs/Cargo.lock" > "$R/lock.diff"; echo "diff rc=$?"
cat "$R/lock.diff"
```

Expected: `cargo 1.95.0 (...)` (from `rs/rust-toolchain.toml`), `cargo rc=0`, `diff rc=1`, and a diff of five lines `< "windows-sys 0.61.2",` / `> "windows-sys 0.52.0",` (plus family entries only if a new family release exists). If `cargo --version` is not 1.95.0, stop and report. If the sandbox blocks the network for the crates.io index, stop and ask the coordinator; do not change the sandbox yourself.

- [ ] **Step 3: Run the new checker**

```bash
WT=/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-738-wasm-lockstep
R=/private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/31ebe72c-c15a-47ff-9c97-71e29e9e5eb6/scratchpad/sma-738-repro
rc=0; python3 "$WT/ci/wasm-lockstep/lockstep_check.py" lock --old "$R/head.lock" --new "$R/tree/rs/Cargo.lock" > "$R/new.out" 2> "$R/new.err" || rc=$?
echo "new checker rc=$rc"; cat "$R/new.out" "$R/new.err"
```

Expected: `new checker rc=4`, seven `family-current` lines, then exactly these five lines, and no `lock-sha256` line, and an empty stderr:

```
edge-moved errno 0.3.14 windows-sys 0.61.2 0.52.0
edge-moved quinn-udp 0.5.15 windows-sys 0.61.2 0.52.0
edge-moved rustix 1.1.5 windows-sys 0.61.2 0.52.0
edge-moved tempfile 3.27.0 windows-sys 0.61.2 0.52.0
edge-moved winapi-util 0.1.11 windows-sys 0.61.2 0.52.0
```

Also correct (spec 5.4 step 3): rc=0 with `family-moved` lines, the same five `edge-moved` lines and one `lock-sha256` line, if a new family release appeared. Any `R-NONFAMILY` or `R-EDGE` is a failure: stop and report.

- [ ] **Step 4: Run the workflow's `sed` on the exit-4 output (Review Focus 5)**

```bash
R=/private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/31ebe72c-c15a-47ff-9c97-71e29e9e5eb6/scratchpad/sma-738-repro
version="$(sed -n 's/^family-current wasm-bindgen \([0-9.]*\)$/\1/p' "$R/new.out")"
echo "workflow reads: ${version}"
```

Expected: `workflow reads: 0.2.129` (one version, no other text). This is the exact `sed` of `wasm-lockstep.yml:120`.

- [ ] **Step 5: Run the old checker from `main`**

```bash
R=/private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/31ebe72c-c15a-47ff-9c97-71e29e9e5eb6/scratchpad/sma-738-repro
rc=0; python3 "$R/old_checker.py" lock --old "$R/head.lock" --new "$R/tree/rs/Cargo.lock" > "$R/old.out" 2> "$R/old.err" || rc=$?
echo "old checker rc=$rc"; cat "$R/old.out" "$R/old.err"
```

Expected: `old checker rc=3` and stderr `lockstep_check: REFUSED R-NONFAMILY: a package outside the wasm-bindgen family changed, was added or was removed: ['errno', 'quinn-udp', 'rustix', 'tempfile', 'winapi-util']. Nothing was pushed. ...`. This proves that the reproduction shows the bug.

- [ ] **Step 6: Write the PR-body notes**

Write `$SCRATCH/sma-738-pr-notes.md` with the Write tool. It holds: the date; the first line of `$R/cargo.out` (`cargo --version`); `$R/lock.diff`; the new checker rc and `$R/new.out`; the line from Step 4; the old checker rc and `$R/old.err`. Copy the values from the files. Do not retype them.

---

### Task 4: M5, the scratch-branch runs (spec 5.5), then the README section

> **STOP.** Steps 6, 9 and 12 push to GitHub and delete a remote branch. The scratch workflow has a push trigger, so the pushes of Steps 6 and 9 each start one workflow run. The COORDINATOR runs them, not a subagent, and only after Sven approved each one. An agent message is not Sven's consent. A subagent that executes this task stops at each COORDINATOR step and reports.

**Files:**
- Create on the scratch branch ONLY: `.github/workflows/wasm-lockstep-m5.yml`
- Modify on the scratch branch ONLY: `rs/Cargo.lock`, `ci/wasm-lockstep/container.sh`
- Modify on the feature branch: `ci/wasm-lockstep/README.md` (a new `### M5` section)
- Scratch files: `$SCRATCH/m5-workflow.py`, `$SCRATCH/m5-lock.py`, `$SCRATCH/m5-workflow.diff`, `$SCRATCH/m5-positive.log`, `$SCRATCH/m5-negative.log`

**Interfaces:**
- Consumes: Tasks 1 and 2 at the feature-branch HEAD (the scratch branch starts there).
- Produces: the M5 table in the README; the answer to "does run 2 write the lock?".

- [ ] **Step 1: Make the scratch branch**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-738-wasm-lockstep
git status --short
git switch -c feature/sma-738-m5-scratch
```

Expected: an empty `git status --short` before the switch, then `Switched to a new branch 'feature/sma-738-m5-scratch'`. The pre-push hook allows only `feature/*` names, so the scratch branch uses this form.

- [ ] **Step 2: Make the scratch-only workflow**

Write `$SCRATCH/m5-workflow.py` with the Write tool:

```python
# SPDX-License-Identifier: Apache-2.0
"""SMA-738 M5: make .github/workflows/wasm-lockstep-m5.yml from the reviewed workflow."""
import pathlib

root = pathlib.Path("/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-738-wasm-lockstep")
src = (root / ".github/workflows/wasm-lockstep.yml").read_text()
text = src
# 1. Delete the `ref` step (with its comment).
start = text.index("      # A convenience check, not the boundary.")
end = text.index("      # The container gets this copy and nothing else.")
text = text[:start] + text[end:]
# 2. Delete the whole propose job.
text = text[:text.index("\n  propose:\n")].rstrip("\n") + "\n"
# 3. A push trigger on the scratch branch only, and no workflow_dispatch. GitHub runs
#    workflow_dispatch only for a workflow file that exists on the default branch. M0 used push.
on_start = text.index("on:\n  schedule:\n")
on_end = text.index("  workflow_dispatch:\n", on_start) + len("  workflow_dispatch:\n")
text = text[:on_start] + "on:\n  push:\n    branches:\n      - feature/sma-738-m5-scratch\n" + text[on_end:]
# 4. A scratch name and a scratch header.
text = text.replace("name: wasm-lockstep\n", "name: wasm-lockstep-m5\n", 1)
text = text.replace(
    "# SPDX-License-Identifier: Apache-2.0\n#\n",
    "# SPDX-License-Identifier: Apache-2.0\n#\n"
    "# SMA-738 M5 SCRATCH copy of wasm-lockstep.yml, on feature/sma-738-m5-scratch ONLY. NEVER merge it.\n"
    "# Changes: no `ref` step, no `propose` job (no environment, no secret, no App token), and a push\n"
    "# trigger on this branch only. Each push to the branch starts one run.\n#\n",
    1,
)
assert "id: ref\n" not in text and "\n  propose:" not in text and "schedule:" not in text
assert "workflow_dispatch" not in text and "      - feature/sma-738-m5-scratch\n" in text
assert "secrets." not in text and "environment:" not in text
(root / ".github/workflows/wasm-lockstep-m5.yml").write_text(text)
print("wrote wasm-lockstep-m5.yml")
```

Run:

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-738-wasm-lockstep
SCRATCH=/private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/31ebe72c-c15a-47ff-9c97-71e29e9e5eb6/scratchpad
python3 "$SCRATCH/m5-workflow.py"
diff -u .github/workflows/wasm-lockstep.yml .github/workflows/wasm-lockstep-m5.yml > "$SCRATCH/m5-workflow.diff"; echo "diff rc=$?"
cat "$SCRATCH/m5-workflow.diff"
```

Expected: `wrote wasm-lockstep-m5.yml`, `diff rc=1`, and a diff that shows only the changes of the script: the header, the name, the `on:` block (`push:` with `branches:` `- feature/sma-738-m5-scratch`, and no `schedule` and no `workflow_dispatch`), the removed `ref` step and the removed `propose` job.

Do not push in Steps 1 to 5. The push trigger starts one run for each push. So the first push (Step 6) must hold every scratch edit of the positive run: the workflow and the lock, in one commit.

- [ ] **Step 3: Put the 0.2.128 family of `c9df6f09` into the scratch lock**

Write `$SCRATCH/m5-lock.py` with the Write tool:

```python
# SPDX-License-Identifier: Apache-2.0
"""SMA-738 M5: replace the seven family entries of rs/Cargo.lock with the 0.2.128 entries of
c9df6f09, and the family references of the other packages with the references of c9df6f09."""
import pathlib
import re
import subprocess

root = pathlib.Path("/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-738-wasm-lockstep")
FAMILY = {"wasm-bindgen", "wasm-bindgen-macro", "wasm-bindgen-macro-support", "wasm-bindgen-shared", "js-sys", "web-sys", "wasm-bindgen-futures"}
SEP = "\n[[package]]\n"
REF = re.compile(r'^ "([^" ]+)[^"]*",$', re.M)


def split(text):
    first, *blocks = text.split(SEP)
    return first, blocks


def key(block):
    match = re.match(r'name = "([^"]+)"\nversion = "([^"]+)"\n', block)
    assert match, block[:80]
    return match.group(1), match.group(2)


lock_path = root / "rs/Cargo.lock"
head = lock_path.read_text()
base = subprocess.run(["git", "-C", str(root), "show", "c9df6f09:rs/Cargo.lock"], capture_output=True, text=True, check=True).stdout
h_first, h_blocks = split(head)
b_first, b_blocks = split(base)
assert h_first == b_first, "the lock header differs"
b_family = {key(b)[0]: b for b in b_blocks if key(b)[0] in FAMILY}
b_by_key = {key(b): b for b in b_blocks}
assert len(b_family) == 7 and all(key(b)[1] in ("0.2.128", "0.3.105", "0.4.78") for b in b_family.values()), b_family.keys()
out, swapped, refs = [], 0, 0
for block in h_blocks:
    name, version = key(block)
    if name in FAMILY:
        out.append(b_family[name])
        swapped += 1
        continue
    base_block = b_by_key.get((name, version))
    if base_block is not None:
        base_refs = {m.group(1): m.group(0) for m in REF.finditer(base_block) if m.group(1) in FAMILY}

        def swap(match, base_refs=base_refs):
            global refs
            if match.group(1) in base_refs and base_refs[match.group(1)] != match.group(0):
                refs += 1
                return base_refs[match.group(1)]
            return match.group(0)
        block = REF.sub(swap, block)
    out.append(block)
assert swapped == 7, swapped
lock_path.write_text(SEP.join([h_first, *out]))
print(f"replaced {swapped} family entries and {refs} family references")
```

Run:

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-738-wasm-lockstep
SCRATCH=/private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/31ebe72c-c15a-47ff-9c97-71e29e9e5eb6/scratchpad
python3 "$SCRATCH/m5-lock.py"
git show HEAD:rs/Cargo.lock > "$SCRATCH/m5-head.lock"
rc=0; python3 ci/wasm-lockstep/lockstep_check.py lock --old rs/Cargo.lock --new "$SCRATCH/m5-head.lock" || rc=$?; echo "rc=$rc"
```

Expected: `replaced 7 family entries and 0 family references` (the references are bare names in both locks); then `rc=0` with seven `family-moved` lines from 0.2.128 / 0.3.105 / 0.4.78 to 0.2.129 / 0.3.106 / 0.4.79, no `edge-moved` line, and one `lock-sha256` line. So the scratch lock differs from HEAD only in the family. If the reference count is not 0, record it in the M5 notes; it is allowed by spec 5.5 step 3.

- [ ] **Step 4: Local pre-check with cargo 1.95.0 (spec 5.5 step 3)**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-738-wasm-lockstep
SCRATCH=/private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/31ebe72c-c15a-47ff-9c97-71e29e9e5eb6/scratchpad
P="$SCRATCH/m5-precheck"
rm -rf "$P" && mkdir -p "$P/tree"
cp rs/Cargo.lock "$P/old.lock"
git archive HEAD | tar -x -C "$P/tree"
cp rs/Cargo.lock "$P/tree/rs/Cargo.lock"
( cd "$P/tree/rs" && cargo --version && cargo update -p wasm-bindgen -p js-sys -p web-sys -p wasm-bindgen-futures ); echo "cargo rc=$?"
rc=0; python3 ci/wasm-lockstep/lockstep_check.py lock --old "$P/old.lock" --new "$P/tree/rs/Cargo.lock" > "$P/verdict.txt" || rc=$?; echo "rc=$rc"; cat "$P/verdict.txt"
```

(The lock is copied over the archive, because `git archive HEAD` holds the committed lock and the scratch lock is not committed yet.)

Expected: `cargo 1.95.0`, `cargo rc=0`, `rc=0`, seven `family-moved` lines, the five `edge-moved` lines of Task 3 Step 3, and one `lock-sha256` line. If the five `edge-moved` lines do not appear, stop and report: M5 then does not test the edge flip.

- [ ] **Step 5: Commit on the scratch branch**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-738-wasm-lockstep
git add .github/workflows/wasm-lockstep-m5.yml rs/Cargo.lock
git status --short
git commit -m "test(ci): add the SMA-738 M5 scratch workflow and a 0.2.128 family lock" -m "A scratch branch only. It never merges, and no pull request is opened for it." -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git rev-parse HEAD
```

Expected: two staged files, a successful commit, and a SHA. Record it as `POS_HEAD`. Check with `git status --short` that the tree is clean, so that no positive-run edit is left out of the commit.

- [ ] **Step 6: COORDINATOR — push the scratch branch, which starts the positive run (after Sven approved it)**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-738-wasm-lockstep
git push -u origin feature/sma-738-m5-scratch
gh run list --workflow wasm-lockstep-m5.yml --branch feature/sma-738-m5-scratch --json databaseId,status,headSha --jq '.[] | [.databaseId, .status, .headSha] | @tsv'
```

Expected: the push succeeds and starts one run of `wasm-lockstep-m5`. The list shows one run whose `headSha` equals `POS_HEAD` (a new run can take some seconds to appear; run the list again if it is empty). Note its `databaseId` as `RUN_POS`. Wait with `gh run watch "$RUN_POS" --exit-status`.

- [ ] **Step 7: Collect the positive values**

```bash
SCRATCH=/private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/31ebe72c-c15a-47ff-9c97-71e29e9e5eb6/scratchpad
RUN_POS=<the databaseId from Step 6>
gh run view "$RUN_POS" --json conclusion,url,headSha,jobs --jq '{conclusion, url, headSha, steps: [.jobs[].steps[] | [.name, .conclusion]]}'
gh run view "$RUN_POS" --log > "$SCRATCH/m5-positive.log"
grep -E 'family-moved|edge-moved|lock-sha256|^.*same: rs/Cargo.lock' "$SCRATCH/m5-positive.log"
```

Expected: conclusion `success`; the `lock` step log holds seven `family-moved`, five `edge-moved` and one `lock-sha256 <hex>` line; the `stage` step log holds `same: rs/Cargo.lock sha256 <hex> did not change after the lock verdict` with the SAME hex. The work lock after run 2 has that SHA-256 too, because `stage` copies it with `cp` and `same` hashes the copy. So run 2 did not write the lock. If the `stage` step instead refused with `R-RUN2`, run 2 DOES write the lock: stop, record the two hashes and the size, and report to the coordinator (spec 7, the `R-RUN2` runbook).

- [ ] **Step 8: Prepare the negative run**

In `ci/wasm-lockstep/container.sh` (scratch branch only), in the `build)` case, directly after the line `    moon run paigasus-kernel-ts:generate-wasm`, add the line:

```bash
    printf '\n' >> rs/Cargo.lock
```

Then:

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-738-wasm-lockstep
git add ci/wasm-lockstep/container.sh
git commit -m "test(ci): append a byte to the lock in container run 2 for the SMA-738 M5 negative run" -m "A scratch branch only. It never merges." -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git rev-parse HEAD
```

Expected: a successful commit. Record the SHA as `NEG_HEAD`. Do not push here. Step 9 pushes this one commit, and that push starts the negative run.

- [ ] **Step 9: COORDINATOR — push the container.sh commit, which starts the negative run (after Sven approved it)**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-738-wasm-lockstep
git push origin feature/sma-738-m5-scratch
gh run list --workflow wasm-lockstep-m5.yml --branch feature/sma-738-m5-scratch --json databaseId,status,headSha --jq '.[] | [.databaseId, .status, .headSha] | @tsv'
```

Expected: the push starts one new run. The list now shows two runs. Take the run whose `headSha` equals `NEG_HEAD` (not the newest row by position). Note it as `RUN_NEG`. Wait with `gh run watch "$RUN_NEG" --exit-status` (it ends with a non-zero exit, as expected).

- [ ] **Step 10: Collect the negative values**

```bash
SCRATCH=/private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/31ebe72c-c15a-47ff-9c97-71e29e9e5eb6/scratchpad
RUN_NEG=<the databaseId from Step 9>
gh run view "$RUN_NEG" --json conclusion,url,headSha,jobs --jq '{conclusion, url, headSha, steps: [.jobs[].steps[] | [.name, .conclusion]]}'
gh run view "$RUN_NEG" --log > "$SCRATCH/m5-negative.log"
grep -E 'lock-sha256|REFUSED R-RUN2' "$SCRATCH/m5-negative.log"
```

Expected: conclusion `failure`; the step "Stage the six files" has conclusion `failure`; "Upload the artifact" has conclusion `skipped`; the log holds `lockstep_check: REFUSED R-RUN2: rs/Cargo.lock changed after the lock verdict: the lock step judged sha256 <A>, ... has sha256 <B> (<N> bytes)`, where `<A>` equals the `lock-sha256` of the same run and `<N>` is one more than the size of the positive run's lock.

- [ ] **Step 11: Record M5 in the README on the feature branch**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-738-wasm-lockstep
git switch feature/sma-738-wasm-lockstep-edge-flips
git status --short
```

Expected: the switch succeeds and the tree is clean.

In `ci/wasm-lockstep/README.md`, directly before `### M4 — a second run with an open bot PR`, add the section below. Replace every `<...>` with the recorded value from Steps 5-10. Paste `$SCRATCH/m5-workflow.diff` whole into the `diff` block.

~~~markdown
### M5 — the run-1 lock compare on a GitHub runner (SMA-738)

Date: <YYYY-MM-DD>. The runs used the scratch branch `feature/sma-738-m5-scratch` and the
scratch-only workflow `.github/workflows/wasm-lockstep-m5.yml`. That workflow has a push trigger on
the scratch branch only, not `workflow_dispatch`: GitHub runs `workflow_dispatch` only for a
workflow file that exists on the default branch, and M0 used a push trigger too. Each push started
one run. The branch never merged, and it is deleted. Its `rs/Cargo.lock` held the seven family entries of `c9df6f09` (wasm-bindgen 0.2.128) in
place of the 0.2.129 entries. So the four-package `cargo update -p` moved the family and flipped
the five `windows-sys` edges, as on `main`.

| Item | Value |
|---|---|
| Positive run | <URL> (head <POS_HEAD>) |
| Positive conclusion | success |
| `lock` step | seven `family-moved` lines, five `edge-moved` lines (`errno`, `quinn-udp`, `rustix`, `tempfile`, `winapi-util`: `windows-sys` 0.61.2 to 0.52.0) |
| `lock-sha256` | <hex> |
| `stage` step | `same: rs/Cargo.lock sha256 <hex> did not change after the lock verdict` |
| SHA-256 of the work lock after run 2 | <hex>. It equals the `lock-sha256`, so container run 2 did not write the lock. |
| Negative run | <URL> (head <NEG_HEAD>), with `printf '\n' >> rs/Cargo.lock` at the end of the `build` case of `container.sh` |
| Negative conclusion | failure |
| `stage` step | failure: `<the REFUSED R-RUN2 line>` |
| `upload` step | skipped |

The diff of the scratch workflow against the reviewed file:

```diff
<the content of m5-workflow.diff>
```
~~~

Then check that no marker is left:

```bash
grep -nE '<(YYYY|URL|POS_HEAD|NEG_HEAD|hex|the )' ci/wasm-lockstep/README.md; echo "grep rc=$?"
```

Expected: no output and `grep rc=1`.

- [ ] **Step 12: COORDINATOR — delete the remote scratch branch (after Sven approved it)**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-738-wasm-lockstep
git push origin --delete feature/sma-738-m5-scratch
git ls-remote --heads origin feature/sma-738-m5-scratch; echo "ls-remote rc=$?"
```

Expected: the delete succeeds; `git ls-remote` prints nothing (rc=0 with no line). The M5 artifact expires after 7 days. To delete it now: `gh api -X DELETE "repos/<owner>/<repo>/actions/artifacts/<id>"`, with the id from `gh api "repos/<owner>/<repo>/actions/runs/$RUN_POS/artifacts" --jq '.artifacts[] | [.id, .name] | @tsv'`.

- [ ] **Step 13: Delete the local scratch branch**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-738-wasm-lockstep
git branch -D feature/sma-738-m5-scratch
git branch --list 'feature/sma-738-m5-scratch'
```

Expected: `Deleted branch feature/sma-738-m5-scratch (was ...)`, then no output.

- [ ] **Step 14: Commit the M5 section on the feature branch**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-738-wasm-lockstep
git add ci/wasm-lockstep/README.md
git status --short
git commit -m "docs(ci): record the SMA-738 M5 scratch-branch runs" -m "A positive run on a hosted runner showed that container run 2 does not write rs/Cargo.lock,
and the stage step printed the same SHA-256 as the lock step. A negative run with one byte appended
to the lock in run 2 failed the stage step with R-RUN2, and upload did not run." -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

Expected: only the README is staged. The commit succeeds. If Step 7 found that run 2 writes the lock, change the first sentence of the body to the measured result before you commit.

---

### Task 5: The gates (spec 5.6 and the repo gate graph). No commit.

**Files:**
- None changed. Diagnosis notes go to `$SCRATCH/sma-738-gates.md`.

**Interfaces:**
- Consumes: the feature branch after Task 4 (three commits on top of `main`).
- Produces: a gate report for the coordinator.

- [ ] **Step 1: The wasm-lockstep gate, all three modes**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-738-wasm-lockstep
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
./ci/wasm-lockstep/run.sh --self-test; echo "rc=$?"
./ci/wasm-lockstep/run.sh --negative-control; echo "rc=$?"
./ci/wasm-lockstep/run.sh; echo "rc=$?"
```

Expected: rc=0 for each of the three.

- [ ] **Step 2: `repo:ruff-ci` (needs bash 4+)**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-738-wasm-lockstep
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
/opt/homebrew/bin/bash ci/ruff/run.sh --self-test; echo "rc=$?"
/opt/homebrew/bin/bash ci/ruff/run.sh --negative-control; echo "rc=$?"
/opt/homebrew/bin/bash ci/ruff/run.sh; echo "rc=$?"
```

Expected: rc=0 for each. A `mapfile: command not found` means that bash 3.2 ran it: use the Homebrew path as written.

- [ ] **Step 3: `repo:actionlint` (needs Homebrew bash 5 and a healthy pipe)**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-738-wasm-lockstep
SCRATCH=/private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/31ebe72c-c15a-47ff-9c97-71e29e9e5eb6/scratchpad
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
/opt/homebrew/bin/bash ci/actionlint/run.sh > "$SCRATCH/actionlint.out" 2>&1; echo "rc=$?"
grep -nE 'pipe capacity|FAIL' "$SCRATCH/actionlint.out"
```

Expected: rc=0, and the preflight line `pipe capacity 65536 bytes (floor 8192)`. If the gate exits rc 2 with a "small" pipe message, the host is in the 512-byte-pipe state: record it as a host artifact, not a finding, and CI gives the verdict. Any FAIL row on a healthy pipe is a finding: report the row. Pay attention to check 12 (a file that names moon's CI report without a marker) and check 13 (an early-exit reader): this change must red neither.

- [ ] **Step 4: The full gate graph with `moon ci` (under bash 3.2)**

`repo:affected-smoke` deadlocks under Homebrew bash 5.3.15 on this host, and Moon resolves `bash` through `PATH`. So shim only `bash` to 3.2:

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-738-wasm-lockstep
SCRATCH=/private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/31ebe72c-c15a-47ff-9c97-71e29e9e5eb6/scratchpad
mkdir -p "$SCRATCH/bashshim" && ln -sf /bin/bash "$SCRATCH/bashshim/bash"
export PATH="$SCRATCH/bashshim:$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
git fetch origin main
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site :http-extractor-envelope :input-liveness :promtool :observability-drift :nats-permissions :release-parity :release-parity-py :release-parity-ts :publish-metadata :version-lockstep :workflow-credentials :pyo3-stub-drift :ruff-ci :next-public-free :helm-render :moon-diagnosis-exec :wasm-lockstep :test-e2e --base origin/main --include-relations > "$SCRATCH/moon-ci.out" 2>&1; echo "moon ci rc=$?"
```

Run it with `run_in_background: true` if it can take more than ten minutes, and wait for it with the Monitor tool on the `moon ci rc=` line. Do not end the turn while it runs.

Expected: rc=0, or reds only in the gates that need bash 4+ under this bash 3.2 shim (`repo:ruff-ci`, `repo:next-public-free`, `repo:publish-metadata`) and in `repo:actionlint` (it needs bash 5). Step 2 and Step 3 already give the verdicts of `repo:ruff-ci` and `repo:actionlint`. If `repo:next-public-free` or `repo:publish-metadata` reds here, re-run it directly: `/opt/homebrew/bin/bash ci/next-public/run.sh` or `/opt/homebrew/bin/bash ci/publish-metadata/run.sh`, and read that result instead. A red with an empty `stdout.log` and a one-line `declare`/`mapfile` stderr is a bash-version artifact.

For any other red, follow the diagnosis procedure in the root `CLAUDE.md` ("Diagnosing an unattributed `moon ci` failure"). Do Step 0 first: copy the moon CI report and `.moon/cache/states/<project>/<task>/` to `$SCRATCH` before any re-run.

- [ ] **Step 5: Report**

Write `$SCRATCH/sma-738-gates.md` with each gate, its command, its rc, and for each red the cause (finding, or host artifact with the evidence line). Report it to the coordinator.

---

## Spec questions

**Q1 — Resolved:** the coordinator confirmed that the `name version` and `name version (source)` family rows of `_ref_rows` expect `R-EDGE`, and the bare-name family row stays `PASS` (spec 5.2 now says this).

**Q2 — "One reader function that `lock`, `artifact` and `same` all use" (spec 4.5).** The spec describes the reader as one that decodes strict UTF-8 and parses the text. `same` must not parse (it compares bytes, and a modified lock can be invalid TOML), and it needs `O_NOFOLLOW`, a regular-file check and a size cap. This plan makes ONE byte reader, `read_file(path, *, regular_only=False)`, that all three use: `lock` and `artifact` call it through `read_lock` (which decodes and parses), and `same` calls it with `regular_only=True`. If the spec means something else, only the internal structure changes, not the behaviour.

**Q3 — Resolved:** the coordinator decided that the M5 scratch workflow uses a push trigger on `feature/sma-738-m5-scratch` only, with no `workflow_dispatch`, as M0 did (spec 5.5 now says this).

**Q4 — The scope of "no step assigns `RUNNER_TEMP`" (spec 4.5, P25).** The spec names step `env:` keys and shell assignments. This plan puts the `env:` check into `_env_violations`, so it also covers a job-level and a workflow-level `env:` key. That is stricter than the spec and the real workflow passes it. It also refuses a command that names `RUNNER_TEMP` as an argument (`read -r RUNNER_TEMP`), the same way P4 treats `LOCKSTEP_IMAGE`.

**Q5 — The M5 workflow differs from the reviewed file in two more lines than the spec lists (spec 5.5 step 2).** The plan also changes `name:` to `wasm-lockstep-m5` and adds a three-line SCRATCH header, so that the two workflows are not shown with the same name in the Actions list. The README records the exact diff, as the spec requires.

**Q6 — The target of the 5.3 negative control when a name has three or more versions.** The spec says "re-pointed to the canonical ref of the other version". With three or more versions there is more than one other version. The plan takes the first other version in lock order that the package does not already refer to. On the current lock the target is `ahash 0.8.12`, reference `getrandom 0.3.4`.
