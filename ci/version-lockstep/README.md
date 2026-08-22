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
