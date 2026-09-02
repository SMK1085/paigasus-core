<!-- SPDX-License-Identifier: Apache-2.0 -->

# `repo:ruff-ci`

Lints `ci/**/*.py` against `py/pyproject.toml`'s Ruff rule set (SMA-539).

## Why

`.moon/tasks/python.yml` scopes `ruff check` to the `py` project. `ci/` has never been
linted by anything, and it merged through a full review carrying three RUF005
violations before Task 1 of this issue cleared them (SMA-541). This gate closes that
gap without introducing a second Ruff configuration: the rule set, its version, and its
resolution all come from `py/pyproject.toml` + `py/uv.lock`. There is no
`ci/ruff/pyproject.toml`, no second lockfile, and no `per-file-ignores` carve-out for
`ci/` — a violation here is the same violation it would be in `py/`.

## What it asserts

`run.sh` derives the set of tracked `ci/**/*.py` files, resolves the `ruff` binary from
`py`'s locked environment, and runs `ruff check --config py/pyproject.toml` against that
exact file list. Nothing else. It does not lint `py/` (already gated by
`.moon/tasks/python.yml`), and it does not run `ruff format` (see below).

## Exit codes, and why 1 and 2 must not collapse into each other

`0` pass, `1` the repo is wrong, `2` infrastructure failed — the repo's usual contract.

The reason this gate is two steps (resolve the binary, then invoke it directly) instead
of one (`uv run --project py -- ruff check ...`) is that **both halves of that combined
command exit 1 on their own failure**: `ruff check` exits 1 on a lint violation, and `uv`
exits 1 on a failed dependency resolution and on `--locked` finding a stale lock. A
single piped invocation cannot tell "`ci/` has lint violations" apart from "PyPI is
down" — both would read as the same rc 1. CLAUDE.md records this exact lesson for
`repo:workflow-credentials`, which hit it first.

`resolve_ruff` isolates the `uv`-shaped failure: it resolves the `ruff` executable's
path via `uv run --locked --project py python3 -c '...shutil.which("ruff")...'`, and any
non-zero exit or a non-executable result there is reported at **rc 2** with an
infrastructure message. Only once that has succeeded does `run.sh` invoke `"$ruff"
check` directly — no `uv` in that second command at all — so a violation it reports can
only mean rc 1. This is also why `.moon/tasks/python.yml`'s bare, re-locking `uv run
ruff check .` is not reused here: `py/uv.lock` can genuinely be stale in a working tree,
and without the split that reds this gate for a reason a contributor would "fix" by
re-locking a file this gate has no business touching.

Verified live (Step 5 of the implementation task): temporarily pointing `resolve_ruff`
at a `uv` project with no `ruff` installed (`--project ci/release-plan`) produces

```
ruff-ci: could not resolve ruff via 'uv run --locked --project py' — run 'uv sync --project py'
```

at **rc 2**, never rc 1.

## Corpus derivation: `git ls-files`, not a glob ruff walks itself

The file list `run_check` passes to `ruff check` is derived once, with `git ls-files`,
and passed as an explicit argument list. It is not asserted against ruff's own
discovery after the fact — the list **is** what ruff is given, so the two cannot drift
apart.

**The `:(glob)` pathspec magic is required, and easy to get backwards.** Git's default
pathspec matching does not set `FNM_PATHNAME`, so `**` behaves like two `*`s and the
literal `/` in the pattern must still line up with a `/` in the path:

- `'ci/**/*.py'` **without** `:(glob)` matches `ci/pyo3-stub/check.py` but **not** a
  top-level `ci/foo.py` — the pattern's own `/` after `ci` has nothing to match against
  a path with no further `/` before the last component.
- With the `:(glob)` prefix, git turns on `FNM_PATHNAME`-style matching and the same
  pattern reaches nested files as expected, but a top-level file needs its own
  pathspec: `'ci/*.py'`.

Both pathspecs are passed to every `git ls-files` call in this script
(`ruff_corpus`). Measured on a temporary `ci/_probe.py`: dropping either one drops a
real, moon-schedulable file out of the corpus silently — the gate would still report
"N files clean" for a smaller, wrong N. `run.sh --self-test` pins this with `ci/top.py`
(a top-level file) and `ci/sub/nested.py` (a nested one), both required present.

**The corpus floor.** `run_check` refuses to proceed if the derived file list has fewer
than 10 entries — the tracked count at the time this gate was written. This is what
stops a moved or renamed `ci/` directory from silently emptying the gate: `git
ls-files` returning zero rows is not a lint pass, it is the corpus collapsing, and
`repo:input-liveness` cannot see this (`ci/affected-graph/task_inputs.py` only proves
that *declared* Moon task inputs are live — it has no view of what a gate's own script
derives at runtime).

## Why the negative control runs inside the worktree, not a bare `mktemp -d`

`negative_control` builds its fixture at `$REPO_ROOT/.ruff-negctl-XXXXXX` — nested
inside the repository, not off in `/tmp` — and initializes it as its own git repo, then
copies the real `ci/` tree into it and plants a `RUF005` violation
(`ci/probe/violation.py`). A bare `mktemp -d` outside any git repository would make
`git ls-files` return nothing at all, and ruff's own exclusion handling (`.gitignore`,
`respect-gitignore`) behaves differently outside a repo too — a fixture built that way
would exercise a different code path than the real run and prove nothing about it.

Two pieces of the real tree are pulled in without being copied:

- **`py/` and `rs/` are symlinked in**, not copied. `resolve_ruff` and the `--config`
  path both resolve `py` relative to the current directory, and the control needs the
  real, already-synced venv and lock — not a second one built from scratch on every
  run. `rs/` has to come along too: `py/packages/paigasus-kernel`'s `path =
  "../../../rs/crates/bindings/paigasus-py-bindings"` source dependency is resolved
  textually against the symlink's own location, not the real directory it points to, so
  without a sibling `rs/` next to the symlinked `py/`, `uv` fails with "Distribution not
  found" (measured while building this gate).
- **The root `.gitignore` is copied in, and `git add` is not forced.** The real
  repository has untracked `.venv` trees under `ci/release-plan/` and
  `ci/workflow-credentials/`. Copying the root `.gitignore` into the fixture and adding
  files without `-f` means those directories are excluded from the fixture's own index
  exactly the way they are excluded from the real one — by tracking, not by a pattern
  this script owns. Force-adding them would pull vendored third-party code into the
  corpus the control lints, which is not what the control exists to prove and would
  make it slower and noisier for no reason.

The control's own cleanup uses an `EXIT` trap, not a `RETURN` trap: its failure branch
calls `exit 1` directly, which ends the process without ever returning from the
`negative_control` function, and a `RETURN` trap does not fire on that path (measured —
an earlier version of this script left a stray `.ruff-negctl-*` directory on disk
whenever the control itself failed). The trap references a variable set at file scope
(`tmp=""` near the top of the script), not a function-local one, because by the time an
`EXIT` trap actually runs — after the whole script's `case` dispatch has completed —
a `local` binding from inside the function is already out of scope, and referencing it
under `set -euo pipefail` would itself be an unbound-variable error.

## `ruff format` is deliberately not gated (spec AC B7)

This gate runs `ruff check` only. `ruff format --diff` was run over the ten tracked
files as part of the design work for this issue: **2,998 of roughly 15,600 existing
lines would be rewritten** (in-place lines rewritten, not the raw diff line count —
counting both `+` and `-` sides of the diff double-counts and had earlier been
misreported as ~5,000). Nearly a fifth of the corpus would change on a first run.

The reason is `line-length = 200` colliding with how these files are actually written:
they are roughly **60% comment by design**, and the wrapping in both the comments and
the hand-aligned fixture tables is a readability decision, not an accident that
formatting would merely tidy up. Gating `ruff format` would bury the substance of every
future `ci/` change under a mechanical rewrap diff.

**This is a decision not to gate formatting. It is not a claim that the corpus is
well formatted.** A contributor is free to run `ruff format` locally; nothing here
checks for it, and nothing here asserts the corpus is already formatted to any
particular style.

## Known limitation: isort's first-party classification (spec L9)

Ruff's `src` setting — which import-sorting uses to decide what counts as first-party —
defaults to the directory containing the config file, i.e. `py/`. A future `ci/`
Python file that imports a *sibling* `ci/` module (e.g. something in
`ci/affected-graph/` importing from `ci/pyo3-stub/`) would have that import classified
as third-party by `I001`, and ruff would enforce the wrong import ordering for it. As of
this gate's introduction, no tracked `ci/*.py` file imports another one, so the case is
latent rather than active. Fixing it properly would mean giving `ci/` its own `src`
entry (or its own Ruff section) without turning that into the second-configuration
problem this gate exists to avoid — left as a follow-up if and when a `ci/` file
actually needs a sibling import.
