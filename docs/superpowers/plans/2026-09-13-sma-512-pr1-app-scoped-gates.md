# SMA-512 PR 1 — Parameterize the app-scoped gates: Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the three repository gates that name `ts/apps/iam-console` cover every
`ts/apps/*` app, and make an uncovered app fail CI instead of being skipped in silence.

**Architecture:** Each gate keeps its current assertions and gains an app dimension. The Tailwind
guard takes the app directory as a CLI argument and every app's own `test` task invokes it for
itself, so no cross-app Moon dependency appears. The `next-env` gate discovers apps and loops
inside one repository task. `ts/eslint.config.js` derives one Next block per app from the
filesystem, so coverage cannot drift. Two new liveness assertions — a subset check over the
`package.json`-bearing apps inside
`ci/next-env/run.sh` and a registry check inside `ci/affected-graph/ci_targets.py` — are what stop
a third zone re-creating the trap.

**Tech Stack:** Bash, Node ESM (no dependencies), Python 3 (`ci/affected-graph/ci_targets.py`),
Moon 2.5.3, ESLint flat config.

**Spec:** `docs/superpowers/specs/2026-09-13-sma-512-gateway-console-design.md` — this plan
implements § 8.1, § 8.2 and § 8.3, which § 3 assigns to pull request 1.

## Global Constraints

- Every source file opens with an SPDX header: `// SPDX-License-Identifier: Apache-2.0`
  (`#` for shell and Python).
- Branch: `feature/sma-512-ts-gateway-console`. Conventional commits with a workspace scope.
- **This pull request adds no app.** Every change must be green with `ts/apps/iam-console` as the
  only app. That is the whole point of landing it first.
- **`repo:next-public-free` is NOT touched here.** Its `APP_CONFIG_FLOOR=1` is raised in pull
  request 3, when a second app actually exists (spec § 8.4).
- **`ci/next-env/run.sh` gains no `--self-test` and no `--negative-control`.** Adding them costs a
  `SELF_SCHEDULED_GATES` entry plus a `SELF_TASK_EXPECTED_GLOBS` or `SELF_TASK_GLOBS_EXEMPT` entry
  (spec § 8.1). The loop and the subset assertion are the control. Follow-up § 12 item 3
  owns the rest.
- `ci/tailwind-source/run.mjs` must stay at the repository root. An app directory is Tailwind's
  scan root, and a script there holding the sentinel literal would make Tailwind generate the
  utility the guard asserts on.
- Do not hand-edit `.github/CODEOWNERS`; it is Moon-generated.

## Environment

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
```

The Bash tool's PATH lacks the proto-managed CLIs. Two local-only bash traps, both measured:

- `ci/affected-graph/run.sh` needs the **system** `/bin/bash` 3.2. Bash 5.3.15 deadlocks on a
  `while read` fed by a here-string over roughly 512 bytes on this class of machine.
- `ci/actionlint/run.sh` needs `/opt/homebrew/bin/bash`. Under `/bin/bash` 3.2 it prints two false
  `cargo-lock-step` self-test failures.

Fixing one by copying the other's bash version breaks it. Neither applies in CI.

---

### Task 1: Parameterize the Tailwind guard by app directory

**Files:**
- Modify: `ci/tailwind-source/run.mjs` (module constant, `verdict`, `realRun`, the CLI arm)
- Modify: `ts/apps/iam-console/moon.yml` (the `test` task's `script` block)
- Modify: `ci/tailwind-source/README.md`

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: the CLI contract `node ci/tailwind-source/run.mjs --app <repo-relative-app-dir>`, which
  Task 2 pins and pull request 3 reuses for `gateway-console`. `verdict` keeps its current
  signature plus one optional field: `verdict({ cssFiles, appFiles, readFile, appLabel })` where
  `appLabel` is a string defaulting to `'the app'`.

**Why `verdict` barely changes.** It already takes `cssFiles` and `appFiles` as arguments and is
therefore already app-agnostic, and `selfTest()` and `negativeControl()` build their own temporary
fixtures. Only the module-level `CONSOLE_DIR`, `realRun()` and one prose failure message are
app-scoped. The self-test's and the negative control's expected counts do not change.

- [ ] **Step 1: Write the failing self-test cases**

Add these two `expect(...)` lines to `selfTest()` in `ci/tailwind-source/run.mjs`, directly after
the existing `expect('an empty appFiles set fails', ...)` line:

```js
    expect(
      'an empty appFiles set names the app in its message',
      verdict({ cssFiles: [good], appFiles: [], readFile: read, appLabel: 'ts/apps/zzz' })
        .filter((f) => f.includes('ts/apps/zzz')).length,
      1,
    );
    expect(
      'an empty appFiles set falls back to a generic label',
      verdict({ cssFiles: [good], appFiles: [], readFile: read })
        .filter((f) => f.includes('the app')).length,
      1,
    );
```

- [ ] **Step 2: Run the self-test to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
node ci/tailwind-source/run.mjs --self-test
```

Expected: `SELF-TEST FAIL: an empty appFiles set names the app in its message — expected 1
failure(s), got 0`, exit 2. The current message hardcodes `ts/apps/iam-console`.

- [ ] **Step 3: Give `verdict` the label parameter**

In `ci/tailwind-source/run.mjs`, change the signature and the one message that names the app:

```js
export function verdict({ cssFiles, appFiles, readFile, appLabel = 'the app' }) {
```

```js
  if (appFiles.length === 0) {
    failures.push(`no console source file was scanned — ${appLabel} may have been renamed or emptied, so this assertion would otherwise pass vacuously`);
  }
```

- [ ] **Step 4: Run the self-test to verify it passes**

```bash
node ci/tailwind-source/run.mjs --self-test
```

Expected: PASS, with the check count two higher than before.

- [ ] **Step 5: Replace `CONSOLE_DIR` with a `realRun` parameter**

Delete the module-level constant:

```js
const CONSOLE_DIR = join(REPO_ROOT, 'ts', 'apps', 'iam-console');
```

Add `relative` to the `node:path` import, then rewrite `realRun`:

```js
function realRun(appDir) {
  const nextDir = join(appDir, '.next');
  const cssFiles = currentBuildCssFiles(nextDir);

  const appFiles = existsSync(appDir) ? walk(appDir) : [];

  const appLabel = relative(REPO_ROOT, appDir);
  const failures = verdict({ cssFiles, appFiles, readFile: (f) => readFileSync(f, 'utf8'), appLabel });
  if (failures.length > 0) {
    for (const f of failures) console.error(`FAIL: ${f}`);
    console.error(`== tailwind-source guard FAILED for ${appLabel} ==`);
    process.exit(1);
  }
  console.log(`tailwind-source guard: all three sentinels present for ${appLabel} across ${String(cssFiles.length)} CSS file(s)`);
}
```

- [ ] **Step 6: Make the CLI require `--app`, and make a bare run fail**

Replace the mode block at the end of the file:

```js
const mode = process.argv[2];
if (mode === '--self-test') selfTest();
else if (mode === '--negative-control') negativeControl();
else if (mode === '--app') {
  const dir = process.argv[3];
  if (dir === undefined || dir === '') {
    console.error('--app requires a repository-relative app directory, e.g. --app ts/apps/iam-console');
    process.exit(2);
  }
  realRun(resolve(REPO_ROOT, dir));
} else {
  /*
   * A BARE RUN IS AN ERROR, deliberately. Until SMA-512 this script checked ts/apps/iam-console
   * with no argument. Leaving that behaviour would let a stale invocation in a second app's
   * moon.yml silently re-check the FIRST app and report green, which is the exact silent-skip
   * this parameterization exists to remove.
   */
  console.error(`unknown mode: ${String(mode)} — expected --app <dir>, --self-test or --negative-control`);
  process.exit(2);
}
```

- [ ] **Step 7: Verify the bare run now fails and `--app` works**

```bash
node ci/tailwind-source/run.mjs; echo "bare rc=$?"
node ci/tailwind-source/run.mjs --negative-control; echo "control rc=$?"
```

Expected: `bare rc=2` with the `unknown mode: undefined` message, and `control rc=0` with
`reported red as expected`.

The real run needs a build first, so it is verified in Step 9.

- [ ] **Step 8: Point iam-console's `test` task at its own directory**

In `ts/apps/iam-console/moon.yml`, change the last line of the `test` task's `script` block:

```yaml
    script: |
      set -euo pipefail
      pnpm exec vitest run --passWithNoTests
      node ../../../ci/tailwind-source/run.mjs --self-test
      node ../../../ci/tailwind-source/run.mjs --negative-control
      node ../../../ci/tailwind-source/run.mjs --app ts/apps/iam-console
```

Add this comment above the `script:` key, after the existing block comment:

```yaml
    # SMA-512: the guard takes the app directory, because a second zone app lands in pull request
    # 3. Each app invokes it for ITSELF, so no app's test task depends on another app's build.
```

- [ ] **Step 9: Run the real gate end to end**

```bash
moon run iam-console-ts:test
```

Expected: PASS, ending with
`tailwind-source guard: all three sentinels present for ts/apps/iam-console across N CSS file(s)`.

- [ ] **Step 10: Update the guard's README**

In `ci/tailwind-source/README.md`, replace every statement that the guard checks
`ts/apps/iam-console` with the parameterized contract. Add this to the Limitations section:

```markdown
- The guard checks ONE app per invocation, named by `--app`. Nothing in this script proves every
  app invokes it. That liveness assertion lives in `ci/affected-graph/ci_targets.py` and runs
  inside `repo:affected-smoke` (SMA-512 spec § 8.2).
```

- [ ] **Step 11: Commit**

```bash
git add ci/tailwind-source/run.mjs ci/tailwind-source/README.md ts/apps/iam-console/moon.yml
git commit -m "ci(ts): take the app directory as an argument in the tailwind-source guard (SMA-512)"
```

---

### Task 2: Assert every app invokes the Tailwind guard for itself

**Files:**
- Modify: `ci/affected-graph/ci_targets.py` (a registry constant, a pure check, self-test rows, and
  the real call site)

**Interfaces:**
- Consumes: Task 1's CLI contract, `node ../../../ci/tailwind-source/run.mjs --app <dir>`.
- Produces: `TAILWIND_GUARD_INVOCATIONS: dict[str, tuple[str, ...]]`, keyed by the **app directory
  name** (`"iam-console"`), whose value is the three whole lines that app's `moon.yml` must
  contain. Pull request 3 adds the `"gateway-console"` entry.
  `check_tailwind_guard_invocations(moon_ymls, registry=None)` is pure and returns
  `(unregistered, missing_lines, stale)` — three sorted lists, the same shape
  `check_self_scheduled_coverage` returns.

**Why this task exists.** Task 1 made every app invoke the guard for itself. Nothing yet stops a
new app never invoking it at all — the silent skip moved rather than closed. This check is the
replacement, and it lives in a gate scheduled independently of any app.

**Why it reads `moon.yml` text and not `moon query tasks`.** `_scripts(projects)` is keyed to
`projects.get("repo")` (`ci_targets.py:1578`) and reaches repository tasks only. Deriving app
projects would need `moon query projects` for each project's `source`. Reading each app's
`moon.yml` as text is what `check_self_invocation` already does for the gate scripts, keeps the
check pure, and makes the self-test fixtures plain strings.

- [ ] **Step 1: Add the registry and the pure check**

Add to `ci/affected-graph/ci_targets.py`, beside the other registry tables:

```python
# SMA-512. The tailwind-source guard checks ONE app per invocation (`--app <dir>`), so an app that
# never invokes it is simply unguarded — the silent skip the parameterization was meant to remove,
# moved rather than closed. Every ts/apps/* directory must therefore appear here, and its moon.yml
# must carry all three lines: the self-test and the negative control prove the assertion can fire,
# and the real run for THAT app is the assertion. Whole lines, compared after stripping, so
# reordering a flag or dropping the `--app` argument reds this.
TAILWIND_GUARD_INVOCATIONS = {
    "iam-console": (
        "node ../../../ci/tailwind-source/run.mjs --self-test",
        "node ../../../ci/tailwind-source/run.mjs --negative-control",
        "node ../../../ci/tailwind-source/run.mjs --app ts/apps/iam-console",
    ),
}


def check_tailwind_guard_invocations(moon_ymls, registry=None):
    """SMA-512. Every ts/apps/* app must invoke the tailwind-source guard for its own directory.

    `moon_ymls` is `{app_dir_name: moon.yml text}`, built by the caller from the ts/apps
    directories that exist on disk. PURE, so the self-test drives it with plain strings.

    Returns (unregistered, missing_lines, stale), all sorted — the same shape
    check_self_scheduled_coverage returns, and for the same reason: a typo'd registry key is
    already loud (the real app shows up under `unregistered`), so `stale` exists for the silent
    case, an entry that outlived the app it named.
    """
    registry = TAILWIND_GUARD_INVOCATIONS if registry is None else registry

    unregistered = sorted(app for app in moon_ymls if app not in registry)
    stale = sorted(set(registry) - set(moon_ymls))

    missing_lines = []
    for app, text in sorted(moon_ymls.items()):
        wanted = registry.get(app)
        if wanted is None:
            continue
        present = {line.strip() for line in text.splitlines()}
        for want in wanted:
            if want not in present:
                missing_lines.append(f"{app}: {want}")

    return unregistered, sorted(missing_lines), stale
```

- [ ] **Step 2: Add the self-test rows**

Inside `self_test()`, beside the other check fixtures, add:

```python
    def expect_tailwind(label, moon_ymls, want):
        got = check_tailwind_guard_invocations(moon_ymls, TAILWIND_GUARD_INVOCATIONS)
        if got != want:
            failures.append(f"check_tailwind_guard_invocations[{label}]: got {got}, want {want}")

    _tw_ok = "\n".join(f"      {line}" for line in TAILWIND_GUARD_INVOCATIONS["iam-console"])
    expect_tailwind("compliant", {"iam-console": _tw_ok}, ([], [], []))
    expect_tailwind(
        "a dropped real run reds",
        {"iam-console": "      node ../../../ci/tailwind-source/run.mjs --self-test\n"},
        (
            [],
            [
                "iam-console: node ../../../ci/tailwind-source/run.mjs --app ts/apps/iam-console",
                "iam-console: node ../../../ci/tailwind-source/run.mjs --negative-control",
            ],
            [],
        ),
    )
    expect_tailwind(
        "an unregistered app reds",
        {"iam-console": _tw_ok, "gateway-console": "script: |\n"},
        (["gateway-console"], [], []),
    )
    expect_tailwind("a stale registry entry reds", {}, ([], [], ["iam-console"]))
```

The `f"      {line}"` indentation is deliberate: it proves the check strips leading whitespace, so
a line nested inside a YAML `script: |` block still matches.

- [ ] **Step 3: Run the self-test to verify the rows fail**

```bash
/bin/bash ci/affected-graph/run.sh --self-test
```

Expected: FAIL, four `check_tailwind_guard_invocations[...]` lines, because the function does not
exist yet if Step 1 has not been applied. If Step 1 is already applied, the rows pass and the
**real** call site is what is still missing — Step 5 proves that separately.

Use the **system** `/bin/bash`, not bash 5.3.15. See Environment.

- [ ] **Step 4: Wire the real call site**

Inside `main()`'s `try` block, beside the other `read_input` calls, build the mapping. Do it in the
try so a read error surfaces as rc 2 rather than escaping uncaught:

```python
        tailwind_moon_ymls = {
            d.name: read_input(d / "moon.yml", f"ts/apps/{d.name}/moon.yml")
            for d in sorted((root / "ts" / "apps").iterdir())
            if d.is_dir() and (d / "moon.yml").is_file()
        }
```

Below the try, beside the other check invocations:

```python
    tw_unregistered, tw_missing_lines, tw_stale = check_tailwind_guard_invocations(
        tailwind_moon_ymls
    )
```

Add all three to the `if not (...)` pass condition, and print them in the failure branch beside the
other findings, following the surrounding style.

- [ ] **Step 5: Prove the check bites on the real repository**

Temporarily delete the `--app ts/apps/iam-console` line from `ts/apps/iam-console/moon.yml`'s
`test` script, then:

```bash
/bin/bash ci/affected-graph/run.sh 2>&1 | grep -i tailwind
```

Expected: a finding naming
`iam-console: node ../../../ci/tailwind-source/run.mjs --app ts/apps/iam-console`.

Restore the line with an editor — **not** `git checkout --`, which would revert any unstaged work
from Task 1.

- [ ] **Step 6: Run both control arms**

```bash
/bin/bash ci/affected-graph/run.sh --self-test
/bin/bash ci/affected-graph/run.sh --negative-control
```

Expected: both PASS.

- [ ] **Step 7: Commit**

```bash
git add ci/affected-graph/ci_targets.py
git commit -m "ci(ts): assert every ts/apps app invokes the tailwind-source guard (SMA-512)"
```

---

### Task 3: Make the next-env gate loop over every app

**Files:**
- Modify: `ci/next-env/run.sh`
- Modify: `moon.yml` (the `next-env-drift` task's `inputs`; `deps` is unchanged in this PR)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: a gate that checks every discovered app. Pull request 3 adds
  `gateway-console-ts:build` to the task's `deps` when the second app exists; this PR must not,
  because the target does not exist yet and Moon would fail to load the graph.

**The liveness assertion is the point.** Discovery alone would still skip an app that has a
`next.config.ts` under a name the glob misses. The gate therefore compares the discovered set
against the `ts/apps/*` directory set and fails on any difference.

- [ ] **Step 1: Rewrite the gate's body as a loop**

Replace lines 26-82 of `ci/next-env/run.sh` (everything from `APP='ts/apps/iam-console'` to the
end) with:

```bash
# SMA-512: this gate checked ONE hardcoded app until a second console zone landed, and it would
# have skipped the new one in silence. Discovery plus the set-equality assertion below is what
# makes a future third zone impossible to miss.
#
# NOTE: this gate has no --self-test and no --negative-control, deliberately. Adding them costs a
# SELF_SCHEDULED_GATES entry plus a SELF_TASK_EXPECTED_GLOBS or SELF_TASK_GLOBS_EXEMPT entry in
# ci/affected-graph/ci_targets.py. The loop and the set-equality assertion are the control.
shopt -s nullglob
apps=()
for cfg in ts/apps/*/next.config.[tjmc][sj]*; do
  apps+=("$(dirname "$cfg")")
done
shopt -u nullglob

if [ "${#apps[@]}" -eq 0 ]; then
  echo "next-env gate: no ts/apps/*/next.config.* found — this gate is guarding nothing." >&2
  exit 2
fi

# LIVENESS. A Next app directory with no discoverable config would otherwise be skipped without a
# word, which is the exact defect this rewrite exists to remove.
dirs=()
for d in ts/apps/*/; do
  dirs+=("${d%/}")
done
missing=()
for d in "${dirs[@]}"; do
  found=0
  for a in "${apps[@]}"; do
    [ "$a" = "$d" ] && found=1
  done
  [ "$found" -eq 1 ] || missing+=("$d")
done
if [ "${#missing[@]}" -gt 0 ]; then
  echo "next-env gate: these ts/apps/* directories have no discoverable next.config.*:" >&2
  printf '  %s\n' "${missing[@]}" >&2
  echo "  Each would be skipped by this gate in silence. Add a config, or remove the directory." >&2
  exit 2
fi

rc=0
for APP in "${apps[@]}"; do
  check_app "$APP" || rc=1
done
exit "$rc"
```

- [ ] **Step 2: Move the existing per-app logic into `check_app`**

Insert this function definition **above** the loop, immediately after the `cd` on line 24. It is
the previous body verbatim, with `$APP` as a parameter and `return` in place of `exit`:

```bash
check_app() {
  local APP="$1"
  local FILE="$APP/next-env.d.ts"

  if [ ! -f "$FILE" ]; then
    echo "next-env gate: '$FILE' is missing from the working tree — it is a tracked file." >&2
    return 2
  fi

  # Present is not the same as TRACKED, and only tracked is meaningful here. `git diff` ignores
  # untracked paths, so after a `git rm --cached` typegen would recreate the file, the diff would
  # compare nothing, and this gate would report a clean pass forever. (CodeRabbit, SMA-519)
  if ! git ls-files --error-unmatch -- "$FILE" >/dev/null 2>&1; then
    echo "next-env gate: '$FILE' exists but is NOT tracked by git." >&2
    echo "  This gate compares generated output against the committed copy; with nothing" >&2
    echo "  committed that comparison is vacuous and would pass unconditionally." >&2
    return 2
  fi

  rm -f "$FILE"
  RESTORE_FILES+=("$FILE")

  if ! pnpm --dir "$APP" exec next typegen >/dev/null 2>&1; then
    echo "next-env gate: 'next typegen' failed in $APP." >&2
    pnpm --dir "$APP" exec next typegen >&2 || true
    return 2
  fi

  # Control: typegen must actually have produced the file. Without this the gate would go quietly
  # vacuous the day Next changes how this file is emitted.
  if [ ! -f "$FILE" ]; then
    echo "next-env gate: 'next typegen' completed but did not emit $FILE." >&2
    echo "  Next no longer generates this file the same way, so this gate is guarding nothing." >&2
    return 2
  fi

  if ! git diff --exit-code -- "$FILE"; then
    echo "" >&2
    echo "next-env gate: the committed $FILE does not match what Next generates." >&2
    echo "  The regenerated file has been left in your working tree — commit it." >&2
    return 1
  fi

  echo "next-env gate: $FILE matches 'next typegen' output."
  return 0
}
```

- [ ] **Step 3: Make the restore trap cover every app**

The current trap closes over a single `$FILE`. Replace lines 47-52 with an array-driven trap,
placed **above** `check_app` so the array exists before the first append:

```bash
# If typegen dies before writing a file, restore it rather than leaving the tree broken. A
# DRIFTING file is deliberately left in place: it is the corrected content, ready to commit.
RESTORE_FILES=()
restore_if_absent() {
  local f
  for f in "${RESTORE_FILES[@]:-}"; do
    [ -f "$f" ] || git checkout -- "$f" 2>/dev/null || true
  done
}
trap restore_if_absent EXIT
```

- [ ] **Step 4: Run the gate**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run iam-console-ts:build
bash ci/next-env/run.sh; echo "rc=$?"
```

Expected: `rc=0`, one line —
`next-env gate: ts/apps/iam-console/next-env.d.ts matches 'next typegen' output.`

- [ ] **Step 5: Prove the liveness assertion bites**

```bash
mkdir -p ts/apps/zzz-probe
bash ci/next-env/run.sh; echo "rc=$?"
rmdir ts/apps/zzz-probe
```

Expected: `rc=2`, and the output names `ts/apps/zzz-probe` as having no discoverable config.

> **Superseded during execution.** The final review ruled that the liveness assertion counts only
> directories containing a `package.json`, so a bare `mkdir` no longer triggers it. The probe that
> actually exercises the missing-config branch is
> `mkdir -p ts/apps/zzz-probe && printf '{}' > ts/apps/zzz-probe/package.json`, run the gate,
> expect rc 2, then `rm -rf ts/apps/zzz-probe`. A directory with no `package.json` is now
> correctly ignored (rc 0), which is its own probe.

- [ ] **Step 6: Prove the drift assertion still bites**

```bash
printf '\n// drift probe\n' >> ts/apps/iam-console/next-env.d.ts
git add ts/apps/iam-console/next-env.d.ts
bash ci/next-env/run.sh; echo "rc=$?"
git restore --staged ts/apps/iam-console/next-env.d.ts
git checkout -- ts/apps/iam-console/next-env.d.ts
```

Expected: `rc=1`, with `the committed ts/apps/iam-console/next-env.d.ts does not match`.

**The `git add` is required, and the reason is worth knowing.** `check_app` deletes the file and
regenerates it before diffing, so an UNSTAGED edit is destroyed before `git diff` ever runs and the
gate passes. `git diff` compares the worktree against the INDEX, so the drift has to be staged for
the assertion to see it. This is pre-existing behaviour of the gate, not something this task
introduces. `git checkout --` is safe on this one file: it carries no uncommitted work of ours.

- [ ] **Step 7: Widen the Moon task's inputs per path**

In `moon.yml`, replace the `next-env-drift` task's four `ts/apps/iam-console/...` input lines:

```yaml
    inputs:
      - 'ci/next-env/**/*'
      # SMA-512: per PATH, never `ts/apps/*/**`. A blanket tree glob sweeps in `.next`, which
      # .moon/workspace.yml's hasher.ignorePatterns deliberately does NOT ignore because `.next`
      # is a declared build output — repo:next-public-free negates it by hand for that reason.
      - 'ts/apps/*/next-env.d.ts'
      - 'ts/apps/*/next.config.ts'
      - 'ts/apps/*/tsconfig.json'
      - 'ts/apps/*/app/**/*'
      - 'ts/pnpm-lock.yaml'
```

Leave `deps: ['iam-console-ts:build']` unchanged. Pull request 3 adds the gateway build.

- [ ] **Step 8: Verify the task and the input-liveness gate**

```bash
moon run repo:next-env-drift
moon run repo:input-liveness
```

Expected: both PASS. `repo:input-liveness` fails when a declared glob matches zero tracked files,
so it is the check that the widened globs still resolve.

- [ ] **Step 9: Commit**

```bash
git add ci/next-env/run.sh moon.yml
git commit -m "ci(ts): check every ts/apps app in the next-env drift gate (SMA-512)"
```

---

### Task 4: Derive one Next ESLint block per app

**Files:**
- Modify: `ts/packages/paigasus-next-config/src/eslint.mjs` (export `nextAppRules`)
- Modify: `ts/eslint.config.js` (spread it over the discovered app directories)
- Test: `ts/packages/paigasus-next-config/tests/next-app-rules.test.ts` (create)
- Modify: `ts/packages/paigasus-next-config/moon.yml` (test inputs)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: `nextAppRules({ appsDir, appNames, plugin })` from `@paigasus/next-config/eslint`,
  returning one flat-config block per app name:
  `{ files: ['apps/<name>/**/*.{ts,tsx}'], settings: { next: { rootDir: join(appsDir, name) } },
  plugins: { '@next/next': plugin }, rules: { ...plugin.configs.recommended.rules } }`.
  Pull request 3 needs no edit here — coverage is derived, not listed.

**Why a helper in the package, not a `readdirSync` inline in the config.** The rules must be
testable without touching the filesystem. An inline derivation could only be tested by creating a
directory under `ts/apps/`, and an empty directory there would trip Task 3's new liveness
assertion if the two gates ran concurrently. Taking `appNames` as a parameter makes the test pure,
and it puts the helper beside `boundaryRules` and `sourceRules`, which ship from the same module
for the same reason.

- [ ] **Step 1: Write the failing test**

Create `ts/packages/paigasus-next-config/tests/next-app-rules.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import { nextAppRules } from '../src/eslint.mjs';

/**
 * SMA-512. The Next plugin's rules were scoped to one hardcoded app, so a second zone app would
 * have shipped with NO Next lint rules and nothing would have said so — the same silent-skip
 * shape as the next-env and tailwind-source gates.
 */
describe('nextAppRules', () => {
  const plugin = { configs: { recommended: { rules: { 'x/y': 'error' } } } };

  it('returns one block per app name', () => {
    const blocks = nextAppRules({ appsDir: '/repo/ts/apps', appNames: ['a', 'b'], plugin });
    expect(blocks).toHaveLength(2);
    expect(blocks.map((b) => b.files[0])).toEqual(['apps/a/**/*.{ts,tsx}', 'apps/b/**/*.{ts,tsx}']);
  });

  it('gives every block its OWN rootDir', () => {
    const blocks = nextAppRules({ appsDir: '/repo/ts/apps', appNames: ['a', 'b'], plugin });
    expect(blocks.map((b) => b.settings.next.rootDir)).toEqual([
      join('/repo/ts/apps', 'a'),
      join('/repo/ts/apps', 'b'),
    ]);
  });

  it('carries the plugin and its recommended rules', () => {
    const [block] = nextAppRules({ appsDir: '/repo/ts/apps', appNames: ['a'], plugin });
    expect(block.plugins['@next/next']).toBe(plugin);
    expect(block.rules).toEqual({ 'x/y': 'error' });
  });

  it('returns nothing for no apps, rather than one catch-all block', () => {
    expect(nextAppRules({ appsDir: '/repo/ts/apps', appNames: [], plugin })).toEqual([]);
  });
});

/** Mirrors the existing boundaryRules/sourceRules spread assertions. */
describe('ts/eslint.config.js', () => {
  it('spreads nextAppRules over the discovered app directories', () => {
    const config = readFileSync(join(import.meta.dirname, '..', '..', '..', 'eslint.config.js'), 'utf8');
    expect(config).toContain('nextAppRules');
    expect(config).toContain('...nextAppRules(');
    // The hardcoded single-app block must be gone, or coverage silently stops being derived.
    expect(config).not.toContain("files: ['apps/iam-console/**/*.{ts,tsx}']");
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm --dir ts/packages/paigasus-next-config exec vitest run tests/next-app-rules.test.ts
```

Expected: FAIL at import — `nextAppRules` is not exported from `../src/eslint.mjs`.

- [ ] **Step 3: Export `nextAppRules`**

Add to `ts/packages/paigasus-next-config/src/eslint.mjs`, beside `boundaryRules` and
`sourceRules`:

```js
/**
 * One Next.js flat-config block per app (SMA-512).
 *
 * `settings.next.rootDir` must be PER APP so `no-html-link-for-pages` resolves each App Router at
 * its own apps/<name>/app/ rather than searching the cwd. The caller supplies `appsDir` as an
 * absolute path, which keeps the result cwd-independent.
 *
 * `appNames` is a PARAMETER rather than a readdir inside this function, so the tests are pure. An
 * inline derivation could only be tested by creating a directory under ts/apps/, and an empty one
 * there trips ci/next-env/run.sh's liveness assertion.
 */
export function nextAppRules({ appsDir, appNames, plugin }) {
  return appNames.map((app) => ({
    files: [`apps/${app}/**/*.{ts,tsx}`],
    settings: { next: { rootDir: join(appsDir, app) } },
    plugins: { '@next/next': plugin },
    rules: { ...plugin.configs.recommended.rules },
  }));
}
```

If `eslint.mjs` does not already import `join`, add `import { join } from 'node:path';` at the top.

- [ ] **Step 4: Run the test to verify the first four cases pass**

```bash
pnpm --dir ts/packages/paigasus-next-config exec vitest run tests/next-app-rules.test.ts
```

Expected: the four `nextAppRules` cases PASS; the `ts/eslint.config.js` case still FAILS, because
the config has not been changed yet.

- [ ] **Step 5: Spread it in `ts/eslint.config.js`**

Add `readdirSync` to the `node:fs` import, add `nextAppRules` to the
`@paigasus/next-config/eslint` import, and replace the single Next block — the object whose
`files` is `['apps/iam-console/**/*.{ts,tsx}']`, together with its preceding comment — with:

```js
  /*
   * Next.js rules — ONE BLOCK PER APP, derived from the filesystem (SMA-512).
   *
   * This was a single hardcoded `apps/iam-console/**` block. A second zone app would have shipped
   * with no Next rules at all and nothing would have said so. The blocks are built by
   * nextAppRules in @paigasus/next-config/eslint, which is unit-tested there; this file's job is
   * only to supply the app list. paigasus-next-config-ts:test asserts this spread is still here.
   */
  ...nextAppRules({
    appsDir: path.join(import.meta.dirname, 'apps'),
    appNames: readdirSync(path.join(import.meta.dirname, 'apps'), { withFileTypes: true })
      .filter((entry) => entry.isDirectory())
      .map((entry) => entry.name)
      .sort(),
    plugin: nextPlugin,
  }),
```

- [ ] **Step 6: Run the test to verify it all passes**

```bash
pnpm --dir ts/packages/paigasus-next-config exec vitest run tests/next-app-rules.test.ts
```

Expected: PASS, all five cases.

- [ ] **Step 7: Confirm ESLint still enforces the Next rules**

```bash
moon run ts:lint
```

Expected: PASS. To confirm the rules are actually attached rather than silently dropped, add
`<a href="/iam/orgs">x</a>` to a page under `ts/apps/iam-console/app/`, re-run `moon run ts:lint`,
and expect `@next/next/no-html-link-for-pages` to fire. Remove the line afterwards with an editor.

- [ ] **Step 8: Add the Moon inputs that make the assertion reachable**

In `ts/packages/paigasus-next-config/moon.yml`'s `test` task, confirm `/ts/eslint.config.js` is
already an input — the existing boundary-spread assertion needs it. Add the app list as an input
too, or the new test serves a cached pass on the very pull request that adds a second app:

```yaml
      # SMA-512: adding or removing an app changes the derived Next block set, which
      # tests/next-app-rules.test.ts asserts. Without this input that test serves a cached pass on
      # the PR that adds a second app.
      - '/ts/apps/*/package.json'
```

- [ ] **Step 9: Run the package's tests and the liveness gate**

```bash
moon run paigasus-next-config-ts:test
moon run repo:input-liveness
```

Expected: both PASS.

- [ ] **Step 10: Commit**

```bash
git add ts/eslint.config.js ts/packages/paigasus-next-config/src/eslint.mjs \
  ts/packages/paigasus-next-config/tests/next-app-rules.test.ts \
  ts/packages/paigasus-next-config/moon.yml
git commit -m "ci(ts): derive one Next ESLint block per app (SMA-512)"
```

---

### Task 5: Documentation and the full gate run

**Files:**
- Modify: `CLAUDE.md` (the gotcha describing the Tailwind guard)
- Modify: `ts/README.md` (the commands table)

**Interfaces:**
- Consumes: Tasks 1-4.
- Produces: nothing code-facing. This task is the verification gate for the pull request.

- [ ] **Step 1: Update the CLAUDE.md Tailwind entry**

Find the bullet beginning "Tailwind v4's automatic scan root is the **current working
directory**". Append:

```markdown
  Since SMA-512 the guard is **per app**: `ci/tailwind-source/run.mjs --app <dir>`, invoked by each
  app's own `test` task, and a BARE run now exits 2 rather than silently checking `iam-console`.
  `TAILWIND_GUARD_INVOCATIONS` in `ci/affected-graph/ci_targets.py` fails `repo:affected-smoke` if
  a `ts/apps/*` project does not invoke all three modes for itself. `repo:next-env-drift` and the
  Next ESLint blocks are app-agnostic too: the first discovers `ts/apps/*/next.config.*` and
  asserts the discovered set equals the `ts/apps/*` directory set, and `ts/eslint.config.js`
  derives one block per app directory. The next-env gate still has **no negative control**.
```

- [ ] **Step 2: Update `ts/README.md`**

In the commands table, change the Tailwind guard row to the `--app` form. Leave the app bullet
list alone — pull request 3 adds `gateway-console` to it.

- [ ] **Step 3: Run the full repository gate graph**

Per-project tasks do not run the repository gates, so run the graph the way CI does. Use the
marker-delimited command in `CLAUDE.md`'s "Per-project Moon tasks" bullet verbatim, with
`--base origin/main --include-relations`.

Expected: all targets PASS. `repo:affected-smoke`, `repo:input-liveness`, `repo:next-env-drift`
and `repo:actionlint` are the four this pull request can plausibly red.

- [ ] **Step 4: If `repo:affected-smoke` reds, diagnose before re-running**

A re-run overwrites the evidence — a passing re-run truncates `stderr.log` and rewrites the report
row. Copy `.moon/cache/states/repo/affected-smoke/` outside the repository first, then follow the
diagnosis procedure between the `moon-diagnosis` markers in `CLAUDE.md`.

A sub-3s `affected-smoke` failure under a concurrent `moon ci` is a known, unreproduced abort:
capture the full task output before re-running, and grep it for `proto-shim`.

- [ ] **Step 5: Commit and push**

```bash
git add CLAUDE.md ts/README.md
git commit -m "docs(ts): record the per-app gate parameterization (SMA-512)"
git push -u origin feature/sma-512-ts-gateway-console
```

---

## Rules that the code depends on

- The Tailwind guard's bare mode is an **error**, not a default. A stale invocation must fail, not
  silently re-check the first app.
- `repo:next-env-drift` has no negative control. Its correctness rests on the loop and the
  subset assertion (every `ts/apps/*` directory with a `package.json` is in the discovered set,
  not set-equality).
- `ts/eslint.config.js` derives its Next blocks from `nextAppRules`. Do not add a literal block
  for a new app.
- `moon.yml`'s `next-env-drift` inputs are per path. A `ts/apps/*/**` tree glob would hash `.next`.

## Test tiers

| Tier | Command |
|---|---|
| Tailwind guard | `node ci/tailwind-source/run.mjs --self-test` and `--negative-control` |
| Affected-graph registry | `/bin/bash ci/affected-graph/run.sh --self-test` |
| next-env gate | `bash ci/next-env/run.sh` after `moon run iam-console-ts:build` |
| ESLint derivation | `moon run paigasus-next-config-ts:test` |
| Registry check | `/bin/bash ci/affected-graph/run.sh --negative-control` |
| Everything | the marker-delimited `moon ci` command in `CLAUDE.md` |

## Known limits

- This pull request cannot prove the gates work for a **second** app, because none exists yet.
  Task 2's self-test and Task 4's unit test both drive the multi-app path with synthetic inputs,
  which is why neither needs a real directory. Task 3's liveness probe is the one step that
  creates a throwaway `ts/apps/zzz-probe` directory, and it removes it in the same step — do not
  leave it in place, because `ci/next-env/run.sh` then fails for every other caller.
  Pull request 3 is the real proof.
- `repo:next-env-drift`'s `deps` still names only `iam-console-ts:build`. Pull request 3 must add
  the gateway build, or `next typegen` will race that app's `.next` directory.
