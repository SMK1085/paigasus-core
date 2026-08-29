# SMA-580 Release Activation E — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship the three artifacts that let the owner execute the release activation sequence, and add the temporary `workflow_dispatch` trigger step I needs.

**Architecture:** No production code changes and no new CI gates. Two committed artifacts (a `release.yml` edit and an operator runbook) plus one drafted Notion artifact. The workflow edit is comments plus one trigger key; the runbook carries the whole operational sequence, including six steps the agent never executes.

**Tech Stack:** GitHub Actions YAML, Markdown. Verification runs through `moon run repo:actionlint` and `moon ci`.

**Spec:** `docs/superpowers/specs/2026-08-29-sma-580-release-activation-e-design.md`

## Global Constraints

- **This branch publishes nothing.** No task runs `cargo publish`, `npm publish`, `maturin upload`, `cargo yank`, `gh variable set`, or `gh api -X PUT .../environments`. Those are runbook content, executed by the owner later.
- Every source file opens with an SPDX header: `// SPDX-License-Identifier: Apache-2.0` (`#` for Python). **Markdown files carry none** — check the existing `docs/ops/RUNBOOK-*.md` files, which have no header.
- Conventional commits with a workspace scope. This branch uses `docs(repo):` and `ci(repo):`.
- Branch: `feature/sma-580-release-activation-e-pre-flight-checklist-flip`. Already checked out.
- `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` before any `moon`, `uv`, `buf` or `cargo` command. The Bash tool's PATH lacks the proto-managed CLIs.
- Prose follows ASD-STE100 Simplified Technical English: short sentences, active voice, no idiom. Technical names (`release-plz`, `PAIGASUS_RELEASE_ENABLED`, `workflow_dispatch`) are never reworded.
- **`release.yml` must never gain `pull_request` or `pull_request_target`.** `workflow_dispatch` is the only trigger this branch adds.

---

### Task 1: The `release.yml` trigger and the two App comments

**Files:**
- Modify: `.github/workflows/release.yml` — the `on:` block (lines 20-24), and both `Mint the App installation token` steps (near lines 137 and 357)

**Interfaces:**
- Consumes: nothing.
- Produces: a `workflow_dispatch` trigger on `release.yml`, which Task 2's runbook step I depends on by name.

- [ ] **Step 1: Record the current gate state, so Step 6 can prove nothing regressed**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:actionlint 2>&1 | tail -5
```

Expected: PASS. If it already fails, stop and report — this task must not be built on a red baseline.

- [ ] **Step 2: Add the `workflow_dispatch` trigger**

Replace the `on:` block. It currently reads:

```yaml
on:
  push:
    branches:
      - main
```

with:

```yaml
on:
  push:
    branches:
      - main
  # TEMPORARY (SMA-580). Step I of the activation sequence dispatches this workflow explicitly
  # rather than re-running a run whose jobs skipped, which removes two premises about re-run
  # semantics that SMA-580's design never measured: that a re-run re-reads repository variables,
  # and that it re-executes jobs that previously skipped.
  #
  # (The shipped comment is longer: local review found that the two mitigations named here do NOT
  # bound the trigger, because a dispatch runs the workflow file from the DISPATCHED REF. See the
  # committed `release.yml` and spec §3.3 for the real boundary — the `release-publish` environment
  # branch policy plus the registries' required environment claim.)
  #
  # REMOVE once the first release has published and its verification has passed — see
  # docs/ops/RUNBOOK-release-activation.md step J. NO GATE ENFORCES THAT REMOVAL.
  workflow_dispatch:
```

Add no `inputs:`. A bare `workflow_dispatch` gives `repo:actionlint`'s branches-filter extractor nothing to parse, so it needs no `BRANCH_SKIP` entry.

- [ ] **Step 3: Verify the YAML parses and the trigger set is exactly what was intended**

```bash
python3 -c "
import yaml
d = yaml.safe_load(open('.github/workflows/release.yml'))
on = d.get('on', d.get(True))
print('triggers:', sorted(on.keys()))
assert sorted(on.keys()) == ['push', 'workflow_dispatch'], on.keys()
assert 'pull_request' not in on and 'pull_request_target' not in on
assert on['workflow_dispatch'] is None, 'workflow_dispatch must carry no inputs'
print('OK')
"
```

Expected: `triggers: ['push', 'workflow_dispatch']` then `OK`.

Note the `d.get('on', d.get(True))` form. PyYAML parses a bare `on:` key as the **boolean** `True`, which is why every workflow parser in `ci/` handles both.

- [ ] **Step 4: Add the "which App" line to the FIRST mint step**

In the `release-pr` job, the comment block above `- name: Mint the App installation token` ends with:

```
      # The action registers the token as a log mask and revokes it in its post-step, so it
      # does not outlive the run.
```

Insert immediately after that paragraph, before the `- name:` line:

```
      #
      # WHICH APP: the EXISTING Paigasus bot GitHub App. Do not create a second one. If this job
      # skips, the cause is a missing `PAIGASUS_BOT_APP_ID` secret, not a missing App.
```

- [ ] **Step 5: Add the same line to the SECOND mint step**

The `release` job has its own mint step with a much shorter comment:

```
      # A SECOND mint: tokens are per-job and live one hour. Every checkout in this repo sets
      # persist-credentials: false, so there are no ambient git credentials and release-plz's
      # tag push needs this explicitly.
```

Append to that block, before the `- name:` line:

```
      # Same App as the release-pr job above: the EXISTING Paigasus bot GitHub App.
```

Both steps are required. The spec's §6.2 says "at **both** mint steps" because a reader debugging the `release` job may never scroll to the `release-pr` job.

- [ ] **Step 6: Verify the comments landed at both sites and the gates stay green**

```bash
grep -c "EXISTING Paigasus bot GitHub App" .github/workflows/release.yml
```

Expected: `2`.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:actionlint --force 2>&1 | tail -5
```

Expected: PASS, matching Step 1's baseline. `--force` is required: the task is cached, and Step 1 already ran it.

- [ ] **Step 7: Commit**

```bash
git add .github/workflows/release.yml
git commit -m "ci(repo): add a temporary release dispatch trigger and name the App (SMA-580)

Step I of the activation sequence needs an explicit trigger. A re-run of a
run whose jobs skipped rests on two premises the design never measured: that
a re-run re-reads repository variables, and that it re-executes skipped jobs.
A workflow_dispatch trigger removes both.

The trigger cannot publish on its own. PAIGASUS_RELEASE_ENABLED still gates
every build job, and approve-release still pauses for a required reviewer.
It is temporary; the runbook's step J removes it and no gate enforces that.

Both mint steps now name the existing Paigasus bot GitHub App, so a reader
debugging a skipped job does not create a second one."
```

---

### Task 2: The operator runbook

**Files:**
- Create: `docs/ops/RUNBOOK-release-activation.md`

**Interfaces:**
- Consumes: Task 1's `workflow_dispatch` trigger, named in step I.
- Produces: the document Task 3's ADR amendment cross-references, and the document the owner executes steps C, D, E, H, I and J from.

**Content source.** Every fact comes from the spec. Do not invent values. The mapping is:

| Runbook section | Spec section |
| --- | --- |
| Step D's six edits and the publish commands | §2.2.1 – §2.2.4 |
| Why the tree must not be a git repository | §2.5, §11.2 |
| Step F's three-way decision table | §3.1 |
| The environment settings | §4.1, §4.2 |
| PyPI / npm / crates.io field values | §5.1, §5.2, §5.3 |
| The local crates.io token's scope and revocation | §5.4 |
| Verification, including installability | §7 |
| Partial-failure recovery | §8.1 |
| The two tracked removals | §9.1 |

- [ ] **Step 1: Write the runbook**

Create `docs/ops/RUNBOOK-release-activation.md`. No SPDX header — the other `docs/ops/RUNBOOK-*.md` files carry none.

Structure it as follows. Each numbered section is required.

1. **A header that states the stakes.** Name the issue, link the spec by relative path (`../superpowers/specs/2026-08-29-sma-580-release-activation-e-design.md`), and state in the first paragraph that crates.io allows only `cargo yank`, PyPI allows delete but never reuse, and npm allows unpublish within 72 hours only. Say plainly that steps D and I are irreversible and every other step is not.
2. **The step table**, copied from spec §3, with the trigger and owner columns intact.
3. **One section per step, A through J**, each with the exact commands. Steps A and B are already done by the time the owner reads this — mark them so.
4. **Step D in full detail.** This is the section that must not be paraphrased:
   - the `git archive` command and the `git -C /tmp/seed rev-parse --show-toplevel` assertion that MUST fail
   - the six-edit table, verbatim from spec §2.2.2, including the line numbers
   - the pre-upload assertion loop from §2.2.3
   - the publish commands from §2.2.4, **without** `--allow-dirty`, with the `cargo info` poll between derive and proto
   - a bolded warning that `--allow-dirty` must never be added, with the measured reason from §11.2: in a git tree it converts cargo's hard error into a silent success that embeds the SHA1
5. **Step F's decision table**, all three rows, keyed on the `release-pr` job's `--output json` line. Include the instruction to identify the release PR by `.prs[0].number`, never by the literal 170.
6. **Step I**, using `gh workflow run release.yml --ref main`, followed by "approve the `approve-release` deployment when it enters `waiting`".
7. **A verification section** carrying spec §7 whole, including the three installability checks and the note that they run inside the 72-hour npm window.
8. **A recovery section** carrying spec §8.1's four-row table.
9. **A closing section listing the two tracked removals** from §9.1, with the note that no gate enforces either.

Keep the prose in Simplified Technical English. Keep every command copy-pasteable.

- [ ] **Step 2: Verify the runbook has no unresolved placeholder and no forbidden flag**

```bash
grep -nE "TBD|TODO|FIXME|XXX|<fill|allow-dirty" docs/ops/RUNBOOK-release-activation.md
```

Expected: the ONLY matches are the lines that **warn against** `--allow-dirty`. Zero matches for `TBD`, `TODO`, `FIXME`, `XXX` and `<fill`. If a bare `cargo publish --allow-dirty` command appears anywhere, remove it — that is the Task's one hard failure.

- [ ] **Step 3: Verify every relative link resolves**

```bash
python3 -c "
import re, os
p = 'docs/ops/RUNBOOK-release-activation.md'
bad = [l for l in re.findall(r']\((\.\.?/[^)]+)\)', open(p).read())
       if not os.path.exists(os.path.normpath(os.path.join(os.path.dirname(p), l.split('#')[0])))]
print('broken:', bad); assert not bad
"
```

Expected: `broken: []`.

- [ ] **Step 4: Verify the six-edit table matches the real line numbers**

The runbook cites `rs/Cargo.toml:140`, `:143` and `:146`. Confirm they still point at the three pins:

```bash
sed -n '140p;143p;146p' rs/Cargo.toml
```

Expected: the three `[workspace.dependencies]` lines for `paigasus-proto-derive`, `paigasus-kernel` and `paigasus-proto`. If a line number has drifted, correct the runbook — do not correct `rs/Cargo.toml`.

- [ ] **Step 5: Commit**

```bash
git add docs/ops/RUNBOOK-release-activation.md
git commit -m "docs(repo): operator runbook for the release activation (SMA-580)

Carries the activation sequence the owner executes: the crates.io seed, the
two GitHub environments, the three registry configurations, the observation
gate, the flip and the verification.

Step D is the section that must not be paraphrased. The seed tree must not be
a git repository, and --allow-dirty must never be added: in a git tree it
converts cargo's hard error into a silent success that embeds the commit SHA1,
which would truncate the first release's changelog."
```

---

### Task 3: The ADR-0011 amendment draft

**Files:**
- Create: `docs/superpowers/specs/2026-08-29-sma-580-adr-0011-amendment-draft.md`

This is a **draft for the owner to paste into Notion**. ADRs live in Notion, not in this repository — CLAUDE.md says so, and there is no `docs/adr/` directory. Committing the draft is what makes it reviewable in the pull request; the owner applies it to the Notion page separately.

**Interfaces:**
- Consumes: Task 2's runbook, cross-referenced by name.
- Produces: nothing other tasks consume.

- [ ] **Step 1: Read the current ADR-0011 to match its amendment style**

The page is `ADR-0011: Polyglot versioning & release strategy`. Its two existing amendments are headed `## Amendment — 2026-06-04 (SMA-406, E4)` and carry numbered sub-points. Match that shape exactly: a dated heading naming the issue, then numbered items.

Its **Status** line currently reads:

```
**Status:** Accepted *(amended 2026-06-03 · SMA-405: … ; amended 2026-06-04 · SMA-406: …)*
```

The draft must include the replacement Status line, with `amended 2026-08-29 · SMA-580: crates.io bootstrap exception to S3` appended inside the parentheses.

- [ ] **Step 2: Write the draft with all five items**

Write the five items from spec §6.4, in that order, each as a numbered sub-point:

1. **S1 clarification** — proto's lockstep is realized structurally, through the committed generated code plus S5 file-path attribution. No contract version is introduced.
2. **S4 activation shape** — `release-pr` is live; `release` is gated behind `vars.PAIGASUS_RELEASE_ENABLED`. The guard lives in `ci/actionlint/run.sh` and protects the mechanism, not the decision.
3. **Decision G deferred again** — semantic-release ejects `@paigasus/sdk` and `@paigasus/ui` to `1.0.0` on their first breaking change, while release-plz and python-semantic-release stay in 0.x. Both packages are `private: true` at `0.0.0`, so semantic-release governs no package that publishes and the premise for the decision has not arrived. **State the reopening trigger: either package dropping `private: true`.**
4. **The temporary S1 exception** — `@paigasus/kernel` and `@paigasus/proto` sit at `0.0.0` while their family siblings move to `0.1.0`. They rejoin at the family's *current* version, not at `0.1.0`.
5. **NEW — the crates.io bootstrap exception to S3.** This is the item that needs the most care, because it reads as a contradiction of S3 and is not one. Write it to cover:
   - S3 says the tool owns every tag. The seed publishes three versions the tool did not cut.
   - **It places no tag.** That is why it does not reproduce the SMA-385 failure, which was caused by hand-placed *tags* carrying no release-plz metadata — not by hand-placed registry versions.
   - The reason it is unavoidable: RFC 3691 states a Trusted Publisher Configuration can only be created after an initial manual publish, and `PENDING` configurations are an unimplemented future possibility.
   - release-plz still owns every tag, including `0.1.0`.
   - The seeds are `0.1.0-alpha.1`, yanked after verification.
   - Cross-reference `docs/ops/RUNBOOK-release-activation.md` and the spec.

- [ ] **Step 3: Verify the draft covers all five items and states the reopening trigger**

```bash
python3 -c "
s = open('docs/superpowers/specs/2026-08-29-sma-580-adr-0011-amendment-draft.md').read()
for probe in ['S1', 'S3', 'S4', 'SMA-385', 'RFC 3691', 'private: true', '0.1.0-alpha.1', 'Status:']:
    assert probe in s, 'MISSING: ' + probe
print('all five items and the reopening trigger present')
"
```

Expected: `all five items and the reopening trigger present`.

- [ ] **Step 4: Commit**

```bash
git add docs/superpowers/specs/2026-08-29-sma-580-adr-0011-amendment-draft.md
git commit -m "docs(repo): draft the ADR-0011 amendment for release activation (SMA-580)

ADRs live in Notion, so this draft is committed only to make it reviewable in
the pull request. The owner applies it to the page.

Five items: the S1 structural-lockstep clarification, the S4 activation shape,
Decision G deferred again with its reopening trigger, the temporary S1 version
exception, and the new crates.io bootstrap exception to S3.

The bootstrap item reads as a contradiction of S3 and is not one. The seed
places no tag, which is what separates it from the SMA-385 failure."
```

---

### Task 4: Whole-graph verification

**Files:** none modified.

Per-project Moon tasks do not run the repo-level gates. This branch touches a workflow file and two documents, which selects `repo:actionlint`, `repo:input-liveness` and `repo:publish-metadata` at minimum — all three select on everything.

- [ ] **Step 1: Run the full affected graph the way CI does**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :http-extractor-envelope :input-liveness :promtool :observability-drift \
  :nats-permissions :release-parity :release-parity-py :release-parity-ts \
  :publish-metadata :version-lockstep :workflow-credentials --base origin/main \
  --include-relations 2>&1 | tail -30
```

Expected: all green.

- [ ] **Step 2: If `repo:affected-smoke` fails in under 3 seconds, capture the output before re-running**

That symptom has appeared twice, both times under a concurrent `moon ci`. Grep the captured output for `proto-shim`. If that line is present, the failure is an infrastructure abort rather than a red verdict, and `moon run repo:affected-smoke --force` passes. If it is absent, diagnose the failure on its own terms — the known entry does not explain it.

- [ ] **Step 3: If a Docker-backed `paigasus-iam` suite fails, baseline it before blaming this branch**

Those suites are flaky under parallel load. This branch changes no Rust code, so a failure there is not caused by it. Confirm by running the same suite on unmodified `origin/main`.

- [ ] **Step 4: Confirm the branch contains exactly the intended changes**

```bash
git diff --stat origin/main...HEAD
```

Expected: exactly five files — the spec, the plan, `.github/workflows/release.yml`, `docs/ops/RUNBOOK-release-activation.md`, and the ADR draft. **No Rust, Python or TypeScript source file may appear.** If one does, it was not part of this issue.

---

## Self-review

**Spec coverage.** §1 (pre-flight) is recorded in the spec and needs no task — it is measurement already performed. §2 (bootstrap) → Task 2 step D. §3 (order) → Task 2. §4 (environments) → Task 2, executed by the owner. §5 (credentials) → Task 2. §6.2 (App comment) and §6.3 (dispatch trigger) → Task 1. §6.1 (runbook) → Task 2. §6.4 (ADR) → Task 3. §7 (verification) → Task 2. §8.1 (recovery) → Task 2. §9.1 (tracked removals) → Task 1's comment and Task 2's closing section. §10.2's two remaining questions are the owner's to answer at steps J and E; neither blocks this branch.

**Gap accepted, deliberately.** Nothing in this plan enforces the removal of the `workflow_dispatch` trigger or of `NPM_TOKEN`. Both are named in §9.1 and in Task 1's inline comment. Building a gate for either is out of scope per spec §9.2.

**No placeholders.** Task 2 step 1 describes a document by required section rather than by literal text, which is the one place this plan does not embed the final content. That is deliberate: the runbook is ~300 lines and every fact is sourced from a named spec section in the mapping table above. Steps 2 through 4 are mechanical checks on the result.
