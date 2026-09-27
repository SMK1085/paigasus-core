<!-- moon-diagnosis:ok -->

# SMA-708 Stop Background Git Maintenance in CI Fixtures — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The CI fixture repositories that commit, fetch or receive a push start no background
`git maintenance`, so `shutil.rmtree` / `rm -rf` of a fixture cannot race a detached git process.

**Architecture:** Each in-scope fixture persists `maintenance.auto false` and `gc.auto 0` in its
own `.git/config` directly after `git init`. `release_plan.py` gets a self-test row that records a
git trace2 event log of a fixture build and fails if git starts a maintenance or gc child.

**Tech Stack:** Python 3.12 (stdlib only, run through `uv`), bash, git ≥ 2.47.

**Spec:** `docs/superpowers/specs/2026-09-27-sma-708-git-maintenance-fixtures-design.md`

## Global Constraints

- The two keys, exactly: `maintenance.auto false` and `gc.auto 0`, set with `git config` (persisted), directly after `git init`.
- Never use `shutil.rmtree(..., ignore_errors=True)`, a retry around `rmtree`, or `rm -rf ... || true` to hide the race.
- No shared `ci/lib/` helper (SMA-596 D4). Repeat the lines at each site.
- Scope: only sites C1–C6 of spec §5.1. Do NOT touch `ci/next-public/run.sh`, `ci/ruff/run.sh`, or `release_plan.py:1247`.
- `release_plan.py` stays stdlib-only; lines ≤ 100 characters; every existing row keeps its behaviour.
- Every source file keeps its SPDX header. No new files in the repository except as listed.
- Commits: conventional, scope `ci` (for example `fix(ci): …`), and end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Never `--amend`, never `git reset`, never `--no-verify`.
- Shell environment for every command: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` and `export PROTO_REPORTER=text`.
- Work only in the worktree `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-708-git-maintenance-fixtures` on branch `feature/sma-708-git-maintenance-fixtures`. Check `git branch --show-current` before the first commit.
- Scratch scripts go in `/private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/e747dd0a-54a8-4f57-85d0-7f1f732b0ead/scratchpad/`, never in the repository. Do not install host software; use `docker run ubuntu:24.04` if a Linux tool is needed.

## Review Focus

1. **Host git config makes the row pass wrongly.** A developer with a global `maintenance.auto false` must still see the row fail on unfixed code. Pinned by `GIT_CONFIG_GLOBAL=/dev/null` + `GIT_CONFIG_NOSYSTEM=1` in Task 1 and by mutation 1 run on this Mac.
2. **An empty or missing trace file makes the row pass vacuously.** Pinned by the non-vacuous `commit` start-event check and by mutation 3 in Task 1.
3. **Config set after the commit.** The key is present but the commit already started maintenance. Pinned by mutation 2 in Task 1.
4. **Existing rows change behaviour through the new `git_env` argument.** Default `None` must mean "inherit the environment", exactly as before. Pinned by the full `--self-test` staying green in Task 1 Step 6.
5. **External tools run git where the keys do not apply** (a clone made by `release-plz`, or the bare `origin.git` in `semantic-release.sh`). Pinned by the trace check with a failing control in Task 2.

---

### Task 1: `release_plan.py` — the keys, the `git_env` plumbing, and the SMA-708 self-test row

**Files:**
- Modify: `ci/release-plan/release_plan.py:1018-1032` (`_git_commit_and_tag`)
- Modify: `ci/release-plan/release_plan.py:1035-1069` (`_complete_chain_tree`)
- Modify: `ci/release-plan/release_plan.py` (new helper after `_console_package_inconclusive`, about line 1404; new entry at the end of `COLLECTION_ROWS`, about line 1681; the comment at lines 1618-1623)
- Modify: `ci/release-plan/README.md:96-97`

**Interfaces:**
- Produces: `_git_commit_and_tag(tmp: str, tags: tuple[str, ...], *, git_env: dict[str, str] | None = None) -> None`
- Produces: `_complete_chain_tree(tmp, *, versions=None, changelogs=None, console_texts=None, tags=("unrelated-tag",), git_env: dict[str, str] | None = None) -> Path`
- Produces: `_fixture_repo_starts_no_maintenance() -> str | None` and the row label `"SMA-708 a committed fixture repository starts no background git maintenance"`

- [ ] **Step 1: Add the `git_env` plumbing (no behaviour change yet)**

Replace `_git_commit_and_tag` (lines 1018-1032) with this version. It has NO maintenance keys yet — they come in Step 4, after the row is seen to fail.

```python
def _git_commit_and_tag(tmp: str, tags: tuple[str, ...], *,
                        git_env: dict[str, str] | None = None) -> None:
    """One commit, then each tag in `tags`. Moved here unchanged from
    _service_unparsable_version_asserts_three_tree, so that several trees can share it.

    `git_env` is the environment of every git call here. None inherits this process's
    environment, as before. The SMA-708 row passes a copy with a trace2 target and no host config.
    """
    def git(*args: str) -> None:
        subprocess.run(["git", *args], check=True, env=git_env)

    git("init", "-q", tmp)
    git("-C", tmp, "config", "user.email", "release-plan-self-test@example.com")
    git("-C", tmp, "config", "user.name", "release-plan self-test")
    git("-C", tmp, "add", "-A")
    # `-c commit.gpgsign=false` / `-c tag.gpgSign=false`: this repo's global git config signs
    # every commit and tag (1Password-backed SSH signing). A throwaway fixture tree must not
    # depend on that being unlocked, and an unsigned, unannotated tag is all `repo_tags` reads.
    git("-C", tmp, "-c", "commit.gpgsign=false", "commit", "-q", "-m", "init")
    for tag in tags:
        git("-C", tmp, "-c", "tag.gpgSign=false", "tag", tag)
```

In `_complete_chain_tree`, add the keyword parameter and pass it through. The signature becomes:

```python
def _complete_chain_tree(tmp: str, *, versions: dict[str, str] | None = None,
                         changelogs: dict[str, str] | None = None,
                         console_texts: dict[str, str] | None = None,
                         tags: tuple[str, ...] = ("unrelated-tag",),
                         git_env: dict[str, str] | None = None) -> Path:
```

Append this sentence to its docstring (before the closing `"""`): `` `git_env` goes to _git_commit_and_tag unchanged.`` and change its last-but-one line from `_git_commit_and_tag(tmp, tags)` to `_git_commit_and_tag(tmp, tags, git_env=git_env)`.

- [ ] **Step 2: Write the failing row**

Insert this helper directly after `_console_package_inconclusive` (after its `finally: shutil.rmtree(tmp)`, about line 1403), with two blank lines around it:

```python
def _fixture_repo_starts_no_maintenance() -> str | None:
    """SMA-708. A fixture repository that commits must start no background git maintenance.

    After a commit, git starts `git maintenance run --auto --detach`, which creates and deletes
    .git/objects/maintenance.lock. A `shutil.rmtree` of the fixture that races it raised
    FileNotFoundError on Python 3.12 (CI run 36273324959; reproduced 7 times in 2,400 runs under
    load, spec §3 M7). The row records a trace2 event log of a real fixture build and fails on any
    maintenance or gc child. GIT_CONFIG_GLOBAL / GIT_CONFIG_NOSYSTEM stop a host setting from
    making it pass. It also requires a `commit` start event, so an empty trace cannot pass, and it
    reads maintenance.auto back from the fixture's own .git/config.

    Mutations: delete the two config lines in _git_commit_and_tag; move them after the commit;
    drop `env=git_env` there. Each one reds this row.
    """
    tmp = tempfile.mkdtemp()
    trace = Path(f"{tmp}.trace2.json")
    try:
        git_env = {**os.environ, "GIT_TRACE2_EVENT": str(trace),
                   "GIT_CONFIG_GLOBAL": os.devnull, "GIT_CONFIG_NOSYSTEM": "1"}
        _complete_chain_tree(tmp, git_env=git_env)
        events = ([json.loads(line) for line in trace.read_text().splitlines() if line.strip()]
                  if trace.exists() else [])
        spawned = [e["argv"] for e in events if e.get("event") == "child_start"
                   and ("maintenance" in e.get("argv", []) or "gc" in e.get("argv", []))]
        if spawned:
            return f"the fixture commit started background git maintenance: {spawned!r}"
        if not any(e.get("event") == "start" and "commit" in e.get("argv", []) for e in events):
            return (f"the trace2 log {trace} holds no `commit` start event "
                    f"({len(events)} events); the row did not observe the fixture build")
        got = subprocess.run(["git", "-C", tmp, "config", "--local", "--get", "maintenance.auto"],
                             capture_output=True, text=True, env=git_env).stdout.strip()
        if got != "false":
            return f"the fixture's .git/config has maintenance.auto={got!r}, want 'false'"
        return None
    finally:
        shutil.rmtree(tmp)
        trace.unlink(missing_ok=True)
```

Add `import os` to the imports, in alphabetical order between `import json` and `import re` (line 32-33).

Append this entry as the LAST element of `COLLECTION_ROWS` (after the `_sed_parity_fixture_registry` entry):

```python
    ("SMA-708 a committed fixture repository starts no background git maintenance",
     _fixture_repo_starts_no_maintenance),
```

- [ ] **Step 3: Run the self-test and see the new row fail**

Run (from the worktree root):
```bash
uv run --locked --project ci/release-plan --python '>=3.12' python3 ci/release-plan/release_plan.py --self-test; echo "rc=$?"
```
Expected: `rc=3`, and stderr holds exactly one FAIL line, for `'SMA-708 a committed fixture repository starts no background git maintenance'`, with the text `the fixture commit started background git maintenance: [['git', 'maintenance', 'run', '--auto', ...]]`. No other row fails. If the row passes here, STOP and report: the row cannot see the defect.

- [ ] **Step 4: Add the two keys in `_git_commit_and_tag`**

Directly after the line `git("init", "-q", tmp)`, insert:

```python
    # SMA-708: no background `git maintenance run --auto --detach` after the commit below. It
    # writes .git/objects/maintenance.lock while the caller's shutil.rmtree runs, and on Python
    # 3.12 that race raised FileNotFoundError. maintenance.auto alone stops it (spec §3 M2);
    # gc.auto 0 is a second layer only, for a git that runs `gc --auto` directly.
    git("-C", tmp, "config", "maintenance.auto", "false")
    git("-C", tmp, "config", "gc.auto", "0")
```

- [ ] **Step 5: Fix the stale comment above `COLLECTION_ROWS`**

Replace lines 1618-1623 (the comment that starts `# The collection-layer rows: paths a pure-function fixture cannot reach. Fourteen of the fifteen`) with:

```python
# The collection-layer rows: paths a pure-function fixture cannot reach. Most of them need a
# filesystem (they build throwaway trees under tempfile.mkdtemp()); a few, such as
# _markers_are_mutually_exclusive, need none but still cannot be expressed as decide()-only
# FIXTURES rows, since they assert a property of a helper other than decide(). Module-level
# so `--collection-count` can count them and so self_test()'s floor below has something to floor;
# the FIXTURES floor's own comment explains why a countable table matters.
```

- [ ] **Step 6: Run the self-test and see it pass**

Run:
```bash
uv run --locked --project ci/release-plan --python '>=3.12' python3 ci/release-plan/release_plan.py --self-test; echo "rc=$?"
uv run --locked --project ci/release-plan --python '>=3.12' python3 ci/release-plan/release_plan.py --collection-count
```
Expected: `rc=0` and no FAIL line; the count prints `38`.

- [ ] **Step 7: Mutation 2 — keys after the commit**

Temporarily move the two `git("-C", tmp, "config", ...)` lines of Step 4 (only the two calls; leave the comment) to directly after the `git(... "commit" ...)` line. Run the Step 3 command.
Expected: `rc=3`; the SMA-708 FAIL line says `started background git maintenance`. Then move the two lines back to where Step 4 put them, by an edit (never `git checkout --`).

- [ ] **Step 8: Mutation 3 — the row does not see the build**

Temporarily change `subprocess.run(["git", *args], check=True, env=git_env)` in the inner `git` function to `subprocess.run(["git", *args], check=True)`. Run the Step 3 command.
Expected: `rc=3`; the SMA-708 FAIL line says `holds no `commit` start event`. Restore by an edit.

Mutation 1 (delete the keys) is Step 3 itself: it already showed the row fails without them.

- [ ] **Step 9: Run the full self-test once more after the restores**

Run the Step 6 commands. Expected: `rc=0`, count `38`. Also run `git diff --stat` and confirm only `release_plan.py` changed so far.

- [ ] **Step 10: Update the README row count**

In `ci/release-plan/README.md` line 96, change `plus thirty-seven collection-layer rows (SMA-688 raises this from fifteen).` to `plus thirty-eight collection-layer rows (SMA-688 raised this from fifteen; SMA-708 added one).` Then, at the end of that bullet's paragraph (before the next `- ` bullet), add this sentence on its own line, indented like the paragraph:

```
  SMA-708's row builds a committed fixture under a git trace2 event log and fails if git starts a background `git maintenance` or `gc` child, which raced the row's `shutil.rmtree` on Python 3.12.
```

Read the bullet after the edit and confirm it still reads as one paragraph.

- [ ] **Step 11: Lint**

Run:
```bash
/opt/homebrew/bin/bash ci/ruff/run.sh; echo "rc=$?"
```
Expected: `rc=0`. (`repo:ruff-ci` needs bash 4+, so do not use `/bin/bash` here. A failure that names `release_plan.py` is a real finding: fix it and re-run.)

- [ ] **Step 12: Commit**

```bash
git add ci/release-plan/release_plan.py ci/release-plan/README.md
git commit -F - <<'EOF'
fix(ci): stop background git maintenance in release_plan.py fixtures (SMA-708)

A fixture commit started a detached `git maintenance run --auto`,
which raced the row's shutil.rmtree on Python 3.12. The fixture
now sets maintenance.auto false and gc.auto 0, and a new self-test
row fails if a fixture build starts a maintenance or gc child.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
```

---

### Task 2: The shell fixtures — `release-plan/run.sh` and the three `release-parity` modules

**Files:**
- Modify: `ci/release-plan/run.sh:236-242` (`_build_synthetic_tree`)
- Modify: `ci/release-parity/ecosystems/release-plz.sh:120-124`
- Modify: `ci/release-parity/ecosystems/python-semantic-release.sh:163-167`
- Modify: `ci/release-parity/ecosystems/semantic-release.sh:86-88` and `:123-128`
- Modify (line citations): `ci/affected-graph/cargo_moon_parity.py:249`, `:270`; `ci/affected-graph/README.md:163`; `rs/CLAUDE.md:104`
- Scratch (not committed): `<scratchpad>/trace_check.py`

**Interfaces:**
- Consumes: nothing from Task 1.
- Produces: no new names. Each fixture's `.git/config` holds `maintenance.auto=false` and `gc.auto=0`.

- [ ] **Step 1: Write the trace checker (scratch, not committed)**

Create `/private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/e747dd0a-54a8-4f57-85d0-7f1f732b0ead/scratchpad/trace_check.py`:

```python
# Usage: trace_check.py <trace2-event-file>. Prints counts; exit 1 on a maintenance/gc child,
# exit 2 if no commit start event was recorded (the trace saw nothing).
import json, sys
events = []
for line in open(sys.argv[1], encoding="utf-8", errors="replace"):
    try:
        events.append(json.loads(line))
    except ValueError:
        pass
kids = [e["argv"] for e in events if e.get("event") == "child_start"
        and ("maintenance" in e.get("argv", []) or "gc" in e.get("argv", []))]
commits = sum(1 for e in events if e.get("event") == "start" and "commit" in e.get("argv", []))
pushes = sum(1 for e in events if e.get("event") == "start" and "receive-pack" in e.get("argv", []))
print(f"events={len(events)} commit_starts={commits} receive_pack_starts={pushes} "
      f"maintenance_or_gc_children={len(kids)}")
for k in kids[:5]:
    print("  child:", k)
sys.exit(1 if kids else (2 if commits == 0 else 0))
```

- [ ] **Step 2: Record the failing control (before any shell edit)**

Run each gate once with a trace file, on the UNCHANGED shell scripts. Use `/opt/homebrew/bin/bash` for `ci/release-parity/run.sh` and `ci/release-plan/run.sh`.

```bash
S=/private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/e747dd0a-54a8-4f57-85d0-7f1f732b0ead/scratchpad
for g in "release-plz" "python-semantic-release" "semantic-release"; do
  rm -f "$S/t-$g.json"
  GIT_TRACE2_EVENT="$S/t-$g.json" /opt/homebrew/bin/bash ci/release-parity/run.sh --ecosystem "$g" >/dev/null 2>&1; echo "$g gate rc=$?"
  python3 "$S/trace_check.py" "$S/t-$g.json"; echo "check rc=$?"
done
rm -f "$S/t-plan.json"
GIT_TRACE2_EVENT="$S/t-plan.json" /opt/homebrew/bin/bash ci/release-plan/run.sh --negative-control >/dev/null 2>&1; echo "release-plan negctl rc=$?"
python3 "$S/trace_check.py" "$S/t-plan.json"; echo "check rc=$?"
```
Expected: every gate rc=0; every `check rc=1` with `maintenance_or_gc_children` > 0. Record the four lines of counts. If a gate itself fails here, STOP and report — it is a pre-existing failure, not this change. If a `check rc=2` appears, STOP and report: the trace does not reach that gate's git calls.

Note: the release-plan negative control also runs `release_plan.py --self-test`, whose fixtures are already fixed by Task 1 — the children counted here come from `_build_synthetic_tree`.

- [ ] **Step 3: Add the keys to `ci/release-plan/run.sh`**

In `_build_synthetic_tree`, after `git -C "$dir" config user.name "release-plan negative control"` insert:

```bash
    # SMA-708: no background `git maintenance run --auto` after the commit below; it races the
    # caller's `rm -rf`. gc.auto 0 is a second layer only.
    git -C "$dir" config maintenance.auto false
    git -C "$dir" config gc.auto 0
```

- [ ] **Step 4: Add the keys to `release-plz.sh`**

After `    git config tag.gpgsign false` (line 124) insert:

```bash
    # SMA-708: no background `git maintenance run --auto` after a commit here, by us or by
    # release-plz; it races check_case's `rm -rf`. gc.auto 0 is a second layer only.
    git config maintenance.auto false
    git config gc.auto 0
```

- [ ] **Step 5: Add the keys to `python-semantic-release.sh`**

After `      git config tag.gpgsign false` (line 167) insert (six-space indent, as the block):

```bash
      # SMA-708: no background `git maintenance run --auto` after a commit here, by us or by
      # PSR; it races check_case's `rm -rf`. gc.auto 0 is a second layer only.
      git config maintenance.auto false
      git config gc.auto 0
```

- [ ] **Step 6: Add the keys to `semantic-release.sh` — both repositories**

Replace lines 86-88:

```bash
  ( cd "$dir" && git -c init.defaultBranch=main init -q \
    && git config user.email "parity@example.com" && git config user.name "parity" \
    && git config commit.gpgsign false && git config tag.gpgsign false )
```

with:

```bash
  # SMA-708: maintenance.auto false / gc.auto 0 — no background `git maintenance run --auto`
  # after a commit or a fetch here; it races check_case's `rm -rf`. gc.auto is a second layer.
  ( cd "$dir" && git -c init.defaultBranch=main init -q \
    && git config user.email "parity@example.com" && git config user.name "parity" \
    && git config commit.gpgsign false && git config tag.gpgsign false \
    && git config maintenance.auto false && git config gc.auto 0 )
```

In the last subshell of the function, replace the line
`    && git -c init.defaultBranch=main init --bare "$dir/origin.git" -q \`
with:

```bash
    && git -c init.defaultBranch=main init --bare "$dir/origin.git" -q \
    && git -C "$dir/origin.git" config maintenance.auto false \
    && git -C "$dir/origin.git" config gc.auto 0 \
```

Also add one line to the comment block above that subshell (after `# Fully offline.`): `# The bare origin gets the SMA-708 keys before the push: receive-pack can start maintenance.`

- [ ] **Step 7: Run the trace check again (must pass now)**

Run the Step 2 commands again.
Expected: every gate rc=0; every `check rc=0` with `maintenance_or_gc_children=0` and `commit_starts` > 0. For `semantic-release`, `receive_pack_starts` > 0 (this proves the trace reached the push into `origin.git`). If `release-plz` still shows children, a tool runs git in a clone: STOP and report the child argv lines.

- [ ] **Step 8: Run the negative controls**

```bash
/opt/homebrew/bin/bash ci/release-plan/run.sh --negative-control; echo "rc=$?"
for g in release-plz python-semantic-release semantic-release; do
  /opt/homebrew/bin/bash ci/release-parity/run.sh --ecosystem "$g" --negative-control >/dev/null 2>&1; echo "$g negctl rc=$?"
done
```
Expected: all rc=0.

- [ ] **Step 9: Update the four `release-plz.sh:152` citations**

Run `grep -n 'RELEASE_PLZ_BIN" update' ci/release-parity/ecosystems/release-plz.sh` and note the new line number N (expected 156, because Step 4 inserted four lines). In each of these four places, change `release-plz.sh:152` to `release-plz.sh:N`:
- `ci/affected-graph/cargo_moon_parity.py:249`
- `ci/affected-graph/cargo_moon_parity.py:270`
- `ci/affected-graph/README.md:163`
- `rs/CLAUDE.md:104`

Then run `grep -rn 'release-plz.sh:152' . --include='*.py' --include='*.md'` — expected: no output (except inside `docs/superpowers/specs/` history, which stays as written).

- [ ] **Step 10: Commit**

```bash
git add ci/release-plan/run.sh ci/release-parity/ecosystems/release-plz.sh \
  ci/release-parity/ecosystems/python-semantic-release.sh \
  ci/release-parity/ecosystems/semantic-release.sh \
  ci/affected-graph/cargo_moon_parity.py ci/affected-graph/README.md rs/CLAUDE.md
git commit -F - <<'EOF'
fix(ci): stop background git maintenance in the shell fixture repos (SMA-708)

The release-plan negative control and the three release-parity
modules commit, fetch or push in throwaway repositories. Each one
now sets maintenance.auto false and gc.auto 0 after git init,
including the bare origin.git that receives a push.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
```

---

### Task 3: The `ci/CLAUDE.md` rule and the whole-branch verification

**Files:**
- Modify: `ci/CLAUDE.md` (append one section at the end)

**Interfaces:**
- Consumes: Tasks 1 and 2 committed.

- [ ] **Step 1: Add the rule**

Append to `ci/CLAUDE.md`:

```markdown
## Fixture git repositories: no background maintenance (SMA-708)

- A fixture repository under `ci/` in which any process runs `git commit`, `merge`, `fetch`,
  `pull`, `am`, `rebase` or `cherry-pick`, or which receives a push, sets
  `maintenance.auto false` and `gc.auto 0` with `git config` directly after `git init`. This
  includes git calls that an external tool (release-plz, semantic-release, PSR) makes in it.
- Why: after such a command, git starts a detached `git maintenance run --auto`. It writes
  `.git/objects/maintenance.lock` while the fixture's `shutil.rmtree` or `rm -rf` runs. On Python
  3.12 that raced into `FileNotFoundError: 'maintenance.lock'` (7 of 2,400 runs under load).
- A fixture with only `init`, `add`, `tag`, `ls-files` or `rm --cached` needs no keys. If you add
  one of the commands above to such a fixture, add the two keys in the same edit.
- Do not hide the race with `ignore_errors=True`, a retry, or `|| true`.
- Spec: `docs/superpowers/specs/2026-09-27-sma-708-git-maintenance-fixtures-design.md`.
```

- [ ] **Step 2: Commit**

```bash
git add ci/CLAUDE.md
git commit -F - <<'EOF'
docs(ci): record the fixture git maintenance rule (SMA-708)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
```

- [ ] **Step 3: Load reproduction against the real helper**

Create `<scratchpad>/load_real.sh` that runs, in `ubuntu:24.04` with git from `ppa:git-core/ppa` and `python3` (same install lines as `<scratchpad>/load.sh`), with the worktree's `ci/release-plan/release_plan.py` mounted read-only at `/rp/release_plan.py`, 16 parallel loops of this Python, 150 iterations each:

```python
import importlib.util, shutil, sys, tempfile
spec = importlib.util.spec_from_file_location("rp", "/rp/release_plan.py")
rp = importlib.util.module_from_spec(spec); sys.modules["rp"] = rp; spec.loader.exec_module(rp)
fails = 0
for _ in range(150):
    tmp = tempfile.mkdtemp()
    try:
        rp._complete_chain_tree(tmp)
    finally:
        try:
            shutil.rmtree(tmp)
        except Exception as e:
            fails += 1; print(type(e).__name__, e)
print("failures", fails)
```
Expected: `failures 0` from all 16 loops. (The M7 baseline was 7 failures in 2,400 runs.)

- [ ] **Step 4: Run the full gate graph**

Run the marker-delimited command from the root `CLAUDE.md` (`moon ci :build :test … :test-e2e --base origin/main --include-relations`) with the bash shim of memory `affected-smoke-hang-fixed-by-system-bash`. Then re-run directly with `/opt/homebrew/bin/bash` the gates that need bash 4+ (`ci/ruff/run.sh`, `ci/release-parity/run.sh` for the three ecosystems) and read those results. For `repo:actionlint`, read its pipe preflight line: if it reports a capacity ≥ 8192, run `/opt/homebrew/bin/bash ci/actionlint/run.sh` and require rc 0; if it reports 512, say so and rely on CI.
Expected: all green; any red is reported with its `ciReport.json` step-1 output, per the root `CLAUDE.md` diagnosis procedure.
