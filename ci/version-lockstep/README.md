<!-- SPDX-License-Identifier: Apache-2.0 -->

# `repo:version-lockstep`

Asserts every version-carrying site in a lockstep family agrees with that family's
source-of-truth Cargo crate (ADR-0011 S1; SMA-576).

## Why 18 sites and not 6

release-plz owns the Cargo `[package] version` of every group member and the
`[workspace.dependencies]` version *requirements* — both measured against the pinned
0.3.158, not assumed. But four classes of site are owned by nobody:

- `pyproject.toml` / `package.json` versions (maturin and napi read these, not Cargo)
- the `paigasus-py-bindings==X.Y.Z` pin in the Python wrapper — `[tool.uv.sources]` is
  development-only metadata that uv strips from the built wheel
- `rs/Cargo.lock` and `py/uv.lock`
- `rs/crates/bindings/paigasus-node-bindings/index.js`, whose 26 committed
  `bindingPackageVersion !== '<v>'` guards napi regenerates from `package.json`

`py/packages/paigasus-kernel/moon.yml` runs bare `uv sync` (not `--locked`), and
`ci.yml`'s codegen-drift gate covers only the three `**/generated` proto dirs — so the
last two drift **silently** today.

## Why `--check` verifies sites release-plz owns

A gate that trusted release-plz to have done its half would not notice a `version_group`
that silently stopped applying. Checking them costs nothing and closes that.

## Groups are checked independently

The gate asserts *intra-group* agreement. `kernel` at `0.1.0` and `proto` at `0.0.0` is a
passing state — the proto family activates in SMA-577.

## Modes

| Mode | Behaviour |
|---|---|
| `--check` (default) | Compare all 18 sites. Exit 1 on any drift. |
| `--write` | Rewrite the six sites release-plz cannot reach and regenerate the three derived ones. |
| `--negative-control` | Prove the checker can still report red. |
| `--self-test` | Fixture tables for the verdict function. |

Exit codes: `0` pass, `1` the repo is wrong, `2` infrastructure failed.

## How it runs in CI

`moon.yml`'s `repo:version-lockstep` task, scheduled by `moon ci` because `:version-lockstep`
is in `.github/workflows/ci.yml`'s `T=(…)` array and in CLAUDE.md's marker-delimited copy of it
(`repo:affected-smoke` fails if the two ever disagree). The task script runs the negative control
FIRST and then the real check, under an explicit `set -euo pipefail` — Moon does not enable
errexit for `script:` blocks, so a script's status is just its LAST command's and without that
line a control that had stopped reporting red would be masked by the passing real run.

Three things about that task are pinned from `ci/affected-graph/ci_targets.py`, which runs inside
`repo:affected-smoke` — a separately scheduled gate, so this one is not the sole judge of its own
wiring:

- `SELF_SCHEDULED_GATES` pins all three script lines, whole-line matched. Whole lines matter
  because `bash ci/version-lockstep/run.sh` is a strict PREFIX of the `--negative-control` line: a
  substring test would read the script as wired after the real run had been deleted.
- `SELF_TASK_EXPECTED_GLOBS` pins the task's sixteen `inputs:` entries. Drop one and the gate
  stops re-keying on that version site — it then reports PASS from Moon's cache over a file it
  never read. All sixteen are literal paths, so moon resolves them into `inputFiles` rather than
  `inputGlobs`; that constant compares the whole authored set across both buckets (SMA-576).
- `repo:input-liveness` asserts each of those sixteen still names a TRACKED file, so moving one
  reds CI instead of silently switching part of this gate off.

## The negative control

`--negative-control` stages a scratch copy of every version-carrying file, drifts
`@paigasus/node-bindings` to `99.99.99`, and asserts `run_check` exits 1. It drives the
**real** `run_check` rather than a reimplementation — a second, differently-wrong checker
would prove nothing.

Measured: with `site_verdict` neutered to always return `OK`, the real run still prints
`== all 18 version-lockstep sites agree ==` and exits 0. The control reds.

## `--write` implementation notes

- The `packagejson` writer edits the `"version"` field **in place with a regex**, the same
  approach as `pyproject`, rather than round-tripping through `json.loads`/`json.dumps`.
  A full re-serialization reformats every array in the file onto multiple lines (Python's
  `json.dumps` has no compact-array mode) — measured against this repo's committed
  `package.json` files, that reports `wrote N site(s)` and rewrites unrelated Prettier-style
  formatting even when the version was already correct, which breaks the "already in
  lockstep" no-op case and pollutes the release-PR diff.
- `@napi-rs/cli` is a devDependency of `@paigasus/kernel` (`ts/packages/paigasus-kernel`),
  not of the ts workspace root — a `file:`-linked consumer's own devDeps aren't installed at
  the root `node_modules`. A bare `pnpm exec napi …` from `ts/` finds no `napi` binary and
  pnpm treats it as a recursive exec across every workspace package, failing on the first
  one that lacks it. `run_write` scopes the call with `pnpm --filter @paigasus/kernel exec`
  instead.
