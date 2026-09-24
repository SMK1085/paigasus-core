# SMA-680 release PR wasm glue — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A release PR no longer runs a full `cargo update`, so it cannot make the committed wasm glue stale. Measurements prove it before the merge.

**Architecture:** One config key in `rs/release-plz.toml` changes from `true` to `false`. Four wrong comments are rewritten. One note is added above `generate-wasm`. Three measurements (P1, M1, M2) prove the change with the pinned release-plz 0.3.158 and record the results in a measurements file.

**Tech Stack:** release-plz 0.3.158 (proto-pinned), Cargo, Moon 2.5.3, wasm-pack, vitest.

**Spec:** `docs/superpowers/specs/2026-09-24-sma-680-release-pr-wasm-glue-design.md`

> **Execution note (read before you re-run this plan).** The M1 and M2 steps below are the
> ORIGINAL plan, and both were void as written. At `028cdd20` release-plz proposed no bump, so
> the key never acted. At the branch head a README-only `fix(rs):` commit gave release-plz no
> commits for the kernel. The procedures that ran are in
> `docs/superpowers/specs/2026-09-24-sma-680-measurements.md`. M1 attempt 2 added a forced
> kernel-bump commit and committed the key edit. M2 attempt 2 bumped the four kernel-group
> manifests by hand and ran `cargo update --workspace`, so it is lockfile-scope evidence only.
> Every acceptance test run recorded there used `--force`, and its full summary line is recorded.
> The `| tail` pipelines below do not keep `moon`'s exit status; use `set -o pipefail` if you
> re-run them.

## Global Constraints

- Worktree: `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-680-release-wasm-glue`, branch `feature/sma-680-release-wasm-glue`. Run `git branch --show-current` before every commit. It must print `feature/sma-680-release-wasm-glue`.
- Every Bash call starts with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` and `export PROTO_REPORTER=text`.
- Scratch clones go under `/private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/548bd898-d3c7-4161-b0e2-6b21da2dad76/scratchpad/sma-680/`. Never run a measurement in the worktree itself.
- Commits: conventional, lower-case subject, scope `rs` or `repo`, subject ends with `(SMA-680)`. Body ends with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. No `#NNN` and no `token: value` line in the body (commitlint `footer-leading-blank`).
- Never use `git commit --amend`, `git reset HEAD~`, `git stash`, or `--no-verify` in the worktree.
- Do not start background jobs. Run each long command in the foreground with the Bash tool's `timeout: 600000`. If a build times out, run the same command again: cargo continues from its cache.
- Write all prose (comments, measurement notes) in ASD-STE100 Simplified Technical English: short sentences, active voice, no idiom.
- Every source file keeps its SPDX header. Markdown files need none.
- Record every measurement as MEASURED (you ran it), READ (you read source or docs) or INFERRED. Do not state a result that you did not observe.

## Review Focus

1. **The cascade with `false`.** A kernel-group bump must still bump all four kernel-group crates (the cascade is unconditional, spec F2). M2 step 4 records each version.
2. **The real stamp step after release-plz.** `ci/version-lockstep/run.sh --write` runs its own `cargo update -w` after release-plz. The lock must still move only workspace entries after it. M2 step 5 runs it.
3. **A proto-only release PR.** At `028cdd20` the pending release was proto-only. M1 run A covers that shape.
4. **A void negative control.** If run B does not move `wasm-bindgen`, M1 proves nothing. M1 step 7 checks that and stops.
5. **The parity fixture.** The key moves out of the classification block. `repo:release-parity` must stay green. Task 2 step 6 runs it.

---

### Task 1: P1 and M1 — read the key's readers, replay the v0.2.0 incident

**Files:**
- Create: `docs/superpowers/specs/2026-09-24-sma-680-measurements.md`

**Interfaces:**
- Produces: the measurements file with sections `## P1`, `## M1`. Tasks 3 and 4 append `## M2` and `## Gates`.

- [ ] **Step 1: Get the release-plz source at the pinned tag**

```bash
S=/private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/548bd898-d3c7-4161-b0e2-6b21da2dad76/scratchpad/sma-680
mkdir -p "$S/rp" && gh api repos/release-plz/release-plz/tarball/release-plz-v0.3.158 > "$S/rp.tgz" && tar -xzf "$S/rp.tgz" -C "$S/rp" --strip-components=1 && ls "$S/rp/crates"
```
Expected: the list holds `release_plz_core` and `release_plz`.

- [ ] **Step 2: P1 — find every reader of the key**

```bash
grep -rn "dependencies_update\|update_dependencies\b\|should_update_dependencies\|update_all_dependencies" "$S/rp/crates/release_plz_core/src" "$S/rp/crates/release_plz/src"
```
Expected: the key is parsed in the config and request code and reaches exactly one use: `update_cargo_lock(root, update_all_dependencies)` in `release_plz_core/src/command/update/mod.rs`. Record each hit (file:line, one line of context). If a second use exists, STOP and report it: the spec's F1 is then wrong.

Then read the function that calls `update_cargo_lock` and answer: does `release-plz update` call it when it proposes no version change? Record the answer with file:line.

- [ ] **Step 3: M1 — make the scratch clone at the pre-incident commit**

```bash
git clone -q /Users/smaschek/dev/paigasus/paigasus-core "$S/m1" && cd "$S/m1" && git checkout -q -B m1-base 028cdd20 && grep -A1 '^name = "wasm-bindgen"$' rs/Cargo.lock && grep -o '__wbg_Error_[0-9a-f]*' rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.js | head -1 && grep -n '^dependencies_update' rs/release-plz.toml
```
Expected: `version = "0.2.127"`, `__wbg_Error_408e67f47ca7b58b`, `dependencies_update = true`. If any differs, STOP: the base is wrong.

- [ ] **Step 4: Provision the clone and warm the build**

```bash
cd "$S/m1" && proto install >/dev/null && pnpm -C ts install --frozen-lockfile >/dev/null && moon run paigasus-kernel-ts:build
```
Expected: exit 0. This only fills the caches. Run it again if it times out.

- [ ] **Step 5: M1 run A (`false`)**

```bash
cd "$S/m1" && sed -i '' 's/^dependencies_update = true$/dependencies_update = false/' rs/release-plz.toml && grep -n '^dependencies_update' rs/release-plz.toml && cp rs/Cargo.lock "$S/lock-base" && (cd rs && release-plz update) && cp rs/Cargo.lock "$S/lock-A"
```
Pass no `--update-deps` flag. Expected: `dependencies_update = false`, then release-plz output, exit 0.

Compare the locks with this script. Save it once as `$S/lockdiff.py` and use it for every run:

```python
# SPDX-License-Identifier: Apache-2.0
import sys, tomllib
from collections import defaultdict
def load(p):
    # (name, source) -> sorted versions. A crate can be locked at two versions at once.
    out = defaultdict(list)
    with open(p, "rb") as f:
        for x in tomllib.load(f)["package"]:
            out[(x["name"], x.get("source", "workspace"))].append(x["version"])
    return {k: sorted(v) for k, v in out.items()}
a, b = load(sys.argv[1]), load(sys.argv[2])
for key in sorted(set(a) | set(b)):
    if a.get(key) != b.get(key):
        kind = "workspace" if key[1] == "workspace" else "third-party"
        print(f"{kind}\t{key[0]}\t{a.get(key)} -> {b.get(key)}")
```

```bash
python3 "$S/lockdiff.py" "$S/lock-base" "$S/lock-A"
```
Expected: only `workspace` lines, or no lines. Record the output. `wasm-bindgen` must stay at 0.2.127.

Then test the committed glue:

```bash
cd "$S/m1" && moon run paigasus-kernel-ts:test 2>&1 | tail -25
```
Expected: PASS. Record the vitest summary lines.

- [ ] **Step 6: M1 run B (`true`, the negative control)**

```bash
cd "$S/m1" && git reset -q --hard && git clean -qfdx -e target -e node_modules && grep -n '^dependencies_update' rs/release-plz.toml && (cd rs && release-plz update) && cp rs/Cargo.lock "$S/lock-B" && python3 "$S/lockdiff.py" "$S/lock-base" "$S/lock-B" | grep -E 'wasm-bindgen|^workspace' ; python3 "$S/lockdiff.py" "$S/lock-base" "$S/lock-B" | wc -l
```
Expected: `dependencies_update = true`; `wasm-bindgen` moves to a version above 0.2.127; the line count is much larger than in run A.

```bash
cd "$S/m1" && moon run paigasus-kernel-ts:test 2>&1 | grep -E "check 1|FAIL|✓|×|Tests" | head -20
```
Expected: FAIL, with at least one "check 1: the committed paigasus_wasm_bg.js equals the fresh build" failure. Record the failing test names.

- [ ] **Step 7: Void check**

If `wasm-bindgen` has the same version in `$S/lock-A` and `$S/lock-B`, M1 is VOID. Write that in the file and STOP. Report to the controller. Do not continue to Task 2.

- [ ] **Step 8: Write the measurements file and commit**

Create `docs/superpowers/specs/2026-09-24-sma-680-measurements.md` in the WORKTREE (not in the clone). Structure:

```markdown
# SMA-680 — measurements

Spec: `2026-09-24-sma-680-release-pr-wasm-glue-design.md`. Host: macOS (this development Mac),
2026-09-24. release-plz 0.3.158 (proto-pinned).

## P1 — the readers of `dependencies_update` (READ)

<each grep hit: file:line and the line>
<answer: does `update` call `update_cargo_lock` with no version change? file:line>

## M1 — replay of the v0.2.0 incident (MEASURED)

Base `028cdd20`: wasm-bindgen 0.2.127, glue hash `__wbg_Error_408e67f47ca7b58b`.

| Run | Key | wasm-bindgen after | Workspace entries moved | Third-party entries moved | `paigasus-kernel-ts:test` |
|---|---|---|---|---|---|
| A | false | … | … | … | … |
| B | true | … | … | … | … |

<the lockdiff output of run A, verbatim>
<the failing test names of run B, verbatim>
Verdict: <valid / void, and why>
```

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-680-release-wasm-glue && git branch --show-current && git add docs/superpowers/specs/2026-09-24-sma-680-measurements.md && git commit -m "docs(repo): measure the release PR lockfile scope (SMA-680)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: set the key to `false` and correct the comments

**Files:**
- Modify: `rs/release-plz.toml:8-22` and `rs/release-plz.toml:102-105`
- Modify: `.github/CLAUDE.md:9-14`
- Modify: `ts/packages/paigasus-kernel/moon.yml` (the comment block above `generate-wasm:`, near line 185-207)

**Interfaces:**
- Consumes: nothing from Task 1.
- Produces: `rs/release-plz.toml` with `[workspace] dependencies_update = false`. Task 3 clones this branch.

- [ ] **Step 1: Write the check that must fail now**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-680-release-wasm-glue && python3 -c "import tomllib;v=tomllib.load(open('rs/release-plz.toml','rb'))['workspace']['dependencies_update'];print(v);assert v is False" ; git grep -n "dependencies_update = true\|not tagging\|permanently tag\|dependencies_update cascade\|stops \`dependencies_update\`" -- rs/release-plz.toml .github/CLAUDE.md
```
Expected now: `True`, an `AssertionError`, and grep hits in both files.

- [ ] **Step 2: Edit `rs/release-plz.toml`, lines 11-12**

Replace:
```toml
# "no public packages found", so a release-PR job cannot run under it. Releasability is now
# declared per package below — which is also what stops `dependencies_update` from cascading
# tags into crates nobody intended to release (spec §8).
```
with:
```toml
# "no public packages found", so a release-PR job cannot run under it. Releasability is now
# declared per package below. A per-package `release = false` keeps a crate out of the
# release-PR proposal, so the "dependencies changed" cascade cannot bump it (spec §8). The
# cascade itself is unconditional — see the `dependencies_update` note below.
```

- [ ] **Step 3: Edit `rs/release-plz.toml`, lines 16-22**

Replace from `features_always_increment_minor = true` through `dependencies_update = true` (the "Kept ON deliberately" comment and the key) with:

```toml
features_always_increment_minor = true

# --- Lockfile scope of the release PR (SMA-680) --------------------------------------------
# NOT a classification key. The SMA-398 parity harness does not read it:
# ci/release-parity/ecosystems/release-plz.sh derives only `features_always_increment_minor`.
#
# WHAT IT DOES. Read from the release-plz 0.3.158 source, not measured. Read it again when
# .prototools moves the release-plz pin. Its only reader is `update_cargo_lock`
# (release_plz_core, src/command/update/mod.rs:142-158). `true` runs `cargo update`, which
# moves every third-party entry of rs/Cargo.lock. `false` (the release-plz default) runs
# `cargo update --workspace`, which rewrites only the workspace members' entries. That holds
# only while the lock satisfies the manifests; ci/cargo-lock-integrity enforces that on main.
#
# WHAT IT DOES NOT DO. It does not control the "dependencies changed" cascade, where a
# dependent is bumped because a workspace dependency of it was bumped. `update_dependencies`
# (mod.rs:202-232) and `add_dependencies_update_if_any` (updater.rs:749-783) never read this
# key. An earlier comment here said the opposite. It was wrong.
#
# WHY FALSE (SMA-680):
#   1. The wasm glue under rs/crates/bindings/paigasus-wasm/ is committed (SMA-634), and no
#      release step regenerates it. A full `cargo update` that moved wasm-bindgen made that glue
#      stale, and the v0.2.0 release PR failed committed-wasm.test.ts.
#   2. The release-pr job's stamp step builds with a write-capable App token in its environment
#      (release.yml, GH_TOKEN_FOR_PUSH). With `true`, third-party build scripts that no person
#      reviewed ran there.
# Setting it to `true` again CAN make a release PR fail committed-wasm.test.ts, on a day when the
# registry has a newer compatible wasm-bindgen. That test's message then asks for a manual
# `generate-wasm` commit. The real cause is this key. Third-party Rust updates come only from
# dependabot's cargo group.
#
# `false` is not enough if a crate version ever enters the glue. See the note above
# `generate-wasm` in ts/packages/paigasus-kernel/moon.yml.
dependencies_update = false
```

- [ ] **Step 4: Edit `rs/release-plz.toml`, the "Not releasable" block**

Replace:
```toml
# Every other member. `release = false` removes a package from the release-PR proposal
# ENTIRELY (measured: it is neither bumped nor listed), which is what keeps the
# dependencies_update cascade from bumping and permanently tagging crates nobody released.
```
with:
```toml
# Every other member. `release = false` removes a package from the release-PR proposal
# ENTIRELY (measured: it is neither bumped nor listed), which is what keeps the
# "dependencies changed" cascade from bumping crates nobody released. None of these crates can
# get a tag: release-plz tags only what it publishes (see the TAGS note above).
```

- [ ] **Step 5a: Edit `.github/CLAUDE.md`, lines 9-16**

Line 16 reads `  one explicitly. \`paigasus-gateway\` / \`paigasus-iam\` are versioned BY HAND (SMA-658, option`.
The replacement below ends at "one explicitly." so the rest of line 16 stays attached to the
first bullet. Replace:
```markdown
- `rs/release-plz.toml` declares releasability **per package**, never workspace-wide. A
  `[workspace] release = false` makes release-plz hard-error (`no public packages found`), and
  simply deleting it is worse: `dependencies_update = true` cascades a patch bump into every
  transitive dependent — a crate neither in the version group nor touched by the commit still
  gets bumped ("dependencies changed") — and Cargo's `publish = false` suppresses publishing but
  **not tagging**, so the first release would permanently tag most of the workspace. Per-package
  `release = false` removes a package from the proposal entirely; every non-family crate needs
  one explicitly.
```
with:
```markdown
- `rs/release-plz.toml` declares releasability **per package**, never workspace-wide. A
  `[workspace] release = false` makes release-plz hard-error (`no public packages found`). The
  "dependencies changed" cascade is unconditional (read from the 0.3.158 source, not measured):
  a dependent of a bumped crate is bumped too, and `dependencies_update` does not control it. It
  reaches only the packages that release-plz processes. The fixture measurement that
  `rs/release-plz.toml` records used publishable crates: a crate neither in the version group nor
  touched by the commit was still bumped. A `publish = false` crate outside a group with a
  publishable head is not processed (M7, below), and release-plz tags only what it publishes
  (SMA-580, below). Per-package `release = false` removes a package from the proposal entirely;
  every non-family crate needs one explicitly.
```
Re-wrap nothing after it: line 16's remainder (` \`paigasus-gateway\` / …`) now follows
"one explicitly." on the same line, as before.

- [ ] **Step 5b: Add the `dependencies_update` bullet to `.github/CLAUDE.md`**

Insert a new bullet directly AFTER the line `  parked on that value (SMA-505 R7).`, which ends the first bullet:

```markdown
- `dependencies_update` is `false` since SMA-680. `true` runs a full `cargo update` in the release
  PR. That made the committed wasm glue stale on v0.2.0, and it ran unreviewed third-party build
  scripts in the stamp step, which holds a write-capable token. `false` runs
  `cargo update --workspace`. The reasons and the source lines are in the key's comment.
```

- [ ] **Step 6: Add the F6 note in `ts/packages/paigasus-kernel/moon.yml`**

Insert these lines directly above the line `  # runInCI: false — CI never regenerates; it only compares. cache: false — the task writes tracked`:

```yaml
  # WHEN A CRATE VERSION ENTERS THE GLUE (SMA-680 spec F5/F6). wasm-bindgen hashes the compiling
  # crate's CARGO_PKG_NAME and CARGO_PKG_VERSION into shim names. Export names are restored, and
  # import names keep the hash. Today the only hashed import is `__wbg_Error_*`, which wasm-bindgen
  # declares itself, so the glue follows the wasm-bindgen version and not ours. If the kernel or
  # paigasus-wasm gets its own `#[wasm_bindgen] extern "C"` block, `inline_js` or `module = "..."`,
  # our version enters an import name. Then every kernel-group release PR fails check 1 of
  # tests/committed-wasm.test.ts, also with `dependencies_update = false` in rs/release-plz.toml.
  #
```

- [ ] **Step 7: Run the checks**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-680-release-wasm-glue && python3 -c "import tomllib;c=tomllib.load(open('rs/release-plz.toml','rb'));v=c['workspace']['dependencies_update'];assert v is False;assert c['workspace']['features_always_increment_minor'] is True;print('ok',len(c['package']))" && git grep -n "dependencies_update = true\|not tagging\|permanently tag\|dependencies_update cascade\|stops \`dependencies_update\`" -- rs/release-plz.toml .github/CLAUDE.md ; echo "grep rc=$?" && moon query projects >/dev/null && echo "moon config ok" && moon run repo:release-parity 2>&1 | tail -15
```
Expected: `ok 13`, `grep rc=1` (no hits), `moon config ok`, and `repo:release-parity` passes. If `repo:release-parity` fails, read `.moon/cache/states/repo/release-parity/stdout.log` and `stderr.log` before anything else. It needs proto's text reporter (already exported).

- [ ] **Step 8: Commit**

```bash
git branch --show-current && git add rs/release-plz.toml .github/CLAUDE.md ts/packages/paigasus-kernel/moon.yml && git commit -m "fix(rs): stop the release PR from running a full cargo update (SMA-680)

release-plz ran a full cargo update in every release PR. On v0.2.0 that
moved wasm-bindgen, made the committed wasm glue stale, and failed
committed-wasm.test.ts. With dependencies_update = false, release-plz runs
cargo update --workspace, which moves only the workspace entries.

Also correct four comments that said the key controls the dependencies-changed
cascade, or that a publish = false crate gets a tag.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: M2 — force a kernel-group bump on this branch

**Files:**
- Modify: `docs/superpowers/specs/2026-09-24-sma-680-measurements.md` (append `## M2`)

**Interfaces:**
- Consumes: Task 2's commit (the branch has `dependencies_update = false`). Task 1's `$S/lockdiff.py`.

- [ ] **Step 1: Clone this branch**

```bash
S=/private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/548bd898-d3c7-4161-b0e2-6b21da2dad76/scratchpad/sma-680
git clone -q /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-680-release-wasm-glue "$S/m2" && cd "$S/m2" && git checkout -q -B m2 origin/feature/sma-680-release-wasm-glue && grep -n '^dependencies_update' rs/release-plz.toml && grep -A1 '^name = "wasm-bindgen"$' rs/Cargo.lock
```
Expected: `dependencies_update = false`, `version = "0.2.128"`.

- [ ] **Step 2: Provision and warm**

```bash
cd "$S/m2" && proto install >/dev/null && pnpm -C ts install --frozen-lockfile >/dev/null && moon run paigasus-kernel-ts:build
```
Expected: exit 0.

- [ ] **Step 3: Add the scratch `fix(rs):` commit**

```bash
cd "$S/m2" && printf '\n' >> rs/crates/libs/paigasus-kernel/README.md && git add rs/crates/libs/paigasus-kernel/README.md && git commit -q -m "fix(rs): scratch readme edit for the sma-680 m2 measurement" && git log --oneline -1
```
`README.md` is in the crate's `include` list (`rs/crates/libs/paigasus-kernel/Cargo.toml:20`) and does not reach the wasm. This clone is throwaway. Never push it.

- [ ] **Step 4: Run release-plz and record the versions**

```bash
cd "$S/m2" && cp rs/Cargo.lock "$S/lock-m2-base" && (cd rs && release-plz update) && cp rs/Cargo.lock "$S/lock-m2" && python3 "$S/lockdiff.py" "$S/lock-m2-base" "$S/lock-m2" && grep -n '^version' rs/crates/libs/paigasus-kernel/Cargo.toml rs/crates/bindings/paigasus-py-bindings/Cargo.toml rs/crates/bindings/paigasus-node-bindings/Cargo.toml rs/crates/bindings/paigasus-wasm/Cargo.toml
```
Expected: only `workspace` lines. All four kernel-group crates show the same new version. **Void if** no kernel-group version changed: record VOID and STOP.

- [ ] **Step 5: Run the real stamp step's lockstep write**

```bash
cd "$S/m2" && /opt/homebrew/bin/bash ci/version-lockstep/run.sh --write 2>&1 | tail -10 && cp rs/Cargo.lock "$S/lock-m2-stamped" && python3 "$S/lockdiff.py" "$S/lock-m2-base" "$S/lock-m2-stamped"
```
Use Homebrew bash: `version-lockstep` needs bash 4+ on this Mac. Expected: exit 0, and still only `workspace` lines. If the script fails for a bash or pipe reason, record the error verbatim and continue: step 5 is extra evidence, not the acceptance check.

- [ ] **Step 6: Test the committed glue FIRST**

```bash
cd "$S/m2" && moon run paigasus-kernel-ts:test 2>&1 | tail -25
```
Expected: PASS. CI tests the committed files, so this must run before any regeneration.

- [ ] **Step 7: Regenerate and diff**

```bash
cd "$S/m2" && moon run paigasus-kernel-ts:generate-wasm 2>&1 | tail -5 && git status --short -- rs/crates/bindings/paigasus-wasm/
```
Expected: no line for `paigasus_wasm.js`, `paigasus_wasm_bg.js`, `paigasus_wasm.d.ts` or `paigasus_wasm_bg.wasm.d.ts`. A line for `paigasus_wasm_bg.wasm` is allowed: the binary holds the version (SMA-634 S3-2). Record the output verbatim.

- [ ] **Step 8: Append `## M2` to the measurements file and commit**

In the WORKTREE, append a section with: the scratch commit, the lockdiff output of steps 4 and 5, the four crate versions, the test summary of step 6, the `git status` output of step 7, and a verdict (valid / void).

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-680-release-wasm-glue && git branch --show-current && git add docs/superpowers/specs/2026-09-24-sma-680-measurements.md && git commit -m "docs(repo): measure a kernel-group bump with the workspace-only update (SMA-680)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4 (controller): full gate graph and follow-ups

- [ ] **Step 1: Run the full gate graph**

Run the command between the `ci-targets` markers in the root `CLAUDE.md`, with `--base origin/main`. Read any red with the moon-diagnosis procedure. Re-run bash-version-sensitive gates with the right bash, per the root `CLAUDE.md`. Append a `## Gates` section to the measurements file with each result and commit.

- [ ] **Step 2: Make the two follow-up Linear issues** (spec §7)

Issue A (the dependabot case) and issue B (the token scope in the stamp step). Project "Paigasus Polyglot", milestone "CI & Tooling", priority Medium. Relate both to SMA-680.

- [ ] **Step 3: After the merge** — update the auto-memory entry `release-pr-wasm-glue-drift.md` (spec §7).
