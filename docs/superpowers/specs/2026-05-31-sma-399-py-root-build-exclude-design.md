# SMA-399 — py root `:build` emits a junk UNKNOWN wheel; exclude the inherited `build`

**Status:** Design approved
**Date:** 2026-05-31
**Linear:** SMA-399
**Branch:** `feature/sma-399-py-py-root-build-emits-junk-unknown-wheel-exclude-inherited`
**Related:** SMA-394 (the TS analog), SMA-361 (CI review that surfaced this), SMA-401 (the N+1
whole-tree redundancy spun out of this spec's review)

## Problem

The `py` root project (`py/moon.yml`, `language: python`) inherits `build: uv build` from
`.moon/tasks/python.yml`. But `py/pyproject.toml` is a **virtual uv workspace root** —
`[tool.uv.workspace]` with **no `[project]` table**. Running `uv build` there warns
*"appears to be a workspace root without a Python project"* and falls back to a legacy
setuptools build of the bare directory, producing meaningless artifacts:

```
py:build | warning: `…/py` appears to be a workspace root without a Python project; …
Successfully built dist/packages-0.0.0.tar.gz
Successfully built dist/unknown-0.0.0-py3-none-any.whl   # UNKNOWN-0.0.0, via packages.egg-info/
```

It **exits 0**, so it is green, not red — it does not fail CI. But `py:build` builds garbage and
runs a pointless task whenever the py root is affected. The real per-package builds already work
as their own Moon projects (`paigasus-kernel-py:build`, `-ml`, `-proto`, `-workflows` — all pass
via the `uv_build` backend, emitting the real `paigasus_*-0.0.0-py3-none-any.whl` wheels).

## Root cause / context

`.moon/tasks/python.yml` attaches `build`, `lint`, `fmt`, `typecheck`, `test` to **every**
`language: python` project (scoped `inheritedBy.languages: ['python']`):

```yaml
build:     uv build
lint:      uv run ruff check .
fmt:       uv run ruff format --check .
typecheck: uv run basedpyright
test:      uv run pytest
```

Of these, **only `build` is per-distribution.** `lint`/`fmt`/`typecheck`/`test` are all
config-driven whole-tree tasks that run correctly from the `py/` root cwd (ruff/basedpyright/
pytest read the central config in `py/pyproject.toml`). The py root inherits and runs them
whole-tree — the same role `ts/`'s comment ascribes to its inherited `lint`/`fmt`/`test`.
`uv build` is the odd one out: it expects a `[project]` table, which the virtual workspace root
deliberately lacks.

### Why this is the py-twin of SMA-394 — and where it diverges

SMA-394 made Moon own the TS `:build`/`:typecheck` graph and **excluded** the inherited
`build` *and* `typecheck` at the `ts` root. SMA-394's spec explicitly forward-files this py
cleanup as a follow-up twin. The shapes rhyme: both roots are `layer: configuration` config
roots with no buildable root package; real builds live under `packages/*`.

They **diverge on `typecheck`**, and that divergence is the one real design decision here:

- **TS** excluded `typecheck` because it genuinely *broke* at the root — `tsc -p tsconfig.json
  --noEmit` fails `TS5058` since `ts/` has only `tsconfig.base.json`, no root `tsconfig.json`.
- **PY** root `typecheck` does **not** break. `uv run basedpyright` from `py/` reads the central
  `[tool.basedpyright]` config (`include = ["packages/*/src", "packages/*/tests"]`) and runs
  clean — verified: **0 errors, 0 warnings, 0 notes** (it currently analyzes 0 files; the four
  packages are still empty scaffolds with a one-line `__init__.py` each). It is *not* a uniquely
  necessary pass — per-package `typecheck` covers the same configured tree (see Out of scope) —
  but it runs correctly, which `build` does not.

So the AC's conditional — *"(and `typecheck` if it has the same root problem)"* — resolves to
**false** for `typecheck`. Mirroring TS's exclude *list* verbatim would cargo-cult the symptom
rather than the cause: it would drop a working whole-tree check and split `typecheck` off from
the `lint`/`fmt`/`test` family it belongs to. We mirror the **idiom** (`inheritedTasks.exclude`),
not the list.

## Decision

Exclude **only `build`** at the py root, via the same `workspace.inheritedTasks.exclude` field
`ts/moon.yml` and `commitlint-config-ts` already use. Keep `language: python`, the inherited
whole-tree `lint`/`fmt`/`typecheck`/`test`, and the existing `fileGroups`. Moon's per-project
fan-out keeps owning the `:build` graph (the four `py/packages/*` projects).

### `py/moon.yml` end state

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

layer: 'configuration'
language: 'python'

# The inherited fileGroups from .moon/tasks/python.yml assume src/ at the project root.
# The py workspace keeps sources under packages/*/src, so extend the inherited groups
# here. Moon merges (not overrides) fileGroups across the layers, so the resolved
# @group(sources) and @group(tests) contain both python.yml's defaults and these
# additions — fine in practice because py/src/ and py/tests/ don't exist; only
# packages/*/src/** and packages/*/tests/** actually match.
fileGroups:
  sources:
    - 'packages/*/src/**/*'
  tests:
    - 'packages/*/tests/**/*'

# The py root owns no build of its own: Moon's per-project fan-out owns the whole :build
# graph — each py/packages/* inherits `uv build` and emits a real wheel via the uv_build
# backend. We EXCLUDE (not merely omit) the inherited build here, because py/pyproject.toml
# is a virtual uv workspace root ([tool.uv.workspace], NO [project] table); `uv build` there
# falls back to legacy setuptools and emits a junk UNKNOWN-0.0.0 wheel + packages.egg-info/.
# Unlike ts/ (SMA-394, which excludes typecheck too), typecheck is KEPT here: `uv run
# basedpyright` from py/ runs clean (it reads the central [tool.basedpyright] config), so it has
# none of the root problem `build` has. It stays inherited alongside the whole-tree lint/fmt/test
# for consistency — not as a uniquely necessary pass (per-package typecheck already covers the
# same configured tree; see the redundancy note deferred to a follow-up issue).
workspace:
  inheritedTasks:
    exclude: ['build']
```

Field order follows the CONTRIBUTING rule (`$schema`, `layer`, `language`, `fileGroups`, …,
`workspace` trailing) — identical placement to `ts/moon.yml`.

### Cross-file legibility: make the ts/py divergence deliberate-on-its-face

After this lands, the two `layer: configuration` roots intentionally differ — `ts/moon.yml`
excludes `['build', 'typecheck']`, `py/moon.yml` excludes `['build']`. To stop a future
maintainer diffing the two roots from reading that as drift and "fixing" it, add a one-line
back-reference in `ts/moon.yml`'s exclude comment pointing the other way:

```yaml
# (SMA-399: py/moon.yml deliberately excludes only ['build'] — basedpyright reads a central
#  config and runs fine at the py root, whereas tsc needs a root tsconfig.json this dir lacks.)
```

This is a comment-only edit to `ts/moon.yml` (no task-graph change), making the change span two
files instead of one. It is the only reason this is not a strictly single-file change.

### What deliberately stays

- `language: python` — kept, so the root still inherits `lint`/`fmt`/`typecheck`/`test`.
- The inherited whole-tree `typecheck` (`uv run basedpyright`), `lint` (`uv run ruff check .`),
  `fmt` (`uv run ruff format --check .`), and `test` (`uv run pytest`) on the root. These run
  from `py/` cwd and cover the whole tree via the central config; left unchanged.
- `fileGroups` — still feed the input globs (and thus cache invalidation) of those inherited
  tasks.

### Field already proven

`workspace.inheritedTasks.exclude` is a valid project-level field in Moon 2.2.5, established in
the repo by SMA-395 (`commitlint-config-ts`) and SMA-394 (`ts/moon.yml`). This is the third use
of the same field/idiom.

### Resilience, not just cleanup

`py:build` is green today only because this uv version treats *"workspace root with no
`[project]` table"* as a **warning plus a legacy setuptools fallback**, not an error — so the
root build's greenness is contingent on uv tolerating a malformed build target. Excluding the
task removes that dependency: we stop asking uv to build something that isn't a distribution,
rather than relying on it to keep accepting the attempt. This makes no claim about uv's roadmap;
the point is only that not depending on the lenient fallback is strictly safer than depending on
it. It reframes the change from "stop emitting junk" (cosmetic) to "stop depending on uv
tolerating a malformed build target" (resilience).

## No README fallout

Unlike the TS twin, no docs change is needed:

- `py/README.md`'s command table lists only `py:lint` / `py:format` / `py:typecheck` / `py:test`
  — there is **no `py:build` row** to update.
- `py:typecheck` stays valid (the task is kept), so that row remains correct.

(`py/README.md` line 30 labels `fmt` as `moon run py:format` — a pre-existing mismatch with the
actual `fmt` task name, orthogonal to this issue and left untouched.)

## Alternatives considered

- **Exclude both `build` and `typecheck` (mirror `ts/moon.yml` verbatim).** Rejected: py's root
  `typecheck` is not broken (proven: runs clean, 0 errors) and belongs to the root-owned
  whole-tree family with `lint`/`fmt`/`test`. Mirroring the exclude *list* copies TS's symptom,
  not its cause.
- **Redefine root `build` as a no-op task.** Rejected: Moon has no clean builtin no-op, `true` is
  platform-fragile, and it adds task noise. `inheritedTasks.exclude` is the established repo idiom.
- **Add a `[project]` table to `py/pyproject.toml` so `uv build` produces something real.**
  Rejected: the root is a *virtual* workspace aggregator by design; it should not be a publishable
  distribution. Excluding the task is the correct model, matching `ts/`.

## Out of scope / non-goals

- **N+1 whole-tree redundancy.** Every `py/packages/*` project also inherits
  `typecheck`/`lint`/`fmt`/`test`, and the central basedpyright/ruff/pytest config is keyed off
  `packages/*` (e.g. `testpaths = ["packages/*/tests"]`, basedpyright `include = ["packages/*/src",
  …]`). So each per-package run is configured against the *whole* `packages/*` tree, not just its
  own dir — per-package runs overlap rather than partition — and the root runs each once more on
  top. `moon ci :test` therefore trends toward *(N+1)×* the full suite as packages and tests grow
  (for `pytest`, that also means the same tests counted/reported multiple times, which muddies
  failures). It is masked today only because the packages are empty (basedpyright analyzes 0
  files, pytest collects 0 tests). This is pre-existing, repo-wide (the TS twin's spec flags the
  same lint/fmt/test redundancy and defers it), and orthogonal to the build junk — so it is right
  to leave it out of *this* change. Tracked as its own follow-up in **SMA-401**. (It also means
  keeping the root
  `typecheck` adds no unique coverage — see the corrected rationale above; the reasons to keep it
  are non-breakage and consistency with `lint`/`fmt`/`test`, not uniqueness.)
- **Existing local junk artifacts.** `py/dist/unknown-*.whl`, `py/dist/packages-0.0.0.tar.gz`,
  and `py/packages.egg-info/` are already gitignored (confirmed via `git check-ignore`) — not in
  the repo. After this change they simply stop being regenerated. Removing the stale local copies
  is an optional `git clean -fdX py/` / manual `rm`, not required by this change.

## Acceptance criteria

- [ ] `py/moon.yml` excludes the inherited `build` via `workspace.inheritedTasks.exclude:
      ['build']`, mirroring the `ts/moon.yml` idiom; `typecheck` is deliberately **kept** (it has
      no root problem — see Decision).
- [ ] `moon run py:build` no longer runs `uv build` at the root (reports an unknown task) / no
      longer emits an `UNKNOWN` wheel or `packages.egg-info/`.
- [ ] `moon ci :build` still covers every real `py/packages/*` project.
- [ ] Whole-graph `moon run :build` stays green.

## Verification plan

1. **Inspect the resolved root task list:**
   ```bash
   moon project py
   ```
   Expect: no `build`; `lint`, `fmt`, `typecheck`, `test` still present.
2. **Root build target is gone:**
   ```bash
   moon run py:build      # expect: unknown task — proves the inherited build is excluded
   ```
3. **Cold full build graph emits no junk:**
   ```bash
   rm -rf py/dist py/packages.egg-info
   moon run :build        # expect 0 failed; only the 4 paigasus_*-py builds run
   ls py/dist             # expect: NO unknown-0.0.0*.whl, NO packages-0.0.0.tar.gz
   test ! -e py/packages.egg-info && echo "no egg-info ✓"
   ```
4. **Affected-graph CI form** (matches what PR CI runs):
   ```bash
   moon ci :build --base origin/main
   ```
   Expect: every `py/packages/*` build present, root `py:build` absent.
5. **The kept `typecheck` still passes** — asserts the load-bearing half of the keep-`typecheck`
   decision, not just the removal of `build`:
   ```bash
   moon run py:typecheck   # expect: 0 errors, 0 warnings (whole-tree basedpyright still runs at py/)
   ```

## Files touched

- `py/moon.yml` — add `workspace.inheritedTasks.exclude: ['build']` with the explanatory comment;
  keep `fileGroups`, `language: python`, and the inherited whole-tree `lint`/`fmt`/`typecheck`/
  `test`. This is the only functional change.
- `ts/moon.yml` — comment-only: add a one-line back-reference in the existing exclude comment
  noting that `py/` deliberately excludes only `['build']` (the divergence is intentional, not
  drift). No task-graph change.
