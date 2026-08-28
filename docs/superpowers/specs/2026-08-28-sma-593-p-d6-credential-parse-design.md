# SMA-593 — Parse the workflow YAML, and lift the credential guard into its own gate

**Status:** Revision 3. Revisions 1 and 2 were each challenged and each returned NEEDS
REWORK. This revision folds in every justified finding from both reviews.
**Date:** 2026-08-28
**Linear:** [SMA-593](https://linear.app/smaschek/issue/SMA-593/ci-repopublish-metadatas-p-d6-credential-guard-misses-five-ordinary)
**Branch:** `feature/sma-593-close-p-d6-credential-spelling-gaps`
**Targets:** `main` (currently `4f0d9b2`; the branch was rebased onto it after SMA-595 landed the moon 2.5.3 / proto 0.61.1 bump).
**References:** SMA-407 §7 review M2 (the rule); SMA-578 (which added P-D6); SMA-542
(guard-the-guard); SMA-529/SMA-530 (negative controls); SMA-541 (the `T` array and its
marker-delimited twin); SMA-553/SMA-572/SMA-576 (gate bookkeeping); SMA-579.

**Related gate, not redundant with this one.** SMA-579 adds a release guard whose V6 rule
asserts that a workflow reachable by `uses: ./.github/workflows/*.yml` may carry a
registry-reaching job only if its `on:` block is `workflow_call` and nothing else. That is a
**reachability** rule; this gate's is a **trigger** rule. They share no predicate. Both will
red on `wheels.yml` if it ever gains a publish step, for different and complementary reasons.
Neither should be deleted as redundant to the other.

---

## 1. The rule

`.github/workflows/wheels.yml` carries a `pull_request` trigger. Same-repo pull requests
receive repository secrets, so a registry credential there is readable by any code the pull
request introduces. SMA-407 §7 review M2 forbids it.

`assert_wheels_has_no_credentials` in `ci/publish-metadata/run.sh` is the current assertion.
It strips YAML comments with a hand-written scanner, then applies three regular expressions.

## 2. The measurement

Every claim comes from driving the real `strip_comments` and `PATTERNS` code
(`ci/publish-metadata/run.sh:1070-1074`). `rc 0` means the checker accepted the input.

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

Rows 12 and 13 matter most: `write-all` grants every scope, `id-token` included, and it is
what a person writes to make a step work.

**Row 14 is a real hazard, not a theoretical one.** GitHub added YAML anchor and alias
support on 2025-09-18, so a workflow using the alias form runs. The banned value never
stands next to its key, so no regular expression over raw text reaches it cleanly. This is
the single strongest argument for parsing.

**One result that is not a hazard.** `ID-TOKEN: WRITE` also returns rc 0. Actions reads
workflow schema keys case-sensitively, so that workflow grants nothing. The count of real
gaps is 14, not 15.

**The already-caught set.** Six inputs, verified against the old code:

| Input | Old rc | Note |
|---|---|---|
| single-line `run:` block scalar with `${{ secrets.X }}` | 1 | |
| `${{secrets.X}}` (no space) | 1 | |
| `"${{ secrets.X }}"` (quoted value) | 1 | |
| `permissions: {id-token: write}` | 1 | |
| `workflow_call` `secrets:` declaration | 1 | |
| YAML merge key (`<<: *p`) | 1 | **cannot occur** |

The merge-key row needs care. GitHub's anchor support deliberately **excludes merge keys**,
because they are not in the YAML 1.2 spec, so a workflow using `<<:` does not run. The old
code caught it only incidentally — the anchor block's text appears literally, so the pattern
matched the **definition**, not the merged result. Revision 2 called this "caught by
accident" and counted it as coverage. That overclaims. It is coverage of an input Actions
rejects. PyYAML resolves merge keys even though Actions will not, so the new gate red on it
is harmless over-coverage, and §8 labels its row that way rather than as a closed hole.

**What a parser sees.** Rows 2, 3, 6, 7, 14 and the merge-key form all parse to the identical
mapping `{'id-token': 'write'}`.

## 3. Decisions

**Decision A — parse the YAML, with PyYAML from a dedicated one-dependency uv project.**
`ci/workflow-credentials/pyproject.toml` declares `pyyaml>=6.0.3,<7` as its only dependency
and carries its own `uv.lock`. Invocation is
`uv run --project ci/workflow-credentials --python '>=3.12'`.

Two earlier routes are withdrawn, each for a measured reason.

*Revision 1 proposed `uv run --no-project --with 'pyyaml==6.0.3'`.* The CI uv cache key is
`uv-${{ runner.os }}-${{ hashFiles('py/uv.lock') }}` (`.github/workflows/ci.yml:167`). A
`--with` dependency never changes `py/uv.lock`, so the restore is an exact primary-key hit and
`actions/cache` skips its save. PyYAML would be re-fetched from PyPI on every CI run,
indefinitely, on a required check. The pin would also sit in no lockfile, invisible to
Dependabot and to `repo:osv`.

*Revision 2 proposed adding PyYAML to `py/pyproject.toml`'s dev group and using
`uv run --project py`.* That is worse, and it contradicts Decision C. `py/` is a
`[tool.uv.workspace]` virtual root; its member `py/packages/paigasus-kernel` declares
`dependencies = ["paigasus-py-bindings==0.1.0"]` with a `[tool.uv.sources]` path into
`rs/crates/bindings/paigasus-py-bindings`, whose `build-backend` is **maturin**. Syncing that
workspace compiles a PyO3 cdylib. `ci.yml` has no `uv sync` step and never caches `py/.venv`,
so the cost would land on every run — of a gate whose inputs are `.github/workflows/*.y*ml`,
which is precisely the PR shape Decision C exists to keep cheap. It would also make a YAML
parser depend on a working Rust toolchain.

The dedicated project has neither problem, and it keeps both benefits: the pin is in a
lockfile that Dependabot reads and `repo:osv` scans, and the gate's own lockfile is its cache
key.

> **Correction, folded in after the final branch review (F8c).** The clause "a lockfile that
> Dependabot reads and `repo:osv` scans" was **not true when this paragraph was written**.
> Neither tool watched `ci/workflow-credentials/uv.lock`: Dependabot had no entry for the
> directory, and the file was absent from `repo:osv`'s `LOCKFILES` list. Writing a lockfile
> does not by itself enlist a watcher. The claim is true **now**, and only because this branch
> added both — the `dependabot.yml` entry and the `LOCKFILES` entry — as separate, deliberate
> work items. Read the sentence as a requirement this decision imposed, not as a property the
> decision inherited. A future gate that copies this pattern must add both entries too. **Measured on this host:** the lock resolves to 2 packages; cold, with no virtualenv,
`uv run` takes **0.959 s** (venv creation plus a 3 ms install); warm it takes **0.073 s**. No
cargo and no maturin are invoked.

**Decision B — pin the discovered subject set by strict equality.**
`ci/publish-metadata/run.sh` already holds two runtime-discovered sets behind exact-equality
lists (`EXPECTED_PUBLISHABLE`, `EXPECTED_PYPI_PUBLISHABLE`), and Check P0's comment
(`:55-62`) states the reason: a stale list silently **shrinks** the gate rather than
reporting red. A bare non-empty assertion does not survive an unbounded allowlist.

**Decision C — lift the check into its own gate, `repo:workflow-credentials`.**
`repo:publish-metadata` runs `cargo publish --dry-run` per publish group plus a crates.io
category check. `ci.yml` is the most-edited workflow in the repo, so widening that gate's
inputs to every workflow would have made each such PR pay that cost on a required check.
P-D6 leaves `repo:publish-metadata` entirely.

## 4. The checker

The checker takes the repository root as `argv[1]`. It must not rely on the current
directory: `run.sh` computes `REPO_ROOT` from `BASH_SOURCE` (`:93`) and works relative to it,
so a directory-relative glob would find zero files and report a false infrastructure failure.

### 4.1 Parsing

Documents are read with `yaml.safe_load_all` under a loader that **rejects duplicate mapping
keys**. This is not optional. PyYAML's default is last-wins, which is a regression against
the code being replaced:

```yaml
permissions: {id-token: write}    # silently discarded
permissions: {contents: read}
```

Measured: `safe_load` returns `{'permissions': {'contents': 'read'}}`, so R2 and R3 never see
the dangerous block, while the old regex matched it. A duplicate-key-rejecting loader was
measured to reject the same input cleanly. A duplicate key is rc 1 with a named message.

A document that is not a mapping is rc 1 with a named message: an empty `.yml` parses to
`None` and a top-level sequence parses to a `list`, and `doc.get` on either raises
`AttributeError`, which Python reports as exit 1 — a lie about which thing broke. A
`yaml.YAMLError` on a malformed workflow is rc 1: the repo is wrong, and `repo:actionlint`
owns YAML validity independently.

**Added during implementation: a merge key plus an explicit override is rejected, and that
is accepted over-rejection.** The strict loader calls `flatten_mapping` — which resolves
`<<: *anchor` merge keys — before it walks the key set for duplicates. If the same mapping
also sets one of the merged keys explicitly, the loader sees two keys of that name post-merge
and raises a duplicate-key error, even though the YAML 1.2 spec treats an explicit key as a
legal override of a merged one. This is accepted, not fixed, because GitHub Actions does not
support merge keys at all: Actions added anchor and alias support on 2025-09-18, but merge
keys are not in the YAML 1.2 spec Actions follows and never shipped. A workflow using `<<:`
therefore never runs on GitHub Actions regardless, so this rejection can only ever fire on an
input Actions itself would already refuse.

### 4.2 The four rules

| Rule | Condition | Closes |
|---|---|---|
| R1 | a mapping key equals `secrets` | row 8; `secrets:`, `secrets: inherit`, flow mappings, `workflow_call` pass-through |
| R2 | a mapping key equals `id-token` and its value is `write` | rows 1, 2, 3, 6, 7, 14 |
| R3 | a mapping key equals `permissions` and its scalar value is `write-all` | rows 12, 13 |
| R4 | an Actions expression references the `secrets` context | rows 4, 5, 9, 10, 11 |

R2 and R3 compare the parsed value after `str(value).strip().lower()`. Actions rejects a
case-varied value, so the lowercase comparison only ever adds a conservative red.

The walk visits mapping keys, mapping values and sequence items. R4 applies to every scalar
**string**, wherever it sits, including a key.

### 4.3 R4 must extract the span, strip literals, then match tightly

Revision 1's pattern `\$\{\{[^}]*\bsecrets\s*(?:\.|\[)` was measured to miss two ordinary
expressions that read real secrets: `${{ format('{0}', secrets.X) }}`, where the `}` inside
`{0}` stops the character class, and `${{ toJSON(secrets) }}`, the canonical dump-every-secret
expression, where `secrets` is followed by `)`.

Revision 2 replaced it with span extraction plus a bare `\bsecrets\b`. **That is also
withdrawn.** Measured false positives on ordinary strings:

| Input | rev 2 bare | with literals stripped and a tight boundary |
|---|---|---|
| `${{ inputs.secrets-file }}` | **false positive** | pass |
| `${{ steps.x.outputs.secrets }}` | **false positive** | pass |
| `${{ hashFiles('secrets.txt') }}` | **false positive** | pass |

R4 therefore: extracts each `${{ … }}` span with `\$\{\{(.*?)\}\}` under `re.S`; removes
single- and double-quoted string literals from the span; then searches what remains for
`(?<![\w.-])secrets(?![\w-])` with `re.IGNORECASE`. Stripping literals is what defeats
`hashFiles('secrets.txt')`, and it does not weaken `secrets['X']`, because the context name
sits outside the literal. `re.IGNORECASE` is correct here and only here: Actions expression
context names are case-insensitive, while workflow schema keys are not.

**R4 also matches a bare `if:` expression (controller ruling 8, added during
implementation).** GitHub evaluates an `if:` value as an expression even without the
`${{ }}` wrapper, so `if: secrets.TOKEN != ''` reads the secrets context with no span for the
scan above to extract. R4 therefore makes a second pass over every `if:` key whose value is a
string: it removes any wrapped `${{ … }}` part first — that part was already reported by the
span loop, and leaving it would double-count — then strips literals from what remains and
applies the same tight boundary. Rows I, J, K and L pin it: a bare read, an uppercase bare
read, a wrapped read (reported once, not twice), and an `if:` with no secrets reference that
must pass.

**The span pattern is literal-aware (F2, added during the final review).** `\$\{\{(.*?)\}\}`
is non-greedy over raw text, so a `}}` inside a string literal ends the span early:
`${{ format('{0} }}', secrets.PYPI) }}` never reached `secrets`, and the read went unseen
(measured). The repeat now consumes a whole quoted literal atomically, or one character that
does not begin an unquoted `}}`, so it can stop only at a real span end. The obvious
alternative — stripping literals from the whole scalar **before** extracting spans — was
measured and rejected: it deletes a shell-quoted expression whole, so `run: echo
"${{ secrets.X }}"` stops matching, which is the commonest shape a real secret read takes and
one this repo already carries at `.github/workflows/wheels.yml:233` and `:262`. Rows M and N
pin both directions.

**No `secrets.GITHUB_TOKEN` exemption.** Revision 2 proposed one. It is dropped for two
measured reasons. First, `secrets.GITHUB_TOKEN` appears **nowhere** in this repo's six
workflows — the idiom is `${{ github.token }}` (`.github/workflows/ci.yml:58`) — so the
exemption would suppress zero occurrences and was justified by assertion, not measurement.
Second, it would be unsafe: a `pull_request_target` workflow that checks out
`github.event.pull_request.head.sha` and holds a write-capable token is the textbook
pwn-request, and §5 deliberately brings that trigger into scope as "strictly more dangerous".
An exemption by name would apply there uniformly. If R4 ever fires legitimately, the answer
is a scoped allowlist entry (§5.2), not a blanket rule.

**R1's red means "a human must look".** Revision 2 justified R1 with "such an input does pass
a secret". That is false for a `with: secrets:` input sourced from `github.token` or from a
file. R1 keeps the conservative red; the stated reason is corrected.

### 4.4 Comment handling disappears

PyYAML never returns a comment, so `strip_comments` and its whole defect class leave the
codebase. Row 1 closes because the escape bug no longer exists, not because the escape rule
was fixed. A `#` inside a block scalar is literal by construction, and a block scalar arrives
as an ordinary scalar value, so R4 covers `run:` bodies with no special case.

## 5. Discovery

### 5.1 The glob and the traps

The checker globs `.github/workflows/*.y*ml`. **Both extensions must be covered.** Actions
accepts `.yaml`, and `ci/actionlint/run.sh` already iterates both. Globbing `*.yml` alone is a
complete bypass needing no allowlist and no malice: renaming workflows to `.yaml`, or adding
one `publish.yaml` from a template, leaves the rest unguarded with every registry satisfied.

The same string is evaluated by three different matchers — Python `glob` in the checker,
Moon's wax for scheduling, and git pathspec in `task_inputs.py`'s liveness check. They agree
on this pattern; the implementation must confirm rather than assume, and `moon.yml`'s comment
must say the checker's glob and the input glob are deliberately identical.

A workflow is a subject when its triggers include `pull_request` or `pull_request_target`.
`pull_request_target` is included because it runs with the base repository's secrets.

Trigger parsing has two traps and one edge:

- **The YAML 1.1 boolean trap.** PyYAML parses the `on:` key as the boolean `True`, so
  `doc.get("on")` returns `None`. Read both `"on"` and `True`. Measured on `release.yml`: its
  top-level keys are `['name', True, 'concurrency', 'permissions', 'jobs']`, `'on' in d` is
  `False`, `True in d` is `True`. **Union the two, never prefer one (F3, final review.)** A
  document can hold both keys at once — `"on": push` is the string key, a bare
  `on: pull_request` is the boolean — and they are distinct dict keys, so the strict loader
  finds no duplicate to reject. `doc.get("on", doc.get(True))` returned `{'push'}` for such a
  document (measured on keys `['on', True, 'jobs']`), and the workflow silently left the
  subject set with its grants unchecked.
- **Three shapes.** `on:` appears as a string, a list, or a mapping; all normalise to a set.
- **A bare `on:`** parses to `None`. That is an empty trigger set, so the workflow is not a
  subject.

### 5.2 The subject set, the allowlist, and their order

`EXPECTED_PR_SUBJECTS` names the five subjects: `ci.yml`, `images.yml`, `prebuild.yml`,
`security-scan.yml`, `wheels.yml`.

Discovery is compared to `EXPECTED_PR_SUBJECTS` **before any allowlist is applied**. The
allowlist suppresses **rule verdicts only, never membership**. Revision 2 left this ambiguous
and added a separate non-vacuity count; that count is dropped, because strict equality
against five names already implies non-vacuity.

`PR_CREDENTIAL_ALLOWED` is keyed by **`(filename, rule)`**, not by filename. Revision 2's
file-level key was an all-or-nothing kill switch: the entry permitting a
`docker/build-push-action` build-arg under R1 would equally permit `id-token: write` and
`${{ secrets.PYPI_API_TOKEN }}` in the same file, silently and forever. Each entry states
what a human verified. The table is empty when this lands, and an allowlisted subject is
still reported on stdout.

A stale allowlist entry — one naming a file that does not exist — is **rc 1**. It is an
authorial mistake, and `ci_targets.py:28-36` states the repo's rule: an authorial mistake is
rc 1, never rc 2, because rc 2 triages as "re-run the job". Revision 2 had this as rc 2.

## 6. Exit codes

The checker and the wrapper use different codes on purpose. Revision 2 proposed a stdout
sentinel to tell an assertion failure apart from a toolchain failure; that is dropped as
underspecified and untamper-evident — it overloaded stdout, said nothing about a crash after
the sentinel was printed, and nothing pinned that `run.sh` still checked it.

**The checker** exits `0` for pass, **`3`** for "a subject declares a credential, or discovery
disagrees with `EXPECTED_PR_SUBJECTS`, or a document is malformed", and `2` for its own
infrastructure failures. Its `main()` is wrapped so any unexpected exception becomes rc 2 with
a named message.

**`run.sh`** maps `0 → 0`, `3 → 1`, and **everything else → 2**. `uv`'s own rc 1 therefore
cannot be mistaken for an assertion, and there is nothing to pin.

This matters because it was measured. `uv` exits **1** on a failed resolution, online and with
`UV_OFFLINE=1`; `uv` absent from `PATH` yields **127**; child codes propagate unchanged. Under
revision 1's contract a PyPI outage would have reported "a subject declares a credential".

`run.sh` still preflights `command -v uv` and maps absence to rc 2 with the one-line fix, so
the common local failure gives a useful message rather than a bare 127.

**Zero subjects.** Revision 2 contradicted itself, saying rc 1 in §6 and rc 2 in §8. The two
cases are genuinely different and both are kept: the glob matching **zero files** is rc 2,
because the scan root moved and the gate is scanning nothing; **zero of N files** carrying a
pull-request trigger is rc 1, because that is a disagreement with `EXPECTED_PR_SUBJECTS`.

**Amended (controller ruling 10): the "glob matches zero files" case itself splits in two.**
Implementation found that "zero files matched" is not one cause — it hides two, and they
triage differently. An **absent** `.github/workflows/` directory means the checker was
handed the wrong repository root: a broken tool, rc 2. A **present** `.github/workflows/`
directory holding no `.y*ml` file means someone removed or renamed the workflows: an
authorial act. `ci_targets.py:28-36` states the repo's rule for exactly that case — "someone
edited a file into a shape this gate cannot read … is a red with a fix, not a broken tool" —
so this second case is rc 1, not rc 2. Collapsing both into rc 2 would tell a contributor to
re-run a job that can never go green. This split is orthogonal to the "zero of N files
carrying a pull-request trigger" case above, which stays rc 1 via the `EXPECTED_PR_SUBJECTS`
equality check in `check()` and needs no change.

**Corrected (F9, final review): the discriminator is `.github/`, not `.github/workflows/`.**
As written above, the split did not survive git. Git tracks no empty directory, so "the
workflows directory is present and holds no `.y*ml`" is essentially unreachable in a CI
checkout. The reachable authorial act — a pull request deleting every workflow — removes the
directory along with its files, which landed on the **absent** branch and therefore on rc 2:
"re-run the job", for a job that can never go green. That is precisely the misclassification
the split exists to prevent. The test is now `.github/` itself. Absent means the wrong root,
rc 2. Present, with no `.y*ml` matched, means the workflows went missing, rc 1 — and it covers
both shapes, the deleted directory and the empty one. `.github/` survives such a pull request,
because `CODEOWNERS`, the issue templates and `dependabot.yml` all live there. Both shapes
carry a filesystem self-test row (rows 2a and 2b).

**The red output contract.** A red names the workflow file, the rule, and the YAML path to the
offending node. With six files and four rules, triage is otherwise a manual re-run.

The interpreter is selected with `--python '>=3.12'`, matching every package's
`requires-python`. A bare `--python 3.12` would fail on a host carrying only 3.13.

## 7. Non-goals

The gate asserts **declaration**, not that no credential can reach a workflow by any path:

- an expression built by concatenation, or laundered through `${{ env.X }}`;
- a credential a third-party action fetches by itself;
- `workflow_run` and `merge_group` triggers. Both run with repository secrets and
  `workflow_run` is the classic pwn-request vector. Neither is used here today. They are out
  of scope **by decision, not by oversight**; adding them is a one-line change to the trigger
  set plus two control rows.
- **`${{ github.token }}`, and an individual write-scope permission grant paired with it
  (added during implementation).** A workflow that reads `${{ github.token }}` under
  `permissions: contents: write` obtains a real, usable credential, and this gate does not
  catch that case. Two measured reasons. First, `.github/workflows/ci.yml:58` already reads
  `GH_TOKEN: ${{ github.token }}` deliberately, so a rule against `github.token` would red
  the gate on day one against correct usage. Second, `github.token` is ephemeral and bounded
  by the workflow's own `permissions:` block. Broadening R3 from `write-all` to any write scope
  would turn a registry-credential-declaration gate into a least-privilege audit — a different
  job. This is a named boundary, stated here and in the README, not a silent gap.
  **Correction (F4).** An earlier draft of this bullet added "and `repo:actionlint` and zizmor
  already govern `permissions:` for least privilege". That was false and is withdrawn. zizmor
  runs nowhere in this repository — nothing pins it, installs it or calls it; the name occurs
  only in prose and in one comment in `.github/workflows/release.yml`. `ci/actionlint/run.sh`
  audits trigger filters and gate wiring, and holds no check on `permissions:` at all. **No
  control in this repository catches an individual write-scope grant on a
  pull-request-triggered workflow, and this gate deliberately does not either.** The boundary
  rests on the scope decision alone. Adding such a rule is out of scope for SMA-593; only the
  false justification is removed.

The README states these, and states R4's residual false-positive surface, so the gate does
not overclaim.

## 8. Controls

**In-process, behind `--self-test`.** The rule-level table lives in
`workflow_credentials.py`, as every other checker in this repo does
(`categories.py`, `ci/error-registry/check.py`, `ci/http-extractor/check.py`,
`ci/affected-graph/task_inputs.py`). Rows: each of the 14 bypasses; each of the 6
already-caught inputs; the 6 honest passes; `format('{0}', secrets.X)`; `toJSON(secrets)`;
the three R4 false-positive strings from §4.3, which must **pass**; a duplicate-key document;
a non-mapping document; and a bare `on:`.

**Count, as shipped: 54 rows, not the ~34 this list enumerates.** The list above describes the
plan; implementation and the two reviews added rows to it, and the total is what
`--self-test` prints. The 54 split across four tables: **37** `RULE_CASES` (the enumeration
above, plus the four bare/wrapped `if:` rows from ruling 8 and the two literal-aware span rows
from F2), **8** `TRIGGER_CASES` (the six trigger shapes, the no-`on:` document, and the
dual-key document from F3), **3** `PARSE_CASES`, and **6** filesystem rows that need a real
directory. Each of the first three tables carries an **arity floor** checked before any row
runs, and the filesystem count is asserted against `FILESYSTEM_CASES`, so an emptied table
raises rather than printing a vacuous pass (F6). The floors are floors: adding a row needs no
edit, removing one is deliberate.

**It is invoked through `run.sh --self-test`, not directly.** Running
`python3 workflow_credentials.py --self-test` under system python3 would fail on `import
yaml`; every mode must go through the wrapper so it gets the `uv` invocation and the
preflight.

**In `run.sh --negative-control`.** Only rows needing the real tree: discovery over the real
workflows; `release.yml` excluded; the strict-equality mismatch path; the no-`.github/`
rc 2 (the discriminator is `.github/`, not `.github/workflows/` — see F9 below); a stale allowlist entry rc 1; the `uv` preflight rc 2 path; and the `3 → 1` mapping.

At least one rc-0 row stays in each table, so a checker that fails unconditionally cannot
satisfy either.

**The `release.yml` row.** That workflow fails the credential rules and passes only because
discovery excludes it, which proves the trigger filter does real work. Its failure message
must say "re-baseline: `release.yml` no longer reads a secret", so an SMA-589 follow-up moving
to OIDC does not red this gate with no obvious cause.

**Rows that pin the parser rather than the gate.** Rows 2, 3, 6, 7 and 14 all parse to one
mapping, and rows 12 and 13 are the same structural test under R3. The merge-key row
documents an input Actions rejects. All are labelled as regression pins, not as coverage.

## 9. Files and bookkeeping

| File | Change |
|---|---|
| `ci/workflow-credentials/workflow_credentials.py` | new — discovery, four rules, `--self-test` table |
| `ci/workflow-credentials/run.sh` | new — preflight, mode dispatch, rc mapping, `--negative-control` |
| `ci/workflow-credentials/pyproject.toml` | new — one dependency, `pyyaml>=6.0.3,<7` |
| `ci/workflow-credentials/uv.lock` | new — generated |
| `ci/workflow-credentials/README.md` | new — the check, the allowlist, the non-goals, R4's FP surface |
| `ci/publish-metadata/run.sh` | delete `assert_wheels_has_no_credentials`, `strip_comments`, `PATTERNS`, the six P-D6 fixture rows, and the `Check P-D6` summary entry at `:73-77` |
| `ci/publish-metadata/README.md` | the check table has **no** P-D6 row to remove; it is already stale for the whole Python arm (no P0/P1/P2 rows). Add the missing rows or record the staleness — do not silently leave it |
| `moon.yml` | new `repo:workflow-credentials` task; remove `.github/workflows/wheels.yml` from `publish-metadata` inputs **and** its explaining comment; correct the `description:` at `:497`, which still ends "while wheels.yml stays credential-free" |
| `.github/workflows/ci.yml` | add `:workflow-credentials` to `T=(…)` |
| `CLAUDE.md` | add `:workflow-credentials` to the marker command **in the same position**; correct `:351`, which says `repo:publish-metadata` asserts the wheels ban; record the gotchas |
| `.github/workflows/wheels.yml` | the sentence that becomes false is at `:12` ("`repo:publish-metadata` asserts this"), not the ban at `:9-10` |
| `ci/affected-graph/ci_targets.py` | four edits, below |
| `moon.yml` (`repo:osv`) | add `ci/workflow-credentials/uv.lock` to its inputs so the new lockfile is scanned |

All three new source files carry the SPDX header (`#` form).

**`ci_targets.py` obligations.** Missing any one reds `repo:affected-smoke`:

1. `SELF_TASK_EXPECTED_GLOBS["publish-metadata"]` — **shrinks**; `.github/workflows/wheels.yml`
   was listed only because P-D6 read it. `security-scan.yml` stays, because Check 4 asserts on
   it. Rewrite the explaining comment at `:218-226`.
2. `SELF_TASK_EXPECTED_GLOBS["workflow-credentials"]` — new. Ordering is globs first (sorted),
   then literal files (sorted), verified at `:1068-1083`; the injected `.moon/*` glob is
   filtered before comparison. Confirm with a real `moon query projects` before writing it.
3. `SELF_SCHEDULED_GATES["workflow-credentials"]` — new, pinning the `moon.yml` lines
   verbatim. There are **four**, all routed through the wrapper:
   `set -euo pipefail`; `bash ci/workflow-credentials/run.sh --self-test`;
   `bash ci/workflow-credentials/run.sh --negative-control`;
   `bash ci/workflow-credentials/run.sh`.
4. `REQUIRED_REPO_TASKS` — add `workflow-credentials`. That tuple is the floor for gates
   carrying a negative control; without an entry the control can be switched off while every
   other check stays green.

**A fifth obligation: pin the negative-control body, and make the pin reachable.**
`SELF_SCHEDULED_GATES` pins the invocation, and the repo has measured twice that this is not
enough. `RELEASE_PARITY_SH_CALL_SITES` (`ci_targets.py:600-644`) exists because pinning the
invocation alone left two bypasses: neutering the flag parse so `--negative-control` falls
through to the real suite, and gutting the assertion body so the control prints "reported red
as expected" while calling nothing. A bare `run.sh --negative-control` inherits both. Pin the
discrete lines the same way — and **`repo:affected-smoke`'s own `inputs` must gain
`ci/workflow-credentials/**/*`**, or the pin stays green on exactly the PR that breaks it.
That is why the `ci/actionlint/**/*` and `ci/release-parity/**/*` entries carry do-not-remove
comments. `T_AFFECTED_SMOKE_REQUIRED_INPUTS` (`ci/actionlint/run.sh:2097-2117`) is a
containment floor, so adding an input reds nothing.

**Why no scan-glob registry.** Decision B subsumes it: narrowing the checker's glob shrinks
the discovered set, which fails the strict-equality comparison. (Revision 2 argued this from a
claim that `http-extractor`'s `SCAN_GLOB` has one guard. That was wrong — it has four:
zero-files, no-signature-parsed, no-`EnvelopeJson`, and a self-test scope assertion. The
conclusion stands; the supporting claim does not.)

**`check_docs` compares `T` and the CLAUDE.md command as ordered sequences**
(`ci_targets.py:868-881`), so the new target must occupy the same position in both.

**CLAUDE.md hazard.** Do not add a second copy of the ci-targets markers or of the `T=(…)`
command anywhere in that file; `repo:affected-smoke` counts occurrences (SMA-541).

**`repo:input-liveness`.** `.github/workflows/*.y*ml` matches six tracked files. If Moon's
matcher rejects the pattern, declare `*.yml` and `*.yaml` separately and add an
`ALLOW_DEAD_INPUT` entry for the `.yaml` one, which matches zero tracked files today.

**Recorded decision, not an omission.** `workflow-credentials` is *not* added to
`task_inputs.py`'s `REQUIRED_TASKS` floor (`:90`); that tuple is explicitly non-exhaustive.

**No `py/uv.lock` regeneration.** Decision A's dedicated project means `py/uv.lock` is
untouched, so `repo:version-lockstep` and `repo:release-parity-py` are not scheduled by this
change and no lock-drift step is needed.

## 10. Risks

- **A conservative red on an unrelated `secrets` key.** R1 matches any mapping key named
  `secrets`. The red means "a human must look". The `(file, rule)` allowlist carries a scoped
  exemption.
- **R4 has a residual false-positive surface.** Stripping literals and tightening the boundary
  closes the three measured cases, but an expression that names a non-secret identifier
  `secrets` outside a literal would still red. The README documents it.
- **The gate depends on `uv` and on PyPI reachability for a cold lock.** Cold cost measured at
  0.959 s, warm at 0.073 s. A cold-cache PyPI outage reds as rc 2, which triages correctly.
- **A new gate is new bookkeeping.** Five `ci_targets.py` obligations, the `T` array, the
  CLAUDE.md marker command and `repo:affected-smoke`'s inputs must all agree. Two peer
  sessions are editing `ci_targets.py` concurrently and have been told.

## 11. What earlier revisions got wrong

Recorded in full, because a summary that omits most of the record is what revision 2 did.

**Revision 1** carried five defects, four found by review and one by its own author: R4 missed
`format()` and `toJSON()`; the rc-2 contract for an unavailable PyYAML was asserted and is
false, since `uv` exits 1; discovery globbed `.yml` only; the uv cache key never changed, so
`actions/cache` skipped its save; the control table was specified three inconsistent ways; and
Risk 1's justification for R1 was factually wrong.

**Revision 2** fixed those and introduced four more: `uv run --project py` drags a maturin
compile into a gate created to be cheap; the stdout sentinel was underspecified and
untamper-evident; `--self-test` was placed where it could not run; §6 and §8 disagreed on the
zero-subject case; the `GITHUB_TOKEN` exemption was unmeasured and unsafe under
`pull_request_target`; R4's bare `\bsecrets\b` false-positived on three ordinary strings; the
allowlist was keyed too coarsely; and the removal list mis-cited two line ranges and named a
README row that does not exist.

**Revision 1's stated lesson was that a green verification is worth exactly its corpus.**
Revision 2 then repeated the underlying error in a new place: it wrote a removal list from
memory rather than from the files. The general form is that a claim about the tree must be
read out of the tree at the moment it is written, and every citation in this revision was
checked against the file it names.
