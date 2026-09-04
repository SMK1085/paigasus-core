<!-- SPDX-License-Identifier: Apache-2.0 -->

# `ci/release-plan`

**There is no `repo:release-plan` Moon target.** This suite runs as check 11 of `repo:actionlint`, and the release workflow's `plan` job runs `--github-output` directly. The heading said `repo:release-plan` until the SMA-603 fix wave; a reader who went looking for that task found nothing.

Decides whether a push to `main` has anything to release. The release workflow's `plan` job
runs this decision first, so a push that changes nothing releasable can skip its ~15-minute
build matrix instead of running it and finding nothing to publish.

## Why this is not a dry run

The obvious design reads `release-plz release --dry-run --output json` and skips when its
`releases` array is empty. That reading is wrong. Measurement M6 in the SMA-603 spec shows
release-plz, with only the `kernel` version group bumped, logging that it WOULD publish
`paigasus-kernel` and cut `paigasus-kernel-v0.1.1` — and still printing `{"releases":[]}` at
exit 0. That array records releases release-plz PERFORMED, and a dry run performs none. It
cannot distinguish "nothing to release" from "a release is pending." Reading it would have
silently, greenly, and permanently skipped every kernel-group release.

## What it reads instead: tag existence

Measurements M2 and M6 both show release-plz short-circuiting on tag existence, before any
registry or cargo work: `Already published - Tag <pkg>-v<version> already exists`. Tag
existence is a pure function of local git state. It needs no token, no network, and no cargo
call — and it can be fixture-tested in-process, which the dry-run reading could not be.

The decision, `decide(event_name, packages, tags)`, returns `True` ("nothing to release, skip
the build") only when every package release-plz would tag already has that tag. Any other
state builds.

The package set is DERIVED from `rs/Cargo.toml`'s `[workspace] members`, not from a hardcoded
`crates/*/*` glob. That glob matched today's layout and nothing else: a publishable member
declared anywhere outside it was invisible, no tag was ever demanded for it, and a release with
its tag still uncut read as "every releasable package is already tagged" — a silent skip.
`--assert`'s strict-equality pin could not catch that, because both sides of its comparison come
from the same function. An unresolvable member pattern is `InconclusiveError`, which builds. A
`[workspace] exclude` list is refused outright rather than ignored: this function does not model
exclusion, and reading it as absent would make the skip permanently unreachable in silence.

## Fail-safe direction

Every inconclusive outcome returns `False`, which builds. A false build costs runner time. A
false skip silently drops a release. The two costs are not symmetric, so nothing in this
checker may invert that direction — not to save runner minutes, not to simplify a branch.

Concretely: a non-`push` event always builds (a `workflow_dispatch` is a deliberate act
meaning "release now," and it is the lever for the state where tags are cut but a registry
publish is missing); an empty releasable-package set builds; a repository reporting no tags
at all builds; and any collection failure — an unreadable `release-plz.toml`, a package with
no literal `[package] version`, a `git tag -l` failure — builds.

## What counts as releasable

`releasable_packages()` resolves manifests from `rs/Cargo.toml`'s `[workspace] members` and pairs
them with
`rs/release-plz.toml`. A package is releasable, and therefore expected to be tagged, when its
Cargo manifest does not say `publish = false` **and** its `rs/release-plz.toml` entry says
neither `release = false` nor `publish = false`. A package with no `rs/release-plz.toml`
entry at all reads as releasable — release-plz's own default is `release = true` /
`publish = true` — so a newly added, unlisted crate counts as releasable and its missing tag
makes the decision build. That is the fail-safe direction again, applied to a config gap
rather than a runtime error.

`EXPECTED_RELEASABLE` pins the derived set to today's three: `paigasus-kernel`,
`paigasus-proto`, `paigasus-proto-derive`. `--assert` checks the derivation against that pin
by strict equality — the `EXPECTED_PR_SUBJECTS` idiom `ci/workflow-credentials` already uses.
The **runtime** path (`--github-output`, and the bare `run(...)` it calls) never reads this
set; it derives fresh every time, so a newly publishable crate is honoured immediately even if
nobody re-baselined the pin. The pin exists only to force that re-baseline to happen
consciously, on a gate (`--assert`) CI runs, never to drive the actual decision.

## The tag-format assumption

`tag_for(name, version)` assumes release-plz's default tag format, `<package>-v<version>`.
`config_sections()` reads `rs/release-plz.toml`'s two sections and validates five properties
before anything else runs: `[workspace]` is a table, `package` is an array of tables, each entry
in it is a table, each entry has a non-empty string `name`, and no `name` repeats. Any other
shape is now **refused** with `InconclusiveError` rather than silently ignored — a malformed
`[workspace]`, a malformed `[[package]]` entry, or a nameless entry used to fall through a
`... or {}`/`isinstance` guard that substituted a harmless-looking default past it (SMA-608). The
duplicate-`name` case is the one whose old direction was a silent **skip**, not a build: the
entry map kept the LAST entry for a repeated name, so a duplicate carrying `release = false`
dropped that crate from the demanded-tag set entirely, with nothing raised.
`assert_default_tag_format()` then receives these already-validated sections — it no longer reads
the file itself — and raises `InconclusiveError`, and only `InconclusiveError`, so the fail-safe
direction still applies, if `rs/release-plz.toml` sets `git_tag_name` anywhere, workspace-wide or
on an individual package. This checker does not attempt to parse a custom tag template; it
refuses to guess and builds instead.

## Modes

`run.sh` has four modes, and one of them is required:

- `--self-test` — runs `release_plan.py --self-test` in-process: the pure `decide()` fixture
  table (nine rows) plus fifteen collection-layer rows that build throwaway trees under
  `tempfile.mkdtemp()` to exercise paths a pure-function fixture cannot reach. The original six: a
  missing `release-plz.toml`, a `version.workspace = true` inheritance, a `git_tag_name` override,
  a publishable member declared OUTSIDE `crates/*/*`, an unresolvable `[workspace] members`
  entry, and a malformed `release-plz.toml` (which must exit 3, not 1). SMA-608 adds nine more:
  a non-table `[workspace]`, an array-of-tables `[workspace]`, a table-valued `package` section, a
  non-table `[[package]]` entry, a nameless `[[package]]` entry, a duplicated `[[package]] name`,
  an untyped collection failure making `--assert` exit 3, an untyped collection failure making
  `run()` build rather than raise, and a check that the five shape-validation error markers above
  are mutually exclusive (so a reworded message cannot silently start matching the wrong fixture).
- `--negative-control` — eight rows against real and throwaway trees. Row 1 proves the checker's
  exit-3-to-1 translation; row 2 proves `--self-test` still notices a broken table. Rows 3 and 4
  each build their own throwaway git repository — one crate, one commit, tags added by the row
  — and invoke `release_plan.py` directly against it, asserting `nothing_to_release=true` when
  the wanted tag exists and `nothing_to_release=false` when it does not. Without both, the
  control cannot tell a working decision from one wired to a constant in either direction.
  **They use a synthetic tree rather than the real repository on purpose**: asserting a
  direction against the live repository redded this gate on exactly the PR it exists to serve —
  a release PR bumps `rs/crates/*/Cargo.toml` before the new tags exist, so a row asserting
  `nothing_to_release=true` there would fail every time. Row 5 then closes the coverage gap
  rows 3/4 leave: it runs the real `--github-output` mode against the real repository, with
  `$GITHUB_OUTPUT` pointed at a scratch file, and asserts the wrapper exits `0` and writes
  exactly one matching verdict line — proving the wrapper's non-zero/malformed-output catch, its
  `::warning::` annotation, and its `$GITHUB_OUTPUT` append all still work. Row 5 asserts nothing
  about *which* verdict comes back, for the same reason rows 3/4 no longer touch the real
  repository directionally: the real repository's tag state is not a safe thing for this control
  to depend on. **Row 6** is the C1 regression row: it runs `--github-output` under a
  hermetic `PATH` holding symlinks to `bash`, `dirname`, `grep` and `tail` and nothing else,
  asserts `uv` is genuinely unreachable under it, and then asserts the wrapper still exits
  `0` and writes `nothing_to_release=false`. **Row 7** mutates a COPY of `release_plan.py`,
  inverting the first fixture's expected verdict, and asserts the mutant's `--self-test`
  exits 3 — which is what proves `self_test()`'s FIXTURES loop still evaluates its rows
  rather than having been deleted in silence. **Row 8** does the same for the
  COLLECTION_ROWS loop and the `[workspace]` shape check specifically: it mutates a COPY of
  `release_plan.py`, neutering `config_sections`'s `isinstance(workspace, dict)` guard with a
  `if False and ...` condition (a condition neutering, not a line deletion — every
  `raise InconclusiveError(...)` spans two physical lines, so deleting the raise leaves an empty
  `if` body and an `IndentationError`, which would red this row for the wrong reason), asserts via
  `cmp -s` that the mutation actually changed the file (so a renamed check cannot make the row
  vacuous), asserts the mutant's `--self-test` exits 3, and then greps the mutant's stderr for the
  specific `"a non-table [workspace] is inconclusive"` fixture label. The stderr assertion exists
  because rc 3 alone is not sufficient: `self_test()`'s own arity floor also returns 3 if
  `COLLECTION_ROWS` is short two or more rows, so an rc-only check would go green whether the
  shape check fired or the floor did — the two controls covering for each other's absence.
- `--assert` — runs `release_plan.py --assert` against the real repository: the derived
  releasable set must equal `EXPECTED_RELEASABLE`, and the repository must report at least one
  tag (a shallow checkout with no tags cannot exercise the real decision).
- `--github-output` — the runtime entry point invoked by the release workflow. See the next
  section; it is the one mode that never fails its caller.

## The `--github-output` arm inverts the usual contract, deliberately

Every other mode follows this repo's usual three exit codes: `0` pass, `1` the repo is wrong,
`2` infrastructure failed. `--github-output` always exits `0`.

A failed `plan` job would **skip** its dependents rather than build them — GitHub applies an
implicit `success()` to a job-level `if:` with no status function named — so a broken decision
that exited non-zero would stop the release entirely rather than fail safe. `--github-output`
therefore catches every failure mode of the underlying checker call, writes
`nothing_to_release=false` to `$GITHUB_OUTPUT`, prints a `::warning::` annotation naming the
failure, and exits `0`. The build proceeds; nothing is silently skipped.

`--self-test`, `--negative-control`, and `--assert` keep the normal contract. CI runs those
three as the actual gate; `--github-output` is exercised by the negative control's row 5 (see
above — direction-agnostic, against the real repository, with `$GITHUB_OUTPUT` pointed at a
scratch file) and by the release workflow itself at runtime.

## The checker's own exit codes, and why 3

`release_plan.py` exits `0` pass, `2` its own infrastructure failure (an argument error is the
only case today), and `3` for an assertion failure — never `1`. That was FALSE until the
SMA-603 fix wave, and the code was fixed rather than the sentence: `workspace = 3` in
`rs/release-plz.toml` used to raise a bare `TypeError` out of `assert_default_tag_format`, which
`_assert_repo`'s `except InconclusiveError` did not catch, so `--assert` exited 1 with a traceback
and `run_checker` mapped that onto `die_infra` (2) — reporting a broken repository file as
"infrastructure failed". `_assert_repo` now catches `Exception`, because collection reads
only repository files, so any failure of it is a statement about the repository. SMA-608 has
since typed the `workspace = 3` shape itself — `config_sections` now raises `InconclusiveError`
for it before `assert_default_tag_format` runs — but the broad catch stays as the floor for
shapes this module does not model, and two self-test rows (`_untyped_collection_failure_asserts_three`
and `_untyped_collection_failure_builds`, MEASURED against a crate manifest holding
`package = 3`) red if it is narrowed to `except InconclusiveError`. `uv` itself exits `1` on a
failed resolution. A shared code would let a `uv`/PyPI-mirror hiccup during `uv run` read as
"the repo is wrong" instead of "the tool failed." `run.sh`'s `run_checker()` owns the
translation: checker `0` -> wrapper `0`; checker `3` -> wrapper `1`; anything else -> wrapper
`die_infra` (`2`).

## The dependency, and why there is none

`ci/release-plan` is its own `uv` project, deliberately not the main `py/` workspace. `py/` is
a `[tool.uv.workspace]` root whose member `paigasus-kernel` depends on `paigasus-py-bindings`
by path, and that crate builds with maturin — so `uv run --project py` compiles a PyO3 cdylib.
This checker needs only `tomllib`, which has been part of the standard library since Python
3.11, so it declares zero dependencies. `uv lock` still runs, and the committed
`ci/release-plan/uv.lock` still pins the interpreter floor the same way a real dependency set
would.

## Non-goals

- **It does not verify a package is actually reachable on crates.io, PyPI, or npm.** It checks
  only that the git tag `release-plz` would cut already exists locally. A tag that exists but
  whose registry publish failed reads as "already released" here.
- **It does not verify that `release-approval` (or any other required-reviewer gate) has its
  required reviewers configured.** That is a repository/environment setting this checker never
  reads.
- **Its tag-format assumption is asserted only by `assert_default_tag_format()`.** A
  `git_tag_name` override anywhere in `rs/release-plz.toml` is caught — the checker refuses to
  guess and builds instead — but nothing here understands a custom template well enough to
  follow it.
