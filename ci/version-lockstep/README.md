<!-- SPDX-License-Identifier: Apache-2.0 -->

# `repo:version-lockstep`

Asserts every version-carrying site in a lockstep family agrees with that family's
source-of-truth Cargo crate (ADR-0011 S1; SMA-576).

## Why 20 sites and not 9

release-plz owns the Cargo `[package] version` of each group's publishable crate and the
`[workspace.dependencies]` version *requirements* — both measured against the pinned
0.3.158, not assumed. It does NOT write a Cargo `publish = false` crate's version, group or
not (SMA-685). So five classes of site are owned by nobody:

- the three `publish = false` binding crates' Cargo `[package] version`
  (`rs/crates/bindings/*/Cargo.toml`)
- `pyproject.toml` / `package.json` versions (maturin and napi read these, not Cargo)
- the `paigasus-py-bindings==X.Y.Z` pin in the Python wrapper — `[tool.uv.sources]` is
  development-only metadata that uv strips from the built wheel
- `rs/Cargo.lock` and `py/uv.lock`
- `rs/crates/bindings/paigasus-node-bindings/index.js`, whose 27 committed guards (26 native,
  1 WASI) each carry the version twice: `bindingPackageVersion !== '<v>'` and
  `expected <v> but got`

`py/packages/paigasus-kernel/moon.yml` runs bare `uv sync` (not `--locked`), and
`ci.yml`'s codegen-drift gate covers only the three `**/generated` proto dirs — so the
`uv.lock` class still drifts **silently** today. The napi glue does not since SMA-667:
`paigasus-kernel-ts:test` (`tests/committed-napi-glue.test.ts`) fails when the committed
`index.js`, version guards included, is not the generator's output.

## Why `--check` verifies sites release-plz owns

A gate that trusted release-plz to have done its half would not notice a `version_group`
that silently stopped applying. Checking them costs nothing and closes that. `--write`
writes only the `publish = false` non-head `cargo-package` sites (SMA-685), so it cannot
hide a proto `version_group` fault: `paigasus-proto-derive` is publishable and stays
release-plz's.

## Groups are checked independently

The gate asserts *intra-group* agreement. `kernel` at `0.1.0` and `proto` at `0.0.0` is a
passing state — the proto family activates in SMA-577.

## Modes

| Mode | Behaviour |
|---|---|
| `--check` (default) | Compare all 20 sites, and assert static uv metadata (SMA-684). Exit 1 on any drift or violation. |
| `--write` | Rewrite the ten sites release-plz cannot reach (the six non-Cargo sites, the three `publish = false` binding manifests, and the version literals of the napi glue `index.js`) and regenerate the two lock files (four `SITES` rows: 16, 17, 19 and 20). It compiles nothing, and it exits 2 when a path outside the SITES paths changed (SMA-684). |
| `--negative-control` | Prove the checker can still report red. |
| `--self-test` | Fixture tables for the verdict function, the lock readers, the cargo-package writer, the napi-glue writer and the static uv metadata check, plus `stamp_sites` on a staged copy of the real tree. |

Exit codes: `0` pass, `1` the repo is wrong, `2` infrastructure failed.

## The static uv metadata check (SMA-684)

`--write` runs `uv lock` in the release-PR job, where every step can read the App private key.
`uv lock` runs no build backend only while uv can read every local package's metadata without a
build (spec F7). `--check` therefore asserts that no uv workspace member, and no
`[tool.uv.sources]` path source, lists `version`, `dependencies` or `optional-dependencies` in
`[project].dynamic`. A package with no `[project]` table, and a path source that is not a
directory, are violations too. A violation exits 1 and names the file and the field. The check
reads the members from `py/pyproject.toml`'s members glob, not from `SITES`, so it covers
`paigasus-ml` and `paigasus-workflows`. `stage_pristine_tree` stages the same files
(`uv_static_metadata_check --list`), so the negative control and `stamp_sites_self_test` run the
check on a complete tree.

## The changed-path check (SMA-684)

`run_write` records the dirty and untracked paths before it starts (`dirty_paths`, which reads
`git status --porcelain=v1 -z --untracked-files=all`). After it finishes, every path that is new
in that list must be a `SITES` path; `rs/Cargo.lock` and `py/uv.lock` are `SITES` paths.
Otherwise it exits 2 and names the paths. The release-PR stamp step runs `git add -A`, so this
check is the boundary of what that job commits. A path that was already dirty before `--write`
is not new, so a local run in a dirty tree does not red on it. `stamp_sites_self_test` drives the
check on a scratch git repository and pins its two call sites in `run_write`.

## How it runs in CI

`moon.yml`'s `repo:version-lockstep` task, scheduled by `moon ci` because `:version-lockstep`
is in `.github/workflows/ci.yml`'s `T=(…)` array and in CLAUDE.md's marker-delimited copy of it
(`repo:affected-smoke` fails if the two ever disagree). The task script runs `--self-test`
FIRST, then the negative control, then the real check, under an explicit `set -euo pipefail` —
Moon does not enable errexit for `script:` blocks, so a script's status is just its LAST
command's and without that line a failing self-test or control would be masked by the passing
real run. `--self-test` was not invoked from anywhere in CI until SMA-576's fix wave (finding
1) added it here — before that, `run_self_tests` / `SELF_TEST_COUNT` / `site_verdict_self_test`
could bit-rot silently while this doc kept presenting `--self-test` as part of the guard.

Three things about that task are pinned from `ci/affected-graph/ci_targets.py`, which runs inside
`repo:affected-smoke` — a separately scheduled gate, so this one is not the sole judge of its own
wiring:

- `SELF_SCHEDULED_GATES` pins all four script lines, whole-line matched. Whole lines matter
  because `bash ci/version-lockstep/run.sh` is a strict PREFIX of both the `--self-test` line
  and the `--negative-control` line: a substring test would read the script as wired after the
  real run had been deleted.
- `SELF_TASK_EXPECTED_GLOBS` pins the task's eighteen `inputs:` entries. Drop one and the gate
  stops re-keying on that file — it then reports PASS from Moon's cache over a file it never
  read. Seventeen are literal paths, which moon resolves into `inputFiles`; one,
  `py/packages/*/pyproject.toml` (SMA-684), is a glob, which moon resolves into `inputGlobs`.
  That constant compares the whole authored set across both buckets (SMA-576).
- `repo:input-liveness` asserts each of those eighteen still matches a TRACKED file, so moving
  one reds CI instead of silently switching part of this gate off.

## The negative control

`--negative-control` drives three drifts, each staged into its **own** pristine copy of every
version-carrying file (`stage_pristine_tree`, one call per drift): the original drift of
`@paigasus/node-bindings`'s `packagejson` to `99.99.99`; a second drift of a `cargo-lock`
row (`paigasus-proto-derive`'s entry in `rs/Cargo.lock`, made non-uniform against
`paigasus-proto`'s); and a third drift of a `cargo-package` site, `paigasus-wasm`'s
`Cargo.toml` (SMA-685). Each asserts `run_check` exits 1 against its own tree. It drives the
**real** `run_check` rather than a reimplementation — a second, differently-wrong checker
would prove nothing. Splitting the drifts across separate pristine trees, rather than
reusing one scratch dir, is itself load-bearing: `run_check`'s loop keeps checking every site
after the first mismatch, so a shared tree would let the first (packagejson) drift alone
guarantee the second `run_check`'s red regardless of whether the lock-row drift landed at all
(SMA-577 review, Critical).

Measured: with `site_verdict` neutered to always return `OK`, the real run still prints
`== all 20 version-lockstep sites agree ==` and exits 0. The control reds.

## Limitations

**L1 — The control drifts exactly three sites, of twenty.** `--negative-control` mutates site 13
(`@paigasus/node-bindings`'s `packagejson`), one `cargo-lock` row, and one `cargo-package` site
(`paigasus-wasm`'s `Cargo.toml`, SMA-685), and asserts `run_check` exits 1 against each drift's
own pristine tree. That proves the **pipeline** — scratch staging, `run_check`'s loop,
exit-code plumbing — can still report red for the `packagejson`, `cargo-lock` and
`cargo-package` kinds specifically. It does NOT prove the remaining five `read_version`
**kinds** (`cargo-wsdep`, `pyproject`, `pyproject-dep`, `uv-lock`, `napi-glue`) are themselves
honest — including `uv-lock`, whose kind is exercised by `lock_reader_self_test` below but not
by an end-to-end drift here. A reader that silently always printed the expected value,
regardless of what its file actually contained, would pass both the real check (vacuously) and
the negative control for any of those five kinds (since the control never touches that
reader's file).

**L2 — Fixture-table coverage now spans six of the eight `read_version` kinds, plus the
cargo-package and napi-glue writers and the production stamping call site.**
`--self-test` (`SELF_TEST_COUNT=6`) runs six tables: `site_verdict_self_test` (OK/MISMATCH
logic), `lock_reader_self_test`, `cargo_package_writer_self_test` (SMA-685),
`stamp_sites_self_test` (SMA-685), `napi_glue_writer_self_test` (SMA-684), and
`uv_static_metadata_self_test` (SMA-684).

`napi_glue_writer_self_test` drives `write_site napi-glue` against generated glue: a normal bump,
a length-changing bump (`0.9.9` to `0.10.0`), an already-current file, a new version string that
already occurs outside the guards, a decoy old version string (E6), the WASI guard alone, an
in-place write (the inode stays), and the refusals E1, E2, E3, L (also across a length change)
and a non-plain target. It drives E4 (both the G1 and the G2 half) and E5 through
`napi_glue_py verify`, because a correct writer cannot trip them. It also pins the `napi-glue)`
arm in `write_site`, the kind in the `stamp_sites` filter, and the `verify()` call in the write
mode of `napi_glue_py`.

`lock_reader_self_test`, added in SMA-577 to close this limitation for the lock kinds
specifically: before it, neither lock arm had ever been exercised in isolation, so dropping
`paigasus-proto-derive` from `LOCK_MEMBERS[proto:cargo-lock]` would have been a silent
false-green on the very change that introduced that table. `lock_reader_self_test` drives
`read_version` directly against synthetic `Cargo.lock`/`uv.lock` fixtures — a uniform member
set, a **missing member** (must read `""`, not the survivor's version), a non-uniform set
(must read `""`), and a `uv-lock` read — covering both `cargo-lock` and `uv-lock`.

`cargo_package_writer_self_test` drives `write_site` directly against thirteen fixture
scenarios (varied spacing, comments, table order, CRLF, no trailing newline, and
refusal cases). It proves the WRITER is honest for the `cargo-package` kind. It does not read
back through `read_version`, so it closes only the WRITE half of this limitation.

`stamp_sites_self_test` drives the real `stamp_sites` call site on a staged copy of the real
tree. It moves the kernel head and the proto head to two distinct sentinels. It then reads
every `cargo-package`, `pyproject`, `pyproject-dep` and `packagejson` site back through
`read_version`; each site must match its own group's sentinel. It also asserts the publishable
`paigasus-proto-derive` (`cargo-package`, non-head) stayed untouched. This proves the real
production path, not a synthetic fixture. It closes the READ half for those four kinds, but
only on the real tree's own file shapes.

**Limit, stated plainly.** `stamp_sites_self_test` reads back through `read_version`. This is
the same function it is meant to check. A `cargo-package` reader that always printed the
head's version would still pass this table. The site's real value and the head sentinel are
the same value, by construction. This table proves `stamp_sites` writes the right sites. It
does not prove `read_version` reads them correctly, on its own.

`cargo-wsdep` still has no fixture of its own, so a broken parser inside it — the wrong TOML key,
a regex that matches the wrong table — is caught only if it happens to manifest on the real
repo's current files. `napi-glue` has a WRITE fixture since SMA-684
(`napi_glue_writer_self_test`), and `stamp_sites_self_test` writes the real `index.js`, reads it
back and counts both literal forms, but the READ arm has no synthetic fixture. The
`cargo-package` kind's READ side is also proven only on the real tree's own file
shapes, not on the varied layouts the write-side fixtures cover.

**L3 — The non-vacuity anchors are literals, not derived.** Both the `checked == ${#SITES[@]}`
loop guard and the `EXPECTED_SITE_COUNT` anchor above it are numbers, not a comparison against
Moon's own resolved view of the task graph — pragmatic given this script has no dependency on
the `moon` binary or a YAML parser, but it means each is only as good as the reviewer noticing
a stale count on an edit that adds or removes a site. `ci_targets.py`'s
`SELF_TASK_EXPECTED_GLOBS["version-lockstep"]` is the independent second signature on the same
number, from a separately scheduled gate — not proof either number is right, but proof they
cannot silently drift apart from each other.

## `--write` implementation notes

- The `packagejson` writer edits the `"version"` field **in place with a regex**, the same
  approach as `pyproject`, rather than round-tripping through `json.loads`/`json.dumps`.
  A full re-serialization reformats every array in the file onto multiple lines (Python's
  `json.dumps` has no compact-array mode) — measured against this repo's committed
  `package.json` files, that reports `wrote N site(s)` and rewrites unrelated Prettier-style
  formatting even when the version was already correct, which breaks the "already in
  lockstep" no-op case and pollutes the release-PR diff.
- The `napi-glue` writer (SMA-684) edits the version literals of `index.js` as text. It
  replaced `napi build`, which compiled the binding crate and its whole build graph in the
  release-PR job, where every step can read the App private key. It checks the file in memory
  before it writes (spec §5.1: V, E1, E2, E6, E3, L, E4, E5) and writes in place, never by
  rename, because a rename breaks the pnpm hard link (`ts/CLAUDE.md`). A guard format change in
  a new `@napi-rs/cli` exits 2. `stamp_sites_self_test` runs the writer on the real `index.js`,
  and `index.js` is an input of this task, so a napi bump PR that regenerates the glue reds here
  before a release. `run_write` runs no `pnpm`, `npx`, `napi` or `node` command;
  `napi_glue_writer_self_test` pins that.
