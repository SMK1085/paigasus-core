# SMA-683 wasm-bindgen freeze — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Record that dependabot cannot move `wasm-bindgen`, and give a person a runbook to move it on purpose.

**Architecture:** No code path changes. Three files get documentation: a comment in `.github/dependabot.yml`, a runbook bullet in `rs/CLAUDE.md` (plus a pointer fix in `rs/Cargo.toml`), and one sentence in the `REGENERATE` message of `committed-wasm.test.ts`. The Linear follow-up and the M0 comment are controller work after the tasks.

**Tech Stack:** YAML (dependabot v2 config), Markdown, TypeScript (vitest), Moon, Docker (for the schema check only).

**Spec:** `docs/superpowers/specs/2026-09-26-sma-683-wasm-bindgen-freeze-design.md`

## Global Constraints

- Work only in the worktree `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-683-wasm-bindgen-dependabot`, on branch `feature/sma-683-wasm-bindgen-dependabot`. Check `git branch --show-current` before the first edit.
- Prefix every shell with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- All prose is ASD-STE100 Simplified Technical English: short sentences (max 20 words for an instruction, 25 for a description), active voice, no idiom. Keep technical names exactly.
- Conventional commits with a workspace scope. End every commit message with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Never use `--no-verify`. Never `git commit --amend`, never `git reset`.
- Do not install software on the host (no `brew`, no `pip` on the host). Use a `docker run` container.
- Do not change the test logic of `committed-wasm.test.ts`, the checks, or `SURFACE_CHANGED`.
- No new dependabot group and no `ignore` entry (spec 4.1).
- Run every command in the foreground. Do not start background jobs.

## Review Focus

1. A YAML comment line in `dependabot.yml` that is not a comment (a missing `#`) makes the file invalid and stops updates for EVERY ecosystem. No CI gate reads this file (spec F8). Task 1 pins this with the schema check and a negative control.
2. The runbook's `cargo update` must run in `rs/`, not the repository root, or cargo finds no workspace. Task 2 pins the `( cd rs && … )` form with a grep.
3. The runbook's `git add … paigasus_wasm*` must stage the five artifacts and nothing else (no `.wasmpack-*-out` scratch directory). Task 2 pins this with `git ls-files`/`git check-ignore` on the scratch directories.
4. The dangling "dependency-bump runbook" reference in `rs/Cargo.toml` must point to a real location after the change. Task 2 pins it with a grep that must return zero hits for the old phrase.
5. `SURFACE_CHANGED` concatenates `REGENERATE`, so the new sentence also appears in the check-2 message. It must read correctly there. Task 3 pins it with a grep of both constants and a full test run.

---

### Task 1: The freeze comment in `.github/dependabot.yml`

**Files:**
- Modify: `.github/dependabot.yml:6-7` (insert a comment block directly above `  - package-ecosystem: cargo`, after the `# ---- Rust: Cargo workspace at rs/ ----` line)

**Interfaces:**
- Consumes: nothing.
- Produces: a comment that names `rs/CLAUDE.md` as the runbook location. Task 2 writes that runbook under the bullet title "The wasm-bindgen family does not move through dependabot".

- [ ] **Step 1: Prove the validator bites (negative control)**

Make a scratch copy with a bad key and validate it. It must FAIL.

```bash
S=/private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/46bb56e1-abfa-4d35-a935-faa6cf37f273/scratchpad/sma683-v1 && mkdir -p "$S" && { echo 'bogus-key: true'; cat .github/dependabot.yml; } > "$S/dependabot.yml"
docker run --rm -v "$S:/w" -w /w python:3.13-slim sh -c "pip install -q check-jsonschema && check-jsonschema --builtin-schema vendor.dependabot dependabot.yml"; echo "rc=$?"
```

Expected: a schema error that names `bogus-key`, and `rc=1`. If rc is 0, STOP and report: the validator does not bite, so step 4 proves nothing.

- [ ] **Step 2: Validate the unchanged file (baseline)**

```bash
docker run --rm -v "$PWD:/w" -w /w python:3.13-slim sh -c "pip install -q check-jsonschema && check-jsonschema --builtin-schema vendor.dependabot .github/dependabot.yml"; echo "rc=$?"
```

Expected: `ok -- validation done`, `rc=0`.

- [ ] **Step 3: Add the comment block**

Insert these lines directly after `  # ---- Rust: Cargo workspace at rs/ ----` and before `  - package-ecosystem: cargo`:

```yaml
  # SMA-683: this entry cannot move wasm-bindgen. js-sys, web-sys and wasm-bindgen-futures pin it
  # with `=`, and dependabot updates one package at a time. `cargo update -p wasm-bindgen` then
  # locks 0 packages (MEASURED, spec M0). Since SMA-680 the release PR does not move it either, so
  # wasm-bindgen stays frozen until a person moves the whole family. The runbook is in
  # rs/CLAUDE.md, "The wasm-bindgen family does not move through dependabot".
  # One exception (INFERRED): a new reqwest can need newer wasm crates. Then a cargo-minor-patch
  # PR moves wasm-bindgen and fails committed-wasm.test.ts. The same runbook covers that PR.
```

- [ ] **Step 4: Validate the changed file**

Run the command of step 2 again. Expected: `ok -- validation done`, `rc=0`.

Also check that YAML still reads the same data (the comment changed nothing):

```bash
git show HEAD:.github/dependabot.yml > /private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/46bb56e1-abfa-4d35-a935-faa6cf37f273/scratchpad/sma683-v1/before.yml
docker run --rm -v "$PWD:/w" -v /private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/46bb56e1-abfa-4d35-a935-faa6cf37f273/scratchpad/sma683-v1:/b -w /w python:3.13-slim sh -c "pip install -q pyyaml && python -c \"import yaml; a=yaml.safe_load(open('/b/before.yml')); b=yaml.safe_load(open('.github/dependabot.yml')); print('same' if a==b else 'DIFFERENT')\""
```

Expected: `same`.

- [ ] **Step 5: Commit**

```bash
git add .github/dependabot.yml
git commit -m "docs(ci): record that dependabot cannot move wasm-bindgen (SMA-683)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: The runbook in `rs/CLAUDE.md` and the pointer in `rs/Cargo.toml`

> **Note (after execution):** the final review, the local review and the PR review changed the
> runbook text after this task ran. The `reqwest` case is now a numbered procedure, and step 6
> uses `@dependabot recreate` for a `Cargo.lock` conflict. `rs/CLAUDE.md` and spec section 4.2
> are the authority. The text below is the version this task wrote.

**Files:**
- Modify: `rs/CLAUDE.md:170` (append a bullet at the end of the section `## Cargo, the lockfile and nextest`, directly before the blank line and `## Container images`)
- Modify: `rs/Cargo.toml:148-150` (the `wasm-bindgen` comment, the words "(dependency-bump runbook)")

**Interfaces:**
- Consumes: the bullet title that Task 1's comment names: "The wasm-bindgen family does not move through dependabot".
- Produces: that runbook bullet. Task 3's test message names it as "the wasm-bindgen runbook in rs/CLAUDE.md".

- [ ] **Step 1: Write the failing checks**

```bash
grep -c "The wasm-bindgen family does not move through dependabot" rs/CLAUDE.md   # expect 1 after the change; now 0
grep -c "dependency-bump runbook" rs/Cargo.toml                                    # expect 0 after the change; now 1
```

Run both now. Expected now: `0` and `1`.

- [ ] **Step 2: Add the runbook bullet to `rs/CLAUDE.md`**

Append this bullet after the last line of the Cargo section (the line that ends `A11 reds on any *, ? or [ in members.`), before the blank line above `## Container images`:

````markdown
- **The wasm-bindgen family does not move through dependabot (SMA-683).** `js-sys`, `web-sys`
  and `wasm-bindgen-futures` pin `wasm-bindgen` with `=`. Dependabot updates one package at a
  time, and `cargo update -p wasm-bindgen` then locks 0 packages (MEASURED, spec M0). Since
  SMA-680 the release PR does not move it either. So `wasm-bindgen` stays frozen until a person
  moves the whole family. Do that at a `rust-toolchain.toml` or `wasm-pack` bump, at a
  `repo:deny` advisory for the family, or when a newer `wasm-bindgen` is needed. Use a normal
  `feature/sma-NNN-<slug>` PR.
  Before you start: put the proto shims on `PATH`; in a fresh worktree run `proto install` and
  `pnpm -C ts install`; have network access (`wasm-pack` downloads `wasm-bindgen-cli`); unlock
  1Password for commit signing.
  ```bash
  ( cd rs && cargo update -p wasm-bindgen -p js-sys -p web-sys -p wasm-bindgen-futures )
  git diff -- rs/Cargo.lock          # only the seven family entries, all crates.io sources
  moon run paigasus-kernel-ts:generate-wasm
  moon run paigasus-kernel-ts:test   # the drift gate, before the push
  git add rs/Cargo.lock rs/crates/bindings/paigasus-wasm/paigasus_wasm*
  ```
  Read the `git diff` BEFORE `generate-wasm`: that task compiles the new proc-macro and build
  scripts on your machine, where your `gh` token and signing agent are available. Run
  `generate-wasm` on ONE host (SMA-634 F12). If the pinned `wasm-pack` does not support the new
  0.2.z, bump it in `.prototools` in the same PR (the invariant above `wasm-bindgen` in
  `rs/Cargo.toml`). Record its error text here when a bump first shows it; it is not measured.
  **The `reqwest` exception (INFERRED).** A new `reqwest` can need newer wasm crates. Then a
  `cargo-minor-patch` PR moves `wasm-bindgen` and fails `committed-wasm.test.ts`. Use
  `gh pr checkout <N>` (it keeps the `dependabot/*` branch name the pre-push hook needs). Run
  the steps above from the `git diff`, then commit and push. Merge every other open cargo PR
  first, so the regeneration is the last change before the merge. Update the branch only with a
  merge: `gh api -X PUT repos/<owner>/<repo>/pulls/<N>/update-branch -f expected_head_sha=<sha>`.
  Never use `--rebase`, `@dependabot recreate` or `[dependabot skip]`: each one deletes the glue
  commit. If that merge brings a kernel, wasm-binding or artifact change, run `generate-wasm`
  again. For a conflict in the five files, take either side and regenerate.
````

- [ ] **Step 3: Fix the pointer in `rs/Cargo.toml`**

In the `wasm-bindgen` comment, replace `bump the two together (dependency-bump runbook), or this re-introduces` with `bump the two together (runbook: rs/CLAUDE.md, "The wasm-bindgen family does not move through dependabot"), or this re-introduces`. Re-wrap the comment lines to at most 100 characters, and keep every line starting with `# `. Do not change the `wasm-bindgen = "0.2"` line.

- [ ] **Step 4: Run the checks**

```bash
grep -c "The wasm-bindgen family does not move through dependabot" rs/CLAUDE.md   # 1
grep -c "The wasm-bindgen family does not move through dependabot" rs/Cargo.toml  # 1
grep -c "The wasm-bindgen family does not move through dependabot" .github/dependabot.yml  # 1
grep -c "dependency-bump runbook" rs/Cargo.toml                                   # 0
grep -n '( cd rs && cargo update -p wasm-bindgen' rs/CLAUDE.md                    # 1 line
git check-ignore -v --no-index rs/crates/bindings/paigasus-wasm/.wasmpack-test-out/ rs/crates/bindings/paigasus-wasm/.wasmpack-regen-out/
git ls-files rs/crates/bindings/paigasus-wasm/ | grep paigasus_wasm                # exactly the five artifacts
( cd rs && cargo metadata --format-version 1 --no-deps > /dev/null && echo manifest-ok )
```

Expected: the counts as commented; `check-ignore` prints a rule for both scratch directories (so `git add … paigasus_wasm*` cannot stage them — the glob also does not match a leading dot); `ls-files` lists exactly `paigasus_wasm.d.ts`, `paigasus_wasm.js`, `paigasus_wasm_bg.js`, `paigasus_wasm_bg.wasm`, `paigasus_wasm_bg.wasm.d.ts`; `manifest-ok`. If `check-ignore` prints nothing for a directory, STOP and report it: the runbook's glob then needs a review.

Check the root `CLAUDE.md` rule that the file stays readable: `wc -l rs/CLAUDE.md` (no hard limit for a nested file; report the number).

- [ ] **Step 5: Commit**

```bash
git add rs/CLAUDE.md rs/Cargo.toml
git commit -m "docs(rs): add the wasm-bindgen lockstep runbook (SMA-683)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: The pointer in the drift-gate message

**Files:**
- Modify: `ts/packages/paigasus-kernel/tests/committed-wasm.test.ts:65` (the `REGENERATE` constant)

**Interfaces:**
- Consumes: the runbook bullet title from Task 2.
- Produces: nothing later tasks use.

- [ ] **Step 1: Write the failing check**

```bash
grep -c "wasm-bindgen runbook in rs/CLAUDE.md" ts/packages/paigasus-kernel/tests/committed-wasm.test.ts   # now 0, expect 1
```

- [ ] **Step 2: Change the constant**

Replace line 65 with:

```ts
const REGENERATE =
  'Run `moon run paigasus-kernel-ts:generate-wasm` and commit all five files under rs/crates/bindings/paigasus-wasm/ (paigasus_wasm_bg.wasm and the four glue files). If wasm-bindgen moved, follow the wasm-bindgen runbook in rs/CLAUDE.md ("The wasm-bindgen family does not move through dependabot").';
```

`SURFACE_CHANGED` appends `REGENERATE` after `the committed binary is stale: `. Read the joined text once and confirm it is two correct sentences.

- [ ] **Step 3: Format**

```bash
cd ts && pnpm exec prettier --write packages/paigasus-kernel/tests/committed-wasm.test.ts && cd ..
git diff --stat
```

Expected: only `committed-wasm.test.ts` changes. If Prettier joins the constant back onto one line, keep Prettier's form.

- [ ] **Step 4: Run the checks and the test**

```bash
grep -c "wasm-bindgen runbook in rs/CLAUDE.md" ts/packages/paigasus-kernel/tests/committed-wasm.test.ts   # 1
moon run paigasus-kernel-ts:test ts:fmt paigasus-kernel-ts:typecheck
```

Expected: all three targets pass. `paigasus-kernel-ts:test` runs `napi build` and `wasm-pack build`, so it takes several minutes; run it in the foreground. If `ts:fmt` or `paigasus-kernel-ts:typecheck` is not a task name, run `moon query tasks paigasus-kernel-ts` and `moon query tasks ts` and use the real names; report what you used.

- [ ] **Step 5: Commit**

```bash
git add ts/packages/paigasus-kernel/tests/committed-wasm.test.ts
git commit -m "test(ts): point the wasm drift message at the wasm-bindgen runbook (SMA-683)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4 (controller, after Tasks 1-3): Linear and the full graph

Not for an implementer subagent.

- [ ] **Step 1:** Post the M0 table from spec section 3 as a comment on SMA-683 (spec V5).
- [ ] **Step 2:** Create the follow-up issue from spec section 7 (project Paigasus Polyglot, milestone CI & Tooling, priority Medium), related to SMA-683. Include the four safety constraints verbatim.
- [ ] **Step 3:** Run the full `moon ci` graph from the root `CLAUDE.md` (`ci-targets` block) with `--base origin/main --include-relations`. Re-run bash-version-sensitive gates directly per the root `CLAUDE.md` if they red for a bash-version reason. Record the result.
