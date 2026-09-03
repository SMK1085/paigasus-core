# SMA-599 Cargo-Invocation Invariant Gates Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add A10 — an assertion that every Moon task whose cargo subcommand is influenced by `rs/.cargo/config.toml` declares that file as an input — and widen A8 so both assertions also see cargo reached through a gate's shell script.

**Architecture:** Everything lands in the existing `ci/affected-graph/cargo_moon_parity.py`, which already runs inside `repo:affected-smoke`. Two new shared helpers (a shell-line classifier and a task derivation returning match kinds) are consumed by both A8 and the new A10. A10 uses its own verb predicate — subcommands that compile or link — deliberately separate from A8's lock-resolving verbs. No new `repo:*` task, so none of the five new-gate registry obligations apply.

**Tech Stack:** Python 3 standard library only (`re`, `json`, `subprocess`, `tomllib`, `pathlib`). The gate is `toolchain: 'system'` and must not shell out to cargo. Bash for the Moon task and `ci/actionlint/run.sh`. YAML for `moon.yml`.

**Spec:** `docs/superpowers/specs/2026-08-29-sma-599-cargo-invocation-invariants-design.md`

## Global Constraints

- Every source file opens with `# SPDX-License-Identifier: Apache-2.0` (already present in every file touched).
- `cargo_moon_parity.py` must NOT shell out to cargo — `repo:affected-smoke` is `toolchain: 'system'`.
- Never parse `moon.yml`; read moon's RESOLVED graph via `moon query projects` (no `--json` flag — it errors on moon 2.5.3).
- An allowlist entry is a RECORDED DECISION: an empty reason is itself a violation row.
- Every derived set needs an anti-vacuity floor; a derived set that empties prints PASS while asserting nothing.
- "Infrastructure is broken" raises `MoonOutputError` (rc 2); "the graph regressed" returns a violation row (rc 1). Never conflate them.
- Adding a check means adding its key to `EXPECTED_FINDING_KEYS` AND its tuple to `collect_findings`, in the same order.
- Run all commands with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` first.
- Commits: conventional, workspace-scoped (`ci(repo):`, `docs(repo):`), subject lowercase and ≤100 chars, one contiguous footer, no `#NNN` in the body. Do NOT use `--no-verify`.
- Restore a mutation by reverting the marked edit, NEVER by `git checkout --` on the file — that discards the uncommitted fix under test.

---

### Task 1: The shared shell-line classifier — THE CONSERVATIVE RULE

**Files:**
- Modify: `ci/affected-graph/cargo_moon_parity.py` (constants after `LOCKED_FLAG`; `_blank_operator_spans`, `_escaped`, `_odd_quotes`, `_line_regions`, `_join`, `_classify_shell_line`, `script_cargo_lines`; self-test rows inside `self_test()` before its `return`)

**Interfaces:**
- Consumes: `CARGO_INVOCATION_RE`, `LOCKED_FLAG`, `MoonOutputError` (all existing).
- Produces: `ScriptCargoLine` namedtuple with fields `lineno raw segment resolves locked`; `script_cargo_lines(path) -> list[ScriptCargoLine]`. Task 2 and Task 3 both consume it.
- There is **no `unclassifiable` field.** The conservative rule reports such a line as an ordinary row, so the field would be dead.

**COURSE CHANGE, recorded.** This task was first delivered as a four-layer shell lexer — heredoc
tracking, comment cutting, cross-line quote parity, paren-depth substitution extraction,
arithmetic stripping, escape awareness — 441 lines, of which the quote-span tracker alone was 196
with six states feeding three consumers. Three review rounds each found a real **silent false
negative**, and rounds 2-4 were each an interaction between one layer and the layer added before
it. The last one, unfixed at the time of the replacement:

```bash
bash -c \
  "cargo build"
```

reported 0 rows and no error while real bash runs cargo unlocked, because the exec-vs-plain
decision read the RAW physical line while continuation joining happened later on the LOGICAL
line. The lexer was deleted rather than patched a fourth time. Against the four historical
defects the conservative rule scores 4/4 where the lexer scored, at each round, whatever the
previous round had not yet broken — and it is ~25 lines of decision logic against 441. Its
failure mode is a **loud false positive**, not a silent pass.

- [x] **Step 1: The rule**

Report every cargo invocation whose own command segment lacks `--locked`. Exclude exactly three
regions, because in each the shell provably never executes the text:

1. **Heredoc bodies** (a heredoc open at EOF still raises `MoonOutputError`).
2. **`#` comment tails**, cut PER PHYSICAL LINE before continuations are joined — a `#` comment
   ends at the newline even when the previous line ends in a backslash.
3. **Bracketed operator spans** — `$(( ... ))`, a bare `(( ... ))` and anything in `[ ... ]`,
   where a `<<` is a shift and a `#` is a base marker (`_blank_operator_spans`).

All three are ONE decision, taken together in `_line_regions` from ONE within-line quote mask.
Keeping them apart shipped a phantom heredoc (round 5) and then left the same defect standing for
`<<`'s siblings (round 6): a `<<EOF` inside a string, a comment, an ambiguous-parity line, an
arithmetic span or a subscript opened a real heredoc and SWALLOWED the lines after it, silently.
Six sub-decisions, each with a fixture and a mutation:

- a `#` must start a WORD and be UNESCAPED (`${#arr[@]}` is bash's length operator, and
  `echo a\ #b` keeps `#b` inside the word; the word-start guard had shipped with no fixture);
- a heredoc opener is accepted only where the `<<` survives the mask, so `cat <<'EOF' > "$out"`,
  plain `cat <<EOF` and `cat <<-EOF` are all positive controls;
- `<<<` is a here-string and opens nothing;
- operator spans are blanked in the MASK, never the code, and only when they CLOSE — the old
  form blanked from an unclosed `$((` to EOL in the code and deleted a real invocation;
- a heredoc body starts after the whole LOGICAL line, so the opener is held across a
  backslash continuation;
- ambiguous parity refuses BOTH decisions — counting `"` and `'` for the heredoc, `"` only for
  the comment cut, because an apostrophe in prose is English and counting it reds
  `ci/publish-metadata/run.sh:772` (measured, both directions pinned).

Refusing can only add a false positive; opening wrongly swallows.

Plus the `cargo metadata --no-deps` carve-out (MEASURED: `--no-deps` never resolves, so `--locked`
on it is inert).

**Quoted strings are NOT stripped.** That layer created every silent drop. A cargo verb inside a
string simply reports; the three live instances in `ci/` are waived in Task 3.

`--locked` is required **in the segment, after the verb**. The segment scope keeps
`cargo build && cargo metadata --locked` reporting `cargo build`; the after-the-verb scope keeps a
`--locked` that is string content sitting BEFORE the verb (`X="abc` / `--locked" cargo build`, one
bash statement across two physical lines) from covering a genuinely unlocked call.

The `"`-parity guard on the comment cut is A/B-measured against the whole `ci/**/*.sh` corpus: it
fires on 343 physical lines and changes not one row. It does NOT close every drop — see L9 in the
spec: `X='a` / `b # c' cargo build` still truncates at the `#`, because counting single quotes
there costs a false positive on every prose comment with an apostrophe. Recorded, not fixed.

- [x] **Step 2: Fail-first**

Against the pre-change classifier, `bash -c \` + newline + `"cargo build"` returned `[]` — no row,
no error — while `--self-test` returned 0. The silent pass was the point.

- [x] **Step 3: Implement, and prove the fixtures bite**

Fifteen fixtures. Must NOT report: a heredoc body, a full-line comment, `cargo build --locked`,
`cargo metadata --no-deps`. Must report (each verified against real bash to actually run cargo):

```
VERSION="$(cargo metadata --format-version 1 | jq -r .version)"
if ! OUT="$(cargo build 2>&1)"; then
X="abc\n--locked" cargo build          (locked=False)
X="$(\n  cargo build\n)"
X="$(cargo build) more\nstuff"
bash -c \\\n  "cargo build"            (the defect that retired the previous design)
MASK=$((1 << BITS))\ncargo build\nBITS
echo "a # b" && cargo build
X="a\nb # c" cargo build               (the odd-quote guard)
# note \\\ncargo build                 (the per-physical-line comment cut)
n=${#arr[@]} && cargo build            (the `#` word-start guard)
echo "a <<EOF b"                       (phantom heredoc: 5 string shapes + comment + ambiguous)
X='a\nb <<EOF c'                       (single-quote parity on the heredoc decision)
cat <<<EOF                             (here-string, not an opener)
echo '$(( x' && cargo build            (mask-not-code blanking, unclosed span)
(( MASK = 1 << BITS )) / a[1 << N]=2   (the two operator spans beyond `$(( ))`)
cat <<EOF \\\n| cargo build             (the opener held across a continuation)
echo a\\ #b && cargo build              (escaped space is not a word boundary)
echo "start\ncargo build\nend"         (THE ACCEPTED FALSE POSITIVE, pinned deliberately)
```

Five positive controls stop "never open a heredoc" passing the phantom rows: `cat <<'EOF' >
"$out"`, plain `cat <<EOF` and `cat <<-EOF` must all still open one whose body is skipped, an
apostrophe in a trailing comment must not stop one, and `ls a[bc <<EOF` — an UNCLOSED bracketed
span before a real opener — must still open.

A nineteen-mutation battery killed every mutation, one per decision. Two mutations were retired
rather than kept, both because they are unkillable BY CONSTRUCTION and a fixture for them would
be theatre: "scan the opener on the raw line" is equivalent (the mask-position check already
rejects any offset past the cut), and the word-character requirement that told a subscript from a
`[ -f x ]` test changed no row across the corpus's 871 non-subscript `[` occurrences — so that
guard was DELETED rather than shipped untested, the same call as the contraction regex a round
earlier.

- [x] **Step 4: Verify against the real corpus**

```bash
python3 - <<'PY'
import sys; sys.path.insert(0, "ci/affected-graph")
from pathlib import Path
from cargo_moon_parity import script_cargo_lines
for s in sorted(Path("ci").rglob("*.sh")):
    rows = script_cargo_lines(s)
    bad = [r for r in rows if r.resolves and not r.locked]
    print(f"{s}: {len(rows)} cargo line(s), {len(bad)} would report")
    for r in bad:
        print(f"    {r.lineno}: {r.segment.strip()[:90]}")
PY
```

Measured at the time this task was written — five would-report rows. **The shipped set differs; see the note below the table.**

| file:line | segment | what it is |
| --- | --- | --- |
| `ci/version-lockstep/run.sh:583` | `cargo update -w --offline >/dev/null 2>` | real call |
| `ci/version-lockstep/run.sh:583` | `cargo update -w >/dev/null )` | real call |
| `ci/version-lockstep/run.sh:583` | `die_infra "cargo update -w failed (site 16)"` | prose in an error string |
| `ci/publish-metadata/run.sh:1663` | ``die_infra "FATAL: \`cargo metadata\` failed in $RS_DIR …"`` | prose in an error string |
| `ci/cargo-lock-integrity/run.sh:60` | `1) echo "::error::rs/Cargo.lock does not satisfy … run 'cargo metadata' in rs/ …"` | prose — but NOT waived, see below |

All three `ci/version-lockstep/run.sh` rows carry line **583**, not 583/584/585: `:583-585` is ONE
logical line joined by backslash continuations, and a row is reported against the FIRST physical
line. `ci/publish-metadata/run.sh:1663-1664` is the same shape.

**Corrected after implementation.** `ci/cargo-lock-integrity/run.sh` is an unconditional `ci.yml` step that no Moon task invokes, so A8's script arm never follows it and its row needs no waiver — an entry would be stale on arrival. Conversely the final fix wave's per-invocation scoping surfaced a sixth row at `ci/actionlint/run.sh:3715` (the parameter expansion that builds check 8f's own negative control), which IS waived. The shipped `ALLOW_UNLOCKED_CARGO_SCRIPT` therefore holds five entries: three for `version-lockstep`, one for `publish-metadata`, one for `actionlint`.


- [x] **Step 5: Commit**

```bash
git add ci/affected-graph/cargo_moon_parity.py
git commit -m "ci(repo): replace the shell cargo lexer with the conservative rule (SMA-599)"
```
---

### Task 2: The shared derivation with match kinds

**Files:**
- Modify: `ci/affected-graph/cargo_moon_parity.py` (add `SCRIPT_REF_RE` beside the Task 1 constants; add `derive_cargo_tasks` after `script_cargo_lines`; extend `REQUIRED_LOCKED_TASKS` at :199-206; add self-test rows)

**Interfaces:**
- Consumes: `script_cargo_lines` (Task 1), `FFI_MARKERS`, `CARGO_INVOCATION_RE`, `MoonOutputError`.
- Produces: `derive_cargo_tasks(projects, root) -> dict[str, str]` mapping `"<pid>:<task>"` to kind `"wrapper" | "literal" | "script"`; `task_script_refs(projects, root, target) -> list[Path]`. Tasks 3 and 4 consume both.

- [ ] **Step 1: Write the failing self-test rows**

Add inside `self_test()` after Task 1's block:

```python
    # SMA-599 — derive_cargo_tasks must keep the three kinds DISTINGUISHABLE at the
    # derivation boundary. A8 measured that a wrapper match and a literal match cannot be
    # treated alike (paigasus-kernel-ts:build carries a --locked belonging to a DIFFERENT
    # command), so a flat set would silently reintroduce the vacuous form.
    with tempfile.TemporaryDirectory() as tmp:
        ci_dir = Path(tmp) / "ci" / "probe"
        ci_dir.mkdir(parents=True)
        (ci_dir / "run.sh").write_text("cd rs\ncargo build\n")
        kinds_fixture = {
            "p": {
                "source_dir": ".", "deps": {}, "tasks": {},
                "task_inputs": {}, "task_input_globs": {},
                "invocations": {
                    "lit": "cargo build --locked",
                    "wrap": "pnpm exec napi build --platform",
                    "scr": "bash ci/probe/run.sh --negative-control",
                    "none": "echo hello",
                },
            },
        }
        got = derive_cargo_tasks(kinds_fixture, Path(tmp))
        want = {"p:lit": "literal", "p:wrap": "wrapper", "p:scr": "script"}
        if got != want:
            failures.append(f"derive_cargo_tasks returned {got}, expected {want}")

        # An unresolvable script path must ABORT, not silently shrink the derived set.
        missing = json.loads(json.dumps(kinds_fixture))
        missing["p"]["invocations"]["scr"] = "bash ci/gone/run.sh"
        try:
            derive_cargo_tasks(missing, Path(tmp))
        except MoonOutputError:
            pass
        else:
            failures.append("derive_cargo_tasks did not raise on a script path that does not exist")

    if not REQUIRED_LOCKED_TASKS:
        failures.append("REQUIRED_LOCKED_TASKS is empty — A8's floor would assert nothing")

    # PRECEDENCE, and it needs its own fixture because none of the four tasks above carries
    # two signals at once. Measured: with only single-signal fixtures, swapping the wrapper
    # and literal branches still passes --self-test at rc 0. A8 records as MEASURED that a
    # wrapper and a literal must not be treated alike (paigasus-kernel-ts:build runs an
    # unlocked `napi build` beside a `wasm-pack build ... -- --locked`), so a silent collapse
    # to `literal` would green a task that still repairs the lock.
    both = {
        "p": {
            "source_dir": ".", "deps": {}, "tasks": {},
            "task_inputs": {}, "task_input_globs": {},
            "invocations": {"mixed": "pnpm exec napi build --platform && cargo build --locked"},
        },
    }
    if derive_cargo_tasks(both, Path(".")) != {"p:mixed": "wrapper"}:
        failures.append(
            "derive_cargo_tasks did not apply wrapper > literal precedence to a task matching "
            "BOTH kinds — the stricter rule must win"
        )
```

- [ ] **Step 2: Run to verify it fails**

```bash
python3 ci/affected-graph/cargo_moon_parity.py --self-test
```

Expected: FAIL with `NameError: name 'derive_cargo_tasks' is not defined`.

- [ ] **Step 3: Add `SCRIPT_REF_RE`**

Beside the Task 1 constants:

```python
# The `bash`/`sh` prefix is OPTIONAL. Measured: five of the eight invoked gate scripts are
# called BARE (`ci/release-parity/run.sh --ecosystem semantic-release`), so a prefix-requiring
# extractor sees 3 of 8 scripts — which is exactly the error the first draft of the SMA-599
# spec made, and it invalidated its own "zero false positives" measurement.
SCRIPT_REF_RE = re.compile(r"(?:^|[\s;&|(])(?:bash\s+|sh\s+)?(ci/[\w./-]+\.sh)\b")
```

**Do NOT extend `REQUIRED_LOCKED_TASKS` in this task.** That extension moved to Task 3, and the
reason is a measured regression: `REQUIRED_LOCKED_TASKS` is read by `check_cargo_locked`, the
already-shipped A8 from SMA-601, which does not gain script-following until Task 3. Adding
`repo:publish-metadata` and `repo:version-lockstep` to the floor here makes A8 fail immediately —
`moon run repo:affected-smoke --force` exits 1 with two `A8 examines 60 task(s) and … is not
among them` rows — because the floor names tasks A8's blob-only derivation cannot yet reach.
`--self-test` does NOT catch it: it exercises synthetic fixtures and never runs the real corpus
through `collect_findings`. Landing the floor with its consumer keeps every commit on the branch
green and bisectable.

- [ ] **Step 4: Implement the derivation**

After `script_cargo_lines`:

```python
def task_script_refs(projects, root, target):
    """The `ci/**/*.sh` files a task's resolved invocation runs, as existing Paths.

    Raises MoonOutputError when a referenced script does not resolve to a readable file: a
    rename would otherwise silently empty the derived set.
    """
    pid, _, name = target.partition(":")
    blob = (projects[pid].get("invocations") or {}).get(name)
    if not blob:
        return []
    paths = []
    for rel in sorted(set(SCRIPT_REF_RE.findall(blob))):
        path = Path(root) / rel
        if not path.is_file():
            raise MoonOutputError(
                f"{target} invokes {rel}, which does not resolve to a readable file — the "
                f"derived set would silently shrink. If the script moved, update the task."
            )
        paths.append(path)
    return paths


def derive_cargo_tasks(projects, root):
    """{target: kind} for every task reaching cargo. kind is wrapper | literal | script.

    NOT a flat set, deliberately. check_cargo_locked records, as measured, that a wrapper
    match and a literal match must not be treated alike: `paigasus-kernel-ts:build` runs an
    unlocked `napi build` beside a `wasm-pack build ... -- --locked`, so a blob-level flag
    test greens a task that still repairs the lock. Collapsing the kinds here would
    reintroduce that measured-vacuous form one level down.

    Precedence is wrapper > literal > script, matching check_cargo_locked's existing rule
    that a task matching both kinds is governed by the stricter (wrapper) one.
    """
    kinds = {}
    for pid in sorted(projects):
        invocations = projects[pid].get("invocations") or {}
        for name in sorted(invocations):
            target, blob = f"{pid}:{name}", invocations[name]
            if blob is None:
                continue
            if any(marker in blob for marker in FFI_MARKERS):
                kinds[target] = "wrapper"
            elif CARGO_INVOCATION_RE.search(blob):
                kinds[target] = "literal"
            elif any(script_cargo_lines(p) for p in task_script_refs(projects, root, target)):
                kinds[target] = "script"
    return kinds
```

- [ ] **Step 5: Run to verify it passes**

```bash
python3 ci/affected-graph/cargo_moon_parity.py --self-test
```

Expected: PASS.

- [ ] **Step 6: Verify the real derived set**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 - <<'PY'
import sys, collections; sys.path.insert(0, "ci/affected-graph")
from pathlib import Path
from cargo_moon_parity import moon_projects, derive_cargo_tasks
k = derive_cargo_tasks(moon_projects(), Path("."))
print(collections.Counter(k.values()))
print([t for t, v in sorted(k.items()) if v == "script"])
PY
```

Expected: `Counter({'literal': 57, 'wrapper': 3, 'script': 3})` and the script list is exactly `['repo:actionlint', 'repo:publish-metadata', 'repo:version-lockstep']`.

`repo:actionlint` belongs in that list, and an earlier draft of this step said `2` because it was measured against the four-layer classifier that stripped quoted strings. The shipped conservative rule does not strip them, so `ci/actionlint/run.sh`'s five pinned cargo strings surface as rows — all carrying `--locked`, so 0 would-report. It needs no waiver anywhere: A8 clears it on the flag, and A10 excludes it on both tests (that script holds zero config-sensitive verbs and zero cd-into-`rs` tokens). If the counts differ from 57/3/3, reconcile before continuing — spec §1.3 and §2.2 depend on them.

- [ ] **Step 7: Commit**

```bash
git add ci/affected-graph/cargo_moon_parity.py
git commit -m "ci(repo): derive cargo-reaching tasks with their match kind (SMA-599)"
```

---

### Task 3: Widen A8 to gate scripts

**Files:**
- Modify: `ci/affected-graph/cargo_moon_parity.py` (add `ALLOW_UNLOCKED_CARGO_SCRIPT` after `ALLOW_UNLOCKED_CARGO` at :183-197; extend `check_cargo_locked` at :399-471; extend the `a8` title in `collect_findings` at :1778; add self-test rows)

**Interfaces:**
- Consumes: `script_cargo_lines`, `task_script_refs` (Tasks 1–2).
- Produces: `check_cargo_locked_scripts(projects, root, allow=...) -> list[str]`, folded into the existing `a8` key.

- [ ] **Step 1: Write the failing self-test rows**

```python
    # SMA-599 — A8's script arm.
    with tempfile.TemporaryDirectory() as tmp:
        probe = Path(tmp) / "ci" / "probe"
        probe.mkdir(parents=True)
        (probe / "run.sh").write_text("cd rs\ncargo update -w\ncargo build --locked\n")
        fixture = {
            "repo": {
                "source_dir": ".", "deps": {}, "tasks": {},
                "task_inputs": {}, "task_input_globs": {},
                "invocations": {"g": "bash ci/probe/run.sh"},
            },
        }
        # Fires on the unlocked resolving line, and NOT on the locked one.
        rows = check_cargo_locked_scripts(fixture, Path(tmp), allow={})
        if len(rows) != 1 or "cargo update -w" not in rows[0]:
            failures.append(f"A8's script arm did not report exactly the unlocked line: {rows}")

        # A waiver keyed by unique TEXT clears it.
        allow_ok = {("ci/probe/run.sh", "cargo update -w"): "deliberate lock writer"}
        if check_cargo_locked_scripts(fixture, Path(tmp), allow=allow_ok):
            failures.append("A8's script arm ignored a valid text-keyed waiver")

        # An empty reason is itself a row.
        allow_bare = {("ci/probe/run.sh", "cargo update -w"): "  "}
        if not any("empty reason" in r for r in
                   check_cargo_locked_scripts(fixture, Path(tmp), allow=allow_bare)):
            failures.append("A8's script arm accepted a waiver with an empty reason")

        # A STALE waiver — text no longer present — must be reported, not ignored.
        allow_stale = dict(allow_ok)
        allow_stale[("ci/probe/run.sh", "cargo vendor")] = "gone"
        if not any("matches no line" in r for r in
                   check_cargo_locked_scripts(fixture, Path(tmp), allow=allow_stale)):
            failures.append("A8's script arm did not report a stale waiver entry")

        # A waiver whose text occurs TWICE is ambiguous and must be rejected.
        (probe / "run.sh").write_text("cargo update -w\ncargo update -w\n")
        if not any("occurs 2 times" in r for r in
                   check_cargo_locked_scripts(fixture, Path(tmp), allow=allow_ok)):
            failures.append("A8's script arm accepted a waiver text that is not unique")

    if not ALLOW_UNLOCKED_CARGO_SCRIPT:
        failures.append("ALLOW_UNLOCKED_CARGO_SCRIPT is empty — its stale-entry rule asserts nothing")

    # The waivers' PREMISE, asserted. Both entries rest on "the Moon task never passes
    # --write"; adding it would make them silently wrong.
    write_fixture = {"repo": {"invocations": {"version-lockstep": "bash x.sh --write"}}}
    if not check_version_lockstep_no_write(write_fixture):
        failures.append("check_version_lockstep_no_write missed a --write in the task blob")
    clean_fixture = {"repo": {"invocations": {"version-lockstep": "bash x.sh --self-test"}}}
    if check_version_lockstep_no_write(clean_fixture):
        failures.append("check_version_lockstep_no_write fired on a task that passes no --write")
    if check_version_lockstep_no_write({"repo": {"invocations": {}}}) == []:
        failures.append("check_version_lockstep_no_write treated an absent task as a pass")
```

- [ ] **Step 2: Run to verify it fails**

```bash
python3 ci/affected-graph/cargo_moon_parity.py --self-test
```

Expected: FAIL with `NameError: name 'check_cargo_locked_scripts' is not defined`.

- [ ] **Step 3: Add the allowlist AND extend the floor**

Extend `REQUIRED_LOCKED_TASKS` (`:199-206`), keeping its existing comment and adding:

```python
REQUIRED_LOCKED_TASKS = (
    "paigasus-kernel-rs:lint",
    "paigasus-iam-rs:test",
    "repo:deny",
    "repo:wasm-getrandom-free",
    # SMA-599 — these two reach cargo ONLY through a gate script, so they are the floor
    # members that fail if script-following silently stops working. Without them a broken
    # follower degrades the derived set in exactly the direction nothing else can see.
    # This landed in Task 3 rather than Task 2 DELIBERATELY: the floor is read by
    # check_cargo_locked, which cannot reach either task until this task's script arm
    # exists, so extending it earlier reds repo:affected-smoke on every commit in between.
    "repo:publish-metadata",
    "repo:version-lockstep",
)
```

Then, after `ALLOW_UNLOCKED_CARGO` (`:197`):

```python
# SMA-599 — waivers for cargo lines inside a gate's own script. Keyed by
# (script path, stripped segment text) and NOT by line number: a line-number key would red
# repo:affected-smoke on any unrelated insertion above the line, in a 620-line file that
# SMA-576 and SMA-579 both edited. The uniqueness assertion is what makes text safe — a text
# occurring twice is ambiguous and is reported rather than silently covering both.
#
# A stale entry (text no longer present) is a row, the stale-skip idiom
# ci/actionlint/run.sh:2376-2383 already uses.
ALLOW_UNLOCKED_CARGO_SCRIPT = {
    ("ci/version-lockstep/run.sh", "cargo update -w --offline >/dev/null 2>"): (
        "MEASURED unreachable from the Moon task (SMA-599 §2.4): repo:version-lockstep runs "
        "run.sh --self-test, --negative-control and bare, while this line is inside "
        "run_write(), reached only by `--write`. `--locked` would defeat the function, whose "
        "PURPOSE is to regenerate the lock after writing the six non-Cargo version sites. The "
        "scan is path-insensitive and cannot see this (L1), so the waiver stands in for it; "
        "check_version_lockstep_no_write below is what keeps the premise honest."
    ),
    ("ci/version-lockstep/run.sh", "cargo update -w >/dev/null )"): (
        "the un-offline fallback of the line above, same reason"
    ),
    ("ci/version-lockstep/run.sh", 'die_infra "cargo update -w failed (site 16)"'): (
        "PROSE, not an invocation: the failure message for the two lines above. The "
        "conservative rule does not strip quoted strings — that stripping is exactly what "
        "silently dropped real invocations before SMA-599's classifier was replaced — so a "
        "cargo verb inside a diagnostic surfaces as a row and is waived here instead. "
        "A false positive waived is the trade the design makes for never missing a real call."
    ),
    ("ci/publish-metadata/run.sh",
     'die_infra "FATAL: \\`cargo metadata\\` failed in $RS_DIR — nothing could be verified."'): (
        "PROSE, same class as the entry above: the diagnostic for the `cargo metadata "
        "--no-deps` call on the joined logical line starting at :1663. The real invocation "
        "itself does not report, because --no-deps never resolves (MEASURED, §2.1)."
    ),
}
```

- [ ] **Step 4: Implement the script arm**

After `check_cargo_locked`:

```python
def check_cargo_locked_scripts(projects, root, allow=None):
    """A8 rows for cargo invocations inside the gate scripts a Moon task runs.

    A blob-level derivation cannot see these: `repo:publish-metadata`'s invocation is
    `bash ci/publish-metadata/run.sh`, while its `cargo package --list --locked` and
    `cargo publish --dry-run --locked` live in the script. Before SMA-599 that whole class of
    gate was outside A8.

    Path-INSENSITIVE (SMA-599 L1): it reports a line the task's arguments may never reach.
    That is why the version-lockstep waiver exists, and why a reviewer must check reachability
    by hand rather than trusting a row.
    """
    allow = ALLOW_UNLOCKED_CARGO_SCRIPT if allow is None else allow
    rows, seen = [], {}
    for target in sorted(derive_cargo_tasks(projects, root)):
        for path in task_script_refs(projects, root, target):
            rel = path.relative_to(root).as_posix()
            if rel in seen:
                continue
            lines = script_cargo_lines(path)
            seen[rel] = lines
            for line in lines:
                text = line.segment.strip()
                # Task 1's conservative rule leaves nothing "unclassifiable": every row is an
                # ordinary row, and a benign string that mentions a cargo verb is waived here.
                # Use `line.locked`, not `LOCKED_FLAG in text` — the classifier already scoped
                # the flag to the segment tail AFTER the verb, and a bare substring test on the
                # segment throws that scoping away.
                if not line.resolves or line.locked:
                    continue
                reason = allow.get((rel, text))
                if reason is None:
                    rows.append(
                        f"{rel}:{line.lineno} reaches cargo without {LOCKED_FLAG} — it will "
                        f"re-resolve and REWRITE an inconsistent Cargo.lock in place: {text[:100]}"
                    )
                elif not reason.strip():
                    rows.append(
                        f"{rel}:{line.lineno} is in ALLOW_UNLOCKED_CARGO_SCRIPT with an empty "
                        f"reason — an exemption is allowed, a silent one is not"
                    )
    # Stale and ambiguous waiver entries. A waiver that matches nothing has silently stopped
    # asserting; one that matches twice covers a line nobody reviewed.
    for (rel, text), _reason in sorted(allow.items()):
        hits = [l for l in seen.get(rel, []) if l.segment.strip() == text]
        if not hits:
            rows.append(
                f"ALLOW_UNLOCKED_CARGO_SCRIPT entry ({rel}, {text[:60]!r}) matches no line — "
                f"the waiver is stale; delete it or update the text"
            )
        elif len(hits) > 1:
            rows.append(
                f"ALLOW_UNLOCKED_CARGO_SCRIPT entry ({rel}, {text[:60]!r}) occurs "
                f"{len(hits)} times — the key is ambiguous and would waive a line nobody reviewed"
            )
    return rows


def check_version_lockstep_no_write(projects):
    """The premise of ALLOW_UNLOCKED_CARGO_SCRIPT's two entries, asserted.

    Their reason is "unreachable, because the Moon task never passes --write". Adding
    `--write` to that task would make both waivers silently wrong, so assert it directly.
    """
    blob = (projects.get("repo", {}).get("invocations") or {}).get("version-lockstep")
    if blob is None:
        return [
            "repo:version-lockstep has no resolved invocation — ALLOW_UNLOCKED_CARGO_SCRIPT's "
            "reachability premise cannot be evaluated"
        ]
    if "--write" in blob:
        return [
            "repo:version-lockstep now passes --write, so its `cargo update -w` lines ARE "
            "reachable and their ALLOW_UNLOCKED_CARGO_SCRIPT waivers are wrong (SMA-599 §2.4)"
        ]
    return []
```

- [ ] **Step 5: Fold into `collect_findings`**

Change the `a8` assignment (`:1706`) to:

```python
    a8 = (
        check_cargo_locked(projects)
        + check_dockerfile_locked(root)
        + check_cargo_locked_scripts(projects, root)
        + check_version_lockstep_no_write(projects)
    )
```

Append to the `a8` title text (`:1778-1788`), before its closing quote:

```
    "    A `<script>:<line>` row is inside a gate's own run.sh: add `--locked` there, or an\n"
    "    ALLOW_UNLOCKED_CARGO_SCRIPT entry keyed by (script, exact segment text). The scan is\n"
    "    path-insensitive — check by hand whether the task's arguments actually reach that\n"
    "    line before waiving it (SMA-599 L1)."
```

- [ ] **Step 6: Restate the arity fixture's tmp-root contract**

`collect_findings` now reaches the filesystem through a second path: `check_cargo_locked_scripts`
→ `derive_cargo_tasks` → `task_script_refs`, which RAISES when a referenced script does not
resolve under `root`. The existing arity fixture (`:1032-1044`) calls
`collect_findings(ok, crates, Path(tmp))` with a tmp root holding only `rs/Dockerfile`.

Confirm the `ok` fixture carries no `invocations` referencing a `ci/**/*.sh` — it does not
today, so `task_script_refs` returns `[]` and nothing raises. Make that explicit rather than
accidental by adding a comment beside the fixture:

```python
        # SMA-599 — this tmp root holds ONLY rs/Dockerfile. That is safe because `ok` declares
        # no invocation referencing a ci/**/*.sh, so task_script_refs never looks for one. If a
        # future fixture adds such an invocation it MUST also create the script under this tmp
        # root, or task_script_refs raises MoonOutputError and the arity check stops being
        # about arity.
```

Then verify the contract holds by asserting the fixture directly:

```python
    if any("ci/" in (blob or "") for p in ok.values()
           for blob in (p.get("invocations") or {}).values()):
        failures.append(
            "the arity fixture now references a ci/ script but its tmp root does not create one"
        )
```

- [ ] **Step 7: Run to verify it passes**

```bash
python3 ci/affected-graph/cargo_moon_parity.py --self-test
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 ci/affected-graph/cargo_moon_parity.py
```

Expected: self-test PASS; the real run PASS (the two `cargo update -w` lines are waived).

- [ ] **Step 8: Prove A8's script arm can FAIL on the real tree**

```bash
python3 - <<'PY'
import pathlib
p = pathlib.Path("ci/publish-metadata/run.sh")
t = p.read_text()
p.write_text(t.replace("cargo package --list --locked", "cargo package --list", 1))
PY
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 ci/affected-graph/cargo_moon_parity.py; echo "rc=$?"
```

Expected: rc=1 and a row naming `ci/publish-metadata/run.sh:742`. Now restore by reverting the marked edit (NOT `git checkout --`, which would discard this task's uncommitted work):

```bash
python3 - <<'PY'
import pathlib
p = pathlib.Path("ci/publish-metadata/run.sh")
t = p.read_text()
p.write_text(t.replace("cargo package --list -p", "cargo package --list --locked -p", 1))
PY
git diff --stat ci/publish-metadata/run.sh
```

Expected: empty diff.

- [ ] **Step 9: Commit**

```bash
git add ci/affected-graph/cargo_moon_parity.py
git commit -m "ci(repo): widen A8 to cargo reached through a gate script (SMA-599)"
```

---

### Task 4: A10 — the `rs/.cargo/config.toml` assertion

**Files:**
- Modify: `ci/affected-graph/cargo_moon_parity.py` (add A10 constants after `SCRIPT_REF_RE`; add `check_cargo_config_inputs` after `check_cargo_locked_scripts`; extend `EXPECTED_FINDING_KEYS` at :1690; add the `a9` tuple to `collect_findings`; update the PASS string at :1673; add self-test rows)

**Interfaces:**
- Consumes: `derive_cargo_tasks`, `task_script_refs`, `script_cargo_lines`.
- Produces: `check_cargo_config_inputs(projects, root, allow=..., floor=...) -> list[str]`, reported under the new key `a9`.

- [ ] **Step 1: Write the failing self-test rows**

```python
    # SMA-599 — A10. Its verb predicate is its OWN, not LOCK_RESOLVING_VERBS: reusing A8's list
    # excluded the thirteen `fmt` tasks by COINCIDENCE and would hide any future
    # compiling-but-not-resolving subcommand (cargo llvm-cov, insta, udeps).
    a9_fixture = {
        "c-rs": {
            "source_dir": "rs/crates/libs/c", "deps": {}, "tasks": {},
            "task_inputs": {"build": ["rs/.cargo/config.toml"], "fmt": []},
            "task_input_globs": {"build": [], "fmt": []},
            "invocations": {"build": "cargo build --locked", "fmt": "cargo fmt --check"},
        },
        "repo": {
            "source_dir": ".", "deps": {}, "tasks": {},
            "task_inputs": {"deny": [], "tree": [], "mach": []},
            "task_input_globs": {"deny": [], "tree": [], "mach": []},
            "invocations": {
                "deny": "cargo deny --locked --manifest-path rs/Cargo.toml check",
                "tree": "cd rs && cargo tree --locked -p x",
                "mach": "cargo machete rs",
            },
        },
    }
    if check_cargo_config_inputs(a9_fixture, Path("."), allow={}, floor=("c-rs:build",)):
        failures.append("A10 reported violations on a clean fixture")

    # The core assertion: an in-scope task missing the input.
    broken = json.loads(json.dumps(a9_fixture))
    broken["c-rs"]["task_inputs"]["build"] = []
    if not any("c-rs:build" in r for r in
               check_cargo_config_inputs(broken, Path("."), allow={}, floor=("c-rs:build",))):
        failures.append("A10 did not fire on a cargo-from-rs task missing rs/.cargo/config.toml")

    # `fmt` is out of scope BY THE VERB, not by accident. It declares nothing and must pass.
    if any("c-rs:fmt" in r for r in
           check_cargo_config_inputs(a9_fixture, Path("."), allow={}, floor=("c-rs:build",))):
        failures.append("A10 demanded rs/.cargo/config.toml from `cargo fmt`, which cannot read it")

    # cwd is what excludes repo:deny — NOT a waiver. `--manifest-path rs/...` and a bare `rs`
    # argument must never confer scope, or D2's structural exclusion collapses.
    for task in ("repo:deny", "repo:mach"):
        if any(task in r for r in
               check_cargo_config_inputs(a9_fixture, Path("."), allow={}, floor=("c-rs:build",))):
            failures.append(f"A10 pulled {task} into scope from an `rs` path ARGUMENT, not a cd")

    # `cargo tree` runs from rs/ but resolves without compiling — out of scope by verb (AC 4).
    if any("repo:tree" in r for r in
           check_cargo_config_inputs(a9_fixture, Path("."), allow={}, floor=("c-rs:build",))):
        failures.append("A10 demanded the config file from `cargo tree`, which never compiles")

    # Floor: a member that leaves scope must fail, or the derivation could empty silently.
    if not any("FLOOR:" in r for r in
               check_cargo_config_inputs(a9_fixture, Path("."), allow={}, floor=("c-rs:nope",))):
        failures.append("A10's floor did not fire on a member outside the derived set")

    # Second vacuity mode, specific to default-deny: an allowlist that swallows a floor member.
    swallow = {"c-rs:build": "a reason"}
    if not any("FLOOR:" in r for r in
               check_cargo_config_inputs(broken, Path("."), allow=swallow, floor=("c-rs:build",))):
        failures.append("A10's floor let an allowlist entry cover a floor member")

    # An empty reason is itself a row.
    if not any("empty reason" in r for r in check_cargo_config_inputs(
            broken, Path("."), allow={"c-rs:build": " "}, floor=())):
        failures.append("A10 accepted an ALLOW_MISSING_CARGO_CONFIG entry with an empty reason")

    if not REQUIRED_CARGO_CONFIG_TASKS:
        failures.append("REQUIRED_CARGO_CONFIG_TASKS is empty — A10's floor would assert nothing")
    if not CONFIG_SENSITIVE_VERBS:
        failures.append("CONFIG_SENSITIVE_VERBS is empty — A10 would examine nothing")
```

- [ ] **Step 2: Run to verify it fails**

```bash
python3 ci/affected-graph/cargo_moon_parity.py --self-test
```

Expected: FAIL with `NameError: name 'check_cargo_config_inputs' is not defined`.

- [ ] **Step 3: Add the A10 constants**

After `SCRIPT_REF_RE`:

```python
# SMA-599 — A10's verb predicate, deliberately NOT LOCK_RESOLVING_VERBS.
#
# The two lists answer different questions. A8 asks "does this resolve the lock"; A10 asks
# "can rs/.cargo/config.toml change this command's OUTPUT". Reusing A8's list made A10 fail to
# implement its own rule: the thirteen `cargo fmt --check` tasks run with cwd inside `rs/` and
# fell out of scope only because `fmt` happens to be absent from a lock-oriented list — an
# accidental coupling, not a stated exclusion. It would also have left any future
# compiling-but-not-resolving subcommand (cargo llvm-cov, insta, udeps, bloat) silently
# out of scope, where no floor can see it.
#
# In: subcommands that COMPILE or LINK, so the two *-apple-darwin rustflags reach them.
# Out, each for a stated reason:
#   fmt                 formats; neither compiles nor links (.moon/tasks/rust.yml:125-149)
#   tree, metadata      resolve the graph; never compile (this is AC 4's `cargo tree`
#                       exclusion, encoded in the predicate so a FUTURE cargo-tree gate is
#                       covered on day one rather than needing its own waiver)
#   deny, machete       third-party static scans over the manifest and lock
#   add/remove/update/generate-lockfile/vendor/fetch   lock manipulation, no build
CONFIG_SENSITIVE_VERBS = (
    "bench", "build", "check", "clippy", "doc", "fix", "nextest",
    "package", "publish", "run", "test",
)
CONFIG_SENSITIVE_RE = re.compile(
    r"\bcargo\s+(?:\+\S+\s+)?(?:" + "|".join(CONFIG_SENSITIVE_VERBS) + r")\b"
)
CARGO_CONFIG_INPUT = "rs/.cargo/config.toml"

# Only these tokens confer a cwd. A bare `rs`-containing ARGUMENT must never do so:
# `cargo deny --manifest-path rs/Cargo.toml` and `cargo machete rs` both mention `rs` and both
# run from the repo ROOT. MEASURED on cargo 1.95.0 (SMA-599 §2.3): with rs/.cargo/config.toml
# made malformed, cwd=rs/ fails at rc 101 while cwd=root with --manifest-path succeeds at rc 0,
# so --manifest-path does NOT move cargo's config walk.
CWD_TOKEN_RE = re.compile(r"(?:\bcd\b|\bpushd\b|--cwd)\s+[\"']?([^\"'\s;&|)]+)")
# One round of literal substitution, enough for `RS_DIR="$REPO_ROOT/rs"` … `cd "$RS_DIR"`
# (ci/publish-metadata/run.sh:89,1654). Both `$VAR` and `${VAR}` forms.
VAR_ASSIGN_RE = re.compile(r"""(?m)^\s*([A-Za-z_][A-Za-z0-9_]*)=["']?([^"'\s]+)["']?""")
RS_PATH_RE = re.compile(r"(?:^|/)rs(?:/|$)")

# A10's waivers. EMPTY, like ALLOW_OVER_APPROXIMATION: every exclusion is structural, via the
# verb predicate or the cwd rule. An entry needs a non-empty reason, and an entry naming a task
# outside the examined set is itself a row.
ALLOW_MISSING_CARGO_CONFIG = {}

# A10's floor. Members must be IN SCOPE and NOT allowlisted — a default-deny gate has a second
# vacuity mode the FFI floors do not: an allowlist that grows to swallow the derived set.
REQUIRED_CARGO_CONFIG_TASKS = (
    "paigasus-kernel-rs:build",
    "paigasus-iam-rs:test",
    "paigasus-kernel-ts:build",
    "repo:parity-corpus-drift",
    "repo:publish-metadata",
)
```

- [ ] **Step 4: Implement A10**

After `check_version_lockstep_no_write`:

```python
def _cwd_inside_rs(text, source_dir):
    """True when this task's cargo runs with cwd under `rs/`.

    Reads RAW text, never the quote-stripped form: stripping strings first turns
    `RS_DIR="$REPO_ROOT/rs"` into `RS_DIR=` and `cd "$RS_DIR"` into `cd`, and the whole
    shape dies.
    """
    if source_dir == "rs" or source_dir.startswith("rs/"):
        return True
    env = dict(VAR_ASSIGN_RE.findall(text))
    for token in CWD_TOKEN_RE.findall(text):
        resolved = token
        for name, value in env.items():
            resolved = resolved.replace(f"${{{name}}}", value).replace(f"${name}", value)
        if RS_PATH_RE.search(resolved):
            return True
    return False


def check_cargo_config_inputs(projects, root, allow=None, floor=None):
    """A10: every task whose cargo can READ rs/.cargo/config.toml must key on it.

    Scope is the conjunction of two independent tests, and both matter:
      * the subcommand is in CONFIG_SENSITIVE_VERBS (it compiles or links); and
      * cwd resolves inside `rs/` (cargo finds the file by walking UP from cwd).

    Spans both input buckets and treats an absent bucket as a violation, never a skip — the
    contract A4/A5/A6/A7 share. MEASURED: `moon.yml:239`'s `rs/.cargo/config.toml` and
    `.moon/tasks/rust.yml:46`'s `/rs/.cargo/config.toml` both resolve to the same slash-free
    path, which is why all 58 declaring tasks match verbatim.
    """
    allow = ALLOW_MISSING_CARGO_CONFIG if allow is None else allow
    floor = REQUIRED_CARGO_CONFIG_TASKS if floor is None else floor
    rows, in_scope = [], set()
    for target, kind in sorted(derive_cargo_tasks(projects, root).items()):
        pid, _, name = target.partition(":")
        blob = projects[pid]["invocations"][name]
        text = blob
        if kind == "script":
            for path in task_script_refs(projects, root, target):
                text += "\n" + Path(path).read_text()
        # A wrapper reaches cargo without a literal verb, so the verb test cannot see it. The
        # three FFI tasks compile and link cdylibs and wasm32 by construction.
        sensitive = kind == "wrapper" or bool(CONFIG_SENSITIVE_RE.search(text))
        if not (sensitive and _cwd_inside_rs(text, projects[pid]["source_dir"])):
            continue
        in_scope.add(target)
        reason = allow.get(target)
        if reason is not None:
            if not reason.strip():
                rows.append(
                    f"{target} is in ALLOW_MISSING_CARGO_CONFIG with an empty reason — an "
                    f"exemption is allowed, a silent one is not"
                )
            continue
        files = (projects[pid].get("task_inputs") or {}).get(name)
        globs = (projects[pid].get("task_input_globs") or {}).get(name)
        if files is None or globs is None:
            rows.append(
                f"{target} reported no `inputFiles`/`inputGlobs` — moon's output shape "
                f"changed, so this assertion cannot be evaluated (treated as a violation, "
                f"never skipped)"
            )
            continue
        if CARGO_CONFIG_INPUT not in set(files) | set(globs):
            rows.append(
                f"{target} runs a compiling cargo command with cwd inside rs/ but does not "
                f"key on {CARGO_CONFIG_INPUT} — a rustflags edit replays its cached result"
            )
    for target in sorted(set(floor) - in_scope):
        rows.append(
            f"FLOOR: A10 examines {len(in_scope)} task(s) and {target} is not among them — "
            f"the derivation or the cwd rule has degraded and would assert nothing"
        )
    for target in sorted(set(floor) & set(allow)):
        rows.append(
            f"FLOOR: {target} is a floor member AND allowlisted — an allowlist that grows to "
            f"cover the floor is how a default-deny gate becomes vacuous"
        )
    for target in sorted(set(allow) - in_scope):
        rows.append(
            f"ALLOW_MISSING_CARGO_CONFIG names {target}, which A10 does not examine — the "
            f"waiver is stale; delete it"
        )
    return rows
```

- [ ] **Step 5: Register A10**

Change `EXPECTED_FINDING_KEYS` (`:1690`):

```python
EXPECTED_FINDING_KEYS = ("a1", "a2", "a3", "a4-lint", "a4-fmt", "a5", "a6", "a7", "a8", "a9", "a10")
```

Append to `collect_findings`' list, after the `a8` tuple:

```python
        ("a10", check_cargo_config_inputs(projects, root),
             "A task runs a COMPILING cargo command with cwd inside rs/ but does not key on\n"
             "    rs/.cargo/config.toml, so a rustflags edit replays its cached result\n"
             "    (SMA-594, SMA-599).\n"
             "    Fix: for a crate task the input is declared once for ALL crates in\n"
             "    .moon/tasks/rust.yml — restore it there, not per-crate. For a repo:* gate it\n"
             "    is declared in that task's own `inputs` in moon.yml.\n"
             "    `cargo fmt`, `cargo tree`, `cargo metadata`, `cargo deny` and `cargo machete`\n"
             "    are out of scope BY VERB (they never compile or link) — see\n"
             "    CONFIG_SENSITIVE_VERBS. A `FLOOR:` row means the check itself cannot be\n"
             "    trusted; fix that first, every other A10 row is meaningless until it passes."),
```

Update the PASS string (`:1673`) — change `every cargo-resolving task passes --locked` to:

```python
            f"every cargo-resolving task passes --locked, and every compiling cargo task "
            f"inside rs/ keys on .cargo/config.toml"
```

- [ ] **Step 6: Run to verify it passes**

```bash
python3 ci/affected-graph/cargo_moon_parity.py --self-test
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 ci/affected-graph/cargo_moon_parity.py; echo "rc=$?"
```

Expected: both PASS, rc=0. If A10 reports rows on the real tree, do NOT waive them — reconcile
against the spec's §1.3 table first (58 of 60 blob-matched tasks already declare the file).

**Two scope outcomes to expect, neither a bug — do not "fix" them.**
`paigasus-kernel-ts:build` enters scope through its `--cwd ../../../rs/...`, which is why it is
a floor member. `paigasus-kernel-py:test` does NOT: its blob is
`uv sync --reinstall-package paigasus-py-bindings` with no cd and a `py/` source dir, and
maturin injects the apple-darwin args itself, so cargo's config walk is not what reaches it.
A5 already demands the input from all three FFI tasks via `FFI_TASK_INPUTS`, so it stays
covered — by the assertion that owns it. If the `paigasus-kernel-ts:build` floor row fires,
the `--cwd` token is not surviving into moon's resolved blob; fix `CWD_TOKEN_RE`, do not drop
the floor member.

- [ ] **Step 7: Prove A10 can FAIL — mutation 1, a `repo:*` gate**

```bash
python3 - <<'PY'
import pathlib
p = pathlib.Path("moon.yml"); t = p.read_text()
old = """      - 'rs/.config/nextest.toml'
      # Compiles and links from inside `rs/`, so cargo reads this file on its upward walk and it
      # influences this task's output (SMA-594). Same argument as parity-corpus-drift above. Also
      # NOT inherited from .moon/tasks/rust.yml, for the same reason nextest.toml above is not.
      - 'rs/.cargo/config.toml'"""
assert t.count(old) == 1, "anchor not unique — re-read moon.yml"
p.write_text(t.replace(old, "      - 'rs/.config/nextest.toml'", 1))
PY
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 ci/affected-graph/cargo_moon_parity.py; echo "rc=$?"
```

Expected: rc=1, with a row naming `repo:observability-drift`. Restore:

```bash
git checkout -- moon.yml && git diff --stat moon.yml
```

(`git checkout --` is safe HERE and only here: `moon.yml` carries no uncommitted work of ours until Task 5.)

- [ ] **Step 8: Prove A10 can FAIL — mutation 2, the shared inherited line**

```bash
python3 - <<'PY'
import pathlib
p = pathlib.Path(".moon/tasks/rust.yml"); t = p.read_text()
old = """      - '/rs/.config/nextest.toml'
      - '@group(upstreams)'
      - '/rs/.cargo/config.toml'"""
assert t.count(old) == 1, "anchor not unique"
p.write_text(t.replace(old, """      - '/rs/.config/nextest.toml'
      - '@group(upstreams)'""", 1))
PY
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 ci/affected-graph/cargo_moon_parity.py; echo "rc=$?"
```

Expected: rc=1 with **thirteen** `:test` rows — one per crate — proving A10 sees inheritance. Restore:

```bash
git checkout -- .moon/tasks/rust.yml && git diff --stat .moon/tasks/rust.yml
```

- [ ] **Step 9: Commit**

```bash
git add ci/affected-graph/cargo_moon_parity.py
git commit -m "ci(repo): assert every compiling cargo task keys on .cargo/config.toml (SMA-599)"
```

---

### Task 5: Reachability — schedule the gate on its own inputs

**Files:**
- Modify: `moon.yml` (`repo:affected-smoke` `inputs`, around `:196`)
- Modify: `ci/actionlint/run.sh` (`T_AFFECTED_SMOKE_REQUIRED_INPUTS`, `:2100-2122`)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: nothing consumed later. This task is what makes Tasks 3–4's script pins reachable.

- [ ] **Step 1: Add the broad glob to `repo:affected-smoke`**

In `moon.yml`, immediately after the existing `- 'ci/actionlint/**/*'` entry, add:

```yaml
      # SMA-599 — A8's script arm and A10 both READ the gate scripts a Moon task invokes
      # (ci/publish-metadata/run.sh, ci/version-lockstep/run.sh today, any future one
      # tomorrow). Without a glob covering them the assertions are real but unreachable: the
      # PR that drops a `--locked` from a gate script does not schedule this task. The four
      # narrower ci/ globs above are KEPT rather than replaced — check 8e in
      # ci/actionlint/run.sh asserts containment over a 21-entry array and floors it at
      # `-ge 20`, so replacing four with one would take it to 18 and force loosening a floor.
      - 'ci/**/*'
```

- [ ] **Step 2: Extend check 8e's array**

In `ci/actionlint/run.sh`, add to `T_AFFECTED_SMOKE_REQUIRED_INPUTS`, after the
`'ci/workflow-credentials/**/*'` entry:

```bash
  'ci/**/*'
```

- [ ] **Step 3: Verify moon still resolves the task and the count grew**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 - <<'PY'
import json, subprocess
raw = json.loads(subprocess.run(["moon", "query", "projects"],
                                capture_output=True, text=True, check=True).stdout)
for p in raw["projects"]:
    if p["id"] != "repo":
        continue
    t = p["tasks"]["affected-smoke"]
    ins = list(t.get("inputFiles") or []) + list(t.get("inputGlobs") or [])
    print("resolved inputs:", len(ins))
    print("ci/ globs:", sorted(i for i in ins if i.startswith("ci/")))
PY
```

Expected: 23 resolved inputs, and the `ci/` list now holds five entries including `ci/**/*`.

- [ ] **Step 4: Run the two gates that guard each other**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:actionlint --force 2>&1 | tail -20
moon run repo:affected-smoke --force 2>&1 | tail -20
```

Expected: both PASS. `repo:actionlint` failing here means check 8e's array and `moon.yml`
disagree — fix the array, not the moon.yml entry.

If `affected-smoke` fails in under 3 seconds, capture the full output BEFORE re-running and
grep it for `proto-shim`: that is the known infrastructure abort (CLAUDE.md), not a real red,
and a re-run destroys the evidence.

- [ ] **Step 5: Run `repo:input-liveness`, which asserts declared globs are live**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:input-liveness --force 2>&1 | tail -15
```

Expected: PASS. `ci/**/*` matches many tracked files, so no dead-input row.

- [ ] **Step 6: Commit**

```bash
git add moon.yml ci/actionlint/run.sh
git commit -m "ci(repo): key repo:affected-smoke on the gate scripts its pins read (SMA-599)"
```

---

### Task 6: Correct the documentation the new gate falsifies

**Files:**
- Modify: `CLAUDE.md` (the `rs/.cargo/config.toml` bullet and its follow-on bullet)
- Modify: `ci/affected-graph/README.md` (add the A10 bullet; correct `:173-177`)

**Interfaces:**
- Consumes: nothing.
- Produces: nothing. **No gate asserts this prose**, which is exactly why it must land in the
  same PR — a future engineer reading the stale text would either rebuild A10 or assume the
  input may be omitted.

- [ ] **Step 1: Correct the CLAUDE.md counts**

In the `rs/.cargo/config.toml` bullet, replace the sentence *"**Only 16 of those 61
declarations are asserted**"* and the sentence beginning *"The 39 `build`/`build-release`/`test`
declarations and the three gates are declared by hand and asserted by nothing — delete one and
CI stays green."* with:

```
  **Every one of those declarations is now asserted** by `repo:affected-smoke`'s A10
  (`ci/affected-graph/cargo_moon_parity.py`, SMA-599): a task is in scope when its cargo
  subcommand COMPILES or LINKS (`CONFIG_SENSITIVE_VERBS`) and its cwd resolves inside `rs/`.
  A10 reads moon's RESOLVED inputs, so the three inherited lines in `.moon/tasks/rust.yml`
  cover thirteen crates each and deleting one reds thirteen tasks. `cargo fmt` is out of
  scope BY VERB, not by accident — it neither compiles nor links, which is why `fmt` omits
  the input deliberately.
```

- [ ] **Step 2: Correct the follow-on bullet**

Replace the bullet opening *"**Nothing enforces that one rule.**"* with:

```
- **A10 enforces the cargo-config rule; nothing enforces the GENERAL one.** A10
  (`ci/affected-graph/cargo_moon_parity.py`, SMA-599) closes the `rs/.cargo/config.toml`
  case specifically, including for a gate that reaches cargo through its own
  `ci/**/run.sh` — that whole class was outside A8 too until SMA-599. What is still true:
  A4 covers each crate's `lint`/`fmt`, A5 the three derived FFI tasks, and
  `repo:input-liveness` proves DECLARED inputs are live, never that NEEDED ones are
  declared. A future `repo:*` task can omit some OTHER input it reads and nothing reds, so
  check by hand when adding a gate.
```

- [ ] **Step 3: Correct `ci/affected-graph/README.md:173-177`**

That passage states a cargo call inside a `.sh` a Moon task invokes is outside A8's derived
set. Replace it with:

```markdown
A cargo call inside a `ci/**/*.sh` that a Moon task invokes IS in scope since SMA-599: the
derivation follows the script and classifies its lines (heredoc bodies skipped, quoted
strings stripped before `#` comments, backslash continuations joined, both flag tests scoped
to the command segment). Two limits remain. The scan is PATH-INSENSITIVE — it reports a line
the task's arguments may never reach, which is why `repo:version-lockstep`'s `cargo update -w`
is waived rather than excluded. And following is one level deep and shell-only: a script
invoking another script, `ops/nats/check-subjects.sh`, and the three `.py` gate entrypoints
are all unfollowed.
```

- [ ] **Step 4: Add the A10 bullet to the README's assertion list**

Beside the existing A8 bullet:

```markdown
- **A10** — every Moon task whose cargo subcommand COMPILES or LINKS, with cwd inside `rs/`,
  keys on `rs/.cargo/config.toml`. Scope is a conjunction: the verb predicate
  (`CONFIG_SENSITIVE_VERBS`, deliberately NOT A8's `LOCK_RESOLVING_VERBS`) and a cwd rule that
  reads raw text so `RS_DIR="$REPO_ROOT/rs"` … `cd "$RS_DIR"` resolves. `repo:deny` and
  `repo:machete` fall out structurally — MEASURED on cargo 1.95.0, `--manifest-path` does not
  move cargo's config walk. Ships with an EMPTY allowlist; every exclusion is structural.
```

- [ ] **Step 5: Verify no gate reds on the doc edits**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:actionlint --force 2>&1 | tail -10
```

Expected: PASS. CLAUDE.md carries the marker-delimited `ci-targets` command, which this task
does NOT touch — if actionlint reds, check that neither marker was duplicated or quoted.

- [ ] **Step 6: Commit**

```bash
git add CLAUDE.md ci/affected-graph/README.md
git commit -m "docs(repo): record A10 where the stale no-enforcement claims lived (SMA-599)"
```

---

### Task 7: Full-graph verification

**Files:**
- No production changes. This task runs the gates the way CI does and records the results.

**Interfaces:**
- Consumes: everything from Tasks 1–6.
- Produces: the evidence for AC 8 and for the PR body.

- [ ] **Step 1: Run the affected-graph guard for expected-set movement (AC 8)**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
unset AI_AGENT CLAUDECODE CLAUDE_CODE_ENTRYPOINT
bash ci/affected-graph/run.sh 2>&1 | tail -40; echo "rc=$?"
```

Expected: rc=0 and no expected-set movement. AC 8 asks for a RUN, not an inspection — if any
case moves, explain it in the PR body rather than re-baselining silently.

- [ ] **Step 2: Run the negative control, so a green run is not a broken harness**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
bash ci/affected-graph/run.sh --negative-control 2>&1 | tail -20; echo "rc=$?"
```

Expected: rc=0 with the control reporting red as expected. An rc of 2 is an infrastructure
abort, NOT a pass — read the message before continuing.

- [ ] **Step 3: Run the full CI graph exactly as `ci.yml` does**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :http-extractor-envelope :input-liveness :promtool :observability-drift \
  :nats-permissions :release-parity :release-parity-py :release-parity-ts \
  :publish-metadata :version-lockstep :workflow-credentials --base origin/main \
  --include-relations 2>&1 | tail -40
```

Expected: all green. Notes for reading a failure:
- an unattributed "1 failed" is diagnosed from `.moon/cache/ciReport.json`;
- the three `repo:release-parity*` gates abort INCONCLUSIVE at rc=2 inside an agent session
  unless `AI_AGENT`, `CLAUDECODE` and `CLAUDE_CODE_ENTRYPOINT` are unset — Step 1 already
  unset them in that shell, so use the same shell or unset them again;
- `paigasus-iam`'s Docker suites need a reachable daemon; a Docker-less run reds exactly one
  canary (`docker_preflight`), which is the intended signal, not a regression from this change.

- [ ] **Step 4: Confirm the working tree is clean**

```bash
git status --short
```

Expected: empty. Any leftover file means a mutation from Task 3 or 4 was not restored — restore
it before opening the PR.

- [ ] **Step 5: Commit any verification-driven fix**

Only if Steps 1–3 required a change:

```bash
git add -A
git commit -m "ci(repo): reconcile the affected-graph expected sets with A10 (SMA-599)"
```

---

## Acceptance criteria coverage

| AC | Where |
| -- | -- |
| 1 — single derivation with an anti-vacuity floor, emptying proven to fail | Task 2 (Steps 1, 3, 4) |
| 2 — a cargo-from-rs task without the input fails the gate | Task 4 (Steps 1, 4) |
| 3 — proven to red by mutation on a `repo:*` gate, then restored | Task 4 (Steps 7, 8) |
| 4 — the `cargo tree` exclusion encoded with a stated reason | Task 4 (Step 3, `CONFIG_SENSITIVE_VERBS`) |
| 5 — the `--locked` decision recorded, with its ergonomics cost | Spec §6 D6 (already committed) |
| 6 — proven with the staleness experiment, not a bogus-flag test | Spec §2.1 (already committed); bounded by L6 |
| 7 — n/a, the behavioural change was taken, not declined | Spec §6 D6 |
| 8 — `ci/affected-graph/run.sh` reports no expected-set movement | Task 7 (Steps 1, 2) |

<!-- moon-diagnosis:superseded -->
> **Superseded (SMA-597).** The `ciReport.json` diagnosis advice above does not work as written:
> there is no action-level `exitCode` key, and the file carries no stdout/stderr at all. The
> measured procedure is in CLAUDE.md between the `moon-diagnosis` markers. This document is left
> otherwise unedited as a record of what was believed when it was written.
