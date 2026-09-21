# SMA-663 napi staging race — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop `moon ci` from failing when `napi build` makes a `.napi-stage-*` directory that the `rs/Cargo.toml` members glob matches, and guard against a regression.

**Architecture:** Replace the three `members` globs in `rs/Cargo.toml` with 13 literal crate paths. Add assertion A11 to `ci/affected-graph/cargo_moon_parity.py` (run by `repo:affected-smoke`). A11 reds when a `members` entry contains a glob character. Prove the fix with a forced-overlap measurement, and record the mechanism in `CLAUDE.md`.

**Tech Stack:** Cargo workspace TOML, Python 3.11+ (`tomllib`), bash, Moon 2.5.3, `@napi-rs/cli` 3.10.3.

**Spec:** `docs/superpowers/specs/2026-09-21-sma-663-napi-stage-race-design.md`

## Global Constraints

- Work only in the worktree `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-663-napi-stage` on branch `feature/sma-663-napi-stage-race`. Run `git branch --show-current` before the first commit of each task.
- Prefix shell commands with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` so that `moon`, `pnpm` and `uv` resolve to the repo pins. `cargo` comes from rustup, and `rs/rust-toolchain.toml` selects its toolchain.
- Use `/opt/homebrew/bin/python3` (3.14) for `cargo_moon_parity.py`. System `/usr/bin/python3` is 3.9 and has no `tomllib`.
- Conventional commits with a workspace scope and a lowercase subject: `fix(rs): …`, `feat(ci): …`, `docs(repo): …`. Put `(SMA-663)` at the end of the subject. Do not put `#NNN` or `token: value` lines in the body. End each message with `Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>`.
- Never use `--no-verify`. Never `git commit --amend`. Add new commits only.
- Every new source file opens with an SPDX header. (No new source files are expected in this plan.)
- `rs/Cargo.lock` must stay byte-identical to `origin/main`.
- Do not edit the two gated marker blocks (`ci-targets`, `moon-diagnosis`) in the root `CLAUDE.md`.
- Write all prose (comments, docs, commit bodies) in ASD-STE100 Simplified Technical English: short sentences, active voice, no idiom.

---

### Task 1: Literal `members` in `rs/Cargo.toml`, and a `.gitignore` rule

**Files:**
- Modify: `rs/Cargo.toml:1-10` (the comment and the `members` line)
- Modify: `.gitignore` (append one rule)

**Interfaces:**
- Consumes: nothing.
- Produces: a `members` list with 13 literal entries. Task 2's A11 asserts this form.

- [ ] **Step 1: Show that the current form fails with a staging directory (red)**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-663-napi-stage
mkdir rs/crates/bindings/.paigasus-node-bindings.napi-stage-PROBE
cargo metadata --manifest-path rs/Cargo.toml --format-version=1 --locked >/dev/null; echo "rc=$?"
```

Expected: `rc=101` and stderr `failed to load manifest for workspace member` that names `.paigasus-node-bindings.napi-stage-PROBE`. Leave the directory in place for step 3.

- [ ] **Step 2: Replace the `members` line and its comment**

Replace `rs/Cargo.toml` lines 1-10 (from `[workspace]` to the `members = …` line, inclusive) with:

```toml
[workspace]
# Every entry is a LITERAL crate path, one per crate directory (SMA-663). A glob such as
# `crates/bindings/*` also matches a dot-directory. `napi build` stages its output in a sibling
# directory `.paigasus-node-bindings.napi-stage-<random>` that has no Cargo.toml, so while it
# exists every concurrent `cargo metadata` fails with exit 101. `repo:affected-smoke`'s A11 reds
# on any glob character here. A new crate needs one new line; `cargo new` adds it, and A9 reds if
# a crate directory is missing.
#
# Dependabot (SMA-604): its `expand_workspaces` lists only ONE directory level below a glob's
# literal prefix, so `crates/*/*` resolved to ZERO members, it built its sandbox from the five
# `path =` crates in [workspace.dependencies], and it wrote a truncated rs/Cargo.lock (176
# packages against 543) and failed `Failed to update serde!`. A literal entry is taken verbatim,
# so it is safe for Dependabot. A9 transcribes that expander.
members = [
  "crates/bindings/paigasus-node-bindings",
  "crates/bindings/paigasus-py-bindings",
  "crates/bindings/paigasus-wasm",
  "crates/libs/paigasus-iam-core",
  "crates/libs/paigasus-kernel",
  "crates/libs/paigasus-kernel-parity",
  "crates/libs/paigasus-logging",
  "crates/libs/paigasus-observability",
  "crates/libs/paigasus-proto",
  "crates/libs/paigasus-proto-derive",
  "crates/libs/paigasus-service-info",
  "crates/services/paigasus-gateway",
  "crates/services/paigasus-iam",
]
```

Keep the `resolver = "3"` line after it unchanged. First confirm that the list matches the tree:

```bash
ls -d rs/crates/*/*/Cargo.toml | sed 's#^rs/##; s#/Cargo.toml$##' | sort
```

Expected: the same 13 paths. If the tree differs, the tree wins; use its list.

- [ ] **Step 3: Show that the staging directory is now harmless (green)**

```bash
cargo metadata --manifest-path rs/Cargo.toml --format-version=1 --locked >/dev/null; echo "rc=$?"
rmdir rs/crates/bindings/.paigasus-node-bindings.napi-stage-PROBE
```

Expected: `rc=0`.

- [ ] **Step 4: Check that the member set and the lock did not change**

```bash
cargo metadata --manifest-path rs/Cargo.toml --format-version=1 --locked --no-deps \
  | /opt/homebrew/bin/python3 -c 'import json,sys; print(len(json.load(sys.stdin)["workspace_members"]))'
git diff --exit-code origin/main -- rs/Cargo.lock; echo "lock-diff-rc=$?"
```

Expected: `13` and `lock-diff-rc=0`.

- [ ] **Step 5: Add the `.gitignore` rule**

Append to the root `.gitignore`:

```gitignore
# A leftover `napi build` staging directory (SMA-663). napi removes it in a `finally` block, so it
# stays only after a SIGINT or SIGKILL. Cargo ignores it now, but `git add -A` would commit it.
.*.napi-stage-*/
```

Verify:

```bash
mkdir -p rs/crates/bindings/.x.napi-stage-TEST && touch rs/crates/bindings/.x.napi-stage-TEST/f
git status --porcelain --ignored rs/crates/bindings | grep napi-stage
git check-ignore -v rs/crates/bindings/.x.napi-stage-TEST/f
rm -r rs/crates/bindings/.x.napi-stage-TEST
git status --porcelain
```

Expected: the `--ignored` line starts with `!!`. `check-ignore` names the new `.gitignore` line. The final `git status --porcelain` shows only `rs/Cargo.toml` and `.gitignore`.

- [ ] **Step 6: Commit**

```bash
git add rs/Cargo.toml .gitignore
git commit -m "fix(rs): list Cargo workspace members literally so napi staging is not a member (SMA-663)

napi build stages beside the crate in a dot-directory with no Cargo.toml.
The crates/bindings/* glob matched it, so a concurrent cargo metadata
failed with exit 101 for the full life of that directory.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 2: Assertion A11 in `repo:affected-smoke`

**Files:**
- Modify: `ci/affected-graph/cargo_moon_parity.py`
  - `check_member_globs` (about line 1716): extract the members read into a helper.
  - Add `check_member_literals` directly after `check_member_globs`.
  - `self_test()`: add A11 rows directly after the A9 block (the block that ends with the "missing rs/Cargo.toml" infra row, about line 3087).
  - `EXPECTED_FINDING_KEYS` (about line 4230), `collect_findings` (about line 4341), `main()` PASS text (about line 4380).
  - The module docstring near line 25 that introduces A9: add one A11 sentence.
- Modify: `ci/affected-graph/README.md` (A9 section about line 220-244, and "A1-A10" at about line 340)

**Interfaces:**
- Consumes: Task 1's literal `members` (the real-tree run must be green).
- Produces:
  - `read_workspace_members(root, assertion) -> list[str]`. It raises `MoonOutputError` for a missing `rs/Cargo.toml` or an absent/empty `members`. `assertion` is the label in the message (`"A9"` or `"A11"`).
  - `check_member_literals(root) -> list[str]`. One row for each entry that contains `*`, `?` or `[`.
  - Findings key `"a11"`, after `"a10"`.

- [ ] **Step 1: Write the failing self-test rows**

Insert this block in `self_test()` directly after the A9 `with tempfile.TemporaryDirectory() as tmp:` block ends (after the `check_member_globs(root9, crates9)` "missing rs/Cargo.toml" `try/except`), at the same indentation as that `with`:

```python
    # A11 (SMA-663): no `members` entry may carry a glob character. A glob also matches a
    # dot-directory, and `napi build` stages its output in `.<crate>.napi-stage-<random>` beside
    # the crate, with no Cargo.toml in it. A concurrent `cargo metadata` then exits 101.
    with tempfile.TemporaryDirectory() as tmp:
        root11 = Path(tmp)
        rs11 = root11 / "rs"
        rs11.mkdir()

        def write_members11(entries):
            (rs11 / "Cargo.toml").write_text(
                f"""[workspace]\nmembers = [{", ".join(f'"{e}"' for e in entries)}]\n"""
            )

        # a: the SMA-663 shape itself.
        write_members11(["crates/bindings/*"])
        rows = check_member_literals(root11)
        if len(rows) != 1 or "crates/bindings/*" not in rows[0]:
            failures.append(f"A11 did not fire exactly once on `crates/bindings/*`: {rows}")

        # b: the fixed shape — every entry literal.
        write_members11(["crates/bindings/node", "crates/libs/kernel", "crates/services/iam"])
        if check_member_literals(root11):
            failures.append("A11 fired on a members list with only literal entries")

        # c: mixed — one literal, one `*`, one `[` class. Exactly the two globs fire.
        write_members11(["crates/libs/kernel", "crates/services/*", "crates/x/[ab]"])
        rows = check_member_literals(root11)
        if len(rows) != 2 or not any("crates/services/*" in r for r in rows) \
                or not any("crates/x/[ab]" in r for r in rows):
            failures.append(f"A11 did not fire exactly on the `*` and `[` entries: {rows}")

        # d: `?` is a glob character too.
        write_members11(["crates/libs/paigasus-?"])
        if len(check_member_literals(root11)) != 1:
            failures.append("A11 did not fire on a `?` members entry")

        # Infra: the same two shapes as A9, through the shared reader.
        (rs11 / "Cargo.toml").write_text("[workspace]\nresolver = \"3\"\n")
        try:
            check_member_literals(root11)
            failures.append("A11 did not raise infra on a workspace with no `members` key")
        except MoonOutputError:
            pass
        (rs11 / "Cargo.toml").unlink()
        try:
            check_member_literals(root11)
            failures.append("A11 did not raise infra on a missing rs/Cargo.toml")
        except MoonOutputError:
            pass
```

- [ ] **Step 2: Run the self-test and see it fail**

```bash
/opt/homebrew/bin/python3 ci/affected-graph/cargo_moon_parity.py --self-test; echo "rc=$?"
```

Expected: a non-zero rc with `NameError: name 'check_member_literals' is not defined`.

- [ ] **Step 3: Extract the reader and add `check_member_literals`**

In `check_member_globs`, replace the block from `path = root / "rs" / "Cargo.toml"` down to the second `raise MoonOutputError(...)` (inclusive) with:

```python
    members = read_workspace_members(root, "A9")
```

Add this function directly BEFORE `check_member_globs`. The messages keep the A9 wording, with the label passed in:

```python
def read_workspace_members(root, assertion):
    """Return rs/Cargo.toml's `[workspace] members` list, for A9 and A11.

    An absent manifest or an absent/empty `members` key is infrastructure, never a silent pass:
    both would make the assertion vacuous while it still prints PASS.
    """
    path = root / "rs" / "Cargo.toml"
    if not path.is_file():
        raise MoonOutputError(
            f"{path} is absent — {assertion}'s members assertion cannot be evaluated. If the "
            f"workspace root legitimately moved, update read_workspace_members rather than "
            f"deleting the check"
        )
    members = (tomllib.loads(path.read_text()).get("workspace") or {}).get("members")
    if not members:
        raise MoonOutputError(
            f"{path} declares no `[workspace] members` — {assertion} cannot examine a member "
            f"set that does not exist"
        )
    return members
```

Add this function directly AFTER `check_member_globs`:

```python
MEMBER_GLOB_CHARS = ("*", "?", "[")


def check_member_literals(root):
    """Return the A11 violation list: `[workspace] members` entries that carry a glob (SMA-663).

    A glob also matches a dot-directory. `napi build` (@napi-rs/cli 3.10.3, dist/cli.js:10350)
    stages every output in `mkdtemp(dirname(outputDir) + "/." + basename(outputDir) +
    ".napi-stage-")`, a sibling of the crate with no Cargo.toml in it. While it exists, every
    concurrent `cargo metadata` fails with exit 101 on the missing manifest. `moon ci` runs the
    two napi tasks in parallel with the crate tasks, so the race reached 9 of 9 runs on SMA-658.

    The rule is deliberately blunt. A finer rule ("no glob may match a napi staging path") has
    to model how napi resolves its crate from `--cwd`, `--manifest-path`, `-p` and `-o`, and a
    wrong model passes in silence. A literal list cannot drift: A9 reds on a crate directory
    that no entry reaches.
    """
    return [
        f"members entry {entry!r} carries a glob character — it also matches a dot-directory "
        f"such as napi's `.<crate>.napi-stage-<random>`, and a concurrent `cargo metadata` then "
        f"fails on its missing Cargo.toml"
        for entry in read_workspace_members(root, "A11")
        if any(ch in entry for ch in MEMBER_GLOB_CHARS)
    ]
```

- [ ] **Step 4: Wire A11 into the findings list**

Change `EXPECTED_FINDING_KEYS` to:

```python
EXPECTED_FINDING_KEYS = ("a1", "a2", "a3", "a4-lint", "a4-fmt", "a5", "a6", "a7", "a8", "a9", "a10", "a11")
```

In `collect_findings`, add this tuple directly after the `("a10", …)` tuple, before the closing `]`:

```python
        ("a11", check_member_literals(root),
             "A `[workspace] members` entry in rs/Cargo.toml carries a glob character. A glob\n"
             "    also matches a dot-directory, and `napi build` stages its output in\n"
             "    `.<crate>.napi-stage-<random>` beside the crate with no Cargo.toml in it, so\n"
             "    every concurrent `cargo metadata` fails with exit 101 (SMA-663).\n"
             "    Fix: list each crate directory as its own literal entry. A9 reds if one is\n"
             "    missing."),
```

In `main()`, change the end of the PASS text from

```python
            f"every compiling cargo task inside rs/ keys on .cargo/config.toml"
```

to

```python
            f"every compiling cargo task inside rs/ keys on .cargo/config.toml, and every "
            f"workspace members entry is a literal path"
```

In the module docstring, find the paragraph that starts `# It also carries A9 (SMA-604)` (about line 25). Add after that paragraph one comment paragraph in the same style:

```python
# A11 (SMA-663) reads the same `members` list and reds on any glob character in it, because a
# glob also matches napi's `.<crate>.napi-stage-<random>` staging directory.
```

- [ ] **Step 5: Run the self-test and see it pass**

```bash
/opt/homebrew/bin/python3 ci/affected-graph/cargo_moon_parity.py --self-test; echo "rc=$?"
```

Expected: `rc=0`. The arity row now expects 12 keys. It passes because the arity fixture writes literal members derived from `crates`.

- [ ] **Step 6: Prove that the self-test rows bite (mutation)**

Change the body of `check_member_literals` temporarily to `return []`. Run the self-test again.

Expected: a non-zero rc with the rows `A11 did not fire exactly once`, `A11 did not fire exactly on the`, and `A11 did not fire on a \`?\``. Restore the body with the Edit tool (not with `git checkout`, which would also revert the uncommitted work). Run the self-test again and expect `rc=0`.

- [ ] **Step 7: Prove the production call site (mutation on the real tree)**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
/opt/homebrew/bin/python3 ci/affected-graph/cargo_moon_parity.py; echo "rc=$?"
```

Expected: `rc=0` and a `PASS` line that ends with `every workspace members entry is a literal path`.

Then edit `rs/Cargo.toml` temporarily: replace the three `"crates/bindings/paigasus-…"` lines with the single entry `"crates/bindings/*",`. Run the same command.

Expected: `rc=1`, and stderr lists the A11 title with one row for `'crates/bindings/*'`. Restore `rs/Cargo.toml` with the Edit tool to the three literal binding lines. Run `git diff --exit-code -- rs/Cargo.toml` and expect rc 0. Run the command again and expect `rc=0`.

- [ ] **Step 8: Update the README**

In `ci/affected-graph/README.md`:

1. In the A9 bullet (about lines 220-244), replace the sentence

   `**Adding a crate directory** (a fourth sibling of `libs`/`services`/`bindings`) needs a new `members` entry; adding a crate inside an existing one does not.`

   with

   `Each `members` entry is a literal crate path since SMA-663, so **every new crate** needs its own `members` line. `cargo new` inside the workspace adds it. If one is missing, A9 reds with a "never reaches" row.`

2. Directly after the A9 bullet and before the A10 bullet, add:

   ```markdown
   - **A11** (`check_member_literals` in `cargo_moon_parity.py`, SMA-663, findings key `a11`) reds
     on any `[workspace] members` entry in `rs/Cargo.toml` that carries a glob character (`*`, `?`
     or `[`). A glob also matches a dot-directory. `napi build` (@napi-rs/cli 3.10.3) stages its
     output in `.<crate>.napi-stage-<random>` beside the crate, and that directory has no
     `Cargo.toml`. While it exists, every concurrent `cargo metadata` fails with exit 101, and
     `moon ci` runs the napi tasks in parallel with the crate tasks. The rule is blunt on purpose:
     a finer rule would have to model how napi resolves its crate from `--cwd`,
     `--manifest-path`, `-p` and `-o`, and a wrong model passes in silence. A9 and A11 share
     `read_workspace_members`, so both raise infra on a missing file or an empty list.
   ```

3. Change `A1-A10` to `A1-A11` (about line 340).

- [ ] **Step 9: Run the full gate the way CI does**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:affected-smoke --force; echo "rc=$?"
```

Expected: `rc=0`. NOTE: on this Mac `repo:affected-smoke` needs system bash 3.2, and Homebrew bash 5.3 can deadlock on a here-string (see root `CLAUDE.md`, "This development Mac only"). If the task hangs at about 0% CPU, stop it and run `/bin/bash ci/affected-graph/run.sh --negative-control && /bin/bash ci/affected-graph/run.sh` directly, with `/opt/homebrew/bin` before `/usr/bin` in `PATH` so that `python3` is 3.14. Report which form ran.

- [ ] **Step 10: Commit**

```bash
git add ci/affected-graph/cargo_moon_parity.py ci/affected-graph/README.md
git commit -m "feat(ci): assert every Cargo workspace member is a literal path (SMA-663)

A11 in repo:affected-smoke reds on a glob character in rs/Cargo.toml
members. A glob also matches the napi staging dot-directory, which
made concurrent cargo metadata calls fail in moon ci.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 3: Forced-overlap proof (spec section 5, P1 to P3)

This task makes measurements. It commits only the spec's section 9.

**Files:**
- Create (scratch, not committed): `$SCRATCH/p2-race.sh`, where `SCRATCH=/private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/5090252d-457d-40a5-b403-9a290ed2fa45/scratchpad`
- Modify: `docs/superpowers/specs/2026-09-21-sma-663-napi-stage-race-design.md` (section 9 only)

**Interfaces:**
- Consumes: Task 1's literal `members`. Task 2 is not needed.
- Produces: the numbers for spec section 9 and the PR description.

- [ ] **Step 1: P1, the deterministic probe, including Moon**

For each of the two `rs/Cargo.toml` forms: first the `origin/main` form, then the branch form.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cp rs/Cargo.toml "$SCRATCH/Cargo.toml.branch"
git show origin/main:rs/Cargo.toml > rs/Cargo.toml          # main form
mkdir rs/crates/bindings/.paigasus-node-bindings.napi-stage-PROBE
cargo metadata --manifest-path rs/Cargo.toml --format-version=1 --locked >/dev/null 2>"$SCRATCH/p1-main.err"; echo "main rc=$?"
moon query projects > "$SCRATCH/p1-moon-main.json" 2>"$SCRATCH/p1-moon-main.err"; echo "moon rc=$?"
grep -c napi-stage "$SCRATCH/p1-moon-main.json" "$SCRATCH/p1-moon-main.err"
cp "$SCRATCH/Cargo.toml.branch" rs/Cargo.toml && touch rs/Cargo.toml   # branch form
cargo metadata --manifest-path rs/Cargo.toml --format-version=1 --locked >/dev/null; echo "branch rc=$?"
rmdir rs/crates/bindings/.paigasus-node-bindings.napi-stage-PROBE
git diff --exit-code -- rs/Cargo.toml; echo "restored-rc=$?"
```

Expected: `main rc=101`, `branch rc=0`, `restored-rc=0`. Record the `moon rc` and the two `grep -c` counts.

**Decision point.** If `moon rc` is not 0, or a count is not 0, Moon also sees the dot-directory. Then STOP and report to the controller. The spec says the three `rs/crates/*/*` Moon project globs in `.moon/workspace.yml` then become literal in this PR, and that needs its own task.

- [ ] **Step 2: Write the P2 script**

Create `$SCRATCH/p2-race.sh`:

```bash
#!/bin/bash
# SMA-663 P2: force napi staging to overlap with concurrent `cargo metadata`.
# Usage: N=200 B=3 OUT=<dir> p2-race.sh <repo-root>
set -u
ROOT=$1
N=${N:-200}
B=${B:-3}
OUT=${OUT:?set OUT}
rm -rf "$OUT"; mkdir -p "$OUT"
STOP="$OUT/stop"

b_loop() {
  local id=$1 calls=0 fails=0 rc
  while [ ! -e "$STOP" ]; do
    calls=$((calls + 1))
    cargo metadata --manifest-path "$ROOT/rs/Cargo.toml" --format-version=1 --locked \
      >/dev/null 2>"$OUT/b$id.last.err"
    rc=$?
    if [ "$rc" -ne 0 ]; then
      fails=$((fails + 1))
      { echo "--- call=$calls rc=$rc"; cat "$OUT/b$id.last.err"; } >> "$OUT/b$id.fail.log"
    fi
  done
  echo "$calls $fails" > "$OUT/b$id.summary"
}

for i in $(seq 1 "$B"); do b_loop "$i" & done

napi_fail=0
for k in $(seq 1 "$N"); do
  pnpm -C "$ROOT/ts/packages/paigasus-kernel" exec napi build --platform \
    --cwd ../../../rs/crates/bindings/paigasus-node-bindings >/dev/null 2>>"$OUT/napi.err" \
    || napi_fail=$((napi_fail + 1))
done
touch "$STOP"
wait

calls=0; fails=0
for i in $(seq 1 "$B"); do
  read -r c f < "$OUT/b$i.summary"; calls=$((calls + c)); fails=$((fails + f))
done
stage=$(cat "$OUT"/b*.fail.log 2>/dev/null | grep -c 'napi-stage' || true)
echo "N=$N B=$B napi_fail=$napi_fail b_calls=$calls b_nonzero=$fails b_napi_stage_lines=$stage"
```

- [ ] **Step 3: Warm the build, then run P2 on the main form**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts/packages/paigasus-kernel exec napi build --platform --cwd ../../../rs/crates/bindings/paigasus-node-bindings >/dev/null
git show origin/main:rs/Cargo.toml > rs/Cargo.toml
N=200 B=3 OUT="$SCRATCH/p2-main" /bin/bash "$SCRATCH/p2-race.sh" "$PWD"
cp "$SCRATCH/Cargo.toml.branch" rs/Cargo.toml && touch rs/Cargo.toml
git diff --exit-code -- rs/Cargo.toml; echo "restored-rc=$?"
```

Run it in the background, because 200 builds take several minutes. Expected: `b_napi_stage_lines` of at least 10, and `restored-rc=0`.

- [ ] **Step 4: Run P2 on the branch form**

```bash
N=200 B=3 OUT="$SCRATCH/p2-branch" /bin/bash "$SCRATCH/p2-race.sh" "$PWD"
```

Expected: `b_nonzero=0`. If it is not 0, read `$SCRATCH/p2-branch/b*.fail.log` and report every distinct error. Do not count only the napi-stage errors.

- [ ] **Step 5: P3, only if step 3 gave fewer than 10 napi-stage lines**

Report to the controller before you start P3. P3 runs the same script in a Linux container (Docker 29.8.0 is installed). It needs its own design for the toolchain inside the container, so the controller decides it.

- [ ] **Step 6: Record the results in spec section 9**

Replace the text under `## 9. Measurements` with:

- the host (`uname -srm`), the napi version (`pnpm -C ts/packages/paigasus-kernel exec napi --version`) and the cargo version;
- a P1 table: main rc, branch rc, moon rc, and the two grep counts;
- a P2 table: for main and branch, the `N`, `B`, `napi_fail`, `b_calls`, `b_nonzero` and `b_napi_stage_lines` values;
- the full text of `p2-race.sh` in a fenced block;
- the line `P4: to be filled with the PR's moon ci run ID.`

- [ ] **Step 7: Commit**

```bash
git add docs/superpowers/specs/2026-09-21-sma-663-napi-stage-race-design.md
git commit -m "docs(repo): record the forced-overlap measurements for the napi staging race (SMA-663)

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 4: Record the mechanism and the fix in `CLAUDE.md`

**Files:**
- Modify: `CLAUDE.md` (root), the first bullet under `### Diagnosing an unattributed \`moon ci\` failure`
- Modify: `rs/CLAUDE.md`, the `members` bullet (the bullet that starts `` `rs/Cargo.toml`'s `[workspace] members` entries must carry``)

**Interfaces:**
- Consumes: Task 2's A11 name and Task 3's numbers.
- Produces: documentation only.

- [ ] **Step 1: Replace the root `CLAUDE.md` bullet**

Replace the whole first bullet under `### Diagnosing an unattributed \`moon ci\` failure` (from `- \`paigasus-kernel-ts:build\`/\`:test\`` to `before assuming a real regression.`) with the text below. Put in the real P2 numbers from Task 3 where the angle brackets are:

```markdown
- **FIXED (SMA-663): a `cargo metadata` error that names a `.napi-stage-<random>` path under
  `rs/crates/bindings/`.** `napi build` (@napi-rs/cli 3.10.3) stages every output in a sibling
  directory `.<crate>.napi-stage-<random>` that has no `Cargo.toml`. The old `crates/bindings/*`
  members glob matched it, so every concurrent `cargo metadata` failed with exit 101 for the full
  life of that directory. Only `paigasus-kernel-ts:build` and `:test` stage; every other task in
  the error, `paigasus-kernel-py:test` included, was a victim. It reached 9 of 9 `moon ci` runs on
  SMA-658, and re-runs did not clear it. The fix: every `rs/Cargo.toml` `members` entry is a
  literal path, and `repo:affected-smoke`'s A11 reds on a glob character there. Measured with a
  forced overlap of 200 napi builds: <main count> failures with the glob, 0 with literal entries.
  **This error must not occur any more.** If it does, it is a new defect: check first for a glob
  back in `members`, or a new crate path outside the list.
```

Keep the text within the file's line width (100 characters). Do not touch the `moon-diagnosis` marker block below it.

- [ ] **Step 2: Update the `rs/CLAUDE.md` `members` bullet**

In that bullet:

1. Replace the sentence `Adding a crate DIRECTORY (a fourth sibling of \`libs\`/\`services\`/\`bindings\`) needs a new \`members\` entry; adding a crate inside an existing one does not.` with:

   `Since SMA-663 every entry is a LITERAL crate path, so **every new crate** needs its own \`members\` line (\`cargo new\` adds it; A9 reds if one is missing).`

2. Add at the end of the bullet:

   `**No glob at all (SMA-663).** A glob also matches a dot-directory, and \`napi build\` stages its output in \`.<crate>.napi-stage-<random>\` beside the crate, with no \`Cargo.toml\`. A concurrent \`cargo metadata\` then exits 101. \`repo:affected-smoke\`'s **A11** reds on any \`*\`, \`?\` or \`[\` in \`members\`.`

- [ ] **Step 3: Check that the gated blocks did not change**

```bash
git diff origin/main -- CLAUDE.md | grep -nE '^[-+].*(ci-targets|moon-diagnosis)' ; echo "marker-lines=$?"
grep -c 'ci-targets:begin' CLAUDE.md
```

Expected: `marker-lines=1` (no changed marker line) and `1`.

- [ ] **Step 4: Commit**

```bash
git add CLAUDE.md rs/CLAUDE.md
git commit -m "docs(repo): record the napi staging race mechanism and its fix (SMA-663)

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Controller-only steps (not for a subagent)

- After Task 4: update the auto-memory file `moon-ci-napi-stage-race.md` and its `MEMORY.md` line. The memory directory is outside the repo.
- Before the PR: run the full gate graph from the root `CLAUDE.md` (`ci-targets` block) as far as this Mac allows, per the "This development Mac only" rules.
- File the follow-up Linear issue from spec section 10 (napi `--locked` passthrough).
- After the PR's `moon ci` run: put its run ID into spec section 9 (P4).
