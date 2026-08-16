# actionlint gate

Lints `.github/workflows/**` and proves every `paths:` filter glob still matches the tree.

## Why

A `paths:` filter that comes to match nothing does not error. The workflow stops running,
forever, with no red check and no notification — `prebuild.yml` triggers only on
push-to-`main`, `workflow_dispatch` and a narrow `pull_request` filter, so its 7-platform
verification would silently cease. See SMA-525 and
`docs/superpowers/specs/2026-08-16-sma-525-actionlint-gate-design.md`.

actionlint alone is **not** sufficient: it validates syntax and has no view of the file tree,
so a valid-but-never-matching glob (`rz/**`) passes it cleanly. Checks 5–7 close that.

## The checks

| # | Check |
|---|---|
| 1 | `actionlint` over the auto-discovered workflow set |
| 2 | No `.github/actionlint.{yaml,yml}` carrying an `ignore:` key (it would neuter check 1 invisibly) |
| 3 | Four stdin fixtures, one per defect class, each must fail **with its expected rule tag** |
| 4 | A healthy stdin fixture must pass — the control for check 3 |
| 5 | Every `paths:` glob is in the supported vocabulary and matches the tracked tree |
| 6 | Every extracted `paths:` key carries at least one sequence entry |
| 7 | Extractor self-test against a fixture table (`run.sh --self-test`) |

## Supported glob vocabulary

`git ls-files ':(glob)P'` is not a sound model of GitHub filter patterns, so check 5 accepts
only the subset where both provably agree:

- **literals** — must be an *exact* tracked file path. A bare directory name (`rs`) matches
  nothing on GitHub, though git's pathspec would match everything beneath it.
- **`dir/**`**, **`**/name`** — `**` as a whole path component.
- **`*`** within a single segment.

Rejected loudly, never guessed at: `?`, `+`, `[]`, and `**` embedded in a segment (`**.js`).

## Escape hatches

- A **new GitHub runner label** the pinned actionlint does not know: add it to
  `self-hosted-runner.labels` in `.github/actionlint.yaml`. Check 2 permits that file; it bans
  only `ignore:`.
- A **GitHub-valid pattern outside the vocabulary**: add it to `SKIP_PATTERNS` in `run.sh` with
  a comment justifying it and saying what verifies it instead.
- **Anything worse**: drop `:actionlint` from `T=(…)` in `.github/workflows/ci.yml`. One line.

## Cost

`inputs: ['**/*']` is deliberate (see the WHY comment on the `actionlint:` task in `moon.yml`),
and it was benchmarked before being accepted (SMA-525): the gate itself runs in ~1.4s standalone
(`ci/actionlint/run.sh`, bypassing Moon), but Moon's own per-task floor in this repo is ~9s
regardless of what a task does — an existing narrow-input task (`repo:promtool`) measures about
the same. Once `.moon/workspace.yml`'s `hasher.ignorePatterns` excludes gitignored dependency
trees (`node_modules`, `target`, `.venv`) from the hash walk, broad `inputs: ['**/*']` costs
only ~1s over a narrow input list — not the ~8.5s it costs without that filter. Narrowing this
task's inputs would not meaningfully help; do not do it without also revisiting
`hasher.ignorePatterns`.

## Running it

```bash
moon run repo:actionlint      # via Moon, as CI does
ci/actionlint/run.sh          # directly, bypassing the Moon cache
ci/actionlint/run.sh --self-test   # extractor fixtures only, for fast iteration
```

Exit codes: `1` = assertion failure, `2` = infrastructure error.
