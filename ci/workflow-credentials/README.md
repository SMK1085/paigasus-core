<!-- SPDX-License-Identifier: Apache-2.0 -->

# `repo:workflow-credentials`

Asserts that no pull-request-triggered GitHub Actions workflow can obtain a repository
credential.

A same-repo pull request runs with repository secrets. Any code the pull request
introduces can read them. SMA-407 §7 review M2 forbids a credential in a
pull-request-triggered workflow for this reason. Publishing must happen in a workflow
with no `pull_request` or `pull_request_target` trigger instead.

`run.sh` runs the checker, `workflow_credentials.py`, in three modes: a bare check of the
real tree, `--self-test` (an in-process table of 50 rows), and `--negative-control`
(assertions against the real tree, including the `release.yml` exclusion and the exit-code
mapping below).

## The four rules

The checker parses every workflow file into YAML and walks it. Four rules fire on a
parsed document.

| Rule | Condition | Example that turns it red |
|---|---|---|
| R1 | A mapping key equals `secrets` | `secrets: inherit` |
| R2 | A mapping key equals `id-token`, and its value is `write` | `permissions: {id-token: write}` |
| R3 | A mapping key equals `permissions`, and its value is `write-all` | `permissions: write-all` |
| R4 | An Actions expression reads the `secrets` context | `${{ secrets.PYPI_API_TOKEN }}` |

Keys are compared case-sensitively for R1–R3, because Actions reads schema keys that
way. Values are lowered before comparison, which only ever adds a conservative red.

R4 also catches a bare `if:` expression, such as `if: secrets.TOKEN != ''`. Actions
evaluates an `if:` value as an expression even without the `${{ }}` wrapper, so a
credential read can hide there with no span for a plain `${{ }}` scan to find.

## Parsing: a strict loader, and one accepted over-rejection

The checker loads YAML with a loader that rejects duplicate mapping keys. PyYAML's
default keeps only the second of two duplicate keys and drops the first silently. Two
`permissions:` blocks at the same level could hide a credential grant from a plain
`yaml.safe_load` read. The strict loader raises an error instead, and the gate exits 1.

**A merge key plus an explicit override is rejected, and that is accepted.** The strict
loader expands a `<<: *anchor` merge key before it checks for duplicate keys. If the same
mapping also sets one of the merged keys by name, the loader now sees two keys with that
name and raises a duplicate-key error — even for a legal override, one the YAML 1.2 spec
allows. The gate keeps this over-rejection on purpose. GitHub Actions does not support
merge keys at all: Actions added anchor and alias support on 2025-09-18, but merge keys
are not part of the YAML 1.2 spec Actions follows. A workflow using `<<:` never runs on
GitHub, so this rejection can only ever fire on an input Actions itself would already
refuse.

## Discovery

The checker globs `.github/workflows/*.y*ml`. The pattern covers both `.yml` and
`.yaml`, because Actions accepts both extensions.

A workflow is a subject when its `on:` block names `pull_request` or
`pull_request_target`. `pull_request_target` runs with the base repository's secrets
even on a fork pull request, so it counts too.

**The `on:` → `True` trap.** PyYAML parses a bare `on:` key as the YAML 1.1 boolean
`True`, not the string `"on"`. `doc.get("on")` then returns nothing, and a naive reader
would see no trigger at all. The checker reads both `doc.get("on")` and `doc.get(True)`,
so it still finds the trigger block.

### `EXPECTED_PR_SUBJECTS`, and how to re-baseline it

Discovery must match `EXPECTED_PR_SUBJECTS` by strict equality: `ci.yml`, `images.yml`,
`prebuild.yml`, `security-scan.yml`, `wheels.yml`. A mismatch in either direction exits
1. This is deliberate — a stale list would silently shrink the gate instead of turning
red, the same reasoning `ci/publish-metadata/run.sh`'s `EXPECTED_PUBLISHABLE` and
`EXPECTED_PYPI_PUBLISHABLE` already use.

To add a workflow, add its filename to `EXPECTED_PR_SUBJECTS` in
`workflow_credentials.py`, in sorted order, once you have confirmed the new workflow
should carry a pull-request trigger at all.

## The allowlist — `PR_CREDENTIAL_ALLOWED`

`PR_CREDENTIAL_ALLOWED` is the gate's one escape hatch. It maps a `(filename, rule)`
pair to a text reason. The table is empty today.

The key is the pair, never the filename alone. A filename-only key would let one
approved finding also hide every other rule's findings in the same file, silently and
forever. Each entry must record what a human checked, and why the finding is safe —
not just that it is allowed.

The allowlist suppresses a rule verdict only. It never suppresses subject membership:
a workflow still counts toward `EXPECTED_PR_SUBJECTS` even when every one of its
findings is allowed, and an allowed finding still prints on stdout. An entry naming a
workflow that is not a current subject exits 1, because that is a stale entry.

## Exit codes

`run.sh` uses the repo's usual three codes: `0` pass, `1` the repo is wrong, `2`
infrastructure failed.

The checker itself uses different codes: `0` pass, `2` its own infrastructure failure,
and `3` for an assertion failure. `run.sh` maps `0 → 0`, `3 → 1`, and every other code
(including a bare `1`) to `2`.

**Why the checker's assertion code is 3, not 1.** `uv` exits 1 on its own failures —
measured on a failed dependency resolution, both online and under `UV_OFFLINE=1`. If
the checker also used 1 for "a workflow declares a credential," a PyPI outage during
`uv run` would be indistinguishable from a real finding. The checker exits 3 instead,
and only `run.sh` translates 3 into the repo's usual 1.

**The zero-match split.** Discovery can match no `.y*ml` file at all, and that splits
into two different causes with two different codes:

- `.github/workflows/` does not exist: the checker exits 2. The checker was handed the
  wrong repository root — a broken tool, not a repo mistake.
- `.github/workflows/` exists but holds no `.y*ml` file: the checker exits 1. Someone
  removed or renamed every workflow — an authorial mistake, not a broken tool.
  `ci/affected-graph/ci_targets.py` (lines 28–36) states the same rule for its own
  gate: a file edited into a shape this gate cannot read is a red with a fix, never a
  broken tool.

## The dependency, and where the PyYAML pin lives

The gate lives in its own `uv` project: `ci/workflow-credentials/pyproject.toml`
declares one dependency, `pyyaml>=6.0.3,<7`, pinned in
`ci/workflow-credentials/uv.lock`. `run.sh` invokes it as
`uv run --project ci/workflow-credentials --python '>=3.12' python3 workflow_credentials.py`.

This project is separate from `py/`, the main Python workspace, on purpose. `py/`'s
member `paigasus-kernel` builds a PyO3 extension through maturin, and reusing `py/`
here would pull a Rust compile into every workflow-file change — the opposite of the
cheap, self-contained gate this needs to be.

## Non-goals

The gate asserts that no subject **declares** a credential. It does not prove that no
credential can reach a pull-request-triggered workflow by any path. Four things sit
outside its scope, by decision, not by oversight:

- **An expression built by string concatenation, or one laundered through
  `${{ env.X }}`.** The gate matches a literal `secrets` reference. It does not trace
  a value through an intermediate variable.
- **A credential a third-party action fetches on its own**, outside any expression
  this gate can read.
- **The `workflow_run` and `merge_group` triggers.** Both run with repository secrets,
  and `workflow_run` is a known attack path. Neither is used in this repository today.
  Adding either is a one-line change to the trigger set plus two new control rows, not
  a redesign.
- **`${{ github.token }}`, and a write permission paired with it.** A workflow that
  reads `${{ github.token }}` under `permissions: contents: write` obtains a real,
  usable credential, and this gate does not catch that case. Two reasons support this
  limit. First, `.github/workflows/ci.yml:58` already reads
  `GH_TOKEN: ${{ github.token }}` deliberately, and that use is correct — a rule
  against `github.token` would turn the gate red on day one against valid code.
  Second, `github.token` is ephemeral and is bounded by the workflow's own
  `permissions:` block, and `repo:actionlint` and zizmor already check `permissions:`
  for least privilege. Widening R3 from `write-all` to any write scope would turn a
  credential-declaration gate into a least-privilege audit — a different job.

**R4's residual false-positive surface.** R4 strips string literals out of each
`${{ … }}` span, then matches the whole word `secrets`. Three measured cases pass
cleanly because of this: `${{ inputs.secrets-file }}`, `${{ steps.x.outputs.secrets }}`,
and `${{ hashFiles('secrets.txt') }}`. An identifier literally named `secrets`,
written outside a string literal and not matching one of these three shapes, still
turns the gate red. No such identifier exists in this repository's workflows today.

## Related gate: SMA-579's V6 rule

SMA-579 adds a release gate whose V6 rule checks a different thing: whether a workflow
reachable by `uses: ./.github/workflows/*.yml` can reach a registry-publishing job
without an `on: workflow_call` trigger and nothing else. That is a **reachability**
rule. This gate's rule is a **trigger** rule. The two share no condition, so V6 is
complementary, not redundant. Both would turn red if `wheels.yml` grew a publish step,
for different and independent reasons. Neither should be removed as a duplicate of the
other.
