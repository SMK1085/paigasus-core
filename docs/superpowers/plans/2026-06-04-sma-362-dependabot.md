# Dependabot Setup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `.github/dependabot.yml` so cargo, npm/pnpm, uv, and GitHub-Actions dependencies get grouped weekly update PRs whose commit subjects pass the repo's commitlint gate.

**Architecture:** A single declarative config file — four `updates` entries, one per ecosystem, each weekly (Monday 06:00 UTC) with one minor+patch group and a hardcoded Conventional-Commit prefix (`build(deps)` for package managers, `ci(deps)` for actions). No code and no commitlint change: commitlint's built-in URL carve-out already lets Dependabot's URL-laden bodies pass `body-max-line-length` (verified). Dependabot reads version-update config from the **default branch**, so the generated-PR acceptance criterion is verified *after* this PR merges.

**Tech Stack:** GitHub Dependabot (config v2), commitlint 21 (`@commitlint/config-conventional` via `@paigasus/commitlint-config`), Moon CI, `gh` CLI, `python3` + PyYAML (local YAML validation).

**Spec:** `docs/superpowers/specs/2026-06-04-sma-362-dependabot-design.md`

---

## Prerequisites

- On branch `feature/sma-362-dependabot-cla-bot-setup` (already created; carries the spec commits).
- proto-managed tools on PATH for this shell. Moon/pnpm/uv are off the default Bash PATH — export the proto dirs at the start of every shell that runs the commands below:
  ```bash
  export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"
  ```
- TS deps installed (`pnpm --dir ts install` already done — `ts/node_modules/.bin/commitlint` exists). If not: `pnpm --dir ts install --frozen-lockfile`.
- `gh` authenticated (`gh auth status`) for the PR + Dependabot-trigger steps.

---

## Task 1: Pre-flight — confirm the chosen commit prefixes pass commitlint

Locks in the prefix decision before writing the config: prove `build(deps): …` and `ci(deps): …` pass, and that the naive alternatives Dependabot would otherwise emit fail. No files change.

**Files:** none (verification only).

- [ ] **Step 1: Write the four probe messages**

```bash
export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core
printf 'build(deps): bump serde from 1.0.1 to 1.0.2\n'                      > /tmp/p-cargo.txt
printf 'ci(deps): bump actions/checkout from 4 to 5\n'                       > /tmp/p-actions.txt
printf 'build(deps-dev): bump vitest from 4.1.7 to 4.1.8\n'                  > /tmp/p-devscope.txt   # what include:scope WOULD emit
printf 'deps: bump serde from 1.0.1 to 1.0.2\n'                             > /tmp/p-noscope.txt    # no type — illustrative
```

- [ ] **Step 2: Run the two GOOD prefixes — expect PASS**

```bash
pnpm --dir ts exec commitlint --config commitlint.config.cjs --edit /tmp/p-cargo.txt;   echo "cargo  exit=$?"
pnpm --dir ts exec commitlint --config commitlint.config.cjs --edit /tmp/p-actions.txt;  echo "actions exit=$?"
```
Expected: both print `exit=0` (no problems).

- [ ] **Step 3: Run the two BAD prefixes — expect FAIL**

```bash
pnpm --dir ts exec commitlint --config commitlint.config.cjs --edit /tmp/p-devscope.txt; echo "devscope exit=$?"
pnpm --dir ts exec commitlint --config commitlint.config.cjs --edit /tmp/p-noscope.txt;   echo "noscope  exit=$?"
```
Expected: both print `exit=1`. `p-devscope` fails `scope-enum` (`deps-dev` not allowed) — this is exactly why the spec hardcodes `prefix`/`prefix-development` to `build(deps)` instead of using Dependabot's `include: "scope"`. `p-noscope` fails `type-enum`/`scope-empty`.

No commit — this task only validates the decision.

---

## Task 2: Create `.github/dependabot.yml`

**Files:**
- Create: `.github/dependabot.yml`
- Verify: structural assertion via `python3` (throwaway, not committed)

- [ ] **Step 1: Write the structural check and run it FIRST (expect failure — file absent)**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core
python3 - <<'PY'
import sys, yaml
try:
    cfg = yaml.safe_load(open('.github/dependabot.yml'))
except FileNotFoundError:
    print('FAIL: .github/dependabot.yml does not exist yet'); sys.exit(1)
ecos = {u['package-ecosystem']: u for u in cfg['updates']}
assert cfg['version'] == 2, cfg.get('version')
assert set(ecos) == {'cargo', 'npm', 'uv', 'github-actions'}, set(ecos)
assert ecos['cargo']['directory'] == '/rs'
assert ecos['npm']['directory'] == '/ts'
assert ecos['uv']['directory'] == '/py'
assert ecos['github-actions']['directory'] == '/'
assert ecos['cargo']['commit-message']['prefix'] == 'build(deps)'
assert ecos['npm']['commit-message']['prefix'] == 'build(deps)'
assert ecos['uv']['commit-message']['prefix'] == 'build(deps)'
assert ecos['github-actions']['commit-message']['prefix'] == 'ci(deps)'
for k in ecos:
    g = next(iter(ecos[k]['groups'].values()))
    assert g['applies-to'] == 'version-updates', (k, g)
    assert g['update-types'] == ['minor', 'patch'], (k, g)
    assert ecos[k]['schedule']['interval'] == 'weekly', k
print('dependabot.yml OK')
PY
```
Expected now: `FAIL: .github/dependabot.yml does not exist yet` (exit 1).

- [ ] **Step 2: Create `.github/dependabot.yml`**

Create the file with exactly this content (no SPDX header — repo YAML config files don't carry one):

```yaml
version: 2
updates:
  # ---- Rust: Cargo workspace at rs/ ----
  - package-ecosystem: cargo
    directory: /rs
    schedule:
      interval: weekly
      day: monday
      time: "06:00"
      timezone: Etc/UTC
    commit-message:
      prefix: "build(deps)"
      prefix-development: "build(deps)"
    groups:
      cargo-minor-patch:
        applies-to: version-updates
        update-types: ["minor", "patch"]

  # ---- JS/TS: pnpm workspace + catalog at ts/ ----
  - package-ecosystem: npm
    directory: /ts
    schedule:
      interval: weekly
      day: monday
      time: "06:00"
      timezone: Etc/UTC
    commit-message:
      prefix: "build(deps)"
      prefix-development: "build(deps)"
    groups:
      npm-minor-patch:
        applies-to: version-updates
        update-types: ["minor", "patch"]

  # ---- Python: uv workspace at py/ ----
  - package-ecosystem: uv
    directory: /py
    schedule:
      interval: weekly
      day: monday
      time: "06:00"
      timezone: Etc/UTC
    commit-message:
      prefix: "build(deps)"
      prefix-development: "build(deps)"
    groups:
      uv-minor-patch:
        applies-to: version-updates
        update-types: ["minor", "patch"]

  # ---- GitHub Actions: workflows at repo root ----
  - package-ecosystem: github-actions
    directory: /
    schedule:
      interval: weekly
      day: monday
      time: "06:00"
      timezone: Etc/UTC
    commit-message:
      prefix: "ci(deps)"
    groups:
      actions-minor-patch:
        applies-to: version-updates
        update-types: ["minor", "patch"]
```

- [ ] **Step 3: Re-run the structural check — expect PASS**

```bash
python3 - <<'PY'
import sys, yaml
cfg = yaml.safe_load(open('.github/dependabot.yml'))
ecos = {u['package-ecosystem']: u for u in cfg['updates']}
assert cfg['version'] == 2
assert set(ecos) == {'cargo', 'npm', 'uv', 'github-actions'}, set(ecos)
assert ecos['cargo']['directory'] == '/rs'
assert ecos['npm']['directory'] == '/ts'
assert ecos['uv']['directory'] == '/py'
assert ecos['github-actions']['directory'] == '/'
assert ecos['cargo']['commit-message']['prefix'] == 'build(deps)'
assert ecos['npm']['commit-message']['prefix'] == 'build(deps)'
assert ecos['uv']['commit-message']['prefix'] == 'build(deps)'
assert ecos['github-actions']['commit-message']['prefix'] == 'ci(deps)'
for k in ecos:
    g = next(iter(ecos[k]['groups'].values()))
    assert g['applies-to'] == 'version-updates', (k, g)
    assert g['update-types'] == ['minor', 'patch'], (k, g)
    assert ecos[k]['schedule']['interval'] == 'weekly', k
print('dependabot.yml OK')
PY
```
Expected: `dependabot.yml OK` (exit 0).

- [ ] **Step 4: Commit**

```bash
git add .github/dependabot.yml
git commit -m "feat(ci): add Dependabot grouped weekly updates (SMA-362)" \
  -m "Grouped minor+patch weekly updates for cargo (/rs), npm (/ts), uv (/py),
and github-actions (/). Subjects use build(deps)/ci(deps) so they pass commitlint;
majors stay un-grouped for individual review.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```
Expected: the lefthook `commit-msg` hook runs commitlint and prints `✔️ commitlint`; commit succeeds.

---

## Task 3: Open the PR and confirm the repo's own CI is green

This verifies the *config-adding change* passes the repo's gates (commitlint over the PR commit range + `moon ci`). It does **not** yet produce Dependabot PRs — see Task 4.

**Files:** none (push + PR).

- [ ] **Step 1: Push the branch**

```bash
git push -u origin feature/sma-362-dependabot-cla-bot-setup
```

- [ ] **Step 2: Open the PR**

```bash
gh pr create \
  --base main \
  --title "feat(ci): add Dependabot grouped weekly updates (SMA-362)" \
  --body "Adds .github/dependabot.yml (cargo/npm/uv/github-actions, grouped minor+patch, weekly). See docs/superpowers/specs/2026-06-04-sma-362-dependabot-design.md. CLA split to SMA-408.

🤖 Generated with [Claude Code](https://claude.com/claude-code)"
```
(Do not attach a Linear link — the `sma-362` branch name auto-links the issue.)

- [ ] **Step 3: Wait for CI and confirm green**

```bash
gh pr checks --watch
```
Expected: the `moon ci` workflow passes — including the `commitlint` step over the PR range and `moon ci :build :test :lint :fmt :typecheck …`. GitHub also surfaces any `dependabot.yml` syntax error here; expect none.

- [ ] **Step 4: Merge once green**

Merge via the repo's normal flow (e.g. `gh pr merge --squash` if that's the convention, or merge in the UI). Dependabot version updates are read from the **default branch**, so the config only goes live after this lands on `main`.

---

## Task 4: Trigger Dependabot and verify the first generated PRs

Satisfies the AC "First Dependabot PR runs cleanly through CI." Requires the config to be on `main` (Task 3 Step 4). Dependabot's "check for updates" trigger has no stable CLI; use the GitHub UI, then inspect the resulting PRs with `gh`.

**Files:** none (observational; may produce follow-up fixes).

- [ ] **Step 1: Trigger an immediate run (don't wait for Monday)**

Open the Dependabot page and click **"Check for updates"** for each ecosystem:
```bash
gh repo view --web   # then: Insights → Dependency graph → Dependabot
```
Expected: the page lists all four ecosystems (cargo, npm, uv, github-actions) with **no config error**. If any ecosystem shows a parse/validation error, fix `.github/dependabot.yml` on a new branch and re-PR before continuing.

- [ ] **Step 2: List the PRs Dependabot opened**

```bash
gh pr list --author "app/dependabot" --state open
```
Expected: at most one **grouped** PR per ecosystem that has eligible minor/patch updates (some ecosystems may legitimately have nothing to bump and open no PR).

- [ ] **Step 3: Verify each Dependabot PR**

For each PR from Step 2:
```bash
gh pr view <number>                 # subject line + body
gh pr checks <number> --watch       # CI must go green
```
Confirm for each:
- Subject is `build(deps): …` (cargo/npm/uv) or `ci(deps): …` (github-actions).
- `moon ci` and the `commitlint` step both pass (the body's URL lines are exempt via commitlint's built-in URL carve-out — no predicate needed).

- [ ] **Step 4: Watch the known-risky ecosystems (catalogs first, then uv, then actions)**

- **npm (highest risk)** — open the npm PR's diff and confirm it bumps a `catalog:` entry in `ts/pnpm-workspace.yaml` **and** updates `ts/pnpm-lock.yaml` to a resolvable state (the lockfile-install in CI passing is the proof). If Dependabot's catalog support is regressed (it has open defects, dependabot-core #11953 / #14339) and the npm PR is empty or ships a broken lockfile: note it on the PR, fall back to manual catalog bumps for now, and **do not block** the other three ecosystems on it.
- **uv (medium risk)** — confirm the uv PR actually updates `py/uv.lock` (Dependabot won't bump a dep in `uv.lock` that has no constraint in `pyproject.toml`, and has case-sensitivity bugs). If a known-stale dep is missing, record it; not a blocker for landing the config.
- **github-actions (low risk)** — confirm the PR bumps both the pinned SHA and the trailing `# v4`/`# v0` version comment in `.github/workflows/ci.yml`.

- [ ] **Step 5: Record the outcome on SMA-362**

Add a short comment to the Linear issue (or the PR) noting which ecosystems produced a clean first PR and any caveats hit (e.g. catalog fallback used). This closes the "First Dependabot PR runs cleanly through CI" AC with evidence.

---

## Notes & fallbacks (do not implement pre-emptively)

- **No commitlint change.** Verified: commitlint's `body-max-line-length` ignores URL-containing lines, and Dependabot bodies are URL-laden, so they pass unchanged. **Reactive fallback only** — if a real Dependabot PR ever fails CI on `body-max-line-length` (a >100-char line *without* a URL, e.g. a very long group name), add to `ts/commitlint.config.cjs` (the consumer config, **not** the published `@paigasus/commitlint-config`):
  ```js
  ignores: [(m) => /^Signed-off-by: dependabot\[bot\]/m.test(m)],
  ```
  Local lefthook already skips `*[bot]@*` authors, so CI is the only path that would need it.
- **Majors are un-grouped by design** — a week with N major bumps in one ecosystem yields N separate PRs (so a breaking change isn't hidden in a green group). This is a deliberate reading of "one PR per ecosystem per week"; flagged for the SMA-363 foundation gate.
- **Out of scope:** CLA bot (→ SMA-408), proto-managed CLIs (`buf`/`lefthook`/`moon`/`release-plz` — no Dependabot ecosystem), and Dependabot security updates (a repo-Settings toggle).

---

## Self-Review

**Spec coverage:**
- `.github/dependabot.yml`, 4 ecosystems, grouped minor+patch, weekly → Task 2. ✓
- Conventional subjects `build(deps)`/`ci(deps)` → Task 1 (proves the choice) + Task 2 (encodes it) + Task 4 Step 3 (observed). ✓
- "at most one PR per ecosystem per week" / majors-separate → encoded by the group `update-types` in Task 2; verified Task 4 Step 2. ✓
- "First Dependabot PR runs cleanly through CI" → Task 4 Step 3. ✓
- No commitlint change (URL carve-out) → Notes; reactive fallback documented. ✓
- Catalog / uv caveats → Task 4 Step 4. ✓

**Placeholder scan:** every code/command step contains the exact content; no TBD/TODO. ✓

**Type/value consistency:** ecosystem keys (`cargo`/`npm`/`uv`/`github-actions`), directories (`/rs`,`/ts`,`/py`,`/`), prefixes (`build(deps)`/`ci(deps)`), group `update-types: ["minor","patch"]`, and `applies-to: version-updates` are identical across the config, the structural check, and the verification steps. ✓
