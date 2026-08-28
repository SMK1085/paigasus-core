# SMA-593 — Parse the workflow YAML in `repo:publish-metadata`'s P-D6 credential guard

**Status:** Approved (gate 1, 2026-08-28).
**Date:** 2026-08-28
**Linear:** [SMA-593](https://linear.app/smaschek/issue/SMA-593/ci-repopublish-metadatas-p-d6-credential-guard-misses-five-ordinary)
— follow-up from [SMA-578](https://linear.app/smaschek/issue/SMA-578)'s whole-branch review,
deliberately deferred there.
**Branch:** `feature/sma-593-close-p-d6-credential-spelling-gaps`
**Targets:** `main` (currently `82fe78e`, after SMA-560 landed).
**References:** SMA-407 §7 review M2 (the rule this gate enforces); SMA-578 (which added
P-D6); SMA-542 (guard-the-guard — a control that lies is worse than no control);
SMA-529/SMA-530 (negative controls); SMA-541/SMA-553/SMA-572 (gate bookkeeping);
SMA-579 (which makes this gate load-bearing).

---

## 1. The rule P-D6 enforces

`.github/workflows/wheels.yml` carries a `pull_request` trigger. Same-repo pull requests
receive repository secrets. A registry credential in that workflow is therefore readable by
any code the pull request introduces. SMA-407 §7 review M2 forbids it. `wheels.yml`'s own
header comment (line 9) states the ban and names this gate as the thing that asserts it.

`assert_wheels_has_no_credentials` in `ci/publish-metadata/run.sh` is that assertion. It
strips YAML comments with a hand-written scanner, then applies three regular expressions to
the remaining text.

## 2. The measurement

Every claim below comes from driving the real `strip_comments` and `PATTERNS` code, not
from reading it. `rc 0` means the checker accepted the input. The workflow is a credential
leak and the gate stays silent.

**The five spellings the issue reports. All five are confirmed.**

| # | Input | rc |
|---|---|---|
| 1 | `permissions: { note: "a \" # z", id-token: write }` | 0 |
| 2 | `id-token: 'write'` | 0 |
| 3 | `"id-token": write` | 0 |
| 4 | `${{ secrets['PYPI_API_TOKEN'] }}` | 0 |
| 5 | `${{ Secrets.PYPI_API_TOKEN }}` | 0 |

**Nine more that the issue does not list. All were found in one sitting.**

| # | Input | rc | Class |
|---|---|---|---|
| 6 | `id-token: "write"` | 0 | quoted value |
| 7 | `'id-token': write` | 0 | quoted key |
| 8 | `"secrets": inherit` | 0 | quoted key |
| 9 | `${{ secrets["PYPI_API_TOKEN"] }}` | 0 | index form |
| 10 | `${{ SECRETS.PYPI_API_TOKEN }}` | 0 | context case |
| 11 | `${{ secrets[ 'X' ] }}` | 0 | index form |
| 12 | `permissions: write-all` (workflow level) | 0 | implicit grant |
| 13 | `permissions: write-all` (job level) | 0 | implicit grant |
| 14 | `x: &w write` … `id-token: *w` | 0 | YAML alias |

Rows 12 and 13 matter most. `write-all` grants every permission scope, `id-token` included.
It is what a person writes to make a step work. It is ordinary YAML and it needs no craft.

Row 14 matters for a different reason. The banned value never stands next to its key in the
text. No regular expression over raw text reaches it cleanly.

**One result that is not a hazard.** `ID-TOKEN: WRITE` also returns rc 0. GitHub Actions
reads workflow schema keys case-sensitively, so that workflow does not grant the permission.
The count of real gaps is 14, not 15. This distinction is recorded to keep the count honest.

**Four inputs are already caught,** and the redesign must keep them caught: a single-line
`run:` block scalar carrying `${{ secrets.X }}`, the no-space form `${{secrets.X}}`, a
double-quoted `"${{ secrets.X }}"` value, and `permissions: {id-token: write}`.

**What a parser sees.** `id-token: 'write'`, `"id-token": write`, the alias form and the
merge-key form all parse to the identical mapping `{'id-token': 'write'}`. One structural
test replaces the whole quoting and aliasing column. This is the argument for section 4.

## 3. Decision: parse, and widen the subject

The issue offers two routes. Route 1 extends the regular expressions. Route 2 parses the
YAML. The measurement decides it. Route 1 is incomplete by construction: 9 spellings beyond
the reported 5 appeared in one sitting, and row 14 has no clean regular-expression answer.
A longer pattern table makes the check look complete while it stays incomplete. SMA-542
records what that failure mode costs.

Two decisions were taken at gate 1.

**Decision A — parse the YAML.** Obtain PyYAML through the pinned `uv`:
`uv run --no-project --with 'pyyaml==6.0.3' python3`. `uv` is pinned in `.prototools`,
`moon setup` installs it before `moon ci`, and CI restores the uv cache before `moon ci`
runs. Measured cost on a warm cache: 0.19 s. The exact version pin keeps the gate
deterministic. A PyYAML change cannot alter the verdict without a visible edit here.

Nothing under `ci/` imports `yaml` today, and PyYAML is absent from the `py/` workspace.
This is the dependency implication the issue flags. The pinned-`uv` route answers it without
a gamble on what the runner's system Python contains.

**Decision B — widen the subject from `wheels.yml` to every pull-request-triggered
workflow.** The hazard is a property of the trigger, not of one file. Measured today: five
of six workflows carry a `pull_request` trigger (`ci.yml`, `images.yml`, `prebuild.yml`,
`security-scan.yml`, `wheels.yml`) and **none of them uses a secret**. `release.yml` is the
only workflow that reads secrets, and it has no `pull_request` trigger. The invariant
already holds repo-wide, so widening costs nothing today and guards the class instead of the
instance.

Gate 1 chose runtime discovery with an allowlist. It did **not** choose a pinned expected
subject set. Section 5 adds a bare non-empty assertion instead, which closes the vacuity
hole at near-zero maintenance cost.

## 4. The checker

The checker parses each workflow with `yaml.safe_load_all` and walks every document. Four
rules apply to the parsed tree.

| Rule | Condition | Closes |
|---|---|---|
| R1 | a mapping key equals `secrets` | rows 8; `secrets:`, `secrets: inherit`, flow mappings, `workflow_call` pass-through |
| R2 | a mapping key equals `id-token` and its value is `write` | rows 1, 2, 3, 6, 7, 14 |
| R3 | a mapping key equals `permissions` and its scalar value is `write-all` | rows 12, 13 |
| R4 | a scalar string matches the secrets-context pattern | rows 4, 5, 9, 10, 11 |

R2 and R3 compare the parsed value after `str(value).strip().lower()`. Actions rejects a
case-varied value, so the lowercase comparison only ever adds a conservative red.

R4's pattern is `\$\{\{[^}]*\bsecrets\s*(?:\.|\[)` with `re.IGNORECASE`. It requires the
`${{` wrapper, so the word "secrets" in ordinary prose does not match. `re.IGNORECASE` is
correct here and only here: Actions expression **context names** are case-insensitive, while
workflow **schema keys** are not.

**Comment handling disappears.** PyYAML never returns a comment, so `strip_comments` and its
whole class of defects leave the codebase. Row 1 closes because the escape bug no longer
exists, not because the escape rule was fixed. A `#` inside a block scalar is literal by
construction. A block scalar arrives as an ordinary scalar value, so R4 covers `run:` bodies
without a special case.

## 5. Discovery

The checker globs `.github/workflows/*.yml`. A workflow is a subject when its triggers
include `pull_request` or `pull_request_target`. `pull_request_target` is included because it
is strictly more dangerous: it runs with the base repository's secrets.

Two traps must be handled explicitly.

- **The YAML 1.1 boolean trap.** PyYAML parses the `on:` key as the boolean `True`, so
  `doc.get("on")` returns `None`. The checker must read both the `"on"` key and the `True`
  key.
- **Three trigger shapes.** `on:` appears as a string (`on: pull_request`), as a list
  (`on: [push, pull_request]`), and as a mapping. The checker normalises all three to a set.

`PR_CREDENTIAL_ALLOWED` maps a workflow filename to a stated reason, following the repo's
`T_EXEMPT` and `ALLOW_DEAD_INPUT` pattern. It is empty when this lands.

**Non-vacuity.** Discovery that returns nothing is a gate that asserts nothing. Zero
discovered subjects is rc 2, never rc 0. An allowlist entry that names a file which does not
exist is also rc 2, so a stale exemption reds instead of silently widening.

## 6. Exit codes

| Code | Meaning |
|---|---|
| 0 | every subject is credential-free |
| 1 | a subject declares a credential |
| 2 | infrastructure: a file is unreadable, a file does not parse, PyYAML is unavailable, discovery found no subject, or an allowlist entry is stale |

A parse failure is rc 2 by decision, not rc 1. `repo:actionlint` owns workflow YAML validity.
This gate cannot assert on a tree it does not have, so it reports that it could not run.

**Accepted new failure mode.** A cold uv cache with PyPI unreachable makes the gate exit 2.
The gate reds loudly rather than passing silently. That ordering is deliberate.

## 7. Non-goals

The gate asserts **declaration**. It does not assert that no credential can reach the
workflow by any path. These are out of reach and are stated so the check does not overclaim:

- an expression built by concatenation, or laundered through `${{ env.X }}` — both need
  dataflow analysis;
- a credential a third-party action fetches by itself;
- a credential reaching the workflow through a `workflow_call` from a caller.

## 8. Control table

`ci/publish-metadata/run.sh --negative-control` gains one row per closed spelling. Every one
of the 14 measured bypasses gets its own row, so each is pinned rather than assumed. The
four already-caught inputs from section 2 keep their rows, which is what proves the redesign
did not trade old coverage for new.

Discovery gets its own rows: a `pull_request` workflow is selected; `release.yml` is not
selected; zero subjects is rc 2; a stale allowlist entry is rc 2; an allowlist entry
suppresses a real subject.

The `release.yml` row is the strongest of these, and section 11 explains why: that workflow
fails the credential rules and passes the gate only because discovery excludes it. A row that
asserts both halves proves the trigger filter does real work.

At least one rc-0 row stays in the table. Without it a checker that fails unconditionally
would satisfy every other row and the table would prove nothing.

## 9. Files, and the obligation the widening forces

| File | Change |
|---|---|
| `ci/publish-metadata/workflow_credentials.py` | new — discovery, the four rules, the exit codes |
| `ci/publish-metadata/run.sh` | replace `assert_wheels_has_no_credentials`; add the control rows |
| `ci/publish-metadata/README.md` | document the check, the allowlist, and the non-goals |
| `moon.yml` | `publish-metadata` inputs gain `.github/workflows/*.yml` and the new `.py` |
| `ci/affected-graph/ci_targets.py` | re-baseline `SELF_TASK_EXPECTED_GLOBS["publish-metadata"]` |
| `CLAUDE.md` | record the gotchas |

**The obligation.** The gate now reads every workflow, so `moon.yml`'s `publish-metadata`
`inputs` must gain `.github/workflows/*.yml`. It must be a glob, not a list of literal paths.
A glob selects the task for a workflow that does not exist yet; a literal list cannot. This
is the same argument the existing comment already makes for the `py/packages/*` globs.

`SELF_TASK_EXPECTED_GLOBS["publish-metadata"]` in `ci/affected-graph/ci_targets.py` pins that
input list by **exact match**. It must be re-baselined in the same commit, or
`repo:affected-smoke` reds. The new `.py` file needs an entry in both places too, because
that tuple lists `ci/publish-metadata/` files literally rather than by glob.

`repo:input-liveness` asserts every declared glob still matches a tracked file.
`.github/workflows/*.yml` matches six files, so it passes.

## 10. Risks

- **A conservative red on an unrelated `secrets` key.** R1 matches any mapping key named
  `secrets`, including a `with:` input to a third-party action. The red is deliberate: such
  an input does pass a secret. The allowlist carries the exemption if one is ever justified.
- **A new dependency on `uv` for a `toolchain: 'system'` task.** The task keeps
  `toolchain: 'system'` and calls the `uv` shim, as other tasks call `cargo` or `buf`.
- **PyYAML is pinned outside any lockfile,** so Dependabot does not see it. The pin is a
  literal in `run.sh`. The API used (`safe_load_all`) is old and stable, so the risk of a
  silent behaviour change is low. The README records where the pin lives.
- **SMA-579 interaction.** `wheels.yml` is a reusable workflow that SMA-579's gated
  `release` job will call. It carries a `pull_request` trigger, so it stays a subject and
  must never declare `secrets:`. A credential for the real publish path belongs in
  `release.yml`, which has no `pull_request` trigger and is therefore not a subject.

## 11. Verification of this design

The rules in section 4 and the discovery in section 5 were built as a prototype and driven
against every case before this document was approved. The design is measured, not asserted.

**All 14 bypasses from section 2 now report rc 1.** The rule that catches each one matches
the mapping in section 4's table.

**All six inputs the old checker already caught stay caught:** the single-line `run:` block
scalar, the no-space `${{secrets.X}}`, the double-quoted `"${{ secrets.X }}"` value,
`permissions: {id-token: write}`, the YAML merge key, and a `workflow_call` `secrets:`
declaration. The redesign trades no old coverage for new.

**All six honest passes stay green:** `contents: read`; a header comment that quotes the ban;
a `#` inside a quoted scalar; the word "secrets" in ordinary prose; `permissions: read-all`;
and `id-token: none`. The second of these is the case that forced `strip_comments` to exist.
The parser makes it green with no comment handling at all.

**Discovery over the six real workflows:**

| Workflow | Subject | Verdict if it were checked |
|---|---|---|
| `ci.yml` | yes | rc 0 |
| `images.yml` | yes | rc 0 |
| `prebuild.yml` | yes | rc 0 |
| `security-scan.yml` | yes | rc 0 |
| `wheels.yml` | yes | rc 0 |
| `release.yml` | **no** | **rc 1** |

The last row is the important one. `release.yml` fails the credential rules, because it
genuinely reads secrets, and it passes the gate only because discovery excludes it. The
trigger filter is therefore load-bearing, not decorative. Section 8 pins both halves of that
row: the workflow is not a subject, and its content would red if it were. A change that
broke discovery into selecting everything would red on this row instead of passing.
