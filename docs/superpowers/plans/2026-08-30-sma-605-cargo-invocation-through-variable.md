# SMA-605 — cargo through a variable: Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make A8 and A10 see a cargo invocation that reaches cargo through a variable, and follow a gate script's `source` statements one level so the one real such invocation becomes reachable.

**Architecture:** Two new regexes sit beside `CARGO_INVOCATION_RE` and merge into one start-sorted match list, so `_classify_shell_line`'s per-invocation tail arithmetic is unchanged. A separate execution-only `source` resolver extends the script closure one level. Everything lands in one file, `ci/affected-graph/cargo_moon_parity.py`, plus documentation.

**Tech Stack:** Python 3 (standard library only — no dependency may be added; the file is executed by `python3` directly from `ci/affected-graph/run.sh`), Moon 2.5.3, bash.

**Spec:** `docs/superpowers/specs/2026-08-30-sma-605-cargo-invocation-through-variable-design.md`

## Global Constraints

- **Every source file opens with an SPDX header.** `cargo_moon_parity.py` already has one; do not disturb it.
- **No new Python dependency.** `cargo_moon_parity.py` runs under bare `python3`, not under `uv`.
- **`EXPECTED_FINDING_KEYS` must stay `("a1", "a2", "a3", "a4-lint", "a4-fmt", "a5", "a6", "a7", "a8", "a9", "a10")`.** This change widens A8 and A10; it adds no assertion. The new `check_sourced_scripts` rows join the **a8** bucket in `collect_findings`.
- **`self_test`'s closing line stays** `"  OK   [parity] all ten assertions fire on synthetic violations"`.
- **A failing self-test fixture appends to `failures`; it never raises.** Follow the existing idiom exactly.
- **A default-deny gate must not be able to pass vacuously.** Every new arm needs a fixture that fires AND a mutation that kills it.
- **Branch:** `feature/sma-605-cargo-invocation-through-variable`. Commits are conventional with a workspace scope, e.g. `ci(repo): …`.
- **Run everything with** `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` first.
- **The self-test is the test suite.** There is no pytest here. `python3 ci/affected-graph/cargo_moon_parity.py --self-test` is the unit-test command; exit 0 is pass.

---

### Task 1: The merged match list

**Files:**
- Modify: `ci/affected-graph/cargo_moon_parity.py` — add constants after `CARGO_CONFIG_INPUT` (near `:222`), add `cargo_matches` after `_tail_end` (near `:781`)
- Test: the same file's `self_test()`

**Interfaces:**
- Consumes: `LOCK_RESOLVING_VERBS`, `CARGO_INVOCATION_RE` (both existing)
- Produces: `CargoMatch(start, end, verb, kind)` namedtuple; `cargo_matches(text) -> list[CargoMatch]` sorted by `start`; `CARGO_VAR_CMD_RE`, `CARGO_ENV_PREFIX_RE`, `CARGO_VAR_NAME`. `kind` is one of `"literal"`, `"var"`, `"env"`. `verb` is the matched subcommand string, or `None` for `"env"`.

- [ ] **Step 1: Write the failing test**

Add to `self_test()`, immediately before the line `if not REQUIRED_LOCKED_TASKS:`:

```python
    # SMA-605 — the merged match list. Both arms are FORWARD COVER: arm 1 reports zero rows on
    # the real corpus and arm 2 exactly one, and only once the source resolver lands. These
    # fixtures are the whole proof that either arm works.
    def _kinds(text):
        return [(c.kind, c.verb) for c in cargo_matches(text)]

    for text, want in (
        ('cargo build --locked', [("literal", "build")]),
        ('"$CARGO_BIN" build', [("var", "build")]),
        ('"${CARGO_BIN}" build', [("var", "build")]),
        ('CARGO=/p release-plz update', [("env", None)]),
        # Arm 1 must NOT fire on a variable whose name does not mention cargo. All three are
        # live lines in this repo, and a naive widening reports all three (SMA-605 M4).
        ('git -C "$dir" add -A', []),
        ('echo "negative control: $failures check(s) failed to bite"', []),
        ('"$RELEASE_PLZ_BIN" update', []),
        # Arm 2 is EXACTLY `CARGO=`. CARGO_NET_OFFLINE configures cargo; it does not redirect it.
        ('CARGO_NET_OFFLINE=true tool update', []),
        # ...and it must still see a real CARGO= that follows one.
        ('CARGO_NET_OFFLINE=true CARGO=/p tool update', [("env", None)]),
        # The lookahead's job: an assignment with nothing to run is not an invocation.
        ('export CARGO=/p', []),
        # A lowercase `$cargo build` is ALREADY matched by CARGO_INVOCATION_RE (measured), so
        # without de-duplication arm 1 double-reports one invocation.
        ('$cargo build', [("literal", "build")]),
    ):
        if _kinds(text) != want:
            failures.append(
                f"cargo_matches({text!r}) is {_kinds(text)}, expected {want}"
            )
```

- [ ] **Step 2: Run it to verify it fails**

Run: `python3 ci/affected-graph/cargo_moon_parity.py --self-test`
Expected: FAIL with `NameError: name 'cargo_matches' is not defined`

- [ ] **Step 3: Add the constants**

Insert directly after the `CARGO_CONFIG_INPUT = "rs/.cargo/config.toml"` line:

```python
# SMA-605 — the two INDIRECT arms, beside CARGO_INVOCATION_RE's literal one.
#
# FORWARD COVER, NOT MEASURED COVERAGE — the same warning FFI_MARKERS carries for `maturin`.
# Arm 1 reports ZERO rows on the real corpus and always has; arm 2 reports exactly one, at
# ci/release-parity/ecosystems/release-plz.sh:152, and only once script_source_refs makes that
# file reachable. Do not read a green run as proof either arm works — the self-test fixtures are
# the proof.
#
# Arm 1 — a cargo-NAMED variable in command position. The NAME is the whole test (spec R1).
# Value resolution was measured and rejected: VAR_ASSIGN_RE captures `$(` as the value of
# CARGO_BIN="$( command -v cargo … )", so it cannot reach the real shape, and a value predicate
# would fire on the three variables in ci/actionlint/run.sh whose literal values mention cargo —
# the file SMA-599 L4 already names as one edit from a spurious row.
CARGO_VAR_CMD_RE = re.compile(
    r"""(?:^|[\s;&|(])["']?\$\{?([A-Za-z_][A-Za-z0-9_]*)\}?["']?\s+"""
    r"(?:\+\S+\s+)?(" + "|".join(LOCK_RESOLVING_VERBS) + r")\b"
)
CARGO_VAR_NAME = "cargo"

# Arm 2 — the `CARGO=` environment prefix, the shape this repo actually uses
# (ci/release-parity/ecosystems/release-plz.sh:152). The name is EXACTLY `CARGO`: CARGO_HOME,
# CARGO_TERM_COLOR and CARGO_NET_OFFLINE configure cargo without redirecting it, and line 152
# carries both `CARGO=` and `CARGO_NET_OFFLINE=` so a "name mentions cargo" predicate reports the
# wrong one (spec M6).
#
# No verb requirement: the tool's verbs belong to the tool, not to cargo. The trailing word is a
# LOOKAHEAD, never consumed — that is what makes `export CARGO=/p`, an assignment with nothing to
# run, report nothing, and it keeps a second env prefix's leading separator intact.
CARGO_ENV_PREFIX_RE = re.compile(
    r"""(?:^|[\s;&|(])CARGO=(?:"[^"]*"|'[^']*'|[^\s;&|]*)(?=\s+\S)"""
)

CargoMatch = collections.namedtuple("CargoMatch", "start end verb kind")
```

- [ ] **Step 4: Add `cargo_matches`**

Insert directly after `_tail_end`, before `def _classify_shell_line`:

```python
def cargo_matches(text):
    """Every cargo invocation in `text` — literal and indirect — sorted by start offset.

    `verb` is carried because the `--no-deps` carve-out must key on it. CARGO_METADATA_RE needs a
    literal lowercase `cargo`, so it never fires for `"$CARGO_BIN" metadata` and that call would
    report despite not resolving, contradicting SMA-599 D4.

    Arm 1's NAME FILTER RUNS HERE, before the list is merged. A rejected match left in the list
    would still act as a `stop` boundary in `_classify_shell_line` and truncate the PRECEDING
    invocation's tail — a silent false negative reached by the back door.

    DE-DUPLICATED ON END OFFSET, literal first. MEASURED: `$cargo build` already matches
    CARGO_INVOCATION_RE (`\\bcargo` needs only a word boundary, and `$` supplies one), so without
    this the lowercase form reports twice for one invocation and any waiver for it is permanently
    AMBIGUOUS — the SMA-599 L15 trap, reached by a new route.
    """
    found = [
        CargoMatch(m.start(), m.end(), m.group(0).split()[-1], "literal")
        for m in CARGO_INVOCATION_RE.finditer(text)
    ]
    found += [
        CargoMatch(m.start(), m.end(), m.group(2), "var")
        for m in CARGO_VAR_CMD_RE.finditer(text)
        if CARGO_VAR_NAME in m.group(1).lower()
    ]
    found += [
        CargoMatch(m.start(), m.end(), None, "env")
        for m in CARGO_ENV_PREFIX_RE.finditer(text)
    ]
    rank = {"literal": 0, "var": 1, "env": 2}
    out, claimed = [], set()
    for match in sorted(found, key=lambda c: (c.start, rank[c.kind])):
        if match.end in claimed:
            continue
        claimed.add(match.end)
        out.append(match)
    return sorted(out, key=lambda c: c.start)
```

- [ ] **Step 5: Run the self-test to verify it passes**

Run: `python3 ci/affected-graph/cargo_moon_parity.py --self-test`
Expected: `OK   [parity] all ten assertions fire on synthetic violations`, exit 0

- [ ] **Step 6: Prove the fixtures bite (mutation)**

Run each of these, confirming the self-test **fails** each time, then undo:

```bash
# M-1a: delete arm 1 from the merged list
python3 - <<'EOF'
import pathlib; p=pathlib.Path("ci/affected-graph/cargo_moon_parity.py"); t=p.read_text()
p.write_text(t.replace('        for m in CARGO_VAR_CMD_RE.finditer(text)\n        if CARGO_VAR_NAME in m.group(1).lower()\n', '        for m in []\n'))
EOF
python3 ci/affected-graph/cargo_moon_parity.py --self-test; echo "expect rc=1, got $?"
git checkout -- ci/affected-graph/cargo_moon_parity.py
```

Repeat for: arm 2 (`for m in CARGO_ENV_PREFIX_RE.finditer(text)` -> `for m in []`); the name
filter (`if CARGO_VAR_NAME in m.group(1).lower()` -> `if True`); the lookahead
(`(?=\s+\S)` -> `\s+\S`); and the de-duplication (`if match.end in claimed:` -> `if False:`).
Each must red. Record the five results in a scratch file for Task 8.

**IMPORTANT:** restore with `git checkout --` ONLY while there is nothing uncommitted you need.
Commit first (Step 7), then mutate, then restore.

- [ ] **Step 7: Commit**

```bash
git add ci/affected-graph/cargo_moon_parity.py
git commit -m "ci(repo): add the merged cargo match list with two indirect arms (SMA-605)"
```

---

### Task 2: Wire the arms into the script scan

**Files:**
- Modify: `ci/affected-graph/cargo_moon_parity.py` — `ScriptCargoLine` (`:315`), `_classify_shell_line` (`:784-834`), `check_cargo_locked_scripts` (`:1018-1086`)
- Test: the same file's `self_test()`

**Interfaces:**
- Consumes: `cargo_matches` from Task 1
- Produces: `ScriptCargoLine(lineno, raw, segment, resolves, locked, kind)` — one field longer than before; `_reports(line) -> bool`

- [ ] **Step 1: Write the failing test**

Add to `self_test()`, in the SMA-599 script-arm block (search for `# SMA-599 — A8's script arm.`), inside its `tempfile.TemporaryDirectory()`:

```python
        # SMA-605 — the indirect arms, through the real script scanner.
        indirect = Path(tmp) / "indirect.sh"
        indirect.write_text(
            '#!/usr/bin/env bash\n'
            '"$CARGO_BIN" build\n'                       # 2: reports
            '"$CARGO_BIN" build --locked\n'              # 3: clean
            '"$CARGO_BIN" metadata --no-deps\n'          # 4: clean, the D4 carve-out
            'CARGO=/p release-plz update\n'              # 5: reports, wrapper rule
            'CARGO=/p release-plz update --locked\n'     # 6: reports ANYWAY
            'out="$(cd x && CARGO=/p tool update)"\n'    # 7: reports, inside $( )
            'export CARGO=/p\n'                          # 8: clean, nothing to run
        )
        got = {(l.lineno, l.kind) for l in script_cargo_lines(indirect) if _reports(l)}
        want = {(2, "var"), (5, "env"), (6, "env"), (7, "env")}
        if got != want:
            failures.append(
                f"A8's script arm reports {sorted(got)} on the indirect fixture, expected "
                f"{sorted(want)}"
            )
        # The `--no-deps` carve-out must key on the VERB, not on CARGO_METADATA_RE: the latter
        # needs a literal lowercase `cargo` and never fires for `"$CARGO_BIN" metadata`.
        if any(l.lineno == 4 and l.resolves for l in script_cargo_lines(indirect)):
            failures.append(
                "A8 treats `\"$CARGO_BIN\" metadata --no-deps` as resolving — the carve-out is "
                "still keyed on CARGO_METADATA_RE rather than on the matched verb (SMA-599 D4)"
            )
```

- [ ] **Step 2: Run it to verify it fails**

Run: `python3 ci/affected-graph/cargo_moon_parity.py --self-test`
Expected: FAIL with `NameError: name '_reports' is not defined`

- [ ] **Step 3: Widen `ScriptCargoLine` and `_classify_shell_line`**

Replace the `ScriptCargoLine` definition:

```python
ScriptCargoLine = collections.namedtuple(
    "ScriptCargoLine", "lineno raw segment resolves locked kind"
)
```

In `_classify_shell_line`, replace the body of the `for segment` loop:

```python
    for segment in COMMAND_SPLIT_RE.split(logical):
        found = cargo_matches(segment)
        for idx, match in enumerate(found):
            stop = found[idx + 1].start if idx + 1 < len(found) else len(segment)
            tail = segment[match.end : _tail_end(segment, match.end, stop)]
            # MEASURED (SMA-599 §2.1): `cargo metadata --no-deps` does not resolve and never
            # rewrites the lock, so --locked on it is INERT. Keyed on the matched VERB, not on
            # CARGO_METADATA_RE over the match text: that regex needs a literal lowercase
            # `cargo`, so an arm-1 `"$CARGO_BIN" metadata --no-deps` would report despite not
            # resolving (SMA-605 review).
            resolves = not (match.verb == "metadata" and re.search(r"--no-deps\b", tail))
            rows.append(
                ScriptCargoLine(
                    lineno, logical, segment, resolves, LOCKED_FLAG in tail, match.kind
                )
            )
```

- [ ] **Step 4: Add `_reports` and use it in BOTH loops**

Insert directly before `def check_cargo_locked_scripts`:

```python
def _reports(line):
    """Whether this row is a violation. ONE definition, used by BOTH loops below.

    An `env` row is NEVER satisfied by a flag: `CARGO=<path> <tool>` reaches cargo through the
    tool, and the tool takes no `--locked`. Reading `line.locked` for it lets the TOOL's own flag
    clear the row, because that flag lands inside arm 2's tail.

    Both loops must share this. With emission kind-aware and the waiver-health loop kind-blind,
    an `env` row whose tool carries `--locked` is emitted, the reviewer adds a waiver, emission
    clears, and the health loop then finds no hits and reports the honest waiver as STALE. The
    row is permanently red with no escape but rewriting the shell line (SMA-605 review).
    """
    return line.kind == "env" or (line.resolves and not line.locked)
```

In `check_cargo_locked_scripts`, replace `if not line.resolves or line.locked:` with:

```python
                if not _reports(line):
```

and replace the waiver-health `hits` comprehension:

```python
        hits = [
            line
            for line in seen.get(rel, [])
            if line.segment.strip() == text and _reports(line)
        ]
```

Add an `env`-specific message. Replace the `if reason is None:` arm's `rows.append(...)` with:

```python
                if reason is None:
                    if line.kind == "env":
                        rows.append(
                            f"{rel}:{line.lineno} sets CARGO= to redirect cargo through another "
                            f"tool, which cannot take {LOCKED_FLAG} — a {LOCKED_FLAG} on the "
                            f"tool does NOT cover it, so this line needs an "
                            f"ALLOW_UNLOCKED_CARGO_SCRIPT entry: {text[:100]}"
                        )
                    else:
                        rows.append(
                            f"{rel}:{line.lineno} reaches cargo without {LOCKED_FLAG} — it will "
                            f"re-resolve and REWRITE an inconsistent Cargo.lock in place: "
                            f"{text[:100]}"
                        )
```

- [ ] **Step 5: Run the self-test to verify it passes**

Run: `python3 ci/affected-graph/cargo_moon_parity.py --self-test`
Expected: exit 0

- [ ] **Step 6: Prove the fixtures bite (mutation)**

```bash
git add -A && git commit -m "wip" --no-verify
# M-2a: make _reports kind-blind
sed -i '' 's/return line.kind == "env" or (line.resolves and not line.locked)/return line.resolves and not line.locked/' ci/affected-graph/cargo_moon_parity.py
python3 ci/affected-graph/cargo_moon_parity.py --self-test; echo "expect rc=1, got $?"
git checkout -- ci/affected-graph/cargo_moon_parity.py
# M-2b: revert the --no-deps carve-out to CARGO_METADATA_RE
python3 - <<'EOF'
import pathlib; p=pathlib.Path("ci/affected-graph/cargo_moon_parity.py"); t=p.read_text()
p.write_text(t.replace('match.verb == "metadata"', 'CARGO_METADATA_RE.search(segment[match.start:match.end])'))
EOF
python3 ci/affected-graph/cargo_moon_parity.py --self-test; echo "expect rc=1, got $?"
git checkout -- ci/affected-graph/cargo_moon_parity.py
git reset --soft HEAD~1
```

Record both results for Task 8.

- [ ] **Step 7: Commit**

```bash
git add ci/affected-graph/cargo_moon_parity.py
git commit -m "ci(repo): report an indirect cargo call from a gate script (SMA-605)"
```

---

### Task 3: Wire the arms into the blob scan

**Files:**
- Modify: `ci/affected-graph/cargo_moon_parity.py` — `derive_cargo_tasks` (`:901-928`), `check_cargo_locked` (`:983-985`)
- Test: the same file's `self_test()`

**Interfaces:**
- Consumes: `cargo_matches`, `CARGO_ENV_PREFIX_RE` from Task 1
- Produces: no new names. `derive_cargo_tasks` now returns `"wrapper"` for an arm-2 blob and `"literal"` for an arm-1 blob.

**Why this task is separate:** every fixture in Tasks 1 and 2 reaches the code through a script. Without blob fixtures, deleting the blob wiring survives the whole suite at rc 0 — a silent false negative of exactly the class SMA-599 burned three review rounds on.

- [ ] **Step 1: Write the failing test**

Add to `self_test()`, immediately after the `both = {...}` wrapper-precedence fixture (search for `wrapper > literal precedence`):

```python
    # SMA-605 — the BLOB arm. Deliberately blob-only fixtures with NO script reference: every
    # other indirect fixture reaches the code through a script, so without these, deleting the
    # blob wiring survives the whole suite at rc 0 (SMA-605 review).
    def _blob(cmd):
        return {
            "p": {
                "source_dir": "rs/crates/libs/p", "deps": {}, "tasks": {},
                "task_inputs": {"t": []}, "task_input_globs": {"t": []},
                "invocations": {"t": cmd},
            },
        }

    for cmd, want_kind in (
        ('"$CARGO_BIN" build', "literal"),
        ('CARGO=/p release-plz update', "wrapper"),
    ):
        got = derive_cargo_tasks(_blob(cmd), Path("."))
        if got != {"p:t": want_kind}:
            failures.append(
                f"derive_cargo_tasks did not classify the blob {cmd!r} as {want_kind} — it "
                f"returned {got}; the blob arm is not wired"
            )

    # ...and A8 must actually REPORT them, not merely derive them.
    if not any(
        "p:t" in r for r in check_cargo_locked(_blob('"$CARGO_BIN" build'), allow={}, floor=())
    ):
        failures.append("A8's blob arm did not report an unlocked indirect cargo invocation")
    # Arm 2 in a blob is a WRAPPER: a --locked in the blob must NOT clear it.
    if not any(
        "p:t" in r
        for r in check_cargo_locked(
            _blob("CARGO=/p release-plz update --locked"), allow={}, floor=()
        )
    ):
        failures.append(
            "A8's blob arm let a --locked clear a CARGO= redirection — the flag reaches the "
            "tool, never the cargo behind it"
        )
```

- [ ] **Step 2: Run it to verify it fails**

Run: `python3 ci/affected-graph/cargo_moon_parity.py --self-test`
Expected: FAIL, with `derive_cargo_tasks did not classify the blob '"$CARGO_BIN" build' as literal — it returned {}`

- [ ] **Step 3: Wire `derive_cargo_tasks`**

Replace its three-branch body:

```python
            target, blob = f"{pid}:{name}", invocations[name]
            if blob is None:
                continue
            # Arm 2 folds into the WRAPPER kind rather than becoming a fourth one: `CARGO=<path>
            # <tool>` reaches cargo through a tool that takes no --locked, which is exactly the
            # FFI wrapper contract, and reusing it means the existing ALLOW_UNLOCKED_CARGO
            # semantics apply unchanged (SMA-605 §5.4).
            if any(marker in blob for marker in FFI_MARKERS) or CARGO_ENV_PREFIX_RE.search(blob):
                kinds[target] = "wrapper"
            elif any(c.kind in ("literal", "var") for c in cargo_matches(blob)):
                kinds[target] = "literal"
            elif any(
                script_cargo_lines(p) for p in task_script_closure(projects, root, target)
            ):
                kinds[target] = "script"
```

**NOTE:** `task_script_closure` does not exist until Task 6. Until then, keep
`task_script_refs(projects, root, target)` on that line and change it in Task 6.

- [ ] **Step 4: Wire `check_cargo_locked`**

Replace its two-line match test:

```python
            is_wrapper = bool(
                any(marker in blob for marker in FFI_MARKERS)
                or CARGO_ENV_PREFIX_RE.search(blob)
            )
            if not (is_wrapper or any(c.kind in ("literal", "var") for c in cargo_matches(blob))):
                continue
```

- [ ] **Step 5: Run the self-test to verify it passes**

Run: `python3 ci/affected-graph/cargo_moon_parity.py --self-test`
Expected: exit 0

- [ ] **Step 6: Prove the fixtures bite (mutation)**

```bash
git add -A && git commit -m "wip" --no-verify
# M-3a: drop arm 2 from derive_cargo_tasks' blob branch
sed -i '' 's/ or CARGO_ENV_PREFIX_RE.search(blob):/:/' ci/affected-graph/cargo_moon_parity.py
python3 ci/affected-graph/cargo_moon_parity.py --self-test; echo "expect rc=1, got $?"
git checkout -- ci/affected-graph/cargo_moon_parity.py
# M-3b: drop the arms from check_cargo_locked's blob test
python3 - <<'EOF'
import pathlib; p=pathlib.Path("ci/affected-graph/cargo_moon_parity.py"); t=p.read_text()
p.write_text(t.replace(
  'if not (is_wrapper or any(c.kind in ("literal", "var") for c in cargo_matches(blob))):',
  'if not (is_wrapper or CARGO_INVOCATION_RE.search(blob)):'))
EOF
python3 ci/affected-graph/cargo_moon_parity.py --self-test; echo "expect rc=1, got $?"
git checkout -- ci/affected-graph/cargo_moon_parity.py
git reset --soft HEAD~1
```

- [ ] **Step 7: Commit**

```bash
git add ci/affected-graph/cargo_moon_parity.py
git commit -m "ci(repo): report an indirect cargo call from a task blob (SMA-605)"
```

---

### Task 4: The Dockerfile arm and its floor

**Files:**
- Modify: `ci/affected-graph/cargo_moon_parity.py` — `check_dockerfile_locked` (`:1227-1260`)
- Test: the same file's `self_test()`, the A8-g block (`:2188`)

**Interfaces:**
- Consumes: `cargo_matches` from Task 1
- Produces: no new names

- [ ] **Step 1: Write the failing test**

Add inside the existing A8-g `tempfile.TemporaryDirectory()` block, after the missing-file check but before `rs / "Dockerfile"` is unlinked — i.e. re-create the file first:

```python
        # SMA-605 — the Dockerfile takes the merged list too, but its FLOOR counts LITERAL
        # matches only. Counting merged matches would let an ENV line satisfy `seen > 0` after
        # the real `RUN cargo build --locked` was deleted, which is floor-satisfied-by-a-
        # non-invocation — the vacuity mode this file guards against everywhere else.
        (rs / "Dockerfile").write_text("ENV CARGO=/usr/local/bin/cargo CARGO_HOME=/cargo\n")
        rows = check_dockerfile_locked(Path(tmp))
        if not any("A8 examines rs/Dockerfile" in r for r in rows):
            failures.append(
                "A8's Dockerfile floor was satisfied by a CARGO= line — a redirection is not an "
                "invocation, and the floor now covers nothing"
            )
        if not any("CARGO=" in r for r in rows):
            failures.append("A8 did not report a CARGO= redirection in rs/Dockerfile")
        (rs / "Dockerfile").write_text('RUN "$CARGO_BIN" build --release\n')
        if not any("without --locked" in r for r in check_dockerfile_locked(Path(tmp))):
            failures.append("A8 did not fire on an indirect unlocked Dockerfile cargo build")
```

- [ ] **Step 2: Run it to verify it fails**

Run: `python3 ci/affected-graph/cargo_moon_parity.py --self-test`
Expected: FAIL with `A8 did not report a CARGO= redirection in rs/Dockerfile`

- [ ] **Step 3: Rewrite the loop body**

Replace the body of `check_dockerfile_locked`'s `for lineno, line` loop:

```python
    for lineno, line in enumerate(path.read_text().splitlines(), 1):
        stripped = line.split("#", 1)[0]
        found = cargo_matches(stripped)
        if not found:
            continue
        # The FLOOR counts LITERAL matches only (SMA-605 review). `seen` exists to catch a
        # Dockerfile that stopped compiling; an `ENV CARGO=…` line redirects cargo but invokes
        # nothing, so letting it increment `seen` would keep the floor quiet after the real
        # `RUN cargo build --locked` was deleted.
        seen += sum(1 for c in found if c.kind == "literal")
        if any(c.kind == "env" for c in found):
            rows.append(
                f"rs/Dockerfile:{lineno} sets CARGO= to redirect cargo through another tool, "
                f"which cannot take {LOCKED_FLAG}: {stripped.strip()}"
            )
        elif LOCKED_FLAG not in stripped:
            rows.append(
                f"rs/Dockerfile:{lineno} reaches cargo without {LOCKED_FLAG}: {stripped.strip()}"
            )
```

- [ ] **Step 4: Run the self-test to verify it passes**

Run: `python3 ci/affected-graph/cargo_moon_parity.py --self-test`
Expected: exit 0

- [ ] **Step 5: Prove the fixtures bite (mutation)**

```bash
git add -A && git commit -m "wip" --no-verify
# M-4a: count merged matches in the floor
sed -i '' 's/seen += sum(1 for c in found if c.kind == "literal")/seen += len(found)/' ci/affected-graph/cargo_moon_parity.py
python3 ci/affected-graph/cargo_moon_parity.py --self-test; echo "expect rc=1, got $?"
git checkout -- ci/affected-graph/cargo_moon_parity.py
# M-4b: revert to the literal-only scan
sed -i '' 's/found = cargo_matches(stripped)/found = list(CARGO_INVOCATION_RE.finditer(stripped))/' ci/affected-graph/cargo_moon_parity.py
python3 ci/affected-graph/cargo_moon_parity.py --self-test; echo "expect rc=1 or a TypeError, got $?"
git checkout -- ci/affected-graph/cargo_moon_parity.py
git reset --soft HEAD~1
```

- [ ] **Step 6: Commit**

```bash
git add ci/affected-graph/cargo_moon_parity.py
git commit -m "ci(repo): see an indirect cargo call in rs/Dockerfile (SMA-605)"
```

---

### Task 5: A10's own arms

**Files:**
- Modify: `ci/affected-graph/cargo_moon_parity.py` — constants near `CONFIG_SENSITIVE_RE` (`:219`), `check_cargo_config_inputs` (`:1183`)
- Test: the same file's `self_test()`, the A10 block (`:3063`)

**Interfaces:**
- Consumes: `CONFIG_SENSITIVE_VERBS`, `CARGO_VAR_NAME`, `CARGO_ENV_PREFIX_RE`
- Produces: `CARGO_VAR_CMD_SENSITIVE_RE`, `_var_sensitive(text) -> bool`

- [ ] **Step 1: Write the failing test**

Add to `self_test()`'s A10 block, after the existing `a10_fixture` assertions:

```python
    # SMA-605 — A10's arms are its OWN, built from CONFIG_SENSITIVE_VERBS. Reusing arm 1
    # (LOCK_RESOLVING_VERBS) pulls `tree`, `deny` and `update` into A10's scope and NOTHING
    # reds — the accident SMA-599 D9 spent a round removing.
    def _a10(cmd):
        return {
            "q": {
                "source_dir": "rs/crates/libs/q", "deps": {}, "tasks": {},
                "task_inputs": {"t": []}, "task_input_globs": {"t": []},
                "invocations": {"t": cmd},
            },
        }

    if not any(
        "q:t" in r and CARGO_CONFIG_INPUT in r
        for r in check_cargo_config_inputs(_a10('"$CARGO_BIN" build'), Path("."), floor=())
    ):
        failures.append("A10 did not demand .cargo/config.toml for an indirect compiling call")
    if check_cargo_config_inputs(_a10('"$CARGO_BIN" tree'), Path("."), floor=()):
        failures.append(
            "A10 examined `\"$CARGO_BIN\" tree` — its arm is built from LOCK_RESOLVING_VERBS "
            "rather than CONFIG_SENSITIVE_VERBS (SMA-599 D9)"
        )
    if not any(
        "q:t" in r
        for r in check_cargo_config_inputs(_a10("CARGO=/p release-plz update"), Path("."), floor=())
    ):
        failures.append(
            "A10 did not treat a CARGO= redirection as sensitive — the tool's inner cargo may "
            "compile, and A10 cannot know that it does not"
        )
```

- [ ] **Step 2: Run it to verify it fails**

Run: `python3 ci/affected-graph/cargo_moon_parity.py --self-test`
Expected: FAIL with `A10 did not demand .cargo/config.toml for an indirect compiling call`

- [ ] **Step 3: Add the constant and helper**

Insert directly after `CONFIG_SENSITIVE_RE`'s definition:

```python
# SMA-605 — A10's OWN arm 1. Built from CONFIG_SENSITIVE_VERBS, never from
# LOCK_RESOLVING_VERBS: A10 asks "can rs/.cargo/config.toml change this command's OUTPUT", and
# reusing A8's list would pull `"$CARGO_BIN" tree` / `deny` / `update` into A10's scope with
# nothing to red it — the coupling SMA-599 D9 removed for the literal arm, re-created for the
# indirect one.
CARGO_VAR_CMD_SENSITIVE_RE = re.compile(
    r"""(?:^|[\s;&|(])["']?\$\{?([A-Za-z_][A-Za-z0-9_]*)\}?["']?\s+"""
    r"(?:\+\S+\s+)?(?:" + "|".join(CONFIG_SENSITIVE_VERBS) + r")\b"
)


def _var_sensitive(text):
    """True when a cargo-NAMED variable runs a COMPILING subcommand in `text`."""
    return any(
        CARGO_VAR_NAME in m.group(1).lower()
        for m in CARGO_VAR_CMD_SENSITIVE_RE.finditer(text)
    )
```

- [ ] **Step 4: Widen the sensitivity test**

In `check_cargo_config_inputs`, replace the `sensitive = ...` line:

```python
        # Arm 2 makes a task sensitive UNCONDITIONALLY: `CARGO=<path> <tool>` reaches cargo
        # through a tool whose subcommand A10 cannot read, so it cannot rule out a compile.
        sensitive = (
            kind == "wrapper"
            or bool(CONFIG_SENSITIVE_RE.search(text))
            or _var_sensitive(text)
            or bool(CARGO_ENV_PREFIX_RE.search(text))
        )
```

- [ ] **Step 5: Run the self-test to verify it passes**

Run: `python3 ci/affected-graph/cargo_moon_parity.py --self-test`
Expected: exit 0

- [ ] **Step 6: Prove the fixtures bite (mutation)**

```bash
git add -A && git commit -m "wip" --no-verify
# M-5a: reuse arm 1 for A10 instead of the sensitive variant
sed -i '' 's/"|".join(CONFIG_SENSITIVE_VERBS) + r")\\b"$/"|".join(LOCK_RESOLVING_VERBS) + r")\\b"/' ci/affected-graph/cargo_moon_parity.py
python3 ci/affected-graph/cargo_moon_parity.py --self-test; echo "expect rc=1, got $?"
git checkout -- ci/affected-graph/cargo_moon_parity.py
# M-5b: drop arm 2's unconditional sensitivity
sed -i '' 's/ or bool(CARGO_ENV_PREFIX_RE.search(text))//' ci/affected-graph/cargo_moon_parity.py
python3 ci/affected-graph/cargo_moon_parity.py --self-test; echo "expect rc=1, got $?"
git checkout -- ci/affected-graph/cargo_moon_parity.py
git reset --soft HEAD~1
```

**WARNING:** the M-5a `sed` edits whichever line matches first. Verify with `git diff` that it
hit `CARGO_VAR_CMD_SENSITIVE_RE` and not `CONFIG_SENSITIVE_RE`; if it hit the wrong one, restore
and mutate by hand.

- [ ] **Step 7: Commit**

```bash
git add ci/affected-graph/cargo_moon_parity.py
git commit -m "ci(repo): give A10 its own indirect arms (SMA-605)"
```

---

### Task 6: The execution-only source resolver

**Files:**
- Modify: `ci/affected-graph/cargo_moon_parity.py` — add after `task_script_refs` (`:900`); rewire `derive_cargo_tasks`, `check_cargo_locked_scripts` (`:1032`), `check_cargo_config_inputs` (`:1177`); add rows to `collect_findings`' `a8` bucket
- Test: the same file's `self_test()`

**Interfaces:**
- Consumes: `SCRIPT_REF_RE`, `VAR_ASSIGN_RE`, `MoonOutputError` (all existing)
- Produces: `SOURCE_STMT_RE`, `HERE_IDIOM_ASSIGN_RE`, `REQUIRED_SOURCED_SCRIPTS`, `script_source_refs(path, root) -> list[Path]`, `task_script_closure(projects, root, target) -> list[Path]`, `check_sourced_scripts(root, required=...) -> list[str]`

- [ ] **Step 1: Write the failing test**

Add to `self_test()`, after the Task-2 indirect-script fixture:

```python
    # SMA-605 — the source resolver. EXECUTION ONLY: a bare `ci/**/*.sh` mention in script text
    # is NOT followed, measured at six edges across the real corpus, every one a comment or a
    # pin-array string constant, one new waiver and ZERO true positives (spec M10).
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        (root / "ci" / "eco").mkdir(parents=True)
        (root / "ci" / "run.sh").write_text(
            'HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"\n'
            'ECO="a"\n'
            'ECO="$2"\n'
            '# see ci/other/run.sh for the idiom\n'
            'source "$HERE/eco/$ECO.sh"\n'
        )
        (root / "ci" / "eco" / "a.sh").write_text('cargo build --locked\n')
        (root / "ci" / "eco" / "b.sh").write_text('cargo build --locked\n')
        got = sorted(p.name for p in script_source_refs(root / "ci" / "run.sh", root))
        if got != ["a.sh", "b.sh"]:
            failures.append(
                f"script_source_refs resolved {got}, expected ['a.sh', 'b.sh'] — a variable "
                f"reassigned more than once must GLOB, not resolve to its first value"
            )
        # The bare `ci/other/run.sh` mention in the comment must NOT appear: the first
        # assertion above already pins the resolved set to exactly the two eco modules, and
        # `ci/other/run.sh` does not exist, so following it would RAISE rather than pass.
        #
        # A cycle must terminate. Relative targets, deliberately: SOURCE_STMT_RE captures
        # `([^"'\s;&|]+)`, so a `source "$(dirname "${BASH_SOURCE[0]}")/b.sh"` target is cut at
        # the first space and never resolves. The one real statement in the tree has no space.
        (root / "ci" / "eco" / "a.sh").write_text('source ./b.sh\n')
        (root / "ci" / "eco" / "b.sh").write_text('source ./a.sh\n')
        proj = {"repo": {"source_dir": ".", "deps": {}, "tasks": {},
                         "task_inputs": {"t": []}, "task_input_globs": {"t": []},
                         "invocations": {"t": "bash ci/run.sh"}}}
        try:
            closure = task_script_closure(proj, root, "repo:t")
        except RecursionError:
            failures.append("task_script_closure recursed on a source cycle")
            closure = []
        if len(closure) != len({p.resolve() for p in closure}):
            failures.append("task_script_closure returned a duplicate on a source cycle")
        # A source that resolves to nothing is infrastructure, never a silent skip.
        (root / "ci" / "run.sh").write_text('source "$HERE/nope/absent.sh"\n')
        try:
            script_source_refs(root / "ci" / "run.sh", root)
            failures.append("script_source_refs did not raise on a source resolving to nothing")
        except MoonOutputError:
            pass

    # The resolver's FLOOR: a rename must red, not silently empty the closure.
    if check_sourced_scripts(Path("."), required={"ci/release-parity/run.sh": ("ci/nope.sh",)}) == []:
        failures.append("check_sourced_scripts did not fire on a wrong expected set")
```

- [ ] **Step 2: Run it to verify it fails**

Run: `python3 ci/affected-graph/cargo_moon_parity.py --self-test`
Expected: FAIL with `NameError: name 'script_source_refs' is not defined`

- [ ] **Step 3: Add the resolver**

Insert directly after `task_script_refs`:

```python
# SMA-605 — SMA-599's L2, closed one level. EXECUTION ONLY.
#
# A `source` / `.` statement is followed. A bare `ci/**/*.sh` mention in a script's text is NOT:
# running SCRIPT_REF_RE over a followed script's own text was MEASURED at six new edges across
# the real corpus, and every one of them is a comment or a pin-array string constant
# (publish-metadata's `:9-10`, `:1686`, `:1726`; actionlint's `:2016`, `:2041`, `:2046` and its
# T_CARGO_LOCK_STEP_REQUIRED array at `:2152-2154`). That buys six scripts into A8's scope on the
# strength of prose, one new waiver, and ZERO true positives — and it still does not reach
# ci/release-parity/ecosystems/release-plz.sh, because SCRIPT_REF_RE cannot match a
# `# shellcheck source=…` directive (the path is preceded by `=`, not by `[\s;&|(]`).
SOURCE_STMT_RE = re.compile(r"""(?m)^\s*(?:source|\.)\s+["']?([^"'\s;&|]+)["']?""")
# A variable assigned from the `dirname "${BASH_SOURCE[0]}"` idiom IS the script's own directory.
# VAR_ASSIGN_RE cannot capture it — it stops at the space inside `$(cd …` — so this reads the
# raw assignment line instead.
HERE_IDIOM_ASSIGN_RE = re.compile(r"(?m)^\s*([A-Za-z_][A-Za-z0-9_]*)=\"?\$\(cd .*BASH_SOURCE.*")

# The resolver's floor. Without it a rename empties the closure in silence — the SMA-553 class.
REQUIRED_SOURCED_SCRIPTS = {
    "ci/release-parity/run.sh": (
        "ci/release-parity/ecosystems/python-semantic-release.sh",
        "ci/release-parity/ecosystems/release-plz.sh",
        "ci/release-parity/ecosystems/semantic-release.sh",
    ),
}


def script_source_refs(path, root):
    """The scripts `path` EXECUTES through a `source` / `.` statement.

    A variable assigned EXACTLY ONCE resolves to its value; one assigned more than once — or
    never — becomes a glob. MEASURED on the one source statement in the tree
    (`ci/release-parity/run.sh:21`): `HERE` is assigned once, at `:7`, so it resolves to the
    script's directory, while `ECOSYSTEM` is assigned at `:8` AND `:13` (from `$2`), so it globs
    and yields all three ecosystem modules rather than only the default `release-plz`. The other
    two are real code a Moon task executes, and a resolver that returned only the default would
    leave them unscanned.

    Over-approximation in the same direction as the path-insensitive scan it feeds (SMA-599 L1):
    all three modules land in every `release-parity*` task's closure, including the two a given
    invocation never sources.

    Raises MoonOutputError when a source resolves to nothing — a rename would otherwise shrink
    the closure in silence, which is the failure this whole task exists to prevent.
    """
    path = Path(path)
    text = path.read_text()
    counts = collections.Counter(name for name, _ in VAR_ASSIGN_RE.findall(text))
    env = {name: value for name, value in VAR_ASSIGN_RE.findall(text) if counts[name] == 1}
    for name in HERE_IDIOM_ASSIGN_RE.findall(text):
        if counts[name] <= 1:
            env[name] = str(path.parent)
    # Longest name first, for the reason _cwd_inside_rs records: `str.replace` on the bare $NAME
    # form has no word boundary, so a short name that prefixes a longer one eats it.
    ordered = sorted(env.items(), key=lambda kv: (-len(kv[0]), kv[0]))
    out = []
    for raw in SOURCE_STMT_RE.findall(text):
        target = raw
        for name, value in ordered:
            target = target.replace(f"${{{name}}}", value).replace(f"${name}", value)
        # The `$(dirname "${BASH_SOURCE[0]}")` form used inline rather than through a variable.
        target = re.sub(r"\$\(dirname [^)]*\)", str(path.parent), target)
        # Anything still unresolved becomes a glob.
        target = re.sub(r"\$\{?[A-Za-z_][A-Za-z0-9_]*\}?", "*", target)
        candidate = Path(target)
        if not candidate.is_absolute():
            candidate = path.parent / candidate
        hits = sorted(Path(candidate.anchor or "/").glob(str(candidate).lstrip("/")))
        # Files only, and never outside the repo: a `source /etc/profile` is not a gate script,
        # and scanning one would put text nobody reviews into A8's corpus.
        root_resolved = Path(root).resolve()
        hits = [
            h for h in hits
            if h.is_file() and root_resolved in h.resolve().parents
        ]
        if not hits:
            raise MoonOutputError(
                f"{path}: `source {raw}` resolves to no readable file — the script closure would "
                f"silently shrink. If the module moved, update the source statement."
            )
        out.extend(hits)
    return out


def task_script_closure(projects, root, target):
    """`task_script_refs` plus the transitive `source` closure, cycle-guarded.

    Breadth-first with a visited set keyed on the RESOLVED path, so a cycle terminates and a
    module reached twice appears once. Depth is unbounded by design; the corpus is depth 2.
    """
    queue, seen, out = list(task_script_refs(projects, root, target)), set(), []
    while queue:
        path = queue.pop(0)
        key = path.resolve()
        if key in seen:
            continue
        seen.add(key)
        out.append(path)
        queue.extend(script_source_refs(path, root))
    return out


def check_sourced_scripts(root, required=None):
    """REQUIRED_SOURCED_SCRIPTS, asserted. Rows join A8's bucket in collect_findings."""
    required = REQUIRED_SOURCED_SCRIPTS if required is None else required
    rows = []
    for rel, expected in sorted(required.items()):
        path = Path(root) / rel
        if not path.is_file():
            rows.append(
                f"{rel} is absent — the source resolver's floor cannot be evaluated"
            )
            continue
        got = tuple(sorted(p.resolve().relative_to(Path(root).resolve()).as_posix()
                           for p in script_source_refs(path, root)))
        if got != tuple(sorted(expected)):
            rows.append(
                f"{rel} sources {got}, expected {tuple(sorted(expected))} — the source resolver "
                f"has degraded and the script closure would silently shrink"
            )
    return rows
```

- [ ] **Step 4: Rewire the three callers**

In `derive_cargo_tasks`, `check_cargo_locked_scripts` and `check_cargo_config_inputs`, replace
every `task_script_refs(projects, root, target)` with
`task_script_closure(projects, root, target)`.

In `collect_findings`, append the resolver's rows to the existing `a8` list — find where `a8` is
computed and add `+ check_sourced_scripts(root)` to it, so `EXPECTED_FINDING_KEYS` is unchanged.

- [ ] **Step 5: Run the self-test to verify it passes**

Run: `python3 ci/affected-graph/cargo_moon_parity.py --self-test`
Expected: exit 0

- [ ] **Step 6: Prove the fixtures bite (mutation)**

```bash
git add -A && git commit -m "wip" --no-verify
# M-6a: drop the source closure
sed -i '' 's/queue.extend(script_source_refs(path, root))/pass/' ci/affected-graph/cargo_moon_parity.py
python3 ci/affected-graph/cargo_moon_parity.py --self-test; echo "expect rc=1, got $?"
git checkout -- ci/affected-graph/cargo_moon_parity.py
# M-6b: resolve a multiply-assigned variable instead of globbing
sed -i '' 's/if counts\[name\] == 1}/}/' ci/affected-graph/cargo_moon_parity.py
python3 ci/affected-graph/cargo_moon_parity.py --self-test; echo "expect rc=1, got $?"
git checkout -- ci/affected-graph/cargo_moon_parity.py
git reset --soft HEAD~1
```

- [ ] **Step 7: Commit**

```bash
git add ci/affected-graph/cargo_moon_parity.py
git commit -m "ci(repo): follow a gate script's source statements one level (SMA-605)"
```

---

### Task 7: The corpus differential and the one real waiver

**Files:**
- Modify: `ci/affected-graph/cargo_moon_parity.py` — `ALLOW_UNLOCKED_CARGO_SCRIPT` (`:349`)
- Modify: `docs/superpowers/specs/2026-08-30-sma-605-cargo-invocation-through-variable-design.md` — §6.3

**Interfaces:**
- Consumes: everything from Tasks 1-6
- Produces: one new `ALLOW_UNLOCKED_CARGO_SCRIPT` entry

- [ ] **Step 1: Run the real gate and capture the after-state**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ci/affected-graph && python3 cargo_moon_parity.py; echo "rc=$?"
```

Expected: FAIL, with an a8 row naming `ci/release-parity/ecosystems/release-plz.sh:152` and a
`CARGO=` redirection. That row is the change's single live true positive.

- [ ] **Step 2: Add the waiver**

Read `ci/release-parity/ecosystems/release-plz.sh:145-158` first, and copy the reported segment
text VERBATIM from the gate's own output — the key is `(script path, stripped segment text)`, and
a hand-typed approximation matches nothing and reads as stale.

```python
    ("ci/release-parity/ecosystems/release-plz.sh", "<PASTE THE EXACT SEGMENT FROM THE GATE OUTPUT>"): (
        "MEASURED true positive, and correct as written (SMA-605). release-plz shells out to "
        "`cargo metadata`, and this line hands it an explicit CWD-independent cargo through the "
        "CARGO env var (SMA-596 D2.1). No --locked can reach that inner cargo: the flag would go "
        "to release-plz, which does not forward it. The call is SAFE because it runs against a "
        "disposable fixture outside the repo — `ecosystem::run_update` cd's into a mktemp dir — "
        "so it cannot rewrite rs/Cargo.lock. If that fixture ever moves inside the workspace, "
        "delete this waiver."
    ),
```

- [ ] **Step 3: Re-run and record the four measures**

```bash
python3 cargo_moon_parity.py; echo "rc=$?"
```

Expected: PASS, rc 0.

Then re-run the before/after probe and write the four numbers into the spec's §6.3 table:
`derive_cargo_tasks` (was 63), A8 blob `matched` (was 60), A10 `in_scope` (was 58), findings
(was 0 rows). **Explain every movement in §6.3 prose. Never re-baseline a number without saying
why it moved.**

- [ ] **Step 4: Run the whole gate as CI does**

```bash
bash ci/affected-graph/run.sh --negative-control && bash ci/affected-graph/run.sh
```

Expected: both exit 0, and no expected-set movement is reported. If a case reports movement,
STOP and explain it in §6.3 before re-baselining anything.

- [ ] **Step 5: Commit**

```bash
git add ci/affected-graph/cargo_moon_parity.py docs/superpowers/specs/2026-08-30-sma-605-cargo-invocation-through-variable-design.md
git commit -m "ci(repo): waive the measured CARGO= redirection in release-plz.sh (SMA-605)"
```

---

### Task 8: Record the mutation battery

**Files:**
- Modify: `docs/superpowers/specs/2026-08-30-sma-605-cargo-invocation-through-variable-design.md` — §6.2

**Interfaces:**
- Consumes: the mutation results recorded in Tasks 1-6

- [ ] **Step 1: Re-run every mutation against the FINAL code**

The results from Tasks 1-6 were measured against intermediate states. Re-run all fourteen
mutations from the spec's §6.2 table against the finished file, one at a time, restoring between
each with `git checkout -- ci/affected-graph/cargo_moon_parity.py`.

**IMPORTANT:** commit everything first. `git checkout --` reverts the whole file, so an
uncommitted fix is destroyed and the next mutation then runs against original code and prints a
meaningless failure that looks real.

- [ ] **Step 2: Write the results into §6.2**

Add a `Result` column to the §6.2 table with the measured exit code and the fixture that fired.
A mutation that SURVIVES is a plan failure: the fixture asserts nothing and must be fixed before
the branch merges. Do not record a survivor as acceptable.

- [ ] **Step 3: Commit**

```bash
git add docs/superpowers/specs/2026-08-30-sma-605-cargo-invocation-through-variable-design.md
git commit -m "docs(repo): record the SMA-605 mutation battery results"
```

---

### Task 9: Documentation

**Files:**
- Modify: `docs/superpowers/specs/2026-08-29-sma-599-cargo-invocation-invariants-design.md` — L2 (`:617`), L10 (`:701`), L11 (`:709`)
- Modify: `ci/affected-graph/cargo_moon_parity.py:3249,3256` — the forward-guard message
- Modify: `ci/affected-graph/cargo_moon_parity.py:147-175` — the `LOCK_RESOLVING_VERBS` comment block
- Modify: `ci/affected-graph/README.md` — the A8 and A10 bullets
- Modify: `CLAUDE.md` — the A8/A10 paragraph

- [ ] **Step 1: Rewrite SMA-599's L10**

Replace the L10 paragraph with:

```markdown
**L10 — cargo invoked through a variable: CLOSED for two shapes, open for the rest (SMA-605).**
`"$CARGO_BIN" build` and `CARGO=<path> <tool>` are now reported by both A8 and A10, through two
arms merged into `cargo_matches`. Four shapes stay uncovered and are recorded as R4-R6 and R8 of
`docs/superpowers/specs/2026-08-30-sma-605-cargo-invocation-through-variable-design.md`: a
variable holding a cargo path but NOT named for it, `"${CARGO_BIN:-cargo}"`, a backtick command
boundary, a `PATH=` prefix, and a Moon task `env:` block (which the resolved blob excludes
entirely). Arm 1 reports zero rows on the real corpus and is labelled forward cover in the code.
```

- [ ] **Step 2: Update SMA-599's L2**

Append to the L2 paragraph:

```markdown
SMA-605 closed the `source` half: a `source` / `.` statement is now followed transitively, with a
cycle guard, and `ci/release-parity/ecosystems/*.sh` is therefore scanned. Bare `ci/**/*.sh`
MENTIONS are still not followed, and deliberately so — measured at six new edges, every one a
comment or a pin-array string constant, one new waiver and zero true positives. Non-`.sh`
entrypoints (`ops/nats/check-subjects.sh`, the three `.py` gates) are still unfollowed.
```

- [ ] **Step 3: Update SMA-599's L11**

Append one sentence:

```markdown
SMA-605 closed L10 for the variable shape without widening `CARGO_INVOCATION_RE` itself, so L11's
subcommand shape (`cargo llvm-cov`, `insta`, `udeps`, `bloat`, `tarpaulin`) is untouched and
stays open.
```

- [ ] **Step 4: Fix the forward-guard message**

At `cargo_moon_parity.py:3249` and `:3256`, the self-test's message says SMA-605 is pending.
Re-word it so it names SMA-599 L11 alone — SMA-605 has landed and no longer describes a gap.

- [ ] **Step 5: Update the constant comment block and the READMEs**

At `cargo_moon_parity.py:147-175`, add a sentence to the `CARGO_INVOCATION_RE` comment pointing
at `cargo_matches` as the merged entry point, so a reader does not conclude the literal regex is
the whole story.

In `ci/affected-graph/README.md`, update the A8 and A10 bullets with the two arms and the source
resolver.

In `CLAUDE.md`, the sentence "A10 shares `CARGO_INVOCATION_RE`, built from `LOCK_RESOLVING_VERBS`,
with A8's derivation" is now partly false. Correct it, and add one sentence recording that a
gate script's `source` statements are followed one level while bare mentions are not.

**Do NOT add a second copy of the `ci-targets:begin` / `ci-targets:end` markers to CLAUDE.md, or
even mention them inside backticks — a second occurrence makes the count 2 and reds
`repo:affected-smoke` (SMA-541).**

- [ ] **Step 6: Run the full gate**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
bash ci/affected-graph/run.sh --negative-control && bash ci/affected-graph/run.sh
python3 ci/affected-graph/cargo_moon_parity.py --self-test
```

Expected: all three exit 0.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "docs(repo): record what SMA-605 closed in L2, L10 and L11"
```

---

## Final verification

Before opening the PR, run the full graph the way CI does. Per-project tasks do NOT run the
repo-level gates:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :http-extractor-envelope :input-liveness :promtool :observability-drift \
  :nats-permissions :release-parity :release-parity-py :release-parity-ts \
  :publish-metadata :version-lockstep :workflow-credentials --base origin/main \
  --include-relations
```

**Note:** `proto` emits NDJSON inside an agent session, so all three `release-parity*` gates abort
INCONCLUSIVE at rc 2 rather than red. Run them with
`env -u AI_AGENT -u CLAUDECODE -u CLAUDE_CODE_ENTRYPOINT`, or an inconclusive abort reads as a
pass.
