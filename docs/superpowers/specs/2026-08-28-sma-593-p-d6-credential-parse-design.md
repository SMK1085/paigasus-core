# SMA-593 — Parse the workflow YAML, and lift the credential guard into its own gate

**Status:** Revision 2. Revision 1 was challenged and returned NEEDS REWORK. This revision
folds in every justified finding and three decisions taken at gate 1.
**Date:** 2026-08-28
**Linear:** [SMA-593](https://linear.app/smaschek/issue/SMA-593/ci-repopublish-metadatas-p-d6-credential-guard-misses-five-ordinary)
— follow-up from [SMA-578](https://linear.app/smaschek/issue/SMA-578)'s whole-branch review.
**Branch:** `feature/sma-593-close-p-d6-credential-spelling-gaps`
**Targets:** `main` (currently `82fe78e`, after SMA-560 landed).
**References:** SMA-407 §7 review M2 (the rule); SMA-578 (which added P-D6); SMA-542
(guard-the-guard); SMA-529/SMA-530 (negative controls); SMA-541 (the `T` array and the
CLAUDE.md marker command); SMA-553/SMA-572/SMA-576 (gate bookkeeping); SMA-579.

---

## 1. The rule

`.github/workflows/wheels.yml` carries a `pull_request` trigger. Same-repo pull requests
receive repository secrets. A registry credential in that workflow is therefore readable by
any code the pull request introduces. SMA-407 §7 review M2 forbids it.

`assert_wheels_has_no_credentials` in `ci/publish-metadata/run.sh` is the current assertion.
It strips YAML comments with a hand-written scanner, then applies three regular expressions.

## 2. The measurement

Every claim comes from driving the real `strip_comments` and `PATTERNS` code. `rc 0` means
the checker accepted the input, so the workflow leaks and the gate stays silent.

**The five spellings the issue reports. All five confirmed.**

| # | Input | rc |
|---|---|---|
| 1 | `permissions: { note: "a \" # z", id-token: write }` | 0 |
| 2 | `id-token: 'write'` | 0 |
| 3 | `"id-token": write` | 0 |
| 4 | `${{ secrets['PYPI_API_TOKEN'] }}` | 0 |
| 5 | `${{ Secrets.PYPI_API_TOKEN }}` | 0 |

**Nine more the issue does not list.**

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

Rows 12 and 13 matter most. `write-all` grants every scope, `id-token` included. It is what
a person writes to make a step work. Row 14 matters differently: the banned value never
stands next to its key, so no regular expression over raw text reaches it cleanly.

**One result that is not a hazard.** `ID-TOKEN: WRITE` also returns rc 0. Actions reads
workflow schema keys case-sensitively, so that workflow grants nothing. The count of real
gaps is 14, not 15.

**The already-caught set.** Revision 1 stated this three different ways. Driving the old code
gives one answer, and it is six inputs, not four:

| Input | Old rc | Note |
|---|---|---|
| single-line `run:` block scalar with `${{ secrets.X }}` | 1 | |
| `${{secrets.X}}` (no space) | 1 | |
| `"${{ secrets.X }}"` (quoted value) | 1 | |
| `permissions: {id-token: write}` | 1 | |
| `workflow_call` `secrets:` declaration | 1 | |
| YAML merge key (`<<: *p`) | 1 | **caught by accident** |

The merge-key row is caught only because the anchor block `x: &p` / `id-token: write`
appears literally in the text, so the pattern matches the anchor **definition**, not the
merged result. It is recorded as accidental so a later refactor cannot lose it silently.
Its near relative, the alias form (row 14), is genuinely uncaught.

**What a parser sees.** Rows 2, 3, 6, 7, 14 and the merge-key form all parse to the identical
mapping `{'id-token': 'write'}`. One structural test replaces the whole column.

## 3. Decisions

Revision 1 proposed extending P-D6 in place. The adversarial review rejected three parts of
that. Three decisions were taken.

**Decision A — parse the YAML, and obtain PyYAML from the `py/` workspace.** Add
`"pyyaml>=6,<7"` to `py/pyproject.toml`'s `[dependency-groups] dev`, and invoke
`uv run --project py python3`.

Revision 1 proposed `uv run --no-project --with 'pyyaml==6.0.3'`. That is withdrawn. The
CI uv cache key is `uv-${{ runner.os }}-${{ hashFiles('py/uv.lock') }}`
(`.github/workflows/ci.yml:167`). PyYAML would not be in `py/uv.lock`, so the key would never
change, the restore would be an exact primary-key hit, and `actions/cache` skips its save on
an exact hit. PyYAML would be re-fetched from PyPI on **every CI run, indefinitely**. Putting
the dependency in `py/uv.lock` changes that key when it lands, so the cache saves. It also
puts the pin in a lockfile, where Dependabot sees it and `repo:osv` already scans it.

**Decision B — pin the discovered subject set by strict equality.** Revision 1 proposed only
a non-empty assertion. `ci/publish-metadata/run.sh` already holds two runtime-discovered sets
behind exact-equality expected lists (`EXPECTED_PUBLISHABLE`, `EXPECTED_PYPI_PUBLISHABLE`),
and Check P0's comment states the reason: a stale list silently **shrinks** the gate rather
than reporting red. A bare non-empty assertion does not survive an unbounded allowlist —
four entries with plausible reasons take the subject set from five to one and stay green.

**Decision C — lift the check into its own gate, `repo:workflow-credentials`.** Revision 1
widened `repo:publish-metadata`'s inputs to every workflow. `repo:publish-metadata` runs
`cargo publish --dry-run` per publish group plus a crates.io category check, and `ci.yml` is
the most-edited workflow in the repo, so every `ci.yml` edit would have paid that cost on a
required check. The credential check moves out instead, with narrow inputs. P-D6 leaves
`repo:publish-metadata` entirely.

## 4. The checker

The checker parses each workflow with `yaml.safe_load_all` and walks every document. It takes
the repository root as `argv[1]`. It must not rely on the current directory: `run.sh` computes
`REPO_ROOT` from `BASH_SOURCE` and works relative to it, and a directory-relative glob would
find zero files and report a false rc 2.

Four rules apply to the parsed tree.

| Rule | Condition | Closes |
|---|---|---|
| R1 | a mapping key equals `secrets` | row 8; `secrets:`, `secrets: inherit`, flow mappings, `workflow_call` pass-through |
| R2 | a mapping key equals `id-token` and its value is `write` | rows 1, 2, 3, 6, 7, 14 |
| R3 | a mapping key equals `permissions` and its scalar value is `write-all` | rows 12, 13 |
| R4 | an Actions expression references the `secrets` context | rows 4, 5, 9, 10, 11 |

R2 and R3 compare the parsed value after `str(value).strip().lower()`. Actions rejects a
case-varied value, so a lowercase comparison only ever adds a conservative red.

### 4.1 R4 must extract the expression span first

Revision 1 specified R4 as the single pattern
`\$\{\{[^}]*\bsecrets\s*(?:\.|\[)` with `re.IGNORECASE`. **That is withdrawn. It was
measured to miss two ordinary expressions that read real secrets:**

| Expression | Revision 1 R4 | Why it escapes |
|---|---|---|
| `${{ format('{0}', secrets.PYPI_API_TOKEN) }}` | **miss** | the `}` inside `{0}` stops `[^}]*` before it reaches `secrets` |
| `${{ toJSON(secrets) }}` | **miss** | `secrets` is followed by `)`, not `.` or `[` |

`toJSON(secrets)` is the canonical expression for dumping every secret at once, so missing it
is not an edge case. Revision 1 argued the whole redesign on the grounds that a pattern table
"makes the check look complete while it stays incomplete", and then reproduced that failure
inside the parser design.

R4 therefore extracts each `${{ … }}` span with `\$\{\{(.*?)\}\}` under `re.S`, then searches
the span for `\bsecrets\b` with `re.IGNORECASE` and **no requirement on the following
character**. Measured: this catches all four inputs above, including both that escaped.

### 4.2 R4 exempts `secrets.GITHUB_TOKEN` by name

`${{ secrets.GITHUB_TOKEN }}` is the workflow's own permission-scoped token, not a repository
secret. It is the most common expression in Actions, and most action READMEs show that
spelling rather than `${{ github.token }}`. Reding it would be a false positive on a standard,
safe construct, and a first false positive is how a gate gets allowlisted into irrelevance.
The exemption is by name, with a stated reason: the token's power is bounded by
`permissions:`, which `repo:actionlint` already governs.

R1 carries the matching correction. Revision 1's Risk 1 justified a red on any `secrets` key
with "such an input does pass a secret". That is **false** for `docker/build-push-action`'s
documented `with: secrets:` input when it is sourced from `github.token` or from a file.
`images.yml` is `pull_request`-triggered and builds images, so the construct is plausible
here. R1 keeps the conservative red, but the reason is corrected: the red means "a human
must look", not "this is a leak".

### 4.3 Comment handling disappears

PyYAML never returns a comment, so `strip_comments` and its whole defect class leave the
codebase. Row 1 closes because the escape bug no longer exists, not because the escape rule
was fixed. A `#` inside a block scalar is literal by construction, and a block scalar arrives
as an ordinary scalar value, so R4 covers `run:` bodies with no special case.

## 5. Discovery

The checker globs `.github/workflows/*.y*ml`. **Both extensions must be covered.** Actions
accepts `.yaml` equally, and `ci/actionlint/run.sh` already iterates both. Globbing `*.yml`
alone is a complete bypass that needs no allowlist and no malice: renaming four workflows to
`.yaml`, or adding one new `publish.yaml` from a template, leaves the remaining `.yml` file
as the only subject while every registry stays satisfied.

A workflow is a subject when its triggers include `pull_request` or `pull_request_target`.
`pull_request_target` is included because it is strictly more dangerous: it runs with the base
repository's secrets.

Two traps must be handled explicitly.

- **The YAML 1.1 boolean trap.** PyYAML parses the `on:` key as the boolean `True`, so
  `doc.get("on")` returns `None`. The checker reads both the `"on"` key and the `True` key.
  Measured on `release.yml`: its top-level keys are `['name', True, 'concurrency',
  'permissions', 'jobs']`, `'on' in d` is `False`, and `True in d` is `True`.
- **Three trigger shapes.** `on:` appears as a string, as a list, and as a mapping. All three
  normalise to a set.

### 5.1 The subject set is pinned by strict equality

`EXPECTED_PR_SUBJECTS` names the five subjects: `ci.yml`, `images.yml`, `prebuild.yml`,
`security-scan.yml`, `wheels.yml`. Discovery that disagrees is rc 1, and the failure message
says to re-baseline deliberately. A new pull-request-triggered workflow reds until someone
adds it. That is the same ergonomics the repo already accepts for a new publishable crate.

The non-vacuity count runs **before** the allowlist is applied. Revision 1 left this
ambiguous, and the two readings are different code: counting after the allowlist would let
"allowlist everything" pass. An allowlisted subject is still **reported on stdout**, so a
suppressed subject is visible rather than silent.

`PR_CREDENTIAL_ALLOWED` maps a filename to a stated reason. It is empty when this lands.

## 6. Exit codes and the dependency preflight

| Code | Meaning |
|---|---|
| 0 | every subject is credential-free |
| 1 | a subject declares a credential, or discovery disagrees with `EXPECTED_PR_SUBJECTS` |
| 2 | infrastructure |

Revision 1 claimed "PyYAML unavailable gives rc 2". **That was unproven and is false.**
Measured:

| Condition | rc |
|---|---|
| `uv` fails to resolve a package, online | **1** |
| `uv` fails to resolve a package, offline (`UV_OFFLINE=1`) | **1** |
| `uv` absent from `PATH` | **127** |
| child exits 2 | 2 (propagated correctly) |
| child exits 1 | 1 (propagated correctly) |

`uv` exits 1 on its own failures, which collides exactly with "1 = the repo is wrong". As
revision 1 specified it, a PyPI outage would have reported "a subject declares a credential".

The remedy has three parts. `run.sh` preflights `command -v uv` and maps absence to rc 2. It
then probes `uv run --project py python3 -c 'import yaml'` and maps any non-zero to rc 2. The
checker prints a **sentinel line** on stdout, and `run.sh` refuses to trust an rc of 1 unless
the sentinel is present. Without the sentinel, an rc 1 from the toolchain cannot be told apart
from an rc 1 from the assertion.

A document that is not a mapping is rc 2 with a named message. An empty `.yml` parses to
`None` and a top-level sequence parses to a `list`; `doc.get` on either raises
`AttributeError`, which Python reports as exit 1 — the same lie about which thing broke.

The interpreter is pinned with `--python 3.12` and `UV_PYTHON_DOWNLOADS=never`, so a host
without a suitable interpreter fails as a clean rc 2 rather than silently downloading one.

**The red output contract.** A red names the workflow file, the rule, and the YAML path to
the offending node. With six files and four rules, triage is otherwise a manual re-run.

## 7. Non-goals

The gate asserts **declaration**. It does not assert that no credential can reach a workflow
by any path:

- an expression built by concatenation, or laundered through `${{ env.X }}` — both need
  dataflow analysis;
- a credential a third-party action fetches by itself;
- `workflow_run` and `merge_group` triggers. Both run with repository secrets, and
  `workflow_run` is the classic pwn-request vector. Neither is used in this repo today.
  They are **out of scope by decision, not by oversight**, and the README says so. Adding
  them later is a one-line change to the trigger set plus two control rows.

## 8. Controls

Revision 1 specified the control table three incompatible ways. This is the single list.

**In-process, behind `workflow_credentials.py --self-test`.** Revision 1 would have run about
29 fixture rows as separate `uv` subprocesses. Every other checker in this repo puts its
fixture table in-process (`categories.py`, `ci/error-registry/check.py`,
`ci/http-extractor/check.py`, `ci/affected-graph/task_inputs.py`). The rule-level table goes
there: one row for each of the 14 bypasses, one for each of the 6 already-caught inputs, the
6 honest passes, plus `format('{0}', secrets.X)`, `toJSON(secrets)`, `secrets.GITHUB_TOKEN`
(must pass), and a `with: secrets:` build-arg case.

**In `run.sh --negative-control`.** Only the wiring rows, which need the real tree: discovery
over the real workflows; `release.yml` excluded; the strict-equality mismatch path; zero
subjects is rc 2; a stale allowlist entry is rc 2; the `uv` preflight rc 2 paths; the sentinel
check.

At least one rc-0 row stays in each table, so a checker that fails unconditionally cannot
satisfy either.

**The `release.yml` row.** That workflow fails the credential rules and passes the gate only
because discovery excludes it. A row asserting both halves proves the trigger filter does
real work rather than decorating. Its failure message must say "re-baseline: `release.yml` no
longer reads a secret", because an SMA-589 follow-up moving to OIDC would otherwise red this
gate with no obvious cause.

**Rows that exercise PyYAML rather than the gate.** Rows 2, 3, 6, 7 and 14 all parse to one
mapping, as section 2 proves. Their rows are regression pins on the parser, not coverage of
the gate's own logic, and the table says so rather than implying otherwise.

## 9. Files and bookkeeping

| File | Change |
|---|---|
| `ci/workflow-credentials/workflow_credentials.py` | new — discovery, four rules, `--self-test` |
| `ci/workflow-credentials/run.sh` | new — preflight, sentinel, `--negative-control` |
| `ci/workflow-credentials/README.md` | new — the check, the allowlist, the non-goals |
| `ci/publish-metadata/run.sh` | **remove** P-D6 and its fixture rows; correct the "pure-Python" header at `:79-82` |
| `ci/publish-metadata/README.md` | remove the P-D6 row |
| `py/pyproject.toml` | add `"pyyaml>=6,<7"` to `[dependency-groups] dev` |
| `py/uv.lock` | regenerate |
| `moon.yml` | new `repo:workflow-credentials` task; **remove** `.github/workflows/wheels.yml` from `publish-metadata` inputs |
| `.github/workflows/ci.yml` | add `:workflow-credentials` to the `T=(…)` array |
| `CLAUDE.md` | add `:workflow-credentials` to the marker-delimited command; record the gotchas |
| `.github/workflows/wheels.yml` | update the header comment at `:9-10` — it names two banned spellings and cites this gate |
| `ci/affected-graph/ci_targets.py` | four edits, below |

**`ci_targets.py` obligations.** All four are required, and missing any one reds
`repo:affected-smoke`:

1. `SELF_TASK_EXPECTED_GLOBS["publish-metadata"]` — **shrinks**. `.github/workflows/wheels.yml`
   was listed only because P-D6 read it. `.github/workflows/security-scan.yml` stays, because
   Check 4 asserts on it. Rewrite the comment at `:213-226`, which names `wheels.yml` by
   reason and goes stale otherwise.
2. `SELF_TASK_EXPECTED_GLOBS["workflow-credentials"]` — new. Ordering is **globs first
   (sorted), then literal files (sorted)**, verified at `ci_targets.py:1067-1082`; the
   injected `.moon/*` glob is filtered before comparison. Confirm with a real
   `moon query projects` before writing the tuple rather than after CI reports it.
3. `SELF_SCHEDULED_GATES["workflow-credentials"]` — new, pinning the `moon.yml` invocation
   lines including `set -euo pipefail`. Note the self-test invocation makes this **four**
   lines, not three, the same shape as `version-lockstep`.
4. `REQUIRED_REPO_TASKS` — add `workflow-credentials`. That tuple is the floor for gates
   carrying a negative control, and without an entry the control can be switched off while
   every other check stays green.

**The scan glob, and why it needs no fifth registry.** `repo:http-extractor-envelope` is the
precedent for keeping a checker's scan glob identical to its `moon.yml` input glob so that
"scheduling and scanning cannot drift apart". Its actual coupling is weaker than it reads:
`SELF_TASK_EXPECTED_GLOBS` pins the `moon.yml` side exactly, but `check.py`'s `SCAN_GLOB` is
held only by a `moon.yml` comment and by an infra guard that fires when the glob matches no
file at all. Narrowing it to ONE file would pass.

This gate needs no equivalent pin, because Decision B already closes that hole. Narrowing the
checker's glob shrinks the discovered subject set, and a shrunk set fails the strict-equality
comparison against `EXPECTED_PR_SUBJECTS` with rc 1. The subject pin subsumes the glob pin.
Keep the two glob strings textually identical and say so in `moon.yml`'s comment, but do not
add a fifth `ci_targets.py` registry for it — that would be ceremony over an assertion that is
already made.

**CLAUDE.md hazard.** Do not add a second copy of the ci-targets markers or of the `T=(…)`
command anywhere in that file. `repo:affected-smoke` counts occurrences, and a second copy —
even inside backticks in prose — makes the count 2 and reds the gate (SMA-541).

**`repo:input-liveness`.** `.github/workflows/*.y*ml` matches six tracked files, so it passes.
If Moon 2.3.2's matcher rejects `*.y*ml`, declare `*.yml` and `*.yaml` separately and add an
`ALLOW_DEAD_INPUT` entry for the `.yaml` one with a stated reason, since it matches zero
tracked files today.

## 10. Risks

- **A conservative red on an unrelated `secrets` key.** R1 matches any mapping key named
  `secrets`, including a third-party action's `with: secrets:` input. The red means "a human
  must look", not "this is a leak" (section 4.2). The allowlist carries a justified exemption.
- **The gate now depends on the `py/` workspace.** `uv run --project py` needs a synced
  environment, and `py`'s `moon.yml` runs bare `uv sync`, not `--locked`. This is the cost of
  Decision A, accepted in exchange for a cache that saves and a pin Dependabot can see.
- **SMA-579 topology.** `wheels.yml` carries a `pull_request` trigger, so it stays a subject
  and must never declare `secrets:` or `id-token: write`. The confirmed topology is that
  `wheels.yml` builds and uploads artifacts, and `release.yml` downloads them and publishes.
  `release.yml` has no `pull_request` trigger, so it is not a subject and may hold both the
  credential and `id-token: write` for Trusted Publishing.
- **A new gate is new bookkeeping.** Four `ci_targets.py` registries, the `T` array and the
  CLAUDE.md marker command must all agree. Two peer sessions are editing `ci_targets.py`
  concurrently and have been told.

## 11. What revision 1 got wrong

Recorded so the same mistakes are not repeated. Revision 1 carried a section claiming the
design was verified and "ALL GREEN". It was green only against a case list that did not
contain `format()` or `toJSON()`. A verification is worth exactly its inputs, and a green
result over a self-chosen corpus is not evidence of completeness. The rewritten section 8
fixes the corpus; it cannot fix the general lesson, which is that the corpus is the claim.
